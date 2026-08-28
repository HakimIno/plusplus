//! Persisted state: the restored workspace, settings, theme and favorites.

use super::*;

fn restore_tab_kind(kind: dbcore::config::WorkspaceTabKind) -> crate::components::QueryTabKind {
    use crate::components::QueryTabKind as Ui;
    use dbcore::config::WorkspaceTabKind as Saved;
    match kind {
        Saved::Query => Ui::Query,
        Saved::Table => Ui::Table,
        Saved::View => Ui::View,
        Saved::Function => Ui::Function,
        Saved::Procedure => Ui::Procedure,
        Saved::Trigger => Ui::Trigger,
    }
}

fn save_tab_kind(kind: crate::components::QueryTabKind) -> dbcore::config::WorkspaceTabKind {
    use crate::components::QueryTabKind as Ui;
    use dbcore::config::WorkspaceTabKind as Saved;
    match kind {
        Ui::Query => Saved::Query,
        Ui::Table => Saved::Table,
        Ui::View => Saved::View,
        Ui::Function => Saved::Function,
        Ui::Procedure => Saved::Procedure,
        Ui::Trigger => Saved::Trigger,
        // Never persisted — `snapshot_workspace` filters Diagram tabs out.
        Ui::Diagram => Saved::Query,
    }
}

pub(super) fn restored_tab_kind(
    kind: Option<dbcore::config::WorkspaceTabKind>,
    has_source: bool,
) -> crate::components::QueryTabKind {
    kind.map(restore_tab_kind).unwrap_or(if has_source {
        crate::components::QueryTabKind::Table
    } else {
        crate::components::QueryTabKind::Query
    })
}

