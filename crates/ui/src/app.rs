//! The application state and the immediate-mode `update` loop.
//!
//! Threading model: the UI never blocks on database I/O. A `tokio` runtime owned by the
//! app runs connect/introspect/query work on background tasks; results come back over an
//! `mpsc` channel that we drain each frame. While work is in flight the UI stays
//! interactive and shows a spinner.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use dbcore::{
    ConnectionColor, ConnectionConfig, Database, DbKind, QueryResult, SchemaTree, TableInfo,
};

use crate::schema::{ObjectEditor, RoutineEditor, SchemaEditor, TriggerEditor, ViewEditor};

mod panels;

use crate::edit::{EditSource, Edits};
use crate::filter::{self, FilterState};
use crate::theme::ThemeRegistry;

/// The most rows a single query will materialize in memory. A `SELECT` over a bigger
/// result streams up to the cap and comes back marked truncated — browse the rest with
/// the pager (table tabs) or a narrower query. ~100k rows keeps even wide results in the
/// hundreds of MB, far below where the grid stops being useful anyway.
const MAX_FETCH_ROWS: usize = 100_000;

/// Messages sent from background tasks back to the UI thread.
enum AppMessage {
    /// A connect+introspect attempt finished.
    Connected {
        conn_id: String,
        name: String,
        /// Populated on initial connect; empty on a re-introspect (schema change).
        databases: Vec<String>,
        result: Result<(Arc<dyn Database>, SchemaTree), String>,
    },
    /// A connection test from the add/edit dialog finished.
    ConnectionTested {
        test_id: u64,
        conn_id: String,
        result: Result<(), String>,
    },
    /// A query finished. `tab_id` routes the result back to the tab that started it, even
    /// if the user has since switched tabs. `conn_id`/`sql` carry what actually ran,
    /// for the query history.
    Queried {
        tab_id: u64,
        conn_id: String,
        sql: String,
        result: Result<QueryResult, String>,
        /// True when the query was aborted via the Cancel button (a `CoreError::Canceled`),
        /// so the UI shows "Query cancelled" instead of a red error and doesn't log a failure.
        canceled: bool,
    },
    /// A batch of staged edits was saved (`Ok` carries the number of rows updated).
    Committed {
        tab_id: u64,
        conn_id: String,
        sql: String,
        elapsed_ms: f64,
        result: Result<usize, String>,
    },
    /// A background `SELECT COUNT(*)` for a paged table tab finished. Failures are
    /// non-fatal — the pager just shows an unknown total.
    Counted {
        tab_id: u64,
        result: Result<u64, String>,
    },
    /// A DDL schema migration finished. `Ok` means success; carry a status message.
    /// `tab_id` is the tab whose schema editor initiated it (to close that editor).
    SchemaApplied {
        tab_id: u64,
        conn_id: String,
        sql: String,
        elapsed_ms: f64,
        result: Result<String, String>,
    },
    /// A per-table export finished. `Ok` carries the file path and the number of rows written.
    Exported {
        table: String,
        result: Result<(std::path::PathBuf, u64), String>,
    },
    /// Rows read and coerced so far by a running import. The total is unknown — the file is
    /// streamed rather than counted first — so this drives a status line, not a percentage.
    ImportProgress { rows: usize },
    /// A file import finished. `Ok` carries the number of rows inserted. `sql` is the synthetic
    /// summary line recorded to the audit trail (the real statements can be hundreds of MB).
    Imported {
        table: String,
        conn_id: String,
        sql: String,
        elapsed_ms: f64,
        result: Result<usize, String>,
    },
    /// Background GitHub Releases check finished.
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    UpdateChecked {
        result: Result<Option<crate::update::UpdateOffer>, String>,
    },
    /// Update package download progress (bytes received, total if known).
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    UpdateProgress { downloaded: u64, total: Option<u64> },
    /// Update package download finished.
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    UpdateDownloaded {
        result: Result<(crate::update::UpdateOffer, std::path::PathBuf), String>,
    },
}

/// What the background runtime is currently doing (drives the spinner / disables buttons).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Busy {
    Idle,
    Connecting,
    Querying,
    Importing,
}

/// In-progress "save / rename favorite" dialog state: the editable name plus the snapshot of
/// what's being saved. `editing_id` is `Some` when renaming an existing favorite.
struct FavoriteDraft {
    name: String,
    sql: String,
    conn_id: Option<String>,
    conn_name: Option<String>,
    /// `Some(id)` when renaming an existing favorite; `None` when creating a new one.
    editing_id: Option<String>,
}

/// How many records of the file the import dialog shows before the user commits. Kept small:
/// the preview shares one vertical scroll with the column mapping, so a long preview buries it.
const IMPORT_PREVIEW_ROWS: usize = 10;

/// In-progress "import file into table" dialog state: the chosen file, the head of its contents,
/// and how its columns map onto the target table's. `None` on the app = no import dialog open.
struct ImportDraft {
    /// The target table, as introspected. Its column names are the only identifiers that ever
    /// reach the generated `INSERT` — never the file's header row.
    table: TableInfo,
    conn_id: String,
    path: std::path::PathBuf,
    format: dbcore::ImportFormat,
    /// Whether the file's first record names the columns. Toggling re-reads the preview.
    has_header: bool,
    /// Source column names, from the header row or synthesized (`column_1`…).
    headers: Vec<String>,
    preview_rows: Vec<dbcore::Record>,
    /// The file holds more records than `preview_rows` shows.
    more: bool,
    /// One entry per *target* column: the source field it reads, or `None` to leave it to the
    /// database's default.
    mapping: Vec<Option<usize>>,
}

impl ImportDraft {
    /// The target table for display: `schema.table`, *unquoted*. `TableInfo::qualified` returns
    /// dialect-quoted SQL (`"public"."users"`), which is right for statements and noise in a
    /// dialog title.
    fn table_label(&self) -> String {
        match &self.table.schema {
            Some(s) => format!("{s}.{}", self.table.name),
            None => self.table.name.clone(),
        }
    }

    /// The file's base name, for labels and the audit summary.
    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    /// Point each target column at the source column of the same name, case-insensitively.
    /// Columns with no match stay unmapped rather than being filled positionally — a silent
    /// off-by-one mapping is far worse than making the user pick.
    fn auto_map(&mut self) {
        self.mapping = self
            .table
            .columns
            .iter()
            .map(|col| {
                self.headers
                    .iter()
                    .position(|h| h.eq_ignore_ascii_case(&col.name))
            })
            .collect();
    }

    /// The mapped columns, in table order, ready for `coerce_row` / `build_insert_batches`.
    fn targets(&self) -> Vec<dbcore::Target> {
        self.table
            .columns
            .iter()
            .zip(&self.mapping)
            .filter_map(|(col, src)| {
                src.map(|source| dbcore::Target {
                    name: col.name.clone(),
                    kind: dbcore::EditorKind::classify(&col.data_type),
                    source,
                })
            })
            .collect()
    }

    /// Mapped columns whose type has no SQL literal form. Importing one is impossible, so the
    /// dialog blocks on it rather than letting the transaction fail halfway.
    fn binary_conflicts(&self) -> Vec<&str> {
        self.table
            .columns
            .iter()
            .zip(&self.mapping)
            .filter(|(col, src)| src.is_some() && dbcore::import::is_binary_type(&col.data_type))
            .map(|(col, _)| col.name.as_str())
            .collect()
    }

    /// Target columns that would be skipped even though the database will reject a missing
    /// value for them: `NOT NULL` and no mapping. Reported as a warning, not a hard block —
    /// the column may well have a default the introspection cannot see.
    fn unmapped_required(&self) -> Vec<&str> {
        self.table
            .columns
            .iter()
            .zip(&self.mapping)
            .filter(|(col, src)| src.is_none() && !col.nullable && !col.primary_key)
            .map(|(col, _)| col.name.as_str())
            .collect()
    }
}

/// Which view of a table tab the central panel shows: the row data, or the introspected
/// structure (columns + indexes), TablePlus-style. Only meaningful for tabs opened on a
/// table; plain query tabs always show data.
#[derive(Clone, Copy, PartialEq, Default)]
enum TabView {
    #[default]
    Data,
    Structure,
}

/// A live connection plus its introspected schema.
struct ActiveConnection {
    /// Id of the originating config; kept for reconnect/refresh in later phases.
    #[allow(dead_code)]
    config_id: String,
    name: String,
    db: Arc<dyn Database>,
    schema: SchemaTree,
    /// All databases available on this server; empty for SQLite.
    databases: Vec<String>,
}

/// One query tab: an independent SQL editor with its own result, view state, and the
/// connection it runs against. Tabs are global (a single row above the editor) but each
/// remembers its own `conn_id`, so switching tabs switches the active connection too.
struct QueryTab {
    /// Stable id, used to route async query/commit results back to the right tab.
    id: u64,
    /// The table name when this tab was opened from the schema sidebar; empty for a plain
    /// query tab (which is then labelled by position — see `tab_label`).
    title: String,
    /// A transient "preview" tab (single-click on a table): shown in italics and reused for
    /// the next previewed table. Becomes permanent when its SQL is edited or it's pinned.
    preview: bool,
    /// Saved-connection id this tab runs against (`None` ⇒ unbound).
    conn_id: Option<String>,
    sql: String,
    result: Option<QueryResult>,
    /// Indices into `result.rows` giving the current display order (filter + sort).
    row_order: Vec<usize>,
    sort: Option<(usize, bool)>,
    /// Current multi-row selection over display rows. Its `lead` drives the Details panel.
    selection: crate::grid::Selection,
    /// Staged cell edits and the editable source of the current result.
    edits: Edits,
    /// TablePlus-style result filter bar (column / operator / value conditions).
    filter: FilterState,
    /// Data vs Structure view in the central panel (table tabs only).
    view: TabView,
    /// Total rows the tab's query matches server-side (ignoring LIMIT/OFFSET), counted in
    /// the background for paged table tabs. `None` while unknown / not a table tab.
    total_rows: Option<u64>,
    /// Open schema editor (Create/Edit Table) shown in the central panel. Per-tab, so
    /// switching tabs or opening another table never leaves a stale editor on screen —
    /// and in-progress edits survive a tab switch.
    schema_editor: Option<ObjectEditor>,
    /// One-shot request to scroll this display row into view next frame (keyboard cursor
    /// moves). Consumed by `central_panel` when it renders the grid.
    pending_scroll: Option<usize>,
}

impl QueryTab {
    fn new(id: u64, title: String) -> Self {
        Self {
            id,
            title,
            preview: false,
            conn_id: None,
            sql: String::new(),
            result: None,
            row_order: Vec::new(),
            sort: None,
            selection: crate::grid::Selection::default(),
            edits: Edits::default(),
            filter: FilterState::default(),
            view: TabView::default(),
            total_rows: None,
            schema_editor: None,
            pending_scroll: None,
        }
    }

    /// Install a freshly returned result and rebuild the display order.
    fn set_result(&mut self, res: QueryResult) {
        self.sort = None;
        self.selection.clear();
        // A fresh result may have a different column count; keep filter conditions but stop
        // them indexing past the new columns, then rebuild the display order through the
        // filter (so a still-open filter bar keeps applying).
        self.filter.clamp_columns(res.column_count());
        // Classify each column once so the cell editors can be type-aware.
        self.edits.set_columns(&res.columns);
        self.result = Some(res);
        self.recompute_view();
    }

    /// Rebuild `row_order` from the current result by applying the filter, then the active
    /// sort. The single place both filtering and sorting funnel through.
    fn recompute_view(&mut self) {
        let Some(result) = &self.result else {
            self.row_order.clear();
            return;
        };
        let mut order = filter::passing_rows(result, &self.filter);
        if let Some((col, ascending)) = self.sort {
            if col < result.column_count() {
                order.sort_by(|&a, &b| {
                    let ord = result.rows[a][col].sort_cmp(&result.rows[b][col]);
                    if ascending {
                        ord
                    } else {
                        ord.reverse()
                    }
                });
            }
        }
        self.row_order = order;
        // Rows that filtered out can't stay selected; new (insert) rows live past the stored
        // rows and are still addressable, so keep them in range.
        self.selection.clamp(
            self.row_order.len() + self.edits.new_rows,
            result.column_count(),
        );
    }

    fn apply_sort(&mut self, col: usize) {
        let Some(result) = &self.result else { return };
        if col >= result.column_count() {
            return;
        }
        // Toggle ascending/descending on repeated clicks of the same column.
        let ascending = match self.sort {
            Some((c, asc)) if c == col => !asc,
            _ => true,
        };
        self.sort = Some((col, ascending));
        self.recompute_view();
    }

    /// Sort a column in an explicit direction (from the header menu, vs `apply_sort`'s toggle).
    fn set_sort(&mut self, col: usize, ascending: bool) {
        let Some(result) = &self.result else { return };
        if col >= result.column_count() {
            return;
        }
        self.sort = Some((col, ascending));
        self.recompute_view();
    }

    /// Drop the sort and return to the result's natural row order.
    fn clear_sort(&mut self) {
        if self.sort.is_none() {
            return;
        }
        self.sort = None;
        self.recompute_view();
    }

    /// Commit the cell currently being typed into the staged set. Returns `false` if its
    /// value is invalid (the editor stays open), so callers can refuse to proceed.
    fn flush_active_edit(&mut self) -> bool {
        let Some(active) = self.edits.active.as_ref() else {
            return true;
        };
        // New (insert) rows have no stored value to diff against, so they commit against NULL.
        let original = if crate::edit::is_new_row(active.row) {
            Some(dbcore::Value::Null)
        } else {
            self.result
                .as_ref()
                .and_then(|r| r.rows.get(active.row).and_then(|row| row.get(active.col)))
                .cloned()
        };
        match original {
            Some(original) => self.edits.commit_active(&original),
            None => {
                self.edits.cancel_active();
                true
            }
        }
    }
}

/// Pull the single scalar out of a `SELECT COUNT(*)` result. Backends decode big counts
/// as Int; NUMERIC-ish ones arrive as Text.
fn count_from_result(res: &QueryResult) -> Result<u64, String> {
    match res.rows.first().and_then(|row| row.first()) {
        Some(dbcore::Value::Int(n)) => Ok((*n).max(0) as u64),
        Some(dbcore::Value::Text(s)) => s.trim().parse::<u64>().map_err(|e| e.to_string()),
        _ => Err("count query returned no scalar".to_string()),
    }
}

/// Stream a whole table to `path` in `format`, returning the number of rows written. The file
/// is wrapped in a `BufWriter` and the backend streams rows straight into the format sink, so
/// the table never has to fit in memory. Runs on the background runtime.
async fn stream_export(
    db: &dyn Database,
    sql: &str,
    format: dbcore::ExportFormat,
    path: &std::path::Path,
) -> dbcore::Result<u64> {
    let file = std::fs::File::create(path)?;
    let mut sink = format.sink(std::io::BufWriter::new(file));
    db.export_query(sql, &mut *sink).await
}

/// How often the import reports progress back to the UI thread.
const IMPORT_PROGRESS_EVERY: usize = 2_000;

/// Read `path`, coerce each record against `targets`, and insert the rows as one transaction.
/// Returns the number of rows inserted. Runs on the background runtime.
///
/// Unlike [`stream_export`], this cannot stream straight through: the import is all-or-nothing,
/// so every statement must exist before the transaction opens. That is what
/// `import::MAX_IMPORT_ROWS` bounds, and the row cap is enforced *while reading* so an
/// oversized file is refused before it can exhaust memory.
#[allow(clippy::too_many_arguments)]
async fn run_import(
    db: &dyn Database,
    kind: dbcore::DbKind,
    path: &std::path::Path,
    format: dbcore::ImportFormat,
    has_header: bool,
    schema: Option<&str>,
    table: &str,
    targets: &[dbcore::Target],
    col_names: &[String],
    tx: &Sender<AppMessage>,
) -> Result<usize, String> {
    let reader = dbcore::import::read_records(path, format, has_header).map_err(|e| e.to_string())?;

    let mut rows: Vec<Vec<dbcore::Value>> = Vec::new();
    for (i, record) in reader.enumerate() {
        let record = record.map_err(|e| e.to_string())?;
        if rows.len() == dbcore::import::MAX_IMPORT_ROWS {
            return Err(format!(
                "file holds more than {} rows — split it and import in parts",
                dbcore::import::MAX_IMPORT_ROWS
            ));
        }
        // `i + 1` is the record number the user sees, header row excluded.
        rows.push(dbcore::import::coerce_row(&record, targets, format, i + 1).map_err(|e| e.to_string())?);
        if rows.len() % IMPORT_PROGRESS_EVERY == 0 {
            let _ = tx.send(AppMessage::ImportProgress { rows: rows.len() });
        }
    }
    if rows.is_empty() {
        return Err("the file has no data rows".to_string());
    }

    let names: Vec<&str> = col_names.iter().map(String::as_str).collect();
    let stmts = dbcore::import::build_insert_batches(kind, schema, table, &names, &rows)
        .map_err(|e| e.to_string())?;
    let n = rows.len();
    drop(rows);

    db.execute_transaction(&stmts)
        .await
        .map(|_| n)
        .map_err(|e| e.to_string())
}

/// Human-readable status line for a completed result.
fn result_status(res: &QueryResult) -> String {
    match res.stats.rows_affected {
        Some(n) => format!("OK — {n} row(s) affected in {:.1} ms", res.stats.elapsed_ms),
        None if res.truncated => format!(
            "First {} row(s) × {} col(s) in {:.1} ms — capped; narrow the query or page through",
            res.row_count(),
            res.column_count(),
            res.stats.elapsed_ms
        ),
        None => format!(
            "{} row(s) × {} col(s) in {:.1} ms",
            res.row_count(),
            res.column_count(),
            res.stats.elapsed_ms
        ),
    }
}

/// A sensible default name when saving a favorite: the first non-empty line of the SQL,
/// trimmed and capped so the favorites list stays scannable.
fn default_favorite_name(sql: &str) -> String {
    let line = sql
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Untitled query");
    let mut name: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        name.push('…');
    }
    name
}

fn validate_connection_test_config(
    cfg: &ConnectionConfig,
) -> std::result::Result<(), (String, Vec<ConnField>)> {
    let mut fields = Vec::new();
    if cfg.name.trim().is_empty() {
        fields.push(ConnField::Name);
    }
    if cfg.kind.is_server() {
        if cfg.host.trim().is_empty() {
            fields.push(ConnField::Host);
        }
        if cfg.port == 0 {
            fields.push(ConnField::Port);
        }
        if cfg.user.trim().is_empty() {
            fields.push(ConnField::User);
        }
        if cfg.database.trim().is_empty() {
            fields.push(ConnField::Database);
        }
    } else if cfg.sqlite_path.trim().is_empty() {
        fields.push(ConnField::SqlitePath);
    }

    if fields.is_empty() {
        Ok(())
    } else {
        Err((
            "Fill the highlighted field(s) before testing.".to_string(),
            fields,
        ))
    }
}

fn infer_connection_error_fields(message: &str, kind: DbKind) -> Vec<ConnField> {
    let msg = message.to_lowercase();
    if !kind.is_server() {
        return vec![ConnField::SqlitePath];
    }
    if msg.contains("password")
        || msg.contains("authentication")
        || msg.contains("login failed")
        || msg.contains("access denied")
        || msg.contains("role")
    {
        return vec![ConnField::User, ConnField::Password];
    }
    if msg.contains("database")
        || msg.contains("unknown database")
        || msg.contains("does not exist")
        || msg.contains("cannot open database")
    {
        return vec![ConnField::Database];
    }
    if msg.contains("port") {
        return vec![ConnField::Port];
    }
    if msg.contains("host")
        || msg.contains("dns")
        || msg.contains("name or service")
        || msg.contains("nodename")
        || msg.contains("connection refused")
        || msg.contains("connection timed out")
        || msg.contains("network")
        || msg.contains("os error")
    {
        return vec![ConnField::Host, ConnField::Port];
    }
    vec![
        ConnField::Host,
        ConnField::Port,
        ConnField::User,
        ConnField::Password,
        ConnField::Database,
    ]
}

