//! SQL autocomplete for the query editor: tables and columns from the connected
//! schema, plus the SQL keywords the highlighter knows. Works for every backend,
//! because they all introspect into the same [`SchemaTree`].
//!
//! Split in two: pure suggestion logic (`complete`, unit-testable, no egui) and the
//! popup widget (`Popup::show`) drawn over the editor at the text cursor.

use dbcore::{DbKind, SchemaTree};

/// What a suggestion refers to, driving the badge and sort order in the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestionKind {
    Keyword,
    Table,
    Column,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    /// Text inserted into the editor (quoted for the dialect when needed).
    pub insert: String,
    /// Context shown right-aligned and faint: a column's table, a table's schema, "keyword".
    pub detail: String,
    pub kind: SuggestionKind,
}

/// A computed completion: the suggestions plus the char range they would replace
/// (`replace_start..cursor`, the identifier prefix being typed).
#[derive(Debug)]
pub struct Completion {
    pub replace_start: usize,
    pub items: Vec<Suggestion>,
}

/// Popup state kept on the app across frames (immediate mode: keys accepted this frame
/// apply to the list computed last frame).
pub struct State {
    pub open: bool,
    pub selected: usize,
    pub items: Vec<Suggestion>,
    pub replace_start: usize,
    /// Last-known caret char index, cached so a click on the popup — which strips the
    /// editor's focus (and thus its live cursor) the same frame — can still resolve where
    /// to insert.
    pub caret_char: usize,
    /// Last-known on-screen caret rect, used to anchor the popup on a frame where the
    /// editor has lost focus and no longer reports a cursor.
    pub anchor: egui::Rect,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            selected: 0,
            items: Vec::new(),
            replace_start: 0,
            caret_char: 0,
            // `egui::Rect` has no `Default`; a zero rect is never read before the editor
            // has reported a caret (the popup only opens while focused).
            anchor: egui::Rect::ZERO,
        }
    }
}

/// Navigation keys consumed before the `TextEdit` sees them, while the popup is open.
#[derive(Default, Clone, Copy)]
pub struct NavKeys {
    pub up: bool,
    pub down: bool,
    pub accept: bool,
    pub dismiss: bool,
}

const MAX_ITEMS: usize = 100;

fn is_ident_char(c: char) -> bool {
    // `is_alphanumeric` already covers ASCII and base letters of other scripts, but it
    // excludes combining marks — Thai sara/tone marks (and similar) — which would split a
    // word like `ชื่อ` mid-identifier. Treat any non-ASCII, non-whitespace char as part of
    // the identifier so multi-byte names stay whole for prefix matching and tokenizing.
    c.is_alphanumeric() || c == '_' || (!c.is_ascii() && !c.is_whitespace() && !c.is_control())
}

