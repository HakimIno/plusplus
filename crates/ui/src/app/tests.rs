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
        // Background queries may legitimately land here in tests that only assert on the
        // UI-side state; an empty result keeps them quiet.
        Ok(QueryResult::default())
    }
    async fn execute_transaction(&self, stmts: &[String]) -> dbcore::Result<usize> {
        // Like query execution above, some UI-state tests intentionally stop before polling
        // the background completion message.
        Ok(stmts.len())
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
                        default: None,
                        check: None,
                        comment: None,
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
        tokio_util::sync::CancellationToken::new(),
    ));

    let messages: Vec<_> = rx.try_iter().collect();
    assert_eq!(messages.len(), 4);
    assert!(matches!(messages[0], AppMessage::DatabaseListLoaded { .. }));
    assert!(matches!(
        messages[1],
        AppMessage::SchemaOverviewLoaded { .. }
    ));
    assert!(matches!(messages[2], AppMessage::SchemaLoaded { .. }));
    assert!(matches!(
        messages[3],
        AppMessage::ConnectionJobFinished { .. }
    ));

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
fn disconnect_cancels_the_connection_metadata_pipeline() {
    let mut app = DbGuiApp::construct();
    let cancel = tokio_util::sync::CancellationToken::new();
    app.connection_jobs.insert("conn-1".into());
    app.connection_cancels
        .insert("conn-1".into(), cancel.clone());

    app.disconnect_conn("conn-1");

    assert!(cancel.is_cancelled());
    assert!(!app.connection_cancels.contains_key("conn-1"));
}

#[test]
fn cancelled_connection_job_drops_a_late_connected_handle() {
    let mut app = DbGuiApp::construct();
    let ctx = egui::Context::default();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.id = "conn-1".into();
    cfg.name = "DB".into();
    app.connections.push(cfg);
    app.connection_jobs.insert("conn-1".into());
    app.tx
        .send(AppMessage::Connected {
            conn_id: "conn-1".into(),
            name: "DB".into(),
            elapsed_ms: 1.0,
            result: Ok(Arc::new(DummyDb)),
        })
        .unwrap();
    app.tx
        .send(AppMessage::ConnectionJobCancelled {
            conn_id: "conn-1".into(),
        })
        .unwrap();

    app.poll_messages(&ctx);

    assert!(app.active_connections.is_empty());
    assert!(!app.connection_jobs.contains("conn-1"));
}

#[test]
fn schema_refresh_result_does_not_clear_a_newer_connection_job() {
    let mut app = DbGuiApp::construct();
    let ctx = egui::Context::default();
    let cancel = tokio_util::sync::CancellationToken::new();
    app.connection_jobs.insert("conn-1".into());
    app.connection_cancels
        .insert("conn-1".into(), cancel.clone());
    app.active_connections.push(ActiveConnection {
        config_id: "conn-1".into(),
        name: "DB".into(),
        db: Arc::new(DummyDb),
        schema: SchemaTree::default(),
        databases: Vec::new(),
    });
    app.tx
        .send(AppMessage::SchemaLoaded {
            conn_id: "conn-1".into(),
            elapsed_ms: 1.0,
            result: Ok(fake_schema(1, 1)),
        })
        .unwrap();

    app.poll_messages(&ctx);

    assert!(app.connection_jobs.contains("conn-1"));
    assert!(app.connection_cancels.contains_key("conn-1"));
    assert!(!cancel.is_cancelled());
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
    assert!(app.connection_jobs.contains("conn-1"));
    assert!(app.status_msg.contains("2 tables"));
    assert_eq!(app.connection_timings["conn-1"].full_schema_ms, Some(80.0));

    app.tx
        .send(AppMessage::ConnectionJobFinished {
            conn_id: "conn-1".into(),
        })
        .unwrap();
    app.poll_messages(&ctx);
    assert!(!app.connection_jobs.contains("conn-1"));

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
    app.tx
        .send(AppMessage::ConnectionJobFinished {
            conn_id: "conn-1".into(),
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

#[test]
fn new_connection_starts_with_an_explicit_development_profile() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::NewConnection);

    let editor = app.editor.as_ref().expect("connection editor");
    assert_eq!(
        editor.config.safety_profile,
        dbcore::SafetyProfile::Development
    );
    assert!(!editor.config.is_production());
    assert!(!editor.config.is_read_only());
    assert!(editor.selecting_provider);
}

#[test]
fn production_safety_profile_is_fail_closed_before_normalization() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.id = "c1".into();
    cfg.safety_profile = dbcore::SafetyProfile::Production;
    // Simulate a hand-edited config with stale legacy flags. The profile must still win.
    cfg.production = false;
    cfg.read_only = false;
    app.connections.push(cfg);
    app.tab_mut().conn_id = Some("c1".into());

    assert!(app.tab_connection_is_production(0));
    assert!(app.tab_connection_is_read_only(0));
    assert!(app.connection_is_read_only("c1"));
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
    assert!(pending.statements[0].missing_where);
    assert!(pending.preflights.is_none());
    assert_eq!(app.busy, Busy::Idle);

    // Cancel drops it without running.
    app.apply_action(Action::CancelDangerQuery);
    assert!(app.danger_pending.is_none());
    assert_eq!(app.busy, Busy::Idle);

    // Confirmation cannot bypass an in-flight preflight.
    app.apply_action(Action::RunQuery);
    app.apply_action(Action::ConfirmDangerQuery);
    assert!(app.danger_pending.is_some());
    assert_eq!(app.busy, Busy::Idle);

    // Critical risk additionally requires the exact target phrase.
    app.danger_pending.as_mut().unwrap().preflights =
        Some(vec![dbcore::safety::ProductionPreflight::default()]);
    app.apply_action(Action::ConfirmDangerQuery);
    assert!(app.danger_pending.is_some());
    app.apply_action(Action::SetDangerConfirmation("table_0".into()));
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

#[test]
fn staging_guard_allows_plain_inserts_but_reviews_other_writes() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    let mut cfg = dbcore::ConnectionConfig::new(dbcore::DbKind::Sqlite);
    cfg.id = "c1".into();
    cfg.set_safety_profile(dbcore::SafetyProfile::Staging);
    app.connections.push(cfg);
    app.active_connections.push(ActiveConnection {
        config_id: "c1".into(),
        name: "staging".into(),
        db: std::sync::Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    app.tab_mut().conn_id = Some("c1".into());

    app.tab_mut().sql = "INSERT INTO table_0 VALUES (1)".into();
    app.apply_action(Action::RunQuery);
    assert!(app.danger_pending.is_none());
    assert_eq!(app.busy, Busy::Querying);

    app.busy = Busy::Idle;
    for sql in [
        "CREATE TABLE staging_copy (id INT)",
        "PRAGMA journal_mode = WAL",
    ] {
        app.tab_mut().sql = sql.into();
        app.apply_action(Action::RunQuery);
        assert!(app.danger_pending.is_some(), "Guardian skipped: {sql}");
        assert_eq!(app.busy, Busy::Idle);
        app.apply_action(Action::CancelDangerQuery);
    }
}

#[test]
fn global_result_budget_evicts_inactive_lru_and_protects_active_result() {
    let mut app = DbGuiApp::construct();
    app.tabs[0].set_result(fake_result(64, 3));
    app.touch_result(0);
    app.new_tab();
    app.tabs[1].set_result(fake_result(64, 3));
    app.touch_result(1);
    app.result_memory_budget = app.tabs[1].estimated_result_memory_bytes();

    let released = app.enforce_result_memory_budget();

    assert!(released > 0);
    assert!(app.tabs[0].result.is_none());
    assert!(app.tabs[0].result_evicted);
    assert!(app.tabs[1].result.is_some());
}

#[test]
fn global_result_budget_never_discards_tabs_with_staged_edits() {
    let mut app = DbGuiApp::construct();
    app.tabs[0].set_result(fake_result(64, 3));
    app.tabs[0].edits.new_rows = 1;
    app.new_tab();
    app.tabs[1].set_result(fake_result(64, 3));
    app.new_tab();
    app.tabs[2].set_result(fake_result(64, 3));
    app.result_memory_budget =
        app.tabs[0].estimated_result_memory_bytes() + app.tabs[2].estimated_result_memory_bytes();

    app.enforce_result_memory_budget();

    assert!(app.tabs[0].result.is_some());
    assert!(app.tabs[1].result.is_none());
    assert!(app.tabs[2].result.is_some());
}

#[test]
fn production_guard_never_runs_a_query_changed_after_preflight() {
    let mut app = DbGuiApp::construct();
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
    app.tab_mut().sql = "UPDATE table_0 SET field_0 = 'safe' WHERE field_0 = 'old'".into();
    app.apply_action(Action::RunQuery);
    app.danger_pending.as_mut().unwrap().preflights =
        Some(vec![dbcore::safety::ProductionPreflight {
            affected_rows: Some(1),
            ..dbcore::safety::ProductionPreflight::default()
        }]);

    // Even a lower-risk reviewed query cannot authorize different SQL typed behind the modal.
    app.tab_mut().sql = "DELETE FROM table_0".into();
    app.apply_action(Action::ConfirmDangerQuery);
    assert!(app.danger_pending.is_none());
    assert_eq!(app.busy, Busy::Idle);
    assert!(app.error.as_deref().unwrap_or("").contains("changed"));
}

#[test]
fn production_guard_audit_failure_is_fail_closed_and_visible() {
    let mut app = DbGuiApp::construct();
    let result = app.handle_guard_audit_result(Err(dbcore::CoreError::Config(
        "audit disk is read-only".into(),
    )));

    assert!(!result);
    assert_eq!(app.status_msg, "Blocked: audit trail unavailable");
    assert!(app
        .error
        .as_deref()
        .is_some_and(|message| message.contains("mandatory audit event")));
}

#[test]
fn production_guard_also_intercepts_schema_preview_ddl() {
    let mut app = DbGuiApp::construct();
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
    let table = app.active().unwrap().schema.tables[0].clone();
    app.apply_action(Action::DropTable(table));
    let pending = app.danger_pending.as_ref().expect("DDL must be guarded");
    assert!(matches!(
        pending.continuation,
        ProductionGuardContinuation::Schema
    ));
    assert_eq!(pending.statements[0].targets, ["table_0"]);
    assert!(
        app.schema_pending.is_some(),
        "DDL stays staged until Guardian confirms"
    );
    assert_eq!(app.busy, Busy::Idle);

    app.apply_action(Action::CancelDangerQuery);
    assert!(app.danger_pending.is_none());
    assert!(
        app.schema_pending.is_none(),
        "cancelling Guardian drops the staged DDL"
    );
}

#[test]
fn production_guard_returns_to_the_staged_edit_tab_before_commit() {
    let mut app = DbGuiApp::construct();
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
    app.tab_mut().set_result(QueryResult {
        columns: vec![ColumnMeta {
            name: "field_0".into(),
            type_name: "INTEGER".into(),
        }],
        rows: vec![vec![Value::Int(1)]],
        ..QueryResult::default()
    });
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_0".into(),
        pk_cols: vec!["field_0".into()],
    });
    app.tab_mut().edits.toggle_delete(0);

    // Save skips the generic transaction preview and launches the one Guardian dialog.
    app.apply_action(Action::PreviewEdits);
    assert!(
        app.commit_pending.is_some(),
        "transaction snapshot must be retained"
    );
    let pending = app
        .danger_pending
        .as_mut()
        .expect("staged edit must be guarded");
    assert!(matches!(
        pending.continuation,
        ProductionGuardContinuation::Edits
    ));
    app.apply_action(Action::CancelDangerQuery);
    assert!(app.danger_pending.is_none());
    assert!(app.commit_pending.is_none());
    assert!(
        app.tab().edits.has_pending(),
        "staged deletion must survive cancellation"
    );

    app.apply_action(Action::PreviewEdits);
    let pending = app
        .danger_pending
        .as_mut()
        .expect("saving again must reopen Guardian directly");
    pending.preflights = Some(vec![dbcore::safety::ProductionPreflight {
        affected_rows: Some(1),
        ..dbcore::safety::ProductionPreflight::default()
    }]);

    // Even if selection changes while the background checks run, execution belongs to
    // the immutable source tab and connection captured by the guardian.
    app.apply_action(Action::NewTab);
    assert_eq!(app.active_query_tab, 1);
    app.apply_action(Action::ConfirmDangerQuery);
    assert_eq!(app.active_query_tab, 0);
    assert!(app.commit_pending.is_none());
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

    // Applying schema DDL is refused before it reaches the database.
    app.error = None;
    let table = app.active().unwrap().schema.tables[0].clone();
    app.apply_action(Action::DropTable(table));
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
        default: None,
        check: None,
        comment: None,
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

#[test]
fn production_import_of_plain_rows_runs_without_guard() {
    let mut app = app_with_users_table(users_columns());
    app.connections[0].production = true;
    let path = temp_csv("prod.csv", "id,email\n1,a@b.c\n");
    app.import_pending = Some(draft_for(&app, &["id", "email"], &path));

    app.apply_action(Action::ConfirmImport);
    assert!(app.danger_pending.is_none());
    assert!(app.import_pending.is_none());
    assert_eq!(app.busy, Busy::Importing);
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

/// The pager rewrites the tab's LIMIT/OFFSET exactly and keeps navigation server-side.
#[test]
fn pager_rewrites_sql_for_navigation_and_custom_window() {
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
        tab.kind = crate::components::QueryTabKind::Table;
        tab.sql = "SELECT * FROM table_0 LIMIT 100;".into();
        tab.edits.source = Some(EditSource {
            schema: None,
            table: "table_0".into(),
            pk_cols: vec!["field_0".into()],
        });
    }

    let go = |app: &mut DbGuiApp, action: Action| {
        app.busy = Busy::Idle; // each page flip leaves a query in flight
        app.apply_action(action);
    };

    go(&mut app, Action::Page(PageNav::Next));
    assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100 OFFSET 100;");
    go(&mut app, Action::Page(PageNav::Prev));
    assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 100;");
    go(
        &mut app,
        Action::SetPageWindow {
            limit: 75_000,
            offset: 1_250,
        },
    );
    assert_eq!(
        app.tab().sql,
        "SELECT * FROM table_0 LIMIT 75000 OFFSET 1250;"
    );
    // The rewrite keeps the tab editable (a fresh pending source is derived).
    assert!(app.tab().edits.pending_source.is_some());
    go(
        &mut app,
        Action::SetPageWindow {
            limit: MAX_FETCH_ROWS as u64 + 1,
            offset: 0,
        },
    );
    assert_eq!(
        app.tab().sql,
        "SELECT * FROM table_0 LIMIT 75000 OFFSET 1250;",
        "the materialization cap must reject an oversized page"
    );
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
    app.show_welcome = false;
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

#[test]
fn streaming_append_filters_only_new_rows_and_preserves_existing_order() {
    let mut app = DbGuiApp::construct();
    let tab = app.tab_mut();
    tab.set_result(fake_result(4, 2));
    tab.filter.conditions = vec![crate::filter::Condition {
        enabled: true,
        column: 0,
        op: crate::filter::FilterOp::Less,
        value: "8".into(),
    }];
    tab.recompute_view();
    assert_eq!(tab.row_order, vec![0, 1, 2, 3]);

    tab.append_result_rows(
        Vec::new(),
        vec![
            vec![Value::Int(6), Value::Int(99)],
            vec![Value::Int(8), Value::Int(100)],
        ],
    );

    assert_eq!(tab.result.as_ref().unwrap().row_count(), 6);
    assert_eq!(tab.row_order, vec![0, 1, 2, 3, 4]);
}

#[test]
fn header_filter_action_targets_the_selected_column() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().set_result(fake_result(3, 3));

    app.apply_action(Action::FilterColumn(2));

    assert!(app.tab().filter.visible);
    assert_eq!(app.tab().filter.conditions.len(), 1);
    assert_eq!(app.tab().filter.conditions[0].column, 2);
}

#[test]
fn toggle_filter_requires_a_result_and_cmd_f_flips_it() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::ToggleFilter);
    assert!(
        !app.tab().filter.visible,
        "no result means there is nothing to filter"
    );

    let (ctx, mut app) = grid_nav_app(3, 3);
    app.apply_action(Action::ToggleFilter);
    assert!(app.tab().filter.visible);
    app.apply_action(Action::ToggleFilter);
    assert!(!app.tab().filter.visible);

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
    );
    assert!(
        app.tab().filter.visible,
        "Cmd/Ctrl+F must open the result filter"
    );
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
    );
    assert!(!app.tab().filter.visible);
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
    app.show_welcome = false;
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

