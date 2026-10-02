//! The part of ghost text that keeps going after the first suggestion.
//!
//! [`super::ghost`]'s original heuristics fire in one spot — right after a table name — so a
//! suggestion helped once and then went quiet the moment it was accepted. This module reads
//! *which clause the caret is in* and offers the natural next step of that clause, so each
//! Tab leads to the next suggestion and a whole query can be driven from the keyboard:
//!
//! ```text
//! FROM orders  →  JOIN users ON …  →  WHERE orders.id =  →  <your usual value>  →
//! ORDER BY orders.id DESC  →  LIMIT 100
//! UPDATE users  →  SET  →  email =  →  <your usual value>  →  WHERE users.id =
//! ```
//!
//! Two things make it more than a template: the column for a new `WHERE` is chosen from the
//! table's *indexes* (a filter that can use one), and the value after `col =` is learned from
//! the literals the user has actually compared that column with before.
//!
//! Everything here is conservative. It stays silent inside parentheses, set operations,
//! `INSERT`, and anywhere the clause can't be read with confidence — a wrong guess is worse
//! than none.

use dbcore::{DbKind, SchemaTree, TableInfo};

use super::ghost::{join_from_fk, qual, single_pk};
use crate::sqlctx;

/// Rows a suggested `LIMIT` caps a query at.
const DEFAULT_LIMIT: u32 = 100;
/// History entries considered when learning values (newest first).
const HISTORY_WINDOW: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    /// A quoted string literal, quotes included.
    Str(String),
    Num(String),
    Op(String),
    Open,
    Close,
    Comma,
    Dot,
}

fn tokenize(chars: &[char]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if c == '\'' {
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            out.push(Tok::Str(chars[start..i].iter().collect()));
        } else if let Some(close) = sqlctx::opening_quote(c).map(closing_quote) {
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != close {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i.min(chars.len())].iter().collect()));
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num(chars[start..i].iter().collect()));
        } else if sqlctx::is_ident_char(c) {
            let start = i;
            while i < chars.len() && sqlctx::is_ident_char(chars[i]) {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i].iter().collect()));
        } else {
            i += 1;
            match c {
                '(' => out.push(Tok::Open),
                ')' => out.push(Tok::Close),
                ',' => out.push(Tok::Comma),
                '.' => out.push(Tok::Dot),
                '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%' | '|' | '&' | ':' | '?' => {
                    let start = i - 1;
                    while i < chars.len() && matches!(chars[i], '=' | '<' | '>' | '!') {
                        i += 1;
                    }
                    out.push(Tok::Op(chars[start..i].iter().collect()));
                }
                _ => {}
            }
        }
    }
    out
}

fn closing_quote(open: char) -> char {
    if open == '[' {
        ']'
    } else {
        open
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    Select,
    From,
    Join,
    On,
    Where,
    GroupBy,
    Having,
    OrderBy,
    Limit,
    Set,
    Update,
}

struct Flow<'a> {
    clause: Clause,
    /// Tokens after the clause keyword, up to the caret.
    body: &'a [Tok],
}

/// The clause the caret is in, or `None` when the statement isn't one this can read safely.
fn locate(tokens: &[Tok]) -> Option<Flow<'_>> {
    let mut depth = 0_i32;
    let mut found: Option<(Clause, usize)> = None;
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Tok::Open => depth += 1,
            Tok::Close => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            Tok::Word(word) if depth == 0 => {
                let by =
                    matches!(tokens.get(i + 1), Some(Tok::Word(n)) if n.eq_ignore_ascii_case("BY"));
                match word.to_ascii_uppercase().as_str() {
                    "SELECT" => found = Some((Clause::Select, i + 1)),
                    "FROM" => found = Some((Clause::From, i + 1)),
                    "JOIN" => found = Some((Clause::Join, i + 1)),
                    "ON" => found = Some((Clause::On, i + 1)),
                    "WHERE" => found = Some((Clause::Where, i + 1)),
                    "HAVING" => found = Some((Clause::Having, i + 1)),
                    "LIMIT" => found = Some((Clause::Limit, i + 1)),
                    "SET" => found = Some((Clause::Set, i + 1)),
                    "UPDATE" => found = Some((Clause::Update, i + 1)),
                    "GROUP" if by => {
                        found = Some((Clause::GroupBy, i + 2));
                        i += 1;
                    }
                    "ORDER" if by => {
                        found = Some((Clause::OrderBy, i + 2));
                        i += 1;
                    }
                    // Shapes whose next step this can't judge.
                    "UNION" | "INTERSECT" | "EXCEPT" | "WITH" | "VALUES" | "RETURNING"
                    | "OFFSET" | "WINDOW" | "INSERT" | "MERGE" | "CASE" => return None,
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    let (clause, start) = found?;
    Some(Flow {
        clause,
        body: tokens.get(start..)?,
    })
}

struct Ctx<'a> {
    schema: &'a SchemaTree,
    kind: Option<DbKind>,
    scope: &'a [(String, String)],
    history: &'a [&'a str],
}

