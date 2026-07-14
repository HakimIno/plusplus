//! Tab lifecycle: opening, labelling, selecting, reordering and closing tabs.

use super::*;

impl DbGuiApp {
    pub(super) fn new_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        // Untitled (labelled by position in the bar); inherit the current tab's connection so
        // a new tab is ready to query the same db.
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = self.tab().conn_id.clone();
        self.tabs.push(tab);
        self.active_query_tab = self.tabs.len() - 1;
        self.status_msg = "New query tab".to_string();
        self.error = None;
        self.workspace_dirty = true;
    }
    /// Display label for the tab at `idx`: the table name for a table tab, otherwise its
    /// position ("Query 1", "Query 2", …) — so numbers stay small and reuse on close.
    pub(super) fn tab_label(&self, idx: usize) -> String {
        match self.tabs.get(idx) {
            Some(tab) if !tab.title.trim().is_empty() => tab.title.clone(),
            _ => format!("Query {}", idx + 1),
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
        // Reflect the newly-shown tab's last result in the status line.
        self.status_msg = match &self.tabs[idx].result {
            Some(res) => result_status(res),
            None => "Ready".to_string(),
        };
        self.error = None;
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
    /// Replace all tabs with one blank scratch tab (keeps the given connection binding).
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
        let conn_id = self.tab().conn_id.clone();
        self.reset_to_single_tab(conn_id);
        self.error = None;
        self.workspace_dirty = true;
    }
}
