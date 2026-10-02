//! Staged row edits: the edit source, undo/redo, and the commit statements.

use super::*;

pub(super) struct PendingEdits {
    pub statements: Vec<String>,
    tab_id: u64,
    conn_id: String,
    db: Arc<dyn Database>,
    source: EditSource,
    result_revision: u64,
}

impl PendingEdits {
    pub(super) fn is_sequential(&self) -> bool {
        self.db.kind().is_cql()
    }
}

/// Per-result-column constraints from the table's introspected schema. Declared lengths are
/// only enforced where the database enforces them (SQLite and DuckDB accept any length).
fn column_rules(
    kind: DbKind,
    table: &dbcore::TableInfo,
    columns: &[dbcore::ColumnMeta],
) -> Vec<crate::edit::ColumnRule> {
    let enforces_length = matches!(
        kind,
        DbKind::Postgres | DbKind::MySql | DbKind::MariaDb | DbKind::SqlServer
    );
    columns
        .iter()
        .map(|meta| {
            let Some(info) = table
                .columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&meta.name))
            else {
                return crate::edit::ColumnRule::default();
            };
            crate::edit::ColumnRule {
                not_null: !info.nullable,
                required: !info.nullable && info.default.is_none() && !info.generated,
                max_chars: if enforces_length {
                    info.char_limit()
                } else {
                    None
                },
                enum_values: info.enum_values().unwrap_or_default(),
            }
        })
        .collect()
}

fn edit_key_columns(table: &dbcore::TableInfo) -> Vec<String> {
    table
        .edit_key_candidates()
        .into_iter()
        .next()
        .map(|(_, columns)| columns)
        .unwrap_or_default()
}

