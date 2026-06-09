//! The application state and the immediate-mode `update` loop.
//!
//! Threading model: the UI never blocks on database I/O. A `tokio` runtime owned by the
//! app runs connect/introspect/query work on background tasks; results come back over an
//! `mpsc` channel that we drain each frame. While work is in flight the UI stays
//! interactive and shows a spinner.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use dbcore::{ConnectionConfig, Database, DbKind, QueryResult, SchemaTree};

mod panels;
mod widgets;

use crate::edit::{EditSource, Edits};
use crate::filter::{self, FilterState};
use crate::theme::ThemeId;

/// Messages sent from background tasks back to the UI thread.
enum AppMessage {
    /// A connect+introspect attempt finished.
    Connected {
        conn_id: String,
        name: String,
        result: Result<(Arc<dyn Database>, SchemaTree), String>,
    },
    /// A query finished.
    Queried(Result<QueryResult, String>),
    /// A batch of staged edits was saved (`Ok` carries the number of rows updated).
    Committed(Result<usize, String>),
}

/// What the background runtime is currently doing (drives the spinner / disables buttons).
#[derive(Clone, Copy, PartialEq)]
enum Busy {
    Idle,
    Connecting,
    Querying,
}

/// A live connection plus its introspected schema.
struct ActiveConnection {
    /// Id of the originating config; kept for reconnect/refresh in later phases.
    #[allow(dead_code)]
    config_id: String,
    name: String,
    db: Arc<dyn Database>,
    schema: SchemaTree,
}

/// State for the add/edit-connection dialog.
struct ConnEditor {
    config: ConnectionConfig,
    password: String,
    is_new: bool,
    /// Index in `connections` being edited (for an existing connection).
    edit_index: Option<usize>,
}

/// Deferred UI actions. Collected from panel closures (which only borrow individual
/// fields) and applied afterwards with full `&mut self`, sidestepping borrow conflicts.
enum Action {
    Connect(usize),
    SelectActive(usize),
    CloseActive(usize),
    Disconnect,
    NewConnection,
    EditConnection(usize),
    DeleteConnection(usize),
    SaveConnection,
    CancelDialog,
    OpenSettings,
    CloseSettings,
    BrowseSqlitePath,
    RunQuery,
    /// Open a table's rows from the sidebar. `source` makes the result editable.
    OpenTable {
        sql: String,
        source: EditSource,
    },
    SortBy(usize),
}

pub struct DbGuiApp {
    // --- persisted config ---
    connections: Vec<ConnectionConfig>,

    // --- async plumbing ---
    rt: tokio::runtime::Runtime,
    tx: Sender<AppMessage>,
    rx: Receiver<AppMessage>,
    busy: Busy,

    // --- connection state ---
    selected_conn: Option<usize>,
    active_connections: Vec<ActiveConnection>,
    active_tab: Option<usize>,

    // --- editor / results ---
    sql: String,
    result: Option<QueryResult>,
    /// Indices into `result.rows` giving the current display order (sorting).
    row_order: Vec<usize>,
    sort: Option<(usize, bool)>,
    /// Currently selected display row (drives the Details panel).
    selected_row: Option<usize>,
    /// Staged cell edits and the editable source of the current result.
    edits: Edits,

    // --- transient UI state ---
    editor: Option<ConnEditor>,
    settings_open: bool,
    schema_filter: String,
    /// TablePlus-style result filter bar (column / operator / value conditions).
    filter: FilterState,
    status_msg: String,
    error: Option<String>,

    // --- layout ---
    show_connection_tabs: bool,
    show_schema_panel: bool,
    show_details_panel: bool,

    // --- preferences ---
    /// Currently selected colour theme (persisted to settings.json).
    theme: ThemeId,
}

