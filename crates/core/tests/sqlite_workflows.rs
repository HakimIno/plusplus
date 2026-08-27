//! End-to-end SQLite workflows exercised through the public core API.

use std::sync::Arc;

use plusplus_core::*;

/// A unique temp-file path for a throwaway SQLite database. We use a real file rather
/// than `:memory:` because our connection pool opens several connections, and each
/// `:memory:` connection is an isolated database — a file is shared across the pool and
/// matches real usage.
fn temp_db_path() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "dbgui-test-{}-{}.sqlite",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// Build a temp-file SQLite connection for tests (no GUI). The returned guard removes
/// the file (and its -wal/-shm siblings) on drop.
async fn temp_db() -> (Arc<dyn Database>, TempDbGuard) {
    let path = temp_db_path();
    let mut cfg = ConnectionConfig::new(DbKind::Sqlite);
    cfg.sqlite_path = path.to_string_lossy().into_owned();
    let db = connect(&cfg, None, None)
        .await
        .expect("connect temp sqlite");
    (db, TempDbGuard(path))
}

struct TempDbGuard(std::path::PathBuf);
impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        for ext in ["-wal", "-shm"] {
            let mut p = self.0.clone().into_os_string();
            p.push(ext);
            let _ = std::fs::remove_file(p);
        }
    }
}

#[tokio::test]
async fn executes_select_and_decodes_values() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, raw BLOB)")
        .await
        .unwrap();
    db.execute(
        "INSERT INTO t (name, score, raw) VALUES ('สวัสดี', 9.5, x'00ff'), (NULL, NULL, NULL)",
    )
    .await
    .unwrap();

    let res = db
        .execute("SELECT id, name, score, raw FROM t ORDER BY id")
        .await
        .unwrap();
    assert_eq!(res.column_count(), 4);
    assert_eq!(res.row_count(), 2);

    // Row 1: integer id, Thai text preserved, float score, blob bytes.
    assert_eq!(res.rows[0][0], Value::Int(1));
    assert_eq!(res.rows[0][1], Value::Text("สวัสดี".to_string()));
    assert_eq!(res.rows[0][2], Value::Float(9.5));
    assert_eq!(res.rows[0][3], Value::Bytes(vec![0x00, 0xff]));

    // Row 2: NULLs decode as Value::Null.
    assert!(res.rows[1][1].is_null());
    assert!(res.rows[1][2].is_null());
}

#[tokio::test]
async fn generated_update_persists_blob_bytes() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE images (id INTEGER PRIMARY KEY, data BLOB)")
        .await
        .unwrap();
    db.execute("INSERT INTO images (id, data) VALUES (1, X'00')")
        .await
        .unwrap();

    let id = Value::Int(1);
    let image = Value::Bytes(vec![0x89, 0x50, 0x4e, 0x47, 0x00, 0xff]);
    let sql = build_update_sql(
        DbKind::Sqlite,
        None,
        "images",
        &[("data", &image)],
        &[("id", &id)],
    )
    .expect("SQLite BLOB update");
    db.execute(&sql).await.unwrap();

    let result = db
        .execute("SELECT data FROM images WHERE id = 1")
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![image]]);
}

#[tokio::test]
async fn select_stops_materializing_at_the_row_cap() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
    let values: Vec<String> = (1..=10).map(|i| format!("({i})")).collect();
    db.execute(&format!("INSERT INTO t (id) VALUES {}", values.join(", ")))
        .await
        .unwrap();

    let res = db.execute_capped("SELECT * FROM t", 4).await.unwrap();
    assert_eq!(res.row_count(), 4);
    assert!(res.truncated);
    assert_eq!(res.column_count(), 1); // metadata survives the cap

    let res = db.execute_capped("SELECT * FROM t", 100).await.unwrap();
    assert_eq!(res.row_count(), 10);
    assert!(!res.truncated);
}

