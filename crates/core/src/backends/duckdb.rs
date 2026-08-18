//! Embedded DuckDB backend for analytical files and in-memory workloads.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use async_trait::async_trait;
use duckdb::types::{TimeUnit, Value as DuckValue};
use duckdb::{AccessMode, Config, Connection};
use tokio_util::sync::CancellationToken;

use crate::database::{returns_rows, Database};
use crate::error::{CoreError, Result};
use crate::model::{
    ColumnInfo, ColumnMeta, ConnectionConfig, DbKind, ForeignKeyInfo, IndexInfo, QueryResult,
    QueryStats, SchemaTree, TableInfo, ViewInfo,
};
use crate::value::Value;

pub struct DuckDb {
    connection: Mutex<Connection>,
    operation: tokio::sync::Mutex<()>,
    interrupt: std::sync::Arc<duckdb::InterruptHandle>,
    name: String,
}

fn blocking<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

impl DuckDb {
    pub fn connect(cfg: &ConnectionConfig) -> Result<Self> {
        let flags = Config::default().access_mode(if cfg.is_read_only() {
            AccessMode::ReadOnly
        } else {
            AccessMode::Automatic
        })?;
        let connection = Connection::open_with_flags(&cfg.duckdb_path, flags)?;
        let name = if cfg.duckdb_path == ":memory:" {
            "memory".to_string()
        } else {
            Path::new(&cfg.duckdb_path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "duckdb".to_string())
        };
        let interrupt = connection.interrupt_handle();
        Ok(Self {
            connection: Mutex::new(connection),
            operation: tokio::sync::Mutex::new(()),
            interrupt,
            name,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| CoreError::Pool("DuckDB connection lock was poisoned".into()))
    }

    fn execute_sync(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        let start = Instant::now();
        let connection = self.lock()?;
        if !returns_rows(sql) {
            connection.execute_batch(sql)?;
            return Ok(QueryResult {
                stats: QueryStats {
                    elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                    rows_affected: Some(0),
                },
                ..QueryResult::default()
            });
        }

        let mut statement = connection.prepare(sql)?;
        let mut rows = statement.query([])?;
        let columns = statement_columns(
            rows.as_ref()
                .ok_or_else(|| CoreError::Pool("DuckDB query lost its statement".into()))?,
        );
        let mut data = Vec::with_capacity(super::initial_row_capacity(max_rows));
        let mut truncated = false;
        while let Some(row) = rows.next()? {
            if data.len() >= max_rows {
                truncated = true;
                break;
            }
            data.push(decode_row(row, columns.len())?);
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
    }

    fn introspect_sync(&self, overview_only: bool) -> Result<SchemaTree> {
        let connection = self.lock()?;
        let mut object_statement = connection.prepare(
            "SELECT table_schema, table_name, table_type \
             FROM information_schema.tables \
             WHERE table_schema NOT IN ('information_schema', 'pg_catalog') \
             ORDER BY table_schema, table_name",
        )?;
        let objects = object_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut tables = Vec::new();
        let mut views = Vec::new();
        for object in objects {
            let (schema, name, kind) = object?;
            let schema = (schema != "main").then_some(schema);
            if kind.eq_ignore_ascii_case("VIEW") {
                views.push(ViewInfo {
                    schema,
                    name,
                    columns: Vec::new(),
                    definition: String::new(),
                    materialized: false,
                });
            } else {
                tables.push(TableInfo {
                    schema,
                    name,
                    columns: Vec::new(),
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                });
            }
        }
        if overview_only {
            return Ok(SchemaTree {
                database_name: self.name.clone(),
                tables,
                views,
                routines: Vec::new(),
                triggers: Vec::new(),
            });
        }

        let mut primary_keys = HashSet::new();
        let mut pk_statement = connection.prepare(
            "SELECT kcu.table_schema, kcu.table_name, kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_catalog = kcu.constraint_catalog \
              AND tc.constraint_schema = kcu.constraint_schema \
              AND tc.constraint_name = kcu.constraint_name \
             WHERE tc.constraint_type = 'PRIMARY KEY'",
        )?;
        for row in pk_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            primary_keys.insert(row?);
        }

        let mut columns_by_table: HashMap<(String, String), Vec<ColumnInfo>> = HashMap::new();
        let mut column_statement = connection.prepare(
            "SELECT table_schema, table_name, column_name, data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema NOT IN ('information_schema', 'pg_catalog') \
             ORDER BY table_schema, table_name, ordinal_position",
        )?;
        let column_rows = column_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in column_rows {
            let (schema, table, name, data_type, nullable, default) = row?;
            let primary_key = primary_keys.contains(&(schema.clone(), table.clone(), name.clone()));
            columns_by_table
                .entry((schema, table))
                .or_default()
                .push(ColumnInfo {
                    name,
                    data_type,
                    nullable: nullable.eq_ignore_ascii_case("YES"),
                    primary_key,
                    default,
                    check: None,
                    comment: None,
                });
        }
        for table in &mut tables {
            let schema = table.schema.clone().unwrap_or_else(|| "main".into());
            table.columns = columns_by_table
                .remove(&(schema, table.name.clone()))
                .unwrap_or_default();
        }
        for view in &mut views {
            let schema = view.schema.clone().unwrap_or_else(|| "main".into());
            view.columns = columns_by_table
                .remove(&(schema, view.name.clone()))
                .unwrap_or_default();
        }
        let mut index_statement = connection.prepare(
            "SELECT schema_name, table_name, index_name, is_unique, sql \
             FROM duckdb_indexes() WHERE schema_name NOT IN ('information_schema', 'pg_catalog')",
        )?;
        for row in index_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, String>(4)?,
            ))
        })? {
            let (schema, table_name, name, unique, sql) = row?;
            if let Some(table) = tables.iter_mut().find(|table| {
                table.name == table_name && table.schema.as_deref().unwrap_or("main") == schema
            }) {
                table.indexes.push(IndexInfo {
                    name,
                    unique,
                    columns: index_columns(&sql),
                });
            }
        }

        let mut foreign_key_statement = connection.prepare(
            "SELECT schema_name, table_name, constraint_name, \
                    array_to_string(constraint_column_names, chr(31)), referenced_table, \
                    array_to_string(referenced_column_names, chr(31)) \
             FROM duckdb_constraints() WHERE constraint_type = 'FOREIGN KEY'",
        )?;
        for row in foreign_key_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })? {
            let (schema, table_name, name, columns, ref_table, ref_columns) = row?;
            if let Some(table) = tables.iter_mut().find(|table| {
                table.name == table_name && table.schema.as_deref().unwrap_or("main") == schema
            }) {
                table.foreign_keys.push(ForeignKeyInfo {
                    name,
                    columns: columns.split('\u{1f}').map(ToString::to_string).collect(),
                    ref_schema: (schema != "main").then_some(schema),
                    ref_table,
                    ref_columns: ref_columns
                        .split('\u{1f}')
                        .map(ToString::to_string)
                        .collect(),
                    on_delete: "NO ACTION".to_string(),
                    on_update: "NO ACTION".to_string(),
                });
            }
        }
        let mut view_statement = connection.prepare(
            "SELECT schema_name, view_name, sql FROM duckdb_views() \
             WHERE internal = FALSE",
        )?;
        for row in view_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (schema, name, definition) = row?;
            if let Some(view) = views.iter_mut().find(|view| {
                view.name == name && view.schema.as_deref().unwrap_or("main") == schema
            }) {
                view.definition = crate::model::select_body_after_as(&definition);
            }
        }
        Ok(SchemaTree {
            database_name: self.name.clone(),
            tables,
            views,
            routines: Vec::new(),
            triggers: Vec::new(),
        })
    }

    fn export_sync(
        &self,
        sql: &str,
        sink: &mut (dyn crate::export::RowSink + Send),
    ) -> Result<u64> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(sql)?;
        let mut rows = statement.query([])?;
        let columns = statement_columns(
            rows.as_ref()
                .ok_or_else(|| CoreError::Pool("DuckDB query lost its statement".into()))?,
        );
        sink.begin(&columns)?;
        let mut count = 0u64;
        while let Some(row) = rows.next()? {
            let values = decode_row(row, columns.len())?;
            sink.write_row(&values)?;
            count += 1;
        }
        sink.finish()?;
        Ok(count)
    }
}

