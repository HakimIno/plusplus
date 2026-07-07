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

/// Build a `SELECT * … WHERE <keys>` capped to `limit` rows, selecting the row(s) a foreign
/// key points at. `keys` pairs each referenced column with the value held in the referencing
/// cell. SQL Server caps with `TOP` (it has no `LIMIT`); the rest append `LIMIT`. Identifiers
/// and string values are escaped for `kind` via [`value_to_literal`] (the same path the
/// UPDATE/DELETE builders use), so caller-supplied values can't break out of the literal.
/// Returns `None` if `keys` is empty or any value can't be rendered as a literal (binary).
pub fn build_select_where_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    keys: &[(&str, &Value)],
    limit: u32,
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
    Some(match kind {
        DbKind::SqlServer => format!("SELECT TOP {limit} * FROM {table_ref} WHERE {where_clause};"),
        _ => format!("SELECT * FROM {table_ref} WHERE {where_clause} LIMIT {limit};"),
    })
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

/// Build one multi-row `INSERT` covering every column, for every row in `rows` (each row's
/// length matching `columns`). Used by "Copy as SQL INSERT". Returns `None` if there are no
/// columns/rows or any value has no literal form (binary — [`Value::Bytes`]). Identifiers and
/// string values are escaped for `kind`.
pub fn build_multi_insert_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    columns: &[ColumnMeta],
    rows: &[&[Value]],
) -> Option<String> {
    if columns.is_empty() || rows.is_empty() {
        return None;
    }
    let table_ref = match schema {
        Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(table)),
        None => kind.quote_ident(table),
    };
    let col_list = columns
        .iter()
        .map(|c| kind.quote_ident(&c.name))
        .collect::<Vec<_>>()
        .join(", ");
    let tuples = rows
        .iter()
        .map(|row| {
            let vals = row
                .iter()
                .map(|v| value_to_literal(v, kind))
                .collect::<Option<Vec<_>>>()?;
            Some(format!("({})", vals.join(", ")))
        })
        .collect::<Option<Vec<_>>>()?
        .join(",\n  ");
    Some(format!(
        "INSERT INTO {table_ref} ({col_list}) VALUES\n  {tuples};"
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
                    && bytes[i..i + k].eq_ignore_ascii_case(kw.as_bytes())
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
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
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

    /// A short security caveat to show beneath the SSL picker, or `None` for the modes that
    /// verify the server's identity (and so need no warning).
    pub fn security_warning(self) -> Option<&'static str> {
        match self {
            SslMode::Disable => {
                Some("Not encrypted — only use on your own machine or a fully trusted network.")
            }
            SslMode::Prefer => Some(
                "Falls back to plaintext when the server has no TLS, and can be forced down to \
                 plaintext by an attacker. Prefer Require or higher.",
            ),
            SslMode::Require => Some(
                "Encrypted, but the server's certificate isn't verified — still open to a \
                 man-in-the-middle. Use Verify Full for production.",
            ),
            SslMode::VerifyCa | SslMode::VerifyFull => None,
        }
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
    /// Hard read-only mode: only provably read statements run (see
    /// [`crate::safety::write_statements`]), in-grid editing and DDL are refused, and the
    /// backends additionally pin the session read-only where the engine supports it
    /// (Postgres `default_transaction_read_only`, MySQL/MariaDB `SET SESSION TRANSACTION
    /// READ ONLY`, SQLite opened read-only; SQL Server has no session-level equivalent —
    /// `ApplicationIntent=ReadOnly` is sent but only enforced by readable replicas).
    #[serde(default)]
    pub read_only: bool,
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
            // New connections default to Require: encrypted, with no silent fallback to
            // plaintext (which an attacker could force). Saved configs are left untouched —
            // a file missing `ssl_mode` still deserializes to Prefer (see SslMode's Default),
            // so upgrading the app never changes an existing connection's security.
            ssl_mode: SslMode::Require,
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
            read_only: false,
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

/// A foreign key as introspected from the schema.
#[derive(Debug, Clone)]
pub struct ForeignKeyInfo {
    /// Constraint name. Empty for SQLite, which doesn't expose one.
    pub name: String,
    /// Referencing columns, in constraint order; pairs positionally with `ref_columns`.
    pub columns: Vec<String>,
    /// Schema of the referenced table, where the backend qualifies it.
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    /// Referential actions as reported by the backend (e.g. "CASCADE", "NO ACTION").
    pub on_delete: String,
    pub on_update: String,
}

impl ForeignKeyInfo {
    /// Human-readable `cols → ref_table(ref_cols)` summary for tree rows and tooltips.
    pub fn display(&self) -> String {
        format!(
            "{} → {}({})",
            self.columns.join(", "),
            self.ref_table,
            self.ref_columns.join(", ")
        )
    }
}

/// A table (or view) with its columns and indexes.
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Schema/namespace the table lives in (e.g. `public` for Postgres). `None` for SQLite.
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

impl TableInfo {
    /// Fully-qualified, quote-safe name for use in generated SQL, quoted for `kind`
    /// (backticks on MySQL/MariaDB, ANSI double quotes elsewhere).
    pub fn qualified(&self, kind: DbKind) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&self.name)),
            None => kind.quote_ident(&self.name),
        }
    }
}

/// A view as introspected from the schema. Like a table it has columns; it also carries
/// the `SELECT` body it was defined with.
#[derive(Debug, Clone)]
pub struct ViewInfo {
    /// Schema/namespace the view lives in. `None` for SQLite.
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    /// The view's defining query (the text after `AS`), as reported by the backend. Empty
    /// when the backend won't surface it (e.g. insufficient privileges).
    pub definition: String,
    /// Postgres materialized view. Always `false` on the other backends.
    pub materialized: bool,
}

impl ViewInfo {
    /// Fully-qualified, quote-safe name for use in generated SQL, quoted for `kind`.
    pub fn qualified(&self, kind: DbKind) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&self.name)),
            None => kind.quote_ident(&self.name),
        }
    }
}

/// Whether a routine is a function (returns a value) or a procedure (called for effect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutineKind {
    Function,
    Procedure,
}

impl RoutineKind {
    pub fn label(self) -> &'static str {
        match self {
            RoutineKind::Function => "Function",
            RoutineKind::Procedure => "Procedure",
        }
    }

    /// SQL keyword for this routine kind (`FUNCTION` / `PROCEDURE`).
    pub fn keyword(self) -> &'static str {
        match self {
            RoutineKind::Function => "FUNCTION",
            RoutineKind::Procedure => "PROCEDURE",
        }
    }
}

/// Parameter-passing mode for a routine parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ParamMode {
    #[default]
    In,
    Out,
    InOut,
    Variadic,
}

impl ParamMode {
    pub const ALL: &'static [ParamMode] = &[
        ParamMode::In,
        ParamMode::Out,
        ParamMode::InOut,
        ParamMode::Variadic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ParamMode::In => "IN",
            ParamMode::Out => "OUT",
            ParamMode::InOut => "INOUT",
            ParamMode::Variadic => "VARIADIC",
        }
    }

    /// Parse a backend-reported parameter mode ("IN", "OUT", "INOUT", "IN OUT", "VARIADIC").
    pub fn from_keyword(s: &str) -> ParamMode {
        match s.trim().replace('_', " ").to_ascii_uppercase().as_str() {
            "OUT" => ParamMode::Out,
            "INOUT" | "IN OUT" => ParamMode::InOut,
            "VARIADIC" => ParamMode::Variadic,
            _ => ParamMode::In,
        }
    }
}