#[test]
fn dragging_a_tab_to_the_workspace_opens_a_split_pane() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tabs[0].sql = "SELECT 'left'".into();
    app.new_tab();
    app.tab_mut().sql = "SELECT 'right'".into();
    app.select_tab(0);

    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
    let run = |app: &mut DbGuiApp, events: Vec<egui::Event>| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ui| app.draw(ui, None),
        )
    };
    let out = run(&mut app, vec![]);
    let query_2 = find_text_pos(&out.shapes, "Query 2").expect("Query 2 chip not painted");
    let start = query_2 + egui::vec2(4.0, 6.0);
    let drop = egui::pos2(820.0, 360.0);
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
    for step in 1..8 {
        let t = step as f32 / 8.0;
        run(
            &mut app,
            vec![egui::Event::PointerMoved(start + (drop - start) * t)],
        );
    }
    run(&mut app, vec![egui::Event::PointerMoved(drop)]);
    run(
        &mut app,
        vec![egui::Event::PointerButton {
            pos: drop,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
    );

    let split = app.split_tab.expect("drop did not create a split pane");
    assert_eq!(app.tabs.len(), 2, "dragged tab should move, not duplicate");
    assert_eq!(app.tabs[split].sql, "SELECT 'right'");
    assert_eq!(app.tabs[app.active_query_tab].sql, "SELECT 'left'");
}

#[test]
fn schema_table_payload_opens_an_editable_table_in_split() {
    let mut app = DbGuiApp::construct();
    connect_fake(&mut app, fake_schema(2, 3));

    app.open_schema_table_in_split(SchemaTableDrag {
        conn_id: "c1".into(),
        schema: None,
        table: "table_1".into(),
        pinned: false,
    });

    let split = app.split_tab.expect("table drop did not create a split");
    assert_eq!(app.tabs[split].kind, crate::components::QueryTabKind::Table);
    assert_eq!(app.tabs[split].conn_id.as_deref(), Some("c1"));
    assert!(app.tabs[split].sql.contains("table_1"));
    let source = app.tabs[split]
        .edits
        .pending_source
        .as_ref()
        .expect("split table should remain editable after loading");
    assert_eq!(source.table, "table_1");
    assert_eq!(source.pk_cols, ["field_0"]);
}

#[test]
fn query_tabs_use_their_database_provider_identity() {
    let mut app = DbGuiApp::construct();
    let mut pg = ConnectionConfig::new(DbKind::Postgres);
    pg.id = "pg".into();
    app.connections.push(pg);
    app.tab_mut().conn_id = Some("pg".into());

    assert_eq!(app.tab_db_kind(0), Some(DbKind::Postgres));
    assert_eq!(app.tab_label(0), "PG Query 1");

    app.new_tab();
    let mut ms = ConnectionConfig::new(DbKind::SqlServer);
    ms.id = "ms".into();
    app.connections.push(ms);
    app.tab_mut().conn_id = Some("ms".into());
    assert_eq!(app.tab_label(1), "MS Query 2");

    app.tab_mut().title = "orders".into();
    assert_eq!(
        app.tab_label(1),
        "orders",
        "named relation tabs must keep their object title"
    );
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

#[test]
fn editor_placement_follows_tab_workflow() {
    use crate::components::QueryTabKind as Kind;

    for kind in [Kind::Query, Kind::Function, Kind::Procedure, Kind::Trigger] {
        assert_eq!(
            query_editor_placement(kind),
            QueryEditorPlacement::Top,
            "{kind:?} should be code-first"
        );
    }
    for kind in [Kind::Table, Kind::View] {
        assert_eq!(
            query_editor_placement(kind),
            QueryEditorPlacement::Bottom,
            "{kind:?} should be data-first"
        );
    }
}

#[test]
fn legacy_workspace_kind_falls_back_from_source() {
    use crate::components::QueryTabKind as Kind;
    use dbcore::config::WorkspaceTabKind as Saved;

    assert_eq!(super::workspace::restored_tab_kind(None, true), Kind::Table);
    assert_eq!(
        super::workspace::restored_tab_kind(None, false),
        Kind::Query
    );
    assert_eq!(
        super::workspace::restored_tab_kind(Some(Saved::View), true),
        Kind::View
    );
}

#[test]
fn workspace_snapshot_keeps_tab_kind_and_editor_size() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().title = "active_users".into();
    app.tab_mut().kind = crate::components::QueryTabKind::View;
    app.tab_mut().editor_size = Some(212.0);

    let saved = app.snapshot_workspace();
    assert_eq!(saved.tabs.len(), 1);
    assert_eq!(saved.tabs[0].title, "active_users");
    assert_eq!(
        saved.tabs[0].kind,
        Some(dbcore::config::WorkspaceTabKind::View)
    );
    assert_eq!(saved.tabs[0].editor_size, Some(212.0));
}

#[test]
fn independent_split_query_uses_the_focused_pane() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().sql = "SELECT 1".into();
    app.tab_mut().editor_split = true;
    app.tab_mut().split_sql = Some("SELECT 2".into());
    app.tab_mut().editor_pane = super::EditorPane::Split;
    assert_eq!(app.resolved_sql_snapshot_for(0).unwrap(), "SELECT 2");
    app.tab_mut().editor_pane = super::EditorPane::Primary;
    assert_eq!(app.resolved_sql_snapshot_for(0).unwrap(), "SELECT 1");
}

#[test]
fn run_current_prefers_selection_then_the_statement_at_the_cursor() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().sql = "SELECT 1;\nSELECT 2;".into();
    app.tab_mut().primary_cursor = 12..12;
    assert_eq!(
        app.resolved_current_sql_snapshot_for(0).unwrap().trim(),
        "SELECT 2"
    );

    app.tab_mut().primary_cursor = 0..8;
    assert_eq!(
        app.resolved_current_sql_snapshot_for(0).unwrap(),
        "SELECT 1"
    );
}

#[test]
fn closing_split_repairs_an_active_hidden_pane_index() {
    let mut app = DbGuiApp::construct();
    app.tabs[0].editor_split = true;
    let split = QueryTab::new(app.next_tab_id, "right".into());
    app.next_tab_id += 1;
    app.tabs.push(split);
    app.split_tab = Some(1);
    // Reproduces the crash: UI rendering temporarily left the hidden pane active when Close
    // removed index 1, leaving active_query_tab == len.
    app.active_query_tab = 1;

    app.close_split_workspace();

    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.active_query_tab, 0);
    assert!(app.split_tab.is_none());
    assert!(!app.tabs[0].editor_split);
    assert_eq!(app.tab().id, 0);
}

#[test]
fn split_panes_keep_independent_editor_assistance_state() {
    let mut app = DbGuiApp::construct();
    app.tabs[0].editor_assist.autocomplete.open = true;
    app.tabs[0].editor_assist.autocomplete.prefix = "cust".into();
    app.tabs[0].editor_assist.ghost_suggestion = Some("omers".into());
    app.tabs[0].editor_assist.syntax_checked = "SELECT left".into();

    app.open_split_workspace();
    let right = app.split_tab.unwrap();

    assert!(app.tabs[0].editor_assist.autocomplete.open);
    assert_eq!(app.tabs[0].editor_assist.autocomplete.prefix, "cust");
    assert_eq!(
        app.tabs[0].editor_assist.ghost_suggestion.as_deref(),
        Some("omers")
    );
    assert_eq!(app.tabs[0].editor_assist.syntax_checked, "SELECT left");
    assert!(!app.tabs[right].editor_assist.autocomplete.open);
    assert!(app.tabs[right].editor_assist.autocomplete.prefix.is_empty());
    assert!(app.tabs[right].editor_assist.ghost_suggestion.is_none());
    assert!(app.tabs[right].editor_assist.syntax_checked.is_empty());
}

#[test]
fn adaptive_editor_renders_on_the_expected_side_of_results() {
    use egui_kittest::kittest::Queryable;

    let build = |kind, result: Option<QueryResult>, size| {
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        app.show_schema_panel = false;
        app.show_details_panel = false;
        app.show_connection_tabs = false;
        app.tab_mut().kind = kind;
        app.tab_mut().sql = "SELECT 1".into();
        if let Some(result) = result {
            app.tab_mut().set_result(result);
        }
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(size)
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
            });
        harness.run_steps(4);
        harness
    };

    let mut query = build(
        crate::components::QueryTabKind::Query,
        None,
        egui::vec2(1000.0, 700.0),
    );
    assert!(
        query.get_by_label("SQL workspace").rect().center().y
            < query.get_by_label("Empty state mark").rect().center().y
    );
    assert!(
        query.query_by_label("SQL line numbers").is_some(),
        "the query editor must expose its line-number gutter"
    );
    assert!(
        query.get_by_label("SQL line numbers").rect().center().y
            < query.get_by_label("Empty state mark").rect().center().y
            && query.get_by_label("Empty state mark").rect().center().y
                < query.get_by_label("Live log").rect().center().y,
        "the live log must dock below the query result, not inside the SQL editor"
    );
    assert!(
        query.get_by_label("SQL line numbers").rect().left()
            < query.get_by_label("SQL workspace").rect().left(),
        "the query editor must reach the panel edge without an outer inset"
    );
    assert!(
        (query.get_by_label("Run Current").rect().center().y
            - query.get_by_label("SQL workspace").rect().center().y)
            .abs()
            < 0.1,
        "query tabs and actions must share one footer row"
    );
    assert!(query.query_by_label("Save query").is_none());
    query.get_by_label("Run options").click();
    query.run_steps(2);
    assert!(query.query_by_label("Run All").is_some());
    assert!(query.query_by_label("Save query").is_some());

    let table = build(
        crate::components::QueryTabKind::Table,
        Some(fake_result(2, 2)),
        egui::vec2(1000.0, 700.0),
    );
    table.get_by_label("col0");
    assert!(
        table.query_by_label("SQL workspace").is_none()
            && table.query_by_label("SQL line numbers").is_none()
            && table.query_by_label("Save query").is_none(),
        "table tabs must reserve SQL authoring for Query tabs"
    );
    assert!(
        table.get_by_label("col0").rect().center().y
            < table.get_by_label("Live log").rect().center().y,
        "table tabs must keep a standalone Live log below the grid"
    );

    let compact = build(
        crate::components::QueryTabKind::Query,
        None,
        egui::vec2(800.0, 500.0),
    );
    let editor_y = compact.get_by_label("SQL workspace").rect().center().y;
    let result_y = compact.get_by_label("Empty state mark").rect().center().y;
    let result_modes_y = compact.get_by_label("Data").rect().center().y;
    let live_log_y = compact.get_by_label("Live log").rect().center().y;
    assert!(editor_y < result_y);
    assert!(
        result_y - editor_y > 50.0,
        "compact result area collapsed: editor={editor_y}, result={result_y}"
    );
    assert!(
        result_y < result_modes_y && result_modes_y < live_log_y,
        "query result modes must dock below the result and above Live log"
    );
}

