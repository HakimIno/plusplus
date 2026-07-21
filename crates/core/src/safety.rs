//! Dialect-aware analysis for destructive SQL. The UI uses this to gate exact query
//! snapshots on connections marked as production behind a read-only preflight and confirmation.

use std::ops::ControlFlow;

use sqlparser::ast::{Expr, FromTable, ObjectName, Query, Statement, TableFactor, Visit, Visitor};
use sqlparser::dialect::{Dialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

use crate::database::{skip_leading_noise, split_statements};
use crate::model::{DbKind, QueryResult};

/// The destructive statement classes worth confirming before they touch production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DangerKind {
    Update,
    Delete,
    Drop,
    Truncate,
    Alter,
    Merge,
}

/// Production Guardian's final severity after static analysis and safe preflight queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    Low,
    Medium,
    Critical,
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            RiskLevel::Low => "LOW",
            RiskLevel::Medium => "MEDIUM",
            RiskLevel::Critical => "CRITICAL",
        }
    }
}

/// Small, backend-neutral subset of an optimizer plan suitable for a confirmation dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryPlanSummary {
    pub scan_type: Option<String>,
    pub estimated_rows: Option<u64>,
    pub index: Option<String>,
    pub full_scan: bool,
    pub detail: String,
}

/// Results of read-only checks performed before a destructive statement may be confirmed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductionPreflight {
    /// Exact count from a safely rewritten `SELECT COUNT(*)`, when the statement is simple.
    pub affected_rows: Option<u64>,
    pub plan: Option<QueryPlanSummary>,
    /// Timeouts and unsupported/failed checks. A failure never lowers risk.
    pub warnings: Vec<String>,
}

impl DangerKind {
    pub fn label(self) -> &'static str {
        match self {
            DangerKind::Update => "UPDATE",
            DangerKind::Delete => "DELETE",
            DangerKind::Drop => "DROP",
            DangerKind::Truncate => "TRUNCATE",
            DangerKind::Alter => "ALTER",
            DangerKind::Merge => "MERGE",
        }
    }
}

/// One destructive statement found in a batch.
#[derive(Debug, Clone)]
pub struct DangerousStatement {
    pub kind: DangerKind,
    /// An `UPDATE`/`DELETE` whose top-level AST has no `WHERE` predicate.
    pub missing_where: bool,
    /// Human-readable, unquoted object names. Multiple names are possible for DROP/TRUNCATE.
    pub targets: Vec<String>,
    /// Safe exact-count query derived from the AST, only for unambiguous single-table DML.
    pub count_sql: Option<String>,
    /// Set when dialect parsing failed and the conservative lexical fallback was used.
    pub analysis_warning: Option<String>,
    /// The statement text (trimmed), for display in the confirmation dialog.
    pub sql: String,
}

impl DangerousStatement {
    /// Static severity before the database contributes an exact count or estimated plan rows.
    pub fn base_risk(&self) -> RiskLevel {
        if self.analysis_warning.is_some()
            || self.targets.is_empty()
            || self.targets.len() > 1
            || self.missing_where
            || matches!(
                self.kind,
                DangerKind::Drop | DangerKind::Truncate | DangerKind::Alter
            )
        {
            RiskLevel::Critical
        } else {
            RiskLevel::Medium
        }
    }

    /// Final risk. Missing preflight evidence fails closed for DML that was not already critical.
    pub fn risk(&self, preflight: &ProductionPreflight) -> RiskLevel {
        let base = self.base_risk();
        if base == RiskLevel::Critical {
            return base;
        }
        let rows = preflight
            .affected_rows
            .or_else(|| preflight.plan.as_ref().and_then(|p| p.estimated_rows));
        match rows {
            Some(0..=10) if !preflight.plan.as_ref().is_some_and(|p| p.full_scan) => RiskLevel::Low,
            Some(0..=1_000) => RiskLevel::Medium,
            Some(_) | None => RiskLevel::Critical,
        }
    }