/// End-to-end pager flow on a real (100k-row) table: rewrite the window, fetch one
/// page, count the total — the exact sequence the UI's pager performs.
#[tokio::test]
async fn paging_walks_a_big_table() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE big (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    db.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100000) \
         INSERT INTO big (id) SELECT i FROM n",
    )
    .await
    .unwrap();

    // Jump to the middle of the table: only that page is materialized.
    let sql = with_page_window(
        DbKind::Sqlite,
        "SELECT * FROM big LIMIT 1000;",
        1000,
        50_000,
    )
    .unwrap();
    assert_eq!(sql, "SELECT * FROM big LIMIT 1000 OFFSET 50000;");
    let page = db.execute_capped(&sql, 100_000).await.unwrap();
    assert_eq!(page.row_count(), 1000);
    assert!(!page.truncated);
    assert_eq!(page.rows[0][0], Value::Int(50_001));

    // The pager's total comes from a COUNT over the same FROM/WHERE.
    let count_sql = build_count_sql(&sql).unwrap();
    let total = db.execute(&count_sql).await.unwrap();
    assert_eq!(total.rows[0][0], Value::Int(100_000));

    // An unpaged SELECT over the same table stops at the cap instead of materializing
    // everything.
    let capped = db.execute_capped("SELECT * FROM big", 5_000).await.unwrap();
    assert_eq!(capped.row_count(), 5_000);
    assert!(capped.truncated);
}

#[tokio::test]
async fn dml_reports_rows_affected() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
    let res = db
        .execute("INSERT INTO t (id) VALUES (1), (2), (3)")
        .await
        .unwrap();
    assert_eq!(res.stats.rows_affected, Some(3));
    assert_eq!(res.row_count(), 0);
}

#[tokio::test]
async fn introspects_tables_columns_and_indexes() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)")
        .await
        .unwrap();
    db.execute("CREATE UNIQUE INDEX idx_users_email ON users (email)")
        .await
        .unwrap();

    let overview = db.introspect_overview().await.unwrap();
    let overview_users = overview
        .tables
        .iter()
        .find(|t| t.name == "users")
        .expect("users table present in overview");
    assert!(overview_users.columns.is_empty());
    assert!(overview_users.indexes.is_empty());

    let schema = db.introspect().await.unwrap();
    let users = schema
        .tables
        .iter()
        .find(|t| t.name == "users")
        .expect("users table present");

    assert_eq!(users.columns.len(), 2);
    let id = &users.columns[0];
    assert_eq!(id.name, "id");
    assert!(id.primary_key);
    let email = &users.columns[1];
    assert_eq!(email.name, "email");
    assert!(!email.nullable);

    let idx = users
        .indexes
        .iter()
        .find(|i| i.name == "idx_users_email")
        .expect("index present");
    assert!(idx.unique);
    assert_eq!(idx.columns, vec!["email".to_string()]);
}

#[tokio::test]
async fn introspects_foreign_keys() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE users (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    db.execute(
        "CREATE TABLE orders (\
             id INTEGER PRIMARY KEY, \
             user_id INTEGER REFERENCES users(id) ON DELETE CASCADE ON UPDATE SET NULL\
         )",
    )
    .await
    .unwrap();
    // Composite FK: both column pairs must come back as one constraint, in order.
    db.execute("CREATE TABLE pairs (a INTEGER, b INTEGER, PRIMARY KEY (a, b))")
        .await
        .unwrap();
    db.execute(
        "CREATE TABLE links (\
             x INTEGER, y INTEGER, \
             FOREIGN KEY (x, y) REFERENCES pairs (a, b)\
         )",
    )
    .await
    .unwrap();

    let schema = db.introspect().await.unwrap();
    let orders = schema.tables.iter().find(|t| t.name == "orders").unwrap();
    assert_eq!(orders.foreign_keys.len(), 1);
    let fk = &orders.foreign_keys[0];
    assert_eq!(fk.columns, vec!["user_id".to_string()]);
    assert_eq!(fk.ref_table, "users");
    assert_eq!(fk.ref_columns, vec!["id".to_string()]);
    assert_eq!(fk.on_delete, "CASCADE");
    assert_eq!(fk.on_update, "SET NULL");
    assert_eq!(fk.display(), "user_id → users(id)");

    let links = schema.tables.iter().find(|t| t.name == "links").unwrap();
    assert_eq!(links.foreign_keys.len(), 1);
    let fk = &links.foreign_keys[0];
    assert_eq!(fk.columns, vec!["x".to_string(), "y".to_string()]);
    assert_eq!(fk.ref_table, "pairs");
    assert_eq!(fk.ref_columns, vec!["a".to_string(), "b".to_string()]);

    // Tables without FKs stay empty.
    let users = schema.tables.iter().find(|t| t.name == "users").unwrap();
    assert!(users.foreign_keys.is_empty());
}

