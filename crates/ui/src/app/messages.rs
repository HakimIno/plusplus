//! Draining the background-task channel: one arm per kind of work the runtime finishes.

use super::*;

impl DbGuiApp {
    pub(super) fn poll_messages(&mut self, ctx: &egui::Context) {
        let mut streamed_batches = 0usize;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AppMessage::Connected {
                    conn_id,
                    name,
                    elapsed_ms,
                    result,
                } => {
                    self.record_connection_timing(&conn_id, ConnectStage::Connect, elapsed_ms);
                    if !self.connections.iter().any(|cfg| cfg.id == conn_id) {
                        self.connection_jobs.remove(&conn_id);
                        self.connection_cancels.remove(&conn_id);
                        continue;
                    }
                    self.busy = Busy::Idle;
                    match result {
                        Ok(db) => {
                            let arrived_id = conn_id.clone();
                            let cached_schema = self.schema_cache.get(&conn_id).cloned();
                            let has_cached_schema = cached_schema.is_some();
                            let initial_schema = cached_schema.unwrap_or_else(|| SchemaTree {
                                database_name: name.clone(),
                                ..SchemaTree::default()
                            });
                            if let Some(idx) = self
                                .active_connections
                                .iter()
                                .position(|conn| conn.config_id == conn_id)
                            {
                                let prev_databases =
                                    std::mem::take(&mut self.active_connections[idx].databases);
                                self.active_connections[idx] = ActiveConnection {
                                    config_id: conn_id,
                                    name: name.clone(),
                                    db,
                                    schema: initial_schema,
                                    databases: prev_databases,
                                };
                            } else {
                                self.active_connections.push(ActiveConnection {
                                    config_id: conn_id,
                                    name: name.clone(),
                                    db,
                                    schema: initial_schema,
                                    databases: Vec::new(),
                                });
                            }
                            self.status_msg = if has_cached_schema {
                                format!("Connected to {name} — cached schema, refreshing…")
                            } else {
                                format!("Connected to {name} — loading schema…")
                            };
                            self.error = None;
                            self.record_audit(
                                dbcore::audit::AuditAction::Connect,
                                &arrived_id,
                                "",
                                true,
                                None,
                                None,
                                0.0,
                            );
                        }
                        Err(e) => {
                            self.connection_jobs.remove(&conn_id);
                            self.connection_cancels.remove(&conn_id);
                            self.record_audit(
                                dbcore::audit::AuditAction::Connect,
                                &conn_id,
                                "",
                                false,
                                Some(e.clone()),
                                None,
                                0.0,
                            );
                            self.error = Some(format!("Connection failed: {e}"));
                            self.status_msg = "Connection failed".to_string();
                        }
                    }
                }
                AppMessage::SchemaOverviewLoaded {
                    conn_id,
                    mut schema,
                    elapsed_ms,
                } => {
                    self.record_connection_timing(&conn_id, ConnectStage::Overview, elapsed_ms);
                    // A complete cached schema is more useful than the name-only overview.
                    // Keep showing it until the refreshed full schema arrives.
                    if self.schema_cache.contains_key(&conn_id) {
                        continue;
                    }
                    if let Some(active) = self
                        .active_connections
                        .iter_mut()
                        .find(|conn| conn.config_id == conn_id)
                    {
                        let n = schema.tables.len();
                        let name = active.name.clone();
                        if schema.database_name.is_empty() {
                            schema.database_name = name.clone();
                        }
                        active.schema = schema;
                        self.status_msg =
                            format!("Connected to {name} — {n} tables, loading details…");
                    }
                }
                AppMessage::SchemaLoaded {
                    conn_id,
                    elapsed_ms,
                    result,
                } => {
                    self.record_connection_timing(&conn_id, ConnectStage::FullSchema, elapsed_ms);
                    let Some(idx) = self
                        .active_connections
                        .iter()
                        .position(|conn| conn.config_id == conn_id)
                    else {
                        continue;
                    };
                    match result {
                        Ok(schema) => {
                            let n = schema.tables.len();
                            let name = self.active_connections[idx].name.clone();
                            self.schema_cache.insert(conn_id.clone(), schema.clone());
                            self.active_connections[idx].schema = schema;
                            // Queries can finish before full PK metadata arrives. Reconcile the
                            // edit source now so their grids become editable without a rerun.
                            self.refresh_edit_sources(&conn_id);
                            self.status_msg = format!("Connected to {name} — {n} tables");
                            self.error = None;
                            // Diagram tabs of this connection track the fresh schema.
                            for i in 0..self.tabs.len() {
                                if self.tabs[i]
                                    .diagram
                                    .as_ref()
                                    .is_some_and(|d| d.conn_id == conn_id && d.tracks_schema)
                                {
                                    self.refresh_diagram_tab(i);
                                }
                            }
                        }
                        Err(e) => {
                            self.error = Some(format!("Schema load failed: {e}"));
                            self.status_msg = if self.schema_cache.contains_key(&conn_id) {
                                "Connected — cached schema; refresh failed".to_string()
                            } else {
                                "Connected — schema unavailable".to_string()
                            };
                        }
                    }
                }
                AppMessage::ConnectionJobFinished { conn_id } => {
                    self.connection_jobs.remove(&conn_id);
                    self.connection_cancels.remove(&conn_id);
                }
                AppMessage::ConnectionJobCancelled { conn_id } => {
                    self.connection_jobs.remove(&conn_id);
                    self.connection_cancels.remove(&conn_id);
                    // A successful `Connected` message may already be queued when the user
                    // disconnects. Ensure that late delivery cannot resurrect its database Arc.
                    self.active_connections
                        .retain(|connection| connection.config_id != conn_id);
                    if self.busy == Busy::Connecting {
                        self.busy = Busy::Idle;
                    }
                }
                AppMessage::DatabaseListLoaded {
                    conn_id,
                    databases,
                    elapsed_ms,
                } => {
                    self.record_connection_timing(&conn_id, ConnectStage::DatabaseList, elapsed_ms);
                    if let Some(active) = self
                        .active_connections
                        .iter_mut()
                        .find(|conn| conn.config_id == conn_id)
                    {
                        active.databases = databases;
                    }
                }
                AppMessage::ConnectionTested {
                    test_id,
                    conn_id,
                    result,
                } => {
                    if let Some(editor) = &mut self.editor {
                        if editor.config.id != conn_id {
                            continue;
                        }
                        if !matches!(editor.test_state, ConnTestState::Testing(id) if id == test_id)
                        {
                            continue;
                        }
                        match result {
                            Ok(()) => {
                                editor.test_state = ConnTestState::Success;
                                self.status_msg = "Connection test succeeded".to_string();
                                self.error = None;
                            }
                            Err(e) => {
                                editor.test_state = ConnTestState::Failed {
                                    fields: infer_connection_error_fields(&e, editor.config.kind),
                                    message: e.clone(),
                                };
                                self.status_msg = "Connection test failed".to_string();
                                self.error = Some(format!("Connection test failed: {e}"));
                            }
                        }
                    }
                }
                AppMessage::ProductionGuarded {
                    tab_id,
                    conn_id,
                    sql,
                    preflights,
                } => {
                    let Some(pending) = &mut self.danger_pending else {
                        continue;
                    };
                    // A cancel, edited query, tab switch, or reconnect can race preflight.
                    // Only the exact snapshot that launched the checks may unlock the dialog.
                    if pending.tab_id != tab_id
                        || pending.conn_id != conn_id
                        || pending.sql != sql
                        || pending.statements.len() != preflights.len()
                    {
                        continue;
                    }
                    pending.preflights = Some(preflights);
                    self.status_msg = "Production Guardian review ready".to_string();
                    self.error = None;
                }
                AppMessage::Queried {
                    tab_id,
                    conn_id,
                    sql,
                    result,
                    canceled,
                    seq,
                } => {
                    // A result from a superseded run (the user started a newer query before
                    // this one finished) must not touch busy/status or the tab: whichever run
                    // finished last would otherwise win, showing stale rows or stealing the
                    // pending edit source. It did execute, though, so it still goes to history.
                    let stale = seq != self.query_seq;
                    if !stale {
                        self.busy = Busy::Idle;
                        self.querying_tab_id = None;
                        self.query_cancel = None;
                    }
                    // A user cancel isn't a failure: don't log it as a failed statement and
                    // don't flag a red error — just note it and leave the previous result up.
                    if canceled {
                        if !stale {
                            self.status_msg = "Query cancelled".to_string();
                            self.error = None;
                        }
                        continue;
                    }
                    match &result {
                        Ok(res) => {
                            let rows = res.stats.rows_affected.unwrap_or(res.row_count() as u64);
                            self.record_history(
                                dbcore::audit::AuditAction::Query,
                                &conn_id,
                                &sql,
                                true,
                                None,
                                Some(rows),
                                res.stats.elapsed_ms,
                            );
                        }
                        Err(e) => self.record_history(
                            dbcore::audit::AuditAction::Query,
                            &conn_id,
                            &sql,
                            false,
                            Some(e.clone()),
                            None,
                            0.0,
                        ),
                    }
                    if stale {
                        continue;
                    }
                    let is_active = self
                        .tabs
                        .get(self.active_query_tab)
                        .is_some_and(|t| t.id == tab_id);
                    let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
                        continue;
                    };
                    // A disconnect can race an in-flight query; ignore stale results.
                    if tab.conn_id.as_deref().is_some_and(|id| {
                        !self.active_connections.iter().any(|c| c.config_id == id)
                    }) {
                        continue;
                    }
                    match result {
                        Ok(res) => {
                            // Promote the in-flight source and start from a clean edit slate.
                            tab.stream = None;
                            tab.query_error = None;
                            tab.edits.source = tab.edits.pending_source.take();
                            tab.edits.clear();
                            let status = result_status(&res);
                            tab.set_result(res);
                            if is_active {
                                self.status_msg = status;
                                self.error = None;
                            }
                        }
                        Err(e) => {
                            tab.stream = None;
                            tab.view = TabView::Data;
                            tab.query_error = Some(e.clone());
                            if is_active {
                                // Query failures already own the result surface. Keep the global
                                // status strip quiet so the same error is not shown twice.
                                self.error = None;
                                self.status_msg = "Ready".to_string();
                            }
                        }
                    }
                }
                AppMessage::QueryTotal { tab_id, total, seq } => {
                    if seq != self.query_seq {
                        continue;
                    }
                    let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                        continue;
                    };
                    tab.total_rows = total;
                    tab.total_rows_pending = false;
                    ctx.request_repaint();
                }
                AppMessage::QueryStreamStarted {
                    tab_id,
                    columns,
                    append,
                    seq,
                } => {
                    if seq != self.query_seq {
                        continue;
                    }
                    let disconnected = self
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .and_then(|tab| tab.conn_id.as_deref())
                        .is_some_and(|id| {
                            !self
                                .active_connections
                                .iter()
                                .any(|connection| connection.config_id == id)
                        });
                    if disconnected {
                        continue;
                    }
                    let is_active = self
                        .tabs
                        .get(self.active_query_tab)
                        .is_some_and(|tab| tab.id == tab_id);
                    let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                        continue;
                    };
                    tab.query_error = None;
                    tab.stream = Some(QueryStreamUi {
                        seq,
                        append,
                        columns,
                        pending_rows: Vec::new(),
                        received_rows: 0,
                    });
                    if is_active {
                        self.error = None;
                    }
                    ctx.request_repaint();
                }
                AppMessage::QueryRows { tab_id, rows, seq } => {
                    if seq != self.query_seq {
                        continue;
                    }
                    let disconnected = self
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .and_then(|tab| tab.conn_id.as_deref())
                        .is_some_and(|id| {
                            !self
                                .active_connections
                                .iter()
                                .any(|connection| connection.config_id == id)
                        });
                    if disconnected {
                        continue;
                    }
                    let is_active = self
                        .tabs
                        .get(self.active_query_tab)
                        .is_some_and(|tab| tab.id == tab_id);
                    let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                        continue;
                    };
                    let Some(stream) = tab.stream.as_ref().filter(|stream| stream.seq == seq)
                    else {
                        continue;
                    };
                    let append = stream.append;
                    let columns = stream.columns.clone();
                    let batch_len = rows.len();
                    if append {
                        let result = tab.result.get_or_insert_with(|| QueryResult {
                            columns,
                            ..QueryResult::default()
                        });
                        result.rows.extend(rows);
                        tab.recompute_view();
                        if let Some(stream) = tab.stream.as_mut() {
                            stream.received_rows += batch_len;
                        }
                        streamed_batches += 1;
                        ctx.request_repaint();
                        // Do not drain an appended continuation in one egui frame. Replacement
                        // batches stay hidden and are cheap to drain before their atomic swap.
                        if streamed_batches >= 2 {
                            self.enforce_result_memory_budget();
                            return;
                        }
                    } else if let Some(stream) = tab.stream.as_mut() {
                        stream.pending_rows.extend(rows);
                        stream.received_rows += batch_len;
                    }
                    if is_active {
                        self.error = None;
                    }
                }
                AppMessage::QueryStreamFinished {
                    tab_id,
                    conn_id,
                    sql,
                    elapsed_ms,
                    rows_loaded,
                    page,
                    result_limit,
                    append,
                    result,
                    canceled,
                    budget_truncated,
                    row_truncated,
                    seq,
                } => {
                    let stale = seq != self.query_seq;
                    if !stale {
                        self.busy = Busy::Idle;
                        self.querying_tab_id = None;
                        self.query_cancel = None;
                    }
                    if canceled {
                        if !stale {
                            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                                tab.stream = None;
                                tab.page_exhausted = false;
                            }
                            self.status_msg = if !append || rows_loaded == 0 {
                                "Query cancelled".to_string()
                            } else {
                                format!("Query cancelled — {rows_loaded} rows loaded")
                            };
                            self.error = None;
                        }
                        continue;
                    }
                    match &result {
                        Ok(rows) => self.record_history(
                            dbcore::audit::AuditAction::Query,
                            &conn_id,
                            &sql,
                            true,
                            None,
                            Some(*rows),
                            elapsed_ms,
                        ),
                        Err(error) => self.record_history(
                            dbcore::audit::AuditAction::Query,
                            &conn_id,
                            &sql,
                            false,
                            Some(error.clone()),
                            None,
                            elapsed_ms,
                        ),
                    }
                    if stale {
                        continue;
                    }
                    let is_active = self
                        .tabs
                        .get(self.active_query_tab)
                        .is_some_and(|tab| tab.id == tab_id);
                    let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                        continue;
                    };
                    if tab.conn_id.as_deref().is_some_and(|id| {
                        !self
                            .active_connections
                            .iter()
                            .any(|connection| connection.config_id == id)
                    }) {
                        continue;
                    }
                    match result {
                        Ok(_) => {
                            let stream = tab.stream.take();
                            // Replacement chunks are deliberately hidden while loading. Install
                            // their complete result once, so Go never exposes a one-row interim
                            // grid. Continuations were already appended chunk by chunk above.
                            if !append {
                                let mut stream = stream.unwrap_or(QueryStreamUi {
                                    seq,
                                    append: false,
                                    columns: Vec::new(),
                                    pending_rows: Vec::new(),
                                    received_rows: 0,
                                });
                                let columns = Some(std::mem::take(&mut stream.columns))
                                    .filter(|columns| !columns.is_empty())
                                    .or_else(|| {
                                        tab.result.as_ref().map(|result| result.columns.clone())
                                    })
                                    .unwrap_or_default();
                                tab.edits.source = tab.edits.pending_source.take();
                                tab.edits.clear();
                                tab.set_result(QueryResult {
                                    columns,
                                    rows: stream.pending_rows,
                                    ..QueryResult::default()
                                });
                            }
                            let capped_by_global_limit = dbcore::parse_page_window(&tab.sql)
                                .and_then(|window| window.limit)
                                .is_some_and(|limit| limit > MAX_FETCH_ROWS as u64);
                            let result = tab.result.get_or_insert_with(QueryResult::default);
                            result.stats.elapsed_ms = elapsed_ms;
                            result.stats.rows_affected = None;
                            let total_rows = result.row_count() as u64;
                            result.truncated = budget_truncated
                                || row_truncated
                                || (total_rows >= MAX_FETCH_ROWS as u64 && capped_by_global_limit);
                            tab.page_exhausted = budget_truncated
                                || row_truncated
                                || total_rows >= result_limit
                                || page.limit.is_some_and(|limit| rows_loaded < limit);
                            let status = result_status(result);
                            tab.query_error = None;
                            tab.recompute_view();
                            if is_active {
                                self.status_msg = status;
                                self.error = None;
                            }
                        }
                        Err(error) => {
                            tab.stream = None;
                            tab.page_exhausted = false;
                            tab.view = TabView::Data;
                            // A continuation/replacement failure should not cover useful rows
                            // already on screen with an error page.
                            if tab.result.is_some() {
                                tab.query_error = None;
                                if is_active {
                                    self.status_msg = "Couldn’t load rows".to_string();
                                    self.error = Some(error);
                                }
                            } else {
                                tab.query_error = Some(error);
                            }
                            if is_active && tab.result.is_none() {
                                self.status_msg = "Ready".to_string();
                                self.error = None;
                            }
                        }
                    }
                }
                AppMessage::Exported { table, result } => match result {
                    Ok((path, rows)) => {
                        let file = path
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.to_string_lossy().into_owned());
                        self.status_msg = format!(
                            "Exported {rows} row{} from {table} to {file}",
                            if rows == 1 { "" } else { "s" }
                        );
                        self.error = None;
                    }
                    Err(e) => {
                        self.error = Some(format!("Export of {table} failed: {e}"));
                        self.status_msg = "Export failed".to_string();
                    }
                },
                AppMessage::ImportProgress { rows } => {
                    self.status_msg = format!("Importing… {rows} rows read");
                }
                AppMessage::Imported {
                    table,
                    conn_id,
                    sql,
                    elapsed_ms,
                    result,
                } => {
                    self.busy = Busy::Idle;
                    // Audited, but not added to query history: the summary line isn't runnable
                    // SQL, and the real statements are far too large to keep.
                    self.record_audit(
                        dbcore::audit::AuditAction::Import,
                        &conn_id,
                        &sql,
                        result.is_ok(),
                        result.as_ref().err().cloned(),
                        result.as_ref().ok().map(|n| *n as u64),
                        elapsed_ms,
                    );
                    match result {
                        Ok(n) => {
                            self.status_msg = format!(
                                "Imported {n} row{} into {table}",
                                if n == 1 { "" } else { "s" }
                            );
                            self.error = None;
                            // Show the new rows if the active tab is reading the table we
                            // just wrote to.
                            let idx = self.active_query_tab;
                            let shows_table = self.tabs.get(idx).is_some_and(|t| {
                                t.edits
                                    .source
                                    .as_ref()
                                    .is_some_and(|s| s.table.eq_ignore_ascii_case(&table))
                            });
                            if shows_table {
                                self.tabs[idx].edits.pending_source =
                                    self.tabs[idx].edits.source.clone();
                                self.start_query_for(idx);
                            }
                        }
                        Err(e) => {
                            // The transaction rolled back: nothing was written.
                            self.error = Some(format!("Import into {table} failed: {e}"));
                            self.status_msg = "Import failed".to_string();
                        }
                    }
                }
                AppMessage::Committed {
                    tab_id,
                    conn_id,
                    sql,
                    elapsed_ms,
                    result,
                } => {
                    self.busy = Busy::Idle;
                    self.record_history(
                        dbcore::audit::AuditAction::EditCommit,
                        &conn_id,
                        &sql,
                        result.is_ok(),
                        result.as_ref().err().cloned(),
                        None,
                        elapsed_ms,
                    );
                    let is_active = self
                        .tabs
                        .get(self.active_query_tab)
                        .is_some_and(|t| t.id == tab_id);
                    match result {
                        Ok(n) => {
                            if is_active {
                                self.status_msg = format!("Saved {n} change(s)");
                                self.error = None;
                            }
                            // Reload so the grid reflects exactly what the database now holds
                            // (triggers, defaults, type coercions). Keep the source editable.
                            if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) {
                                self.tabs[idx].edits.pending_source =
                                    self.tabs[idx].edits.source.clone();
                                self.start_query_for(idx);
                            }
                        }
                        Err(e) if is_active => {
                            self.error = Some(format!("Save failed: {e}"));
                            self.status_msg = "Save failed".to_string();
                        }
                        Err(_) => {}
                    }
                }
                AppMessage::UpdateChecked { result } => match result {
                    Ok(Some(offer)) => {
                        self.update = crate::update::UpdatePhase::Available(offer);
                    }
                    Ok(None) => {
                        self.update = crate::update::UpdatePhase::Idle;
                    }
                    Err(e) => {
                        self.update = crate::update::UpdatePhase::Failed(e);
                    }
                },
                AppMessage::UpdateProgress { downloaded, total } => {
                    if let crate::update::UpdatePhase::Downloading { progress, .. } =
                        &mut self.update
                    {
                        *progress = match total {
                            Some(total) if total > 0 => downloaded as f32 / total as f32,
                            _ => 0.0,
                        };
                    }
                }
                AppMessage::UpdateDownloaded { result } => match result {
                    Ok((offer, package_path)) => {
                        self.update = crate::update::UpdatePhase::Ready {
                            offer,
                            package_path,
                        };
                        self.status_msg = "Update downloaded — ready to install".to_string();
                        self.error = None;
                    }
                    Err(e) => {
                        // Keep the offer so the user can retry without re-checking GitHub.
                        let offer = match std::mem::take(&mut self.update) {
                            crate::update::UpdatePhase::Downloading { offer, .. } => Some(offer),
                            other => {
                                self.update = other;
                                None
                            }
                        };
                        if let Some(offer) = offer {
                            self.update = crate::update::UpdatePhase::Available(offer);
                        } else {
                            self.update = crate::update::UpdatePhase::Failed(e.clone());
                        }
                        self.error = Some(e);
                    }
                },
                AppMessage::SchemaApplied {
                    tab_id,
                    conn_id: history_conn_id,
                    sql,
                    elapsed_ms,
                    result,
                } => {
                    self.busy = Busy::Idle;
                    self.record_history(
                        dbcore::audit::AuditAction::SchemaApply,
                        &history_conn_id,
                        &sql,
                        result.is_ok(),
                        result.as_ref().err().cloned(),
                        None,
                        elapsed_ms,
                    );
                    match result {
                        Ok(msg) => {
                            self.schema_cache.remove(&history_conn_id);
                            self.status_msg = msg;
                            self.error = None;
                            self.schema_pending = None;
                            // Close the editor on the tab that applied the migration (the
                            // user may have switched tabs while it ran).
                            let source_tab = self.tabs.iter_mut().find(|t| t.id == tab_id);
                            let conn_id = source_tab.map(|tab| {
                                tab.schema_editor = None;
                                if matches!(tab.view, TabView::Structure | TabView::Indexes) {
                                    tab.view = TabView::Data;
                                }
                                tab.conn_id.clone()
                            });
                            // Re-introspect that tab's connection to refresh the sidebar tree.
                            if let Some(conn_id) =
                                conn_id.flatten().or_else(|| self.tab().conn_id.clone())
                            {
                                if let Some(ac) = self
                                    .active_connections
                                    .iter()
                                    .find(|c| c.config_id == conn_id)
                                {
                                    let db = ac.db.clone();
                                    let tx = self.tx.clone();
                                    self.rt.spawn(async move {
                                        let started = Instant::now();
                                        let result = match tokio::time::timeout(
                                            std::time::Duration::from_secs(30),
                                            db.introspect(),
                                        )
                                        .await
                                        {
                                            Ok(result) => result.map_err(|e| e.to_string()),
                                            Err(_) => {
                                                Err("schema refresh timed out after 30 seconds"
                                                    .to_string())
                                            }
                                        };
                                        let _ = tx.send(AppMessage::SchemaLoaded {
                                            conn_id,
                                            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                                            result,
                                        });
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            self.error = Some(format!("Schema migration failed: {e}"));
                            self.status_msg = "Schema migration failed".to_string();
                        }
                    }
                }
            }
            self.enforce_result_memory_budget();
            ctx.request_repaint();
        }
    }
}