impl DbGuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Build state first: `construct` activates the saved theme, which `apply` then reads.
        let app = Self::construct();

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
        let theme = dbcore::config::load_settings()
            .theme
            .as_deref()
            .and_then(ThemeId::from_key)
            .unwrap_or(ThemeId::DEFAULT);
        crate::theme::set_current(theme);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        Self {
            connections,
            rt,
            tx,
            rx,
            busy: Busy::Idle,
            selected_conn: None,
            active_connections: Vec::new(),
            active_tab: None,
            sql: "SELECT 1;".to_string(),
            result: None,
            row_order: Vec::new(),
            sort: None,
            selected_row: None,
            edits: Edits::default(),
            editor: None,
            settings_open: false,
            schema_filter: String::new(),
            filter: FilterState::default(),
            status_msg: "Ready".to_string(),
            error: None,
            show_connection_tabs: true,
            show_schema_panel: true,
            show_details_panel: true,
            theme,
        }
    }

    /// Path string for the unified title-bar breadcrumb.
    fn breadcrumb_text(&self) -> String {
        let Some(active) = self.active() else {
            if let Some(idx) = self.selected_conn {
                if let Some(cfg) = self.connections.get(idx) {
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

        if let Some(source) = &self.edits.source {
            let table = match &source.schema {
                Some(schema) => format!("{schema}.{}", source.table),
                None => source.table.clone(),
            };
            path.push_str(&format!(" : {table}"));
        }

        path
    }

    /// Switch the active theme, re-apply the egui style, and persist the choice.
    fn set_theme(&mut self, ctx: &egui::Context, id: ThemeId) {
        self.theme = id;
        crate::theme::set_current(id);
        crate::style::apply(ctx);
        let settings = dbcore::config::Settings {
            theme: Some(id.key().to_string()),
        };
        if let Err(e) = dbcore::config::save_settings(&settings) {
            self.error = Some(format!("Could not save theme: {e}"));
        }
    }

    fn active(&self) -> Option<&ActiveConnection> {
        self.active_tab
            .and_then(|idx| self.active_connections.get(idx))
    }

    fn set_active_tab(&mut self, idx: usize) {
        if idx >= self.active_connections.len() {
            return;
        }
        self.active_tab = Some(idx);
        let conn_id = &self.active_connections[idx].config_id;
        self.selected_conn = self.connections.iter().position(|cfg| &cfg.id == conn_id);
        self.result = None;
        self.row_order.clear();
        self.sort = None;
        self.selected_row = None;
        self.status_msg = format!("Switched to {}", self.active_connections[idx].name);
        self.error = None;
    }

    fn close_active_tab(&mut self, idx: usize) {
        if idx >= self.active_connections.len() {
            return;
        }
        self.active_connections.remove(idx);
        self.result = None;
        self.row_order.clear();
        self.sort = None;
        self.selected_row = None;

        self.active_tab = match self.active_connections.len() {
            0 => {
                self.selected_conn = None;
                None
            }
            len => Some(idx.min(len - 1)),
        };
        if let Some(active_idx) = self.active_tab {
            let conn_id = &self.active_connections[active_idx].config_id;
            self.selected_conn = self.connections.iter().position(|cfg| &cfg.id == conn_id);
            self.status_msg = format!("Switched to {}", self.active_connections[active_idx].name);
        } else {
            self.status_msg = "Disconnected".to_string();
        }
    }

    // --- background work --------------------------------------------------

    fn poll_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AppMessage::Connected {
                    conn_id,
                    name,
                    result,
                } => {
                    self.busy = Busy::Idle;
                    match result {
                        Ok((db, schema)) => {
                            let n = schema.tables.len();
                            let active = ActiveConnection {
                                config_id: conn_id,
                                name: name.clone(),
                                db,
                                schema,
                            };
                            if let Some(idx) = self
                                .active_connections
                                .iter()
                                .position(|conn| conn.config_id == active.config_id)
                            {
                                self.active_connections[idx] = active;
                                self.active_tab = Some(idx);
                            } else {
                                self.active_connections.push(active);
                                self.active_tab = Some(self.active_connections.len() - 1);
                            }
                            self.status_msg = format!("Connected to {name} — {n} tables");
                            self.error = None;
                        }
                        Err(e) => {
                            self.error = Some(format!("Connection failed: {e}"));
                            self.status_msg = "Connection failed".to_string();
                        }
                    }
                }
                AppMessage::Queried(result) => {
                    self.busy = Busy::Idle;
                    match result {
                        Ok(res) => {
                            // Promote the in-flight source and start from a clean edit slate.
                            self.edits.source = self.edits.pending_source.take();
                            self.edits.clear();
                            self.set_result(res);
                        }
                        Err(e) => {
                            self.error = Some(format!("Query error: {e}"));
                            self.status_msg = "Query failed".to_string();
                        }
                    }
                }
                AppMessage::Committed(result) => {
                    self.busy = Busy::Idle;
                    match result {
                        Ok(n) => {
                            self.status_msg = format!("Saved {n} change(s)");
                            self.error = None;
                            // Reload so the grid reflects exactly what the database now holds
                            // (triggers, defaults, type coercions). Keep the source editable.
                            self.edits.pending_source = self.edits.source.clone();
                            self.start_query();
                        }
                        Err(e) => {
                            self.error = Some(format!("Save failed: {e}"));
                            self.status_msg = "Save failed".to_string();
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    fn set_result(&mut self, res: QueryResult) {
        self.sort = None;
        self.selected_row = None;
        // A fresh result may have a different column count; keep filter conditions but stop
        // them indexing past the new columns, then rebuild the display order through the
        // filter (so a still-open filter bar keeps applying).
        self.filter.clamp_columns(res.column_count());
        self.status_msg = match res.stats.rows_affected {
            Some(n) => format!("OK — {n} row(s) affected in {:.1} ms", res.stats.elapsed_ms),
            None => format!(
                "{} row(s) × {} col(s) in {:.1} ms",
                res.row_count(),
                res.column_count(),
                res.stats.elapsed_ms
            ),
        };
        self.error = None;
        // Classify each column once so the cell editors can be type-aware.
        self.edits.set_columns(&res.columns);
        self.result = Some(res);
        self.recompute_view();
    }

    /// Rebuild `row_order` (the grid's display order) from the current result by applying the
    /// filter, then the active sort. The single place both filtering and sorting funnel
    /// through, so they always compose the same way.
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
        // A row that filtered out can't stay selected.
        if self.selected_row.is_some_and(|s| s >= self.row_order.len()) {
            self.selected_row = None;
        }
    }

    fn start_connect(&mut self, idx: usize) {
        let Some(cfg) = self.connections.get(idx).cloned() else {
            return;
        };
        self.selected_conn = Some(idx);
        let password = if cfg.kind.is_server() {
            dbcore::secrets::get_password(&cfg.id).ok().flatten()
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
            let result = async {
                let db = dbcore::connect(&cfg, password).await?;
                let schema = db.introspect().await?;
                Ok::<_, dbcore::CoreError>((db, schema))
            }
            .await
            .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Connected {
                conn_id: id,
                name,
                result,
            });
        });
    }

    fn start_query(&mut self) {
        let Some(active) = self.active() else {
            self.error = Some("Not connected.".to_string());
            return;
        };
        let sql = self.sql.trim().to_string();
        if sql.is_empty() {
            return;
        }
        let db = active.db.clone();
        let tx = self.tx.clone();
        self.busy = Busy::Querying;
        self.error = None;
        self.status_msg = "Running query…".to_string();
        self.rt.spawn(async move {
            let result = db.execute(&sql).await.map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Queried(result));
        });
    }

    /// Commit the cell currently being typed into the staged set, so its value isn't lost
    /// when focus moves or a save is triggered.
    /// Stage the cell being typed into. Returns `false` if its value is invalid (the editor
    /// stays open), so callers can refuse to proceed.
    fn flush_active_edit(&mut self) -> bool {
        let Some(active) = self.edits.active.as_ref() else {
            return true;
        };
        let original = self
            .result
            .as_ref()
            .and_then(|r| r.rows.get(active.row).and_then(|row| row.get(active.col)))
            .cloned();
        match original {
            Some(original) => self.edits.commit_active(&original),
            None => {
                self.edits.cancel_active();
                true
            }
        }
    }

    /// Turn all staged edits into `UPDATE` statements and run them on the background runtime.
    /// Each changed row becomes one statement keyed by the source table's primary key.
    fn commit_edits(&mut self) {
        // A cell still being edited with invalid (red) input blocks the whole save.
        if !self.flush_active_edit() {
            self.error = Some("Fix the highlighted cell before saving.".into());
            self.status_msg = "Invalid value — not saved".to_string();
            return;
        }
        if !self.edits.has_pending() {
            return;
        }
        // Defence in depth: every staged value must still match its column kind before we
        // build any SQL, so a malformed value can never reach the database.
        for (_, colmap) in &self.edits.cells {
            for (&col, value) in colmap {
                if !self.edits.col_kind(col).accepts(value) {
                    self.error =
                        Some("Cannot save: a cell holds a value invalid for its type.".into());
                    self.status_msg = "Invalid value — not saved".to_string();
                    return;
                }
            }
        }
        let Some(source) = self.edits.source.clone() else {
            return;
        };
        // Grab the dialect + a connection handle, then drop the `active()` borrow so we can
        // freely touch `self` below.
        let (kind, db) = match self.active() {
            Some(active) => (active.db.kind(), active.db.clone()),
            None => return,
        };
        let Some(result) = &self.result else { return };

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
            return;
        };

        let mut statements = Vec::new();
        for (&row, colmap) in &self.edits.cells {
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
                Some(sql) => statements.push(sql),
                None => {
                    self.error = Some("Cannot save: a cell holds a value that can't be written "
                        .to_string()
                        + "(e.g. binary data).");
                    return;
                }
            }
        }

        let n = statements.len();
        let tx = self.tx.clone();
        self.busy = Busy::Querying;
        self.error = None;
        self.status_msg = format!("Saving {n} change(s)…");
        self.rt.spawn(async move {
            let mut outcome = Ok(n);
            for stmt in &statements {
                if let Err(e) = db.execute(stmt).await {
                    outcome = Err(e.to_string());
                    break;
                }
            }
            let _ = tx.send(AppMessage::Committed(outcome));
        });
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

    // --- action dispatch --------------------------------------------------

    fn apply_action(&mut self, action: Action) {
        match action {
            Action::Connect(i) => self.start_connect(i),
            Action::SelectActive(i) => self.set_active_tab(i),
            Action::CloseActive(i) => self.close_active_tab(i),
            Action::Disconnect => {
                if let Some(idx) = self.active_tab {
                    self.close_active_tab(idx);
                }
            }
            Action::NewConnection => {
                self.editor = Some(ConnEditor {
                    config: ConnectionConfig::new(DbKind::Postgres),
                    password: String::new(),
                    is_new: true,
                    edit_index: None,
                });
            }
            Action::EditConnection(i) => {
                if let Some(cfg) = self.connections.get(i).cloned() {
                    let password = dbcore::secrets::get_password(&cfg.id)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    self.editor = Some(ConnEditor {
                        config: cfg,
                        password,
                        is_new: false,
                        edit_index: Some(i),
                    });
                }
            }
            Action::DeleteConnection(i) => {
                if i < self.connections.len() {
                    let cfg = self.connections.remove(i);
                    let _ = dbcore::secrets::delete_password(&cfg.id);
                    if let Err(e) = dbcore::config::save_connections(&self.connections) {
                        self.error = Some(e.to_string());
                    }
                    if self.selected_conn == Some(i) {
                        self.selected_conn = None;
                    }
                    let active_before = self.active().map(|active| active.config_id.clone());
                    self.active_connections
                        .retain(|conn| conn.config_id != cfg.id);
                    if self.active_connections.is_empty() {
                        self.active_tab = None;
                    } else if let Some(tab) = self.active_tab {
                        self.active_tab = Some(tab.min(self.active_connections.len() - 1));
                    }
                    if active_before.as_deref() == Some(cfg.id.as_str()) {
                        self.result = None;
                        self.row_order.clear();
                        self.sort = None;
                        self.selected_row = None;
                    }
                }
            }
            Action::SaveConnection => self.save_connection(),
            Action::CancelDialog => self.editor = None,
            Action::OpenSettings => self.settings_open = true,
            Action::CloseSettings => self.settings_open = false,
            Action::BrowseSqlitePath => {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    if let Some(ed) = &mut self.editor {
                        ed.config.sqlite_path = path.to_string_lossy().into_owned();
                    }
                }
            }
            Action::RunQuery => {
                // An ad-hoc query can't be mapped back to one table, so it isn't editable.
                self.edits.pending_source = None;
                self.start_query();
            }
            Action::OpenTable { sql, source } => {
                self.edits.pending_source = Some(source);
                self.sql = sql;
                self.start_query();
            }
            Action::SortBy(col) => self.apply_sort(col),
        }
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

impl eframe::App for DbGuiApp {
    // eframe 0.34 hands us a root `Ui`; panels are added with `show_inside`.
    fn ui(&mut self, ui_root: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.draw(ui_root, Some(frame));
    }
}

impl DbGuiApp {
    /// Draw one frame into the given root ui. Split out from `eframe::App::ui` so it can be
    /// driven headlessly in tests (no `eframe::Frame` needed).
    fn draw(&mut self, ui_root: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        let ctx = ui_root.ctx().clone();
        self.poll_messages(&ctx);

        let mut actions: Vec<Action> = Vec::new();

        // Global shortcut: Cmd/Ctrl+Enter runs the query.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            actions.push(Action::RunQuery);
        }
        // Cmd/Ctrl+S saves staged cell edits (TablePlus-style) as UPDATE statements.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.commit_edits();
        }
        // Cmd/Ctrl+F toggles the filter bar (when there's a result to filter); Esc hides it.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F))
            && self.result.is_some()
        {
            self.filter.visible = !self.filter.visible;
        }
        if self.filter.visible && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.filter.visible = false;
        }

        // Order matters: top/bottom/left/right carve space, central takes the rest.
        self.top_bar(ui_root, frame, &mut actions);
        self.query_console(ui_root, &mut actions);
        self.status_bar(ui_root);
        if self.show_connection_tabs {
            self.connection_tabs(ui_root, &mut actions);
        }
        if self.show_schema_panel {
            self.left_panel(ui_root, &mut actions);
        }
        if self.show_details_panel {
            self.right_panel(ui_root);
        }
        // A top panel after left/right carves the strip directly above the grid.
        self.filter_bar(ui_root);
        self.central_panel(ui_root, &mut actions);
        self.connection_dialog(&ctx, &mut actions);
        self.settings_dialog(&ctx, &mut actions);

        for action in actions {
            self.apply_action(action);
        }

        // Keep animating the spinner while background work is in flight.
        if self.busy != Busy::Idle {
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
        async fn execute(&self, _sql: &str) -> dbcore::Result<QueryResult> {
            unreachable!()
        }
    }

    fn fake_schema(tables: usize, cols: usize) -> SchemaTree {
        SchemaTree {
            database_name: "testdb".into(),
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
        }
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
        // 10 rows, col 0 = 0..10. Keep rows where col0 < 4.
        app.set_result(fake_result(10, 2));
        assert_eq!(app.row_order.len(), 10);

        app.filter.visible = true;
        app.filter.conditions = vec![crate::filter::Condition {
            enabled: true,
            column: 0,
            op: crate::filter::FilterOp::Less,
            value: "8".into(), // col0 values step by `cols`=2: 0,2,4,6,8,... → <8 keeps 4 rows
        }];
        app.recompute_view();
        assert_eq!(app.row_order.len(), 4);

        app.filter.reset();
        app.recompute_view();
        assert_eq!(app.row_order.len(), 10);
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
        let result = fake_result(2000, 6);
        app.row_order = (0..result.rows.len()).collect();
        app.result = Some(result);
        app.selected_row = Some(7); // render the Details panel
        app.filter.visible = true; // render the filter bar too
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "test".into(),
            name: "test-conn".into(),
            db,
            schema: fake_schema(15, 5),
        });
        app.active_tab = Some(0);

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
}
