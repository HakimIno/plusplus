//! Tab lifecycle: opening, labelling, selecting, reordering and closing tabs.

use super::*;

impl DbGuiApp {
    pub(super) fn new_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        // Untitled (labelled by position in the bar); inherit the current tab's connection so
        // a new tab is ready to query the same db. An empty workspace has no binding to inherit.
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = self
            .tabs
            .get(self.active_query_tab)
            .and_then(|tab| tab.conn_id.clone());
        self.tabs.push(tab);
        self.active_query_tab = self.tabs.len() - 1;
        self.status_msg = "New query tab".to_string();
        self.error = None;
        self.workspace_dirty = true;
    }
    /// Database provider bound to this tab, whether the connection is currently live or only
    /// present in the saved connection list.
    pub(super) fn tab_db_kind(&self, idx: usize) -> Option<dbcore::DbKind> {
        let conn_id = self.tabs.get(idx)?.conn_id.as_deref()?;
        self.active_connections
            .iter()
            .find(|conn| conn.config_id == conn_id)
            .map(|conn| conn.db.kind())
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|conn| conn.id == conn_id)
                    .map(|conn| conn.kind)
            })
    }
    /// Display label for the tab at `idx`: named object tabs keep their title; untitled query
    /// tabs identify the bound database provider and retain their compact positional number.
    pub(super) fn tab_label(&self, idx: usize) -> String {
        match self.tabs.get(idx) {
            Some(tab) if !tab.title.trim().is_empty() => tab.title.clone(),
            _ => {
                let provider = match self.tab_db_kind(idx) {
                    Some(dbcore::DbKind::Postgres) => "PG ",
                    Some(dbcore::DbKind::MySql) => "MySQL ",
                    Some(dbcore::DbKind::MariaDb) => "MariaDB ",
                    Some(dbcore::DbKind::SqlServer) => "MS ",
                    Some(dbcore::DbKind::Sqlite) => "SQLite ",
                    None => "",
                };
                format!("{provider}Query {}", idx + 1)
            }
        }
    }
    /// Icon kind for the tab strip, recorded when the tab is opened from the schema tree.
    pub(super) fn tab_kind(&self, idx: usize) -> crate::components::QueryTabKind {
        self.tabs
            .get(idx)
            .map_or(crate::components::QueryTabKind::Query, |tab| tab.kind)
    }
    pub(super) fn select_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.active_query_tab = idx;
        // Query failures are rendered inside their result surface, not duplicated globally.
        if self.tabs[idx].query_error.is_some() {
            self.status_msg = "Ready".to_string();
            self.error = None;
        } else {
            self.status_msg = match &self.tabs[idx].result {
                Some(res) => result_status(res),
                None => "Ready".to_string(),
            };
            self.error = None;
        }
        self.workspace_dirty = true;
    }
    /// Move the tab at `from` so it sits at position `to` (drag-to-reorder). The active
    /// tab stays the same logical tab — only its position changes.
    pub(super) fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let active_id = self.tab().id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(idx) = self.tabs.iter().position(|t| t.id == active_id) {
            self.active_query_tab = idx;
        }
        self.workspace_dirty = true;
    }
    /// Move a saved connection to a new slot and persist the list order.
    pub(super) fn move_connection(&mut self, from: usize, to: usize) {
        if from == to || from >= self.connections.len() || to >= self.connections.len() {
            return;
        }
        let conn = self.connections.remove(from);
        self.connections.insert(to, conn);
        if let Err(e) = dbcore::config::save_connections(&self.connections) {
            self.error = Some(e.to_string());
        }
    }
    pub(super) fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.reset_to_single_tab(self.tabs[0].conn_id.clone());
        } else {
            self.tabs.remove(idx);
            if self.active_query_tab > idx || self.active_query_tab >= self.tabs.len() {
                self.active_query_tab = self.active_query_tab.saturating_sub(1);
            }
        }
        self.error = None;
        self.workspace_dirty = true;
    }
    /// Keep one blank query tab so the workspace never renders as an empty shell.
    pub(super) fn reset_to_single_tab(&mut self, conn_id: Option<String>) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = conn_id;
        self.tabs = vec![tab];
        self.active_query_tab = 0;
        self.status_msg = "Ready".to_string();
    }
    pub(super) fn close_other_tabs(&mut self, keep_idx: usize) {
        if keep_idx >= self.tabs.len() || self.tabs.len() <= 1 {
            return;
        }
        let kept_id = self.tabs[keep_idx].id;
        self.tabs.retain(|t| t.id == kept_id);
        self.active_query_tab = 0;
        self.error = None;
        self.status_msg = "Ready".to_string();
        self.workspace_dirty = true;
    }
    pub(super) fn close_tabs_to_right(&mut self, idx: usize) {
        if idx >= self.tabs.len() || idx + 1 >= self.tabs.len() {
            return;
        }
        self.tabs.truncate(idx + 1);
        if self.active_query_tab > idx {
            self.active_query_tab = idx;
        }
        self.error = None;
        self.workspace_dirty = true;
    }
    pub(super) fn close_all_tabs(&mut self) {
        let conn_id = self
            .tabs
            .get(self.active_query_tab)
            .and_then(|tab| tab.conn_id.clone());
        self.reset_to_single_tab(conn_id);
        self.error = None;
        self.workspace_dirty = true;
    }
}
