//! Query plan rendering and interaction.

use crate::icons;
use crate::style::palette;

fn flatten_plan_json(
    value: &serde_json::Value,
    depth: usize,
    label: Option<&str>,
    lines: &mut Vec<(usize, String)>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(label) = label {
                lines.push((depth, label.to_string()));
            }
            for (key, value) in map {
                flatten_plan_json(
                    value,
                    depth + usize::from(label.is_some()),
                    Some(key),
                    lines,
                );
            }
        }
        serde_json::Value::Array(values) => {
            if let Some(label) = label {
                lines.push((depth, label.to_string()));
            }
            for (index, value) in values.iter().enumerate() {
                flatten_plan_json(
                    value,
                    depth + usize::from(label.is_some()),
                    Some(&format!("Item {}", index + 1)),
                    lines,
                );
            }
        }
        scalar => lines.push((
            depth,
            label.map_or_else(|| scalar.to_string(), |label| format!("{label}: {scalar}")),
        )),
    }
}

fn plan_lines(result: &dbcore::QueryResult) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    for (row_index, row) in result.rows.iter().enumerate() {
        if result.rows.len() > 1 {
            lines.push((0, format!("Plan row {}", row_index + 1)));
        }
        for (column_index, value) in row.iter().enumerate() {
            let label = result
                .columns
                .get(column_index)
                .map_or("Plan", |column| column.name.as_str());
            let text = value.display();
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) if json.is_array() || json.is_object() => flatten_plan_json(
                    &json,
                    usize::from(result.rows.len() > 1),
                    Some(label),
                    &mut lines,
                ),
                _ => lines.push((
                    usize::from(result.rows.len() > 1),
                    format!("{label}: {text}"),
                )),
            }
        }
    }
    lines
}

pub(super) fn plan_viewer(
    ui: &mut egui::Ui,
    result: &dbcore::QueryResult,
    kind: Option<dbcore::DbKind>,
) {
    ui.horizontal(|ui| {
        ui.add(egui::Image::new(icons::diagram()).fit_to_exact_size(egui::Vec2::splat(15.0)));
        ui.label(
            egui::RichText::new("Query plan")
                .strong()
                .color(palette::TEXT()),
        );
        ui.weak(format!("{:.1} ms", result.stats.elapsed_ms));
    });
    ui.separator();
    // Databases we can read get a tree with the costly steps called out; everything else
    // keeps the flat dump, which is always correct if less helpful.
    let parsed = kind.and_then(|kind| dbcore::plan::parse_plan(kind, result));
    match parsed {
        Some(nodes) => plan_tree(ui, &nodes),
        None => plan_dump(ui, result),
    }
}

fn plan_tree(ui: &mut egui::Ui, nodes: &[dbcore::plan::PlanNode]) {
    let warned = nodes.iter().filter(|n| !n.warnings.is_empty()).count();
    ui.label(
        egui::RichText::new(if warned == 0 {
            "No obvious problems found.".to_string()
        } else {
            format!("{warned} step(s) worth a look")
        })
        .small()
        .color(palette::TEXT_FAINT()),
    );
    ui.add_space(4.0);
    // The bar is each step's *own* share of the work, so the widest bar is the place to start.
    let hottest = nodes
        .iter()
        .map(|n| n.share)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    egui::ScrollArea::both().show(ui, |ui| {
        for node in nodes {
            ui.horizontal_top(|ui| {
                ui.add_space(node.depth as f32 * 18.0);
                if node.depth > 0 {
                    ui.label(egui::RichText::new("└").color(palette::TEXT_FAINT()));
                }
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&node.title)
                                    .monospace()
                                    .color(palette::TEXT()),
                            )
                            .selectable(true),
                        );
                        let mut stats = Vec::new();
                        if let Some(ms) = node.actual_ms {
                            stats.push(format!("{ms:.1} ms"));
                        }
                        match (node.actual_rows, node.estimated_rows) {
                            (Some(actual), Some(est)) => {
                                stats.push(format!("{} rows (est. {})", actual as u64, est as u64));
                            }
                            (Some(rows), None) | (None, Some(rows)) => {
                                stats.push(format!("{} rows", rows as u64));
                            }
                            (None, None) => {}
                        }
                        if let Some(cost) = node.cost {
                            stats.push(format!("cost {cost:.0}"));
                        }
                        if !stats.is_empty() {
                            ui.label(
                                egui::RichText::new(stats.join("  ·  "))
                                    .small()
                                    .color(palette::TEXT_FAINT()),
                            );
                        }
                    });
                    if node.share > 0.0 {
                        let width = 140.0 * (node.share / hottest) as f32;
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(140.0, 4.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            rect,
                            egui::CornerRadius::same(2),
                            palette::BORDER(),
                        );
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(rect.min, egui::vec2(width, rect.height())),
                            egui::CornerRadius::same(2),
                            palette::ACCENT(),
                        );
                        response.on_hover_text(format!(
                            "{:.0}% of the plan's work happens in this step itself",
                            node.share * 100.0
                        ));
                    }
                    for detail in &node.details {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(detail)
                                    .monospace()
                                    .small()
                                    .color(palette::TEXT_WEAK()),
                            )
                            .selectable(true),
                        );
                    }
                    for warning in &node.warnings {
                        ui.label(
                            egui::RichText::new(warning)
                                .small()
                                .color(palette::WARNING()),
                        );
                    }
                    ui.add_space(4.0);
                });
            });
        }
    });
}

fn plan_dump(ui: &mut egui::Ui, result: &dbcore::QueryResult) {
    egui::ScrollArea::both().show(ui, |ui| {
        for (depth, text) in plan_lines(result) {
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 16.0);
                if depth > 0 {
                    ui.label(egui::RichText::new("└").color(palette::TEXT_FAINT()));
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(text).monospace().color(palette::TEXT()))
                        .selectable(true),
                );
            });
        }
    });
}

#[cfg(test)]
mod query_plan_tests {
    use super::*;

    #[test]
    fn json_query_plans_become_hierarchical_rows() {
        let result = dbcore::QueryResult {
            columns: vec![dbcore::ColumnMeta {
                name: "QUERY PLAN".into(),
                type_name: "json".into(),
            }],
            rows: vec![vec![dbcore::Value::Text(
                r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"users"}}]"#.into(),
            )]],
            ..Default::default()
        };
        let lines = plan_lines(&result);
        assert!(lines.iter().any(|(depth, line)| {
            *depth >= 2 && line.contains("Node Type") && line.contains("Seq Scan")
        }));
    }

    #[test]
    fn plan_tree_and_fallback_both_render() {
        let plan = r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"users","Plan Rows":50000,
            "Total Cost":900.0,"Filter":"(age > 30)"}}]"#;
        let result = dbcore::QueryResult {
            columns: vec![dbcore::ColumnMeta {
                name: "QUERY PLAN".into(),
                type_name: "json".into(),
            }],
            rows: vec![vec![dbcore::Value::Text(plan.into())]],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::style::apply(&ctx);
        for kind in [
            Some(dbcore::DbKind::Postgres),
            Some(dbcore::DbKind::MySql),
            None,
        ] {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                plan_viewer(ui, &result, kind)
            });
        }
        assert!(dbcore::plan::parse_plan(dbcore::DbKind::Postgres, &result).is_some());
    }
}
