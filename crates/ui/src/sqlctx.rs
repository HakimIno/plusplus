//! Cursor context shared by the editor's two completion surfaces: the popup
//! ([`crate::autocomplete`]) and the inline ghost text ([`crate::ghost`]).
//!
//! Both need the same three things — where the statement under the caret begins, which
//! words precede the caret, and which tables (and aliases) the statement brought into
//! scope. Keeping one implementation means the two can never disagree about the query
//! they are completing.
//!
//! Everything here is pure and offline: char slices in, plain data out.

use std::ops::Range;

pub fn is_ident_char(c: char) -> bool {
    // `is_alphanumeric` already covers ASCII and the base letters of other scripts, but it
    // excludes combining marks — Thai sara/tone marks (and similar) — which would split a
    // word like `ชื่อ` mid-identifier. Treat any non-ASCII, non-whitespace char as part of
    // the identifier so multi-byte names stay whole for prefix matching and tokenizing.
    c.is_alphanumeric() || c == '_' || (!c.is_ascii() && !c.is_whitespace() && !c.is_control())
}

/// True when `word` is one of the SQL keywords the highlighter knows, so it can never be
/// mistaken for a table name or an alias.
pub fn is_keyword(word: &str) -> bool {
    crate::highlight::KEYWORDS.contains(&word.to_ascii_uppercase().as_str())
}

/// The delimiter that must have opened a quoted identifier ending in `c`: ANSI `"…"`,
/// MySQL `` `…` ``, or SQL Server `[…]`. `None` when `c` closes nothing.
pub fn opening_quote(c: char) -> Option<char> {
    match c {
        '"' => Some('"'),
        '`' => Some('`'),
        ']' => Some('['),
        _ => None,
    }
}

// --- statement boundaries -------------------------------------------------------------

/// Char range of the statement containing `cursor`, delimited by the `;` separators that
/// sit outside strings and comments.
///
/// A caret at the end of `SELECT 1;\nSELECT * FROM us` yields just the second statement,
/// so completion never tries to match against statements the user has already finished.
pub fn statement_range(chars: &[char], cursor: usize) -> Range<usize> {
    let cursor = cursor.min(chars.len());
    let mut start = 0;
    let mut end = chars.len();
    // `separators` is ascending, so the first one at or past the caret ends the statement.
    for pos in separators(chars) {
        if pos < cursor {
            start = pos + 1;
        } else {
            end = pos;
            break;
        }
    }
    start..end
}

