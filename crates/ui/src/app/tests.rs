use super::*;
use dbcore::{
    ColumnInfo, ColumnMeta, IndexInfo, QueryResult, QueryStats, SchemaTree, TableInfo, Value,
};

struct DummyDb;

#[test]
fn closing_sql_tabs_releases_egui_editor_history() {
    let ctx = egui::Context::default();
    let mut app = DbGuiApp::construct();
    for _ in 0..8 {
        app.tab_mut().sql = "SELECT 'a retained query';\n".repeat(100);
        let editor_id = egui::Id::new(("sql_editor", app.tab().id, "primary"));
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.sql_editor_body(ui, 0);
        });
        assert!(egui::text_edit::TextEditState::load(&ctx, editor_id).is_some());
        app.close_tab(0);
        assert!(
            egui::text_edit::TextEditState::load(&ctx, editor_id).is_none(),
            "closed SQL tab left its editor and undo history in egui memory"
        );
    }
}

#[test]
fn closing_other_tabs_releases_split_and_find_history_but_keeps_live_editor() {
    let ctx = egui::Context::default();
    let mut app = DbGuiApp::construct();
    let kept_id = egui::Id::new(("sql_editor", app.tab().id, "primary"));
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.sql_editor_body(ui, 0));
    app.new_tab();
    app.tab_mut().sql = "SELECT 1".into();
    app.tab_mut().editor_split = true;
    app.tab_mut().find.open = true;
    app.tab_mut().find.replace_open = true;
    for _ in 0..2 {
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.sql_editor_body(ui, 1));
    }
    assert!(ctx.data(|data| data.count::<egui::text_edit::TextEditState>()) >= 5);
    app.close_other_tabs(0);
    assert_eq!(
        ctx.data(|data| data.count::<egui::text_edit::TextEditState>()),
        1
    );
    assert!(egui::text_edit::TextEditState::load(&ctx, kept_id).is_some());
}

#[test]
fn switching_connection_leaves_running_queries_on_their_own_connection() {
    let mut app = app_with_staged_edit();
    app.tab_mut().edits.clear();
    let first = app.tab().id;
    let (_, first_cancel) = app.begin_query_job(first);
    app.new_tab();
    let second = app.tab().id;
    let (_, second_cancel) = app.begin_query_job(second);
    let mut other = ConnectionConfig::new(DbKind::Sqlite);
    other.id = "other".into();
    app.connections.push(other);
    app.active_connections.push(ActiveConnection {
        config_id: "other".into(),
        name: "other".into(),
        db: Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    app.bind_connection(1, false);
    // Nothing is re-pointed, so nothing is cancelled: both queries keep running against the
    // database they were started on, and the new connection gets a tab of its own.
    assert!(!second_cancel.is_cancelled());
    assert!(!first_cancel.is_cancelled());
    assert!(app.is_tab_querying(first));
    assert!(app.is_tab_querying(second));
    assert_eq!(app.tab().conn_id.as_deref(), Some("other"));
    assert_eq!(app.tabs[0].conn_id.as_deref(), Some("edit-connection"));
    assert_eq!(app.tabs[1].conn_id.as_deref(), Some("edit-connection"));
}

#[test]
fn run_action_starts_another_tab_without_canceling_the_first() {
    let mut app = app_with_staged_edit();
    app.tab_mut().edits.clear();
    app.tab_mut().sql = "SELECT 1".into();
    app.apply_action(Action::RunQuery);
    let first = app.tab().id;
    let first_cancel = app.query_jobs[&first].cancel.clone();
    app.new_tab();
    app.tab_mut().sql = "SELECT 2".into();
    app.apply_action(Action::RunQuery);
    let second = app.tab().id;
    assert!(app.is_tab_querying(first));
    assert!(app.is_tab_querying(second));
    assert!(!first_cancel.is_cancelled());
    let seq = app.tab().query_seq;
    app.apply_action(Action::RunQuery);
    assert_eq!(
        app.tab().query_seq,
        seq,
        "a duplicate run on this tab is refused"
    );
    app.apply_action(Action::CancelTabQuery(first));
    assert!(first_cancel.is_cancelled());
    assert!(app.is_tab_querying(second));
}

#[test]
fn schema_changes_and_quit_use_the_same_unsaved_guard() {
    let mut app = app_with_staged_edit();
    app.tab_mut().edits.clear();
    let table = fake_schema(1, 2).tables.remove(0);
    let mut editor = SchemaEditor::edit_table(&table, DbKind::Sqlite);
    editor.columns[0].name = "renamed_column".into();
    app.tab_mut().schema_editor = Some(ObjectEditor::Table(editor));
    app.apply_action(Action::Quit);
    assert!(app.pending_leave.is_some());
    assert!(!app.pending_quit);
    app.apply_action(Action::CancelLeaving);
    assert!(app.tab_has_unsaved_changes(0));
    app.apply_action(Action::ReloadTableStructure);
    assert!(app.pending_leave.is_some());
    assert!(app.tab().schema_editor.is_some());
    app.apply_action(Action::CancelLeaving);
    app.apply_action(Action::Quit);
    app.apply_action(Action::DiscardBeforeLeaving);
    assert!(app.pending_quit);
}

#[test]
#[ignore = "UX preview renderer; writes images into the system temporary directory"]
fn snapshot_ux_improvements() {
    let dir = std::env::temp_dir().join("plusplus-ux-preview");
    std::fs::create_dir_all(&dir).unwrap();
    let mut welcome = DbGuiApp::construct();
    welcome.connections.clear();
    welcome.show_welcome = true;
    let mut connection = DbGuiApp::construct();
    connection.connections.clear();
    connection.show_welcome = false;
    connection.apply_action(Action::NewConnection);
    connection.editor.as_mut().unwrap().selecting_provider = false;
    let mut unsaved = app_with_staged_edit();
    unsaved.show_welcome = false;
    unsaved.apply_action(Action::CloseTab(0));
    let mut query = DbGuiApp::construct();
    query.connections.clear();
    query.show_welcome = false;
    connect_fake(&mut query, fake_schema(2, 2));
    query.tab_mut().sql = "SELECT * FROM table_0".into();
    query.tab_mut().set_result(fake_result(5, 2));
    query.tab_mut().set_sort(0, true);
    query.tab_mut().filter.visible = true;
    let query_id = query.tab().id;
    query.begin_query_job(query_id);
    for (name, mut app) in [
        ("welcome", welcome),
        ("connection", connection),
        ("unsaved", unsaved),
        ("query", query),
    ] {
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
        harness.run_steps(8);
        harness
            .render()
            .unwrap()
            .save(dir.join(format!("{name}.png")))
            .unwrap();
    }
}

#[test]
fn closing_dirty_tabs_requires_an_explicit_decision() {
    let mut app = app_with_staged_edit();
    let id = app.tab().id;
    app.apply_action(Action::CloseTab(0));
    assert_eq!(app.tab().id, id);
    assert!(app.tab().edits.has_pending());
    assert!(app.pending_leave.is_some());
    app.apply_action(Action::CancelLeaving);
    assert!(app.pending_leave.is_none());
    assert!(app.tab().edits.has_pending());
    app.apply_action(Action::CloseAllTabs);
    app.apply_action(Action::DiscardBeforeLeaving);
    assert_ne!(app.tab().id, id);
    assert!(!app.tab().edits.has_pending());
}

#[test]
fn disconnect_guard_includes_dirty_inactive_tabs() {
    let mut app = app_with_staged_edit();
    app.new_tab();
    app.apply_action(Action::DisconnectConn(0));
    assert!(app.pending_leave.is_some());
    assert_eq!(app.active_connections.len(), 1);
    assert!(app.tabs[0].edits.has_pending());
    app.apply_action(Action::SaveBeforeLeaving);
    assert!(app.pending_leave.is_none());
    assert_eq!(app.active_query_tab, 0);
    assert!(app.commit_pending.is_some());
    assert_eq!(
        app.active_connections.len(),
        1,
        "save review must not disconnect"
    );
    app.apply_action(Action::CancelEdits);
    app.apply_action(Action::DisconnectConn(0));
    app.apply_action(Action::DiscardBeforeLeaving);
    assert!(app.active_connections.is_empty());
    assert!(!app.tabs[0].edits.has_pending());
}

#[test]
fn paging_filtering_and_running_preserve_staged_edits() {
    let mut app = app_with_staged_edit();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().sql = "SELECT * FROM items LIMIT 100".into();
    let sql = app.tab().sql.clone();
    app.run_page(100, 100);
    app.apply_result_filter(0, true);
    app.apply_action(Action::RunQuery);
    assert_eq!(app.tab().sql, sql);
    assert!(app.tab().edits.has_pending());
    assert!(app.query_jobs.is_empty());
    assert!(app.error.as_deref().unwrap().contains("Save or discard"));
}

#[test]
fn a_successful_save_clears_staged_edits_and_reports_no_error() {
    let mut app = app_with_staged_edit();
    app.history_enabled = false;
    app.audit_enabled = false;
    app.tab_mut().sql = "SELECT * FROM items".into();
    let tab_id = app.tab().id;
    app.tx
        .send(AppMessage::Committed {
            tab_id,
            conn_id: String::new(),
            sql: "UPDATE items SET b = 'edited'".into(),
            elapsed_ms: 1.0,
            result: Ok(1),
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert!(!app.tab().edits.has_pending());
    assert!(app.error.is_none(), "{:?}", app.error);
}

#[test]
fn table_metadata_arriving_after_open_makes_the_open_table_editable() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    let mut overview = fake_schema(1, 2);
    for column in &mut overview.tables[0].columns {
        column.primary_key = false;
    }
    overview.tables[0].indexes.clear();
    connect_fake(&mut app, overview);
    app.tab_mut().conn_id = Some("c1".into());
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_0".into(),
        pk_cols: Vec::new(),
    });
    assert!(!app.tab().edits.editable());

    let tab_id = app.tab().id;
    let full = fake_schema(1, 2).tables.remove(0);
    app.tx
        .send(AppMessage::TableMetadataLoaded {
            tab_id,
            conn_id: "c1".into(),
            schema: None,
            table: "table_0".into(),
            result: Ok(Some(full)),
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert!(app.tab().edits.editable());
}

#[test]
fn reloading_a_restored_table_tab_keeps_it_editable() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    connect_fake(&mut app, fake_schema(1, 2));
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().sql = "SELECT * FROM table_0".into();
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_0".into(),
        pk_cols: vec!["field_0".into()],
    });
    app.tab_mut().edits.pending_source = None;
    assert!(app.tab().result.is_none());

    app.reload_data_tab_if_needed(0);
    // The result that this reload installs promotes `pending_source` to `source`.
    let promoted = app.tab().edits.pending_source.clone();
    assert!(promoted.is_some_and(|source| source.editable()));
}

#[test]
fn independent_queries_finish_out_of_order_without_stealing_results() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.history_enabled = false;
    app.audit_enabled = false;
    let first = app.tab().id;
    let (first_seq, first_cancel) = app.begin_query_job(first);
    app.new_tab();
    let second = app.tab().id;
    assert!(app.query_can_run(1));
    let (second_seq, second_cancel) = app.begin_query_job(second);
    assert!(!first_cancel.is_cancelled());
    assert!(!app.query_can_run(0));
    for (tab_id, seq, value) in [(second, second_seq, 22), (first, first_seq, 11)] {
        app.tx
            .send(AppMessage::Queried {
                tab_id,
                conn_id: String::new(),
                sql: "SELECT value".into(),
                result: Ok(QueryResult {
                    columns: vec![ColumnMeta {
                        name: "value".into(),
                        type_name: "INTEGER".into(),
                    }],
                    rows: vec![vec![Value::Int(value)]],
                    ..QueryResult::default()
                }),
                canceled: false,
                seq,
            })
            .unwrap();
        app.poll_messages(&egui::Context::default());
        if tab_id == second {
            assert!(app.is_tab_querying(first));
            assert_eq!(app.busy, Busy::Querying);
        }
    }
    assert_eq!(
        app.tabs[0].result.as_ref().unwrap().rows[0][0],
        Value::Int(11)
    );
    assert_eq!(
        app.tabs[1].result.as_ref().unwrap().rows[0][0],
        Value::Int(22)
    );
    assert!(!second_cancel.is_cancelled());
    assert_eq!(app.busy, Busy::Idle);
}

#[test]
fn cancel_and_disconnect_leave_other_queries_running() {
    let mut app = app_with_staged_edit();
    app.tab_mut().edits.clear();
    let first = app.tab().id;
    let (_, first_cancel) = app.begin_query_job(first);
    app.new_tab();
    app.tab_mut().conn_id = None;
    let second = app.tab().id;
    let (_, second_cancel) = app.begin_query_job(second);
    app.apply_action(Action::DisconnectConn(0));
    assert!(first_cancel.is_cancelled());
    assert!(!second_cancel.is_cancelled());
    assert!(app.is_tab_querying(second));
    assert_eq!(app.busy, Busy::Querying);
    app.apply_action(Action::CancelTabQuery(second));
    assert!(second_cancel.is_cancelled());
    assert_eq!(app.busy, Busy::Idle);
}

#[test]
fn invalid_connection_save_keeps_the_form_and_credentials() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.apply_action(Action::NewConnection);
    let editor = app.editor.as_mut().unwrap();
    editor.config.host.clear();
    editor.password = "keep this draft".into();
    app.apply_action(Action::SaveAndConnect);
    let editor = app.editor.as_ref().expect("invalid form stays open");
    assert_eq!(editor.password, "keep this draft");
    assert!(matches!(editor.test_state, ConnTestState::Failed { .. }));
    assert!(app.connections.is_empty());
}

#[test]
fn replacement_result_keeps_edits_created_while_query_was_running() {
    let mut app = app_with_staged_edit();
    app.history_enabled = false;
    app.audit_enabled = false;
    let tab_id = app.tab().id;
    let (seq, _) = app.begin_query_job(tab_id);
    app.tx
        .send(AppMessage::Queried {
            tab_id,
            conn_id: "edit-connection".into(),
            sql: "SELECT * FROM items".into(),
            result: Ok(fake_result(3, 1)),
            canceled: false,
            seq,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert!(app.tab().edits.has_pending());
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 1);
    assert_eq!(app.tab().result.as_ref().unwrap().rows[0][0], Value::Int(1));
    assert!(app.error.as_deref().unwrap().contains("unsaved edits"));
}

#[test]
fn concurrent_streams_share_the_materialized_result_budget() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    let first = app.tab().id;
    app.tabs[0].set_result(fake_result(20, 2));
    app.new_tab();
    let second = app.tab().id;
    let (seq, cancel) = app.begin_query_job(second);
    app.result_memory_budget = app.total_result_memory_bytes() + 1;
    app.tx
        .send(AppMessage::QueryStreamStarted {
            tab_id: second,
            columns: vec![ColumnMeta {
                name: "value".into(),
                type_name: "TEXT".into(),
            }],
            append: false,
            seq,
        })
        .unwrap();
    app.tx
        .send(AppMessage::QueryRows {
            tab_id: second,
            rows: vec![vec![Value::Text("large row".into())]],
            seq,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert!(cancel.is_cancelled());
    assert!(!app.is_tab_querying(second));
    assert!(app
        .tabs
        .iter()
        .find(|t| t.id == first)
        .unwrap()
        .result
        .is_some());
    assert!(app
        .tab()
        .query_error
        .as_deref()
        .unwrap()
        .contains("memory limit"));
}

pub(super) fn app_with_staged_edit() -> DbGuiApp {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.active_connections.clear();
    let mut config = dbcore::ConnectionConfig::new(DbKind::Sqlite);
    config.id = "edit-connection".into();
    app.connections.push(config);
    app.active_connections.push(ActiveConnection {
        config_id: "edit-connection".into(),
        name: "edits".into(),
        db: Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    app.tab_mut().conn_id = Some("edit-connection".into());
    app.tab_mut().set_result(QueryResult {
        columns: vec![ColumnMeta {
            name: "id".into(),
            type_name: "INTEGER".into(),
        }],
        rows: vec![vec![Value::Int(1)]],
        ..QueryResult::default()
    });
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "items".into(),
        pk_cols: vec!["id".into()],
    });
    app.tab_mut()
        .edits
        .cells
        .insert(0, HashMap::from([(0, Value::Int(2))]));
    app
}

#[test]
fn edit_preview_commits_to_original_tab_after_selection_changes() {
    let mut app = app_with_staged_edit();
    app.apply_action(Action::PreviewEdits);
    app.apply_action(Action::NewTab);
    assert_eq!(app.active_query_tab, 1);
    app.apply_action(Action::ConfirmEdits);
    assert_eq!(app.active_query_tab, 0);
    assert_eq!(app.busy, Busy::Saving);
    assert!(app.commit_pending.is_none());
}

/// Column constraints come from the table's schema once it's known, and a new row missing
/// a required value is stopped before any SQL is planned — naming the cell.
#[test]
fn save_stops_on_a_missing_required_value() {
    let mut app = app_with_staged_edit();
    app.active_connections[0].schema.tables = vec![TableInfo {
        schema: None,
        name: "items".into(),
        columns: vec![
            col("id", "INTEGER", false, true),
            col("name", "TEXT", false, false),
        ],
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
    }];
    app.tab_mut().set_result(QueryResult {
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
        ..QueryResult::default()
    });
    app.sync_edit_rules();
    let rule = app.tab().edits.rule(1).cloned().expect("rules synced");
    assert!(rule.not_null && rule.required);

    let new = app.tab_mut().edits.add_new_row();
    // The fixture already stages the stored row's id 1 → 2, so the new row takes 3.
    app.tab_mut()
        .edits
        .stage(new, 0, Value::Int(3), &Value::Null);
    app.apply_action(Action::PreviewEdits);
    assert!(app.commit_pending.is_none(), "nothing planned");
    assert!(
        app.error
            .as_deref()
            .unwrap_or("")
            .contains("\"name\" is required"),
        "{:?}",
        app.error
    );

    app.tab_mut()
        .edits
        .stage(new, 1, Value::Text("b".into()), &Value::Null);
    app.error = None;
    app.apply_action(Action::PreviewEdits);
    assert!(
        app.commit_pending.is_some(),
        "saves once filled: {:?} {}",
        app.error,
        app.status_msg
    );
}

#[test]
fn save_keeps_the_preview_by_default() {
    let mut app = app_with_staged_edit();
    app.apply_action(Action::PreviewEdits);
    assert!(app.commit_pending.is_some(), "preview shown");
    assert_eq!(app.busy, Busy::Idle, "nothing written yet");
}

#[test]
fn save_skips_the_preview_when_review_is_off() {
    let mut app = app_with_staged_edit();
    app.review_edits_before_save = false;
    app.apply_action(Action::PreviewEdits);
    assert!(app.commit_pending.is_none(), "no preview left open");
    assert_eq!(app.busy, Busy::Saving, "saved straight away");
}

/// Production Guardian still confirms even with review turned off.
#[test]
fn save_without_review_still_guards_production() {
    let mut app = app_with_staged_edit();
    app.connections[0].production = true;
    app.review_edits_before_save = false;
    app.apply_action(Action::PreviewEdits);
    assert!(app.danger_pending.is_some(), "Guardian dialog opened");
    assert_eq!(app.busy, Busy::Idle, "nothing written before confirmation");
}

#[test]
fn edit_preview_rejects_reloaded_result_even_with_identical_sql() {
    let mut app = app_with_staged_edit();
    app.commit_edits();
    let result = app.tab().result.clone().unwrap();
    app.tab_mut().set_result(result);
    app.confirm_edits();
    assert_eq!(app.busy, Busy::Idle);
    assert!(app.commit_pending.is_none());
    assert!(app.error.as_deref().unwrap().contains("changed"));
    assert!(app.tab().edits.has_pending());
}

#[test]
fn edit_preview_rejects_reconnected_database_with_same_config_id() {
    let mut app = app_with_staged_edit();
    app.commit_edits();
    app.active_connections[0].db = Arc::new(DummyDb);
    app.confirm_edits();
    assert_eq!(app.busy, Busy::Idle);
    assert!(app.commit_pending.is_none());
    assert!(app.error.as_deref().unwrap().contains("connection changed"));
}

#[test]
fn edit_preview_rejects_changed_staging_and_read_only_policy() {
    let mut app = app_with_staged_edit();
    app.commit_edits();
    app.tab_mut()
        .edits
        .cells
        .get_mut(&0)
        .unwrap()
        .insert(0, Value::Int(3));
    app.confirm_edits();
    assert_eq!(app.busy, Busy::Idle);
    assert!(app
        .error
        .as_deref()
        .unwrap()
        .contains("staged edits changed"));
    assert!(app.tab().edits.has_pending());

    app.commit_edits();
    app.connections[0].read_only = true;
    app.confirm_edits();
    assert_eq!(app.busy, Busy::Idle);
    assert!(app.commit_pending.is_none());
    assert!(app.error.as_deref().unwrap().contains("read-only"));
}

#[test]
fn edit_preview_rejects_closed_or_rebound_tab() {
    let mut app = app_with_staged_edit();
    app.commit_edits();
    app.tab_mut().conn_id = None;
    app.confirm_edits();
    assert_eq!(app.busy, Busy::Idle);
    assert!(app.commit_pending.is_none());

    let mut app = app_with_staged_edit();
    app.commit_edits();
    app.tabs.clear();
    app.confirm_edits();
    assert_eq!(app.busy, Busy::Idle);
    assert!(app.commit_pending.is_none());
}

#[test]
fn edit_preview_cannot_be_reused_after_invalid_or_empty_save() {
    let mut app = app_with_staged_edit();
    app.commit_edits();
    assert!(app.commit_pending.is_some());
    app.tab_mut().edits.cells.clear();
    app.commit_edits();
    assert!(app.commit_pending.is_none());
    app.tab_mut().edits.new_rows = 1;
    app.commit_edits();
    assert!(
        app.commit_pending.is_none(),
        "untouched new rows do not open an empty preview"
    );
}

#[test]
fn edit_preview_confirmation_while_busy_keeps_plan_without_executing() {
    let mut app = app_with_staged_edit();
    app.commit_edits();
    app.busy = Busy::Querying;
    app.confirm_edits();
    assert!(app.commit_pending.is_some());
}

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

struct ReconnectMetadataDb;

#[async_trait::async_trait]
impl dbcore::Database for ReconnectMetadataDb {
    fn kind(&self) -> dbcore::DbKind {
        dbcore::DbKind::Sqlite
    }

    async fn introspect(&self) -> dbcore::Result<SchemaTree> {
        Ok(fake_schema(1, 2))
    }

    async fn introspect_table(
        &self,
        _schema: Option<&str>,
        _table: &str,
    ) -> dbcore::Result<Option<TableInfo>> {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok(fake_schema(1, 2).tables.into_iter().next())
    }

    async fn execute_capped(&self, _sql: &str, _max_rows: usize) -> dbcore::Result<QueryResult> {
        Ok(QueryResult::default())
    }

    async fn execute_transaction(&self, stmts: &[String]) -> dbcore::Result<usize> {
        Ok(stmts.len())
    }

    async fn export_query(
        &self,
        _sql: &str,
        sink: &mut (dyn dbcore::RowSink + Send),
    ) -> dbcore::Result<u64> {
        sink.begin(&[ColumnMeta {
            name: "field_0".into(),
            type_name: "TEXT".into(),
        }])?;
        sink.write_row(&[Value::Text("loaded".into())])?;
        sink.finish()?;
        Ok(1)
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
                        generated: false,
                        max_length: None,
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
fn new_connection_name_follows_the_selected_provider() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::NewConnection);
    let editor = app.editor.as_mut().unwrap();
    assert_eq!(editor.config.name, "New PostgreSQL");

    // Start with the reported SQLite case, then go back through Change for every provider.
    for kind in [
        DbKind::Sqlite,
        DbKind::DuckDb,
        DbKind::MySql,
        DbKind::MariaDb,
        DbKind::SqlServer,
        DbKind::Cassandra,
        DbKind::ScyllaDb,
        DbKind::Postgres,
    ] {
        editor.selecting_provider = true;
        editor.select_provider(kind);
        assert_eq!(editor.config.name, format!("New {}", kind.label()));
        assert_eq!(editor.config.kind, kind);
        assert_eq!(editor.config.port, kind.default_port());
        assert!(!editor.selecting_provider);
    }
}

#[test]
fn changing_provider_preserves_a_custom_connection_name() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::NewConnection);
    let editor = app.editor.as_mut().unwrap();
    editor.config.name = "Local database".into();
    editor.select_provider(DbKind::Sqlite);
    editor.select_provider(DbKind::MySql);
    assert_eq!(editor.config.name, "Local database");
}

