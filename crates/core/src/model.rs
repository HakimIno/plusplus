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

/// User-chosen glyph for a connection in the sidebar. Persisted with the connection config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionIcon {
    #[default]
    Database,
    Table,
    #[serde(alias = "code")]
    Cloud,
    #[serde(alias = "settings")]
    Storage,
    #[serde(alias = "connect")]
    Star,
    #[serde(alias = "key")]
    Treasure,
}

impl ConnectionIcon {
    pub const ALL: [ConnectionIcon; 6] = [
        ConnectionIcon::Database,
        ConnectionIcon::Table,
        ConnectionIcon::Cloud,
        ConnectionIcon::Storage,
        ConnectionIcon::Star,
        ConnectionIcon::Treasure,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ConnectionIcon::Database => "Database",
            ConnectionIcon::Table => "Table",
            ConnectionIcon::Cloud => "Cloud",
            ConnectionIcon::Storage => "Local disk",
            ConnectionIcon::Star => "Favorite",
            ConnectionIcon::Treasure => "Treasure",
        }
    }
}

/// How strictly a server connection should use TLS, mirroring Postgres' `sslmode`
/// vocabulary so it translates cleanly to every backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SslMode {
    /// Never use TLS; fail if the server insists on it.
    Disable,
    /// Use TLS if the server supports it, fall back to plaintext otherwise.
    /// Matches the pre-TLS-config behavior, so it's the default for old configs.
    #[default]
    Prefer,
    /// Require TLS but don't verify the server certificate.
    Require,
    /// Require TLS and verify the certificate against a trusted CA.
    VerifyCa,
    /// Require TLS, verify the CA, and check the hostname matches the certificate.
    VerifyFull,
}

impl SslMode {
    pub const ALL: [SslMode; 5] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyCa,
        SslMode::VerifyFull,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SslMode::Disable => "Disable",
            SslMode::Prefer => "Prefer",
            SslMode::Require => "Require",
            SslMode::VerifyCa => "Verify CA",
            SslMode::VerifyFull => "Verify Full",
        }
    }

    /// Does this mode validate the server certificate?
    pub fn verifies_certificate(self) -> bool {
        matches!(self, SslMode::VerifyCa | SslMode::VerifyFull)
    }
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
    /// TLS policy for server backends. Ignored by SQLite.
    #[serde(default)]
    pub ssl_mode: SslMode,
    /// Path to a PEM CA certificate used by the verify modes. Empty means the
    /// system trust store.
    #[serde(default)]
    pub ssl_ca_cert: String,
    // --- file backends ---
    #[serde(default)]
    pub sqlite_path: String,
    /// Optional user-chosen title bar color for visually marking important connections.
    #[serde(default)]
    pub title_bar_color: Option<ConnectionColor>,
    /// Sidebar icon for this connection.
    #[serde(default)]
    pub icon: ConnectionIcon,
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
            ssl_mode: SslMode::default(),
            ssl_ca_cert: String::new(),
            sqlite_path: String::new(),
            title_bar_color: None,
            icon: ConnectionIcon::default(),
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

// ─── DDL types ───────────────────────────────────────────────────────────────

/// Column definition for DDL operations (CREATE TABLE / ALTER TABLE ADD COLUMN).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    /// Optional DEFAULT expression rendered verbatim (e.g. `"'hello'"`, `"0"`, `"NOW()"`).
    pub default: Option<String>,
}

/// Index definition for DDL operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

/// ON DELETE / ON UPDATE referential action for a foreign key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FkAction {
    #[default]
    NoAction,
    Cascade,
    SetNull,
    Restrict,
}

impl FkAction {
    pub const ALL: &'static [FkAction] = &[
        FkAction::NoAction,
        FkAction::Cascade,
        FkAction::SetNull,
        FkAction::Restrict,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FkAction::NoAction => "NO ACTION",
            FkAction::Cascade => "CASCADE",
            FkAction::SetNull => "SET NULL",
            FkAction::Restrict => "RESTRICT",
        }
    }
}

/// Foreign key constraint definition (used inside CREATE TABLE or as ADD CONSTRAINT).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForeignKeyDef {
    /// Constraint name — use an empty string to omit the `CONSTRAINT` clause.
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    pub on_delete: FkAction,
}

// ─── DDL builder helpers ─────────────────────────────────────────────────────

fn ddl_table_ref(kind: DbKind, schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(table)),
        None => kind.quote_ident(table),
    }
}

/// Render one column definition for use in CREATE TABLE.
/// `inline_pk` emits `PRIMARY KEY` inline; set to `false` for multi-column PK tables
/// (which use a trailing table-level `PRIMARY KEY (a, b)` clause instead).
fn col_def_sql(kind: DbKind, col: &ColumnDef, inline_pk: bool) -> String {
    let mut parts = vec![kind.quote_ident(&col.name), col.data_type.clone()];
    if !col.nullable {
        parts.push("NOT NULL".into());
    }
    if let Some(def) = &col.default {
        let d = def.trim();
        if !d.is_empty() {
            parts.push(format!("DEFAULT {d}"));
        }
    }
    if col.primary_key && inline_pk {
        parts.push("PRIMARY KEY".into());
    }
    parts.join(" ")
}