#[test]
fn live_log_is_session_only_and_independent_of_history_preferences() {
    let mut app = DbGuiApp::construct();
    app.history_enabled = false;

    app.record_history(
        dbcore::audit::AuditAction::Query,
        "sqlite-workspace",
        "SELECT * FROM categories LIMIT 100",
        true,
        None,
        Some(12),
        4.2,
    );

    assert_eq!(app.live_log.len(), 1);
    assert_eq!(app.live_log[0].sql, "SELECT * FROM categories LIMIT 100");
    assert_eq!(app.live_log[0].rows, Some(12));
}

#[test]
fn visible_history_cache_stays_at_the_disk_history_limit() {
    let mut app = DbGuiApp::construct();
    app.sidebar_tab = SidebarTab::History;
    for i in 0..=dbcore::history::MAX_ENTRIES {
        app.record_history(
            dbcore::audit::AuditAction::Query,
            "c1",
            &format!("SELECT {i}"),
            true,
            None,
            Some(1),
            0.1,
        );
    }

    assert_eq!(app.history_cache.len(), dbcore::history::MAX_ENTRIES);
    assert_eq!(app.history_cache.first().unwrap().sql, "SELECT 1");
    assert_eq!(
        app.history_cache.last().unwrap().sql,
        format!("SELECT {}", dbcore::history::MAX_ENTRIES)
    );
}

#[test]
fn live_log_can_expand_beyond_the_old_fixed_height_cap() {
    assert!(super::panels::live_log_max_size(900.0) > 700.0);
    assert_eq!(super::panels::live_log_max_size(100.0), 32.0);
}

#[test]
fn postgres_type_picker_covers_native_and_alias_types() {
    let types = super::panels::db_type_options(dbcore::DbKind::Postgres);
    for expected in [
        "bool",
        "bytea",
        "char",
        "date",
        "float4",
        "float8",
        "int2",
        "int4",
        "int8",
        "interval",
        "json",
        "jsonb",
        "numeric",
        "text",
        "time",
        "timestamp",
        "timestamptz",
        "timetz",
        "uuid",
        "varchar",
        "xml",
    ] {
        assert!(
            types.contains(&expected),
            "missing PostgreSQL type {expected}"
        );
    }
    assert!(
        types.len() >= 50,
        "the PostgreSQL picker regressed to a short preset list"
    );
}

#[test]
fn live_log_can_close_from_its_header_and_reopen_from_the_title_bar() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);

    harness.get_by_label("Close Live log").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Live log").is_none());

    harness.get_by_label("Layout").click();
    harness.run_steps(2);
    harness.get_by_label("Live log panel").click();
    harness.run_steps(2);
    harness.get_by_label("Live log");
}

#[test]
fn table_tab_keeps_data_controls_without_a_query_console() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    connect_fake(&mut app, fake_schema(2, 3));
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().sql = "SELECT * FROM table_0 LIMIT 100".into();
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_0".into(),
        pk_cols: vec!["field_0".into()],
    });
    app.tab_mut().set_result(fake_result(2, 3));
    app.tab_mut().page_exhausted = true;
    app.tab_mut().total_rows = Some(12_534);

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);

    let grid_y = harness.get_by_label("col0").rect().center().y;
    let modes = harness.get_by_label("Data");
    let modes_y = modes.rect().center().y;
    let log_y = harness.get_by_label("Live log").rect().center().y;
    let log_dock = harness.get_by_label("Live log dock");
    assert!(
        grid_y < modes_y && modes_y < log_y,
        "the table layout must be Grid, Data / Structure / Indexes, then Live log"
    );
    assert!(
        log_dock.rect().top() <= modes.rect().top(),
        "the Live log resize boundary must sit above the table mode bar"
    );
    assert!(
        harness.query_by_label("SQL workspace").is_none()
            && harness.query_by_label("SQL line numbers").is_none()
            && harness.query_by_label("Run").is_none(),
        "table tabs must not render query-console controls"
    );
    for label in ["1–2 of 12,534 rows", "Previous page", "Next page"] {
        harness.get_by_label(label);
    }
    harness.get_by_label("Structure").click();
    harness.run_steps(4);
    for label in ["1–2 of 12,534 rows", "Previous page", "Next page"] {
        harness.get_by_label(label);
    }
    harness.get_by_label("Indexes").click();
    harness.run_steps(4);
    for label in ["1–2 of 12,534 rows", "Previous page", "Next page"] {
        harness.get_by_label(label);
    }
}

/// Regression: an open object designer owns the whole tab — the SQL console and the
/// Data/Message/Chart switch must not render around it. Existing tables edit their schema
/// directly through the persistent Data/Structure/Indexes bar.
#[test]
fn open_designer_owns_the_tab() {
    use egui_kittest::kittest::Queryable;

    let build = |kind: crate::components::QueryTabKind| {
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        app.show_schema_panel = false;
        app.show_details_panel = false;
        app.show_connection_tabs = false;
        connect_fake(&mut app, fake_schema(2, 3));
        app.tab_mut().kind = kind;
        if kind == crate::components::QueryTabKind::Table {
            app.tab_mut().edits.source = Some(EditSource {
                schema: None,
                table: "table_0".into(),
                pk_cols: vec!["field_0".into()],
            });
            let info = app.structure_table(0).cloned().expect("table resolves");
            app.apply_action(Action::OpenEditTable(info));
            // Reproduce a persisted/one-frame-stale Data selection. Existing tables must still
            // use the Structure grid and never fall back to the retired form editor.
            app.tab_mut().view = TabView::Data;
        } else {
            app.apply_action(Action::OpenNewTable);
        }
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1000.0, 700.0))
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
            });
        harness.run_steps(4);
        harness
    };

    let query = build(crate::components::QueryTabKind::Query);
    query.get_by_label("Create Table");
    assert!(
        query.query_by_label("Save query").is_none(),
        "the query workspace bar must hide while designing"
    );
    assert!(
        query.query_by_label("SQL line numbers").is_none(),
        "the SQL editor must hide while designing"
    );
    assert!(
        query.query_by_label("Message").is_none(),
        "the result-mode switch is meaningless while designing"
    );

    let mut table = build(crate::components::QueryTabKind::Table);
    table.get_by_label("Structure");
    table.get_by_label("Indexes");
    table.get_by_label("Column");
    for header in [
        "column_name",
        "data_type",
        "is_nullable",
        "check",
        "column_default",
        "foreign_key",
        "comment",
    ] {
        table.get_by_label(header);
    }
    assert!(table.query_by_label("Foreign Keys").is_none());
    assert!(table.query_by_label("Columns").is_none());
    assert!(table.query_by_label("Table name:").is_none());
    assert!(table.query_by_label("Add Column").is_none());
    assert!(table.query_by_label("Edit Table").is_none());
    assert!(table.query_by_label("Preview SQL").is_none());
    assert!(table.query_by_label("Discard").is_none());
    assert!(
        table.query_by_label("SQL line numbers").is_none(),
        "the SQL editor must hide while designing a table"
    );
    table.get_by_label("Indexes").click();
    table.run_steps(4);
    table.get_by_label("Index");
    table.get_by_label("Live log");
    assert!(
        table.query_by_label("Column").is_none(),
        "Indexes must be a separate surface from Structure columns"
    );
}

#[test]
fn query_result_controls_sit_between_query_toolbar_and_grid() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    connect_fake(&mut app, fake_schema(2, 3));
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().sql = "SELECT * FROM table_1 LIMIT 100".into();
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_1".into(),
        pk_cols: vec!["field_0".into()],
    });
    app.tab_mut().set_result(fake_result(2, 3));

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);

    let query_y = harness.get_by_label("Run Current").rect().center().y;
    let data_y = harness.get_by_label("Data").rect().center().y;
    let grid_y = harness.get_by_label("col0").rect().center().y;
    assert!(harness.query_by_label("Message").is_some());
    assert!(harness.query_by_label("Chart").is_some());
    assert!(harness.query_by_label("Structure").is_none());
    assert!(harness.query_by_label("Edit Table").is_none());
    assert!(
        harness.query_by_label("100 / page").is_none(),
        "Query tabs must not show table-browser paging controls"
    );
    assert!(
        query_y < grid_y && grid_y < data_y,
        "Query toolbar, grid, and result controls must form one continuous top-to-bottom stack"
    );

    harness.get_by_label("Message").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Query message").is_none());
    assert!(harness
        .query_by_label("2 row(s) × 3 col(s) in 0.0 ms")
        .is_some());

    harness.get_by_label("Chart").click();
    harness.run_steps(2);
    assert!(harness
        .query_by_label("Chart visualization is coming soon")
        .is_some());
}

#[test]
fn run_all_result_tabs_render_between_the_toolbar_and_result_modes() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    connect_fake(&mut app, fake_schema(2, 3));
    app.tab_mut().sql = "SELECT 1; SELECT 2;".into();
    app.tab_mut().set_batch_results(vec![
        ("SELECT 1".into(), Ok(fake_result(1, 1))),
        ("SELECT 2".into(), Ok(fake_result(2, 2))),
    ]);

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);

    let toolbar_y = harness.get_by_label("Run Current").rect().center().y;
    let query_1_y = harness.get_by_label("Query 1").rect().center().y;
    let modes_y = harness.get_by_label("Data").rect().center().y;
    harness.get_by_label("Query 2");
    assert!(
        toolbar_y < query_1_y && query_1_y < modes_y,
        "statement tabs must sit between the query toolbar and Data / Message / Chart"
    );
}

#[test]
fn untouched_message_and_chart_views_show_only_the_empty_illustration() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);

    harness.get_by_label("Message").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Empty state mark").is_some());
    assert!(harness
        .query_by_label("Run a query to see execution details")
        .is_none());

    harness.get_by_label("Chart").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Empty state mark").is_some());
    assert!(harness
        .query_by_label("Chart visualization is coming soon")
        .is_none());
}

/// A result arriving from a superseded run (the user started a newer query before the old
/// one finished) must be dropped whole: whichever run finished last used to win, showing
/// stale rows, clearing the newer run's busy flag, or surfacing an outdated error.
#[test]
fn superseded_query_result_never_touches_ui_state() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    // A newer run is in flight: its stamp (1) is ahead of the late result below (0).
    app.query_seq = 1;
    app.busy = Busy::Querying;
    app.querying_tab_id = Some(tab_id);
    app.tx
        .send(AppMessage::Queried {
            tab_id,
            conn_id: String::new(),
            sql: "SELECT 1".into(),
            result: Err("stale failure".into()),
            canceled: false,
            seq: 0,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert_eq!(
        app.busy,
        Busy::Querying,
        "a stale result must not clear the newer run's busy state"
    );
    assert_eq!(app.querying_tab_id, Some(tab_id));
    assert!(
        app.tab().query_error.is_none(),
        "a stale failure must not surface on the tab"
    );
}

#[test]
fn exact_table_total_is_routed_only_to_the_matching_query() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.query_seq = 3;
    app.tab_mut().total_rows_pending = true;

    app.tx
        .send(AppMessage::QueryTotal {
            tab_id,
            total: Some(99),
            seq: 2,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert_eq!(app.tab().total_rows, None, "a stale count must be ignored");
    assert!(app.tab().total_rows_pending);

    app.tx
        .send(AppMessage::QueryTotal {
            tab_id,
            total: Some(12_534),
            seq: 3,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert_eq!(app.tab().total_rows, Some(12_534));
    assert!(!app.tab().total_rows_pending);
}

#[test]
fn duckdb_counts_the_full_table_after_loading_the_visible_page() {
    let mut app = DbGuiApp::construct();
    let config = dbcore::ConnectionConfig::new(dbcore::DbKind::DuckDb);
    let db = std::sync::Arc::new(dbcore::backends::duckdb::DuckDb::connect(&config).unwrap());
    app.rt
        .block_on(db.execute_capped(
            "CREATE TABLE items AS SELECT range AS id FROM range(250);",
            1,
        ))
        .unwrap();
    app.active_connections.push(ActiveConnection {
        config_id: "duck".into(),
        name: "DuckDB".into(),
        db,
        schema: SchemaTree::default(),
        databases: Vec::new(),
    });
    {
        let tab = app.tab_mut();
        tab.conn_id = Some("duck".into());
        tab.kind = crate::components::QueryTabKind::Table;
        tab.sql = "SELECT * FROM items LIMIT 100;".into();
        tab.edits.source = Some(EditSource {
            schema: None,
            table: "items".into(),
            pk_cols: Vec::new(),
        });
    }

    app.start_query_for(0);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && (app.busy != Busy::Idle || app.tab().total_rows_pending)
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.poll_messages(&ctx);
    }

    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 100);
    assert_eq!(app.tab().total_rows, Some(250));
    assert!(!app.tab().total_rows_pending);
}

#[test]
fn run_all_keeps_each_statement_result_in_its_own_result_tab() {
    let mut app = DbGuiApp::construct();
    let config = dbcore::ConnectionConfig::new(dbcore::DbKind::DuckDb);
    let db = std::sync::Arc::new(dbcore::backends::duckdb::DuckDb::connect(&config).unwrap());
    app.active_connections.push(ActiveConnection {
        config_id: "duck-batch".into(),
        name: "DuckDB".into(),
        db,
        schema: SchemaTree::default(),
        databases: Vec::new(),
    });
    app.tab_mut().conn_id = Some("duck-batch".into());

    app.start_resolved_query_batch(
        0,
        "SELECT 11 AS first_value; SELECT 22 AS second_value;".into(),
    );
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && app.busy != Busy::Idle {
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.poll_messages(&ctx);
    }

    assert_eq!(app.tab().batch_results.len(), 2);
    assert_eq!(
        app.tab().result.as_ref().unwrap().rows[0][0],
        Value::Int(11)
    );
    app.tab_mut().activate_batch_result(1);
    assert_eq!(
        app.tab().result.as_ref().unwrap().rows[0][0],
        Value::Int(22)
    );
    app.tab_mut().activate_batch_result(0);
    assert_eq!(
        app.tab().result.as_ref().unwrap().rows[0][0],
        Value::Int(11)
    );
}

#[test]
fn duckdb_filter_searches_rows_beyond_the_loaded_page() {
    let mut app = DbGuiApp::construct();
    let config = dbcore::ConnectionConfig::new(dbcore::DbKind::DuckDb);
    let db = std::sync::Arc::new(dbcore::backends::duckdb::DuckDb::connect(&config).unwrap());
    app.rt
        .block_on(db.execute_capped(
            "CREATE TABLE trades AS SELECT range AS id, \
             CASE WHEN range = 249 THEN 'needle' ELSE 'other' END AS symbol FROM range(250);",
            1,
        ))
        .unwrap();
    app.active_connections.push(ActiveConnection {
        config_id: "duck-filter".into(),
        name: "DuckDB".into(),
        db,
        schema: SchemaTree::default(),
        databases: Vec::new(),
    });
    {
        let tab = app.tab_mut();
        tab.conn_id = Some("duck-filter".into());
        tab.kind = crate::components::QueryTabKind::Table;
        tab.sql = "SELECT * FROM trades LIMIT 100;".into();
        tab.edits.source = Some(EditSource {
            schema: None,
            table: "trades".into(),
            pk_cols: Vec::new(),
        });
        tab.set_result(QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    type_name: "BIGINT".into(),
                },
                ColumnMeta {
                    name: "symbol".into(),
                    type_name: "VARCHAR".into(),
                },
            ],
            ..QueryResult::default()
        });
        tab.filter.conditions[0].column = 1;
        tab.filter.conditions[0].op = crate::filter::FilterOp::Equals;
        tab.filter.conditions[0].value = "needle".into();
    }

    app.apply_result_filter(0, false);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && (app.busy != Busy::Idle || app.tab().total_rows_pending)
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.poll_messages(&ctx);
    }

    let result = app.tab().result.as_ref().unwrap();
    assert_eq!(result.row_count(), 1);
    assert_eq!(result.rows[0][1], Value::Text("needle".into()));
    assert_eq!(app.tab().total_rows, Some(1));
    assert!(app.tab().server_filter_predicate.is_some());

    app.apply_result_filter(0, true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && (app.busy != Busy::Idle || app.tab().total_rows_pending)
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.poll_messages(&ctx);
    }
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 100);
    assert_eq!(app.tab().total_rows, Some(250));
    assert!(app.tab().server_filter_predicate.is_none());

    {
        let tab = app.tab_mut();
        tab.filter.conditions[0].column = 1;
        tab.filter.conditions[0].op = crate::filter::FilterOp::NotEquals;
        tab.filter.conditions[0].value = "needle".into();
    }
    app.apply_result_filter(0, false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && (app.busy != Busy::Idle || app.tab().total_rows_pending)
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.poll_messages(&ctx);
    }
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 100);
    assert_eq!(app.tab().total_rows, Some(249));

    app.page_nav(PageNav::Next);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && app.busy != Busy::Idle {
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.poll_messages(&ctx);
    }
    assert_eq!(
        dbcore::parse_page_window(&app.tab().sql).unwrap().offset,
        100
    );
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 100);
    assert_eq!(app.tab().total_rows, Some(249));
    assert!(app
        .tab()
        .result
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .all(|row| matches!(row.get(1), Some(Value::Text(value)) if value == "other")));
}

