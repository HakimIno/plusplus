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
//! Both operate on the statement under the caret, not the whole buffer, so a finished
//! statement above the caret never blocks a completion for the one being typed. Table
//! references are resolved through [`crate::sqlctx`], which means aliases (`FROM orders o`)
//! work and generated SQL carries the schema qualifier the backend reported.
//!
//! All of it is pure and offline — no model, no network. The suggestion is the text to
//! *append* at the caret; the typed prefix is never rewritten (so its casing is kept).

use crate::sqlctx;
use dbcore::{DbKind, SchemaTree, TableInfo};

/// Compute the ghost suggestion for `sql` with the caret at char index `cursor`.
///
/// Returns the text to append after the caret, or `None` when nothing fits. Only fires
/// when the caret sits at the very end of the buffer (like a shell autosuggestion) and at
/// least a few characters of the current statement have been typed, so it never flickers
/// under light editing.
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

    // Complete the statement the caret is in, ignoring any that precede it. Leading
    // whitespace is dropped so the newline after a `;` doesn't defeat the history match;
    // trailing whitespace is kept, because `WHERE id = ` completing to `2` depends on it.
    let range = sqlctx::statement_range(&chars, cursor);
    let lead = chars[range.clone()]
        .iter()
        .take_while(|c| c.is_whitespace())
        .count();
    let stmt = &chars[range.start + lead..range.end];

    // Not until enough has been typed to be discriminating (avoids a suggestion popping
    // up on the first letter of a statement).
    let stmt_str: String = stmt.iter().collect();
    if stmt_str.trim().chars().count() < 3 {
        return None;
    }

    history_suggestion(&stmt_str, history).or_else(|| schema_suggestion(stmt, schema, kind))
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

    // The remaining heuristics complete right after a table reference and a space.
    if chars.last() != Some(&' ') {
        return None;
    }
    let (table, correlation, keyword) = table_ref_before_caret(chars, schema)?;
    let in_scope = sqlctx::referenced_tables(chars);

    match keyword {
        // `JOIN t ` needs an `ON`, and the schema knows which columns it goes on.
        Keyword::Join => on_from_fk(table, &correlation, &in_scope, schema, kind),
        Keyword::From => {
            // Prefer a JOIN derived from a foreign key — the highest-value completion.
            if let Some(join) = join_from_fk(table, &correlation, &in_scope, schema, kind) {
                return Some(join);
            }
            // Otherwise, with a single-column primary key, scaffold a WHERE on it.
            let pk = single_pk(table)?;
            Some(format!("WHERE {correlation}.{} = ", qual(pk, kind)))
        }
    }
}

/// Which keyword opened the table reference before the caret. They take different
/// continuations: `FROM t ` may start a new join, `JOIN t ` must be followed by `ON`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Keyword {
    From,
    Join,
}

impl Keyword {
    fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_uppercase().as_str() {
            "FROM" => Some(Keyword::From),
            "JOIN" => Some(Keyword::Join),
            _ => None,
        }
    }
}

/// The `FROM`/`JOIN` target immediately before the caret, the name the rest of the
/// statement uses to refer to it — its alias when one was given, else the bare table
/// name — and the keyword that opened it.
///
/// Recognises `FROM t `, `FROM t a `, and `FROM t AS a `. Returns `None` for anything else,
/// including `FROM t WHERE `, where the word before the caret is a keyword rather than an
/// alias.
fn table_ref_before_caret<'a>(
    chars: &[char],
    schema: &'a SchemaTree,
) -> Option<(&'a TableInfo, String, Keyword)> {
    let find = |name: &str| schema.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name));

    let (word, word_start) = sqlctx::word_at(chars, chars.len())?;
    if sqlctx::is_keyword(&word) {
        return None;
    }

    // `FROM t ` — the word before the caret is the table itself.
    let prev = sqlctx::word_at(chars, word_start);
    if let Some((kw, _)) = &prev {
        if let Some(keyword) = Keyword::parse(kw) {
            return find(&word).map(|t| (t, word, keyword));
        }
    }

    // `FROM t a ` / `FROM t AS a ` — the word before the caret is the alias, so step back
    // over the optional `AS` and check that a table reference opened the clause.
    let (mut name, mut name_start) = prev?;
    if name.eq_ignore_ascii_case("AS") {
        (name, name_start) = sqlctx::word_at(chars, name_start)?;
    }
    let (kw, _) = sqlctx::word_at(chars, name_start)?;
    let keyword = Keyword::parse(&kw)?;
    find(&name).map(|t| (t, word, keyword))
}

