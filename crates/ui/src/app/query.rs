//! Running queries and walking result pages.

use futures_util::StreamExt;

use super::*;

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
            self.error =
                Some("The saved connection for this tab no longer exists.".to_string());
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
        let tab_id = tab.id;
        let tab_conn_id = tab.conn_id.clone();
        let conn_id = tab_conn_id.clone().unwrap_or_default();
        let paged_table = (tab.edits.source.is_some() || tab.edits.pending_source.is_some())
            && dbcore::parse_page_window(&sql)
                .is_some_and(|window| window.limit.is_some_and(|limit| limit > 0));
        let count_sql = paged_table.then(|| dbcore::build_count_sql(&sql)).flatten();
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
        let cancel = tokio_util::sync::CancellationToken::new();
        self.query_cancel = Some(cancel.clone());
        self.busy = Busy::Querying;
        self.querying_tab_id = Some(tab_id);
        self.tabs[idx].query_error = None;
        self.tabs[idx].total_rows = None;
        self.error = None;
        self.status_msg = "Loading...".to_string();
        if let Some(count_sql) = count_sql {
            self.pending_page_counts.insert(tab_id);
            let count_db = db.clone();
            let count_tx = tx.clone();
            let count_cancel = cancel.clone();
            let count_query_sql = sql.clone();
            self.rt.spawn(async move {
                let total = count_db
                    .execute_capped_cancellable(&count_sql, 1, count_cancel)
                    .await
                    .ok()
                    .and_then(|result| match result.rows.first()?.first()? {
                        dbcore::Value::Int(value) => u64::try_from(*value).ok(),
                        dbcore::Value::Float(value) if *value >= 0.0 => Some(*value as u64),
                        dbcore::Value::Text(value) => value.parse().ok(),
                        _ => None,
                    });
                let _ = count_tx.send(AppMessage::PageCounted {
                    tab_id,
                    sql: count_query_sql,
                    total,
                    seq,
                });
            });
        } else {
            self.pending_page_counts.remove(&tab_id);
        }
        self.rt.spawn(async move {
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
        let known_total = self.tabs[idx].total_rows;
        self.tabs[idx].sql = sql;
        // The rewrite preserves the simple-select shape, so the result stays editable.
        self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
        self.workspace_dirty = true;
        self.start_query_for(idx);
        // Paging changes only LIMIT/OFFSET, so the previous total remains valid while the
        // fresh background count runs. This keeps Last-page navigation and the label stable.
        self.tabs[idx].total_rows = known_total;
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
        let total = tab.total_rows;
        let offset = match nav {
            PageNav::First => 0,
            PageNav::Prev => win.offset.saturating_sub(limit),
            PageNav::Next => win.offset + limit,
            PageNav::Last => match total {
                Some(t) if t > 0 => ((t - 1) / limit) * limit,
                _ => return,
            },
        };
        // Never run past a known end (the pager disables these buttons, but a stale
        // total or a keyboard repeat could still get here).
        if let Some(t) = total {
            if offset >= t && offset != 0 {
                return;
            }
        }
        if offset == win.offset {
            return;
        }
        self.run_page(limit, offset);
    }
    /// Change the active tab's page size, snapping the offset to the new page grid so
    /// the rows currently on screen stay within the shown page.
    pub(super) fn set_page_size(&mut self, size: u64) {
        if self.busy != Busy::Idle || size == 0 {
            return;
        }
        let Some(win) = dbcore::parse_page_window(&self.tab().sql) else {
            return;
        };
        if win.limit == Some(size) {
            return;
        }
        let offset = (win.offset / size) * size;
        self.run_page(size, offset);
    }
}
