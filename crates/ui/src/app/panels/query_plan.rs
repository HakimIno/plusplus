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

pub(super) fn plan_viewer(ui: &mut egui::Ui, result: &dbcore::QueryResult) {
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
}
