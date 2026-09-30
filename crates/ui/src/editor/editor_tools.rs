//! Pure SQL-editor helpers: find/replace, built-in snippets, and small typing assists.

use std::ops::Range;

/// How the find widget matches: the three toggles inside its query field.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct FindOptions {
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// Most matches the find widget collects (and highlights). Replace All is not capped.
pub const MAX_MATCHES: usize = 10_000;

/// State of the editor's find/replace widget. Match ranges are character offsets into the
/// SQL, so Thai and other multi-byte text select and replace correctly.
#[derive(Default)]
pub struct FindState {
    pub open: bool,
    pub focus_pending: bool,
    /// The replace row is shown.
    pub replace_open: bool,
    pub query: String,
    pub replacement: String,
    pub options: FindOptions,
    /// Index of the highlighted match in [`Self::found`].
    pub current: usize,
    /// Scroll the editor to the current match on its next paint.
    pub reveal: bool,
    /// One of the widget's fields had focus at the end of the last frame. egui drops focus
    /// as a frame with Escape begins, so this is how Escape knows it was meant for us.
    pub had_focus: bool,
    cache: Option<FindCache>,
}

struct FindCache {
    revision: u64,
    len: usize,
    query: String,
    options: FindOptions,
    result: Result<Vec<Range<usize>>, String>,
}

impl FindState {
    /// Recompute the matches if the text (by revision and length), query or options changed
    /// since the last call — otherwise this is free, so it can run every frame.
    pub fn refresh(&mut self, text: &str, revision: u64) {
        let fresh = self.cache.as_ref().is_some_and(|c| {
            c.revision == revision
                && c.len == text.len()
                && c.query == self.query
                && c.options == self.options
        });
        if !fresh {
            self.cache = Some(FindCache {
                revision,
                len: text.len(),
                query: self.query.clone(),
                options: self.options,
                result: find_matches(text, &self.query, self.options, MAX_MATCHES),
            });
        }
    }

    /// Matches from the last [`Self::refresh`], in document order.
    pub fn found(&self) -> &[Range<usize>] {
        match self.cache.as_ref().map(|c| &c.result) {
            Some(Ok(found)) => found,
            _ => &[],
        }
    }

    /// Why the query can't be searched (an invalid regular expression).
    pub fn error(&self) -> Option<&str> {
        match self.cache.as_ref().map(|c| &c.result) {
            Some(Err(error)) => Some(error),
            _ => None,
        }
    }