/// Compute suggestions for the identifier being typed at `cursor` (a char index).
///
/// Context rules, in order:
/// - after `qualifier.` → the qualifier's columns (table name or alias) or, failing
///   that, the tables of a schema named `qualifier`;
/// - after `FROM` / `JOIN` / `INTO` / `UPDATE` / `TABLE` → table names;
/// - otherwise → columns (of tables referenced in the query, or all tables when none
///   are), table names, and SQL keywords.
///
/// Returns `None` when there is nothing to offer: empty prefix without `force`, cursor
/// inside a string/comment, or an unknown qualifier.
pub fn complete(
    sql: &str,
    cursor: usize,
    schema: Option<&SchemaTree>,
    kind: Option<DbKind>,
    force: bool,
) -> Option<Completion> {
    let chars: Vec<char> = sql.chars().collect();
    let cursor = cursor.min(chars.len());

    // The identifier prefix being typed, scanning back from the cursor.
    let mut start = cursor;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    let prefix: String = chars[start..cursor].iter().collect();
    // A prefix that starts with a digit is a number literal, not an identifier.
    if prefix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let after_dot = start > 0 && chars[start - 1] == '.';
    if prefix.is_empty() && !after_dot && !force {
        return None;
    }
    if in_string_or_comment(&chars, start) {
        return None;
    }

    let mut items = Vec::new();

    if after_dot {
        // `qualifier.` → columns of that table/alias, or tables of that schema.
        let qualifier = ident_before(&chars, start - 1)?;
        let schema = schema?;
        let aliases = referenced_tables(&chars);
        let table_name = aliases
            .iter()
            .find(|(alias, _)| alias.eq_ignore_ascii_case(&qualifier))
            .map(|(_, table)| table.clone())
            .unwrap_or_else(|| qualifier.clone());
        let mut found = false;
        for t in &schema.tables {
            if t.name.eq_ignore_ascii_case(&table_name) {
                found = true;
                push_columns(&mut items, t, kind, &prefix);
            }
        }
        if !found {
            // Not a table or alias — maybe a schema namespace (e.g. `public.`).
            for t in &schema.tables {
                if t.schema
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&qualifier))
                {
                    found = true;
                    push_table(&mut items, &t.name, t.schema.as_deref(), kind, &prefix);
                }
            }
        }
        if !found {
            return None;
        }
    } else {
        let prev = previous_word(&chars, start);
        let table_context = matches!(
            prev.as_deref(),
            Some("FROM") | Some("JOIN") | Some("INTO") | Some("UPDATE") | Some("TABLE")
        );

        if let Some(schema) = schema {
            if table_context {
                // Distinct schema namespaces first-class too, so `FROM pub…` can
                // complete to `public` and then offer its tables after the dot.
                let mut namespaces: Vec<&str> = schema
                    .tables
                    .iter()
                    .filter_map(|t| t.schema.as_deref())
                    .collect();
                namespaces.sort_unstable();
                namespaces.dedup();
                for ns in namespaces {
                    if matches_prefix(ns, &prefix) {
                        items.push(Suggestion {
                            insert: maybe_quote(ns, kind),
                            detail: "schema".to_string(),
                            kind: SuggestionKind::Table,
                        });
                    }
                }
                for t in &schema.tables {
                    push_table(&mut items, &t.name, t.schema.as_deref(), kind, &prefix);
                }
            } else {
                // General context: columns of the tables this query references (all
                // tables when it references none yet), then tables, then keywords.
                let referenced = referenced_tables(&chars);
                let mut any_referenced = false;
                for t in &schema.tables {
                    if referenced
                        .iter()
                        .any(|(_, table)| table.eq_ignore_ascii_case(&t.name))
                    {
                        any_referenced = true;
                        push_columns(&mut items, t, kind, &prefix);
                    }
                }
                if !any_referenced {
                    for t in &schema.tables {
                        push_columns(&mut items, t, kind, &prefix);
                    }
                }
                for t in &schema.tables {
                    push_table(&mut items, &t.name, t.schema.as_deref(), kind, &prefix);
                }
            }
        }

        if !table_context {
            // Keywords lead at a statement start (nothing significant before the
            // prefix), where `SE…` should offer SELECT before any column.
            let lead = prev.is_none();
            let mut keywords: Vec<Suggestion> = crate::highlight::KEYWORDS
                .iter()
                .filter(|k| matches_prefix(k, &prefix))
                .map(|k| Suggestion {
                    insert: (*k).to_string(),
                    detail: "keyword".to_string(),
                    kind: SuggestionKind::Keyword,
                })
                .collect();
            if lead {
                keywords.append(&mut items);
                items = keywords;
            } else {
                items.append(&mut keywords);
            }
        }
    }

    // Dedup repeated column names across tables (keep the first, which carries its
    // table in `detail`) and identical keyword/table entries.
    let mut seen = std::collections::HashSet::new();
    items.retain(|s| seen.insert((s.kind, s.insert.to_lowercase())));
    items.truncate(MAX_ITEMS);

    if items.is_empty() {
        None
    } else {
        Some(Completion {
            replace_start: start,
            items,
        })
    }
}

fn matches_prefix(candidate: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    // Compare the leading `prefix` characters case-insensitively *by char*, never by byte:
    // identifiers (and the typed prefix) may hold multi-byte UTF-8 — e.g. Thai, where one
    // glyph is 3 bytes — so `candidate[..prefix.len()]` could slice mid-character and panic.
    let mut cand = candidate.chars();
    for p in prefix.chars() {
        match cand.next() {
            Some(c) if c.eq_ignore_ascii_case(&p) => {}
            _ => return false,
        }
    }
    // A char left over means `candidate` is strictly longer than `prefix`; equal length
    // means it's already fully typed, which we don't suggest.
    cand.next().is_some()
}

