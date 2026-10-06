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
            self.ensure_reload_source(idx);
            let view = self.tabs[idx].view;
            self.start_query_for(idx);
            // Reconnects reload table data automatically, but must not pull a tab the user
            // left on Structure/Indexes back to Data before its metadata recovery starts.
            if matches!(view, TabView::Structure | TabView::Indexes) {
                self.tabs[idx].view = view;
            }
        }
    }

    /// Most columns the workspace can be split into.
    pub(super) const MAX_PANES: usize = 4;

    pub(super) fn pane_count(&self) -> usize {
        self.split_panes.len() + 1
    }

    pub(super) fn is_split(&self) -> bool {
        !self.split_panes.is_empty()
    }

    /// The tab showing in `pane`: pane 0 is the main strip, 1.. are the split columns.
    pub(super) fn pane_active(&self, pane: usize) -> Option<usize> {
        if pane == 0 {
            Some(self.active_query_tab)
        } else {
            self.split_panes.get(pane - 1).copied()
        }
    }

    /// The tab that keyboard actions (run, close, new tab) apply to.
    pub(super) fn focused_tab_idx(&self) -> usize {
        self.pane_active(self.focused_pane)
            .unwrap_or(self.active_query_tab)
    }

    pub(super) fn tab_is_in_split_group(&self, idx: usize) -> bool {
        self.tabs.get(idx).is_some_and(|tab| tab.pane > 0)
    }

    pub(super) fn reset_split_ratios(&mut self) {
        let count = self.pane_count();
        self.split_ratios = vec![1.0 / count as f32; count];
    }

    /// Keep the primary tab's `split_sql` in step with pane 1. It is what a workspace saved by
    /// an older build restores a two-pane split from.
    pub(super) fn mirror_split_sql(&mut self) {
        if let Some(first) = self.split_panes.first().copied() {
            if let Some(sql) = self.tabs.get(first).map(|tab| tab.sql.clone()) {
                if let Some(primary) = self.tabs.get_mut(self.active_query_tab) {
                    primary.split_sql = Some(sql);
                }
            }
        }
    }

    /// Put `split` in column `pane`. `pane == pane_count()` opens a new column; once
    /// [`Self::MAX_PANES`] columns exist it lands in the last one instead.
    pub(super) fn install_split_tab(&mut self, mut split: QueryTab, pane: usize, run: bool) {
        if self.active_query_tab >= self.tabs.len() {
            return;
        }
        let mut target = pane.clamp(1, self.pane_count());
        if target == self.pane_count() && target >= Self::MAX_PANES {
            target -= 1;
        }
        if !self.is_split() {
            let primary_idx = self.active_query_tab;
            self.tabs[primary_idx].editor_split = true;
            self.tabs[primary_idx].editor_size = None;
        }
        split.editor_size = None;
        split.preview = false;
        split.pane = target;
        let split_idx = self.tabs.len();
        self.tabs.push(split);
        if target == self.pane_count() {
            self.split_panes.push(split_idx);
            self.reset_split_ratios();
        } else {
            self.split_panes[target - 1] = split_idx;
        }
        self.mirror_split_sql();
        self.focused_pane = target;
        self.workspace_dirty = true;
        if run {
            self.start_query_for(split_idx);
        }
    }

    /// Open the split with a fresh tab beside the current one. Users split by dragging a tab
    /// or table to the edge; this is how the tests build a split directly.
    #[cfg(test)]
    pub(super) fn open_split_workspace(&mut self) {
        if self.is_split() || self.active_query_tab >= self.tabs.len() {
            return;
        }
        let primary = &self.tabs[self.active_query_tab];
        let mut split = QueryTab::new(self.next_tab_id, primary.title.clone());
        self.next_tab_id = self.next_tab_id.wrapping_add(1);
        split.kind = primary.kind;
        split.conn_id = primary.conn_id.clone();
        split.sql = primary.sql.clone();
        let focus = self.focused_pane;
        self.install_split_tab(split, 1, false);
        self.focused_pane = focus;
    }

    /// Collapse the split, returning its tabs to the main strip without losing their work.
    pub(super) fn close_split_workspace(&mut self) {
        if !self.is_split() {
            return;
        }
        let primary_id = self
            .tabs
            .get(self.active_query_tab)
            .filter(|tab| tab.pane == 0)
            .map(|tab| tab.id)
            .or_else(|| {
                self.tabs
                    .iter()
                    .find(|tab| tab.pane == 0 && tab.editor_split)
                    .map(|tab| tab.id)
            })
            .or_else(|| self.tabs.iter().find(|tab| tab.pane == 0).map(|tab| tab.id));
        for tab in &mut self.tabs {
            tab.pane = 0;
        }
        self.split_panes.clear();
        self.split_ratios = vec![1.0];

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
        self.focused_pane = 0;
        self.workspace_dirty = true;
    }

    pub(super) fn select_split_pane_tab(&mut self, idx: usize, pane: usize) {
        if idx >= self.tabs.len() || self.tabs[idx].pane != pane {
            return;
        }
        if pane > 0 {
            let Some(slot) = self.split_panes.get_mut(pane - 1) else {
                return;
            };
            *slot = idx;
            self.focused_pane = pane;
            if pane == 1 {
                self.mirror_split_sql();
            }
        } else {
            self.active_query_tab = idx;
            self.focused_pane = 0;
        }
        self.touch_result(idx);
        self.reload_data_tab_if_needed(idx);
        self.workspace_dirty = true;
    }

    pub(super) fn close_split_pane_tab(&mut self, idx: usize, pane: usize) {
        if idx >= self.tabs.len() || self.tabs[idx].pane != pane {
            return;
        }
        // The main strip always keeps one tab; a split column disappears with its last tab.
        let closing_connection = self.tabs[idx].conn_id.clone();
        if pane == 0
            && self
                .tabs
                .iter()
                .filter(|tab| tab.pane == 0 && tab.conn_id == closing_connection)
                .count()
                <= 1
        {
            return;
        }
        let closing_id = self.tabs[idx].id;
        let primary_id = self.tabs.get(self.active_query_tab).map(|tab| tab.id);
        let mut active_ids: Vec<Option<u64>> = self
            .split_panes
            .iter()
            .map(|&split_idx| self.tabs.get(split_idx).map(|tab| tab.id))
            .collect();

        self.tabs.remove(idx);

        if pane > 0 {
            if !self.tabs.iter().any(|tab| tab.pane == pane) {
                for tab in &mut self.tabs {
                    if tab.pane > pane {
                        tab.pane -= 1;
                    }
                }
                active_ids.remove(pane - 1);
                self.split_ratios.remove(pane);
                let total: f32 = self.split_ratios.iter().sum();
                for ratio in &mut self.split_ratios {
                    *ratio /= total;
                }
            } else if active_ids[pane - 1] == Some(closing_id) {
                active_ids[pane - 1] = self
                    .tabs
                    .iter()
                    .rev()
                    .find(|tab| tab.pane == pane)
                    .map(|tab| tab.id);
            }
        }

        self.split_panes = active_ids
            .iter()
            .enumerate()
            .map(|(slot, id)| {
                id.and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
                    .or_else(|| self.tabs.iter().rposition(|tab| tab.pane == slot + 1))
                    .unwrap_or(0)
            })
            .collect();
        self.active_query_tab = primary_id
            .filter(|id| *id != closing_id)
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            .or_else(|| {
                self.tabs
                    .iter()
                    .position(|tab| tab.pane == 0 && tab.conn_id == closing_connection)
            })
            .unwrap_or(0)
            .min(self.tabs.len().saturating_sub(1));

        if self.split_panes.is_empty() {
            self.split_ratios = vec![1.0];
            if let Some(primary) = self.tabs.get_mut(self.active_query_tab) {
                primary.editor_split = false;
                primary.split_sql = None;
            }
            self.focused_pane = 0;
        } else {
            self.focused_pane = self.focused_pane.min(self.split_panes.len());
            self.mirror_split_sql();
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

    pub(super) fn new_tab_in_split_pane(&mut self, pane: usize) {
        if !self.is_split() {
            self.new_tab();
            return;
        }
        let source_idx = self.pane_active(pane).unwrap_or(self.active_query_tab);
        let id = self.next_tab_id;
        self.next_tab_id = self.next_tab_id.wrapping_add(1);
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = self
            .tabs
            .get(source_idx)
            .and_then(|tab| tab.conn_id.clone());
        if pane > 0 {
            self.install_split_tab(tab, pane, false);
        } else {
            self.tabs.push(tab);
            self.active_query_tab = self.tabs.len() - 1;
            self.focused_pane = 0;
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

    /// Close every table / view tab on the connection that was showing `dropped`, now that the
    /// object is gone. Tabs holding unsaved edits are closed too: their rows no longer exist.
    pub(super) fn close_tabs_of_dropped(&mut self, dropped: &PendingDrop) {
        use crate::components::QueryTabKind;
        let same_schema = |schema: &Option<String>| match (schema, &dropped.schema) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        };
        let ids: Vec<u64> = self
            .tabs
            .iter()
            .filter(|tab| {
                tab.conn_id.as_deref() == Some(dropped.conn_id.as_str())
                    && matches!(tab.kind, QueryTabKind::Table | QueryTabKind::View)
                    && tab.schema_editor.as_ref().is_none_or(|_| !tab.draft_tab)
                    && match tab
                        .edits
                        .source
                        .as_ref()
                        .or(tab.edits.pending_source.as_ref())
                    {
                        Some(source) => {
                            source.table.eq_ignore_ascii_case(&dropped.name)
                                && same_schema(&source.schema)
                        }
                        None => tab.title.eq_ignore_ascii_case(&dropped.name),
                    }
            })
            .map(|tab| tab.id)
            .collect();
        for id in ids {
            // Closing by id: indices shift as tabs go.
            if let Some(idx) = self.tabs.iter().position(|tab| tab.id == id) {
                self.tabs[idx].edits.clear();
                self.close_tab(idx);
            }
        }
    }

    /// Whether the active tab shows the SQL editor — a query, a function / procedure / trigger
    /// definition, or a draft written in it — so Cmd/Ctrl+F and +H belong to its find widget.
    pub(super) fn tab_has_sql_editor(&self) -> bool {
        use crate::components::QueryTabKind;
        let tab = self.tab();
        if tab.draft_tab {
            // A draft is a form; only some of them hold a SQL editor (a table designer doesn't).
            return self.draft_uses_sql_editor(self.active_query_tab);
        }
        matches!(
            tab.kind,
            QueryTabKind::Query
                | QueryTabKind::Function
                | QueryTabKind::Procedure
                | QueryTabKind::Trigger
        )
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
    /// Tabs belong to the connection they were opened on, and the tab bar shows one
    /// connection's tabs at a time: those bound to the same connection as the active tab.
    pub(super) fn tab_in_current_connection(&self, idx: usize) -> bool {
        match (self.tabs.get(idx), self.tabs.get(self.active_query_tab)) {
            (Some(tab), Some(active)) => tab.conn_id == active.conn_id,
            _ => false,
        }
    }

    /// A blank query tab bound to `conn_id`.
    fn blank_tab(&mut self, conn_id: Option<String>) -> QueryTab {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = conn_id;
        tab
    }

    pub(super) fn close_tab(&mut self, idx: usize) {
        let Some(target) = self.tabs.get(idx).map(|tab| (tab.id, tab.conn_id.clone())) else {
            return;
        };
        let (target_id, conn_id) = target;
        self.close_split_workspace();
        let Some(idx) = self.tabs.iter().position(|tab| tab.id == target_id) else {
            return;
        };
        let siblings: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| self.tabs[i].conn_id == conn_id)
            .collect();
        if siblings.len() == 1 {
            // The connection's last tab: leave it a blank one rather than an empty shell, and
            // never fall through to some other connection's tab.
            let blank = self.blank_tab(conn_id);
            self.tabs[idx] = blank;
            self.active_query_tab = idx;
        } else {
            let was_active = idx == self.active_query_tab;
            let active_id = self.tabs[self.active_query_tab].id;
            // Land on the neighbour in the same connection, preferring the one to the left.
            let at = siblings.iter().position(|&i| i == idx).unwrap_or(0);
            let neighbour = siblings[if at > 0 { at - 1 } else { at + 1 }];
            let neighbour_id = self.tabs[neighbour].id;
            self.tabs.remove(idx);
            let keep = if was_active { neighbour_id } else { active_id };
            self.active_query_tab = self.tabs.iter().position(|tab| tab.id == keep).unwrap_or(0);
        }
        self.error = None;
        self.workspace_dirty = true;
    }
    pub(super) fn close_other_tabs(&mut self, keep_idx: usize) {
        let Some((kept_id, conn_id)) = self
            .tabs
            .get(keep_idx)
            .map(|tab| (tab.id, tab.conn_id.clone()))
        else {
            return;
        };
        self.close_split_workspace();
        // Other connections' tabs are not "other tabs" of this one.
        if !self
            .tabs
            .iter()
            .any(|tab| tab.id != kept_id && tab.conn_id == conn_id)
        {
            return;
        }
        self.tabs
            .retain(|tab| tab.id == kept_id || tab.conn_id != conn_id);
        self.active_query_tab = self
            .tabs
            .iter()
            .position(|tab| tab.id == kept_id)
            .unwrap_or(0);
        self.error = None;
        self.status_msg = "Ready".to_string();
        self.workspace_dirty = true;
    }
    pub(super) fn close_tabs_to_right(&mut self, idx: usize) {
        let Some((target_id, conn_id)) =
            self.tabs.get(idx).map(|tab| (tab.id, tab.conn_id.clone()))
        else {
            return;
        };
        self.close_split_workspace();
        let Some(idx) = self.tabs.iter().position(|tab| tab.id == target_id) else {
            return;
        };
        let active_id = self.tabs[self.active_query_tab].id;
        let mut position = 0;
        self.tabs.retain(|tab| {
            position += 1;
            position <= idx + 1 || tab.conn_id != conn_id
        });
        let keep = if self.tabs.iter().any(|tab| tab.id == active_id) {
            active_id
        } else {
            target_id
        };
        self.active_query_tab = self.tabs.iter().position(|tab| tab.id == keep).unwrap_or(0);
        self.error = None;
        self.workspace_dirty = true;
    }
    /// Close every tab of the current connection, leaving it one blank query tab.
    pub(super) fn close_all_tabs(&mut self) {
        self.close_split_workspace();
        let conn_id = self
            .tabs
            .get(self.active_query_tab)
            .and_then(|tab| tab.conn_id.clone());
        self.tabs.retain(|tab| tab.conn_id != conn_id);
        let blank = self.blank_tab(conn_id);
        self.tabs.push(blank);
        self.active_query_tab = self.tabs.len() - 1;
        self.status_msg = "Ready".to_string();
        self.error = None;
        self.workspace_dirty = true;
    }
}