#[tokio::test]
async fn introspects_views_and_triggers() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)")
        .await
        .unwrap();
    db.execute("CREATE TABLE audit (msg TEXT)").await.unwrap();
    db.execute("CREATE VIEW v_pos AS SELECT id, n FROM t WHERE n > 0")
        .await
        .unwrap();
    db.execute(
        "CREATE TRIGGER trg_ins AFTER INSERT ON t FOR EACH ROW \
         WHEN NEW.n > 0 BEGIN INSERT INTO audit (msg) VALUES ('inserted'); END",
    )
    .await
    .unwrap();

    let schema = db.introspect().await.unwrap();

    // The view is split out of `tables` and carries its columns and defining SELECT.
    assert!(schema.tables.iter().all(|t| t.name != "v_pos"));
    let view = schema
        .views
        .iter()
        .find(|v| v.name == "v_pos")
        .expect("view present");
    assert_eq!(view.columns.len(), 2);
    assert!(view.definition.to_uppercase().contains("SELECT"));
    assert!(!view.materialized);

    // The trigger is parsed into structured fields from its stored DDL.
    let trg = schema
        .triggers
        .iter()
        .find(|t| t.name == "trg_ins")
        .expect("trigger present");
    assert_eq!(trg.timing, TriggerTiming::After);
    assert_eq!(trg.table, "t");
    assert_eq!(trg.events, vec![TriggerEvent::Insert]);
    assert!(trg.when_condition.is_some());

    // SQLite has no stored functions or procedures.
    assert!(schema.routines.is_empty());
}

async fn import_file(
    db: &dyn Database,
    path: &std::path::Path,
    fmt: import::ImportFormat,
    table: &str,
    targets: &[import::Target],
) -> std::result::Result<usize, String> {
    let reader = import::read_records(path, fmt, true).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for (i, rec) in reader.enumerate() {
        let rec = rec.map_err(|e| e.to_string())?;
        rows.push(import::coerce_row(&rec, targets, fmt, i + 1).map_err(|e| e.to_string())?);
    }
    let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    let stmts = import::build_insert_batches(DbKind::Sqlite, None, table, &names, &rows)
        .map_err(|e| e.to_string())?;
    db.execute_transaction(&stmts)
        .await
        .map(|_| rows.len())
        .map_err(|e| e.to_string())
}

fn target(name: &str, ty: &str, source: usize) -> import::Target {
    import::Target {
        name: name.to_string(),
        kind: coerce::EditorKind::classify(ty),
        source,
    }
}

/// The real round trip against a live database: export a table to CSV, wipe it, import the
/// file back, and confirm every value survived — including the ones CSV has to quote.
#[tokio::test]
async fn exported_csv_imports_back_into_an_identical_table() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, note TEXT)")
        .await
        .unwrap();
    // Values that exercise CSV quoting, unicode, and NULL.
    db.execute(
        "INSERT INTO t (id, name, score, note) VALUES \
         (1, 'plain', 1.5, 'ok'), \
         (2, 'has,comma and \"quotes\"', 2.25, NULL), \
         (3, 'line
break', 3.0, 'สวัสดี')",
    )
    .await
    .unwrap();

    let mut path = temp_db_path();
    path.set_extension("csv");
    let file = std::fs::File::create(&path).unwrap();
    let mut sink = ExportFormat::Csv.sink(std::io::BufWriter::new(file));
    let exported = db
        .export_query(
            "SELECT id, name, score, note FROM t ORDER BY id",
            &mut *sink,
        )
        .await
        .unwrap();
    drop(sink);
    assert_eq!(exported, 3);

    db.execute("DELETE FROM t").await.unwrap();
    assert_eq!(
        db.execute("SELECT COUNT(*) FROM t").await.unwrap().rows[0][0],
        Value::Int(0)
    );

    let targets = [
        target("id", "INTEGER", 0),
        target("name", "TEXT", 1),
        target("score", "REAL", 2),
        target("note", "TEXT", 3),
    ];
    let n = import_file(db.as_ref(), &path, import::ImportFormat::Csv, "t", &targets)
        .await
        .expect("import should succeed");
    assert_eq!(n, 3);

    let res = db
        .execute("SELECT id, name, score, note FROM t ORDER BY id")
        .await
        .unwrap();
    assert_eq!(res.row_count(), 3);
    assert_eq!(res.rows[0][1], Value::Text("plain".into()));
    assert_eq!(res.rows[0][2], Value::Float(1.5));
    assert_eq!(
        res.rows[1][1],
        Value::Text("has,comma and \"quotes\"".into()),
        "CSV quoting round-tripped"
    );
    assert!(res.rows[1][3].is_null(), "an empty CSV field is NULL again");
    assert_eq!(res.rows[2][1], Value::Text("line\nbreak".into()));
    assert_eq!(res.rows[2][3], Value::Text("สวัสดี".into()));

    let _ = std::fs::remove_file(&path);
}