#[test]
fn changing_provider_preserves_a_saved_connection_name() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::NewConnection);
    let editor = app.editor.as_mut().unwrap();
    // A saved name belongs to the user even if it matches an automatically generated name.
    editor.is_new = false;
    editor.select_provider(DbKind::Sqlite);
    assert_eq!(editor.config.name, "New PostgreSQL");
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
    app.query_jobs.clear();

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
    app.query_jobs.clear();
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
    app.query_jobs.clear();
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
    assert_eq!(app.busy, Busy::Saving);
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
    app.query_jobs.clear();

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
        generated: false,
        max_length: None,
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
        app.busy = Busy::Idle;
        app.query_jobs.clear(); // each page flip leaves a query in flight
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
    schema.tables[0].indexes.clear(); // and no unique index fallback
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
    app.query_jobs.clear();
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

/// With a single row selected, Cmd/Ctrl+C copies just the value under the cell cursor —
/// unflattened, so a multi-line value keeps its newlines.
#[test]
fn copy_event_on_one_cell_copies_only_its_value() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    let result = QueryResult {
        columns: vec![
            ColumnMeta {
                name: "code".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "note".into(),
                type_name: "TEXT".into(),
            },
        ],
        rows: vec![
            vec![Value::Text("000038".into()), Value::Text("line 1\nline 2".into())],
            vec![Value::Text("000065".into()), Value::Null],
        ],
        stats: QueryStats::default(),
        truncated: false,
    };
    app.tab_mut().set_result(result);
    app.tab_mut().selection.select_one(0);
    app.tab_mut().selection.set_cursor(0, 0);

    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
    let copy = |app: &mut DbGuiApp| {
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| app.draw(ui, None));
        out.platform_output.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
    };
    assert_eq!(copy(&mut app).as_deref(), Some("000038"));

    app.tab_mut().selection.set_cursor(0, 1);
    assert_eq!(copy(&mut app).as_deref(), Some("line 1\nline 2"));
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

    app.apply_action(Action::FilterColumn {
        tab_id: app.tab().id,
        col: 2,
    });

    assert!(app.tab().filter.visible);
    assert_eq!(app.tab().filter.conditions.len(), 1);
    assert_eq!(app.tab().filter.conditions[0].column, 2);
}

#[test]
fn toggle_filter_requires_a_result_and_cmd_f_flips_it() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::ToggleFilter(app.tab().id));
    assert!(
        !app.tab().filter.visible,
        "no result means there is nothing to filter"
    );

    let (ctx, mut app) = grid_nav_app(3, 3);
    app.apply_action(Action::ToggleFilter(app.tab().id));
    assert!(app.tab().filter.visible);
    app.apply_action(Action::ToggleFilter(app.tab().id));
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

#[test]
fn split_filter_actions_stay_with_the_originating_pane() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().set_result(fake_result(3, 3));
    let left_id = app.tab().id;
    app.new_tab();
    app.tab_mut().set_result(fake_result(3, 3));
    let right = app.active_query_tab;
    let right_id = app.tab().id;
    app.tabs[right].pane = 1;
    app.split_panes = vec![right];
    app.reset_split_ratios();
    app.active_query_tab = 0;

    app.apply_action(Action::ToggleFilter(right_id));
    app.apply_action(Action::FilterColumn {
        tab_id: right_id,
        col: 2,
    });

    let left = app.tabs.iter().find(|tab| tab.id == left_id).unwrap();
    let right = app.tabs.iter().find(|tab| tab.id == right_id).unwrap();
    assert!(!left.filter.visible, "the left pane must remain unchanged");
    assert!(
        right.filter.visible,
        "the right pane must own its filter bar"
    );
    assert_eq!(right.filter.conditions[0].column, 2);
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

    assert!(app.is_tab_querying(app.tab().id));
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

fn has_painted_text_near(
    shapes: &[egui::epaint::ClippedShape],
    needle: &str,
    point: egui::Pos2,
) -> bool {
    fn walk(shape: &egui::epaint::Shape, needle: &str, point: egui::Pos2) -> bool {
        match shape {
            egui::epaint::Shape::Text(text) => {
                text.galley.text().contains(needle) && text.pos.distance(point) < 120.0
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().any(|shape| walk(shape, needle, point))
            }
            _ => false,
        }
    }

    shapes.iter().any(|shape| walk(&shape.shape, needle, point))
}

fn has_filled_rect_at(
    shapes: &[egui::epaint::ClippedShape],
    point: egui::Pos2,
    fill: egui::Color32,
) -> bool {
    fn walk(shape: &egui::epaint::Shape, point: egui::Pos2, fill: egui::Color32) -> bool {
        match shape {
            egui::epaint::Shape::Rect(rect) => rect.rect.contains(point) && rect.fill == fill,
            egui::epaint::Shape::Vec(shapes) => shapes.iter().any(|shape| walk(shape, point, fill)),
            _ => false,
        }
    }

    shapes.iter().any(|shape| walk(&shape.shape, point, fill))
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
    let dragged = run(&mut app, vec![egui::Event::PointerMoved(drop)]);
    assert!(
        has_painted_text_near(&dragged.shapes, "Query 2", drop),
        "the dragged tab must follow the pointer into the split target"
    );
    let active_drop_fill = crate::style::palette::ACCENT().gamma_multiply(0.22);
    assert!(
        has_filled_rect_at(&dragged.shapes, drop, active_drop_fill),
        "the split target must remain visible beneath the dragged tab"
    );
    let next_frame = run(&mut app, vec![egui::Event::PointerMoved(drop)]);
    assert!(
        has_filled_rect_at(&next_frame.shapes, drop, active_drop_fill),
        "the split target must not flicker while the pointer stays still"
    );
    run(
        &mut app,
        vec![egui::Event::PointerButton {
            pos: drop,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
    );

    let split = app.split_panes.first().copied().expect("drop did not create a split pane");
    assert_eq!(app.tabs.len(), 2, "dragged tab should move, not duplicate");
    assert_eq!(app.tabs[split].sql, "SELECT 'right'");
    assert_eq!(app.tabs[app.active_query_tab].sql, "SELECT 'left'");
}

#[test]
fn dragging_a_schema_table_paints_its_name_beside_the_pointer() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    connect_fake(&mut app, fake_schema(2, 3));

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
    let table = find_text_pos(&out.shapes, "table_1").expect("schema table not painted");
    let start = table + egui::vec2(4.0, 6.0);
    let pointer = egui::pos2(820.0, 360.0);

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
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        run(
            &mut app,
            vec![egui::Event::PointerMoved(start + (pointer - start) * t)],
        );
    }
    let dragged = run(&mut app, vec![egui::Event::PointerMoved(pointer)]);
    assert!(
        has_painted_text_near(&dragged.shapes, "table_1", pointer),
        "the table drag ghost must show its name beside the pointer"
    );
}

#[test]
fn dropping_a_schema_table_into_an_open_split_adds_a_right_tab() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    connect_fake(&mut app, fake_schema(2, 3));
    app.open_split_workspace();
    assert_eq!(app.tabs.iter().filter(|tab| tab.pane > 0).count(), 1);

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
    let table = find_text_pos(&out.shapes, "table_1").expect("schema table not painted");
    let start = table + egui::vec2(4.0, 6.0);
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
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        run(
            &mut app,
            vec![egui::Event::PointerMoved(start + (drop - start) * t)],
        );
    }
    run(
        &mut app,
        vec![egui::Event::PointerButton {
            pos: drop,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
    );

    assert_eq!(app.tabs.iter().filter(|tab| tab.pane > 0).count(), 2);
    assert_eq!(app.tabs[app.split_panes[0]].title, "table_1");
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
    }, 1);

    let split = app.split_panes.first().copied().expect("table drop did not create a split");
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
    // Numbering is per connection: the first untitled tab on "ms" is Query 1.
    assert_eq!(app.tab_label(1), "MS Query 1");
    app.new_tab();
    assert_eq!(app.tabs[2].conn_id.as_deref(), Some("ms"));
    assert_eq!(app.tab_label(2), "MS Query 2");
    app.active_query_tab = 1;

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
fn restored_workspace_keeps_each_connections_selected_tab() {
    use egui_kittest::kittest::Queryable;

    let (mut app, other) = app_with_two_connections();
    app.tab_mut().edits = Default::default();
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().title = "a_selected".into();
    app.new_tab();
    app.tab_mut().title = "a_other".into();
    app.select_tab(0);
    app.bind_connection(other, false);
    app.tab_mut().title = "b_selected".into();
    let encoded = serde_json::to_vec(&app.snapshot_workspace()).unwrap();
    let saved = serde_json::from_slice(&encoded).unwrap();

    // Startup restores bindings and selection even while every connection is offline.
    app.active_connections.clear();
    app.restore_workspace_from(saved);
    assert_eq!(app.tab().title, "b_selected");
    assert!(!app.tab_in_current_connection(0));
    assert!(!app.tab_in_current_connection(1));
    assert!(app.tab_in_current_connection(2));
    app.switch_to_connection_tabs(Some("other-connection".into()), "edit-connection");
    assert_eq!(
        app.tab().title,
        "a_selected",
        "remember the selected tab, not the last array entry"
    );
    assert!(app.tab_in_current_connection(0));
    assert!(app.tab_in_current_connection(1));
    assert!(!app.tab_in_current_connection(2));

    app.show_welcome = false;
    let mut harness = egui_kittest::Harness::builder().build_ui(move |ui| {
        egui_extras::install_image_loaders(ui.ctx());
        app.query_tab_bar(ui, &mut Vec::new());
    });
    harness.run_steps(3);
    harness.get_by_label("a_selected");
    harness.get_by_label("a_other");
    assert!(harness.query_by_label("b_selected").is_none());
}

#[test]
fn restored_split_tab_strips_filter_by_their_own_connection() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.tabs.clear();
    for (id, title, conn, pane) in [
        (0, "a_main", "a", 0),
        (1, "b_main", "b", 0),
        (2, "a_side", "a", 1),
        (3, "b_side", "b", 1),
    ] {
        let mut tab = QueryTab::new(id, title.into());
        tab.conn_id = Some(conn.into());
        tab.pane = pane;
        app.tabs.push(tab);
    }
    app.active_query_tab = 1;
    app.split_panes = vec![2];
    let encoded = serde_json::to_vec(&app.snapshot_workspace()).unwrap();
    app.restore_workspace_from(serde_json::from_slice(&encoded).unwrap());
    assert_eq!(app.tabs[app.active_query_tab].conn_id.as_deref(), Some("b"));
    assert_eq!(app.tabs[app.split_panes[0]].conn_id.as_deref(), Some("a"));
    app.close_split_pane_tab(1, 0);
    assert_eq!(
        app.tab().title,
        "b_main",
        "the last visible main tab must not switch connections"
    );
    let mut harness = egui_kittest::Harness::builder().build_ui(move |ui| {
        egui_extras::install_image_loaders(ui.ctx());
        let mut actions = Vec::new();
        app.split_pane_tab_bar(ui, app.active_query_tab, 0, &mut actions);
        app.split_pane_tab_bar(ui, app.split_panes[0], 1, &mut actions);
    });
    harness.run_steps(3);
    harness.get_by_label("b_main");
    harness.get_by_label("a_side");
    assert!(harness.query_by_label("a_main").is_none());
    assert!(harness.query_by_label("b_side").is_none());
}

#[test]
fn restored_legacy_workspace_keeps_connection_bindings_and_ignores_invalid_selections() {
    let saved: dbcore::config::Workspace = serde_json::from_value(serde_json::json!({
        "active_tab": 1,
        "connection_active_tabs": {"a": 1, "b": 999},
        "tabs": [
            {"title": "a_tab", "conn_id": "a", "sql": "SELECT 1"},
            {"title": "b_tab", "conn_id": "b", "sql": "SELECT 2"},
            {"title": "unbound", "sql": "SELECT 3"}
        ]
    }))
    .unwrap();
    let mut app = DbGuiApp::construct();
    app.restore_workspace_from(saved);
    assert_eq!(app.tab().title, "b_tab");
    assert!(!app.tab_in_current_connection(0));
    assert!(!app.tab_in_current_connection(2));
    app.switch_to_connection_tabs(Some("b".into()), "a");
    assert_eq!(app.tab().title, "a_tab");
    assert_eq!(app.tab().conn_id.as_deref(), Some("a"));
    assert_eq!(app.tabs[2].conn_id, None);
}

#[test]
fn workspace_snapshot_keeps_tab_kind_and_editor_size() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().title = "active_users".into();
    app.tab_mut().kind = crate::components::QueryTabKind::View;
    app.tab_mut().editor_size = Some(212.0);

    let saved = app.snapshot_workspace();
    let json = serde_json::to_vec(&saved).unwrap();
    let saved: dbcore::config::Workspace = serde_json::from_slice(&json).unwrap();
    assert_eq!(saved.tabs.len(), 1);
    assert_eq!(saved.tabs[0].title, "active_users");
    assert_eq!(
        saved.tabs[0].kind,
        Some(dbcore::config::WorkspaceTabKind::View)
    );
    assert_eq!(saved.tabs[0].editor_size, Some(212.0));
}

#[test]
fn workspace_snapshot_keeps_every_right_split_tab() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().title = "left".into();
    app.open_split_workspace();
    let first = app.split_panes[0];
    app.tabs[first].title = "right_one".into();
    app.new_tab_in_split_pane(1);
    let second = app.split_panes[0];
    app.tabs[second].title = "right_two".into();

    let saved = app.snapshot_workspace();
    let json = serde_json::to_vec(&saved).unwrap();
    let saved: dbcore::config::Workspace = serde_json::from_slice(&json).unwrap();

    assert_eq!(saved.tabs.len(), 3);
    assert_eq!(saved.active_tab, 0);
    assert_eq!(saved.active_split_tab, Some(2));
    let right_titles: Vec<&str> = saved
        .tabs
        .iter()
        .filter(|tab| tab.split_pane)
        .map(|tab| tab.title.as_str())
        .collect();
    assert_eq!(right_titles, ["right_one", "right_two"]);
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
fn run_current_uses_the_nearest_statement_when_the_caret_is_just_after_a_separator() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().sql = "SELECT 1;\nSELECT 2;".into();
    let end = app.tab().sql.chars().count();
    app.tab_mut().primary_cursor = end..end;

    assert_eq!(
        app.resolved_current_sql_snapshot_for(0).unwrap().trim(),
        "SELECT 2"
    );
}

#[test]
fn closing_split_repairs_an_active_hidden_pane_index() {
    let mut app = DbGuiApp::construct();
    app.tabs[0].editor_split = true;
    let split = QueryTab::new(app.next_tab_id, "right".into());
    app.next_tab_id += 1;
    app.tabs.push(split);
    app.tabs[1].pane = 1;
    app.split_panes = vec![1];
    app.reset_split_ratios();
    // Reproduces the crash: UI rendering temporarily left the hidden pane active when Close
    // removed index 1, leaving active_query_tab == len.
    app.active_query_tab = 1;

    app.close_split_workspace();

    assert_eq!(app.tabs.len(), 2, "collapsing a split keeps its tabs");
    assert_eq!(app.active_query_tab, 0);
    assert!(!app.is_split());
    assert!(!app.tabs[0].editor_split);
    assert_eq!(app.tab().id, 0);
}

