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
        self.tab_mut().conn_id = Some(id);
        self.workspace_dirty = true;
        if force || !live {
            self.start_connect(idx);
        } else {
            self.status_msg = format!("Switched to {name}");
            self.error = None;
        }
    }
    /// Drop a live connection from the pool (tabs bound to it become "not connected").
    pub(super) fn disconnect_conn(&mut self, id: &str) {
        self.active_connections.retain(|c| c.config_id != id);
        self.connection_timings.remove(id);
        // Diagram tabs keep their schema snapshot — still viewable, just not refreshable.
        for tab in &mut self.tabs {
            if tab.conn_id.as_deref() == Some(id) {
                tab.result = None;
                tab.row_order.clear();
                tab.sort = None;
                tab.selection.clear();
                tab.edits.clear();
                tab.edits.pending_source = None;
                // A schema editor against a dropped connection is stale; close it.
                tab.schema_editor = None;
            }
        }
        if self.querying_tab_id.is_some_and(|qid| {
            self.tabs
                .iter()
                .any(|t| t.id == qid && t.conn_id.as_deref() == Some(id))
        }) {
            // Abort the in-flight query on the connection we're dropping.
            if let Some(cancel) = self.query_cancel.take() {
                cancel.cancel();
            }
            self.busy = Busy::Idle;
            self.querying_tab_id = None;
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
        self.busy = Busy::Connecting;
        self.error = None;
        self.status_msg = format!("Connecting to {name}…");
        self.rt.spawn(async move {
            let connect_started = Instant::now();
            match dbcore::connect(&cfg, password, ssh_secret).await {
                Ok(db) => {
                    let _ = tx.send(AppMessage::Connected {
                        conn_id: id.clone(),
                        name,
                        elapsed_ms: connect_started.elapsed().as_secs_f64() * 1000.0,
                        result: Ok(db.clone()),
                    });
                    load_connection_metadata(db, id, tx).await;
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
            .is_some_and(|id| self.connections.iter().any(|c| c.id == id && c.production))
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
            .any(|c| c.id == conn_id && c.read_only)
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
        let cfg = editor.config.clone();
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
    pub(super) fn save_connection(&mut self) {
        let Some(ed) = self.editor.take() else { return };
        let cfg = ed.config;
        self.schema_cache.remove(&cfg.id);
        // Persist the password to the keychain (server backends only); never to JSON.
        if cfg.kind.is_server() && !ed.password.is_empty() {
            if let Err(e) = dbcore::secrets::set_password(&cfg.id, &ed.password) {
                self.error = Some(format!("Could not store password: {e}"));
            }
        }
        // Same for the SSH password / key passphrase, in its own keychain entry.
        if cfg.kind.is_server() && cfg.ssh_enabled && !ed.ssh_password.is_empty() {
            if let Err(e) = dbcore::secrets::set_ssh_secret(&cfg.id, &ed.ssh_password) {
                self.error = Some(format!("Could not store SSH password: {e}"));
            }
        }
        match ed.edit_index {
            Some(i) if i < self.connections.len() => self.connections[i] = cfg,
            _ => self.connections.push(cfg),
        }
        if let Err(e) = dbcore::config::save_connections(&self.connections) {
            self.error = Some(e.to_string());
        } else {
            self.status_msg = "Connection saved".to_string();
        }
    }
}