/// A value the target column can't hold is caught before any SQL runs, and names the
/// offending row and column.
#[tokio::test]
async fn a_bad_value_aborts_the_import_before_touching_the_database() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)")
        .await
        .unwrap();

    let mut path = temp_db_path();
    path.set_extension("csv");
    std::fs::write(&path, "id,n\r\n1,10\r\n2,abc\r\n").unwrap();

    let targets = [target("id", "INTEGER", 0), target("n", "INTEGER", 1)];
    let err = import_file(db.as_ref(), &path, import::ImportFormat::Csv, "t", &targets)
        .await
        .expect_err("a non-integer in an INTEGER column must fail");
    assert!(err.contains("row 2"), "{err}");
    assert!(err.contains("column `n`"), "{err}");
    assert!(err.contains("an integer"), "{err}");

    let res = db.execute("SELECT COUNT(*) FROM t").await.unwrap();
    assert_eq!(res.rows[0][0], Value::Int(0), "nothing was written");

    let _ = std::fs::remove_file(&path);
}

/// A failure the database only discovers mid-transaction (a duplicate primary key) rolls
/// the whole import back: the pre-existing row survives, none of the file's rows land.
#[tokio::test]
async fn a_constraint_violation_rolls_the_whole_import_back() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)")
        .await
        .unwrap();
    db.execute("INSERT INTO t (id, n) VALUES (1, 100)")
        .await
        .unwrap();

    let mut path = temp_db_path();
    path.set_extension("csv");
    // id=2 is fine; id=1 collides with the row already there.
    std::fs::write(&path, "id,n\r\n2,200\r\n1,999\r\n").unwrap();

    let targets = [target("id", "INTEGER", 0), target("n", "INTEGER", 1)];
    let err = import_file(db.as_ref(), &path, import::ImportFormat::Csv, "t", &targets)
        .await
        .expect_err("duplicate primary key must fail");
    assert!(err.to_lowercase().contains("unique"), "{err}");

    let res = db.execute("SELECT id, n FROM t").await.unwrap();
    assert_eq!(res.row_count(), 1, "the valid row must not have been kept");
    assert_eq!(res.rows[0][0], Value::Int(1));
    assert_eq!(res.rows[0][1], Value::Int(100), "the old row is untouched");

    let _ = std::fs::remove_file(&path);
}

/// JSON imports too, with `null` distinguished from `""`.
#[tokio::test]
async fn json_imports_with_null_distinct_from_empty_string() {
    let (db, _guard) = temp_db().await;
    db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, note TEXT)")
        .await
        .unwrap();

    let mut path = temp_db_path();
    path.set_extension("json");
    std::fs::write(
        &path,
        r#"[{"id":1,"note":null},{"id":2,"note":""},{"id":3,"note":"hi"}]"#,
    )
    .unwrap();

    // serde_json sorts object keys: id, note.
    let targets = [target("id", "INTEGER", 0), target("note", "TEXT", 1)];
    let n = import_file(
        db.as_ref(),
        &path,
        import::ImportFormat::Json,
        "t",
        &targets,
    )
    .await
    .expect("json import should succeed");
    assert_eq!(n, 3);

    let res = db
        .execute("SELECT id, note FROM t ORDER BY id")
        .await
        .unwrap();
    assert!(res.rows[0][1].is_null(), "JSON null is SQL NULL");
    assert_eq!(
        res.rows[1][1],
        Value::Text(String::new()),
        "JSON \"\" stays an empty string, not NULL"
    );
    assert_eq!(res.rows[2][1], Value::Text("hi".into()));

    let _ = std::fs::remove_file(&path);
}
