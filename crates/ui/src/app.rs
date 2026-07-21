//! The application state and the immediate-mode `update` loop.
//!
//! Threading model: the UI never blocks on database I/O. A `tokio` runtime owned by the
//! app runs connect/introspect/query work on background tasks; results come back over an
//! `mpsc` channel that we drain each frame. While work is in flight the UI stays
//! interactive and shows a spinner.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use dbcore::{
    ConnectionColor, ConnectionConfig, Database, DbKind, QueryResult, SchemaTree, TableInfo,
};

use crate::schema::{ObjectEditor, RoutineEditor, SchemaEditor, TriggerEditor, ViewEditor};

mod actions;
mod connection;
mod edits;
mod journal;
mod layout;
mod messages;
mod navigate;
mod panels;
mod query;
mod tabs;
mod transfer;
mod workspace;

use crate::edit::{EditSource, Edits};
use crate::filter::{self, FilterState};
use crate::theme::ThemeRegistry;

/// The most rows a single query will materialize in memory. A `SELECT` over a bigger
/// result streams up to the cap and comes back marked truncated — browse the rest with
/// the pager (table tabs) or a narrower query. ~100k rows keeps even wide results in the
/// hundreds of MB, far below where the grid stops being useful anyway.
const MAX_FETCH_ROWS: usize = 100_000;

fn schema_table_key(schema: Option<&str>, table: &str) -> String {
    format!("{}\0{table}", schema.unwrap_or_default())
}

#[derive(Clone)]
struct SchemaTableDrag {
    conn_id: String,
    schema: Option<String>,
    table: String,
    pinned: bool,
}

