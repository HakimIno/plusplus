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
            Action::NewTab => {
                self.settings_open = false;
                self.new_tab();
            }
            Action::SelectTab(i) => {
                self.settings_open = false;
                self.select_tab(i);
            }
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
            Action::SetSidebarTab(tab) => self.set_sidebar_tab(tab),
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
            Action::RefreshErd => self.refresh_diagram_tab(self.active_query_tab),
            Action::ShowDatabaseDiagram => {
                let metadata_loading = self.active().is_some_and(|active| {
                    self.connection_jobs.contains(&active.config_id)
                        && !self.schema_cache.contains_key(&active.config_id)
                });
                if metadata_loading {
                    self.status_msg = "Loading schema relationships…".to_string();
                } else if let Some(active) = self.active() {
                    let diagram = crate::erd::ErDiagram::build(&active.config_id, &active.schema);
                    self.open_diagram_tab(active.schema.database_name.clone(), diagram);
                } else {
                    self.error = Some("Connect to a database to open its diagram.".into());
                }
            }
            Action::ImportErd => {
                let Some(conn_id) = self.active().map(|active| active.config_id.clone()) else {
                    self.error = Some("Connect to a target database before importing an ER design.".into());
                    return;
                };
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("PlusPlus ER design", &["json"])
                    .pick_file()
                else {
                    return;
                };
                match std::fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|json| dbcore::ErDesign::from_json(&json))
                {
                    Ok(design) => {
                        let title = design.name.clone();
                        let diagram = crate::erd::ErDiagram::build_design(&conn_id, design);
                        self.open_diagram_tab(title, diagram);
                        self.status_msg = format!("Imported ER design from {}", path.display());
                    }
                    Err(error) => self.error = Some(format!("Could not import ER design: {error}")),
                }
            }
            Action::ExportErd => {
                let Some(design) = self.tab().diagram.as_ref().map(|erd| {
                    let mut design = erd.design.clone();
                    for (table, node) in design.tables.iter_mut().zip(&erd.nodes) {
                        table.layout_x = Some(node.pos.x);
                        table.layout_y = Some(node.pos.y);
                    }
                    design
                }) else {
                    return;
                };
                let file_name = format!(
                    "{}.plusplus-er.json",
                    design
                        .name
                        .chars()
                        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
                        .collect::<String>()
                );
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("PlusPlus ER design", &["json"])
                    .set_file_name(&file_name)
                    .save_file()
                else {
                    return;
                };
                match design
                    .to_json_pretty()
                    .and_then(|json| std::fs::write(&path, json).map_err(|error| error.to_string()))
                {
                    Ok(()) => self.status_msg = format!("ER design saved to {}", path.display()),
                    Err(error) => self.error = Some(format!("Could not export ER design: {error}")),
                }
            }
            Action::ForwardEngineerErd => {
                if self.tab_connection_is_read_only(self.active_query_tab) {
                    self.refuse_read_only("an ER design can't be forward-engineered.");
                    return;
                }
                let Some(active) = self.active() else {
                    self.error = Some("Connect this diagram to a target database first.".into());
                    return;
                };
                let kind = active.db.kind();
                let target_schema = active
                    .schema
                    .tables
                    .first()
                    .and_then(|table| table.schema.clone())
                    .or_else(|| match kind {
                        DbKind::Postgres => Some("public".into()),
                        DbKind::SqlServer => Some("dbo".into()),
                        _ => None,
                    });
                let design = self.tab().diagram.as_ref().map(|erd| erd.design.clone());
                match design.expect("diagram action requires a diagram").forward_ddl(
                    kind,
                    target_schema.as_deref(),
                ) {
                    Ok(statements) if statements.is_empty() => {
                        self.error = Some("Add at least one table before forward engineering.".into());
                    }
                    Ok(statements) => {
                        self.schema_pending = Some(statements);
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Action::AddErdTable => {
                let kind = self.active().map(|active| active.db.kind()).unwrap_or(DbKind::Sqlite);
                let default_schema = self
                    .tab()
                    .diagram
                    .as_ref()
                    .and_then(|erd| erd.design.tables.first())
                    .and_then(|table| table.schema.clone());
                self.tab_mut().schema_editor = Some(ObjectEditor::Table(
                    SchemaEditor::design_table(None, kind, default_schema.as_deref()),
                ));
                self.tab_mut().design_edit_index = Some(None);
            }
            Action::EditErdTable(table_index) => {
                let kind = self.active().map(|active| active.db.kind()).unwrap_or(DbKind::Sqlite);
                let table = self
                    .tab()
                    .diagram
                    .as_ref()
                    .and_then(|erd| erd.design.tables.get(table_index))
                    .cloned();
                let Some(table) = table else { return };
                self.tab_mut().schema_editor = Some(ObjectEditor::Table(
                    SchemaEditor::design_table(Some(&table), kind, None),
                ));
                self.tab_mut().design_edit_index = Some(Some(table_index));
            }
            Action::SaveErdTable => {
                let edit_index = self.tab().design_edit_index.flatten();
                let table = match self.tab().schema_editor.as_ref() {
                    Some(ObjectEditor::Table(editor)) => editor.to_design_table(),
                    _ => return,
                };
                let Ok(mut table) = table else {
                    self.error = table.err();
                    return;
                };
                let Some(old) = self.tab_mut().diagram.take() else { return };
                let mut next_design = old.design.clone();
                if let Some(index) = edit_index {
                    if let Some(node) = old.nodes.get(index) {
                        table.layout_x = Some(node.pos.x);
                        table.layout_y = Some(node.pos.y);
                    }
                }
                let old_name = edit_index
                    .and_then(|index| old.design.tables.get(index))
                    .map(|table| (table.schema.clone(), table.name.clone()));
                if let Some(index) = edit_index {
                    if index >= next_design.tables.len() {
                        self.tab_mut().diagram = Some(old);
                        return;
                    }
                    next_design.tables[index] = table.clone();
                } else {
                    next_design.tables.push(table.clone());
                }
                if let Some((old_schema, old_table)) = old_name {
                    if old_table != table.name || old_schema != table.schema {
                        for candidate in &mut next_design.tables {
                            for fk in &mut candidate.foreign_keys {
                                if fk.ref_table == old_table
                                    && (fk.ref_schema.is_none() || fk.ref_schema == old_schema)
                                {
                                    fk.ref_table = table.name.clone();
                                    fk.ref_schema = table.schema.clone();
                                }
                            }
                        }
                    }
                }
                if let Err(error) = next_design.validate() {
                    self.tab_mut().diagram = Some(old);
                    self.error = Some(error);
                    return;
                }
                let mut fresh = crate::erd::ErDiagram::build_design(&old.conn_id, next_design);
                fresh.scene_rect = old.scene_rect;
                for node in &mut fresh.nodes {
                    if let Some(previous) = old.nodes.iter().find(|old_node| old_node.title == node.title) {
                        node.pos = previous.pos;
                    }
                }
                self.tab_mut().diagram = Some(fresh);
                self.tab_mut().schema_editor = None;
                self.tab_mut().design_edit_index = None;
                self.status_msg = "ER design updated".into();
                self.error = None;
            }
            Action::DeleteErdTable(table_index) => {
                let Some(mut old) = self.tab_mut().diagram.take() else { return };
                if table_index >= old.design.tables.len() {
                    self.tab_mut().diagram = Some(old);
                    return;
                }
                let removed = old.design.tables.remove(table_index);
                for table in &mut old.design.tables {
                    table.foreign_keys.retain(|fk| {
                        !(fk.ref_table == removed.name
                            && (fk.ref_schema.is_none() || fk.ref_schema == removed.schema))
                    });
                }
                let fresh = crate::erd::ErDiagram::build_design(&old.conn_id, old.design);
                self.tab_mut().diagram = Some(fresh);
                self.status_msg = format!("Removed '{}' from the ER design", removed.name);
            }
            Action::ShowTableDiagram { schema, table } => {
                let metadata_loading = self.active().is_some_and(|active| {
                    self.connection_jobs.contains(&active.config_id)
                        && !self.schema_cache.contains_key(&active.config_id)
                });
                if metadata_loading {
                    self.status_msg = "Loading table relationships…".to_string();
                } else if let Some(active) = self.active() {
                    let erd = crate::erd::ErDiagram::build_focused(
                        &active.config_id,
                        &active.schema,
                        crate::erd::ErdFocus {
                            schema,
                            table: table.clone(),
                            depth: 1,
                        },
                    );
                    self.open_diagram_tab(table, erd);
                } else {
                    self.error = Some("Connect to a database to view its diagram.".into());
                }
            }
            Action::SetErdDepth(depth) => self.set_erd_depth(depth),
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
                // The Run button is disabled while busy, but the Cmd+Enter / Cmd+R shortcuts
                // land here unconditionally. Refuse instead of silently racing a second run
                // against the one in flight (or against a connect/import in progress).
                if self.busy != Busy::Idle {
                    self.status_msg = if self.busy == Busy::Querying {
                        "A query is already running — cancel it first to run again.".to_string()
                    } else {
                        "Busy — wait for the current operation to finish.".to_string()
                    };
                    return;
                }
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
                    let Some(kind) = self.tabs[idx]
                        .conn_id
                        .as_deref()
                        .and_then(|id| {
                            self.active_connections
                                .iter()
                                .find(|connection| connection.config_id == id)
                        })
                        .map(|connection| connection.db.kind())
                    else {
                        self.error =
                            Some("Production Guardian requires an active connection.".into());
                        return;
                    };
                    let sql = self.tabs[idx].sql.trim().to_string();
                    let found = dbcore::safety::dangerous_statements(kind, &sql);
                    if !found.is_empty() {
                        self.start_production_guard(
                            idx,
                            sql,
                            found,
                            ProductionGuardContinuation::Query,
                        );
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
                let Some(pending) = self.danger_pending.as_ref() else {
                    return;
                };
                if !pending.can_confirm() {
                    self.error = Some(
                        "Production Guardian checks must finish and the confirmation must match."
                            .to_string(),
                    );
                    return;
                }
                let Some(idx) = self.tabs.iter().position(|tab| tab.id == pending.tab_id) else {
                    self.error = Some("The guarded query tab no longer exists.".to_string());
                    let pending = self.danger_pending.take().expect("guardian checked above");
                    self.record_guard_decision(&pending, "invalidated");
                    if matches!(pending.continuation, ProductionGuardContinuation::Edits) {
                        self.commit_pending = None;
                    }
                    return;
                };
                let source_unchanged = match pending.continuation {
                    ProductionGuardContinuation::Query => self.tabs[idx].sql.trim() == pending.sql,
                    ProductionGuardContinuation::Edits => self
                        .commit_pending
                        .as_ref()
                        .is_some_and(|statements| statements.join("\n") == pending.sql),
                    ProductionGuardContinuation::Schema => self
                        .schema_pending
                        .as_ref()
                        .is_some_and(|statements| statements.join("\n") == pending.sql),
                };
                let unchanged = source_unchanged
                    && self.tabs[idx].conn_id.as_deref() == Some(pending.conn_id.as_str())
                    && self.tab_connection_is_production(idx)
                    && !self.tab_connection_is_read_only(idx)
                    && self
                        .active_connections
                        .iter()
                        .any(|connection| connection.config_id == pending.conn_id);
                if !unchanged {
                    self.error = Some(
                        "The query, connection, or production setting changed. Analyze it again."
                            .to_string(),
                    );
                    let pending = self.danger_pending.take().expect("guardian checked above");
                    self.record_guard_decision(&pending, "invalidated");
                    if matches!(pending.continuation, ProductionGuardContinuation::Edits) {
                        self.commit_pending = None;
                    }
                    return;
                }
                let pending = self.danger_pending.take().expect("guardian checked above");
                if !self.record_guard_decision(&pending, "confirmed") {
                    self.danger_pending = Some(pending);
                    return;
                }
                // Staged-edit and schema continuations operate on the active tab. Restore the
                // guarded tab explicitly in case selection changed while preflight was running.
                self.active_query_tab = idx;
                match pending.continuation {
                    ProductionGuardContinuation::Query => self.start_query_for(idx),
                    ProductionGuardContinuation::Edits => self.confirm_edits(),
                    ProductionGuardContinuation::Schema => self.apply_schema_confirmed(),
                }
            }
            Action::SetDangerConfirmation(value) => {
                if let Some(pending) = &mut self.danger_pending {
                    pending.confirmation = value;
                }
            }
            Action::CancelDangerQuery => {
                if let Some(pending) = self.danger_pending.take() {
                    pending.preflight_cancel.cancel();
                    self.record_guard_decision(&pending, "cancelled");
                    if matches!(pending.continuation, ProductionGuardContinuation::Edits) {
                        // Keep the staged cell edits, but drop the reviewed SQL snapshot so
                        // cancelling Guardian cannot reveal the legacy preview underneath it.
                        self.commit_pending = None;
                    }
                }
                self.status_msg = "Production query cancelled".to_string();
            }
            Action::BeautifySql => self.beautify_sql(),
            Action::OpenTable {
                sql,
                source,
                pin,
                kind,
            } => self.open_table(sql, source, pin, kind),
            Action::OpenDefinition { title, sql, kind } => self.open_definition(title, sql, kind),
            Action::FollowForeignKey { row, col } => self.follow_foreign_key(row, col),
            Action::SetSort { col, asc } => self.tab_mut().set_sort(col, asc),
            Action::ClearSort => self.tab_mut().clear_sort(),
            Action::FilterColumn(col) => {
                let filter = &mut self.tab_mut().filter;
                filter.visible = true;
                if let Some(condition) = filter
                    .conditions
                    .iter_mut()
                    .find(|condition| !condition.is_effective())
                {
                    condition.column = col;
                } else {
                    let condition = crate::filter::Condition {
                        column: col,
                        ..Default::default()
                    };
                    filter.conditions.push(condition);
                }
            }
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
            Action::PreviewEdits => {
                self.commit_edits();
                self.start_pending_edits_guard(self.active_query_tab);
            }
            Action::Undo => self.undo_edits(),
            Action::Redo => self.redo_edits(),
            Action::ConfirmEdits => {
                let idx = self.active_query_tab;
                if self.start_pending_edits_guard(idx) {
                    return;
                }
                self.confirm_edits();
            }
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
            Action::MoveSchemaTable {
                conn_id,
                source_schema,
                source_table,
                target_schema,
                target_table,
                after,
            } => {
                let Some(active) = self
                    .active_connections
                    .iter()
                    .find(|connection| connection.config_id == conn_id)
                else {
                    return;
                };
                let saved_order = self.schema_table_order.get(&conn_id);
                let mut keys: Vec<String> = active
                    .schema
                    .tables
                    .iter()
                    .map(|table| schema_table_key(table.schema.as_deref(), &table.name))
                    .collect();
                keys.sort_by_key(|key| {
                    saved_order
                        .and_then(|order| order.iter().position(|item| item == key))
                        .unwrap_or(usize::MAX)
                });

                let source = schema_table_key(source_schema.as_deref(), &source_table);
                let target = schema_table_key(target_schema.as_deref(), &target_table);
                let Some(source_index) = keys.iter().position(|key| key == &source) else {
                    return;
                };
                keys.remove(source_index);
                let Some(mut target_index) = keys.iter().position(|key| key == &target) else {
                    return;
                };
                if after {
                    target_index += 1;
                }
                keys.insert(target_index, source);
                self.schema_table_order.insert(conn_id, keys);
                self.persist_settings();
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
                let idx = self.active_query_tab;
                if self.tab_connection_is_production(idx) {
                    let Some(statements) = self.schema_pending.as_ref() else {
                        return;
                    };
                    let sql = statements.join("\n");
                    let Some(kind) = self.active().map(|active| active.db.kind()) else {
                        return;
                    };
                    let found = dbcore::safety::dangerous_statements(kind, &sql);
                    if !found.is_empty() {
                        self.start_production_guard(
                            idx,
                            sql,
                            found,
                            ProductionGuardContinuation::Schema,
                        );
                        return;
                    }
                }
                self.apply_schema_confirmed();
            }
            Action::CancelSchema => {
                if self.schema_pending.is_some() {
                    self.schema_pending = None;
                } else {
                    self.tab_mut().schema_editor = None;
                    self.tab_mut().design_edit_index = None;
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

    /// Start the single production confirmation for a staged UPDATE/DELETE transaction.
    /// INSERT-only previews keep the ordinary transaction review dialog.
    fn start_pending_edits_guard(&mut self, idx: usize) -> bool {
        if !self.tab_connection_is_production(idx) || self.tab_connection_is_read_only(idx) {
            return false;
        }
        let Some(statements) = self.commit_pending.as_ref() else {
            return false;
        };
        let sql = statements.join("\n");
        let Some(kind) = self.tabs[idx]
            .conn_id
            .as_deref()
            .and_then(|conn_id| {
                self.active_connections
                    .iter()
                    .find(|connection| connection.config_id == conn_id)
            })
            .map(|connection| connection.db.kind())
        else {
            return false;
        };
        let found = dbcore::safety::dangerous_statements(kind, &sql);
        if found.is_empty() {
            return false;
        }
        self.start_production_guard(idx, sql, found, ProductionGuardContinuation::Edits);
        true
    }

    /// Execute DDL that has already passed read-only checks, preview, and (for production)
    /// Production Guardian. Kept separate so Guardian confirmation cannot recurse into itself.
    fn apply_schema_confirmed(&mut self) {
        if self.tab_connection_is_read_only(self.active_query_tab) {
            self.refuse_read_only("schema changes can't be applied.");
            return;
        }
        let Some(stmts) = self.schema_pending.take() else {
            return;
        };
        let Some((db, conn_id)) = self.active().map(|a| (a.db.clone(), a.config_id.clone())) else {
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
}
