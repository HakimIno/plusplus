//! Schema-aware checks for the SQL editor: names that parse fine but don't exist.
//!
//! [`crate::syntax`] answers "is this SQL?"; this answers "does it name things the connected
//! database has?". It catches the typo that otherwise costs a round trip to the server
//! (`FROM usres`, `u.emial`). The rule that shapes everything here: **a squiggle under valid
//! SQL is worse than none**. The catalog can be stale, partial, or blind to session state, so
//! a name is flagged only when the text leaves no other reading:
//!
//! - tables: `SELECT`/`INSERT`/`UPDATE`/`DELETE` only, never `CREATE`/`ALTER`/`DROP`;
//! - a qualified column (`alias.col`) only when `alias` resolves to exactly one catalog table;
//! - a bare column only in a one-table `SELECT` (no joins, CTEs or subqueries), where it can
//!   only mean that table's column or one of the select list's own aliases.

use std::collections::HashSet;
use std::ops::{ControlFlow, Range};

use sqlparser::ast::{
    Expr, Ident, ObjectName, ObjectNamePart, Query, SelectItem, SetExpr, Statement, TableFactor,
    Visit, Visitor,
};
use sqlparser::parser::Parser;

use crate::model::{ColumnInfo, DbKind, SchemaTree};
use crate::syntax::{char_index, dialect_for, go_lines_as_separators};

/// What kind of name was not found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    Table,
    Column,
}

/// A name in the SQL that the connected database's schema doesn't contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticIssue {
    pub kind: IssueKind,
    /// Char range (not bytes) of the offending identifier. Always non-empty.
    pub range: Range<usize>,
    /// One-line explanation for a tooltip, including a "did you mean" when one is close.
    pub message: String,
}

/// Names the catalog never lists but every database accepts.
const PSEUDO_COLUMNS: &[&str] = &[
    "rowid", "_rowid_", "oid", "ctid", "xmin", "xmax", "cmin", "cmax", "tableoid", "rownum",
    "level",
];

/// A catalog table or view, as far as name resolution needs it.
struct Source<'a> {
    schema: Option<&'a str>,
    name: &'a str,
    columns: &'a [ColumnInfo],
}

fn sources(schema: &SchemaTree) -> Vec<Source<'_>> {
    let tables = schema.tables.iter().map(|t| Source {
        schema: t.schema.as_deref(),
        name: &t.name,
        columns: &t.columns,
    });
    let views = schema.views.iter().map(|v| Source {
        schema: v.schema.as_deref(),
        name: &v.name,
        columns: &v.columns,
    });
    tables.chain(views).collect()
}

/// Every unknown table or column in `sql`, in source order. Empty when the SQL doesn't parse
/// (the syntax check owns that), when the catalog is empty (still loading), and always for
/// backends this can't judge (no connection, CQL).
pub fn check_semantics(kind: Option<DbKind>, sql: &str, schema: &SchemaTree) -> Vec<SemanticIssue> {
    let Some(kind) = kind else {
        return Vec::new();
    };
    if matches!(kind, DbKind::Cassandra | DbKind::ScyllaDb)
        || (schema.tables.is_empty() && schema.views.is_empty())
    {
        return Vec::new();
    }
    let batches;
    let text = if kind == DbKind::SqlServer {
        batches = go_lines_as_separators(sql);
        batches.as_str()
    } else {
        sql
    };
    let dialect = dialect_for(Some(kind));
    let Ok(statements) = Parser::new(dialect.as_ref())
        .with_recursion_limit(128)
        .try_with_sql(text)
        .and_then(|mut parser| parser.parse_statements())
    else {
        return Vec::new();
    };

    let sources = sources(schema);
    // Objects the batch itself creates (temp tables, a view defined two lines up) exist for
    // the statements after them even though the catalog hasn't seen them yet.
    let created: HashSet<String> = statements.iter().filter_map(created_name).collect();

    let mut issues = Vec::new();
    for statement in &statements {
        if !matches!(
            statement,
            Statement::Query(_)
                | Statement::Insert(_)
                | Statement::Update { .. }
                | Statement::Delete(_)
        ) {
            continue;
        }
        let mut scan = Scan::default();
        let _ = statement.visit(&mut scan);
        resolve(kind, statement, &scan, &sources, &created, sql, &mut issues);
    }
    issues.sort_by_key(|issue| issue.range.start);
    issues.dedup_by(|a, b| a.range == b.range);
    issues
}

