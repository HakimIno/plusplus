//! MySQL/MariaDB backend implemented on top of `sqlx`.

use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::{AssertSqlSafe, Column, ConnectOptions, Row, TypeInfo, ValueRef};

use crate::database::{returns_rows, Database};
use crate::error::Result;
use crate::model::{
    ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, IndexInfo, QueryResult, QueryStats,
    SchemaTree, SslMode, TableInfo,
};
use crate::value::Value;

pub struct MySqlDb {
    pool: MySqlPool,
    kind: DbKind,
}

impl MySqlDb {
    pub async fn connect(cfg: &ConnectionConfig, password: Option<String>) -> Result<Self> {
        use sqlx::mysql::MySqlSslMode;
        let ssl_mode = match cfg.ssl_mode {
            SslMode::Disable => MySqlSslMode::Disabled,
            SslMode::Prefer => MySqlSslMode::Preferred,
            SslMode::Require => MySqlSslMode::Required,
            SslMode::VerifyCa => MySqlSslMode::VerifyCa,
            SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
        };
        let mut opts = sqlx::mysql::MySqlConnectOptions::new()
            .host(&cfg.host)
            .port(cfg.port)
            .username(&cfg.user)
            .database(&cfg.database)
            .ssl_mode(ssl_mode);
        if !cfg.ssl_ca_cert.trim().is_empty() {
            opts = opts.ssl_ca(cfg.ssl_ca_cert.trim());
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
        opts = opts.disable_statement_logging();
        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        Ok(Self {
            pool,
            kind: cfg.kind,
        })
    }
}

#[async_trait]
impl Database for MySqlDb {
    fn kind(&self) -> DbKind {
        self.kind
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        let database_name: String = sqlx::query_scalar("SELECT DATABASE()")
            .fetch_one(&self.pool)
            .await?;

        let mut tables: BTreeMap<String, TableInfo> = BTreeMap::new();
        let table_rows: Vec<(String,)> = sqlx::query_as(
            "SELECT TABLE_NAME \
             FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() \
               AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
             ORDER BY TABLE_NAME",
        )
        .fetch_all(&self.pool)
        .await?;

        for (name,) in table_rows {
            tables.insert(
                name.clone(),
                TableInfo {
                    schema: None,
                    name,
                    columns: Vec::new(),
                    indexes: Vec::new(),
                },
            );
        }

        let col_rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_KEY \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME, ORDINAL_POSITION",
        )
        .fetch_all(&self.pool)
        .await?;

        for (table, column, data_type, nullable, key) in col_rows {
            if let Some(info) = tables.get_mut(&table) {
                info.columns.push(ColumnInfo {
                    name: column,
                    data_type,
                    nullable: nullable.eq_ignore_ascii_case("YES"),
                    primary_key: key.eq_ignore_ascii_case("PRI"),
                });
            }
        }

        let idx_rows: Vec<(String, String, i32, String, i32)> = sqlx::query_as(
            "SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, COLUMN_NAME, SEQ_IN_INDEX \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut grouped: BTreeMap<(String, String), (bool, Vec<(i32, String)>)> = BTreeMap::new();
        for (table, index, non_unique, column, seq) in idx_rows {
            let entry = grouped
                .entry((table, index))
                .or_insert_with(|| (non_unique == 0, Vec::new()));
            entry.1.push((seq, column));
        }

        for ((table, name), (unique, mut columns)) in grouped {
            if let Some(info) = tables.get_mut(&table) {
                columns.sort_by_key(|(seq, _)| *seq);
                info.indexes.push(IndexInfo {
                    name,
                    unique,
                    columns: columns.into_iter().map(|(_, column)| column).collect(),
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
        let rows: Vec<(String,)> = sqlx::query_as("SHOW DATABASES")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }
}

fn column_meta(row: &MySqlRow) -> Vec<ColumnMeta> {
    row.columns()
        .iter()
        .map(|c| ColumnMeta {
            name: c.name().to_string(),
            type_name: c.type_info().name().to_string(),
        })
        .collect()
}

/// Decode one MySQL cell, dispatching on the column's upper-cased type name (resolved
/// once per result by the caller).
fn decode(row: &MySqlRow, idx: usize, name: &str) -> Value {
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }
    match name {
        "BOOLEAN" | "BOOL" => row
            .try_get::<bool, _>(idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "TINYINT" => row
            .try_get::<i8, _>(idx)
            .map(|v| Value::Int(v as i64))
            .or_else(|_| row.try_get::<u8, _>(idx).map(|v| Value::Int(v as i64)))
            .unwrap_or(Value::Null),
        "SMALLINT" => row
            .try_get::<i16, _>(idx)
            .map(|v| Value::Int(v as i64))
            .or_else(|_| row.try_get::<u16, _>(idx).map(|v| Value::Int(v as i64)))
            .unwrap_or(Value::Null),
        "MEDIUMINT" | "INT" | "INTEGER" => row
            .try_get::<i32, _>(idx)
            .map(|v| Value::Int(v as i64))
            .or_else(|_| row.try_get::<u32, _>(idx).map(|v| Value::Int(v as i64)))
            .unwrap_or(Value::Null),
        "BIGINT" => row
            .try_get::<i64, _>(idx)
            .map(Value::Int)
            .or_else(|_| {
                row.try_get::<u64, _>(idx)
                    .map(|v| Value::Text(v.to_string()))
            })
            .unwrap_or(Value::Null),
        "FLOAT" => row
            .try_get::<f32, _>(idx)
            .map(|v| Value::Float(v as f64))
            .unwrap_or(Value::Null),
        "DOUBLE" | "REAL" => row
            .try_get::<f64, _>(idx)
            .map(Value::Float)
            .unwrap_or(Value::Null),
        "DECIMAL" | "NEWDECIMAL" => row
            .try_get::<sqlx::types::BigDecimal, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM" | "SET"
        | "JSON" => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "DATETIME" | "TIMESTAMP" => row
            .try_get::<chrono::NaiveDateTime, _>(idx)
            .map(|v| Value::Text(v.to_string()))
            .unwrap_or(Value::Null),
        "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => row
            .try_get::<Vec<u8>, _>(idx)
            .map(Value::Bytes)
            .unwrap_or(Value::Null),
        _ => row
            .try_get::<String, _>(idx)
            .map(Value::Text)
            .or_else(|_| row.try_get::<Vec<u8>, _>(idx).map(Value::Bytes))
            .unwrap_or_else(|_| Value::Text(format!("<{}>", name.to_lowercase()))),
    }
}
