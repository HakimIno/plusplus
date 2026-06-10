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
    /// A query finished. `tab_id` routes the result back to the tab that started it, even
    /// if the user has since switched tabs.
    Queried {
        tab_id: u64,
        result: Result<QueryResult, String>,
    },
    /// A batch of staged edits was saved (`Ok` carries the number of rows updated).
    Committed {
        tab_id: u64,
        result: Result<usize, String>,
    },
}

/// What the background runtime is currently doing (drives the spinner / disables buttons).
#[derive(Clone, Copy, PartialEq)]
enum Busy {
    Idle,
    Connecting,
    Querying,
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
    /// Currently selected display row (drives the Details panel).
    selected_row: Option<usize>,
    /// Staged cell edits and the editable source of the current result.
    edits: Edits,
    /// TablePlus-style result filter bar (column / operator / value conditions).
    filter: FilterState,
    /// Data vs Structure view in the central panel (table tabs only).
    view: TabView,
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
            selected_row: None,
            edits: Edits::default(),
            filter: FilterState::default(),
            view: TabView::default(),
        }
    }

    /// Install a freshly returned result and rebuild the display order.
    fn set_result(&mut self, res: QueryResult) {
        self.sort = None;
        self.selected_row = None;
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
        // A row that filtered out can't stay selected.
        if self.selected_row.is_some_and(|s| s >= self.row_order.len()) {
            self.selected_row = None;
        }
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

    /// Commit the cell currently being typed into the staged set. Returns `false` if its
    /// value is invalid (the editor stays open), so callers can refuse to proceed.
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
}