/// State for the add/edit-connection dialog.
struct ConnEditor {
    config: ConnectionConfig,
    password: String,
    /// SSH password or key passphrase; kept out of `config` like the DB password.
    ssh_password: String,
    is_new: bool,
    /// Index in `connections` being edited (for an existing connection).
    edit_index: Option<usize>,
    test_state: ConnTestState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnField {
    Name,
    Host,
    Port,
    User,
    Password,
    Database,
    SqlitePath,
}

#[derive(Clone)]
enum ConnTestState {
    Untested,
    Testing(u64),
    Success,
    Failed {
        message: String,
        fields: Vec<ConnField>,
    },
}

/// Deferred UI actions. Collected from panel closures (which only borrow individual
/// fields) and applied afterwards with full `&mut self`, sidestepping borrow conflicts.
enum Action {
    /// Bind the active tab to a saved connection and (re)connect it.
    Connect(usize),
    /// Bind the active tab to an already-live connection (no reconnect).
    BindConnection(usize),
    /// Drop the live connection bound to the active tab.
    Disconnect,
    /// Drop a specific live connection (from its context menu).
    DisconnectConn(usize),
    /// Query-tab management.
    NewTab,
    SelectTab(usize),
    CloseTab(usize),
    CloseOtherTabs(usize),
    CloseTabsToRight(usize),
    CloseAllTabs,
    /// Pin a preview tab as permanent (double-click on the tab).
    PinTab(usize),
    /// Drag-to-reorder: move the tab at `from` so it sits at position `to`.
    MoveTab {
        from: usize,
        to: usize,
    },
    /// Drag-to-reorder: move a saved connection to a new position.
    MoveConnection {
        from: usize,
        to: usize,
    },
    NewConnection,
    EditConnection(usize),
    DeleteConnection(usize),
    /// Switch the target database for a saved connection and reconnect.
    SwitchDatabase {
        conn_idx: usize,
        database: String,
    },
    TestConnection,
    SaveConnection,
    CancelDialog,
    OpenSettings,
    CloseSettings,
    /// Show/hide the query-history side panel.
    ToggleHistory,
    /// Show/hide the Favorites panel beside the SQL editor (the Saved button).
    ToggleFavoritesPanel,
    /// Open the name dialog to save the active tab's SQL as a favorite.
    SaveCurrentAsFavorite,
    /// Open the name dialog to save a history entry (by cache index) as a favorite.
    SaveFavoriteFromHistory(usize),
    /// Open the name dialog to rename an existing favorite (by cache index).
    RenameFavorite(usize),
    /// Commit the favorite name dialog (create or rename).
    ConfirmSaveFavorite,
    /// Close the favorite name dialog without saving.
    CancelSaveFavorite,
    /// Load a favorite's SQL into the active tab (by cache index).
    UseFavorite(usize),
    /// Delete a favorite (by cache index).
    DeleteFavorite(usize),
    /// Open/close the ER diagram of the active connection (takes over the central panel).
    ToggleErd,
    /// Rebuild the open ER diagram from the current schema (after DDL / re-introspection).
    RefreshErd,
    /// Wipe the on-disk query history.
    ClearHistory,
    /// Put a history entry's SQL into the active tab's editor.
    UseHistorySql(usize),
    DismissWelcome,
    BrowseSqlitePath,
    BrowseSslCaCert,
    BrowseSslClientCert,
    BrowseSslClientKey,
    BrowseSshKey,
    RunQuery,
    /// Abort the in-flight query (Cancel button): asks the backend to kill the running
    /// statement server-side and unblocks the UI.
    CancelQuery,
    /// Reformat the active tab's SQL in its connection's dialect (Beautify, Cmd/Ctrl+I).
    BeautifySql,
    /// Open a table's rows from the sidebar. `source` makes the result editable. `pin` opens
    /// it as a permanent tab (double-click) rather than the reusable italic preview tab.
    OpenTable {
        sql: String,
        source: EditSource,
        pin: bool,
    },
    /// Show a routine/trigger's definition SQL in a preview tab (read-only; not executed).
    OpenDefinition {
        title: String,
        sql: String,
    },
    /// Follow a foreign key from a grid cell: open a preview tab on the referenced table,
    /// filtered to the key the cell points at. `row`/`col` index the active tab's result.
    FollowForeignKey {
        row: usize,
        col: usize,
    },
    SortBy(usize),
    /// Header menu: sort a column in an explicit direction (vs `SortBy`, which toggles).
    SetSort {
        col: usize,
        asc: bool,
    },
    /// Header menu: drop the sort, back to natural row order.
    ClearSort,
    /// Pager: jump to another page of a paged table tab. Rewrites the tab's LIMIT/OFFSET
    /// in place (the SQL editor always shows what runs) and re-runs the query.
    Page(PageNav),
    /// Pager: switch the page size, staying on the page that holds the current offset.
    SetPageSize(u64),
    /// Copy the currently selected result rows to the clipboard in the given format.
    CopyRows(dbcore::CopyFormat),
    /// Paste clipboard text (TSV) into the active editable table as new (staged) insert rows.
    PasteRows(String),
    /// Export an entire table (every row, streamed server-side) to a file in the chosen
    /// format, after picking a path from a save dialog. Triggered from the sidebar.
    ExportTable {
        table: TableInfo,
        format: dbcore::ExportFormat,
    },
    /// Pick a CSV/JSON file and open the import mapping dialog for this table. Sidebar action.
    ImportIntoTable(TableInfo),
    /// Point a target column at a source column of the open import (or `None` to skip it).
    SetImportMapping {
        target: usize,
        source: Option<usize>,
    },
    /// Re-run the by-name auto-mapping, discarding manual choices.
    AutoMapImport,
    /// Unmap every target column.
    ClearImportMapping,
    /// Flip the open import's "first row is a header" switch and re-read the file's head.
    SetImportHasHeader(bool),
    /// User confirmed the import: read the file and insert every row in one transaction.
    ConfirmImport,
    CancelImport,
    /// Build staged edits into SQL and open the preview dialog.
    PreviewEdits,
    /// Undo / redo the last staged-edit change (cell edit, delete mark, new row, fill, paste).
    Undo,
    Redo,
    /// User confirmed the preview: execute the statements transactionally.
    ConfirmEdits,
    /// User cancelled the preview dialog without committing.
    CancelEdits,
    /// User confirmed running destructive SQL on a production connection.
    ConfirmDangerQuery,
    /// User backed out of the production-confirmation dialog.
    CancelDangerQuery,
    /// Open the schema editor to create a brand-new table.
    OpenNewTable,
    /// Open the schema editor to modify an existing table.
    OpenEditTable(TableInfo),
    /// Stage a `CREATE TABLE … AS`/clone migration for a sidebar table (opens the DDL preview).
    CloneTable(TableInfo),
    /// Stage a `TRUNCATE`/empty-rows migration for a sidebar table (opens the DDL preview).
    TruncateTable(TableInfo),
    /// Stage a `DROP TABLE` migration for a sidebar table (opens the DDL preview).
    DropTable(TableInfo),
    /// Pin/unpin a table in the schema explorer (toggles its "Pinned" bookmark for the
    /// active connection). Carries the table's schema and bare name.
    ToggleBookmark {
        schema: Option<String>,
        table: String,
    },
    /// Open the object editor to create a brand-new view.
    OpenNewView,
    /// Open the object editor to modify an existing view.
    OpenEditView(dbcore::ViewInfo),
    /// Stage a `DROP VIEW` migration for a sidebar view (opens the DDL preview).
    DropView(dbcore::ViewInfo),
    /// Open the object editor to create a brand-new trigger.
    OpenNewTrigger,
    /// Open the object editor to modify an existing trigger.
    OpenEditTrigger(dbcore::TriggerInfo),
    /// Stage a `DROP TRIGGER` migration for a sidebar trigger (opens the DDL preview).
    DropTrigger(dbcore::TriggerInfo),
    /// Open the object editor to create a new function or procedure.
    OpenNewRoutine(dbcore::RoutineKind),
    /// Open the object editor to modify an existing function or procedure.
    OpenEditRoutine(dbcore::RoutineInfo),
    /// Stage a `DROP FUNCTION/PROCEDURE` migration (opens the DDL preview).
    DropRoutine(dbcore::RoutineInfo),
    /// Validate editor state and move to the DDL-preview dialog.
    GenerateSchema,
    /// User confirmed the DDL preview: execute the statements and re-introspect.
    ApplySchema,
    /// Close the schema editor / DDL preview without applying.
    CancelSchema,
    /// Open the in-app update dialog.
    OpenUpdateDialog,
    CloseUpdateDialog,
    /// Dismiss the current update offer for this session.
    DismissUpdate,
    /// Download the offered release DMG.
    DownloadUpdate,
    /// Replace the installed app and relaunch (macOS).
    InstallUpdate,
    /// Dismiss the What's New dialog.
    DismissWhatsNew,
}

/// Where the pager should jump.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageNav {
    First,
    Prev,
    Next,
    /// Only offered when the total row count is known.
    Last,
}

/// Drag-to-reorder state for the query-tab strip.
#[derive(Clone, Copy)]
struct TabDrag {
    /// Stable id of the tab being dragged (ids survive the index changing mid-drag).
    id: u64,
    /// Pointer x-offset from the chip's left edge at grab time, so the floating chip
    /// keeps the grab point under the cursor instead of snapping its centre there.
    grab_x: f32,
}

/// Drag-to-reorder state for the vertical saved-connection strip.
#[derive(Clone)]
struct ConnectionDrag {
    /// Stable id of the connection being dragged.
    id: String,
    /// Pointer y-offset from the chip's top edge at grab time.
    grab_y: f32,
}

/// One recently-run statement in the ghost-text pool, tagged with the connection it ran
/// against so a suggestion only ever completes from the active tab's own database.
struct PooledQuery {
    conn_id: String,
    sql: String,
}

pub struct DbGuiApp {
    // --- persisted config ---
    connections: Vec<ConnectionConfig>,

    // --- async plumbing ---
    rt: tokio::runtime::Runtime,
    tx: Sender<AppMessage>,
    rx: Receiver<AppMessage>,
    busy: Busy,
    next_connection_test_id: u64,
    /// Tab id of the in-flight `SELECT` (cleared when [`AppMessage::Queried`] arrives).
    querying_tab_id: Option<u64>,
    /// Cancellation handle for the in-flight query; firing it asks the backend to abort and
    /// kill the server-side statement. `None` when no query is running.
    query_cancel: Option<tokio_util::sync::CancellationToken>,

    // --- connection state ---
    /// Pool of live connections (one per connected config), shared across tabs.
    active_connections: Vec<ActiveConnection>,

    // --- query tabs ---
    /// Open query tabs. Always non-empty.
    tabs: Vec<QueryTab>,
    active_query_tab: usize,
    /// Monotonic id source for new tabs.
    next_tab_id: u64,

    // --- workspace persistence ---
    /// Set when tabs/SQL/bindings change; flushed to disk on a throttle (see `draw`).
    workspace_dirty: bool,
    last_workspace_save: std::time::Instant,

    // --- transient UI state ---
    editor: Option<ConnEditor>,
    /// Live drag-to-reorder state for a query tab (cleared on mouse release).
    tab_drag: Option<TabDrag>,
    /// Live drag-to-reorder state for a saved connection (cleared on mouse release).
    connection_drag: Option<ConnectionDrag>,
    /// Details panel: live column-name filter (the "Search for field…" box).
    details_filter: String,
    /// Details panel: the (row, col) cell with an inline date picker open, opened from
    /// the value box's actions menu.
    details_date_pick: Option<(usize, usize)>,
    settings_open: bool,
    schema_filter: String,
    /// SQL editor autocomplete (table/column/keyword popup). Transient; not persisted.
    autocomplete: crate::autocomplete::State,
    /// Recently-run, successful SQL — the pool the editor's ghost-text autosuggestion
    /// matches a prefix against (fish-shell style). Seeded from history on launch and
    /// appended to as queries run. In-memory only. Each entry carries its `conn_id` so the
    /// suggestion is filtered to the active tab's database (never another connection's SQL).
    suggest_pool: Vec<PooledQuery>,
    /// The ghost-text remainder shown after the caret last frame (the text Tab accepts),
    /// or `None` when nothing is suggested. Cached: recomputed only when the SQL or caret
    /// changes, then repainted each frame (so scrolling a focused editor doesn't re-scan
    /// history/schema every frame).
    ghost_suggestion: Option<String>,
    /// `(sql char-length, caret char index)` the cached `ghost_suggestion` was computed for;
    /// a mismatch is what triggers a recompute.
    ghost_key: Option<(usize, usize)>,
    status_msg: String,
    error: Option<String>,
    /// Text staged for the OS clipboard this frame (e.g. copied result rows). Flushed to the
    /// clipboard at the end of `draw`, where the egui `Context` is available.
    copy_buffer: Option<String>,
    /// Rasterizes colour emoji from the OS font for inline display in grid cells (lazy; macOS).
    emoji: crate::emoji::EmojiAtlas,
    /// SQL statements staged for the commit-preview dialog. `None` = dialog closed;
    /// `Some(stmts)` = dialog open, waiting for the user to confirm or cancel.
    commit_pending: Option<Vec<String>>,
    /// DDL statements staged for the schema-preview dialog. `None` = preview closed.
    /// (The schema editor itself lives on each [`QueryTab`].)
    schema_pending: Option<Vec<String>>,
    /// Destructive statements found when running a query against a production
    /// connection, held for the confirmation dialog. `None` = dialog closed.
    danger_pending: Option<Vec<dbcore::safety::DangerousStatement>>,
    /// Open "import file into table" dialog, with its column mapping. `None` = dialog closed.
    import_pending: Option<ImportDraft>,
    /// Record executed statements to the on-disk query history (settings toggle).
    history_enabled: bool,
    /// Record connections and statements to the append-only audit trail (settings toggle).
    audit_enabled: bool,
    /// Check GitHub for a newer release at launch (settings toggle) — the app's only
    /// network call apart from the databases the user connects to.
    update_check_enabled: bool,
    /// Whether the right-hand panel (History / Favorites) is open; the caches below hold its
    /// rows while it is.
    history_open: bool,
    history_cache: Vec<dbcore::history::HistoryEntry>,
    /// All saved queries, kept in memory and mirrored to `favorites.json` on every change.
    /// Loaded once at startup so the Saved button's count is correct even before it opens.
    favorites_cache: Vec<dbcore::Favorite>,
    /// Pinned tables, kept in memory and mirrored to `bookmarks.json` on every change. Loaded
    /// once at startup so the schema explorer's "Pinned" group is populated from launch.
    bookmarks: Vec<dbcore::Bookmark>,
    /// Whether the Favorites panel (beside the SQL editor, toggled by the Saved button) is open.
    favorites_open: bool,
    /// Open name-this-favorite dialog. `None` = closed.
    favorite_pending: Option<FavoriteDraft>,
    /// Open ER diagram (takes over the central panel, like the schema editor).
    /// A snapshot of the schema it was built from; not persisted.
    erd: Option<crate::erd::ErDiagram>,

    // --- layout ---
    show_connection_tabs: bool,
    show_schema_panel: bool,
    show_details_panel: bool,
    show_query_console: bool,

    // --- preferences ---
    /// Stable key of the currently selected colour theme (persisted to settings.json).
    /// Resolved against [`themes`](Self::themes) — built-in key or a custom theme's file stem.
    theme: String,
    /// All themes the picker can offer: built-ins plus user-installed `*.json` files.
    themes: ThemeRegistry,
    /// SQL beautifier preferences (persisted to settings.json).
    beautify: crate::format::BeautifyPrefs,

    // --- first-run ---
    /// Show the welcome screen (true only on first launch; cleared when user clicks "Get Started").
    show_welcome: bool,

    // --- in-app updates (macOS) ---
    update: crate::update::UpdatePhase,
    update_dialog_open: bool,
    /// Version the user dismissed; hide the tab-bar badge until a newer one appears.
    update_dismissed: Option<String>,
    /// Set when the updater should close the window after scheduling install.
    pending_quit: bool,
    /// Show the What's New dialog (true when the app version is newer than last seen).
    show_whats_new: bool,
}