fn created_name(statement: &Statement) -> Option<String> {
    let name = match statement {
        Statement::CreateTable(create) => &create.name,
        Statement::CreateView(create) => &create.name,
        _ => return None,
    };
    last_ident(name).map(|ident| ident.value.to_lowercase())
}

fn last_ident(name: &ObjectName) -> Option<&Ident> {
    match name.0.last()? {
        ObjectNamePart::Identifier(ident) => Some(ident),
        ObjectNamePart::Function(_) => None,
    }
}

/// What one statement mentions, gathered in a single AST walk.
#[derive(Default)]
struct Scan {
    /// Every relation name, with the start of its last identifier (to match table functions).
    relations: Vec<ObjectName>,
    /// Starts of relations that are table-valued functions, not tables.
    functions: HashSet<(u64, u64)>,
    /// `(binding, relation)`: the name a `FROM` item answers to. `None` relation = derived
    /// table, function or anything else whose columns aren't in the catalog.
    bindings: Vec<(String, Option<ObjectName>)>,
    ctes: HashSet<String>,
    queries: usize,
    /// Column references: the optional qualifier, then the column identifier.
    columns: Vec<(Option<Ident>, Ident)>,
}

impl Visitor for Scan {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        self.queries += 1;
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.ctes.insert(cte.alias.name.value.to_lowercase());
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<()> {
        match factor {
            TableFactor::Table {
                name, alias, args, ..
            } => {
                let binding = alias
                    .as_ref()
                    .map(|a| a.name.value.clone())
                    .or_else(|| last_ident(name).map(|i| i.value.clone()));
                if args.is_some() {
                    if let Some(ident) = last_ident(name) {
                        self.functions
                            .insert((ident.span.start.line, ident.span.start.column));
                    }
                    if let Some(binding) = binding {
                        self.bindings.push((binding, None));
                    }
                } else if let Some(binding) = binding {
                    self.bindings.push((binding, Some(name.clone())));
                }
            }
            other => {
                if let Some(alias) = factor_alias(other) {
                    self.bindings.push((alias.to_string(), None));
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<()> {
        self.relations.push(relation.clone());
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        match expr {
            Expr::Identifier(ident) => self.columns.push((None, ident.clone())),
            Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
                self.columns
                    .push((Some(parts[0].clone()), parts[1].clone()));
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

fn factor_alias(factor: &TableFactor) -> Option<&str> {
    match factor {
        TableFactor::Derived { alias, .. }
        | TableFactor::TableFunction { alias, .. }
        | TableFactor::Function { alias, .. }
        | TableFactor::UNNEST { alias, .. }
        | TableFactor::NestedJoin { alias, .. } => alias.as_ref().map(|a| a.name.value.as_str()),
        _ => None,
    }
}

fn find<'a>(sources: &'a [Source<'a>], schema: Option<&str>, name: &str) -> Option<&'a Source<'a>> {
    sources.iter().find(|s| {
        s.name.eq_ignore_ascii_case(name)
            && schema.is_none_or(|wanted| {
                s.schema
                    .is_none_or(|actual| actual.eq_ignore_ascii_case(wanted))
            })
    })
}

/// `(schema, table)` of a relation name, or `None` for forms this can't judge (a three-part
/// cross-database name, a function-valued part).
fn split_name(name: &ObjectName) -> Option<(Option<&Ident>, &Ident)> {
    let idents: Vec<&Ident> = name
        .0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(ident) => Some(ident),
            ObjectNamePart::Function(_) => None,
        })
        .collect::<Option<_>>()?;
    match idents.as_slice() {
        [table] => Some((None, table)),
        [schema, table] => Some((Some(schema), table)),
        _ => None,
    }
}

fn range_of(sql: &str, ident: &Ident) -> Option<Range<usize>> {
    if ident.span.start.line == 0 || ident.span.end <= ident.span.start {
        return None;
    }
    let start = char_index(sql, ident.span.start);
    let end = char_index(sql, ident.span.end);
    (start < end).then_some(start..end)
}

fn resolve(
    kind: DbKind,
    statement: &Statement,
    scan: &Scan,
    sources: &[Source<'_>],
    created: &HashSet<String>,
    sql: &str,
    issues: &mut Vec<SemanticIssue>,
) {
    let known_schema = |schema: &Ident| {
        sources.iter().any(|s| {
            s.schema
                .is_some_and(|x| x.eq_ignore_ascii_case(&schema.value))
        })
    };

    // Tables.
    for relation in &scan.relations {
        let Some((schema, table)) = split_name(relation) else {
            continue;
        };
        if scan
            .functions
            .contains(&(table.span.start.line, table.span.start.column))
        {
            continue;
        }
        let lower = table.value.to_lowercase();
        let system = lower.starts_with("sqlite_")
            || lower.starts_with("pg_")
            || lower.starts_with('#')
            || lower.starts_with('@')
            || lower == "dual";
        let own = schema.is_none() && (scan.ctes.contains(&lower) || created.contains(&lower));
        // A qualifier the catalog has never heard of is a database we haven't loaded
        // (`information_schema`, `sys`, another schema), not a typo we can vouch for.
        let schema_unknown = schema.is_some_and(|s| !known_schema(s));
        if system || own || schema_unknown || created.contains(&lower) {
            continue;
        }
        if find(sources, schema.map(|s| s.value.as_str()), &table.value).is_some() {
            continue;
        }
        let Some(range) = range_of(sql, table) else {
            continue;
        };
        let candidates = sources
            .iter()
            .filter(|s| {
                schema.is_none_or(|q| s.schema.is_some_and(|x| x.eq_ignore_ascii_case(&q.value)))
            })
            .map(|s| s.name);
        issues.push(SemanticIssue {
            kind: IssueKind::Table,
            range,
            message: with_suggestion(
                format!("Unknown table \"{}\".", table.value),
                &table.value,
                candidates,
            ),
        });
    }

    // Columns. Resolve each binding to a catalog source, dropping any name that appears more
    // than once with different meanings (the same alias in two subqueries).
    let mut resolved: Vec<(String, Option<&Source<'_>>)> = Vec::new();
    for (binding, relation) in &scan.bindings {
        let source = relation.as_ref().and_then(|name| {
            let (schema, table) = split_name(name)?;
            let lower = table.value.to_lowercase();
            if schema.is_none() && (scan.ctes.contains(&lower) || created.contains(&lower)) {
                return None;
            }
            find(sources, schema.map(|s| s.value.as_str()), &table.value)
        });
        match resolved
            .iter_mut()
            .find(|(b, _)| b.eq_ignore_ascii_case(binding))
        {
            Some((_, existing)) => {
                if existing.map(|s| (s.schema, s.name)) != source.map(|s| (s.schema, s.name)) {
                    *existing = None;
                }
            }
            None => resolved.push((binding.clone(), source)),
        }
    }

    let only_source = match (statement, scan.bindings.as_slice(), scan.queries) {
        (Statement::Query(query), [_], 1) if scan.ctes.is_empty() => match query.body.as_ref() {
            SetExpr::Select(_) => resolved.first().and_then(|(_, source)| *source),
            _ => None,
        },
        _ => None,
    };
    let select_aliases: HashSet<String> = match statement {
        Statement::Query(query) => match query.body.as_ref() {
            SetExpr::Select(select) => select
                .projection
                .iter()
                .filter_map(|item| match item {
                    SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.to_lowercase()),
                    _ => None,
                })
                .collect(),
            _ => HashSet::new(),
        },
        _ => HashSet::new(),
    };

    for (qualifier, column) in &scan.columns {
        let lower = column.value.to_lowercase();
        if PSEUDO_COLUMNS.contains(&lower.as_str()) || column.value.starts_with(['@', ':', '$']) {
            continue;
        }
        let source = match qualifier {
            Some(q) => resolved
                .iter()
                .find(|(b, _)| b.eq_ignore_ascii_case(&q.value))
                .and_then(|(_, source)| *source),
            None => {
                // SQLite quietly reads an unknown "double-quoted" identifier as a string.
                if kind == DbKind::Sqlite && column.quote_style.is_some() {
                    continue;
                }
                if select_aliases.contains(&lower) {
                    continue;
                }
                only_source
            }
        };
        // A view whose columns weren't introspected tells us nothing.
        let Some(source) = source.filter(|s| !s.columns.is_empty()) else {
            continue;
        };
        if source
            .columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&column.value))
        {
            continue;
        }
        let Some(range) = range_of(sql, column) else {
            continue;
        };
        issues.push(SemanticIssue {
            kind: IssueKind::Column,
            range,
            message: with_suggestion(
                format!(
                    "Unknown column \"{}\" in \"{}\".",
                    column.value, source.name
                ),
                &column.value,
                source.columns.iter().map(|c| c.name.as_str()),
            ),
        });
    }
}

fn with_suggestion<'a>(
    mut message: String,
    typed: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> String {
    let typed_lower = typed.to_lowercase();
    let limit = (typed_lower.chars().count() / 3).clamp(1, 2);
    let best = candidates
        .map(|c| (edit_distance(&typed_lower, &c.to_lowercase()), c))
        .filter(|(distance, _)| *distance <= limit)
        .min_by_key(|(distance, _)| *distance);
    if let Some((_, name)) = best {
        message.push_str(&format!(" Did you mean \"{name}\"?"));
    }
    message
}

/// Edit distance over chars (names may be Thai, so never bytes), counting a swap of two
/// adjacent chars as one edit: `usres` is one slip from `users`, not two.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TableInfo, ViewInfo};

    fn column(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: "text".into(),
            nullable: true,
            primary_key: false,
            default: None,
            check: None,
            comment: None,
            generated: false,
            max_length: None,
        }
    }

    fn table(schema: Option<&str>, name: &str, columns: &[&str]) -> TableInfo {
        TableInfo {
            schema: schema.map(Into::into),
            name: name.into(),
            columns: columns.iter().map(|c| column(c)).collect(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        }
    }

    fn catalog() -> SchemaTree {
        SchemaTree {
            tables: vec![
                table(Some("public"), "users", &["id", "name", "email"]),
                table(Some("public"), "orders", &["id", "user_id", "total"]),
                table(Some("public"), "ลูกค้า", &["id", "ชื่อ"]),
            ],
            views: vec![ViewInfo {
                schema: Some("public".into()),
                name: "active_users".into(),
                columns: vec![column("id")],
                definition: String::new(),
                materialized: false,
            }],
            ..SchemaTree::default()
        }
    }

    fn check(sql: &str) -> Vec<SemanticIssue> {
        check_semantics(Some(DbKind::Postgres), sql, &catalog())
    }

    fn marked(sql: &str, issue: &SemanticIssue) -> String {
        sql.chars()
            .skip(issue.range.start)
            .take(issue.range.len())
            .collect()
    }

    #[test]
    fn flags_unknown_table_with_suggestion() {
        let sql = "SELECT * FROM usres";
        let issues = check(sql);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::Table);
        assert_eq!(marked(sql, &issues[0]), "usres");
        assert!(issues[0].message.contains("Did you mean \"users\"?"));
    }

    #[test]
    fn known_names_are_quiet() {
        for sql in [
            "SELECT * FROM users",
            "SELECT * FROM USERS",
            "SELECT * FROM public.users u JOIN orders o ON o.user_id = u.id",
            "SELECT id FROM active_users",
            "SELECT ชื่อ FROM ลูกค้า",
            "UPDATE users SET name = 'x' WHERE id = 1",
            "INSERT INTO users (id) VALUES (1)",
            "DELETE FROM orders WHERE id = 1",
        ] {
            assert!(check(sql).is_empty(), "{sql}");
        }
    }

    #[test]
    fn dml_targets_are_checked_too() {
        assert_eq!(check("DELETE FROM ordres WHERE id = 1").len(), 1);
        assert_eq!(check("UPDATE usres SET a = 1").len(), 1);
        assert_eq!(check("INSERT INTO usres (a) VALUES (1)").len(), 1);
    }

    #[test]
    fn ctes_and_objects_created_in_the_batch_exist() {
        assert!(check("WITH recent AS (SELECT * FROM users) SELECT * FROM recent").is_empty());
        assert!(check("CREATE TEMP TABLE scratch (a int); SELECT * FROM scratch").is_empty());
    }

    #[test]
    fn ddl_is_never_judged() {
        assert!(check("CREATE TABLE brand_new (a int)").is_empty());
        assert!(check("DROP TABLE nothing_here").is_empty());
        assert!(check("ALTER TABLE nothing_here ADD COLUMN a int").is_empty());
    }

    #[test]
    fn unloaded_schemas_and_system_tables_are_not_typos() {
        assert!(check("SELECT * FROM information_schema.tables").is_empty());
        assert!(check("SELECT * FROM pg_stat_activity").is_empty());
        assert!(check("SELECT * FROM other_db.public.users").is_empty());
        assert!(check("SELECT * FROM generate_series(1, 3)").is_empty());
    }

    #[test]
    fn schema_qualifier_must_match() {
        assert_eq!(check("SELECT * FROM public.usres").len(), 1);
    }

    #[test]
    fn flags_unknown_qualified_column() {
        let sql = "SELECT u.emial FROM users u";
        let issues = check(sql);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::Column);
        assert_eq!(marked(sql, &issues[0]), "emial");
        assert!(issues[0].message.contains("Did you mean \"email\"?"));
    }

    #[test]
    fn qualified_columns_follow_the_right_alias() {
        assert!(
            check("SELECT o.total, u.name FROM users u JOIN orders o ON o.user_id = u.id")
                .is_empty()
        );
        assert_eq!(
            check("SELECT u.total FROM users u JOIN orders o ON o.user_id = u.id").len(),
            1
        );
    }

    #[test]
    fn unknown_qualifier_and_derived_tables_are_skipped() {
        assert!(check("SELECT x.anything FROM (SELECT 1 AS anything) x").is_empty());
        assert!(check("SELECT excluded.a FROM users").is_empty());
    }

    #[test]
    fn same_alias_with_different_tables_is_ambiguous_not_flagged() {
        assert!(check(
            "SELECT t.total FROM orders t WHERE EXISTS (SELECT 1 FROM users t WHERE t.name = 'a')"
        )
        .is_empty());
    }

    #[test]
    fn bare_column_in_single_table_select() {
        let sql = "SELECT nme FROM users";
        let issues = check(sql);
        assert_eq!(issues.len(), 1);
        assert_eq!(marked(sql, &issues[0]), "nme");
        assert!(issues[0].message.contains("Did you mean \"name\"?"));
        assert_eq!(check("SELECT id FROM users WHERE emial = 'a'").len(), 1);
    }

    #[test]
    fn bare_columns_are_quiet_when_scope_is_not_obvious() {
        // Joins, subqueries, CTEs and set operations make a bare name ambiguous.
        assert!(check("SELECT zzz FROM users u JOIN orders o ON o.user_id = u.id").is_empty());
        assert!(check("SELECT zzz FROM users WHERE id IN (SELECT user_id FROM orders)").is_empty());
        assert!(check("WITH a AS (SELECT 1) SELECT zzz FROM users").is_empty());
        assert!(check("SELECT zzz FROM users UNION SELECT 1").is_empty());
    }

    #[test]
    fn select_aliases_and_pseudo_columns_are_not_columns() {
        assert!(check("SELECT name AS n FROM users ORDER BY n").is_empty());
        assert!(check("SELECT ctid, xmin FROM users").is_empty());
        assert!(check("SELECT count(*) AS c FROM users GROUP BY c").is_empty());
    }

    #[test]
    fn sqlite_double_quotes_fall_back_to_strings() {
        let schema = catalog();
        assert!(
            check_semantics(Some(DbKind::Sqlite), "SELECT \"hello\" FROM users", &schema)
                .is_empty()
        );
        assert_eq!(
            check_semantics(Some(DbKind::Sqlite), "SELECT hello FROM users", &schema).len(),
            1
        );
    }

    #[test]
    fn view_without_introspected_columns_is_not_judged() {
        let mut schema = catalog();
        schema.views[0].columns.clear();
        assert!(check_semantics(
            Some(DbKind::Postgres),
            "SELECT zzz FROM active_users",
            &schema
        )
        .is_empty());
    }

    #[test]
    fn nothing_is_judged_without_a_catalog_or_a_connection() {
        assert!(check_semantics(None, "SELECT * FROM usres", &catalog()).is_empty());
        assert!(check_semantics(
            Some(DbKind::Postgres),
            "SELECT * FROM usres",
            &SchemaTree::default()
        )
        .is_empty());
        assert!(
            check_semantics(Some(DbKind::Cassandra), "SELECT * FROM usres", &catalog()).is_empty()
        );
    }

    #[test]
    fn unparsable_sql_is_left_to_the_syntax_check() {
        assert!(check("SELECT * FROM usres WHERE").is_empty());
    }

    #[test]
    fn ranges_are_in_chars_on_later_lines() {
        let sql = "SELECT 'ก'\n  FROM usres";
        let issues = check(sql);
        assert_eq!(issues.len(), 1);
        assert_eq!(marked(sql, &issues[0]), "usres");
    }

    #[test]
    fn issues_come_back_in_source_order() {
        let issues = check("SELECT * FROM usres; SELECT * FROM ordres");
        assert_eq!(issues.len(), 2);
        assert!(issues[0].range.start < issues[1].range.start);
    }
}
