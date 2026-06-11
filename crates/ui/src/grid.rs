//! The results grid — a virtualized, resizable, sortable table built on
//! `egui_extras::TableBuilder`, styled after TablePlus: a row-number gutter on the left,
//! dense rows, click-to-select with row highlight, and click-to-sort headers.
//! Only the visible rows are rendered each frame, so it stays smooth at 100k+ rows.

use crate::edit::{EditOutcome, EditorKind, Edits};
use crate::style::palette;
use dbcore::{QueryResult, Value};
use egui_extras::{Column, TableBuilder};

/// Natural per-column width: columns expand past this to fill spare space, but never shrink
/// below it — once the total exceeds the panel the grid scrolls horizontally instead.
const COL_W: f32 = 160.0;

/// What the grid reports back to the app after a frame.
#[derive(Default)]
pub struct GridResponse {
    /// A header was clicked → (re)sort by this column index.
    pub sort: Option<usize>,
    /// A row was clicked → select this *display* row index (index into `order`).
    pub selected: Option<usize>,
    /// A cell was double-clicked → start editing it (raw row index, column index).
    pub begin_edit: Option<(usize, usize)>,
    /// A boolean cell was double-clicked → flip it (raw row index, column index).
    pub toggle: Option<(usize, usize)>,
    /// The open editor should be committed (Enter pressed or focus lost).
    pub commit_edit: bool,
    /// The open editor should be discarded (Escape pressed).
    pub cancel_edit: bool,
    /// Empty table space was double-clicked → append a new (insert) row.
    pub add_row: bool,
    /// Any cell was double-clicked this frame (even non-editable ones like Bytes/deleted).
    /// Used to suppress add_row when the fallback zone overlaps the last visible row.
    cell_dbl_clicked: bool,
}

/// What a body row at a given display index represents.
enum RowKind {
    /// A stored result row (value is the raw index into `result.rows`).
    Stored(usize),
    /// A new (insert) row being filled in (value is its [`crate::edit::NEW_ROW_BASE`] id).
    New(usize),
}

/// Render the result set. `order` maps display rows → indices into `result.rows`.
/// `selected` is the currently selected display row. `grid_id` must be unique per tab so
/// egui's per-widget click-time memory doesn't bleed between tabs.
#[allow(clippy::too_many_arguments)]
pub fn results_grid(
    ui: &mut egui::Ui,
    result: &QueryResult,
    order: &[usize],
    sort: Option<(usize, bool)>,
    selected: Option<usize>,
    edits: &mut Edits,
    editable: bool,
    grid_id: u64,
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

    // egui_extras' Table hardcodes horizontal scrolling off, so once the columns no longer
    // fit it just squeezes them. We detect that case and wrap the table in our own
    // horizontal ScrollArea, sizing the inner ui to the columns' natural width so they keep
    // a readable width and scroll sideways instead. When they fit, render inline so columns
    // still expand to fill the panel.
    let spacing = ui.spacing().item_spacing.x;
    let desired_total = gutter_w + ncols as f32 * (COL_W + spacing);
    let table_rect = ui.available_rect_before_wrap();
    let rendered_rows = order.len() + edits.new_rows;

    if desired_total <= ui.available_width() {
        build_grid(
            ui, result, order, sort, selected, edits, editable, gutter_w, row_height, &mut out,
            grid_id,
        );
    } else {
        egui::ScrollArea::horizontal()
            .id_salt(("results_hscroll", grid_id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(desired_total);
                build_grid(
                    ui, result, order, sort, selected, edits, editable, gutter_w, row_height,
                    &mut out, grid_id,
                );
            });
    }
    capture_empty_table_double_click(ui, table_rect, rendered_rows, row_height, editable, grid_id, &mut out);

    out
}