/// A single routine parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutineParam {
    pub name: String,
    pub data_type: String,
    pub mode: ParamMode,
    /// Optional default expression, rendered verbatim. `None` when the parameter has none.
    pub default: Option<String>,
}

/// A stored function or procedure as introspected from the schema.
#[derive(Debug, Clone)]
pub struct RoutineInfo {
    /// Schema/namespace the routine lives in. `None` for SQLite (which has no routines).
    pub schema: Option<String>,
    pub name: String,
    pub kind: RoutineKind,
    pub params: Vec<RoutineParam>,
    /// Return type for functions; `None` for procedures.
    pub return_type: Option<String>,
    /// Implementation language (Postgres: "plpgsql"/"sql"; often empty elsewhere).
    pub language: String,
    /// The routine body / full definition as reported by the backend. May be empty when the
    /// backend won't surface it (e.g. insufficient privileges).
    pub body: String,
}

impl RoutineInfo {
    /// Compact `name(mode arg type, …) → ret` signature for tree rows and tooltips.
    pub fn signature(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|p| {
                let mode = if p.mode == ParamMode::In {
                    String::new()
                } else {
                    format!("{} ", p.mode.label())
                };
                format!("{mode}{} {}", p.name, p.data_type)
                    .trim()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", ");
        match &self.return_type {
            Some(ret) => format!("{}({params}) → {ret}", self.name),
            None => format!("{}({params})", self.name),
        }
    }

    /// Fully-qualified, quote-safe name for generated SQL, quoted for `kind`.
    pub fn qualified(&self, kind: DbKind) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(&self.name)),
            None => kind.quote_ident(&self.name),
        }
    }
}

/// When a trigger fires relative to the triggering statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerTiming {
    #[default]
    Before,
    After,
    InsteadOf,
}

impl TriggerTiming {
    pub const ALL: &'static [TriggerTiming] = &[
        TriggerTiming::Before,
        TriggerTiming::After,
        TriggerTiming::InsteadOf,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriggerTiming::Before => "BEFORE",
            TriggerTiming::After => "AFTER",
            TriggerTiming::InsteadOf => "INSTEAD OF",
        }
    }

    pub fn from_keyword(s: &str) -> Option<TriggerTiming> {
        match s.trim().replace('_', " ").to_ascii_uppercase().as_str() {
            "BEFORE" => Some(TriggerTiming::Before),
            "AFTER" => Some(TriggerTiming::After),
            "INSTEAD OF" => Some(TriggerTiming::InsteadOf),
            _ => None,
        }
    }
}

/// A data-modification event a trigger can fire on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerEvent {
    Insert,
    Update,
    Delete,
}

impl TriggerEvent {
    pub const ALL: &'static [TriggerEvent] = &[
        TriggerEvent::Insert,
        TriggerEvent::Update,
        TriggerEvent::Delete,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriggerEvent::Insert => "INSERT",
            TriggerEvent::Update => "UPDATE",
            TriggerEvent::Delete => "DELETE",
        }
    }

    pub fn from_keyword(s: &str) -> Option<TriggerEvent> {
        match s.trim().to_ascii_uppercase().as_str() {
            "INSERT" => Some(TriggerEvent::Insert),
            "UPDATE" => Some(TriggerEvent::Update),
            "DELETE" => Some(TriggerEvent::Delete),
            _ => None,
        }
    }
}

/// Whether a trigger fires once per affected row or once per statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriggerLevel {
    #[default]
    Row,
    Statement,
}

impl TriggerLevel {
    pub fn label(self) -> &'static str {
        match self {
            TriggerLevel::Row => "FOR EACH ROW",
            TriggerLevel::Statement => "FOR EACH STATEMENT",
        }
    }

    /// The bare granularity keyword (`ROW` / `STATEMENT`) following `FOR EACH`.
    pub fn sql(self) -> &'static str {
        match self {
            TriggerLevel::Row => "ROW",
            TriggerLevel::Statement => "STATEMENT",
        }
    }
}

/// A trigger as introspected from the schema.
#[derive(Debug, Clone)]
pub struct TriggerInfo {
    /// Schema/namespace the trigger lives in. `None` for SQLite.
    pub schema: Option<String>,
    pub name: String,
    /// Table the trigger is attached to.
    pub table: String,
    pub timing: TriggerTiming,
    /// Events the trigger fires on, in declaration order (MySQL allows only one).
    pub events: Vec<TriggerEvent>,
    pub level: TriggerLevel,
    /// `WHEN (...)` guard condition, if any.
    pub when_condition: Option<String>,
    /// The action body — an inline statement block, or for Postgres an
    /// `EXECUTE FUNCTION fn(...)` clause. For SQLite this is the full stored `CREATE TRIGGER`
    /// text, the only form the backend exposes.
    pub action: String,
}

impl TriggerInfo {
    /// Human-readable `BEFORE INSERT ON table` summary for tree rows and tooltips.
    pub fn display(&self) -> String {
        let events = self
            .events
            .iter()
            .map(|e| e.label())
            .collect::<Vec<_>>()
            .join(" OR ");
        format!("{} {} ON {}", self.timing.label(), events, self.table)
    }
}

/// Best-effort extraction of `(timing, events, level, when_condition)` from a full
/// `CREATE TRIGGER` definition — as returned by `pg_get_triggerdef`, SQL Server's
/// `OBJECT_DEFINITION`, or SQLite's `sqlite_master.sql`. Only the *header* (everything
/// before the action clause: `WHEN` / `EXECUTE` / `BEGIN` / `AS`) is scanned, so DML
/// keywords inside the trigger body are never mistaken for the trigger's own events.
/// Reuses [`keyword_positions`], which already skips string literals, comments, and quoted
/// identifiers. Callers that know their dialect's firing granularity (SQLite = row,
/// SQL Server = statement) may override the returned `level`.
pub fn parse_trigger_header(
    def: &str,
) -> (
    TriggerTiming,
    Vec<TriggerEvent>,
    TriggerLevel,
    Option<String>,
) {
    let first = |kw: &str| keyword_positions(def, kw).first().copied();
    let when_at = first("WHEN");
    let action_at = ["EXECUTE", "BEGIN", "AS"]
        .iter()
        .filter_map(|kw| first(kw))
        .min();
    let header_end = [when_at, action_at]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(def.len());
    let header = &def[..header_end];

    let has = |kw: &str| !keyword_positions(header, kw).is_empty();
    let timing = if has("INSTEAD") {
        TriggerTiming::InsteadOf
    } else if has("AFTER") {
        TriggerTiming::After
    } else {
        TriggerTiming::Before
    };
    let events = TriggerEvent::ALL
        .iter()
        .copied()
        .filter(|e| has(e.label()))
        .collect();
    let level = if has("STATEMENT") {
        TriggerLevel::Statement
    } else {
        TriggerLevel::Row
    };
    let when_condition = when_at.and_then(|w| {
        let stop = action_at.unwrap_or(def.len());
        let cond = def.get(w + "WHEN".len()..stop)?.trim();
        (!cond.is_empty()).then(|| cond.to_string())
    });
    (timing, events, level, when_condition)
}

/// Extract the defining `SELECT` from a `CREATE VIEW … AS <select>` statement: the trimmed
/// text after the first top-level `AS` keyword, or the whole input when there's no such
/// separator. Normalises SQL Server's and SQLite's full `CREATE VIEW` text down to the query
/// body (Postgres and MySQL already report only the body). Skips literals/comments via
/// [`keyword_positions`].
pub fn select_body_after_as(create_sql: &str) -> String {
    match keyword_positions(create_sql, "AS").first() {
        Some(&pos) => create_sql[pos + "AS".len()..].trim().to_string(),
        None => create_sql.trim().to_string(),
    }
}