    /// Phrase required for a critical confirmation. Unknown targets deliberately require RUN.
    pub fn confirmation_phrase(&self) -> &str {
        self.targets.first().map(String::as_str).unwrap_or("RUN")
    }

    /// A safe, non-executing plan command for DML backends where EXPLAIN is unambiguous.
    pub fn explain_sql(&self, kind: DbKind) -> Option<String> {
        if self.analysis_warning.is_some()
            || !matches!(
                self.kind,
                DangerKind::Update | DangerKind::Delete | DangerKind::Merge
            )
        {
            return None;
        }
        Some(match kind {
            DbKind::Postgres => format!("EXPLAIN (FORMAT JSON) {}", self.sql),
            DbKind::MySql | DbKind::MariaDb => format!("EXPLAIN FORMAT=JSON {}", self.sql),
            DbKind::Sqlite => format!("EXPLAIN QUERY PLAN {}", self.sql),
            // SQL Server SHOWPLAN is connection-scoped. It needs a dedicated connection
            // lifecycle so a failed OFF cannot poison a pooled session; fail closed for now.
            DbKind::SqlServer => return None,
        })
    }
}

/// Scan a SQL batch and return every destructive statement in it, in order.
/// Empty means the batch is safe to run without confirmation.
pub fn dangerous_statements(kind: DbKind, sql: &str) -> Vec<DangerousStatement> {
    match parse_statements(kind, sql) {
        Ok(statements) => statements.iter().filter_map(dangerous_from_ast).collect(),
        Err(error) => {
            lexical_dangerous_statements(sql, Some(format!("AST analysis failed: {error}")))
        }
    }
}

fn parse_statements(kind: DbKind, sql: &str) -> Result<Vec<Statement>, String> {
    let dialect: Box<dyn Dialect> = match kind {
        DbKind::Postgres => Box::new(PostgreSqlDialect {}),
        DbKind::MySql | DbKind::MariaDb => Box::new(MySqlDialect {}),
        DbKind::SqlServer => Box::new(MsSqlDialect {}),
        DbKind::Sqlite => Box::new(SQLiteDialect {}),
    };
    Parser::new(dialect.as_ref())
        .with_recursion_limit(128)
        .try_with_sql(sql)
        .map_err(|e| e.to_string())?
        .parse_statements()
        .map_err(|e| e.to_string())
}

fn dangerous_from_ast(statement: &Statement) -> Option<DangerousStatement> {
    if let Statement::Explain {
        analyze: true,
        statement: inner,
        ..
    } = statement
    {
        let mut danger = dangerous_from_ast(inner)?;
        danger.sql = statement.to_string();
        danger.count_sql = None;
        danger.analysis_warning = Some(
            "EXPLAIN ANALYZE executes the nested destructive statement; preflight SQL was skipped"
                .to_string(),
        );
        return Some(danger);
    }

    let normalized = statement.to_string();
    let kind = classify_danger(&normalized)?;
    let mut targets = Vec::new();
    let mut count_sql = None;
    let missing_where = match statement {
        Statement::Update(update) => {
            if let Some((sql_name, display_name)) = table_factor_name(&update.table.relation) {
                targets.push(display_name);
                if update.table.joins.is_empty()
                    && update.from.is_none()
                    && update.order_by.is_empty()
                    && update.limit.is_none()
                {
                    if let Some(selection) = update
                        .selection
                        .as_ref()
                        .filter(|expr| count_predicate_is_safe(expr))
                    {
                        count_sql =
                            Some(format!("SELECT COUNT(*) FROM {sql_name} WHERE {selection}"));
                    }
                }
            }
            update.selection.is_none()
        }
        Statement::Delete(delete) => {
            let from = match &delete.from {
                FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from) => from,
            };
            if !delete.tables.is_empty() {
                targets.extend(delete.tables.iter().map(object_display_name));
            } else if let Some(first) = from.first() {
                if let Some((sql_name, display_name)) = table_factor_name(&first.relation) {
                    targets.push(display_name);
                    if from.len() == 1
                        && first.joins.is_empty()
                        && delete.tables.is_empty()
                        && delete.using.is_none()
                        && delete.order_by.is_empty()
                        && delete.limit.is_none()
                    {
                        if let Some(selection) = delete
                            .selection
                            .as_ref()
                            .filter(|expr| count_predicate_is_safe(expr))
                        {
                            count_sql =
                                Some(format!("SELECT COUNT(*) FROM {sql_name} WHERE {selection}"));
                        }
                    }
                }
            }
            delete.selection.is_none()
        }
        Statement::Drop { names, table, .. } => {
            targets.extend(names.iter().map(object_display_name));
            if let Some(table) = table {
                targets.push(object_display_name(table));
            }
            false
        }
        Statement::Truncate(truncate) => {
            targets.extend(
                truncate
                    .table_names
                    .iter()
                    .map(|target| object_display_name(&target.name)),
            );
            false
        }
        Statement::AlterTable(alter) => {
            targets.push(object_display_name(&alter.name));
            false
        }
        Statement::Merge(merge) => {
            if let Some((_, display_name)) = table_factor_name(&merge.table) {
                targets.push(display_name);
            }
            false
        }
        // Keep the lexical classifier's coverage for less common ALTER/DROP variants.
        _ => false,
    };
    Some(DangerousStatement {
        kind,
        missing_where,
        targets,
        count_sql,
        analysis_warning: None,
        sql: normalized,
    })
}

