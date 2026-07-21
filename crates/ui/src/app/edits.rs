//! Staged row edits: the edit source, undo/redo, and the commit statements.

use super::*;

impl DbGuiApp {
    /// Work out whether the tab's SQL still reads one whole table, and if so build the
    /// [`EditSource`] that makes its rows editable. Matches the table (case-insensitively)
    /// against the bound connection's schema to pick up its primary key; an ambiguous bare
    /// name (same table in several schemas) or a table without a PK stays read-only.
    pub(super) fn derive_edit_source(&self, idx: usize) -> Option<EditSource> {
        let tab = self.tabs.get(idx)?;
        let (schema, table) = dbcore::simple_select_target(&tab.sql)?;
        let conn = tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))?;
        let mut matches = conn.schema.tables.iter().filter(|t| {
            t.name.eq_ignore_ascii_case(&table)
                && schema.as_deref().map_or(true, |s| {
                    t.schema
                        .as_deref()
                        .is_some_and(|ts| ts.eq_ignore_ascii_case(s))
                })
        });
        let Some(info) = matches.next() else {
            // A newly connected database can execute queries before its background schema
            // load has returned. Keep the parsed table identity as a read-only candidate so
            // SchemaLoaded can fill in its primary key instead of leaving this result
            // permanently non-editable merely because the query won the race.
            return conn.schema.tables.is_empty().then_some(EditSource {
                schema,
                table,
                pk_cols: Vec::new(),
            });
        };
        if matches.next().is_some() {
            return None;
        }
        // A read-only connection never gets editable rows: keep the table identity (the
        // pager, Structure view, and row count key off it) but drop the PK columns, which
        // is what `EditSource::editable()` checks. Staging, paste, and commit all follow.
        let pk_cols: Vec<String> = if self.tab_connection_is_read_only(idx) {
            Vec::new()
        } else {
            info.columns
                .iter()
                .filter(|c| c.primary_key)
                .map(|c| c.name.clone())
                .collect()
        };
        // Keep the table identity even when the table has no primary key. The result isn't
        // *editable* (`EditSource::editable()` is false for empty `pk_cols`, so the grid stays
        // read-only and no PK-less UPDATE is ever generated), but it's still a genuine table
        // tab — the pager, Structure view, and server-side row count all key off the source.
        // Dropping it here was the bug behind the pager vanishing on Next / page-size for
        // PK-less tables (e.g. imported dumps), while the sidebar-open path kept the source.
        Some(EditSource {
            schema: info.schema.clone(),
            table: info.name.clone(),
            pk_cols,
        })
    }

    /// Fill primary-key metadata into edit sources created while the connection schema was
    /// still loading. The source already carries the table identity used for the executed
    /// result, so this does not accidentally make an old result editable from newly typed SQL.
    pub(super) fn refresh_edit_sources(&mut self, conn_id: &str) {
        let Some(schema) = self
            .active_connections
            .iter()
            .find(|conn| conn.config_id == conn_id)
            .map(|conn| conn.schema.clone())
        else {
            return;
        };
        let read_only = self
            .connections
            .iter()
            .find(|config| config.id == conn_id)
            .is_some_and(|config| config.read_only);

        for tab in self
            .tabs
            .iter_mut()
            .filter(|tab| tab.conn_id.as_deref() == Some(conn_id))
        {
            for source in [&mut tab.edits.source, &mut tab.edits.pending_source]
                .into_iter()
                .filter_map(Option::as_mut)
                .filter(|source| source.pk_cols.is_empty())
            {
                let mut matches = schema.tables.iter().filter(|table| {
                    table.name.eq_ignore_ascii_case(&source.table)
                        && source.schema.as_deref().map_or(true, |wanted| {
                            table
                                .schema
                                .as_deref()
                                .is_some_and(|actual| actual.eq_ignore_ascii_case(wanted))
                        })
                });
                let Some(table) = matches.next() else {
                    continue;
                };
                if matches.next().is_some() {
                    continue;
                }
                if !read_only {
                    source.pk_cols = table
                        .columns
                        .iter()
                        .filter(|column| column.primary_key)
                        .map(|column| column.name.clone())
                        .collect();
                }
            }
        }
    }
    /// The introspected [`dbcore::TableInfo`] behind the tab at `idx`: the table it was
    /// opened on (loaded or still in flight), looked up in its live connection's schema.
    /// `None` for plain query tabs or when the connection is down — the Structure view
    /// needs this, so without it the tab falls back to Data.
    pub(super) fn structure_table(&self, idx: usize) -> Option<&dbcore::TableInfo> {
        let tab = self.tabs.get(idx)?;
        let source = tab
            .edits
            .source
            .as_ref()
            .or(tab.edits.pending_source.as_ref())?;
        let conn = tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))?;
        conn.schema.tables.iter().find(|t| {
            t.name.eq_ignore_ascii_case(&source.table)
                && match (&source.schema, &t.schema) {
                    (Some(s), Some(ts)) => s.eq_ignore_ascii_case(ts),
                    (None, _) => true,
                    (Some(_), None) => false,
                }
        })
    }
    /// Validate staged edits and build the SQL statements, storing them in
    /// `commit_pending` to show the preview dialog. Nothing is executed yet.
    pub(super) fn commit_edits(&mut self) {
        if self.tab_connection_is_read_only(self.active_query_tab) {
            self.refuse_read_only("staged edits can't be saved.");
            return;
        }
        if let Some(stmts) = self.build_commit_statements() {
            self.commit_pending = Some(stmts);
        }
    }
    /// Take the previewed statements and execute them as a single atomic transaction on
    /// the background runtime. On success the grid reloads; on failure the error is shown.
    pub(super) fn confirm_edits(&mut self) {
        if self.tab_connection_is_read_only(self.active_query_tab) {
            self.refuse_read_only("staged edits can't be saved.");
            return;
        }
        let Some(stmts) = self.commit_pending.take() else {
            return;
        };
        let (db, conn_id) = match self.active() {
            Some(active) => (active.db.clone(), active.config_id.clone()),
            None => return,
        };
        let idx = self.active_query_tab;
        let tab_id = self.tabs[idx].id;
        let n = stmts.len();
        let tx = self.tx.clone();
        self.busy = Busy::Querying;
        self.error = None;
        self.status_msg = format!("Saving {n} change(s)…");
        self.rt.spawn(async move {
            let start = std::time::Instant::now();
            let result = db
                .execute_transaction(&stmts)
                .await
                .map(|_| n)
                .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Committed {
                tab_id,
                conn_id,
                sql: stmts.join("\n"),
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                result,
            });
        });
    }
    /// Undo the last staged-edit change, refreshing the view and selection to match.
    pub(super) fn undo_edits(&mut self) {
        // Commit or drop whatever's in the open editor first — undo acts on staged state,
        // not on a half-typed buffer — then step back.
        self.tab_mut().flush_active_edit();
        if self.tab_mut().edits.undo() {
            self.tab_mut().recompute_view();
            self.status_msg = "Undo".to_string();
            self.error = None;
            self.workspace_dirty = true;
        } else {
            self.status_msg = "Nothing to undo".to_string();
        }
    }
    /// Redo the change undone most recently.
    pub(super) fn redo_edits(&mut self) {
        self.tab_mut().flush_active_edit();
        if self.tab_mut().edits.redo() {
            self.tab_mut().recompute_view();
            self.status_msg = "Redo".to_string();
            self.error = None;
            self.workspace_dirty = true;
        } else {
            self.status_msg = "Nothing to redo".to_string();
        }
    }
    /// Validate staged edits and build UPDATE/DELETE/INSERT statements. Returns `None`
    /// (and sets `self.error`) if validation fails or there is nothing to commit.
    pub(super) fn build_commit_statements(&mut self) -> Option<Vec<String>> {
        let idx = self.active_query_tab;
        // A cell still being edited with invalid (red) input blocks the whole save.
        if !self.tabs[idx].flush_active_edit() {
            self.error = Some("Fix the highlighted cell before saving.".into());
            self.status_msg = "Invalid value — not saved".to_string();
            return None;
        }
        if !self.tabs[idx].edits.has_pending() {
            return None;
        }
        // Defence in depth: every staged value must still match its column kind before we
        // build any SQL, so a malformed value can never reach the database.
        for colmap in self.tabs[idx].edits.cells.values() {
            for (&col, value) in colmap {
                if !self.tabs[idx].edits.col_kind(col).accepts(value) {
                    self.error =
                        Some("Cannot save: a cell holds a value invalid for its type.".into());
                    self.status_msg = "Invalid value — not saved".to_string();
                    return None;
                }
            }
        }
        let source = self.tabs[idx].edits.source.clone()?;
        // Grab the dialect, then drop the `active()` borrow so we can freely touch `self`.
        let kind = match self.active() {
            Some(active) => active.db.kind(),
            None => return None,
        };
        let Some(result) = &self.tabs[idx].result else {
            return None;
        };

        // Resolve each primary-key column to its position in the result set.
        let pk_idx: Option<Vec<(String, usize)>> = source
            .pk_cols
            .iter()
            .map(|name| {
                result
                    .columns
                    .iter()
                    .position(|c| &c.name == name)
                    .map(|i| (name.clone(), i))
            })
            .collect();
        let Some(pk_idx) = pk_idx else {
            self.error = Some("Cannot save: primary key columns are not in the result.".into());
            return None;
        };

        let cant_write = "Cannot save: a value can't be written (e.g. binary data).";
        let mut updates = Vec::new();
        let mut deletes = Vec::new();
        let mut inserts = Vec::new();

        // --- UPDATEs: stored rows with staged cell edits (new rows handled below) ---
        for (&row, colmap) in &self.tabs[idx].edits.cells {
            if crate::edit::is_new_row(row) || colmap.is_empty() {
                continue;
            }
            // Owned (name, value) pairs first, then borrow them for the builder.
            let sets: Vec<(String, dbcore::Value)> = colmap
                .iter()
                .map(|(&col, v)| (result.columns[col].name.clone(), v.clone()))
                .collect();
            let keys: Vec<(String, dbcore::Value)> = pk_idx
                .iter()
                .map(|(name, idx)| (name.clone(), result.rows[row][*idx].clone()))
                .collect();
            let set_refs: Vec<(&str, &dbcore::Value)> =
                sets.iter().map(|(c, v)| (c.as_str(), v)).collect();
            let key_refs: Vec<(&str, &dbcore::Value)> =
                keys.iter().map(|(c, v)| (c.as_str(), v)).collect();
            match dbcore::build_update_sql(
                kind,
                source.schema.as_deref(),
                &source.table,
                &set_refs,
                &key_refs,
            ) {
                Some(sql) => updates.push(sql),
                None => {
                    self.error = Some(cant_write.into());
                    return None;
                }
            }
        }

        // --- DELETEs: rows marked for deletion, keyed by primary key ---
        for &row in &self.tabs[idx].edits.deleted {
            let keys: Vec<(String, dbcore::Value)> = pk_idx
                .iter()
                .map(|(name, idx)| (name.clone(), result.rows[row][*idx].clone()))
                .collect();
            let key_refs: Vec<(&str, &dbcore::Value)> =
                keys.iter().map(|(c, v)| (c.as_str(), v)).collect();
            match dbcore::build_delete_sql(kind, source.schema.as_deref(), &source.table, &key_refs)
            {
                Some(sql) => deletes.push(sql),
                None => {
                    self.error = Some(cant_write.into());
                    return None;
                }
            }
        }

        // --- INSERTs: new rows, with strict primary-key validation ---
        // Existing PK tuples (excluding rows being deleted, whose keys are freed up) plus the
        // new rows already accepted, so a new row can't duplicate a live primary key.
        let existing_pks: Vec<Vec<dbcore::Value>> = (0..result.rows.len())
            .filter(|r| !self.tabs[idx].edits.deleted.contains(r))
            .map(|r| {
                pk_idx
                    .iter()
                    .map(|(_, i)| result.rows[r][*i].clone())
                    .collect()
            })
            .collect();
        let mut new_pks: Vec<Vec<dbcore::Value>> = Vec::new();
        for j in 0..self.tabs[idx].edits.new_rows {
            let id = crate::edit::NEW_ROW_BASE + j;
            // Entered (column index, value) pairs; an untouched new row is skipped entirely.
            let entered: Vec<(usize, dbcore::Value)> = self.tabs[idx]
                .edits
                .cells
                .get(&id)
                .map(|m| m.iter().map(|(&c, v)| (c, v.clone())).collect())
                .unwrap_or_default();
            if entered.is_empty() {
                continue;
            }
            // Every primary-key column must be provided and non-NULL.
            let mut pk_tuple = Vec::with_capacity(pk_idx.len());
            for (name, i) in &pk_idx {
                match entered.iter().find(|(c, _)| c == i).map(|(_, v)| v) {
                    Some(v) if !v.is_null() => pk_tuple.push(v.clone()),
                    _ => {
                        self.error = Some(format!(
                            "Cannot add row: primary key \"{name}\" is required."
                        ));
                        self.status_msg = "Missing primary key — not saved".to_string();
                        return None;
                    }
                }
            }
            // No duplicate primary keys (against live rows or other new rows).
            if existing_pks.contains(&pk_tuple) || new_pks.contains(&pk_tuple) {
                self.error = Some("Cannot add row: duplicate primary key.".into());
                self.status_msg = "Duplicate primary key — not saved".to_string();
                return None;
            }
            new_pks.push(pk_tuple);
            // Build the INSERT from every entered cell (column name → value).
            let cols_owned: Vec<(String, dbcore::Value)> = entered
                .iter()
                .map(|(c, v)| (result.columns[*c].name.clone(), v.clone()))
                .collect();
            let col_refs: Vec<(&str, &dbcore::Value)> =
                cols_owned.iter().map(|(c, v)| (c.as_str(), v)).collect();
            match dbcore::build_insert_sql(kind, source.schema.as_deref(), &source.table, &col_refs)
            {
                Some(sql) => inserts.push(sql),
                None => {
                    self.error = Some(cant_write.into());
                    return None;
                }
            }
        }

        // Run order: UPDATE, then DELETE (frees keys), then INSERT (may reuse them).
        let mut statements = updates;
        statements.extend(deletes);
        statements.extend(inserts);
        Some(statements)
    }
}