/// Replacement chunks stay off-screen until the terminal message installs the complete page.
#[test]
fn replacement_stream_stays_hidden_until_finished() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.query_seq = 7;
    app.busy = Busy::Querying;
    app.querying_tab_id = Some(tab_id);
    app.tx
        .send(AppMessage::QueryStreamStarted {
            tab_id,
            columns: vec![ColumnMeta {
                name: "id".into(),
                type_name: "INTEGER".into(),
            }],
            append: false,
            seq: 7,
        })
        .unwrap();
    for id in 1..=3 {
        app.tx
            .send(AppMessage::QueryRows {
                tab_id,
                rows: vec![vec![Value::Int(id)]],
                seq: 7,
            })
            .unwrap();
    }
    app.poll_messages(&egui::Context::default());
    assert!(app.tab().result.is_none());
    assert_eq!(app.busy, Busy::Querying);

    app.tx
        .send(AppMessage::QueryStreamFinished {
            tab_id,
            conn_id: String::new(),
            sql: "SELECT * FROM items LIMIT 3 OFFSET 0".into(),
            elapsed_ms: 12.5,
            rows_loaded: 3,
            page: dbcore::PageWindow {
                limit: Some(3),
                offset: 0,
            },
            result_limit: 3,
            append: false,
            result: Ok(3),
            canceled: false,
            budget_truncated: false,
            row_truncated: false,
            seq: 7,
        })
        .unwrap();

    app.poll_messages(&egui::Context::default());
    let result = app.tab().result.as_ref().unwrap();
    assert_eq!(result.row_count(), 3);
    assert_eq!(result.stats.elapsed_ms, 12.5);
    assert_eq!(app.busy, Busy::Idle);
    assert_eq!(app.querying_tab_id, None);
}

#[test]
fn memory_limited_stream_is_marked_truncated_and_cannot_auto_continue() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.query_seq = 9;
    app.tab_mut().stream = Some(QueryStreamUi {
        seq: 9,
        append: false,
        columns: vec![ColumnMeta {
            name: "payload".into(),
            type_name: "TEXT".into(),
        }],
        pending_rows: vec![vec![Value::Text("bounded".into())]],
        received_rows: 1,
    });
    app.tx
        .send(AppMessage::QueryStreamFinished {
            tab_id,
            conn_id: String::new(),
            sql: "SELECT * FROM events LIMIT 1000".into(),
            elapsed_ms: 1.0,
            rows_loaded: 1,
            page: dbcore::PageWindow {
                limit: Some(512),
                offset: 0,
            },
            result_limit: 1000,
            append: false,
            result: Ok(1),
            canceled: false,
            budget_truncated: true,
            row_truncated: false,
            seq: 9,
        })
        .unwrap();

    app.poll_messages(&egui::Context::default());

    assert!(app.tab().result.as_ref().unwrap().truncated);
    assert!(app.tab().page_exhausted);
}

#[test]
fn canceled_replacement_keeps_the_previous_result() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.tab_mut().set_result(QueryResult {
        columns: vec![ColumnMeta {
            name: "id".into(),
            type_name: "INTEGER".into(),
        }],
        rows: vec![vec![Value::Int(7)]],
        ..QueryResult::default()
    });
    app.query_seq = 2;
    app.busy = Busy::Querying;
    app.querying_tab_id = Some(tab_id);
    app.tx
        .send(AppMessage::QueryStreamStarted {
            tab_id,
            columns: vec![ColumnMeta {
                name: "id".into(),
                type_name: "INTEGER".into(),
            }],
            append: false,
            seq: 2,
        })
        .unwrap();
    app.tx
        .send(AppMessage::QueryRows {
            tab_id,
            rows: vec![vec![Value::Int(1)]],
            seq: 2,
        })
        .unwrap();
    app.tx
        .send(AppMessage::QueryStreamFinished {
            tab_id,
            conn_id: String::new(),
            sql: "SELECT * FROM items LIMIT 1000 OFFSET 0".into(),
            elapsed_ms: 4.0,
            rows_loaded: 1,
            page: dbcore::PageWindow {
                limit: Some(1000),
                offset: 0,
            },
            result_limit: 1000,
            append: false,
            result: Err("query cancelled".into()),
            canceled: true,
            budget_truncated: false,
            row_truncated: false,
            seq: 2,
        })
        .unwrap();

    app.poll_messages(&egui::Context::default());
    assert_eq!(
        app.tab().result.as_ref().unwrap().rows,
        vec![vec![Value::Int(7)]]
    );
    assert_eq!(app.status_msg, "Query cancelled");
    assert!(app.tab().query_error.is_none());
    assert_eq!(app.busy, Busy::Idle);
}

#[test]
fn replacement_stream_keeps_previous_rows_until_completion() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.tab_mut().set_result(QueryResult {
        columns: vec![ColumnMeta {
            name: "id".into(),
            type_name: "INTEGER".into(),
        }],
        rows: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        ..QueryResult::default()
    });
    app.query_seq = 4;
    app.busy = Busy::Querying;
    app.querying_tab_id = Some(tab_id);
    app.tx
        .send(AppMessage::QueryStreamStarted {
            tab_id,
            columns: vec![ColumnMeta {
                name: "id".into(),
                type_name: "INTEGER".into(),
            }],
            append: false,
            seq: 4,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert_eq!(
        app.tab().result.as_ref().unwrap().row_count(),
        2,
        "starting a replacement must not flash an empty table"
    );

    app.tx
        .send(AppMessage::QueryRows {
            tab_id,
            rows: vec![vec![Value::Int(9)]],
            seq: 4,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert_eq!(
        app.tab().result.as_ref().unwrap().rows,
        vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        "partial replacement batches must remain hidden"
    );
    app.tx
        .send(AppMessage::QueryStreamFinished {
            tab_id,
            conn_id: String::new(),
            sql: "SELECT * FROM items LIMIT 1".into(),
            elapsed_ms: 1.0,
            rows_loaded: 1,
            page: dbcore::PageWindow {
                limit: Some(1),
                offset: 0,
            },
            result_limit: 1,
            append: false,
            result: Ok(1),
            canceled: false,
            budget_truncated: false,
            row_truncated: false,
            seq: 4,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert_eq!(
        app.tab().result.as_ref().unwrap().rows,
        vec![vec![Value::Int(9)]],
        "the completed page replaces the grid once"
    );
}

#[test]
fn load_more_appends_without_rewriting_visible_sql() {
    let mut app = DbGuiApp::construct();
    app.active_connections.push(ActiveConnection {
        config_id: "c1".into(),
        name: "test".into(),
        db: Arc::new(DummyDb),
        schema: fake_schema(1, 1),
        databases: Vec::new(),
    });
    {
        let tab = app.tab_mut();
        tab.conn_id = Some("c1".into());
        tab.kind = crate::components::QueryTabKind::Table;
        tab.sql = "SELECT * FROM table_0 LIMIT 1000;".into();
        tab.edits.source = Some(EditSource {
            schema: None,
            table: "table_0".into(),
            pk_cols: vec!["field_0".into()],
        });
        tab.set_result(fake_result(100, 1));
    }

    app.status_msg = "100 rows".into();
    app.load_more_rows();
    assert_eq!(app.tab().sql, "SELECT * FROM table_0 LIMIT 1000;");
    assert!(app
        .tab()
        .stream
        .as_ref()
        .is_some_and(|stream| stream.append));
    assert_eq!(app.busy, Busy::Querying);
    assert_eq!(app.status_msg, "100 rows");
}

#[test]
fn load_more_never_exceeds_the_user_limit() {
    let mut app = DbGuiApp::construct();
    {
        let tab = app.tab_mut();
        tab.kind = crate::components::QueryTabKind::Table;
        tab.sql = "SELECT * FROM table_0 LIMIT 100;".into();
        tab.edits.source = Some(EditSource {
            schema: None,
            table: "table_0".into(),
            pk_cols: vec!["field_0".into()],
        });
        tab.set_result(fake_result(100, 1));
    }

    app.load_more_rows();
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 100);
    assert!(app.tab().page_exhausted);
    assert!(app.tab().stream.is_none());
    assert_eq!(app.busy, Busy::Idle);
}

#[test]
fn load_more_never_exceeds_the_global_materialization_cap() {
    let mut app = DbGuiApp::construct();
    {
        let tab = app.tab_mut();
        tab.kind = crate::components::QueryTabKind::Table;
        tab.sql = format!(
            "SELECT * FROM table_0 LIMIT {};",
            MAX_FETCH_ROWS as u64 * 10
        );
        tab.edits.source = Some(EditSource {
            schema: None,
            table: "table_0".into(),
            pk_cols: vec!["field_0".into()],
        });
        tab.set_result(fake_result(MAX_FETCH_ROWS, 1));
    }

    app.load_more_rows();
    assert_eq!(
        app.tab().result.as_ref().unwrap().row_count(),
        MAX_FETCH_ROWS
    );
    assert!(app.tab().page_exhausted);
    assert!(app.tab().result.as_ref().unwrap().truncated);
    assert_eq!(app.busy, Busy::Idle);
}

#[test]
fn continuation_stream_appends_and_marks_a_short_page_exhausted() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.tab_mut().set_result(QueryResult {
        columns: vec![ColumnMeta {
            name: "id".into(),
            type_name: "INTEGER".into(),
        }],
        rows: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        ..QueryResult::default()
    });
    app.tab_mut().selection.select_one(0);
    app.query_seq = 8;
    app.busy = Busy::Querying;
    app.querying_tab_id = Some(tab_id);
    app.tx
        .send(AppMessage::QueryStreamStarted {
            tab_id,
            columns: vec![ColumnMeta {
                name: "id".into(),
                type_name: "INTEGER".into(),
            }],
            append: true,
            seq: 8,
        })
        .unwrap();
    app.tx
        .send(AppMessage::QueryRows {
            tab_id,
            rows: vec![vec![Value::Int(3)]],
            seq: 8,
        })
        .unwrap();
    app.tx
        .send(AppMessage::QueryStreamFinished {
            tab_id,
            conn_id: String::new(),
            sql: "SELECT * FROM items LIMIT 2 OFFSET 2".into(),
            elapsed_ms: 2.0,
            rows_loaded: 1,
            page: dbcore::PageWindow {
                limit: Some(2),
                offset: 2,
            },
            result_limit: 3,
            append: true,
            result: Ok(1),
            canceled: false,
            budget_truncated: false,
            row_truncated: false,
            seq: 8,
        })
        .unwrap();

    app.poll_messages(&egui::Context::default());
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 3);
    assert!(app.tab().selection.contains(0));
    assert!(app.tab().page_exhausted);
    assert_eq!(app.busy, Busy::Idle);
}

/// Cmd+Enter / Cmd+R land in `RunQuery` unconditionally; while a query is in flight they must
/// refuse silently instead of racing a second run or exposing background prefetch state.
#[test]
fn run_query_is_refused_while_busy() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().sql = "SELECT 1".into();
    app.busy = Busy::Querying;
    app.status_msg = "512 rows".into();
    app.apply_action(Action::RunQuery);
    assert_eq!(
        app.query_seq, 0,
        "no new run may start while one is in flight"
    );
    assert_eq!(app.status_msg, "512 rows");
}

#[test]
fn query_failure_is_kept_on_its_tab_and_rendered_in_message_view() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().sql = "SELECT missing_column FROM customers".into();
    app.tab_mut().view = TabView::Chart;
    let tab_id = app.tab().id;
    app.tx
        .send(AppMessage::Queried {
            tab_id,
            conn_id: String::new(),
            sql: app.tab().sql.clone(),
            result: Err("no such column: missing_column".into()),
            canceled: false,
            seq: app.query_seq,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());

    let rendered_error = app.tab().query_error.clone().unwrap();
    assert!(rendered_error.contains("Line 1, column 8"));
    assert!(rendered_error.contains("Column \"missing_column\" was not found."));
    assert!(rendered_error.contains("SELECT missing_column FROM customers"));
    assert!(app.tab().view == TabView::Message);
    assert_eq!(app.status_msg, "Ready");
    assert!(
        app.error.is_none(),
        "query errors must not be duplicated in the global status bar"
    );

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);
    assert!(harness.query_by_label("Empty state mark").is_none());
    assert!(harness.query_by_label("Query failed").is_none());
    assert!(harness.query_by_label(&rendered_error).is_some());

    harness.get_by_label("Data").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Empty state mark").is_some());
    assert!(harness.query_by_label(&rendered_error).is_none());
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

/// Closing the only tab keeps one clean query tab so the workspace is never empty.
#[test]
fn closing_last_tab_keeps_one_clean_tab() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().sql = "SELECT 99;".into();
    app.close_tab(0);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.active_query_tab, 0);
    assert_eq!(app.tab().sql, "");
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

