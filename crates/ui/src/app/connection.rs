//! Connecting, disconnecting, testing and saving connections.

use super::*;

impl DbGuiApp {
    /// Bind the active tab to a saved connection. Connects in the background when the
    /// connection isn't live yet (or when `force`, e.g. an explicit "Connect").
    pub(super) fn bind_connection(&mut self, idx: usize, force: bool) {
        let Some(cfg) = self.connections.get(idx) else {
            return;
        };
        let id = cfg.id.clone();
        let name = cfg.name.clone();
        let live = self.active_connections.iter().any(|c| c.config_id == id);
        if self.tab().conn_id.as_deref() != Some(id.as_str()) {
            let kind = self.tab().kind;
            if !matches!(
                kind,
                crate::components::QueryTabKind::Query | crate::components::QueryTabKind::Diagram
            ) {
                // A table / view / routine tab belongs to the database it was opened from:
                // its SQL is written in that dialect and its rows edit that table. Pointing
                // it at another connection would run the one against the wrong database, so
                // the new connection gets a fresh query tab instead.
                let tab_id = self.next_tab_id;
                self.next_tab_id += 1;
                self.tabs.push(QueryTab::new(tab_id, String::new()));
                self.active_query_tab = self.tabs.len() - 1;
            } else if self.tab().edits.has_pending() {
                self.error = Some(
                    "Save or discard this tab's staged edits before switching its connection."
                        .into(),
                );
                return;
            } else {
                self.cancel_tab_query(self.tab().id);
                // The result on screen, its edit source and its paging came from the previous
                // database; keeping them would let an edit or a load-more reach the new one.
                let tab = self.tab_mut();
                tab.result = None;
                tab.clear_batch_results();
                tab.row_order.clear();
                tab.selection.clear();
                tab.edits.clear();
                tab.edits.source = None;
                tab.edits.pending_source = None;
                tab.page_exhausted = false;
                tab.total_rows = None;
                tab.server_filter_predicate = None;
            }
        }
        self.tab_mut().conn_id = Some(id.clone());
        // A portable diagram can be retargeted from the normal connection switcher. Keep
        // its refresh/apply routing in sync with the tab while leaving the design untouched.
        if let Some(diagram) = self.tab_mut().diagram.as_mut() {
            diagram.conn_id = id.clone();
        }
        self.workspace_dirty = true;
        if force || !live {
            if force && live && self.connection_jobs.contains(&id) {
                self.status_msg = format!(
                    "{name} is loading its schema. Wait for it to finish before reconnecting."
                );
                return;
            }
            if force && live {
                self.disconnect_conn(&id);
            }
            self.start_connect(idx);
        } else {
            self.status_msg = format!("Switched to {name}");
            self.error = None;
        }
    }
    /// Show `id`'s own tabs. Every tab belongs to the connection it was opened on and is never
    /// re-pointed at another: its SQL is written in that dialect, its rows edit that database,
    /// and a running query or staged edit would otherwise end up on the wrong server. So
    /// switching returns to the tab the user last had on `id`, or opens a fresh query tab there.
    /// The one exception is a tab with no connection at all, which simply adopts it.
    pub(super) fn switch_to_connection_tabs(&mut self, leaving: Option<String>, id: &str) {
        if let Some(leaving) = leaving {
            let tab_id = self.tab().id;
            self.conn_last_tab.insert(leaving, tab_id);
        }
        let adopts = {
            let tab = self.tab();
            tab.conn_id.is_none()
                && matches!(
                    tab.kind,
                    crate::components::QueryTabKind::Query
                        | crate::components::QueryTabKind::Diagram
                )
        };
        if adopts {
            self.tab_mut().conn_id = Some(id.to_string());
            // A portable diagram follows its tab so refresh/apply route to this connection.
            if let Some(diagram) = self.tab_mut().diagram.as_mut() {
                diagram.conn_id = id.to_string();
            }
            return;
        }
        let owned = |tab: &QueryTab| tab.conn_id.as_deref() == Some(id);
        let target = self
            .conn_last_tab
            .get(id)
            .and_then(|tab_id| {
                self.tabs
                    .iter()
                    .position(|tab| tab.id == *tab_id && owned(tab))
            })
            .or_else(|| self.tabs.iter().rposition(owned));
        match target {
            Some(idx) => self.select_tab(idx),
            None => {
                self.close_split_workspace();
                let tab_id = self.next_tab_id;
                self.next_tab_id += 1;
                let mut tab = QueryTab::new(tab_id, String::new());
                tab.conn_id = Some(id.to_string());
                self.tabs.push(tab);
                self.active_query_tab = self.tabs.len() - 1;
            }
        }
    }
    /// Drop a live connection from the pool (tabs bound to it become "not connected").
    pub(super) fn disconnect_conn(&mut self, id: &str) {
        if let Some(cancel) = self.connection_cancels.remove(id) {
            cancel.cancel();
        }
        self.active_connections.retain(|c| c.config_id != id);
        self.connection_timings.remove(id);
        // Diagram tabs keep their schema snapshot — still viewable, just not refreshable.
        for tab in &mut self.tabs {
            if tab.conn_id.as_deref() == Some(id) {
                tab.result = None;
                tab.clear_batch_results();
                tab.row_order.clear();
                tab.sort = None;
                tab.selection.clear();
                tab.edits.clear();
                tab.edits.pending_source = None;
                tab.stream = None;
                tab.page_exhausted = false;
                // A schema editor against a dropped connection is stale; close it.
                tab.schema_editor = None;
                tab.table_metadata_pending = false;
                tab.design_edit_index = None;
            }
        }
        let tab_ids: Vec<_> = self
            .tabs
            .iter()
            .filter(|t| t.conn_id.as_deref() == Some(id))
            .map(|t| t.id)
            .collect();
        for tab_id in tab_ids {
            self.cancel_tab_query(tab_id);
        }
        self.status_msg = "Disconnected".to_string();
        self.error = None;
    }
    pub(super) fn record_connection_timing(
        &mut self,
        conn_id: &str,
        stage: ConnectStage,
        elapsed_ms: f64,
    ) {
        let timings = self
            .connection_timings
            .entry(conn_id.to_string())
            .or_default();
        let label = match stage {
            ConnectStage::Connect => {
                timings.connect_ms = Some(elapsed_ms);
                "connect"
            }
            ConnectStage::Overview => {
                timings.overview_ms = Some(elapsed_ms);
                "overview"
            }
            ConnectStage::FullSchema => {
                timings.full_schema_ms = Some(elapsed_ms);
                "full_schema"
            }
            ConnectStage::DatabaseList => {
                timings.database_list_ms = Some(elapsed_ms);
                "database_list"
            }
        };
        #[cfg(debug_assertions)]
        eprintln!("plusplus perf: connection={conn_id} stage={label} elapsed_ms={elapsed_ms:.1}");
    }
    pub(super) fn start_connect(&mut self, idx: usize) {
        let Some(cfg) = self.connections.get(idx).cloned() else {
            return;
        };
        if !self.connection_jobs.insert(cfg.id.clone()) {
            self.status_msg = format!("{} is already connecting or loading schema", cfg.name);
            return;
        }
        let password = if cfg.kind.is_server() {
            dbcore::secrets::get_password(&cfg.id).ok().flatten()
        } else {
            None
        };
        let ssh_secret = if cfg.ssh_enabled && cfg.kind.is_server() {
            dbcore::secrets::get_ssh_secret(&cfg.id).ok().flatten()
        } else {
            None
        };
        let tx = self.tx.clone();
        let id = cfg.id.clone();
        let name = cfg.name.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        self.connection_cancels.insert(id.clone(), cancel.clone());
        if self.busy == Busy::Idle {
            self.busy = Busy::Connecting;
        }
        self.error = None;
        self.status_msg = format!("Connecting to {name}…");
        self.rt.spawn(async move {
            let connect_started = Instant::now();
            const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
            let connected = tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = tx.send(AppMessage::ConnectionJobCancelled {
                        conn_id: id.clone(),
                    });
                    return;
                }
                result = tokio::time::timeout(
                    CONNECT_TIMEOUT,
                    dbcore::connect(&cfg, password, ssh_secret),
                ) => match result {
                    Ok(result) => result,
                    Err(_) => Err(dbcore::CoreError::Pool(
                        "connection timed out after 30 seconds".into(),
                    )),
                },
            };
            match connected {
                Ok(db) => {
                    if tx
                        .send(AppMessage::Connected {
                            conn_id: id.clone(),
                            name,
                            elapsed_ms: connect_started.elapsed().as_secs_f64() * 1000.0,
                            result: Ok(db.clone()),
                        })
                        .is_err()
                    {
                        return;
                    }
                    let _ = load_connection_metadata(db, id, tx, cancel).await;
                }
                Err(e) => {
                    let _ = tx.send(AppMessage::Connected {
                        conn_id: id,
                        name,
                        elapsed_ms: connect_started.elapsed().as_secs_f64() * 1000.0,
                        result: Err(e.to_string()),
                    });
                }
            }
        });
    }
    /// Is the tab at `idx` bound to a connection whose saved config is marked production?
    pub(super) fn tab_connection_is_production(&self, idx: usize) -> bool {
        self.tabs
            .get(idx)
            .and_then(|tab| tab.conn_id.as_deref())
            .is_some_and(|id| {
                self.connections
                    .iter()
                    .any(|c| c.id == id && c.is_production())
            })
    }
    /// Is the tab at `idx` bound to a connection whose saved config is marked read-only?
    pub(super) fn tab_connection_is_read_only(&self, idx: usize) -> bool {
        self.tabs
            .get(idx)
            .and_then(|tab| tab.conn_id.as_deref())
            .is_some_and(|id| self.connection_is_read_only(id))
    }
    /// Is the saved config for `conn_id` marked read-only? Sidebar actions (import, export)
    /// act on a connection rather than a tab, so they check it directly.
    pub(super) fn connection_is_read_only(&self, conn_id: &str) -> bool {
        self.connections
            .iter()
            .any(|c| c.id == conn_id && c.is_read_only())
    }
    /// Is the saved config for `conn_id` protected by Production Guardian?
    pub(super) fn connection_is_production(&self, conn_id: &str) -> bool {
        self.connections
            .iter()
            .any(|c| c.id == conn_id && c.is_production())
    }
    /// Refuse an action on a read-only connection with a consistent error + status pair.
    /// `what` completes the sentence "This connection is read-only — {what}".
    pub(super) fn refuse_read_only(&mut self, what: &str) {
        self.error = Some(format!("This connection is read-only — {what}"));
        self.status_msg = "Blocked by read-only mode".to_string();
    }
    /// Open a file picker filtered to `extensions` and store the chosen path into the
    /// connection-editor field selected by `field`.
    pub(super) fn browse_pem_into(
        &mut self,
        extensions: &[&str],
        field: impl FnOnce(&mut dbcore::ConnectionConfig) -> &mut String,
    ) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PEM file", extensions)
            .add_filter("All files", &["*"])
            .pick_file()
        {
            if let Some(ed) = &mut self.editor {
                *field(&mut ed.config) = path.to_string_lossy().into_owned();
                ed.test_state = ConnTestState::Untested;
            }
        }
    }
    pub(super) fn start_connection_test(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let mut cfg = editor.config.clone();
        cfg.apply_safety_profile();
        let password = if cfg.kind.is_server() {
            Some(editor.password.clone())
        } else {
            None
        };
        let ssh_secret = if cfg.ssh_enabled && cfg.kind.is_server() {
            Some(editor.ssh_password.clone())
        } else {
            None
        };
        if let Err((message, fields)) = validate_connection_test_config(&cfg) {
            editor.test_state = ConnTestState::Failed { message, fields };
            self.status_msg = "Connection test failed".to_string();
            return;
        }

        let test_id = self.next_connection_test_id;
        self.next_connection_test_id += 1;
        editor.test_state = ConnTestState::Testing(test_id);
        self.error = None;
        self.status_msg = format!("Testing {}…", cfg.name);

        let tx = self.tx.clone();
        let conn_id = cfg.id.clone();
        self.rt.spawn(async move {
            let result = dbcore::connect(&cfg, password, ssh_secret)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::ConnectionTested {
                test_id,
                conn_id,
                result,
            });
        });
    }
    pub(super) fn save_connection(&mut self) -> Option<usize> {
        let ed = self.editor.as_ref()?;
        let mut cfg = ed.config.clone();
        cfg.apply_safety_profile();
        if let Err((message, fields)) = validate_connection_test_config(&cfg) {
            self.editor.as_mut()?.test_state = ConnTestState::Failed { message, fields };
            return None;
        }
        // Persist the password to the keychain (server backends only); never to JSON.
        if cfg.kind.is_server() && !ed.password.is_empty() {
            if let Err(e) = dbcore::secrets::set_password(&cfg.id, &ed.password) {
                self.connection_save_failed(format!("Could not store password: {e}"));
                return None;
            }
        }
        // Same for the SSH password / key passphrase, in its own keychain entry.
        if cfg.kind.is_server() && cfg.ssh_enabled && !ed.ssh_password.is_empty() {
            if let Err(e) = dbcore::secrets::set_ssh_secret(&cfg.id, &ed.ssh_password) {
                self.connection_save_failed(format!("Could not store SSH password: {e}"));
                return None;
            }
        }
        let idx = ed
            .edit_index
            .filter(|i| *i < self.connections.len())
            .unwrap_or(self.connections.len());
        let mut connections = self.connections.clone();
        if idx < connections.len() {
            connections[idx] = cfg.clone();
        } else {
            connections.push(cfg.clone());
        }
        if let Err(e) = dbcore::config::save_connections(&connections) {
            self.connection_save_failed(e.to_string());
            return None;
        }
        self.schema_cache.remove(&cfg.id);
        self.connections = connections;
        self.editor = None;
        self.error = None;
        self.status_msg = "Connection saved".to_string();
        Some(idx)
    }

    fn connection_save_failed(&mut self, message: String) {
        if let Some(editor) = self.editor.as_mut() {
            editor.test_state = ConnTestState::Failed {
                message: message.clone(),
                fields: Vec::new(),
            };
        }
        self.error = Some(message);
    }

    pub(super) fn open_sample_database(&mut self) {
        let path = match dbcore::config::config_dir() {
            Ok(dir) => dir.join("sample.sqlite"),
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let create = || -> std::io::Result<()> {
            use std::io::Write;
            std::fs::create_dir_all(path.parent().expect("config directory"))?;
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(include_bytes!("../../../../examples/sample.sqlite"))
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(e) => Err(e),
            }
        };
        if let Err(e) = create() {
            self.error = Some(format!("Could not open sample database: {e}"));
            return;
        }
        let sample_path = path.to_string_lossy().into_owned();
        let idx = match self
            .connections
            .iter()
            .position(|c| c.kind == DbKind::Sqlite && c.sqlite_path == sample_path)
        {
            Some(idx) => idx,
            None => {
                let mut config = ConnectionConfig::new(DbKind::Sqlite);
                config.name = "Sample database".into();
                config.sqlite_path = sample_path;
                config.set_safety_profile(dbcore::SafetyProfile::Development);
                let mut connections = self.connections.clone();
                connections.push(config);
                if let Err(e) = dbcore::config::save_connections(&connections) {
                    self.error = Some(e.to_string());
                    return;
                }
                self.connections = connections;
                self.connections.len() - 1
            }
        };
        self.show_welcome = false;
        self.persist_settings();
        if !self.tabs.get(self.active_query_tab).is_some_and(|tab| {
            tab.kind == crate::components::QueryTabKind::Query
                && tab.sql.trim().is_empty()
                && tab.result.is_none()
                && !tab.edits.has_pending()
        }) {
            self.new_tab();
        }
        self.bind_connection(idx, false);
        self.open_table(
            DbKind::Sqlite.preview_query("\"customers\"", 100),
            EditSource {
                schema: None,
                table: "customers".into(),
                pk_cols: vec!["id".into()],
            },
            true,
            crate::components::QueryTabKind::Table,
        );
    }
}
