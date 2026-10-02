//! Tab lifecycle: opening, labelling, selecting, reordering and closing tabs.

use super::*;

impl DbGuiApp {
    pub(super) fn reload_data_tab_if_needed(&mut self, idx: usize) {
        let should_reload = self.tabs.get(idx).is_some_and(|tab| {
            tab.result.is_none()
                && !tab.result_evicted
                && tab.stream.is_none()
                && !self.is_tab_querying(tab.id)
                && matches!(
                    tab.kind,
                    crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View
                )
                && tab.conn_id.as_deref().is_some_and(|conn_id| {
                    self.active_connections
                        .iter()
                        .any(|connection| connection.config_id == conn_id)
                })
        });
        if should_reload {
            if self.tabs[idx].edits.source.is_none()
                && self.tabs[idx].edits.pending_source.is_none()
            {
                self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
            }
            let view = self.tabs[idx].view;
            self.start_query_for(idx);
            // Reconnects reload table data automatically, but must not pull a tab the user
            // left on Structure/Indexes back to Data before its metadata recovery starts.
            if matches!(view, TabView::Structure | TabView::Indexes) {
                self.tabs[idx].view = view;
            }
        }
    }

    pub(super) fn tab_is_in_split_group(&self, idx: usize) -> bool {
        self.tabs
            .get(idx)
            .is_some_and(|tab| self.split_tab_ids.contains(&tab.id))
    }

    pub(super) fn install_split_tab(&mut self, mut split: QueryTab, run: bool) {
        if self.active_query_tab >= self.tabs.len() {
            return;
        }
        if self.split_tab.is_none() {
            let primary_idx = self.active_query_tab;
            self.tabs[primary_idx].editor_split = true;
            self.tabs[primary_idx].editor_size = None;
        }
        split.editor_size = None;
        split.preview = false;
        let split_id = split.id;
        let split_idx = self.tabs.len();
        self.tabs.push(split);
        self.split_tab_ids.push(split_id);
        self.split_tab = Some(split_idx);
        self.tabs[self.active_query_tab].split_sql = Some(self.tabs[split_idx].sql.clone());
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
        let split_id = split.id;
        self.split_tab = Some(self.tabs.len());
        self.tabs.push(split);
        self.split_tab_ids.push(split_id);
        self.workspace_dirty = true;
    }

    /// Collapse the split, returning its tabs to the main strip without losing their work.
    pub(super) fn close_split_workspace(&mut self) {
        let Some(split_idx) = self.split_tab.take() else {
            return;
        };
        let mut split_ids = std::mem::take(&mut self.split_tab_ids);
        if split_ids.is_empty() {
            if let Some(split_id) = self.tabs.get(split_idx).map(|tab| tab.id) {
                split_ids.push(split_id);
            }
        }
        let primary_id = self
            .tabs
            .get(self.active_query_tab)
            .filter(|tab| !split_ids.contains(&tab.id))
            .map(|tab| tab.id)
            .or_else(|| {
                self.tabs
                    .iter()
                    .find(|tab| !split_ids.contains(&tab.id) && tab.editor_split)
                    .map(|tab| tab.id)
            })
            .or_else(|| {
                self.tabs
                    .iter()
                    .find(|tab| !split_ids.contains(&tab.id))
                    .map(|tab| tab.id)
            });

        if let Some(primary_id) = primary_id {
            if let Some(primary) = self.tabs.iter_mut().find(|tab| tab.id == primary_id) {
                primary.editor_split = false;
                primary.split_sql = None;
                primary.editor_pane = super::EditorPane::Primary;
            }
        }
        self.active_query_tab = primary_id
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            .unwrap_or(0)
            .min(self.tabs.len().saturating_sub(1));
        self.split_focus = false;
        self.workspace_dirty = true;
    }

    pub(super) fn select_split_pane_tab(&mut self, idx: usize, right: bool) {
        if idx >= self.tabs.len() || self.tab_is_in_split_group(idx) != right {
            return;
        }
        if right {
            self.split_tab = Some(idx);
            self.split_focus = true;
            self.tabs[self.active_query_tab].split_sql = Some(self.tabs[idx].sql.clone());
        } else {
            self.active_query_tab = idx;
            self.split_focus = false;
        }
        self.touch_result(idx);
        self.reload_data_tab_if_needed(idx);
        self.workspace_dirty = true;
    }

