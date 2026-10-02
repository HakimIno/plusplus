//! Reads a database's `EXPLAIN` output into a flat, depth-tagged list of plan nodes with the
//! few facts worth a glance: what each step costs, how far the planner's row guess was off,
//! and which steps scan a whole table or spill to disk. The UI draws the tree; the judgement
//! calls live here so they're testable without a window.
//!
//! Supported: PostgreSQL `EXPLAIN (FORMAT JSON)` and SQLite `EXPLAIN QUERY PLAN`. Anything
//! else returns `None` and the caller falls back to showing the raw output.

use serde_json::Value as Json;

use crate::model::{DbKind, QueryResult};

/// A guess this many times off (either way) is worth a warning.
const MISESTIMATE_FACTOR: f64 = 10.0;
/// A sequential scan of fewer planned rows than this is cheaper than any index; not worth a flag.
const SCAN_ROWS_WORTH_FLAGGING: f64 = 1_000.0;

/// One step of a query plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanNode {
    /// Nesting depth; the root is 0. Nodes are listed in pre-order, so this alone draws the tree.
    pub depth: usize,
    /// `Seq Scan on users`, `Index Scan using users_pkey on users`, `SEARCH users USING INDEX …`.
    pub title: String,
    /// Conditions and other qualifiers (`Filter: (age > 30)`), one per line.
    pub details: Vec<String>,
    /// The planner's total cost, in its own units (Postgres only).
    pub cost: Option<f64>,
    /// Rows the planner expected this step to produce.
    pub estimated_rows: Option<f64>,
    /// Rows it actually produced, per loop (`EXPLAIN ANALYZE` only).
    pub actual_rows: Option<f64>,
    /// Wall time spent in this step *and its children*, summed over loops (ANALYZE only).
    pub actual_ms: Option<f64>,
    /// This step's share of the whole plan's work, 0..=1: by time under ANALYZE, else by cost.
    pub share: f64,
    /// Things worth the reader's attention, in plain words.
    pub warnings: Vec<String>,
}

/// Parse `result` (the rows an `EXPLAIN` returned) for `kind`, or `None` if it isn't a plan
/// this module understands.
pub fn parse_plan(kind: DbKind, result: &QueryResult) -> Option<Vec<PlanNode>> {
    let nodes = match kind {
        DbKind::Postgres => {
            let text: String = result
                .rows
                .iter()
                .filter_map(|row| row.first())
                .map(|v| v.display())
                .collect::<Vec<_>>()
                .join("");
            parse_postgres(&text)?
        }
        DbKind::Sqlite => parse_sqlite(result)?,
        _ => return None,
    };
    (!nodes.is_empty()).then_some(nodes)
}

fn number(node: &Json, key: &str) -> Option<f64> {
    node.get(key).and_then(Json::as_f64)
}

fn text<'a>(node: &'a Json, key: &str) -> Option<&'a str> {
    node.get(key).and_then(Json::as_str)
}

/// `EXPLAIN (FORMAT JSON)` output: `[{"Plan": {...}, "Execution Time": ...}]`.
pub fn parse_postgres(json: &str) -> Option<Vec<PlanNode>> {
    let parsed: Json = serde_json::from_str(json).ok()?;
    let root = parsed.as_array()?.first()?.get("Plan")?;
    let mut nodes = Vec::new();
    walk_postgres(root, 0, &mut nodes);

    // Share of the whole: time when it was measured, else the planner's cost.
    let measured = nodes.iter().any(|n| n.actual_ms.is_some());
    let total = nodes
        .first()
        .and_then(|n| if measured { n.actual_ms } else { n.cost })
        .filter(|t| *t > 0.0);
    if let Some(total) = total {
        // A node's own share is what it adds on top of its children.
        let own: Vec<f64> = (0..nodes.len())
            .map(|i| {
                let metric = |n: &PlanNode| if measured { n.actual_ms } else { n.cost };
                let children: f64 = direct_children(&nodes, i)
                    .filter_map(|c| metric(&nodes[c]))
                    .sum();
                (metric(&nodes[i]).unwrap_or(0.0) - children).max(0.0)
            })
            .collect();
        for (node, own) in nodes.iter_mut().zip(own) {
            node.share = (own / total).clamp(0.0, 1.0);
        }
    }
    Some(nodes)
}

/// Indices of the nodes directly under `index` in a pre-order, depth-tagged list.
fn direct_children(nodes: &[PlanNode], index: usize) -> impl Iterator<Item = usize> + '_ {
    let depth = nodes[index].depth;
    nodes[index + 1..]
        .iter()
        .enumerate()
        .take_while(move |(_, n)| n.depth > depth)
        .filter(move |(_, n)| n.depth == depth + 1)
        .map(move |(i, _)| index + 1 + i)
}