#[async_trait]
impl Database for DuckDb {
    fn kind(&self) -> DbKind {
        DbKind::DuckDb
    }

    async fn introspect_overview(&self) -> Result<SchemaTree> {
        let _operation = self.operation.lock().await;
        blocking(|| self.introspect_sync(true))
    }

    async fn introspect(&self) -> Result<SchemaTree> {
        let _operation = self.operation.lock().await;
        blocking(|| self.introspect_sync(false))
    }

    async fn execute_capped(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        let _operation = self.operation.lock().await;
        blocking(|| self.execute_sync(sql, max_rows))
    }

    async fn execute_capped_cancellable(
        &self,
        sql: &str,
        max_rows: usize,
        cancel: CancellationToken,
    ) -> Result<QueryResult> {
        if cancel.is_cancelled() {
            return Err(CoreError::Canceled);
        }
        let _operation = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Canceled),
            operation = self.operation.lock() => operation,
        };
        let interrupt = self.interrupt.clone();
        let finished = CancellationToken::new();
        let finished_signal = finished.clone();
        let cancel_signal = cancel.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = cancel_signal.cancelled() => interrupt.interrupt(),
                _ = finished_signal.cancelled() => {}
            }
        });
        let result = blocking(|| self.execute_sync(sql, max_rows));
        finished.cancel();
        watcher.abort();
        if cancel.is_cancelled() {
            Err(CoreError::Canceled)
        } else {
            result
        }
    }

    async fn execute_transaction(&self, statements: &[String]) -> Result<usize> {
        if statements.is_empty() {
            return Ok(0);
        }
        let _operation = self.operation.lock().await;
        blocking(|| {
            let mut connection = self.lock()?;
            let transaction = connection.transaction()?;
            for statement in statements {
                transaction.execute_batch(statement)?;
            }
            transaction.commit()?;
            Ok(statements.len())
        })
    }

    async fn export_query(
        &self,
        sql: &str,
        sink: &mut (dyn crate::export::RowSink + Send),
    ) -> Result<u64> {
        let _operation = self.operation.lock().await;
        blocking(|| self.export_sync(sql, sink))
    }

    async fn export_query_cancellable(
        &self,
        sql: &str,
        cancel: CancellationToken,
        sink: &mut (dyn crate::export::RowSink + Send),
    ) -> Result<u64> {
        if cancel.is_cancelled() {
            return Err(CoreError::Canceled);
        }
        let _operation = tokio::select! {
            _ = cancel.cancelled() => return Err(CoreError::Canceled),
            operation = self.operation.lock() => operation,
        };
        let interrupt = self.interrupt.clone();
        let finished = CancellationToken::new();
        let finished_signal = finished.clone();
        let cancel_signal = cancel.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = cancel_signal.cancelled() => interrupt.interrupt(),
                _ = finished_signal.cancelled() => {}
            }
        });
        let result = blocking(|| self.export_sync(sql, sink));
        finished.cancel();
        watcher.abort();
        if cancel.is_cancelled() {
            Err(CoreError::Canceled)
        } else {
            result
        }
    }
}

