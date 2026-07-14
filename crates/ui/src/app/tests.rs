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
    async fn execute_capped(&self, _sql: &str, _max_rows: usize) -> dbcore::Result<QueryResult> {
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

struct DelayedMetadataDb;

#[async_trait::async_trait]
impl dbcore::Database for DelayedMetadataDb {
    fn kind(&self) -> dbcore::DbKind {
        dbcore::DbKind::Sqlite
    }

    async fn introspect_overview(&self) -> dbcore::Result<SchemaTree> {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        Ok(fake_schema(2, 0))
    }

    async fn introspect(&self) -> dbcore::Result<SchemaTree> {
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        Ok(fake_schema(2, 1))
    }

    async fn list_databases(&self) -> dbcore::Result<Vec<String>> {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        Ok(vec!["testdb".into()])
    }

    async fn execute_capped(&self, _sql: &str, _max_rows: usize) -> dbcore::Result<QueryResult> {
        unreachable!()
    }

    async fn execute_transaction(&self, _stmts: &[String]) -> dbcore::Result<usize> {
        unreachable!()
    }

    async fn export_query(
        &self,
        _sql: &str,
        _sink: &mut (dyn dbcore::RowSink + Send),
    ) -> dbcore::Result<u64> {
        unreachable!()
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

#[test]
fn metadata_pipeline_exposes_fast_results_before_full_schema() {
    let app = DbGuiApp::construct();
    let (tx, rx) = std::sync::mpsc::channel();
    app.rt.block_on(load_connection_metadata(
        Arc::new(DelayedMetadataDb),
        "slow-connection".into(),
        tx,
    ));

    let messages: Vec<_> = rx.try_iter().collect();
    assert_eq!(messages.len(), 3);
    assert!(matches!(messages[0], AppMessage::DatabaseListLoaded { .. }));
    assert!(matches!(
        messages[1],
        AppMessage::SchemaOverviewLoaded { .. }
    ));
    assert!(matches!(messages[2], AppMessage::SchemaLoaded { .. }));

    let overview_ms = match &messages[1] {
        AppMessage::SchemaOverviewLoaded { elapsed_ms, .. } => *elapsed_ms,
        _ => unreachable!(),
    };
    let full_schema_ms = match &messages[2] {
        AppMessage::SchemaLoaded { elapsed_ms, .. } => *elapsed_ms,
        _ => unreachable!(),
    };
    assert!(
        overview_ms >= 25.0,
        "overview timing was {overview_ms:.1} ms"
    );
    assert!(
        full_schema_ms >= 55.0,
        "schema timing was {full_schema_ms:.1} ms"
    );
}

#[test]
fn connection_becomes_live_before_schema_arrives() {
    let mut app = DbGuiApp::construct();
    let ctx = egui::Context::default();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.id = "conn-1".into();
    cfg.name = "Remote DB".into();
    app.connections.push(cfg);
    app.connection_jobs.insert("conn-1".into());
    app.busy = Busy::Connecting;

    app.tx
        .send(AppMessage::Connected {
            conn_id: "conn-1".into(),
            name: "Remote DB".into(),
            elapsed_ms: 12.5,
            result: Ok(Arc::new(DummyDb)),
        })
        .unwrap();
    app.poll_messages(&ctx);

    assert_eq!(app.busy, Busy::Idle);
    assert!(app.connection_jobs.contains("conn-1"));
    assert_eq!(app.active_connections.len(), 1);
    assert!(app.active_connections[0].schema.tables.is_empty());
    assert!(app.status_msg.contains("loading schema"));
    assert_eq!(app.connection_timings["conn-1"].connect_ms, Some(12.5));

    let mut overview = fake_schema(2, 0);
    overview.tables.iter_mut().for_each(|table| {
        table.indexes.clear();
        table.foreign_keys.clear();
    });
    app.tx
        .send(AppMessage::SchemaOverviewLoaded {
            conn_id: "conn-1".into(),
            schema: overview,
            elapsed_ms: 20.0,
        })
        .unwrap();
    app.poll_messages(&ctx);

    assert_eq!(app.active_connections[0].schema.tables.len(), 2);
    assert!(app.connection_jobs.contains("conn-1"));
    assert!(app.active_connections[0].schema.tables[0]
        .columns
        .is_empty());
    assert!(app.status_msg.contains("loading details"));
    assert_eq!(app.connection_timings["conn-1"].overview_ms, Some(20.0));

    app.tx
        .send(AppMessage::SchemaLoaded {
            conn_id: "conn-1".into(),
            elapsed_ms: 80.0,
            result: Ok(fake_schema(2, 1)),
        })
        .unwrap();
    app.poll_messages(&ctx);

    assert_eq!(app.active_connections[0].schema.tables.len(), 2);
    assert!(!app.connection_jobs.contains("conn-1"));
    assert!(app.status_msg.contains("2 tables"));
    assert_eq!(app.connection_timings["conn-1"].full_schema_ms, Some(80.0));

    app.tx
        .send(AppMessage::DatabaseListLoaded {
            conn_id: "conn-1".into(),
            databases: vec!["main".into(), "analytics".into()],
            elapsed_ms: 15.0,
        })
        .unwrap();
    app.poll_messages(&ctx);
    assert_eq!(app.active_connections[0].databases.len(), 2);
    assert_eq!(
        app.connection_timings["conn-1"].database_list_ms,
        Some(15.0)
    );

    app.disconnect_conn("conn-1");
    assert!(app.active_connections.is_empty());
    assert!(app.schema_cache.contains_key("conn-1"));
    app.tx
        .send(AppMessage::Connected {
            conn_id: "conn-1".into(),
            name: "Remote DB".into(),
            elapsed_ms: 9.0,
            result: Ok(Arc::new(DummyDb)),
        })
        .unwrap();
    app.poll_messages(&ctx);
    assert_eq!(app.active_connections[0].schema.tables[0].columns.len(), 1);
    assert!(app.status_msg.contains("cached schema"));

    app.tx
        .send(AppMessage::SchemaOverviewLoaded {
            conn_id: "conn-1".into(),
            schema: fake_schema(2, 0),
            elapsed_ms: 18.0,
        })
        .unwrap();
    app.poll_messages(&ctx);
    assert_eq!(
        app.active_connections[0].schema.tables[0].columns.len(),
        1,
        "name-only overview must not replace a complete cached schema"
    );
}

#[test]
fn schema_failure_keeps_connection_live() {
    let mut app = DbGuiApp::construct();
    let ctx = egui::Context::default();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.id = "conn-1".into();
    cfg.name = "Remote DB".into();
    app.connections.push(cfg);
    app.connection_jobs.insert("conn-1".into());
    app.tx
        .send(AppMessage::Connected {
            conn_id: "conn-1".into(),
            name: "Remote DB".into(),
            elapsed_ms: 10.0,
            result: Ok(Arc::new(DummyDb)),
        })
        .unwrap();
    app.poll_messages(&ctx);

    app.tx
        .send(AppMessage::SchemaLoaded {
            conn_id: "conn-1".into(),
            elapsed_ms: 50.0,
            result: Err("metadata permission denied".into()),
        })
        .unwrap();
    app.poll_messages(&ctx);

    assert_eq!(app.active_connections.len(), 1);
    assert!(!app.connection_jobs.contains("conn-1"));
    assert!(app
        .error
        .as_deref()
        .unwrap()
        .contains("metadata permission denied"));
    assert_eq!(app.status_msg, "Connected — schema unavailable");
}

#[test]
fn duplicate_connect_is_rejected_before_opening_another_pool() {
    let mut app = DbGuiApp::construct();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.id = "conn-1".into();
    cfg.name = "Busy DB".into();
    app.connections.push(cfg);
    app.connection_jobs.insert("conn-1".into());
    let jobs_before = app.connection_jobs.len();

    app.start_connect(app.connections.len() - 1);

    assert_eq!(app.connection_jobs.len(), jobs_before);
    assert!(app.connection_jobs.contains("conn-1"));
    assert!(app.status_msg.contains("already connecting"));
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
    assert!(app
        .error
        .as_deref()
        .unwrap_or("")
        .contains("Binary columns"));
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
    fonts.families.insert(
        egui::FontFamily::Name(crate::HEADING_FAMILY.into()),
        proportional,
    );
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

    app.open_table(
        "SELECT * FROM users".into(),
        src,
        false,
        crate::components::QueryTabKind::Table,
    );

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
    app.open_table(
        "q".into(),
        src("users"),
        false,
        crate::components::QueryTabKind::Table,
    );
    assert_eq!(app.tabs.len(), 1);
    assert!(app.tab().preview);
    assert_eq!(app.tab().title, "users");

    // Re-opening the same table doesn't add a tab.
    app.open_table(
        "q".into(),
        src("users"),
        false,
        crate::components::QueryTabKind::Table,
    );
    assert_eq!(app.tabs.len(), 1);

    // A different table reuses the same preview slot (no pile-up).
    app.open_table(
        "q".into(),
        src("orders"),
        false,
        crate::components::QueryTabKind::Table,
    );
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tab().title, "orders");
    assert!(app.tab().preview);

    // Pinning the open table (double-click) makes it permanent.
    app.open_table(
        "q".into(),
        src("orders"),
        true,
        crate::components::QueryTabKind::Table,
    );
    assert_eq!(app.tabs.len(), 1);
    assert!(!app.tab().preview);

    // With no preview slot and a non-scratch active tab, a new table opens a new tab.
    app.open_table(
        "q".into(),
        src("products"),
        false,
        crate::components::QueryTabKind::Table,
    );
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tab().title, "products");
    assert!(app.tab().preview);
}

#[test]
fn preview_reuse_never_mixes_connection_dialects() {
    let source = EditSource {
        schema: Some("backend".into()),
        table: "ValetParking".into(),
        pk_cols: Vec::new(),
    };
    let mut app = DbGuiApp::construct();
    app.tab_mut().sql.clear();
    app.tab_mut().conn_id = Some("postgres".into());
    app.open_table(
        "SELECT * FROM \"backend\".\"ValetParking\" LIMIT 100;".into(),
        source.clone(),
        false,
        crate::components::QueryTabKind::Table,
    );

    app.new_tab();
    app.tab_mut().conn_id = Some("mysql".into());
    app.open_table(
        "SELECT * FROM `backend`.`ValetParking` LIMIT 100;".into(),
        source,
        false,
        crate::components::QueryTabKind::Table,
    );

    assert_eq!(app.tab().conn_id.as_deref(), Some("mysql"));
    assert_eq!(
        app.tab().sql,
        "SELECT * FROM `backend`.`ValetParking` LIMIT 100;"
    );
}

#[test]
fn view_tabs_keep_their_view_icon_kind() {
    let mut app = DbGuiApp::construct();
    let source = EditSource {
        schema: Some("public".into()),
        table: "active_users".into(),
        pk_cols: Vec::new(),
    };

    app.open_table(
        "SELECT * FROM public.active_users".into(),
        source,
        false,
        crate::components::QueryTabKind::View,
    );

    assert_eq!(
        app.tab_kind(app.active_query_tab),
        crate::components::QueryTabKind::View
    );
}

#[test]
fn definition_tabs_keep_their_schema_object_icon_kind() {
    let mut app = DbGuiApp::construct();
    for kind in [
        crate::components::QueryTabKind::Function,
        crate::components::QueryTabKind::Procedure,
        crate::components::QueryTabKind::Trigger,
    ] {
        app.open_definition("object".into(), "CREATE ...".into(), kind);
        assert_eq!(app.tab_kind(app.active_query_tab), kind);
    }
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

/// Build an app with a live SQLite connection whose schema is `ddl`. Returns the app and the
/// temp directory holding the database (delete when done). Shared by the screenshot generators.
fn demo_app_with_ddl(ddl: &[&str]) -> (DbGuiApp, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    // Unique per call — the screenshot tests run in one process and must not share a file —
    // but the uniqueness lives in the *directory*: the sidebar and title bar render the
    // database's file name, so a pid in it would churn the committed PNG every run.
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "plusplus-snap-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("demo.sqlite");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut cfg = dbcore::ConnectionConfig::new(DbKind::Sqlite);
    cfg.name = "demo".into();
    cfg.sqlite_path = path.to_string_lossy().into_owned();
    let (db, schema): (Arc<dyn dbcore::Database>, SchemaTree) = rt.block_on(async {
        let db = dbcore::connect(&cfg, None, None).await.unwrap();
        for stmt in ddl {
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
    (app, dir)
}

/// A table, a view, and a trigger — the object browser's demo schema.
fn demo_app_with_objects() -> (DbGuiApp, std::path::PathBuf) {
    demo_app_with_ddl(&[
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)",
        "CREATE TABLE audit (id INTEGER PRIMARY KEY, msg TEXT)",
        "CREATE VIEW active_users AS SELECT id, email FROM users WHERE email IS NOT NULL",
        "CREATE TRIGGER log_new_user AFTER INSERT ON users FOR EACH ROW \
             BEGIN INSERT INTO audit(msg) VALUES ('new user'); END",
    ])
}

/// Render `app` headlessly and write a PNG snapshot named `name`. Optionally expands the
/// sidebar object groups first. The UI animates a button glint (continuous repaint), so we
/// step a fixed number of frames rather than running to quiescence.
fn render_and_snapshot(mut app: DbGuiApp, name: &str, expand_groups: bool) {
    use egui_kittest::kittest::Queryable;
    // `construct` loads the developer's real saved connections, which the rail then paints
    // into the PNG: machine-dependent pixels, and their names committed to git. Snapshots
    // render the empty rail instead.
    app.connections.clear();
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
        &[
            "id",
            "Email",
            "age",
            "created_at",
            "is_active",
            "legacy_note",
        ],
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
#[ignore = "screenshot generator; run manually with --ignored"]
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
                    i.events
                        .push(egui::Event::PointerMoved(egui::pos2(300.0, 400.0)));
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
        .map(|i| {
            col(
                &format!("column_{i:02}"),
                types[i % types.len()],
                true,
                i == 0,
            )
        })
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
    let (app, dir) = demo_app_with_objects();
    render_and_snapshot(app, "object_browser", true);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Screenshot generator (ignored): the dialect-adaptive visual Trigger editor, opened on
/// the demo database's existing trigger.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_trigger_editor() {
    let (mut app, dir) = demo_app_with_objects();
    let trigger = app.active().unwrap().schema.triggers[0].clone();
    app.apply_action(Action::OpenEditTrigger(trigger));
    render_and_snapshot(app, "trigger_editor", false);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Screenshot generator (ignored): the table editor's Foreign Keys tab. Its fields once ran
/// on three different height regimes — this pins them to one.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_table_editor_foreign_keys() {
    let (mut app, dir) = demo_app_with_ddl(&[
        "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE orders (id INTEGER PRIMARY KEY)",
        "CREATE TABLE order_items (\
             id INTEGER PRIMARY KEY, \
             product_id INTEGER REFERENCES products(id), \
             order_id INTEGER REFERENCES orders(id) ON DELETE CASCADE)",
    ]);
    let table = app
        .active()
        .unwrap()
        .schema
        .tables
        .iter()
        .find(|t| t.name == "order_items")
        .expect("order_items introspected")
        .clone();
    app.apply_action(Action::OpenEditTable(table));
    match app.tab_mut().schema_editor.as_mut() {
        Some(ObjectEditor::Table(editor)) => {
            editor.active_tab = crate::schema::SchemaTab::ForeignKeys;
        }
        _ => panic!("OpenEditTable should install a table editor"),
    }
    render_and_snapshot(app, "table_editor_foreign_keys", false);
    let _ = std::fs::remove_dir_all(&dir);
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
        kind: crate::components::QueryTabKind::Table,
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
    let pos =
        find_text_pos(&out.shapes, "Coffee").expect("value box not painted") + egui::vec2(4.0, 4.0);
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

/// Every control in a form row must share one height, or a row of them reads as ragged.
/// [`style::CONTROL_H`] is the single knob; this pins each shipped widget to it. Text fields
/// get there via `add_sized`, buttons and combos via `spacing.interact_size.y` — egui's
/// `small_button` opts out of that minimum, which is why the app must not use it.
#[test]
fn every_form_control_shares_one_height() {
    use crate::components;

    let heights: std::rc::Rc<std::cell::RefCell<Vec<(&str, f32)>>> = Default::default();
    let sink = heights.clone();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1200.0, 120.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            let (mut text, mut choice) = (String::new(), 0usize);
            let mut probe = sink.borrow_mut();
            probe.clear();
            ui.horizontal(|ui| {
                let r = components::text_input(ui, &mut text, "hint", 90.0);
                probe.push(("text_input", r.rect.height()));
                let r = components::text_input_enabled(ui, false, &mut text, "hint", 90.0);
                probe.push(("text_input_enabled", r.rect.height()));
                let r = components::password_input(ui, &mut text, "", 90.0);
                probe.push(("password_input", r.rect.height()));
                let r =
                    components::icon_text_input(ui, &mut text, "", crate::icons::search(), 90.0);
                probe.push(("icon_text_input", r.rect.height()));
                let r = components::Btn::new("Default").show(ui);
                probe.push(("Btn::new", r.rect.height()));
                let r = components::Btn::primary("Primary").show(ui);
                probe.push(("Btn::primary", r.rect.height()));
                let r = components::Btn::danger("Drop").show(ui);
                probe.push(("Btn::danger", r.rect.height()));
                let r = components::Btn::new("Icon")
                    .icon(crate::icons::connect())
                    .show(ui);
                probe.push(("Btn+icon", r.rect.height()));
                let r = components::Btn::ghost_icon(crate::icons::trash()).show(ui);
                probe.push(("Btn::ghost_icon", r.rect.height()));
                let r = ui.button("menu item");
                probe.push(("ui.button", r.rect.height()));
                let r = egui::ComboBox::from_id_salt("height_probe")
                    .selected_text("select")
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut choice, 0, "a");
                    });
                probe.push(("ComboBox", r.response.rect.height()));
            });
        });
    // The first frame lays out before `style::apply` lands; step past it.
    harness.run_steps(3);
    drop(harness);

    let probe = heights.borrow();
    assert!(!probe.is_empty(), "no controls were measured");
    let ragged: Vec<_> = probe
        .iter()
        .filter(|(_, h)| (*h - crate::style::CONTROL_H).abs() > 0.01)
        .collect();
    assert!(
        ragged.is_empty(),
        "controls must all be {}pt tall, but these are not: {ragged:?}",
        crate::style::CONTROL_H,
    );
}
