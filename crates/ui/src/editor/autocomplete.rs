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
    cte_names, ident_before, in_string_or_comment, is_ident_char, previous_word, referenced_tables,
};
use dbcore::config::EditorOptions;
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
    Function,
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
    /// The schema a table or view lives in, for inserting it qualified when the editor's
    /// "Prefix schema names" option is on.
    pub schema: Option<String>,
}

/// A computed completion: the suggestions plus the char range they would replace
/// (`replace_start..cursor`), which is the identifier prefix being typed together with any
/// opening quote in front of it — suggestions arrive fully quoted and must overwrite it.
#[derive(Debug)]
pub struct Completion {
    pub replace_start: usize,
    pub items: Vec<Suggestion>,
    /// The bare identifier prefix that was typed (no quotes). The popup paints the run of
    /// each suggestion that matched it in the accent colour, so the reader can see *why*
    /// each row is on the list.
    pub prefix: String,
}

/// Popup state kept on the app across frames (immediate mode: keys accepted this frame
/// apply to the list computed last frame).
pub struct State {
    pub open: bool,
    pub selected: usize,
    pub items: Vec<Suggestion>,
    pub replace_start: usize,
    /// The identifier prefix the list was computed for; painted in the accent colour inside
    /// each row (see [`matched_chars`]).
    pub prefix: String,
    /// Last-known caret char index, cached so a click on the popup — which strips the
    /// editor's focus (and thus its live cursor) the same frame — can still resolve where
    /// to insert.
    pub caret_char: usize,
    /// Last-known on-screen caret rect, used to anchor the popup on a frame where the
    /// editor has lost focus and no longer reports a cursor.
    pub anchor: egui::Rect,
    /// The `(sql revision, caret)` the list was last computed for. An open popup only
    /// recomputes when either moves — not on every repaint.
    pub computed_for: Option<(u64, usize)>,
    /// The one-name inline completion found when the list was computed for `(sql revision,
    /// caret)` — what ghost text would otherwise recompute from scratch.
    pub inline_hint: Option<((u64, usize), Option<String>)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            selected: 0,
            items: Vec::new(),
            replace_start: 0,
            prefix: String::new(),
            caret_char: 0,
            // `egui::Rect` has no `Default`; a zero rect is never read before the editor
            // has reported a caret (the popup only opens while focused).
            anchor: egui::Rect::ZERO,
            computed_for: None,
            inline_hint: None,
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
const FUNCTIONS: &[&str] = &[
    "AVG",
    "COALESCE",
    "COUNT",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "LOWER",
    "MAX",
    "MIN",
    "NULLIF",
    "ROUND",
    "SUM",
    "TRIM",
    "UPPER",
];

/// [`complete_with`] under the default options.
#[cfg(test)]
pub fn complete(
    sql: &str,
    cursor: usize,
    schema: Option<&SchemaTree>,
    kind: Option<DbKind>,
    force: bool,
) -> Option<Completion> {
    complete_with(sql, cursor, schema, kind, force, &EditorOptions::default())
}

/// Compute suggestions for the identifier being typed at `cursor` (a char index).
///
/// Context rules, in order:
/// - after `qualifier.` → the qualifier's columns (table name or alias) or, failing
///   that, the tables of a schema named `qualifier`;
/// - after `FROM` / `JOIN` / `INTO` / `UPDATE` / `TABLE` → table and view names;
/// - after `EXEC` / `EXECUTE` / `CALL` → stored procedures and functions;
/// - otherwise → columns (of tables referenced in the query, or all tables when none
///   are), table and view names, routines, and SQL keywords.
///
/// A name matches when it starts with what was typed, or has it at a word start
/// (`prog` → `x_program`), anywhere inside it, or as a scattered run beginning at its first
/// letter (`emf` → `enum_first`). Closer matches sort first; within a tier the context
/// order above holds.
///
/// Returns `None` when there is nothing to offer: empty prefix without `force`, cursor
/// inside a string/comment, or an unknown qualifier.
///
/// `options` choose the kinds of names to offer, and whether keywords come in upper or
/// lower case.
pub fn complete_with(
    sql: &str,
    cursor: usize,
    schema: Option<&SchemaTree>,
    kind: Option<DbKind>,
    force: bool,
    options: &EditorOptions,
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
    // Column names already offered. With no table referenced yet every column of every table
    // is a candidate — easily 100k on a real database, mostly repeats (`id`, `created_at`) —
    // so a repeat is skipped before it costs a match or a suggestion.
    let mut seen_columns = std::collections::HashSet::new();
    let ctes = cte_names(&chars);

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
                push_columns(
                    &mut items,
                    &mut seen_columns,
                    &t.name,
                    &t.columns,
                    kind,
                    &prefix,
                );
            }
        }
        for v in &schema.views {
            if v.name.eq_ignore_ascii_case(&table_name) {
                found = true;
                push_columns(
                    &mut items,
                    &mut seen_columns,
                    &v.name,
                    &v.columns,
                    kind,
                    &prefix,
                );
            }
        }
        if !found
            && ctes
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&table_name))
        {
            // A CTE's exact projection may still be half-written. Use columns from the
            // physical tables referenced inside it, which is both useful and safe.
            for (_, referenced) in referenced_tables(&chars) {
                if ctes
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&referenced))
                {
                    continue;
                }
                for table in &schema.tables {
                    if table.name.eq_ignore_ascii_case(&referenced) {
                        found = true;
                        push_columns(
                            &mut items,
                            &mut seen_columns,
                            &table.name,
                            &table.columns,
                            kind,
                            &prefix,
                        );
                    }
                }
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
            for v in &schema.views {
                if v.schema
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&qualifier))
                {
                    found = true;
                    push_view(&mut items, v, kind, &prefix);
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
        let routine_context = matches!(
            prev.as_deref(),
            Some("EXEC") | Some("EXECUTE") | Some("CALL")
        );

        if let (Some(schema), true) = (schema, routine_context) {
            for routine in &schema.routines {
                push_routine(&mut items, routine, kind, &prefix);
            }
        } else if let Some(schema) = schema {
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
                    if match_tier(ns, &prefix).is_some() {
                        items.push(Suggestion {
                            insert: maybe_quote(ns, kind),
                            detail: "schema".to_string(),
                            kind: SuggestionKind::Table,
                            schema: None,
                        });
                    }
                }
                for t in &schema.tables {
                    push_table(&mut items, &t.name, t.schema.as_deref(), kind, &prefix);
                }
                for v in &schema.views {
                    push_view(&mut items, v, kind, &prefix);
                }
            } else {
                // General context: columns of the tables this query references (all
                // tables when it references none yet), then tables, then keywords.
                let referenced = referenced_tables(&chars);
                let mut any_referenced = false;
                let is_referenced = |name: &str| {
                    referenced
                        .iter()
                        .any(|(_, table)| table.eq_ignore_ascii_case(name))
                };
                for t in &schema.tables {
                    if is_referenced(&t.name) {
                        any_referenced = true;
                        push_columns(
                            &mut items,
                            &mut seen_columns,
                            &t.name,
                            &t.columns,
                            kind,
                            &prefix,
                        );
                    }
                }
                for v in &schema.views {
                    if is_referenced(&v.name) {
                        any_referenced = true;
                        push_columns(
                            &mut items,
                            &mut seen_columns,
                            &v.name,
                            &v.columns,
                            kind,
                            &prefix,
                        );
                    }
                }
                if !any_referenced {
                    for t in &schema.tables {
                        push_columns(
                            &mut items,
                            &mut seen_columns,
                            &t.name,
                            &t.columns,
                            kind,
                            &prefix,
                        );
                    }
                }
                for t in &schema.tables {
                    push_table(&mut items, &t.name, t.schema.as_deref(), kind, &prefix);
                }
                for v in &schema.views {
                    push_view(&mut items, v, kind, &prefix);
                }
                for routine in &schema.routines {
                    push_routine(&mut items, routine, kind, &prefix);
                }
            }
        }
        if table_context {
            for cte in &ctes {
                if match_tier(cte, &prefix).is_some() {
                    items.push(Suggestion {
                        insert: maybe_quote(cte, kind),
                        detail: "CTE".to_string(),
                        kind: SuggestionKind::Table,
                        schema: None,
                    });
                }
            }
        }

        if !table_context && !routine_context {
            // Keywords lead at a statement start (nothing significant before the
            // prefix), where `SE…` should offer SELECT before any column.
            let lead = prev.is_none();
            let mut keywords: Vec<Suggestion> = crate::highlight::KEYWORDS
                .iter()
                .filter(|k| {
                    match_tier(k, &prefix).is_some()
                        && !FUNCTIONS
                            .iter()
                            .any(|function| function.eq_ignore_ascii_case(k))
                })
                .map(|k| Suggestion {
                    insert: (*k).to_string(),
                    detail: "keyword".to_string(),
                    kind: SuggestionKind::Keyword,
                    schema: None,
                })
                .collect();
            keywords.extend(
                FUNCTIONS
                    .iter()
                    .filter(|function| match_tier(function, &prefix).is_some())
                    .map(|function| Suggestion {
                        insert: (*function).to_string(),
                        detail: "function".to_string(),
                        kind: SuggestionKind::Function,
                        schema: None,
                    }),
            );
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
    items.retain(|s| match s.kind {
        SuggestionKind::Table => options.suggest_tables,
        SuggestionKind::Column => options.suggest_columns,
        SuggestionKind::Function => options.suggest_functions,
        SuggestionKind::Keyword => options.suggest_keywords,
    });
    if !options.uppercase_keywords {
        // Keywords and the built-in functions are the only names spelled by the editor
        // rather than the schema, so they are the only ones whose case is a preference.
        for item in &mut items {
            let builtin = item.kind == SuggestionKind::Keyword
                || (item.kind == SuggestionKind::Function && item.detail == "function");
            if builtin {
                item.insert = item.insert.to_lowercase();
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    items.retain(|s| seen.insert((s.kind, s.insert.to_lowercase())));
    // Closest matches first; the sort is stable, so each tier keeps the context order.
    items.sort_by_cached_key(|s| match_tier(unquoted(&s.insert), &prefix));
    items.truncate(MAX_ITEMS);

    if items.is_empty() {
        None
    } else {
        Some(Completion {
            replace_start,
            items,
            prefix,
        })
    }
}

/// The text accepting `item` inserts. With "Prefix schema names" on, a table or view comes
/// schema-qualified (`dbo.orders`) — unless the user already typed a qualifier before it.
pub fn insertion_text(
    item: &Suggestion,
    after_dot: bool,
    kind: Option<DbKind>,
    options: &EditorOptions,
) -> String {
    match item.schema.as_deref() {
        Some(schema) if options.prefix_schema && !after_dot => {
            format!("{}.{}", maybe_quote(schema, kind), item.insert)
        }
        _ => item.insert.clone(),
    }
}

/// The append-only tail of an unambiguous completion, suitable for inline ghost text.
///
/// A completion that needs to rewrite what the user typed (most notably adding an opening
/// identifier quote) stays in the popup: ghost text can only append after the caret.
pub fn inline_suffix(completion: &Completion) -> Option<String> {
    let [item] = completion.items.as_slice() else {
        return None;
    };
    append_tail(&item.insert, &completion.prefix)
}

/// The part of the popup's highlighted row still to be typed, for previewing it inline after
/// the caret. `None` when the row doesn't simply extend the prefix (a scattered match, or one
/// that has to add an opening quote) — ghost text can only append.
pub fn selected_tail(state: &State) -> Option<String> {
    append_tail(&state.items.get(state.selected)?.insert, &state.prefix)
}

/// What remains of `insert` after the typed `prefix`, when `insert` starts with it.
fn append_tail(insert: &str, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    let mut candidate = insert.char_indices();
    let mut end = 0;
    for expected in prefix.chars() {
        let (at, actual) = candidate.next()?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return None;
        }
        end = at + actual.len_utf8();
    }
    let tail = &insert[end..];
    (!tail.is_empty()).then(|| tail.to_string())
}

/// How a name matched what was typed. Lower tiers are closer matches and sort first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchTier {
    /// `cus` → `customer`.
    Prefix,
    /// At the start of a later word: `prog` → `x_program`, `Name` → `customerName`.
    WordStart,
    /// Anywhere inside: `stom` → `customer`.
    Substring,
    /// Scattered, in order, from the first letter: `emf` → `enum_first`.
    Fuzzy,
}

/// The match tier of `candidate` for the typed `prefix`, compared case-insensitively *by
/// char* (identifiers may be Thai, 3 bytes a glyph). `None` when it doesn't match, or when
/// it is exactly what was typed — a fully typed name has nothing left to complete. Substring
/// and scattered matches need two typed characters; one letter matching anywhere is noise.
fn match_tier(candidate: &str, prefix: &str) -> Option<MatchTier> {
    locate(candidate, prefix).map(|(tier, _)| tier)
}

/// Where `prefix` matches `candidate`: the tier and, for a contiguous match, the char index it
/// starts at. Runs once per schema name on every keystroke — tens of thousands of columns on
/// a large database — so it walks the strings in place and never allocates.
fn locate(candidate: &str, prefix: &str) -> Option<(MatchTier, usize)> {
    let typed = prefix.chars().count();
    if typed == 0 {
        return Some((MatchTier::Prefix, 0));
    }
    if candidate.chars().count() <= typed {
        // Equal-length matches are already fully typed; shorter ones can't match.
        return None;
    }
    let starts_with = |rest: &str| {
        let mut chars = rest.chars();
        prefix
            .chars()
            .all(|p| chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&p)))
    };
    if starts_with(candidate) {
        return Some((MatchTier::Prefix, 0));
    }
    let mut substring = None;
    let mut previous = None;
    for (n, (at, c)) in candidate.char_indices().enumerate() {
        if let Some(before) = previous.replace(c) {
            let word_start = (!before.is_alphanumeric() && c.is_alphanumeric())
                || (before.is_lowercase() && c.is_uppercase());
            if starts_with(&candidate[at..]) {
                if word_start {
                    return Some((MatchTier::WordStart, n));
                }
                substring.get_or_insert(n);
            }
        }
    }
    if typed < 2 {
        return None;
    }
    if let Some(n) = substring {
        return Some((MatchTier::Substring, n));
    }
    // Scattered, in order, starting on the first letter.
    let mut rest = candidate.chars();
    let mut typed_chars = prefix.chars();
    let first = typed_chars.next()?;
    if !rest.next().is_some_and(|c| c.eq_ignore_ascii_case(&first)) {
        return None;
    }
    typed_chars
        .all(|p| rest.any(|c| c.eq_ignore_ascii_case(&p)))
        .then_some((MatchTier::Fuzzy, 0))
}