fn index_columns(sql: &str) -> Vec<String> {
    let Some(start) = sql.find('(') else {
        return Vec::new();
    };
    let Some(end) = sql.rfind(')').filter(|end| *end > start) else {
        return Vec::new();
    };
    let body = &sql[start + 1..end];
    let mut columns = Vec::new();
    let mut field_start = 0usize;
    let mut depth = 0usize;
    let mut quote = None;
    for (index, character) in body.char_indices() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                columns.push(clean_index_column(&body[field_start..index]));
                field_start = index + 1;
            }
            _ => {}
        }
    }
    columns.push(clean_index_column(&body[field_start..]));
    columns
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect()
}

fn clean_index_column(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character| matches!(character, '"' | '`'))
        .to_string()
}

fn statement_columns(statement: &duckdb::Statement<'_>) -> Vec<ColumnMeta> {
    (0..statement.column_count())
        .map(|index| ColumnMeta {
            name: statement
                .column_name(index)
                .map(ToString::to_string)
                .unwrap_or_else(|_| format!("column_{}", index + 1)),
            type_name: format!("{:?}", statement.column_type(index)),
        })
        .collect()
}

fn decode_row(row: &duckdb::Row<'_>, column_count: usize) -> Result<Vec<Value>> {
    (0..column_count)
        .map(|index| {
            row.get::<_, DuckValue>(index)
                .map(decode_value)
                .map_err(Into::into)
        })
        .collect()
}