/// The `ON …` body joining a freshly-named `JOIN` target back to a table the statement
/// already brought into scope, in whichever direction the foreign key runs.
fn on_from_fk(
    table: &TableInfo,
    correlation: &str,
    in_scope: &[(String, String)],
    schema: &SchemaTree,
    kind: Option<DbKind>,
) -> Option<String> {
    for (alias, name) in in_scope {
        if name.eq_ignore_ascii_case(&table.name) {
            continue; // the target itself
        }
        let Some(other) = schema.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name)) else {
            continue;
        };
        // `other` references the target: `target.ref_col = other.col`.
        for fk in &other.foreign_keys {
            if fk.ref_table.eq_ignore_ascii_case(&table.name) {
                if let Some(on) = on_clause(&fk.columns, &fk.ref_columns, alias, correlation, kind)
                {
                    return Some(format!("ON {on}"));
                }
            }
        }
        // The target references `other`: `other.ref_col = target.col`.
        for fk in &table.foreign_keys {
            if fk.ref_table.eq_ignore_ascii_case(name) {
                if let Some(on) = on_clause(&fk.columns, &fk.ref_columns, correlation, alias, kind)
                {
                    return Some(format!("ON {on}"));
                }
            }
        }
    }
    None
}

/// Build a `JOIN … ON …` clause for `table` — referred to as `correlation` — from a
/// foreign key: either one `table` declares, or one another table declares *referencing*
/// `table`.
///
/// Tables already in scope are skipped. Joining one a second time (a self-join through a
/// `manager_id`, say) needs a distinct alias on the target, and inventing one risks
/// colliding with an alias the user picked; leaving the suggestion out is the safe move.
fn join_from_fk(
    table: &TableInfo,
    correlation: &str,
    in_scope: &[(String, String)],
    schema: &SchemaTree,
    kind: Option<DbKind>,
) -> Option<String> {
    let joined = |name: &str| in_scope.iter().any(|(_, t)| t.eq_ignore_ascii_case(name));

    // Outgoing FK: table.columns → fk.ref_table.ref_columns
    for fk in &table.foreign_keys {
        if joined(&fk.ref_table) {
            continue;
        }
        let target = qual_parts(fk.ref_schema.as_deref(), &fk.ref_table, kind);
        // An unaliased target is referred to by its bare name, not its qualified one.
        let target_ref = qual(&fk.ref_table, kind);
        if let Some(on) = on_clause(&fk.columns, &fk.ref_columns, correlation, &target_ref, kind) {
            return Some(format!("JOIN {target} ON {on}"));
        }
    }
    // Incoming FK: some other table references this one.
    for other in &schema.tables {
        if other.name.eq_ignore_ascii_case(&table.name) || joined(&other.name) {
            continue;
        }
        for fk in &other.foreign_keys {
            if !fk.ref_table.eq_ignore_ascii_case(&table.name) {
                continue;
            }
            let target = qual_parts(other.schema.as_deref(), &other.name, kind);
            let target_ref = qual(&other.name, kind);
            // Mirrored: `fk.ref_columns` sit on `table`, `fk.columns` on `other`.
            if let Some(on) =
                on_clause(&fk.ref_columns, &fk.columns, correlation, &target_ref, kind)
            {
                return Some(format!("JOIN {target} ON {on}"));
            }
        }
    }
    None
}

