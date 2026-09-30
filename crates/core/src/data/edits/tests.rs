use super::*;
use crate::ColumnMeta;

fn source() -> EditSource {
    EditSource {
        schema: None,
        table: "items".into(),
        pk_cols: vec!["id".into()],
    }
}

#[test]
fn editable_source_checks_the_entire_statement() {
    for sql in [
        "SELECT * FROM items WHERE id > 0 UNION ALL SELECT * FROM other",
        "SELECT * FROM items WHERE id > 0 GROUP BY id",
        "SELECT * FROM items WHERE id > 0 HAVING count(*) > 1",
        "SELECT * FROM items WHERE id > 0; DELETE FROM items",
        "SELECT * FROM items WHERE",
    ] {
        assert!(
            editable_select_target(DbKind::Sqlite, sql).is_none(),
            "{sql}"
        );
    }
    for (kind, sql) in [
        (
            DbKind::Sqlite,
            "SELECT * FROM items WHERE id > 0 ORDER BY id LIMIT 10",
        ),
        (
            DbKind::Postgres,
            "SELECT * FROM public.items WHERE id IN (SELECT id FROM other) LIMIT 10",
        ),
        (DbKind::MySql, "SELECT * FROM `items` LIMIT 10 OFFSET 20"),
        (
            DbKind::SqlServer,
            "SELECT TOP 10 * FROM [dbo].[items] WHERE id > 0",
        ),
        (DbKind::DuckDb, "SELECT * FROM items LIMIT 10"),
        (DbKind::Cassandra, "SELECT * FROM items WHERE id = 1"),
    ] {
        assert!(
            editable_select_target(kind, sql).is_some(),
            "{kind:?}: {sql}"
        );
    }
}

fn result() -> QueryResult {
    QueryResult {
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
        rows: vec![vec![Value::Int(1), Value::Text("old".into())]],
        ..QueryResult::default()
    }
}

fn plan(
    result: &QueryResult,
    cells: &HashMap<usize, HashMap<usize, Value>>,
    deleted: &HashSet<usize>,
    new_rows: usize,
) -> Result<CommitPlan, EditError> {
    plan_edits(
        DbKind::Sqlite,
        EditBatch {
            source: &source(),
            result,
            cells,
            deleted,
            new_rows,
            generated_columns: &[],
        },
    )
}

#[test]
fn deterministic_order_and_deleted_rows_are_not_updated() {
    let mut rows = result();
    rows.rows
        .push(vec![Value::Int(2), Value::Text("two".into())]);
    let cells = HashMap::from([
        (1, HashMap::from([(1, Value::Text("ignored".into()))])),
        (
            0,
            HashMap::from([(1, Value::Text("O'Brien".into())), (0, Value::Int(3))]),
        ),
        (
            NEW_ROW_BASE,
            HashMap::from([(1, Value::Text("new".into())), (0, Value::Int(2))]),
        ),
    ]);
    let sql = plan(&rows, &cells, &HashSet::from([1]), 1)
        .unwrap()
        .statements;
    assert_eq!(
        sql,
        [
            "UPDATE \"items\" SET \"id\" = 3, \"name\" = 'O''Brien' WHERE \"id\" = 1 AND \"name\" = 'old';",
            "DELETE FROM \"items\" WHERE \"id\" = 2 AND \"name\" = 'two';",
            "INSERT INTO \"items\" (\"id\", \"name\") VALUES (2, 'new');",
        ]
    );
}

#[test]
fn invalid_indices_return_errors_instead_of_panicking() {
    assert_eq!(
        plan(
            &result(),
            &HashMap::from([(5, HashMap::from([(1, Value::Null)]))]),
            &HashSet::new(),
            0
        ),
        Err(EditError::InvalidRow(5))
    );
    assert_eq!(
        plan(
            &result(),
            &HashMap::from([(0, HashMap::from([(9, Value::Null)]))]),
            &HashSet::new(),
            0
        ),
        Err(EditError::InvalidColumn(9))
    );
    assert_eq!(
        plan(&result(), &HashMap::new(), &HashSet::from([5]), 0),
        Err(EditError::InvalidRow(5))
    );
    assert_eq!(
        plan(
            &result(),
            &HashMap::from([(NEW_ROW_BASE, HashMap::from([(0, Value::Int(2))]))]),
            &HashSet::new(),
            0
        ),
        Err(EditError::InvalidRow(NEW_ROW_BASE))
    );
    let mut ragged = result();
    ragged.rows[0].clear();
    assert_eq!(
        plan(&ragged, &HashMap::new(), &HashSet::from([0]), 0),
        Err(EditError::InvalidRow(0))
    );
}

#[test]
fn core_validates_types_without_a_ui() {
    assert!(matches!(
        plan(
            &result(),
            &HashMap::from([(0, HashMap::from([(0, Value::Text("bad".into()))]))]),
            &HashSet::new(),
            0
        ),
        Err(EditError::InvalidValue { row: 0, column: 0 })
    ));
}