/// The syntax squiggle follows the tab's own connection: with none, SQL any dialect
/// accepts (T-SQL variables here) is left alone; once the tab runs on SQLite — which has no
/// DECLARE — the unchanged text is re-checked against that dialect and flagged.
#[test]
fn syntax_check_uses_the_tabs_connection_and_rechecks_when_it_changes() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    let tab = app.tab_mut();
    tab.kind = crate::components::QueryTabKind::Query;
    tab.conn_id = None;
    tab.sql = "DECLARE @id INT = 5;\nSELECT @id;".into();
    tab.mark_sql_changed();

    let mut time = 0.0;
    let mut frame = |app: &mut DbGuiApp| {
        time += 0.5;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    for _ in 0..3 {
        frame(&mut app);
    }
    assert_eq!(
        app.tab().editor_assist.syntax_checked,
        app.tab().sql,
        "the check ran"
    );
    assert!(app.tab().editor_assist.syntax_error.is_none());

    app.tab_mut().conn_id = Some("edit-connection".into());
    for _ in 0..3 {
        frame(&mut app);
    }
    assert_eq!(
        app.tab().editor_assist.syntax_checked_kind,
        Some(DbKind::Sqlite)
    );
    assert!(app.tab().editor_assist.syntax_error.is_some());
}

/// Unknown tables/columns are marked from the tab's own connection schema, and a schema
/// refresh re-checks text that hasn't changed (a table created elsewhere stops being flagged).
#[test]
fn semantic_check_marks_unknown_names_and_rechecks_when_the_schema_changes() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    let tab = app.tab_mut();
    tab.kind = crate::components::QueryTabKind::Query;
    tab.sql = "SELECT field_9 FROM table_0\n;\nSELECT * FROM table_9".into();
    tab.mark_sql_changed();

    let mut time = 0.0;
    let mut frame = |app: &mut DbGuiApp| {
        time += 0.5;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    for _ in 0..3 {
        frame(&mut app);
    }
    let assist = &app.tab().editor_assist;
    assert!(assist.syntax_error.is_none());
    assert_eq!(
        assist.semantic_issues.len(),
        2,
        "{:?}",
        assist.semantic_issues
    );

    // The schema now has a `table_9`: same text, new verdict.
    let mut schema = fake_schema(10, 1);
    let mut extra = schema.tables[0].columns[0].clone();
    extra.name = "field_9".into();
    schema.tables[0].columns.push(extra);
    app.active_connections[0].schema = schema;
    for _ in 0..3 {
        frame(&mut app);
    }
    assert!(app.tab().editor_assist.semantic_issues.is_empty());
}

/// Resting the pointer over the editor text exercises the symbol-hover path (hit-testing the
/// galley, mapping through the fold view) on every position without panicking.
#[test]
fn pointer_sweep_over_the_editor_with_a_schema_does_not_panic() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    let tab = app.tab_mut();
    tab.kind = crate::components::QueryTabKind::Query;
    tab.sql = "SELECT t.field_0, \u{e01}\u{e02} FROM table_0 t\nWHERE field_0 = 1".into();
    tab.mark_sql_changed();
    let mut time = 0.0;
    for step in 0..40 {
        time += 0.1;
        let pos = egui::pos2(150.0 + step as f32 * 12.0, 120.0 + (step % 4) as f32 * 14.0);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            time: Some(time),
            events: vec![egui::Event::PointerMoved(pos)],
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    }
}

/// New Table opens as the dense grid, and each of its sections (columns, indexes, foreign
/// keys) renders and takes input — it must never trap the user on one of them.
#[test]
fn new_table_grid_renders_every_section_and_edits_columns() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.apply_action(Action::OpenNewTable);
    app.apply_action(Action::AddSchemaColumn);
    app.apply_action(Action::AddSchemaIndex);

    let mut time = 0.0;
    let mut frame = |app: &mut DbGuiApp| {
        time += 0.1;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    for tab in [
        crate::schema::SchemaTab::Columns,
        crate::schema::SchemaTab::Indexes,
        crate::schema::SchemaTab::ForeignKeys,
        crate::schema::SchemaTab::Columns,
    ] {
        let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut()
        else {
            panic!("New Table did not open an editor");
        };
        assert_eq!(editor.mode, crate::schema::SchemaEditorMode::New);
        editor.active_tab = tab;
        for _ in 0..3 {
            frame(&mut app);
        }
    }
    let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab().schema_editor.as_ref() else {
        panic!("editor closed while rendering");
    };
    assert_eq!(editor.columns.len(), 2, "starter column + AddSchemaColumn");
    assert_eq!(editor.indexes.len(), 1);
}

/// New View opens with a free suggested name (selected, so typing replaces it) over a
/// full-height query editor, and keeps working as the query grows past one line.
#[test]
fn new_view_opens_with_a_suggested_name_and_renders_the_editor() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.apply_action(Action::OpenNewView);
    let Some(crate::schema::ObjectEditor::View(editor)) = app.tab_mut().schema_editor.as_mut()
    else {
        panic!("New View did not open an editor");
    };
    assert_eq!(editor.name, "untitled_view_1");
    // The body is written in the tab's SQL editor; the view editor follows it.
    app.tab_mut().sql = "SELECT 1\nFROM table_0\nWHERE x = 'ก'".into();
    app.tab_mut().mark_sql_changed();

    let mut time = 0.0;
    for _ in 0..4 {
        time += 0.1;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    }
    let Some(crate::schema::ObjectEditor::View(editor)) = app.tab().schema_editor.as_ref() else {
        panic!("editor closed while rendering");
    };
    assert_eq!(editor.name, "untitled_view_1", "rendering must not edit it");
    assert_eq!(editor.select_body.lines().count(), 3);
}

/// A new table starts with an `id` integer primary key, and double-clicking the blank space
/// under the grid adds a column (or an index, on the Indexes section).
#[test]
fn new_table_starts_with_an_id_key_and_double_click_adds_a_column() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.apply_action(Action::OpenNewTable);
    {
        let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab().schema_editor.as_ref()
        else {
            panic!("New Table did not open an editor");
        };
        assert_eq!(editor.columns.len(), 1);
        let id = &editor.columns[0];
        assert_eq!(id.name, "id");
        assert!(id.data_type.eq_ignore_ascii_case("integer"));
        assert!(id.primary_key && !id.nullable);
    }

    let mut time = 0.0;
    let mut frame = |app: &mut DbGuiApp, events: Vec<egui::Event>| {
        // Idle frames let the double-click window lapse, so the second gesture below is a
        // fresh double-click rather than the third and fourth click of the first.
        time += if events.is_empty() { 0.3 } else { 0.01 };
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    let columns = |app: &DbGuiApp| match app.tab().schema_editor.as_ref() {
        Some(crate::schema::ObjectEditor::Table(editor)) => editor.columns.len(),
        _ => panic!("editor closed"),
    };
    let indexes = |app: &DbGuiApp| match app.tab().schema_editor.as_ref() {
        Some(crate::schema::ObjectEditor::Table(editor)) => editor.indexes.len(),
        _ => panic!("editor closed"),
    };
    for _ in 0..4 {
        frame(&mut app, Vec::new());
    }
    let pos = egui::pos2(600.0, 450.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let double_click =
        |app: &mut DbGuiApp, frame: &mut dyn FnMut(&mut DbGuiApp, Vec<egui::Event>)| {
            frame(app, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false, true, false] {
                frame(app, vec![button(pressed)]);
            }
            frame(app, Vec::new());
            frame(app, Vec::new());
        };

    double_click(&mut app, &mut frame);
    assert_eq!(columns(&app), 2, "blank space under the grid adds a column");

    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.active_tab = crate::schema::SchemaTab::Indexes;
    }
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    double_click(&mut app, &mut frame);
    assert_eq!(indexes(&app), 1, "…and on the Indexes section, an index");
    assert_eq!(columns(&app), 2);
}

/// The object editors have no Apply / Cancel buttons: Cmd/Ctrl+S applies the DDL and Esc
/// leaves, for New Table, New View and New Trigger alike.
#[test]
fn object_editors_apply_with_the_save_shortcut_and_leave_with_escape() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut time = 0.0;
    let mut frame = |app: &mut DbGuiApp, events: Vec<egui::Event>| {
        time += 0.1;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            time: Some(time),
            // egui reads the held modifiers from the frame's input, not from the key event.
            modifiers: events
                .iter()
                .find_map(|event| match event {
                    egui::Event::Key { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default(),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    };

    for (name, open) in [
        ("New Table", Action::OpenNewTable),
        ("New View", Action::OpenNewView),
        ("New Trigger", Action::OpenNewTrigger),
    ] {
        let mut app = app_with_staged_edit();
        app.show_welcome = false;
        app.apply_action(open);
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        assert!(app.tab().schema_editor.is_some(), "{name}: editor opened");

        // Esc, with nothing focused, leaves.
        ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        frame(&mut app, Vec::new());
        assert!(app.tab().schema_editor.is_none(), "{name}: Esc closes it");
    }

    // Cmd/Ctrl+S applies the DDL. A read-only connection refuses it, which is how the test sees
    // that the shortcut reached the apply path without running anything against the database.
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.connections[0].read_only = true;
    app.apply_action(Action::OpenNewTable);
    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.table_name = "customers".into();
    }
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    assert!(app.error.is_none());
    frame(&mut app, vec![key(egui::Key::S, egui::Modifiers::COMMAND)]);
    frame(&mut app, Vec::new());
    assert!(
        app.error
            .as_deref()
            .is_some_and(|e| e.contains("read-only")),
        "the shortcut must reach the apply path, got {:?}",
        app.error
    );
    assert!(
        app.tab().schema_editor.is_some(),
        "a refused apply keeps the draft"
    );
}

/// Reload (Cmd/Ctrl+R) with unsaved work asks "Discard all changes?" first — for staged row
/// edits and for a New Table / View / Trigger draft alike — and never runs the query that is
/// hidden behind a draft.
#[test]
fn reload_with_unsaved_work_asks_before_discarding() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut time = 0.0;
    let mut frame = |app: &mut DbGuiApp, key: Option<(egui::Key, egui::Modifiers)>| {
        time += 0.1;
        let (events, modifiers) = match key {
            Some((key, modifiers)) => (
                vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
            ),
            None => (Vec::new(), egui::Modifiers::NONE),
        };
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            time: Some(time),
            modifiers,
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    let reload = Some((egui::Key::R, egui::Modifiers::COMMAND));
    let clean_query_tab = || {
        let mut app = app_with_staged_edit();
        app.show_welcome = false;
        app.tab_mut().kind = crate::components::QueryTabKind::Query;
        app.tab_mut().edits = Default::default();
        app.tab_mut().sql = "SELECT 1".into();
        app
    };

    // Control: nothing unsaved, so reload simply runs.
    let mut app = clean_query_tab();
    for _ in 0..3 {
        frame(&mut app, None);
    }
    let before = app.query_seq;
    frame(&mut app, reload);
    frame(&mut app, None);
    assert_ne!(app.query_seq, before, "Cmd+R reloads a clean query tab");
    assert!(app.pending_leave.is_none());

    // Staged row edits: ask first; Cancel keeps them, Discard drops them and then reloads.
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.tab_mut().sql = "SELECT 1".into();
    for _ in 0..3 {
        frame(&mut app, None);
    }
    assert!(app.tab().edits.has_pending());
    let before = app.query_seq;
    frame(&mut app, reload);
    frame(&mut app, None);
    assert!(app.pending_leave.is_some(), "reload must ask first");
    assert_eq!(app.query_seq, before, "nothing runs while it asks");
    assert!(app.tab().edits.has_pending(), "…and nothing is discarded");
    app.apply_action(Action::CancelLeaving);
    assert!(app.pending_leave.is_none() && app.tab().edits.has_pending());
    frame(&mut app, reload);
    frame(&mut app, None);
    app.apply_action(Action::DiscardBeforeLeaving);
    assert!(!app.tab().edits.has_pending(), "Discard drops the edits");
    assert_ne!(app.query_seq, before, "…and then reloads");

    // Drafts: the query behind them never runs; a used draft asks, a pristine one resets.
    for (name, open) in [
        ("New Table", Action::OpenNewTable),
        ("New View", Action::OpenNewView),
        ("New Trigger", Action::OpenNewTrigger),
    ] {
        let mut app = clean_query_tab();
        app.apply_action(open);
        for _ in 0..3 {
            frame(&mut app, None);
        }
        let before = app.query_seq;
        frame(&mut app, reload);
        frame(&mut app, None);
        assert_eq!(app.query_seq, before, "{name}: reload ran the hidden query");
        assert!(
            app.pending_leave.is_none(),
            "{name}: pristine draft resets silently"
        );
        assert!(app.tab().schema_editor.is_some(), "{name}: still open");
        frame(&mut app, Some((egui::Key::Enter, egui::Modifiers::COMMAND)));
        frame(&mut app, None);
        assert_eq!(
            app.query_seq, before,
            "{name}: Cmd+Enter ran the hidden query"
        );
    }

    let mut app = clean_query_tab();
    app.apply_action(Action::OpenNewView);
    app.tab_mut().sql = "SELECT id FROM users".into();
    app.tab_mut().mark_sql_changed();
    for _ in 0..3 {
        frame(&mut app, None);
    }
    frame(&mut app, reload);
    frame(&mut app, None);
    assert!(
        app.pending_leave.is_some(),
        "a written view asks before reset"
    );
    assert!(
        matches!(app.tab().schema_editor.as_ref(), Some(crate::schema::ObjectEditor::View(e)) if e.select_body == "SELECT id FROM users"),
        "…and keeps it until the answer"
    );
    app.apply_action(Action::DiscardBeforeLeaving);
    assert_eq!(app.tabs.len(), 1, "Discard closes the draft's tab");
    assert!(!app.tab().draft_tab);

    // Esc out of a used draft asks too.
    let mut app = clean_query_tab();
    app.apply_action(Action::OpenNewTable);
    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.table_name = "customers".into();
    }
    for _ in 0..3 {
        frame(&mut app, None);
    }
    ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
    frame(&mut app, None);
    frame(&mut app, Some((egui::Key::Escape, egui::Modifiers::NONE)));
    frame(&mut app, None);
    assert!(
        app.pending_leave.is_some(),
        "Esc on a used draft asks first"
    );
    assert!(app.tab().schema_editor.is_some());
}

/// Opening a New Table / View / Trigger editor puts the pending object in the explorer, in
/// its own folder, and the entry follows the name being typed.
#[test]
fn new_objects_appear_in_the_explorer_while_they_are_drafted() {
    use egui_kittest::kittest::Queryable;
    let build = |open: Action| {
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        app.show_schema_panel = true;
        app.show_details_panel = false;
        app.show_connection_tabs = false;
        connect_fake(&mut app, fake_schema(2, 3));
        app.apply_action(open);
        app
    };
    let render = |app: DbGuiApp| {
        let mut setup = false;
        let mut app = app;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1100.0, 700.0))
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
            });
        harness.run_steps(6);
        harness
    };

    // Table: suggested name, listed beside the real tables.
    let harness = render(build(Action::OpenNewTable));
    let drafts = harness.query_all_by_label("untitled_table_1").count();
    assert!(drafts >= 2, "listed in the explorer and named on its tab");

    // Typing renames it live.
    let mut app = build(Action::OpenNewTable);
    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.table_name = "customers_archive".into();
    }
    let harness = render(app);
    assert!(
        harness.query_all_by_label("customers_archive").count() >= 2,
        "named in the explorer and on the tab"
    );
    assert!(harness.query_by_label("untitled_table_1").is_none());

    // View: its folder opens by itself even though the connection has no views yet.
    let harness = render(build(Action::OpenNewView));
    assert!(harness.query_by_label("Views").is_some());
    assert!(harness.query_all_by_label("untitled_view_1").count() >= 2);

    // Trigger: unnamed so far, so the placeholder shows.
    let harness = render(build(Action::OpenNewTrigger));
    assert!(harness.query_by_label("Triggers").is_some());
    assert!(harness.query_all_by_label("untitled_trigger").count() >= 2);

    // Leaving the draft takes it away again.
    let mut app = build(Action::OpenNewTable);
    app.apply_action(Action::CancelSchema);
    let harness = render(app);
    assert!(harness.query_by_label("untitled_table_1").is_none());
}

/// A New Table / View / Trigger draft gets a tab of its own: it shows in the tab strip under
/// its name, leaves the tab the user was in alone, closes on cancel, and after Apply is replaced
/// by the table or view it created.
#[test]
fn drafts_open_in_their_own_tab_and_close_with_the_apply_or_cancel() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut time = 0.0;
    let mut frame = |app: &mut DbGuiApp| {
        time += 0.1;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.tab_mut().edits = Default::default();
    app.tab_mut().sql = "SELECT 1".into();
    let original = app.tab().id;

    app.apply_action(Action::OpenNewTable);
    assert_eq!(app.tabs.len(), 2, "a tab of its own");
    assert!(app.tab().draft_tab && app.tab().id != original);
    assert_eq!(
        app.tab_kind(app.active_query_tab),
        crate::components::QueryTabKind::Table
    );
    assert_eq!(
        app.tabs[0].sql, "SELECT 1",
        "the tab the user was in is untouched"
    );
    assert!(app.tabs[0].schema_editor.is_none());

    // The title follows the name being typed.
    frame(&mut app);
    assert_eq!(app.tab_label(app.active_query_tab), "untitled_table_1");
    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.table_name = "customers".into();
        editor.columns[0].name = "customer_id".into();
    }
    frame(&mut app);
    assert_eq!(app.tab_label(app.active_query_tab), "customers");

    // A second "New Table" while this one has work in it opens a second tab, not a reset.
    app.apply_action(Action::OpenNewTable);
    assert_eq!(app.tabs.len(), 3);
    // Every "New …" opens another tab, even over an untouched draft, with a free name.
    app.apply_action(Action::OpenNewView);
    assert_eq!(app.tabs.len(), 4);
    app.apply_action(Action::OpenNewView);
    assert_eq!(app.tabs.len(), 5);
    frame(&mut app);
    assert_ne!(
        app.tab_label(app.active_query_tab),
        app.tab_label(app.active_query_tab - 1),
        "draft names don't collide"
    );
    for _ in 0..3 {
        app.apply_action(Action::CancelSchema);
    }

    // Drafts are not saved with the workspace.
    let saved = app.snapshot_workspace();
    assert_eq!(saved.tabs.len(), 1, "only the real tab is persisted");

    // Cancel closes a draft's tab (the three untouched ones above).
    assert_eq!(app.tabs.len(), 2, "the extra drafts' tabs are gone");
    assert!(
        app.tab().schema_editor.is_some() && app.tab().draft_tab,
        "…and the table draft with work in it is still there"
    );

    // Apply: the draft tab is replaced by the table it created, opened to its rows.
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.tab_mut().edits = Default::default();
    app.apply_action(Action::OpenNewTable);
    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.table_name = "customers".into();
    }
    let draft_id = app.tab().id;
    app.tx
        .send(AppMessage::SchemaApplied {
            tab_id: draft_id,
            conn_id: "edit-connection".into(),
            sql: "CREATE TABLE customers (id INTEGER PRIMARY KEY)".into(),
            elapsed_ms: 1.0,
            result: Ok("applied".into()),
        })
        .unwrap();
    for _ in 0..4 {
        frame(&mut app);
    }
    assert!(
        app.tabs.iter().all(|t| !t.draft_tab),
        "the draft tab is gone"
    );
    let opened = app.tab();
    assert_eq!(opened.title, "customers");
    assert_eq!(opened.kind, crate::components::QueryTabKind::Table);
    assert_eq!(
        opened
            .edits
            .pending_source
            .as_ref()
            .or(opened.edits.source.as_ref())
            .map(|s| s.pk_cols.clone()),
        Some(vec!["id".to_string()]),
        "opened editable on the key the draft declared"
    );
}

/// The "Discard all changes?" confirmation is a modal over the normal screen — the explorer
/// and the rest stay on screen — and Escape answers it with Cancel.
#[test]
fn discard_confirmation_keeps_the_screen_and_escape_cancels() {
    use egui_kittest::kittest::Queryable;
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = true;
    app.active_connections[0].schema = fake_schema(3, 2);
    let tab_id = app.tab().id;
    app.pending_leave = Some(super::unsaved::PendingLeave {
        action: Action::RunQuery,
        tab_ids: vec![tab_id],
    });
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 700.0))
        .build_ui_state(
            move |ui, app: &mut DbGuiApp| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
            },
            app,
        );
    harness.run_steps(4);
    harness.get_by_label("Discard all changes?");
    harness.get_by_label("table_0");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    assert!(harness.state().pending_leave.is_none(), "Esc cancels");
    assert!(
        harness.state().tab().edits.has_pending(),
        "…and keeps the work"
    );
}

