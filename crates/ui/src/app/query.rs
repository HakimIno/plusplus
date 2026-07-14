//! Running queries and walking result pages.

use super::*;

impl DbGuiApp {
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
        let conn_id = tab.conn_id.clone().unwrap_or_default();
        let db = match tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
        {
            Some(active) => active.db.clone(),
            None => {
                self.error = Some("Not connected.".to_string());
                return;
            }
        };
        let tx = self.tx.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        self.query_cancel = Some(cancel.clone());
        self.busy = Busy::Querying;
        self.querying_tab_id = Some(tab_id);
        self.error = None;
        self.status_msg = "Loading...".to_string();
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
