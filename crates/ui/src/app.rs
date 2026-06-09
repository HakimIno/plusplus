//! The application state and the immediate-mode `update` loop.
//!
//! Threading model: the UI never blocks on database I/O. A `tokio` runtime owned by the
//! app runs connect/introspect/query work on background tasks; results come back over an
//! `mpsc` channel that we drain each frame. While work is in flight the UI stays
//! interactive and shows a spinner.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use dbcore::{ConnectionConfig, Database, DbKind, QueryResult, SchemaTree};

use crate::filter::{self, FilterEvent, FilterState};
use crate::grid::results_grid;
use crate::icons;
use crate::style::{self, palette};
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

fn compact_connection_label(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "DB".to_string();
    }
    let mut label: String = trimmed.chars().take(7).collect();
    if trimmed.chars().count() > 7 {
        label.push('…');
    }
    label
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
    BrowseSqlitePath,
    RunQuery,
    OpenTable(String),
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

    // --- transient UI state ---
    editor: Option<ConnEditor>,
    schema_filter: String,
    /// TablePlus-style result filter bar (column / operator / value conditions).
    filter: FilterState,
    status_msg: String,
    error: Option<String>,

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
            editor: None,
            schema_filter: String::new(),
            filter: FilterState::default(),
            status_msg: "Ready".to_string(),
            error: None,
            theme,
        }
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
                        Ok(res) => self.set_result(res),
                        Err(e) => {
                            self.error = Some(format!("Query error: {e}"));
                            self.status_msg = "Query failed".to_string();
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
            Action::BrowseSqlitePath => {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    if let Some(ed) = &mut self.editor {
                        ed.config.sqlite_path = path.to_string_lossy().into_owned();
                    }
                }
            }
            Action::RunQuery => self.start_query(),
            Action::OpenTable(sql) => {
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
    fn ui(&mut self, ui_root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui_root);
    }
}