/// Messages sent from background tasks back to the UI thread.
enum AppMessage {
    /// The transport/authentication handshake finished. Schema metadata and the database list
    /// arrive separately so a slow introspection never keeps the connection unusable.
    Connected {
        conn_id: String,
        name: String,
        elapsed_ms: f64,
        result: Result<Arc<dyn Database>, String>,
    },
    /// Lightweight table/view names arrived; full object details continue loading.
    SchemaOverviewLoaded {
        conn_id: String,
        schema: SchemaTree,
        elapsed_ms: f64,
    },
    /// Full schema metadata finished loading for an already-live connection.
    SchemaLoaded {
        conn_id: String,
        elapsed_ms: f64,
        result: Result<SchemaTree, String>,
    },
    /// Databases visible to an already-live connection finished loading.
    DatabaseListLoaded {
        conn_id: String,
        databases: Vec<String>,
        elapsed_ms: f64,
    },
    /// A connection test from the add/edit dialog finished.
    ConnectionTested {
        test_id: u64,
        conn_id: String,
        result: Result<(), String>,
    },
    /// Read-only Production Guardian checks finished for the exact tab/query snapshot.
    ProductionGuarded {
        tab_id: u64,
        conn_id: String,
        sql: String,
        preflights: Vec<dbcore::safety::ProductionPreflight>,
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
    /// The background COUNT(*) for a paged table query finished. Kept separate from
    /// [`Queried`](Self::Queried) so rows render immediately without waiting for the count.
    PageCounted {
        tab_id: u64,
        sql: String,
        total: Option<u64>,
    },
    /// A batch of staged edits was saved (`Ok` carries the number of rows updated).
    Committed {
        tab_id: u64,
        conn_id: String,
        sql: String,
        elapsed_ms: f64,
        result: Result<usize, String>,
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

#[derive(Clone, Copy)]
enum ProductionGuardContinuation {
    Query,
    Edits,
    Schema,
}

#[derive(Clone)]
struct ProductionGuardPending {
    tab_id: u64,
    conn_id: String,
    connection_name: String,
    database: String,
    sql: String,
    statements: Vec<dbcore::safety::DangerousStatement>,
    preflights: Option<Vec<dbcore::safety::ProductionPreflight>>,
    confirmation: String,
    preflight_cancel: tokio_util::sync::CancellationToken,
    continuation: ProductionGuardContinuation,
}

impl ProductionGuardPending {
    fn risk(&self, index: usize) -> dbcore::safety::RiskLevel {
        self.preflights
            .as_ref()
            .and_then(|items| items.get(index))
            .map(|preflight| self.statements[index].risk(preflight))
            .unwrap_or(dbcore::safety::RiskLevel::Critical)
    }

    fn confirmation_phrase(&self) -> Option<&str> {
        self.preflights.as_ref()?;
        self.statements
            .iter()
            .enumerate()
            .find(|(index, _)| self.risk(*index) == dbcore::safety::RiskLevel::Critical)
            .map(|(_, statement)| statement.confirmation_phrase())
    }

    fn can_confirm(&self) -> bool {
        if self.preflights.is_none() {
            return false;
        }
        self.confirmation_phrase()
            .map_or(true, |phrase| self.confirmation.trim() == phrase)
    }

    fn audit_details(&self, decision: &str) -> String {
        let risks = self
            .statements
            .iter()
            .enumerate()
            .map(|(index, statement)| {
                let risk = self
                    .preflights
                    .as_ref()
                    .and_then(|items| items.get(index))
                    .map(|preflight| statement.risk(preflight))
                    .unwrap_or_else(|| statement.base_risk());
                let rows = self
                    .preflights
                    .as_ref()
                    .and_then(|items| items.get(index))
                    .and_then(|item| {
                        item.affected_rows
                            .or_else(|| item.plan.as_ref().and_then(|plan| plan.estimated_rows))
                    })
                    .map(|rows| rows.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                format!(
                    "{}:{}:rows={rows}",
                    statement.kind.label(),
                    risk.label()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("decision={decision}; {risks}")
    }
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

/// Which result surface the central panel shows. Query tabs use Data / Message / Chart;
/// table and view tabs use Data / Structure.
#[derive(Clone, Copy, PartialEq, Default)]
enum TabView {
    #[default]
    Data,
    Message,
    Chart,
    Structure,
}

/// Which side of the result area owns the SQL editor. Code-first tabs follow execution order
/// (editor, then result); data-first tabs keep the browsable grid as the primary surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum QueryEditorPlacement {
    Top,
    Bottom,
}

fn query_editor_placement(kind: crate::components::QueryTabKind) -> QueryEditorPlacement {
    match kind {
        crate::components::QueryTabKind::Query
        | crate::components::QueryTabKind::Function
        | crate::components::QueryTabKind::Procedure
        | crate::components::QueryTabKind::Trigger
        // Diagram tabs never draw an editor; the placement is inert.
        | crate::components::QueryTabKind::Diagram => QueryEditorPlacement::Top,
        crate::components::QueryTabKind::Table | crate::components::QueryTabKind::View => {
            QueryEditorPlacement::Bottom
        }
    }
}

/// Which list fills the left sidebar (TablePlus-style tabs at its top).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum SidebarTab {
    /// The schema tree: databases, tables, views, routines.
    #[default]
    Items,
    /// Saved queries (favorites).
    Queries,
    /// The executed-statement history.
    History,
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

#[derive(Clone, Copy)]
enum ConnectStage {
    Connect,
    Overview,
    FullSchema,
    DatabaseList,
}

#[derive(Default)]
struct ConnectionTimings {
    connect_ms: Option<f64>,
    overview_ms: Option<f64>,
    full_schema_ms: Option<f64>,
    database_list_ms: Option<f64>,
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
    /// Visual identity in the tab strip. Stored explicitly so a View is not mistaken for a Table
    /// merely because both tabs carry a schema-object title.
    kind: crate::components::QueryTabKind,
    /// A transient "preview" tab (single-click on a table): shown in italics and reused for
    /// the next previewed table. Becomes permanent when its SQL is edited or it's pinned.
    preview: bool,
    /// Saved-connection id this tab runs against (`None` ⇒ unbound).
    conn_id: Option<String>,
    sql: String,
    /// Last splitter-selected SQL editor height in egui points. `None` uses the contextual
    /// default; once the user drags the splitter this is persisted with the workspace.
    editor_size: Option<f32>,
    /// Last execution failure for this tab. Kept beside the result so an error follows the
    /// query tab that produced it instead of existing only in the global status bar.
    query_error: Option<String>,
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
    /// The ER diagram shown by a `QueryTabKind::Diagram` tab. A schema snapshot, so it
    /// stays viewable after a disconnect; not persisted with the workspace.
    diagram: Option<crate::erd::ErDiagram>,
}

impl QueryTab {
    fn new(id: u64, title: String) -> Self {
        Self {
            id,
            title,
            kind: crate::components::QueryTabKind::Query,
            preview: false,
            conn_id: None,
            sql: String::new(),
            editor_size: None,
            query_error: None,
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
            diagram: None,
        }
    }

    /// Install a freshly returned result and rebuild the display order.
    fn set_result(&mut self, res: QueryResult) {
        self.view = TabView::Data;
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

    /// Sort a column in an explicit direction from the header menu.
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

/// Load metadata after authentication. The name-only overview intentionally completes before
/// full introspection for that branch, while the independent database list runs alongside it.
async fn load_connection_metadata(db: Arc<dyn Database>, conn_id: String, tx: Sender<AppMessage>) {
    let schema_tx = tx.clone();
    let schema_id = conn_id.clone();
    let schema_db = db.clone();
    let schema = async move {
        let overview_started = Instant::now();
        if let Ok(schema) = schema_db.introspect_overview().await {
            let _ = schema_tx.send(AppMessage::SchemaOverviewLoaded {
                conn_id: schema_id.clone(),
                schema,
                elapsed_ms: overview_started.elapsed().as_secs_f64() * 1000.0,
            });
        }
        let schema_started = Instant::now();
        let result = schema_db.introspect().await.map_err(|e| e.to_string());
        let _ = schema_tx.send(AppMessage::SchemaLoaded {
            conn_id: schema_id,
            elapsed_ms: schema_started.elapsed().as_secs_f64() * 1000.0,
            result,
        });
    };
    let databases = async move {
        let started = Instant::now();
        let databases = db.list_databases().await.unwrap_or_default();
        let _ = tx.send(AppMessage::DatabaseListLoaded {
            conn_id,
            databases,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        });
    };
    tokio::join!(schema, databases);
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
    let reader =
        dbcore::import::read_records(path, format, has_header).map_err(|e| e.to_string())?;

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
        rows.push(
            dbcore::import::coerce_row(&record, targets, format, i + 1)
                .map_err(|e| e.to_string())?,
        );
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
    /// Switch the left sidebar between the schema tree, saved queries, and history.
    SetSidebarTab(SidebarTab),
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
    /// Rebuild the open ER diagram from the current schema (after DDL / re-introspection).
    RefreshErd,
    /// Open the diagram scoped to one table and its FK neighborhood (depth 1).
    ShowTableDiagram {
        schema: Option<String>,
        table: String,
    },
    /// Change the focused diagram's hop depth (`erd::DEPTH_ALL` = the whole schema).
    SetErdDepth(usize),
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
        kind: crate::components::QueryTabKind,
    },
    /// Show a routine/trigger's definition SQL in a preview tab (read-only; not executed).
    OpenDefinition {
        title: String,
        sql: String,
        kind: crate::components::QueryTabKind,
    },
    /// Follow a foreign key from a grid cell: open a preview tab on the referenced table,
    /// filtered to the key the cell points at. `row`/`col` index the active tab's result.
    FollowForeignKey {
        row: usize,
        col: usize,
    },
    /// Header menu: sort a column in an explicit direction.
    SetSort {
        col: usize,
        asc: bool,
    },
    /// Header menu: drop the sort, back to natural row order.
    ClearSort,
    /// Header menu: reveal the filter bar and target a condition at this result column.
    FilterColumn(usize),
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
    /// Update the typed Critical-risk confirmation phrase.
    SetDangerConfirmation(String),
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
    /// Reorder one table relative to another table in the same pinned/unpinned group.
    MoveSchemaTable {
        conn_id: String,
        source_schema: Option<String>,
        source_table: String,
        target_schema: Option<String>,
        target_table: String,
        after: bool,
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
    /// Tabs waiting for a background COUNT(*) so the pager can repaint as soon as totals arrive.
    pending_page_counts: HashSet<u64>,
    /// Cancellation handle for the in-flight query; firing it asks the backend to abort and
    /// kill the server-side statement. `None` when no query is running.
    query_cancel: Option<tokio_util::sync::CancellationToken>,

    // --- connection state ---
    /// Pool of live connections (one per connected config), shared across tabs.
    active_connections: Vec<ActiveConnection>,
    /// Connect + metadata pipelines currently in flight. One saved connection may own at most
    /// one pipeline, preventing repeated clicks from creating overlapping five-connection pools.
    connection_jobs: HashSet<String>,
    /// Last complete schema per saved connection. Survives disconnects within this process so
    /// reconnect can paint immediately; mutations and config changes invalidate it.
    schema_cache: HashMap<String, SchemaTree>,
    /// Latest startup timings per connection. Kept in memory for diagnostics only.
    connection_timings: HashMap<String, ConnectionTimings>,

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
    /// Queries tab: live name/SQL filter over the saved-query list.
    favorites_filter: String,
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
    danger_pending: Option<ProductionGuardPending>,
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
    sidebar_tab: SidebarTab,
    history_cache: Vec<dbcore::history::HistoryEntry>,
    /// All saved queries, kept in memory and mirrored to `favorites.json` on every change.
    /// Loaded once at startup so the Saved queries tab count is immediately correct.
    favorites_cache: Vec<dbcore::Favorite>,
    /// Pinned tables, kept in memory and mirrored to `bookmarks.json` on every change. Loaded
    /// once at startup so pinned entries sort to the top of the schema explorer from launch.
    bookmarks: Vec<dbcore::Bookmark>,
    /// User-arranged table order for each connection, persisted in settings.json.
    schema_table_order: HashMap<String, Vec<String>>,
    /// Whether the Saved queries tab is active inside the SQL editor panel.
    /// Open name-this-favorite dialog. `None` = closed.
    favorite_pending: Option<FavoriteDraft>,
    /// Open ER diagram (takes over the central panel, like the schema editor).
    /// A snapshot of the schema it was built from; not persisted.

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

        // Load pinned tables so they sort to the top of the explorer from launch. Kept out
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
        let schema_table_order = settings.schema_table_order.clone();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        // Start with one clean query tab so the workspace never opens as an empty shell.
        // A saved workspace replaces it later through `restore_workspace`.
        let default_tab = QueryTab::new(0, String::new());

        Self {
            connections,
            rt,
            tx,
            rx,
            busy: Busy::Idle,
            next_connection_test_id: 1,
            querying_tab_id: None,
            pending_page_counts: HashSet::new(),
            query_cancel: None,
            active_connections: Vec::new(),
            connection_jobs: HashSet::new(),
            schema_cache: HashMap::new(),
            connection_timings: HashMap::new(),
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
            favorites_filter: String::new(),
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
            sidebar_tab: SidebarTab::default(),
            history_cache: Vec::new(),
            // Loaded from disk in `new` (this builder stays config-dir-free for tests).
            favorites_cache: Vec::new(),
            bookmarks: Vec::new(),
            schema_table_order,
            favorite_pending: None,
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

    /// Path string for the unified title-bar breadcrumb.
    fn breadcrumb_text(&self) -> String {
        let Some(active) = self.active() else {
            if let Some(id) = self
                .tabs
                .get(self.active_query_tab)
                .and_then(|tab| tab.conn_id.as_deref())
            {
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
        let id = self.tabs.get(self.active_query_tab)?.conn_id.as_deref()?;
        self.active_connections.iter().find(|c| c.config_id == id)
    }

    /// Saved config the active tab is bound to, regardless of live connection state.
    fn active_connection_config(&self) -> Option<&ConnectionConfig> {
        let id = self.tabs.get(self.active_query_tab)?.conn_id.as_deref()?;
        self.connections.iter().find(|c| c.id == id)
    }

    /// Open `diagram` in a new Diagram tab and select it. Reuses an existing Diagram
    /// tab showing the same connection and scope instead of stacking duplicates.
    fn open_diagram_tab(&mut self, title: String, diagram: crate::erd::ErDiagram) {
        // Scope = the root table (or the whole schema), regardless of hop depth: a
        // re-opened table should land in its existing tab even after widening it.
        let root = |d: &crate::erd::ErDiagram| {
            d.focus.as_ref().map(|f| (f.schema.clone(), f.table.clone()))
        };
        let same_scope = |t: &QueryTab| {
            t.kind == crate::components::QueryTabKind::Diagram
                && t.diagram.as_ref().is_some_and(|d| {
                    d.conn_id == diagram.conn_id && root(d) == root(&diagram)
                })
        };
        if let Some(i) = self.tabs.iter().position(same_scope) {
            self.select_tab(i);
            return;
        }
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = QueryTab::new(id, title);
        tab.kind = crate::components::QueryTabKind::Diagram;
        tab.conn_id = Some(diagram.conn_id.clone());
        tab.diagram = Some(diagram);
        self.tabs.push(tab);
        self.select_tab(self.tabs.len() - 1);
    }

    /// Rebuild the diagram in `tab_idx` from its connection's current schema, keeping
    /// the user's pan/zoom, the focus scope, and the position of every node whose
    /// table survived. Keeps the stale snapshot when the connection is gone.
    fn refresh_diagram_tab(&mut self, tab_idx: usize) {
        let Some(old) = self.tabs.get_mut(tab_idx).and_then(|t| t.diagram.take()) else {
            return;
        };
        let Some(conn) = self
            .active_connections
            .iter()
            .find(|c| c.config_id == old.conn_id)
        else {
            // Connection dropped: the snapshot stays viewable, just not refreshable.
            self.tabs[tab_idx].diagram = Some(old);
            return;
        };
        let mut fresh = match &old.focus {
            Some(f) => crate::erd::ErDiagram::build_focused(&old.conn_id, &conn.schema, f.clone()),
            None => crate::erd::ErDiagram::build(&old.conn_id, &conn.schema),
        };
        for node in &mut fresh.nodes {
            if let Some(prev) = old.nodes.iter().find(|n| n.title == node.title) {
                node.pos = prev.pos;
            }
        }
        fresh.scene_rect = old.scene_rect;
        self.tabs[tab_idx].diagram = Some(fresh);
    }

    /// Switch the sidebar list, lazily (re)loading what the target tab shows. The
    /// history cache lives only while its tab is visible (same lifecycle the old
    /// side panel had); favorites re-read so out-of-band edits show up.
    fn set_sidebar_tab(&mut self, tab: SidebarTab) {
        if self.sidebar_tab == tab {
            return;
        }
        match tab {
            SidebarTab::History => {
                self.history_cache =
                    dbcore::history::load(dbcore::history::MAX_ENTRIES).unwrap_or_default();
            }
            SidebarTab::Queries => {
                if !cfg!(test) {
                    self.favorites_cache = dbcore::favorites::load().unwrap_or_default();
                }
            }
            SidebarTab::Items => {}
        }
        if tab != SidebarTab::History {
            self.history_cache = Vec::new();
        }
        self.sidebar_tab = tab;
    }

    /// Change the FK-hop depth of the active tab's focused diagram ([`crate::erd::DEPTH_ALL`]
    /// = whole schema, root still highlighted), re-laying it out from scratch. No-op when
    /// the active tab isn't a focused diagram or its connection is gone.
    fn set_erd_depth(&mut self, depth: usize) {
        let idx = self.active_query_tab;
        let Some(old) = self.tabs.get_mut(idx).and_then(|t| t.diagram.take()) else {
            return;
        };
        let (Some(focus), Some(conn)) = (
            old.focus.clone(),
            self.active_connections
                .iter()
                .find(|c| c.config_id == old.conn_id),
        ) else {
            self.tabs[idx].diagram = Some(old);
            return;
        };
        self.tabs[idx].diagram = Some(crate::erd::ErDiagram::build_focused(
            &old.conn_id,
            &conn.schema,
            crate::erd::ErdFocus { depth, ..focus },
        ));
    }

    fn active_title_bar_color(&self) -> Option<ConnectionColor> {
        self.active_connection_config()?.title_bar_color
    }

    // --- query-tab management ---------------------------------------------

    // --- background work --------------------------------------------------

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

    // --- action dispatch --------------------------------------------------

    // --- workspace persistence --------------------------------------------
}

#[cfg(test)]
mod tests;
