//! Running queries and walking result pages.

use futures_util::StreamExt;
use std::io;

use super::*;

/// Bridges the core's row-at-a-time database stream to coarser UI messages, avoiding channel and
/// repaint overhead from dominating a fast local database.
struct UiQuerySink {
    tx: Sender<AppMessage>,
    tab_id: u64,
    seq: u64,
    append: bool,
    rows: Vec<Vec<dbcore::Value>>,
    sent_rows: usize,
}

impl UiQuerySink {
    fn new(tx: Sender<AppMessage>, tab_id: u64, seq: u64, append: bool) -> Self {
        Self {
            tx,
            tab_id,
            seq,
            append,
            rows: Vec::with_capacity(QUERY_STREAM_PAINT_ROWS),
            sent_rows: 0,
        }
    }

    fn send(&self, message: AppMessage) -> io::Result<()> {
        self.tx
            .send(message)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "UI closed"))
    }

    fn flush_rows(&mut self) -> io::Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let rows = std::mem::take(&mut self.rows);
        self.sent_rows += rows.len();
        self.send(AppMessage::QueryRows {
            tab_id: self.tab_id,
            rows,
            seq: self.seq,
        })
    }
}

impl dbcore::RowSink for UiQuerySink {
    fn begin(&mut self, columns: &[dbcore::ColumnMeta]) -> io::Result<()> {
        self.send(AppMessage::QueryStreamStarted {
            tab_id: self.tab_id,
            columns: columns.to_vec(),
            append: self.append,
            seq: self.seq,
        })
    }

    fn write_row(&mut self, row: &[dbcore::Value]) -> io::Result<()> {
        self.rows.push(row.to_vec());
        // Replacement rows stay hidden until completion. Continuations arrive before the user
        // reaches the loaded tail, so paint compact batches instead of exposing one early row.
        if self.rows.len() >= QUERY_STREAM_PAINT_ROWS {
            self.flush_rows()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        self.flush_rows()
    }
}

#[cfg(test)]
mod query_sink_tests {
    use super::*;

    #[test]
    fn continuation_rows_are_grouped_in_smooth_paint_batches() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut sink = UiQuerySink::new(tx, 7, 9, true);
        dbcore::RowSink::begin(&mut sink, &[]).unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            AppMessage::QueryStreamStarted { append: true, .. }
        ));

        for _ in 0..QUERY_STREAM_PAINT_ROWS - 1 {
            dbcore::RowSink::write_row(&mut sink, &[]).unwrap();
        }
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        dbcore::RowSink::write_row(&mut sink, &[]).unwrap();
        assert!(matches!(
            rx.recv().unwrap(),
            AppMessage::QueryRows { rows, .. } if rows.len() == QUERY_STREAM_PAINT_ROWS
        ));
    }
}