    pub(super) fn close_split_pane_tab(&mut self, idx: usize, right: bool) {
        if idx >= self.tabs.len() || self.tab_is_in_split_group(idx) != right {
            return;
        }
        let closing_id = self.tabs[idx].id;
        let primary_id = self.tabs.get(self.active_query_tab).map(|tab| tab.id);
        let active_split_id = self
            .split_tab
            .and_then(|split_idx| self.tabs.get(split_idx))
            .map(|tab| tab.id);
        if right {
            self.split_tab_ids.retain(|id| *id != closing_id);
            self.tabs.remove(idx);
            self.active_query_tab = primary_id
                .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
                .unwrap_or(0)
                .min(self.tabs.len().saturating_sub(1));
            if self.split_tab_ids.is_empty() {
                self.split_tab = None;
                let primary_idx = self.active_query_tab.min(self.tabs.len().saturating_sub(1));
                if let Some(primary) = self.tabs.get_mut(primary_idx) {
                    primary.editor_split = false;
                    primary.split_sql = None;
                }
                self.active_query_tab =
                    self.active_query_tab.min(self.tabs.len().saturating_sub(1));
                self.split_focus = false;
            } else {
                let next_id = active_split_id
                    .filter(|id| *id != closing_id && self.split_tab_ids.contains(id))
                    .unwrap_or_else(|| *self.split_tab_ids.last().unwrap());
                self.split_tab = self.tabs.iter().position(|tab| tab.id == next_id);
            }
        } else {
            let left_count = self
                .tabs
                .iter()
                .filter(|tab| !self.split_tab_ids.contains(&tab.id))
                .count();
            if left_count <= 1 {
                return;
            }
            self.tabs.remove(idx);
            self.split_tab =
                active_split_id.and_then(|id| self.tabs.iter().position(|tab| tab.id == id));
            self.active_query_tab = primary_id
                .filter(|id| *id != closing_id)
                .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
                .or_else(|| {
                    self.tabs
                        .iter()
                        .position(|tab| !self.split_tab_ids.contains(&tab.id))
                })
                .unwrap_or(0);
        }
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

    pub(super) fn new_tab_in_split_pane(&mut self, right: bool) {
        if self.split_tab.is_none() {
            self.new_tab();
            return;
        }
        let source_idx = if right {
            self.split_tab.unwrap_or(self.active_query_tab)
        } else {
            self.active_query_tab
        };
        let id = self.next_tab_id;
        self.next_tab_id = self.next_tab_id.wrapping_add(1);
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = self
            .tabs
            .get(source_idx)
            .and_then(|tab| tab.conn_id.clone());
        if right {
            self.install_split_tab(tab, false);
        } else {
            self.tabs.push(tab);
            self.active_query_tab = self.tabs.len() - 1;
            self.split_focus = false;
            self.workspace_dirty = true;
        }
        self.status_msg = "New query tab".to_string();
        self.error = None;
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
        use crate::components::QueryTabKind;
        use crate::schema::ObjectEditor;
        let Some(tab) = self.tabs.get(idx) else {
            return QueryTabKind::Query;
        };
        // A draft tab is a plain editor tab underneath; the strip shows what it is drafting.
        if tab.draft_tab {
            return match tab.schema_editor.as_ref() {
                Some(ObjectEditor::Table(_)) => QueryTabKind::Table,
                Some(ObjectEditor::View(_)) => QueryTabKind::View,
                Some(ObjectEditor::Trigger(_)) => QueryTabKind::Trigger,
                Some(ObjectEditor::Routine(e)) if e.kind == dbcore::RoutineKind::Procedure => {
                    QueryTabKind::Procedure
                }
                Some(ObjectEditor::Routine(_)) => QueryTabKind::Function,
                None => tab.kind,
            };
        }
        tab.kind
    }

    /// Whether the active connection's driver can create the object kind `check` asks about.
    /// When it can't, say so (and why nothing opened) instead of failing later at Apply.
    pub(super) fn object_supported(&mut self, check: fn(DbKind) -> bool, what: &str) -> bool {
        let Some(kind) = self.active().map(|a| a.db.kind()) else {
            return true;
        };
        if check(kind) {
            return true;
        }
        self.error = Some(format!("{what} are not available for {}.", kind.label()));
        self.status_msg = "Not available for this driver".into();
        false
    }

    /// Lower-cased titles of the open draft tabs, so a new draft's `untitled_…` name is free.
    pub(super) fn open_draft_titles(&self) -> Vec<String> {
        self.tabs
            .iter()
            .filter(|t| t.draft_tab)
            .map(|t| t.title.to_lowercase())
            .collect()
    }

    /// Start drafting a new table / view / trigger / routine in a tab of its own, so it shows
    /// in the tab strip like any other object instead of taking over the tab the user was in.
    /// Every "New …" opens a tab of its own, so several drafts can be open side by side.
    pub(super) fn open_draft_tab(&mut self, editor: crate::schema::ObjectEditor) {
        self.new_tab();
        let tab = self.tab_mut();
        tab.draft_tab = true;
        // A new view's SELECT trigger's or routine's body is edited in the tab's own SQL editor.
        let body = match &editor {
            crate::schema::ObjectEditor::View(view) => Some(&view.select_body),
            crate::schema::ObjectEditor::Trigger(trigger) => Some(&trigger.body),
            crate::schema::ObjectEditor::Routine(routine) => Some(&routine.body),
            _ => None,
        };
        if let Some(body) = body {
            tab.sql.clone_from(body);
            tab.mark_sql_changed();
        }
        tab.schema_editor = Some(editor);
        self.sync_draft_title(self.active_query_tab);
        self.schema_pending = None;
        self.status_msg = if cfg!(target_os = "macos") {
            "⌘S to apply · Esc to cancel".into()
        } else {
            "Ctrl+S to apply · Esc to cancel".into()
        };
    }

    /// Whether the active tab shows the SQL editor — a query, a function / procedure / trigger
    /// definition, or a draft written in it — so Cmd/Ctrl+F and +H belong to its find widget.
    pub(super) fn tab_has_sql_editor(&self) -> bool {
        use crate::components::QueryTabKind;
        matches!(
            self.tab().kind,
            QueryTabKind::Query
                | QueryTabKind::Function
                | QueryTabKind::Procedure
                | QueryTabKind::Trigger
        ) || self.draft_uses_sql_editor(self.active_query_tab)
    }

    /// Whether the tab at `idx` is a New View / Trigger / Routine draft, whose body is written
    /// in the full SQL editor.
    pub(super) fn draft_uses_sql_editor(&self, idx: usize) -> bool {
        use crate::schema::{ObjectEditor, ObjectMode};
        self.tabs.get(idx).is_some_and(|tab| {
            tab.draft_tab
                && match tab.schema_editor.as_ref() {
                    Some(ObjectEditor::View(e)) => e.mode == ObjectMode::Create,
                    Some(ObjectEditor::Trigger(e)) => e.mode == ObjectMode::Create,
                    Some(ObjectEditor::Routine(e)) => e.mode == ObjectMode::Create,
                    _ => false,
                }
        })
    }

    /// What a draft tab just created, as the action that opens it — a table or view opens to its
    /// rows. `None` for a trigger or routine (nothing to browse) or when `tab_id` isn't a draft.
    pub(super) fn draft_created_object(&self, tab_id: u64) -> Option<Action> {
        use crate::schema::{ObjectEditor, ObjectMode, SchemaEditorMode};
        let tab = self.tabs.iter().find(|t| t.id == tab_id && t.draft_tab)?;
        let kind = tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
            .map(|c| c.db.kind())?;
        let schema_of = |name: &str| (!name.trim().is_empty()).then(|| name.trim().to_string());
        match tab.schema_editor.as_ref()? {
            ObjectEditor::Table(e) if e.mode == SchemaEditorMode::New => {
                let table = dbcore::TableInfo {
                    schema: schema_of(&e.schema_name),
                    name: e.table_name.trim().to_string(),
                    columns: Vec::new(),
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                };
                Some(Action::OpenTable {
                    sql: kind.preview_query(&table.qualified(kind), 100),
                    source: EditSource {
                        schema: table.schema.clone(),
                        table: table.name.clone(),
                        pk_cols: e
                            .columns
                            .iter()
                            .filter(|c| c.primary_key && !c.name.trim().is_empty())
                            .map(|c| c.name.trim().to_string())
                            .collect(),
                    },
                    pin: true,
                    kind: crate::components::QueryTabKind::Table,
                })
            }
            ObjectEditor::View(e) if e.mode == ObjectMode::Create && !e.materialized => {
                let view = dbcore::ViewInfo {
                    schema: schema_of(&e.schema_name),
                    name: e.name.trim().to_string(),
                    columns: Vec::new(),
                    definition: String::new(),
                    materialized: false,
                };
                Some(Action::OpenTable {
                    sql: kind.preview_query(&view.qualified(kind), 100),
                    source: EditSource {
                        schema: view.schema.clone(),
                        table: view.name.clone(),
                        pk_cols: Vec::new(),
                    },
                    pin: true,
                    kind: crate::components::QueryTabKind::View,
                })
            }
            _ => None,
        }
    }

    /// Keep a draft tab's title in step with the name being typed (a placeholder while empty).
    pub(super) fn sync_draft_title(&mut self, idx: usize) {
        use crate::schema::{ObjectEditor, ObjectMode, SchemaEditorMode};
        let Some(tab) = self.tabs.get_mut(idx).filter(|tab| tab.draft_tab) else {
            return;
        };
        let (name, placeholder): (&str, &str) = match tab.schema_editor.as_ref() {
            Some(ObjectEditor::Table(e)) if e.mode == SchemaEditorMode::New => {
                (&e.table_name, "untitled_table")
            }
            Some(ObjectEditor::View(e)) if e.mode == ObjectMode::Create => {
                (&e.name, "untitled_view")
            }
            Some(ObjectEditor::Trigger(e)) if e.mode == ObjectMode::Create => {
                (&e.name, "untitled_trigger")
            }
            Some(ObjectEditor::Routine(e)) if e.mode == ObjectMode::Create => {
                (&e.name, "untitled_routine")
            }
            _ => return,
        };
        let title = if name.trim().is_empty() {
            placeholder.to_string()
        } else {
            name.trim().to_string()
        };
        if tab.title != title {
            tab.title = title;
        }
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
        self.reload_data_tab_if_needed(idx);
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