/// [`match_tier`] plus the char indices of `candidate` that matched, for highlighting. Only
/// the rows on screen ask for this, so it is free to allocate.
fn match_positions(candidate: &str, prefix: &str) -> Option<(MatchTier, Vec<usize>)> {
    let (tier, start) = locate(candidate, prefix)?;
    let typed = prefix.chars().count();
    if tier != MatchTier::Fuzzy {
        return Some((tier, (start..start + typed).collect()));
    }
    let mut positions = Vec::with_capacity(typed);
    let mut typed_chars = prefix.chars().peekable();
    for (n, c) in candidate.chars().enumerate() {
        match typed_chars.peek() {
            Some(p) if c.eq_ignore_ascii_case(p) => {
                positions.push(n);
                typed_chars.next();
            }
            Some(_) => {}
            None => break,
        }
    }
    Some((tier, positions))
}

/// An inserted identifier without the dialect quotes around it — what the typed prefix is
/// matched against.
fn unquoted(insert: &str) -> &str {
    let inner = insert
        .strip_prefix(['"', '`', '['])
        .and_then(|rest| rest.strip_suffix(['"', '`', ']']));
    inner.unwrap_or(insert)
}

/// The char indices of `insert` to paint in the accent colour — the characters the typed
/// `prefix` matched — so the reader sees *why* each row is on the list. Suggestions arrive
/// quoted for the dialect while the prefix is bare, so the opening quote joins a leading
/// match: highlighting `co` but not the `"` in front of it would strand the quote mid-word.
fn matched_chars(insert: &str, prefix: &str) -> Vec<usize> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let inner = unquoted(insert);
    let quoted = inner.len() != insert.len();
    let Some((_, positions)) = match_positions(inner, prefix) else {
        return Vec::new();
    };
    let shift = usize::from(quoted);
    let mut out: Vec<usize> = positions.into_iter().map(|i| i + shift).collect();
    if quoted && out.first() == Some(&1) {
        out.insert(0, 0);
    }
    out
}

