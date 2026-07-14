//! Moving rows in and out: clipboard copy/paste, file export, file import.

use super::*;

impl DbGuiApp {
    /// Copy the selected result rows to the clipboard in `format`. Only stored rows are copied
    /// (unsaved new rows aren't data yet); their cloned values are rendered by
    /// [`dbcore::copy_rows`] and the text is staged in `copy_buffer` for `draw` to flush.
    pub(super) fn copy_selection(&mut self, format: dbcore::CopyFormat) {
        let idx = self.active_query_tab;
        // Dialect + table identity for the SQL INSERT form (ignored by CSV/JSON).
        let kind = self
            .active()
            .map(|a| a.db.kind())
            .unwrap_or(dbcore::DbKind::Postgres);
        let tab = &self.tabs[idx];
        let Some(result) = tab.result.as_ref() else {
            return;
        };
        let order_len = tab.row_order.len();
        // Selected stored rows, in display order, cloned so no borrow of `self` outlives them.
        let rows: Vec<Vec<dbcore::Value>> = tab
            .selection
            .iter()
            .filter(|&d| d < order_len)
            .map(|d| result.rows[tab.row_order[d]].clone())
            .collect();
        if rows.is_empty() {
            return;
        }
        let columns = result.columns.clone();
        let (schema, table) = match tab
            .edits
            .source
            .as_ref()
            .or(tab.edits.pending_source.as_ref())
        {
            Some(s) => (s.schema.clone(), s.table.clone()),
            None => (None, "table".to_string()),
        };
        let row_refs: Vec<&[dbcore::Value]> = rows.iter().map(|r| r.as_slice()).collect();
        match dbcore::copy_rows(format, &columns, &row_refs, kind, schema.as_deref(), &table) {
            Some(text) => {
                self.status_msg = format!("Copied {} row(s) as {}", rows.len(), format.label());
                self.error = None;
                self.copy_buffer = Some(text);
            }
            // Only the INSERT path fails — on binary cells with no SQL literal form.
            None => self.error = Some("Can't copy binary values as SQL INSERT.".to_string()),
        }
    }
    /// Paste clipboard `text` (TSV: one row per line, tab-separated fields) into the active
    /// table as new staged insert rows — the counterpart to "Copy". Fields map to columns by
    /// position; each is typed by its column's editor kind (empty → NULL). Nothing touches the
    /// database until the user reviews and Saves. Only works on an editable (PK-bearing) table.
    pub(super) fn paste_rows(&mut self, text: &str) {
        let idx = self.active_query_tab;
        if !self.tabs[idx].edits.editable() {
            self.status_msg = "Paste needs an editable table (open one with a primary key).".into();
            return;
        }
        let ncols = match self.tabs[idx].result.as_ref() {
            Some(r) => r.column_count(),
            None => return,
        };
        // TSV → rows of fields. Skip blank lines so a trailing newline doesn't add an empty row.
        let parsed: Vec<Vec<&str>> = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split('\t').collect())
            .collect();
        if parsed.is_empty() {
            return;
        }
        let added = parsed.len();
        // One undo group so the whole paste takes a single Cmd/Ctrl+Z.
        self.tabs[idx].edits.begin_undo_group();
        for fields in parsed {
            let id = self.tabs[idx].edits.add_new_row();
            for (c, field) in fields.into_iter().enumerate().take(ncols) {
                if !field.is_empty() {
                    self.tabs[idx].edits.stage_text(id, c, field);
                }
            }
        }
        self.tabs[idx].edits.end_undo_group();
        // Select the freshly pasted rows (they sit just past the stored rows) so they're
        // highlighted and scrolled into view, ready to review before saving.
        let order_len = self.tabs[idx].row_order.len();
        let total = order_len + self.tabs[idx].edits.new_rows;
        let sel = &mut self.tabs[idx].selection;
        sel.select_one(total - added);
        sel.range_to(total - 1);
        self.status_msg = format!("Pasted {added} row(s) — review, then Save to insert.");
        self.error = None;
        self.workspace_dirty = true;
    }
    /// Export a whole table to a file. Streams every row server-side (no row cap, never
    /// materialized) straight into the chosen format, on the background runtime. Opens a save
    /// dialog seeded with the table name first; a cancelled dialog is a no-op.
    pub(super) fn export_table(&mut self, table: &TableInfo, format: dbcore::ExportFormat) {
        let Some(active) = self.active() else {
            self.error = Some("Connect to a database to export a table.".into());
            return;
        };
        let db = active.db.clone();
        let kind = db.kind();
        // SELECT * over the whole table — no LIMIT, so the stream covers every row.
        let sql = format!("SELECT * FROM {}", table.qualified(kind));
        let table_name = table.name.clone();

        let default_name = format!("{}.{}", table_name, format.extension());
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter(format.label(), &[format.extension()])
            .save_file()
        else {
            return;
        };

        let tx = self.tx.clone();
        self.status_msg = format!("Exporting {table_name}…");
        self.error = None;
        self.rt.spawn(async move {
            let result = stream_export(db.as_ref(), &sql, format, &path)
                .await
                .map(|rows| (path, rows))
                .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Exported {
                table: table_name,
                result,
            });
        });
    }
    /// Pick a CSV/JSON file and open the import dialog for `table`. Nothing is read into the
    /// database here — the dialog previews the file and lets the user map its columns first.
    /// A cancelled file dialog is a no-op.
    pub(super) fn open_import(&mut self, table: &TableInfo) {
        let Some(active) = self.active() else {
            self.error = Some("Connect to a database to import into a table.".into());
            return;
        };
        let conn_id = active.config_id.clone();
        if self.connection_is_read_only(&conn_id) {
            self.refuse_read_only("data can't be imported.");
            return;
        }

        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV or JSON", &["csv", "json"])
            .add_filter("CSV", &["csv"])
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let Some(format) = dbcore::ImportFormat::from_path(&path) else {
            self.error = Some("Import supports .csv and .json files only.".into());
            return;
        };

        // A header row is by far the common case (and what our own export writes), so start
        // there; the dialog's checkbox re-reads the file if the user disagrees.
        match dbcore::import::preview(&path, format, true, IMPORT_PREVIEW_ROWS) {
            Ok(preview) => {
                let mut draft = ImportDraft {
                    table: table.clone(),
                    conn_id,
                    path,
                    format,
                    has_header: true,
                    headers: preview.headers,
                    preview_rows: preview.rows,
                    more: preview.more,
                    mapping: Vec::new(),
                };
                draft.auto_map();
                self.error = None;
                self.import_pending = Some(draft);
            }
            Err(e) => {
                let name = path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.error = Some(format!("Could not read {name}: {e}"));
            }
        }
    }
    /// Re-read the open draft's file after the "first row is a header" checkbox changed, and
    /// re-derive the column mapping from the new headers.
    pub(super) fn reload_import_preview(&mut self) {
        let Some(draft) = self.import_pending.as_mut() else {
            return;
        };
        match dbcore::import::preview(
            &draft.path,
            draft.format,
            draft.has_header,
            IMPORT_PREVIEW_ROWS,
        ) {
            Ok(preview) => {
                draft.headers = preview.headers;
                draft.preview_rows = preview.rows;
                draft.more = preview.more;
                draft.auto_map();
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Could not read the file: {e}")),
        }
    }
    /// Read the whole file, coerce every field against its target column, and insert the rows
    /// as a single transaction on the background runtime.
    pub(super) fn confirm_import(&mut self) {
        // Snapshot what the spawned task needs, then drop the borrow so the validation below
        // can touch `self`. A failed validation leaves the dialog open with its mapping intact.
        let Some(draft) = self.import_pending.as_ref() else {
            return;
        };
        let conn_id = draft.conn_id.clone();
        let binary: Vec<String> = draft
            .binary_conflicts()
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let targets = draft.targets();
        let table = draft.table.clone();
        let file_name = draft.file_name();
        let (path, format, has_header) = (draft.path.clone(), draft.format, draft.has_header);

        // Defence in depth: the sidebar already refused, but the connection could have been
        // edited to read-only while the dialog was open.
        if self.connection_is_read_only(&conn_id) {
            self.import_pending = None;
            self.refuse_read_only("data can't be imported.");
            return;
        }
        // `EditorKind::classify` maps an unknown type (BLOB included) to Text, so a mapped
        // binary column would quietly insert a *string literal* into it. Refuse here as well as
        // in the dialog, which is only a visual gate.
        if !binary.is_empty() {
            self.error = Some(format!(
                "Binary columns can't be imported: {}. Set them to “skip”.",
                binary.join(", ")
            ));
            return;
        }
        if targets.is_empty() {
            self.error = Some("Map at least one column before importing.".into());
            return;
        }
        let Some(active) = self.active() else {
            self.error = Some("Connect to a database to import into a table.".into());
            return;
        };
        let db = active.db.clone();
        let kind = db.kind();

        let table_name = table.name.clone();
        let schema = table.schema.clone();
        let col_names: Vec<String> = targets.iter().map(|t| t.name.clone()).collect();
        // The audit trail records this summary, not the statements: a 200k-row import would
        // otherwise write hundreds of megabytes of SQL into the log.
        let summary = format!(
            "-- IMPORT INTO {} ({}) FROM {}",
            table.qualified(kind),
            col_names.join(", "),
            file_name,
        );
        self.import_pending = None;

        let tx = self.tx.clone();
        self.busy = Busy::Importing;
        self.error = None;
        self.status_msg = format!("Importing into {table_name}…");
        self.rt.spawn(async move {
            let start = std::time::Instant::now();
            let result = run_import(
                db.as_ref(),
                kind,
                &path,
                format,
                has_header,
                schema.as_deref(),
                &table_name,
                &targets,
                &col_names,
                &tx,
            )
            .await;
            let _ = tx.send(AppMessage::Imported {
                table: table_name,
                conn_id,
                sql: summary,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                result,
            });
        });
    }
}