fn decode_value(value: DuckValue) -> Value {
    match value {
        DuckValue::Null => Value::Null,
        DuckValue::Boolean(value) => Value::Bool(value),
        DuckValue::TinyInt(value) => Value::Int(value.into()),
        DuckValue::SmallInt(value) => Value::Int(value.into()),
        DuckValue::Int(value) => Value::Int(value.into()),
        DuckValue::BigInt(value) => Value::Int(value),
        DuckValue::UTinyInt(value) => Value::Int(value.into()),
        DuckValue::USmallInt(value) => Value::Int(value.into()),
        DuckValue::UInt(value) => Value::Int(value.into()),
        DuckValue::UBigInt(value) => i64::try_from(value)
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(value.to_string())),
        DuckValue::HugeInt(value) => i64::try_from(value)
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(value.to_string())),
        DuckValue::UHugeInt(value) => i64::try_from(value)
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(value.to_string())),
        DuckValue::Float(value) => Value::Float(value.into()),
        DuckValue::Double(value) => Value::Float(value),
        DuckValue::Decimal(value) => Value::Text(value.to_string()),
        DuckValue::Text(value) | DuckValue::Enum(value) => Value::Text(value),
        DuckValue::Blob(value) | DuckValue::Geometry(value) => Value::Bytes(value),
        DuckValue::Timestamp(unit, value) => Value::Text(
            format_timestamp(unit, value).unwrap_or_else(|| format!("{value:?} {unit:?}")),
        ),
        DuckValue::Date32(value) => Value::Text(
            chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .and_then(|date| date.checked_add_signed(chrono::TimeDelta::days(value.into())))
                .map(|date| date.to_string())
                .unwrap_or_else(|| value.to_string()),
        ),
        DuckValue::Time64(unit, value) => {
            Value::Text(format_time(unit, value).unwrap_or_else(|| format!("{value:?} {unit:?}")))
        }
        DuckValue::Interval {
            months,
            days,
            nanos,
        } => Value::Text(format!("{months} months {days} days {nanos} ns")),
        other => Value::Text(format!("{other:?}")),
    }
}

fn format_timestamp(unit: TimeUnit, value: i64) -> Option<String> {
    let (seconds, nanos_per_remainder, divisor) = match unit {
        TimeUnit::Second => (value, 0, 1),
        TimeUnit::Millisecond => (value.div_euclid(1_000), 1_000_000, 1_000),
        TimeUnit::Microsecond => (value.div_euclid(1_000_000), 1_000, 1_000_000),
        TimeUnit::Nanosecond => (value.div_euclid(1_000_000_000), 1, 1_000_000_000),
    };
    let nanos = if nanos_per_remainder == 0 {
        0
    } else {
        value.rem_euclid(divisor) as u32 * nanos_per_remainder
    };
    chrono::DateTime::from_timestamp(seconds, nanos).map(|date| date.naive_utc().to_string())
}