fn push_table(
    items: &mut Vec<Suggestion>,
    name: &str,
    schema: Option<&str>,
    kind: Option<DbKind>,
    prefix: &str,
) {
    if match_tier(name, prefix).is_some() {
        items.push(Suggestion {
            insert: maybe_quote(name, kind),
            detail: schema.unwrap_or("table").to_string(),
            kind: SuggestionKind::Table,
            schema: schema.map(str::to_string),
        });
    }
}

/// A view completes like a table; its detail says it is one, since the icon can't.
fn push_view(
    items: &mut Vec<Suggestion>,
    view: &dbcore::ViewInfo,
    kind: Option<DbKind>,
    prefix: &str,
) {
    if match_tier(&view.name, prefix).is_some() {
        let label = if view.materialized {
            "materialized view"
        } else {
            "view"
        };
        items.push(Suggestion {
            insert: maybe_quote(&view.name, kind),
            detail: match view.schema.as_deref() {
                Some(schema) => format!("{schema} · {label}"),
                None => label.to_string(),
            },
            kind: SuggestionKind::Table,
            schema: view.schema.clone(),
        });
    }
}

/// A stored function or procedure, detailed with its parameter count and return type.
fn push_routine(
    items: &mut Vec<Suggestion>,
    routine: &dbcore::RoutineInfo,
    kind: Option<DbKind>,
    prefix: &str,
) {
    if match_tier(&routine.name, prefix).is_none() {
        return;
    }
    let mut detail = routine.kind.label().to_lowercase();
    if let Some(returns) = routine.return_type.as_deref().filter(|r| !r.is_empty()) {
        detail = format!("{detail} → {returns}");
    }
    items.push(Suggestion {
        insert: maybe_quote(&routine.name, kind),
        detail,
        kind: SuggestionKind::Function,
        schema: None,
    });
}