impl DbGuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Build state first: `construct` activates the saved theme, which `apply` then reads.
        let mut app = Self::construct();
        // Restore the saved workspace (open tabs + their SQL/connection binding). Kept out of
        // `construct` so tests get a deterministic single-tab app independent of disk state.
        app.restore_workspace();

        // Seed the editor's ghost-text autosuggestion pool from the on-disk history (only
        // statements that ran cleanly). Kept out of `construct` for the same reason as the
        // workspace — tests stay off the user's real history file.
        app.suggest_pool = dbcore::history::load(dbcore::history::MAX_ENTRIES)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.ok)
            .map(|e| PooledQuery {
                conn_id: e.conn_id,
                sql: e.sql,
            })
            .collect();

        // Load saved queries so the Favorites toolbar count is right from launch. Also kept
        // out of `construct` so tests don't read the user's favorites file.
        app.favorites_cache = dbcore::favorites::load().unwrap_or_default();

        // Load pinned tables so the explorer's "Pinned" group is right from launch. Kept out
        // of `construct` so tests don't read the user's bookmarks file.
        app.bookmarks = dbcore::bookmarks::load().unwrap_or_default();

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if crate::update::automatic_updates_supported() && app.update_check_enabled {
            app.start_update_check();
        }

        // Theme + SVG icon loader (Iconoir icons are embedded SVGs).
        crate::style::apply(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        // `egui_extras::Table` can emit a spurious "ID clash" warning (a flashing red
        // outline) while scrolling fast on HiDPI/retina displays — its virtualized cells'
        // rects round to slightly different pixels between egui's interaction passes. Our
        // own widgets are verified clash-free by the headless probes in this module's
        // tests, so we turn off this debug-only diagnostic (it never fires in release).
        cc.egui_ctx.options_mut(|o| o.warn_on_id_clash = false);

        app
    }

    /// Build the app state without touching an egui context (used by `new` and tests).
    ///
    /// Side effect: activates the saved theme via [`crate::theme::set_current`] so a later
    /// [`crate::style::apply`] renders in it. This is a thread-local, no context needed.
    fn construct() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let connections = dbcore::config::load_connections().unwrap_or_default();

        // Restore the saved theme (falling back to the default), and make it active.
        // The registry merges the built-ins with any user-installed theme files on disk.
        let settings = dbcore::config::load_settings();
        let themes = ThemeRegistry::load();
        let theme = themes.resolve_key(settings.theme.as_deref().unwrap_or(""));
        crate::theme::set_current(themes.theme_of(&theme));
        let beautify_defaults = crate::format::BeautifyPrefs::default();
        let beautify = crate::format::BeautifyPrefs {
            uppercase: settings
                .beautify_uppercase
                .unwrap_or(beautify_defaults.uppercase),
            indent: settings.beautify_indent.unwrap_or(beautify_defaults.indent),
        };
        let show_welcome = !settings.welcomed.unwrap_or(false);
        let mut show_whats_new = false;
        if !show_welcome {
            let last_seen = settings.last_seen_version.as_deref().unwrap_or("0.0.0");
            if crate::update::version_gt(crate::update::CURRENT_VERSION, last_seen) {
                show_whats_new = true;
                let mut new_settings = settings.clone();
                new_settings.last_seen_version = Some(crate::update::CURRENT_VERSION.to_string());
                let _ = dbcore::config::save_settings(&new_settings);
            }
        }
        let history_enabled = settings.history_enabled.unwrap_or(true);
        let audit_enabled = settings.audit_enabled.unwrap_or(true);
        let update_check_enabled = settings.update_check_enabled.unwrap_or(true);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        // Start with a single default query tab. The saved workspace (if any) is layered on
        // top later by `restore_workspace`, called from `new` (not here, so tests are
        // deterministic and don't read the user's config dir).
        let mut default_tab = QueryTab::new(0, String::new());
        default_tab.sql = "SELECT 1;".to_string();

        Self {
            connections,
            rt,
            tx,
            rx,
            busy: Busy::Idle,
            next_connection_test_id: 1,
            querying_tab_id: None,
            query_cancel: None,
            active_connections: Vec::new(),
            tabs: vec![default_tab],
            active_query_tab: 0,
            next_tab_id: 1,
            workspace_dirty: false,
            last_workspace_save: std::time::Instant::now(),
            editor: None,
            tab_drag: None,
            connection_drag: None,
            details_filter: String::new(),
            details_date_pick: None,
            settings_open: false,
            schema_filter: String::new(),
            autocomplete: crate::autocomplete::State::default(),
            suggest_pool: Vec::new(),
            ghost_suggestion: None,
            ghost_key: None,
            status_msg: "Ready".to_string(),
            error: None,
            copy_buffer: None,
            emoji: crate::emoji::EmojiAtlas::default(),
            show_connection_tabs: true,
            show_schema_panel: true,
            show_details_panel: true,
            show_query_console: true,
            theme,
            themes,
            beautify,
            commit_pending: None,
            schema_pending: None,
            danger_pending: None,
            import_pending: None,
            history_enabled,
            audit_enabled,
            update_check_enabled,
            history_open: false,
            history_cache: Vec::new(),
            // Loaded from disk in `new` (this builder stays config-dir-free for tests).
            favorites_cache: Vec::new(),
            bookmarks: Vec::new(),
            favorites_open: false,
            favorite_pending: None,
            erd: None,
            show_welcome,
            update: crate::update::UpdatePhase::Idle,
            update_dialog_open: false,
            update_dismissed: None,
            pending_quit: false,
            show_whats_new,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn start_update_check(&mut self) {
        if !matches!(self.update, crate::update::UpdatePhase::Idle) {
            return;
        }
        self.update = crate::update::UpdatePhase::Checking;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = crate::update::check_for_update().await;
            let _ = tx.send(AppMessage::UpdateChecked { result });
        });
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn start_update_download(&mut self) {
        let offer = match &self.update {
            crate::update::UpdatePhase::Available(o) => o.clone(),
            crate::update::UpdatePhase::Ready { offer, .. } => offer.clone(),
            _ => return,
        };
        self.update = crate::update::UpdatePhase::Downloading {
            offer: offer.clone(),
            progress: 0.0,
        };
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = crate::update::download_update(&offer, |downloaded, total| {
                let _ = tx.send(AppMessage::UpdateProgress { downloaded, total });
            })
            .await
            .map(|path| (offer, path))
            .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::UpdateDownloaded { result });
        });
    }

    fn tab(&self) -> &QueryTab {
        &self.tabs[self.active_query_tab]
    }

    fn tab_mut(&mut self) -> &mut QueryTab {
        &mut self.tabs[self.active_query_tab]
    }

    /// Replace the tabs with the saved workspace, if one exists. We never auto-connect or
    /// auto-run — tabs come back with their connection selected but idle.
    fn restore_workspace(&mut self) {
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

    /// Path string for the unified title-bar breadcrumb.
    fn breadcrumb_text(&self) -> String {
        let Some(active) = self.active() else {
            if let Some(id) = self.tab().conn_id.as_deref() {
                if let Some(cfg) = self.connections.iter().find(|c| c.id == id) {
                    return format!("{} | {} — not connected", cfg.name, cfg.kind.label());
                }
            }
            return "No connection".to_string();
        };

        let mut path = format!(
            "{} | {} : {}",
            active.name,
            active.db.kind().label(),
            active.schema.database_name,
        );

        if let Some(source) = &self.tab().edits.source {
            let table = match &source.schema {
                Some(schema) => format!("{schema}.{}", source.table),
                None => source.table.clone(),
            };
            path.push_str(&format!(" : {table}"));
        }

        path
    }

    /// Switch the active theme, re-apply the egui style, and persist the choice.
    fn set_theme(&mut self, ctx: &egui::Context, key: String) {
        crate::theme::set_current(self.themes.theme_of(&key));
        self.theme = key;
        crate::style::apply(ctx);
        self.persist_settings();
    }

    /// Flush all settings.json-backed preferences (theme, beautifier, welcomed) to disk.
    fn persist_settings(&mut self) {
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

    /// Reformat the active tab's SQL in the dialect of its live connection (generic SQL
    /// when disconnected). Token-preserving, so the query's meaning never changes.
    fn beautify_sql(&mut self) {
        let kind = self.active().map(|a| a.db.kind());
        let prefs = self.beautify;
        let tab = self.tab_mut();
        if tab.sql.trim().is_empty() {
            return;
        }
        let pretty = crate::format::beautify(&tab.sql, kind, prefs);
        if pretty != tab.sql {
            tab.sql = pretty;
            // Only whitespace/keyword-case changed, so the result grid still matches the
            // SQL — staged edits and editability are deliberately left untouched.
            self.workspace_dirty = true;
            self.status_msg = "Query beautified".to_string();
        }
    }

    /// The live connection the active tab is bound to, if it's currently connected.
    fn active(&self) -> Option<&ActiveConnection> {
        let id = self.tab().conn_id.as_deref()?;
        self.active_connections.iter().find(|c| c.config_id == id)
    }

    /// Saved config the active tab is bound to, regardless of live connection state.
    fn active_connection_config(&self) -> Option<&ConnectionConfig> {
        let id = self.tab().conn_id.as_deref()?;
        self.connections.iter().find(|c| c.id == id)
    }

    /// Rebuild the open ER diagram from its connection's current schema, keeping the
    /// user's pan/zoom and the position of every node whose table survived.
    fn refresh_erd(&mut self) {
        let Some(old) = self.erd.take() else { return };
        let Some(conn) = self
            .active_connections
            .iter()
            .find(|c| c.config_id == old.conn_id)
        else {
            // Connection dropped while the diagram was open: nothing to rebuild from.
            return;
        };
        let mut fresh = crate::erd::ErDiagram::build(&old.conn_id, &conn.schema);
        for node in &mut fresh.nodes {
            if let Some(prev) = old.nodes.iter().find(|n| n.title == node.title) {
                node.pos = prev.pos;
            }
        }
        fresh.scene_rect = old.scene_rect;
        self.erd = Some(fresh);
    }

    fn active_title_bar_color(&self) -> Option<ConnectionColor> {
        self.active_connection_config()?.title_bar_color
    }

    // --- query-tab management ---------------------------------------------

    fn new_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        // Untitled (labelled by position in the bar); inherit the current tab's connection so
        // a new tab is ready to query the same db.
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = self.tab().conn_id.clone();
        self.tabs.push(tab);
        self.active_query_tab = self.tabs.len() - 1;
        self.status_msg = "New query tab".to_string();
        self.error = None;
        self.workspace_dirty = true;
    }

    /// Display label for the tab at `idx`: the table name for a table tab, otherwise its
    /// position ("Query 1", "Query 2", …) — so numbers stay small and reuse on close.
    fn tab_label(&self, idx: usize) -> String {
        match self.tabs.get(idx) {
            Some(tab) if !tab.title.trim().is_empty() => tab.title.clone(),
            _ => format!("Query {}", idx + 1),
        }
    }

    /// Icon kind for the tab strip: table tabs carry a sidebar table name; the rest are
    /// plain query editors.
    fn tab_kind(&self, idx: usize) -> crate::components::QueryTabKind {
        if self
            .tabs
            .get(idx)
            .is_some_and(|t| !t.title.trim().is_empty())
        {
            crate::components::QueryTabKind::Table
        } else {
            crate::components::QueryTabKind::Query
        }
    }

    fn select_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.active_query_tab = idx;
        // Reflect the newly-shown tab's last result in the status line.
        self.status_msg = match &self.tabs[idx].result {
            Some(res) => result_status(res),
            None => "Ready".to_string(),
        };
        self.error = None;
        self.workspace_dirty = true;
    }

    /// Move the tab at `from` so it sits at position `to` (drag-to-reorder). The active
    /// tab stays the same logical tab — only its position changes.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let active_id = self.tab().id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        if let Some(idx) = self.tabs.iter().position(|t| t.id == active_id) {
            self.active_query_tab = idx;
        }
        self.workspace_dirty = true;
    }

    /// Move a saved connection to a new slot and persist the list order.
    fn move_connection(&mut self, from: usize, to: usize) {
        if from == to || from >= self.connections.len() || to >= self.connections.len() {
            return;
        }
        let conn = self.connections.remove(from);
        self.connections.insert(to, conn);
        if let Err(e) = dbcore::config::save_connections(&self.connections) {
            self.error = Some(e.to_string());
        }
    }

    fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.reset_to_single_tab(self.tabs[0].conn_id.clone());
        } else {
            self.tabs.remove(idx);
            if self.active_query_tab > idx || self.active_query_tab >= self.tabs.len() {
                self.active_query_tab = self.active_query_tab.saturating_sub(1);
            }
        }
        self.error = None;
        self.workspace_dirty = true;
    }

    /// Replace all tabs with one blank scratch tab (keeps the given connection binding).
    fn reset_to_single_tab(&mut self, conn_id: Option<String>) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = QueryTab::new(id, String::new());
        tab.conn_id = conn_id;
        self.tabs = vec![tab];
        self.active_query_tab = 0;
        self.status_msg = "Ready".to_string();
    }

    fn close_other_tabs(&mut self, keep_idx: usize) {
        if keep_idx >= self.tabs.len() || self.tabs.len() <= 1 {
            return;
        }
        let kept_id = self.tabs[keep_idx].id;
        self.tabs.retain(|t| t.id == kept_id);
        self.active_query_tab = 0;
        self.error = None;
        self.status_msg = "Ready".to_string();
        self.workspace_dirty = true;
    }

    fn close_tabs_to_right(&mut self, idx: usize) {
        if idx >= self.tabs.len() || idx + 1 >= self.tabs.len() {
            return;
        }
        self.tabs.truncate(idx + 1);
        if self.active_query_tab > idx {
            self.active_query_tab = idx;
        }
        self.error = None;
        self.workspace_dirty = true;
    }

    fn close_all_tabs(&mut self) {
        let conn_id = self.tab().conn_id.clone();
        self.reset_to_single_tab(conn_id);
        self.error = None;
        self.workspace_dirty = true;
    }

    /// Open a table (from the schema sidebar) as a named tab.
    ///
    /// - If the table is already open in a tab, just switch to it (no duplicate).
    /// - Otherwise show it in the reusable italic *preview* tab (single-click): one preview
    ///   slot is reused as you click through tables, so they don't pile up. A blank scratch
    ///   tab is upgraded into that preview slot rather than spawning a new tab.
    /// - `pin` (double-click) makes the tab permanent (non-italic) instead.
    fn open_table(&mut self, sql: String, source: EditSource, pin: bool) {
        let same = |s: &EditSource| s.table == source.table && s.schema == source.schema;
        // Already open (loaded or in-flight)? Activate it, pinning if asked.
        if let Some(idx) = self.tabs.iter().position(|t| {
            t.edits
                .source
                .as_ref()
                .or(t.edits.pending_source.as_ref())
                .is_some_and(same)
        }) {
            if pin {
                self.tabs[idx].preview = false;
            }
            self.select_tab(idx);
            // Result is cleared on disconnect; re-selecting the same table from the sidebar
            // must re-run the preview query instead of leaving an empty grid.
            if self.tabs[idx].result.is_none()
                && self.tabs[idx]
                    .conn_id
                    .as_deref()
                    .is_some_and(|cid| self.active_connections.iter().any(|c| c.config_id == cid))
            {
                self.start_query_for(idx);
            }
            return;
        }

        self.open_in_preview_slot(sql, source, !pin);
    }

    /// Load `sql` into a table tab bound to `source` and run it. Picks the reusable preview
    /// slot, else a blank scratch active tab, else a fresh tab (see [`Self::preview_target_slot`]),
    /// then rebuilds that tab from scratch — clearing any previous preview's result/filter/edits —
    /// while keeping its stable id and connection binding. Shared by [`Self::open_table`] and
    /// foreign-key follow. `preview` marks the tab as the transient (italic, reusable) preview.
    fn open_in_preview_slot(&mut self, sql: String, source: EditSource, preview: bool) {
        let idx = self.preview_target_slot();
        let id = self.tabs[idx].id;
        let conn_id = self.tabs[idx].conn_id.clone();
        let mut tab = QueryTab::new(id, source.table.clone());
        tab.conn_id = conn_id;
        tab.sql = sql;
        tab.preview = preview;
        tab.edits.pending_source = Some(source);
        self.tabs[idx] = tab;
        self.active_query_tab = idx;
        self.workspace_dirty = true;
        self.start_query_for(idx);
    }

    /// Follow the foreign key that column `col` of the active table tab's row `row` participates
    /// in: open a preview tab on the referenced table, filtered to the key the cell points at.
    /// `row`/`col` index the *raw* result (not the display order). A no-op with a status hint
    /// when the column isn't a foreign key or its value is NULL (nothing to navigate to).
    fn follow_foreign_key(&mut self, row: usize, col: usize) {
        let idx = self.active_query_tab;
        match self.build_fk_follow(idx, row, col) {
            Some((sql, source)) => self.open_in_preview_slot(sql, source, true),
            None => {
                self.status_msg =
                    "No foreign key to follow here (or the value is empty).".to_string();
            }
        }
    }

    /// Resolve the foreign key column `col` (raw result column) belongs to on the tab at `idx`,
    /// and build a `SELECT … WHERE <ref key = cell value>` on the referenced table plus its edit
    /// source. `row` is the raw result-row index. Returns `None` when the column isn't a foreign
    /// key, the referenced value is wholly NULL, or the connection/columns can't be resolved.
    fn build_fk_follow(&self, idx: usize, row: usize, col: usize) -> Option<(String, EditSource)> {
        let tab = self.tabs.get(idx)?;
        let result = tab.result.as_ref()?;
        let conn = tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))?;
        let kind = conn.db.kind();
        let info = self.structure_table(idx)?;
        let col_name = &result.columns.get(col)?.name;
        // The FK this column takes part in (first match; a column is rarely in more than one).
        let fk = info
            .foreign_keys
            .iter()
            .find(|fk| fk.columns.iter().any(|c| c.eq_ignore_ascii_case(col_name)))?;
        // Pair each referenced column with the value from this row's matching referencing column.
        let mut keys: Vec<(&str, &dbcore::Value)> = Vec::with_capacity(fk.ref_columns.len());
        for (referencing, referenced) in fk.columns.iter().zip(fk.ref_columns.iter()) {
            let ci = result
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(referencing))?;
            keys.push((referenced.as_str(), result.rows.get(row)?.get(ci)?));
        }
        // A wholly-NULL foreign key references nothing — nothing to navigate to.
        if keys.iter().all(|(_, v)| v.is_null()) {
            return None;
        }
        let sql = dbcore::build_select_where_sql(
            kind,
            fk.ref_schema.as_deref(),
            &fk.ref_table,
            &keys,
            100,
        )?;
        // Edit source for the referenced table: its PK columns make the opened rows editable —
        // empty on a read-only connection, exactly like the sidebar and `derive_edit_source` paths.
        let ref_info = conn.schema.tables.iter().find(|t| {
            t.name.eq_ignore_ascii_case(&fk.ref_table)
                && match (&fk.ref_schema, &t.schema) {
                    (Some(s), Some(ts)) => s.eq_ignore_ascii_case(ts),
                    (None, _) => true,
                    (Some(_), None) => false,
                }
        });
        let pk_cols = match ref_info {
            Some(t) if !self.tab_connection_is_read_only(idx) => t
                .columns
                .iter()
                .filter(|c| c.primary_key)
                .map(|c| c.name.clone())
                .collect(),
            _ => Vec::new(),
        };
        Some((
            sql,
            EditSource {
                schema: ref_info
                    .and_then(|t| t.schema.clone())
                    .or_else(|| fk.ref_schema.clone()),
                table: ref_info
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| fk.ref_table.clone()),
                pk_cols,
            },
        ))
    }

    /// Per-result-column foreign-key labels for the tab at `idx`: `Some(ref_table)` where the
    /// column takes part in a foreign key on the current table, else `None`. Drives the grid's
    /// FK link tint + "Follow →" affordance. Empty when there's no result or no structure yet.
    fn fk_column_labels(&self, idx: usize) -> Vec<Option<String>> {
        let Some(result) = self.tabs.get(idx).and_then(|t| t.result.as_ref()) else {
            return Vec::new();
        };
        let Some(info) = self.structure_table(idx) else {
            return vec![None; result.column_count()];
        };
        result
            .columns
            .iter()
            .map(|col| {
                info.foreign_keys
                    .iter()
                    .find(|fk| fk.columns.iter().any(|c| c.eq_ignore_ascii_case(&col.name)))
                    .map(|fk| fk.ref_table.clone())
            })
            .collect()
    }

    /// The tab slot a preview should land in: the existing reusable preview slot, else the
    /// active tab when it's a blank scratch tab, else a freshly opened tab. Shared by
    /// [`Self::open_table`] and [`Self::open_definition`].
    fn preview_target_slot(&mut self) -> usize {
        if let Some(i) = self.tabs.iter().position(|t| t.preview) {
            i
        } else {
            let cur = &self.tabs[self.active_query_tab];
            if cur.edits.source.is_none() && cur.result.is_none() && cur.sql.trim().is_empty() {
                self.active_query_tab
            } else {
                self.new_tab();
                self.active_query_tab
            }
        }
    }

    /// Show a database object's definition SQL (a routine body, a trigger's `CREATE` text)
    /// in a preview tab for reading. Unlike [`Self::open_table`] the SQL is *not* executed —
    /// re-running a `CREATE` would recreate the object — it's just placed in the editor so the
    /// user can read, copy, or run it deliberately.
    fn open_definition(&mut self, title: String, sql: String) {
        let idx = self.preview_target_slot();
        let id = self.tabs[idx].id;
        let conn_id = self.tabs[idx].conn_id.clone();
        let mut tab = QueryTab::new(id, title);
        tab.conn_id = conn_id;
        tab.sql = if sql.trim().is_empty() {
            "-- No definition available (the backend did not expose this object's source).".into()
        } else {
            sql
        };
        tab.preview = true;
        self.tabs[idx] = tab;
        self.active_query_tab = idx;
        self.workspace_dirty = true;
    }

    /// Bind the active tab to a saved connection. Connects in the background when the
    /// connection isn't live yet (or when `force`, e.g. an explicit "Connect").
    fn bind_connection(&mut self, idx: usize, force: bool) {
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
    fn disconnect_conn(&mut self, id: &str) {
        self.active_connections.retain(|c| c.config_id != id);
        // An ER diagram of the dropped connection is stale; close it.
        if self.erd.as_ref().is_some_and(|e| e.conn_id == id) {
            self.erd = None;
        }
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

    // --- background work --------------------------------------------------

    fn poll_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AppMessage::Connected {
                    conn_id,
                    name,
                    databases,
                    result,
                } => {
                    self.busy = Busy::Idle;
                    match result {
                        Ok((db, schema)) => {
                            let n = schema.tables.len();
                            let arrived_id = conn_id.clone();
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
                                    schema,
                                    databases: if databases.is_empty() {
                                        prev_databases
                                    } else {
                                        databases
                                    },
                                };
                            } else {
                                self.active_connections.push(ActiveConnection {
                                    config_id: conn_id,
                                    name: name.clone(),
                                    db,
                                    schema,
                                    databases,
                                });
                            }
                            self.status_msg = format!("Connected to {name} — {n} tables");
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
                            // A fresh schema invalidates an open diagram of this connection
                            // (e.g. after a DDL migration re-introspects).
                            if self.erd.as_ref().is_some_and(|e| e.conn_id == arrived_id) {
                                self.refresh_erd();
                            }
                        }
                        Err(e) => {
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
                AppMessage::Queried {
                    tab_id,
                    conn_id,
                    sql,
                    result,
                    canceled,
                } => {
                    self.busy = Busy::Idle;
                    self.querying_tab_id = None;
                    self.query_cancel = None;
                    // A user cancel isn't a failure: don't log it as a failed statement and
                    // don't flag a red error — just note it and leave the previous result up.
                    if canceled {
                        self.status_msg = "Query cancelled".to_string();
                        self.error = None;
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
                            tab.edits.source = tab.edits.pending_source.take();
                            tab.edits.clear();
                            let status = result_status(&res);
                            let fetched = res.row_count() as u64;
                            let truncated = res.truncated;
                            tab.set_result(res);
                            // Refresh the pager total for paged table tabs: a short first
                            // page already tells us the total; otherwise count server-side
                            // in the background (the WHERE clause may have changed).
                            tab.total_rows = None;
                            if tab.edits.source.is_some() {
                                let window = dbcore::parse_page_window(&tab.sql);
                                if let Some(limit) =
                                    window.and_then(|w| w.limit.map(|l| (w.offset, l)))
                                {
                                    let (offset, limit) = limit;
                                    if offset == 0 && fetched < limit && !truncated {
                                        tab.total_rows = Some(fetched);
                                    } else if let Some((db, count_sql)) = tab
                                        .conn_id
                                        .as_deref()
                                        .and_then(|id| {
                                            self.active_connections
                                                .iter()
                                                .find(|c| c.config_id == id)
                                        })
                                        .map(|c| c.db.clone())
                                        .zip(dbcore::build_count_sql(&tab.sql))
                                    {
                                        let tx = self.tx.clone();
                                        self.rt.spawn(async move {
                                            let result = db
                                                .execute(&count_sql)
                                                .await
                                                .map_err(|e| e.to_string())
                                                .and_then(|r| count_from_result(&r));
                                            let _ = tx.send(AppMessage::Counted { tab_id, result });
                                        });
                                    }
                                }
                            }
                            if is_active {
                                self.status_msg = status;
                                self.error = None;
                            }
                        }
                        Err(e) if is_active => {
                            self.error = Some(format!("Query error: {e}"));
                            self.status_msg = "Query failed".to_string();
                        }
                        Err(_) => {}
                    }
                }
                AppMessage::Counted { tab_id, result } => {
                    if let (Some(tab), Ok(n)) =
                        (self.tabs.iter_mut().find(|t| t.id == tab_id), result)
                    {
                        tab.total_rows = Some(n);
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
                            self.status_msg = msg;
                            self.error = None;
                            self.schema_pending = None;
                            // Close the editor on the tab that applied the migration (the
                            // user may have switched tabs while it ran).
                            let source_tab = self.tabs.iter_mut().find(|t| t.id == tab_id);
                            let conn_id = source_tab.map(|tab| {
                                tab.schema_editor = None;
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
                                    let name = ac.name.clone();
                                    let tx = self.tx.clone();
                                    self.rt.spawn(async move {
                                        let result = db
                                            .introspect()
                                            .await
                                            .map(|schema| (db, schema))
                                            .map_err(|e| e.to_string());
                                        let _ = tx.send(AppMessage::Connected {
                                            conn_id,
                                            name,
                                            databases: Vec::new(),
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
            ctx.request_repaint();
        }
    }

    fn start_connect(&mut self, idx: usize) {
        let Some(cfg) = self.connections.get(idx).cloned() else {
            return;
        };
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
            let mut databases = Vec::new();
            let result = async {
                let db = dbcore::connect(&cfg, password, ssh_secret).await?;
                let schema = db.introspect().await?;
                databases = db.list_databases().await.unwrap_or_default();
                Ok::<_, dbcore::CoreError>((db, schema))
            }
            .await
            .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Connected {
                conn_id: id,
                name,
                databases,
                result,
            });
        });
    }

    /// Append one event to the append-only audit trail (`dbcore::audit`). Separate from
    /// history: audit also records connection events, rotates monthly instead of being
    /// compacted, and has no in-app clear. Best effort — never load-bearing.
    fn record_audit(
        &self,
        action: dbcore::audit::AuditAction,
        conn_id: &str,
        sql: &str,
        ok: bool,
        error: Option<String>,
        rows: Option<u64>,
        elapsed_ms: f64,
    ) {
        if !self.audit_enabled || cfg!(test) {
            return;
        }
        let (conn_name, target) = self
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .map(|c| (c.name.clone(), c.target_summary()))
            .unwrap_or_default();
        let _ = dbcore::audit::append(&dbcore::audit::AuditEntry {
            at: dbcore::history::now_rfc3339(),
            action,
            conn_id: conn_id.to_string(),
            conn_name,
            target,
            sql: sql.to_string(),
            ok,
            error,
            rows,
            elapsed_ms,
        });
    }

    /// Append one executed statement to the on-disk query history and the audit trail.
    /// Best effort: history is never load-bearing, so failures are swallowed. History
    /// and audit honour their own settings toggles independently.
    fn record_history(
        &mut self,
        action: dbcore::audit::AuditAction,
        conn_id: &str,
        sql: &str,
        ok: bool,
        error: Option<String>,
        rows: Option<u64>,
        elapsed_ms: f64,
    ) {
        self.record_audit(action, conn_id, sql, ok, error.clone(), rows, elapsed_ms);
        // Feed the editor's ghost-text pool with statements that ran cleanly (independent
        // of the history-logging setting, which only governs the on-disk audit log). Skip
        // under test so the pool stays deterministic.
        if ok && !cfg!(test) {
            self.suggest_pool.push(PooledQuery {
                conn_id: conn_id.to_string(),
                sql: sql.to_string(),
            });
            if self.suggest_pool.len() > dbcore::history::MAX_ENTRIES {
                self.suggest_pool.remove(0);
            }
        }
        if !self.history_enabled {
            return;
        }
        // Unit tests construct real apps and pump real messages; never let them write
        // into the user's actual history file.
        if cfg!(test) {
            return;
        }
        let conn_name = self
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let entry = dbcore::history::HistoryEntry {
            at: dbcore::history::now_rfc3339(),
            conn_id: conn_id.to_string(),
            conn_name,
            sql: sql.to_string(),
            ok,
            error,
            rows,
            elapsed_ms,
        };
        let _ = dbcore::history::append(&entry);
        // Keep an open History dialog live.
        if self.history_open {
            self.history_cache.push(entry);
        }
    }

    /// The active tab's bound connection id and its saved display name, if any.
    fn active_conn_id_name(&self) -> (Option<String>, Option<String>) {
        let conn_id = self.tab().conn_id.clone();
        let conn_name = conn_id.as_deref().and_then(|id| {
            self.connections
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
        });
        (conn_id, conn_name)
    }

    /// Commit the favorite name dialog: rename an existing favorite or add a new one, then
    /// persist. An empty name falls back to a placeholder so the entry is never nameless.
    fn confirm_save_favorite(&mut self) {
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
    fn persist_favorites(&mut self) {
        if cfg!(test) {
            return;
        }
        if let Err(e) = dbcore::favorites::save(&self.favorites_cache) {
            self.error = Some(format!("Could not save favorites: {e}"));
        }
    }

    /// Is the tab at `idx` bound to a connection whose saved config is marked production?
    fn tab_connection_is_production(&self, idx: usize) -> bool {
        self.tabs
            .get(idx)
            .and_then(|tab| tab.conn_id.as_deref())
            .is_some_and(|id| self.connections.iter().any(|c| c.id == id && c.production))
    }

    /// Is the tab at `idx` bound to a connection whose saved config is marked read-only?
    fn tab_connection_is_read_only(&self, idx: usize) -> bool {
        self.tabs
            .get(idx)
            .and_then(|tab| tab.conn_id.as_deref())
            .is_some_and(|id| self.connection_is_read_only(id))
    }

    /// Is the saved config for `conn_id` marked read-only? Sidebar actions (import, export)
    /// act on a connection rather than a tab, so they check it directly.
    fn connection_is_read_only(&self, conn_id: &str) -> bool {
        self.connections
            .iter()
            .any(|c| c.id == conn_id && c.read_only)
    }

    /// Refuse an action on a read-only connection with a consistent error + status pair.
    /// `what` completes the sentence "This connection is read-only — {what}".
    fn refuse_read_only(&mut self, what: &str) {
        self.error = Some(format!("This connection is read-only — {what}"));
        self.status_msg = "Blocked by read-only mode".to_string();
    }

    /// Run the SQL of the tab at `idx` against its bound connection.
    fn start_query_for(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        let sql = tab.sql.trim().to_string();
        if sql.is_empty() {
            return;
        }
        let tab_id = tab.id;
        let conn_id = tab.conn_id.clone().unwrap_or_default();
        let db = match tab
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
        {
            Some(active) => active.db.clone(),
            None => {
                self.error = Some("Not connected.".to_string());
                return;
            }
        };
        let tx = self.tx.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        self.query_cancel = Some(cancel.clone());
        self.busy = Busy::Querying;
        self.querying_tab_id = Some(tab_id);
        self.error = None;
        self.status_msg = "Running query…".to_string();
        self.rt.spawn(async move {
            let res = db
                .execute_capped_cancellable(&sql, MAX_FETCH_ROWS, cancel)
                .await;
            // Distinguish a user cancel from a real failure before flattening to a string.
            let canceled = matches!(res, Err(dbcore::CoreError::Canceled));
            let result = res.map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Queried {
                tab_id,
                conn_id,
                sql,
                result,
                canceled,
            });
        });
    }

    /// Rewrite the active tab's paging window to `(limit, offset)` in its connection's
    /// dialect and re-run. No-op when the tab isn't a paged simple-table read.
    fn run_page(&mut self, limit: u64, offset: u64) {
        let Some(kind) = self.active().map(|a| a.db.kind()) else {
            return;
        };
        let idx = self.active_query_tab;
        let Some(sql) = dbcore::with_page_window(kind, &self.tabs[idx].sql, limit, offset) else {
            return;
        };
        self.tabs[idx].sql = sql;
        // The rewrite preserves the simple-select shape, so the result stays editable.
        self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
        self.workspace_dirty = true;
        self.start_query_for(idx);
    }

    /// Pager navigation for the active (paged) table tab.
    fn page_nav(&mut self, nav: PageNav) {
        if self.busy != Busy::Idle {
            return;
        }
        let tab = self.tab();
        let Some(win) = dbcore::parse_page_window(&tab.sql) else {
            return;
        };
        let Some(limit) = win.limit.filter(|&l| l > 0) else {
            return;
        };
        let total = tab.total_rows;
        let offset = match nav {
            PageNav::First => 0,
            PageNav::Prev => win.offset.saturating_sub(limit),
            PageNav::Next => win.offset + limit,
            PageNav::Last => match total {
                Some(t) if t > 0 => ((t - 1) / limit) * limit,
                _ => return,
            },
        };
        // Never run past a known end (the pager disables these buttons, but a stale
        // total or a keyboard repeat could still get here).
        if let Some(t) = total {
            if offset >= t && offset != 0 {
                return;
            }
        }
        if offset == win.offset {
            return;
        }
        self.run_page(limit, offset);
    }

    /// Change the active tab's page size, snapping the offset to the new page grid so
    /// the rows currently on screen stay within the shown page.
    fn set_page_size(&mut self, size: u64) {
        if self.busy != Busy::Idle || size == 0 {
            return;
        }
        let Some(win) = dbcore::parse_page_window(&self.tab().sql) else {
            return;
        };
        if win.limit == Some(size) {
            return;
        }
        let offset = (win.offset / size) * size;
        self.run_page(size, offset);
    }

    /// Copy the selected result rows to the clipboard in `format`. Only stored rows are copied
    /// (unsaved new rows aren't data yet); their cloned values are rendered by
    /// [`dbcore::copy_rows`] and the text is staged in `copy_buffer` for `draw` to flush.
    fn copy_selection(&mut self, format: dbcore::CopyFormat) {
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
    fn paste_rows(&mut self, text: &str) {
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
    fn export_table(&mut self, table: &TableInfo, format: dbcore::ExportFormat) {
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
    fn open_import(&mut self, table: &TableInfo) {
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
    fn reload_import_preview(&mut self) {
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
    fn confirm_import(&mut self) {
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

    /// Work out whether the tab's SQL still reads one whole table, and if so build the
    /// [`EditSource`] that makes its rows editable. Matches the table (case-insensitively)
    /// against the bound connection's schema to pick up its primary key; an ambiguous bare
    /// name (same table in several schemas) or a table without a PK stays read-only.
    fn derive_edit_source(&self, idx: usize) -> Option<EditSource> {
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
        let info = matches.next()?;
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

    /// The introspected [`dbcore::TableInfo`] behind the tab at `idx`: the table it was
    /// opened on (loaded or still in flight), looked up in its live connection's schema.
    /// `None` for plain query tabs or when the connection is down — the Structure view
    /// needs this, so without it the tab falls back to Data.
    fn structure_table(&self, idx: usize) -> Option<&dbcore::TableInfo> {
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
    fn commit_edits(&mut self) {
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
    fn confirm_edits(&mut self) {
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
    fn undo_edits(&mut self) {
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
    fn redo_edits(&mut self) {
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
    fn build_commit_statements(&mut self) -> Option<Vec<String>> {
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
        let Some(source) = self.tabs[idx].edits.source.clone() else {
            return None;
        };
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

    // --- action dispatch --------------------------------------------------

    fn apply_action(&mut self, action: Action) {
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
            Action::OpenTable { sql, source, pin } => self.open_table(sql, source, pin),
            Action::OpenDefinition { title, sql } => self.open_definition(title, sql),
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

    /// Open a file picker filtered to `extensions` and store the chosen path into the
    /// connection-editor field selected by `field`.
    fn browse_pem_into(
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

    fn start_connection_test(&mut self) {
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

    fn save_connection(&mut self) {
        let Some(ed) = self.editor.take() else { return };
        let cfg = ed.config;
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

    // --- workspace persistence --------------------------------------------

    /// Snapshot the open tabs into the serialisable workspace (no result rows — only SQL,
    /// the bound connection, and the table source needed to re-open editable).
    fn snapshot_workspace(&self) -> dbcore::config::Workspace {
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
    fn maybe_save_workspace(&mut self, force: bool) {
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
}

impl eframe::App for DbGuiApp {
    // eframe 0.34 hands us a root `Ui`; panels are added with `show_inside`.
    fn ui(&mut self, ui_root: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.draw(ui_root, Some(frame));
    }

    /// Match the window clear colour to the active theme so hairline panel gaps don't flash
    /// eframe's default near-black clear (reads as a thick black bar on light themes).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        crate::theme::current().base.to_normalized_gamma_f32()
    }
}

impl DbGuiApp {
    /// Draw one frame into the given root ui. Split out from `eframe::App::ui` so it can be
    /// driven headlessly in tests (no `eframe::Frame` needed).
    fn draw(&mut self, ui_root: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        let ctx = ui_root.ctx().clone();
        self.poll_messages(&ctx);

        // First-run welcome page: replace the entire window until "Get Started" is clicked.
        if self.show_welcome {
            let mut actions = Vec::new();
            self.draw_welcome_page(ui_root, &mut actions);
            for action in actions {
                self.apply_action(action);
            }
            return;
        }

        let mut actions: Vec<Action> = Vec::new();

        // Global shortcut: Cmd/Ctrl+Enter runs the query.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            actions.push(Action::RunQuery);
        }
        // Cmd/Ctrl+S opens the SQL preview dialog for staged cell edits.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            actions.push(Action::PreviewEdits);
        }
        // Cmd/Ctrl+R reloads the current result (re-runs the tab's SQL), dropping any
        // unsaved cell edits — the reloaded result starts from a clean edit slate.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::R)) {
            actions.push(Action::RunQuery);
        }
        // Esc discards unsaved cell edits (revert to the stored values) when no cell editor
        // is open — the open-editor case is handled inside `render_editor` (cancel that
        // cell only). Skipped while the filter bar is up, which uses Esc to close itself.
        // Recorded as one undo step so an accidental discard can be taken back with Cmd/Ctrl+Z.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && self.tab().edits.active.is_none()
            && self.tab().edits.has_pending()
            && !self.tab().filter.visible
        {
            self.tab_mut().edits.discard_all();
            self.tab_mut().recompute_view();
            self.status_msg = "Discarded unsaved edits (⌘Z to undo)".to_string();
            self.error = None;
            self.workspace_dirty = true;
        }
        // Cmd/Ctrl+Z undoes, Cmd/Ctrl+Shift+Z redoes, the last staged-edit change (cell edit,
        // delete mark, new row, fill, paste, discard). Only when no text field is focused —
        // an open cell editor / SQL console handles its own in-field undo. Shift+Z is matched
        // first so a redo isn't also read as an undo.
        let typing_now = ctx.memory(|m| m.focused().is_some());
        if !typing_now && self.tab().edits.editable() {
            let (undo, redo) = ctx.input_mut(|i| {
                let redo = i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                );
                let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
                (undo, redo)
            });
            if redo {
                actions.push(Action::Redo);
            } else if undo {
                actions.push(Action::Undo);
            }
        }
        // Backspace/Delete on the selected rows (when nothing is being typed) marks every
        // selected stored row for deletion (red) and drops any selected pending new rows.
        // `focused()` is `Some` while any text field — a cell editor, the SQL console, the
        // field filter — has focus, so this never steals a real backspace keystroke.
        let typing = ctx.memory(|m| m.focused().is_some());
        if !typing
            && self.tab().edits.editable()
            && self.tab().edits.active.is_none()
            && ctx
                .input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete))
            && !self.tab().selection.is_empty()
        {
            let order_len = self.tab().row_order.len();
            let selected: Vec<usize> = self.tab().selection.iter().collect();
            // One undo group so the whole multi-row delete takes a single Cmd/Ctrl+Z.
            self.tab_mut().edits.begin_undo_group();
            // Mark stored rows for deletion. New (insert) rows are removed instead, highest
            // display index first so the renumbering of the rows above each removal never
            // invalidates an index we still have to process.
            for &disp in &selected {
                if disp < order_len {
                    let raw = self.tab().row_order[disp];
                    self.tab_mut().edits.toggle_delete(raw);
                }
            }
            let mut removed_new = false;
            for &disp in selected.iter().rev() {
                if disp >= order_len {
                    let new_id = crate::edit::NEW_ROW_BASE + (disp - order_len);
                    self.tab_mut().edits.remove_new_row(new_id);
                    removed_new = true;
                }
            }
            self.tab_mut().edits.end_undo_group();
            // Removing new rows shifts the trailing display indices; clear the selection so it
            // can't point at the wrong (renumbered) rows. Stored-only deletes keep their
            // selection so the marked rows stay highlighted.
            if removed_new {
                self.tab_mut().selection.clear();
            }
        }
        // Arrow keys drive the grid's cell cursor, spreadsheet-style, when nothing is being
        // typed: ↑/↓ move rows (Shift extends the selection from the anchor), ←/→ move
        // columns. Enter or F2 opens the editor on the cursor cell (Enter toggles booleans
        // in place). All keys are *consumed* so nothing else — in particular the freshly
        // opened editor, which would otherwise see this very Enter press later in the same
        // frame and instantly commit itself — reacts to them.
        if !typing
            && self.tab().result.is_some()
            && self.tab().edits.active.is_none()
            && self.tab().view == TabView::Data
            && self.tab().schema_editor.is_none()
        {
            let (mut dr, mut dc, mut extend) = (0isize, 0isize, false);
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown) {
                    dr += 1;
                    extend = true;
                }
                if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp) {
                    dr -= 1;
                    extend = true;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    dr += 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    dr -= 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    dc -= 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    dc += 1;
                }
            });
            if dr != 0 || dc != 0 {
                let tab = self.tab_mut();
                let len = tab.row_order.len() + tab.edits.new_rows;
                let ncols = tab.result.as_ref().map_or(0, |r| r.column_count());
                if tab.selection.move_cursor(dr, dc, len, ncols, extend) {
                    tab.pending_scroll = tab.selection.cursor().map(|(r, _)| r);
                }
            }
            let open_editor = ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::F2)
            });
            if open_editor && self.tab().edits.editable() {
                let tab = self.tab_mut();
                if let (Some((disp, col)), Some(result)) =
                    (tab.selection.cursor(), tab.result.as_ref())
                {
                    if let Some(raw) =
                        crate::edit::disp_to_raw(&tab.row_order, tab.edits.new_rows, disp)
                    {
                        let deleted = tab.edits.row_state(raw) == crate::edit::RowState::Deleted;
                        let bytes = crate::edit::original_value(result, raw, col)
                            .is_some_and(|v| matches!(v, dbcore::Value::Bytes(_)));
                        if !deleted && !bytes {
                            if tab.edits.col_kind(col) == crate::edit::EditorKind::Bool {
                                if let Some(orig) = crate::edit::original_value(result, raw, col) {
                                    tab.edits.toggle_bool(raw, col, &orig);
                                }
                            } else {
                                crate::edit::begin_cell_edit(&mut tab.edits, result, raw, col);
                            }
                        }
                    }
                }
            }
        }
        // Cmd/Ctrl+A selects every row in the grid — but only when not typing, so it keeps
        // its native "select all text" meaning inside the SQL console or any field editor.
        if !typing
            && self.tab().result.is_some()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A))
        {
            let len = self.tab().row_order.len() + self.tab().edits.new_rows;
            self.tab_mut().selection.select_all(len);
        }
        // Cmd/Ctrl+C copies the selected rows as TSV (spreadsheet-native, and what paste reads
        // back). The OS turns the copy shortcut into an `Event::Copy` (a raw `Key::C` press
        // never arrives for it on macOS), so match the event — and only when not typing, so a
        // focused text field keeps its native copy.
        if !typing
            && !self.tab().selection.is_empty()
            && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))
        {
            actions.push(Action::CopyRows(dbcore::CopyFormat::Tsv));
        }
        // Cmd/Ctrl+V pastes clipboard rows (TSV) as new insert rows in an editable table. Paste
        // also arrives as an `Event::Paste(text)`; `!typing` lets a focused cell/field paste
        // its text natively instead.
        if !typing {
            let pasted = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Paste(text) => Some(text.clone()),
                    _ => None,
                })
            });
            if let Some(text) = pasted {
                actions.push(Action::PasteRows(text));
            }
        }
        // Cmd/Ctrl+I beautifies the active tab's SQL (TablePlus-style).
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::I)) {
            actions.push(Action::BeautifySql);
        }
        // Cmd/Ctrl+T opens a new query tab; Cmd/Ctrl+W closes the active one.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::T)) {
            actions.push(Action::NewTab);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::W)) {
            actions.push(Action::CloseTab(self.active_query_tab));
        }
        // Cmd/Ctrl+F toggles the filter bar (when there's a result to filter); Esc hides it.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F))
            && self.tab().result.is_some()
        {
            let visible = self.tab().filter.visible;
            self.tab_mut().filter.visible = !visible;
        }
        if self.tab().filter.visible && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.tab_mut().filter.visible = false;
        }

        // Order matters: top/bottom/left/right carve space, central takes the rest. The
        // status bar is carved first so it pins to the very bottom edge. The left/right side
        // panels are carved BEFORE the query console so they run the full height (down to the
        // status bar); the console is then confined to the central column under the grid,
        // instead of spanning the whole width and clipping the details/schema panels.
        self.top_bar(ui_root, frame, &mut actions);
        self.query_tab_bar(ui_root, &mut actions);
        self.status_bar(ui_root, &mut actions);
        if self.show_connection_tabs {
            self.connection_tabs(ui_root, &mut actions);
        }
        if self.show_schema_panel {
            self.left_panel(ui_root, &mut actions);
        }
        // History sits outermost on the right, so the details panel stays next to the grid.
        if self.history_open {
            self.history_panel(ui_root, &mut actions);
        }
        if self.show_details_panel {
            self.right_panel(ui_root);
        }
        // Carved last among the edge panels: the console borders only the central grid, so its
        // top resize handle drags cleanly with nothing but the grid above it.
        if self.show_query_console {
            self.query_console(ui_root, &mut actions);
        }
        // A top panel after left/right carves the strip directly above the grid.
        self.filter_bar(ui_root);
        // ...and a bottom panel here carves the Data/Structure switch directly below it.
        self.view_mode_bar(ui_root, &mut actions);
        self.central_panel(ui_root, &mut actions);
        self.connection_dialog(&ctx, &mut actions);
        self.settings_dialog(&ctx, &mut actions);
        self.commit_preview_dialog(&ctx, &mut actions);
        self.favorite_name_dialog(&ctx, &mut actions);
        self.danger_confirm_dialog(&ctx, &mut actions);
        self.import_dialog(&ctx, &mut actions);
        self.schema_preview_dialog(&ctx, &mut actions);
        self.update_dialog(&ctx, &mut actions);
        self.whats_new_dialog(&ctx, &mut actions);

        let structural = actions.iter().any(|a| {
            matches!(
                a,
                Action::NewTab
                    | Action::CloseTab(_)
                    | Action::CloseOtherTabs(_)
                    | Action::CloseTabsToRight(_)
                    | Action::CloseAllTabs
                    | Action::SelectTab(_)
                    | Action::Connect(_)
                    | Action::BindConnection(_)
                    | Action::OpenTable { .. }
                    | Action::OpenDefinition { .. }
                    | Action::FollowForeignKey { .. }
                    | Action::DeleteConnection(_)
            )
        });
        for action in actions {
            self.apply_action(action);
        }

        // Flush any text an action staged for the clipboard (e.g. copied result rows) now that
        // the egui Context is in hand.
        if let Some(text) = self.copy_buffer.take() {
            ctx.copy_text(text);
        }

        if self.pending_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Persist the workspace: immediately after structural changes, otherwise on a throttle
        // (so typing SQL into a tab is eventually saved without writing every frame).
        self.maybe_save_workspace(structural);
        if self.workspace_dirty {
            ctx.request_repaint_after(std::time::Duration::from_millis(1600));
        }

        // Keep animating the spinner while background work is in flight.
        if self.busy != Busy::Idle || self.update.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::{
        ColumnInfo, ColumnMeta, IndexInfo, QueryResult, QueryStats, SchemaTree, TableInfo, Value,
    };

    struct DummyDb;
    #[async_trait::async_trait]
    impl dbcore::Database for DummyDb {
        fn kind(&self) -> dbcore::DbKind {
            dbcore::DbKind::Sqlite
        }
        async fn introspect(&self) -> dbcore::Result<SchemaTree> {
            unreachable!()
        }
        async fn execute_capped(
            &self,
            _sql: &str,
            _max_rows: usize,
        ) -> dbcore::Result<QueryResult> {
            // Background tasks (queries, pager counts) may legitimately land here in tests
            // that only assert on the UI-side state; an empty result keeps them quiet.
            Ok(QueryResult::default())
        }
        async fn execute_transaction(&self, _stmts: &[String]) -> dbcore::Result<usize> {
            unreachable!()
        }
        async fn export_query(
            &self,
            _sql: &str,
            sink: &mut (dyn dbcore::RowSink + Send),
        ) -> dbcore::Result<u64> {
            sink.finish()?;
            Ok(0)
        }
    }

    fn fake_schema(tables: usize, cols: usize) -> SchemaTree {
        SchemaTree {
            database_name: "testdb".into(),
            views: Vec::new(),
            routines: Vec::new(),
            triggers: Vec::new(),
            tables: (0..tables)
                .map(|t| TableInfo {
                    schema: None,
                    name: format!("table_{t}"),
                    columns: (0..cols)
                        .map(|c| ColumnInfo {
                            name: format!("field_{c}"),
                            data_type: "TEXT".into(),
                            nullable: c % 2 == 0,
                            primary_key: c == 0,
                        })
                        .collect(),
                    indexes: vec![IndexInfo {
                        name: format!("idx_{t}"),
                        unique: true,
                        columns: vec!["field_0".into()],
                    }],
                    foreign_keys: Vec::new(),
                })
                .collect(),
        }
    }

    fn fake_result(rows: usize, cols: usize) -> QueryResult {
        let columns = (0..cols)
            .map(|c| ColumnMeta {
                name: format!("col{c}"),
                type_name: "TEXT".into(),
            })
            .collect();
        let data = (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| Value::Int((r * cols + c) as i64))
                    .collect()
            })
            .collect();
        QueryResult {
            columns,
            rows: data,
            stats: QueryStats::default(),
            truncated: false,
        }
    }

    /// Destructive SQL on a production connection is held for confirmation; cancelling
    /// drops it, confirming runs it. Safe SQL runs straight through.
    #[test]
    fn production_connection_gates_destructive_queries() {
        let mut app = DbGuiApp::construct();
        // construct() loads the user's saved connections; drop them so the test only
        // sees its own.
        app.connections.clear();
        let mut cfg = dbcore::ConnectionConfig::new(dbcore::DbKind::Sqlite);
        cfg.id = "c1".into();
        cfg.production = true;
        app.connections.push(cfg);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "prod".into(),
            db: std::sync::Arc::new(DummyDb),
            databases: Vec::new(),
            schema: fake_schema(1, 1),
        });
        app.tab_mut().conn_id = Some("c1".into());

        // A plain SELECT is not destructive: it runs without confirmation.
        app.tab_mut().sql = "SELECT * FROM table_0".into();
        app.apply_action(Action::RunQuery);
        assert!(app.danger_pending.is_none());
        assert_eq!(app.busy, Busy::Querying);
        app.busy = Busy::Idle;

        // Destructive SQL is intercepted: dialog state set, nothing executed.
        app.tab_mut().sql = "DELETE FROM table_0".into();
        app.apply_action(Action::RunQuery);
        let pending = app.danger_pending.as_ref().expect("query held back");
        assert!(pending[0].missing_where);
        assert_eq!(app.busy, Busy::Idle);

        // Cancel drops it without running.
        app.apply_action(Action::CancelDangerQuery);
        assert!(app.danger_pending.is_none());
        assert_eq!(app.busy, Busy::Idle);

        // Confirm actually starts the query.
        app.apply_action(Action::RunQuery);
        app.apply_action(Action::ConfirmDangerQuery);
        assert!(app.danger_pending.is_none());
        assert_eq!(app.busy, Busy::Querying);

        // On a non-production connection the same SQL runs without confirmation.
        app.busy = Busy::Idle;
        app.connections[0].production = false;
        app.apply_action(Action::RunQuery);
        assert!(app.danger_pending.is_none());
        assert_eq!(app.busy, Busy::Querying);
    }

    /// A read-only connection refuses writes outright (no confirmation dialog), refuses
    /// staged-edit saves and DDL, and still runs reads.
    #[test]
    fn read_only_connection_blocks_writes() {
        let mut app = DbGuiApp::construct();
        app.connections.clear();
        let mut cfg = dbcore::ConnectionConfig::new(dbcore::DbKind::Sqlite);
        cfg.id = "c1".into();
        cfg.read_only = true;
        app.connections.push(cfg);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "replica".into(),
            db: std::sync::Arc::new(DummyDb),
            databases: Vec::new(),
            schema: fake_schema(1, 1),
        });
        app.tab_mut().conn_id = Some("c1".into());

        // Reads run normally.
        app.tab_mut().sql = "SELECT * FROM table_0".into();
        app.apply_action(Action::RunQuery);
        assert!(app.error.is_none());
        assert_eq!(app.busy, Busy::Querying);
        app.busy = Busy::Idle;

        // A write is refused outright — no danger dialog, no query.
        app.tab_mut().sql = "DELETE FROM table_0".into();
        app.apply_action(Action::RunQuery);
        assert!(app.danger_pending.is_none());
        assert_eq!(app.busy, Busy::Idle);
        assert!(app.error.as_deref().unwrap_or("").contains("read-only"));

        // So is a CTE-wrapped write the old lexical guard used to miss.
        app.error = None;
        app.tab_mut().sql = "WITH x AS (SELECT 1) UPDATE table_0 SET col0 = 1".into();
        app.apply_action(Action::RunQuery);
        assert_eq!(app.busy, Busy::Idle);
        assert!(app.error.as_deref().unwrap_or("").contains("read-only"));

        // Committing staged edits is refused before any SQL is built.
        app.error = None;
        app.apply_action(Action::PreviewEdits);
        assert!(app.commit_pending.is_none());
        assert!(app.error.as_deref().unwrap_or("").contains("read-only"));

        // Applying a staged schema migration is refused and the preview is dropped.
        app.error = None;
        app.schema_pending = Some(vec!["ALTER TABLE table_0 ADD c INT".into()]);
        app.apply_action(Action::ApplySchema);
        assert!(app.schema_pending.is_none());
        assert_eq!(app.busy, Busy::Idle);
        assert!(app.error.as_deref().unwrap_or("").contains("read-only"));

        // Turning the flag off lets the same write reach the danger-free run path.
        app.error = None;
        app.connections[0].read_only = false;
        app.tab_mut().sql = "DELETE FROM table_0".into();
        app.apply_action(Action::RunQuery);
        assert_eq!(app.busy, Busy::Querying);
    }

    // ─── import ──────────────────────────────────────────────────────────────

    /// An app with one live SQLite connection (`c1`) whose schema holds `users`.
    fn app_with_users_table(columns: Vec<ColumnInfo>) -> DbGuiApp {
        let mut app = DbGuiApp::construct();
        app.connections.clear();
        let mut cfg = dbcore::ConnectionConfig::new(dbcore::DbKind::Sqlite);
        cfg.id = "c1".into();
        app.connections.push(cfg);

        let mut schema = fake_schema(0, 0);
        schema.tables.push(TableInfo {
            schema: None,
            name: "users".into(),
            columns,
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        });
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "local".into(),
            db: std::sync::Arc::new(DummyDb),
            databases: Vec::new(),
            schema,
        });
        app.tab_mut().conn_id = Some("c1".into());
        app
    }

    fn col(name: &str, ty: &str, nullable: bool, pk: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            primary_key: pk,
        }
    }

    fn users_columns() -> Vec<ColumnInfo> {
        vec![
            col("id", "INTEGER", false, true),
            col("email", "TEXT", false, false),
            col("age", "INTEGER", true, false),
        ]
    }

    /// Build a draft directly, as `open_import` would after the (untestable) file dialog.
    fn draft_for(app: &DbGuiApp, headers: &[&str], path: &std::path::Path) -> ImportDraft {
        let table = app.active_connections[0].schema.tables[0].clone();
        let mut draft = ImportDraft {
            table,
            conn_id: "c1".into(),
            path: path.to_path_buf(),
            format: dbcore::ImportFormat::Csv,
            has_header: true,
            headers: headers.iter().map(|h| (*h).to_string()).collect(),
            preview_rows: Vec::new(),
            more: false,
            mapping: Vec::new(),
        };
        draft.auto_map();
        draft
    }

    fn temp_csv(name: &str, body: &str) -> std::path::PathBuf {
        use std::io::Write;
        let mut p = std::env::temp_dir();
        p.push(format!("plusplus-ui-import-{}-{name}", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    /// The read-only refusal happens before the file dialog opens, so the sidebar action is a
    /// pure no-op on a replica — no dialog, no picker.
    #[test]
    fn import_refuses_on_a_read_only_connection() {
        let mut app = app_with_users_table(users_columns());
        app.connections[0].read_only = true;
        let table = app.active_connections[0].schema.tables[0].clone();

        app.apply_action(Action::ImportIntoTable(table));
        assert!(app.import_pending.is_none(), "no dialog should open");
        assert!(app.error.as_deref().unwrap_or("").contains("read-only"));

        // And confirming an already-open dialog is refused too (defence in depth), which is the
        // path that matters if the connection is flipped to read-only mid-dialog.
        let path = temp_csv("ro.csv", "id,email\n1,a@b.c\n");
        app.error = None;
        app.import_pending = Some(draft_for(&app, &["id", "email"], &path));
        app.apply_action(Action::ConfirmImport);
        assert!(app.import_pending.is_none());
        assert_eq!(app.busy, Busy::Idle, "nothing was spawned");
        assert!(app.error.as_deref().unwrap_or("").contains("read-only"));
        std::fs::remove_file(&path).ok();
    }

    /// Headers map onto target columns by name regardless of case, and an unmatched target
    /// stays unmapped rather than being filled positionally.
    #[test]
    fn import_maps_headers_case_insensitively_and_never_positionally() {
        let app = app_with_users_table(users_columns());
        let path = temp_csv("map.csv", "EMAIL,Id\n");
        let draft = draft_for(&app, &["EMAIL", "Id"], &path);

        // id <- source 1, email <- source 0, age unmatched.
        assert_eq!(draft.mapping, vec![Some(1), Some(0), None]);

        let targets = draft.targets();
        assert_eq!(targets.len(), 2, "only mapped columns are written");
        assert_eq!(targets[0].name, "id");
        assert_eq!(targets[0].source, 1);
        assert_eq!(targets[0].kind, dbcore::EditorKind::Int);
        assert_eq!(targets[1].name, "email");
        assert_eq!(targets[1].source, 0);

        // `age` is nullable, so skipping it raises no warning.
        assert!(draft.unmapped_required().is_empty());
        std::fs::remove_file(&path).ok();
    }

    /// A NOT NULL column with no mapping is surfaced as a warning (it may still have a default).
    #[test]
    fn import_warns_about_unmapped_not_null_columns() {
        let app = app_with_users_table(users_columns());
        let path = temp_csv("warn.csv", "id\n");
        let draft = draft_for(&app, &["id"], &path);

        // `email` is NOT NULL and unmapped; `id` is a PK so it is excused (autoincrement).
        assert_eq!(draft.unmapped_required(), vec!["email"]);
        std::fs::remove_file(&path).ok();
    }

    /// A mapped binary column is refused. `EditorKind::classify("BLOB")` falls through to Text,
    /// so without this guard the import would insert a string literal into a BLOB column.
    #[test]
    fn import_refuses_a_mapped_binary_column() {
        let mut app = app_with_users_table(vec![
            col("id", "INTEGER", false, true),
            col("avatar", "BLOB", true, false),
        ]);
        let path = temp_csv("bin.csv", "id,avatar\n1,xx\n");
        let draft = draft_for(&app, &["id", "avatar"], &path);
        assert_eq!(draft.binary_conflicts(), vec!["avatar"]);

        app.import_pending = Some(draft);
        app.apply_action(Action::ConfirmImport);
        assert_eq!(app.busy, Busy::Idle, "nothing was spawned");
        assert!(app.error.as_deref().unwrap_or("").contains("Binary columns"));
        assert!(
            app.import_pending.is_some(),
            "a rejected import keeps the dialog open so the mapping isn't lost"
        );

        // Skipping the binary column unblocks it.
        app.error = None;
        app.import_pending.as_mut().unwrap().mapping[1] = None;
        app.apply_action(Action::ConfirmImport);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.busy, Busy::Importing);
        std::fs::remove_file(&path).ok();
    }

    /// Importing with nothing mapped is refused, and the dialog stays open.
    #[test]
    fn import_requires_at_least_one_mapped_column() {
        let mut app = app_with_users_table(users_columns());
        let path = temp_csv("nomap.csv", "x,y\n1,2\n");
        let mut draft = draft_for(&app, &["x", "y"], &path);
        assert_eq!(draft.mapping, vec![None, None, None], "no names match");
        draft.mapping = vec![None, None, None];

        app.import_pending = Some(draft);
        app.apply_action(Action::ConfirmImport);
        assert_eq!(app.busy, Busy::Idle);
        assert!(app.error.as_deref().unwrap_or("").contains("at least one"));
        assert!(app.import_pending.is_some());
        std::fs::remove_file(&path).ok();
    }

    /// A valid confirm closes the dialog and hands the work to the background runtime.
    #[test]
    fn import_confirm_spawns_the_transaction() {
        let mut app = app_with_users_table(users_columns());
        let path = temp_csv("ok.csv", "id,email,age\n1,a@b.c,30\n2,d@e.f,\n");
        app.import_pending = Some(draft_for(&app, &["id", "email", "age"], &path));

        app.apply_action(Action::ConfirmImport);
        assert!(app.import_pending.is_none(), "dialog closes");
        assert_eq!(app.busy, Busy::Importing);
        assert!(app.error.is_none());
        std::fs::remove_file(&path).ok();
    }

    /// Render the import dialog headlessly: its mapping combo boxes and two grids all live in
    /// one window, so a missing `id_salt` would collide. Also proves it doesn't panic.
    /// Bind the `heading` family to the default proportional fonts. The real app installs Inter
    /// for it (`install_fonts`); a dialog title is the first thing in the test suite to ask for
    /// that family, and epaint panics on an unbound one.
    fn bind_heading_font(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        let proportional = fonts.families[&egui::FontFamily::Proportional].clone();
        fonts
            .families
            .insert(egui::FontFamily::Name(crate::HEADING_FAMILY.into()), proportional);
        ctx.set_fonts(fonts);
    }

    #[test]
    fn probe_import_dialog_renders_without_id_clash() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);
        bind_heading_font(&ctx);

        let mut app = app_with_users_table(users_columns());
        let path = temp_csv("probe.csv", "id,email,age\n1,a@b.c,30\n2,d@e.f,\n");
        let mut draft = draft_for(&app, &["id", "email", "age"], &path);
        // Give the preview something to lay out, including a JSON-style NULL cell.
        draft.preview_rows = vec![
            vec![Some("1".into()), Some("a@b.c".into()), Some("30".into())],
            vec![Some("2".into()), Some("d@e.f".into()), None],
        ];
        draft.more = true;
        app.import_pending = Some(draft);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        for _ in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
            clashes.extend(collect_clash_text(&out.shapes));
        }
        clashes.sort();
        clashes.dedup();
        assert!(clashes.is_empty(), "ID clashes:\n{}", clashes.join("\n"));
        assert!(app.import_pending.is_some(), "dialog stayed open");
        std::fs::remove_file(&path).ok();
    }

    /// "Skip all" unmaps everything; "Match by name" restores the auto-mapping, discarding
    /// whatever the user picked by hand.
    #[test]
    fn import_quick_actions_clear_and_restore_the_mapping() {
        let mut app = app_with_users_table(users_columns());
        let path = temp_csv("quick.csv", "id,email,age\n1,a@b.c,30\n");
        app.import_pending = Some(draft_for(&app, &["id", "email", "age"], &path));

        app.apply_action(Action::ClearImportMapping);
        assert_eq!(
            app.import_pending.as_ref().unwrap().mapping,
            vec![None, None, None]
        );

        // A hand-picked, deliberately wrong mapping is discarded by Match by name.
        app.apply_action(Action::SetImportMapping {
            target: 0,
            source: Some(2),
        });
        app.apply_action(Action::AutoMapImport);
        assert_eq!(
            app.import_pending.as_ref().unwrap().mapping,
            vec![Some(0), Some(1), Some(2)]
        );
        std::fs::remove_file(&path).ok();
    }

    /// The dialog's other render branches: the blocking binary callout, the not-null warning,
    /// and the empty-file state (which draws its own footer and returns early).
    #[test]
    fn probe_import_dialog_alternate_states_render() {
        let render = |app: &mut DbGuiApp| {
            let ctx = egui::Context::default();
            egui_extras::install_image_loaders(&ctx);
            crate::style::apply(&ctx);
            bind_heading_font(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
            let mut clashes = Vec::new();
            for _ in 0..2 {
                let raw = egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
                clashes.extend(collect_clash_text(&out.shapes));
            }
            clashes.sort();
            clashes.dedup();
            assert!(clashes.is_empty(), "ID clashes:\n{}", clashes.join("\n"));
        };

        // Blocking binary conflict + a not-null column left unmapped.
        let mut app = app_with_users_table(vec![
            col("id", "INTEGER", false, true),
            col("email", "TEXT", false, false),
            col("avatar", "BLOB", true, false),
        ]);
        let path = temp_csv("alt.csv", "id,avatar\n1,xx\n");
        let mut draft = draft_for(&app, &["id", "avatar"], &path);
        draft.preview_rows = vec![vec![Some("1".into()), Some("xx".into())]];
        assert_eq!(draft.binary_conflicts(), vec!["avatar"]);
        assert_eq!(draft.unmapped_required(), vec!["email"]);
        app.import_pending = Some(draft);
        render(&mut app);

        // Empty file: no headers at all.
        let empty = temp_csv("none.csv", "");
        let mut draft = draft_for(&app, &[], &empty);
        draft.preview_rows.clear();
        app.import_pending = Some(draft);
        render(&mut app);
        assert!(app.import_pending.is_some());

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&empty).ok();
    }

    /// Toggling the header checkbox re-reads the file: the first row becomes data, the source
    /// columns get synthetic names, and the name-based mapping falls away.
    #[test]
    fn import_toggling_header_rereads_the_file_and_remaps() {
        let mut app = app_with_users_table(users_columns());
        let path = temp_csv("hdr.csv", "id,email,age\n1,a@b.c,30\n");
        app.import_pending = Some(draft_for(&app, &["id", "email", "age"], &path));
        assert_eq!(
            app.import_pending.as_ref().unwrap().mapping,
            vec![Some(0), Some(1), Some(2)]
        );

        app.apply_action(Action::SetImportHasHeader(false));
        let draft = app.import_pending.as_ref().unwrap();
        assert!(!draft.has_header);
        assert_eq!(draft.headers, ["column_1", "column_2", "column_3"]);
        assert_eq!(draft.preview_rows.len(), 2, "the header row is now data");
        assert_eq!(
            draft.mapping,
            vec![None, None, None],
            "synthetic names match nothing, so the user must map explicitly"
        );
        std::fs::remove_file(&path).ok();
    }

    /// The pager rewrites the tab's LIMIT/OFFSET in place and never runs past a known end.
    #[test]
    fn pager_rewrites_sql_and_respects_total() {
        let mut app = DbGuiApp::construct();
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "conn".into(),
            db: std::sync::Arc::new(DummyDb),
            schema: fake_schema(1, 2),
            databases: Vec::new(),
        });
        {
            let tab = app.tab_mut();
            tab.conn_id = Some("c1".into());
            tab.sql = "SELECT * FROM table_0 LIMIT 100;".into();
            tab.edits.source = Some(EditSource {
                schema: None,
                table: "table_0".into(),
                pk_cols: vec!["field_0".into()],
            });
            tab.total_rows = Some(250);
        }

        let go = |app: &mut DbGuiApp, action: Action| {
            app.busy = Busy::Idle; // each page flip leaves a query in flight
            app.apply_action(action);
        };

        go(&mut app, Action::Page(PageNav::Next));
        assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100 OFFSET 100;");
        go(&mut app, Action::Page(PageNav::Last));
        assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100 OFFSET 200;");
        // Past the known end → no-op.
        go(&mut app, Action::Page(PageNav::Next));
        assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100 OFFSET 200;");
        go(&mut app, Action::Page(PageNav::Prev));
        assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100 OFFSET 100;");
        go(&mut app, Action::Page(PageNav::First));
        assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100;");
        // Changing the page size snaps the offset onto the new grid.
        go(&mut app, Action::Page(PageNav::Last));
        go(&mut app, Action::SetPageSize(500));
        assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 500;");
        // The rewrite keeps the tab editable (a fresh pending source is derived).
        assert!(app.tab().edits.pending_source.is_some());
    }

    /// A primary-key-less table (e.g. an imported dump) is browsable but read-only. Paging it
    /// must keep working: the source *identity* the pager keys off has to survive a page flip,
    /// even though the rows can't be edited. (Regression: `derive_edit_source` dropped the
    /// source for PK-less tables, so the pager — gated on `source.is_some()` — vanished the
    /// moment you pressed Next or changed the page size, after showing fine on page one.)
    #[test]
    fn pager_survives_on_pk_less_table() {
        let mut app = DbGuiApp::construct();
        let mut schema = fake_schema(1, 2);
        for col in &mut schema.tables[0].columns {
            col.primary_key = false; // imported dump: no primary key at all
        }
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "conn".into(),
            db: std::sync::Arc::new(DummyDb),
            schema,
            databases: Vec::new(),
        });
        {
            let tab = app.tab_mut();
            tab.conn_id = Some("c1".into());
            tab.sql = "SELECT * FROM table_0 LIMIT 100;".into();
            // Opened from the sidebar: source present but PK-less, so the grid is read-only.
            tab.edits.source = Some(EditSource {
                schema: None,
                table: "table_0".into(),
                pk_cols: Vec::new(),
            });
            tab.total_rows = Some(250);
        }
        assert!(
            !app.tab().edits.editable(),
            "a PK-less table must not be editable"
        );

        app.busy = Busy::Idle;
        app.apply_action(Action::Page(PageNav::Next));
        // The page advanced …
        assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100 OFFSET 100;");
        // … and the source survived, so the pager stays visible on page two and beyond.
        let src = app.tab().edits.pending_source.as_ref();
        assert!(
            src.is_some(),
            "paging a PK-less table must keep its source so the pager stays visible"
        );
        // Keeping the identity must not make a PK-less table editable.
        assert!(src.is_some_and(|s| !s.editable()));
    }

    /// Copy-as-CSV wiring: a multi-row selection routed through `Action::CopyRows` stages the
    /// CSV (header + the selected rows, in display order) in `copy_buffer` for `draw` to flush.
    #[test]
    fn copy_rows_action_stages_csv_for_selection() {
        let mut app = DbGuiApp::construct();
        let result = QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    type_name: "INTEGER".into(),
                },
                ColumnMeta {
                    name: "name".into(),
                    type_name: "TEXT".into(),
                },
            ],
            rows: vec![
                vec![Value::Int(1), Value::Text("a".into())],
                vec![Value::Int(2), Value::Text("b".into())],
                vec![Value::Int(3), Value::Text("c".into())],
            ],
            stats: QueryStats::default(),
            truncated: false,
        };
        app.tab_mut().set_result(result);
        // Select rows 0 and 2 (Cmd-click style), skipping row 1.
        app.tab_mut().selection.select_one(0);
        app.tab_mut().selection.toggle(2);

        app.apply_action(Action::CopyRows(dbcore::CopyFormat::Csv));

        let buf = app.copy_buffer.clone().expect("clipboard text staged");
        assert_eq!(buf, "id,name\r\n1,a\r\n3,c\r\n");
        assert!(app.status_msg.contains("Copied 2"));
    }

    /// End-to-end: the OS delivers Cmd/Ctrl+C as an `Event::Copy` (never a raw `Key::C` press on
    /// macOS), so a real frame fed that event must actually push the selected rows to the
    /// clipboard. (Regression: the handler matched `key_pressed(Key::C)` and so never fired.)
    #[test]
    fn copy_event_pushes_selection_to_clipboard() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        let result = QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    type_name: "INTEGER".into(),
                },
                ColumnMeta {
                    name: "name".into(),
                    type_name: "TEXT".into(),
                },
            ],
            rows: vec![
                vec![Value::Int(1), Value::Text("a".into())],
                vec![Value::Int(2), Value::Text("b".into())],
            ],
            stats: QueryStats::default(),
            truncated: false,
        };
        app.tab_mut().set_result(result);
        app.tab_mut().selection.select_all(2);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| app.draw(ui, None));

        let copied = out.platform_output.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        });
        // Cmd/Ctrl+C copies TSV (no header, no trailing newline) for clean spreadsheet round-trip.
        assert_eq!(copied.as_deref(), Some("1\ta\n2\tb"));
    }

    /// Paste round-trips a copy: TSV clipboard text becomes new staged insert rows on an
    /// editable table, fields typed by column kind (id parses to an int) and mapped by position.
    #[test]
    fn paste_rows_adds_typed_insert_rows() {
        let mut app = DbGuiApp::construct();
        let result = QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    type_name: "INTEGER".into(),
                },
                ColumnMeta {
                    name: "name".into(),
                    type_name: "TEXT".into(),
                },
            ],
            rows: vec![vec![Value::Int(1), Value::Text("a".into())]],
            stats: QueryStats::default(),
            truncated: false,
        };
        app.tab_mut().set_result(result);
        // Make the table editable (a PK column is what unlocks inserts).
        app.tab_mut().edits.source = Some(crate::edit::EditSource {
            schema: None,
            table: "t".into(),
            pk_cols: vec!["id".into()],
        });

        app.apply_action(Action::PasteRows("2\tb\n3\tc".to_string()));

        // Two new (insert) rows were staged …
        assert_eq!(app.tab().edits.new_rows, 2);
        // … with the id column parsed to an Int (not left as text) and the name as text.
        let first = crate::edit::NEW_ROW_BASE;
        assert_eq!(app.tab().edits.staged(first, 0), Some(&Value::Int(2)));
        assert_eq!(
            app.tab().edits.staged(first, 1),
            Some(&Value::Text("b".into()))
        );
        // … and the pasted rows are selected for review.
        assert_eq!(app.tab().selection.len(), 2);
    }

    /// Undo/redo run through the app the same way the Cmd/Ctrl+Z shortcut does: a whole paste
    /// is one undo step, and redo replays it. Exercises the `Action::Undo`/`Action::Redo` path
    /// (flush editor → step history → recompute view) end to end.
    #[test]
    fn undo_redo_actions_step_staged_edits() {
        let mut app = DbGuiApp::construct();
        let result = QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    type_name: "INTEGER".into(),
                },
                ColumnMeta {
                    name: "name".into(),
                    type_name: "TEXT".into(),
                },
            ],
            rows: vec![vec![Value::Int(1), Value::Text("a".into())]],
            stats: QueryStats::default(),
            truncated: false,
        };
        app.tab_mut().set_result(result);
        app.tab_mut().edits.source = Some(crate::edit::EditSource {
            schema: None,
            table: "t".into(),
            pk_cols: vec!["id".into()],
        });

        // A stored-cell edit, then a two-row paste — two separate undo steps.
        app.tab_mut()
            .edits
            .stage(0, 1, Value::Text("edited".into()), &Value::Text("a".into()));
        app.apply_action(Action::PasteRows("2\tb\n3\tc".to_string()));
        assert_eq!(app.tab().edits.new_rows, 2);

        // Undo drops the whole paste in one step; the cell edit survives.
        app.apply_action(Action::Undo);
        assert_eq!(app.tab().edits.new_rows, 0, "paste undone in a single step");
        assert_eq!(
            app.tab().edits.staged(0, 1),
            Some(&Value::Text("edited".into()))
        );

        // A second undo reverts the cell edit; nothing pending remains.
        app.apply_action(Action::Undo);
        assert_eq!(app.tab().edits.staged(0, 1), None);
        assert!(!app.tab().edits.has_pending());

        // Redo replays the cell edit, then the paste.
        app.apply_action(Action::Redo);
        assert_eq!(
            app.tab().edits.staged(0, 1),
            Some(&Value::Text("edited".into()))
        );
        app.apply_action(Action::Redo);
        assert_eq!(app.tab().edits.new_rows, 2);
    }

    /// Paste into a read-only result is a no-op with a hint (no phantom rows).
    #[test]
    fn paste_rows_ignored_when_not_editable() {
        let mut app = DbGuiApp::construct();
        let result = QueryResult {
            columns: vec![ColumnMeta {
                name: "x".into(),
                type_name: "TEXT".into(),
            }],
            rows: vec![vec![Value::Text("a".into())]],
            stats: QueryStats::default(),
            truncated: false,
        };
        app.tab_mut().set_result(result); // no edit source → read-only
        app.apply_action(Action::PasteRows("b\nc".to_string()));
        assert_eq!(app.tab().edits.new_rows, 0);
    }

    fn collect_clash_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    let s = t.galley.text();
                    if s.contains('🔥') {
                        out.push(s.to_string());
                    }
                }
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for cs in shapes {
            walk(&cs.shape, &mut out);
        }
        out
    }

    /// Sanity check: a deliberately-clashing UI must be detected by `collect_clash_text`,
    /// proving the probe below is meaningful when it reports *no* clashes.
    #[test]
    fn detector_catches_known_clash() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| {
            // Two widgets forced to the same Id at different rects → guaranteed clash.
            let id = egui::Id::new("intentional_clash");
            ui.interact(
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0)),
                id,
                egui::Sense::click(),
            );
            ui.interact(
                egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(10.0, 10.0)),
                id,
                egui::Sense::click(),
            );
        });
        assert!(
            !collect_clash_text(&out.shapes).is_empty(),
            "detector failed to catch an intentional clash"
        );
    }

    /// Filtering narrows `row_order` to the matching rows, and clearing restores them all.
    #[test]
    fn filter_recomputes_view() {
        let mut app = DbGuiApp::construct();
        let tab = app.tab_mut();
        // 10 rows, col 0 = 0..10. Keep rows where col0 < 4.
        tab.set_result(fake_result(10, 2));
        assert_eq!(tab.row_order.len(), 10);

        tab.filter.visible = true;
        tab.filter.conditions = vec![crate::filter::Condition {
            enabled: true,
            column: 0,
            op: crate::filter::FilterOp::Less,
            value: "8".into(), // col0 values step by `cols`=2: 0,2,4,6,8,... → <8 keeps 4 rows
        }];
        tab.recompute_view();
        assert_eq!(tab.row_order.len(), 4);

        tab.filter.reset();
        tab.recompute_view();
        assert_eq!(tab.row_order.len(), 10);
    }

    /// A new app always has exactly one tab, and `active()` resolves through the active tab's
    /// connection binding.
    #[test]
    fn active_resolves_through_tab_binding() {
        let mut app = DbGuiApp::construct();
        assert_eq!(app.tabs.len(), 1);
        assert!(app.active().is_none()); // unbound tab → no connection

        // Make a live connection and bind the active tab to it.
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(2, 2),
        });
        app.tab_mut().conn_id = Some("c1".into());
        assert!(app.active().is_some());
        assert_eq!(app.active().unwrap().config_id, "c1");

        // A second tab bound to nothing resolves to no connection again.
        app.new_tab();
        assert_eq!(app.tabs.len(), 2);
        // new_tab inherits the previous tab's connection, so it should still resolve.
        assert_eq!(app.active().unwrap().config_id, "c1");
        app.tab_mut().conn_id = None;
        assert!(app.active().is_none());
    }

    /// Disconnect drops cached results for bound tabs so stale rows don't linger on screen.
    #[test]
    fn disconnect_clears_bound_tab_results() {
        let mut app = DbGuiApp::construct();
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(1, 1),
        });
        app.tab_mut().conn_id = Some("c1".into());
        app.tab_mut().set_result(fake_result(4, 2));
        app.tab_mut().edits.source = Some(crate::edit::EditSource {
            schema: None,
            table: "table_0".into(),
            pk_cols: vec!["field_0".into()],
        });

        app.disconnect_conn("c1");

        assert!(app.active().is_none());
        assert!(app.tab().result.is_none());
        assert!(app.tab().row_order.is_empty());
        assert!(app.tab().edits.source.is_some()); // table identity kept for sidebar dedupe
    }

    /// Re-selecting an already-open table after reconnect must re-run its query.
    #[test]
    fn reopen_table_after_disconnect_starts_query() {
        let src = crate::edit::EditSource {
            schema: None,
            table: "users".into(),
            pk_cols: vec!["id".into()],
        };
        let mut app = DbGuiApp::construct();
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(1, 1),
        });
        app.tab_mut().conn_id = Some("c1".into());
        app.tab_mut().sql = "SELECT * FROM users".into();
        app.tab_mut().set_result(fake_result(3, 2));
        app.tab_mut().edits.source = Some(src.clone());

        app.disconnect_conn("c1");
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db: std::sync::Arc::new(DummyDb),
            databases: Vec::new(),
            schema: fake_schema(1, 1),
        });

        app.open_table("SELECT * FROM users".into(), src, false);

        assert_eq!(app.querying_tab_id, Some(app.tab().id));
        assert!(app.tab().result.is_none());
    }

    /// The Beautify action reformats the active tab's SQL in the bound connection's
    /// dialect, marks the workspace dirty, and leaves staged-edit state untouched.
    #[test]
    fn beautify_reformats_active_tab() {
        let mut app = DbGuiApp::construct();
        app.beautify = crate::format::BeautifyPrefs::default();
        app.tab_mut().sql = "select id, name from users where id = 1".into();
        app.workspace_dirty = false;
        app.beautify_sql();
        assert_eq!(
            app.tab().sql,
            "SELECT\n  id,\n  name\nFROM\n  users\nWHERE\n  id = 1"
        );
        assert!(app.workspace_dirty);

        // Already-formatted SQL is a no-op: no dirty flag, no status churn.
        app.workspace_dirty = false;
        app.beautify_sql();
        assert!(!app.workspace_dirty);

        // Empty SQL never panics or dirties anything.
        app.tab_mut().sql = "   ".into();
        app.beautify_sql();
        assert_eq!(app.tab().sql, "   ");
        assert!(!app.workspace_dirty);
    }

    /// Drag-to-reorder: `move_tab` moves a tab to its target slot in both directions,
    /// keeps the active tab the same logical tab, and ignores out-of-range moves.
    #[test]
    fn move_tab_reorders_and_tracks_active() {
        let mut app = DbGuiApp::construct();
        // Three tabs with recognisable SQL; ids 0, 1, 2.
        app.tab_mut().sql = "q0".into();
        app.new_tab();
        app.tab_mut().sql = "q1".into();
        app.new_tab();
        app.tab_mut().sql = "q2".into();
        app.select_tab(0);

        let order =
            |app: &DbGuiApp| -> Vec<String> { app.tabs.iter().map(|t| t.sql.clone()).collect() };

        // Drag the first tab to the end; the active tab (q0) follows its new position.
        app.move_tab(0, 2);
        assert_eq!(order(&app), ["q1", "q2", "q0"]);
        assert_eq!(app.active_query_tab, 2);
        assert_eq!(app.tab().sql, "q0");

        // Drag a tab leftwards; the active tab keeps pointing at q0.
        app.move_tab(1, 0);
        assert_eq!(order(&app), ["q2", "q1", "q0"]);
        assert_eq!(app.tab().sql, "q0");

        // No-op and out-of-range moves change nothing.
        app.move_tab(1, 1);
        app.move_tab(5, 0);
        app.move_tab(0, 5);
        assert_eq!(order(&app), ["q2", "q1", "q0"]);
    }

    /// Find the painted position of the first text run containing `needle`.
    fn find_text_pos(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Option<egui::Pos2> {
        fn walk(shape: &egui::epaint::Shape, needle: &str, out: &mut Option<egui::Pos2>) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    if out.is_none() && t.galley.text().contains(needle) {
                        *out = Some(t.pos);
                    }
                }
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, needle, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = None;
        for s in shapes {
            walk(&s.shape, needle, &mut out);
        }
        out
    }

    /// End-to-end drag-to-reorder: simulate a real pointer press → move → release over
    /// the tab strip and assert the tab order actually changes.
    #[test]
    fn drag_reorders_tabs_headlessly() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        app.tab_mut().sql = "q0".into();
        app.new_tab();
        app.tab_mut().sql = "q1".into();
        app.new_tab();
        app.tab_mut().sql = "q2".into();
        app.select_tab(0);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let run = |app: &mut DbGuiApp, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            ctx.run_ui(raw, |ui| app.draw(ui, None))
        };

        // Lay out once and locate the first and last chips by their painted labels.
        let out = run(&mut app, vec![]);
        let q1 = find_text_pos(&out.shapes, "Query 1").expect("Query 1 chip not painted");
        let q3 = find_text_pos(&out.shapes, "Query 3").expect("Query 3 chip not painted");
        // Grab inside the label (text pos is its top-left), clear of the × hit area.
        let start = q1 + egui::vec2(4.0, 6.0);
        let end = egui::pos2(q3.x + 80.0, start.y);

        run(&mut app, vec![egui::Event::PointerMoved(start)]);
        run(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }],
        );
        // Drag rightwards in steps, well past egui's is-this-a-drag threshold.
        let steps = 8;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let pos = start + (end - start) * t;
            run(&mut app, vec![egui::Event::PointerMoved(pos)]);
        }
        run(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        run(&mut app, vec![]); // settle frame: drag state clears

        let order: Vec<&str> = app.tabs.iter().map(|t| t.sql.as_str()).collect();
        assert_eq!(order, ["q1", "q2", "q0"], "drag did not reorder the tabs");
        assert_eq!(app.tab().sql, "q0", "dragged tab should stay active");
        assert!(app.tab_drag.is_none(), "drag state should clear on release");
    }

    /// Switching tabs swaps the active result; per-tab state stays independent.
    #[test]
    fn tabs_keep_independent_state() {
        let mut app = DbGuiApp::construct();
        app.tab_mut().set_result(fake_result(5, 2));
        app.new_tab(); // tab 1, empty
        assert!(app.tab().result.is_none());
        app.select_tab(0);
        assert!(app.tab().result.is_some());
        assert_eq!(app.tab().row_order.len(), 5);
    }

    /// Opening tables: the single italic preview tab is reused, an already-open table is
    /// re-activated rather than duplicated, and pinning makes a tab permanent.
    #[test]
    fn open_table_previews_dedupes_and_pins() {
        // No live connection, so `start_query_for` returns early (no background spawn) but the
        // tab is still set up — exactly the state we assert on.
        let src = |t: &str| EditSource {
            schema: None,
            table: t.into(),
            pk_cols: vec!["id".into()],
        };

        let mut app = DbGuiApp::construct();
        app.tab_mut().sql.clear(); // make the single default tab a blank scratch tab
                                   // First table reuses the blank scratch tab as a preview.
        app.open_table("q".into(), src("users"), false);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tab().preview);
        assert_eq!(app.tab().title, "users");

        // Re-opening the same table doesn't add a tab.
        app.open_table("q".into(), src("users"), false);
        assert_eq!(app.tabs.len(), 1);

        // A different table reuses the same preview slot (no pile-up).
        app.open_table("q".into(), src("orders"), false);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tab().title, "orders");
        assert!(app.tab().preview);

        // Pinning the open table (double-click) makes it permanent.
        app.open_table("q".into(), src("orders"), true);
        assert_eq!(app.tabs.len(), 1);
        assert!(!app.tab().preview);

        // With no preview slot and a non-scratch active tab, a new table opens a new tab.
        app.open_table("q".into(), src("products"), false);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tab().title, "products");
        assert!(app.tab().preview);
    }

    /// Closing the only tab keeps one (blank) tab rather than leaving zero.
    #[test]
    fn closing_last_tab_keeps_one() {
        let mut app = DbGuiApp::construct();
        app.tab_mut().sql = "SELECT 99;".into();
        app.close_tab(0);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_query_tab, 0);
        assert_eq!(app.tab().sql, ""); // reset to a blank scratch tab
    }

    /// `structure_table` resolves the tab's source table against its live connection's
    /// schema (case-insensitively), and returns `None` when either side is missing.
    #[test]
    fn structure_table_resolves_source() {
        let mut app = DbGuiApp::construct();
        assert!(app.structure_table(0).is_none()); // no source, no connection

        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(3, 4),
        });
        app.tab_mut().conn_id = Some("c1".into());
        assert!(app.structure_table(0).is_none()); // connected, but a plain query tab

        app.tab_mut().edits.source = Some(EditSource {
            schema: None,
            table: "TABLE_1".into(), // matches case-insensitively
            pk_cols: vec!["field_0".into()],
        });
        let info = app.structure_table(0).expect("source table should resolve");
        assert_eq!(info.name, "table_1");
        assert_eq!(info.columns.len(), 4);

        // Connection drops → no schema to describe.
        app.tab_mut().conn_id = None;
        assert!(app.structure_table(0).is_none());
    }

    /// Render the Structure view headlessly (a table tab switched to Structure mode) and
    /// capture ID clashes between its columns/indexes grids. Also checks the mode survives
    /// drawing — `view_mode_bar` must not force it back to Data while the table resolves.
    #[test]
    fn probe_structure_view_id_clash() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(3, 30),
        });
        {
            let tab = app.tab_mut();
            tab.conn_id = Some("c1".into());
            tab.edits.source = Some(EditSource {
                schema: None,
                table: "table_1".into(),
                pk_cols: vec!["field_0".into()],
            });
            tab.view = TabView::Structure;
        }

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        for _ in 0..5 {
            let events = vec![
                egui::Event::PointerMoved(egui::pos2(500.0, 350.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -20.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                },
            ];
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
            clashes.extend(collect_clash_text(&out.shapes));
        }

        assert!(app.tab().view == TabView::Structure);
        clashes.sort();
        clashes.dedup();
        assert!(
            clashes.is_empty(),
            "ID clashes detected in structure view:\n{}",
            clashes.join("\n")
        );
    }

    /// Render the inline schema editor headlessly (Edit Table now occupies the central
    /// panel instead of a dialog) across its three tabs, catching panics and ID clashes.
    /// Also checks it stays open across frames and closes via CancelSchema.
    #[test]
    fn probe_inline_schema_editor() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(2, 6),
        });
        {
            let tab = app.tab_mut();
            tab.conn_id = Some("c1".into());
            tab.edits.source = Some(EditSource {
                schema: None,
                table: "table_0".into(),
                pk_cols: vec!["field_0".into()],
            });
        }
        let info = app.structure_table(0).cloned().expect("table resolves");
        app.apply_action(Action::OpenEditTable(info));
        assert!(app.tab().schema_editor.is_some());

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        let tabs = [
            crate::schema::SchemaTab::Columns,
            crate::schema::SchemaTab::Indexes,
            crate::schema::SchemaTab::ForeignKeys,
        ];
        for tab in tabs {
            if let Some(ObjectEditor::Table(e)) = app.tab_mut().schema_editor.as_mut() {
                e.active_tab = tab;
            }
            for _ in 0..3 {
                let raw = egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(egui::pos2(500.0, 350.0))],
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
                clashes.extend(collect_clash_text(&out.shapes));
            }
            assert!(
                app.tab().schema_editor.is_some(),
                "editor must survive drawing"
            );
        }
        clashes.sort();
        clashes.dedup();
        assert!(
            clashes.is_empty(),
            "ID clashes in inline schema editor:\n{}",
            clashes.join("\n")
        );

        // Cancel returns the central panel to the grid views.
        app.apply_action(Action::CancelSchema);
        assert!(app.tab().schema_editor.is_none());
    }

    /// The schema explorer renders pinned + unpinned table rows without id clashes. A pinned
    /// table appears both in the "Pinned" group and the main list, so the two rows must key
    /// their collapsing state independently (different `id_salt`).
    #[test]
    fn probe_schema_explorer_bookmarks() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(3, 4),
        });
        app.tab_mut().conn_id = Some("c1".into());
        // Pin one table so it shows in both the "Pinned" group and the main list, and make it
        // the active tab's table so the selection pill draws too.
        app.bookmarks = vec![dbcore::Bookmark {
            conn_id: "c1".into(),
            schema: None,
            table: "table_0".into(),
        }];
        app.tab_mut().edits.source = Some(EditSource {
            schema: None,
            table: "table_0".into(),
            pk_cols: vec!["field_0".into()],
        });

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        for _ in 0..4 {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                // Hover near the top of the tree to exercise the hover fill + star paint.
                events: vec![egui::Event::PointerMoved(egui::pos2(120.0, 120.0))],
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
            clashes.extend(collect_clash_text(&out.shapes));
        }
        clashes.sort();
        clashes.dedup();
        assert!(
            clashes.is_empty(),
            "ID clashes in schema explorer:\n{}",
            clashes.join("\n")
        );
    }

    /// Build an app with a live SQLite connection carrying a table, view, and trigger. Returns
    /// the app and the temp-db path (delete when done). Shared by the screenshot generators.
    fn demo_app_with_objects() -> (DbGuiApp, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        // Unique per call: the two screenshot tests run in one process and must not share a file.
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "plusplus-snap-{}-{}.sqlite",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = std::fs::remove_file(&path);
        let mut cfg = dbcore::ConnectionConfig::new(DbKind::Sqlite);
        cfg.name = "demo".into();
        cfg.sqlite_path = path.to_string_lossy().into_owned();
        let (db, schema): (Arc<dyn dbcore::Database>, SchemaTree) = rt.block_on(async {
            let db = dbcore::connect(&cfg, None, None).await.unwrap();
            for stmt in [
                "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)",
                "CREATE TABLE audit (id INTEGER PRIMARY KEY, msg TEXT)",
                "CREATE VIEW active_users AS SELECT id, email FROM users WHERE email IS NOT NULL",
                "CREATE TRIGGER log_new_user AFTER INSERT ON users FOR EACH ROW \
                 BEGIN INSERT INTO audit(msg) VALUES ('new user'); END",
            ] {
                db.execute(stmt).await.unwrap();
            }
            let schema = db.introspect().await.unwrap();
            (db, schema)
        });
        let mut app = DbGuiApp::construct();
        app.show_schema_panel = true;
        app.active_connections.push(ActiveConnection {
            config_id: cfg.id.clone(),
            name: cfg.name.clone(),
            db,
            databases: Vec::new(),
            schema,
        });
        app.tab_mut().conn_id = Some(cfg.id.clone());
        (app, path)
    }

    /// Render `app` headlessly and write a PNG snapshot named `name`. Optionally expands the
    /// sidebar object groups first. The UI animates a button glint (continuous repaint), so we
    /// step a fixed number of frames rather than running to quiescence.
    fn render_and_snapshot(mut app: DbGuiApp, name: &str, expand_groups: bool) {
        use egui_kittest::kittest::Queryable;
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1180.0, 760.0))
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
            });
        harness.run_steps(4);
        if expand_groups {
            for label in ["Views (1)", "Triggers (1)"] {
                if harness.query_by_label(label).is_some() {
                    harness.get_by_label(label).click();
                    harness.run_steps(4);
                }
            }
        }
        harness.run_steps(6);
        harness.snapshot(name);
    }

    /// Screenshot generator (ignored): the import dialog with a realistic mapping — one column
    /// auto-matched, one renamed in the file, one skipped.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_import_dialog() {
        let mut app = app_with_users_table(vec![
            col("id", "INTEGER", false, true),
            col("email", "VARCHAR(255)", false, false),
            col("full_name", "TEXT", true, false),
            col("age", "INTEGER", true, false),
            col("created_at", "TIMESTAMP", true, false),
            col("is_active", "BOOLEAN", true, false),
        ]);
        // A stable file name: `temp_csv` embeds the pid, which would make the committed PNG
        // churn on every regeneration.
        let path = std::env::temp_dir().join("plusplus-snapshot-users.csv");
        std::fs::write(
            &path,
            "id,Email,age,created_at,is_active,legacy_note\n\
             1,ada@lovelace.org,36,2026-07-10 09:15:00,true,imported from v1\n\
             2,grace@hopper.mil,45,2026-07-10 09:16:30,true,\n\
             3,alan@turing.uk,41,2026-07-10 09:18:02,false,archived\n",
        )
        .unwrap();
        let mut draft = draft_for(
            &app,
            &["id", "Email", "age", "created_at", "is_active", "legacy_note"],
            &path,
        );
        draft.preview_rows = vec![
            vec![
                Some("1".into()),
                Some("ada@lovelace.org".into()),
                Some("36".into()),
                Some("2026-07-10 09:15:00".into()),
                Some("true".into()),
                Some("imported from v1".into()),
            ],
            vec![
                Some("2".into()),
                Some("grace@hopper.mil".into()),
                Some("45".into()),
                Some("2026-07-10 09:16:30".into()),
                Some("true".into()),
                None,
            ],
        ];
        draft.more = true;
        app.import_pending = Some(draft);

        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(940.0, 700.0))
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    bind_heading_font(ui.ctx());
                    setup = true;
                    // `set_fonts` lands at the end of the frame, and the dialog title asks for
                    // the `heading` family — draw nothing until it is bound.
                    return;
                }
                app.draw(ui, None);
            });
        harness.run_steps(8);
        harness.snapshot("import_dialog");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[ignore = "temporary repro"]
    fn snapshot_import_scrolled() {
        let columns: Vec<_> = (0..14)
            .map(|i| col(&format!("column_{i:02}"), "INTEGER", false, false))
            .collect();
        let mut app = app_with_users_table(columns);
        let path = std::env::temp_dir().join("plusplus-scroll-probe.csv");
        std::fs::write(&path, "Task Name\nA\n").unwrap();
        let mut draft = draft_for(&app, &["Task Name"], &path);
        draft.preview_rows = (0..6).map(|i| vec![Some(format!("row-{i}"))]).collect();
        draft.more = true;
        app.import_pending = Some(draft);

        let mut setup = false;
        let mut scrolled = 0;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 760.0))
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    bind_heading_font(ui.ctx());
                    setup = true;
                    return;
                }
                if scrolled < 30 {
                    scrolled += 1;
                    ui.ctx().input_mut(|i| {
                        i.events.push(egui::Event::PointerMoved(egui::pos2(300.0, 400.0)));
                        i.events.push(egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, -30.0),
                            phase: egui::TouchPhase::Move,
                            modifiers: egui::Modifiers::default(),
                        });
                    });
                }
                app.draw(ui, None);
            });
        harness.run_steps(34);
        harness.snapshot("import_scrolled");
        let _ = std::fs::remove_file(&path);
    }

    /// Screenshot generator (ignored): a table with more columns than fit, to check that the
    /// single body scroll engages and the footer stays put.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_import_dialog_many_columns() {
        let types = [
            "INTEGER",
            "VARCHAR(255)",
            "TEXT",
            "TIMESTAMP",
            "BOOLEAN",
            "NUMERIC(10,2)",
        ];
        let columns: Vec<_> = (0..18)
            .map(|i| col(&format!("column_{i:02}"), types[i % types.len()], true, i == 0))
            .collect();
        let mut app = app_with_users_table(columns);

        let headers: Vec<String> = (0..18).map(|i| format!("column_{i:02}")).collect();
        let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        let path = std::env::temp_dir().join("plusplus-snapshot-wide.csv");
        std::fs::write(&path, format!("{}\n", refs.join(","))).unwrap();

        let mut draft = draft_for(&app, &refs, &path);
        draft.preview_rows = (0..6)
            .map(|r| (0..18).map(|c| Some(format!("v{r}_{c}"))).collect())
            .collect();
        draft.more = true;
        app.import_pending = Some(draft);

        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(940.0, 700.0))
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    bind_heading_font(ui.ctx());
                    setup = true;
                    return;
                }
                app.draw(ui, None);
            });
        harness.run_steps(8);
        harness.snapshot("import_dialog_many_columns");
        let _ = std::fs::remove_file(&path);
    }

    /// Screenshot generator (ignored in normal runs): the schema sidebar with its Views and
    /// Triggers groups expanded. Run with:
    /// `UPDATE_SNAPSHOTS=1 cargo test -p plusplus-ui snapshot_ -- --ignored`.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_object_browser() {
        let (app, path) = demo_app_with_objects();
        render_and_snapshot(app, "object_browser", true);
        let _ = std::fs::remove_file(&path);
    }

    /// Screenshot generator (ignored): the dialect-adaptive visual Trigger editor, opened on
    /// the demo database's existing trigger.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_trigger_editor() {
        let (mut app, path) = demo_app_with_objects();
        let trigger = app.active().unwrap().schema.triggers[0].clone();
        app.apply_action(Action::OpenEditTrigger(trigger));
        render_and_snapshot(app, "trigger_editor", false);
        let _ = std::fs::remove_file(&path);
    }

    /// Regression: the schema editor must not linger when another table is opened — it
    /// belongs to the tab it was opened on, and comes back when switching back.
    #[test]
    fn schema_editor_is_per_tab() {
        let mut app = DbGuiApp::construct();
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(2, 3),
        });
        {
            let tab = app.tab_mut();
            tab.conn_id = Some("c1".into());
            tab.edits.source = Some(EditSource {
                schema: None,
                table: "table_0".into(),
                pk_cols: vec!["field_0".into()],
            });
        }
        let info = app.structure_table(0).cloned().expect("table resolves");
        app.apply_action(Action::OpenEditTable(info));
        assert!(app.tab().schema_editor.is_some());

        // Open a different table from the sidebar: lands on a fresh tab with no editor.
        app.apply_action(Action::OpenTable {
            sql: "SELECT * FROM table_1 LIMIT 100;".into(),
            source: EditSource {
                schema: None,
                table: "table_1".into(),
                pk_cols: vec!["field_0".into()],
            },
            pin: false,
        });
        assert!(
            app.tab().schema_editor.is_none(),
            "editor must not follow to a new table"
        );

        // ...but the original tab still holds its in-progress editor.
        app.apply_action(Action::SelectTab(0));
        assert!(app.tab().schema_editor.is_some());
    }

    /// Drive the Details panel headlessly with one column per editor kind, editable, so
    /// the type-aware widgets (type badges, boolean checkbox, date picker) all render.
    /// Catches panics and ID clashes in the per-column widgets (e.g. the per-column
    /// date-picker salts).
    #[test]
    fn probe_details_panel_typed_columns() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        let columns = [
            ("id", "INTEGER"),
            ("price", "DECIMAL(10,2)"),
            ("ratio", "REAL"),
            ("active", "BOOLEAN"),
            ("born", "DATE"),
            ("seen", "TIMESTAMP"),
            ("name", "TEXT"),
        ];
        let result = QueryResult {
            columns: columns
                .iter()
                .map(|(n, t)| ColumnMeta {
                    name: (*n).into(),
                    type_name: (*t).into(),
                })
                .collect(),
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Text("19.99".into()),
                    Value::Float(0.5),
                    Value::Bool(true),
                    Value::Text("2024-05-01".into()),
                    Value::Text("2024-05-01 10:30:00".into()),
                    Value::Text("ปลาทู".into()),
                ],
                // A NULL-heavy row exercises the NULL fallbacks of every kind.
                vec![Value::Null; 7],
            ],
            stats: QueryStats::default(),
            truncated: false,
        };
        {
            let tab = app.tab_mut();
            tab.set_result(result);
            tab.selection.select_one(0);
            tab.edits.source = Some(crate::edit::EditSource {
                schema: None,
                table: "t".into(),
                pk_cols: vec!["id".into()],
            });
        }

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        for row in [0usize, 1] {
            app.tab_mut().selection.select_one(row);
            for _ in 0..3 {
                let raw = egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(egui::pos2(880.0, 300.0))],
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
                clashes.extend(collect_clash_text(&out.shapes));
            }
        }
        clashes.sort();
        clashes.dedup();
        assert!(
            clashes.is_empty(),
            "ID clashes in typed Details panel:\n{}",
            clashes.join("\n")
        );
    }

    /// Clicking a Details-panel value box must open the inline editor, give it focus, and
    /// accept typed characters (regression: the editor opened but typing went nowhere).
    #[test]
    fn details_box_click_then_type() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        let result = QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    type_name: "INTEGER".into(),
                },
                ColumnMeta {
                    name: "name".into(),
                    type_name: "TEXT".into(),
                },
            ],
            rows: vec![vec![Value::Int(13), Value::Text("Coffee".into())]],
            stats: QueryStats::default(),
            truncated: false,
        };
        {
            let tab = app.tab_mut();
            tab.set_result(result);
            tab.selection.select_one(0);
            tab.edits.source = Some(crate::edit::EditSource {
                schema: None,
                table: "t".into(),
                pk_cols: vec!["id".into()],
            });
        }

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let run = |app: &mut DbGuiApp, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            ctx.run_ui(raw, |ui| app.draw(ui, None))
        };

        // Locate the "Coffee" value box and click it.
        let out = run(&mut app, vec![]);
        let pos = find_text_pos(&out.shapes, "Coffee").expect("value box not painted")
            + egui::vec2(4.0, 4.0);
        run(&mut app, vec![egui::Event::PointerMoved(pos)]);
        run(
            &mut app,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }],
        );
        run(
            &mut app,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        // One frame for the editor to appear and request focus, then type.
        run(&mut app, vec![]);
        assert!(
            app.tab().edits.is_active(0, 1),
            "click should open the inline editor"
        );
        run(&mut app, vec![egui::Event::Text("X".into())]);
        let buf = app.tab().edits.active.as_ref().unwrap().buf.clone();
        assert!(
            buf.contains('X'),
            "typed text should reach the editor, buf = {buf:?}"
        );

        // The editor must survive idle frames (no spurious commit/cancel)…
        for _ in 0..3 {
            run(&mut app, vec![egui::Event::PointerMoved(pos)]);
        }
        assert!(
            app.tab().edits.is_active(0, 1),
            "editor should stay open across idle frames"
        );
        // …and a second click inside it (cursor placement) must not close it or kill focus.
        for pressed in [true, false] {
            run(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                }],
            );
        }
        run(&mut app, vec![egui::Event::Text("Y".into())]);
        assert!(
            app.tab().edits.is_active(0, 1),
            "clicking inside the editor should not close it"
        );
        let buf = app.tab().edits.active.as_ref().unwrap().buf.clone();
        assert!(
            buf.contains('Y'),
            "typing after an in-editor click should still work, buf = {buf:?}"
        );
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Set up an app with an editable rows×cols result and return it with a frame-runner
    /// context.
    fn grid_nav_app(rows: usize, cols: usize) -> (egui::Context, DbGuiApp) {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);
        let mut app = DbGuiApp::construct();
        let tab = app.tab_mut();
        tab.set_result(fake_result(rows, cols));
        tab.edits.source = Some(crate::edit::EditSource {
            schema: None,
            table: "t".into(),
            pk_cols: vec!["col0".into()],
        });
        (ctx, app)
    }

    fn run_frame(
        ctx: &egui::Context,
        app: &mut DbGuiApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| app.draw(ui, None))
    }

    /// Arrow keys drive the grid's cell cursor when nothing has keyboard focus: ↑/↓ move
    /// and re-select rows, ←/→ move columns, Shift+↓ extends the range from the anchor.
    #[test]
    fn arrow_keys_move_cursor_and_selection() {
        let (ctx, mut app) = grid_nav_app(5, 3);
        app.tab_mut().selection.select_one(0);

        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        assert_eq!(app.tab().selection.lead(), Some(1));
        assert_eq!(app.tab().selection.cursor(), Some((1, 0)));

        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)],
        );
        assert_eq!(app.tab().selection.cursor(), Some((1, 1)));
        assert_eq!(
            app.tab().selection.lead(),
            Some(1),
            "column move keeps the row"
        );

        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::SHIFT)],
        );
        let rows: Vec<usize> = app.tab().selection.iter().collect();
        assert_eq!(rows, [1, 2], "Shift+Down extends from the anchor");
        assert_eq!(
            app.tab().selection.cursor(),
            Some((2, 1)),
            "cursor keeps its column"
        );
    }

    /// Enter opens the editor on the cursor cell — and the very same Enter press must not
    /// leak into the freshly opened editor and instantly commit it.
    #[test]
    fn enter_opens_editor_at_cursor() {
        let (ctx, mut app) = grid_nav_app(5, 3);
        app.tab_mut().selection.select_one(1);
        app.tab_mut().selection.set_cursor(1, 1);

        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        {
            let active = app
                .tab()
                .edits
                .active
                .as_ref()
                .expect("Enter opens the editor");
            assert_eq!((active.row, active.col), (1, 1));
            assert_eq!(active.origin, crate::edit::EditOrigin::Grid);
            assert_eq!(active.buf, "4"); // row 1 col 1 of fake_result(5, 3)
        }
        run_frame(&ctx, &mut app, vec![]);
        assert!(
            app.tab().edits.is_active(1, 1),
            "editor must survive the frame after opening (Enter must not self-commit)"
        );
        assert!(!app.tab().edits.has_pending(), "nothing staged yet");
    }

    /// Tab commits the open editor and moves it one cell right, spreadsheet-style.
    #[test]
    fn tab_commits_and_advances() {
        let (ctx, mut app) = grid_nav_app(5, 3);
        app.tab_mut().selection.select_one(0); // cursor lands on (0, 0)

        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(app.tab().edits.is_active(0, 0), "editor open at the cursor");
        run_frame(&ctx, &mut app, vec![]); // editor takes focus
        run_frame(&ctx, &mut app, vec![egui::Event::Text("7".into())]);
        let buf = app.tab().edits.active.as_ref().unwrap().buf.clone();
        assert!(
            buf.contains('7'),
            "typed text reaches the editor, buf = {buf:?}"
        );

        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
        );
        assert!(
            app.tab().edits.staged(0, 0).is_some(),
            "Tab commits the edited cell"
        );
        assert!(
            app.tab().edits.is_active(0, 1),
            "Tab moves the editor to the next column"
        );
        assert_eq!(app.tab().selection.cursor(), Some((0, 1)));
    }

    /// Keyboard cursor moves must scroll the grid to keep the cursor visible — vertically
    /// via the table's `scroll_to_row`, and horizontally via the wide-grid ScrollArea (whose
    /// scroll request must be issued outside the table: egui scroll areas swallow pending
    /// scroll targets for *both* axes, so a request set inside the table never escapes its
    /// internal vertical scroll area).
    #[test]
    fn keyboard_cursor_scrolls_into_view() {
        fn painted(shapes: &[egui::epaint::ClippedShape], needle: &str) -> bool {
            fn walk(shape: &egui::epaint::Shape, needle: &str) -> bool {
                match shape {
                    egui::epaint::Shape::Text(t) => t.galley.text() == needle,
                    egui::epaint::Shape::Vec(v) => v.iter().any(|s| walk(s, needle)),
                    _ => false,
                }
            }
            shapes.iter().any(|cs| walk(&cs.shape, needle))
        }

        // Vertical: 200 rows × 3 cols (fits horizontally). Rows are virtualized, so row
        // 151's first cell ("453" = 151*3) is only ever painted once the table scrolled
        // down to it.
        let (ctx, mut app) = grid_nav_app(200, 3);
        app.tab_mut().selection.select_one(150);
        let out = run_frame(&ctx, &mut app, vec![]);
        assert!(
            !painted(&out.shapes, "453"),
            "row 151 must start out of view"
        );
        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        let seen = (0..30).any(|_| {
            let out = run_frame(&ctx, &mut app, vec![]);
            painted(&out.shapes, "453")
        });
        assert!(
            seen,
            "ArrowDown past the viewport must scroll the row into view"
        );

        // Horizontal: 5 rows × 30 cols → wider than the panel → wrapped in the horizontal
        // ScrollArea. Off-screen columns skip their cell text, so cell (0, 25) ("25") is
        // only painted once the grid scrolled sideways to the cursor's column.
        let (ctx, mut app) = grid_nav_app(5, 30);
        app.tab_mut().selection.select_one(0);
        let out = run_frame(&ctx, &mut app, vec![]);
        assert!(
            !painted(&out.shapes, "25"),
            "column 25 must start out of view"
        );
        for _ in 0..25 {
            run_frame(
                &ctx,
                &mut app,
                vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)],
            );
        }
        let seen = (0..30).any(|_| {
            let out = run_frame(&ctx, &mut app, vec![]);
            painted(&out.shapes, "25")
        });
        assert!(
            seen,
            "ArrowRight past the viewport must scroll the column into view"
        );
    }

    /// While a cell editor has focus, arrow keys belong to the text field — the grid cursor
    /// must not move underneath it.
    #[test]
    fn arrows_ignored_while_typing() {
        let (ctx, mut app) = grid_nav_app(5, 3);
        app.tab_mut().selection.select_one(1);
        app.tab_mut().selection.set_cursor(1, 1);

        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        run_frame(&ctx, &mut app, vec![]); // editor takes focus
        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        assert_eq!(
            app.tab().selection.cursor(),
            Some((1, 1)),
            "grid cursor must not move while the editor is open"
        );
        assert!(app.tab().edits.is_active(1, 1), "editor stays open");
    }

    /// Drive the full app layout headlessly while scrolling, and capture egui "ID clash"
    /// markers (🔥) to pinpoint the offending widget.
    #[test]
    fn probe_full_app_id_clash() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);
        ctx.set_pixels_per_point(2.0); // emulate a retina display

        let mut app = DbGuiApp::construct();
        // Add a second tab so the query-tab bar renders multiple chips (exercises its ids).
        app.new_tab();
        app.select_tab(0);
        let result = fake_result(2000, 6);
        {
            let tab = app.tab_mut();
            tab.row_order = (0..result.rows.len()).collect();
            tab.result = Some(result);
            tab.selection.select_one(7); // render the Details panel
            tab.filter.visible = true; // render the filter bar too
            tab.conn_id = Some("test".into());
        }
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "test".into(),
            name: "test-conn".into(),
            db,
            databases: Vec::new(),
            schema: fake_schema(15, 5),
        });

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        for frame in 0..60 {
            // Sweep through many sub-pixel scroll offsets to hit boundary-row states.
            let delta = if frame % 7 == 0 { 13.3 } else { 7.0 };
            let events = vec![
                egui::Event::PointerMoved(egui::pos2(500.0, 350.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -delta),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                },
            ];
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
            clashes.extend(collect_clash_text(&out.shapes));
        }

        clashes.sort();
        clashes.dedup();
        assert!(
            clashes.is_empty(),
            "ID clashes detected:\n{}",
            clashes.join("\n")
        );
    }

    /// The Favorites panel carves a SidePanel inside the query console after the header row;
    /// render it open with entries to confirm that nested layout is clash-free and doesn't
    /// panic (the full-app probe keeps it closed).
    #[test]
    fn probe_favorites_panel_open() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        app.tab_mut().sql = "SELECT * FROM t".into();
        app.favorites_open = true;
        for i in 0..3 {
            app.favorites_cache.push(dbcore::Favorite {
                id: format!("id-{i}"),
                name: format!("Saved query {i}"),
                sql: format!("SELECT {i} FROM t WHERE x = {i}"),
                conn_id: None,
                conn_name: Some("test-conn".into()),
                created_at: "2026-06-24T00:00:00Z".into(),
            });
        }

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        for _ in 0..4 {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
            clashes.extend(collect_clash_text(&out.shapes));
        }
        clashes.sort();
        clashes.dedup();
        assert!(
            clashes.is_empty(),
            "ID clashes detected:\n{}",
            clashes.join("\n")
        );
    }

    /// A small schema with a real FK so ERD tests exercise edges, not just boxes.
    fn fake_schema_with_fk() -> SchemaTree {
        let mut schema = fake_schema(3, 4);
        schema.tables[1].foreign_keys.push(dbcore::ForeignKeyInfo {
            name: "fk_t1_t0".into(),
            columns: vec!["field_1".into()],
            ref_schema: None,
            ref_table: "table_0".into(),
            ref_columns: vec!["field_0".into()],
            on_delete: "CASCADE".into(),
            on_update: "NO ACTION".into(),
        });
        schema
    }

    fn connect_fake(app: &mut DbGuiApp, schema: SchemaTree) {
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "c1".into(),
            name: "one".into(),
            db,
            databases: Vec::new(),
            schema,
        });
        app.tab_mut().conn_id = Some("c1".into());
    }

    /// A result over `field_0..field_{n-1}` (matching [`fake_schema`]'s column names) with one row.
    fn field_result(values: Vec<Value>) -> QueryResult {
        QueryResult {
            columns: (0..values.len())
                .map(|c| ColumnMeta {
                    name: format!("field_{c}"),
                    type_name: "TEXT".into(),
                })
                .collect(),
            rows: vec![values],
            stats: QueryStats::default(),
            truncated: false,
        }
    }

    /// Set up a `table_1` tab (whose `field_1` is a FK → `table_0.field_0`) holding `row`.
    fn fk_tab(row: Vec<Value>) -> DbGuiApp {
        let mut app = DbGuiApp::construct();
        connect_fake(&mut app, fake_schema_with_fk());
        let tab = app.tab_mut();
        tab.edits.source = Some(EditSource {
            schema: None,
            table: "table_1".into(),
            pk_cols: vec!["field_0".into()],
        });
        tab.result = Some(field_result(row));
        app
    }

    /// Following a FK cell builds a filtered `SELECT` of the referenced table (with its PK as
    /// the edit source) and opens it in a reusable preview tab bound to the same connection.
    #[test]
    fn follow_foreign_key_opens_filtered_referenced_table() {
        let mut app = fk_tab(vec![
            Value::Text("row-pk".into()),
            Value::Text("u7".into()),
            Value::Null,
            Value::Null,
        ]);

        // Per-column labels drive the grid's link affordance: only the FK column is tagged.
        assert_eq!(
            app.fk_column_labels(0),
            vec![None, Some("table_0".to_string()), None, None]
        );

        // Resolve the FK at (row 0, col 1 = field_1) → filtered SELECT of table_0.
        let (sql, source) = app
            .build_fk_follow(0, 0, 1)
            .expect("field_1 is a foreign key");
        assert_eq!(
            sql,
            "SELECT * FROM \"table_0\" WHERE \"field_0\" = 'u7' LIMIT 100;"
        );
        assert_eq!(source.table, "table_0");
        assert_eq!(source.schema, None);
        assert_eq!(source.pk_cols, vec!["field_0".to_string()]);

        // The action opens a *second* (preview) tab on the referenced table.
        app.apply_action(Action::FollowForeignKey { row: 0, col: 1 });
        assert_eq!(
            app.tabs.len(),
            2,
            "follow opens a new tab, not clobbering the source"
        );
        let opened = app.tab();
        assert!(
            opened.preview,
            "FK follow lands in the reusable preview tab"
        );
        assert_eq!(opened.conn_id.as_deref(), Some("c1"));
        assert_eq!(
            opened
                .edits
                .pending_source
                .as_ref()
                .map(|s| s.table.as_str()),
            Some("table_0")
        );
        assert_eq!(opened.sql, sql);
    }

    /// A non-FK column, or a NULL foreign-key value, has nothing to follow → status hint, no tab.
    #[test]
    fn follow_foreign_key_noops_on_non_fk_and_null() {
        let mut app = fk_tab(vec![
            Value::Text("pk".into()),
            Value::Null, // the FK column, but empty here
            Value::Null,
            Value::Null,
        ]);
        assert!(
            app.build_fk_follow(0, 0, 0).is_none(),
            "field_0 isn't a foreign key"
        );
        assert!(
            app.build_fk_follow(0, 0, 1).is_none(),
            "NULL FK references nothing"
        );

        app.apply_action(Action::FollowForeignKey { row: 0, col: 1 });
        assert_eq!(app.tabs.len(), 1, "a NULL FK opens no tab");
        assert!(app.status_msg.contains("No foreign key"));
    }

    /// ToggleErd needs a live connection; with one it snapshots the schema, and a second
    /// toggle closes the diagram again.
    #[test]
    fn toggle_erd_builds_from_the_active_connection() {
        let mut app = DbGuiApp::construct();
        app.apply_action(Action::ToggleErd);
        assert!(app.erd.is_none());
        assert!(app.error.is_some(), "no connection should surface an error");

        connect_fake(&mut app, fake_schema_with_fk());
        app.error = None;
        app.apply_action(Action::ToggleErd);
        let erd = app.erd.as_ref().expect("diagram should open");
        assert_eq!(erd.nodes.len(), 3);
        assert_eq!(erd.edges.len(), 1);
        assert_eq!(erd.conn_id, "c1");

        app.apply_action(Action::ToggleErd);
        assert!(app.erd.is_none());
    }

    /// RefreshErd rebuilds from the connection's current schema, keeping the position of
    /// nodes whose table survived; disconnecting closes the stale diagram outright.
    #[test]
    fn erd_refresh_keeps_positions_and_disconnect_closes() {
        let mut app = DbGuiApp::construct();
        connect_fake(&mut app, fake_schema_with_fk());
        app.apply_action(Action::ToggleErd);

        // The user drags table_0 somewhere specific…
        let moved = egui::pos2(1234.0, 567.0);
        app.erd.as_mut().unwrap().nodes[0].pos = moved;

        // …then the schema gains a table and the diagram refreshes.
        app.active_connections[0].schema = {
            let mut s = fake_schema_with_fk();
            s.tables.push(TableInfo {
                schema: None,
                name: "brand_new".into(),
                columns: vec![ColumnInfo {
                    name: "id".into(),
                    data_type: "INTEGER".into(),
                    nullable: false,
                    primary_key: true,
                }],
                indexes: Vec::new(),
                foreign_keys: Vec::new(),
            });
            s
        };
        app.apply_action(Action::RefreshErd);
        let erd = app.erd.as_ref().expect("refresh keeps the diagram open");
        assert_eq!(erd.nodes.len(), 4);
        let kept = erd.nodes.iter().find(|n| n.title == "table_0").unwrap();
        assert_eq!(
            kept.pos, moved,
            "surviving nodes keep their dragged position"
        );

        app.disconnect_conn("c1");
        assert!(app.erd.is_none(), "diagram closes with its connection");
    }

    /// Render the ER diagram headlessly (open over a connected app) and capture ID
    /// clashes; also exercises the Scene's pan/zoom plumbing for a few frames.
    #[test]
    fn probe_erd_view_id_clash() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);

        let mut app = DbGuiApp::construct();
        connect_fake(&mut app, fake_schema_with_fk());
        app.apply_action(Action::ToggleErd);
        assert!(app.erd.is_some());

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let mut clashes: Vec<String> = Vec::new();
        for _ in 0..5 {
            let events = vec![
                egui::Event::PointerMoved(egui::pos2(500.0, 350.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -20.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                },
            ];
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
            clashes.extend(collect_clash_text(&out.shapes));
        }

        assert!(app.erd.is_some(), "the diagram must survive drawing");
        clashes.sort();
        clashes.dedup();
        assert!(
            clashes.is_empty(),
            "ID clashes detected in the ER diagram:\n{}",
            clashes.join("\n")
        );
    }
}