fn table_factor_name(factor: &TableFactor) -> Option<(String, String)> {
    match factor {
        TableFactor::Table { name, .. } => Some((name.to_string(), object_display_name(name))),
        _ => None,
    }
}

fn object_display_name(name: &ObjectName) -> String {
    name.0
        .iter()
        .filter_map(|part| part.as_ident())
        .map(|ident| ident.value.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// COUNT evaluates its predicate, unlike EXPLAIN. Restrict rewrites to expressions without
/// functions, sequences, or subqueries so preflight cannot invoke user code or volatile SQL.
fn count_predicate_is_safe(expr: &Expr) -> bool {
    struct SideEffectCheck;
    impl Visitor for SideEffectCheck {
        type Break = ();

        fn pre_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
            ControlFlow::Break(())
        }

        fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
            if matches!(expr, Expr::Function(_)) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }
    }
    expr.visit(&mut SideEffectCheck).is_continue()
}

fn lexical_dangerous_statements(
    sql: &str,
    analysis_warning: Option<String>,
) -> Vec<DangerousStatement> {
    split_statements(sql)
        .into_iter()
        .filter_map(|stmt| {
            let kind = classify_danger(stmt)?;
            let missing_where = matches!(kind, DangerKind::Update | DangerKind::Delete)
                && !contains_keyword(stmt, "where");
            Some(DangerousStatement {
                kind,
                missing_where,
                targets: Vec::new(),
                count_sql: None,
                analysis_warning: analysis_warning.clone(),
                sql: stmt.to_string(),
            })
        })
        .collect()
}

fn classify_danger(stmt: &str) -> Option<DangerKind> {
    let head = skip_leading_noise(stmt);
    let first = head
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match first.as_str() {
        "update" => Some(DangerKind::Update),
        "delete" => Some(DangerKind::Delete),
        "drop" => Some(DangerKind::Drop),
        "truncate" => Some(DangerKind::Truncate),
        "alter" => Some(DangerKind::Alter),
        "merge" => Some(DangerKind::Merge),
        "with" => first_danger_verb(stmt),
        _ => None,
    }
}

pub(crate) fn query_count(result: &QueryResult) -> Option<u64> {
    match result.rows.first()?.first()? {
        crate::Value::Int(value) => u64::try_from(*value).ok(),
        crate::Value::Float(value) if *value >= 0.0 => Some(*value as u64),
        crate::Value::Text(value) => value.parse().ok(),
        _ => None,
    }
}

