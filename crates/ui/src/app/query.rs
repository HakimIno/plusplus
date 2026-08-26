//! Running queries and walking result pages.

use futures_util::StreamExt;
use std::io;

use super::*;

async fn fetch_query_total(
    db: std::sync::Arc<dyn dbcore::Database>,
    count_sql: String,
    cancel: tokio_util::sync::CancellationToken,
) -> Option<u64> {
    const COUNT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let query_cancel = cancel.clone();
    match tokio::time::timeout(
        COUNT_TIMEOUT,
        db.execute_capped_cancellable(&count_sql, 1, query_cancel),
    )
    .await
    {
        Ok(Ok(result)) => total_from_count_result(&result),
        Ok(Err(_)) => None,
        Err(_) => {
            cancel.cancel();
            None
        }
    }
}

/// Bridges the core's row-at-a-time database stream to coarser UI messages, avoiding channel and
/// repaint overhead from dominating a fast local database.
struct UiQuerySink {
    tx: Sender<AppMessage>,
    tab_id: u64,
    seq: u64,
    append: bool,
    rows: Vec<Vec<dbcore::Value>>,
    sent_rows: usize,
    accepted_bytes: usize,
    byte_limit: usize,
    budget_reached: bool,
    max_rows: usize,
    row_limit_reached: bool,
}

impl UiQuerySink {
    fn new(
        tx: Sender<AppMessage>,
        tab_id: u64,
        seq: u64,
        append: bool,
        byte_limit: usize,
        max_rows: usize,
    ) -> Self {
        Self {
            tx,
            tab_id,
            seq,
            append,
            rows: Vec::with_capacity(QUERY_STREAM_PAINT_ROWS),
            sent_rows: 0,
            accepted_bytes: 0,
            byte_limit,
            budget_reached: false,
            max_rows,
            row_limit_reached: false,
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
        if self.sent_rows.saturating_add(self.rows.len()) >= self.max_rows {
            self.row_limit_reached = true;
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "query result row limit reached",
            ));
        }
        let row_bytes = std::mem::size_of::<Vec<dbcore::Value>>()
            + std::mem::size_of_val(row)
            + row
                .iter()
                .map(|value| {
                    value
                        .estimated_memory_bytes()
                        .saturating_sub(std::mem::size_of::<dbcore::Value>())
                })
                .sum::<usize>();
        if self.accepted_bytes.saturating_add(row_bytes) > self.byte_limit {
            self.budget_reached = true;
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "query result memory budget reached",
            ));
        }
        self.accepted_bytes = self.accepted_bytes.saturating_add(row_bytes);
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

fn total_from_count_result(result: &QueryResult) -> Option<u64> {
    match result.rows.first()?.first()? {
        dbcore::Value::Int(value) => u64::try_from(*value).ok(),
        dbcore::Value::Text(value) => value.parse().ok(),
        _ => None,
    }
}

/// Resolve the last loaded primary-key tuple in catalog order. Missing columns or values that
/// cannot be rendered as portable SQL literals make the caller fall back to OFFSET pagination.
fn keyset_cursor(tab: &QueryTab) -> Option<(Vec<String>, Vec<dbcore::Value>)> {
    let source = tab
        .edits
        .source
        .as_ref()
        .or(tab.edits.pending_source.as_ref())?;
    if source.pk_cols.is_empty() {
        return None;
    }
    let result = tab.result.as_ref()?;
    let last = result.rows.last()?;
    let values = source
        .pk_cols
        .iter()
        .map(|key| {
            let index = result
                .columns
                .iter()
                .position(|column| column.name.eq_ignore_ascii_case(key))?;
            last.get(index).cloned()
        })
        .collect::<Option<Vec<_>>>()?;
    Some((source.pk_cols.clone(), values))
}

#[cfg(test)]
mod query_sink_tests {
    use super::*;

    #[test]
    fn continuation_rows_are_grouped_in_smooth_paint_batches() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut sink = UiQuerySink::new(tx, 7, 9, true, usize::MAX, usize::MAX);
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