    /// Index of the first match starting at or after char offset `caret`, wrapping to 0.
    pub fn first_from(&self, caret: usize) -> usize {
        let found = self.found();
        let at = found.partition_point(|r| r.start < caret);
        if at < found.len() {
            at
        } else {
            0
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every match of `query` in `text` as char ranges, up to `limit`. Literal matching folds case
/// char-by-char (so offsets stay aligned with the text); regex mode uses the `regex` crate.
/// `Err` carries a short message for an invalid regular expression.
pub fn find_matches(
    text: &str,
    query: &str,
    options: FindOptions,
    limit: usize,
) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if options.regex {
        let re = build_regex(query, options)?;
        let mut found = Vec::new();
        let (mut byte_at, mut char_at) = (0, 0);
        for m in re.find_iter(text) {
            if m.start() == m.end() {
                continue;
            }
            char_at += text[byte_at..m.start()].chars().count();
            let len = m.as_str().chars().count();
            found.push(char_at..char_at + len);
            char_at += len;
            byte_at = m.end();
            if found.len() >= limit {
                break;
            }
        }
        return Ok(found);
    }
    let fold = |c: char| {
        if options.match_case {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let hay: Vec<char> = text.chars().map(fold).collect();
    let needle: Vec<char> = query.chars().map(fold).collect();
    let n = needle.len();
    let bounded = |start: usize, end: usize| {
        (start == 0 || !is_word_char(hay[start - 1]))
            && (end == hay.len() || !is_word_char(hay[end]))
    };
    let mut found = Vec::new();
    let mut at = 0;
    while at + n <= hay.len() && found.len() < limit {
        if hay[at..at + n] == needle[..] && (!options.whole_word || bounded(at, at + n)) {
            found.push(at..at + n);
            at += n;
        } else {
            at += 1;
        }
    }
    Ok(found)
}

fn build_regex(query: &str, options: FindOptions) -> Result<regex::Regex, String> {
    let pattern = if options.whole_word {
        format!(r"\b(?:{query})\b")
    } else {
        query.to_string()
    };
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!options.match_case)
        .multi_line(true)
        // Bounds compile time/memory for pathological patterns typed into the field.
        .size_limit(1 << 20)
        .build()
        .map_err(|_| "Invalid regular expression".to_string())
}

/// The text that replaces the match at char `range` of `text`: `replacement` as-is, or in
/// regex mode with `$1` / `${name}` expanded from that match's captures.
pub fn replacement_for(
    text: &str,
    range: Range<usize>,
    query: &str,
    replacement: &str,
    options: FindOptions,
) -> String {
    if !options.regex {
        return replacement.to_string();
    }
    match build_regex(query, options) {
        Ok(re) => expand_at(&re, text, char_to_byte(text, range.start), replacement),
        Err(_) => replacement.to_string(),
    }
}

fn expand_at(re: &regex::Regex, text: &str, byte_start: usize, replacement: &str) -> String {
    match re.captures_at(text, byte_start) {
        Some(caps) if caps.get(0).is_some_and(|m| m.start() == byte_start) => {
            let mut out = String::new();
            caps.expand(replacement, &mut out);
            out
        }
        _ => replacement.to_string(),
    }
}

pub fn replace_range(text: &mut String, range: Range<usize>, replacement: &str) -> usize {
    let start = char_to_byte(text, range.start);
    let end = char_to_byte(text, range.end);
    text.replace_range(start..end, replacement);
    range.start + replacement.chars().count()
}

/// Replace every match (no cap) in one pass over the text. Returns how many were replaced.
pub fn replace_all(
    text: &mut String,
    query: &str,
    replacement: &str,
    options: FindOptions,
) -> Result<usize, String> {
    let found = find_matches(text, query, options, usize::MAX)?;
    if found.is_empty() {
        return Ok(0);
    }
    let re = if options.regex {
        Some(build_regex(query, options)?)
    } else {
        None
    };
    // Byte offset of every char boundary, so each match maps to bytes in O(1).
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for range in &found {
        let (start, end) = (offsets[range.start], offsets[range.end]);
        out.push_str(&text[last..start]);
        match &re {
            Some(re) => out.push_str(&expand_at(re, text, start, replacement)),
            None => out.push_str(replacement),
        }
        last = end;
    }
    out.push_str(&text[last..]);
    *text = out;
    Ok(found.len())
}

#[derive(Clone, Copy)]
pub struct Snippet {
    pub trigger: &'static str,
    pub label: &'static str,
    pub body: &'static str,
}

pub const SNIPPETS: &[Snippet] = &[
    Snippet {
        trigger: "sel",
        label: "SELECT from table",
        body: "SELECT ${1:*}\nFROM ${2:table}\nWHERE ${3:condition};",
    },
    Snippet {
        trigger: "ins",
        label: "INSERT row",
        body: "INSERT INTO ${1:table} (${2:columns})\nVALUES (${3:values});",
    },
    Snippet {
        trigger: "upd",
        label: "UPDATE rows",
        body: "UPDATE ${1:table}\nSET ${2:column = value}\nWHERE ${3:condition};",
    },
    Snippet {
        trigger: "del",
        label: "DELETE rows",
        body: "DELETE FROM ${1:table}\nWHERE ${2:condition};",
    },
    Snippet {
        trigger: "cte",
        label: "Common table expression",
        body: "WITH ${1:name} AS (\n  ${2:SELECT * FROM table}\n)\nSELECT *\nFROM ${1:name};",
    },
];

/// Expand `${n:default}` placeholders and return their ranges in tab order.
pub fn expand_snippet(body: &str) -> (String, Vec<Range<usize>>) {
    let chars: Vec<char> = body.chars().collect();
    let mut output = String::new();
    let mut placeholders: Vec<(usize, Range<usize>)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars.get(i) == Some(&'$') && chars.get(i + 1) == Some(&'{') {
            let mut end = i + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end < chars.len() {
                let inner: String = chars[i + 2..end].iter().collect();
                if let Some((number, default)) = inner.split_once(':') {
                    if let Ok(order) = number.parse::<usize>() {
                        let start = output.chars().count();
                        output.push_str(default);
                        placeholders.push((order, start..start + default.chars().count()));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        output.push(chars[i]);
        i += 1;
    }
    placeholders.sort_by_key(|(order, _)| *order);
    (
        output,
        placeholders.into_iter().map(|(_, range)| range).collect(),
    )
}

pub fn indentation_after_newline(text: &str, caret: usize) -> String {
    let mut before: String = text.chars().take(caret).collect();
    if before.ends_with('\n') {
        before.pop();
    }
    let line = before
        .rsplit_once('\n')
        .map_or(before.as_str(), |(_, line)| line);
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    let extra = line.trim_end().ends_with('(')
        || line.trim_end().to_ascii_uppercase().ends_with(" THEN")
        || line.trim_end().to_ascii_uppercase().ends_with(" AS");
    format!("{indent}{}", if extra { "  " } else { "" })
}

/// Find the bracket at/just before the caret and its balanced partner.
pub fn matching_bracket(text: &str, caret: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let (at, bracket) = caret
        .checked_sub(1)
        .and_then(|at| chars.get(at).copied().map(|bracket| (at, bracket)))
        .filter(|(_, bracket)| "()[]{}".contains(*bracket))
        .or_else(|| chars.get(caret).copied().map(|bracket| (caret, bracket)))?;
    let (open, close, direction) = match bracket {
        '(' => ('(', ')', 1isize),
        '[' => ('[', ']', 1),
        '{' => ('{', '}', 1),
        ')' => ('(', ')', -1),
        ']' => ('[', ']', -1),
        '}' => ('{', '}', -1),
        _ => return None,
    };
    let mut depth = 0isize;
    let mut i = at as isize;
    while i >= 0 && (i as usize) < chars.len() {
        let character = chars[i as usize];
        if character == open {
            depth += direction;
        } else if character == close {
            depth -= direction;
        }
        if i != at as isize && depth == 0 {
            return Some((at, i as usize));
        }
        i += direction;
    }
    None
}

fn char_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(match_case: bool, whole_word: bool, regex: bool) -> FindOptions {
        FindOptions {
            match_case,
            whole_word,
            regex,
        }
    }

    #[test]
    fn find_and_replace_are_unicode_and_multiline_safe() {
        let mut sql = "SELECT ชื่อ\nFROM users\nSELECT ชื่อ".to_string();
        let found = find_matches(&sql, "select ชื่อ", FindOptions::default(), MAX_MATCHES);
        assert_eq!(found, Ok(vec![0..11, 23..34]), "char offsets, case-folded");
        let n = replace_all(&mut sql, "ชื่อ", "name", opts(true, false, false));
        assert_eq!(n, Ok(2));
        assert_eq!(sql, "SELECT name\nFROM users\nSELECT name");
    }

    #[test]
    fn whole_word_and_case_toggles() {
        let sql = "id, user_id, ID, idx";
        let plain = find_matches(sql, "id", FindOptions::default(), MAX_MATCHES).unwrap();
        assert_eq!(plain.len(), 4);
        let words = find_matches(sql, "id", opts(false, true, false), MAX_MATCHES).unwrap();
        assert_eq!(words, vec![0..2, 13..15], "not user_id or idx");
        let exact = find_matches(sql, "id", opts(true, true, false), MAX_MATCHES).unwrap();
        assert_eq!(exact, vec![0..2]);
    }

    #[test]
    fn regex_mode_matches_expands_captures_and_reports_bad_patterns() {
        let mut sql = "WHERE a = 1 AND bb = 22".to_string();
        let re = opts(false, false, true);
        let found = find_matches(&sql, r"(\w+) = (\d+)", re, MAX_MATCHES).unwrap();
        assert_eq!(found, vec![6..11, 16..23]);
        assert_eq!(
            replacement_for(&sql, found[1].clone(), r"(\w+) = (\d+)", "$2 = $1", re),
            "22 = bb"
        );
        assert_eq!(
            replace_all(&mut sql, r"(\w+) = (\d+)", "$1 IS $2", re),
            Ok(2)
        );
        assert_eq!(sql, "WHERE a IS 1 AND bb IS 22");
        assert!(find_matches(&sql, "(", re, MAX_MATCHES).is_err());
        // Whole word wraps the pattern: `a` alone, not the `a` inside `AND`.
        let words = find_matches(&sql, "a", opts(false, true, true), MAX_MATCHES).unwrap();
        assert_eq!(words, vec![6..7]);
    }

    #[test]
    fn find_state_caches_until_text_or_query_change() {
        let mut find = FindState {
            query: "a".into(),
            ..FindState::default()
        };
        find.refresh("a b a", 1);
        assert_eq!(find.found(), &[0..1, 4..5]);
        assert_eq!(find.first_from(2), 1, "first match at/after the caret");
        assert_eq!(find.first_from(5), 0, "wraps");
        find.refresh("a", 2);
        assert_eq!(
            find.found().to_vec(),
            vec![0..1],
            "new revision re-searches"
        );
        find.options.regex = true;
        find.query = "[".into();
        find.refresh("a", 2);
        assert!(find.error().is_some() && find.found().is_empty());
    }

    #[test]
    fn match_count_is_capped_but_replace_all_is_not() {
        let mut text = "x".repeat(MAX_MATCHES + 5);
        let found = find_matches(&text, "x", FindOptions::default(), MAX_MATCHES).unwrap();
        assert_eq!(found.len(), MAX_MATCHES);
        let n = replace_all(&mut text, "x", "y", FindOptions::default()).unwrap();
        assert_eq!(n, MAX_MATCHES + 5);
    }

    #[test]
    fn snippet_placeholders_follow_numeric_order() {
        let (sql, ranges) = expand_snippet("SELECT ${2:*} FROM ${1:table}");
        assert_eq!(sql, "SELECT * FROM table");
        assert_eq!(&sql[ranges[0].clone()], "table");
        assert_eq!(&sql[ranges[1].clone()], "*");
    }

    #[test]
    fn newline_keeps_indent_and_indents_open_blocks() {
        assert_eq!(indentation_after_newline("  SELECT (", 10), "    ");
        assert_eq!(indentation_after_newline("  value", 7), "  ");
    }

    #[test]
    fn bracket_matching_respects_nesting_in_both_directions() {
        assert_eq!(matching_bracket("fn(a[0])", 3), Some((2, 7)));
        assert_eq!(matching_bracket("fn(a[0])", 8), Some((7, 2)));
    }
}