fn walk_postgres(plan: &Json, depth: usize, out: &mut Vec<PlanNode>) {
    let node_type = text(plan, "Node Type").unwrap_or("Node");
    let mut title = node_type.to_string();
    if let Some(index) = text(plan, "Index Name") {
        title.push_str(&format!(" using {index}"));
    }
    if let Some(relation) = text(plan, "Relation Name") {
        title.push_str(&format!(" on {relation}"));
        if let Some(alias) = text(plan, "Alias").filter(|a| *a != relation) {
            title.push_str(&format!(" {alias}"));
        }
    }

    let details: Vec<String> = [
        "Index Cond",
        "Filter",
        "Hash Cond",
        "Join Filter",
        "Merge Cond",
        "Recheck Cond",
        "Sort Key",
        "Group Key",
    ]
    .iter()
    .filter_map(|key| {
        let value = plan.get(*key)?;
        let rendered = match value {
            Json::String(s) => s.clone(),
            Json::Array(items) => items
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            _ => return None,
        };
        Some(format!("{key}: {rendered}"))
    })
    .collect();

    let loops = number(plan, "Actual Loops").unwrap_or(1.0).max(1.0);
    let estimated_rows = number(plan, "Plan Rows");
    let actual_rows = number(plan, "Actual Rows");
    let actual_ms = number(plan, "Actual Total Time").map(|ms| ms * loops);

    let mut warnings = Vec::new();
    if node_type == "Seq Scan"
        && plan.get("Filter").is_some()
        && estimated_rows.is_none_or(|rows| rows >= SCAN_ROWS_WORTH_FLAGGING)
    {
        let removed = number(plan, "Rows Removed by Filter");
        warnings.push(match removed {
            Some(removed) if removed > 0.0 => format!(
                "Scans the whole table and throws away {} rows — an index on the filtered column may help.",
                removed as u64 * loops as u64
            ),
            _ => "Scans the whole table to apply a filter — an index on the filtered column may help."
                .into(),
        });
    }
    if let (Some(estimated), Some(actual)) = (estimated_rows, actual_rows) {
        let (low, high) = (estimated.max(1.0), actual.max(1.0));
        if high / low >= MISESTIMATE_FACTOR || low / high >= MISESTIMATE_FACTOR {
            warnings.push(format!(
                "Planner expected {} rows but got {} — table statistics may be stale (try ANALYZE).",
                estimated as u64, actual as u64
            ));
        }
    }
    if text(plan, "Sort Method").is_some_and(|m| m.to_lowercase().contains("external")) {
        warnings.push("Sort spilled to disk — more work_mem would keep it in memory.".into());
    }

    out.push(PlanNode {
        depth,
        title,
        details,
        cost: number(plan, "Total Cost"),
        estimated_rows,
        actual_rows,
        actual_ms,
        share: 0.0,
        warnings,
    });
    if let Some(children) = plan.get("Plans").and_then(Json::as_array) {
        for child in children {
            walk_postgres(child, depth + 1, out);
        }
    }
}