/// Render direct Structure editing headlessly and ensure selecting the mode installs the
/// existing table editor without an intermediate read-only surface or ID clashes.
#[test]
fn probe_structure_view_id_clash() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
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
        tab.kind = crate::components::QueryTabKind::Table;
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
    assert!(matches!(
        app.tab().schema_editor.as_ref(),
        Some(ObjectEditor::Table(editor))
            if editor.active_tab == crate::schema::SchemaTab::Columns
    ));
    clashes.sort();
    clashes.dedup();
    assert!(
        clashes.is_empty(),
        "ID clashes detected in structure view:\n{}",
        clashes.join("\n")
    );
}

#[test]
fn structure_rows_use_data_grid_delete_keys() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    let schema = fake_schema(1, 3);
    let table = schema.tables[0].clone();
    let db: std::sync::Arc<dyn dbcore::Database> = std::sync::Arc::new(DummyDb);
    app.connections.clear();
    let mut cfg = dbcore::ConnectionConfig::new(dbcore::DbKind::Sqlite);
    cfg.id = "c1".into();
    cfg.production = true;
    app.connections.push(cfg);
    app.active_connections.push(ActiveConnection {
        config_id: "c1".into(),
        name: "one".into(),
        db,
        databases: Vec::new(),
        schema,
    });
    app.tab_mut().conn_id = Some("c1".into());
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: table.name.clone(),
        pk_cols: vec!["field_0".into()],
    });
    app.apply_action(Action::OpenEditTable(table));
    let editor = match app.tab_mut().schema_editor.as_mut() {
        Some(ObjectEditor::Table(editor)) => editor,
        _ => panic!("table editor should be open"),
    };
    editor.grid_selection = Some(crate::schema::SchemaGridSelection {
        tab: crate::schema::SchemaTab::Columns,
        row: 1,
    });

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Delete, egui::Modifiers::NONE)],
    );
    let dropped = match app.tab().schema_editor.as_ref() {
        Some(ObjectEditor::Table(editor)) => editor.columns[1].drop,
        _ => false,
    };
    assert!(dropped, "Delete marks the selected existing column");

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Delete, egui::Modifiers::NONE)],
    );
    let restored = match app.tab().schema_editor.as_ref() {
        Some(ObjectEditor::Table(editor)) => !editor.columns[1].drop,
        _ => false,
    };
    assert!(restored, "pressing Delete again restores the marked column");

    if let Some(ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.columns[1].name = "renamed_field".into();
    }
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::S, egui::Modifiers::COMMAND)],
    );
    assert!(
        app.danger_pending.is_some(),
        "Cmd/Ctrl+S reviews schema changes in Production Guardian"
    );
    app.apply_action(Action::CancelDangerQuery);
    assert!(app.tab().schema_editor.is_some());

    if let Some(ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.columns[1].name = "discard_me".into();
    }
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    let reset_name = match app.tab().schema_editor.as_ref() {
        Some(ObjectEditor::Table(editor)) => editor.columns[1].name.as_str(),
        _ => "",
    };
    assert_eq!(reset_name, "field_1");
    assert!(app.tab().view == TabView::Structure);
}

/// Render the create-table editor across its local tabs, catching panics and ID clashes.
/// Existing table tabs promote Structure and Indexes to their persistent result bar instead.
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
    app.apply_action(Action::OpenNewTable);
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