fn push_table(
    items: &mut Vec<Suggestion>,
    name: &str,
    schema: Option<&str>,
    kind: Option<DbKind>,
    prefix: &str,
) {
    if matches_prefix(name, prefix) {
        items.push(Suggestion {
            insert: maybe_quote(name, kind),
            detail: schema.unwrap_or("table").to_string(),
            kind: SuggestionKind::Table,
        });
    }
}

fn push_columns(
    items: &mut Vec<Suggestion>,
    table: &dbcore::TableInfo,
    kind: Option<DbKind>,
    prefix: &str,
) {
    for col in &table.columns {
        if matches_prefix(&col.name, prefix) {
            items.push(Suggestion {
                insert: maybe_quote(&col.name, kind),
                detail: format!("{} · {}", table.name, col.data_type),
                kind: SuggestionKind::Column,
            });
        }
    }
}

/// Quote an identifier for the dialect only when the bare form wouldn't parse (or, for
/// Postgres, wouldn't fold back to the introspected name).
fn maybe_quote(name: &str, kind: Option<DbKind>) -> String {
    let plain = !name.is_empty()
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        // Unquoted identifiers fold to lowercase in Postgres, so a name introspected
        // with uppercase letters must stay quoted to keep referring to itself.
        && !(kind == Some(DbKind::Postgres) && name.chars().any(|c| c.is_ascii_uppercase()))
        && !crate::highlight::KEYWORDS.contains(&name.to_ascii_uppercase().as_str());
    if plain {
        name.to_string()
    } else {
        match kind {
            Some(k) => k.quote_ident(name),
            None => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }
}

/// The identifier ending right before `end` (exclusive), e.g. the qualifier before a dot.
fn ident_before(chars: &[char], end: usize) -> Option<String> {
    let mut s = end;
    while s > 0 && is_ident_char(chars[s - 1]) {
        s -= 1;
    }
    (s < end).then(|| chars[s..end].iter().collect())
}

/// The previous significant word before `pos`, uppercased — used for context detection.
/// Skips trailing whitespace; stops at punctuation (returning `None` for things like `(`).
fn previous_word(chars: &[char], pos: usize) -> Option<String> {
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
fn in_string_or_comment(chars: &[char], pos: usize) -> bool {
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
                while i < chars.len() && !(chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/') {
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

/// Scan the SQL for `FROM`/`JOIN`/`UPDATE`/`INTO` targets, returning `(alias, table)`
/// pairs. A table without an alias maps to itself, so the result doubles as the set of
/// referenced tables. Quoted and schema-qualified names keep their last bare segment.
fn referenced_tables(chars: &[char]) -> Vec<(String, String)> {
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
                    let next_upper = next.to_ascii_uppercase();
                    if !crate::highlight::KEYWORDS.contains(&next_upper.as_str()) {
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
fn tokenize_words(chars: &[char]) -> Vec<String> {
    let mut words = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '\'' {
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
        } else if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            while i < n && !(chars[i] == '*' && i + 1 < n && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(n);
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

// --- popup widget -------------------------------------------------------------------

/// What the popup reported this frame.
pub enum Event {
    None,
    /// The user accepted item `i` (click, or Enter/Tab routed through [`NavKeys`]).
    Accept(usize),
}

/// Draw the suggestion popup anchored under the text cursor and return what happened
/// along with the popup's screen rect (for the caller's click-outside hit-test). Pure
/// rendering — list mutation and text insertion stay with the caller.
pub fn show_popup(
    ctx: &egui::Context,
    state: &State,
    anchor: egui::Rect,
    nav_moved: bool,
) -> (Event, egui::Rect) {
    use crate::style::palette;

    let mono = egui::FontId::monospace(12.0);
    let small = egui::FontId::proportional(10.5);
    let row_h = 22.0;
    let visible = state.items.len().min(8);
    let width: f32 = 320.0;
    let height = visible as f32 * row_h + 10.0;

    // Below the cursor line by default; above it when the screen runs out underneath.
    let screen = ctx.content_rect();
    let mut pos = egui::pos2(anchor.left(), anchor.bottom() + 4.0);
    if pos.y + height > screen.bottom() {
        pos.y = anchor.top() - height - 4.0;
    }
    pos.x = pos.x.min(screen.right() - width - 8.0).max(screen.left() + 4.0);

    let mut event = Event::None;
    let area = egui::Area::new(egui::Id::new("sql_autocomplete_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(&ctx.global_style())
                .fill(palette::PANEL())
                .stroke(egui::Stroke::new(1.0, palette::BORDER()))
                .inner_margin(4.0)
                .show(ui, |ui| {
                    ui.set_width(width);
                    egui::ScrollArea::vertical()
                        .id_salt("sql_autocomplete_scroll")
                        .max_height(8.0 * row_h)
                        .show(ui, |ui| {
                            for (i, item) in state.items.iter().enumerate() {
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), row_h),
                                    egui::Sense::click(),
                                );
                                if !ui.is_rect_visible(rect) {
                                    continue;
                                }
                                let selected = i == state.selected;
                                if selected {
                                    ui.painter().rect_filled(rect, 4.0, palette::SELECTION());
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        4.0,
                                        palette::SURFACE_HOVER(),
                                    );
                                }
                                if selected && nav_moved {
                                    resp.scroll_to_me(None);
                                }

                                // Kind badge: a small colour-coded letter chip, so kinds
                                // scan as a column (T = table, C = column, K = keyword).
                                let (letter, color) = match item.kind {
                                    SuggestionKind::Table => ("T", palette::ACCENT()),
                                    SuggestionKind::Column => ("C", palette::SUCCESS()),
                                    SuggestionKind::Keyword => ("K", palette::WARNING()),
                                };
                                let badge = egui::Rect::from_center_size(
                                    egui::pos2(rect.left() + 13.0, rect.center().y),
                                    egui::vec2(16.0, 16.0),
                                );
                                ui.painter().rect_filled(
                                    badge,
                                    4.0,
                                    color.gamma_multiply(0.18),
                                );
                                ui.painter().text(
                                    badge.center(),
                                    egui::Align2::CENTER_CENTER,
                                    letter,
                                    small.clone(),
                                    color,
                                );

                                let label_pos =
                                    egui::pos2(rect.left() + 26.0, rect.center().y);
                                ui.painter().text(
                                    label_pos,
                                    egui::Align2::LEFT_CENTER,
                                    &item.insert,
                                    mono.clone(),
                                    palette::TEXT(),
                                );
                                // Detail, right-aligned and clipped against the label.
                                let detail_pos =
                                    egui::pos2(rect.right() - 6.0, rect.center().y);
                                ui.painter().text(
                                    detail_pos,
                                    egui::Align2::RIGHT_CENTER,
                                    &item.detail,
                                    small.clone(),
                                    palette::TEXT_FAINT(),
                                );

                                if resp.clicked() {
                                    event = Event::Accept(i);
                                }
                            }
                        });
                });
        });
    (event, area.response.rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::{ColumnInfo, TableInfo};

    fn schema() -> SchemaTree {
        let col = |name: &str, ty: &str| ColumnInfo {
            name: name.to_string(),
            data_type: ty.to_string(),
            nullable: true,
            primary_key: false,
        };
        SchemaTree {
            database_name: "test".to_string(),
            views: Vec::new(),
            routines: Vec::new(),
            triggers: Vec::new(),
            tables: vec![
                TableInfo {
                    schema: Some("public".to_string()),
                    name: "users".to_string(),
                    columns: vec![col("id", "int"), col("email", "text"), col("name", "text")],
                    indexes: vec![],
                    foreign_keys: vec![],
                },
                TableInfo {
                    schema: Some("public".to_string()),
                    name: "orders".to_string(),
                    columns: vec![col("id", "int"), col("user_id", "int"), col("total", "numeric")],
                    indexes: vec![],
                    foreign_keys: vec![],
                },
            ],
        }
    }

    fn labels(c: &Completion) -> Vec<&str> {
        c.items.iter().map(|s| s.insert.as_str()).collect()
    }

    #[test]
    fn tables_after_from() {
        let s = schema();
        let sql = "SELECT * FROM us";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["users"]);
        assert_eq!(c.replace_start, sql.len() - 2);
    }

    #[test]
    fn columns_after_table_dot() {
        let s = schema();
        let sql = "SELECT users. FROM users";
        let c = complete(sql, 13, Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["id", "email", "name"]);
    }

    #[test]
    fn columns_via_alias() {
        let s = schema();
        let sql = "SELECT u.em FROM users u";
        let c = complete(sql, 11, Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["email"]);
    }

    #[test]
    fn keywords_lead_at_statement_start() {
        let s = schema();
        let c = complete("SEL", 3, Some(&s), None, false).unwrap();
        assert_eq!(c.items[0].insert, "SELECT");
        assert_eq!(c.items[0].kind, SuggestionKind::Keyword);
    }

    #[test]
    fn referenced_table_columns_in_select() {
        let s = schema();
        let sql = "SELECT to FROM orders";
        let c = complete(sql, 9, Some(&s), None, false).unwrap();
        // Only orders is referenced, so its `total` leads (users' columns excluded).
        assert_eq!(c.items[0].insert, "total");
        assert_eq!(c.items[0].kind, SuggestionKind::Column);
    }

    #[test]
    fn schema_namespace_dot_lists_tables() {
        let s = schema();
        let sql = "SELECT * FROM public.";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["users", "orders"]);
        assert!(c.items.iter().all(|i| i.kind == SuggestionKind::Table));
    }

    #[test]
    fn no_popup_inside_string_or_comment() {
        let s = schema();
        assert!(complete("SELECT 'us", 10, Some(&s), None, false).is_none());
        assert!(complete("-- us", 5, Some(&s), None, false).is_none());
    }

    #[test]
    fn empty_prefix_needs_force_or_dot() {
        let s = schema();
        let sql = "SELECT * FROM ";
        assert!(complete(sql, sql.len(), Some(&s), None, false).is_none());
        let forced = complete(sql, sql.len(), Some(&s), None, true).unwrap();
        assert!(forced.items.iter().any(|i| i.insert == "users"));
    }

    #[test]
    fn comma_keeps_table_context() {
        let s = schema();
        let sql = "SELECT * FROM users, ord";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert!(labels(&c).contains(&"orders"));
        assert!(c.items.iter().all(|i| i.kind == SuggestionKind::Table));
    }

    #[test]
    fn quoting_follows_dialect() {
        assert_eq!(maybe_quote("order", Some(DbKind::MySql)), "`order`");
        assert_eq!(maybe_quote("MyCol", Some(DbKind::Postgres)), "\"MyCol\"");
        assert_eq!(maybe_quote("MyCol", Some(DbKind::MySql)), "MyCol");
        assert_eq!(maybe_quote("plain", Some(DbKind::Postgres)), "plain");
        assert_eq!(maybe_quote("has space", None), "\"has space\"");
    }

    #[test]
    fn keywords_without_connection() {
        let c = complete("SEL", 3, None, None, false).unwrap();
        assert_eq!(c.items[0].insert, "SELECT");
    }

    #[test]
    fn multibyte_prefix_does_not_panic() {
        // A Thai prefix (3 bytes/char) once sliced `candidate` on a byte boundary and
        // panicked; matching must be char-aware. Both the typed prefix and the candidate
        // identifiers carry multi-byte text here.
        let s = SchemaTree {
            database_name: "db".to_string(),
            views: Vec::new(),
            routines: Vec::new(),
            triggers: Vec::new(),
            tables: vec![TableInfo {
                schema: None,
                name: "ลูกค้า".to_string(),
                columns: vec![
                    ColumnInfo {
                        name: "ชื่อ".to_string(),
                        data_type: "text".to_string(),
                        nullable: true,
                        primary_key: false,
                    },
                    ColumnInfo {
                        name: "อีเมล".to_string(),
                        data_type: "text".to_string(),
                        nullable: true,
                        primary_key: false,
                    },
                ],
                indexes: vec![],
                foreign_keys: vec![],
            }],
        };
        // `SELECT ชื่` should offer the matching Thai column without panicking. Non-ASCII
        // identifiers come back quoted for the dialect (here, generic double quotes).
        let sql = "SELECT ชื่";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert!(c.items.iter().any(|i| i.insert == "\"ชื่อ\""));
        // And a Thai table name after FROM.
        let sql2 = "SELECT * FROM ลูก";
        let c2 = complete(sql2, sql2.chars().count(), Some(&s), None, false).unwrap();
        assert!(c2.items.iter().any(|i| i.insert == "\"ลูกค้า\""));
    }
}
