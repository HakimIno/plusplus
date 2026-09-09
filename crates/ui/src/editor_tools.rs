//! Pure SQL-editor helpers: find/replace, built-in snippets, and small typing assists.

use std::ops::Range;

#[derive(Default)]
pub struct FindState {
    pub open: bool,
    pub focus_pending: bool,
    pub query: String,
    pub replacement: String,
    pub match_case: bool,
    pub current: usize,
}

pub fn matches(text: &str, query: &str, match_case: bool) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let haystack: Vec<char> = text.chars().collect();
    let needle: Vec<char> = query.chars().collect();
    let folded_needle = (!match_case).then(|| query.to_lowercase());
    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= haystack.len() {
        let same = if match_case {
            haystack[at..at + needle.len()] == needle
        } else {
            haystack[at..at + needle.len()]
                .iter()
                .collect::<String>()
                .to_lowercase()
                == *folded_needle.as_ref().expect("created above")
        };
        if same {
            found.push(at..at + needle.len());
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

pub fn replace_range(text: &mut String, range: Range<usize>, replacement: &str) -> usize {
    let start = char_to_byte(text, range.start);
    let end = char_to_byte(text, range.end);
    text.replace_range(start..end, replacement);
    range.start + replacement.chars().count()
}

pub fn replace_all(text: &mut String, query: &str, replacement: &str, match_case: bool) -> usize {
    let found = matches(text, query, match_case);
    for range in found.iter().rev() {
        replace_range(text, range.clone(), replacement);
    }
    found.len()
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

    #[test]
    fn find_and_replace_are_unicode_and_multiline_safe() {
        let mut sql = "SELECT ชื่อ\nFROM users\nSELECT ชื่อ".to_string();
        let found = matches(&sql, "select ชื่อ", false);
        assert_eq!(found.len(), 2);
        assert_eq!(replace_all(&mut sql, "ชื่อ", "name", true), 2);
        assert_eq!(sql, "SELECT name\nFROM users\nSELECT name");
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
