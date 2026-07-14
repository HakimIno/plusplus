//! Persisted state: the restored workspace, settings, theme and favorites.

use super::*;

impl DbGuiApp {
    /// Replace the tabs with the saved workspace, if one exists. We never auto-connect or
    /// auto-run — tabs come back with their connection selected but idle.
    pub(super) fn restore_workspace(&mut self) {
        let saved = dbcore::config::load_workspace();
        let mut next_tab_id = 0u64;
        let tabs: Vec<QueryTab> = saved
            .tabs
            .into_iter()
            .map(|wt| {
                let id = next_tab_id;
                next_tab_id += 1;
                let source = wt.source.map(|s| EditSource {
                    schema: s.schema,
                    table: s.table,
                    pk_cols: s.pk_cols,
                });
                // The title is meaningful only for a table tab (the table name); untitled
                // query tabs are labelled by position in the bar, so we don't bake a number in.
                let title = source.as_ref().map(|s| s.table.clone()).unwrap_or_default();
                let mut tab = QueryTab::new(id, title);
                tab.sql = wt.sql;
                tab.conn_id = wt.conn_id;
                tab.edits.source = source;
                tab
            })
            .collect();
        if tabs.is_empty() {
            return; // no saved workspace → keep the default tab from `construct`
        }
        self.active_query_tab = saved.active_tab.min(tabs.len() - 1);
        self.next_tab_id = next_tab_id;
        self.tabs = tabs;
    }
    /// Snapshot the open tabs into the serialisable workspace (no result rows — only SQL,
    /// the bound connection, and the table source needed to re-open editable).
    pub(super) fn snapshot_workspace(&self) -> dbcore::config::Workspace {
        dbcore::config::Workspace {
            active_tab: self.active_query_tab,
            tabs: self
                .tabs
                .iter()
                .map(|t| dbcore::config::WorkspaceTab {
                    title: t.title.clone(),
                    conn_id: t.conn_id.clone(),
                    sql: t.sql.clone(),
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
        settings.beautify_uppercase = Some(self.beautify.uppercase);
        settings.beautify_indent = Some(self.beautify.indent);
        settings.welcomed = Some(!self.show_welcome);
        settings.history_enabled = Some(self.history_enabled);
        settings.audit_enabled = Some(self.audit_enabled);
        settings.update_check_enabled = Some(self.update_check_enabled);
        if let Err(e) = dbcore::config::save_settings(&settings) {
            self.error = Some(format!("Could not save settings: {e}"));
        }
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
                    created_at: dbcore::history::now_rfc3339(),
                });
                // Reveal the panel so the just-saved query is visible (e.g. when saving from
                // a history entry while the panel was closed).
                self.favorites_open = true;
                self.status_msg = "Saved to favorites".to_string();
            }
        }
        self.persist_favorites();
    }
    /// Mirror the in-memory favorites to disk. Best effort; skipped under test so unit tests
    /// never touch the user's favorites file.
    pub(super) fn persist_favorites(&mut self) {
        if cfg!(test) {
            return;
        }
        if let Err(e) = dbcore::favorites::save(&self.favorites_cache) {
            self.error = Some(format!("Could not save favorites: {e}"));
        }
    }
}