    #[test]
    fn exact_total_decodes_from_backend_count_results() {
        let result = QueryResult {
            rows: vec![vec![dbcore::Value::Int(12_534)]],
            ..QueryResult::default()
        };
        assert_eq!(total_from_count_result(&result), Some(12_534));
    }

    #[test]
    fn stream_stops_before_crossing_its_byte_budget() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let row = vec![dbcore::Value::Text("x".repeat(1024))];
        let mut sink = UiQuerySink::new(tx, 1, 1, false, 128, usize::MAX);

        let error = dbcore::RowSink::write_row(&mut sink, &row).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
        assert!(sink.budget_reached);
        assert_eq!(sink.sent_rows, 0);
    }

    #[test]
    fn stream_stops_at_the_global_row_cap_without_buffering_one_more_row() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut sink = UiQuerySink::new(tx, 1, 1, false, usize::MAX, 1);
        dbcore::RowSink::write_row(&mut sink, &[dbcore::Value::Int(1)]).unwrap();

        let error = dbcore::RowSink::write_row(&mut sink, &[dbcore::Value::Int(2)]).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
        assert!(sink.row_limit_reached);
        assert_eq!(sink.rows.len(), 1);
    }
}

impl DbGuiApp {
    pub(super) fn resolved_sql_for(&mut self, idx: usize) -> Result<String, String> {
        let Some(tab) = self.tabs.get_mut(idx) else {
            return Err("Query tab is no longer open.".into());
        };
        tab.sync_query_parameters();
        self.resolved_sql_snapshot_for(idx)
    }