/// The schema explorer renders its single pinned-first table list without id clashes.
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
    // Pin one table so it sorts to the top, and make it the active tab's table so the
    // selection pill draws too.
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
        for label in ["Views", "Triggers"] {
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
fn snapshot_welcome_page() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = true;
    render_and_snapshot(app, "welcome_page", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_connection_provider_picker() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.apply_action(Action::NewConnection);
    render_and_snapshot(app, "connection_provider_picker", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_connection_details() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.apply_action(Action::NewConnection);
    app.editor.as_mut().unwrap().selecting_provider = false;
    render_and_snapshot(app, "connection_details", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_connection_details_advanced() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.apply_action(Action::NewConnection);
    let editor = app.editor.as_mut().unwrap();
    editor.selecting_provider = false;
    editor.show_advanced = true;
    render_and_snapshot(app, "connection_details_advanced", false);
}

#[test]
fn settings_is_a_transient_utility_tab() {
    let mut app = DbGuiApp::construct();
    let tab_count = app.tabs.len();

    app.apply_action(Action::OpenSettings);
    assert!(app.settings_open);
    assert_eq!(app.tabs.len(), tab_count);

    app.apply_action(Action::NewTab);
    assert!(!app.settings_open);
    assert_eq!(app.tabs.len(), tab_count + 1);

    app.apply_action(Action::OpenSettings);
    app.apply_action(Action::SelectTab(0));
    assert!(!app.settings_open);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_settings_page() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.settings_open = true;
    render_and_snapshot(app, "settings_page", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_settings_narrow() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.show_welcome = false;
    app.settings_open = true;

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(820.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(10);
    harness.snapshot("settings_narrow");
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_settings_appearance_page() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.settings_open = true;
    app.settings_section = SettingsSection::Appearance;
    render_and_snapshot(app, "settings_appearance_page", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_settings_appearance_typography() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.show_welcome = false;
    app.settings_open = true;
    app.settings_section = SettingsSection::Appearance;

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1180.0, 1080.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::install_fonts(
                    ui.ctx(),
                    &crate::AppFonts {
                        ui_regular: include_bytes!("../../../app/assets/Inter-Regular.ttf"),
                        ui_semibold: include_bytes!("../../../app/assets/Inter-SemiBold.ttf"),
                        thai_regular: include_bytes!("../../../app/assets/Anuphan-Regular.ttf"),
                        thai_semibold: include_bytes!("../../../app/assets/Anuphan-SemiBold.ttf"),
                        universal_regular: include_bytes!(
                            "../../../app/assets/Unifont-Regular.otf"
                        ),
                    },
                );
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(10);
    harness.snapshot("settings_appearance_typography");
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_settings_privacy_page() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.settings_open = true;
    app.settings_section = SettingsSection::Privacy;
    render_and_snapshot(app, "settings_privacy_page", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_object_browser() {
    let (mut app, dir) = demo_app_with_objects();
    app.show_welcome = false;
    render_and_snapshot(app, "object_browser", true);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Adaptive-layout references: code-first tabs place the editor above an inviting result
/// state, while data-first tabs keep the grid dominant and the editable SQL below it.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_adaptive_query_layout() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().sql = "SELECT id, email\nFROM customers\nWHERE active = true;".into();
    render_and_snapshot(app, "adaptive_query_layout", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_adaptive_table_layout() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().title = "customers".into();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().sql = "SELECT * FROM customers LIMIT 100;".into();
    app.tab_mut().set_result(fake_result(24, 6));
    render_and_snapshot(app, "adaptive_table_layout", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_saved_queries_tab() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().sql = "SELECT * FROM customers WHERE active = true;".into();
    app.sidebar_tab = SidebarTab::Queries;
    for (name, sql) in [
        (
            "Active customers",
            "SELECT * FROM customers WHERE active = true",
        ),
        (
            "Monthly revenue",
            "SELECT month, SUM(total) FROM orders GROUP BY month",
        ),
        // Saved without a title: the SQL doubles as the name and must render once.
        (
            "SELECT * FROM \"backend\".\"Document\" LIMIT 10",
            "SELECT * FROM \"backend\".\"Document\" LIMIT 10",
        ),
    ] {
        app.favorites_cache.push(dbcore::Favorite {
            id: name.into(),
            name: name.into(),
            sql: sql.into(),
            conn_id: None,
            conn_name: None,
            folder: None,
            created_at: "2026-07-16T00:00:00Z".into(),
        });
    }
    render_and_snapshot(app, "saved_queries_tab", false);
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_query_error_state() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().sql = "SELECT customer_nam FROM customers;".into();
    app.tab_mut().query_error =
        Some("SQLite error: no such column: customer_nam\nat line 1, column 8".into());
    app.tab_mut().view = TabView::Message;
    app.status_msg = "Ready".into();
    render_and_snapshot(app, "query_error_state", false);
}

/// Screenshot generator (ignored): the live syntax check — a red squiggle under the token
/// the parser tripped on, before the query is ever run.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_sql_syntax_error() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().sql =
        "SELECT id, email\nFROM customers\nWHERE created_at > '2026-01-01'\nORDR BY created_at DESC"
            .into();
    render_and_snapshot(app, "sql_syntax_error", false);
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
/// the type-aware widgets (type labels, boolean checkbox, date picker) all render.
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
        ("image", "BLOB"),
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
                Value::Bytes(include_bytes!("../../assets/illus/empty-chameleon.png").to_vec()),
            ],
            // A NULL-heavy row exercises the NULL fallbacks of every kind.
            vec![Value::Null; 8],
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

#[test]
fn staged_image_blob_builds_a_saveable_update() {
    let mut app = DbGuiApp::construct();
    app.active_connections.push(ActiveConnection {
        config_id: "c1".into(),
        name: "local".into(),
        db: std::sync::Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    app.tab_mut().conn_id = Some("c1".into());
    app.tab_mut().set_result(QueryResult {
        columns: vec![
            ColumnMeta {
                name: "id".into(),
                type_name: "INTEGER".into(),
            },
            ColumnMeta {
                name: "image".into(),
                type_name: "BLOB".into(),
            },
        ],
        rows: vec![vec![Value::Int(1), Value::Bytes(vec![0])]],
        ..QueryResult::default()
    });
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "images".into(),
        pk_cols: vec!["id".into()],
    });
    app.tab_mut().edits.stage(
        0,
        1,
        Value::Bytes(vec![0x89, 0x50, 0x4e, 0x47]),
        &Value::Bytes(vec![0]),
    );

    let statements = app.build_commit_statements().expect("saveable BLOB update");
    assert_eq!(
        statements,
        ["UPDATE \"images\" SET \"image\" = X'89504E47' WHERE \"id\" = 1;"]
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
    app.show_welcome = false;
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
    // `construct` reads the machine's settings.json; on a box that hasn't been "welcomed"
    // the welcome page would replace the grid and every navigation assertion below.
    app.show_welcome = false;
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
    let modifiers = events
        .iter()
        .find_map(|event| match event {
            egui::Event::Key { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or_default();
    let raw = egui::RawInput {
        screen_rect: Some(screen),
        modifiers,
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

/// In edit mode Up/Down commit and continue on the adjacent row in the same column, while
/// Left/Right still belong to the text field and never move the grid cursor across columns.
#[test]
fn edit_mode_arrows_move_only_within_the_column() {
    let (ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(1);
    app.tab_mut().selection.set_cursor(1, 1);

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    run_frame(&ctx, &mut app, vec![]); // editor takes focus
    run_frame(&ctx, &mut app, vec![egui::Event::Text("7".into())]);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
    );
    assert_eq!(
        app.tab().selection.cursor(),
        Some((2, 1)),
        "ArrowDown advances one row without changing the column"
    );
    assert!(
        app.tab().edits.is_active(2, 1),
        "editor continues on the next row"
    );
    assert!(
        app.tab().edits.staged(1, 1).is_some(),
        "the previous value is committed before advancing"
    );

    run_frame(&ctx, &mut app, vec![]); // the new editor takes focus
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::ArrowLeft, egui::Modifiers::NONE)],
    );
    assert_eq!(app.tab().selection.cursor(), Some((2, 1)));
    assert!(app.tab().edits.is_active(2, 1));

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)],
    );
    assert_eq!(app.tab().selection.cursor(), Some((1, 1)));
    assert!(app.tab().edits.is_active(1, 1));
}

/// The welcome page rendered through `Context::run_ui`, whose root max_rect is effectively
/// unbounded. Locks in the fixed-size rasterization of the hills SVG: sizing that texture
/// from the painted rect requested rect × pixels_per_point texels and panicked on the GPU
/// max-texture-side limit. Also sweeps for ID clashes among the welcome widgets.
#[test]
fn welcome_page_renders_headless() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    ctx.set_pixels_per_point(2.0);

    let mut app = DbGuiApp::construct();
    app.show_welcome = true;

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
    assert!(
        clashes.is_empty(),
        "ID clashes on welcome page:\n{}",
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
    app.show_welcome = false;
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

/// The Saved queries tab renders its full-width list without ID clashes or panics.
#[test]
fn probe_saved_queries_tab() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);

    let mut app = DbGuiApp::construct();
    app.tab_mut().sql = "SELECT * FROM t".into();
    app.sidebar_tab = SidebarTab::Queries;
    for i in 0..3 {
        app.favorites_cache.push(dbcore::Favorite {
            id: format!("id-{i}"),
            name: format!("Saved query {i}"),
            sql: format!("SELECT {i} FROM t WHERE x = {i}"),
            conn_id: None,
            conn_name: Some("test-conn".into()),
            folder: None,
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

#[test]
fn full_erd_can_be_edited_and_forward_engineered() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    let mut cfg = dbcore::ConnectionConfig::new(dbcore::DbKind::Sqlite);
    cfg.id = "c1".into();
    cfg.production = true;
    app.connections.push(cfg);
    connect_fake(&mut app, fake_schema_with_fk());

    app.apply_action(Action::ShowDatabaseDiagram);
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Diagram);
    assert_eq!(app.tab().diagram.as_ref().unwrap().design.tables.len(), 3);

    // Rename the referenced table in the portable editor. Incoming FK references must follow
    // the rename so the design remains valid and its diagram can be rebuilt.
    app.apply_action(Action::EditErdTable(0));
    let Some(ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() else {
        panic!("ER table editor did not open");
    };
    editor.table_name = "accounts".into();
    app.apply_action(Action::SaveErdTable);

    let design = &app.tab().diagram.as_ref().unwrap().design;
    assert_eq!(design.tables[0].name, "accounts");
    assert_eq!(design.tables[1].foreign_keys[0].ref_table, "accounts");
    assert!(app.tab().schema_editor.is_none());

    // A rejected edit must not leak into the canvas if the user cancels afterward.
    app.apply_action(Action::EditErdTable(0));
    let Some(ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() else {
        panic!("ER table editor did not reopen");
    };
    editor.table_name = "table_1".into();
    app.apply_action(Action::SaveErdTable);
    assert!(app
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Duplicate table")));
    app.apply_action(Action::CancelSchema);
    assert_eq!(
        app.tab().diagram.as_ref().unwrap().design.tables[0].name,
        "accounts"
    );

    app.apply_action(Action::ForwardEngineerErd);
    let pending = app
        .danger_pending
        .as_ref()
        .expect("forward-engineer DDL must be guarded");
    assert!(pending.sql.contains("CREATE TABLE \"accounts\""));
    assert!(pending.sql.contains("REFERENCES \"accounts\""));
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

/// Following a FK cell from a Query tab builds a filtered `SELECT` of the referenced table
/// and keeps the code-first layout: editor above, referenced rows below.
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
    assert_eq!(
        opened.kind,
        crate::components::QueryTabKind::Query,
        "FK navigation from a Query tab must keep the result table below the editor"
    );
    assert_eq!(
        query_editor_placement(opened.kind),
        QueryEditorPlacement::Top
    );
}

/// "Show Diagram" on a table opens the ERD scoped to that table's FK neighborhood;
/// the depth control can then widen it to the whole schema without losing the root.
#[test]
fn show_table_diagram_opens_scoped_erd_and_widens() {
    let mut app = DbGuiApp::construct();
    connect_fake(&mut app, fake_schema_with_fk());

    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "table_1".into(),
    });
    assert_eq!(app.tabs.len(), 2, "the diagram opens in its own tab");
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Diagram);
    assert_eq!(app.tab().title, "table_1");
    let erd = app.tab().diagram.as_ref().expect("diagram opened");
    assert_eq!(
        erd.nodes.len(),
        2,
        "table_1 plus its FK parent table_0 — unrelated table_2 stays out"
    );
    assert_eq!(erd.focus.as_ref().map(|f| f.depth), Some(1));
    assert_eq!(
        erd.selected,
        Some(1),
        "the root table (table_1, second in schema order) is highlighted"
    );

    // Re-opening the same table selects the existing tab instead of stacking one.
    app.select_tab(0);
    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "table_1".into(),
    });
    assert_eq!(app.tabs.len(), 2, "same scope reuses its tab");
    assert_eq!(app.active_query_tab, 1);

    // Refresh (after DDL / re-introspection) must keep the focus scope.
    app.apply_action(Action::RefreshErd);
    let erd = app
        .tab()
        .diagram
        .as_ref()
        .expect("diagram survives refresh");
    assert_eq!(erd.nodes.len(), 2);
    assert!(erd.focus.is_some());

    // Widening to "All" shows the whole schema — root still highlighted, focus (and
    // with it the depth control) retained so the user can narrow back down.
    app.apply_action(Action::SetErdDepth(crate::erd::DEPTH_ALL));
    let erd = app.tab().diagram.as_ref().expect("diagram still open");
    assert_eq!(erd.nodes.len(), 3);
    assert_eq!(
        erd.focus.as_ref().map(|f| f.depth),
        Some(crate::erd::DEPTH_ALL)
    );
    assert_eq!(erd.selected, Some(1));

    // …and back to 1 hop.
    app.apply_action(Action::SetErdDepth(1));
    let erd = app.tab().diagram.as_ref().expect("diagram still open");
    assert_eq!(erd.nodes.len(), 2, "the All detour is fully reversible");
    assert_eq!(erd.focus.as_ref().map(|f| f.depth), Some(1));
}

#[test]
fn show_table_diagram_waits_for_full_schema_metadata() {
    let mut app = DbGuiApp::construct();
    connect_fake(&mut app, fake_schema(3, 0));
    app.connection_jobs.insert("c1".into());

    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "table_1".into(),
    });

    assert_eq!(
        app.tabs.len(),
        1,
        "overview metadata must not open an empty ERD"
    );
    assert_eq!(app.status_msg, "Loading table relationships…");
}

/// Screenshot generator (ignored): the ER diagram views — table-scoped (depth 1 and 2),
/// the full layered layout, and the zoomed-out LOD — over a realistic shop schema.
/// Also prints build timings for a 400-table schema (the old freeze case).
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_erd_views() {
    let col = |name: &str, ty: &str, pk: bool| dbcore::ColumnInfo {
        name: name.into(),
        data_type: ty.into(),
        nullable: !pk,
        primary_key: pk,
        default: None,
        check: None,
        comment: None,
    };
    let fk = |cols: &[&str], ref_table: &str| dbcore::ForeignKeyInfo {
        name: format!("fk_{ref_table}"),
        columns: cols.iter().map(|s| s.to_string()).collect(),
        ref_schema: None,
        ref_table: ref_table.into(),
        ref_columns: vec!["id".into()],
        on_delete: "CASCADE".into(),
        on_update: "NO ACTION".into(),
    };
    let table = |name: &str, columns: Vec<dbcore::ColumnInfo>, fks: Vec<dbcore::ForeignKeyInfo>| {
        dbcore::TableInfo {
            schema: None,
            name: name.into(),
            columns,
            indexes: Vec::new(),
            foreign_keys: fks,
        }
    };
    let schema = SchemaTree {
        database_name: "shop".into(),
        views: Vec::new(),
        routines: Vec::new(),
        triggers: Vec::new(),
        tables: vec![
            table(
                "users",
                vec![
                    col("id", "INTEGER", true),
                    col("email", "TEXT", false),
                    col("name", "TEXT", false),
                ],
                vec![],
            ),
            table(
                "addresses",
                vec![
                    col("id", "INTEGER", true),
                    col("user_id", "INTEGER", false),
                    col("street", "TEXT", false),
                    col("city", "TEXT", false),
                ],
                vec![fk(&["user_id"], "users")],
            ),
            table(
                "categories",
                vec![
                    col("id", "INTEGER", true),
                    col("parent_id", "INTEGER", false),
                    col("name", "TEXT", false),
                ],
                vec![fk(&["parent_id"], "categories")],
            ),
            table(
                "products",
                vec![
                    col("id", "INTEGER", true),
                    col("category_id", "INTEGER", false),
                    col("name", "TEXT", false),
                    col("price", "NUMERIC", false),
                ],
                vec![fk(&["category_id"], "categories")],
            ),
            table(
                "orders",
                vec![
                    col("id", "INTEGER", true),
                    col("user_id", "INTEGER", false),
                    col("address_id", "INTEGER", false),
                    col("status", "TEXT", false),
                    col("total", "NUMERIC", false),
                ],
                vec![fk(&["user_id"], "users"), fk(&["address_id"], "addresses")],
            ),
            table(
                "order_items",
                vec![
                    col("id", "INTEGER", true),
                    col("order_id", "INTEGER", false),
                    col("product_id", "INTEGER", false),
                    col("qty", "INTEGER", false),
                ],
                vec![fk(&["order_id"], "orders"), fk(&["product_id"], "products")],
            ),
            table(
                "payments",
                vec![
                    col("id", "INTEGER", true),
                    col("order_id", "INTEGER", false),
                    col("method", "TEXT", false),
                    col("amount", "NUMERIC", false),
                ],
                vec![fk(&["order_id"], "orders")],
            ),
            table(
                "shipments",
                vec![
                    col("id", "INTEGER", true),
                    col("order_id", "INTEGER", false),
                    col("carrier", "TEXT", false),
                    col("tracking", "TEXT", false),
                ],
                vec![fk(&["order_id"], "orders")],
            ),
            table(
                "reviews",
                vec![
                    col("id", "INTEGER", true),
                    col("user_id", "INTEGER", false),
                    col("product_id", "INTEGER", false),
                    col("rating", "INTEGER", false),
                ],
                vec![fk(&["user_id"], "users"), fk(&["product_id"], "products")],
            ),
            table(
                "app_settings",
                vec![
                    col("id", "INTEGER", true),
                    col("key", "TEXT", false),
                    col("value", "TEXT", false),
                ],
                vec![],
            ),
            table(
                "audit_log",
                vec![
                    col("id", "INTEGER", true),
                    col("action", "TEXT", false),
                    col("at", "TIMESTAMP", false),
                ],
                vec![],
            ),
        ],
    };

    // Timing probe: the freeze case was a big schema. 400 tables, chained FKs.
    let big = SchemaTree {
        database_name: "big".into(),
        views: Vec::new(),
        routines: Vec::new(),
        triggers: Vec::new(),
        tables: (0..400)
            .map(|i| {
                let fks = if i % 5 != 0 {
                    vec![fk(
                        &["parent_id"],
                        Box::leak(format!("t{}", i / 5 * 5).into_boxed_str()),
                    )]
                } else {
                    vec![]
                };
                table(
                    Box::leak(format!("t{i}").into_boxed_str()),
                    vec![
                        col("id", "INTEGER", true),
                        col("parent_id", "INTEGER", false),
                        col("payload", "TEXT", false),
                    ],
                    fks,
                )
            })
            .collect(),
    };
    let t0 = std::time::Instant::now();
    let big_erd = crate::erd::ErDiagram::build("c1", &big);
    println!(
        "BUILD 400 tables: {:?} ({} nodes, {} edges)",
        t0.elapsed(),
        big_erd.nodes.len(),
        big_erd.edges.len()
    );

    let theme = crate::theme::ThemeRegistry::load().theme_of("plusplus-dark");
    let schema2 = schema.clone();
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    connect_fake(&mut app, schema);
    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "orders".into(),
    });

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1360.0, 850.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::theme::set_current(theme);
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);
    harness.snapshot("erd_focused_depth1");

    // Widen to 2 hops via the header's segmented control. Clicked through the
    // accessibility tree: at pixels_per_point 2 the pointer-simulation path maps
    // the node rect to the wrong spot.
    use egui_kittest::kittest::Queryable as _;
    harness.get_by_label("2").click_accesskit();
    harness.run_steps(4);
    println!(
        "  depth 2 widened: {}",
        harness
            .query_by_label("shop · 8 tables · 9 relations")
            .is_some()
    );
    harness.snapshot("erd_focused_depth2");

    // The whole schema, layered; the depth control must survive the widening.
    harness.get_by_label("All").click_accesskit();
    harness.run_steps(4);
    println!(
        "  after All: {}",
        harness
            .query_by_label("shop · 11 tables · 11 relations")
            .is_some()
    );
    harness.snapshot("erd_full");

    // Narrowing back down still works — "All" must not strand the user.
    harness.get_by_label("1").click_accesskit();
    harness.run_steps(4);
    println!(
        "  back to depth 1: {}",
        harness
            .query_by_label("shop · 6 tables · 6 relations")
            .is_some()
    );
    // Two harnesses in one test must funnel their snapshot verdicts through one
    // `SnapshotResults`, or kittest panics on drop.
    let mut snapshot_results = harness.take_snapshot_results();
    drop(harness);

    // The same scoped view on the light theme: dots, borders, and edges must not
    // wash out against the white canvas.
    let theme = crate::theme::ThemeRegistry::load().theme_of("daylight");
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    connect_fake(&mut app, schema2);
    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "orders".into(),
    });
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1360.0, 850.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::theme::set_current(theme);
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);
    harness.snapshot("erd_focused_daylight");
    snapshot_results.extend(harness.take_snapshot_results());
}

