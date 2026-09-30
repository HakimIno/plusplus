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

/// Dialects tried when no connection says which one applies.
const UNCONNECTED_DIALECTS: [DbKind; 5] = [
    DbKind::SqlServer,
    DbKind::Postgres,
    DbKind::MySql,
    DbKind::Sqlite,
    DbKind::DuckDb,
];

/// Parse `sql` for `kind` and report the first syntax error, or `None` when it parses.
///
/// `kind` is `None` when no connection is active. The SQL could then be any dialect, and a
/// red squiggle under correct code is worse than none, so an error is reported only when
/// *no* dialect accepts the text — what's left is a genuine typo.
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
    if kind.is_some() {
        return check_kind(kind, sql);
    }
    let error = check_kind(None, sql)?;
    if UNCONNECTED_DIALECTS
        .iter()
        .any(|&dialect| check_kind(Some(dialect), sql).is_none())
    {
        return None;
    }
    Some(error)
}

/// [`check_syntax`] against exactly one dialect (`None` = generic).
fn check_kind(kind: Option<DbKind>, sql: &str) -> Option<SyntaxError> {
    // SQL Server scripts separate batches with `GO` lines, which only client tools know.
    let batches;
    let sql = if kind == Some(DbKind::SqlServer) {
        batches = go_lines_as_separators(sql);
        batches.as_str()
    } else {
        sql
    };
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

    // sqlparser 0.62 parses each comma-separated ALTER TABLE item as a complete
    // operation. SQL Server instead permits one ADD followed by several column
    // definitions (`ADD a INT, b DATETIME`), so retry that spelling with the
    // implicit ADDs made explicit before showing a false-positive squiggle.
    if matches!(kind, Some(DbKind::SqlServer)) {
        if let Some(rewritten) = mssql_explicit_adds(sql, &tokens) {
            if Parser::new(dialect)
                .with_recursion_limit(RECURSION_LIMIT)
                .try_with_sql(&rewritten)
                .and_then(|mut parser| parser.parse_statements())
                .is_ok()
            {
                return None;
            }
        }
    }
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
    match parser_blind_spot(kind, sql, &tokens, range.start) {
        BlindSpot::None => {}
        BlindSpot::Procedural => return None,
        // Skip just that statement: blank it out (keeping every char position) and check
        // what follows its `;`, so a typo further down is still found.
        BlindSpot::Statement => {
            let end = tokens
                .iter()
                .filter(|t| t.token == Token::SemiColon)
                .map(|t| char_index(sql, t.span.start))
                .find(|&at| at >= range.start)?;
            let rest: String = sql
                .chars()
                .enumerate()
                .map(|(i, c)| if i <= end && c != '\n' { ' ' } else { c })
                .collect();
            return check_kind(kind, &rest);
        }
    }
    Some(SyntaxError {
        range,
        message: humanize(&message),
    })
}

