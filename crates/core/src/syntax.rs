//! Dialect-aware syntax checking for the SQL editor. The UI turns a [`SyntaxError`] into
//! the red squiggle under the offending token and the tooltip that explains it, so the
//! parser — and the dialect table it needs — stays here with the rest of the SQL analysis.
//!
//! Only the *first* error is reported: sqlparser stops at the first thing it can't parse,
//! and one squiggle is what an editor wants anyway.

use std::ops::Range;

use sqlparser::dialect::{
    Dialect, DuckDbDialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect,
    SQLiteDialect,
};
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer};

use crate::model::DbKind;

/// Guard against a pathological nesting depth blowing the stack. Matches [`crate::safety`].
const RECURSION_LIMIT: usize = 128;

/// The first syntax error in a SQL buffer: where it is, and what to say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    /// Char range (not bytes) of the offending token in the checked SQL. Always non-empty,
    /// so there is something to underline.
    pub range: Range<usize>,
    /// One-line English explanation, ready to show in a tooltip.
    pub message: String,
}

/// The sqlparser dialect for a backend, or the generic one when no connection is active.
pub(crate) fn dialect_for(kind: Option<DbKind>) -> Box<dyn Dialect> {
    match kind {
        Some(DbKind::Postgres) => Box::new(PostgreSqlDialect {}),
        Some(DbKind::MySql | DbKind::MariaDb) => Box::new(MySqlDialect {}),
        Some(DbKind::SqlServer) => Box::new(MsSqlDialect {}),
        Some(DbKind::Sqlite) => Box::new(SQLiteDialect {}),
        Some(DbKind::DuckDb) => Box::new(DuckDbDialect {}),
        // sqlparser has no CQL dialect. Generic parses the SQL-shaped core of CQL
        // (UPDATE/DELETE/DROP/TRUNCATE/ALTER); CQL-only clauses (USING TTL, ALLOW
        // FILTERING, IF EXISTS updates) fail to parse and fall back to the conservative
        // lexical scan, which can only over-flag, never under-flag.
        Some(DbKind::Cassandra | DbKind::ScyllaDb) => Box::new(GenericDialect {}),
        None => Box::new(GenericDialect {}),
    }
}

/// Parse `sql` for `kind` and report the first syntax error, or `None` when it parses.
///
/// `kind` is `None` when no connection is active — the generic dialect then accepts the
/// broadest syntax, which is the right bias for a check whose only job is to flag typos.
pub fn check_syntax(kind: Option<DbKind>, sql: &str) -> Option<SyntaxError> {
    if sql.trim().is_empty() {
        return None;
    }
    // CQL is not SQL. The generic dialect rejects perfectly valid CQL (ALLOW FILTERING,
    // USING TTL, IF NOT EXISTS on updates), and a red squiggle under correct code is worse
    // than no squiggle at all — so the check simply does not run for those backends.
    if matches!(kind, Some(DbKind::Cassandra | DbKind::ScyllaDb)) {
        return None;
    }

    let dialect = dialect_for(kind);
    let dialect = dialect.as_ref();

    // Tokenize first, for two reasons: a lexical error (unterminated string, stray quote)
    // carries a real location rather than one embedded in a message, and the token list is
    // what turns the parser's line/column into the *span* of the token to underline.
    let tokens = match Tokenizer::new(dialect, sql).tokenize_with_location() {
        Ok(tokens) => tokens,
        Err(err) => {
            let start = char_index(sql, err.location);
            return Some(SyntaxError {
                // The tokenizer stops where the text stopped making sense and cannot say
                // where the token was meant to end, so mark the rest of that line.
                range: non_empty(start..line_end(sql, start), sql),
                message: humanize(&err.message),
            });
        }
    };

    let error = Parser::new(dialect)
        .with_recursion_limit(RECURSION_LIMIT)
        .try_with_sql(sql)
        .and_then(|mut parser| parser.parse_statements())
        .err()?;
    let raw = match &error {
        ParserError::TokenizerError(message) | ParserError::ParserError(message) => message,
        // Nesting deeper than the limit is our guard tripping, not the user's typo.
        ParserError::RecursionLimitExceeded => return None,
    };

    let (message, location) = split_location(raw);
    let range = match location {
        Some(location) => token_range(sql, &tokens, location),
        None => last_token_range(sql, &tokens),
    }?;
    Some(SyntaxError {
        range,
        message: humanize(&message),
    })
}