/// Positions of the `;` characters that actually separate statements — those outside
/// string literals, quoted identifiers, and comments.
fn separators(chars: &[char]) -> Vec<usize> {
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        match chars[i] {
            '\'' => i = skip_string(chars, i),
            '"' => i = skip_delimited(chars, i, '"'),
            '`' => i = skip_delimited(chars, i, '`'),
            '[' => i = skip_delimited(chars, i, ']'),
            '-' if i + 1 < n && chars[i + 1] == '-' => {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < n && chars[i + 1] == '*' => i = skip_block_comment(chars, i),
            ';' => {
                out.push(i);
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Index just past the single-quoted string opening at `i`, honouring `''` escapes.
fn skip_string(chars: &[char], mut i: usize) -> usize {
    let n = chars.len();
    i += 1; // opening quote
    while i < n {
        if chars[i] == '\'' {
            if i + 1 < n && chars[i + 1] == '\'' {
                i += 2; // doubled quote is an escaped literal, not the end
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    n
}

/// Index just past the run opened at `i` and closed by the next `close`.
fn skip_delimited(chars: &[char], mut i: usize, close: char) -> usize {
    let n = chars.len();
    i += 1;
    while i < n && chars[i] != close {
        i += 1;
    }
    (i + 1).min(n)
}

/// Index just past the `/* … */` comment opening at `i`.
fn skip_block_comment(chars: &[char], mut i: usize) -> usize {
    let n = chars.len();
    i += 2;
    while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
        i += 1;
    }
    (i + 2).min(n)
}

// --- words around the caret -----------------------------------------------------------

/// The identifier ending at `end` (exclusive) and the index it starts at, skipping one run
/// of whitespace before `end` first. `None` when the char before is punctuation (`.`, `(`)
/// or the text runs out.
pub fn word_at(chars: &[char], end: usize) -> Option<(String, usize)> {
    let mut e = end.min(chars.len());
    while e > 0 && chars[e - 1].is_whitespace() {
        e -= 1;
    }
    let mut s = e;
    while s > 0 && is_ident_char(chars[s - 1]) {
        s -= 1;
    }
    (s < e).then(|| (chars[s..e].iter().collect(), s))
}

/// The identifier ending right before `end` (exclusive), e.g. the qualifier before a dot.
///
/// A quoted identifier (`"My Table".`, `` `t`. ``, `[t].`) yields its *inner* text, so a
/// name that had to be quoted still resolves against the schema, whose names are unquoted.
pub fn ident_before(chars: &[char], end: usize) -> Option<String> {
    let end = end.min(chars.len());
    if end == 0 {
        return None;
    }
    if let Some(open) = opening_quote(chars[end - 1]) {
        // Walk back to the opening delimiter. `s` lands just past it.
        let mut s = end - 1;
        while s > 0 && chars[s - 1] != open {
            s -= 1;
        }
        if s == 0 {
            return None; // no opener — an unterminated quote, not an identifier
        }
        return Some(chars[s..end - 1].iter().collect());
    }
    let mut s = end;
    while s > 0 && is_ident_char(chars[s - 1]) {
        s -= 1;
    }
    (s < end).then(|| chars[s..end].iter().collect())
}

/// The previous significant word before `pos`, uppercased — used for context detection.
/// Skips trailing whitespace; stops at punctuation (returning `None` for things like `(`).
pub fn previous_word(chars: &[char], pos: usize) -> Option<String> {
    let mut i = pos;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    let end = i;
    while i > 0 && is_ident_char(chars[i - 1]) {
        i -= 1;
    }
    // A comma keeps the context of the word before the list, TablePlus-style:
    // `SELECT a, b…` is still column context, `FROM t1, t2…` still table context.
    if i == end {
        if i > 0 && chars[i - 1] == ',' {
            return previous_word_skipping_list(chars, i - 1);
        }
        return None;
    }
    Some(chars[i..end].iter().collect::<String>().to_ascii_uppercase())
}

/// Walk back over a comma-separated identifier list to the keyword that opened it.
fn previous_word_skipping_list(chars: &[char], mut pos: usize) -> Option<String> {
    // Bounded walk so pathological input can't loop forever.
    for _ in 0..64 {
        let word_start = {
            let mut i = pos;
            while i > 0 && chars[i - 1].is_whitespace() {
                i -= 1;
            }
            let end = i;
            while i > 0 && (is_ident_char(chars[i - 1]) || chars[i - 1] == '.') {
                i -= 1;
            }
            if i == end {
                return None;
            }
            i
        };
        // The element before this one: another comma continues the list, anything
        // else means this word is preceded by the opening keyword.
        let mut j = word_start;
        while j > 0 && chars[j - 1].is_whitespace() {
            j -= 1;
        }
        if j > 0 && chars[j - 1] == ',' {
            pos = j - 1;
            continue;
        }
        return previous_word(chars, word_start);
    }
    None
}

/// True when `pos` sits inside a string literal or comment (where no completion makes sense).
pub fn in_string_or_comment(chars: &[char], pos: usize) -> bool {
    let mut i = 0;
    while i < pos {
        match chars[i] {
            '\'' => {
                i += 1;
                while i < pos {
                    if chars[i] == '\'' {
                        if i + 1 < chars.len() && chars[i + 1] == '\'' {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                if i >= pos {
                    return true; // ran past the cursor while still inside the string
                }
                i += 1; // closing quote
            }
            '-' if i + 1 < chars.len() && chars[i + 1] == '-' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                if i >= pos {
                    return true;
                }
            }
            '/' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                i += 2;
                while i < chars.len()
                    && !(chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/')
                {
                    i += 1;
                }
                if i >= pos {
                    return true;
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    false
}

// --- tables in scope ------------------------------------------------------------------

/// Scan the SQL for `FROM`/`JOIN`/`UPDATE`/`INTO` targets, returning `(alias, table)`
/// pairs. A table without an alias maps to itself, so the result doubles as the set of
/// referenced tables. Quoted and schema-qualified names keep their last bare segment.
pub fn referenced_tables(chars: &[char]) -> Vec<(String, String)> {
    let words = tokenize_words(chars);
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let upper = words[i].to_ascii_uppercase();
        if matches!(upper.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            if let Some(table) = words.get(i + 1) {
                let table = table.clone();
                let mut alias = table.clone();
                let mut j = i + 2;
                if words.get(j).is_some_and(|w| w.eq_ignore_ascii_case("AS")) {
                    j += 1;
                }
                if let Some(next) = words.get(j) {
                    if !is_keyword(next) {
                        alias = next.clone();
                    }
                }
                out.push((alias, table));
            }
        }
        i += 1;
    }
    out
}

/// Split the SQL into bare words for the table/alias scan: identifiers (dotted chains
/// reduced to their last segment, quotes stripped), skipping strings and comments.
pub fn tokenize_words(chars: &[char]) -> Vec<String> {
    let mut words = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '\'' {
            i = skip_string(chars, i);
        } else if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i = skip_block_comment(chars, i);
        } else if is_ident_char(c) || c == '"' || c == '`' || c == '[' {
            // An identifier chain like `schema.table` or `"My Table"`; keep the last segment.
            let mut last = String::new();
            let mut segment = String::new();
            while i < n {
                match chars[i] {
                    ch if is_ident_char(ch) => {
                        segment.push(ch);
                        i += 1;
                    }
                    '"' | '`' => {
                        let quote = chars[i];
                        i += 1;
                        while i < n && chars[i] != quote {
                            segment.push(chars[i]);
                            i += 1;
                        }
                        i = (i + 1).min(n);
                    }
                    '[' => {
                        i += 1;
                        while i < n && chars[i] != ']' {
                            segment.push(chars[i]);
                            i += 1;
                        }
                        i = (i + 1).min(n);
                    }
                    '.' => {
                        last = std::mem::take(&mut segment);
                        let _ = last; // replaced below if another segment follows
                        i += 1;
                    }
                    _ => break,
                }
            }
            if !segment.is_empty() {
                last = segment;
            }
            if !last.is_empty() {
                words.push(last);
            }
        } else {
            i += 1;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cv(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn statement_range_splits_on_semicolons() {
        let c = cv("SELECT 1;\nSELECT * FROM us");
        let r = statement_range(&c, c.len());
        assert_eq!(c[r].iter().collect::<String>(), "\nSELECT * FROM us");
    }

    #[test]
    fn statement_range_is_whole_buffer_without_separators() {
        let c = cv("SELECT * FROM users");
        assert_eq!(statement_range(&c, c.len()), 0..c.len());
    }

    #[test]
    fn statement_range_ignores_semicolons_in_strings_and_comments() {
        let c = cv("SELECT ';' AS a -- ;\nFROM t");
        assert_eq!(statement_range(&c, c.len()), 0..c.len());
        let c = cv("SELECT /* ; */ 1 FROM t");
        assert_eq!(statement_range(&c, c.len()), 0..c.len());
    }

    #[test]
    fn statement_range_stops_at_the_separator_after_the_caret() {
        let c = cv("SELECT 1; SELECT 2");
        // Caret inside the first statement: the range ends at the `;`, exclusive.
        let r = statement_range(&c, 8);
        assert_eq!(c[r].iter().collect::<String>(), "SELECT 1");
    }

    #[test]
    fn word_at_skips_trailing_whitespace() {
        let c = cv("FROM orders ");
        let (w, s) = word_at(&c, c.len()).unwrap();
        assert_eq!(w, "orders");
        assert_eq!(word_at(&c, s).unwrap().0, "FROM");
    }

    #[test]
    fn word_at_stops_at_punctuation() {
        let c = cv("o.user_id");
        // `user_id` is a word, but nothing identifier-like precedes the dot boundary.
        let (w, s) = word_at(&c, c.len()).unwrap();
        assert_eq!(w, "user_id");
        assert!(word_at(&c, s).is_none());
    }

    #[test]
    fn referenced_tables_binds_aliases() {
        let c = cv("SELECT * FROM orders o JOIN users AS u ON u.id = o.user_id");
        let refs = referenced_tables(&c);
        assert_eq!(
            refs,
            vec![
                ("o".to_string(), "orders".to_string()),
                ("u".to_string(), "users".to_string()),
            ]
        );
    }

    #[test]
    fn referenced_tables_maps_unaliased_table_to_itself() {
        let c = cv("SELECT * FROM orders WHERE id = 1");
        assert_eq!(
            referenced_tables(&c),
            vec![("orders".to_string(), "orders".to_string())]
        );
    }
}
