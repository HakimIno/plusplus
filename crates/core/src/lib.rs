//! `core` — the backend-agnostic data layer for plusplus.
//!
//! This crate knows nothing about egui. It exposes:
//! - [`Database`]: the trait every backend implements.
//! - [`connect`]: a factory that returns an `Arc<dyn Database>` for a [`ConnectionConfig`].
//! - [`model`]: plain data types (configs, schema metadata, query results).
//! - [`config`] / [`secrets`]: persistence of connections (JSON) and passwords (keychain).
//!
//! Everything here is testable without a window (see the tests at the bottom of this file).

pub mod audit;
pub mod backends;
pub mod bookmarks;
pub mod clipboard;
pub mod coerce;
pub mod config;
pub mod database;
pub mod error;
pub mod export;
pub mod favorites;
pub mod history;
pub mod import;
pub mod model;
pub mod safety;
pub mod secrets;
pub mod tunnel;
pub mod value;

use std::sync::Arc;

pub use bookmarks::Bookmark;
pub use clipboard::{copy_rows, CopyFormat};
pub use coerce::{CoerceError, EditorKind};
pub use database::Database;
pub use error::{CoreError, Result};
pub use export::{ExportFormat, RowSink};
pub use favorites::Favorite;
pub use import::{ImportFormat, Preview, Record, Target};
pub use model::{
    build_add_column_sql, build_add_fk_sql, build_alter_column_sql, build_clone_table_sql,
    build_count_sql, build_create_index_sql, build_create_routine_sql, build_create_table_sql,
    build_create_trigger_sql, build_create_view_sql, build_delete_sql, build_drop_column_sql,
    build_drop_fk_sql, build_drop_index_sql, build_drop_routine_sql, build_drop_table_sql,
    build_drop_trigger_sql, build_drop_view_sql, build_insert_sql, build_rename_column_sql,
    build_select_where_sql, build_truncate_table_sql, build_update_sql, parse_page_window,
    parse_trigger_header, routine_supports_replace, select_body_after_as, simple_select_target,
    view_supports_replace, with_page_window, ColumnDef, ColumnInfo, ColumnMeta, ConnectionColor,
    ConnectionConfig, ConnectionIcon, DbKind, FkAction, ForeignKeyDef, ForeignKeyInfo, IndexDef,
    IndexInfo, PageWindow, ParamMode, QueryResult, QueryStats, RoutineBuild, RoutineInfo,
    RoutineKind, RoutineParam, SchemaTree, SslMode, TableInfo, TriggerBuild, TriggerEvent,
    TriggerInfo, TriggerLevel, TriggerTiming, ViewInfo,
};
pub use value::Value;

use backends::{mssql::MsSqlDb, mysql::MySqlDb, postgres::PostgresDb, sqlite::SqliteDb};

/// Connect to the database described by `cfg`, returning a shareable handle.
///
/// `password` and `ssh_secret` are the secrets fetched from the OS keychain by the
/// caller (or `None` for passwordless / file-based connections). With `ssh_enabled`,
/// an SSH tunnel to the bastion is opened first and the backend connects through it;
/// the tunnel lives exactly as long as the returned handle. Adding a new backend means
/// adding a match arm to [`connect_direct`] and an implementation in [`backends`].
pub async fn connect(
    cfg: &ConnectionConfig,
    password: Option<String>,
    ssh_secret: Option<String>,
) -> Result<Arc<dyn Database>> {
    if cfg.ssh_enabled && cfg.kind.is_server() {
        let tun = tunnel::SshTunnel::open(cfg, ssh_secret.as_deref()).await?;
        // The driver dials the tunnel's loopback end instead of the real host. Note for
        // TLS: verify-full then checks the certificate against the *original* hostname
        // only if the driver pins it — with a tunnel, prefer verify-ca.
        let mut local = cfg.clone();
        local.host = "127.0.0.1".to_string();
        local.port = tun.local_port;
        let inner = connect_direct(&local, password).await?;
        return Ok(Arc::new(Tunneled {
            inner,
            _tunnel: tun,
        }));
    }
    connect_direct(cfg, password).await
}

/// Connect straight to `cfg.host:cfg.port` (or the SQLite file) with no tunnel.
async fn connect_direct(
    cfg: &ConnectionConfig,
    password: Option<String>,
) -> Result<Arc<dyn Database>> {
    match cfg.kind {
        DbKind::Postgres => Ok(Arc::new(PostgresDb::connect(cfg, password).await?)),
        DbKind::MySql | DbKind::MariaDb => Ok(Arc::new(MySqlDb::connect(cfg, password).await?)),
        DbKind::SqlServer => Ok(Arc::new(MsSqlDb::connect(cfg, password).await?)),
        DbKind::Sqlite => {
            if cfg.sqlite_path.trim().is_empty() {
                return Err(CoreError::InvalidConfig("SQLite path is empty".into()));
            }
            Ok(Arc::new(SqliteDb::connect(cfg).await?))
        }
    }
}