/// `EXPLAIN QUERY PLAN` rows: `id, parent, notused, detail`.
fn parse_sqlite(result: &QueryResult) -> Option<Vec<PlanNode>> {
    let column = |name: &str| {
        result
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    };
    let (id, parent, detail) = (column("id")?, column("parent")?, column("detail")?);
    let rows: Vec<(i64, i64, String)> = result
        .rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get(id)?.display().parse().ok()?,
                row.get(parent)?.display().parse().ok()?,
                row.get(detail)?.display(),
            ))
        })
        .collect();

    let mut nodes = Vec::new();
    fn add(rows: &[(i64, i64, String)], parent: i64, depth: usize, out: &mut Vec<PlanNode>) {
        for (id, _, detail) in rows.iter().filter(|(_, p, _)| *p == parent) {
            let upper = detail.to_uppercase();
            let mut warnings = Vec::new();
            if upper.starts_with("SCAN") && !upper.contains("USING") {
                warnings.push("Full table scan — no index is used for this table.".to_string());
            }
            if upper.contains("USE TEMP B-TREE") {
                warnings.push("Builds a temporary structure — an index could avoid it.".into());
            }
            out.push(PlanNode {
                depth,
                title: detail.clone(),
                details: Vec::new(),
                cost: None,
                estimated_rows: None,
                actual_rows: None,
                actual_ms: None,
                share: 0.0,
                warnings,
            });
            add(rows, *id, depth + 1, out);
        }
    }
    add(&rows, 0, 0, &mut nodes);
    Some(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ColumnMeta;
    use crate::value::Value;

    const PG: &str = r#"[{"Plan":{
        "Node Type":"Sort","Total Cost":1200.0,"Plan Rows":50,"Actual Rows":50,
        "Actual Total Time":80.0,"Actual Loops":1,"Sort Key":["u.name"],"Sort Method":"external merge",
        "Plans":[{
            "Node Type":"Seq Scan","Relation Name":"users","Alias":"u","Total Cost":1000.0,
            "Plan Rows":50000,"Actual Rows":40,"Actual Total Time":70.0,"Actual Loops":1,
            "Filter":"(age > 30)","Rows Removed by Filter":49960}]},
        "Execution Time":81.0}]"#;

    #[test]
    fn postgres_tree_has_depths_and_titles() {
        let nodes = parse_postgres(PG).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!((nodes[0].depth, nodes[1].depth), (0, 1));
        assert_eq!(nodes[0].title, "Sort");
        assert_eq!(nodes[1].title, "Seq Scan on users u");
        assert!(nodes[1].details.iter().any(|d| d == "Filter: (age > 30)"));
        assert!(nodes[0].details.iter().any(|d| d == "Sort Key: u.name"));
    }

    #[test]
    fn postgres_flags_seq_scan_misestimate_and_spill() {
        let nodes = parse_postgres(PG).unwrap();
        assert!(nodes[1]
            .warnings
            .iter()
            .any(|w| w.contains("Scans the whole table")));
        assert!(nodes[1]
            .warnings
            .iter()
            .any(|w| w.contains("expected 50000 rows but got 40")));
        assert!(nodes[0]
            .warnings
            .iter()
            .any(|w| w.contains("spilled to disk")));
    }

    #[test]
    fn share_is_each_nodes_own_time_not_its_subtree() {
        let nodes = parse_postgres(PG).unwrap();
        // Sort: 80ms total, 70ms of it in the child → 10/80. Scan: 70/80.
        assert!((nodes[0].share - 0.125).abs() < 1e-9);
        assert!((nodes[1].share - 0.875).abs() < 1e-9);
    }

    #[test]
    fn share_falls_back_to_cost_without_analyze() {
        let nodes = parse_postgres(
            r#"[{"Plan":{"Node Type":"Limit","Total Cost":100.0,"Plan Rows":1,
                "Plans":[{"Node Type":"Seq Scan","Relation Name":"t","Total Cost":90.0,"Plan Rows":5}]}}]"#,
        )
        .unwrap();
        assert!((nodes[1].share - 0.9).abs() < 1e-9);
        assert!(nodes[1].actual_rows.is_none());
    }

    #[test]
    fn small_tables_and_unfiltered_scans_are_not_flagged() {
        let nodes = parse_postgres(
            r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"tiny","Plan Rows":20,"Filter":"(a = 1)"}}]"#,
        )
        .unwrap();
        assert!(nodes[0].warnings.is_empty());
        let nodes = parse_postgres(
            r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"big","Plan Rows":900000}}]"#,
        )
        .unwrap();
        assert!(
            nodes[0].warnings.is_empty(),
            "reading everything is what you asked for"
        );
    }

    #[test]
    fn loops_multiply_time() {
        let nodes = parse_postgres(
            r#"[{"Plan":{"Node Type":"Index Scan","Index Name":"i","Relation Name":"t",
                "Actual Total Time":2.0,"Actual Loops":50,"Plan Rows":1,"Actual Rows":1}}]"#,
        )
        .unwrap();
        assert_eq!(nodes[0].actual_ms, Some(100.0));
        assert_eq!(nodes[0].title, "Index Scan using i on t");
    }

    #[test]
    fn garbage_is_not_a_plan() {
        assert!(parse_postgres("not json").is_none());
        assert!(parse_postgres("[]").is_none());
        assert!(parse_postgres(r#"[{"nope":1}]"#).is_none());
    }

    fn sqlite_result(rows: &[(i64, i64, &str)]) -> QueryResult {
        QueryResult {
            columns: ["id", "parent", "notused", "detail"]
                .iter()
                .map(|n| ColumnMeta {
                    name: (*n).into(),
                    type_name: String::new(),
                })
                .collect(),
            rows: rows
                .iter()
                .map(|(id, parent, detail)| {
                    vec![
                        Value::Int(*id),
                        Value::Int(*parent),
                        Value::Int(0),
                        Value::Text((*detail).into()),
                    ]
                })
                .collect(),
            ..QueryResult::default()
        }
    }

    #[test]
    fn sqlite_plan_nests_by_parent_and_flags_full_scans() {
        let result = sqlite_result(&[
            (2, 0, "SCAN orders"),
            (5, 0, "SEARCH users USING INTEGER PRIMARY KEY (rowid=?)"),
            (9, 5, "USE TEMP B-TREE FOR ORDER BY"),
        ]);
        let nodes = parse_plan(DbKind::Sqlite, &result).unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes.iter().map(|n| n.depth).collect::<Vec<_>>(), [0, 0, 1]);
        assert!(nodes[0]
            .warnings
            .iter()
            .any(|w| w.contains("Full table scan")));
        assert!(nodes[1].warnings.is_empty());
        assert!(nodes[2].warnings.iter().any(|w| w.contains("temporary")));
    }

    #[test]
    fn other_backends_fall_back() {
        let result = sqlite_result(&[(1, 0, "x")]);
        assert!(parse_plan(DbKind::MySql, &result).is_none());
    }
}