#[test]
fn split_panes_keep_independent_editor_assistance_state() {
    let mut app = DbGuiApp::construct();
    app.tabs[0].editor_assist.autocomplete.open = true;
    app.tabs[0].editor_assist.autocomplete.prefix = "cust".into();
    app.tabs[0].editor_assist.ghost_suggestion = Some("omers".into());
    app.tabs[0].editor_assist.syntax_checked = "SELECT left".into();

    app.open_split_workspace();
    let right = app.split_panes[0];

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

/// Ids of the tabs in `pane`, in tab-strip order.
fn pane_ids(app: &DbGuiApp, pane: usize) -> Vec<u64> {
    app.tabs
        .iter()
        .filter(|tab| tab.pane == pane)
        .map(|tab| tab.id)
        .collect()
}

#[test]
fn split_group_adds_selects_and_closes_tabs_independently() {
    let mut app = DbGuiApp::construct();
    app.open_split_workspace();
    let first_id = app.tabs[app.split_panes[0]].id;

    app.new_tab_in_split_pane(1);
    let second_id = app.tabs[app.split_panes[0]].id;
    assert_eq!(pane_ids(&app, 1), [first_id, second_id]);
    assert_eq!(app.tabs.len(), 3);

    let first_idx = app.tabs.iter().position(|tab| tab.id == first_id).unwrap();
    app.select_split_pane_tab(first_idx, 1);
    assert_eq!(app.tabs[app.split_panes[0]].id, first_id);

    app.close_split_pane_tab(first_idx, 1);
    assert_eq!(pane_ids(&app, 1), [second_id]);
    assert_eq!(app.tabs[app.split_panes[0]].id, second_id);
    let second_idx = app.tabs.iter().position(|tab| tab.id == second_id).unwrap();
    app.close_split_pane_tab(second_idx, 1);
    assert!(!app.is_split());
    assert!(pane_ids(&app, 1).is_empty());
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn dragging_more_tables_into_an_open_split_adds_right_group_tabs() {
    let mut app = DbGuiApp::construct();
    connect_fake(&mut app, fake_schema(3, 3));

    for table in ["table_0", "table_1"] {
        app.open_schema_table_in_split(SchemaTableDrag {
            conn_id: "c1".into(),
            schema: None,
            table: table.into(),
            pinned: false,
        }, 1);
    }

    assert_eq!(app.tabs.iter().filter(|tab| tab.pane > 0).count(), 2);
    let right_titles: Vec<&str> = app
        .tabs
        .iter()
        .filter(|tab| tab.pane > 0)
        .map(|tab| tab.title.as_str())
        .collect();
    assert_eq!(right_titles, ["table_0", "table_1"]);
    assert_eq!(app.tabs[app.split_panes[0]].title, "table_1");
}

#[test]
fn split_workspace_renders_a_tab_header_inside_each_pane() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().title = "left_query".into();
    app.open_split_workspace();
    let right = app.split_panes[0];
    app.tabs[right].title = "right_query".into();
    app.new_tab_in_split_pane(1);
    let second_right = app.split_panes[0];
    app.tabs[second_right].title = "right_table".into();

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

    let left = harness.get_by_label("left_query").rect();
    let right = harness.get_by_label("right_query").rect();
    let right_table = harness.get_by_label("right_table").rect();
    assert!(
        left.center().x < 500.0 && right.center().x > 500.0 && right_table.center().x > 500.0,
        "each split pane must own its tab header"
    );
    assert!(
        (left.center().y - right.center().y).abs() < 1.0,
        "split tab headers must align across the divider"
    );
    harness.get_by_label("Close left_query");
    harness.get_by_label("Close right_query");
    harness.get_by_label("Close right_table");
}

#[test]
fn workspace_splits_into_more_than_two_columns() {
    let mut app = DbGuiApp::construct();
    app.open_split_workspace();
    // Dropping on `pane_count()` opens another column until MAX_PANES; past that the drop
    // joins the last column instead (two columns are added, then two tabs pile into the last).
    for _ in 0..4 {
        let source = &app.tabs[0];
        let mut tab = QueryTab::new(app.next_tab_id, source.title.clone());
        app.next_tab_id += 1;
        tab.conn_id = source.conn_id.clone();
        let target = app.pane_count();
        app.install_split_tab(tab, target, false);
    }
    assert_eq!(app.pane_count(), DbGuiApp::MAX_PANES);
    assert_eq!(app.split_ratios.len(), DbGuiApp::MAX_PANES);
    assert!((app.split_ratios.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    assert_eq!(pane_ids(&app, DbGuiApp::MAX_PANES - 1).len(), 3);
    for pane in 1..app.pane_count() {
        let active = app.split_panes[pane - 1];
        assert_eq!(app.tabs[active].pane, pane);
    }
}

#[test]
fn closing_a_middle_column_renumbers_the_ones_after_it() {
    let mut app = DbGuiApp::construct();
    app.open_split_workspace();
    for _ in 0..2 {
        let mut tab = QueryTab::new(app.next_tab_id, String::new());
        app.next_tab_id += 1;
        tab.conn_id = None;
        let target = app.pane_count();
        app.install_split_tab(tab, target, false);
    }
    assert_eq!(app.pane_count(), 4);
    let ids: Vec<u64> = (1..4).map(|pane| pane_ids(&app, pane)[0]).collect();
    let middle_idx = app.tabs.iter().position(|tab| tab.id == ids[0]).unwrap();

    app.close_split_pane_tab(middle_idx, 1);

    assert_eq!(app.pane_count(), 3);
    assert_eq!(pane_ids(&app, 1), [ids[1]]);
    assert_eq!(pane_ids(&app, 2), [ids[2]]);
    assert!((app.split_ratios.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    for pane in 1..app.pane_count() {
        assert_eq!(app.tabs[app.split_panes[pane - 1]].pane, pane);
    }

    // Closing the last extra columns collapses back to a single workspace.
    for id in [ids[1], ids[2]] {
        let idx = app.tabs.iter().position(|tab| tab.id == id).unwrap();
        let pane = app.tabs[idx].pane;
        app.close_split_pane_tab(idx, pane);
    }
    assert!(!app.is_split());
    assert_eq!(app.split_ratios, vec![1.0]);
}

#[test]
fn three_column_workspace_survives_a_save_and_restore() {
    let mut app = DbGuiApp::construct();
    app.open_split_workspace();
    let mut tab = QueryTab::new(app.next_tab_id, String::new());
    app.next_tab_id += 1;
    tab.sql = "SELECT 3".into();
    app.install_split_tab(tab, 2, false);

    let saved = app.snapshot_workspace();
    assert_eq!(saved.tabs.iter().map(|tab| tab.pane).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(saved.active_pane_tabs, [1, 2]);

    // An older build only knows `split_pane`, so every extra column must still flag it.
    assert!(saved.tabs[1].split_pane && saved.tabs[2].split_pane);
}

#[test]
fn pane_widths_respect_the_minimum_and_fill_the_row() {
    use super::layout::pane_widths;
    let even = pane_widths(&[0.25; 4], 1000.0, 220.0);
    assert!(even.iter().all(|w| (*w - 250.0).abs() < 1e-3));

    // A pane squeezed below the minimum is pinned and the others absorb the difference.
    let squeezed = pane_widths(&[0.1, 0.45, 0.45], 1000.0, 220.0);
    assert!((squeezed[0] - 220.0).abs() < 1e-3);
    assert!((squeezed.iter().sum::<f32>() - 1000.0).abs() < 1e-2);
    assert!(squeezed.iter().all(|w| *w >= 220.0 - 1e-3));

    // Too narrow to honour the minimum: share evenly rather than overflow.
    let tiny = pane_widths(&[0.5, 0.5], 300.0, 220.0);
    assert!((tiny[0] - 150.0).abs() < 1e-3);
}

#[test]
fn three_split_columns_each_render_their_own_tab_header() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.tab_mut().title = "col_a".into();
    app.open_split_workspace();
    let second = app.split_panes[0];
    app.tabs[second].title = "col_b".into();
    let tab = QueryTab::new(app.next_tab_id, "col_c".into());
    app.next_tab_id += 1;
    app.install_split_tab(tab, 2, false);

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1200.0, 700.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);

    let a = harness.get_by_label("col_a").rect();
    let b = harness.get_by_label("col_b").rect();
    let c = harness.get_by_label("col_c").rect();
    assert!(
        a.center().x < b.center().x && b.center().x < c.center().x,
        "columns must be laid out left to right"
    );
    assert!((a.center().y - c.center().y).abs() < 1.0);
}

#[test]
fn details_follow_the_focused_split_pane() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().set_result(fake_result(3, 3));
    app.open_split_workspace();
    let right = app.split_panes[0];
    app.tabs[right].set_result(fake_result(3, 3));
    app.tabs[right].selection.select_one(1);

    // Nothing is selected in the main pane, so focusing it shows no Details...
    app.focused_pane = 0;
    assert_eq!(app.details_target(), None);
    // ...and focusing the split pane shows that pane's selected row.
    app.focused_pane = 1;
    assert_eq!(app.details_target(), Some((right, 1)));
}

#[test]
fn view_mode_bar_sheds_parts_as_the_column_narrows() {
    use super::panels::pager::BarDensity;
    assert_eq!(BarDensity::for_width(900.0), BarDensity::Full);
    assert_eq!(BarDensity::for_width(500.0), BarDensity::Compact);
    assert_eq!(BarDensity::for_width(300.0), BarDensity::Tight);
    // Whatever the density, the segmented control plus the add button and the right-hand
    // cluster must fit in the bar.
    for width in [260.0_f32, 320.0, 420.0, 500.0, 700.0, 1100.0] {
        let density = BarDensity::for_width(width);
        let segments = density.segment_width(width, 300.0, 0.0);
        assert!(segments <= 300.0);
        if segments > 150.0 {
            assert!(
                segments + density.add_button_width() + density.right_reserved() <= width,
                "{width}pt bar overflows at {density:?}"
            );
        }
    }
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
        query.get_by_label("Editor options").rect().center().y
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
        (query.get_by_label("Run Current").rect().center().y
            - query.get_by_label("Editor options").rect().center().y)
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
        table.query_by_label("Editor options").is_none()
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
    let editor_y = compact.get_by_label("Editor options").rect().center().y;
    let result_y = compact.get_by_label("Empty state mark").rect().center().y;
    let result_modes_y = compact.get_by_label("Data").rect().center().y;
    let live_log_y = compact.get_by_label("Live log").rect().center().y;
    let log_dock = compact.get_by_label("Live log dock");
    assert!(editor_y < result_y);
    assert!(
        result_y - editor_y > 50.0,
        "compact result area collapsed: editor={editor_y}, result={result_y}"
    );
    assert!(
        result_y < result_modes_y && result_modes_y < live_log_y,
        "query result modes must dock below the result and above Live log"
    );
    assert!(
        compact.get_by_label("Data").rect().bottom() < log_dock.rect().top(),
        "query result modes must sit outside Live log, above its resize boundary"
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
fn live_log_resizes_below_the_separate_result_mode_dock() {
    for kind in [
        crate::components::QueryTabKind::Query,
        crate::components::QueryTabKind::Table,
    ] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        app.show_schema_panel = false;
        app.show_details_panel = false;
        app.show_connection_tabs = false;
        app.show_query_console = false;
        app.show_live_log = true;
        app.tab_mut().kind = kind;
        app.tab_mut().set_result(fake_result(2, 3));
        let log_id = egui::Id::new(("live_log", app.tab().id));
        let modes_id = egui::Id::new(("view_mode_bar", app.tab().id));
        let mut time = 0.0;
        let mut frame = |app: &mut DbGuiApp, events| {
            time += 0.05;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 700.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| app.draw(ui, None),
            )
        };
        for _ in 0..3 {
            let _ = frame(&mut app, vec![]);
        }
        let panel = |id| {
            egui::containers::panel::PanelState::load(&ctx, id)
                .unwrap()
                .rect
        };
        let modes_before = panel(modes_id);
        let log_before = panel(log_id);
        let handle = ctx.read_response(log_id.with("__resize")).unwrap();
        let start = handle.rect.center();
        assert!(
            modes_before.bottom() <= start.y,
            "{kind:?}: modes={modes_before:?}, log={log_before:?}, handle={:?}",
            handle.rect
        );
        assert!(start.y < log_before.top() + 8.0);
        assert!(modes_before.bottom() <= log_before.top());

        let _ = frame(&mut app, vec![egui::Event::PointerMoved(start)]);
        let _ = frame(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let end = start - egui::vec2(0.0, 60.0);
        let _ = frame(&mut app, vec![egui::Event::PointerMoved(end)]);
        let _ = frame(
            &mut app,
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let _ = frame(&mut app, vec![]);
        assert!(panel(log_id).height() > log_before.height() + 40.0);
        assert!((panel(modes_id).height() - modes_before.height()).abs() < 0.1);
        assert!(panel(modes_id).bottom() <= panel(log_id).top());
    }
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
        modes.rect().bottom() < log_dock.rect().top(),
        "table modes must sit outside Live log, above its resize boundary"
    );
    assert!(
        harness.query_by_label("Editor options").is_none()
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

#[test]
fn table_tab_keeps_view_modes_while_schema_metadata_loads() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.show_live_log = false;
    connect_fake(&mut app, SchemaTree::default());
    app.connection_jobs.insert("c1".into());
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_0".into(),
        pk_cols: Vec::new(),
    });
    app.tab_mut().set_result(fake_result(2, 3));
    app.tab_mut().view = TabView::Structure;

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

    harness.get_by_label("Data");
    harness.get_by_label("Structure");
    harness.get_by_label("Indexes");
    assert!(
        harness.query_by_label("Loading table structure…").is_some(),
        "Structure should show a loading state until reconnect metadata arrives"
    );
}

#[test]
fn data_view_shows_loading_message_before_its_first_result() {
    use egui_kittest::kittest::Queryable;

    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.show_live_log = false;
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.busy = Busy::Querying;
    let tab_id = app.tab().id;
    app.query_jobs.insert(
        tab_id,
        query::QueryJob {
            cancel: tokio_util::sync::CancellationToken::new(),
            running: true,
            started: std::time::Instant::now(),
        },
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

    assert!(harness.query_by_label("Loading data…").is_some());
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
    query.get_by_label("Columns");
    assert!(
        query.query_by_label("Apply").is_none() && query.query_by_label("Cancel").is_none(),
        "Cmd/Ctrl+S and Esc replace the Apply / Cancel buttons"
    );
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
    // The fixture is SQLite: it can't edit check/comment metadata or alter foreign keys in
    // place, so those columns are left out rather than shown inert.
    for header in ["column_name", "data_type", "is_nullable", "column_default"] {
        table.get_by_label(header);
    }
    for header in ["check", "foreign_key", "comment"] {
        assert!(
            table.query_by_label(header).is_none(),
            "{header} is not editable on this provider"
        );
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
        .query_by_label("2 rows · 3 columns · 0.0 ms")
        .is_some());

    harness.get_by_label("Chart").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Export").is_some());
    assert!(harness.query_by_label("Line").is_some());
    assert!(harness.query_by_label("Y: col0").is_some());
    assert!(harness.query_by_label("X: Row number").is_some());
    assert!(harness.query_by_label("Style").is_some());

    harness.get_by_label("Line").click();
    harness.run_steps(2);
    for kind in [
        "Area chart",
        "Bar chart",
        "Stacked bar",
        "Scatter plot",
        "Donut chart",
    ] {
        assert!(harness.query_by_label(kind).is_some());
    }
    harness.get_by_label("Area chart").click();
    harness.run_steps(2);
    harness.get_all_by_label("Style").next().unwrap().click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Titles").is_some());
    assert!(harness.query_by_label("Legend").is_some());
    assert!(harness.query_by_label("Reset style").is_some());

    harness.get_all_by_label("Style").next().unwrap().click();
    harness.run_steps(2);
    harness.get_by_label("X: Row number").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("X axis").is_some());
    assert!(harness.query_by_label("Row number").is_some());

    harness.get_by_label("X: Row number").click();
    harness.run_steps(2);
    harness.get_by_label("Y: col0").click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Y values").is_some());
    assert!(harness.query_all_by_label("col0").next().is_some());
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
    app.tab_mut().query_seq = 1;
    app.busy = Busy::Querying;
    app.query_jobs.insert(
        tab_id,
        query::QueryJob {
            cancel: tokio_util::sync::CancellationToken::new(),
            running: true,
            started: std::time::Instant::now(),
        },
    );
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
    assert!(app.is_tab_querying(tab_id));
    assert!(
        app.tab().query_error.is_none(),
        "a stale failure must not surface on the tab"
    );
}

#[test]
fn reconnect_reloads_the_active_table_tab() {
    let mut app = DbGuiApp::construct();
    let ctx = egui::Context::default();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.id = "conn-1".into();
    cfg.name = "Remote DB".into();
    app.connections.push(cfg);
    app.connection_jobs.insert("conn-1".into());
    app.tab_mut().conn_id = Some("conn-1".into());
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().sql = "SELECT * FROM users".into();

    app.tx
        .send(AppMessage::Connected {
            conn_id: "conn-1".into(),
            name: "Remote DB".into(),
            elapsed_ms: 1.0,
            result: Ok(Arc::new(DummyDb)),
        })
        .unwrap();
    app.poll_messages(&ctx);

    // `poll_messages` drains the channel, so on a fast machine the instant dummy query can
    // finish and be applied within the same call. Either way the reload must have started.
    let tab = app.tab();
    assert!(
        app.is_tab_querying(tab.id) || tab.result.is_some() || tab.query_error.is_some(),
        "reconnect did not start a reload of the open table tab"
    );
}

#[test]
fn reconnect_restores_structure_and_indexes_for_an_open_table_tab() {
    let mut app = DbGuiApp::construct();
    let ctx = egui::Context::default();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.id = "conn-1".into();
    cfg.name = "Remote DB".into();
    app.connections.push(cfg);
    app.connection_jobs.insert("conn-1".into());
    app.tab_mut().conn_id = Some("conn-1".into());
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().sql = "SELECT * FROM table_0".into();
    app.tab_mut().view = TabView::Indexes;

    app.tx
        .send(AppMessage::Connected {
            conn_id: "conn-1".into(),
            name: "Remote DB".into(),
            elapsed_ms: 1.0,
            result: Ok(Arc::new(ReconnectMetadataDb)),
        })
        .unwrap();
    app.poll_messages(&ctx);

    assert!(app.tab().table_metadata_pending);
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.poll_messages(&ctx);
        if app.tab().result.is_some() && app.tab().schema_editor.is_some() {
            break;
        }
    }

    assert!(
        app.tab().result.is_some(),
        "Data should reload after reconnect"
    );
    let editor = match app.tab().schema_editor.as_ref() {
        Some(ObjectEditor::Table(editor)) => editor,
        _ => panic!("fresh table metadata should restore the schema editor"),
    };
    assert_eq!(editor.active_tab, crate::schema::SchemaTab::Indexes);
    assert_eq!(editor.columns.len(), 2);
    assert_eq!(editor.indexes.len(), 1);
}

#[test]
fn selecting_an_unloaded_table_tab_after_reconnect_runs_its_query() {
    let mut app = DbGuiApp::construct();
    app.active_connections.push(ActiveConnection {
        config_id: "conn-1".into(),
        name: "Remote DB".into(),
        db: Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    app.tab_mut().conn_id = Some("conn-1".into());

    let mut table_tab = QueryTab::new(app.next_tab_id, "users".into());
    app.next_tab_id += 1;
    table_tab.conn_id = Some("conn-1".into());
    table_tab.kind = crate::components::QueryTabKind::Table;
    table_tab.sql = "SELECT * FROM users".into();
    let table_id = table_tab.id;
    app.tabs.push(table_tab);

    app.select_tab(1);

    assert!(app.is_tab_querying(table_id));
    assert_eq!(app.busy, Busy::Querying);
}

#[test]
fn exact_table_total_is_routed_only_to_the_matching_query() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.query_seq = 3;
    app.tab_mut().query_seq = 3;
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
fn database_sort_preserves_unsaved_cell_edits() {
    let mut app = app_with_staged_edit();
    let original_sql = app.tab().sql.clone();
    let original_rows = app.tab().row_order.clone();
    app.apply_action(Action::SetSort { col: 0, asc: false });
    assert_eq!(app.tab().sql, original_sql);
    assert_eq!(app.tab().row_order, original_rows);
    assert!(app.tab().edits.has_pending());
    assert!(app.tab().sort_base_sql.is_none());
    assert!(!app.is_tab_querying(app.tab().id));
    assert!(app.error.as_deref().unwrap().contains("Save or discard"));
}

#[test]
fn database_sort_fetches_unloaded_rows_and_keeps_order_when_paging() {
    fn finish(app: &mut DbGuiApp) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && (app.busy != Busy::Idle || app.tab().total_rows_pending)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
            app.poll_messages(&ctx);
        }
        assert_eq!(app.busy, Busy::Idle);
        assert!(
            app.tab().query_error.is_none(),
            "{:?}",
            app.tab().query_error
        );
    }
    let mut app = DbGuiApp::construct();
    app.active_connections.clear();
    let config = ConnectionConfig::new(DbKind::DuckDb);
    let db = Arc::new(dbcore::backends::duckdb::DuckDb::connect(&config).unwrap());
    app.rt
        .block_on(db.execute_capped(
            "CREATE TABLE items AS SELECT range AS id FROM range(1050);",
            1,
        ))
        .unwrap();
    app.active_connections.push(ActiveConnection {
        config_id: "sort-test".into(),
        name: "sort-test".into(),
        db,
        schema: SchemaTree::default(),
        databases: Vec::new(),
    });
    let original = "SELECT * FROM items ORDER BY id ASC LIMIT 800;";
    {
        let tab = app.tab_mut();
        tab.conn_id = Some("sort-test".into());
        tab.kind = crate::components::QueryTabKind::Table;
        tab.sql = original.into();
        tab.edits.pending_source = Some(EditSource {
            schema: None,
            table: "items".into(),
            pk_cols: Vec::new(),
        });
    }
    app.start_query_for(0);
    finish(&mut app);
    assert_eq!(app.tab().result.as_ref().unwrap().rows[0][0], Value::Int(0));

    // Split panes dispatch after the primary tab is restored. Sorting must target the
    // clicked tab's connection/result without changing the primary tab's SQL.
    let target_tab_id = app.tab().id;
    let other = QueryTab::new(app.next_tab_id, "Other".into());
    app.next_tab_id += 1;
    app.tabs.push(other);
    app.active_query_tab = 1;
    let other_sql = app.tab().sql.clone();
    app.apply_action(Action::ForTab {
        tab_id: target_tab_id,
        action: Box::new(Action::SetSort { col: 0, asc: false }),
    });
    assert_eq!(app.active_query_tab, 1);
    assert_eq!(app.tab().sql, other_sql);
    app.active_query_tab = 0;
    finish(&mut app);
    let result = app.tab().result.as_ref().unwrap();
    assert_eq!(result.row_count(), QUERY_STREAM_CHUNK_ROWS);
    assert_eq!(
        result.rows[0][0],
        Value::Int(1049),
        "the largest value is outside the cached page"
    );
    assert_eq!(app.tab().sort, Some((0, false)));
    assert_eq!(app.tab().sort_base_sql.as_deref(), Some(original));

    app.load_more_rows();
    finish(&mut app);
    let result = app.tab().result.as_ref().unwrap();
    assert_eq!(result.row_count(), 800);
    assert_eq!(result.rows.last().unwrap()[0], Value::Int(250));
    assert!(result
        .rows
        .windows(2)
        .all(|pair| pair[0][0].sort_cmp(&pair[1][0]).is_gt()));

    app.page_nav(PageNav::Next);
    finish(&mut app);
    assert_eq!(
        app.tab().result.as_ref().unwrap().rows[0][0],
        Value::Int(249)
    );
    assert_eq!(app.tab().sort, Some((0, false)));

    app.apply_action(Action::ClearSort);
    finish(&mut app);
    assert_eq!(app.tab().sql, original);
    assert_eq!(app.tab().sort, None);
    assert!(app.tab().sort_base_sql.is_none());
    assert_eq!(app.tab().result.as_ref().unwrap().rows[0][0], Value::Int(0));
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
fn closing_a_result_tab_keeps_the_other_statement_results() {
    let mut app = DbGuiApp::construct();
    app.tab_mut().set_batch_results(vec![
        ("SELECT 1".into(), Ok(fake_result(1, 1))),
        ("SELECT 2".into(), Ok(fake_result(2, 1))),
        ("SELECT 3".into(), Ok(fake_result(3, 1))),
    ]);

    app.tab_mut().close_batch_result(1);
    assert_eq!(app.tab().batch_results.len(), 2);
    assert_eq!(app.tab().active_batch_result, 0);
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 1);

    app.tab_mut().activate_batch_result(1);
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 3);
    app.tab_mut().close_batch_result(1);

    assert_eq!(app.tab().batch_results.len(), 1);
    assert_eq!(app.tab().active_batch_result, 0);
    assert_eq!(app.tab().result.as_ref().unwrap().row_count(), 1);
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
    app.tab_mut().query_seq = 7;
    app.busy = Busy::Querying;
    app.query_jobs.insert(
        tab_id,
        query::QueryJob {
            cancel: tokio_util::sync::CancellationToken::new(),
            running: true,
            started: std::time::Instant::now(),
        },
    );
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
    assert!(!app.query_jobs.values().any(|job| job.running));
}

#[test]
fn memory_limited_stream_is_marked_truncated_and_cannot_auto_continue() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.query_seq = 9;
    app.tab_mut().query_seq = 9;
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

/// A failed load-more must not retry by itself: the grid stays at its tail, so without
/// this every idle frame re-issued the same failing query (a runaway loop in the log).
#[test]
fn failed_load_more_stops_auto_continue() {
    let mut app = DbGuiApp::construct();
    let tab_id = app.tab().id;
    app.tab_mut().set_result(fake_result(2, 1));
    app.query_seq = 4;
    app.tab_mut().query_seq = 4;
    app.tx
        .send(AppMessage::QueryStreamFinished {
            tab_id,
            conn_id: String::new(),
            sql: "SELECT TOP 77 * FROM t WHERE ([id] > 23) ORDER BY [id];".into(),
            elapsed_ms: 1.0,
            rows_loaded: 0,
            page: dbcore::PageWindow {
                limit: Some(100),
                offset: 2,
            },
            result_limit: 100,
            append: true,
            result: Err("Invalid object name 't'.".into()),
            canceled: false,
            budget_truncated: false,
            row_truncated: false,
            seq: 4,
        })
        .unwrap();
    app.poll_messages(&egui::Context::default());
    assert!(app.tab().page_exhausted, "no automatic retry");
    assert!(app.tab().result.is_some(), "rows already shown stay");
}

/// `app_with_staged_edit` plus a second live connection to switch to.
fn app_with_two_connections() -> (DbGuiApp, usize) {
    let mut app = app_with_staged_edit();
    let mut other = dbcore::ConnectionConfig::new(DbKind::Sqlite);
    other.id = "other-connection".into();
    app.connections.push(other);
    app.active_connections.push(ActiveConnection {
        config_id: "other-connection".into(),
        name: "other".into(),
        db: Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    let idx = app.connections.len() - 1;
    (app, idx)
}

/// Choosing another connection never re-points a table tab: its SQL and edits belong to
/// the database it was opened from. The new connection gets a fresh query tab.
#[test]
fn switching_connection_leaves_table_tabs_bound_to_their_database() {
    let (mut app, other) = app_with_two_connections();
    app.tab_mut().edits.clear();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().sql = "SELECT * FROM \"customers\" LIMIT 100;".into();
    let before = app.tabs.len();

    app.bind_connection(other, false);

    assert_eq!(app.tabs.len(), before + 1, "a new query tab");
    assert_eq!(app.tab().conn_id.as_deref(), Some("other-connection"));
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Query);
    assert_eq!(app.tabs[0].conn_id.as_deref(), Some("edit-connection"));
    assert_eq!(app.tabs[0].sql, "SELECT * FROM \"customers\" LIMIT 100;");
}

/// A tab with staged edits stays on its own connection, edits intact: switching connection
/// opens another tab instead of re-pointing it, so the UPDATEs can never reach the new database.
#[test]
fn switching_connection_keeps_a_tabs_staged_edits_on_their_database() {
    let (mut app, other) = app_with_two_connections();
    assert!(app.tab().edits.has_pending());

    app.bind_connection(other, false);

    assert_eq!(app.tab().conn_id.as_deref(), Some("other-connection"));
    assert!(!app.tab().edits.has_pending());
    assert_eq!(app.tabs[0].conn_id.as_deref(), Some("edit-connection"));
    assert!(app.tabs[0].edits.has_pending());
}

/// A plain query tab keeps its result and edit source too: only its own connection ever
/// fills or edits it.
#[test]
fn switching_connection_keeps_a_query_tabs_result_with_its_connection() {
    let (mut app, other) = app_with_two_connections();
    app.tab_mut().edits.clear();
    let before = app.tabs.len();
    let had_result = app.tab().result.is_some();

    app.bind_connection(other, false);

    assert_eq!(app.tabs.len(), before + 1);
    assert_eq!(app.tab().conn_id.as_deref(), Some("other-connection"));
    assert!(app.tab().result.is_none());
    assert_eq!(app.tabs[0].result.is_some(), had_result);
    assert_eq!(app.tabs[0].conn_id.as_deref(), Some("edit-connection"));
}

/// The tab bar shows one connection's tabs, and clicking back on a connection returns to the
/// tab the user left there instead of piling up new ones.
#[test]
fn tabs_are_scoped_to_their_connection_and_switching_back_returns_to_the_last_one() {
    let (mut app, other) = app_with_two_connections();
    app.tab_mut().edits.clear();
    app.new_tab();
    let remembered = app.tab().id; // second tab on edit-connection, the one in use
    app.bind_connection(other, false);
    let on_other = app.tab().id;

    assert!(!app.tab_in_current_connection(0));
    assert!(!app.tab_in_current_connection(1));
    assert!(app.tab_in_current_connection(app.active_query_tab));

    let tabs_before = app.tabs.len();
    app.bind_connection(0, false);

    assert_eq!(app.tabs.len(), tabs_before, "no extra tab");
    assert_eq!(app.tab().id, remembered);
    let on_other_idx = app.tabs.iter().position(|t| t.id == on_other).unwrap();
    assert!(!app.tab_in_current_connection(on_other_idx));
}

/// Closing the last tab of a connection leaves that connection a blank tab; it must not
/// drop the user into another connection's tab.
#[test]
fn closing_a_connections_last_tab_stays_on_that_connection() {
    let (mut app, other) = app_with_two_connections();
    app.tab_mut().edits.clear();
    app.bind_connection(other, false);
    assert_eq!(app.tab().conn_id.as_deref(), Some("other-connection"));

    app.close_tab(app.active_query_tab);

    assert_eq!(app.tab().conn_id.as_deref(), Some("other-connection"));
    assert!(app.tab().sql.is_empty());
    assert_eq!(app.tabs.len(), 2, "the other connection's tab is untouched");
}

#[test]
fn close_other_and_close_all_only_touch_the_current_connection() {
    let (mut app, other) = app_with_two_connections();
    app.tab_mut().edits.clear();
    app.new_tab();
    app.new_tab(); // three tabs on edit-connection
    app.bind_connection(other, false);
    app.new_tab(); // two tabs on other-connection
    let keep = app.active_query_tab;

    app.close_other_tabs(keep);
    assert_eq!(
        app.tabs.len(),
        4,
        "3 on the first connection + the kept one"
    );
    assert_eq!(
        app.tabs
            .iter()
            .filter(|t| t.conn_id.as_deref() == Some("edit-connection"))
            .count(),
        3
    );

    app.close_all_tabs();
    assert_eq!(app.tab().conn_id.as_deref(), Some("other-connection"));
    assert_eq!(
        app.tabs
            .iter()
            .filter(|t| t.conn_id.as_deref() == Some("edit-connection"))
            .count(),
        3
    );
    assert_eq!(app.tabs.len(), 4);
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
    app.tab_mut().query_seq = 2;
    app.busy = Busy::Querying;
    app.query_jobs.insert(
        tab_id,
        query::QueryJob {
            cancel: tokio_util::sync::CancellationToken::new(),
            running: true,
            started: std::time::Instant::now(),
        },
    );
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
    app.tab_mut().query_seq = 4;
    app.busy = Busy::Querying;
    app.query_jobs.insert(
        tab_id,
        query::QueryJob {
            cancel: tokio_util::sync::CancellationToken::new(),
            running: true,
            started: std::time::Instant::now(),
        },
    );
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
    app.tab_mut().query_seq = 8;
    app.busy = Busy::Querying;
    app.query_jobs.insert(
        tab_id,
        query::QueryJob {
            cancel: tokio_util::sync::CancellationToken::new(),
            running: true,
            started: std::time::Instant::now(),
        },
    );
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
fn render_and_snapshot(app: DbGuiApp, name: &str, expand_groups: bool) {
    render_and_snapshot_at(app, name, expand_groups, 1.0);
}

/// [`render_and_snapshot`] at a given pixel density (2.0 to judge icons as on Retina).
fn render_and_snapshot_at(mut app: DbGuiApp, name: &str, expand_groups: bool, ppp: f32) {
    use egui_kittest::kittest::Queryable;
    // `construct` loads the developer's real saved connections, which the rail then paints
    // into the PNG: machine-dependent pixels, and their names committed to git. Snapshots
    // render the empty rail instead.
    app.connections.clear();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1180.0, 760.0))
        .with_pixels_per_point(ppp)
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

/// Cmd/Ctrl+F opens the find widget in every tab that shows the SQL editor — a function,
/// procedure or trigger definition and a draft view too — not only in query tabs.
#[test]
fn cmd_f_finds_in_definition_and_draft_editors() {
    use crate::components::QueryTabKind;
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    for kind in [
        QueryTabKind::Function,
        QueryTabKind::Procedure,
        QueryTabKind::Trigger,
    ] {
        let tab = app.tab_mut();
        tab.kind = kind;
        tab.sql = "CREATE FUNCTION f() RETURNS integer".into();
        tab.mark_sql_changed();
        tab.find.open = false;
        run_frame(&ctx, &mut app, vec![]);
        let editor = egui::Id::new(("sql_editor", app.tab().id, "primary"));
        ctx.memory_mut(|m| m.request_focus(editor));
        run_frame(&ctx, &mut app, vec![]);
        run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
        );
        assert!(app.tab().find.open, "{kind:?}: Cmd+F opens find");
    }

    app.apply_action(Action::OpenNewView);
    assert!(app.tab_has_sql_editor(), "a draft view is written in the SQL editor");
}

/// The floating find widget end to end: Cmd/Ctrl+F seeds the query from a one-line editor
/// selection, typing jumps to the first match from the caret, Enter steps to the next one
/// (selecting it in the editor without leaving the widget), Enter in the replace field
/// replaces the current match, and Escape closes the widget.
#[test]
fn find_widget_seeds_steps_replaces_and_closes() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    let tab = app.tab_mut();
    tab.kind = crate::components::QueryTabKind::Query;
    tab.sql = "SELECT a FROM t; SELECT a, b FROM t WHERE a = 1".into();
    tab.mark_sql_changed();
    tab.primary_cursor = 14..15; // the first `t`
    run_frame(&ctx, &mut app, vec![]);

    let editor = egui::Id::new(("sql_editor", app.tab().id, "primary"));
    ctx.memory_mut(|m| m.request_focus(editor));
    app.open_find(&ctx, false);
    assert!(app.tab().find.open && !app.tab().find.replace_open);
    assert_eq!(app.tab().find.query, "t", "seeded from the selection");
    for _ in 0..3 {
        run_frame(&ctx, &mut app, vec![]);
    }
    let find_id = editor.with("find_query");
    assert!(
        ctx.memory(|m| m.has_focus(find_id)),
        "query field focused despite the popover's sizing pass"
    );

    // The seeded query is selected, so typing replaces it.
    run_frame(&ctx, &mut app, vec![egui::Event::Text("a".into())]);
    assert_eq!(app.tab().find.query, "a");
    assert_eq!(app.tab().find.found().len(), 3);
    assert_eq!(
        app.tab().primary_cursor,
        24..25,
        "first match after the caret"
    );

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(app.tab().find.current, 2);
    assert_eq!(app.tab().primary_cursor, 42..43);
    assert!(
        ctx.memory(|m| m.has_focus(find_id)),
        "focus stays in the widget"
    );

    // Replace the current match from the replace row.
    app.tab_mut().find.replace_open = true;
    app.tab_mut().find.replacement = "x".into();
    run_frame(&ctx, &mut app, vec![]);
    let replace_id = editor.with("find_replace");
    ctx.memory_mut(|m| m.request_focus(replace_id));
    run_frame(&ctx, &mut app, vec![]);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(
        app.tab().sql,
        "SELECT a FROM t; SELECT a, b FROM t WHERE x = 1"
    );
    assert_eq!(app.tab().find.found().len(), 2);

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(!app.tab().find.open);
}