/// Reduce a backend's EXPLAIN output to stable fields. Unknown shapes retain a short detail
/// string and never claim an index/full scan that was not present in the server response.
pub(crate) fn summarize_plan(kind: DbKind, result: &QueryResult) -> Option<QueryPlanSummary> {
    let text = result
        .rows
        .iter()
        .flat_map(|row| row.iter())
        .map(crate::Value::display)
        .collect::<Vec<_>>()
        .join(" | ");
    if text.is_empty() {
        return None;
    }
    let mut summary = QueryPlanSummary {
        detail: truncate_detail(&text, 240),
        ..QueryPlanSummary::default()
    };
    match kind {
        DbKind::Postgres => {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                let mut node_types = json_find_all(&json, "Node Type")
                    .into_iter()
                    .filter_map(json_string)
                    .filter(|node| node.to_ascii_lowercase().contains("scan"))
                    .collect::<Vec<_>>();
                summary.full_scan = node_types
                    .iter()
                    .any(|node| node.eq_ignore_ascii_case("Seq Scan"));
                // Surface the most dangerous access path when a ModifyTable/Join root wraps
                // several scans; otherwise use the first scan emitted by the optimizer.
                summary.scan_type = node_types
                    .iter()
                    .position(|node| node.eq_ignore_ascii_case("Seq Scan"))
                    .map(|index| node_types.remove(index))
                    .or_else(|| node_types.into_iter().next());
                // DML roots commonly report Plan Rows = 0. The largest nested estimate is a
                // conservative bound and cannot accidentally downgrade a large scan to Low.
                summary.estimated_rows = json_find_all(&json, "Plan Rows")
                    .into_iter()
                    .filter_map(json_u64)
                    .max();
                if !summary.full_scan {
                    summary.index = json_find_all(&json, "Index Name")
                        .into_iter()
                        .find_map(json_string);
                }
            }
        }
        DbKind::MySql | DbKind::MariaDb => {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                let access_types = json_find_all(&json, "access_type")
                    .into_iter()
                    .filter_map(json_string)
                    .collect::<Vec<_>>();
                summary.full_scan = access_types
                    .iter()
                    .any(|access| access.eq_ignore_ascii_case("ALL"));
                summary.scan_type = access_types
                    .iter()
                    .find(|access| access.eq_ignore_ascii_case("ALL"))
                    .or_else(|| access_types.first())
                    .map(|access| format!("{access} access"));
                summary.estimated_rows = json_find_all(&json, "rows_examined_per_scan")
                    .into_iter()
                    .chain(json_find_all(&json, "rows"))
                    .filter_map(json_u64)
                    .max();
                if !summary.full_scan {
                    summary.index = json_find_all(&json, "key")
                        .into_iter()
                        .find_map(json_string);
                }
            }
        }
        DbKind::Sqlite => {
            let upper = text.to_ascii_uppercase();
            summary.scan_type = if upper.contains("SEARCH ") {
                Some("Index search".to_string())
            } else if upper.contains("SCAN ") {
                Some("Table scan".to_string())
            } else {
                None
            };
            summary.full_scan = upper.contains("SCAN ")
                && !upper.contains("USING INDEX")
                && !upper.contains("USING COVERING INDEX");
            summary.index = extract_sqlite_index(&text);
        }
        DbKind::SqlServer => return None,
    }
    Some(summary)
}

fn json_find_all<'a>(value: &'a serde_json::Value, key: &str) -> Vec<&'a serde_json::Value> {
    let mut found = Vec::new();
    json_collect(value, key, &mut found);
    found
}

fn json_collect<'a>(
    value: &'a serde_json::Value,
    key: &str,
    found: &mut Vec<&'a serde_json::Value>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(value) = map.get(key) {
                found.push(value);
            }
            for value in map.values() {
                json_collect(value, key, found);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                json_collect(value, key, found);
            }
        }
        _ => {}
    }
}

fn json_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64))
}

fn extract_sqlite_index(detail: &str) -> Option<String> {
    let upper = detail.to_ascii_uppercase();
    for marker in ["USING COVERING INDEX ", "USING INDEX "] {
        if let Some(start) = upper.find(marker) {
            let value = &detail[start + marker.len()..];
            let end = value
                .find(|c: char| c.is_whitespace() || c == '(')
                .unwrap_or(value.len());
            if end > 0 {
                return Some(value[..end].to_string());
            }
        }
    }
    None
}

