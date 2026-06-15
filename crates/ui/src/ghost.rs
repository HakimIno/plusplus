//! Inline "ghost text" suggestion for the SQL editor — the greyed-out completion that
//! trails the caret and is accepted with Tab, fish-shell style.
//!
//! Two sources, history first:
//! 1. **History** — the most recent successfully-run statement that starts with what's
//!    typed so far. Covers "re-run that query I wrote yesterday" with one keystroke.
//! 2. **Schema/FK heuristics** — deterministic clause completions from the connected
//!    schema: `SELECT ` → `* FROM `, `FROM users ` → a `JOIN` built from a foreign key,
//!    or a `WHERE <pk> = ` when the table has a single-column primary key.
//!
//! All of it is pure and offline — no model, no network. The suggestion is the text to
//! *append* at the caret; the typed prefix is never rewritten (so its casing is kept).

use dbcore::{DbKind, SchemaTree, TableInfo};

fn is_ident_char(c: char) -> bool {
    // Keep multi-byte names (e.g. Thai, whose combining sara/tone marks aren't
    // `is_alphanumeric`) whole when scanning words back from the caret.
    c.is_alphanumeric() || c == '_' || (!c.is_ascii() && !c.is_whitespace() && !c.is_control())
}

/// Compute the ghost suggestion for `sql` with the caret at char index `cursor`.
///
/// Returns the text to append after the caret, or `None` when nothing fits. Only fires
/// when the caret sits at the very end of the buffer (like a shell autosuggestion) and at
/// least a few characters have been typed, so it never flickers under light editing.
///
/// `history` is newest-last (the order [`dbcore::history::load`] returns).
pub fn suggest(
    sql: &str,
    cursor: usize,
    history: &[&str],
    schema: Option<&SchemaTree>,
    kind: Option<DbKind>,
) -> Option<String> {
    let chars: Vec<char> = sql.chars().collect();
    // Caret must be at the end of the text — autosuggestions only complete the tail.
    if cursor != chars.len() {
        return None;
    }
    // Don't suggest on an empty or whitespace-only buffer, and not until enough has been
    // typed to be discriminating (avoids a suggestion popping up on the first letter).
    let typed = sql.trim_end();
    if typed.trim().chars().count() < 3 {
        return None;
    }

    history_suggestion(sql, history).or_else(|| schema_suggestion(&chars, schema, kind))
}

/// Most-recent history entry that starts with `sql` (case-insensitively) and is strictly
/// longer; the suggestion is the untyped remainder, in the history entry's own casing.
fn history_suggestion(sql: &str, history: &[&str]) -> Option<String> {
    let typed_len = sql.chars().count();
    // Newest first: the last matching entry the user ran wins.
    for entry in history.iter().rev() {
        let entry_chars: Vec<char> = entry.chars().collect();
        if entry_chars.len() <= typed_len {
            continue;
        }
        let prefix: String = entry_chars[..typed_len].iter().collect();
        if prefix.eq_ignore_ascii_case(sql) {
            let remainder: String = entry_chars[typed_len..].iter().collect();
            if !remainder.trim().is_empty() {
                return Some(remainder);
            }
        }
    }
    None
}

/// Deterministic clause completions from the schema. Conservative by design — only fires
/// in a handful of unambiguous spots so a wrong guess never gets in the way.
fn schema_suggestion(
    chars: &[char],
    schema: Option<&SchemaTree>,
    kind: Option<DbKind>,
) -> Option<String> {
    // `SELECT ` with nothing meaningful after it → the near-universal next step. This one
    // needs no schema.
    let trimmed: String = chars.iter().collect::<String>().trim().to_string();
    if trimmed.eq_ignore_ascii_case("SELECT") {
        return Some(" * FROM ".to_string());
    }

    let schema = schema?;

    // The remaining heuristics complete right after `FROM <table> ` / `JOIN <table> `,
    // i.e. the caret follows a table name and a space. Find that shape.
    if chars.last() != Some(&' ') {
        return None;
    }
    let (table_word, before) = last_word(chars, chars.len() - 1)?;
    let keyword = word_before(chars, before)?;
    if !matches!(keyword.to_ascii_uppercase().as_str(), "FROM" | "JOIN") {
        return None;
    }
    let table = schema
        .tables
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(&table_word))?;

    // Prefer a JOIN derived from a foreign key — the highest-value completion.
    if let Some(join) = join_from_fk(table, schema, kind) {
        return Some(join);
    }
    // Otherwise, if the table has a single-column primary key, scaffold a WHERE on it.
    if let Some(pk) = single_pk(table) {
        return Some(format!("WHERE {}.{} = ", qual(&table.name, kind), qual(pk, kind)));
    }
    None
}