/// Turn each SQL Server `GO` batch-separator line (`GO`, or `GO 5` to repeat) into a `;`,
/// padded with spaces to the same length so reported char ranges still index the original.
fn go_lines_as_separators(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    for line in sql.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim();
        // `get` (not `[..2]`): a line starting with a multi-byte char (Thai, CJK) would
        // otherwise slice inside it and panic.
        let is_go = trimmed
            .get(..2)
            .is_some_and(|head| head.eq_ignore_ascii_case("GO"))
            && trimmed[2..]
                .trim_start()
                .chars()
                .all(|c| c.is_ascii_digit())
            && (trimmed.len() == 2 || trimmed[2..].starts_with(char::is_whitespace));
        if is_go {
            let lead = body.len() - body.trim_start().len();
            out.push_str(&body[..lead]);
            out.push(';');
            out.extend(std::iter::repeat_n(' ', body.chars().count() - lead - 1));
            out.push_str(&line[body.len()..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Rewrite SQL Server's `ALTER TABLE t ADD a INT, b INT` into the equivalent shape
/// understood by sqlparser: `ALTER TABLE t ADD a INT, ADD b INT`.
fn mssql_explicit_adds(sql: &str, tokens: &[TokenWithSpan]) -> Option<String> {
    let mut saw_alter = false;
    let mut saw_table = false;
    let mut in_add = false;
    let mut depth = 0usize;
    let mut insertions = Vec::new();

    for token in tokens.iter().filter(|token| is_significant(token)) {
        match token.token {
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            Token::SemiColon if depth == 0 => {
                saw_alter = false;
                saw_table = false;
                in_add = false;
            }
            Token::Comma if depth == 0 && in_add => {
                insertions.push(char_index(sql, token.span.end));
            }
            _ if depth == 0 => {
                let word = token.token.to_string();
                if !saw_alter {
                    saw_alter = word.eq_ignore_ascii_case("ALTER");
                } else if !saw_table {
                    saw_table = word.eq_ignore_ascii_case("TABLE");
                    if !saw_table {
                        saw_alter = word.eq_ignore_ascii_case("ALTER");
                    }
                } else if word.eq_ignore_ascii_case("ADD") {
                    in_add = true;
                }
            }
            _ => {}
        }
    }

    if insertions.is_empty() {
        return None;
    }

    let mut rewritten = sql.to_string();
    for index in insertions.into_iter().rev() {
        let byte_index = rewritten
            .char_indices()
            .nth(index)
            .map(|(index, _)| index)
            .unwrap_or(rewritten.len());
        rewritten.insert_str(byte_index, " ADD");
    }
    Some(rewritten)
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

/// Statements sqlparser can't parse even though the database accepts them: procedural code
/// (procedure / function / trigger bodies, T-SQL control flow, MySQL `DELIMITER` scripts,
/// Postgres `DO` blocks), maintenance commands, and a few dialect clauses. An error inside
/// one says nothing about the user's SQL, so it's not shown — staying quiet on valid code
/// matters more than catching a typo inside a stored procedure.
fn parser_blind_spot(
    kind: Option<DbKind>,
    sql: &str,
    tokens: &[TokenWithSpan],
    error_at: usize,
) -> BlindSpot {
    // Upper-cased words (and the separators that matter) up to the error, with positions.
    let words: Vec<(usize, String)> = tokens
        .iter()
        .filter(|t| is_significant(t))
        .map(|t| {
            let text = match &t.token {
                Token::Word(w) => w.value.to_ascii_uppercase(),
                Token::SemiColon => ";".into(),
                Token::LParen => "(".into(),
                _ => String::new(),
            };
            (char_index(sql, t.span.start), text)
        })
        .take_while(|(at, _)| *at <= error_at)
        .collect();
    let word = |i: usize| words.get(i).map_or("", |(_, w)| w.as_str());

    // Procedural code anywhere before the error: its body can hold statements of any
    // shape, and `;` inside it doesn't end the outer statement.
    for i in 0..words.len() {
        let routine = |w: &str| matches!(w, "PROCEDURE" | "PROC" | "FUNCTION" | "TRIGGER");
        let procedural = match word(i) {
            "CREATE" => {
                routine(word(i + 1))
                    || (word(i + 1) == "OR"
                        && matches!(word(i + 2), "REPLACE" | "ALTER")
                        && routine(word(i + 3)))
            }
            "ALTER" => routine(word(i + 1)),
            "BEGIN" => matches!(word(i + 1), "TRY" | "CATCH"),
            "WHILE" | "DELIMITER" => true,
            _ => false,
        };
        if procedural {
            return BlindSpot::Procedural;
        }
    }

    // The statement holding the error: from just after the last `;` before it.
    let start = words
        .iter()
        .rposition(|(_, w)| w == ";")
        .map_or(0, |i| i + 1);
    let statement = &words[start.min(words.len())..];
    let leading = statement.first().map_or("", |(_, w)| w.as_str());
    if matches!(
        leading,
        "DO" | "VACUUM"
            | "ANALYZE"
            | "PRAGMA"
            | "OPTIMIZE"
            | "CHECKPOINT"
            | "DBCC"
            | "BACKUP"
            | "RESTORE"
            | "RECONFIGURE"
    ) {
        return BlindSpot::Statement;
    }
    if kind == Some(DbKind::SqlServer) {
        let has = |a: &str, b: &str| statement.windows(2).any(|p| p[0].1 == a && p[1].1 == b);
        // `ALTER COLUMN c <type>` and `OPTION (…)` query hints.
        if has("ALTER", "COLUMN") || has("OPTION", "(") {
            return BlindSpot::Statement;
        }
    }
    BlindSpot::None
}

/// What [`parser_blind_spot`] found at a parse error.
enum BlindSpot {
    /// An ordinary error: report it.
    None,
    /// One statement the parser can't read: skip it and keep checking after its `;`.
    Statement,
    /// Procedural code whose body hides statement boundaries: stop checking.
    Procedural,
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
    #[test]
    fn go_separator_scan_survives_lines_starting_with_multibyte_text() {
        let sql = "ฆ\nกข ค\nGO\nSELECT 1";
        assert_eq!(
            super::go_lines_as_separators(sql).chars().count(),
            sql.chars().count()
        );
    }

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
    fn sql_server_accepts_multiple_columns_after_one_add() {
        let sql = "ALTER TABLE hr_ms_training_tutor\n\
                   ADD record_id SMALLINT NULL,\n\
                       record_date DATETIME NULL;";
        assert!(check_syntax(Some(DbKind::SqlServer), sql).is_none());
    }

    #[test]
    fn sql_server_multi_column_add_still_reports_real_errors() {
        let sql = "ALTER TABLE users ADD first_name VARCHAR(50), second_name VARCHAR(";
        assert!(check_syntax(Some(DbKind::SqlServer), sql).is_some());
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

    /// With no connection the SQL could be any dialect: SQL Server brackets, variables and
    /// IF, MySQL backticks all pass; only text no dialect accepts is flagged.
    #[test]
    fn without_a_connection_any_dialect_that_parses_wins() {
        for sql in [
            "SELECT TOP 100 * FROM [dbo].[ac_ms_account_group1];",
            "DECLARE @id INT = 5;\nSELECT * FROM t WHERE id = @id;",
            "IF OBJECT_ID('t') IS NOT NULL DROP TABLE t;",
            "SELECT `id` FROM `users`",
        ] {
            assert!(check_syntax(None, sql).is_none(), "{sql}");
        }
        let error = check_syntax(None, "SELECT * FORM t").expect("a typo in every dialect");
        assert_eq!(marked("SELECT * FORM t", &error), "FORM");
    }

    /// Valid SQL sqlparser can't parse must not light up red, per dialect.
    #[test]
    fn valid_sql_outside_the_parser_is_not_flagged() {
        use DbKind::*;
        let cases: &[(DbKind, &str)] = &[
            (Postgres, "DO $$ BEGIN RAISE NOTICE 'hi'; END $$;"),
            (Postgres, "VACUUM ANALYZE t"),
            (Postgres, "SELECT 1; VACUUM t"),
            (
                MySql,
                "DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1; END //\nDELIMITER ;",
            ),
            (
                SqlServer,
                "CREATE PROCEDURE p @a INT AS BEGIN SELECT @a END",
            ),
            (
                SqlServer,
                "CREATE OR ALTER PROC p AS SET NOCOUNT ON; SELECT 1;",
            ),
            (
                SqlServer,
                "WITH c AS (SELECT 1 AS x) SELECT * FROM c OPTION (MAXRECURSION 0)",
            ),
            (
                SqlServer,
                "BEGIN TRY SELECT 1 END TRY BEGIN CATCH SELECT ERROR_MESSAGE() END CATCH",
            ),
            (SqlServer, "WHILE @i < 10 BEGIN SET @i = @i + 1 END"),
            (
                SqlServer,
                "ALTER TABLE t ALTER COLUMN a NVARCHAR(50) NOT NULL",
            ),
            (Sqlite, "PRAGMA table_info(t)"),
        ];
        for (kind, sql) in cases {
            assert!(check_syntax(Some(*kind), sql).is_none(), "{kind:?}: {sql}");
        }
    }

    /// The blind spots are scoped: a typo in an ordinary statement before or after them is
    /// still reported.
    #[test]
    fn typos_around_blind_spots_are_still_reported() {
        let sql = "SELEC 1;\nVACUUM t";
        let error = check_syntax(Some(DbKind::Postgres), sql).expect("typo before VACUUM");
        assert_eq!(marked(sql, &error), "SELEC");
        let sql = "VACUUM t;\nSELECT * FORM t";
        assert!(check_syntax(Some(DbKind::Postgres), sql).is_some());
        let sql = "ALTER TABLE t ALTER COLUMN a INT; SELECT * FORM t";
        let error = check_syntax(Some(DbKind::SqlServer), sql).expect("typo after");
        assert_eq!(marked(sql, &error), "FORM");
    }

    #[test]
    fn sql_server_go_lines_separate_batches() {
        let sql = "SELECT * FROM [dbo].[a]\nGO\n  go 3\nSELECT 1\nGO";
        assert!(check_syntax(Some(DbKind::SqlServer), sql).is_none());
        // A real error after a GO is still found, at its original position.
        let sql = "SELECT 1\nGO\nSELEC 2";
        let error = check_syntax(Some(DbKind::SqlServer), sql).expect("typo");
        assert_eq!(marked(sql, &error), "SELEC");
        // `GO` inside a statement is not a separator.
        assert!(check_syntax(Some(DbKind::SqlServer), "SELECT go FROM t").is_none());
    }
}