impl DbGuiApp {
    /// Hold a destructive production query, run only read-only preflight checks in the
    /// background, then let the dialog decide whether the exact snapshot may execute.
    pub(super) fn start_production_guard(
        &mut self,
        idx: usize,
        sql: String,
        statements: Vec<dbcore::safety::DangerousStatement>,
        continuation: ProductionGuardContinuation,
    ) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        let Some(conn_id) = tab.conn_id.clone() else {
            self.error = Some("This tab is not bound to a connection.".to_string());
            return;
        };
        let Some(active) = self
            .active_connections
            .iter()
            .find(|connection| connection.config_id == conn_id)
        else {
            self.error = Some("Production Guardian requires an active connection.".to_string());
            return;
        };
        let Some(config) = self.connections.iter().find(|config| config.id == conn_id) else {
            self.error = Some("The saved connection for this tab no longer exists.".to_string());
            return;
        };
        let database = if !active.schema.database_name.is_empty() {
            active.schema.database_name.clone()
        } else if config.kind == DbKind::Sqlite {
            config.sqlite_path.clone()
        } else {
            config.database.clone()
        };
        let connection_name = config.name.clone();
        let tab_id = tab.id;
        let db = active.db.clone();
        let tx = self.tx.clone();
        self.error = None;
        if let Some(previous) = self.danger_pending.take() {
            previous.preflight_cancel.cancel();
            if !self.record_guard_decision(&previous, "superseded") {
                return;
            }
        }
        let preflight_cancel = tokio_util::sync::CancellationToken::new();
        let pending = ProductionGuardPending {
            tab_id,
            conn_id: conn_id.clone(),
            connection_name,
            database,
            sql: sql.clone(),
            statements: statements.clone(),
            preflights: None,
            confirmation: String::new(),
            preflight_cancel: preflight_cancel.clone(),
            continuation,
        };
        if !self.record_guard_decision(&pending, "started") {
            return;
        }
        self.danger_pending = Some(pending);
        self.status_msg = "Production Guardian is analyzing the query…".to_string();
        self.rt.spawn(async move {
            // Bound concurrency: a large batch should not serialize every timeout, but it
            // also must not flood the production pool with COUNT/EXPLAIN requests.
            let work = futures_util::stream::iter(statements)
                .map(|statement| {
                    let db = db.clone();
                    async move { db.production_preflight(&statement).await }
                })
                .buffered(4)
                .collect::<Vec<_>>();
            let preflights = tokio::select! {
                _ = preflight_cancel.cancelled() => return,
                preflights = work => preflights,
            };
            let _ = tx.send(AppMessage::ProductionGuarded {
                tab_id,
                conn_id,
                sql,
                preflights,
            });
        });
    }

    /// Run the SQL of the tab at `idx` against its bound connection.
    pub(super) fn start_query_for(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        let sql = tab.sql.trim().to_string();
        if sql.is_empty() {
            return;
        }
        let result_limit = dbcore::parse_page_window(&sql).and_then(|window| window.limit);
        let fetch_sql = result_limit
            .filter(|limit| *limit > QUERY_STREAM_CHUNK_ROWS as u64)
            .and_then(|_| {
                let kind = tab.conn_id.as_deref().and_then(|id| {
                    self.active_connections
                        .iter()
                        .find(|connection| connection.config_id == id)
                        .map(|connection| connection.db.kind())
                })?;
                let window = dbcore::parse_page_window(&sql)?;
                dbcore::with_page_window(kind, &sql, QUERY_STREAM_CHUNK_ROWS as u64, window.offset)
            })
            .unwrap_or_else(|| sql.clone());
        self.start_query_sql(idx, fetch_sql, false, result_limit);
    }

    /// Start either a replacement query or a bounded continuation. A continuation deliberately
    /// runs rewritten SQL without changing the editor text: the visible SQL remains the initial
    /// window the user chose, while `result.rows` grows beneath it as they scroll.
    fn start_query_sql(
        &mut self,
        idx: usize,
        sql: String,
        append: bool,
        result_limit: Option<u64>,
    ) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        let tab_id = tab.id;
        let tab_conn_id = tab.conn_id.clone();
        let conn_id = tab_conn_id.clone().unwrap_or_default();
        // A new execution always returns to the primary result surface, so fresh data and
        // query errors cannot remain hidden behind Message or the Chart placeholder.
        self.tabs[idx].view = TabView::Data;
        let db = match tab_conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
        {
            Some(active) => active.db.clone(),
            None => {
                let message = "Not connected.".to_string();
                self.tabs[idx].query_error = Some(message.clone());
                self.error = None;
                self.status_msg = "Ready".to_string();
                return;
            }
        };
        let tx = self.tx.clone();
        // Supersede any run still in flight: cancel it and advance the generation stamp so
        // its (or any earlier run's) late result cannot clobber this run's result or state.
        // This point is only reached when the new run definitely starts — cancelling on an
        // earlier bail-out path would strand `busy` with no message left to reset it.
        if let Some(previous) = self.query_cancel.take() {
            previous.cancel();
        }
        self.query_seq += 1;
        let seq = self.query_seq;
        let page = dbcore::parse_page_window(&sql).filter(|window| {
            window
                .limit
                .is_some_and(|limit| limit <= MAX_FETCH_ROWS as u64)
        });
        let cancel = tokio_util::sync::CancellationToken::new();
        self.query_cancel = Some(cancel.clone());
        self.busy = Busy::Querying;
        self.querying_tab_id = Some(tab_id);
        self.tabs[idx].query_error = None;
        self.tabs[idx].stream = page.map(|_| QueryStreamUi {
            seq,
            append,
            columns: Vec::new(),
            pending_rows: Vec::new(),
            received_rows: 0,
        });
        if !append {
            self.tabs[idx].page_exhausted = false;
        }
        self.error = None;
        self.rt.spawn(async move {
            if let Some(page) = page {
                let started = Instant::now();
                let mut sink = UiQuerySink::new(tx.clone(), tab_id, seq, append);
                let res = db.export_query_cancellable(&sql, cancel, &mut sink).await;
                // Cancellation or a driver error can end the stream before `finish`; preserve
                // rows already decoded so useful partial progress is never thrown away.
                let _ = sink.flush_rows();
                let rows_loaded = sink.sent_rows as u64;
                let canceled = matches!(res, Err(dbcore::CoreError::Canceled));
                let result = res.map_err(|e| e.to_string());
                let _ = tx.send(AppMessage::QueryStreamFinished {
                    tab_id,
                    conn_id,
                    sql,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                    rows_loaded,
                    page,
                    result_limit: result_limit.unwrap_or_else(|| page.limit.unwrap_or(0)),
                    append,
                    result,
                    canceled,
                    seq,
                });
                return;
            }
            let res = db
                .execute_capped_cancellable(&sql, MAX_FETCH_ROWS, cancel)
                .await;
            // Distinguish a user cancel from a real failure before flattening to a string.
            let canceled = matches!(res, Err(dbcore::CoreError::Canceled));
            let result = res.map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Queried {
                tab_id,
                conn_id,
                sql,
                result,
                canceled,
                seq,
            });
        });
    }

    /// Fetch the next page when the virtualized grid approaches its loaded tail. The query is
    /// derived from the tab's original page SQL but its rows append to the current result.
    pub(super) fn load_more_rows(&mut self) {
        if self.busy != Busy::Idle {
            return;
        }
        let idx = self.active_query_tab;
        let tab = &self.tabs[idx];
        if !matches!(
            tab.kind,
            crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View
        ) || tab.page_exhausted
            || tab.result.is_none()
            || (tab.edits.source.is_none() && tab.edits.pending_source.is_none())
            || tab.sort.is_some()
        {
            return;
        }
        let Some(window) = dbcore::parse_page_window(&tab.sql) else {
            return;
        };
        let Some(limit) = window.limit.filter(|limit| *limit > 0) else {
            return;
        };
        let loaded = tab
            .result
            .as_ref()
            .map_or(0, |result| result.row_count() as u64);
        if loaded >= limit {
            self.tabs[idx].page_exhausted = true;
            return;
        }
        let offset = window.offset.saturating_add(loaded);
        let fetch_limit = (limit - loaded).min(QUERY_STREAM_CHUNK_ROWS as u64);
        let Some(kind) = self.active().map(|active| active.db.kind()) else {
            return;
        };
        let Some(sql) = dbcore::with_page_window(kind, &tab.sql, fetch_limit, offset) else {
            return;
        };
        self.start_query_sql(idx, sql, true, Some(limit));
    }
    /// Rewrite the active tab's paging window to `(limit, offset)` in its connection's
    /// dialect and re-run. No-op when the tab isn't a paged simple-table read.
    pub(super) fn run_page(&mut self, limit: u64, offset: u64) {
        let Some(kind) = self.active().map(|a| a.db.kind()) else {
            return;
        };
        let idx = self.active_query_tab;
        let Some(sql) = dbcore::with_page_window(kind, &self.tabs[idx].sql, limit, offset) else {
            return;
        };
        self.tabs[idx].sql = sql;
        // The rewrite preserves the simple-select shape, so the result stays editable.
        self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
        self.workspace_dirty = true;
        self.start_query_for(idx);
    }
    /// Pager navigation for the active (paged) table tab.
    pub(super) fn page_nav(&mut self, nav: PageNav) {
        if self.busy != Busy::Idle {
            return;
        }
        let tab = self.tab();
        let Some(win) = dbcore::parse_page_window(&tab.sql) else {
            return;
        };
        let Some(limit) = win.limit.filter(|&l| l > 0) else {
            return;
        };
        let offset = match nav {
            PageNav::Prev => win.offset.saturating_sub(limit),
            PageNav::Next => win.offset.saturating_add(limit),
        };
        if offset == win.offset {
            return;
        }
        self.run_page(limit, offset);
    }
    /// Apply the exact LIMIT/OFFSET window entered in the pager popover.
    pub(super) fn set_page_window(&mut self, limit: u64, offset: u64) {
        if self.busy != Busy::Idle || limit == 0 || limit > MAX_FETCH_ROWS as u64 {
            return;
        }
        let Some(win) = dbcore::parse_page_window(&self.tab().sql) else {
            return;
        };
        if win.limit == Some(limit) && win.offset == offset {
            return;
        }
        self.run_page(limit, offset);
    }
}