/// Char index in `sql` of a 1-based tokenizer `(line, column)`. The tokenizer counts
/// columns in `char`s, so this stays in chars too — the UI indexes the buffer the same way.
fn char_index(sql: &str, location: Location) -> usize {
    let mut line = 1;
    let mut column = 1;
    for (i, c) in sql.chars().enumerate() {
        if line == location.line && column == location.column {
            return i;
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    sql.chars().count()
}

/// Char index of the newline ending the line `from` sits on (or the end of the text).
fn line_end(sql: &str, from: usize) -> usize {
    sql.chars()
        .enumerate()
        .skip(from)
        .find(|(_, c)| *c == '\n')
        .map(|(i, _)| i)
        .unwrap_or_else(|| sql.chars().count())
}

/// Widen an empty range to one char so it can be drawn, backing up when it sits at the end.
fn non_empty(range: Range<usize>, sql: &str) -> Range<usize> {
    if !range.is_empty() {
        return range;
    }
    let len = sql.chars().count();
    if range.start < len {
        range.start..range.start + 1
    } else {
        len.saturating_sub(1)..len
    }
}

/// The span of the token starting at `location`, which is where the parser reports the
/// thing it did not expect.
fn token_range(sql: &str, tokens: &[TokenWithSpan], location: Location) -> Option<Range<usize>> {
    let found = tokens
        .iter()
        .find(|t| t.span.start == location && is_significant(t));
    match found {
        Some(token) => {
            let start = char_index(sql, token.span.start);
            let end = char_index(sql, token.span.end);
            Some(non_empty(start..end, sql))
        }
        // No token there means the parser ran off the end of the input ("found: EOF"):
        // point at the last real token instead — where the statement broke off.
        None => last_token_range(sql, tokens),
    }
}

fn last_token_range(sql: &str, tokens: &[TokenWithSpan]) -> Option<Range<usize>> {
    let token = tokens.iter().rev().find(|t| is_significant(t))?;
    let start = char_index(sql, token.span.start);
    let end = char_index(sql, token.span.end);
    Some(non_empty(start..end, sql))
}

fn is_significant(token: &TokenWithSpan) -> bool {
    !matches!(token.token, Token::Whitespace(_) | Token::EOF)
}

/// Split sqlparser's `" at Line: L, Column: C"` suffix off a message, since the squiggle
/// already puts the reader at that spot.
fn split_location(message: &str) -> (String, Option<Location>) {
    const MARKER: &str = " at Line: ";
    let Some(at) = message.rfind(MARKER) else {
        return (message.to_string(), None);
    };
    let mut parts = message[at + MARKER.len()..].split(", Column: ");
    let line = parts.next().and_then(|s| s.trim().parse().ok());
    let column = parts.next().and_then(|s| s.trim().parse().ok());
    match (line, column) {
        (Some(line), Some(column)) => {
            (message[..at].to_string(), Some(Location::new(line, column)))
        }
        _ => (message.to_string(), None),
    }
}

/// sqlparser's wording is already English; this only tidies what reads as jargon in a
/// tooltip and gives the sentence a capital letter.
fn humanize(message: &str) -> String {
    let message = message
        .trim()
        .replace("found: EOF", "found: end of statement");
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Syntax error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text a reported error underlines, so the assertions read like the editor looks.
    fn marked(sql: &str, error: &SyntaxError) -> String {
        sql.chars()
            .skip(error.range.start)
            .take(error.range.end - error.range.start)
            .collect()
    }

    fn check(sql: &str) -> Option<SyntaxError> {
        check_syntax(Some(DbKind::Postgres), sql)
    }

    #[test]
    fn valid_sql_has_no_error() {
        assert!(check("SELECT id, name FROM users WHERE id = 1").is_none());
        assert!(check("SELECT 1; SELECT 2;").is_none());
        // Empty or whitespace-only buffers are "not yet written", not wrong.
        assert!(check("").is_none());
        assert!(check("   \n\t ").is_none());
    }

    #[test]
    fn underlines_the_misspelled_keyword() {
        let sql = "SELCT * FROM users";
        let error = check(sql).unwrap();
        assert_eq!(marked(sql, &error), "SELCT");
        assert!(error.message.contains("Expected"), "{}", error.message);
        // The location is carried by the range, not repeated in the sentence.
        assert!(!error.message.contains("Line:"), "{}", error.message);
    }

    #[test]
    fn underlines_the_unexpected_token_mid_statement() {
        // `ORDER` without its `BY`. The mark lands on the token the parser gave up at —
        // mid-statement, not smeared over the rest of the line.
        let sql = "SELECT * FROM users ORDER id";
        let error = check(sql).unwrap();
        assert_eq!(marked(sql, &error), "ORDER");
    }

    #[test]
    fn locates_an_error_on_a_later_line() {
        let sql = "SELECT *\nFROM users\nWHERE id ,= 1";
        let error = check(sql).unwrap();
        assert_eq!(marked(sql, &error), ",");
    }

    #[test]
    fn multibyte_text_before_the_error_does_not_shift_the_range() {
        // Char indices, not bytes: the Thai literal is 3 bytes per char, so a byte-based
        // range would land far to the left of the token it means to mark.
        let sql = "SELECT 'ลูกค้า' FROM users WHERE id ,= 1";
        let error = check(sql).unwrap();
        assert_eq!(marked(sql, &error), ",");
    }

    #[test]
    fn an_unterminated_string_is_reported_where_it_opens() {
        let sql = "SELECT 'oops FROM users";
        let error = check(sql).unwrap();
        assert!(marked(sql, &error).starts_with('\''), "{error:?}");
        assert!(!error.range.is_empty());
    }

    #[test]
    fn an_incomplete_statement_marks_the_last_token() {
        let sql = "SELECT * FROM";
        let error = check(sql).unwrap();
        assert_eq!(marked(sql, &error), "FROM");
        // "EOF" is parser jargon; the tooltip says it in words.
        assert!(!error.message.contains("EOF"), "{}", error.message);
    }

    #[test]
    fn the_range_always_covers_at_least_one_char() {
        for sql in ["(", "'", "SELECT", ",", "SELECT * FROM users WHERE"] {
            if let Some(error) = check(sql) {
                assert!(!error.range.is_empty(), "empty range for {sql:?}");
                assert!(
                    error.range.end <= sql.chars().count(),
                    "range past the end for {sql:?}"
                );
            }
        }
    }

    #[test]
    fn dialects_disagree_and_the_check_follows_the_connection() {
        // Backtick-quoted identifiers are MySQL's spelling; Postgres has no such syntax.
        let sql = "SELECT `id` FROM `users`";
        assert!(check_syntax(Some(DbKind::MySql), sql).is_none());
        assert!(check_syntax(Some(DbKind::Postgres), sql).is_some());
    }

    #[test]
    fn cql_is_never_checked() {
        // Valid CQL the generic dialect cannot parse must not light up red.
        let sql = "SELECT * FROM users WHERE id = 1 ALLOW FILTERING";
        assert!(check_syntax(Some(DbKind::Cassandra), sql).is_none());
        assert!(check_syntax(Some(DbKind::ScyllaDb), sql).is_none());
    }

    #[test]
    fn works_without_a_connection() {
        assert!(check_syntax(None, "SELECT 1").is_none());
        assert!(check_syntax(None, "SELCT 1").is_some());
    }
}