#[test]
fn follow_foreign_key_from_table_keeps_data_first_layout() {
    let mut app = fk_tab(vec![
        Value::Text("row-pk".into()),
        Value::Text("u7".into()),
        Value::Null,
        Value::Null,
    ]);
    app.tab_mut().kind = crate::components::QueryTabKind::Table;

    app.apply_action(Action::FollowForeignKey { row: 0, col: 1 });

    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Table);
    assert_eq!(
        query_editor_placement(app.tab().kind),
        QueryEditorPlacement::Bottom
    );
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

/// The sidebar History tab owns the history cache: entering loads it, leaving
/// drops it (same lifecycle the old side panel had).
#[test]
fn sidebar_history_tab_owns_the_cache() {
    let mut app = DbGuiApp::construct();
    assert_eq!(app.sidebar_tab, SidebarTab::Items);
    app.apply_action(Action::SetSidebarTab(SidebarTab::History));
    assert_eq!(app.sidebar_tab, SidebarTab::History);
    app.apply_action(Action::SetSidebarTab(SidebarTab::Items));
    assert_eq!(app.sidebar_tab, SidebarTab::Items);
    assert!(
        app.history_cache.is_empty(),
        "leaving the History tab drops the cache"
    );
}

fn history_entry(sql: &str) -> dbcore::history::HistoryEntry {
    dbcore::history::HistoryEntry {
        at: "2026-08-19T07:00:00Z".into(),
        conn_id: "c1".into(),
        conn_name: "local".into(),
        sql: sql.into(),
        ok: true,
        error: None,
        rows: Some(1),
        elapsed_ms: 1.0,
    }
}

fn connected_sqlite_app() -> DbGuiApp {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    let mut cfg = dbcore::ConnectionConfig::new(dbcore::DbKind::Sqlite);
    cfg.id = "c1".into();
    app.connections.push(cfg);
    app.active_connections.push(ActiveConnection {
        config_id: "c1".into(),
        name: "local".into(),
        db: std::sync::Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    app.tab_mut().conn_id = Some("c1".into());
    app
}

#[test]
fn run_history_sql_opens_a_query_tab_and_executes() {
    let mut app = connected_sqlite_app();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().title = "users".into();
    app.tab_mut().sql = "SELECT * FROM users".into();
    app.settings_open = true;
    app.history_cache.push(history_entry("SELECT 42"));

    app.apply_action(Action::RunHistorySql(0));

    assert!(!app.settings_open);
    assert_eq!(app.tabs.len(), 2, "table tab must stay, query tab is added");
    assert_eq!(app.tabs[0].sql, "SELECT * FROM users");
    assert_eq!(app.tabs[0].kind, crate::components::QueryTabKind::Table);
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Query);
    assert_eq!(app.tab().sql, "SELECT 42");
    assert_eq!(app.busy, Busy::Querying);
}

#[test]
fn run_history_sql_reuses_a_blank_query_tab() {
    let mut app = connected_sqlite_app();
    app.history_cache.push(history_entry("SELECT 7"));

    app.apply_action(Action::RunHistorySql(0));

    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Query);
    assert_eq!(app.tab().sql, "SELECT 7");
    assert_eq!(app.busy, Busy::Querying);
}

#[test]
fn use_history_sql_from_a_table_tab_opens_a_query_tab() {
    let mut app = connected_sqlite_app();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().title = "users".into();
    app.tab_mut().sql = "SELECT * FROM users".into();
    app.history_cache.push(history_entry("SELECT 1"));

    app.apply_action(Action::UseHistorySql(0));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[0].sql, "SELECT * FROM users");
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Query);
    assert_eq!(app.tab().sql, "SELECT 1");
    assert_eq!(app.busy, Busy::Idle, "insert does not run");
}

fn saved_query(name: &str, sql: &str) -> dbcore::Favorite {
    dbcore::Favorite {
        id: "fav-1".into(),
        name: name.into(),
        sql: sql.into(),
        conn_id: Some("c1".into()),
        conn_name: Some("local".into()),
        folder: None,
        created_at: "2026-08-19T07:00:00Z".into(),
    }
}

#[test]
fn run_favorite_opens_a_query_tab_and_executes() {
    let mut app = connected_sqlite_app();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().title = "users".into();
    app.tab_mut().sql = "SELECT * FROM users".into();
    app.favorites_cache
        .push(saved_query("forty two", "SELECT 42"));

    app.apply_action(Action::RunFavorite(0));

    assert_eq!(app.tabs.len(), 2, "table tab must stay, query tab is added");
    assert_eq!(app.tabs[0].sql, "SELECT * FROM users");
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Query);
    assert_eq!(app.tab().sql, "SELECT 42");
    assert_eq!(app.busy, Busy::Querying);
}

#[test]
fn use_favorite_from_a_table_tab_opens_a_query_tab() {
    let mut app = connected_sqlite_app();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().title = "users".into();
    app.tab_mut().sql = "SELECT * FROM users".into();
    app.favorites_cache.push(saved_query("one", "SELECT 1"));

    app.apply_action(Action::UseFavorite(0));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[0].sql, "SELECT * FROM users");
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Query);
    assert_eq!(app.tab().sql, "SELECT 1");
    assert_eq!(app.busy, Busy::Idle, "open does not run");
}

#[test]
fn saved_queries_group_into_folders_and_move() {
    let mut app = DbGuiApp::construct();
    app.favorites_cache.push(saved_query("Sac", "SELECT 1"));
    app.apply_action(Action::NewFavoriteFolder { move_id: None });
    if let Some(draft) = app.folder_pending.as_mut() {
        draft.name = "Reports".into();
    }
    app.apply_action(Action::ConfirmFavoriteFolder);
    assert_eq!(app.favorite_folders, ["Reports"]);
    assert_eq!(app.favorites_cache[0].folder, None);

    app.apply_action(Action::MoveFavorite {
        idx: 0,
        folder: Some("Reports".into()),
    });
    assert_eq!(app.favorites_cache[0].folder.as_deref(), Some("Reports"));

    let groups = dbcore::favorites::grouped(&app.favorites_cache, &app.favorite_folders, "", true);
    assert_eq!(groups[0].0, "Reports");
    assert_eq!(groups[0].1, vec![0]);
}

#[test]
fn named_query_folder_can_be_renamed_and_deleted() {
    let mut app = DbGuiApp::construct();
    app.favorites_cache.push(saved_query("Sac", "SELECT 1"));
    app.apply_action(Action::NewFavoriteFolder {
        move_id: Some("fav-1".into()),
    });
    if let Some(draft) = app.folder_pending.as_mut() {
        draft.name = "Reports".into();
    }
    app.apply_action(Action::ConfirmFavoriteFolder);
    assert_eq!(app.favorite_folders, ["Reports"]);
    assert_eq!(app.favorites_cache[0].folder.as_deref(), Some("Reports"));

    app.apply_action(Action::RenameFavoriteFolder("Reports".into()));
    if let Some(draft) = app.folder_pending.as_mut() {
        draft.name = "Ops".into();
    }
    app.apply_action(Action::ConfirmFavoriteFolder);
    assert_eq!(app.favorite_folders, ["Ops"]);
    assert_eq!(app.favorites_cache[0].folder.as_deref(), Some("Ops"));

    app.apply_action(Action::DeleteFavoriteFolder("Ops".into()));
    assert!(app.favorite_folders.is_empty());
    assert_eq!(app.favorites_cache[0].folder, None);
}

#[test]
fn ungrouped_query_folder_cannot_be_renamed_or_deleted() {
    let mut app = DbGuiApp::construct();
    app.favorites_cache.push(saved_query("Sac", "SELECT 1"));
    app.apply_action(Action::RenameFavoriteFolder(
        dbcore::favorites::UNGROUPED.into(),
    ));
    assert!(
        app.folder_pending.is_none(),
        "Ungrouped is not a real folder"
    );
    app.apply_action(Action::DeleteFavoriteFolder(
        dbcore::favorites::UNGROUPED.into(),
    ));
    assert_eq!(app.favorites_cache[0].folder, None);
    assert_eq!(app.favorites_cache.len(), 1);
}

#[test]
fn saved_query_folders_and_queries_can_be_reordered_by_drop() {
    let mut app = DbGuiApp::construct();
    app.favorites_cache.push(saved_query("One", "SELECT 1"));
    app.favorites_cache.push(saved_query("Two", "SELECT 2"));
    app.favorites_cache[0].id = "q1".into();
    app.favorites_cache[1].id = "q2".into();
    app.apply_action(Action::NewFavoriteFolder { move_id: None });
    if let Some(draft) = app.folder_pending.as_mut() {
        draft.name = "Reports".into();
    }
    app.apply_action(Action::ConfirmFavoriteFolder);
    app.apply_action(Action::NewFavoriteFolder { move_id: None });
    if let Some(draft) = app.folder_pending.as_mut() {
        draft.name = "Ops".into();
    }
    app.apply_action(Action::ConfirmFavoriteFolder);
    assert_eq!(app.favorite_folders, ["Reports", "Ops"]);

    app.apply_action(Action::ReorderFavoriteFolder {
        source: "Reports".into(),
        target: "Ops".into(),
        after: true,
    });
    assert_eq!(app.favorite_folders, ["Ops", "Reports"]);

    app.apply_action(Action::DropFavoriteOnFolder {
        id: "q1".into(),
        folder: Some("Reports".into()),
    });
    assert_eq!(app.favorites_cache[1].id, "q1");
    assert_eq!(app.favorites_cache[1].folder.as_deref(), Some("Reports"));

    app.apply_action(Action::DropFavoriteOnQuery {
        source_id: "q2".into(),
        target_id: "q1".into(),
        after: true,
    });
    assert_eq!(
        app.favorites_cache
            .iter()
            .map(|q| q.id.as_str())
            .collect::<Vec<_>>(),
        ["q1", "q2"]
    );
    assert_eq!(app.favorites_cache[1].folder.as_deref(), Some("Reports"));
}

#[test]
fn query_history_groups_newest_first_by_local_day_and_filters_entries() {
    let entry = |at: &str, conn: &str, sql: &str| dbcore::history::HistoryEntry {
        at: at.into(),
        conn_id: conn.into(),
        conn_name: conn.into(),
        sql: sql.into(),
        ok: true,
        error: None,
        rows: Some(1),
        elapsed_ms: 1.0,
    };
    let entries = vec![
        entry("2026-07-29T12:00:00+07:00", "archive", "SELECT old"),
        entry("2026-08-11T09:00:00+07:00", "primary", "SELECT first"),
        entry("2026-08-11T10:00:00+07:00", "primary", "SELECT newest"),
    ];

    let days = super::panels::grouped_history(&entries, "", Some("primary"));
    assert_eq!(days.len(), 1);
    assert_eq!(days[0].entries, vec![2, 1]);
    assert!(days[0].label.contains("August"));

    let archive = super::panels::grouped_history(&entries, "", Some("archive"));
    assert_eq!(archive.len(), 1);
    assert_eq!(archive[0].entries, vec![0]);
    assert!(archive[0].label.contains("July"));

    let filtered = super::panels::grouped_history(&entries, "newest", Some("primary"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].entries, vec![2]);

    assert!(super::panels::grouped_history(&entries, "", None).is_empty());
}

#[test]
fn saved_queries_are_scoped_by_connection_and_keep_legacy_queries_global() {
    let mut primary = saved_query("Primary", "SELECT 1");
    primary.id = "primary".into();
    let mut archive = saved_query("Archive", "SELECT 2");
    archive.id = "archive".into();
    archive.conn_id = Some("c2".into());
    archive.folder = Some("Valet".into());
    let mut global = saved_query("Legacy", "SELECT 3");
    global.id = "legacy".into();
    global.conn_id = None;
    global.conn_name = None;
    let queries = vec![primary, archive, global];

    let folders = vec!["Valet".to_string()];
    let primary_groups =
        super::panels::grouped_favorites_for_connection(&queries, &folders, "", true, Some("c1"));
    assert_eq!(primary_groups.len(), 1);
    assert_eq!(primary_groups[0].0, dbcore::favorites::UNGROUPED);
    assert_eq!(primary_groups[0].1, vec![0, 2]);

    let archive_groups =
        super::panels::grouped_favorites_for_connection(&queries, &folders, "", true, Some("c2"));
    assert_eq!(archive_groups[0], ("Valet".into(), vec![1]));
    assert_eq!(archive_groups[1].1, vec![2]);

    let unbound_groups =
        super::panels::grouped_favorites_for_connection(&queries, &folders, "", true, None);
    assert_eq!(unbound_groups[0].1, vec![2]);
}

/// Show Diagram needs a live connection: without one it surfaces an error and
/// opens nothing.
#[test]
fn show_table_diagram_needs_a_connection() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "table_1".into(),
    });
    assert_eq!(app.tabs.len(), 1, "no connection, no diagram tab");
    assert!(app.error.is_some(), "no connection should surface an error");
}