impl DbGuiApp {
    /// Replace the default query tab with the saved workspace when tabs were persisted.
    /// We never auto-connect or auto-run — restored tabs come back with their connection
    /// selected but idle.
    pub(super) fn restore_workspace(&mut self) {
        let saved = dbcore::config::load_workspace();
        let mut next_tab_id = 0u64;
        let tabs: Vec<QueryTab> = saved
            .tabs
            .into_iter()
            .map(|wt| {
                let id = next_tab_id;
                next_tab_id += 1;
                let legacy_has_source = wt.source.is_some();
                let kind = restored_tab_kind(wt.kind, legacy_has_source);
                let source = wt.source.map(|s| EditSource {
                    schema: s.schema,
                    table: s.table,
                    pk_cols: s.pk_cols,
                });
                // Data tabs use the source relation as their title. Definition tabs have no
                // source but still need their persisted object name; plain queries stay empty
                // and are labelled by position in the tab bar.
                let title = source.as_ref().map(|s| s.table.clone()).unwrap_or(wt.title);
                let mut tab = QueryTab::new(id, title);
                tab.kind = kind;
                tab.sql = wt.sql;
                tab.editor_size = wt.editor_size;
                tab.editor_split = wt.editor_split;
                tab.editor_split_size = wt.editor_split_size;
                tab.split_sql = wt.split_sql;
                tab.editor_pane = super::EditorPane::Primary;
                tab.conn_id = wt.conn_id;
                tab.edits.source = source;
                tab
            })
            .collect();
        if tabs.is_empty() {
            return; // no saved tabs → keep the default query tab from `construct`
        }
        self.active_query_tab = saved.active_tab.min(tabs.len() - 1);
        self.next_tab_id = next_tab_id;
        self.tabs = tabs;
        // Rehydrate the hidden right-hand workspace pane for a persisted split. Only the
        // active tab can have an open split in the current UI, so keep restoration deterministic
        // even when older workspace files contain split metadata on several tabs.
        let primary_idx = self.active_query_tab;
        if self.tabs[primary_idx].editor_split {
            let (title, kind, conn_id, sql, revision, editor_size) = {
                let primary = &self.tabs[primary_idx];
                (
                    primary.title.clone(),
                    primary.kind,
                    primary.conn_id.clone(),
                    primary
                        .split_sql
                        .clone()
                        .unwrap_or_else(|| primary.sql.clone()),
                    primary.sql_revision,
                    primary.editor_split_size.or(primary.editor_size),
                )
            };
            let mut split = QueryTab::new(self.next_tab_id, title);
            self.next_tab_id = self.next_tab_id.wrapping_add(1);
            split.kind = kind;
            split.conn_id = conn_id;
            split.sql = sql;
            split.sql_revision = revision;
            split.editor_size = editor_size;
            split.preview = false;
            self.split_tab = Some(self.tabs.len());
            self.tabs.push(split);
        }
    }
    /// Snapshot the open tabs into the serialisable workspace (no result rows — only SQL,
    /// the bound connection, and the table source needed to re-open editable).
    /// Diagram tabs are skipped: their content is a schema snapshot that can't be
    /// rebuilt without a live connection, so they simply don't survive a restart.
    pub(super) fn snapshot_workspace(&self) -> dbcore::config::Workspace {
        // The right side of a split workspace is represented internally as a hidden tab so it
        // can own its SQL, result and connection state. It must not leak into the persisted tab
        // list as a second top-level workspace tab.
        let split_id = self
            .split_tab
            .and_then(|idx| self.tabs.get(idx))
            .map(|t| t.id);
        let saved = |t: &&QueryTab| {
            t.kind != crate::components::QueryTabKind::Diagram && Some(t.id) != split_id
        };
        dbcore::config::Workspace {
            // The saved index must count only the tabs that are actually saved.
            active_tab: self
                .tabs
                .iter()
                .take(self.active_query_tab)
                .filter(saved)
                .count(),
            tabs: self
                .tabs
                .iter()
                .filter(saved)
                .map(|t| dbcore::config::WorkspaceTab {
                    title: t.title.clone(),
                    conn_id: t.conn_id.clone(),
                    sql: t.sql.clone(),
                    kind: Some(save_tab_kind(t.kind)),
                    editor_size: t.editor_size,
                    editor_split: t.editor_split,
                    editor_split_size: t.editor_split_size,
                    split_sql: t.split_sql.clone(),
                    source: t
                        .edits
                        .source
                        .as_ref()
                        .map(|s| dbcore::config::WorkspaceSource {
                            schema: s.schema.clone(),
                            table: s.table.clone(),
                            pk_cols: s.pk_cols.clone(),
                        }),
                })
                .collect(),
        }
    }
    /// Flush the workspace to disk if it changed. Throttled so typing SQL doesn't write every
    /// frame; pass `force` to flush immediately (e.g. on a structural change).
    pub(super) fn maybe_save_workspace(&mut self, force: bool) {
        if !self.workspace_dirty {
            return;
        }
        if !force && self.last_workspace_save.elapsed() < std::time::Duration::from_millis(1500) {
            return;
        }
        if dbcore::config::save_workspace(&self.snapshot_workspace()).is_ok() {
            self.workspace_dirty = false;
            self.last_workspace_save = std::time::Instant::now();
        }
    }
    /// Flush all settings.json-backed preferences (theme, beautifier, welcomed) to disk.
    pub(super) fn persist_settings(&mut self) {
        let mut settings = dbcore::config::load_settings();
        settings.theme = Some(self.theme.clone());
        settings.ui_font = self.ui_font.clone();
        settings.code_font = self.code_font.clone();
        settings.beautify_uppercase = Some(self.beautify.uppercase);
        settings.beautify_indent = Some(self.beautify.indent);
        settings.run_all_by_default = Some(self.run_all_by_default);
        settings.welcomed = Some(!self.show_welcome);
        settings.history_enabled = Some(self.history_enabled);
        settings.audit_enabled = Some(self.audit_enabled);
        settings.update_check_enabled = Some(self.update_check_enabled);
        settings.result_memory_budget_mb = Some((self.result_memory_budget / (1024 * 1024)) as u32);
        settings.schema_table_order = self.schema_table_order.clone();
        if let Err(e) = dbcore::config::save_settings(&settings) {
            self.error = Some(format!("Could not save settings: {e}"));
        }
    }
    /// Rebuild egui's font families from the selected custom faces and embedded fallbacks.
    pub(super) fn apply_fonts(&self, ctx: &egui::Context) -> Result<(), String> {
        let Some(app_fonts) = self.app_fonts else {
            return Ok(());
        };
        crate::fonts::install(
            ctx,
            app_fonts,
            self.ui_font.as_deref(),
            self.code_font.as_deref(),
        )
    }
    /// Switch the active theme, re-apply the egui style, and persist the choice.
    pub(super) fn set_theme(&mut self, ctx: &egui::Context, key: String) {
        crate::theme::set_current(self.themes.theme_of(&key));
        self.theme = key;
        crate::style::apply(ctx);
        self.persist_settings();
    }
    /// Commit the favorite name dialog: rename an existing favorite or add a new one, then
    /// persist. An empty name falls back to a placeholder so the entry is never nameless.
    pub(super) fn confirm_save_favorite(&mut self) {
        let Some(draft) = self.favorite_pending.take() else {
            return;
        };
        let name = {
            let trimmed = draft.name.trim();
            if trimmed.is_empty() {
                "Untitled query".to_string()
            } else {
                trimmed.to_string()
            }
        };
        match draft.editing_id {
            Some(id) => {
                if let Some(fav) = self.favorites_cache.iter_mut().find(|f| f.id == id) {
                    fav.name = name;
                }
                self.status_msg = "Favorite renamed".to_string();
            }
            None => {
                self.favorites_cache.push(dbcore::Favorite {
                    id: dbcore::favorites::new_id(),
                    name,
                    sql: draft.sql,
                    conn_id: draft.conn_id,
                    conn_name: draft.conn_name,
                    folder: None,
                    created_at: dbcore::history::now_rfc3339(),
                });
                // Reveal the sidebar's Queries tab so the just-saved query is visible
                // (the cache is already current — no disk round-trip needed).
                self.sidebar_tab = SidebarTab::Queries;
                self.status_msg = "Saved to favorites".to_string();
            }
        }
        self.persist_favorites();
    }