fn two_schema_tree() -> SchemaTree {
    let mut schema = fake_schema(3, 1);
    schema.tables[0].schema = Some("dbo".into());
    schema.tables[1].schema = Some("payroll".into());
    schema.tables[2].schema = Some("dbo".into());
    schema
}

/// The picker lists every schema (sorted, only when there's a choice), and a chosen
/// schema that no longer exists quietly falls back to all schemas.
#[test]
fn sidebar_schema_picker_lists_schemas_and_scopes() {
    let mut app = DbGuiApp::construct();
    connect_fake(&mut app, fake_schema(2, 1));
    assert!(app.sidebar_schemas().is_empty(), "no schemas, no picker");

    let mut app = DbGuiApp::construct();
    connect_fake(&mut app, two_schema_tree());
    assert_eq!(app.sidebar_schemas(), ["dbo", "payroll"]);
    assert_eq!(app.sidebar_schema_scope(), None, "all schemas by default");
    let id = app.active().unwrap().config_id.clone();
    app.sidebar_schema.insert(id.clone(), "payroll".into());
    assert_eq!(app.sidebar_schema_scope(), Some("payroll"));
    app.sidebar_schema.insert(id, "gone".into());
    assert_eq!(app.sidebar_schema_scope(), None);
}

/// Screenshot generator (ignored): the sidebar scoped to one schema, picker at the bottom.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_sidebar_schema_picker() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    connect_fake(&mut app, two_schema_tree());
    let id = app.active().unwrap().config_id.clone();
    app.sidebar_schema.insert(id, "dbo".into());
    render_and_snapshot_at(app, "sidebar_schema_picker", false, 2.0);
}

/// Screenshot generator (ignored): the SQL workspace bar's Editor options dropdown, opened.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_editor_options() {
    use egui_kittest::kittest::Queryable;
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.connections.clear();
    connect_fake(&mut app, fake_schema(2, 3));
    app.tab_mut().sql =
        "select top 10 customer_code\nfrom customer;\n\nselect *\n\tfrom orders;".into();
    app.tab_mut().set_result(fake_result(3, 3));
    app.editor_options.show_invisibles = true;
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1180.0, 760.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);
    // Accessibility rects are in physical pixels; pointer events take points.
    let at = harness.get_by_label("Editor options").rect().center() / 2.0;
    harness.hover_at(at);
    harness.run_steps(1);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
        harness.run_steps(1);
    }
    harness.run_steps(6);
    let submenu = harness
        .query_by_label_contains("Autocomplete")
        .expect("the dropdown must open")
        .rect()
        .center()
        / 2.0;
    harness.hover_at(submenu);
    harness.run_steps(6);
    harness.snapshot("editor_options");
}

/// Screenshot generator (ignored): result tabs of a multi-statement run.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_batch_result_tabs() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    connect_fake(&mut app, fake_schema(2, 3));
    app.tab_mut().sql = "SELECT 1; SELECT 2;".into();
    app.tab_mut().set_batch_results(vec![
        ("SELECT 1".into(), Ok(fake_result(2, 3))),
        ("SELECT 2".into(), Ok(fake_result(3, 3))),
    ]);
    render_and_snapshot_at(app, "batch_result_tabs", false, 2.0);
}

