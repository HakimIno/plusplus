//! Tab lifecycle: opening, labelling, selecting, reordering and closing tabs.

use super::*;

impl DbGuiApp {
    pub(super) fn install_split_tab(&mut self, mut split: QueryTab, run: bool) {
        self.close_split_workspace();
        if self.active_query_tab >= self.tabs.len() {
            return;
        }
        let primary_idx = self.active_query_tab;
        self.tabs[primary_idx].editor_split = true;
        self.tabs[primary_idx].split_sql = Some(split.sql.clone());
        self.tabs[primary_idx].editor_size = None;
        split.editor_size = None;
        split.preview = false;
        let split_idx = self.tabs.len();
        self.tabs.push(split);
        self.split_tab = Some(split_idx);
        self.split_focus = true;
        self.workspace_dirty = true;
        if run {
            self.start_query_for(split_idx);
        }
    }

    pub(super) fn open_split_workspace(&mut self) {
        if self.split_tab.is_some() || self.active_query_tab >= self.tabs.len() {
            return;
        }
        let primary_idx = self.active_query_tab;
        self.tabs[primary_idx].editor_split = true;
        self.tabs[primary_idx].split_sql = Some(self.tabs[primary_idx].sql.clone());
        self.tabs[primary_idx].editor_size = None;
        let primary = &self.tabs[primary_idx];
        let mut split = QueryTab::new(self.next_tab_id, primary.title.clone());
        self.next_tab_id = self.next_tab_id.wrapping_add(1);
        split.kind = primary.kind;
        split.conn_id = primary.conn_id.clone();
        split.sql = primary.sql.clone();
        split.editor_size = None;
        split.preview = false;
        self.split_tab = Some(self.tabs.len());
        self.tabs.push(split);
        self.workspace_dirty = true;
    }

    /// Remove the hidden tab that backs the right-hand split pane and always leave the
    /// top-level active-tab index pointing at a real, visible tab.
    pub(super) fn close_split_workspace(&mut self) {
        let Some(split_idx) = self.split_tab.take() else {
            return;
        };
        let primary_id = self
            .tabs
            .get(self.active_query_tab)
            .filter(|_| self.active_query_tab != split_idx)
            .map(|tab| tab.id)
            .or_else(|| {
                self.tabs
                    .iter()
                    .enumerate()
                    .find(|(idx, tab)| *idx != split_idx && tab.editor_split)
                    .map(|(_, tab)| tab.id)
            })
            .or_else(|| {
                self.tabs
                    .iter()
                    .enumerate()
                    .find(|(idx, _)| *idx != split_idx)
                    .map(|(_, tab)| tab.id)
            });

        if let Some(primary_id) = primary_id {
            if let Some(primary) = self.tabs.iter_mut().find(|tab| tab.id == primary_id) {
                primary.editor_split = false;
                primary.split_sql = None;
                primary.editor_pane = super::EditorPane::Primary;
            }
        }
        if split_idx < self.tabs.len() {
            self.tabs.remove(split_idx);
        }
        self.active_query_tab = primary_id
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            .unwrap_or(0)
            .min(self.tabs.len().saturating_sub(1));
        self.split_focus = false;
        self.workspace_dirty = true;
    }

    pub(super) fn new_tab(&mut self) {
        self.close_split_workspace();
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

    /// Land history SQL in a Query tab instead of overwriting a table/diagram/designer tab.
    ///
    /// `reuse_current_query` is for Insert into SQL Editor: a Query tab already in use is
    /// overwritten. Run only reuses a blank untitled query tab so an in-progress query is
    /// left alone.
    pub(super) fn present_sql_in_query_tab(&mut self, sql: String, reuse_current_query: bool) {
        self.settings_open = false;
        let can_reuse = self.tabs.get(self.active_query_tab).is_some_and(|tab| {
            tab.kind == crate::components::QueryTabKind::Query
                && tab.schema_editor.is_none()
                && tab.diagram.is_none()
                && (reuse_current_query
                    || (tab.title.is_empty() && tab.sql.trim().is_empty() && tab.result.is_none()))
        });
        if !can_reuse {
            self.new_tab();
        }
        let tab = self.tab_mut();
        tab.kind = crate::components::QueryTabKind::Query;
        tab.schema_editor = None;
        tab.diagram = None;
        tab.replace_sql(sql);
        tab.folds.clear();
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
                    Some(dbcore::DbKind::DuckDb) => "DuckDB ",
                    Some(dbcore::DbKind::Cassandra) => "Cassandra ",
                    Some(dbcore::DbKind::ScyllaDb) => "Scylla ",
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
        let Some(target_id) = self.tabs.get(idx).map(|tab| tab.id) else {
            return;
        };
        self.close_split_workspace();
        let Some(idx) = self.tabs.iter().position(|tab| tab.id == target_id) else {
            return;
        };
        self.active_query_tab = idx;
        self.touch_result(idx);
        // Query failures are rendered inside their result surface, not duplicated globally.
        if self.tabs[idx].query_error.is_some() {
            self.status_msg = "Ready".to_string();
            self.error = None;
        } else {
            self.status_msg = match &self.tabs[idx].result {
                Some(res) => result_status(res),
                None if self.tabs[idx].result_evicted => {
                    "Result released to stay within the memory budget — run the query to reload"
                        .to_string()
                }
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
        let Some(target_id) = self.tabs.get(idx).map(|tab| tab.id) else {
            return;
        };
        self.close_split_workspace();
        let Some(idx) = self.tabs.iter().position(|tab| tab.id == target_id) else {
            return;
        };
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
        let Some(kept_id) = self.tabs.get(keep_idx).map(|tab| tab.id) else {
            return;
        };
        self.close_split_workspace();
        if self.tabs.len() <= 1 || !self.tabs.iter().any(|tab| tab.id == kept_id) {
            return;
        }
        self.tabs.retain(|t| t.id == kept_id);
        self.active_query_tab = 0;
        self.error = None;
        self.status_msg = "Ready".to_string();
        self.workspace_dirty = true;
    }
    pub(super) fn close_tabs_to_right(&mut self, idx: usize) {
        let Some(target_id) = self.tabs.get(idx).map(|tab| tab.id) else {
            return;
        };
        self.close_split_workspace();
        let Some(idx) = self.tabs.iter().position(|tab| tab.id == target_id) else {
            return;
        };
        if idx + 1 >= self.tabs.len() {
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
        self.close_split_workspace();
        let conn_id = self
            .tabs
            .get(self.active_query_tab)
            .and_then(|tab| tab.conn_id.clone());
        self.reset_to_single_tab(conn_id);
        self.error = None;
        self.workspace_dirty = true;
    }
}