impl Ctx<'_> {
    fn table(&self, name: &str) -> Option<&TableInfo> {
        self.schema
            .tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// The statement's main table (first one in scope that the schema knows) and the name
    /// the statement refers to it by.
    fn main(&self) -> Option<(&str, &TableInfo)> {
        self.scope
            .iter()
            .find_map(|(alias, name)| Some((alias.as_str(), self.table(name)?)))
    }

    /// `alias.column`, or just `column` when only one table is in play.
    fn column_ref(&self, alias: &str, column: &str) -> String {
        if self.scope.len() > 1 {
            format!("{}.{}", qual(alias, self.kind), qual(column, self.kind))
        } else {
            qual(column, self.kind)
        }
    }

    /// A column worth filtering on: the first column of an index (a predicate that can use it),
    /// then the primary key, skipping any the statement already mentions.
    fn filter_column<'t>(&self, table: &'t TableInfo, mentioned: &[Tok]) -> Option<&'t str> {
        let used = |name: &str| {
            mentioned
                .iter()
                .any(|t| matches!(t, Tok::Word(w) if w.eq_ignore_ascii_case(name)))
        };
        let indexed = table
            .indexes
            .iter()
            .filter_map(|index| index.columns.first().map(String::as_str));
        let keys = table
            .columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.as_str());
        indexed.chain(keys).find(|name| !used(name))
    }
}

/// The next clause-shaped step after the caret, or `None`. The caller guarantees `stmt` is
/// the statement under the caret.
pub(super) fn next_clause(
    stmt: &[char],
    history: &[&str],
    schema: &SchemaTree,
    kind: Option<DbKind>,
) -> Option<String> {
    // Mid-word is the popup's territory; a ghost here would fight the word being typed.
    if !stmt.last().is_some_and(|c| c.is_whitespace()) {
        return None;
    }
    let tokens = tokenize(stmt);
    let flow = locate(&tokens)?;
    let scope = sqlctx::referenced_tables(stmt);
    let cx = Ctx {
        schema,
        kind,
        scope: &scope,
        history,
    };
    match flow.clause {
        Clause::Where => where_next(&cx, &tokens, flow.body),
        Clause::On => on_next(&cx, &tokens, flow.body),
        Clause::OrderBy => order_next(&cx, flow.body),
        Clause::Set => set_next(&cx, flow.body),
        Clause::Select => select_next(flow.body),
        _ => None,
    }
}

fn is_comparison(op: &str) -> bool {
    matches!(op, "=" | "<>" | "!=" | "<" | ">" | "<=" | ">=")
}

/// The body ends with `column <comparison>` — the caret is where a value goes. Returns the
/// column name and the operator.
fn awaiting_value(body: &[Tok]) -> Option<(&str, &str)> {
    match body {
        [.., Tok::Word(column), Tok::Op(op)] if is_comparison(op) => {
            Some((column.as_str(), op.as_str()))
        }
        _ => None,
    }
}

/// A trailing complete `column op literal` predicate.
fn ends_with_predicate(body: &[Tok]) -> bool {
    matches!(
        body,
        [.., Tok::Word(_), Tok::Op(op), Tok::Str(_) | Tok::Num(_)] if is_comparison(op)
    ) || matches!(body, [.., Tok::Close])
}

fn where_next(cx: &Ctx, tokens: &[Tok], body: &[Tok]) -> Option<String> {
    let (alias, table) = cx.main()?;
    // `WHERE ` / `… AND ` → a new predicate on a column worth filtering.
    let starts_predicate = body.is_empty()
        || matches!(body.last(), Some(Tok::Word(w))
            if w.eq_ignore_ascii_case("AND") || w.eq_ignore_ascii_case("OR"));
    if starts_predicate {
        let column = cx.filter_column(table, body)?;
        return Some(format!("{} = ", cx.column_ref(alias, column)));
    }
    // `col = ` → the value this user usually compares it with.
    if let Some((column, op)) = awaiting_value(body) {
        return learned_value(cx.history, column, op);
    }
    // A finished predicate on a SELECT: cap the result, newest first when there's a key to
    // order by. (An UPDATE/DELETE has no ORDER BY to offer; its WHERE is the end of the road.)
    let is_select =
        matches!(tokens.first(), Some(Tok::Word(w)) if w.eq_ignore_ascii_case("SELECT"));
    if is_select && ends_with_predicate(body) {
        return Some(match single_pk(table) {
            Some(pk) => format!("ORDER BY {} DESC ", cx.column_ref(alias, pk)),
            None => format!("LIMIT {DEFAULT_LIMIT}"),
        });
    }
    None
}