/// Screenshot generator (ignored): the Beautify chevron's preferences popover, opened.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_beautify_popover() {
    let rect = std::rc::Rc::new(std::cell::Cell::new(egui::Rect::NOTHING));
    let seen = rect.clone();
    let mut prefs = crate::format::BeautifyPrefs::default();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(360.0, 220.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            ui.painter()
                .rect_filled(ui.ctx().content_rect(), 0.0, crate::style::palette::PANEL());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(150.0);
                let before = ui.cursor().min;
                crate::components::beautify_button(ui, &mut prefs, true, "SQL Server");
                seen.set(egui::Rect::from_min_max(before, ui.min_rect().max));
            });
        });
    harness.run_steps(2);
    let chevron = egui::pos2(rect.get().right() - 8.0, rect.get().center().y);
    harness.hover_at(chevron);
    harness.event(egui::Event::PointerButton {
        pos: chevron,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    });
    harness.event(egui::Event::PointerButton {
        pos: chevron,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    });
    harness.run_steps(6);
    harness.snapshot("beautify_popover");
}

/// Screenshot generator (ignored): the Run chevron's menu with its Default run submenu open,
/// framed and laid out like the Beautify and Editor options dropdowns.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_run_menu() {
    use egui_kittest::kittest::Queryable;
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(560.0, 260.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            ui.painter()
                .rect_filled(ui.ctx().content_rect(), 0.0, crate::style::palette::PANEL());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(380.0);
                crate::components::run_button(ui, true, true, true);
            });
        });
    harness.run_steps(2);
    let click = |harness: &mut egui_kittest::Harness<'_>, label: &str| {
        let at = harness.get_by_label(label).rect().center() / 2.0;
        harness.hover_at(at);
        harness.run_steps(1);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
            harness.run_steps(1);
        }
    };
    click(&mut harness, "Run options");
    harness.run_steps(4);
    let submenu = harness.get_by_label("Default run").rect().center() / 2.0;
    harness.hover_at(submenu);
    harness.run_steps(6);
    harness.snapshot("run_menu");
}

/// Screenshot generator (ignored): the Structure view when the table's metadata can't be
/// loaded — the empty state with its icon, not a bare line of text.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_structure_unavailable() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    let tab = app.tab_mut();
    tab.kind = crate::components::QueryTabKind::Table;
    tab.view = TabView::Structure;
    tab.edits.source = Some(EditSource {
        schema: None,
        table: "ac_ms_account_group1".into(),
        pk_cols: Vec::new(),
    });
    render_and_snapshot_at(app, "structure_unavailable", false, 2.0);
}

/// Screenshot generator (ignored): grid alignment by column type — text left, integers and
/// decimals (incl. SQL Server's text-carried DECIMAL) right, booleans centred, NULL following
/// its column.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_grid_alignment() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    let meta = |name: &str, ty: &str| ColumnMeta {
        name: name.into(),
        type_name: ty.into(),
    };
    let text = |s: &str| Value::Text(s.into());
    let rows = vec![
        (
            1,
            "CT",
            "-5000.00",
            "-327.10",
            false,
            None,
            "2026-09-01 10:15:00",
        ),
        (
            2,
            "CT",
            "-45000.00",
            "-2943.93",
            false,
            None,
            "2026-09-01 10:16:30",
        ),
        (
            3,
            "IV",
            "9000.00",
            "588.79",
            true,
            Some("12.50"),
            "2026-09-02 08:00:00",
        ),
        (
            10,
            "IV",
            "9000.00",
            "-42056.07",
            true,
            None,
            "2026-09-03 17:45:12",
        ),
        (
            125,
            "RT",
            "123456.78",
            "0.05",
            false,
            Some("0.00"),
            "2026-09-04 09:00:00",
        ),
    ];
    app.tab_mut().set_result(QueryResult {
        columns: vec![
            meta("receipt_no", "NVARCHAR"),
            meta("seq", "INT"),
            meta("doc_type", "NVARCHAR"),
            meta("received_amount", "DECIMAL"),
            meta("received_vat_amount", "DECIMAL"),
            meta("posted", "BIT"),
            meta("tax_amount", "DECIMAL"),
            meta("created_at", "DATETIME"),
        ],
        rows: rows
            .into_iter()
            .map(|(seq, doc, amount, vat, posted, tax, at)| {
                vec![
                    text("JV6007279"),
                    Value::Int(seq),
                    text(doc),
                    text(amount),
                    text(vat),
                    Value::Bool(posted),
                    tax.map_or(Value::Null, text),
                    text(at),
                ]
            })
            .collect(),
        ..QueryResult::default()
    });
    render_and_snapshot_at(app, "grid_alignment", false, 2.0);
}

/// Screenshot generator (ignored): multi-cursor selections (Cmd/Ctrl+D) are painted under
/// the text like the primary selection, so every selected word stays readable.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_multi_cursor_selection() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    let tab = app.tab_mut();
    tab.kind = crate::components::QueryTabKind::Query;
    tab.sql = "SELECT * FROM [dbo].[ac_ms_account_group1];\n\
               SELECT * FROM [dbo].[ac_ms_account_group1];\n\
               SELECT * FROM [dbo].[ac_ms_account_group1];"
        .into();
    tab.mark_sql_changed();
    tab.extra_cursors = vec![65..85, 109..129];
    tab.primary_cursor = 21..41;
    render_and_snapshot_at(app, "multi_cursor_selection", false, 2.0);
}

/// Screenshot generator (ignored): the SQL editor's floating find/replace widget with the
/// replace row open, matches highlighted and the current one outlined.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_find_widget() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    let tab = app.tab_mut();
    tab.kind = crate::components::QueryTabKind::Query;
    tab.sql = "SELECT\n  TOP 100 *\nFROM\n  [dbo].[ac_ms_account_group1];\n\n\
               SELECT g.id, g.name\nFROM ac_ms_account_group1 AS g\n\
               WHERE g.parent_id IN (SELECT id FROM ac_ms_account_group1);"
        .into();
    tab.mark_sql_changed();
    tab.find.open = true;
    tab.find.replace_open = true;
    tab.find.query = "ac_ms_account_group1".into();
    tab.find.replacement = "ac_ms_account_group2".into();
    tab.find.current = 1;
    render_and_snapshot_at(app, "find_widget", false, 2.0);
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

/// IntelliJ Light reference with a join query, schema tree, and matching result grid.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_intellij_light() {
    let (mut app, dir) = demo_app_with_ddl(&[
        "CREATE TABLE actor (actor_id INTEGER PRIMARY KEY, first_name TEXT, last_name TEXT)",
        "CREATE TABLE film (film_id INTEGER PRIMARY KEY, title TEXT)",
        "CREATE TABLE film_actor (actor_id INTEGER, film_id INTEGER)",
        "CREATE TABLE film_category (film_id INTEGER, category_id INTEGER)",
        "CREATE TABLE category (category_id INTEGER PRIMARY KEY, name TEXT)",
    ]);
    app.show_welcome = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.theme = "intellij-light".into();
    let custom = serde_json::from_str::<crate::theme::ThemeFile>(include_str!(
        "../../../../examples/themes/intellij-light.json"
    ))
    .unwrap();
    crate::theme::set_current(custom.to_theme());
    let tab = app.tab_mut();
    tab.title = "console".into();
    tab.kind = crate::components::QueryTabKind::Query;
    tab.sql = "select f.title, c.name, a.first_name, a.last_name\n\
               from actor a\n\
                   join film_actor fa on a.actor_id = fa.actor_id\n\
                   join film f on fa.film_id = f.film_id\n\
                   join film_category fc on f.film_id = fc.film_id\n\
                   join category c on c.category_id = fc.category_id\n\
               ORDER BY f.title;"
        .into();
    tab.mark_sql_changed();
    tab.set_result(QueryResult {
        columns: ["title", "name", "first_name", "last_name"]
            .into_iter()
            .map(|name| ColumnMeta {
                name: name.into(),
                type_name: "TEXT".into(),
            })
            .collect(),
        rows: [
            ["ACADEMY DINOSAUR", "Documentary", "ROCK", "DUKAKIS"],
            ["ACADEMY DINOSAUR", "Documentary", "MARY", "KEITEL"],
            ["ACADEMY DINOSAUR", "Documentary", "JOHNNY", "CAGE"],
            ["ACADEMY DINOSAUR", "Documentary", "PENELOPE", "GUINESS"],
        ]
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| Value::Text(value.into()))
                .collect()
        })
        .collect(),
        ..QueryResult::default()
    });
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
            // The renderer expands its font atlas during setup; use a fresh SQL galley
            // so the preview doesn't retain texture coordinates from that first frame.
            app.tab_mut().sql_editor_cache.layout = None;
            app.draw(ui, None);
        });
    harness.run_steps(10);
    harness.snapshot("intellij_light");
    let _ = std::fs::remove_dir_all(dir);
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

#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_chart_view() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.show_query_console = false;
    app.tab_mut().set_result(QueryResult {
        columns: vec![
            ColumnMeta {
                name: "month".into(),
                type_name: "TEXT".into(),
            },
            ColumnMeta {
                name: "revenue".into(),
                type_name: "NUMERIC".into(),
            },
            ColumnMeta {
                name: "orders".into(),
                type_name: "INTEGER".into(),
            },
        ],
        rows: [
            ("Jan", 42_000.0, 318),
            ("Feb", 51_500.0, 354),
            ("Mar", 49_200.0, 341),
            ("Apr", 63_800.0, 422),
            ("May", 71_200.0, 465),
            ("Jun", 79_400.0, 508),
            ("Jul", 74_300.0, 481),
            ("Aug", 88_600.0, 557),
        ]
        .into_iter()
        .map(|(month, revenue, orders)| {
            vec![
                Value::Text(month.into()),
                Value::Float(revenue),
                Value::Int(orders),
            ]
        })
        .collect(),
        ..QueryResult::default()
    });
    app.tab_mut().view = TabView::Chart;
    render_and_snapshot(app, "chart_view", false);
}

/// "Discard all changes?" — the confirmation for reloading with staged edits.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_discard_changes_dialog() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = true;
    app.active_connections[0].schema = fake_schema(4, 3);
    app.tab_mut().sql = "SELECT * FROM items".into();
    let tab_id = app.tab().id;
    app.pending_leave = Some(super::unsaved::PendingLeave {
        action: Action::RunQuery,
        tab_ids: vec![tab_id],
    });
    render_and_snapshot(app, "discard_changes_dialog", false);
}

/// The explorer lists a table being drafted, highlighted in green.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_explorer_draft_table() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = true;
    app.show_details_panel = false;
    connect_fake(&mut app, fake_schema(5, 3));
    app.tab_mut().title = "orders".into();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.apply_action(Action::OpenNewTable);
    render_and_snapshot(app, "explorer_draft_table", false);
}

/// New View: compact bar over a full-height highlighted query editor.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_new_view_editor() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.apply_action(Action::OpenNewView);
    app.tab_mut().sql = "SELECT id, email\nFROM users\nWHERE status = 'active'".into();
    app.tab_mut().mark_sql_changed();
    render_and_snapshot(app, "new_view_editor", false);
}

/// New Trigger: the same compact bar as a view, over the full query editor.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_new_trigger_editor() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.apply_action(Action::OpenNewTrigger);
    app.tab_mut().sql = "INSERT INTO audit(msg) VALUES ('changed');".into();
    app.tab_mut().mark_sql_changed();
    render_and_snapshot(app, "new_trigger_editor", false);
}

/// New Function: the same compact bar, with a short parameter list, over the full editor.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_new_routine_editor() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    let mut editor = crate::schema::RoutineEditor::new_routine(
        dbcore::DbKind::Postgres,
        dbcore::RoutineKind::Function,
        Some("public"),
    );
    editor.name = "order_total".into();
    editor.return_type = "numeric".into();
    editor.params.push(crate::schema::ParamDraft::new_empty());
    editor.params[0].name = "order_id".into();
    editor.params[0].data_type = "integer".into();
    app.open_draft_tab(crate::schema::ObjectEditor::Routine(editor));
    app.tab_mut().sql = "BEGIN\n  RETURN (SELECT sum(price) FROM order_items WHERE order_id = $1);\nEND;".into();
    app.tab_mut().mark_sql_changed();
    render_and_snapshot(app, "new_routine_editor", false);
}

/// A trigger's or routine's body is written in the tab's SQL editor and follows into the
/// editor that builds the DDL.
#[test]
fn trigger_and_routine_bodies_follow_the_sql_editor() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    let frames = |app: &mut DbGuiApp| {
        for i in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 700.0),
                )),
                time: Some(0.1 * (i + 1) as f64),
                ..Default::default()
            };
            let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
        }
    };

    app.apply_action(Action::OpenNewTrigger);
    app.tab_mut().sql = "INSERT INTO audit(msg) VALUES ('x');".into();
    app.tab_mut().mark_sql_changed();
    frames(&mut app);
    let Some(crate::schema::ObjectEditor::Trigger(trigger)) = app.tab().schema_editor.as_ref()
    else {
        panic!("trigger editor closed");
    };
    assert_eq!(trigger.body, "INSERT INTO audit(msg) VALUES ('x');");

    let routine = crate::schema::RoutineEditor::new_routine(
        dbcore::DbKind::Postgres,
        dbcore::RoutineKind::Function,
        None,
    );
    app.open_draft_tab(crate::schema::ObjectEditor::Routine(routine));
    app.tab_mut().sql = "BEGIN RETURN 1; END;".into();
    app.tab_mut().mark_sql_changed();
    frames(&mut app);
    let Some(crate::schema::ObjectEditor::Routine(routine)) = app.tab().schema_editor.as_ref()
    else {
        panic!("routine editor closed");
    };
    assert_eq!(routine.body, "BEGIN RETURN 1; END;");
}

/// Every kind of draft, routines included, is listed in the explorer under its own group.
#[test]
fn every_draft_kind_is_listed_in_the_explorer() {
    use crate::components::QueryTabKind;
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.apply_action(Action::OpenNewTable);
    app.apply_action(Action::OpenNewView);
    app.apply_action(Action::OpenNewTrigger);
    for kind in [dbcore::RoutineKind::Function, dbcore::RoutineKind::Procedure] {
        let routine = crate::schema::RoutineEditor::new_routine(dbcore::DbKind::Postgres, kind, None);
        app.open_draft_tab(crate::schema::ObjectEditor::Routine(routine));
    }
    let kinds: Vec<_> = app.sidebar_drafts().into_iter().map(|(kind, ..)| kind).collect();
    assert_eq!(
        kinds,
        [
            QueryTabKind::Table,
            QueryTabKind::View,
            QueryTabKind::Trigger,
            QueryTabKind::Function,
            QueryTabKind::Procedure
        ]
    );
}

#[test]
fn status_bar_shows_result_statistics_once_and_groups_connection_metadata() {
    use egui_kittest::kittest::Queryable;

    let mut app = app_with_staged_edit();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    let mut result = fake_result(100, 45);
    result.stats.elapsed_ms = 210.9;
    app.status_msg = result_status(&result);
    app.tab_mut().set_result(result);
    app.tab_mut().edits.clear();
    app.active_connections[0].name = "valet-p".into();

    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 60.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.status_bar(ui, &mut Vec::new());
        });
    harness.run_steps(3);
    let summary = harness.get_by_label("100 rows · 45 columns · 211 ms").rect();
    assert!(harness.query_by_label("100 rows · 211 ms").is_none());
    let connection = harness.get_by_label("valet-p").rect();
    let version = harness
        .get_by_label(&format!("v{}", crate::update::CURRENT_VERSION))
        .rect();
    assert!(summary.right() < connection.left() && connection.right() < version.left());
    assert!((summary.center().y - connection.center().y).abs() < 1.0);
}

/// The status bar with a result, a multi-row selection, a staged edit and a running clock.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_status_bar() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.tab_mut().selection.select_one(0);
    app.tab_mut().selection.range_to(2);
    render_and_snapshot(app, "status_bar", false);
}

/// Frame-time probe for the 120 fps goal (8.3 ms per frame). Prints the average, 95th
/// percentile and worst CPU time of `run_ui` for the heaviest screens. Run optimized, since
/// debug builds say nothing about it:
/// `CARGO_PROFILE_DEV_OPT_LEVEL=2 cargo test -p plusplus-ui frame_budget -- --ignored --nocapture`
#[test]
#[ignore = "performance probe; run manually with --ignored --nocapture"]
fn frame_budget_probe() {
    fn measure(
        label: &str,
        ctx: &egui::Context,
        app: &mut DbGuiApp,
        frames: usize,
        mut events: impl FnMut(usize) -> Vec<egui::Event>,
    ) {
        for i in 0..10 {
            run_frame(ctx, app, events(i));
        }
        let mut ms: Vec<f64> = (0..frames)
            .map(|i| {
                let evs = events(i);
                let t = std::time::Instant::now();
                run_frame(ctx, app, evs);
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let avg = ms.iter().sum::<f64>() / ms.len() as f64;
        let p95 = ms[(ms.len() as f64 * 0.95) as usize - 1];
        println!(
            "{label:<44} avg {avg:6.2} ms  p95 {p95:6.2} ms  max {:6.2} ms  {}",
            ms[ms.len() - 1],
            if p95 <= 8.3 { "ok for 120 fps" } else { "OVER 8.3 ms" }
        );
    }
    let scroll = |i: usize| {
        vec![
            egui::Event::PointerMoved(egui::pos2(500.0, 400.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, if i.is_multiple_of(2) { -3.0 } else { 3.0 }),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ]
    };

    for (rows, cols) in [(1_000, 12), (100_000, 20)] {
        let (ctx, mut app) = grid_nav_app(rows, cols);
        measure(
            &format!("grid {rows}x{cols}: idle"),
            &ctx,
            &mut app,
            200,
            |_| vec![],
        );
        measure(
            &format!("grid {rows}x{cols}: scrolling"),
            &ctx,
            &mut app,
            200,
            scroll,
        );
    }

    let (ctx, mut app) = grid_nav_app(10, 3);
    let sql: String = (0..5_000)
        .map(|i| format!("SELECT col{i}, name FROM table_{i} WHERE id = {i};\n"))
        .collect();
    app.tab_mut().sql = sql;
    app.tab_mut().mark_sql_changed();
    measure("editor 5000 lines: idle", &ctx, &mut app, 200, |_| vec![]);
    measure("editor 5000 lines: scrolling", &ctx, &mut app, 200, scroll);
    measure("editor 5000 lines: typing", &ctx, &mut app, 100, |i| {
        vec![egui::Event::Text(((b'a' + (i % 26) as u8) as char).to_string())]
    });
}

/// One long-running screen for `sample`: a 5,000-line editor redrawn for ~25 s.
#[test]
#[ignore = "profiling target; run manually with --ignored"]
fn frame_budget_editor_loop() {
    let (ctx, mut app) = grid_nav_app(10, 3);
    app.tab_mut().sql = (0..5_000)
        .map(|i| format!("SELECT col{i}, name FROM table_{i} WHERE id = {i};\n"))
        .collect();
    app.tab_mut().mark_sql_changed();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(25);
    while std::time::Instant::now() < until {
        run_frame(&ctx, &mut app, vec![]);
    }
}

/// An empty table comes back from the database without column metadata. The grid fills the
/// columns in from the table's definition so "+ Row" has something to add to.
#[test]
fn an_empty_table_gets_its_columns_from_the_schema() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.tab_mut().edits.cells.clear();
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_0".into(),
        pk_cols: vec!["field_0".into()],
    });
    app.tab_mut().set_result(QueryResult::default());
    assert_eq!(app.tab().result.as_ref().unwrap().column_count(), 0);

    app.fill_empty_result_columns();

    let result = app.tab().result.as_ref().unwrap();
    assert_eq!(result.column_count(), 1);
    assert_eq!(result.columns[0].name, "field_0");
    assert_eq!(result.columns[0].type_name, "TEXT");
    // The new-row editor now has a column to start in.
    let id = app.tab_mut().edits.add_new_row();
    assert!(crate::edit::is_new_row(id));
    assert_eq!(app.tab().edits.new_rows, 1);

    // A query that is not a table read keeps its (empty) result untouched.
    app.tab_mut().edits.source = None;
    app.tab_mut().set_result(QueryResult::default());
    app.fill_empty_result_columns();
    assert_eq!(app.tab().result.as_ref().unwrap().column_count(), 0);
}