/// Build a `JOIN … ON …` clause for `table` from a foreign key: either one `table`
/// declares, or one another table declares *referencing* `table`.
fn join_from_fk(table: &TableInfo, schema: &SchemaTree, kind: Option<DbKind>) -> Option<String> {
    // Outgoing FK: table.col → ref_table.ref_col
    if let Some(fk) = table.foreign_keys.first() {
        if let (Some(col), Some(ref_col)) = (fk.columns.first(), fk.ref_columns.first()) {
            return Some(format!(
                "JOIN {ref_table} ON {ref_table}.{ref_col} = {t}.{col}",
                ref_table = qual(&fk.ref_table, kind),
                ref_col = qual(ref_col, kind),
                t = qual(&table.name, kind),
                col = qual(col, kind),
            ));
        }
    }
    // Incoming FK: some other table references this one.
    for other in &schema.tables {
        if other.name.eq_ignore_ascii_case(&table.name) {
            continue;
        }
        for fk in &other.foreign_keys {
            if fk.ref_table.eq_ignore_ascii_case(&table.name) {
                if let (Some(col), Some(ref_col)) = (fk.columns.first(), fk.ref_columns.first()) {
                    return Some(format!(
                        "JOIN {other} ON {other}.{col} = {t}.{ref_col}",
                        other = qual(&other.name, kind),
                        col = qual(col, kind),
                        t = qual(&table.name, kind),
                        ref_col = qual(ref_col, kind),
                    ));
                }
            }
        }
    }
    None
}

/// The sole primary-key column of `table`, if it has exactly one.
fn single_pk(table: &TableInfo) -> Option<&str> {
    let mut pks = table.columns.iter().filter(|c| c.primary_key);
    let first = pks.next()?;
    if pks.next().is_some() {
        return None; // composite key — no single obvious column to filter on
    }
    Some(&first.name)
}

/// Quote an identifier for the dialect only when it isn't a plain lowercase word, keeping
/// ghost text readable (`users`, but `"My Table"`). Mirrors the editor's own quoting.
fn qual(name: &str, kind: Option<DbKind>) -> String {
    let plain = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit());
    if plain {
        name.to_string()
    } else {
        match kind {
            Some(k) => k.quote_ident(name),
            None => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }
}

/// The identifier ending at `end` (exclusive), plus the index where it starts. Skips one
/// run of trailing whitespace before `end` first.
fn last_word(chars: &[char], end: usize) -> Option<(String, usize)> {
    let mut e = end;
    while e > 0 && chars[e - 1].is_whitespace() {
        e -= 1;
    }
    let mut s = e;
    while s > 0 && is_ident_char(chars[s - 1]) {
        s -= 1;
    }
    if s == e {
        return None;
    }
    Some((chars[s..e].iter().collect(), s))
}