/// The `ON` body pairing each source column with its target column, `AND`-joined for a
/// composite key. `None` when the constraint is malformed (mismatched arity), so a partial
/// — and silently wrong — join is never suggested.
fn on_clause(
    source_cols: &[String],
    target_cols: &[String],
    source_ref: &str,
    target_ref: &str,
    kind: Option<DbKind>,
) -> Option<String> {
    if source_cols.is_empty() || source_cols.len() != target_cols.len() {
        return None;
    }
    let parts: Vec<String> = source_cols
        .iter()
        .zip(target_cols)
        .map(|(src, tgt)| {
            format!(
                "{target_ref}.{} = {source_ref}.{}",
                qual(tgt, kind),
                qual(src, kind)
            )
        })
        .collect();
    Some(parts.join(" AND "))
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

/// A table reference for generated SQL, schema-qualified when the backend reports a schema
/// (Postgres, SQL Server; MySQL and SQLite report none). Without the qualifier the
/// suggestion would only resolve on the default search path — matching what
/// [`TableInfo::qualified`] produces for the SQL the rest of the app generates.
fn qual_parts(schema: Option<&str>, name: &str, kind: Option<DbKind>) -> String {
    match schema {
        Some(s) => format!("{}.{}", qual(s, kind), qual(name, kind)),
        None => qual(name, kind),
    }
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
            views: Vec::new(),
            routines: Vec::new(),
            triggers: Vec::new(),
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

    // --- statement under the caret ----------------------------------------------------

    #[test]
    fn history_matches_the_statement_under_the_caret() {
        // A finished statement above the caret used to defeat the whole-buffer prefix match.
        let hist = vec!["SELECT * FROM users WHERE id = 1".to_string()];
        let g = suggest_end("SELECT 1;\nSELECT * FROM us", &hist, None).unwrap();
        assert_eq!(g, "ers WHERE id = 1");
    }

    #[test]
    fn schema_heuristics_see_past_a_finished_statement() {
        let s = schema();
        let g = suggest_end("SELECT 1;\nSELECT * FROM orders ", &[], Some(&s)).unwrap();
        assert_eq!(g, "JOIN users ON users.id = orders.user_id");
    }

    #[test]
    fn min_length_applies_to_the_statement_not_the_buffer() {
        // `S` alone must not trigger a match against every history entry starting with it.
        let hist = vec!["SELECT * FROM users".to_string()];
        assert!(suggest_end("SELECT 1; S", &hist, None).is_none());
    }

    // --- aliases ----------------------------------------------------------------------

    #[test]
    fn alias_resolves_for_fk_join() {
        let s = schema();
        let g = suggest_end("SELECT * FROM orders o ", &[], Some(&s)).unwrap();
        assert_eq!(g, "JOIN users ON users.id = o.user_id");
    }

    #[test]
    fn as_alias_resolves_for_fk_join() {
        let s = schema();
        let g = suggest_end("SELECT * FROM orders AS o ", &[], Some(&s)).unwrap();
        assert_eq!(g, "JOIN users ON users.id = o.user_id");
    }

    #[test]
    fn alias_resolves_for_where_scaffold() {
        let mut s = schema();
        s.tables.push(TableInfo {
            schema: None,
            name: "settings".to_string(),
            columns: vec![col("id", true), col("value", false)],
            indexes: vec![],
            foreign_keys: vec![],
        });
        let g = suggest_end("SELECT * FROM settings s ", &[], Some(&s)).unwrap();
        assert_eq!(g, "WHERE s.id = ");
    }

    #[test]
    fn keyword_after_table_is_not_an_alias() {
        let s = schema();
        // `WHERE` closes the table reference; it must never be read as `orders`' alias.
        assert!(suggest_end("SELECT * FROM orders WHERE ", &[], Some(&s)).is_none());
    }

    // --- generated SQL is valid -------------------------------------------------------

    #[test]
    fn join_carries_the_schema_qualifier() {
        let mut s = schema();
        s.tables[0].schema = Some("app".to_string());
        s.tables[1].schema = Some("app".to_string());
        s.tables[1].foreign_keys[0].ref_schema = Some("app".to_string());
        // The target is qualified; its correlation name stays bare, as SQL requires.
        let g = suggest_end("SELECT * FROM orders ", &[], Some(&s)).unwrap();
        assert_eq!(g, "JOIN app.users ON users.id = orders.user_id");
    }

    #[test]
    fn composite_fk_joins_on_every_column_pair() {
        let mut s = schema();
        s.tables[1].foreign_keys[0].columns = vec!["user_id".into(), "tenant_id".into()];
        s.tables[1].foreign_keys[0].ref_columns = vec!["id".into(), "tenant_id".into()];
        let g = suggest_end("SELECT * FROM orders ", &[], Some(&s)).unwrap();
        assert_eq!(
            g,
            "JOIN users ON users.id = orders.user_id AND users.tenant_id = orders.tenant_id"
        );
    }

    #[test]
    fn malformed_fk_arity_never_yields_a_partial_join() {
        let mut s = schema();
        // Two referencing columns, one referenced: a partial ON would be silently wrong.
        s.tables[1].foreign_keys[0].columns = vec!["user_id".into(), "tenant_id".into()];
        let g = suggest_end("SELECT * FROM orders ", &[], Some(&s)).unwrap();
        // The join is dropped; the primary-key scaffold still stands.
        assert_eq!(g, "WHERE orders.id = ");
    }

    // --- JOIN wants an ON, not a WHERE ------------------------------------------------

    #[test]
    fn join_target_scaffolds_its_on_clause() {
        let s = schema();
        let g = suggest_end("SELECT * FROM orders o JOIN users ", &[], Some(&s)).unwrap();
        assert_eq!(g, "ON users.id = o.user_id");
    }

    #[test]
    fn join_target_scaffolds_on_against_an_aliased_scope_table() {
        let s = schema();
        // The FK runs the other way here: the target declares it.
        let g = suggest_end("SELECT * FROM users u JOIN orders o ", &[], Some(&s)).unwrap();
        assert_eq!(g, "ON u.id = o.user_id");
    }

    #[test]
    fn join_target_with_no_relation_suggests_nothing() {
        let mut s = schema();
        s.tables.push(TableInfo {
            schema: None,
            name: "settings".to_string(),
            columns: vec![col("id", true), col("value", false)],
            indexes: vec![],
            foreign_keys: vec![],
        });
        // `settings` relates to nothing in scope — a `WHERE` here would not even parse.
        assert!(suggest_end("SELECT * FROM users u JOIN settings ", &[], Some(&s)).is_none());
    }

    #[test]
    fn self_referencing_fk_does_not_produce_an_ambiguous_join() {
        let mut s = schema();
        s.tables.push(TableInfo {
            schema: None,
            name: "employees".to_string(),
            columns: vec![col("id", true), col("manager_id", false)],
            indexes: vec![],
            foreign_keys: vec![ForeignKeyInfo {
                name: "fk_mgr".to_string(),
                columns: vec!["manager_id".to_string()],
                ref_schema: None,
                ref_table: "employees".to_string(),
                ref_columns: vec!["id".to_string()],
                on_delete: String::new(),
                on_update: String::new(),
            }],
        });
        // A self-join needs an alias on the target; fall back to the WHERE scaffold.
        let g = suggest_end("SELECT * FROM employees ", &[], Some(&s)).unwrap();
        assert_eq!(g, "WHERE employees.id = ");
    }
}
