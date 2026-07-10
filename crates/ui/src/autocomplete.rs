//! SQL autocomplete for the query editor: tables and columns from the connected
//! schema, plus the SQL keywords the highlighter knows. Works for every backend,
//! because they all introspect into the same [`SchemaTree`].
//!
//! Split in two: pure suggestion logic (`complete`, unit-testable, no egui) and the
//! popup widget (`Popup::show`) drawn over the editor at the text cursor.
//!
//! Cursor context — word scanning, string/comment detection, and the table/alias scan —
//! lives in [`crate::sqlctx`], shared with the inline ghost suggestion.

use crate::sqlctx::{
    ident_before, in_string_or_comment, is_ident_char, previous_word, referenced_tables,
};
use dbcore::{DbKind, SchemaTree};

/// Whether `c` opens a *quoted identifier* in this dialect — deliberately not "any quote
/// character". MySQL spells identifiers with backticks and uses `"` for string literals, so
/// treating a typed `"` as an identifier there would let a table name overwrite the string
/// the user was halfway through. Mirrors [`DbKind::quote_ident`]; SQL Server also accepts
/// the `[…]` form it does not itself emit.
fn opens_quoted_ident(kind: Option<DbKind>, c: char) -> bool {
    match kind {
        Some(DbKind::MySql | DbKind::MariaDb) => c == '`',
        Some(DbKind::SqlServer) => c == '"' || c == '[',
        _ => c == '"',
    }
}

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
/// (`replace_start..cursor`), which is the identifier prefix being typed together with any
/// opening quote in front of it — suggestions arrive fully quoted and must overwrite it.
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

    // An opening quote the user already typed (`FROM "cus…`) belongs to the identifier, so
    // it has to be part of the range the suggestion replaces — every suggestion carries its
    // own quotes. Leaving it out doubles it. The prefix itself stays unquoted, because that
    // is what schema names are matched against.
    let replace_start = match start.checked_sub(1) {
        Some(before) if opens_quoted_ident(kind, chars[before]) => before,
        _ => start,
    };

    // Read context from where the identifier really begins, or a leading quote hides the
    // `FROM` that precedes it and table suggestions never fire.
    let after_dot = replace_start > 0 && chars[replace_start - 1] == '.';
    if prefix.is_empty() && !after_dot && !force {
        return None;
    }
    if in_string_or_comment(&chars, replace_start) {
        return None;
    }

    let mut items = Vec::new();

    if after_dot {
        // `qualifier.` → columns of that table/alias, or tables of that schema.
        let qualifier = ident_before(&chars, replace_start - 1)?;
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
        let prev = previous_word(&chars, replace_start);
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
            replace_start,
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

// --- popup widget -------------------------------------------------------------------

/// The icon-rail colour for a suggestion kind: one hue, three weights.
///
/// The theme's accent carries the schema — a table at full strength, a column muted toward
/// the body text because it is a detail *of* a table — while a keyword stays uncoloured, as
/// it belongs to SQL rather than to this database. A second and third hue would turn the
/// rail into a legend the reader has to learn; a weight ladder is read at a glance.
fn kind_color(kind: SuggestionKind) -> egui::Color32 {
    let t = crate::theme::current();
    match kind {
        SuggestionKind::Table => t.accent,
        SuggestionKind::Column => crate::style::mix(t.accent, t.text_weak, 0.75),
        SuggestionKind::Keyword => t.text_faint,
    }
}

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
    // Tight rows, small margins — a dense, clean list with no wasted space.
    let row_h = 20.0;
    let margin = 3.0_f32;
    let visible = state.items.len().min(9);
    let width: f32 = 300.0;
    let height = visible as f32 * row_h + margin * 2.0;

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
            // A flat panel: hairline border, no shadow, so it reads as part of the editor
            // rather than a floating card.
            egui::Frame::popup(&ctx.global_style())
                .fill(palette::PANEL())
                .stroke(egui::Stroke::new(1.0, palette::BORDER()))
                .shadow(egui::epaint::Shadow::NONE)
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(margin)
                .show(ui, |ui| {
                    ui.set_width(width);
                    // Rows sit flush against each other — the tight, dense list the design calls
                    // for (egui would otherwise insert `item_spacing.y` between them).
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::ScrollArea::vertical()
                        .id_salt("sql_autocomplete_scroll")
                        .max_height(9.0 * row_h)
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
                                    ui.painter().rect_filled(rect, 3.0, palette::SELECTION());
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        3.0,
                                        palette::SURFACE_HOVER(),
                                    );
                                }
                                if selected && nav_moved {
                                    resp.scroll_to_me(None);
                                }

                                // Kind icon, coloured so the three kinds separate at a glance:
                                // a filled header band for a table, a filled vertical band for
                                // a column, `< >` for a keyword.
                                let icon = match item.kind {
                                    SuggestionKind::Table => crate::icons::table(),
                                    SuggestionKind::Column => crate::icons::column(),
                                    SuggestionKind::Keyword => crate::icons::code(),
                                };
                                const ICON: f32 = 13.0;
                                let icon_rect = egui::Rect::from_center_size(
                                    egui::pos2(rect.left() + 11.0, rect.center().y),
                                    egui::vec2(ICON, ICON),
                                );
                                egui::Image::new(icon)
                                    .fit_to_exact_size(egui::vec2(ICON, ICON))
                                    .tint(kind_color(item.kind))
                                    .paint_at(ui, icon_rect);

                                let label_pos =
                                    egui::pos2(rect.left() + 24.0, rect.center().y);
                                ui.painter().text(
                                    label_pos,
                                    egui::Align2::LEFT_CENTER,
                                    &item.insert,
                                    mono.clone(),
                                    palette::TEXT(),
                                );
                                // Detail, right-aligned and clipped against the label.
                                let detail_pos =
                                    egui::pos2(rect.right() - 7.0, rect.center().y);
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

    /// Apply the chosen suggestion the way `accept_suggestion` does: overwrite
    /// `replace_start..cursor` with `insert`.
    fn accept(sql: &str, s: &SchemaTree, kind: Option<DbKind>, pick: &str) -> String {
        let cursor = sql.chars().count();
        let c = complete(sql, cursor, Some(s), kind, false).unwrap();
        let item = c
            .items
            .iter()
            .find(|i| i.insert.contains(pick))
            .unwrap_or_else(|| panic!("no item containing {pick:?} in {:?}", c.items));
        let mut out: Vec<char> = sql.chars().collect();
        out.splice(c.replace_start..cursor, item.insert.chars());
        out.into_iter().collect()
    }

    #[test]
    fn accepting_absorbs_an_opening_quote_the_user_typed() {
        let s = thai_schema();
        let pg = Some(DbKind::Postgres);
        // Without a typed quote the suggestion simply brings its own.
        assert_eq!(
            accept("SELECT * FROM ลูก", &s, pg, "ลูกค้า"),
            "SELECT * FROM \"ลูกค้า\""
        );
        // With one, it must be overwritten rather than left in front — this used to yield
        // `FROM ""ลูกค้า"`.
        assert_eq!(
            accept("SELECT * FROM \"ลูก", &s, pg, "ลูกค้า"),
            "SELECT * FROM \"ลูกค้า\""
        );
    }

    #[test]
    fn a_typed_quote_still_reads_as_table_context() {
        let s = thai_schema();
        // The `"` must not hide the `FROM` behind it, or columns and keywords crowd out the
        // table the user is clearly reaching for.
        let sql = "SELECT * FROM \"ลูก";
        let c = complete(sql, sql.chars().count(), Some(&s), Some(DbKind::Postgres), false).unwrap();
        assert_eq!(c.items[0].kind, SuggestionKind::Table);
    }

    #[test]
    fn backtick_is_the_identifier_quote_on_mysql() {
        let s = thai_schema();
        let my = Some(DbKind::MySql);
        assert_eq!(
            accept("SELECT * FROM `ลูก", &s, my, "ลูกค้า"),
            "SELECT * FROM `ลูกค้า`"
        );
        // …and `"` is a *string literal* there, so it is left alone. Suggestions may still
        // appear, but they must never eat the quote that opened the string.
        let sql = "SELECT * FROM t WHERE name = \"ลูก";
        if let Some(c) = complete(sql, sql.chars().count(), Some(&s), my, false) {
            let quote_at = sql.chars().count() - 4; // the `"` before `ลูก`
            assert!(c.replace_start > quote_at, "must not absorb a string's quote");
        }
    }

    #[test]
    fn brackets_quote_identifiers_on_sql_server() {
        let s = thai_schema();
        assert_eq!(
            accept("SELECT * FROM [ลูก", &s, Some(DbKind::SqlServer), "ลูกค้า"),
            "SELECT * FROM \"ลูกค้า\""
        );
    }

    #[test]
    fn columns_complete_after_a_quoted_table_name() {
        let s = thai_schema();
        // `"ลูกค้า".` used to offer nothing at all: the qualifier scan stopped at the quote.
        let sql = "SELECT * FROM \"ลูกค้า\" WHERE \"ลูกค้า\".";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(c.items[0].insert, "\"ชื่อ\"");
        assert_eq!(c.items[0].kind, SuggestionKind::Column);
    }

    fn thai_schema() -> SchemaTree {
        SchemaTree {
            database_name: "db".to_string(),
            views: Vec::new(),
            routines: Vec::new(),
            triggers: Vec::new(),
            tables: vec![TableInfo {
                schema: None,
                name: "ลูกค้า".to_string(),
                columns: vec![ColumnInfo {
                    name: "ชื่อ".to_string(),
                    data_type: "text".to_string(),
                    nullable: true,
                    primary_key: false,
                }],
                indexes: vec![],
                foreign_keys: vec![],
            }],
        }
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

    /// Render the popup with a mix of all three kinds under `theme_key`, so the icon rail's
    /// colour and glyph legibility can be judged at the size it actually ships at.
    fn render_popup_snapshot(theme_key: &str, name: &str) {
        let item = |insert: &str, detail: &str, kind: SuggestionKind| Suggestion {
            insert: insert.to_string(),
            detail: detail.to_string(),
            kind,
        };
        let state = State {
            open: true,
            selected: 1,
            items: vec![
                item("orders", "public", SuggestionKind::Table),
                item("order_items", "public", SuggestionKind::Table),
                item("user_id", "orders · integer", SuggestionKind::Column),
                item("created_at", "orders · timestamptz", SuggestionKind::Column),
                item("SELECT", "keyword", SuggestionKind::Keyword),
                item("ORDER BY", "keyword", SuggestionKind::Keyword),
            ],
            replace_start: 0,
            caret_char: 0,
            anchor: egui::Rect::ZERO,
        };
        let theme = crate::theme::ThemeRegistry::load().theme_of(theme_key);
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(340.0, 190.0))
            .with_pixels_per_point(2.0)
            .build_ui(move |ui| {
                if !setup {
                    egui_extras::install_image_loaders(ui.ctx());
                    crate::theme::set_current(theme);
                    crate::style::apply(ui.ctx());
                    setup = true;
                }
                ui.painter().rect_filled(
                    ui.ctx().content_rect(),
                    0.0,
                    crate::style::palette::CODE_BG(),
                );
                let anchor = egui::Rect::from_min_size(egui::pos2(8.0, 4.0), egui::vec2(1.0, 14.0));
                show_popup(ui.ctx(), &state, anchor, false);
            });
        harness.run_steps(8);
        harness.snapshot(name);
    }

    /// Screenshot generator (ignored): the popup on the default dark theme.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_popup() {
        render_popup_snapshot("midnight-conversational", "autocomplete_popup");
    }

    /// Screenshot generator (ignored): the same popup on the light theme, where the kind
    /// hues have to hold up against a white panel.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_popup_light() {
        render_popup_snapshot("daylight", "autocomplete_popup_light");
    }
}