/// The uppercase identifier immediately before index `pos` (skipping whitespace).
fn word_before(chars: &[char], pos: usize) -> Option<String> {
    let (w, _) = last_word(chars, pos)?;
    Some(w.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::{ColumnInfo, ForeignKeyInfo, TableInfo};

    fn col(name: &str, pk: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "int".to_string(),
            nullable: !pk,
            primary_key: pk,
        }
    }

    fn schema() -> SchemaTree {
        SchemaTree {
            database_name: "db".to_string(),
            tables: vec![
                TableInfo {
                    schema: None,
                    name: "users".to_string(),
                    columns: vec![col("id", true), col("email", false)],
                    indexes: vec![],
                    foreign_keys: vec![],
                },
                TableInfo {
                    schema: None,
                    name: "orders".to_string(),
                    columns: vec![col("id", true), col("user_id", false)],
                    indexes: vec![],
                    foreign_keys: vec![ForeignKeyInfo {
                        name: "fk_user".to_string(),
                        columns: vec!["user_id".to_string()],
                        ref_schema: None,
                        ref_table: "users".to_string(),
                        ref_columns: vec!["id".to_string()],
                        on_delete: String::new(),
                        on_update: String::new(),
                    }],
                },
            ],
        }
    }

    fn suggest_end(sql: &str, history: &[String], schema: Option<&SchemaTree>) -> Option<String> {
        let refs: Vec<&str> = history.iter().map(String::as_str).collect();
        suggest(sql, sql.chars().count(), &refs, schema, None)
    }

    #[test]
    fn history_completes_the_tail() {
        let hist = vec!["SELECT * FROM users WHERE id = 1".to_string()];
        let g = suggest_end("SELECT * FROM us", &hist, None).unwrap();
        assert_eq!(g, "ers WHERE id = 1");
    }

    #[test]
    fn history_is_case_insensitive_on_prefix() {
        let hist = vec!["SELECT * FROM users".to_string()];
        let g = suggest_end("select * from us", &hist, None).unwrap();
        // Only the untyped remainder is appended; the typed prefix keeps its casing.
        assert_eq!(g, "ers");
    }

    #[test]
    fn history_newest_match_wins() {
        let hist = vec![
            "SELECT * FROM users WHERE id = 1".to_string(),
            "SELECT * FROM users WHERE id = 2".to_string(),
        ];
        let g = suggest_end("SELECT * FROM users WHERE id = ", &hist, None).unwrap();
        assert_eq!(g, "2");
    }

    #[test]
    fn no_suggestion_when_caret_not_at_end() {
        let hist = vec!["SELECT * FROM users".to_string()];
        let refs: Vec<&str> = hist.iter().map(String::as_str).collect();
        assert!(suggest("SELECT * FROM us", 3, &refs, None, None).is_none());
    }

    #[test]
    fn no_suggestion_below_min_length() {
        let hist = vec!["SELECT 1".to_string()];
        assert!(suggest_end("SE", &hist, None).is_none());
    }

    #[test]
    fn select_keyword_scaffolds_from() {
        assert_eq!(suggest_end("SELECT", &[], None).as_deref(), Some(" * FROM "));
    }

    #[test]
    fn from_table_suggests_fk_join() {
        let s = schema();
        // orders has an outgoing FK to users.
        let g = suggest_end("SELECT * FROM orders ", &[], Some(&s)).unwrap();
        assert_eq!(g, "JOIN users ON users.id = orders.user_id");
    }

    #[test]
    fn from_referenced_table_suggests_incoming_join() {
        let s = schema();
        // users is referenced by orders.user_id.
        let g = suggest_end("SELECT * FROM users ", &[], Some(&s)).unwrap();
        assert_eq!(g, "JOIN orders ON orders.user_id = users.id");
    }

    #[test]
    fn history_beats_schema() {
        let s = schema();
        let hist = vec!["SELECT * FROM orders LIMIT 10".to_string()];
        let g = suggest_end("SELECT * FROM orders ", &hist, Some(&s)).unwrap();
        assert_eq!(g, "LIMIT 10");
    }

    #[test]
    fn unknown_table_yields_nothing() {
        let s = schema();
        // `x` isn't a table in the schema — guard against false-positive suggestions.
        let g = suggest_end("SELECT * FROM x ", &[], Some(&s));
        assert!(g.is_none());
    }

    #[test]
    fn where_pk_scaffold_when_no_fk() {
        let mut s = schema();
        // A standalone table with a single-column PK and no foreign keys.
        s.tables.push(TableInfo {
            schema: None,
            name: "settings".to_string(),
            columns: vec![col("id", true), col("value", false)],
            indexes: vec![],
            foreign_keys: vec![],
        });
        let g = suggest_end("SELECT * FROM settings ", &[], Some(&s)).unwrap();
        assert_eq!(g, "WHERE settings.id = ");
    }

    #[test]
    fn quotes_uppercase_identifiers_for_postgres() {
        assert_eq!(qual("orders", Some(DbKind::Postgres)), "orders");
        assert_eq!(qual("MyTable", Some(DbKind::Postgres)), "\"MyTable\"");
        assert_eq!(qual("has space", None), "\"has space\"");
    }
}