/// Build the `egui_extras` table into `ui`, reporting header/row clicks via `out`. Split out
/// of [`results_grid`] so it can render either inline or inside a horizontal scroll area.
#[allow(clippy::too_many_arguments)]
fn build_grid(
    ui: &mut egui::Ui,
    result: &QueryResult,
    order: &[usize],
    sort: Option<(usize, bool)>,
    selected: Option<usize>,
    edits: &mut Edits,
    editable: bool,
    gutter_w: f32,
    row_height: f32,
    out: &mut GridResponse,
    grid_id: u64,
) {
    let ncols = result.columns.len();
    let mut builder = TableBuilder::new(ui)
        // A stable, unique id keeps the table's internal scroll/resize/row ids consistent
        // across frames — this is what prevents egui's "ID clash" outline from flickering
        // while scrolling fast (egui's own warning advises giving tables a unique id_salt).
        // grid_id is per-tab so widgets across tabs never share egui click-time memory.
        .id_salt(("results_grid", grid_id))
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
        builder = builder.column(
            Column::initial(COL_W)
                .at_least(40.0)
                .clip(true)
                .resizable(true),
        );
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
                    text = text.color(if sorted {
                        palette::ACCENT()
                    } else {
                        palette::TEXT()
                    });
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
            let new_rows = edits.new_rows;
            let total = order.len() + new_rows;
            body.rows(row_height, total, |mut row| {
                let disp = row.index();
                // Display index splits into: stored rows, then new rows.
                let kind = if disp < order.len() {
                    RowKind::Stored(order[disp])
                } else {
                    RowKind::New(crate::edit::NEW_ROW_BASE + (disp - order.len()))
                };
                let r = match kind {
                    RowKind::Stored(r) | RowKind::New(r) => r,
                };
                let state = edits.row_state(r);
                row.set_selected(selected == Some(disp));

                // Row-number gutter: number for stored rows, a mark for new rows. Tinted
                // green (edit/new) or red (delete) like the cells. Double-clicking the gutter
                // is a common way to start editing a row, so we treat it the same as
                // double-clicking the first data cell.
                let (_, gutter_resp) = row.col(|ui| {
                    tint_row(ui, state);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(4.0);
                        let label = match kind {
                            RowKind::Stored(_) => format!("{}", disp + 1),
                            RowKind::New(_) => "✱".to_string(),
                        };
                        ui.weak(egui::RichText::new(label).monospace());
                    });
                });
                if editable && gutter_resp.double_clicked() && state != crate::edit::RowState::Deleted {
                    let first_editable = (0..ncols)
                        .find(|&c| edits.col_kind(c) != EditorKind::Bool);
                    if let Some(c) = first_editable {
                        out.begin_edit = Some((r, c));
                    }
                }

                let null = Value::Null;
                for c in 0..ncols {
                    // The original/stored value behind this cell (NULL for new rows).
                    let stored = match kind {
                        RowKind::Stored(r) => &result.rows[r][c],
                        _ => &null,
                    };
                    let mut label_resp = None;
                    let (_, col_resp) = row.col(|ui| {
                        tint_row(ui, state);
                        // Skip text layout, HashMap lookups, and widget allocation for
                        // columns outside the visible horizontal viewport. The cell rect is
                        // still allocated by egui_extras (needed for column sizing), but
                        // we avoid all the per-cell work for invisible columns.
                        if !ui.is_rect_visible(ui.max_rect()) {
                            return;
                        }
                        if edits.is_active_from(r, c, crate::edit::EditOrigin::Grid) {
                            // The cell under edit fills the whole cell; the editor is
                            // type-aware and validates numbers/dates before they can commit.
                            // (An edit begun in the Details panel renders its editor there,
                            // not here — two editors would fight over keyboard focus.)
                            if let Some(active) = edits.active.as_mut() {
                                let size = ui.available_size();
                                match crate::edit::render_editor(ui, active, Some(size)) {
                                    EditOutcome::Commit => out.commit_edit = true,
                                    EditOutcome::Cancel => out.cancel_edit = true,
                                    EditOutcome::Continue => {}
                                }
                            }
                        } else {
                            // Show the staged value if present, else the stored one.
                            let staged = edits.staged(r, c);
                            label_resp = Some(cell(ui, staged.unwrap_or(stored), staged.is_some()));
                        }
                    });

                    // Double-click to edit (binary cells aren't editable; deleted rows are
                    // on their way out). Booleans toggle in place; everything else opens the
                    // inline editor. The label must sense clicks (see `cell`) — plain labels
                    // only hover, so double-click on the text itself would otherwise be lost.
                    let dbl = col_resp.double_clicked()
                        || label_resp.is_some_and(|r| r.double_clicked());
                    if dbl {
                        out.cell_dbl_clicked = true;
                    }
                    if editable
                        && dbl
                        && state != crate::edit::RowState::Deleted
                        && !matches!(stored, Value::Bytes(_))
                    {
                        if edits.col_kind(c) == EditorKind::Bool {
                            out.toggle = Some((r, c));
                        } else {
                            out.begin_edit = Some((r, c));
                        }
                    }
                }

                if row.response().clicked() {
                    out.selected = Some(disp);
                }
            });
        });
}