fn fk_clause_sql(kind: DbKind, fk: &ForeignKeyDef) -> String {
    let cols = fk.columns.iter().map(|c| kind.quote_ident(c)).collect::<Vec<_>>().join(", ");
    let ref_t = kind.quote_ident(&fk.ref_table);
    let ref_c = fk.ref_columns.iter().map(|c| kind.quote_ident(c)).collect::<Vec<_>>().join(", ");
    let constraint = if fk.name.trim().is_empty() {
        String::new()
    } else {
        format!("CONSTRAINT {} ", kind.quote_ident(fk.name.trim()))
    };
    format!(
        "{constraint}FOREIGN KEY ({cols}) REFERENCES {ref_t} ({ref_c}) ON DELETE {}",
        fk.on_delete.label()
    )
}

// ─── DDL builders ────────────────────────────────────────────────────────────

/// Build a `CREATE TABLE` statement with column definitions and optional foreign keys.
pub fn build_create_table_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    columns: &[ColumnDef],
    fks: &[ForeignKeyDef],
) -> String {
    let pk_count = columns.iter().filter(|c| c.primary_key).count();
    let inline_pk = pk_count == 1;
    let mut defs: Vec<String> = columns.iter().map(|c| col_def_sql(kind, c, inline_pk)).collect();
    if pk_count > 1 {
        let pk_cols = columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| kind.quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ");
        defs.push(format!("PRIMARY KEY ({pk_cols})"));
    }
    for fk in fks {
        defs.push(fk_clause_sql(kind, fk));
    }
    let body = defs.join(",\n    ");
    let engine = match kind {
        DbKind::MySql | DbKind::MariaDb => " ENGINE=InnoDB",
        _ => "",
    };
    format!(
        "CREATE TABLE {} (\n    {body}\n){engine};",
        ddl_table_ref(kind, schema, table)
    )
}

/// Build a `DROP TABLE` statement.
pub fn build_drop_table_sql(kind: DbKind, schema: Option<&str>, table: &str) -> String {
    format!("DROP TABLE {};", ddl_table_ref(kind, schema, table))
}

/// Build an `ALTER TABLE … ADD COLUMN` statement.
pub fn build_add_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col: &ColumnDef,
) -> String {
    format!(
        "ALTER TABLE {} ADD COLUMN {};",
        ddl_table_ref(kind, schema, table),
        col_def_sql(kind, col, false)
    )
}

/// Build an `ALTER TABLE … DROP COLUMN` statement.
pub fn build_drop_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col_name: &str,
) -> String {
    format!(
        "ALTER TABLE {} DROP COLUMN {};",
        ddl_table_ref(kind, schema, table),
        kind.quote_ident(col_name)
    )
}

/// Build an `ALTER TABLE … RENAME COLUMN` (or `sp_rename` for SQL Server).
pub fn build_rename_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    old_name: &str,
    new_name: &str,
) -> String {
    match kind {
        DbKind::SqlServer => {
            let qualified =
                format!("{}.{}.{}", schema.unwrap_or("dbo"), table, old_name);
            format!("EXEC sp_rename '{qualified}', '{new_name}', 'COLUMN';")
        }
        _ => format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {};",
            ddl_table_ref(kind, schema, table),
            kind.quote_ident(old_name),
            kind.quote_ident(new_name)
        ),
    }
}

/// Build a `CREATE [UNIQUE] INDEX` statement (dialect-aware).
pub fn build_create_index_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    idx: &IndexDef,
) -> String {
    let unique = if idx.unique { "UNIQUE " } else { "" };
    let cols = idx
        .columns
        .iter()
        .map(|c| kind.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    // MySQL/MariaDB don't support schema-qualified index names in CREATE INDEX.
    let idx_ref = match kind {
        DbKind::MySql | DbKind::MariaDb => kind.quote_ident(&idx.name),
        _ => match schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&idx.name)),
            None => kind.quote_ident(&idx.name),
        },
    };
    format!(
        "CREATE {unique}INDEX {idx_ref} ON {} ({cols});",
        ddl_table_ref(kind, schema, table)
    )
}

/// Build a `DROP INDEX` statement (MySQL/SQL Server require `ON table`; others don't).
pub fn build_drop_index_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    idx_name: &str,
) -> String {
    let q = kind.quote_ident(idx_name);
    match kind {
        DbKind::MySql | DbKind::MariaDb | DbKind::SqlServer => {
            format!("DROP INDEX {q} ON {};", ddl_table_ref(kind, schema, table))
        }
        _ => format!("DROP INDEX {q};"),
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