/// The full introspected schema of a connected database.
#[derive(Debug, Clone, Default)]
pub struct SchemaTree {
    pub database_name: String,
    pub tables: Vec<TableInfo>,
    pub views: Vec<ViewInfo>,
    pub routines: Vec<RoutineInfo>,
    pub triggers: Vec<TriggerInfo>,
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

    /// Parse a backend-reported referential action ("CASCADE", "SET NULL", "SET_NULL", …).
    /// Unknown actions (e.g. SET DEFAULT, which the editor doesn't offer) map to `None`.
    pub fn from_rule(rule: &str) -> Option<FkAction> {
        match rule.trim().replace('_', " ").to_ascii_uppercase().as_str() {
            "NO ACTION" => Some(FkAction::NoAction),
            "CASCADE" => Some(FkAction::Cascade),
            "SET NULL" => Some(FkAction::SetNull),
            "RESTRICT" => Some(FkAction::Restrict),
            _ => None,
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
    let cols = fk
        .columns
        .iter()
        .map(|c| kind.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let ref_t = kind.quote_ident(&fk.ref_table);
    let ref_c = fk
        .ref_columns
        .iter()
        .map(|c| kind.quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
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
    let mut defs: Vec<String> = columns
        .iter()
        .map(|c| col_def_sql(kind, c, inline_pk))
        .collect();
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

/// Build the statement that empties a table of all rows, keeping its structure.
/// SQLite has no `TRUNCATE`; it falls back to an unfiltered `DELETE`.
pub fn build_truncate_table_sql(kind: DbKind, schema: Option<&str>, table: &str) -> String {
    let tref = ddl_table_ref(kind, schema, table);
    match kind {
        DbKind::Sqlite => format!("DELETE FROM {tref};"),
        _ => format!("TRUNCATE TABLE {tref};"),
    }
}

/// Build the statement(s) that copy an existing table's structure and data into a new
/// table named `new_table` (in the same schema). Dialects diverge on how much structure
/// survives:
/// - Postgres/MySQL/MariaDB clone the full definition, then bulk-insert the rows, so
///   constraints and indexes are preserved.
/// - SQLite (`CREATE TABLE … AS SELECT`) and SQL Server (`SELECT … INTO`) copy columns and
///   data only — indexes, keys, and constraints are not carried over.
pub fn build_clone_table_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    new_table: &str,
) -> Vec<String> {
    let src = ddl_table_ref(kind, schema, table);
    let dst = ddl_table_ref(kind, schema, new_table);
    match kind {
        DbKind::Postgres => vec![
            format!("CREATE TABLE {dst} (LIKE {src} INCLUDING ALL);"),
            format!("INSERT INTO {dst} SELECT * FROM {src};"),
        ],
        DbKind::MySql | DbKind::MariaDb => vec![
            format!("CREATE TABLE {dst} LIKE {src};"),
            format!("INSERT INTO {dst} SELECT * FROM {src};"),
        ],
        DbKind::SqlServer => vec![format!("SELECT * INTO {dst} FROM {src};")],
        DbKind::Sqlite => vec![format!("CREATE TABLE {dst} AS SELECT * FROM {src};")],
    }
}

/// Build an `ALTER TABLE … ADD [CONSTRAINT] FOREIGN KEY` statement.
/// Not supported by SQLite (which requires a table rebuild); callers must not emit it there.
pub fn build_add_fk_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    fk: &ForeignKeyDef,
) -> String {
    format!(
        "ALTER TABLE {} ADD {};",
        ddl_table_ref(kind, schema, table),
        fk_clause_sql(kind, fk)
    )
}

/// Build the statement dropping a foreign key constraint (dialect-aware).
/// Not supported by SQLite (which requires a table rebuild); callers must not emit it there.
pub fn build_drop_fk_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    constraint: &str,
) -> String {
    let verb = match kind {
        // MySQL/MariaDB use DROP FOREIGN KEY; DROP CONSTRAINT only exists from MySQL 8.0.19.
        DbKind::MySql | DbKind::MariaDb => "DROP FOREIGN KEY",
        _ => "DROP CONSTRAINT",
    };
    format!(
        "ALTER TABLE {} {verb} {};",
        ddl_table_ref(kind, schema, table),
        kind.quote_ident(constraint)
    )
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

/// Build `ALTER TABLE … ALTER/MODIFY COLUMN` statement(s) to change an existing column's
/// type and nullability (and, when a non-empty `default` is given, its DEFAULT).
///
/// Dialects diverge enough that this returns a list: Postgres needs one statement per aspect,
/// while MySQL restates the whole column in a single `MODIFY`. SQLite can't alter a column in
/// place — callers must guard against it (it isn't handled here). `primary_key` is ignored:
/// changing a table's primary key is a separate constraint operation, not a column alter.
pub fn build_alter_column_sql(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col: &ColumnDef,
) -> Vec<String> {
    let tref = ddl_table_ref(kind, schema, table);
    let c = kind.quote_ident(&col.name);
    let ty = col.data_type.trim();
    let default = col
        .default
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());

    match kind {
        DbKind::Postgres => {
            let mut out = vec![
                format!("ALTER TABLE {tref} ALTER COLUMN {c} TYPE {ty};"),
                format!(
                    "ALTER TABLE {tref} ALTER COLUMN {c} {};",
                    if col.nullable {
                        "DROP NOT NULL"
                    } else {
                        "SET NOT NULL"
                    }
                ),
            ];
            if let Some(d) = default {
                out.push(format!(
                    "ALTER TABLE {tref} ALTER COLUMN {c} SET DEFAULT {d};"
                ));
            }
            out
        }
        DbKind::MySql | DbKind::MariaDb => {
            let null = if col.nullable { "NULL" } else { "NOT NULL" };
            let def = default.map(|d| format!(" DEFAULT {d}")).unwrap_or_default();
            vec![format!(
                "ALTER TABLE {tref} MODIFY COLUMN {c} {ty} {null}{def};"
            )]
        }
        DbKind::SqlServer => {
            let null = if col.nullable { "NULL" } else { "NOT NULL" };
            let mut out = vec![format!("ALTER TABLE {tref} ALTER COLUMN {c} {ty} {null};")];
            if let Some(d) = default {
                // SQL Server attaches a DEFAULT through a (here unnamed) constraint.
                out.push(format!("ALTER TABLE {tref} ADD DEFAULT {d} FOR {c};"));
            }
            out
        }
        // SQLite has no in-place column alter; the caller refuses this before reaching here.
        DbKind::Sqlite => Vec::new(),
    }
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
            let qualified = format!("{}.{}.{}", schema.unwrap_or("dbo"), table, old_name);
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

// ─── View DDL builders ───────────────────────────────────────────────────────

/// The in-place "replace" keyword for `CREATE … VIEW`, or `""` when the dialect has none.
/// Postgres/MySQL use `OR REPLACE`; SQL Server uses `OR ALTER` (2016+); SQLite has no such
/// form (callers drop-then-create). Postgres can't `OR REPLACE` a materialized view, so
/// `materialized` suppresses it there too.
fn view_replace_kw(kind: DbKind, materialized: bool) -> &'static str {
    if materialized {
        return "";
    }
    match kind {
        DbKind::Postgres | DbKind::MySql | DbKind::MariaDb => "OR REPLACE ",
        DbKind::SqlServer => "OR ALTER ",
        DbKind::Sqlite => "",
    }
}

/// Build a `CREATE [OR REPLACE] [MATERIALIZED] VIEW … AS <select>` statement.
///
/// `or_replace` requests an in-place redefinition where the dialect supports one (see
/// [`view_supports_replace`]); when it doesn't, the caller must drop the view first.
/// `materialized` is Postgres-only and ignored on the other backends.
pub fn build_create_view_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    select_body: &str,
    materialized: bool,
    or_replace: bool,
) -> String {
    let vref = ddl_table_ref(kind, schema, name);
    let body = select_body.trim().trim_end_matches(';').trim_end();
    let mat = if materialized && kind == DbKind::Postgres {
        "MATERIALIZED "
    } else {
        ""
    };
    let replace = if or_replace {
        view_replace_kw(kind, materialized)
    } else {
        ""
    };
    format!("CREATE {replace}{mat}VIEW {vref} AS\n{body};")
}

/// Build a `DROP [MATERIALIZED] VIEW` statement. `materialized` is Postgres-only.
pub fn build_drop_view_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    materialized: bool,
) -> String {
    let mat = if materialized && kind == DbKind::Postgres {
        "MATERIALIZED "
    } else {
        ""
    };
    format!("DROP {mat}VIEW {};", ddl_table_ref(kind, schema, name))
}