fn on_next(cx: &Ctx, tokens: &[Tok], body: &[Tok]) -> Option<String> {
    // Only a finished `a.x = b.y`: otherwise the user is mid-condition.
    let finished = body.iter().any(|t| matches!(t, Tok::Op(op) if op == "="))
        && matches!(body.last(), Some(Tok::Word(w))
            if !matches!(w.to_ascii_uppercase().as_str(), "AND" | "OR" | "ON"));
    if !finished {
        return None;
    }
    // The table this ON belongs to: the word after the nearest preceding JOIN.
    let join_at = tokens
        .iter()
        .rposition(|t| matches!(t, Tok::Word(w) if w.eq_ignore_ascii_case("JOIN")))?;
    let joined = match tokens.get(join_at + 1)? {
        Tok::Word(name) => name.as_str(),
        _ => return None,
    };
    let joined_table = cx.table(joined)?;
    let correlation = match tokens.get(join_at + 2) {
        Some(Tok::Word(w)) if w.eq_ignore_ascii_case("AS") => match tokens.get(join_at + 3) {
            Some(Tok::Word(alias)) => alias.as_str(),
            _ => joined,
        },
        Some(Tok::Word(w)) if !sqlctx::is_keyword(w) => w.as_str(),
        _ => joined,
    };
    // Chain another table the schema relates to this one, else move on to filtering.
    if let Some(join) = join_from_fk(joined_table, correlation, cx.scope, cx.schema, cx.kind) {
        return Some(format!("{join} "));
    }
    let (alias, table) = cx.main()?;
    let column = cx.filter_column(table, &[])?;
    Some(format!("WHERE {} = ", cx.column_ref(alias, column)))
}

fn order_next(cx: &Ctx, body: &[Tok]) -> Option<String> {
    let (alias, table) = cx.main()?;
    match body.last() {
        None => single_pk(table).map(|pk| format!("{} DESC ", cx.column_ref(alias, pk))),
        Some(Tok::Word(w)) if matches!(w.to_ascii_uppercase().as_str(), "ASC" | "DESC") => {
            Some(format!("LIMIT {DEFAULT_LIMIT}"))
        }
        // A bare column: default to newest/largest first, the usual reason to sort.
        Some(Tok::Word(w))
            if !sqlctx::is_keyword(w)
                && !matches!(body.get(body.len().wrapping_sub(2)), Some(Tok::Comma)) =>
        {
            Some("DESC ".to_string())
        }
        _ => None,
    }
}

fn set_next(cx: &Ctx, body: &[Tok]) -> Option<String> {
    let (alias, table) = cx.main()?;
    if body.is_empty() {
        let column = table
            .columns
            .iter()
            .find(|c| !c.primary_key && !c.generated)?;
        return Some(format!("{} = ", qual(&column.name, cx.kind)));
    }
    if let Some((column, op)) = awaiting_value(body) {
        return learned_value(cx.history, column, op);
    }
    // A finished assignment: the next thing an UPDATE needs is its WHERE.
    if matches!(body, [.., Tok::Word(_), Tok::Op(op), Tok::Str(_) | Tok::Num(_)] if op == "=") {
        let pk = single_pk(table)?;
        return Some(format!("WHERE {} = ", cx.column_ref(alias, pk)));
    }
    None
}

fn select_next(body: &[Tok]) -> Option<String> {
    // A finished select list is always followed by FROM; the popup then offers the tables.
    let finished = match body.last()? {
        Tok::Word(w) => !sqlctx::is_keyword(w) || w.eq_ignore_ascii_case("END"),
        Tok::Close => true,
        Tok::Op(op) => op == "*",
        Tok::Num(_) | Tok::Str(_) => true,
        _ => false,
    };
    (finished && !body.is_empty()).then(|| "FROM ".to_string())
}

