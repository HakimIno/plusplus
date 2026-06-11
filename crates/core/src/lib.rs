//! `core` — the backend-agnostic data layer for plusplus.
//!
//! This crate knows nothing about egui. It exposes:
//! - [`Database`]: the trait every backend implements.
//! - [`connect`]: a factory that returns an `Arc<dyn Database>` for a [`ConnectionConfig`].
//! - [`model`]: plain data types (configs, schema metadata, query results).
//! - [`config`] / [`secrets`]: persistence of connections (JSON) and passwords (keychain).
//!
//! Everything here is testable without a window (see the tests at the bottom of this file).

pub mod backends;
pub mod config;
pub mod database;
pub mod error;
pub mod model;
pub mod secrets;
pub mod value;

use std::sync::Arc;

pub use database::Database;
pub use error::{CoreError, Result};
pub use model::{
    build_add_column_sql, build_create_index_sql, build_create_table_sql, build_delete_sql,
    build_drop_column_sql, build_drop_index_sql, build_drop_table_sql, build_insert_sql,
    build_rename_column_sql, build_update_sql, simple_select_target, ColumnDef, ColumnInfo,
    ColumnMeta, ConnectionColor, ConnectionConfig, ConnectionIcon, DbKind, FkAction, ForeignKeyDef,
    IndexDef,
    IndexInfo, QueryResult, QueryStats, SchemaTree, SslMode, TableInfo,
};
pub use value::Value;

use backends::{mssql::MsSqlDb, mysql::MySqlDb, postgres::PostgresDb, sqlite::SqliteDb};

/// Connect to the database described by `cfg`, returning a shareable handle.
///
/// `password` is the secret fetched from the OS keychain by the caller (or `None` for
/// passwordless / file-based connections). Adding a new backend means adding a match arm
/// here and an implementation in [`backends`] — no UI changes required.
pub async fn connect(
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
        let db = connect(&cfg, None).await.expect("connect temp sqlite");
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