/// Whether `kind` can redefine a view in place (so an edit is a single statement rather than
/// a drop-then-create). False for SQLite, and for Postgres materialized views.
pub fn view_supports_replace(kind: DbKind, materialized: bool) -> bool {
    !view_replace_kw(kind, materialized).is_empty()
}

// ─── Trigger DDL builders ────────────────────────────────────────────────────

/// Inputs for [`build_create_trigger_sql`]. Bundled into a struct because a trigger carries
/// far more dialect-sensitive parts than the other objects.
pub struct TriggerBuild<'a> {
    pub schema: Option<&'a str>,
    pub name: &'a str,
    pub table: &'a str,
    pub timing: TriggerTiming,
    pub events: &'a [TriggerEvent],
    pub level: TriggerLevel,
    pub when_condition: Option<&'a str>,
    /// The trigger action. For MySQL/SQLite/SQL Server this is the statement body. For
    /// Postgres it is either the name of an existing trigger function (when
    /// `pg_existing_function`) or a PL/pgSQL body wrapped in a generated `RETURNS trigger`
    /// function.
    pub body: &'a str,
    /// Postgres only: treat `body` as the name of an existing function to `EXECUTE`.
    pub pg_existing_function: bool,
}

/// Build the statement(s) creating a trigger. Postgres returns two — a backing function plus
/// the trigger — when generating a function from an inline body; the other dialects return
/// one. `Err` is returned for requests a dialect can't express (a `BEFORE` trigger on SQL
/// Server, multiple events on MySQL, an empty body, …).
pub fn build_create_trigger_sql(kind: DbKind, t: &TriggerBuild) -> Result<Vec<String>, String> {
    let name = t.name.trim();
    if name.is_empty() {
        return Err("Trigger name is required.".into());
    }
    if t.table.trim().is_empty() {
        return Err("Trigger requires a target table.".into());
    }
    if t.events.is_empty() {
        return Err("Select at least one event (INSERT / UPDATE / DELETE).".into());
    }
    let body = t.body.trim();
    let tref = ddl_table_ref(kind, t.schema, t.table);
    let nm = kind.quote_ident(name);
    let when = t.when_condition.map(str::trim).filter(|w| !w.is_empty());

    match kind {
        DbKind::Postgres => {
            let events = t
                .events
                .iter()
                .map(|e| e.label())
                .collect::<Vec<_>>()
                .join(" OR ");
            let when_clause = when.map(|w| format!("\nWHEN ({w})")).unwrap_or_default();
            let mut out = Vec::new();
            let call = if t.pg_existing_function {
                if body.is_empty() {
                    return Err("Enter the trigger function to execute.".into());
                }
                if body.ends_with(')') {
                    body.to_string()
                } else {
                    format!("{body}()")
                }
            } else {
                if body.is_empty() {
                    return Err("Enter the trigger function body.".into());
                }
                let fn_ref = match t.schema {
                    Some(s) => format!(
                        "{}.{}",
                        kind.quote_ident(s),
                        kind.quote_ident(&format!("{name}_trigfn"))
                    ),
                    None => kind.quote_ident(&format!("{name}_trigfn")),
                };
                out.push(format!(
                    "CREATE OR REPLACE FUNCTION {fn_ref}()\n\
                     RETURNS trigger LANGUAGE plpgsql AS $$\n{body}\n$$;"
                ));
                format!("{fn_ref}()")
            };
            out.push(format!(
                "CREATE TRIGGER {nm} {} {events} ON {tref}\n\
                 FOR EACH {}{when_clause}\nEXECUTE FUNCTION {call};",
                t.timing.label(),
                t.level.sql(),
            ));
            Ok(out)
        }
        DbKind::MySql | DbKind::MariaDb => {
            if t.timing == TriggerTiming::InsteadOf {
                return Err("MySQL/MariaDB have no INSTEAD OF triggers.".into());
            }
            if t.events.len() != 1 {
                return Err("A MySQL/MariaDB trigger fires on exactly one event.".into());
            }
            if body.is_empty() {
                return Err("Enter the trigger body.".into());
            }
            Ok(vec![format!(
                "CREATE TRIGGER {nm} {} {} ON {tref}\nFOR EACH ROW\n{body};",
                t.timing.label(),
                t.events[0].label(),
            )])
        }
        DbKind::Sqlite => {
            if t.events.len() != 1 {
                return Err("A SQLite trigger fires on a single event.".into());
            }
            if body.is_empty() {
                return Err("Enter the trigger body.".into());
            }
            let when_clause = when.map(|w| format!("\nWHEN ({w})")).unwrap_or_default();
            Ok(vec![format!(
                "CREATE TRIGGER {nm} {} {} ON {tref}\n\
                 FOR EACH ROW{when_clause}\nBEGIN\n{body}\nEND;",
                t.timing.label(),
                t.events[0].label(),
            )])
        }
        DbKind::SqlServer => {
            if t.timing == TriggerTiming::Before {
                return Err("SQL Server has no BEFORE triggers; use AFTER or INSTEAD OF.".into());
            }
            if body.is_empty() {
                return Err("Enter the trigger body.".into());
            }
            let events = t
                .events
                .iter()
                .map(|e| e.label())
                .collect::<Vec<_>>()
                .join(", ");
            Ok(vec![format!(
                "CREATE TRIGGER {nm} ON {tref}\n{} {events}\nAS\n{body};",
                t.timing.label(),
            )])
        }
    }
}

/// Build a `DROP TRIGGER` statement. Postgres needs the owning `table`; the others ignore it.
pub fn build_drop_trigger_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    table: &str,
) -> String {
    let nm = kind.quote_ident(name);
    match kind {
        DbKind::Postgres => format!(
            "DROP TRIGGER {nm} ON {};",
            ddl_table_ref(kind, schema, table)
        ),
        // MySQL allows database-qualifying the trigger; SQL Server allows schema-qualifying it.
        DbKind::MySql | DbKind::MariaDb | DbKind::SqlServer => match schema {
            Some(s) => format!("DROP TRIGGER {}.{nm};", kind.quote_ident(s)),
            None => format!("DROP TRIGGER {nm};"),
        },
        DbKind::Sqlite => format!("DROP TRIGGER {nm};"),
    }
}

