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

    /// Whether this backend can present a client certificate (mutual TLS).
    /// tiberius hardcodes no-client-auth, so SQL Server can't; SQLite has no TLS at all.
    pub fn supports_client_cert(self) -> bool {
        matches!(self, DbKind::Postgres | DbKind::MySql | DbKind::MariaDb)
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

// ─── Server-side paging ──────────────────────────────────────────────────────
//
// Table tabs page through big tables server-side instead of fetching everything: the pager
// rewrites the tab's paging clauses (`LIMIT/OFFSET`, `TOP`, `OFFSET … FETCH`) in place and
// re-runs, so the SQL editor always shows exactly what ran. All helpers below only operate
// on queries [`simple_select_target`] accepts — for anything more complex they return
// `None` and the pager stays hidden.

/// The paging window of a simple single-table SELECT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageWindow {
    /// Rows per page (`LIMIT n`, `TOP n`, or `FETCH NEXT n ROWS ONLY`). `None` = unbounded.
    pub limit: Option<u64>,
    /// Rows skipped before the page starts (`OFFSET n`). 0 when absent.
    pub offset: u64,
}

/// Byte offsets of every top-level, whole-word, case-insensitive occurrence of `kw`,
/// skipping string literals, quoted identifiers, and comments.
fn keyword_positions(sql: &str, kw: &str) -> Vec<usize> {
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let k = kw.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    // Whether the previous byte could continue an identifier (so `xlimit`/`'a'limit`
    // never match). Quoted regions count as identifier-enders only across whitespace.
    let mut prev_ident = false;
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    while i < n {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < n {
                    if bytes[i] == quote {
                        if i + 1 < n && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                prev_ident = true; // a keyword can't butt right up against a quote
            }
            b'[' => {
                i += 1;
                while i < n {
                    if bytes[i] == b']' {
                        if i + 1 < n && bytes[i + 1] == b']' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                prev_ident = true;
            }
            b'-' if i + 1 < n && bytes[i + 1] == b'-' => {
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
                prev_ident = false;
            }
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                let mut depth = 1u32;
                while i < n && depth > 0 {
                    if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                prev_ident = false;
            }
            b => {
                if is_ident(b)
                    && !prev_ident
                    && i + k <= n
                    && sql[i..i + k].eq_ignore_ascii_case(kw)
                    && !bytes.get(i + k).copied().is_some_and(is_ident)
                {
                    out.push(i);
                    i += k;
                    prev_ident = true;
                    continue;
                }
                prev_ident = is_ident(b);
                i += 1;
            }
        }
    }
    out
}

/// Parse the leading unsigned integer of `s` (after whitespace), if any.
fn leading_u64(s: &str) -> Option<u64> {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(s.len());
    s[..end].parse().ok()
}

/// If the whole of `s` parses as a sequence of paging clauses, return `(limit, offset)`.
///
/// Grammar (any order, MySQL's `LIMIT offset, count` included):
///   `LIMIT n [, m]` · `LIMIT n OFFSET m` · `OFFSET n [ROW|ROWS]` ·
///   `FETCH FIRST|NEXT n ROW|ROWS ONLY`
///
/// Requiring the *entire* tail to match keeps an unquoted column that happens to be named
/// `offset`/`fetch` inside a WHERE clause from being mistaken for a paging clause.
fn parse_paging_tail(s: &str) -> Option<(Option<u64>, u64)> {
    let mut toks: Vec<String> = Vec::new();
    for word in s.split_whitespace() {
        // Commas may be glued to numbers (`LIMIT 10,20`); make them their own token.
        let mut rest = word;
        while let Some(pos) = rest.find(',') {
            if pos > 0 {
                toks.push(rest[..pos].to_string());
            }
            toks.push(",".to_string());
            rest = &rest[pos + 1..];
        }
        if !rest.is_empty() {
            toks.push(rest.to_string());
        }
    }
    if toks.is_empty() {
        return None;
    }
    let mut limit = None;
    let mut offset = 0u64;
    let mut i = 0usize;
    let up = |t: Option<&String>| t.map(|t| t.to_ascii_uppercase()).unwrap_or_default();
    while i < toks.len() {
        match up(toks.get(i)).as_str() {
            "LIMIT" => {
                let a: u64 = toks.get(i + 1)?.parse().ok()?;
                if toks.get(i + 2).map(String::as_str) == Some(",") {
                    // MySQL `LIMIT offset, count`.
                    offset = a;
                    limit = Some(toks.get(i + 3)?.parse().ok()?);
                    i += 4;
                } else {
                    limit = Some(a);
                    i += 2;
                }
            }
            "OFFSET" => {
                offset = toks.get(i + 1)?.parse().ok()?;
                i += 2;
                if matches!(up(toks.get(i)).as_str(), "ROW" | "ROWS") {
                    i += 1;
                }
            }
            "FETCH" => {
                if !matches!(up(toks.get(i + 1)).as_str(), "FIRST" | "NEXT") {
                    return None;
                }
                limit = Some(toks.get(i + 2)?.parse().ok()?);
                if !matches!(up(toks.get(i + 3)).as_str(), "ROW" | "ROWS") {
                    return None;
                }
                if up(toks.get(i + 4)).as_str() != "ONLY" {
                    return None;
                }
                i += 5;
            }
            _ => return None,
        }
    }
    Some((limit, offset))
}

/// Locate the trailing paging clauses of `sql` (already `;`-trimmed). Returns the byte
/// index where they start (== `sql.len()` when there are none) plus the parsed window.
fn trailing_paging(sql: &str) -> (usize, Option<u64>, u64) {
    let mut candidates: Vec<usize> = ["LIMIT", "OFFSET", "FETCH"]
        .iter()
        .flat_map(|kw| keyword_positions(sql, kw))
        .collect();
    candidates.sort_unstable();
    for pos in candidates {
        if let Some((limit, offset)) = parse_paging_tail(&sql[pos..]) {
            return (pos, limit, offset);
        }
    }
    (sql.len(), None, 0)
}

/// The paging window of `sql`, if it's a simple single-table read (per
/// [`simple_select_target`]). A query with no LIMIT/TOP/FETCH comes back as
/// `PageWindow { limit: None, offset }`.
pub fn parse_page_window(sql: &str) -> Option<PageWindow> {
    simple_select_target(sql)?;
    let sql = sql.trim().trim_end_matches(';').trim_end();
    // SQL Server's `TOP n` sits right after SELECT.
    let top = strip_keyword(sql, "SELECT")
        .and_then(|rest| strip_keyword(rest, "TOP"))
        .and_then(leading_u64);
    let (_, limit, offset) = trailing_paging(sql);
    Some(PageWindow {
        limit: limit.or(top),
        offset,
    })
}

/// Rewrite the paging clauses of a simple single-table SELECT so it returns `limit` rows
/// starting at `offset`, in `kind`'s dialect. WHERE and ORDER BY are preserved verbatim.
/// Returns `None` when `sql` isn't a simple single-table read.
pub fn with_page_window(kind: DbKind, sql: &str, limit: u64, offset: u64) -> Option<String> {
    simple_select_target(sql)?;
    let sql = sql.trim().trim_end_matches(';').trim_end();

    // Strip an existing `TOP n` (always directly after SELECT) and any trailing paging
    // clauses, leaving `SELECT * FROM t [WHERE …] [ORDER BY …]`.
    let mut base = sql.to_string();
    if let Some(after_select) = strip_keyword(sql, "SELECT") {
        if let Some(after_top) = strip_keyword(after_select, "TOP") {
            let digits = after_top
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_top.len());
            if digits > 0 {
                let rest = after_top[digits..].trim_start();
                base = format!("SELECT {rest}");
            }
        }
    }
    let (cut, _, _) = trailing_paging(&base);
    base.truncate(cut);
    let base = base.trim_end();

    Some(match kind {
        DbKind::SqlServer => {
            if keyword_positions(base, "ORDER").is_empty() {
                if offset == 0 {
                    // No ORDER BY to hang OFFSET…FETCH on; plain TOP keeps page one simple.
                    let rest = strip_keyword(base, "SELECT").unwrap_or(base);
                    format!("SELECT TOP {limit} {rest};")
                } else {
                    format!(
                        "{base} ORDER BY (SELECT NULL) OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY;"
                    )
                }
            } else {
                format!("{base} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY;")
            }
        }
        _ if offset == 0 => format!("{base} LIMIT {limit};"),
        _ => format!("{base} LIMIT {limit} OFFSET {offset};"),
    })
}

/// `SELECT COUNT(*)` over the same table and WHERE clause as `sql`, ignoring its ORDER BY
/// and paging — the total the pager shows. `None` when `sql` isn't a simple read.
pub fn build_count_sql(sql: &str) -> Option<String> {
    simple_select_target(sql)?;
    let sql = sql.trim().trim_end_matches(';').trim_end();
    let (cut, _, _) = trailing_paging(sql);
    let body = &sql[..cut];
    let from = *keyword_positions(body, "FROM").first()?;
    let end = keyword_positions(body, "ORDER")
        .first()
        .copied()
        .unwrap_or(body.len());
    let from_clause = body[from..end].trim_end();
    Some(format!("SELECT COUNT(*) {from_clause};"))
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
    /// Path to a PEM client certificate for mutual TLS. Empty means none.
    /// Only honoured by backends where [`DbKind::supports_client_cert`] is true.
    #[serde(default)]
    pub ssl_client_cert: String,
    /// Path to the PEM private key matching `ssl_client_cert`. Empty means none.
    #[serde(default)]
    pub ssl_client_key: String,
    // --- SSH tunnel (server backends) ---
    /// Reach the database through an SSH bastion instead of connecting directly.
    /// `host`/`port` above then name the database as seen *from the bastion*.
    #[serde(default)]
    pub ssh_enabled: bool,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub ssh_user: String,
    /// Path to a private key file for the bastion. Empty means password authentication.
    /// The key passphrase / SSH password lives in the keychain, never here.
    #[serde(default)]
    pub ssh_key_path: String,
    // --- file backends ---
    #[serde(default)]
    pub sqlite_path: String,
    /// Optional user-chosen title bar color for visually marking important connections.
    #[serde(default)]
    pub title_bar_color: Option<ConnectionColor>,
    /// Sidebar icon for this connection.
    #[serde(default)]
    pub icon: ConnectionIcon,
    /// Marks a production database: destructive queries (UPDATE/DELETE/DROP/TRUNCATE/ALTER)
    /// must be confirmed in a dialog before they run.
    #[serde(default)]
    pub production: bool,
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
            ssl_client_cert: String::new(),
            ssl_client_key: String::new(),
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: default_ssh_port(),
            ssh_user: String::new(),
            ssh_key_path: String::new(),
            sqlite_path: String::new(),
            title_bar_color: None,
            icon: ConnectionIcon::default(),
            production: false,
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

fn default_ssh_port() -> u16 {
    22
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
    /// The fetch stopped at the caller's row cap; the server had more rows to give.
    pub truncated: bool,
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
    fn parse_page_window_reads_every_dialect() {
        let win = |sql: &str| parse_page_window(sql).unwrap();
        assert_eq!(
            win("SELECT * FROM t"),
            PageWindow { limit: None, offset: 0 }
        );
        assert_eq!(
            win("SELECT * FROM t LIMIT 100;"),
            PageWindow { limit: Some(100), offset: 0 }
        );
        assert_eq!(
            win("SELECT * FROM t LIMIT 100 OFFSET 300"),
            PageWindow { limit: Some(100), offset: 300 }
        );
        // MySQL's comma form puts the offset first.
        assert_eq!(
            win("SELECT * FROM t LIMIT 300, 100"),
            PageWindow { limit: Some(100), offset: 300 }
        );
        assert_eq!(
            win("SELECT TOP 50 * FROM [dbo].[t];"),
            PageWindow { limit: Some(50), offset: 0 }
        );
        assert_eq!(
            win("SELECT * FROM t ORDER BY id OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;"),
            PageWindow { limit: Some(100), offset: 200 }
        );
        // WHERE/ORDER BY don't confuse the parser; quoted text containing keywords is inert.
        assert_eq!(
            win("SELECT * FROM t WHERE name = 'limit 5 offset 2' ORDER BY id LIMIT 10 OFFSET 20"),
            PageWindow { limit: Some(10), offset: 20 }
        );
        // An unquoted column named `offset` in WHERE isn't a paging clause.
        assert_eq!(
            win("SELECT * FROM t WHERE offset > 5 LIMIT 10"),
            PageWindow { limit: Some(10), offset: 0 }
        );
        // Not a simple single-table read → no window.
        assert!(parse_page_window("SELECT a, b FROM t LIMIT 5").is_none());
    }

    #[test]
    fn with_page_window_rewrites_in_place() {
        let pg = |sql: &str, l, o| with_page_window(DbKind::Postgres, sql, l, o).unwrap();
        assert_eq!(pg("SELECT * FROM t LIMIT 100;", 100, 200), "SELECT * FROM t LIMIT 100 OFFSET 200;");
        assert_eq!(pg("SELECT * FROM t LIMIT 100 OFFSET 200;", 100, 0), "SELECT * FROM t LIMIT 100;");
        // WHERE and ORDER BY survive the rewrite.
        assert_eq!(
            pg("SELECT * FROM t WHERE a > 1 ORDER BY a LIMIT 50 OFFSET 50", 50, 100),
            "SELECT * FROM t WHERE a > 1 ORDER BY a LIMIT 50 OFFSET 100;"
        );
        // A query with no paging clause gains one.
        assert_eq!(pg("SELECT * FROM t", 100, 0), "SELECT * FROM t LIMIT 100;");

        let ms = |sql: &str, l, o| with_page_window(DbKind::SqlServer, sql, l, o).unwrap();
        // Page one without ORDER BY keeps the TOP form.
        assert_eq!(ms("SELECT TOP 100 * FROM t;", 100, 0), "SELECT TOP 100 * FROM t;");
        // Deeper pages need OFFSET…FETCH, which needs an ORDER BY.
        assert_eq!(
            ms("SELECT TOP 100 * FROM t;", 100, 200),
            "SELECT * FROM t ORDER BY (SELECT NULL) OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;"
        );
        assert_eq!(
            ms("SELECT * FROM t ORDER BY id OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;", 100, 300),
            "SELECT * FROM t ORDER BY id OFFSET 300 ROWS FETCH NEXT 100 ROWS ONLY;"
        );
        // Joins and projections are refused.
        assert!(with_page_window(DbKind::Postgres, "SELECT a FROM t", 10, 0).is_none());
    }

    #[test]
    fn build_count_sql_keeps_where_drops_order_and_paging() {
        assert_eq!(
            build_count_sql("SELECT * FROM t LIMIT 100 OFFSET 200;").unwrap(),
            "SELECT COUNT(*) FROM t;"
        );
        assert_eq!(
            build_count_sql("SELECT * FROM \"s\".\"t\" WHERE a > 1 ORDER BY a LIMIT 50").unwrap(),
            "SELECT COUNT(*) FROM \"s\".\"t\" WHERE a > 1;"
        );
        assert_eq!(
            build_count_sql("SELECT TOP 100 * FROM [dbo].[t]").unwrap(),
            "SELECT COUNT(*) FROM [dbo].[t];"
        );
        assert!(build_count_sql("SELECT a, b FROM t").is_none());
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
