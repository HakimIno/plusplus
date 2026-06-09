//! The results grid — a virtualized, resizable, sortable table built on
//! `egui_extras::TableBuilder`, styled after TablePlus: a row-number gutter on the left,
//! dense rows, click-to-select with row highlight, and click-to-sort headers.
//! Only the visible rows are rendered each frame, so it stays smooth at 100k+ rows.

use crate::style::palette;
use dbcore::{QueryResult, Value};
use egui_extras::{Column, TableBuilder};

/// What the grid reports back to the app after a frame.
#[derive(Default)]
pub struct GridResponse {
    /// A header was clicked → (re)sort by this column index.
    pub sort: Option<usize>,
    /// A row was clicked → select this *display* row index (index into `order`).
    pub selected: Option<usize>,
}

/// Render the result set. `order` maps display rows → indices into `result.rows`.
/// `selected` is the currently selected display row.
pub fn results_grid(
    ui: &mut egui::Ui,
    result: &QueryResult,
    order: &[usize],
    sort: Option<(usize, bool)>,
    selected: Option<usize>,
) -> GridResponse {
    let mut out = GridResponse::default();
    let ncols = result.columns.len();
    if ncols == 0 {
        ui.weak("No columns to display.");
        return out;
    }

    let row_height = egui::TextStyle::Monospace.resolve(ui.style()).size + 8.0;
    // Width of the row-number gutter, sized to the largest row number.
    let digits = (order.len().max(1) as f64).log10().floor() as usize + 1;
    let gutter_w = 18.0 + 8.0 * digits as f32;

    let mut builder = TableBuilder::new(ui)
        // A stable, unique id keeps the table's internal scroll/resize/row ids consistent
        // across frames — this is what prevents egui's "ID clash" outline from flickering
        // while scrolling fast (egui's own warning advises giving tables a unique id_salt).
        .id_salt("results_grid")
        // Cells must sense clicks for row selection (`row.response().clicked()`) to work;
        // the default is hover-only.
        .sense(egui::Sense::click())
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .column(Column::exact(gutter_w)); // gutter (not resizable)
    for _ in 0..ncols {
        builder = builder.column(Column::initial(160.0).at_least(40.0).clip(true).resizable(true));
    }

    builder
        .header(24.0, |mut header| {
            header.col(|ui| {
                ui.add_space(4.0);
                ui.weak("#");
            });
            for (i, col) in result.columns.iter().enumerate() {
                header.col(|ui| {
                    let (arrow, sorted) = match sort {
                        Some((c, asc)) if c == i => (if asc { "  ↑" } else { "  ↓" }, true),
                        _ => ("", false),
                    };
                    let mut text = egui::RichText::new(format!("{}{arrow}", col.name)).strong();
                    text = text.color(if sorted { palette::ACCENT } else { palette::TEXT });
                    let label = egui::Label::new(text)
                        .sense(egui::Sense::click())
                        .selectable(false);
                    if ui.add(label).on_hover_text(&col.type_name).clicked() {
                        out.sort = Some(i);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(row_height, order.len(), |mut row| {
                let disp = row.index();
                let r = order[disp];
                row.set_selected(selected == Some(disp));

                // Row-number gutter.
                row.col(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add_space(4.0);
                            ui.weak(egui::RichText::new(format!("{}", disp + 1)).monospace());
                        },
                    );
                });

                for value in &result.rows[r] {
                    row.col(|ui| cell(ui, value));
                }

                if row.response().clicked() {
                    out.selected = Some(disp);
                }
            });
        });

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcore::{ColumnMeta, QueryResult, QueryStats};

    fn fake_result(rows: usize, cols: usize) -> QueryResult {
        let columns = (0..cols)
            .map(|c| ColumnMeta {
                name: format!("col{c}"),
                type_name: "TEXT".into(),
            })
            .collect();
        let data = (0..rows)
            .map(|r| (0..cols).map(|c| Value::Int((r * cols + c) as i64)).collect())
            .collect();
        QueryResult {
            columns,
            rows: data,
            stats: QueryStats::default(),
        }
    }

    fn collect_clash_text(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<String>) {
        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    let s = t.galley.text();
                    if s.contains('🔥') {
                        out.push(s.to_string());
                    }
                }
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        for cs in shapes {
            walk(&cs.shape, out);
        }
    }

    /// Render the grid headlessly across a few frames while injecting scroll, and capture
    /// any egui "ID clash" markers (🔥) so we can pinpoint the offending widget.
    #[test]
    fn probe_id_clash_while_scrolling() {
        let ctx = egui::Context::default();
        let result = fake_result(2000, 5);
        let order: Vec<usize> = (0..result.rows.len()).collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));

        let mut clashes: Vec<String> = Vec::new();
        for _ in 0..5 {
            let events = vec![
                egui::Event::PointerMoved(egui::pos2(400.0, 300.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -40.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                },
            ];
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let _ = results_grid(ui, &result, &order, None, None);
                });
            });
            collect_clash_text(&out.shapes, &mut clashes);
        }

        assert!(
            clashes.is_empty(),
            "ID clashes detected in results grid:\n{}",
            clashes.join("\n")
        );
    }
}

/// Render a single cell, dimming NULLs and monospacing numbers.
fn cell(ui: &mut egui::Ui, value: &Value) {
    match value {
        Value::Null => {
            ui.colored_label(palette::TEXT_FAINT, egui::RichText::new("NULL").italics());
        }
        Value::Int(_) | Value::Float(_) => {
            ui.label(egui::RichText::new(value.display()).monospace());
        }
        other => {
            ui.label(other.display());
        }
    }
}