/// RefreshErd rebuilds from the connection's current schema, keeping the position of
/// nodes whose table survived; after a disconnect the snapshot stays viewable.
#[test]
fn erd_refresh_keeps_positions_and_disconnect_keeps_snapshot() {
    let mut app = DbGuiApp::construct();
    connect_fake(&mut app, fake_schema_with_fk());
    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "table_1".into(),
    });
    // Widen to the whole schema so the refresh below can pick up a new table.
    app.apply_action(Action::SetErdDepth(crate::erd::DEPTH_ALL));
    assert_eq!(app.tab().diagram.as_ref().unwrap().nodes.len(), 3);

    // The user drags table_0 somewhere specific…
    let moved = egui::pos2(1234.0, 567.0);
    app.tab_mut().diagram.as_mut().unwrap().nodes[0].pos = moved;

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
                default: None,
                check: None,
                comment: None,
            }],
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        });
        s
    };
    app.apply_action(Action::RefreshErd);
    let erd = app
        .tab()
        .diagram
        .as_ref()
        .expect("refresh keeps the diagram open");
    assert_eq!(erd.nodes.len(), 4);
    let kept = erd.nodes.iter().find(|n| n.title == "table_0").unwrap();
    assert_eq!(
        kept.pos, moved,
        "surviving nodes keep their dragged position"
    );

    // Disconnecting keeps the snapshot on screen; a refresh without the connection
    // is a no-op rather than a wipe.
    app.disconnect_conn("c1");
    assert!(
        app.tab().diagram.is_some(),
        "the snapshot outlives the connection"
    );
    app.apply_action(Action::RefreshErd);
    assert_eq!(app.tab().diagram.as_ref().unwrap().nodes.len(), 4);
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
    app.apply_action(Action::ShowTableDiagram {
        schema: None,
        table: "table_1".into(),
    });
    assert!(app.tab().diagram.is_some());

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

    assert!(
        app.tab().diagram.is_some(),
        "the diagram must survive drawing"
    );
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

// ─── Cmd/Ctrl+/ line-comment toggle ──────────────────────────────────────────

/// Apply the pure comment toggle and return the resulting buffer. `sel` is a sorted char
/// range; `None` (no edit) leaves the text untouched.
fn toggle(text: &str, sel: std::ops::Range<usize>) -> String {
    match super::panels::toggle_comment_edit(text, sel.clone()) {
        Some((bytes, replacement)) => {
            let mut out = text.to_string();
            out.replace_range(bytes, &replacement);
            out
        }
        None => text.to_string(),
    }
}

#[test]
fn comment_toggle_single_line_roundtrips() {
    // A bare caret comments the whole line, then uncomments it back.
    let commented = toggle("SELECT 1", 3..3);
    assert_eq!(commented, "-- SELECT 1");
    assert_eq!(toggle(&commented, 3..3), "SELECT 1");
}

#[test]
fn comment_toggle_preserves_indent_and_aligns_markers() {
    // Markers align at the shallowest indent (column 2 here); the deeper line keeps its extra
    // indentation after the marker, so relative nesting survives — exactly like VS Code.
    let src = "  a\n    b";
    let out = toggle(src, 0..src.chars().count());
    assert_eq!(out, "  -- a\n  --   b");
    // Round-trips: uncommenting restores the original indentation exactly.
    assert_eq!(toggle(&out, 0..out.chars().count()), src);
}

#[test]
fn comment_toggle_uncomments_only_when_all_lines_commented() {
    // One bare line among commented ones means the block is not fully commented, so the
    // toggle comments everything (rather than stripping markers).
    let src = "-- a\nb";
    let out = toggle(src, 0..src.chars().count());
    assert_eq!(out, "-- -- a\n-- b");
    // Now every line carries a marker: the next toggle strips exactly one level back.
    assert_eq!(toggle(&out, 0..out.chars().count()), src);
}

#[test]
fn comment_toggle_skips_blank_lines_but_still_toggles() {
    // A blank line inside the block is left untouched when commenting, and ignored when
    // deciding whether the block is fully commented.
    let src = "a\n\nb";
    let out = toggle(src, 0..src.chars().count());
    assert_eq!(out, "-- a\n\n-- b");
    assert_eq!(toggle(&out, 0..out.chars().count()), src);
    // An all-blank selection is a no-op.
    assert_eq!(toggle("\n\n", 0..2), "\n\n");
}

#[test]
fn comment_toggle_selection_ending_at_line_start_drops_trailing_line() {
    // Selecting "a\n" (caret parked at the start of line two) must not comment line two.
    let src = "a\nb";
    let out = toggle(src, 0..2);
    assert_eq!(out, "-- a\nb");
}

#[test]
fn comment_toggle_uncomment_handles_marker_without_trailing_space() {
    // `--x` (no space) uncomments to `x`; `-- x` uncomments to `x` as well.
    assert_eq!(toggle("--x", 0..3), "x");
    assert_eq!(toggle("-- x", 0..4), "x");
}

#[test]
fn comment_toggle_is_multibyte_safe() {
    // Char indices past a multi-byte glyph must map to byte boundaries, not split it.
    let src = "café\nSELECT 1";
    let out = toggle(src, 0..src.chars().count());
    assert_eq!(out, "-- café\n-- SELECT 1");
    assert_eq!(toggle(&out, 0..out.chars().count()), src);
}

// ─── live SQL syntax check (red squiggle) ────────────────────────────────────

/// A typo has to light up on its own, without running the query — but only after the user
/// pauses, and it has to clear itself once the SQL parses again.
#[test]
fn sql_typos_are_flagged_after_a_pause_and_clear_when_fixed() {
    use std::sync::{Arc, Mutex};

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().sql = "SELCT * FROM users".into();

    // The app lives inside the harness closure, so the test reads its diagnostic through a
    // probe and edits the SQL through a slot the closure drains.
    let seen: Arc<Mutex<Option<dbcore::SyntaxError>>> = Arc::new(Mutex::new(None));
    let next_sql: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let (probe, edit) = (seen.clone(), next_sql.clone());
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            if let Some(sql) = edit.lock().unwrap().take() {
                app.tab_mut().sql = sql;
            }
            app.draw(ui, None);
            probe
                .lock()
                .unwrap()
                .clone_from(&app.tab().editor_assist.syntax_error);
        });

    // The harness steps 0.25s at a time, so a couple of frames covers the debounce.
    harness.run_steps(4);
    let error = seen.lock().unwrap().clone().expect("typo must be flagged");
    let marked: String = "SELCT * FROM users"
        .chars()
        .skip(error.range.start)
        .take(error.range.end - error.range.start)
        .collect();
    assert_eq!(
        marked, "SELCT",
        "the misspelled keyword is what gets marked"
    );
    assert!(
        !error.message.is_empty(),
        "the tooltip needs something to say"
    );

    *next_sql.lock().unwrap() = Some("SELECT * FROM users".to_string());
    harness.run_steps(4);
    assert!(
        seen.lock().unwrap().is_none(),
        "fixing the SQL must clear the mark"
    );
}

#[test]
fn the_token_being_typed_is_never_marked() {
    use super::panels::error_under_caret;

    // `SELE` is flagged at 0..4. While the caret is inside it — or parked just past its last
    // char, which is where typing leaves it — the word is still being written.
    for caret in 0..=4 {
        assert!(
            error_under_caret(&(0..4), Some(caret)),
            "caret {caret} is inside the word being typed"
        );
    }
    // Once the caret has moved on (or the editor lost focus), the mark is fair game.
    assert!(!error_under_caret(&(0..4), Some(5)));
    assert!(!error_under_caret(&(2..4), Some(1)));
    assert!(!error_under_caret(&(0..4), None));
}

/// Hovering the marked token must explain it. The squiggle alone says "wrong"; the tooltip
/// is where the reason lives — and it has to win the hover against the `TextEdit` under it.
#[test]
fn hovering_a_marked_token_explains_the_error() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().sql = "SELCT * FROM users".into();

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);

    // The line-number gutter sits flush against the text, so a few points to its right and
    // down is inside the first token — which is the one that failed to parse.
    let gutter = harness.get_by_label("SQL line numbers").rect();
    let over_token = egui::pos2(gutter.right() + 10.0, gutter.top() + 7.0);
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(over_token));
    harness.run_steps(6);

    assert!(
        harness.query_by_label("Syntax error").is_some(),
        "hovering the squiggle must open the explanation"
    );

    // Control: the rest of the editor is just text. Hovering it says nothing.
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(egui::pos2(
            gutter.right() + 400.0,
            gutter.top() + 7.0,
        )));
    harness.run_steps(6);
    assert!(
        harness.query_by_label("Syntax error").is_none(),
        "the tooltip belongs to the marked token, not the whole editor"
    );
}

/// The fold chevrons in the SQL gutter collapse a region and open it again — and the query
/// itself must come through untouched, because the editor is writing through a folded view of
/// it the whole time.
#[test]
fn clicking_a_gutter_chevron_folds_a_statement_without_touching_the_sql() {
    use egui_kittest::kittest::Queryable;
    use std::sync::{Arc, Mutex};

    const SCRIPT: &str = "SELECT a,\n       b\nFROM users;\n\nSELECT 2;\n";

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().sql = SCRIPT.into();

    // (SQL, folded anchors) as of the last frame.
    let state: Arc<Mutex<(String, Vec<usize>)>> = Arc::new(Mutex::new((String::new(), Vec::new())));
    let probe = state.clone();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
            let tab = app.tab();
            *probe.lock().unwrap() = (tab.sql.clone(), tab.folds.iter().copied().collect());
        });
    harness.run_steps(4);

    let regions = crate::fold::regions(SCRIPT);
    let first = regions
        .iter()
        .find(|r| r.header_line == 0)
        .expect("the opening statement spans three lines");

    // The chevron column sits at the right edge of the gutter, against the code, on the first
    // line's row.
    let gutter = harness.get_by_label("SQL line numbers").rect();
    let chevron = egui::pos2(gutter.right() - 7.0, gutter.top() + 7.0);
    let click = |harness: &mut egui_kittest::Harness<'_>| {
        harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(chevron));
        for pressed in [true, false] {
            harness.input_mut().events.push(egui::Event::PointerButton {
                pos: chevron,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run_steps(4);
    };

    click(&mut harness);
    let (sql, folds) = state.lock().unwrap().clone();
    assert_eq!(
        folds,
        vec![first.anchor],
        "the chevron folds its own region"
    );
    assert_eq!(sql, SCRIPT, "folding must not rewrite the query");

    click(&mut harness);
    let (sql, folds) = state.lock().unwrap().clone();
    assert!(folds.is_empty(), "clicking again opens it back up");
    assert_eq!(sql, SCRIPT, "unfolding must not rewrite the query either");
}

/// Everything the editor does downstream of the caret — completion, ghost text, diagnostics,
/// and the keystrokes themselves — indexes the real SQL, not the folded view. Typing below a
/// collapsed region must therefore land where the user is pointing, not `N lines` earlier.
#[test]
fn typing_below_a_fold_lands_in_the_real_sql() {
    use egui_kittest::kittest::Queryable;
    use std::sync::{Arc, Mutex};

    const SCRIPT: &str = "SELECT a,\n       b\nFROM users;\n\nSELECT 2;\n";

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().sql = SCRIPT.into();
    let anchor = crate::fold::regions(SCRIPT)
        .into_iter()
        .find(|r| r.header_line == 0)
        .expect("the first statement folds")
        .anchor;
    app.tab_mut().folds.insert(anchor);

    let seen: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let probe = seen.clone();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
            probe.lock().unwrap().clone_from(&app.tab().sql);
        });
    harness.run_steps(4);

    // With the first statement collapsed the third visible row is `SELECT 2;`. Click past its
    // end (the click clamps to the end of the line) and type there.
    let gutter = harness.get_by_label("SQL line numbers").rect();
    let at = egui::pos2(gutter.right() + 300.0, gutter.top() + 2.0 * 14.0 + 7.0);
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        harness.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.run_steps(3);
    harness
        .input_mut()
        .events
        .push(egui::Event::Text("!".to_string()));
    harness.run_steps(3);

    assert_eq!(
        *seen.lock().unwrap(),
        "SELECT a,\n       b\nFROM users;\n\nSELECT 2;!\n",
        "the keystroke belongs after the visible line, not inside the folded one"
    );
}

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn shot_fold() {
    use egui_kittest::kittest::Queryable;
    use std::sync::{Arc, Mutex};
    const SCRIPT: &str = "-- Monthly revenue by plan.\n-- Excludes trials.\nWITH paid AS (\n    SELECT customer_id,\n           amount\n    FROM invoices\n    WHERE status = 'paid'\n)\nSELECT p.name,\n       SUM(paid.amount) AS revenue,\n       CASE\n         WHEN SUM(paid.amount) > 1000 THEN 'high'\n         ELSE 'low'\n       END AS band\nFROM paid\nJOIN plans p ON p.id = paid.plan_id\nGROUP BY p.name\nORDER BY revenue DESC;\n\nSELECT count(*)\nFROM invoices;\n";
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().sql = SCRIPT.into();
    app.tab_mut().editor_size = Some(420.0);
    let want: Arc<Mutex<Option<Vec<usize>>>> = Arc::new(Mutex::new(None));
    let slot = want.clone();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(900.0, 560.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            if let Some(folds) = slot.lock().unwrap().take() {
                app.tab_mut().folds = folds.into_iter().collect();
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);
    // Park the pointer over the gutter so the open regions show their chevrons.
    let gutter = harness.get_by_label("SQL line numbers").rect();
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(egui::pos2(
            gutter.right() / 2.0,
            gutter.center().y / 2.0,
        )));
    harness.run_steps(3);
    harness.snapshot("sql_fold_open");

    let regions = crate::fold::regions(SCRIPT);
    let anchors: Vec<usize> = regions
        .iter()
        .filter(|r| matches!(r.header_line, 0 | 10))
        .map(|r| r.anchor)
        .collect();
    *want.lock().unwrap() = Some(anchors);
    harness.run_steps(4);
    harness.snapshot("sql_fold_closed");
}
