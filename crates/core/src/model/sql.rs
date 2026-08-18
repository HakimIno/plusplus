//! SQL DML builders, single-table query recognition, and pagination helpers.

use super::{ColumnMeta, DbKind};
use crate::value::Value;

/// Render `value` as a SQL literal for `kind`, safely escaping strings. Returns `None` for
/// [`Value::Bytes`], which has no portable literal form (those cells aren't editable).
fn value_to_literal(value: &Value, kind: DbKind) -> Option<String> {
    Some(match value {
        Value::Null => "NULL".to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.is_finite() => f.to_string(),
        Value::Float(_) => return None,
        Value::Bool(b) => match kind {
            // Postgres and CQL have a real boolean type; the others store it as an
            // integer/bit (CQL additionally rejects 0/1 as boolean literals).
            DbKind::Postgres | DbKind::DuckDb | DbKind::Cassandra | DbKind::ScyllaDb => {
                if *b { "TRUE" } else { "FALSE" }.to_string()
            }
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
    let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    build_multi_insert_for(kind, schema, table, &names, rows)
}

/// Build one multi-row `INSERT` over an explicit subset of columns — the columns a file import
/// actually maps, leaving the rest to their database defaults. Each row's length must match
/// `col_names`.
///
/// Returns `None` if there are no columns/rows, or any value has no literal form (binary —
/// [`Value::Bytes`]). `col_names` are quoted as identifiers for `kind` and values escaped as
/// literals, so neither may originate from an untrusted file: import passes the *table's* own
/// introspected column names here.
pub fn build_multi_insert_for(
    kind: DbKind,
    schema: Option<&str>,
    table: &str,
    col_names: &[&str],
    rows: &[&[Value]],
) -> Option<String> {
    if col_names.is_empty() || rows.is_empty() {
        return None;
    }
    let table_ref = match schema {
        Some(s) => format!("{}.{}", kind.quote_ident(s), kind.quote_ident(table)),
        None => kind.quote_ident(table),
    };
    let col_list = col_names
        .iter()
        .map(|c| kind.quote_ident(c))
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
pub(super) fn keyword_positions(sql: &str, kw: &str) -> Vec<usize> {
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

/// Build a stable keyset page for a simple `SELECT *` using ascending unique-key columns.
/// Existing filters are preserved and combined with the cursor predicate. Queries with a
/// custom `ORDER BY`, non-zero OFFSET, CQL dialects, NULL/binary cursors, or no keys return
/// `None` so callers can safely fall back to offset pagination.
pub fn with_keyset_page(
    kind: DbKind,
    sql: &str,
    key_columns: &[String],
    after: Option<&[Value]>,
    limit: u64,
) -> Option<String> {
    if key_columns.is_empty() || limit == 0 || kind.is_cql() {
        return None;
    }
    simple_select_target(sql)?;
    let window = parse_page_window(sql)?;
    if window.offset != 0 {
        return None;
    }

    let sql = sql.trim().trim_end_matches(';').trim_end();
    let mut base = sql.to_string();
    if let Some(after_select) = strip_keyword(sql, "SELECT") {
        if let Some(after_top) = strip_keyword(after_select, "TOP") {
            let digits = after_top
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_top.len());
            if digits > 0 {
                base = format!("SELECT {}", after_top[digits..].trim_start());
            }
        }
    }
    let (cut, _, _) = trailing_paging(&base);
    base.truncate(cut);
    let base = base.trim_end();
    if !keyword_positions(base, "ORDER").is_empty() {
        return None;
    }

    let order = key_columns
        .iter()
        .map(|column| kind.quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ");
    let filtered = if let Some(values) = after {
        if values.len() != key_columns.len()
            || values
                .iter()
                .any(|value| matches!(value, Value::Null | Value::Bytes(_)))
        {
            return None;
        }
        let literals = values
            .iter()
            .map(|value| value_to_literal(value, kind))
            .collect::<Option<Vec<_>>>()?;
        let cursor = (0..key_columns.len())
            .map(|greater_idx| {
                let mut terms = (0..greater_idx)
                    .map(|equal_idx| {
                        format!(
                            "{} = {}",
                            kind.quote_ident(&key_columns[equal_idx]),
                            literals[equal_idx]
                        )
                    })
                    .collect::<Vec<_>>();
                terms.push(format!(
                    "{} > {}",
                    kind.quote_ident(&key_columns[greater_idx]),
                    literals[greater_idx]
                ));
                format!("({})", terms.join(" AND "))
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        if let Some(where_pos) = keyword_positions(base, "WHERE").first().copied() {
            let prefix = base[..where_pos].trim_end();
            let condition = base[where_pos + "WHERE".len()..].trim();
            format!("{prefix} WHERE ({condition}) AND ({cursor})")
        } else {
            format!("{base} WHERE {cursor}")
        }
    } else {
        base.to_string()
    };

    Some(match kind {
        DbKind::SqlServer => {
            let rest = strip_keyword(&filtered, "SELECT")?;
            format!("SELECT TOP {limit} {rest} ORDER BY {order};")
        }
        _ => format!("{filtered} ORDER BY {order} LIMIT {limit};"),
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