    pub(super) fn resolved_sql_snapshot_for(&self, idx: usize) -> Result<String, String> {
        let Some(tab) = self.tabs.get(idx) else {
            return Err("Query tab is no longer open.".into());
        };
        let source_sql = match tab.editor_pane {
            super::EditorPane::Primary => &tab.sql,
            super::EditorPane::Split => tab.split_sql.as_ref().unwrap_or(&tab.sql),
        };
        let kind = tab
            .conn_id
            .as_deref()
            .and_then(|id| {
                self.active_connections
                    .iter()
                    .find(|connection| connection.config_id == id)
                    .map(|connection| connection.db.kind())
                    .or_else(|| {
                        self.connections
                            .iter()
                            .find(|connection| connection.id == id)
                            .map(|connection| connection.kind)
                    })
            })
            .unwrap_or(dbcore::DbKind::Sqlite);
        let values = tab
            .query_parameters
            .iter()
            .map(|parameter| Ok((parameter.name.clone(), parameter.parse()?)))
            .collect::<Result<Vec<_>, String>>()?;
        dbcore::resolve_query_parameters(source_sql, kind, &values)
            .map_err(|error| error.to_string())
    }

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
        } else if matches!(config.kind, DbKind::Sqlite | DbKind::DuckDb) {
            if config.kind == DbKind::DuckDb {
                config.duckdb_path.clone()
            } else {
                config.sqlite_path.clone()
            }
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
        self.start_query_for_with_total(idx, true);
    }

    fn start_query_for_with_total(&mut self, idx: usize, refresh_total: bool) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        if tab.sql.trim().is_empty() {
            return;
        }
        let base_sql = match self.resolved_sql_for(idx) {
            Ok(sql) => sql.trim().to_string(),
            Err(message) => {
                self.tabs[idx].query_error = Some(message);
                self.tabs[idx].view = TabView::Data;
                self.status_msg = "Query parameters need attention.".into();
                return;
            }
        };
        let tab = &self.tabs[idx];
        let sql = tab
            .server_filter_predicate
            .as_deref()
            .and_then(|predicate| dbcore::with_where_predicate(&base_sql, predicate))
            .unwrap_or(base_sql);
        let requested_result_limit =
            dbcore::parse_page_window(&sql).and_then(|window| window.limit);
        // SQL typed directly into the editor bypasses the pager dialog's validation. Clamp it
        // here as well so lazy continuation can never accumulate beyond the process-wide cap.
        let result_limit = requested_result_limit.map(|limit| limit.min(MAX_FETCH_ROWS as u64));
        let fetch_sql = requested_result_limit
            .filter(|limit| *limit > QUERY_STREAM_CHUNK_ROWS as u64)
            .and_then(|_| {
                let kind = tab.conn_id.as_deref().and_then(|id| {
                    self.active_connections
                        .iter()
                        .find(|connection| connection.config_id == id)
                        .map(|connection| connection.db.kind())
                })?;
                let window = dbcore::parse_page_window(&sql)?;
                let key_columns = tab
                    .edits
                    .pending_source
                    .as_ref()
                    .or(tab.edits.source.as_ref())
                    .map(|source| source.pk_cols.as_slice())
                    .unwrap_or_default();
                dbcore::with_keyset_page(
                    kind,
                    &sql,
                    key_columns,
                    None,
                    QUERY_STREAM_CHUNK_ROWS as u64,
                )
                .or_else(|| {
                    dbcore::with_page_window(
                        kind,
                        &sql,
                        QUERY_STREAM_CHUNK_ROWS as u64,
                        window.offset,
                    )
                })
            })
            .unwrap_or_else(|| sql.clone());
        let count_sql = (refresh_total
            && requested_result_limit.is_some()
            && matches!(
                tab.kind,
                crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View
            ))
        .then(|| dbcore::build_count_sql(&sql))
        .flatten();
        if refresh_total {
            self.tabs[idx].total_rows = None;
            self.tabs[idx].total_rows_pending = count_sql.is_some();
        }
        self.start_query_sql(idx, fetch_sql, false, result_limit, count_sql);
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
        count_sql: Option<String>,
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
        let parsed_page = dbcore::parse_page_window(&sql).filter(|window| {
            window
                .limit
                .is_some_and(|limit| limit <= MAX_FETCH_ROWS as u64)
        });
        let page = parsed_page.or_else(|| {
            dbcore::query_returns_rows(&sql).then_some(dbcore::PageWindow {
                limit: Some(MAX_FETCH_ROWS as u64),
                offset: 0,
            })
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
        self.enforce_result_memory_budget();
        const MIN_REPLACEMENT_HEADROOM: usize = 1024 * 1024;
        let retained = self.total_result_memory_bytes();
        if !append
            && self.result_memory_budget.saturating_sub(retained) < MIN_REPLACEMENT_HEADROOM
            && !self.tabs[idx].edits.has_pending()
        {
            // A very large active result cannot be an eviction candidate. Release it only when
            // a replacement is definitely starting and it would otherwise leave no room for
            // even one useful stream batch. Normal replacements keep the old grid visible.
            self.tabs[idx].result = None;
            self.tabs[idx].row_order.clear();
            self.tabs[idx].row_order.shrink_to_fit();
            self.tabs[idx].selection.clear();
        }
        let query_byte_limit = self
            .result_memory_budget
            .saturating_sub(self.total_result_memory_bytes());
        // DuckDB serializes work on its embedded connection. Let its visible page finish first,
        // then count on the same task; networked backends can count beside the page fetch.
        let deferred_count_sql = if db.kind() == DbKind::DuckDb {
            count_sql
        } else if let Some(count_sql) = count_sql {
            let count_db = db.clone();
            let count_tx = tx.clone();
            let count_cancel = cancel.clone();
            self.rt.spawn(async move {
                let total = fetch_query_total(count_db, count_sql, count_cancel).await;
                let _ = count_tx.send(AppMessage::QueryTotal { tab_id, total, seq });
            });
            None
        } else {
            None
        };
        self.rt.spawn(async move {
            if let Some(page) = page {
                let started = Instant::now();
                let mut sink = UiQuerySink::new(
                    tx.clone(),
                    tab_id,
                    seq,
                    append,
                    query_byte_limit,
                    MAX_FETCH_ROWS,
                );
                let res = db
                    .export_query_cancellable(&sql, cancel.clone(), &mut sink)
                    .await;
                // Cancellation or a driver error can end the stream before `finish`; preserve
                // rows already decoded so useful partial progress is never thrown away.
                let _ = sink.flush_rows();
                let rows_loaded = sink.sent_rows as u64;
                let canceled = matches!(res, Err(dbcore::CoreError::Canceled));
                let budget_truncated = sink.budget_reached;
                let row_truncated = sink.row_limit_reached;
                let result = if budget_truncated || row_truncated {
                    Ok(rows_loaded)
                } else {
                    res.map_err(|e| e.to_string())
                };
                let page_succeeded = result.is_ok() && !canceled;
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
                    budget_truncated,
                    row_truncated,
                    seq,
                });
                if let Some(count_sql) = deferred_count_sql {
                    let total = if page_succeeded {
                        fetch_query_total(db, count_sql, cancel).await
                    } else {
                        None
                    };
                    let _ = tx.send(AppMessage::QueryTotal { tab_id, total, seq });
                }
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
        let Some(requested_limit) = window.limit.filter(|limit| *limit > 0) else {
            return;
        };
        let limit = requested_limit.min(MAX_FETCH_ROWS as u64);
        let loaded = tab
            .result
            .as_ref()
            .map_or(0, |result| result.row_count() as u64);
        if loaded >= limit {
            let tab = &mut self.tabs[idx];
            tab.page_exhausted = true;
            if requested_limit > MAX_FETCH_ROWS as u64 {
                if let Some(result) = &mut tab.result {
                    result.truncated = true;
                }
            }
            return;
        }
        let fetch_limit = (limit - loaded).min(QUERY_STREAM_CHUNK_ROWS as u64);
        let Some(kind) = self.active().map(|active| active.db.kind()) else {
            return;
        };
        let query_sql = tab
            .server_filter_predicate
            .as_deref()
            .and_then(|predicate| dbcore::with_where_predicate(&tab.sql, predicate))
            .unwrap_or_else(|| tab.sql.clone());
        let keyset_sql = keyset_cursor(tab).and_then(|(keys, values)| {
            dbcore::with_keyset_page(kind, &query_sql, &keys, Some(&values), fetch_limit)
        });
        let offset = window.offset.saturating_add(loaded);
        let Some(sql) =
            keyset_sql.or_else(|| dbcore::with_page_window(kind, &query_sql, fetch_limit, offset))
        else {
            return;
        };
        self.start_query_sql(idx, sql, true, Some(limit), None);
    }

    /// Apply the filter draft. Simple table/view reads run it on the database; unsupported
    /// result shapes retain the bounded in-memory fallback.
    pub(super) fn apply_result_filter(&mut self, idx: usize, clear: bool) {
        if clear {
            self.tabs[idx].filter.reset();
        }
        let Some(kind) = self.tabs[idx].conn_id.as_deref().and_then(|conn_id| {
            self.active_connections
                .iter()
                .find(|connection| connection.config_id == conn_id)
                .map(|connection| connection.db.kind())
        }) else {
            self.tabs[idx].server_filter_predicate = None;
            self.tabs[idx].recompute_view();
            return;
        };
        let server_capable = !kind.is_cql()
            && matches!(
                self.tabs[idx].kind,
                crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View
            )
            && dbcore::parse_page_window(&self.tabs[idx].sql).is_some();
        let predicate = if !clear && server_capable {
            self.tabs[idx].result.as_ref().and_then(|result| {
                filter::server_predicate(kind, &self.tabs[idx].filter, &result.columns)
            })
        } else {
            None
        };
        let had_server_filter = self.tabs[idx].server_filter_predicate.is_some();

        if server_capable && (predicate.is_some() || had_server_filter) {
            self.tabs[idx].server_filter_predicate = predicate;
            if let Some(window) = dbcore::parse_page_window(&self.tabs[idx].sql) {
                if let Some(limit) = window.limit {
                    if let Some(sql) = dbcore::with_page_window(kind, &self.tabs[idx].sql, limit, 0)
                    {
                        self.tabs[idx].replace_sql(sql);
                    }
                }
            }
            self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
            self.workspace_dirty = true;
            self.start_query_for(idx);
        } else {
            self.tabs[idx].server_filter_predicate = None;
            self.tabs[idx].recompute_view();
        }
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
        self.tabs[idx].replace_sql(sql);
        // The rewrite preserves the simple-select shape, so the result stays editable.
        self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
        self.workspace_dirty = true;
        let refresh_total = self.tabs[idx].total_rows.is_none();
        self.start_query_for_with_total(idx, refresh_total);
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
