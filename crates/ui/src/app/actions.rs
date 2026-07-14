//! The `Action` dispatch table: every user intent the panels raise, applied to state.

use super::*;

impl DbGuiApp {
    pub(super) fn apply_action(&mut self, action: Action) {
        match action {
            Action::Connect(i) => self.bind_connection(i, true),
            Action::BindConnection(i) => self.bind_connection(i, false),
            Action::Disconnect => {
                if let Some(id) = self.tab().conn_id.clone() {
                    self.disconnect_conn(&id);
                }
            }
            Action::DisconnectConn(i) => {
                if let Some(id) = self.connections.get(i).map(|c| c.id.clone()) {
                    self.disconnect_conn(&id);
                }
            }
            Action::NewTab => self.new_tab(),
            Action::SelectTab(i) => self.select_tab(i),
            Action::CloseTab(i) => self.close_tab(i),
            Action::CloseOtherTabs(i) => self.close_other_tabs(i),
            Action::CloseTabsToRight(i) => self.close_tabs_to_right(i),
            Action::CloseAllTabs => self.close_all_tabs(),
            Action::PinTab(i) => {
                if let Some(tab) = self.tabs.get_mut(i) {
                    tab.preview = false;
                }
                self.select_tab(i);
            }
            Action::MoveTab { from, to } => self.move_tab(from, to),
            Action::MoveConnection { from, to } => self.move_connection(from, to),
            Action::NewConnection => {
                self.editor = Some(ConnEditor {
                    config: ConnectionConfig::new(DbKind::Postgres),
                    password: String::new(),
                    ssh_password: String::new(),
                    is_new: true,
                    edit_index: None,
                    test_state: ConnTestState::Untested,
                });
            }
            Action::EditConnection(i) => {
                if let Some(cfg) = self.connections.get(i).cloned() {
                    let password = dbcore::secrets::get_password(&cfg.id)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let ssh_password = dbcore::secrets::get_ssh_secret(&cfg.id)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    self.editor = Some(ConnEditor {
                        config: cfg,
                        password,
                        ssh_password,
                        is_new: false,
                        edit_index: Some(i),
                        test_state: ConnTestState::Untested,
                    });
                }
            }
            Action::DeleteConnection(i) => {
                if i < self.connections.len() {
                    let cfg = self.connections.remove(i);
                    self.connection_jobs.remove(&cfg.id);
                    self.schema_cache.remove(&cfg.id);
                    self.connection_timings.remove(&cfg.id);
                    let _ = dbcore::secrets::delete_password(&cfg.id);
                    let _ = dbcore::secrets::delete_ssh_secret(&cfg.id);
                    if let Err(e) = dbcore::config::save_connections(&self.connections) {
                        self.error = Some(e.to_string());
                    }
                    self.active_connections
                        .retain(|conn| conn.config_id != cfg.id);
                    // Any tab bound to the deleted connection becomes unbound.
                    for tab in &mut self.tabs {
                        if tab.conn_id.as_deref() == Some(cfg.id.as_str()) {
                            tab.conn_id = None;
                        }
                    }
                    self.workspace_dirty = true;
                }
            }
            Action::SwitchDatabase { conn_idx, database } => {
                let switching_id = self.connections.get(conn_idx).map(|cfg| cfg.id.clone());
                if switching_id
                    .as_ref()
                    .is_some_and(|id| self.connection_jobs.contains(id))
                {
                    self.status_msg =
                        "Wait for the current connection load before switching databases"
                            .to_string();
                    return;
                }
                if let Some(id) = switching_id {
                    self.schema_cache.remove(&id);
                }
                if let Some(cfg) = self.connections.get_mut(conn_idx) {
                    cfg.database = database;
                    if let Err(e) = dbcore::config::save_connections(&self.connections) {
                        self.error = Some(e.to_string());
                    }
                }
                self.bind_connection(conn_idx, true);
            }
            Action::TestConnection => self.start_connection_test(),
            Action::SaveConnection => self.save_connection(),
            Action::CancelDialog => self.editor = None,
            Action::OpenSettings => self.settings_open = true,
            Action::CloseSettings => self.settings_open = false,
            Action::ToggleHistory => {
                if self.history_open {
                    self.history_open = false;
                    self.history_cache = Vec::new();
                } else {
                    self.history_cache =
                        dbcore::history::load(dbcore::history::MAX_ENTRIES).unwrap_or_default();
                    self.history_open = true;
                }
            }
            Action::ToggleFavoritesPanel => {
                self.favorites_open = !self.favorites_open;
                // Re-read on open so the list reflects any out-of-band change.
                if self.favorites_open {
                    self.favorites_cache = dbcore::favorites::load().unwrap_or_default();
                }
            }
            Action::SaveCurrentAsFavorite => {
                let sql = self.tab().sql.trim().to_string();
                if sql.is_empty() {
                    self.error = Some("Nothing to save — the editor is empty.".into());
                } else {
                    let (conn_id, conn_name) = self.active_conn_id_name();
                    self.favorite_pending = Some(FavoriteDraft {
                        name: default_favorite_name(&sql),
                        sql,
                        conn_id,
                        conn_name,
                        editing_id: None,
                    });
                }
            }
            Action::SaveFavoriteFromHistory(i) => {
                if let Some(entry) = self.history_cache.get(i) {
                    self.favorite_pending = Some(FavoriteDraft {
                        name: default_favorite_name(&entry.sql),
                        sql: entry.sql.clone(),
                        conn_id: Some(entry.conn_id.clone()),
                        conn_name: Some(entry.conn_name.clone()),
                        editing_id: None,
                    });
                }
            }
            Action::RenameFavorite(i) => {
                if let Some(fav) = self.favorites_cache.get(i) {
                    self.favorite_pending = Some(FavoriteDraft {
                        name: fav.name.clone(),
                        sql: fav.sql.clone(),
                        conn_id: fav.conn_id.clone(),
                        conn_name: fav.conn_name.clone(),
                        editing_id: Some(fav.id.clone()),
                    });
                }
            }
            Action::ConfirmSaveFavorite => self.confirm_save_favorite(),
            Action::CancelSaveFavorite => self.favorite_pending = None,
            Action::UseFavorite(i) => {
                if let Some(fav) = self.favorites_cache.get(i) {
                    self.tab_mut().sql = fav.sql.clone();
                    self.workspace_dirty = true;
                }
            }
            Action::DeleteFavorite(i) => {
                if i < self.favorites_cache.len() {
                    self.favorites_cache.remove(i);
                    self.persist_favorites();
                    self.status_msg = "Favorite deleted".to_string();
                }
            }
            Action::ToggleErd => {
                if self.erd.is_some() {
                    self.erd = None;
                } else if let Some(active) = self.active() {
                    self.erd = Some(crate::erd::ErDiagram::build(
                        &active.config_id,
                        &active.schema,
                    ));
                } else {
                    self.error = Some("Connect to a database to view its ER diagram.".into());
                }
            }
            Action::RefreshErd => self.refresh_erd(),
            Action::ClearHistory => {
                if let Err(e) = dbcore::history::clear() {
                    self.error = Some(format!("Could not clear history: {e}"));
                } else {
                    self.history_cache.clear();
                    self.status_msg = "Query history cleared".to_string();
                }
            }
            // The panel stays open: picking entries to compare or replay in sequence is
            // the whole point of a sidebar.
            Action::UseHistorySql(i) => {
                if let Some(entry) = self.history_cache.get(i) {
                    let sql = entry.sql.clone();
                    self.tab_mut().sql = sql;
                    self.workspace_dirty = true;
                }
            }
            Action::DismissWelcome => {
                self.show_welcome = false;
                self.persist_settings();
            }
            Action::BrowseSqlitePath => {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    if let Some(ed) = &mut self.editor {
                        ed.config.sqlite_path = path.to_string_lossy().into_owned();
                        ed.test_state = ConnTestState::Untested;
                    }
                }
            }
            Action::BrowseSslCaCert => {
                self.browse_pem_into(&["pem", "crt", "cer"], |cfg| &mut cfg.ssl_ca_cert)
            }
            Action::BrowseSslClientCert => {
                self.browse_pem_into(&["pem", "crt", "cer"], |cfg| &mut cfg.ssl_client_cert)
            }
            Action::BrowseSslClientKey => {
                self.browse_pem_into(&["pem", "key"], |cfg| &mut cfg.ssl_client_key)
            }
            // No extension filter: SSH keys (id_ed25519, id_rsa, ...) usually have none.
            Action::BrowseSshKey => {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    if let Some(ed) = &mut self.editor {
                        ed.config.ssh_key_path = path.to_string_lossy().into_owned();
                        ed.test_state = ConnTestState::Untested;
                    }
                }
            }
            Action::RunQuery => {
                let idx = self.active_query_tab;
                // Editability is re-derived from the SQL itself on every run: any simple
                // single-table `SELECT *` — including a hand-tuned LIMIT/WHERE/ORDER BY —
                // stays editable; anything else runs as a read-only ad-hoc query.
                self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
                // A read-only connection refuses anything that isn't provably a read —
                // no confirmation dialog, it simply doesn't run. The backends enforce
                // this at the session level too where the engine supports it; this check
                // gives the clear, local error.
                if self.tab_connection_is_read_only(idx) {
                    let found = dbcore::safety::write_statements(&self.tabs[idx].sql);
                    if let Some(first) = found.first() {
                        let shown: String = first.chars().take(80).collect();
                        self.refuse_read_only(&format!(
                            "not running: {shown}{}",
                            if found.len() > 1 {
                                format!(" (+{} more)", found.len() - 1)
                            } else {
                                String::new()
                            }
                        ));
                        return;
                    }
                }
                // A production connection holds destructive SQL for confirmation first.
                if self.tab_connection_is_production(idx) {
                    let found = dbcore::safety::dangerous_statements(&self.tabs[idx].sql);
                    if !found.is_empty() {
                        self.danger_pending = Some(found);
                        return;
                    }
                }
                self.start_query_for(idx);
            }
            Action::CancelQuery => {
                if let Some(cancel) = self.query_cancel.take() {
                    cancel.cancel();
                    self.status_msg = "Cancelling…".to_string();
                }
            }
            Action::ConfirmDangerQuery => {
                if self.danger_pending.take().is_some() {
                    self.start_query_for(self.active_query_tab);
                }
            }
            Action::CancelDangerQuery => self.danger_pending = None,
            Action::BeautifySql => self.beautify_sql(),
            Action::OpenTable {
                sql,
                source,
                pin,
                kind,
            } => self.open_table(sql, source, pin, kind),
            Action::OpenDefinition { title, sql, kind } => self.open_definition(title, sql, kind),
            Action::FollowForeignKey { row, col } => self.follow_foreign_key(row, col),
            Action::SortBy(col) => self.tab_mut().apply_sort(col),
            Action::SetSort { col, asc } => self.tab_mut().set_sort(col, asc),
            Action::ClearSort => self.tab_mut().clear_sort(),
            Action::Page(nav) => self.page_nav(nav),
            Action::SetPageSize(n) => self.set_page_size(n),
            Action::CopyRows(format) => self.copy_selection(format),
            Action::PasteRows(text) => self.paste_rows(&text),
            Action::ExportTable { table, format } => self.export_table(&table, format),
            Action::ImportIntoTable(table) => self.open_import(&table),
            Action::SetImportMapping { target, source } => {
                if let Some(draft) = self.import_pending.as_mut() {
                    if let Some(slot) = draft.mapping.get_mut(target) {
                        *slot = source;
                    }
                }
            }
            Action::AutoMapImport => {
                if let Some(draft) = self.import_pending.as_mut() {
                    draft.auto_map();
                }
            }
            Action::ClearImportMapping => {
                if let Some(draft) = self.import_pending.as_mut() {
                    draft.mapping.iter_mut().for_each(|m| *m = None);
                }
            }
            Action::SetImportHasHeader(on) => {
                if let Some(draft) = self.import_pending.as_mut() {
                    draft.has_header = on;
                }
                self.reload_import_preview();
            }
            Action::ConfirmImport => self.confirm_import(),
            Action::CancelImport => self.import_pending = None,
            Action::PreviewEdits => self.commit_edits(),
            Action::Undo => self.undo_edits(),
            Action::Redo => self.redo_edits(),
            Action::ConfirmEdits => self.confirm_edits(),
            Action::CancelEdits => {
                self.commit_pending = None;
            }
            Action::OpenNewTable => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                let schema = self
                    .active()
                    .and_then(|a| a.schema.tables.first().and_then(|t| t.schema.as_deref()))
                    .map(|s| s.to_string());
                self.tab_mut().schema_editor = Some(ObjectEditor::Table(SchemaEditor::new_table(
                    kind,
                    schema.as_deref(),
                )));
                self.schema_pending = None;
            }
            Action::OpenEditTable(table) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor =
                    Some(ObjectEditor::Table(SchemaEditor::edit_table(&table, kind)));
                self.schema_pending = None;
            }
            Action::OpenNewView => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                let schema = self
                    .active()
                    .and_then(|a| a.schema.tables.first().and_then(|t| t.schema.as_deref()))
                    .map(|s| s.to_string());
                self.tab_mut().schema_editor = Some(ObjectEditor::View(ViewEditor::new_view(
                    kind,
                    schema.as_deref(),
                )));
                self.schema_pending = None;
            }
            Action::OpenEditView(view) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor =
                    Some(ObjectEditor::View(ViewEditor::edit_view(&view, kind)));
                self.schema_pending = None;
            }
            Action::DropView(view) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor = None;
                self.schema_pending = Some(vec![dbcore::build_drop_view_sql(
                    kind,
                    view.schema.as_deref(),
                    &view.name,
                    view.materialized,
                )]);
                self.error = None;
            }
            Action::OpenNewTrigger => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                let (schema, tables) = self
                    .active()
                    .map(|a| {
                        let schema = a
                            .schema
                            .tables
                            .first()
                            .and_then(|t| t.schema.as_deref())
                            .map(|s| s.to_string());
                        let tables = a.schema.tables.iter().map(|t| t.name.clone()).collect();
                        (schema, tables)
                    })
                    .unwrap_or_default();
                self.tab_mut().schema_editor = Some(ObjectEditor::Trigger(
                    TriggerEditor::new_trigger(kind, schema.as_deref(), tables),
                ));
                self.schema_pending = None;
            }
            Action::OpenEditTrigger(trg) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                let tables = self
                    .active()
                    .map(|a| a.schema.tables.iter().map(|t| t.name.clone()).collect())
                    .unwrap_or_default();
                self.tab_mut().schema_editor = Some(ObjectEditor::Trigger(
                    TriggerEditor::edit_trigger(&trg, kind, tables),
                ));
                self.schema_pending = None;
            }
            Action::DropTrigger(trg) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor = None;
                self.schema_pending = Some(vec![dbcore::build_drop_trigger_sql(
                    kind,
                    trg.schema.as_deref(),
                    &trg.name,
                    &trg.table,
                )]);
                self.error = None;
            }
            Action::OpenNewRoutine(routine_kind) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                let schema = self
                    .active()
                    .and_then(|a| a.schema.tables.first().and_then(|t| t.schema.as_deref()))
                    .map(|s| s.to_string());
                self.tab_mut().schema_editor = Some(ObjectEditor::Routine(
                    RoutineEditor::new_routine(kind, routine_kind, schema.as_deref()),
                ));
                self.schema_pending = None;
            }
            Action::OpenEditRoutine(routine) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor = Some(ObjectEditor::Routine(
                    RoutineEditor::edit_routine(&routine, kind),
                ));
                self.schema_pending = None;
            }
            Action::DropRoutine(routine) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor = None;
                self.schema_pending = Some(vec![dbcore::build_drop_routine_sql(
                    kind,
                    routine.schema.as_deref(),
                    &routine.name,
                    routine.kind,
                    &routine.params,
                )]);
                self.error = None;
            }
            Action::CloneTable(table) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor = None;
                self.schema_pending = Some(dbcore::build_clone_table_sql(
                    kind,
                    table.schema.as_deref(),
                    &table.name,
                    &format!("{}_copy", table.name),
                ));
                self.error = None;
            }
            Action::TruncateTable(table) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor = None;
                self.schema_pending = Some(vec![dbcore::build_truncate_table_sql(
                    kind,
                    table.schema.as_deref(),
                    &table.name,
                )]);
                self.error = None;
            }
            Action::DropTable(table) => {
                let kind = self.active().map(|a| a.db.kind()).unwrap_or(DbKind::Sqlite);
                self.tab_mut().schema_editor = None;
                self.schema_pending = Some(vec![dbcore::build_drop_table_sql(
                    kind,
                    table.schema.as_deref(),
                    &table.name,
                )]);
                self.error = None;
            }
            Action::ToggleBookmark { schema, table } => {
                // Bookmarks are keyed by the active connection's config id; ignore the toggle
                // if (somehow) there's no live connection to attribute it to.
                if let Some(conn_id) = self.active().map(|a| a.config_id.clone()) {
                    dbcore::bookmarks::toggle(
                        &mut self.bookmarks,
                        &conn_id,
                        schema.as_deref(),
                        &table,
                    );
                    if let Err(e) = dbcore::bookmarks::save(&self.bookmarks) {
                        self.error = Some(format!("Couldn't save bookmarks: {e}"));
                    }
                }
            }
            Action::GenerateSchema => {
                let Some(editor) = &self.tab().schema_editor else {
                    return;
                };
                match editor.build_ddl() {
                    Ok(stmts) => {
                        self.schema_pending = Some(stmts);
                        self.error = None;
                    }
                    Err(msg) => {
                        self.error = Some(msg);
                    }
                }
            }
            Action::ApplySchema => {
                if self.tab_connection_is_read_only(self.active_query_tab) {
                    self.schema_pending = None;
                    self.refuse_read_only("schema changes can't be applied.");
                    return;
                }
                let Some(stmts) = self.schema_pending.take() else {
                    return;
                };
                let Some((db, conn_id)) =
                    self.active().map(|a| (a.db.clone(), a.config_id.clone()))
                else {
                    return;
                };
                let n = stmts.len();
                let tab_id = self.tab().id;
                let tx = self.tx.clone();
                self.busy = Busy::Querying;
                self.error = None;
                self.status_msg = format!("Applying {n} DDL statement(s)…");
                self.rt.spawn(async move {
                    let start = std::time::Instant::now();
                    let result = db
                        .execute_transaction(&stmts)
                        .await
                        .map(|_| format!("Schema migration applied ({n} statement(s))"))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppMessage::SchemaApplied {
                        tab_id,
                        conn_id,
                        sql: stmts.join("\n"),
                        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                        result,
                    });
                });
            }
            Action::CancelSchema => {
                if self.schema_pending.is_some() {
                    self.schema_pending = None;
                } else {
                    self.tab_mut().schema_editor = None;
                }
            }
            Action::OpenUpdateDialog => self.update_dialog_open = true,
            Action::CloseUpdateDialog => self.update_dialog_open = false,
            Action::DismissUpdate => {
                if let Some(version) = match &self.update {
                    crate::update::UpdatePhase::Available(o) => Some(o.version.clone()),
                    crate::update::UpdatePhase::Ready { offer, .. } => Some(offer.version.clone()),
                    _ => None,
                } {
                    self.update_dismissed = Some(version);
                }
                self.update_dialog_open = false;
            }
            Action::DownloadUpdate => {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                self.start_update_download();
            }
            Action::InstallUpdate => {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                if let crate::update::UpdatePhase::Ready { package_path, .. } = &self.update {
                    match crate::update::schedule_install_and_quit(package_path) {
                        Ok(()) => self.pending_quit = true,
                        Err(e) => self.error = Some(e),
                    }
                }
            }
            Action::DismissWhatsNew => self.show_whats_new = false,
        }
    }
}