/// Standard change review: one flat code surface and the shared dialog footer.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_review_changes_dialog() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.tab_mut().edits.cells.insert(
        0,
        HashMap::from([(0, Value::Int(2))]),
    );
    app.apply_action(Action::PreviewEdits);
    assert!(app.commit_pending.is_some());
    render_and_snapshot(app, "review_changes_dialog", false);
}

/// The production Guardian dialog: plain rows, plain-word risk, one danger accent.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_production_review_dialog() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.connections[0].production = true;
    app.review_edits_before_save = false;
    app.apply_action(Action::PreviewEdits);
    if let Some(pending) = app.danger_pending.as_mut() {
        pending.preflights = Some(vec![dbcore::safety::ProductionPreflight::default(); pending.statements.len()]);
    }
    assert!(app.danger_pending.is_some());
    render_and_snapshot(app, "production_review_dialog", false);
}

/// The Open Anything palette closes with Esc, a click outside it, or Cmd/Ctrl+P again — not
/// only Esc — and a click inside it keeps it open.
#[test]
fn open_anything_closes_by_click_outside_or_its_shortcut() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    let press = |pos: egui::Pos2| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    let release = |pos: egui::Pos2| {
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]
    };

    // Opens with the shortcut; a click inside the palette's search box leaves it open.
    app.open_open_anything();
    for _ in 0..3 {
        run_frame(&ctx, &mut app, vec![]);
    }
    assert!(app.open_anything.is_some());
    run_frame(&ctx, &mut app, press(egui::pos2(500.0, 100.0)));
    run_frame(&ctx, &mut app, release(egui::pos2(500.0, 100.0)));
    assert!(app.open_anything.is_some(), "a click inside keeps it open");

    // A click well outside it (bottom-left corner) closes it.
    run_frame(&ctx, &mut app, press(egui::pos2(5.0, 690.0)));
    assert!(app.open_anything.is_none(), "click outside closes");
    run_frame(&ctx, &mut app, release(egui::pos2(5.0, 690.0)));

    // Cmd+P opens it, and Cmd+P again closes it.
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::P, egui::Modifiers::COMMAND)],
    );
    assert!(app.open_anything.is_some(), "Cmd+P opens");
    run_frame(&ctx, &mut app, vec![]);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::P, egui::Modifiers::COMMAND)],
    );
    assert!(app.open_anything.is_none(), "Cmd+P again closes");
}

/// Dropping a table closes the tabs that were showing it — once the drop has been applied, not
/// when it is merely staged, and only for that table.
#[test]
fn dropping_a_table_closes_its_open_tabs() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.tab_mut().edits.cells.clear();
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().title = "table_0".into();
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "table_0".into(),
        pk_cols: vec!["field_0".into()],
    });
    // A second tab on another table, which must survive.
    app.apply_action(Action::NewTab);
    app.tab_mut().conn_id = Some("edit-connection".into());
    app.tab_mut().kind = crate::components::QueryTabKind::Table;
    app.tab_mut().title = "other".into();
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "other".into(),
        pk_cols: Vec::new(),
    });
    assert_eq!(app.tabs.len(), 2);

    let table = app.active().unwrap().schema.tables[0].clone();
    app.apply_action(Action::DropTable(table));
    assert!(app.pending_drop.is_some(), "staged, not applied");
    assert_eq!(app.tabs.len(), 2, "nothing closes before the drop is applied");

    // The apply itself is fire-and-forget on a dummy database; deliver its result by hand.
    let sql = app.pending_drop.as_ref().unwrap().sql.clone();
    app.tx
        .send(AppMessage::SchemaApplied {
            tab_id: app.tab().id,
            conn_id: "edit-connection".into(),
            sql,
            elapsed_ms: 1.0,
            result: Ok("applied".into()),
        })
        .unwrap();
    for _ in 0..3 {
        run_frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.tabs.len(), 1, "the dropped table's tab is gone");
    assert_eq!(app.tabs[0].title, "other", "other tables' tabs stay");
    assert!(app.pending_drop.is_none());
}

/// The title-bar Layout menu: a plain list of panels with a check on the ones shown.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_layout_menu() {
    use egui_kittest::kittest::Queryable;
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = true;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    app.connections.clear();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1000.0, 520.0))
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);
    harness.get_by_label("Layout").click();
    harness.run_steps(4);
    harness.snapshot("layout_menu");
}

/// A driver that can't create an object says so instead of opening an editor that could
/// never apply; drivers that can are untouched.
#[test]
fn unsupported_objects_are_refused_up_front() {
    use dbcore::DbKind;
    assert!(!DbKind::DuckDb.supports_triggers());
    assert!(!DbKind::Cassandra.supports_triggers() && !DbKind::ScyllaDb.supports_views());
    assert!(!DbKind::Sqlite.supports_routines());
    assert!(DbKind::Sqlite.supports_triggers() && DbKind::Sqlite.supports_views());
    assert!(DbKind::Postgres.supports_routines() && DbKind::MySql.supports_triggers());

    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    // The test connection is SQLite: it has no stored routines.
    let tabs = app.tabs.len();
    app.apply_action(Action::OpenNewRoutine(dbcore::RoutineKind::Function));
    assert_eq!(app.tabs.len(), tabs, "no draft tab opened");
    assert!(app.error.as_deref().is_some_and(|e| e.contains("SQLite")));
    // …while a trigger, which SQLite has, opens.
    app.error = None;
    app.apply_action(Action::OpenNewTrigger);
    assert_eq!(app.tabs.len(), tabs + 1);
    assert!(app.error.is_none());
}

/// New Table: the dense column grid with its Columns / Indexes / Foreign Keys switch.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_new_table_grid() {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.apply_action(Action::OpenNewTable);
    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.table_name = "customers".into();
        editor.columns[0].name = "id".into();
        editor.columns[0].data_type = "INTEGER".into();
        editor.columns[0].primary_key = true;
        editor.columns[0].nullable = false;
    }
    app.apply_action(Action::AddSchemaColumn);
    if let Some(crate::schema::ObjectEditor::Table(editor)) = app.tab_mut().schema_editor.as_mut() {
        editor.columns[1].name = "email".into();
    }
    render_and_snapshot(app, "new_table_grid", false);
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

/// Screenshot generator (ignored): the foreign-key popover hanging from a Structure cell.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_foreign_key_popover() {
    let (mut app, dir) = demo_app_with_ddl(&[
        "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE order_items (id INTEGER PRIMARY KEY, product_id INTEGER, qty INTEGER)",
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
    app.show_welcome = false;
    app.show_details_panel = false;
    app.apply_action(Action::OpenForeignKeysForColumn(
        "product_id".into(),
        egui::Rect::from_min_size(egui::pos2(520.0, 150.0), egui::vec2(160.0, 21.0)),
    ));
    render_and_snapshot_at(app, "foreign_key_popover", false, 2.0);
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
    assert_eq!(statements.len(), 1);
    assert!(statements[0].starts_with(
        "UPDATE \"images\" SET \"image\" = X'89504E47' WHERE \"id\" = 1 AND \"image\" = X'00';"
    ));
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

/// Typing on the cursor cell opens its editor with the typed text replacing the value —
/// exactly once (the same Text event must not also reach the freshly focused editor).
#[test]
fn typing_on_cursor_cell_starts_editing() {
    let (ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(1);
    app.tab_mut().selection.set_cursor(1, 1);

    run_frame(&ctx, &mut app, vec![egui::Event::Text("Z".into())]);
    assert!(app.tab().edits.is_active(1, 1), "typing opens the editor");
    run_frame(&ctx, &mut app, vec![]);
    assert_eq!(app.tab().edits.active.as_ref().unwrap().buf, "Z");

    run_frame(&ctx, &mut app, vec![egui::Event::Text("q".into())]);
    assert_eq!(
        app.tab().edits.active.as_ref().unwrap().buf,
        "Zq",
        "further typing appends at the caret"
    );
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(
        app.tab().edits.staged(1, 1),
        Some(&Value::Text("Zq".into()))
    );
}

/// Cmd/Ctrl+D copies the selected rows into new insert rows, leaving the key empty.
#[test]
fn cmd_d_duplicates_selected_rows_without_the_key() {
    let (ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(0);
    app.tab_mut().selection.range_to(1);
    // A staged edit on the source is what gets copied, not the stored value.
    app.tab_mut()
        .edits
        .stage(1, 2, Value::Text("edited".into()), &Value::Int(5));

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::D, egui::Modifiers::COMMAND)],
    );

    let base = crate::edit::NEW_ROW_BASE;
    let edits = &app.tab().edits;
    assert_eq!(edits.new_rows, 2);
    assert_eq!(edits.staged(base, 0), None, "primary key left empty");
    assert_eq!(edits.staged(base, 1), Some(&Value::Int(1)));
    assert_eq!(
        edits.staged(base + 1, 2),
        Some(&Value::Text("edited".into()))
    );
    let selected: Vec<usize> = app.tab().selection.iter().collect();
    assert_eq!(selected, [5, 6], "the copies are selected");
}

/// "Set NULL" from the cell menu applies to that column on every selected row.
#[test]
fn set_cells_action_targets_the_selection() {
    let (_ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(0);
    app.tab_mut().selection.toggle(2);

    app.apply_action(Action::SetCells {
        col: 1,
        to: crate::edit::SetTo::Null,
    });

    let edits = &app.tab().edits;
    assert_eq!(edits.staged(0, 1), Some(&Value::Null));
    assert_eq!(edits.staged(1, 1), None);
    assert_eq!(edits.staged(2, 1), Some(&Value::Null));
    assert_eq!(edits.staged(0, 0), None, "other columns untouched");
}

/// Typing over a multi-row selection edits that column on every selected row (TablePlus
/// multi-row edit), and a single Cmd/Ctrl+Z takes the whole edit back.
#[test]
fn typing_over_multi_row_selection_edits_every_row() {
    let (ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(0);
    app.tab_mut().selection.range_to(2);
    app.tab_mut().selection.set_cursor(0, 1);

    run_frame(&ctx, &mut app, vec![egui::Event::Text("Q".into())]);
    assert_eq!(
        app.tab().edits.active.as_ref().map(|a| a.fan_out_len()),
        Some(2)
    );
    run_frame(&ctx, &mut app, vec![]);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    for row in 0..3 {
        assert_eq!(
            app.tab().edits.staged(row, 1),
            Some(&Value::Text("Q".into())),
            "row {row}"
        );
    }
    assert_eq!(
        app.tab().edits.staged(3, 1),
        None,
        "unselected row untouched"
    );

    app.apply_action(Action::Undo);
    assert!(!app.tab().edits.has_pending(), "one undo reverts all rows");
}

/// Pasting narrower-than-a-row text overwrites cells from the cursor, clipped to the grid.
#[test]
fn paste_overwrites_cells_from_the_cursor() {
    let (_ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(3);
    app.tab_mut().selection.set_cursor(3, 1);

    app.apply_action(Action::PasteRows("a\tb\tc\nd\te".into()));

    let edits = &app.tab().edits;
    assert_eq!(edits.new_rows, 0, "no insert rows");
    let text = |s: &str| Some(Value::Text(s.into()));
    assert_eq!(edits.staged(3, 1).cloned(), text("a"));
    assert_eq!(edits.staged(3, 2).cloned(), text("b"));
    assert_eq!(edits.staged(4, 1).cloned(), text("d"));
    assert_eq!(edits.staged(4, 2).cloned(), text("e"));
    assert!(
        app.status_msg.contains("1 outside the grid"),
        "{}",
        app.status_msg
    );
}

/// One pasted value over a multi-row selection fills the cursor column on every row.
#[test]
fn paste_single_value_fills_the_selection() {
    let (_ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(0);
    app.tab_mut().selection.range_to(2);
    app.tab_mut().selection.set_cursor(1, 2);

    app.apply_action(Action::PasteRows("z".into()));

    for row in 0..3 {
        assert_eq!(
            app.tab().edits.staged(row, 2),
            Some(&Value::Text("z".into()))
        );
    }
    assert_eq!(app.tab().edits.staged(3, 2), None);
}

/// Whole rows (one field per column, as Copy writes them) still paste as insert rows even
/// with a cell cursor in the grid.
#[test]
fn paste_whole_rows_still_inserts() {
    let (_ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(0);
    app.tab_mut().selection.set_cursor(0, 1);

    app.apply_action(Action::PasteRows("9\tx\ty".into()));

    assert_eq!(app.tab().edits.new_rows, 1);
    assert!(!app.tab().edits.row_dirty(0));
}

/// Shift+Enter breaks the line and expands the editor; Up/Down then move within the text
/// (no row advance) and Enter commits the multi-line value.
#[test]
fn shift_enter_inserts_a_line_break_and_expands() {
    let (ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().selection.select_one(1);
    app.tab_mut().selection.set_cursor(1, 1);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    run_frame(&ctx, &mut app, vec![]);

    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::SHIFT)],
    );
    run_frame(&ctx, &mut app, vec![egui::Event::Text("x".into())]);
    {
        let active = app.tab().edits.active.as_ref().expect("still editing");
        assert!(active.is_expanded());
        assert_eq!(active.buf, "4\nx");
    }
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::ArrowUp, egui::Modifiers::NONE)],
    );
    assert!(
        app.tab().edits.is_active(1, 1),
        "Up moves the caret, not the row"
    );
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(
        app.tab().edits.staged(1, 1),
        Some(&Value::Text("4\nx".into()))
    );
}

/// A value too wide for its cell opens straight into the expanded editor.
#[test]
fn long_values_open_expanded() {
    let (ctx, mut app) = grid_nav_app(5, 3);
    app.tab_mut().result.as_mut().unwrap().rows[1][1] = Value::Text("word ".repeat(80));
    app.tab_mut().selection.select_one(1);
    app.tab_mut().selection.set_cursor(1, 1);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    run_frame(&ctx, &mut app, vec![]);
    assert!(app.tab().edits.active.as_ref().unwrap().is_expanded());
    // The popover holds focus through its first (sizing) frame: typing lands, Enter saves.
    run_frame(&ctx, &mut app, vec![egui::Event::Text("!".into())]);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    let expected = format!("{}!", "word ".repeat(80));
    assert_eq!(app.tab().edits.staged(1, 1), Some(&Value::Text(expected)));

    // Short values keep the one-line cell editor.
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    app.tab_mut().selection.set_cursor(2, 1);
    run_frame(
        &ctx,
        &mut app,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    run_frame(&ctx, &mut app, vec![]);
    assert!(!app.tab().edits.active.as_ref().unwrap().is_expanded());
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

/// Back after a FK jump that opened another tab returns to the tab it came from.
#[test]
fn back_after_following_a_foreign_key_returns_to_the_origin_tab() {
    let mut app = fk_tab(vec![
        Value::Text("row-pk".into()),
        Value::Text("u7".into()),
        Value::Null,
        Value::Null,
    ]);
    let origin = app.tab().id;
    app.apply_action(Action::FollowForeignKey { row: 0, col: 1 });
    assert_ne!(app.tab().id, origin);
    assert_eq!(app.tab().nav_back.len(), 1);

    app.apply_action(Action::NavigateBack);
    assert_eq!(app.tab().id, origin, "Back selects the tab the jump started from");
}

/// A preview tab that follows its own FK is rebuilt in place; Back re-opens the table it
/// showed before, and refuses while the new view has unsaved edits.
#[test]
fn back_restores_a_preview_tab_replaced_by_a_foreign_key_jump() {
    let mut app = fk_tab(vec![
        Value::Text("row-pk".into()),
        Value::Text("u7".into()),
        Value::Null,
        Value::Null,
    ]);
    let origin_sql = "SELECT * FROM \"table_1\" LIMIT 100;".to_string();
    {
        let tab = app.tab_mut();
        tab.preview = true;
        tab.kind = crate::components::QueryTabKind::Table;
        tab.title = "table_1".into();
        tab.sql = origin_sql.clone();
    }
    let id = app.tab().id;
    app.apply_action(Action::FollowForeignKey { row: 0, col: 1 });
    assert_eq!(app.tabs.len(), 1, "the preview slot is reused");
    assert_eq!(app.tab().id, id);
    assert_ne!(app.tab().sql, origin_sql);

    // Unsaved edits on the new view block Back instead of being thrown away.
    app.tab_mut().edits.new_rows = 1;
    app.apply_action(Action::NavigateBack);
    assert_ne!(app.tab().sql, origin_sql);
    assert_eq!(app.tab().nav_back.len(), 1, "the refused step stays available");
    app.tab_mut().edits.new_rows = 0;

    app.apply_action(Action::NavigateBack);
    let tab = app.tab();
    assert_eq!(tab.id, id);
    assert_eq!(tab.sql, origin_sql);
    assert_eq!(tab.title, "table_1");
    assert_eq!(
        tab.edits
            .source
            .as_ref()
            .or(tab.edits.pending_source.as_ref())
            .map(|s| s.table.as_str()),
        Some("table_1")
    );
    assert!(tab.nav_back.is_empty());
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
        generated: false,
        max_length: None,
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
                generated: false,
                max_length: None,
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

#[test]
fn query_workspace_border_stays_below_dialogs() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    bind_heading_font(&ctx);
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.show_schema_panel = false;
    app.show_details_panel = false;
    app.show_connection_tabs = false;
    connect_fake(&mut app, fake_schema(2, 3));
    app.tab_mut().kind = crate::components::QueryTabKind::Query;
    app.tab_mut().set_result(fake_result(2, 3));
    let footer_id = egui::Id::new((
        "query_footer",
        app.tab().id,
        QueryEditorPlacement::Top,
        false,
    ));
    let overlay_color = egui::Color32::from_rgb(213, 17, 149);
    let mut output = None;
    for step in 0..3 {
        output = Some(ctx.run_ui(
            egui::RawInput {
                time: Some(step as f64),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                ..Default::default()
            },
            |ui| {
                app.draw(ui, None);
                egui::Window::new("Border layering regression")
                    .fixed_pos(egui::pos2(200.0, 100.0))
                    .frame(egui::Frame::new().fill(overlay_color))
                    .show(ui.ctx(), |ui| {
                        ui.allocate_space(egui::vec2(400.0, 500.0));
                    });
            },
        ));
    }
    let footer =
        egui::containers::panel::PanelState::load(&ctx, footer_id).expect("query toolbar rendered");
    let border_y = footer.rect.bottom() - 0.5;
    fn flatten<'a>(shape: &'a egui::Shape, shapes: &mut Vec<&'a egui::Shape>) {
        if let egui::Shape::Vec(children) = shape {
            for child in children {
                flatten(child, shapes);
            }
        } else {
            shapes.push(shape);
        }
    }
    let output = output.unwrap();
    let mut shapes = Vec::new();
    for clipped in &output.shapes {
        flatten(&clipped.shape, &mut shapes);
    }
    let border = shapes
        .iter()
        .position(|shape| {
            matches!(shape, egui::Shape::LineSegment { points, stroke }
            if (points[0].y - border_y).abs() < 0.1
                && (points[1].y - border_y).abs() < 0.1
                && points[1].x - points[0].x > 500.0
                && stroke.color == crate::style::palette::BORDER())
        })
        .expect("workspace divider must remain visible");
    let dialog = shapes
        .iter()
        .position(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == overlay_color))
        .expect("dialog background rendered");
    assert!(
        border < dialog,
        "dialog must paint over the workspace divider"
    );
}

fn app_with_activity_monitor(read_only: bool) -> DbGuiApp {
    use dbcore::activity::Session;
    let mut app = app_with_staged_edit();
    let session = |id: &str, state: &str, sql: &str| Session {
        id: id.into(),
        user: "app".into(),
        database: "shop".into(),
        client: "10.0.0.5".into(),
        state: state.into(),
        seconds: 12.0,
        waiting: String::new(),
        sql: sql.into(),
    };
    let mut tab = QueryTab::new(app.next_tab_id, "Activity".into());
    app.next_tab_id += 1;
    tab.kind = crate::components::QueryTabKind::Activity;
    tab.conn_id = Some("pg".into());
    tab.activity = Some(activity::ActivityMonitor {
        conn_id: "pg".into(),
        conn_name: "pg".into(),
        kind: DbKind::Postgres,
        read_only,
        production: false,
        sessions: vec![
            session("11", "active", "SELECT pg_sleep(60)"),
            session("12", "idle", ""),
        ],
        error: None,
        loading: false,
        last_refresh: Some(std::time::Instant::now()),
        auto_refresh: false,
        hide_idle: false,
        filter: String::new(),
        confirm: None,
        selected: None,
        notice: None,
    });
    app.tabs.push(tab);
    app.active_query_tab = app.tabs.len() - 1;
    app
}

#[test]
fn activity_monitor_filters_and_hides_idle() {
    let mut app = app_with_activity_monitor(false);
    let monitor = app.tab_mut().activity.as_mut().unwrap();
    assert_eq!(monitor.visible().count(), 2);
    monitor.hide_idle = true;
    assert_eq!(monitor.visible().count(), 1);
    monitor.hide_idle = false;
    monitor.filter = "PG_SLEEP".into();
    assert_eq!(
        monitor.visible().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["11"]
    );
}

#[test]
fn activity_tab_renders_with_and_without_a_pending_confirmation() {
    let mut app = app_with_activity_monitor(false);
    let ctx = egui::Context::default();
    crate::style::apply(&ctx);
    let raw = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 800.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run_ui(raw(), |ui| app.draw(ui, None));
    app.tab_mut().activity.as_mut().unwrap().confirm =
        Some(("11".into(), dbcore::activity::StopMode::Terminate));
    let _ = ctx.run_ui(raw(), |ui| app.draw(ui, None));
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Activity);
}

