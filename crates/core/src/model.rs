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
        .map(|(c, v)| {
            Some(format!(
                "{} = {}",
                kind.quote_ident(c),
                value_to_literal(v, kind)?
            ))
        })
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
    Some(format!(
        "UPDATE {table_ref} SET {set_clause} WHERE {where_clause};"
    ))
}

/// Build a single-row `DELETE` statement matched by the `keys` (typically primary-key
/// columns). Returns `None` if any key value can't be rendered as a literal.
pub fn build_delete_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    keys: &[(&str, &Value)],
) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let table_ref = match schema {
        Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(table)),
        None => kind.quote_ident(table),
    };
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
    Some(format!("DELETE FROM {table_ref} WHERE {where_clause};"))
}

/// Build a single-row `INSERT` statement from the given `cols` (column, value) pairs.
/// Returns `None` if there are no columns or any value can't be rendered as a literal.
pub fn build_insert_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    cols: &[(&str, &Value)],
) -> Option<String> {
    if cols.is_empty() {
        return None;
    }
    let table_ref = match schema {
        Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(table)),
        None => kind.quote_ident(table),
    };
    let col_list = cols
        .iter()
        .map(|(c, _)| kind.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let val_list = cols
        .iter()
        .map(|(_, v)| value_to_literal(v, kind))
        .collect::<Option<Vec<_>>>()?
        .join(", ");
    Some(format!(
        "INSERT INTO {table_ref} ({col_list}) VALUES ({val_list});"
    ))
}

/// Strip `kw` (case-insensitively) off the front of `s`, requiring a non-identifier
/// character after it so `FROMx` doesn't match `FROM`. Returns the trimmed remainder.
fn strip_keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let s = s.trim_start();
    let head = s.get(..kw.len())?;
    if !head.eq_ignore_ascii_case(kw) {
        return None;
    }
    let rest = &s[kw.len()..];
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(rest.trim_start())
}

/// Parse one (possibly quoted) identifier off the front of `s`, returning it unquoted plus
/// the remaining input. Supports `"x"` (ANSI), `` `x` `` (MySQL), `[x]` (SQL Server), and
/// bare names; doubled closing quotes inside a quoted name un-double.
fn parse_ident(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let close = match s.chars().next()? {
        '"' => '"',
        '`' => '`',
        '[' => ']',
        c if c.is_alphanumeric() || c == '_' => {
            let end = s
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                .unwrap_or(s.len());
            return Some((s[..end].to_string(), &s[end..]));
        }
        _ => return None,
    };
    let mut name = String::new();
    let mut rest = &s[1..];
    loop {
        let pos = rest.find(close)?;
        name.push_str(&rest[..pos]);
        rest = &rest[pos + close.len_utf8()..];
        if close != ']' && rest.starts_with(close) {
            name.push(close);
            rest = &rest[close.len_utf8()..];
        } else {
            return Some((name, rest));
        }
    }
}