fn push_columns<'s>(
    items: &mut Vec<Suggestion>,
    seen: &mut std::collections::HashSet<&'s str>,
    table: &str,
    columns: &'s [dbcore::ColumnInfo],
    kind: Option<DbKind>,
    prefix: &str,
) {
    for col in columns {
        // A name met before was matched then: offered already, or not a match anywhere.
        if seen.insert(&col.name) && match_tier(&col.name, prefix).is_some() {
            items.push(Suggestion {
                insert: maybe_quote(&col.name, kind),
                detail: format!("{table} · {}", col.data_type),
                kind: SuggestionKind::Column,
                schema: None,
            });
        }
    }
}

/// Quote an identifier for the dialect only when the bare form wouldn't parse (or, for
/// Postgres, wouldn't fold back to the introspected name).
fn maybe_quote(name: &str, kind: Option<DbKind>) -> String {
    // The positive, per-rule form reads clearer than clippy's De Morgan rewrite, and each
    // clause is documented inline; keep it.
    #[allow(clippy::nonminimal_bool)]
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

/// Each kind has its own colour, as in VS Code, so a mixed list scans by kind at a glance:
/// purple functions, blue columns, orange tables, neutral keywords. Light themes use deeper
/// shades of the same hues to keep contrast on a pale background.
fn icon_color(kind: SuggestionKind) -> egui::Color32 {
    let t = crate::theme::current();
    let rgb = egui::Color32::from_rgb;
    match (kind, t.is_dark) {
        (SuggestionKind::Function, true) => rgb(0xb1, 0x80, 0xd7),
        (SuggestionKind::Function, false) => rgb(0x65, 0x2d, 0x90),
        (SuggestionKind::Column, true) => rgb(0x75, 0xbe, 0xff),
        (SuggestionKind::Column, false) => rgb(0x00, 0x7a, 0xcc),
        (SuggestionKind::Table, true) => rgb(0xee, 0x9d, 0x28),
        (SuggestionKind::Table, false) => rgb(0xb3, 0x6b, 0x00),
        (SuggestionKind::Keyword, _) => t.text_weak,
    }
}

/// What the popup reported this frame.
pub enum Event {
    None,
    /// The user accepted item `i` (click, or Enter/Tab routed through [`NavKeys`]).
    Accept(usize),
}

const POPUP_ROW_H: f32 = 23.0;
const POPUP_MARGIN: f32 = 4.0;
const POPUP_MAX_VISIBLE_ROWS: usize = 6;

fn popup_width(items: &[Suggestion], screen_width: f32) -> f32 {
    let content_width = items
        .iter()
        .take(24)
        .map(|item| {
            let label = item.insert.chars().count() as f32 * 7.3;
            let detail = item.detail.chars().count() as f32 * 5.7;
            40.0 + label + if detail > 0.0 { 18.0 + detail } else { 0.0 }
        })
        .fold(0.0_f32, f32::max);
    content_width
        .clamp(260.0, 340.0)
        .min((screen_width - 16.0).max(0.0))
}

fn popup_geometry(
    item_count: usize,
    anchor: egui::Rect,
    screen: egui::Rect,
    width: f32,
) -> (egui::Pos2, f32, usize) {
    let preferred_rows = item_count.clamp(1, POPUP_MAX_VISIBLE_ROWS);
    let preferred_height = preferred_rows as f32 * POPUP_ROW_H + POPUP_MARGIN * 2.0;
    let below_y = anchor.bottom() + 4.0;
    let above_bottom = anchor.top() - 4.0;
    let below_space = (screen.bottom() - 8.0 - below_y).max(0.0);
    let above_space = (above_bottom - screen.top() - 8.0).max(0.0);

    let (place_below, available) = if preferred_height <= below_space {
        (true, below_space)
    } else if preferred_height <= above_space {
        (false, above_space)
    } else {
        (below_space >= above_space, below_space.max(above_space))
    };
    let visible_rows = (((available - POPUP_MARGIN * 2.0) / POPUP_ROW_H).floor() as usize)
        .clamp(1, preferred_rows);
    let height = visible_rows as f32 * POPUP_ROW_H + POPUP_MARGIN * 2.0;
    let y = if place_below {
        below_y
    } else {
        above_bottom - height
    };
    let x = anchor
        .left()
        .min(screen.right() - width - 8.0)
        .max(screen.left() + 4.0);
    (egui::pos2(x, y), height, visible_rows)
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
    let screen = ctx.content_rect();
    let width = popup_width(&state.items, screen.width());
    let (pos, _height, visible_rows) = popup_geometry(state.items.len(), anchor, screen, width);

    let mut event = Event::None;
    let area = egui::Area::new(egui::Id::new("sql_autocomplete_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            // A flat panel: hairline border, no shadow, so it reads as part of the editor
            // rather than a floating card.
            egui::Frame::popup(&ctx.global_style())
                .fill(palette::PANEL())
                .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
                .shadow(egui::epaint::Shadow::NONE)
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(POPUP_MARGIN)
                .show(ui, |ui| {
                    ui.set_width(width);
                    // Rows sit flush against each other — the tight, dense list the design calls
                    // for (egui would otherwise insert `item_spacing.y` between them).
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::ScrollArea::vertical()
                        .id_salt("sql_autocomplete_scroll")
                        .max_height(visible_rows as f32 * POPUP_ROW_H)
                        .show(ui, |ui| {
                            for (i, item) in state.items.iter().enumerate() {
                                let (rect, resp) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), POPUP_ROW_H),
                                    egui::Sense::click(),
                                );
                                if !ui.is_rect_visible(rect) {
                                    continue;
                                }
                                let selected = i == state.selected;
                                if selected {
                                    ui.painter().rect_filled(rect, 3.0, palette::SELECTION());
                                } else if resp.hovered() {
                                    ui.painter()
                                        .rect_filled(rect, 3.0, palette::SURFACE_HOVER());
                                }
                                if selected && nav_moved {
                                    resp.scroll_to_me(None);
                                }

                                let icon = match item.kind {
                                    SuggestionKind::Keyword => crate::icons::code(),
                                    SuggestionKind::Function => crate::icons::suggest_function(),
                                    SuggestionKind::Table => crate::icons::suggest_table(),
                                    SuggestionKind::Column => crate::icons::suggest_column(),
                                };
                                const ICON_SIZE: f32 = 16.0;
                                let icon_rect = egui::Rect::from_center_size(
                                    egui::pos2(rect.left() + 11.0, rect.center().y),
                                    egui::Vec2::splat(ICON_SIZE),
                                );
                                egui::Image::new(icon)
                                    .fit_to_exact_size(egui::Vec2::splat(ICON_SIZE))
                                    .tint(icon_color(item.kind))
                                    .paint_at(ui, icon_rect);

                                // The label, with the run the typed prefix matched in the
                                // accent colour: the reader sees at a glance which part of
                                // each row they have already typed, and how it lines up.
                                let detail_pos = egui::pos2(rect.right() - 6.0, rect.center().y);
                                let detail_rect = ui.painter().text(
                                    detail_pos,
                                    egui::Align2::RIGHT_CENTER,
                                    &item.detail,
                                    small.clone(),
                                    palette::TEXT_FAINT(),
                                );
                                let label_clip = egui::Rect::from_min_max(
                                    rect.left_top(),
                                    egui::pos2(detail_rect.left() - 10.0, rect.bottom()),
                                );
                                let matched = matched_chars(&item.insert, &state.prefix);
                                let mut job = egui::text::LayoutJob::default();
                                for (n, c) in item.insert.chars().enumerate() {
                                    let color = if matched.contains(&n) {
                                        palette::ACCENT()
                                    } else {
                                        palette::TEXT()
                                    };
                                    job.append(
                                        c.encode_utf8(&mut [0; 4]),
                                        0.0,
                                        egui::TextFormat::simple(mono.clone(), color),
                                    );
                                }
                                let galley = ui.fonts_mut(|f| f.layout_job(job));
                                let label_pos = egui::pos2(
                                    rect.left() + 25.0,
                                    rect.center().y - galley.size().y / 2.0,
                                );
                                ui.painter().with_clip_rect(label_clip).galley(
                                    label_pos,
                                    galley,
                                    palette::TEXT(),
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

    #[test]
    fn popup_height_is_content_aware_and_capped_at_six_rows() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let anchor = egui::Rect::from_min_size(egui::pos2(100.0, 80.0), egui::vec2(2.0, 18.0));

        let (_, short_height, short_rows) = popup_geometry(2, anchor, screen, 280.0);
        assert_eq!(short_rows, 2);
        assert_eq!(short_height, 2.0 * POPUP_ROW_H + POPUP_MARGIN * 2.0);

        let (_, long_height, long_rows) = popup_geometry(30, anchor, screen, 280.0);
        assert_eq!(long_rows, POPUP_MAX_VISIBLE_ROWS);
        assert_eq!(
            long_height,
            POPUP_MAX_VISIBLE_ROWS as f32 * POPUP_ROW_H + POPUP_MARGIN * 2.0
        );
    }

    #[test]
    fn popup_moves_above_the_caret_when_space_below_is_tight() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 300.0));
        let anchor = egui::Rect::from_min_size(egui::pos2(100.0, 270.0), egui::vec2(2.0, 18.0));
        let (position, _, rows) = popup_geometry(20, anchor, screen, 280.0);

        assert_eq!(rows, POPUP_MAX_VISIBLE_ROWS);
        assert!(position.y < anchor.top());
    }

    fn schema() -> SchemaTree {
        let col = |name: &str, ty: &str| ColumnInfo {
            name: name.to_string(),
            data_type: ty.to_string(),
            nullable: true,
            primary_key: false,
            default: None,
            check: None,
            comment: None,
            generated: false,
            max_length: None,
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
                    columns: vec![
                        col("id", "int"),
                        col("user_id", "int"),
                        col("total", "numeric"),
                    ],
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
    fn one_unambiguous_completion_can_be_shown_inline() {
        let c = complete("SEL", 3, None, None, false).unwrap();
        assert_eq!(inline_suffix(&c).as_deref(), Some("ECT"));
    }

    #[test]
    fn ambiguous_or_rewriting_completion_stays_in_the_popup() {
        let s = schema();
        let c = complete("SELECT ", 7, Some(&s), None, true).unwrap();
        assert!(inline_suffix(&c).is_none());

        let c = Completion {
            replace_start: 14,
            prefix: "My".to_string(),
            items: vec![Suggestion {
                insert: "\"MyTable\"".to_string(),
                detail: "table".to_string(),
                kind: SuggestionKind::Table,
                schema: None,
            }],
        };
        assert!(inline_suffix(&c).is_none());
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
    fn the_matched_characters_are_what_gets_accented() {
        // Case-insensitive, so a lowercase prefix still highlights an uppercase keyword.
        assert_eq!(matched_chars("SELECT", "sel"), vec![0, 1, 2]);
        assert_eq!(matched_chars("country", "co"), vec![0, 1]);
        // A quoted suggestion counts its opening quote into a leading run.
        assert_eq!(matched_chars("\"MyCol\"", "my"), vec![0, 1, 2]);
        assert_eq!(matched_chars("`order`", "or"), vec![0, 1, 2]);
        // Inner and scattered matches light up exactly the letters that matched.
        assert_eq!(matched_chars("xprogram", "prog"), vec![1, 2, 3, 4]);
        assert_eq!(matched_chars("enum_first", "emf"), vec![0, 3, 5]);
        // Nothing typed, or nothing matching, accents nothing.
        assert!(matched_chars("country", "").is_empty());
        assert!(matched_chars("country", "zz").is_empty());
        // Indices are chars, not bytes: Thai is 3 bytes a glyph.
        assert_eq!(matched_chars("\"ลูกค้า\"", "ลูก"), vec![0, 1, 2, 3]);
    }

    #[test]
    fn closer_matches_rank_first() {
        assert_eq!(match_tier("program", "prog"), Some(MatchTier::Prefix));
        assert_eq!(match_tier("x_program", "prog"), Some(MatchTier::WordStart));
        assert_eq!(
            match_tier("customerName", "name"),
            Some(MatchTier::WordStart)
        );
        assert_eq!(match_tier("xprogram", "prog"), Some(MatchTier::Substring));
        assert_eq!(match_tier("enum_first", "emf"), Some(MatchTier::Fuzzy));
        // A fully typed name has nothing to complete; a lone letter only matches at a word
        // start; a scattered match must begin at the first letter.
        assert_eq!(match_tier("program", "program"), None);
        assert_eq!(match_tier("xprogram", "p"), None);
        assert_eq!(match_tier("program", "rgm"), None);

        let mut s = schema();
        s.tables[0].name = "xprogram".into();
        s.tables[1].name = "program_log".into();
        let sql = "SELECT * FROM prog";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["program_log", "xprogram"]);
    }

    #[test]
    fn views_and_routines_are_suggested() {
        let mut s = schema();
        s.views.push(dbcore::ViewInfo {
            schema: Some("public".into()),
            name: "active_users".into(),
            columns: vec![s.tables[0].columns[1].clone()],
            definition: String::new(),
            materialized: false,
        });
        s.routines.push(dbcore::RoutineInfo {
            schema: Some("public".into()),
            name: "refresh_totals".into(),
            kind: dbcore::RoutineKind::Procedure,
            params: Vec::new(),
            return_type: None,
            language: String::new(),
            body: String::new(),
        });

        let sql = "SELECT * FROM act";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["active_users"]);
        assert_eq!(c.items[0].detail, "public · view");

        // A view's columns resolve through its name like a table's.
        let sql = "SELECT active_users. FROM active_users";
        let c = complete(sql, 20, Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["email"]);

        // EXEC / CALL offer routines only.
        let sql = "EXEC ref";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(labels(&c), vec!["refresh_totals"]);
        assert_eq!(c.items[0].kind, SuggestionKind::Function);
    }

    #[test]
    fn editor_options_shape_the_list() {
        let s = schema();
        let options = |f: fn(&mut EditorOptions)| {
            let mut o = EditorOptions::default();
            f(&mut o);
            o
        };
        // Turning a kind off removes it; the rest stay.
        let sql = "SELECT * FROM users WHERE e";
        let at = sql.chars().count();
        let no_columns = options(|o| o.suggest_columns = false);
        let c = complete_with(sql, at, Some(&s), None, false, &no_columns).unwrap();
        assert!(c.items.iter().all(|i| i.kind != SuggestionKind::Column));
        assert!(c.items.iter().any(|i| i.kind == SuggestionKind::Keyword));

        // Keywords and built-in functions follow the case preference; schema names don't.
        let lower = options(|o| o.uppercase_keywords = false);
        let c = complete_with("sel", 3, Some(&s), None, false, &lower).unwrap();
        assert_eq!(c.items[0].insert, "select");
        let c = complete_with("SELECT cou", 10, None, None, false, &lower).unwrap();
        assert!(c.items.iter().any(|i| i.insert == "count"));

        // "Prefix schema names" qualifies a table on insertion — not after a typed dot.
        let qualified = options(|o| o.prefix_schema = true);
        let sql = "SELECT * FROM us";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(
            insertion_text(&c.items[0], false, None, &qualified),
            "public.users"
        );
        assert_eq!(insertion_text(&c.items[0], true, None, &qualified), "users");
        assert_eq!(
            insertion_text(&c.items[0], false, None, &EditorOptions::default()),
            "users"
        );
    }

    #[test]
    fn the_selected_row_previews_inline() {
        let item = |insert: &str| Suggestion {
            insert: insert.to_string(),
            detail: String::new(),
            kind: SuggestionKind::Table,
            schema: None,
        };
        let mut state = State {
            open: true,
            items: vec![item("xprogram"), item("xpgroup"), item("\"XpType\"")],
            prefix: "xp".into(),
            ..State::default()
        };
        assert_eq!(selected_tail(&state).as_deref(), Some("rogram"));
        // A row that doesn't simply extend what was typed can't be previewed by appending.
        state.selected = 2;
        assert_eq!(selected_tail(&state), None);
        state.prefix = "prog".into();
        state.selected = 0;
        assert_eq!(selected_tail(&state), None);
    }

    #[test]
    fn the_completion_reports_the_prefix_it_matched() {
        let s = schema();
        let sql = "SELECT * FROM us";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(c.prefix, "us");
        // A typed quote is not part of the prefix — schema names are matched unquoted.
        let sql = "SELECT * FROM \"us";
        let c = complete(sql, sql.chars().count(), Some(&s), None, false).unwrap();
        assert_eq!(c.prefix, "us");
    }

    #[test]
    fn keywords_without_connection() {
        let c = complete("SEL", 3, None, None, false).unwrap();
        assert_eq!(c.items[0].insert, "SELECT");
    }

    #[test]
    fn functions_are_suggested_in_expression_context() {
        let c = complete("SELECT COU", 10, None, None, false).unwrap();
        assert!(c
            .items
            .iter()
            .any(|item| item.insert == "COUNT" && item.detail == "function"));
    }

    #[test]
    fn ctes_are_suggested_as_tables_without_a_connection() {
        let sql = "WITH recent AS (SELECT * FROM orders) SELECT * FROM rec";
        let c = complete(sql, sql.chars().count(), None, None, false).unwrap();
        assert!(c
            .items
            .iter()
            .any(|item| item.insert == "recent" && item.detail == "CTE"));
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
        let c = complete(
            sql,
            sql.chars().count(),
            Some(&s),
            Some(DbKind::Postgres),
            false,
        )
        .unwrap();
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
            assert!(
                c.replace_start > quote_at,
                "must not absorb a string's quote"
            );
        }
    }

    #[test]
    fn brackets_quote_identifiers_on_sql_server() {
        let s = thai_schema();
        assert_eq!(
            accept("SELECT * FROM [ลูก", &s, Some(DbKind::SqlServer), "ลูกค้า"),
            "SELECT * FROM [ลูกค้า]"
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
                    default: None,
                    check: None,
                    comment: None,
                    generated: false,
                    max_length: None,
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
                        default: None,
                        check: None,
                        comment: None,
                        generated: false,
                        max_length: None,
                    },
                    ColumnInfo {
                        name: "อีเมล".to_string(),
                        data_type: "text".to_string(),
                        nullable: true,
                        primary_key: false,
                        default: None,
                        check: None,
                        comment: None,
                        generated: false,
                        max_length: None,
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

    /// Render the popup with a mix of all four kinds under `theme_key`, so the icon rail's
    /// colour and glyph legibility can be judged at the size it actually ships at.
    fn render_popup_snapshot(theme_key: &str, name: &str) {
        let item = |insert: &str, detail: &str, kind: SuggestionKind| Suggestion {
            insert: insert.to_string(),
            detail: detail.to_string(),
            kind,
            schema: None,
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
                item("SUM", "function", SuggestionKind::Function),
            ],
            replace_start: 0,
            // Mixed on purpose: `or…` matches some rows and not others, so the snapshot
            // shows both the accented prefix run and a plain label.
            prefix: "or".to_string(),
            ..State::default()
        };
        let theme = crate::theme::ThemeRegistry::load().theme_of(theme_key);
        let mut setup = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(420.0, 200.0))
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
        render_popup_snapshot("carbon", "autocomplete_popup");
    }

    /// Screenshot generator (ignored): the same popup on the light theme, where the kind
    /// hues have to hold up against a white panel.
    #[test]
    #[ignore = "screenshot generator; run manually with --ignored"]
    fn snapshot_popup_light() {
        render_popup_snapshot("daylight", "autocomplete_popup_light");
    }
}