/// A backend riding an SSH tunnel: delegates everything and keeps the tunnel alive for
/// as long as the connection itself.
struct Tunneled {
    inner: Arc<dyn Database>,
    _tunnel: tunnel::SshTunnel,
}

#[async_trait::async_trait]
impl Database for Tunneled {
    fn kind(&self) -> DbKind {
        self.inner.kind()
    }
    async fn introspect(&self) -> Result<model::SchemaTree> {
        self.inner.introspect().await
    }
    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        self.inner.execute_capped(sql, max_rows).await
    }
    async fn execute_capped_cancellable(
        &self,
        sql: &str,
        max_rows: usize,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<QueryResult> {
        self.inner
            .execute_capped_cancellable(sql, max_rows, cancel)
            .await
    }
    async fn execute_transaction(&self, stmts: &[String]) -> Result<usize> {
        self.inner.execute_transaction(stmts).await
    }
    async fn export_query(
        &self,
        sql: &str,
        sink: &mut (dyn export::RowSink + Send),
    ) -> Result<u64> {
        self.inner.export_query(sql, sink).await
    }
    async fn list_databases(&self) -> Result<Vec<String>> {
        self.inner.list_databases().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn value_sorting_handles_nulls_and_numbers() {
        let mut v = vec![Value::Int(3), Value::Null, Value::Int(1), Value::Float(2.0)];
        v.sort_by(|a, b| a.sort_cmp(b));
        assert_eq!(v[0], Value::Int(1));
        assert_eq!(v[1], Value::Float(2.0));
        assert_eq!(v[2], Value::Int(3));
        assert!(v[3].is_null()); // NULL sorts last
    }

    #[test]
    fn build_update_escapes_and_quotes_per_dialect() {
        use model::build_update_sql;

        // Postgres: ANSI double-quoted identifiers, doubled single-quotes in strings.
        let sql = build_update_sql(
            DbKind::Postgres,
            Some("public"),
            "users",
            &[
                ("name", &Value::Text("O'Brien".into())),
                ("age", &Value::Int(30)),
            ],
            &[("id", &Value::Int(7))],
        )
        .unwrap();
        assert_eq!(
            sql,
            r#"UPDATE "public"."users" SET "name" = 'O''Brien', "age" = 30 WHERE "id" = 7;"#
        );

        // MySQL: backtick identifiers, and backslashes are escaped in strings.
        let sql = build_update_sql(
            DbKind::MySql,
            None,
            "logs",
            &[("path", &Value::Text(r"C:\tmp".into()))],
            &[("id", &Value::Int(1))],
        )
        .unwrap();
        assert_eq!(sql, r"UPDATE `logs` SET `path` = 'C:\\tmp' WHERE `id` = 1;");

        // A NULL key matches with IS NULL, and binary SET values are rejected.
        let sql = build_update_sql(
            DbKind::Sqlite,
            None,
            "t",
            &[("v", &Value::Null)],
            &[("k", &Value::Null)],
        )
        .unwrap();
        assert_eq!(sql, r#"UPDATE "t" SET "v" = NULL WHERE "k" IS NULL;"#);
        assert!(build_update_sql(
            DbKind::Sqlite,
            None,
            "t",
            &[("v", &Value::Bytes(vec![1, 2]))],
            &[("k", &Value::Int(1))]
        )
        .is_none());
    }

    /// Coerce every record of `path` against `targets` and insert them in one transaction —
    /// exactly what the UI's `run_import` does, minus the progress messages.
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
            .export_query("SELECT id, name, score, note FROM t ORDER BY id", &mut *sink)
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
        let n = import_file(db.as_ref(), &path, import::ImportFormat::Json, "t", &targets)
            .await
            .expect("json import should succeed");
        assert_eq!(n, 3);

        let res = db.execute("SELECT id, note FROM t ORDER BY id").await.unwrap();
        assert!(res.rows[0][1].is_null(), "JSON null is SQL NULL");
        assert_eq!(
            res.rows[1][1],
            Value::Text(String::new()),
            "JSON \"\" stays an empty string, not NULL"
        );
        assert_eq!(res.rows[2][1], Value::Text("hi".into()));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn returns_rows_classifies_statements() {
        use database::returns_rows;
        assert!(returns_rows("SELECT 1"));
        assert!(returns_rows("  with x as (select 1) select * from x"));
        assert!(returns_rows("DESCRIBE users"));
        assert!(returns_rows("PRAGMA table_info(t)"));
        assert!(!returns_rows("INSERT INTO t VALUES (1)"));
        assert!(!returns_rows("update t set a = 1"));
    }
}
