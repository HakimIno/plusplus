//! A lightweight SQL syntax highlighter used by the query editor. It produces an egui
//! `LayoutJob`, colouring keywords, strings, numbers, comments, and punctuation. It is
//! deliberately simple (no external parser) — enough to give the editor a polished,
//! TablePlus-like look.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) struct SqlColors {
    keyword: Color32,
    string: Color32,
    number: Color32,
    comment: Color32,
    punct: Color32,
    ident: Color32,
    qualified_ident: Color32,
}

pub(crate) fn sql_colors() -> SqlColors {
    let t = crate::theme::current();
    if let Some(syntax) = t.syntax {
        return SqlColors {
            keyword: syntax.keyword,
            string: syntax.string,
            number: syntax.number,
            comment: syntax.comment,
            punct: syntax.punctuation,
            ident: syntax.identifier,
            qualified_ident: syntax.qualified_identifier,
        };
    }
    SqlColors {
        keyword: t.accent,
        string: mix(t.danger, t.warning, 0.4),
        number: t.warning,
        comment: t.success,
        punct: mix(t.accent, t.text_weak, if t.is_dark { 0.62 } else { 0.45 }),
        ident: t.text,
        qualified_ident: t.text,
    }
}

use crate::style::mix;

/// Highlight `text`, rendering the char ranges in `placeholders` as inert markers rather than
/// as SQL. The editor uses this for the `⋯ N lines` stand-ins that [`crate::fold`] splices in
/// for collapsed regions: they are not part of the query, so they must not read like code.
///
/// `placeholders` must be sorted and non-overlapping, which is how a folded view builds them.
pub fn highlight_sql_folded(
    text: &str,
    font: FontId,
    placeholders: &[std::ops::Range<usize>],
) -> LayoutJob {
    highlight_runs(text, font, placeholders)
}

#[derive(Default)]
struct Highlighter;

impl egui::cache::ComputerMut<(&str, &FontId, &[std::ops::Range<usize>], SqlColors), LayoutJob>
    for Highlighter
{
    fn compute(
        &mut self,
        (text, font, placeholders, colors): (&str, &FontId, &[std::ops::Range<usize>], SqlColors),
    ) -> LayoutJob {
        highlight_runs_with_colors(text, font.clone(), placeholders, colors)
    }
}

type HighlightCache = egui::cache::FrameCache<LayoutJob, Highlighter>;

/// Memoized variant for read-only SQL labels (history, favorites, previews). Entries survive
/// only while used on consecutive frames, so closed panels release their text automatically.
pub fn highlight_sql_cached(ctx: &egui::Context, text: &str, font: FontId) -> LayoutJob {
    let colors = sql_colors();
    ctx.memory_mut(|memory| {
        memory
            .caches
            .cache::<HighlightCache>()
            .get((text, &font, &[], colors))
            .clone()
    })
}

fn highlight_runs(text: &str, font: FontId, placeholders: &[std::ops::Range<usize>]) -> LayoutJob {
    highlight_runs_with_colors(text, font, placeholders, sql_colors())
}

fn highlight_runs_with_colors(
    text: &str,
    font: FontId,
    placeholders: &[std::ops::Range<usize>],
    colors: SqlColors,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    if text.is_empty() {
        // An empty job has no section to take a font from, so its one empty row — and the
        // caret drawn in it — would fall back to the default size and ignore the editor's.
        job.append(
            "",
            0.0,
            egui::TextFormat {
                font_id: font,
                ..Default::default()
            },
        );
        return job;
    }
    if placeholders.is_empty() {
        append_sql(&mut job, text, &font, &colors);
        return job;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0usize;
    for range in placeholders {
        let start = range.start.min(chars.len());
        let end = range.end.min(chars.len());
        if at < start {
            append_sql(
                &mut job,
                &chars[at..start].iter().collect::<String>(),
                &font,
                &colors,
            );
        }
        job.append(
            &chars[start..end].iter().collect::<String>(),
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: colors.comment,
                italics: true,
                ..Default::default()
            },
        );
        at = end;
    }
    if at < chars.len() {
        append_sql(
            &mut job,
            &chars[at..].iter().collect::<String>(),
            &font,
            &colors,
        );
    }
    job
}