#[test]
fn stopping_a_session_is_refused_on_a_read_only_connection() {
    let mut app = app_with_activity_monitor(true);
    app.apply_action(Action::StopSession {
        id: "11".into(),
        mode: dbcore::activity::StopMode::Terminate,
    });
    let monitor = app.tab().activity.as_ref().unwrap();
    assert!(matches!(&monitor.notice, Some(Err(e)) if e.contains("read-only")));
}

#[test]
fn a_failed_session_list_keeps_the_last_good_rows_and_stops_polling() {
    let mut app = app_with_activity_monitor(false);
    app.tab_mut().activity.as_mut().unwrap().auto_refresh = true;
    let tab_id = app.tab().id;
    app.apply_activity_sessions(tab_id, Err("permission denied".into()));
    let monitor = app.tab().activity.as_ref().unwrap();
    assert_eq!(monitor.sessions.len(), 2);
    assert_eq!(monitor.error.as_deref(), Some("permission denied"));
    assert!(!monitor.auto_refresh);
    assert!(monitor.next_refresh_in().is_none());
}

#[test]
fn activity_refresh_stays_with_its_tab_after_the_active_pane_changes() {
    let mut app = app_with_activity_monitor(false);
    let activity_idx = app.active_query_tab;
    let activity_id = app.tab().id;
    let monitor = app.tab_mut().activity.as_mut().unwrap();
    monitor.auto_refresh = true;
    monitor.last_refresh = None;
    let ctx = egui::Context::default();
    crate::style::apply(&ctx);
    let mut actions = Vec::new();
    let _ = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            ..Default::default()
        },
        |ui| app.activity_view(ui, &mut actions),
    );
    assert!(actions.iter().any(|action| matches!(
        action,
        Action::ForTab { tab_id, action }
            if *tab_id == activity_id && matches!(action.as_ref(), Action::RefreshActivity)
    )));
    app.select_tab(0);
    for action in actions {
        app.apply_action(action);
    }
    assert_eq!(app.active_query_tab, 0);
    // The fixture has no live pg connection: this error proves the originating monitor
    // received the refresh even though the primary pane was restored before dispatch.
    assert_eq!(
        app.tabs[activity_idx]
            .activity
            .as_ref()
            .unwrap()
            .error
            .as_deref(),
        Some("The connection is closed.")
    );
}

#[test]
fn activity_monitor_refuses_embedded_databases() {
    let (mut app, other) = app_with_two_connections();
    let tabs = app.tabs.len();
    app.apply_action(Action::OpenActivity { conn_idx: other });
    assert_eq!(app.tabs.len(), tabs, "no tab is opened");
    assert!(app
        .error
        .as_deref()
        .unwrap_or("")
        .contains("no other sessions"));
}

/// Asking for the monitor of a connection that already has one returns to its tab.
#[test]
fn opening_the_activity_monitor_twice_reuses_its_tab() {
    let mut app = app_with_staged_edit();
    let mut pg = ConnectionConfig::new(DbKind::Postgres);
    pg.id = "pg".into();
    app.connections.push(pg);
    app.active_connections.push(ActiveConnection {
        config_id: "pg".into(),
        name: "pg".into(),
        db: Arc::new(DummyDb),
        databases: Vec::new(),
        schema: fake_schema(1, 1),
    });
    let idx = app.connections.len() - 1;
    app.apply_action(Action::OpenActivity { conn_idx: idx });
    let first = app.active_query_tab;
    assert_eq!(app.tab().kind, crate::components::QueryTabKind::Activity);
    app.apply_action(Action::SelectTab(0));
    app.apply_action(Action::OpenActivity { conn_idx: idx });
    assert_eq!(app.active_query_tab, first);
    assert_eq!(
        app.tabs
            .iter()
            .filter(|t| t.kind == crate::components::QueryTabKind::Activity)
            .count(),
        1
    );
}

/// Activity monitor with a realistic mix of sessions, in a dark and a light theme.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_activity_monitor() {
    for (theme, name) in [
        ("carbon", "activity_monitor_dark"),
        ("daylight", "activity_monitor_light"),
    ] {
        let mut app = app_with_activity_monitor(false);
        app.show_welcome = false;
        let monitor = app.tab_mut().activity.as_mut().unwrap();
        monitor.conn_name = "valet-p".into();
        let queries = [
            "SELECT \"backend\".\"User\".\"id\", \"backend\".\"User\".\"name\" FROM \"backend\".\"User\" WHERE id = $1",
            "COMMIT",
            "SELECT COUNT(*) FROM (SELECT \"backend\".\"ValetParking\".\"id\" FROM \"backend\".\"ValetParking\") t",
            "",
        ];
        monitor.sessions = (0..26)
            .map(|i| dbcore::activity::Session {
                id: (1_985_939 + i * 37).to_string(),
                user: if i % 7 == 0 { "rdsAdmin" } else { "root" }.into(),
                database: if i % 7 == 0 {
                    "postgres"
                } else {
                    "valet-parking-production"
                }
                .into(),
                client: "10.0.0.5".into(),
                state: match i {
                    0 | 1 => "active",
                    2 => "idle in transaction",
                    _ => "idle",
                }
                .into(),
                seconds: 2900.0 / (i + 1) as f64,
                waiting: if i % 2 == 0 { "Client: ClientRead" } else { "" }.into(),
                sql: queries[i as usize % queries.len()].into(),
            })
            .collect();
        app.theme = theme.into();
        crate::theme::set_current(app.themes.theme_of(&app.theme));
        render_and_snapshot_at(app, name, false, 2.0);
    }
}

/// Wide tables scroll sideways: a horizontal wheel over the table moves it.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_activity_monitor_scrolled_sideways() {
    let mut app = app_with_activity_monitor(false);
    app.show_welcome = false;
    app.connections.clear();
    let monitor = app.tab_mut().activity.as_mut().unwrap();
    monitor.conn_name = "valet-p".into();
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1180.0, 760.0))
        .with_pixels_per_point(1.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(6);
    harness.hover_at(egui::pos2(800.0, 400.0));
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(-700.0, 0.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(6);
    harness.snapshot("activity_monitor_scrolled");
}

#[test]
fn grid_stays_editable_while_more_rows_load() {
    let mut app = app_with_staged_edit();
    let tab_id = app.tab().id;
    assert!(app.grid_editable(app.active_query_tab));

    // Scrolling near the tail fetches the next chunk. Those rows only append below the
    // loaded ones, so a double-click on a networked database must still open the editor.
    let (seq, _) = app.begin_query_job(tab_id);
    app.tab_mut().stream = Some(QueryStreamUi {
        seq,
        append: true,
        columns: Vec::new(),
        pending_rows: Vec::new(),
        received_rows: 0,
    });
    assert!(app.grid_editable(app.active_query_tab));

    // A replacement run swaps the rows out, so editing waits for it.
    app.tab_mut().stream.as_mut().unwrap().append = false;
    assert!(!app.grid_editable(app.active_query_tab));
}

#[test]
fn connection_url_opens_a_prefilled_draft_without_connecting() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.show_welcome = false;
    app.apply_action(Action::OpenConnectionUrl(
        "postgres://alice:secret@db.example.com:6543/shop".into(),
    ));
    let editor = app
        .editor
        .as_ref()
        .expect("the link opens the connection form");
    assert!(editor.is_new);
    assert!(!editor.selecting_provider);
    assert_eq!(editor.config.kind, DbKind::Postgres);
    assert_eq!(editor.config.host, "db.example.com");
    assert_eq!(editor.config.port, 6543);
    assert_eq!(editor.config.database, "shop");
    assert_eq!(editor.password, "secret");
    // A link can come from any page: it never saves or connects by itself.
    assert!(app.connections.is_empty());
    assert!(app.connection_jobs.is_empty());
}

#[test]
fn connection_url_reuses_a_matching_saved_connection() {
    let mut app = DbGuiApp::construct();
    app.connections.clear();
    app.show_welcome = false;
    let mut saved = ConnectionConfig::new(DbKind::Postgres);
    saved.host = "DB.example.com".into();
    saved.port = 5432;
    saved.user = "alice".into();
    saved.database = "shop".into();
    app.connections.push(saved);
    app.apply_action(Action::OpenConnectionUrl(
        "postgresql://alice@db.example.com/shop".into(),
    ));
    assert!(
        app.editor.is_none(),
        "a saved match connects instead of drafting a duplicate"
    );
    assert_eq!(app.connections.len(), 1);
}

#[test]
fn bad_connection_url_reports_an_error() {
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.apply_action(Action::OpenConnectionUrl("postgres://host:99999/db".into()));
    assert!(app.editor.is_none());
    assert!(app.error.as_deref().is_some_and(|e| e.contains("port")));
}

#[test]
fn pasting_a_url_into_host_fills_the_draft() {
    let mut app = DbGuiApp::construct();
    app.apply_action(Action::NewConnection);
    let editor = app.editor.as_mut().unwrap();
    let parsed = dbcore::parse_connection_url("mysql://root:pw@127.0.0.1:3307/app").unwrap();
    editor.apply_connection_url(parsed);
    assert_eq!(editor.config.kind, DbKind::MySql);
    assert_eq!(editor.config.host, "127.0.0.1");
    assert_eq!(editor.config.port, 3307);
    assert_eq!(editor.config.user, "root");
    assert_eq!(editor.config.database, "app");
    assert_eq!(editor.password, "pw");
    assert_eq!(editor.config.name, "app @ 127.0.0.1");
}

/// Typing-latency probe (ignored): frame time of one keystroke in a long query against a
/// large schema, and the cost of each piece of per-keystroke editor work.
#[test]
#[ignore = "latency probe; run manually with --ignored --nocapture"]
fn probe_typing_latency() {
    use std::time::Instant;
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.connections.clear();
    connect_fake(&mut app, fake_schema(2000, 30));
    let mut sql = String::new();
    for i in 0..150 {
        sql.push_str(&format!(
            "SELECT field_1, field_2, field_3 FROM table_{i} WHERE field_0 = {i};\n"
        ));
    }
    sql.push_str("SELECT * FROM ta");
    app.tab_mut().sql = sql.clone();
    let tab_id = app.tab().id;
    let editor_id = egui::Id::new(("sql_editor", tab_id, "primary"));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let frame = |app: &mut DbGuiApp, events: Vec<egui::Event>| {
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let started = Instant::now();
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
        started.elapsed()
    };
    frame(&mut app, vec![]);
    ctx.memory_mut(|m| m.request_focus(editor_id));
    let end = sql.chars().count();
    if let Some(mut state) = egui::text_edit::TextEditState::load(&ctx, editor_id) {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(end))));
        state.store(&ctx, editor_id);
    }
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    for label in ["all on", "no autocomplete", "no ghost", "neither", "no highlight stmt"] {
        match label {
            "no autocomplete" => app.autocomplete_enabled = false,
            "no ghost" => {
                app.autocomplete_enabled = true;
                app.ghost_suggestions_enabled = false;
            }
            "neither" => app.autocomplete_enabled = false,
            "no highlight stmt" => {
                app.editor_options.highlight_current_statement = false;
            }
            _ => {}
        }
        let mut keystrokes = Vec::new();
        let mut idle = Vec::new();
        for c in "ble_1".chars() {
            keystrokes.push(frame(&mut app, vec![egui::Event::Text(c.to_string())]).as_millis());
            idle.push(frame(&mut app, vec![]).as_millis());
        }
        for _ in 0.."ble_1".len() {
            frame(&mut app, vec![egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        eprintln!("{label}: keystroke ms {keystrokes:?} idle ms {idle:?}");
    }
    eprintln!("typed tail: {:?}", &app.tab().sql[app.tab().sql.len() - 30..]);

    let schema = fake_schema(2000, 30);
    let opts = dbcore::config::EditorOptions::default();
    let text = app.tab().sql.clone();
    let caret = text.chars().count() - 2;
    let t = Instant::now();
    let c = crate::autocomplete::complete_with(&text, caret, Some(&schema), None, false, &opts);
    eprintln!("complete_with: {:?} ({} items)", t.elapsed(), c.map_or(0, |c| c.items.len()));
    // Worst case: no table referenced yet, so every column of every table is a candidate.
    for typed in ["SELECT fi", "SELECT f", "SELECT xq"] {
        let t = Instant::now();
        let c = crate::autocomplete::complete_with(typed, typed.len(), Some(&schema), None, false, &opts);
        eprintln!(
            "complete_with {typed:?}: {:?} ({} items)",
            t.elapsed(),
            c.map_or(0, |c| c.items.len())
        );
    }
    let t = Instant::now();
    let _ = crate::ghost::suggest_with(&text, caret, &[], Some(&schema), None, &opts, None);
    eprintln!("ghost::suggest_with: {:?}", t.elapsed());
    let font = egui::FontId::monospace(14.0);
    let t = Instant::now();
    let job = crate::highlight::highlight_sql_folded(&text, font, &[]);
    eprintln!("highlight: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = ctx.fonts_mut(|f| f.layout_job(job));
    eprintln!("layout: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = dbcore::check_syntax(None, &text);
    eprintln!("check_syntax: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = dbcore::check_semantics(None, &text, &schema);
    eprintln!("check_semantics: {:?}", t.elapsed());
}

/// Typing into the focused SQL editor one character per frame, the way a keyboard does.
fn type_into_editor(text: &str, keys: &str) -> String {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    crate::style::apply(&ctx);
    let mut app = DbGuiApp::construct();
    app.show_welcome = false;
    app.connections.clear();
    app.tab_mut().sql = text.to_string();
    let editor_id = egui::Id::new(("sql_editor", app.tab().id, "primary"));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let frame = |app: &mut DbGuiApp, events: Vec<egui::Event>| {
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| app.draw(ui, None));
    };
    frame(&mut app, vec![]);
    ctx.memory_mut(|m| m.request_focus(editor_id));
    let end = text.chars().count();
    if let Some(mut state) = egui::text_edit::TextEditState::load(&ctx, editor_id) {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(end))));
        state.store(&ctx, editor_id);
    }
    frame(&mut app, vec![]);
    for c in keys.chars() {
        frame(&mut app, vec![egui::Event::Text(c.to_string())]);
        frame(&mut app, vec![]);
    }
    app.tab().sql.clone()
}

/// A typed closer steps over the one auto-inserted after the caret instead of doubling it:
/// `count(*)` must not come out as `count(*))`, nor `'a'` as `'a''`.
#[test]
fn typing_a_closer_steps_over_the_auto_inserted_one() {
    // The opener really does insert its closer — otherwise the cases below prove nothing.
    assert_eq!(type_into_editor("SELECT ", "count("), "SELECT count()");
    assert_eq!(type_into_editor("SELECT ", "count(*)"), "SELECT count(*)");
    assert_eq!(type_into_editor("SELECT ", "'a'"), "SELECT 'a'");
    assert_eq!(type_into_editor("SELECT ", "f(g(1))"), "SELECT f(g(1))");
}

/// An editable two-column table tab showing `rows` (code, flag) pairs.
fn app_with_editable_grid(rows: usize) -> DbGuiApp {
    let mut app = app_with_staged_edit();
    app.show_welcome = false;
    app.tab_mut().edits.cells.clear();
    app.tab_mut().set_result(QueryResult {
        columns: vec![
            ColumnMeta {
                name: "code".into(),
                type_name: "nvarchar".into(),
            },
            ColumnMeta {
                name: "flag".into(),
                type_name: "char".into(),
            },
        ],
        rows: (0..rows)
            .map(|r| {
                vec![
                    Value::Text(format!("C{r:03}")),
                    Value::Text(if r % 2 == 0 { "Y" } else { "N" }.into()),
                ]
            })
            .collect(),
        ..QueryResult::default()
    });
    app.tab_mut().edits.source = Some(EditSource {
        schema: None,
        table: "items".into(),
        pk_cols: vec!["code".into()],
    });
    app
}

/// A double-click on a grid cell opens its editor at a relaxed pace too. egui's default
/// window is 0.3s, stricter than the 0.5s macOS and Windows default to, and it times clicks
/// by frame: a heavy frame between the two clicks pushed ordinary double-clicks past it, so
/// editing failed now and then — mostly on wide SQL Server results. ~420ms must still count;
/// past the window, two clicks are two clicks.
#[test]
fn grid_double_click_follows_the_os_interval() {
    use egui_kittest::kittest::Queryable;
    for (gap_frames, expected) in [(1usize, true), (24, true), (36, false)] {
        let mut app = app_with_editable_grid(20);
        let opened = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = opened.clone();
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1200.0, 800.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
                if app.tab().edits.active.is_some() {
                    seen.set(true);
                }
            });
        harness.run_steps(4);
        let at = harness.get_by_label("C005").rect().center();
        let click = |harness: &mut egui_kittest::Harness<'_>| {
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                });
                harness.step();
            }
        };
        harness.hover_at(at);
        harness.step();
        click(&mut harness);
        for _ in 0..gap_frames {
            harness.step();
        }
        click(&mut harness);
        harness.run_steps(3);
        assert_eq!(
            opened.get(),
            expected,
            "gap of {gap_frames} frames at 60fps"
        );
    }
}

/// A query's clock and Cancel button stay out of sight for the first few seconds — most
/// queries are done by then, and controls that flash up and vanish for each of them read as
/// flicker — and appear at the bottom once it is clearly slow. The toolbar never grows a
/// Cancel button that shifts Beautify and Run sideways.
#[test]
fn slow_query_controls_wait_before_appearing() {
    use egui_kittest::kittest::Queryable;
    for (ran_for, shown) in [(0u64, false), (5, true)] {
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        app.connections.clear();
        connect_fake(&mut app, fake_schema(1, 2));
        app.tab_mut().sql = "SELECT 1".into();
        app.busy = Busy::Querying;
        let tab_id = app.tab().id;
        app.query_jobs.insert(
            tab_id,
            query::QueryJob {
                cancel: tokio_util::sync::CancellationToken::new(),
                running: true,
                started: std::time::Instant::now() - std::time::Duration::from_secs(ran_for),
            },
        );
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1200.0, 800.0))
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                app.draw(ui, None);
            });
        harness.run_steps(3);
        assert!(harness.query_by_label("Cancel query").is_none());
        assert_eq!(harness.query_by_label("Cancel").is_some(), shown, "after {ran_for}s");
        assert_eq!(
            harness.query_by_label_contains("Running").is_some(),
            shown,
            "after {ran_for}s"
        );
    }
}

/// Screenshot generator (ignored): the cursor cell with both fill handles.
#[test]
#[ignore = "screenshot generator; run manually with --ignored"]
fn snapshot_fill_handles() {
    let mut app = app_with_editable_grid(8);
    app.tab_mut().selection.select_one(3);
    app.tab_mut().selection.set_cursor(3, 0);
    let mut setup = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            if !setup {
                egui_extras::install_image_loaders(ui.ctx());
                crate::style::apply(ui.ctx());
                setup = true;
            }
            app.draw(ui, None);
        });
    harness.run_steps(4);
    harness.snapshot("fill_handles");
}