    fn remember_favorite_folder(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty()
            || name.eq_ignore_ascii_case(dbcore::favorites::UNGROUPED)
            || self
                .favorite_folders
                .iter()
                .any(|folder| folder.eq_ignore_ascii_case(name))
        {
            return;
        }
        self.favorite_folders.push(name.to_string());
    }

    pub(super) fn move_favorite(&mut self, idx: usize, folder: Option<String>) {
        let Some(id) = self.favorites_cache.get(idx).map(|fav| fav.id.clone()) else {
            return;
        };
        self.drop_favorite_on_folder(&id, folder.as_deref());
    }

    pub(super) fn drop_favorite_on_folder(&mut self, id: &str, folder: Option<&str>) {
        if !self.favorites_cache.iter().any(|fav| fav.id == id) {
            return;
        }
        if let Some(name) = folder {
            self.remember_favorite_folder(name);
        }
        dbcore::favorites::move_query_to_folder(&mut self.favorites_cache, id, folder);
        self.persist_favorites();
        self.status_msg = "Query moved".to_string();
    }

    pub(super) fn drop_favorite_on_query(&mut self, source_id: &str, target_id: &str, after: bool) {
        if source_id == target_id {
            return;
        }
        dbcore::favorites::reorder_query(&mut self.favorites_cache, source_id, target_id, after);
        if let Some(folder) = self
            .favorites_cache
            .iter()
            .find(|fav| fav.id == source_id)
            .and_then(|fav| fav.folder.clone())
        {
            self.remember_favorite_folder(&folder);
        }
        self.persist_favorites();
        self.status_msg = "Query moved".to_string();
    }

    pub(super) fn reorder_favorite_folder(&mut self, source: &str, target: &str, after: bool) {
        dbcore::favorites::reorder_folder(&mut self.favorite_folders, source, target, after);
        self.persist_favorites();
        self.status_msg = "Folder moved".to_string();
    }

    pub(super) fn delete_favorite_folder(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case(dbcore::favorites::UNGROUPED) {
            return;
        }
        self.favorite_folders
            .retain(|folder| !folder.eq_ignore_ascii_case(name));
        for fav in &mut self.favorites_cache {
            if fav
                .folder
                .as_deref()
                .is_some_and(|folder| folder.eq_ignore_ascii_case(name))
            {
                fav.folder = None;
            }
        }
        self.persist_favorites();
        self.status_msg = format!("Folder \"{name}\" removed");
    }

    pub(super) fn confirm_favorite_folder(&mut self) {
        let Some(draft) = self.folder_pending.take() else {
            return;
        };
        let name = draft.name.trim().to_string();
        if name.is_empty() || name.eq_ignore_ascii_case(dbcore::favorites::UNGROUPED) {
            self.status_msg = "Folder needs a name".to_string();
            return;
        }
        if let Some(from) = draft.from {
            if from.eq_ignore_ascii_case(dbcore::favorites::UNGROUPED) {
                return;
            }
            if from.eq_ignore_ascii_case(&name) {
                return;
            }
            if self
                .favorite_folders
                .iter()
                .any(|folder| folder.eq_ignore_ascii_case(&name))
            {
                self.status_msg = format!("Folder \"{name}\" already exists");
                return;
            }
            let mut renamed = false;
            for folder in &mut self.favorite_folders {
                if folder.eq_ignore_ascii_case(&from) {
                    *folder = name.clone();
                    renamed = true;
                }
            }
            if !renamed {
                self.favorite_folders.push(name.clone());
            }
            for fav in &mut self.favorites_cache {
                if fav
                    .folder
                    .as_deref()
                    .is_some_and(|folder| folder.eq_ignore_ascii_case(&from))
                {
                    fav.folder = Some(name.clone());
                }
            }
            self.persist_favorites();
            self.status_msg = "Folder renamed".to_string();
            return;
        }
        self.remember_favorite_folder(&name);
        if let Some(id) = draft.move_id {
            if let Some(fav) = self.favorites_cache.iter_mut().find(|fav| fav.id == id) {
                fav.folder = Some(name.clone());
            }
        }
        self.persist_favorites();
        self.status_msg = format!("Folder \"{name}\" saved");
    }

    /// Mirror the in-memory favorites to disk. Best effort; skipped under test so unit tests
    /// never touch the user's favorites file.
    pub(super) fn persist_favorites(&mut self) {
        if cfg!(test) {
            return;
        }
        if let Err(e) = dbcore::favorites::save(&dbcore::favorites::FavoritesStore {
            folders: self.favorite_folders.clone(),
            queries: self.favorites_cache.clone(),
        }) {
            self.error = Some(format!("Could not save favorites: {e}"));
        }
    }
}