impl DbGuiApp {
    /// Draw one frame into the given root ui. Split out from `eframe::App::ui` so it can be
    /// driven headlessly in tests (no `eframe::Frame` needed).
    fn draw(&mut self, ui_root: &mut egui::Ui) {
        let ctx = ui_root.ctx().clone();
        self.poll_messages(&ctx);

        let mut actions: Vec<Action> = Vec::new();

        // Global shortcut: Cmd/Ctrl+Enter runs the query.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            actions.push(Action::RunQuery);
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
        self.top_bar(ui_root, &mut actions);
        self.query_console(ui_root, &mut actions);
        self.status_bar(ui_root);
        self.connection_tabs(ui_root, &mut actions);
        self.left_panel(ui_root, &mut actions);
        self.right_panel(ui_root);
        // A top panel after left/right carves the strip directly above the grid.
        self.filter_bar(ui_root);
        self.central_panel(ui_root, &mut actions);
        self.connection_dialog(&ctx, &mut actions);

        for action in actions {
            self.apply_action(action);
        }

        // Keep animating the spinner while background work is in flight.
        if self.busy != Busy::Idle {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }
}

// --- panels ---------------------------------------------------------------

impl DbGuiApp {
    fn top_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::top("top_bar").show_inside(root, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                let connected = self.active().is_some();
                let can_run = connected && self.busy == Busy::Idle;
                if icons::primary_button(ui, icons::play(), "Run", can_run)
                    .on_hover_text("Run query  (Cmd/Ctrl+Enter)")
                    .clicked()
                {
                    actions.push(Action::RunQuery);
                }
                if connected {
                    if icons::button(ui, icons::disconnect(), "Disconnect", true).clicked() {
                        actions.push(Action::Disconnect);
                    }
                }

                // Filter toggle — enabled once there's a result to filter, highlighted while
                // the bar is open (TablePlus's ⌘F).
                let has_result = self.result.is_some();
                let filter_on = self.filter.visible;
                if icons::toggle_button(ui, icons::filter(), "Filter", has_result, filter_on)
                    .on_hover_text("Show / hide filter bar  (Cmd/Ctrl+F)")
                    .clicked()
                {
                    self.filter.visible = !self.filter.visible;
                }

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Breadcrumb: ● connection › database.
                if let Some(active) = self.active() {
                    style::status_dot(ui, palette::SUCCESS());
                    ui.add_space(1.0);
                    ui.strong(&active.name);
                    ui.colored_label(palette::TEXT_FAINT(), "›");
                    ui.colored_label(palette::TEXT_WEAK(), &active.schema.database_name);
                } else {
                    style::status_dot(ui, palette::TEXT_FAINT());
                    ui.add_space(1.0);
                    ui.colored_label(
                        palette::TEXT_WEAK(),
                        "Not connected — pick a connection on the left",
                    );
                }

                // Theme picker + busy indicator, right-aligned.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.theme_picker(ui);

                    match self.busy {
                        Busy::Connecting => {
                            ui.add_space(2.0);
                            ui.colored_label(palette::TEXT_WEAK(), "connecting…");
                            ui.spinner();
                        }
                        Busy::Querying => {
                            ui.add_space(2.0);
                            ui.colored_label(palette::TEXT_WEAK(), "running…");
                            ui.spinner();
                        }
                        Busy::Idle => {}
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    /// A small combo box for choosing the colour theme. Switching applies immediately and
    /// the choice is remembered across launches.
    fn theme_picker(&mut self, ui: &mut egui::Ui) {
        let mut chosen = self.theme;
        egui::ComboBox::from_id_salt("theme_picker")
            .selected_text(self.theme.label())
            .show_ui(ui, |ui| {
                for id in ThemeId::ALL {
                    ui.selectable_value(&mut chosen, id, id.label());
                }
            });
        if chosen != self.theme {
            self.set_theme(ui.ctx(), chosen);
        }
    }

    /// Thin bar between the grid and the SQL console: row count / selection / errors.
    fn status_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status_bar").show_inside(root, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                if let Some(err) = &self.error {
                    icons::show_colored(ui, icons::warning(), 15.0, palette::DANGER());
                    ui.colored_label(palette::DANGER(), err);
                } else {
                    icons::show_weak(ui, icons::table(), 14.0);
                    ui.colored_label(palette::TEXT_WEAK(), &self.status_msg);
                    // When a filter is narrowing the result, show "shown of total".
                    if let Some(res) = &self.result {
                        if self.filter.is_active() && self.row_order.len() != res.row_count() {
                            ui.colored_label(palette::TEXT_FAINT(), "·");
                            icons::show_colored(ui, icons::filter(), 13.0, palette::ACCENT());
                            ui.colored_label(
                                palette::ACCENT(),
                                format!("{} of {} rows", self.row_order.len(), res.row_count()),
                            );
                        }
                    }
                    if let (Some(sel), true) = (self.selected_row, self.result.is_some()) {
                        ui.colored_label(palette::TEXT_FAINT(), "·");
                        ui.colored_label(palette::TEXT_WEAK(), format!("row {}", sel + 1));
                    }
                }
            });
            ui.add_space(3.0);
        });
    }

    /// SQL editor at the very bottom, with syntax highlighting and a Run button.
    fn query_console(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::bottom("query_console")
            .resizable(true)
            .default_size(150.0)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    style::section_header(ui, "Query");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_run = self.active().is_some() && self.busy == Busy::Idle;
                        if icons::primary_button(ui, icons::play(), "Run", can_run)
                            .on_hover_text("Cmd/Ctrl+Enter")
                            .clicked()
                        {
                            actions.push(Action::RunQuery);
                        }
                    });
                });
                ui.add_space(6.0);

                // Syntax-highlighting layouter for the editor.
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                    let mut job = crate::highlight::highlight_sql(buf.as_str(), font.clone());
                    job.wrap.max_width = wrap_width;
                    ui.ctx().fonts_mut(|f| f.layout_job(job))
                };

                egui::ScrollArea::vertical()
                    .id_salt("sql_scroll")
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.sql)
                                .code_editor()
                                .desired_rows(5)
                                .desired_width(f32::INFINITY)
                                .layouter(&mut layouter)
                                .hint_text("Write SQL here, then press Run (Cmd/Ctrl+Enter)"),
                        );
                    });
                ui.add_space(4.0);
            });
    }

    /// Right-hand Details panel: the selected row's columns and values.
    fn right_panel(&mut self, root: &mut egui::Ui) {
        egui::Panel::right("details_panel")
            .resizable(true)
            .default_size(260.0)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                style::section_header(ui, "Details");
                ui.separator();

                let selected = match (self.result.as_ref(), self.selected_row) {
                    (Some(res), Some(disp)) if disp < self.row_order.len() => {
                        Some((res, self.row_order[disp]))
                    }
                    _ => None,
                };

                let Some((res, row_idx)) = selected else {
                    style::empty_state(
                        ui,
                        icons::table(),
                        "No row selected",
                        "Click a row to inspect it",
                    );
                    return;
                };

                egui::ScrollArea::vertical()
                    .id_salt("details_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("details_grid")
                            .num_columns(2)
                            .striped(true)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                for (c, col) in res.columns.iter().enumerate() {
                                    ui.weak(&col.name);
                                    let value = &res.rows[row_idx][c];
                                    if value.is_null() {
                                        ui.weak("NULL");
                                    } else {
                                        ui.add(egui::Label::new(value.display()).wrap());
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
    }

    fn connection_tabs(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("connection_tabs")
            .resizable(false)
            .exact_size(66.0)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    if icons::icon_button(ui, icons::plus(), "New connection").clicked() {
                        actions.push(Action::NewConnection);
                    }
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .id_salt("active_connection_tabs")
                    .show(ui, |ui| {
                        for (idx, conn) in self.active_connections.iter().enumerate() {
                            let selected = self.active_tab == Some(idx);
                            let label = compact_connection_label(&conn.name);
                            let fill = if selected {
                                palette::SELECTION()
                            } else {
                                palette::SURFACE()
                            };
                            let stroke = if selected {
                                egui::Stroke::new(1.0, palette::ACCENT())
                            } else {
                                egui::Stroke::new(1.0, palette::BORDER())
                            };
                            let text = egui::RichText::new(label).size(10.5).color(if selected {
                                palette::TEXT()
                            } else {
                                palette::TEXT_WEAK()
                            });
                            let resp = ui
                                .add_sized(
                                    egui::vec2(56.0, 36.0),
                                    egui::Button::new(text).fill(fill).stroke(stroke),
                                )
                                .on_hover_text(format!(
                                    "{}\n{}",
                                    conn.name, conn.schema.database_name
                                ));
                            if resp.clicked() {
                                actions.push(Action::SelectActive(idx));
                            }
                            resp.context_menu(|ui| {
                                if icons::button(ui, icons::close(), "Close tab", true).clicked() {
                                    actions.push(Action::CloseActive(idx));
                                    ui.close();
                                }
                            });
                            ui.add_space(4.0);
                        }

                        if self.active_connections.is_empty() {
                            ui.vertical_centered(|ui| {
                                icons::show_weak(ui, icons::database(), 18.0);
                            });
                        }
                    });
            });
    }

    fn left_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(280.0)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    style::section_header(ui, "Connections");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::icon_button(ui, icons::plus(), "New connection").clicked() {
                            actions.push(Action::NewConnection);
                        }
                    });
                });

                egui::ScrollArea::vertical()
                    .id_salt("conn_scroll")
                    .max_height(ui.available_height() * 0.4)
                    .show(ui, |ui| {
                        for (i, conn) in self.connections.iter().enumerate() {
                            let selected = self.selected_conn == Some(i);
                            let is_active = self
                                .active()
                                .is_some_and(|active| active.config_id == conn.id);
                            let resp = ui
                                .horizontal(|ui| {
                                    let dot = if is_active {
                                        palette::SUCCESS()
                                    } else {
                                        palette::TEXT_FAINT()
                                    };
                                    style::status_dot(ui, dot);
                                    ui.add_space(2.0);
                                    ui.selectable_label(selected, &conn.name)
                                })
                                .inner;
                            if resp.clicked() {
                                actions.push(Action::Connect(i));
                            }
                            resp.on_hover_text(conn.target_summary())
                                .context_menu(|ui| {
                                    if icons::button(ui, icons::connect(), "Connect", true)
                                        .clicked()
                                    {
                                        actions.push(Action::Connect(i));
                                        ui.close();
                                    }
                                    if icons::button(ui, icons::edit(), "Edit…", true).clicked() {
                                        actions.push(Action::EditConnection(i));
                                        ui.close();
                                    }
                                    if icons::button(ui, icons::trash(), "Delete", true).clicked() {
                                        actions.push(Action::DeleteConnection(i));
                                        ui.close();
                                    }
                                });
                        }
                        if self.connections.is_empty() {
                            ui.add_space(4.0);
                            ui.colored_label(palette::TEXT_FAINT(), "No saved connections.");
                            ui.colored_label(palette::TEXT_FAINT(), "Click + to add one.");
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                style::section_header(ui, "Schema");
                ui.horizontal(|ui| {
                    icons::show_weak(ui, icons::filter(), 15.0);
                    let w = ui.available_width();
                    style::text_input(ui, &mut self.schema_filter, "filter tables…", w);
                });
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .id_salt("schema_scroll")
                    .show(ui, |ui| {
                        self.schema_tree(ui, actions);
                    });
            });
    }

    fn schema_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(active) = self.active() else {
            ui.add_space(4.0);
            ui.colored_label(
                palette::TEXT_FAINT(),
                "Connect to a database to browse its schema.",
            );
            return;
        };

        // Database node (non-collapsible header row).
        ui.horizontal(|ui| {
            icons::show(ui, icons::database(), icons::SIZE);
            ui.strong(&active.schema.database_name);
        });
        ui.add_space(2.0);

        let filter = self.schema_filter.to_lowercase();
        for table in &active.schema.tables {
            if !filter.is_empty() && !table.name.to_lowercase().contains(&filter) {
                continue;
            }
            let id = ui.make_persistent_id(("tbl", table.name.as_str()));
            let (_toggle, header, _body) =
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false,
                )
                .show_header(ui, |ui| {
                    icons::show_weak(ui, icons::table(), 15.0);
                    ui.add_space(2.0);
                    ui.add(
                        egui::Label::new(table.name.as_str())
                            .sense(egui::Sense::click())
                            .selectable(false),
                    )
                })
                .body(|ui| {
                    // Columns.
                    for col in &table.columns {
                        ui.horizontal(|ui| {
                            let glyph = if col.primary_key {
                                icons::key()
                            } else {
                                icons::column()
                            };
                            icons::show_weak(ui, glyph, 13.0);
                            ui.add_space(2.0);
                            ui.label(col.name.as_str());
                            let nn = if col.nullable { "" } else { " · not null" };
                            ui.weak(format!("{}{nn}", col.data_type));
                        });
                    }
                    // Indexes.
                    if !table.indexes.is_empty() {
                        ui.add_space(3.0);
                        for idx in &table.indexes {
                            ui.horizontal(|ui| {
                                icons::show_weak(ui, icons::index(), 13.0);
                                ui.add_space(2.0);
                                let u = if idx.unique { "unique " } else { "" };
                                ui.weak(format!("{u}{} ({})", idx.name, idx.columns.join(", ")));
                            });
                        }
                    }
                });

            let resp = header.inner.on_hover_text("Click to preview rows");
            if resp.clicked() {
                actions.push(Action::OpenTable(format!(
                    "SELECT * FROM {} LIMIT 100;",
                    table.qualified()
                )));
            }
        }
    }

    /// The TablePlus-style filter strip directly above the grid. Only shown when toggled on
    /// and a result with columns is loaded. Edits mutate `self.filter` directly; an Apply or
    /// Clear rebuilds the view.
    fn filter_bar(&mut self, root: &mut egui::Ui) {
        if !self.filter.visible {
            return;
        }
        // Snapshot the column names so the closure can borrow `self.filter` mutably without
        // also holding a borrow of `self.result`.
        let col_names: Vec<String> = match &self.result {
            Some(res) if res.column_count() > 0 => {
                res.columns.iter().map(|c| c.name.clone()).collect()
            }
            _ => return,
        };

        let mut event: Option<FilterEvent> = None;
        egui::Panel::top("filter_bar")
            .resizable(false)
            .show_inside(root, |ui| {
                event = filter::ui(ui, &mut self.filter, &col_names);
            });

        match event {
            Some(FilterEvent::Apply) => self.recompute_view(),
            Some(FilterEvent::Clear) => {
                self.filter.reset();
                self.recompute_view();
            }
            None => {}
        }
    }

    fn central_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::CentralPanel::default().show_inside(root, |ui| match &self.result {
            Some(result) if result.column_count() > 0 => {
                let resp = results_grid(ui, result, &self.row_order, self.sort, self.selected_row);
                if let Some(col) = resp.sort {
                    actions.push(Action::SortBy(col));
                }
                if let Some(row) = resp.selected {
                    self.selected_row = Some(row);
                }
            }
            Some(_) => {
                style::empty_state(ui, icons::table(), "No columns", &self.status_msg);
            }
            None => {
                style::empty_state(
                    ui,
                    icons::play(),
                    "No results yet",
                    "Write a query below and press Run (Cmd/Ctrl+Enter)",
                );
            }
        });
    }

    fn connection_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let title = if editor.is_new {
            "New Connection"
        } else {
            "Edit Connection"
        };
        let mut open = true;
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("conn_form")
                    .num_columns(2)
                    .spacing([8.0, 8.0])
                    .show(ui, |ui| {
                        // One width for every field so the form reads as a tidy column.
                        let field_w = 240.0;

                        ui.label("Name");
                        style::text_input(ui, &mut editor.config.name, "", field_w);
                        ui.end_row();

                        ui.label("Type");
                        let previous_kind = editor.config.kind;
                        egui::ComboBox::from_id_salt("kind")
                            .selected_text(editor.config.kind.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    DbKind::Postgres,
                                    "PostgreSQL",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    DbKind::MySql,
                                    "MySQL",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    DbKind::MariaDb,
                                    "MariaDB",
                                );
                                ui.selectable_value(
                                    &mut editor.config.kind,
                                    DbKind::Sqlite,
                                    "SQLite",
                                );
                            });
                        if editor.config.kind != previous_kind {
                            editor.config.port = editor.config.kind.default_port();
                        }
                        ui.end_row();

                        if editor.config.kind.is_server() {
                            ui.label("Host");
                            style::text_input(ui, &mut editor.config.host, "", field_w);
                            ui.end_row();

                            ui.label("Port");
                            ui.add_sized(
                                egui::vec2(80.0, style::CONTROL_H),
                                egui::DragValue::new(&mut editor.config.port),
                            );
                            ui.end_row();

                            ui.label("User");
                            style::text_input(ui, &mut editor.config.user, "", field_w);
                            ui.end_row();

                            ui.label("Password");
                            ui.add_sized(
                                egui::vec2(field_w, style::CONTROL_H),
                                egui::TextEdit::singleline(&mut editor.password)
                                    .password(true)
                                    .vertical_align(egui::Align::Center)
                                    .margin(egui::Margin::symmetric(6, 0)),
                            );
                            ui.end_row();

                            ui.label("Database");
                            style::text_input(ui, &mut editor.config.database, "", field_w);
                            ui.end_row();
                        } else {
                            ui.label("File");
                            ui.horizontal(|ui| {
                                style::text_input(
                                    ui,
                                    &mut editor.config.sqlite_path,
                                    "/path/to/database.sqlite",
                                    field_w,
                                );
                                if ui.button("Browse…").clicked() {
                                    actions.push(Action::BrowseSqlitePath);
                                }
                            });
                            ui.end_row();
                        }
                    });

                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if icons::button(ui, icons::save(), "Save", true).clicked() {
                        actions.push(Action::SaveConnection);
                    }
                    if icons::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelDialog);
                    }
                });
            });
        if !open {
            actions.push(Action::CancelDialog);
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
            let out = ctx.run_ui(raw, |ui| app.draw(ui));
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