/// The literal the history most often compares `column` against with `op`, weighted toward
/// recent use. `None` when the user has never filtered on it.
fn learned_value(history: &[&str], column: &str, op: &str) -> Option<String> {
    let mut scores: Vec<(String, f64)> = Vec::new();
    for (rank, entry) in history.iter().rev().take(HISTORY_WINDOW).enumerate() {
        let weight = 1.0 / (1.0 + rank as f64 * 0.1);
        let chars: Vec<char> = entry.chars().collect();
        for window in tokenize(&chars).windows(3) {
            let [Tok::Word(name), Tok::Op(used), Tok::Str(lit) | Tok::Num(lit)] = window else {
                continue;
            };
            if used != op || !name.eq_ignore_ascii_case(column) {
                continue;
            }
            match scores.iter_mut().find(|(seen, _)| seen == lit) {
                Some((_, score)) => *score += weight,
                None => scores.push((lit.clone(), weight)),
            }
        }
    }
    // Strict `>` keeps the earlier (newer) literal on a tie.
    scores
        .into_iter()
        .reduce(|best, next| if next.1 > best.1 { next } else { best })
        .map(|(literal, _)| literal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::{ColumnInfo, ForeignKeyInfo, IndexInfo};

    fn col(name: &str, pk: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: "int".into(),
            nullable: !pk,
            primary_key: pk,
            default: None,
            check: None,
            comment: None,
            generated: false,
            max_length: None,
        }
    }

    fn schema() -> SchemaTree {
        SchemaTree {
            tables: vec![
                TableInfo {
                    schema: None,
                    name: "users".into(),
                    columns: vec![col("id", true), col("email", false), col("status", false)],
                    indexes: vec![IndexInfo {
                        name: "users_email".into(),
                        unique: true,
                        columns: vec!["email".into()],
                    }],
                    foreign_keys: vec![],
                },
                TableInfo {
                    schema: None,
                    name: "orders".into(),
                    columns: vec![col("id", true), col("user_id", false), col("total", false)],
                    indexes: vec![],
                    foreign_keys: vec![ForeignKeyInfo {
                        name: "fk".into(),
                        columns: vec!["user_id".into()],
                        ref_schema: None,
                        ref_table: "users".into(),
                        ref_columns: vec!["id".into()],
                        on_delete: String::new(),
                        on_update: String::new(),
                    }],
                },
                TableInfo {
                    schema: None,
                    name: "logs".into(),
                    columns: vec![col("id", true), col("msg", false)],
                    indexes: vec![],
                    foreign_keys: vec![],
                },
            ],
            ..SchemaTree::default()
        }
    }

    fn next(sql: &str) -> Option<String> {
        next_with(sql, &[])
    }

    fn next_with(sql: &str, history: &[&str]) -> Option<String> {
        let chars: Vec<char> = sql.chars().collect();
        next_clause(&chars, history, &schema(), None)
    }

    #[test]
    fn where_picks_an_indexed_column_before_the_key() {
        assert_eq!(
            next("SELECT * FROM users WHERE ").as_deref(),
            Some("email = ")
        );
        // No index on logs: fall back to the primary key.
        assert_eq!(next("SELECT * FROM logs WHERE ").as_deref(), Some("id = "));
    }

    #[test]
    fn and_adds_a_column_the_statement_has_not_used() {
        assert_eq!(
            next("SELECT * FROM users WHERE email = 'a' AND ").as_deref(),
            Some("id = ")
        );
    }

    #[test]
    fn columns_are_qualified_only_when_several_tables_are_in_play() {
        assert_eq!(
            next("SELECT * FROM users u JOIN orders o ON o.user_id = u.id WHERE ").as_deref(),
            Some("u.email = ")
        );
    }

    #[test]
    fn value_is_learned_from_history_by_frequency_and_recency() {
        let history = [
            "SELECT * FROM users WHERE status = 'banned'",
            "SELECT * FROM users WHERE status = 'active'",
            "SELECT * FROM users WHERE status = 'active' AND id > 5",
            "SELECT * FROM users WHERE status = 'pending'",
        ];
        assert_eq!(
            next_with("SELECT * FROM users WHERE status = ", &history).as_deref(),
            Some("'active'")
        );
    }

    #[test]
    fn value_needs_the_same_operator_and_column() {
        let history = ["SELECT * FROM users WHERE status = 'active'"];
        assert!(next_with("SELECT * FROM users WHERE status > ", &history).is_none());
        assert!(next_with("SELECT * FROM users WHERE email = ", &history).is_none());
    }

    #[test]
    fn numbers_are_learned_too() {
        let history = ["SELECT * FROM orders WHERE user_id = 42"];
        assert_eq!(
            next_with("SELECT * FROM orders WHERE user_id = ", &history).as_deref(),
            Some("42")
        );
    }

    #[test]
    fn a_finished_filter_leads_to_ordering_then_a_limit() {
        let sql = "SELECT * FROM users WHERE email = 'a' ";
        assert_eq!(next(sql).as_deref(), Some("ORDER BY id DESC "));
        assert_eq!(
            next("SELECT * FROM users WHERE email = 'a' ORDER BY id DESC ").as_deref(),
            Some("LIMIT 100")
        );
        assert_eq!(
            next("SELECT * FROM users WHERE email = 'a' ORDER BY id ").as_deref(),
            Some("DESC ")
        );
        assert!(
            next("SELECT * FROM users WHERE email = 'a' ORDER BY id DESC LIMIT 100 ").is_none()
        );
    }

    #[test]
    fn order_by_alone_suggests_the_key_descending() {
        assert_eq!(
            next("SELECT * FROM orders ORDER BY ").as_deref(),
            Some("id DESC ")
        );
    }

    #[test]
    fn a_table_with_no_key_gets_a_plain_limit() {
        let mut s = schema();
        s.tables[2].columns[0].primary_key = false;
        let chars: Vec<char> = "SELECT * FROM logs WHERE msg = 'x' ".chars().collect();
        assert_eq!(
            next_clause(&chars, &[], &s, None).as_deref(),
            Some("LIMIT 100")
        );
    }

    #[test]
    fn a_finished_join_chains_on_to_the_next_relation_or_a_filter() {
        // orders → users is done; users has no further relation, so move on to WHERE.
        assert_eq!(
            next("SELECT * FROM orders o JOIN users u ON u.id = o.user_id ").as_deref(),
            Some("WHERE o.id = ")
        );
    }

    #[test]
    fn an_unfinished_on_condition_is_left_alone() {
        assert!(next("SELECT * FROM orders o JOIN users u ON u.id = ").is_none());
        assert!(next("SELECT * FROM orders o JOIN users u ON u.id = o.user_id AND ").is_none());
    }

    #[test]
    fn update_walks_set_value_where() {
        assert_eq!(next("UPDATE users SET ").as_deref(), Some("email = "));
        let history = ["UPDATE users SET email = 'x@y.z' WHERE id = 1"];
        assert_eq!(
            next_with("UPDATE users SET email = ", &history).as_deref(),
            Some("'x@y.z'")
        );
        assert_eq!(
            next("UPDATE users SET email = 'a' ").as_deref(),
            Some("WHERE id = ")
        );
        // The WHERE is there now: nothing more to push.
        assert!(next("UPDATE users SET email = 'a' WHERE id = 1 ").is_none());
    }

    #[test]
    fn a_finished_select_list_leads_to_from() {
        assert_eq!(next("SELECT id, email ").as_deref(), Some("FROM "));
        assert_eq!(next("SELECT count(*) ").as_deref(), Some("FROM "));
        assert!(next("SELECT id, ").is_none());
        assert!(next("SELECT DISTINCT ").is_none());
    }

    #[test]
    fn stays_silent_where_it_cannot_read_the_statement() {
        assert!(
            next("SELECT * FROM users WHERE id IN (SELECT user_id FROM orders WHERE ").is_none()
        );
        assert!(next("SELECT 1 UNION SELECT * FROM users WHERE ").is_none());
        assert!(next("INSERT INTO users (email) VALUES (").is_none());
        assert!(next("SELECT * FROM nowhere WHERE ").is_none());
    }

    #[test]
    fn mid_word_is_left_to_the_popup() {
        assert!(next("SELECT * FROM users WHERE em").is_none());
    }

    #[test]
    fn tokenizer_handles_strings_comments_and_quotes() {
        let toks = tokenize(
            &"a = 'it''s' -- x = 1\n AND \"b c\" = 2"
                .chars()
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            toks,
            [
                Tok::Word("a".into()),
                Tok::Op("=".into()),
                Tok::Str("'it''s'".into()),
                Tok::Word("AND".into()),
                Tok::Word("b c".into()),
                Tok::Op("=".into()),
                Tok::Num("2".into()),
            ]
        );
    }

    #[test]
    fn thai_identifiers_and_values_work() {
        let history = ["SELECT * FROM users WHERE status = 'ใช้งาน'"];
        assert_eq!(
            next_with("SELECT * FROM users WHERE status = ", &history).as_deref(),
            Some("'ใช้งาน'")
        );
    }
}