// ─── Routine (function / procedure) DDL builders ─────────────────────────────

/// Inputs for [`build_create_routine_sql`].
pub struct RoutineBuild<'a> {
    pub schema: Option<&'a str>,
    pub name: &'a str,
    pub kind: RoutineKind,
    pub params: &'a [RoutineParam],
    /// Return type — required for functions, ignored for procedures.
    pub return_type: Option<&'a str>,
    /// Postgres: "plpgsql" / "sql". Ignored by the other backends.
    pub language: &'a str,
    pub body: &'a str,
}

/// Format a routine's parameter list (comma-separated, no surrounding parentheses) for `kind`.
/// MySQL functions take no mode keyword; SQL Server prefixes `@` and uses `OUTPUT`.
fn routine_params_sql(kind: DbKind, is_function: bool, params: &[RoutineParam]) -> String {
    params
        .iter()
        .map(|p| {
            let ty = p.data_type.trim();
            let nm = p.name.trim();
            let default = p
                .default
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty());
            match kind {
                DbKind::Postgres => {
                    let mode = if p.mode == ParamMode::In {
                        String::new()
                    } else {
                        format!("{} ", p.mode.label())
                    };
                    let def = default.map(|d| format!(" DEFAULT {d}")).unwrap_or_default();
                    format!("{mode}{nm} {ty}{def}")
                }
                DbKind::MySql | DbKind::MariaDb => {
                    // Functions take no mode keyword; procedures may.
                    let mode = if is_function || p.mode == ParamMode::In {
                        String::new()
                    } else {
                        format!("{} ", p.mode.label())
                    };
                    format!("{mode}{nm} {ty}")
                }
                DbKind::SqlServer => {
                    let at = if nm.starts_with('@') {
                        nm.to_string()
                    } else {
                        format!("@{nm}")
                    };
                    let def = default.map(|d| format!(" = {d}")).unwrap_or_default();
                    let out = if matches!(p.mode, ParamMode::Out | ParamMode::InOut) {
                        " OUTPUT"
                    } else {
                        ""
                    };
                    format!("{at} {ty}{def}{out}")
                }
                DbKind::Sqlite => String::new(),
            }
        })
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build a `CREATE [OR REPLACE] FUNCTION|PROCEDURE` statement. SQLite has no routines and
/// returns `Err`. `or_replace` uses `OR REPLACE` on Postgres / `OR ALTER` on SQL Server; the
/// MySQL family has no portable inline replace, so the caller drops first (see the editor).
pub fn build_create_routine_sql(
    kind: DbKind,
    r: &RoutineBuild,
    or_replace: bool,
) -> Result<Vec<String>, String> {
    if kind == DbKind::Sqlite {
        return Err("SQLite has no stored functions or procedures.".into());
    }
    let name = r.name.trim();
    if name.is_empty() {
        return Err("Routine name is required.".into());
    }
    let body = r.body.trim();
    if body.is_empty() {
        return Err("Routine body is required.".into());
    }
    let is_fn = r.kind == RoutineKind::Function;
    let ret = r.return_type.map(str::trim).filter(|s| !s.is_empty());
    if is_fn && ret.is_none() {
        return Err("A function needs a return type.".into());
    }

    let rref = ddl_table_ref(kind, r.schema, name);
    let plist = routine_params_sql(kind, is_fn, r.params);
    let kw = r.kind.keyword();

    let sql = match kind {
        DbKind::Postgres => {
            let repl = if or_replace { "OR REPLACE " } else { "" };
            let lang = {
                let l = r.language.trim();
                if l.is_empty() {
                    "plpgsql"
                } else {
                    l
                }
            };
            let returns = ret.map(|t| format!(" RETURNS {t}")).unwrap_or_default();
            let returns = if is_fn { returns } else { String::new() };
            format!(
                "CREATE {repl}{kw} {rref}({plist}){returns}\nLANGUAGE {lang} AS $$\n{body}\n$$;"
            )
        }
        DbKind::MySql | DbKind::MariaDb => {
            let returns = if is_fn {
                format!(" RETURNS {}", ret.unwrap())
            } else {
                String::new()
            };
            format!("CREATE {kw} {rref}({plist}){returns}\n{body}")
        }
        DbKind::SqlServer => {
            let repl = if or_replace { "OR ALTER " } else { "" };
            if is_fn {
                format!(
                    "CREATE {repl}FUNCTION {rref}({plist})\nRETURNS {}\nAS\n{body};",
                    ret.unwrap()
                )
            } else {
                // SQL Server procedures list parameters without parentheses.
                let params = if plist.is_empty() {
                    String::new()
                } else {
                    format!(" {plist}")
                };
                format!("CREATE {repl}PROCEDURE {rref}{params}\nAS\n{body};")
            }
        }
        DbKind::Sqlite => unreachable!("guarded above"),
    };
    Ok(vec![sql])
}

