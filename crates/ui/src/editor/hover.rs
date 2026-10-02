//! What the editor says when the pointer rests on a table or column name: its type, key
//! role, nullability, default, comment and foreign keys, straight from the connected schema.
//! Pure over `(text, char index, schema)` so it's testable without a window.

use std::ops::Range;

use dbcore::{ColumnInfo, SchemaTree};

use crate::sqlctx::{
    ident_before, in_string_or_comment, is_ident_char, referenced_tables, statement_range,
};

/// Columns listed in a table's hover before the rest is summarised.
const MAX_COLUMNS: usize = 12;

/// Tooltip content for one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    /// Char range of the name under the pointer.
    pub range: Range<usize>,
    pub title: String,
    pub lines: Vec<String>,
}

/// The catalog entry a name resolved to, borrowed from the schema.
struct Entry<'a> {
    schema: Option<&'a str>,
    name: &'a str,
    columns: &'a [ColumnInfo],
    foreign_keys: &'a [dbcore::ForeignKeyInfo],
    is_view: bool,
}

fn entries(schema: &SchemaTree) -> Vec<Entry<'_>> {
    let tables = schema.tables.iter().map(|t| Entry {
        schema: t.schema.as_deref(),
        name: &t.name,
        columns: &t.columns,
        foreign_keys: &t.foreign_keys,
        is_view: false,
    });
    let views = schema.views.iter().map(|v| Entry {
        schema: v.schema.as_deref(),
        name: &v.name,
        columns: &v.columns,
        foreign_keys: &[],
        is_view: true,
    });
    tables.chain(views).collect()
}

/// Describe the name at char index `at` of `sql`, or `None` when the pointer is on anything
/// else (a keyword, a string, whitespace) or the name isn't in the schema.
pub fn describe_at(sql: &str, at: usize, schema: &SchemaTree) -> Option<Hover> {
    let chars: Vec<char> = sql.chars().collect();
    if !chars.get(at).copied().is_some_and(is_ident_char) || in_string_or_comment(&chars, at) {
        return None;
    }
    let mut start = at;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    let word: String = chars[start..end].iter().collect();
    let qualifier = (start > 0 && chars[start - 1] == '.')
        .then(|| ident_before(&chars, start - 1))
        .flatten();

    let entries = entries(schema);
    let find = |name: &str| entries.iter().find(|e| e.name.eq_ignore_ascii_case(name));
    // Tables this statement reads, so an alias or a bare column resolves to its table.
    let scope = {
        let range = statement_range(&chars, at);
        referenced_tables(&chars[range])
    };
    let resolve_alias = |name: &str| {
        scope
            .iter()
            .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
            .map(|(_, table)| table.as_str())
    };

    let hover = |title: String, lines: Vec<String>| Hover {
        range: start..end,
        title,
        lines,
    };

    if let Some(qualifier) = qualifier {
        let table = resolve_alias(&qualifier).unwrap_or(&qualifier);
        if let Some(entry) = find(table) {
            if let Some(column) = entry
                .columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&word))
            {
                return Some(column_hover(entry, column, hover));
            }
        }
        // `schema.table`: the word is the table itself.
        if let Some(entry) = find(&word) {
            return Some(table_hover(entry, hover));
        }
        return None;
    }

    // An alias or a table name.
    let table_name = resolve_alias(&word).unwrap_or(&word);
    if let Some(entry) = find(table_name) {
        return Some(table_hover(entry, hover));
    }
    // A bare column: only if exactly one table in scope has it.
    let mut owners = scope
        .iter()
        .filter_map(|(_, table)| find(table))
        .filter(|e| e.columns.iter().any(|c| c.name.eq_ignore_ascii_case(&word)));
    let owner = owners.next()?;
    if owners.any(|other| other.name != owner.name) {
        return None;
    }
    let column = owner
        .columns
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(&word))?;
    Some(column_hover(owner, column, hover))
}

fn qualified(entry: &Entry<'_>) -> String {
    match entry.schema {
        Some(schema) => format!("{schema}.{}", entry.name),
        None => entry.name.to_string(),
    }
}

fn column_hover(
    entry: &Entry<'_>,
    column: &ColumnInfo,
    make: impl FnOnce(String, Vec<String>) -> Hover,
) -> Hover {
    let mut facts = vec![column.data_type.clone()];
    if column.primary_key {
        facts.push("primary key".into());
    }
    facts.push(
        if column.nullable {
            "nullable"
        } else {
            "NOT NULL"
        }
        .into(),
    );
    if column.generated {
        facts.push("generated".into());
    }
    let mut lines = vec![facts.join(" · ")];
    if let Some(default) = column.default.as_deref().filter(|d| !d.is_empty()) {
        lines.push(format!("default {default}"));
    }
    for fk in entry.foreign_keys.iter().filter(|fk| {
        fk.columns
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&column.name))
    }) {
        let position = fk
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&column.name));
        let target = position.and_then(|p| fk.ref_columns.get(p));
        lines.push(match target {
            Some(target) => format!("→ {}.{}", fk.ref_table, target),
            None => format!("→ {}", fk.ref_table),
        });
    }
    if let Some(check) = column.check.as_deref().filter(|c| !c.is_empty()) {
        lines.push(format!("check {check}"));
    }
    if let Some(comment) = column.comment.as_deref().filter(|c| !c.is_empty()) {
        lines.push(comment.to_string());
    }
    make(format!("{}.{}", qualified(entry), column.name), lines)
}