/// Lex `text` as SQL and append its coloured runs to `job`.
fn append_sql(job: &mut LayoutJob, text: &str, font: &FontId, colors: &SqlColors) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    let push = |job: &mut LayoutJob, s: &str, color: Color32| {
        job.append(
            s,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };

    while i < n {
        let c = chars[i];

        // Line comment: -- … end of line
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            let start = i;
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            push(
                job,
                &chars[start..i].iter().collect::<String>(),
                colors.comment,
            );
            continue;
        }

        // Block comment: /* … */
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            while i < n && !(chars[i] == '*' && i + 1 < n && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(n);
            push(
                job,
                &chars[start..i].iter().collect::<String>(),
                colors.comment,
            );
            continue;
        }

        // String literal: '…' with '' as an escaped quote.
        if c == '\'' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\'' {
                    if i + 1 < n && chars[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            push(
                job,
                &chars[start..i].iter().collect::<String>(),
                colors.string,
            );
            continue;
        }

        // Identifier / keyword.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let color = if is_keyword(&word) {
                colors.keyword
            } else if colors.qualified_ident != colors.ident
                && chars[..start].iter().rev().find(|c| !c.is_whitespace()) == Some(&'.')
            {
                colors.qualified_ident
            } else {
                colors.ident
            };
            push(job, &word, color);
            continue;
        }

        // Number.
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            push(
                job,
                &chars[start..i].iter().collect::<String>(),
                colors.number,
            );
            continue;
        }

        // Punctuation / operators.
        if "(),;*=<>!+-/%|.".contains(c) {
            push(job, &c.to_string(), colors.punct);
            i += 1;
            continue;
        }

        // Whitespace and everything else.
        let start = i;
        i += 1;
        push(
            job,
            &chars[start..i].iter().collect::<String>(),
            colors.ident,
        );
    }
}

fn is_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    KEYWORDS.contains(&upper.as_str())
}

pub(crate) const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "TABLE",
    "VIEW",
    "INDEX",
    "DROP",
    "ALTER",
    "ADD",
    "COLUMN",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "ON",
    "USING",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "DISTINCT",
    "AS",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "EXISTS",
    "UNION",
    "ALL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "UNIQUE",
    "DEFAULT",
    "WITH",
    "PRAGMA",
    "EXPLAIN",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRANSACTION",
    "INT",
    "INTEGER",
    "TEXT",
    "REAL",
    "BLOB",
    "BOOLEAN",
    "VARCHAR",
    "TIMESTAMP",
    "DATE",
    "TRUE",
    "FALSE",
    "CAST",
    "COALESCE",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty editor still lays out one row at the editor's font size, so the caret in it
    /// grows with the font instead of staying at egui's default height.
    #[test]
    fn empty_text_keeps_the_editor_font_height() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let height = |size: f32| {
            let job = highlight_sql_folded("", FontId::monospace(size), &[]);
            ctx.fonts_mut(|f| f.layout_job(job)).rows[0].row.size.y
        };
        let expected = ctx.fonts_mut(|f| f.row_height(&FontId::monospace(24.0)));
        assert!((height(24.0) - expected).abs() < 0.5);
        assert!(height(24.0) > height(12.0));
    }

    #[test]
    fn intellij_sql_colours_follow_tokens_and_theme_switches() {
        let registry = crate::theme::ThemeRegistry::load();
        let previous = crate::theme::current();
        let light = serde_json::from_str::<crate::theme::ThemeFile>(include_str!(
            "../../../../examples/themes/intellij-light.json"
        ))
        .unwrap()
        .to_theme();
        let syntax = light.syntax.unwrap();
        let ctx = egui::Context::default();
        let sql =
            "select f.title, f . name from film f where f.id = 42 and f.title = 'Film'; -- note";
        crate::theme::set_current(light);
        let job = highlight_sql_cached(&ctx, sql, FontId::monospace(13.0));
        let colour = |job: &LayoutJob, token: &str| {
            job.sections
                .iter()
                .find(|section| &job.text[section.byte_range.clone()] == token)
                .unwrap()
                .format
                .color
        };
        assert_eq!(colour(&job, "select"), syntax.keyword);
        assert_eq!(colour(&job, "f"), syntax.identifier);
        assert_eq!(colour(&job, "film"), syntax.identifier);
        assert_eq!(colour(&job, "title"), syntax.qualified_identifier);
        assert_eq!(colour(&job, "name"), syntax.qualified_identifier);
        assert_eq!(colour(&job, "."), syntax.punctuation);
        assert_eq!(colour(&job, "42"), syntax.number);
        assert_eq!(colour(&job, "'Film'"), syntax.string);
        assert_eq!(colour(&job, "-- note"), syntax.comment);

        // The same SQL in the same context must refresh when the palette changes.
        let dark = registry.theme_of("carbon");
        crate::theme::set_current(dark);
        let job = highlight_sql_cached(&ctx, sql, FontId::monospace(13.0));
        assert_eq!(colour(&job, "select"), dark.accent);
        assert_eq!(colour(&job, "title"), dark.text);
        crate::theme::set_current(previous);
    }
}