/// If `sql` is a simple single-table read — `SELECT [TOP n] * FROM table`, optionally
/// followed by `WHERE`/`ORDER BY`/`LIMIT`/`OFFSET`/`FETCH` — return the `(schema, table)`
/// it reads. Rows of such a result map 1:1 onto table rows (and `*` guarantees the primary
/// key is present), so the grid can stay editable no matter how the query was written:
/// a hand-tuned `LIMIT 20000`, a `WHERE`, a sort. Joins, projections, aggregates, and
/// multi-statement scripts return `None` (read-only).
pub fn simple_select_target(sql: &str) -> Option<(Option<String>, String)> {
    let sql = sql.trim().trim_end_matches(';').trim_end();
    if sql.contains(';') {
        return None; // multiple statements — don't try to reason about them
    }
    let rest = strip_keyword(sql, "SELECT")?;
    let rest = match strip_keyword(rest, "TOP") {
        Some(after) => {
            let digits = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            if digits == 0 {
                return None;
            }
            after[digits..].trim_start()
        }
        None => rest,
    };
    let rest = rest.strip_prefix('*')?;
    let rest = strip_keyword(rest, "FROM")?;
    let (first, rest) = parse_ident(rest)?;
    let (schema, table, rest) = match rest.strip_prefix('.') {
        Some(r) => {
            let (second, r) = parse_ident(r)?;
            (Some(first), second, r)
        }
        None => (None, first, rest),
    };
    // Whatever follows the table must be a row-preserving clause; an alias, a comma, or a
    // JOIN means result rows no longer map 1:1 to table rows.
    let tail = rest.trim_start();
    if !tail.is_empty() {
        let next = tail
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if !matches!(
            next.as_str(),
            "WHERE" | "ORDER" | "LIMIT" | "OFFSET" | "FETCH"
        ) {
            return None;
        }
    }
    Some((schema, table))
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
    /// Optional user-chosen title bar color for visually marking important connections.
    #[serde(default)]
    pub title_bar_color: Option<ConnectionColor>,
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
            title_bar_color: None,
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

/// Stored RGB color for per-connection UI markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ConnectionColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn target(sql: &str) -> Option<(Option<String>, String)> {
        simple_select_target(sql)
    }

    #[test]
    fn simple_select_target_accepts_single_table_reads() {
        assert_eq!(target("SELECT * FROM users"), Some((None, "users".into())));
        assert_eq!(
            target("select * from users limit 20000;"),
            Some((None, "users".into()))
        );
        assert_eq!(
            target("SELECT * FROM \"public\".\"users\" LIMIT 100;"),
            Some((Some("public".into()), "users".into()))
        );
        assert_eq!(
            target("SELECT * FROM `db`.`orders` WHERE total > 10 ORDER BY id LIMIT 50"),
            Some((Some("db".into()), "orders".into()))
        );
        assert_eq!(
            target("SELECT TOP 100 * FROM [dbo].[Invoices];"),
            Some((Some("dbo".into()), "Invoices".into()))
        );
        // Embedded doubled quotes un-double.
        assert_eq!(
            target(r#"SELECT * FROM "we""ird""#),
            Some((None, "we\"ird".into()))
        );
    }

    #[test]
    fn simple_select_target_rejects_everything_else() {
        // Projections lose the guarantee that the PK columns are present.
        assert_eq!(target("SELECT id, name FROM users"), None);
        // Joins/aliases/commas: rows no longer map 1:1 to table rows.
        assert_eq!(target("SELECT * FROM a JOIN b ON a.id = b.id"), None);
        assert_eq!(target("SELECT * FROM a, b"), None);
        assert_eq!(target("SELECT * FROM users u WHERE u.id = 1"), None);
        assert_eq!(target("SELECT * FROM (SELECT * FROM users) x"), None);
        // Non-SELECT and multi-statement scripts.
        assert_eq!(target("UPDATE users SET name = 'x'"), None);
        assert_eq!(target("SELECT * FROM a; SELECT * FROM b"), None);
        assert_eq!(target("SELECT * FROM users GROUP BY id"), None);
    }

    #[test]
    fn build_insert_quotes_and_escapes() {
        let name = Value::Text("O'Brien".into());
        let age = Value::Int(42);
        let cols = [("name", &name), ("age", &age)];
        assert_eq!(
            build_insert_sql(DbKind::Postgres, Some("public"), "users", &cols),
            Some(
                "INSERT INTO \"public\".\"users\" (\"name\", \"age\") VALUES ('O''Brien', 42);"
                    .to_string()
            )
        );
        // MySQL uses backtick identifiers.
        assert_eq!(
            build_insert_sql(DbKind::MySql, None, "users", &cols),
            Some("INSERT INTO `users` (`name`, `age`) VALUES ('O''Brien', 42);".to_string())
        );
        // No columns ⇒ nothing to insert.
        assert_eq!(build_insert_sql(DbKind::Postgres, None, "users", &[]), None);
        // Binary has no portable literal form.
        let blob = Value::Bytes(vec![1, 2, 3]);
        assert_eq!(
            build_insert_sql(DbKind::Postgres, None, "t", &[("data", &blob)]),
            None
        );
    }

    #[test]
    fn build_delete_targets_by_key() {
        let id = Value::Int(7);
        assert_eq!(
            build_delete_sql(DbKind::Postgres, Some("public"), "users", &[("id", &id)]),
            Some("DELETE FROM \"public\".\"users\" WHERE \"id\" = 7;".to_string())
        );
        // NULL keys compare with IS NULL, and composite keys AND together.
        let null = Value::Null;
        let tenant = Value::Int(3);
        assert_eq!(
            build_delete_sql(
                DbKind::Postgres,
                None,
                "t",
                &[("tenant", &tenant), ("note", &null)]
            ),
            Some("DELETE FROM \"t\" WHERE \"tenant\" = 3 AND \"note\" IS NULL;".to_string())
        );
        // No keys ⇒ refuse (never emit an unfiltered DELETE).
        assert_eq!(build_delete_sql(DbKind::Postgres, None, "t", &[]), None);
    }
}