fn table_hover(entry: &Entry<'_>, make: impl FnOnce(String, Vec<String>) -> Hover) -> Hover {
    let kind = if entry.is_view { "View" } else { "Table" };
    let mut lines = vec![format!("{kind} · {} columns", entry.columns.len())];
    for column in entry.columns.iter().take(MAX_COLUMNS) {
        let key = if column.primary_key { "  PK" } else { "" };
        lines.push(format!("{}  {}{key}", column.name, column.data_type));
    }
    if entry.columns.len() > MAX_COLUMNS {
        lines.push(format!("… {} more", entry.columns.len() - MAX_COLUMNS));
    }
    make(qualified(entry), lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::{ForeignKeyInfo, TableInfo};

    fn column(name: &str, data_type: &str, pk: bool, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: data_type.into(),
            nullable,
            primary_key: pk,
            default: None,
            check: None,
            comment: None,
            generated: false,
            max_length: None,
        }
    }

    fn schema() -> SchemaTree {
        let mut email = column("email", "text", false, false);
        email.comment = Some("Login address".into());
        email.default = Some("''".into());
        SchemaTree {
            tables: vec![
                TableInfo {
                    schema: Some("public".into()),
                    name: "users".into(),
                    columns: vec![column("id", "integer", true, false), email],
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                },
                TableInfo {
                    schema: Some("public".into()),
                    name: "orders".into(),
                    columns: vec![
                        column("id", "integer", true, false),
                        column("user_id", "integer", false, true),
                    ],
                    indexes: Vec::new(),
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
            ],
            ..SchemaTree::default()
        }
    }

    fn at(sql: &str, needle: &str, offset: usize) -> Option<Hover> {
        let byte = sql.find(needle).unwrap() + offset;
        let index = sql[..byte].chars().count();
        describe_at(sql, index, &schema())
    }

    #[test]
    fn table_name_lists_columns_and_keys() {
        let sql = "SELECT * FROM users";
        let hover = at(sql, "users", 2).unwrap();
        assert_eq!(hover.title, "public.users");
        assert_eq!(hover.lines[0], "Table · 2 columns");
        assert!(hover.lines.contains(&"id  integer  PK".to_string()));
        assert_eq!(hover.range, 14..19);
    }

    #[test]
    fn qualified_column_resolves_through_the_alias() {
        let sql = "SELECT u.email FROM users u";
        let hover = at(sql, "email", 0).unwrap();
        assert_eq!(hover.title, "public.users.email");
        assert_eq!(hover.lines[0], "text · NOT NULL");
        assert!(hover.lines.contains(&"default ''".to_string()));
        assert!(hover.lines.contains(&"Login address".to_string()));
    }

    #[test]
    fn foreign_key_columns_say_where_they_point() {
        let sql = "SELECT o.user_id FROM orders o";
        let hover = at(sql, "user_id", 0).unwrap();
        assert!(hover.lines.contains(&"→ users.id".to_string()), "{hover:?}");
    }

    #[test]
    fn alias_hover_describes_its_table() {
        let sql = "SELECT u.id FROM users u";
        let hover = at(sql, "u.id", 0).unwrap();
        assert_eq!(hover.title, "public.users");
    }

    #[test]
    fn bare_column_resolves_only_when_one_table_owns_it() {
        let sql = "SELECT email FROM users";
        assert_eq!(at(sql, "email", 1).unwrap().title, "public.users.email");
        // `id` exists in both tables of the join: ambiguous, so no guess.
        let sql = "SELECT id FROM users u JOIN orders o ON o.user_id = u.id";
        assert!(at(sql, "id FROM", 0).is_none());
    }

    #[test]
    fn keywords_strings_comments_and_unknowns_are_silent() {
        let sql = "SELECT 'users' FROM users -- users\nWHERE zzz = 1";
        assert!(at(sql, "SELECT", 1).is_none());
        assert!(at(sql, "'users'", 2).is_none());
        assert!(at(sql, "-- users", 4).is_none());
        assert!(at(sql, "zzz", 0).is_none());
        assert!(at(sql, " FROM", 0).is_none(), "whitespace");
    }

    #[test]
    fn positions_count_chars_not_bytes() {
        let sql = "SELECT 'ก', email FROM users";
        assert_eq!(at(sql, "email", 2).unwrap().title, "public.users.email");
    }
}