fn format_time(unit: TimeUnit, value: i64) -> Option<String> {
    let nanos = match unit {
        TimeUnit::Second => i128::from(value) * 1_000_000_000,
        TimeUnit::Millisecond => i128::from(value) * 1_000_000,
        TimeUnit::Microsecond => i128::from(value) * 1_000,
        TimeUnit::Nanosecond => i128::from(value),
    };
    let day_nanos = 86_400_i128 * 1_000_000_000;
    if !(0..day_nanos).contains(&nanos) {
        return None;
    }
    let seconds = (nanos / 1_000_000_000) as u32;
    let subsecond = (nanos % 1_000_000_000) as u32;
    chrono::NaiveTime::from_num_seconds_from_midnight_opt(seconds, subsecond)
        .map(|time| time.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_config() -> ConnectionConfig {
        ConnectionConfig::new(DbKind::DuckDb)
    }

    #[tokio::test]
    async fn in_memory_queries_are_capped_and_catalog_is_introspected() {
        let db = DuckDb::connect(&memory_config()).unwrap();
        db.execute_capped(
            "CREATE TABLE events(id BIGINT PRIMARY KEY, label VARCHAR); \
             INSERT INTO events VALUES (1, 'one'), (2, 'two'), (3, 'three'); \
             CREATE INDEX events_label_idx ON events(label); \
             CREATE TABLE event_notes( \
                 id BIGINT PRIMARY KEY, event_id BIGINT, \
                 CONSTRAINT event_notes_event_fk FOREIGN KEY(event_id) REFERENCES events(id) \
             );",
            10,
        )
        .await
        .unwrap();

        let result = db
            .execute_capped("SELECT * FROM events ORDER BY id", 2)
            .await
            .unwrap();
        assert_eq!(result.row_count(), 2);
        assert!(result.truncated);
        assert_eq!(
            result.rows[0],
            vec![Value::Int(1), Value::Text("one".into())]
        );

        let schema = db.introspect().await.unwrap();
        let table = schema
            .tables
            .iter()
            .find(|table| table.name == "events")
            .unwrap();
        assert!(table
            .columns
            .iter()
            .any(|column| column.name == "id" && column.primary_key));
        assert!(table
            .indexes
            .iter()
            .any(|index| index.name == "events_label_idx" && index.columns == ["label"]));
        let note_table = schema
            .tables
            .iter()
            .find(|table| table.name == "event_notes")
            .unwrap();
        assert!(
            note_table.foreign_keys.iter().any(|foreign_key| {
                !foreign_key.name.is_empty()
                    && foreign_key.columns == ["event_id"]
                    && foreign_key.ref_table == "events"
                    && foreign_key.ref_columns == ["id"]
            }),
            "unexpected DuckDB foreign keys: {:?}",
            note_table.foreign_keys
        );

        let statement = crate::safety::dangerous_statements(
            DbKind::DuckDb,
            "UPDATE events SET label = 'changed' WHERE id > 0",
        )
        .remove(0);
        let preflight = db.production_preflight(&statement).await;
        assert_eq!(preflight.affected_rows, Some(3));
        assert!(
            preflight.plan.is_some(),
            "DuckDB EXPLAIN should be reduced to Guardian evidence: {:?}",
            preflight.warnings
        );
    }

    #[tokio::test]
    async fn analytical_values_keep_precision_or_render_without_panicking() {
        let db = DuckDb::connect(&memory_config()).unwrap();
        let result = db
            .execute_capped(
                "SELECT 170141183460469231731687303715884105727::HUGEINT AS huge, \
                        12.340::DECIMAL(8,3) AS exact, [1, 2, 3] AS nested",
                1,
            )
            .await
            .unwrap();
        assert_eq!(result.row_count(), 1);
        assert!(matches!(result.rows[0][0], Value::Text(_)));
        assert_eq!(result.rows[0][1], Value::Text("12.340".into()));
        assert!(matches!(result.rows[0][2], Value::Text(_)));
    }

    #[tokio::test]
    async fn temporal_values_are_rendered_as_sql_friendly_text() {
        let db = DuckDb::connect(&memory_config()).unwrap();
        let result = db
            .execute_capped(
                "SELECT DATE '2026-08-14', TIME '13:45:12.123', \
                        TIMESTAMP '2026-08-14 13:45:12.123'",
                1,
            )
            .await
            .unwrap();
        assert_eq!(result.rows[0][0], Value::Text("2026-08-14".into()));
        assert_eq!(result.rows[0][1], Value::Text("13:45:12.123".into()));
        assert_eq!(
            result.rows[0][2],
            Value::Text("2026-08-14 13:45:12.123".into())
        );
    }

    #[tokio::test]
    async fn bundled_parquet_round_trips_without_an_external_extension() {
        let db = DuckDb::connect(&memory_config()).unwrap();
        let path = std::env::temp_dir().join(format!(
            "plusplus-duckdb-parquet-{}.parquet",
            std::process::id()
        ));
        let sql_path = path.to_string_lossy().replace('\'', "''");
        db.execute_capped(
            &format!(
                "COPY (SELECT i AS id FROM range(3) AS source(i)) \
                 TO '{sql_path}' (FORMAT PARQUET)"
            ),
            1,
        )
        .await
        .unwrap();

        let result = db
            .execute_capped(
                &format!("SELECT sum(id) FROM read_parquet('{sql_path}')"),
                1,
            )
            .await
            .unwrap();
        std::fs::remove_file(path).ok();

        assert_eq!(result.rows, vec![vec![Value::Int(3)]]);
    }
}