/// Human-readable status line for a completed result.
fn result_status(res: &QueryResult) -> String {
    match res.stats.rows_affected {
        Some(n) => format!("OK — {n} row(s) affected in {:.1} ms", res.stats.elapsed_ms),
        None => format!(
            "{} row(s) × {} col(s) in {:.1} ms",
            res.row_count(),
            res.column_count(),
            res.stats.elapsed_ms
        ),
    }
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
    /// Pin a preview tab as permanent (double-click on the tab).
    PinTab(usize),
    NewConnection,
    EditConnection(usize),
    DeleteConnection(usize),
    SaveConnection,
    CancelDialog,
    OpenSettings,
    CloseSettings,
    BrowseSqlitePath,
    RunQuery,
    /// Open a table's rows from the sidebar. `source` makes the result editable. `pin` opens
    /// it as a permanent tab (double-click) rather than the reusable italic preview tab.
    OpenTable {
        sql: String,
        source: EditSource,
        pin: bool,
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
    settings_open: bool,
    schema_filter: String,
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
        let mut app = Self::construct();
        // Restore the saved workspace (open tabs + their SQL/connection binding). Kept out of
        // `construct` so tests get a deterministic single-tab app independent of disk state.
        app.restore_workspace();

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
            active_connections: Vec::new(),
            tabs: vec![default_tab],
            active_query_tab: 0,
            next_tab_id: 1,
            workspace_dirty: false,
            last_workspace_save: std::time::Instant::now(),
            editor: None,
            settings_open: false,
            schema_filter: String::new(),
            status_msg: "Ready".to_string(),
            error: None,
            show_connection_tabs: true,
            show_schema_panel: true,
            show_details_panel: true,
            theme,
        }
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

    /// The live connection the active tab is bound to, if it's currently connected.
    fn active(&self) -> Option<&ActiveConnection> {
        let id = self.tab().conn_id.as_deref()?;
        self.active_connections.iter().find(|c| c.config_id == id)
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

    fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            // Always keep at least one tab: reset the last one to a blank scratch tab,
            // preserving its connection binding.
            let id = self.next_tab_id;
            self.next_tab_id += 1;
            let conn_id = self.tabs[0].conn_id.clone();
            self.tabs[0] = QueryTab::new(id, String::new());
            self.tabs[0].conn_id = conn_id;
            self.active_query_tab = 0;
        } else {
            self.tabs.remove(idx);
            if self.active_query_tab > idx
                || self.active_query_tab >= self.tabs.len()
            {
                self.active_query_tab = self.active_query_tab.saturating_sub(1);
            }
        }
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
            return;
        }

        // Pick the target tab: the existing preview slot, else a blank scratch active tab,
        // else a brand-new tab.
        let idx = if let Some(i) = self.tabs.iter().position(|t| t.preview) {
            i
        } else {
            let cur = &self.tabs[self.active_query_tab];
            if cur.edits.source.is_none() && cur.result.is_none() && cur.sql.trim().is_empty() {
                self.active_query_tab
            } else {
                self.new_tab();
                self.active_query_tab
            }
        };

        // Rebuild the tab from scratch (clearing any previous preview's result/filter/edits),
        // keeping its stable id and connection binding.
        let id = self.tabs[idx].id;
        let conn_id = self.tabs[idx].conn_id.clone();
        let mut tab = QueryTab::new(id, source.table.clone());
        tab.conn_id = conn_id;
        tab.sql = sql;
        tab.preview = !pin;
        tab.edits.pending_source = Some(source);
        self.tabs[idx] = tab;
        self.active_query_tab = idx;
        self.workspace_dirty = true;
        self.start_query_for(idx);
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
                            } else {
                                self.active_connections.push(active);
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
                AppMessage::Queried { tab_id, result } => {
                    self.busy = Busy::Idle;
                    let is_active = self.tabs.get(self.active_query_tab).is_some_and(|t| t.id == tab_id);
                    let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
                        continue;
                    };
                    match result {
                        Ok(res) => {
                            // Promote the in-flight source and start from a clean edit slate.
                            tab.edits.source = tab.edits.pending_source.take();
                            tab.edits.clear();
                            let status = result_status(&res);
                            tab.set_result(res);
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
                AppMessage::Committed { tab_id, result } => {
                    self.busy = Busy::Idle;
                    let is_active = self.tabs.get(self.active_query_tab).is_some_and(|t| t.id == tab_id);
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

    /// Run the SQL of the tab at `idx` against its bound connection.
    fn start_query_for(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let sql = tab.sql.trim().to_string();
        if sql.is_empty() {
            return;
        }
        let tab_id = tab.id;
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
        self.busy = Busy::Querying;
        self.error = None;
        self.status_msg = "Running query…".to_string();
        self.rt.spawn(async move {
            let result = db.execute(&sql).await.map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::Queried { tab_id, result });
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
                    t.schema.as_deref().is_some_and(|ts| ts.eq_ignore_ascii_case(s))
                })
        });
        let info = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let pk_cols: Vec<String> = info
            .columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.clone())
            .collect();
        if pk_cols.is_empty() {
            return None;
        }
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

    /// Turn all staged edits into `UPDATE` statements and run them on the background runtime.
    /// Each changed row becomes one statement keyed by the source table's primary key.
    fn commit_edits(&mut self) {
        let idx = self.active_query_tab;
        // A cell still being edited with invalid (red) input blocks the whole save.
        if !self.tabs[idx].flush_active_edit() {
            self.error = Some("Fix the highlighted cell before saving.".into());
            self.status_msg = "Invalid value — not saved".to_string();
            return;
        }
        if !self.tabs[idx].edits.has_pending() {
            return;
        }
        // Defence in depth: every staged value must still match its column kind before we
        // build any SQL, so a malformed value can never reach the database.
        for colmap in self.tabs[idx].edits.cells.values() {
            for (&col, value) in colmap {
                if !self.tabs[idx].edits.col_kind(col).accepts(value) {
                    self.error =
                        Some("Cannot save: a cell holds a value invalid for its type.".into());
                    self.status_msg = "Invalid value — not saved".to_string();
                    return;
                }
            }
        }
        let Some(source) = self.tabs[idx].edits.source.clone() else {
            return;
        };
        // Grab the dialect + a connection handle, then drop the `active()` borrow so we can
        // freely touch `self` below.
        let (kind, db) = match self.active() {
            Some(active) => (active.db.kind(), active.db.clone()),
            None => return,
        };
        let tab_id = self.tabs[idx].id;
        let Some(result) = &self.tabs[idx].result else {
            return;
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
            return;
        };

        let mut statements = Vec::new();
        for (&row, colmap) in &self.tabs[idx].edits.cells {
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
            let _ = tx.send(AppMessage::Committed {
                tab_id,
                result: outcome,
            });
        });
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
            Action::PinTab(i) => {
                if let Some(tab) = self.tabs.get_mut(i) {
                    tab.preview = false;
                }
                self.select_tab(i);
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
                let idx = self.active_query_tab;
                // Editability is re-derived from the SQL itself on every run: any simple
                // single-table `SELECT *` — including a hand-tuned LIMIT/WHERE/ORDER BY —
                // stays editable; anything else runs as a read-only ad-hoc query.
                self.tabs[idx].edits.pending_source = self.derive_edit_source(idx);
                self.start_query_for(idx);
            }
            Action::OpenTable { sql, source, pin } => self.open_table(sql, source, pin),
            Action::SortBy(col) => self.tab_mut().apply_sort(col),
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
                    source: t.edits.source.as_ref().map(|s| dbcore::config::WorkspaceSource {
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

        let mut actions: Vec<Action> = Vec::new();

        // Global shortcut: Cmd/Ctrl+Enter runs the query.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            actions.push(Action::RunQuery);
        }
        // Cmd/Ctrl+S saves staged cell edits (TablePlus-style) as UPDATE statements.
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.commit_edits();
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
        // status bar is carved first so it pins to the very bottom edge; the query console is
        // carved next so it sits directly below the grid, leaving its top resize handle
        // bordering the central area (nothing on top of it) for a clean, smooth drag.
        self.top_bar(ui_root, frame, &mut actions);
        self.query_tab_bar(ui_root, &mut actions);
        self.status_bar(ui_root);
        self.query_console(ui_root, &mut actions);
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
        // ...and a bottom panel here carves the Data/Structure switch directly below it.
        self.view_mode_bar(ui_root);
        self.central_panel(ui_root, &mut actions);
        self.connection_dialog(&ctx, &mut actions);
        self.settings_dialog(&ctx, &mut actions);

        let structural = actions.iter().any(|a| {
            matches!(
                a,
                Action::NewTab
                    | Action::CloseTab(_)
                    | Action::SelectTab(_)
                    | Action::Connect(_)
                    | Action::BindConnection(_)
                    | Action::OpenTable { .. }
                    | Action::DeleteConnection(_)
            )
        });
        for action in actions {
            self.apply_action(action);
        }

        // Persist the workspace: immediately after structural changes, otherwise on a throttle
        // (so typing SQL into a tab is eventually saved without writing every frame).
        self.maybe_save_workspace(structural);
        if self.workspace_dirty {
            ctx.request_repaint_after(std::time::Duration::from_millis(1600));
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
            tab.selected_row = Some(7); // render the Details panel
            tab.filter.visible = true; // render the filter bar too
            tab.conn_id = Some("test".into());
        }
        let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
        app.active_connections.push(ActiveConnection {
            config_id: "test".into(),
            name: "test-conn".into(),
            db,
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
}