impl DbGuiApp {
    pub(super) fn confirm_key_chooser(&mut self) {
        let Some(chooser) = self.key_chooser.take() else {
            return;
        };
        let Some(idx) = self.tabs.iter().position(|tab| tab.id == chooser.tab_id) else {
            return;
        };
        let Some((_, columns)) = chooser.candidates.get(chooser.selected) else {
            return;
        };
        let columns = columns.clone();
        if let Some(source) = self.tabs[idx].edits.source.as_mut() {
            source.pk_cols = columns.clone();
        }
        if let Some(source) = self.tabs[idx].edits.pending_source.as_mut() {
            source.pk_cols = columns;
        }
        self.commit_pending = None;
        self.status_msg = "Row key columns updated".into();
        self.error = None;
    }
    /// Work out whether the tab's SQL still reads one whole table, and if so build the
    /// [`EditSource`] that makes its rows editable. Matches the table (case-insensitively)
    /// against the bound connection's schema to pick up its primary key; an ambiguous bare
    /// name (same table in several schemas) or a table without a PK stays read-only.
    pub(super) fn derive_edit_source(&self, idx: usize) -> Option<EditSource> {
        let tab = self.tabs.get(idx)?;
        let conn = tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))?;
        let (schema, table) = dbcore::edits::editable_select_target(conn.db.kind(), &tab.sql)?;
        let mut matches = conn.schema.tables.iter().filter(|t| {
            t.name.eq_ignore_ascii_case(&table)
                && schema.as_deref().is_none_or(|s| {
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
            edit_key_columns(info)
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
            .map(|conn| &conn.schema)
        else {
            return;
        };
        let read_only = self
            .connections
            .iter()
            .find(|config| config.id == conn_id)
            .is_some_and(|config| config.is_read_only());

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
                        && source.schema.as_deref().is_none_or(|wanted| {
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
                    source.pk_cols = edit_key_columns(table);
                }
            }
        }
    }
    /// The introspected [`dbcore::TableInfo`] behind the tab at `idx`: the table it was
    /// opened on (loaded or still in flight), looked up in its live connection's schema.
    /// `None` for plain query tabs, while metadata is still loading, or when the connection
    /// is down. Structure/Indexes keep their mode and render a loading/unavailable state.
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
    /// Install column constraints on the visible tabs' edits once their table metadata is
    /// known. Cheap per frame: a tab is skipped once synced for its current result.
    pub(super) fn sync_edit_rules(&mut self) {
        let visible = [Some(self.active_query_tab), self.split_tab];
        for idx in visible.into_iter().flatten() {
            let Some(tab) = self.tabs.get(idx) else {
                continue;
            };
            if tab.edits.rules_synced || !tab.edits.editable() {
                continue;
            }
            let (Some(result), Some(table)) = (tab.result.as_ref(), self.structure_table(idx))
            else {
                continue;
            };
            let Some(kind) = tab
                .conn_id
                .as_deref()
                .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
                .map(|c| c.db.kind())
            else {
                continue;
            };
            let rules = column_rules(kind, table, &result.columns);
            let edits = &mut self.tabs[idx].edits;
            edits.set_rules(rules);
            edits.rules_synced = true;
        }
    }
    /// Validate staged edits and build the SQL statements, storing them in
    /// `commit_pending` to show the preview dialog. Nothing is executed yet.
    pub(super) fn commit_edits(&mut self) {
        if self.busy != Busy::Idle {
            return;
        }
        self.commit_pending = None;
        if self.tab_connection_is_read_only(self.active_query_tab) {
            self.refuse_read_only("staged edits can't be saved.");
            return;
        }
        if let Some(stmts) = self.build_commit_statements() {
            let Some(active) = self.active() else {
                return;
            };
            let tab = self.tab();
            let Some(source) = tab.edits.source.clone() else {
                return;
            };
            self.commit_pending = Some(PendingEdits {
                statements: stmts,
                tab_id: tab.id,
                conn_id: active.config_id.clone(),
                db: active.db.clone(),
                source,
                result_revision: tab.result_revision,
            });
        }
    }

    /// Select the preview's source, never the currently selected connection. A reconnect
    /// (even under the same config id), result reload or changed source invalidates it.
    pub(super) fn pending_edits_tab(&mut self) -> Option<usize> {
        let pending = self.commit_pending.as_ref()?;
        let idx = self.tabs.iter().position(|tab| tab.id == pending.tab_id);
        let valid = idx.is_some_and(|idx| {
            let tab = &self.tabs[idx];
            tab.conn_id.as_deref() == Some(pending.conn_id.as_str())
                && tab.result.is_some()
                && tab.result_revision == pending.result_revision
                && tab.edits.source.as_ref() == Some(&pending.source)
                && self.active_connections.iter().any(|conn| {
                    conn.config_id == pending.conn_id && Arc::ptr_eq(&conn.db, &pending.db)
                })
        });
        if !valid {
            self.commit_pending = None;
            self.error =
                Some("The edit source or connection changed. Preview the edits again.".into());
            return None;
        }
        let idx = idx?;
        if self.tab_connection_is_read_only(idx) {
            self.commit_pending = None;
            self.refuse_read_only("staged edits can't be saved.");
            return None;
        }
        self.active_query_tab = idx;
        Some(idx)
    }

    /// Execute the exact preview on its captured connection. CQL is sequential, not atomic.
    pub(super) fn confirm_edits(&mut self) {
        if self.busy != Busy::Idle {
            return;
        }
        let Some(_) = self.pending_edits_tab() else {
            return;
        };
        // Re-plan only the staged changes; deterministic SQL detects edits made since preview.
        let current = self.build_commit_statements();
        if current.as_ref() != self.commit_pending.as_ref().map(|p| &p.statements) {
            self.commit_pending = None;
            self.error = Some("The staged edits changed. Preview the edits again.".into());
            return;
        }
        let Some(PendingEdits {
            statements: stmts,
            db,
            conn_id,
            tab_id,
            ..
        }) = self.commit_pending.take()
        else {
            return;
        };
        let n = stmts.len();
        let tx = self.tx.clone();
        self.busy = Busy::Saving;
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
    /// Raw row ids of the active tab's selection, in display order.
    fn selected_raw_rows(&self) -> Vec<usize> {
        let tab = self.tab();
        tab.selection
            .iter()
            .filter_map(|disp| crate::edit::disp_to_raw(&tab.row_order, tab.edits.new_rows, disp))
            .collect()
    }
    /// Stage NULL / `''` into `col` on every selected row, as one undo step.
    pub(super) fn set_selected_cells(&mut self, col: usize, to: crate::edit::SetTo) {
        if !self.tab().edits.editable() {
            return;
        }
        let rows = self.selected_raw_rows();
        let tab = self.tab_mut();
        let Some(result) = tab.result.as_ref() else {
            return;
        };
        let targets: Vec<(usize, dbcore::Value)> = rows
            .into_iter()
            .filter_map(|raw| crate::edit::original_value(result, raw, col).map(|v| (raw, v)))
            .collect();
        let rejected = tab.edits.set_cells(&targets, col, to);
        self.error = None;
        if rejected > 0 {
            self.status_msg = format!("{rejected} row(s) skipped — the column is NOT NULL");
        }
        self.workspace_dirty = true;
    }
    /// Copy every selected (non-deleted) row into a new staged insert row, TablePlus-style.
    /// Key columns are left empty so the copies don't collide with their sources — an
    /// auto-increment key fills itself in, any other key must be typed before saving.
    pub(super) fn duplicate_rows(&mut self) {
        if !self.tab().edits.editable() {
            self.status_msg =
                "Duplicate needs an editable table (open one with a primary key).".into();
            return;
        }
        let rows = self.selected_raw_rows();
        let tab = self.tab_mut();
        tab.flush_active_edit();
        let Some(result) = tab.result.as_ref() else {
            return;
        };
        let pk_cols = tab
            .edits
            .source
            .as_ref()
            .map(|s| s.pk_cols.clone())
            .unwrap_or_default();
        // Snapshot the current values (staged edits win) before adding rows renumbers nothing
        // we read — new ids are appended past every existing one.
        let copies: Vec<Vec<(usize, dbcore::Value)>> = rows
            .into_iter()
            .filter(|raw| tab.edits.row_state(*raw) != crate::edit::RowState::Deleted)
            .map(|raw| {
                (0..result.column_count())
                    .filter(|&c| !pk_cols.contains(&result.columns[c].name))
                    .filter_map(|c| {
                        let value = tab
                            .edits
                            .staged(raw, c)
                            .cloned()
                            .or_else(|| crate::edit::original_value(result, raw, c))?;
                        (!value.is_null()).then_some((c, value))
                    })
                    .collect()
            })
            .collect();
        if copies.is_empty() {
            return;
        }
        let added = copies.len();
        tab.edits.begin_undo_group();
        for cells in copies {
            let id = tab.edits.add_new_row();
            for (c, value) in cells {
                tab.edits.stage(id, c, value, &dbcore::Value::Null);
            }
        }
        tab.edits.end_undo_group();
        // Select the copies (they sit just past the stored rows) and bring them into view.
        let total = tab.row_order.len() + tab.edits.new_rows;
        tab.selection.select_one(total - added);
        tab.selection.range_to(total - 1);
        tab.pending_scroll = Some(total - 1);
        self.status_msg = format!("Duplicated {added} row(s) — review, then Save to insert.");
        self.error = None;
        self.workspace_dirty = true;
    }
    /// Flush the UI editor, then delegate validation and SQL planning to the shared core.
    pub(super) fn build_commit_statements(&mut self) -> Option<Vec<String>> {
        let idx = self.active_query_tab;
        if !self.tabs[idx].flush_active_edit() {
            self.error = Some("Fix the highlighted cell before saving.".into());
            self.status_msg = "Invalid value — not saved".into();
            return None;
        }
        if !self.tabs[idx].edits.has_pending() {
            return None;
        }
        // A new row missing a NOT NULL value with no default would fail the whole
        // transaction at the database; name the cell instead.
        if let Some((slot, col)) = self.tabs[idx].edits.missing_required() {
            let column = self.tabs[idx]
                .result
                .as_ref()
                .and_then(|r| r.columns.get(col))
                .map_or("?", |c| c.name.as_str());
            self.error = Some(format!("New row {}: \"{column}\" is required.", slot + 1));
            self.status_msg = "Missing required value — not saved".into();
            return None;
        }
        let kind = self.active()?.db.kind();
        let generated_columns: Vec<usize> = self
            .structure_table(idx)
            .map(|table| {
                table
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, column)| column.generated)
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default();
        let tab = &self.tabs[idx];
        let batch = dbcore::edits::EditBatch {
            source: tab.edits.source.as_ref()?,
            result: tab.result.as_ref()?,
            cells: &tab.edits.cells,
            deleted: &tab.edits.deleted,
            new_rows: tab.edits.new_rows,
            generated_columns: &generated_columns,
        };
        match dbcore::edits::plan_edits(kind, batch) {
            Ok(plan) if !plan.statements.is_empty() => Some(plan.statements),
            Ok(_) => None,
            Err(error) => {
                self.status_msg = match &error {
                    dbcore::edits::EditError::MissingPrimaryKey(_) => {
                        "Missing primary key — not saved"
                    }
                    dbcore::edits::EditError::DuplicatePrimaryKey => {
                        "Duplicate primary key — not saved"
                    }
                    _ => "Invalid edits — not saved",
                }
                .into();
                self.error = Some(error.to_string());
                None
            }
        }
    }
}