/// Build a `DROP FUNCTION|PROCEDURE` statement. Postgres disambiguates overloads by argument
/// types (OUT parameters excluded); the others drop by name alone.
pub fn build_drop_routine_sql(
    kind: DbKind,
    schema: Option<&str>,
    name: &str,
    routine_kind: RoutineKind,
    params: &[RoutineParam],
) -> String {
    let kw = routine_kind.keyword();
    let rref = ddl_table_ref(kind, schema, name);
    match kind {
        DbKind::Postgres => {
            let types = params
                .iter()
                .filter(|p| p.mode != ParamMode::Out)
                .map(|p| p.data_type.trim().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("DROP {kw} {rref}({types});")
        }
        _ => format!("DROP {kw} {rref};"),
    }
}

/// Whether `kind` can redefine a routine in place (an edit is a single statement, not a
/// drop-then-create). True for Postgres (`OR REPLACE`) and SQL Server (`OR ALTER`).
pub fn routine_supports_replace(kind: DbKind) -> bool {
    matches!(kind, DbKind::Postgres | DbKind::SqlServer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(sql: &str) -> Option<(Option<String>, String)> {
        simple_select_target(sql)
    }

    #[test]
    fn create_view_per_dialect() {
        // Postgres / MySQL redefine in place with OR REPLACE.
        assert_eq!(
            build_create_view_sql(
                DbKind::Postgres,
                Some("public"),
                "v",
                "SELECT 1",
                false,
                true
            ),
            "CREATE OR REPLACE VIEW \"public\".\"v\" AS\nSELECT 1;"
        );
        assert_eq!(
            build_create_view_sql(DbKind::MySql, None, "v", "SELECT 1;", false, true),
            "CREATE OR REPLACE VIEW `v` AS\nSELECT 1;"
        );
        // SQL Server uses OR ALTER instead.
        assert_eq!(
            build_create_view_sql(DbKind::SqlServer, Some("dbo"), "v", "SELECT 1", false, true),
            "CREATE OR ALTER VIEW \"dbo\".\"v\" AS\nSELECT 1;"
        );
        // SQLite has no replace form even when asked (caller drops first).
        assert_eq!(
            build_create_view_sql(DbKind::Sqlite, None, "v", "SELECT 1", false, false),
            "CREATE VIEW \"v\" AS\nSELECT 1;"
        );
        // Postgres materialized view: MATERIALIZED, and OR REPLACE is suppressed for it.
        assert_eq!(
            build_create_view_sql(DbKind::Postgres, None, "mv", "SELECT 1", true, true),
            "CREATE MATERIALIZED VIEW \"mv\" AS\nSELECT 1;"
        );
    }

    #[test]
    fn drop_view_per_dialect() {
        assert_eq!(
            build_drop_view_sql(DbKind::Postgres, Some("public"), "v", false),
            "DROP VIEW \"public\".\"v\";"
        );
        assert_eq!(
            build_drop_view_sql(DbKind::Postgres, None, "mv", true),
            "DROP MATERIALIZED VIEW \"mv\";"
        );
        assert_eq!(
            build_drop_view_sql(DbKind::MySql, None, "v", false),
            "DROP VIEW `v`;"
        );
        // Only Postgres has materialized views; the keyword is dropped elsewhere.
        assert_eq!(
            build_drop_view_sql(DbKind::MySql, None, "v", true),
            "DROP VIEW `v`;"
        );
    }

    #[test]
    fn view_replace_support_per_dialect() {
        assert!(view_supports_replace(DbKind::Postgres, false));
        assert!(view_supports_replace(DbKind::SqlServer, false));
        assert!(!view_supports_replace(DbKind::Sqlite, false));
        assert!(!view_supports_replace(DbKind::Postgres, true)); // materialized: no OR REPLACE
    }

    #[test]
    fn create_trigger_postgres_generates_function() {
        let t = TriggerBuild {
            schema: Some("public"),
            name: "trg",
            table: "t",
            timing: TriggerTiming::Before,
            events: &[TriggerEvent::Insert, TriggerEvent::Update],
            level: TriggerLevel::Row,
            when_condition: Some("NEW.n > 0"),
            body: "BEGIN NEW.updated := now(); RETURN NEW; END;",
            pg_existing_function: false,
        };
        let sql = build_create_trigger_sql(DbKind::Postgres, &t).unwrap();
        assert_eq!(sql.len(), 2);
        assert!(sql[0].starts_with("CREATE OR REPLACE FUNCTION \"public\".\"trg_trigfn\"()"));
        assert!(
            sql[1].contains("CREATE TRIGGER \"trg\" BEFORE INSERT OR UPDATE ON \"public\".\"t\"")
        );
        assert!(sql[1].contains("FOR EACH ROW"));
        assert!(sql[1].contains("WHEN (NEW.n > 0)"));
        assert!(sql[1].ends_with("EXECUTE FUNCTION \"public\".\"trg_trigfn\"();"));
    }

    #[test]
    fn create_trigger_postgres_existing_function() {
        let t = TriggerBuild {
            schema: None,
            name: "trg",
            table: "t",
            timing: TriggerTiming::After,
            events: &[TriggerEvent::Delete],
            level: TriggerLevel::Statement,
            when_condition: None,
            body: "audit_fn",
            pg_existing_function: true,
        };
        let sql = build_create_trigger_sql(DbKind::Postgres, &t).unwrap();
        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("FOR EACH STATEMENT"));
        assert!(sql[0].ends_with("EXECUTE FUNCTION audit_fn();"));
    }

    #[test]
    fn create_trigger_mysql_single_event_only() {
        let one = TriggerBuild {
            schema: None,
            name: "trg",
            table: "t",
            timing: TriggerTiming::Before,
            events: &[TriggerEvent::Insert],
            level: TriggerLevel::Row,
            when_condition: None,
            body: "SET NEW.created = NOW()",
            pg_existing_function: false,
        };
        assert_eq!(
            build_create_trigger_sql(DbKind::MySql, &one).unwrap(),
            vec![
                "CREATE TRIGGER `trg` BEFORE INSERT ON `t`\nFOR EACH ROW\nSET NEW.created = NOW();"
            ]
        );
        let multi = TriggerBuild {
            events: &[TriggerEvent::Insert, TriggerEvent::Update],
            ..one
        };
        assert!(build_create_trigger_sql(DbKind::MySql, &multi).is_err());
    }

    #[test]
    fn create_trigger_sqlite_wraps_begin_end() {
        let t = TriggerBuild {
            schema: None,
            name: "trg",
            table: "t",
            timing: TriggerTiming::After,
            events: &[TriggerEvent::Update],
            level: TriggerLevel::Row,
            when_condition: Some("NEW.n <> OLD.n"),
            body: "INSERT INTO audit VALUES ('u');",
            pg_existing_function: false,
        };
        let sql = build_create_trigger_sql(DbKind::Sqlite, &t).unwrap();
        assert!(sql[0].contains("CREATE TRIGGER \"trg\" AFTER UPDATE ON \"t\""));
        assert!(sql[0].contains("WHEN (NEW.n <> OLD.n)"));
        assert!(sql[0].contains("BEGIN\nINSERT INTO audit VALUES ('u');\nEND;"));
    }

    #[test]
    fn create_trigger_sqlserver_rejects_before() {
        let after = TriggerBuild {
            schema: Some("dbo"),
            name: "trg",
            table: "t",
            timing: TriggerTiming::After,
            events: &[TriggerEvent::Insert, TriggerEvent::Delete],
            level: TriggerLevel::Statement,
            when_condition: None,
            body: "BEGIN SET NOCOUNT ON; END",
            pg_existing_function: false,
        };
        let sql = build_create_trigger_sql(DbKind::SqlServer, &after).unwrap();
        assert!(sql[0].contains("CREATE TRIGGER \"trg\" ON \"dbo\".\"t\""));
        assert!(sql[0].contains("AFTER INSERT, DELETE"));
        let before = TriggerBuild {
            timing: TriggerTiming::Before,
            ..after
        };
        assert!(build_create_trigger_sql(DbKind::SqlServer, &before).is_err());
    }

    #[test]
    fn drop_trigger_per_dialect() {
        assert_eq!(
            build_drop_trigger_sql(DbKind::Postgres, Some("public"), "trg", "t"),
            "DROP TRIGGER \"trg\" ON \"public\".\"t\";"
        );
        assert_eq!(
            build_drop_trigger_sql(DbKind::MySql, Some("app"), "trg", "t"),
            "DROP TRIGGER `app`.`trg`;"
        );
        assert_eq!(
            build_drop_trigger_sql(DbKind::Sqlite, None, "trg", "t"),
            "DROP TRIGGER \"trg\";"
        );
        assert_eq!(
            build_drop_trigger_sql(DbKind::SqlServer, Some("dbo"), "trg", "t"),
            "DROP TRIGGER \"dbo\".\"trg\";"
        );
    }

    fn param(name: &str, ty: &str, mode: ParamMode, default: Option<&str>) -> RoutineParam {
        RoutineParam {
            name: name.into(),
            data_type: ty.into(),
            mode,
            default: default.map(str::to_string),
        }
    }

    #[test]
    fn create_function_postgres() {
        let params = [
            param("a", "integer", ParamMode::In, None),
            param("b", "integer", ParamMode::In, Some("0")),
        ];
        let r = RoutineBuild {
            schema: Some("public"),
            name: "add",
            kind: RoutineKind::Function,
            params: &params,
            return_type: Some("integer"),
            language: "sql",
            body: "SELECT a + b;",
        };
        assert_eq!(
            build_create_routine_sql(DbKind::Postgres, &r, true).unwrap()[0],
            "CREATE OR REPLACE FUNCTION \"public\".\"add\"(a integer, b integer DEFAULT 0) \
             RETURNS integer\nLANGUAGE sql AS $$\nSELECT a + b;\n$$;"
        );
    }

    #[test]
    fn create_procedure_mysql_with_modes() {
        let params = [
            param("x", "INT", ParamMode::In, None),
            param("y", "INT", ParamMode::Out, None),
        ];
        let r = RoutineBuild {
            schema: None,
            name: "p",
            kind: RoutineKind::Procedure,
            params: &params,
            return_type: None,
            language: "",
            body: "BEGIN SET y = x; END",
        };
        assert_eq!(
            build_create_routine_sql(DbKind::MySql, &r, false).unwrap()[0],
            "CREATE PROCEDURE `p`(x INT, OUT y INT)\nBEGIN SET y = x; END"
        );
    }

    #[test]
    fn create_routine_sqlserver_function_and_procedure() {
        let fparams = [param("a", "int", ParamMode::In, None)];
        let f = RoutineBuild {
            schema: Some("dbo"),
            name: "f",
            kind: RoutineKind::Function,
            params: &fparams,
            return_type: Some("int"),
            language: "",
            body: "BEGIN RETURN @a; END",
        };
        assert_eq!(
            build_create_routine_sql(DbKind::SqlServer, &f, true).unwrap()[0],
            "CREATE OR ALTER FUNCTION \"dbo\".\"f\"(@a int)\nRETURNS int\nAS\nBEGIN RETURN @a; END;"
        );
        // Procedures list parameters without parentheses.
        let pparams = [param("id", "int", ParamMode::In, None)];
        let p = RoutineBuild {
            schema: None,
            name: "p",
            kind: RoutineKind::Procedure,
            params: &pparams,
            return_type: None,
            language: "",
            body: "BEGIN SELECT 1; END",
        };
        assert_eq!(
            build_create_routine_sql(DbKind::SqlServer, &p, false).unwrap()[0],
            "CREATE PROCEDURE \"p\" @id int\nAS\nBEGIN SELECT 1; END;"
        );
    }

    #[test]
    fn routine_validation_and_sqlite() {
        // A function with no return type is rejected.
        let f = RoutineBuild {
            schema: None,
            name: "f",
            kind: RoutineKind::Function,
            params: &[],
            return_type: None,
            language: "sql",
            body: "SELECT 1",
        };
        assert!(build_create_routine_sql(DbKind::Postgres, &f, false).is_err());
        // SQLite has no routines at all.
        let p = RoutineBuild {
            schema: None,
            name: "p",
            kind: RoutineKind::Procedure,
            params: &[],
            return_type: None,
            language: "",
            body: "x",
        };
        assert!(build_create_routine_sql(DbKind::Sqlite, &p, false).is_err());
    }

    #[test]
    fn drop_routine_per_dialect() {
        let params = [
            param("a", "integer", ParamMode::In, None),
            param("o", "integer", ParamMode::Out, None),
        ];
        // Postgres disambiguates by IN/INOUT arg types (OUT excluded).
        assert_eq!(
            build_drop_routine_sql(
                DbKind::Postgres,
                Some("public"),
                "f",
                RoutineKind::Function,
                &params
            ),
            "DROP FUNCTION \"public\".\"f\"(integer);"
        );
        assert_eq!(
            build_drop_routine_sql(DbKind::MySql, None, "p", RoutineKind::Procedure, &params),
            "DROP PROCEDURE `p`;"
        );
        assert_eq!(
            build_drop_routine_sql(
                DbKind::SqlServer,
                Some("dbo"),
                "f",
                RoutineKind::Function,
                &params
            ),
            "DROP FUNCTION \"dbo\".\"f\";"
        );
        assert!(routine_supports_replace(DbKind::Postgres));
        assert!(routine_supports_replace(DbKind::SqlServer));
        assert!(!routine_supports_replace(DbKind::MySql));
    }

    /// New connections start at Require (encrypted, no plaintext fallback). The bare
    /// `SslMode::default()` stays Prefer — that's the value an old, pre-TLS config file
    /// deserializes to, and it must not change underneath existing connections.
    #[test]
    fn new_connection_defaults_to_require_but_default_stays_prefer() {
        assert_eq!(
            ConnectionConfig::new(DbKind::Postgres).ssl_mode,
            SslMode::Require
        );
        assert_eq!(SslMode::default(), SslMode::Prefer);
        // Only the non-verifying modes carry a warning; the verifying ones don't.
        assert!(SslMode::Prefer.security_warning().is_some());
        assert!(SslMode::Require.security_warning().is_some());
        assert!(SslMode::VerifyFull.security_warning().is_none());
    }

    #[test]
    fn alter_column_postgres_splits_type_and_nullability() {
        let col = ColumnDef {
            name: "price".into(),
            data_type: "numeric(10,2)".into(),
            nullable: false,
            primary_key: false,
            default: Some("0".into()),
        };
        let sql = build_alter_column_sql(DbKind::Postgres, Some("public"), "items", &col);
        assert_eq!(
            sql,
            vec![
                "ALTER TABLE \"public\".\"items\" ALTER COLUMN \"price\" TYPE numeric(10,2);",
                "ALTER TABLE \"public\".\"items\" ALTER COLUMN \"price\" SET NOT NULL;",
                "ALTER TABLE \"public\".\"items\" ALTER COLUMN \"price\" SET DEFAULT 0;",
            ]
        );
    }

    #[test]
    fn alter_column_mysql_uses_single_modify() {
        let col = ColumnDef {
            name: "name".into(),
            data_type: "varchar(255)".into(),
            nullable: true,
            primary_key: false,
            default: None,
        };
        let sql = build_alter_column_sql(DbKind::MySql, None, "users", &col);
        assert_eq!(
            sql,
            vec!["ALTER TABLE `users` MODIFY COLUMN `name` varchar(255) NULL;"]
        );
    }

    #[test]
    fn alter_column_sqlserver_alters_then_adds_default() {
        let col = ColumnDef {
            name: "qty".into(),
            data_type: "int".into(),
            nullable: false,
            primary_key: false,
            default: Some("1".into()),
        };
        let sql = build_alter_column_sql(DbKind::SqlServer, Some("dbo"), "orders", &col);
        assert_eq!(
            sql,
            vec![
                "ALTER TABLE \"dbo\".\"orders\" ALTER COLUMN \"qty\" int NOT NULL;",
                "ALTER TABLE \"dbo\".\"orders\" ADD DEFAULT 1 FOR \"qty\";",
            ]
        );
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
            PageWindow {
                limit: None,
                offset: 0
            }
        );
        assert_eq!(
            win("SELECT * FROM t LIMIT 100;"),
            PageWindow {
                limit: Some(100),
                offset: 0
            }
        );
        assert_eq!(
            win("SELECT * FROM t LIMIT 100 OFFSET 300"),
            PageWindow {
                limit: Some(100),
                offset: 300
            }
        );
        // MySQL's comma form puts the offset first.
        assert_eq!(
            win("SELECT * FROM t LIMIT 300, 100"),
            PageWindow {
                limit: Some(100),
                offset: 300
            }
        );
        assert_eq!(
            win("SELECT TOP 50 * FROM [dbo].[t];"),
            PageWindow {
                limit: Some(50),
                offset: 0
            }
        );
        assert_eq!(
            win("SELECT * FROM t ORDER BY id OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;"),
            PageWindow {
                limit: Some(100),
                offset: 200
            }
        );
        // WHERE/ORDER BY don't confuse the parser; quoted text containing keywords is inert.
        assert_eq!(
            win("SELECT * FROM t WHERE name = 'limit 5 offset 2' ORDER BY id LIMIT 10 OFFSET 20"),
            PageWindow {
                limit: Some(10),
                offset: 20
            }
        );
        // An unquoted column named `offset` in WHERE isn't a paging clause.
        assert_eq!(
            win("SELECT * FROM t WHERE offset > 5 LIMIT 10"),
            PageWindow {
                limit: Some(10),
                offset: 0
            }
        );
        // Not a simple single-table read → no window.
        assert!(parse_page_window("SELECT a, b FROM t LIMIT 5").is_none());
    }

    #[test]
    fn with_page_window_rewrites_in_place() {
        let pg = |sql: &str, l, o| with_page_window(DbKind::Postgres, sql, l, o).unwrap();
        assert_eq!(
            pg("SELECT * FROM t LIMIT 100;", 100, 200),
            "SELECT * FROM t LIMIT 100 OFFSET 200;"
        );
        assert_eq!(
            pg("SELECT * FROM t LIMIT 100 OFFSET 200;", 100, 0),
            "SELECT * FROM t LIMIT 100;"
        );
        // WHERE and ORDER BY survive the rewrite.
        assert_eq!(
            pg(
                "SELECT * FROM t WHERE a > 1 ORDER BY a LIMIT 50 OFFSET 50",
                50,
                100
            ),
            "SELECT * FROM t WHERE a > 1 ORDER BY a LIMIT 50 OFFSET 100;"
        );
        // A query with no paging clause gains one.
        assert_eq!(pg("SELECT * FROM t", 100, 0), "SELECT * FROM t LIMIT 100;");

        let ms = |sql: &str, l, o| with_page_window(DbKind::SqlServer, sql, l, o).unwrap();
        // Page one without ORDER BY keeps the TOP form.
        assert_eq!(
            ms("SELECT TOP 100 * FROM t;", 100, 0),
            "SELECT TOP 100 * FROM t;"
        );
        // Deeper pages need OFFSET…FETCH, which needs an ORDER BY.
        assert_eq!(
            ms("SELECT TOP 100 * FROM t;", 100, 200),
            "SELECT * FROM t ORDER BY (SELECT NULL) OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;"
        );
        assert_eq!(
            ms(
                "SELECT * FROM t ORDER BY id OFFSET 200 ROWS FETCH NEXT 100 ROWS ONLY;",
                100,
                300
            ),
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

    #[test]
    fn build_select_where_follows_a_foreign_key() {
        // Single-column FK: filter the referenced table to the pointed-at key.
        let uid = Value::Int(7);
        assert_eq!(
            build_select_where_sql(
                DbKind::Postgres,
                Some("public"),
                "users",
                &[("id", &uid)],
                100
            ),
            Some("SELECT * FROM \"public\".\"users\" WHERE \"id\" = 7 LIMIT 100;".to_string())
        );
        // SQL Server caps with TOP, not LIMIT.
        assert_eq!(
            build_select_where_sql(DbKind::SqlServer, None, "users", &[("id", &uid)], 100),
            Some("SELECT TOP 100 * FROM \"users\" WHERE \"id\" = 7;".to_string())
        );
        // Composite FK ANDs the key columns; string values are escaped (no literal breakout).
        let tenant = Value::Text("O'Brien".into());
        let seq = Value::Int(3);
        assert_eq!(
            build_select_where_sql(
                DbKind::MySql,
                None,
                "orders",
                &[("tenant", &tenant), ("seq", &seq)],
                100
            ),
            Some(
                "SELECT * FROM `orders` WHERE `tenant` = 'O''Brien' AND `seq` = 3 LIMIT 100;"
                    .to_string()
            )
        );
        // No keys ⇒ refuse (never emit an unfiltered scan); binary keys have no literal form.
        assert_eq!(
            build_select_where_sql(DbKind::Postgres, None, "t", &[], 100),
            None
        );
        let blob = Value::Bytes(vec![1, 2, 3]);
        assert_eq!(
            build_select_where_sql(DbKind::Postgres, None, "t", &[("k", &blob)], 100),
            None
        );
    }

    #[test]
    fn fk_action_parses_backend_rule_spellings() {
        // information_schema spells it with spaces; sys.foreign_keys with underscores.
        assert_eq!(FkAction::from_rule("CASCADE"), Some(FkAction::Cascade));
        assert_eq!(FkAction::from_rule("SET NULL"), Some(FkAction::SetNull));
        assert_eq!(FkAction::from_rule("SET_NULL"), Some(FkAction::SetNull));
        assert_eq!(FkAction::from_rule("no action"), Some(FkAction::NoAction));
        assert_eq!(FkAction::from_rule("RESTRICT"), Some(FkAction::Restrict));
        assert_eq!(FkAction::from_rule("SET DEFAULT"), None);
        assert_eq!(FkAction::from_rule(""), None);
    }

    #[test]
    fn build_fk_ddl_is_dialect_aware() {
        let fk = ForeignKeyDef {
            name: "fk_orders_user".into(),
            columns: vec!["user_id".into()],
            ref_table: "users".into(),
            ref_columns: vec!["id".into()],
            on_delete: FkAction::Cascade,
        };
        assert_eq!(
            build_add_fk_sql(DbKind::Postgres, Some("public"), "orders", &fk),
            "ALTER TABLE \"public\".\"orders\" ADD CONSTRAINT \"fk_orders_user\" \
             FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\") ON DELETE CASCADE;"
        );
        assert_eq!(
            build_drop_fk_sql(DbKind::Postgres, Some("public"), "orders", "fk_orders_user"),
            "ALTER TABLE \"public\".\"orders\" DROP CONSTRAINT \"fk_orders_user\";"
        );
        // MySQL drops via DROP FOREIGN KEY, with backtick identifiers.
        assert_eq!(
            build_drop_fk_sql(DbKind::MySql, None, "orders", "fk_orders_user"),
            "ALTER TABLE `orders` DROP FOREIGN KEY `fk_orders_user`;"
        );
    }

    #[test]
    fn truncate_table_dialects() {
        assert_eq!(
            build_truncate_table_sql(DbKind::Postgres, Some("public"), "orders"),
            "TRUNCATE TABLE \"public\".\"orders\";"
        );
        // SQLite has no TRUNCATE — it falls back to an unfiltered DELETE.
        assert_eq!(
            build_truncate_table_sql(DbKind::Sqlite, None, "orders"),
            "DELETE FROM \"orders\";"
        );
    }

    #[test]
    fn clone_table_dialects() {
        // Postgres preserves structure (LIKE … INCLUDING ALL), then copies rows.
        assert_eq!(
            build_clone_table_sql(DbKind::Postgres, Some("public"), "orders", "orders_copy"),
            vec![
                "CREATE TABLE \"public\".\"orders_copy\" (LIKE \"public\".\"orders\" INCLUDING ALL);"
                    .to_string(),
                "INSERT INTO \"public\".\"orders_copy\" SELECT * FROM \"public\".\"orders\";"
                    .to_string(),
            ]
        );
        // MySQL uses CREATE TABLE … LIKE with backtick identifiers.
        assert_eq!(
            build_clone_table_sql(DbKind::MySql, None, "orders", "orders_copy"),
            vec![
                "CREATE TABLE `orders_copy` LIKE `orders`;".to_string(),
                "INSERT INTO `orders_copy` SELECT * FROM `orders`;".to_string(),
            ]
        );
        // SQLite copies columns + data only, in a single statement.
        assert_eq!(
            build_clone_table_sql(DbKind::Sqlite, None, "orders", "orders_copy"),
            vec!["CREATE TABLE \"orders_copy\" AS SELECT * FROM \"orders\";".to_string()]
        );
    }
}
