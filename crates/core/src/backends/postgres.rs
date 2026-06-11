//! PostgreSQL backend implemented on top of `sqlx`.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::{AssertSqlSafe, Column, ConnectOptions, Row, TypeInfo, ValueRef};

use crate::database::{returns_rows, Database};
use crate::error::Result;
use crate::model::{
    ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, IndexInfo, QueryResult, QueryStats,
    SchemaTree, SslMode, TableInfo,
};
use crate::value::Value;

/// Schemas that are part of the server's machinery and not interesting to browse.
const SYSTEM_SCHEMAS: &str = "('pg_catalog', 'information_schema')";

pub struct PostgresDb {
    pool: PgPool,
}

impl PostgresDb {
    /// Open a pooled connection using the non-secret config plus an optional password
    /// (fetched from the keychain by the caller).
    pub async fn connect(cfg: &ConnectionConfig, password: Option<String>) -> Result<Self> {
        use sqlx::postgres::PgSslMode;
        let ssl_mode = match cfg.ssl_mode {
            SslMode::Disable => PgSslMode::Disable,
            SslMode::Prefer => PgSslMode::Prefer,
            SslMode::Require => PgSslMode::Require,
            SslMode::VerifyCa => PgSslMode::VerifyCa,
            SslMode::VerifyFull => PgSslMode::VerifyFull,
        };
        let mut opts = sqlx::postgres::PgConnectOptions::new()
            .host(&cfg.host)
            .port(cfg.port)
            .username(&cfg.user)
            .database(&cfg.database)
            .ssl_mode(ssl_mode);
        if !cfg.ssl_ca_cert.trim().is_empty() {
            opts = opts.ssl_root_cert(cfg.ssl_ca_cert.trim());
        }
        if !cfg.ssl_client_cert.trim().is_empty() {
            opts = opts.ssl_client_cert(cfg.ssl_client_cert.trim());
        }
        if !cfg.ssl_client_key.trim().is_empty() {
            opts = opts.ssl_client_key(cfg.ssl_client_key.trim());
        }
        if let Some(pw) = password {
            opts = opts.password(&pw);
        }
        // Quieten sqlx's statement logging; the UI surfaces errors itself.
        opts = opts.disable_statement_logging();
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl Database for PostgresDb {
    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        let database_name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&self.pool)
            .await?;

        // Build tables keyed by (schema, name) so we can attach columns/indexes by lookup.
        let mut tables: BTreeMap<(String, String), TableInfo> = BTreeMap::new();

        let table_rows: Vec<(String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT table_schema, table_name FROM information_schema.tables \
             WHERE table_type = 'BASE TABLE' AND table_schema NOT IN {SYSTEM_SCHEMAS} \
             ORDER BY table_schema, table_name"
        )))
        .fetch_all(&self.pool)
        .await?;
        for (schema, name) in table_rows {
            tables.insert(
                (schema.clone(), name.clone()),
                TableInfo {
                    schema: Some(schema),
                    name,
                    columns: Vec::new(),
                    indexes: Vec::new(),
                },
            );
        }

        // Columns (ordered by ordinal position).
        let col_rows: Vec<(String, String, String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT table_schema, table_name, column_name, data_type FROM information_schema.columns \
             WHERE table_schema NOT IN {SYSTEM_SCHEMAS} \
             ORDER BY table_schema, table_name, ordinal_position"
        )))
        .fetch_all(&self.pool)
        .await?;

        // Nullability is kept separate to keep the tuple small; query both at once instead.
        let null_rows: Vec<(String, String, String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT table_schema, table_name, column_name, is_nullable FROM information_schema.columns \
             WHERE table_schema NOT IN {SYSTEM_SCHEMAS}"
        )))
        .fetch_all(&self.pool)
        .await?;
        let mut nullable: BTreeMap<(String, String, String), bool> = BTreeMap::new();
        for (s, t, c, n) in null_rows {
            nullable.insert((s, t, c), n.eq_ignore_ascii_case("YES"));
        }

        // Primary-key columns.
        let pk_rows: Vec<(String, String, String)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT tc.table_schema, tc.table_name, kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name \
              AND tc.table_schema = kcu.table_schema \
             WHERE tc.constraint_type = 'PRIMARY KEY' \
               AND tc.table_schema NOT IN {SYSTEM_SCHEMAS}"
        )))
        .fetch_all(&self.pool)
        .await?;
        let mut pk_set: BTreeMap<(String, String, String), ()> = BTreeMap::new();
        for (s, t, c) in pk_rows {
            pk_set.insert((s, t, c), ());
        }

        for (schema, table, column, data_type) in col_rows {
            if let Some(info) = tables.get_mut(&(schema.clone(), table.clone())) {
                let key = (schema, table, column.clone());
                info.columns.push(ColumnInfo {
                    name: column,
                    data_type,
                    nullable: nullable.get(&key).copied().unwrap_or(true),
                    primary_key: pk_set.contains_key(&key),
                });
            }
        }

        // Indexes (parsed best-effort from pg_indexes definitions).
        let idx_rows: Vec<(String, String, String, String)> =
            sqlx::query_as(AssertSqlSafe(format!(
                "SELECT schemaname, tablename, indexname, indexdef FROM pg_indexes \
             WHERE schemaname NOT IN {SYSTEM_SCHEMAS} \
             ORDER BY schemaname, tablename, indexname"
            )))
            .fetch_all(&self.pool)
            .await?;
        for (schema, table, indexname, indexdef) in idx_rows {
            if let Some(info) = tables.get_mut(&(schema, table)) {
                info.indexes.push(IndexInfo {
                    name: indexname,
                    unique: indexdef.to_ascii_uppercase().contains("UNIQUE INDEX"),
                    columns: parse_index_columns(&indexdef),
                });
            }
        }

        Ok(SchemaTree {
            database_name,
            tables: tables.into_values().collect(),
        })
    }

    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        use futures_util::TryStreamExt;
        let start = Instant::now();
        if returns_rows(sql) {
            // Stream rows instead of fetch_all: a SELECT over a huge table materializes at
            // most `max_rows` rows; dropping the stream early cancels the rest of the fetch.
            let mut stream = sqlx::query(AssertSqlSafe(sql.to_string())).fetch(&self.pool);
            let mut columns: Vec<ColumnMeta> = Vec::new();
            // Upper-cased type names resolved once per result; `decode` dispatches on
            // these instead of re-uppercasing the type name for every cell.
            let mut types: Vec<String> = Vec::new();
            let mut data: Vec<Vec<Value>> = Vec::new();
            let mut truncated = false;
            while let Some(row) = stream.try_next().await? {
                if columns.is_empty() {
                    columns = column_meta(&row);
                    types = columns
                        .iter()
                        .map(|c| c.type_name.to_ascii_uppercase())
                        .collect();
                }
                if data.len() >= max_rows {
                    truncated = true;
                    break;
                }
                data.push((0..columns.len()).map(|i| decode(&row, i, &types[i])).collect());
            }
            Ok(QueryResult {
                columns,
                rows: data,
                stats: QueryStats {
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    rows_affected: None,
                },
                truncated,
            })
        } else {
            let res = sqlx::query(AssertSqlSafe(sql.to_string()))
                .execute(&self.pool)
                .await?;
            Ok(QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                stats: QueryStats {
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    rows_affected: Some(res.rows_affected()),
                },
                truncated: false,
            })
        }
    }

    async fn execute_transaction(&self, stmts: &[String]) -> Result<usize> {
        if stmts.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        for stmt in stmts {
            sqlx::query(AssertSqlSafe(stmt.as_str())).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(stmts.len())
    }

    async fn list_databases(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT datname FROM pg_database \
             WHERE datistemplate = false AND datallowconn = true \
             ORDER BY datname",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }
}

/// Build result-column metadata from a sample row.
fn column_meta(row: &PgRow) -> Vec<ColumnMeta> {
    row.columns()
        .iter()
        .map(|c| ColumnMeta {
            name: c.name().to_string(),
            type_name: c.type_info().name().to_string(),
        })
        .collect()
}

/// Decode one Postgres cell into a backend-agnostic [`Value`], dispatching on the
/// column's upper-cased type name (resolved once per result by the caller) and falling
/// back gracefully for types we don't special-case.
fn decode(row: &PgRow, idx: usize, name: &str) -> Value {
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }
    match name {
        "BOOL" => row
            .try_get::<bool, _>(idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "INT2" => row
            .try_get::<i16, _>(idx)
            .map(|v| Value::Int(v as i64))
            .unwrap_or(Value::Null),
        "INT4" => row
            .try_get::<i32, _>(idx)
            .map(|v| Value::Int(v as i64))
            .unwrap_or(Value::Null),
        "INT8" => row
            .try_get::<i64, _>(idx)
            .map(Value::Int)
            .unwrap_or(Value::Null),
        "FLOAT4" => row
            .try_get::<f32, _>(idx)
            .map(|v| Value::Float(v as f64))
            .unwrap_or(Value::Null),
        "FLOAT8" => row
            .try_get::<f64, _>(idx)
            .map(Value::Float)
            .unwrap_or(Value::Null),
        "NUMERIC" => row
            .try_get::<sqlx::types::BigDecimal, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TEXT" | "VARCHAR" | "BPCHAR" | "CHAR" | "NAME" | "CITEXT" => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .unwrap_or(Value::Null),
        "UUID" => row
            .try_get::<sqlx::types::Uuid, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => row
            .try_get::<serde_json::Value, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIMESTAMP" => row
            .try_get::<chrono::NaiveDateTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => row
            .try_get::<chrono::DateTime<chrono::Utc>, _>(idx)
            .map(|v| Value::Text(v.to_rfc3339()))
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "BYTEA" => row
            .try_get::<Vec<u8>, _>(idx)
            .map(Value::Bytes)
            .unwrap_or(Value::Null),
        _ => {
            // Unknown/extension type: try text, then bytes, else show the type name.
            row.try_get::<String, _>(idx)
                .map(Value::Text)
                .or_else(|_| row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes))
                .unwrap_or_else(|_| Value::Text(format!("<{}>", name.to_lowercase())))
        }
    }
}

/// Extract column names from the `(...)` portion of a `CREATE INDEX` definition.
fn parse_index_columns(indexdef: &str) -> Vec<String> {
    let Some(start) = indexdef.find('(') else {
        return Vec::new();
    };
    let Some(end) = indexdef.rfind(')') else {
        return Vec::new();
    };
    if end <= start + 1 {
        return Vec::new();
    }
    indexdef[start + 1..end]
        .split(',')
        .map(|c| c.trim().trim_matches('"').to_string())
        .filter(|c| !c.is_empty())
        .collect()
}