/// Reserve the blank space under the table rows as an invisible add-row target. When rows
/// fill the entire panel there is no natural empty space, so a thin fixed zone is kept at
/// the bottom regardless — this ensures a new row can always be created by double-clicking.
fn capture_empty_table_double_click(
    ui: &mut egui::Ui,
    table_rect: egui::Rect,
    rendered_rows: usize,
    row_height: f32,
    editable: bool,
    grid_id: u64,
    out: &mut GridResponse,
) {
    if !editable {
        return;
    }
    // When rows don't fill the panel, use the empty space below them. When they do, keep a
    // 24 px fallback zone at the bottom so the user can always double-click to add a row.
    const ADD_ROW_ZONE: f32 = 24.0;
    // egui_extras positions each row at row_height + item_spacing.y intervals (see
    // TableBody::rows), so content_bottom must use the same stride or the zone will overlap
    // the last row and trigger add_row when the user double-clicks its bottom portion.
    let row_step = row_height + ui.spacing().item_spacing.y;
    let content_bottom = table_rect.top() + 24.0 + rendered_rows as f32 * row_step;
    let zone_top = content_bottom.min(table_rect.bottom() - ADD_ROW_ZONE);
    if zone_top >= table_rect.bottom() {
        return;
    }
    let zone_rect = egui::Rect::from_min_max(
        egui::pos2(table_rect.left(), zone_top),
        egui::pos2(table_rect.right(), table_rect.bottom()),
    );
    // grid_id is included so the click-time memory is per-tab, matching the table's own id_salt.
    let resp = ui.interact(
        zone_rect,
        egui::Id::new(("results_grid_empty_add_row", grid_id)),
        egui::Sense::click(),
    );
    // Suppress add_row when the fallback zone overlaps the last visible row and the user
    // actually double-clicked any cell — including Bytes or deleted-row cells that don't set
    // begin_edit/toggle but still represent a deliberate click on an existing row.
    if resp.double_clicked()
        && out.begin_edit.is_none()
        && out.toggle.is_none()
        && !out.cell_dbl_clicked
    {
        out.add_row = true;
    }
}

/// Paint a faint wash over the current cell to flag its pending state: green for edited or
/// new rows (a pending write), red for rows marked for deletion. Clean rows are untouched.
fn tint_row(ui: &egui::Ui, state: crate::edit::RowState) {
    use crate::edit::RowState;
    let tint = match state {
        RowState::Clean => return,
        RowState::Edited | RowState::New => {
            let s = palette::SUCCESS();
            egui::Color32::from_rgba_unmultiplied(s.r(), s.g(), s.b(), 28)
        }
        RowState::Deleted => {
            let d = palette::DANGER();
            egui::Color32::from_rgba_unmultiplied(d.r(), d.g(), d.b(), 32)
        }
    };
    ui.painter().rect_filled(ui.max_rect(), 0.0, tint);
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
            .map(|r| {
                (0..cols)
                    .map(|c| Value::Int((r * cols + c) as i64))
                    .collect()
            })
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
            let mut edits = Edits::default();
            let out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let _ = results_grid(ui, &result, &order, None, None, &mut edits, false, 0);
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

/// Render a single cell, dimming NULLs and monospacing numbers. A `staged` value (an edit
/// not yet saved) is drawn in the success colour so it stands out from stored data.
fn cell(ui: &mut egui::Ui, value: &Value, staged: bool) -> egui::Response {
    let (text, color) = if staged {
        let text = match value {
            Value::Null => egui::RichText::new("NULL").italics(),
            other => egui::RichText::new(other.display()),
        };
        (text, palette::SUCCESS())
    } else {
        match value {
            Value::Null => (
                egui::RichText::new("NULL").italics(),
                palette::TEXT_FAINT(),
            ),
            Value::Int(_) | Value::Float(_) => (
                egui::RichText::new(value.display()).monospace(),
                palette::TEXT(),
            ),
            other => (egui::RichText::new(other.display()), palette::TEXT()),
        }
    };
    ui.add(
        egui::Label::new(text.color(color))
            .sense(egui::Sense::click()),
    )
}
