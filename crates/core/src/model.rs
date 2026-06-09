//! Plain data structures shared across the app: connection configs, schema metadata,
//! and query results. None of these depend on a specific backend.

use serde::{Deserialize, Serialize};

use crate::value::Value;

/// Which database backend a connection targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbKind {
    Postgres,
    MySql,
    MariaDb,
    SqlServer,
    Sqlite,
}

impl DbKind {
    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "PostgreSQL",
            DbKind::MySql => "MySQL",
            DbKind::MariaDb => "MariaDB",
            DbKind::SqlServer => "SQL Server",
            DbKind::Sqlite => "SQLite",
        }
    }

    /// Whether this backend authenticates with a server (host/port/user/password)
    /// versus a local file path.
    pub fn is_server(self) -> bool {
        matches!(
            self,
            DbKind::Postgres | DbKind::MySql | DbKind::MariaDb | DbKind::SqlServer
        )
    }

    pub fn default_port(self) -> u16 {
        match self {
            DbKind::Postgres => 5432,
            DbKind::MySql | DbKind::MariaDb => 3306,
            DbKind::SqlServer => 1433,
            DbKind::Sqlite => 0,
        }
    }

    /// Build a "preview the first `limit` rows" query for `qualified_table` in this dialect.
    /// SQL Server has no `LIMIT`; it caps rows with `TOP` instead.
    pub fn preview_query(self, qualified_table: &str, limit: u32) -> String {
        match self {
            DbKind::SqlServer => format!("SELECT TOP {limit} * FROM {qualified_table};"),
            _ => format!("SELECT * FROM {qualified_table} LIMIT {limit};"),
        }
    }

    /// Quote a table/column identifier for this dialect. MySQL/MariaDB use backticks; the
    /// rest use ANSI double quotes. Embedded quote characters are doubled to neutralise them.
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            DbKind::MySql | DbKind::MariaDb => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }
}

/// Render `value` as a SQL literal for `kind`, safely escaping strings. Returns `None` for
/// [`Value::Bytes`], which has no portable literal form (those cells aren't editable).
fn value_to_literal(value: &Value, kind: DbKind) -> Option<String> {
    Some(match value {
        Value::Null => "NULL".to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => match kind {
            // Postgres has a real boolean type; the others store it as an integer/bit.
            DbKind::Postgres => if *b { "TRUE" } else { "FALSE" }.to_string(),
            _ => if *b { "1" } else { "0" }.to_string(),
        },
        Value::Text(s) => {
            // Double single-quotes everywhere; MySQL also treats backslash as an escape
            // unless NO_BACKSLASH_ESCAPES is set, so double those too for that dialect.
            let escaped = s.replace('\'', "''");
            let escaped = match kind {
                DbKind::MySql | DbKind::MariaDb => escaped.replace('\\', "\\\\"),
                _ => escaped,
            };
            format!("'{escaped}'")
        }
        Value::Bytes(_) => return None,
    })
}

/// Build a single-row `UPDATE` statement: `SET` the given `sets`, matched by the `keys`
/// (typically primary-key columns). Returns `None` if any value can't be rendered as a
/// literal (e.g. binary data). Identifiers and string values are escaped for `kind`.
pub fn build_update_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    sets: &[(&str, &Value)],
    keys: &[(&str, &Value)],
) -> Option<String> {
    if sets.is_empty() || keys.is_empty() {
        return None;
    }
    let table_ref = match schema {
        Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(table)),
        None => kind.quote_ident(table),
    };
    let set_clause = sets
        .iter()
        .map(|(c, v)| Some(format!("{} = {}", kind.quote_ident(c), value_to_literal(v, kind)?)))
        .collect::<Option<Vec<_>>>()?
        .join(", ");
    let where_clause = keys
        .iter()
        .map(|(c, v)| {
            Some(if v.is_null() {
                format!("{} IS NULL", kind.quote_ident(c))
            } else {
                format!("{} = {}", kind.quote_ident(c), value_to_literal(v, kind)?)
            })
        })
        .collect::<Option<Vec<_>>>()?
        .join(" AND ");
    Some(format!("UPDATE {table_ref} SET {set_clause} WHERE {where_clause};"))
}

/// A saved connection. Secret fields (passwords) are **never** stored here — they live in
/// the OS keychain keyed by [`ConnectionConfig::id`]. Only non-secret fields are persisted
/// to the JSON config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Stable unique id, also used as the keychain account name.
    pub id: String,
    /// User-facing name for this connection.
    pub name: String,
    pub kind: DbKind,
    // --- server backends ---
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub database: String,
    // --- file backends ---
    #[serde(default)]
    pub sqlite_path: String,
}

impl ConnectionConfig {
    /// Create a new config with a freshly generated id and sane defaults for `kind`.
    pub fn new(kind: DbKind) -> Self {
        Self {
            id: generate_id(),
            name: format!("New {}", kind.label()),
            kind,
            host: "localhost".to_string(),
            port: kind.default_port(),
            user: String::new(),
            database: String::new(),
            sqlite_path: String::new(),
        }
    }

    /// A short subtitle describing the target, shown in the connection list.
    pub fn target_summary(&self) -> String {
        match self.kind {
            DbKind::Postgres | DbKind::MySql | DbKind::MariaDb | DbKind::SqlServer => {
                format!(
                    "{}@{}:{}/{}",
                    self.user, self.host, self.port, self.database
                )
            }
            DbKind::Sqlite => self.sqlite_path.clone(),
        }
    }
}

/// Generate a process-unique, time-ordered id without pulling in a uuid dependency.
fn generate_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("conn-{nanos:x}-{n:x}")
}

/// A column as introspected from the schema.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

/// An index on a table.
#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub columns: Vec<String>,
}

/// A table (or view) with its columns and indexes.
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Schema/namespace the table lives in (e.g. `public` for Postgres). `None` for SQLite.
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
}

impl TableInfo {
    /// Fully-qualified, quote-safe name for use in generated SQL.
    pub fn qualified(&self) -> String {
        match &self.schema {
            Some(s) => format!("\"{}\".\"{}\"", s, self.name),
            None => format!("\"{}\"", self.name),
        }
    }
}

/// The full introspected schema of a connected database.
#[derive(Debug, Clone)]
pub struct SchemaTree {
    pub database_name: String,
    pub tables: Vec<TableInfo>,
}

/// Metadata for a single result-set column.
#[derive(Debug, Clone)]
pub struct ColumnMeta {
    pub name: String,
    /// The backend's native type name (best-effort), shown in tooltips.
    pub type_name: String,
}

/// Stats about a single query execution.
#[derive(Debug, Clone, Default)]
pub struct QueryStats {
    /// Wall-clock execution time in milliseconds.
    pub elapsed_ms: f64,
    /// Rows affected for DML statements (INSERT/UPDATE/DELETE). `None` for SELECTs.
    pub rows_affected: Option<u64>,
}

/// A complete result set: column metadata plus rows of [`Value`]s.
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    pub stats: QueryStats,
}

impl QueryResult {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}