#[test]
fn inserts_reject_missing_null_and_duplicate_keys() {
    for value in [None, Some(Value::Null), Some(Value::Int(1))] {
        let mut entered = HashMap::from([(1, Value::Text("new".into()))]);
        if let Some(value) = value {
            entered.insert(0, value);
        }
        assert!(plan(
            &result(),
            &HashMap::from([(NEW_ROW_BASE, entered)]),
            &HashSet::new(),
            1
        )
        .is_err());
    }
    let cells = HashMap::from([
        (NEW_ROW_BASE, HashMap::from([(0, Value::Int(2))])),
        (NEW_ROW_BASE + 1, HashMap::from([(0, Value::Int(2))])),
    ]);
    assert_eq!(
        plan(&result(), &cells, &HashSet::new(), 2),
        Err(EditError::DuplicatePrimaryKey)
    );
}

#[test]
fn generated_primary_key_is_omitted_when_inserted_row_leaves_it_blank() {
    let cells = HashMap::from([(
        NEW_ROW_BASE,
        HashMap::from([(0, Value::Null), (1, Value::Text("new".into()))]),
    )]);
    let plan = plan_edits(
        DbKind::Sqlite,
        EditBatch {
            source: &source(),
            result: &result(),
            cells: &cells,
            deleted: &HashSet::new(),
            new_rows: 1,
            generated_columns: &[0],
        },
    )
    .unwrap();
    assert_eq!(
        plan.statements,
        ["INSERT INTO \"items\" (\"name\") VALUES ('new');"]
    );
}

#[test]
fn inserts_use_updated_primary_keys() {
    let mut cells = HashMap::from([
        (0, HashMap::from([(0, Value::Int(2))])),
        (NEW_ROW_BASE, HashMap::from([(0, Value::Int(1))])),
    ]);
    assert_eq!(
        plan(&result(), &cells, &HashSet::new(), 1)
            .unwrap()
            .statements
            .len(),
        2
    );
    cells
        .get_mut(&NEW_ROW_BASE)
        .unwrap()
        .insert(0, Value::Int(2));
    assert_eq!(
        plan(&result(), &cells, &HashSet::new(), 1),
        Err(EditError::DuplicatePrimaryKey)
    );
}

#[test]
fn update_only_does_not_scan_unrelated_rows_even_with_blank_new_slots() {
    let mut rows = result();
    // An unrelated malformed row would fail PK indexing if planning scanned it.
    rows.rows.push(Vec::new());
    let cells = HashMap::from([(0, HashMap::from([(1, Value::Text("changed".into()))]))]);
    assert_eq!(
        plan(&rows, &cells, &HashSet::new(), 100_000)
            .unwrap()
            .statements
            .len(),
        1
    );
    assert!(plan(&rows, &HashMap::new(), &HashSet::new(), 100_000)
        .unwrap()
        .statements
        .is_empty());
}

#[test]
fn keys_are_required_even_for_insert_only_batches() {
    let mut source = source();
    source.pk_cols.clear();
    assert_eq!(
        plan_edits(
            DbKind::Sqlite,
            EditBatch {
                source: &source,
                result: &result(),
                cells: &HashMap::new(),
                deleted: &HashSet::new(),
                new_rows: 0,
                generated_columns: &[],
            }
        ),
        Err(EditError::NoPrimaryKey)
    );
    let mut rows = result();
    rows.columns[1].name = "id".into();
    assert_eq!(
        plan(&rows, &HashMap::new(), &HashSet::new(), 0),
        Err(EditError::PrimaryKeyColumns)
    );
}

#[test]
fn composite_binary_and_float_keys_preserve_equality() {
    let source = EditSource {
        pk_cols: vec!["a".into(), "b".into()],
        ..source()
    };
    let rows = QueryResult {
        columns: vec![
            ColumnMeta {
                name: "a".into(),
                type_name: "REAL".into(),
            },
            ColumnMeta {
                name: "b".into(),
                type_name: "BLOB".into(),
            },
        ],
        rows: vec![vec![Value::Float(-0.0), Value::Bytes(vec![1, 2])]],
        ..QueryResult::default()
    };
    let mut cells = HashMap::from([(
        NEW_ROW_BASE,
        HashMap::from([(0, Value::Float(0.0)), (1, Value::Bytes(vec![1, 2]))]),
    )]);
    let deleted = HashSet::new();
    assert_eq!(
        plan_edits(
            DbKind::Sqlite,
            EditBatch {
                source: &source,
                result: &rows,
                cells: &cells,
                deleted: &deleted,
                new_rows: 1,
                generated_columns: &[],
            }
        ),
        Err(EditError::DuplicatePrimaryKey)
    );
    cells
        .get_mut(&NEW_ROW_BASE)
        .unwrap()
        .insert(1, Value::Bytes(vec![1, 3]));
    assert!(plan_edits(
        DbKind::Sqlite,
        EditBatch {
            source: &source,
            result: &rows,
            cells: &cells,
            deleted: &deleted,
            new_rows: 1,
            generated_columns: &[],
        }
    )
    .is_ok());
}

#[test]
fn optimistic_update_contains_original_changed_value_in_where_clause() {
    let result = result();
    let cells = HashMap::from([(0, HashMap::from([(1, Value::Text("ours".into()))]))]);
    let plan = plan(&result, &cells, &HashSet::new(), 0).unwrap();
    assert_eq!(
        plan.statements[0],
        "UPDATE \"items\" SET \"name\" = 'ours' WHERE \"id\" = 1 AND \"name\" = 'old';"
    );
}