fn truncate_detail(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let short: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// The first destructive verb appearing anywhere in `stmt` (outside literals/comments),
/// used to classify a `WITH` statement by what it wraps. "First" follows the order of
/// the list below, not position in the text — UPDATE/DELETE outrank DDL for the label.
fn first_danger_verb(stmt: &str) -> Option<DangerKind> {
    [
        ("update", DangerKind::Update),
        ("delete", DangerKind::Delete),
        ("merge", DangerKind::Merge),
        ("truncate", DangerKind::Truncate),
        ("drop", DangerKind::Drop),
        ("alter", DangerKind::Alter),
    ]
    .into_iter()
    .find(|(kw, _)| contains_keyword(stmt, kw))
    .map(|(_, kind)| kind)
}

/// Verbs that modify data or schema when they appear inside an otherwise-read statement
/// (a subquery, a CTE body, the target of an `EXPLAIN`, a `SELECT … INTO`).
const EMBEDDED_WRITE_VERBS: &[&str] = &[
    "insert", "update", "delete", "merge", "truncate", "drop", "alter", "create", "grant",
    "revoke", "exec", "execute", "call", "into",
];

/// The statements in `sql` that are not provably read-only. Read-only mode refuses to run
/// a batch unless this comes back empty.
///
/// Default-deny: a statement passes only when its leading keyword is a known read
/// (`SELECT`, `SHOW`, `EXPLAIN`, …) — anything unrecognised (DML, DDL, `SET`, `GRANT`,
/// `CALL`, `COPY`, vendor-specific verbs…) is rejected. Statements that can embed other
/// statements are scanned for write verbs too, because `EXPLAIN ANALYZE UPDATE …` really
/// runs the UPDATE, `WITH x AS (DELETE …) SELECT` really deletes, and `SELECT … INTO t`
/// creates a table.
///
/// Lexical, so it can over-block exotic-but-legal SQL (e.g. MySQL's `INSERT()` string
/// function in a SELECT); over-blocking is the safe direction here. It also cannot see
/// side effects hidden in function calls (`SELECT setval(…)`), which is why the backends
/// additionally enforce read-only at the session level where the engine supports it —
/// this check exists to give a clear, local error before the server rejects the write.
pub fn write_statements(sql: &str) -> Vec<String> {
    split_statements(sql)
        .into_iter()
        .filter(|stmt| !statement_is_read_only(stmt))
        .map(|s| s.to_string())
        .collect()
}

/// Is this single statement provably read-only? See [`write_statements`].
fn statement_is_read_only(stmt: &str) -> bool {
    let head = skip_leading_noise(stmt);
    let first = head
        .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match first.as_str() {
        // Reads that can embed arbitrary expressions or whole statements.
        "select" | "with" | "explain" | "table" | "values" => !EMBEDDED_WRITE_VERBS
            .iter()
            .any(|kw| contains_keyword(stmt, kw)),
        // Metadata reads that take no subquery. (`SHOW CREATE TABLE` would trip the verb
        // scan above, which is why these skip it.)
        "show" | "describe" | "desc" | "use" => true,
        // SQLite PRAGMA: reading (`PRAGMA table_info(t)`) is fine; assignment writes.
        "pragma" => !stmt.contains('='),
        // Transaction control around reads is fine, but `START TRANSACTION READ WRITE`
        // would override the session's read-only default, so any WRITE token rejects.
        "begin" | "start" | "commit" | "rollback" | "end" => !contains_keyword(stmt, "write"),
        _ => false,
    }
}

/// Does `stmt` contain `keyword` as a standalone word, outside string literals, quoted
/// identifiers, and comments? Case-insensitive; `keyword` must be ASCII lowercase.
fn contains_keyword(stmt: &str, keyword: &str) -> bool {
    let bytes = stmt.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        match bytes[i] {
            // String literal or quoted identifier; a doubled quote escapes the delimiter.
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < n {
                    if bytes[i] == quote {
                        if i + 1 < n && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // SQL Server bracket identifier; `]]` escapes a literal `]`.
            b'[' => {
                i += 1;
                while i < n {
                    if bytes[i] == b']' {
                        if i + 1 < n && bytes[i + 1] == b']' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // Line comment, runs to end of line.
            b'-' if i + 1 < n && bytes[i + 1] == b'-' => {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // Block comment (T-SQL allows nesting).
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                let mut depth = 1u32;
                while i < n && depth > 0 {
                    if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if stmt[start..i].eq_ignore_ascii_case(keyword) {
                    return true;
                }
            }
            _ => i += 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dangerous(sql: &str) -> Vec<DangerousStatement> {
        dangerous_statements(DbKind::Postgres, sql)
    }

    fn kinds(sql: &str) -> Vec<DangerKind> {
        dangerous(sql).iter().map(|d| d.kind).collect()
    }

    #[test]
    fn selects_are_safe() {
        assert!(dangerous("SELECT * FROM users").is_empty());
        assert!(dangerous("WITH c AS (SELECT 1) SELECT * FROM c").is_empty());
        assert!(dangerous("INSERT INTO t VALUES (1)").is_empty());
    }

    #[test]
    fn classifies_each_destructive_kind() {
        assert_eq!(
            kinds("UPDATE t SET a = 1 WHERE id = 1"),
            [DangerKind::Update]
        );
        assert_eq!(kinds("delete from t where id = 1"), [DangerKind::Delete]);
        assert_eq!(kinds("DROP TABLE t"), [DangerKind::Drop]);
        assert_eq!(kinds("TRUNCATE TABLE t"), [DangerKind::Truncate]);
        assert_eq!(kinds("ALTER TABLE t ADD c INT"), [DangerKind::Alter]);
    }

    #[test]
    fn flags_update_delete_without_where() {
        let found = dangerous("DELETE FROM t");
        assert!(found[0].missing_where);
        let found = dangerous("UPDATE t SET a = 1");
        assert!(found[0].missing_where);
        let found = dangerous("UPDATE t SET a = 1 WHERE id = 3");
        assert!(!found[0].missing_where);
        // DROP has no WHERE concept — never flagged for it.
        let found = dangerous("DROP TABLE t");
        assert!(!found[0].missing_where);
    }

    #[test]
    fn ast_does_not_mistake_a_subquery_where_for_the_dml_where() {
        let found =
            dangerous("UPDATE users SET score = (SELECT max(score) FROM archive WHERE active)");
        assert!(found[0].missing_where);
        assert_eq!(found[0].targets, ["users"]);
        assert_eq!(found[0].base_risk(), RiskLevel::Critical);
    }

    #[test]
    fn exact_count_is_only_built_for_simple_non_volatile_predicates() {
        let found = dangerous("DELETE FROM public.users WHERE id = 7 AND active = true");
        assert_eq!(found[0].targets, ["public.users"]);
        assert_eq!(
            found[0].count_sql.as_deref(),
            Some("SELECT COUNT(*) FROM public.users WHERE id = 7 AND active = true")
        );

        let found = dangerous("DELETE FROM users WHERE created_at < now()");
        assert!(
            found[0].count_sql.is_none(),
            "volatile functions must not be evaluated"
        );

        let found = dangerous("DELETE FROM users WHERE id IN (SELECT id FROM stale_users)");
        assert!(
            found[0].count_sql.is_none(),
            "subqueries must not be evaluated"
        );
    }

    #[test]
    fn parse_failure_falls_back_and_fails_closed() {
        let found = dangerous("UPDATE users SET name =");
        assert_eq!(found.len(), 1);
        assert!(found[0].analysis_warning.is_some());
        assert!(found[0].targets.is_empty());
        assert_eq!(found[0].base_risk(), RiskLevel::Critical);
        assert_eq!(found[0].confirmation_phrase(), "RUN");
    }

    #[test]
    fn explain_analyze_dml_is_guarded_without_running_another_preflight_explain() {
        assert!(dangerous("EXPLAIN UPDATE users SET active = false").is_empty());

        let found = dangerous("EXPLAIN ANALYZE UPDATE users SET active = false WHERE id = 7");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, DangerKind::Update);
        assert_eq!(found[0].targets, ["users"]);
        assert_eq!(found[0].base_risk(), RiskLevel::Critical);
        assert!(found[0].count_sql.is_none());
        assert!(found[0].explain_sql(DbKind::Postgres).is_none());
    }

    #[test]
    fn preflight_evidence_can_lower_or_raise_scoped_dml_risk() {
        let statement = dangerous("UPDATE users SET active = false WHERE id = 7").remove(0);
        assert_eq!(
            statement.risk(&ProductionPreflight::default()),
            RiskLevel::Critical
        );
        assert_eq!(
            statement.risk(&ProductionPreflight {
                affected_rows: Some(1),
                ..ProductionPreflight::default()
            }),
            RiskLevel::Low
        );
        assert_eq!(
            statement.risk(&ProductionPreflight {
                affected_rows: Some(1_001),
                ..ProductionPreflight::default()
            }),
            RiskLevel::Critical
        );
    }

    #[test]
    fn postgres_json_plan_is_reduced_to_guardian_fields() {
        let result = QueryResult {
            rows: vec![vec![crate::Value::Text(
                r#"[{"Plan":{"Node Type":"Index Scan","Plan Rows":4,"Index Name":"users_pkey"}}]"#
                    .to_string(),
            )]],
            ..QueryResult::default()
        };
        let plan = summarize_plan(DbKind::Postgres, &result).unwrap();
        assert_eq!(plan.scan_type.as_deref(), Some("Index Scan"));
        assert_eq!(plan.estimated_rows, Some(4));
        assert_eq!(plan.index.as_deref(), Some("users_pkey"));
        assert!(!plan.full_scan);
    }

    #[test]
    fn postgres_dml_plan_uses_nested_scan_instead_of_zero_row_modify_root() {
        let result = QueryResult {
            rows: vec![vec![crate::Value::Text(
                r#"[{"Plan":{"Node Type":"ModifyTable","Plan Rows":0,"Plans":[{"Node Type":"Seq Scan","Plan Rows":42000}]}}]"#
                    .to_string(),
            )]],
            ..QueryResult::default()
        };
        let plan = summarize_plan(DbKind::Postgres, &result).unwrap();
        assert_eq!(plan.scan_type.as_deref(), Some("Seq Scan"));
        assert_eq!(plan.estimated_rows, Some(42_000));
        assert!(plan.full_scan);
    }

    #[test]
    fn mysql_nested_plan_uses_the_most_conservative_access_path() {
        let result = QueryResult {
            rows: vec![vec![crate::Value::Text(
                r#"{"query_block":{"nested_loop":[{"table":{"access_type":"ref","rows_examined_per_scan":4,"key":"idx_user"}},{"table":{"access_type":"ALL","rows_examined_per_scan":9000}}]}}"#
                    .to_string(),
            )]],
            ..QueryResult::default()
        };
        let plan = summarize_plan(DbKind::MySql, &result).unwrap();
        assert_eq!(plan.scan_type.as_deref(), Some("ALL access"));
        assert_eq!(plan.estimated_rows, Some(9_000));
        assert_eq!(
            plan.index, None,
            "an index on another branch must not mask ALL access"
        );
        assert!(plan.full_scan);
    }

    #[test]
    fn where_inside_literal_or_comment_does_not_count() {
        let found = dangerous("DELETE FROM t -- where id = 1");
        assert!(found[0].missing_where);
        let found = dangerous("UPDATE t SET a = 'where'");
        assert!(found[0].missing_where);
        let found = dangerous_statements(DbKind::SqlServer, "UPDATE [where] SET a = 1");
        assert!(found[0].missing_where);
        // `WHEREabouts` is a different word, not a WHERE.
        let found = dangerous("DELETE FROM whereabouts_x");
        assert!(found[0].missing_where);
    }

    #[test]
    fn scans_every_statement_in_a_batch() {
        let found =
            dangerous("SELECT 1; UPDATE t SET a = 1; DELETE FROM u WHERE id = 2; DROP TABLE v");
        assert_eq!(
            found.iter().map(|d| d.kind).collect::<Vec<_>>(),
            [DangerKind::Update, DangerKind::Delete, DangerKind::Drop]
        );
        assert!(found[0].missing_where);
        assert!(!found[1].missing_where);
    }

    #[test]
    fn leading_comments_do_not_hide_danger() {
        assert_eq!(kinds("-- cleanup\nDROP TABLE t"), [DangerKind::Drop]);
        assert_eq!(kinds("/* x */ TRUNCATE t"), [DangerKind::Truncate]);
        // ...and a destructive keyword inside a comment is not a statement.
        assert!(dangerous("-- DROP TABLE t\nSELECT 1").is_empty());
    }

    #[test]
    fn cte_wrapped_dml_is_dangerous() {
        // The main statement after the CTE list…
        assert_eq!(
            kinds("WITH old AS (SELECT id FROM t WHERE ts < now()) DELETE FROM t USING old WHERE t.id = old.id"),
            [DangerKind::Delete]
        );
        // …and Postgres' data-modifying CTE bodies.
        assert_eq!(
            kinds("WITH gone AS (DELETE FROM t RETURNING *) SELECT count(*) FROM gone"),
            [DangerKind::Delete]
        );
        assert_eq!(
            kinds("MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = 1"),
            [DangerKind::Merge]
        );
        // A read-only WITH stays safe.
        assert!(dangerous("WITH c AS (SELECT 1 AS n) SELECT n FROM c").is_empty());
    }

    #[test]
    fn read_only_allows_reads() {
        for sql in [
            "SELECT * FROM users WHERE id = 1",
            "select id, updated_at from orders order by updated_at desc limit 10",
            "WITH recent AS (SELECT * FROM orders) SELECT count(*) FROM recent",
            "EXPLAIN SELECT * FROM t",
            "SHOW CREATE TABLE t", // SHOW skips the embedded-verb scan on purpose
            "DESCRIBE users",
            "PRAGMA table_info(users)",
            "USE analytics",
            "SELECT 1; SELECT 2",
            "BEGIN; SELECT 1; COMMIT",
        ] {
            assert!(write_statements(sql).is_empty(), "should pass: {sql}");
        }
    }

    #[test]
    fn read_only_blocks_writes_and_the_unknown() {
        for sql in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1 WHERE id = 1",
            "DELETE FROM t",
            "DROP TABLE t",
            "CREATE TABLE t (id INT)",
            "GRANT ALL ON t TO PUBLIC",
            "SET default_transaction_read_only = off",
            "CALL cleanup()",
            "EXEC sp_who",
            "COPY t FROM '/tmp/x.csv'", // unknown verb → default deny
            "VACUUM",                   // unknown verb → default deny
            "WITH gone AS (DELETE FROM t RETURNING *) SELECT * FROM gone",
            "WITH c AS (SELECT 1) UPDATE t SET a = 1",
            "EXPLAIN ANALYZE UPDATE t SET a = 1", // EXPLAIN ANALYZE executes the DML
            "SELECT * INTO backup FROM t",        // T-SQL/PG: creates a table
            "PRAGMA journal_mode = DELETE",       // pragma assignment
            "START TRANSACTION READ WRITE",       // would escape the read-only default
            "SELECT 1; DELETE FROM t",            // one bad statement taints the batch
        ] {
            assert!(!write_statements(sql).is_empty(), "should block: {sql}");
        }
        // The offending statement (not the whole batch) is what's reported.
        let found = write_statements("SELECT 1; DELETE FROM t");
        assert_eq!(found, ["DELETE FROM t"]);
    }

    #[test]
    fn read_only_ignores_verbs_in_literals_and_comments() {
        for sql in [
            "SELECT * FROM log WHERE action = 'delete'",
            "SELECT * FROM t -- drop table t",
            "SELECT \"insert\" FROM audit_events",
        ] {
            assert!(write_statements(sql).is_empty(), "should pass: {sql}");
        }
    }
}
