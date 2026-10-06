//! Foreign-key editor popover, anchored under the Structure grid cell it was opened from.
//!
//! Layout follows the TablePlus relation editor: right-aligned labels, schema + table pickers
//! for both ends of the relation, column lists to tick, ON UPDATE / ON DELETE, Delete / OK.

use crate::app::{Action, DbGuiApp};
use crate::components;
use crate::style::palette;
use dbcore::{FkAction, TableInfo};

const POPOVER_W: f32 = 470.0;
const PAD: f32 = 16.0;
const LABEL_W: f32 = 124.0;
const LABEL_GAP: f32 = 12.0;
const ARROW_H: f32 = 8.0;
const ARROW_HALF_W: f32 = 9.0;
const ROW_GAP: f32 = 10.0;
const LIST_ROW_H: f32 = 22.0;
const LIST_MAX_H: f32 = 88.0;
/// Height assumed for the first frame, before the popover has been measured, when deciding
/// whether it fits below the cell.
const GUESS_H: f32 = 400.0;

impl DbGuiApp {
    /// Relation editor opened from a Structure grid foreign-key cell.
    pub(in crate::app) fn foreign_key_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(pending) = self.foreign_key_editor.as_ref() else {
            return;
        };
        let (tab_id, index, anchor) = (pending.tab_id, pending.index, pending.anchor);
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            actions.push(Action::CancelForeignKeyEdit);
            return;
        };
        let conn_id = tab.conn_id.clone();
        let Some(crate::schema::ObjectEditor::Table(editor)) = tab.schema_editor.as_mut() else {
            actions.push(Action::CancelForeignKeyEdit);
            return;
        };
        let source_columns: Vec<String> = editor
            .columns
            .iter()
            .filter(|column| !column.drop && !column.name.trim().is_empty())
            .map(|column| column.name.trim().to_string())
            .collect();
        let source_schema = editor.schema_name.trim().to_string();
        let source_table = editor.table_name.trim().to_string();
        let Some(foreign_key) = editor.fks.get_mut(index) else {
            actions.push(Action::CancelForeignKeyEdit);
            return;
        };
        let tables: &[TableInfo] = conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
            .map_or(&[], |conn| conn.schema.tables.as_slice());

        let mut schemas: Vec<String> = tables
            .iter()
            .filter_map(|table| table.schema.clone())
            .collect();
        schemas.sort();
        schemas.dedup();
        // A new relation starts in the source table's own schema, the common case.
        if foreign_key.ref_schema.is_none() && !foreign_key.is_existing && !schemas.is_empty() {
            let own = schemas
                .iter()
                .find(|schema| **schema == source_schema)
                .unwrap_or(&schemas[0]);
            foreign_key.ref_schema = Some(own.clone());
        }

        let screen = ctx.content_rect();
        let id = egui::Id::new(("foreign_key_popover", tab_id));
        let last_h = ctx
            .data(|d| d.get_temp::<f32>(id.with("h")))
            .unwrap_or(GUESS_H);
        let below = anchor.bottom() + ARROW_H + last_h <= screen.bottom() - 8.0
            || anchor.top() - ARROW_H - last_h < screen.top() + 8.0;
        let left = (anchor.center().x - POPOVER_W * 0.5).clamp(
            screen.left() + 8.0,
            (screen.right() - POPOVER_W - 8.0).max(8.0),
        );
        let (pos, pivot) = if below {
            (
                egui::pos2(left, anchor.bottom() + ARROW_H),
                egui::Align2::LEFT_TOP,
            )
        } else {
            (
                egui::pos2(left, anchor.top() - ARROW_H),
                egui::Align2::LEFT_BOTTOM,
            )
        };

        let fill = palette::PANEL();
        let stroke = egui::Stroke::new(1.0_f32, palette::BORDER_STRONG());
        let mut edited = foreign_key.clone();
        let area = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .pivot(pivot)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(egui::CornerRadius::same(14))
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 28,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(90),
                    })
                    .inner_margin(egui::Margin::same(PAD as i8))
                    .show(ui, |ui| {
                        ui.set_width(POPOVER_W - 2.0 * PAD);
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, ROW_GAP);
                        form(
                            ui,
                            id,
                            &mut edited,
                            &Context {
                                tables,
                                schemas: &schemas,
                                source_columns: &source_columns,
                                source_schema: &source_schema,
                                source_table: &source_table,
                            },
                            actions,
                        );
                    });
            });
        *foreign_key = edited;

        let rect = area.response.rect;
        ctx.data_mut(|d| d.insert_temp(id.with("h"), rect.height()));
        paint_arrow(
            ctx,
            area.response.layer_id,
            rect,
            anchor,
            below,
            fill,
            stroke,
        );

        // Click outside (when no dropdown is open) or Esc dismisses it as a cancel.
        let outside_press = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|pos| !rect.expand(ARROW_H).contains(pos))
        });
        let escaped = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if (outside_press && !egui::Popup::is_any_open(ctx))
            || (escaped && !egui::Popup::is_any_open(ctx))
        {
            actions.push(Action::CancelForeignKeyEdit);
        }
    }
}

struct Context<'a> {
    tables: &'a [TableInfo],
    schemas: &'a [String],
    source_columns: &'a [String],
    source_schema: &'a str,
    source_table: &'a str,
}

fn form(
    ui: &mut egui::Ui,
    id: egui::Id,
    fk: &mut crate::schema::FkDraft,
    cx: &Context,
    actions: &mut Vec<Action>,
) {
    let control_w = ui.available_width() - LABEL_W - LABEL_GAP;
    let half_w = (control_w - 8.0) / 2.0;
    let has_schemas = !cx.schemas.is_empty();

    // Source table: fixed, it is the table being designed.
    row(ui, "Table", false, |ui| {
        ui.add_enabled_ui(false, |ui| {
            if has_schemas {
                let schema = if cx.source_schema.is_empty() {
                    "—"
                } else {
                    cx.source_schema
                };
                components::searchable_combo_box(
                    ui,
                    id.with("src_schema"),
                    schema,
                    half_w,
                    &[],
                    None,
                    None,
                );
            }
            let table = if cx.source_table.is_empty() {
                "(new table)"
            } else {
                cx.source_table
            };
            components::searchable_combo_box(
                ui,
                id.with("src_table"),
                table,
                if has_schemas { half_w } else { control_w },
                &[],
                None,
                None,
            );
        });
    });

    row(ui, "Columns", true, |ui| {
        let mut picked = split(&fk.columns_raw);
        if column_list(
            ui,
            id.with("src_cols"),
            control_w,
            cx.source_columns,
            &mut picked,
            "Column name…",
        ) {
            fk.columns_raw = picked.join(", ");
        }
    });

    row(ui, "Referenced Table", false, |ui| {
        if has_schemas {
            let current = fk.ref_schema.clone().unwrap_or_default();
            let selected = cx.schemas.iter().position(|schema| *schema == current);
            let shown = if current.is_empty() {
                "Schema…"
            } else {
                &current
            };
            if let Some(Some(choice)) = components::searchable_combo_box(
                ui,
                id.with("ref_schema"),
                shown,
                half_w,
                cx.schemas,
                selected,
                None,
            ) {
                fk.ref_schema = Some(cx.schemas[choice].clone());
                fk.ref_table.clear();
                fk.ref_columns_raw.clear();
            }
        }
        let ref_schema = fk.ref_schema.clone().unwrap_or_default();
        let names = table_names(cx.tables, &ref_schema);
        let selected = names.iter().position(|name| *name == fk.ref_table);
        let shown = if fk.ref_table.is_empty() {
            "Select a table…"
        } else {
            &fk.ref_table
        };
        if let Some(Some(choice)) = components::searchable_combo_box(
            ui,
            id.with("ref_table"),
            shown,
            if has_schemas { half_w } else { control_w },
            &names,
            selected,
            None,
        ) {
            fk.ref_table = names[choice].clone();
            // Pre-tick the primary key: it is what a foreign key points at almost every time.
            fk.ref_columns_raw = find_table(cx.tables, &ref_schema, &fk.ref_table)
                .map(|table| {
                    table
                        .columns
                        .iter()
                        .filter(|column| column.primary_key)
                        .map(|column| column.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
        }
    });

    row(ui, "Referenced Columns", true, |ui| {
        let ref_schema = fk.ref_schema.clone().unwrap_or_default();
        let available: Vec<String> = find_table(cx.tables, &ref_schema, &fk.ref_table)
            .map(|table| table.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        let mut picked = split(&fk.ref_columns_raw);
        if column_list(
            ui,
            id.with("ref_cols"),
            control_w,
            &available,
            &mut picked,
            "Column name…",
        ) {
            fk.ref_columns_raw = picked.join(", ");
        }
    });

    row(ui, "On Update", false, |ui| {
        action_combo(ui, id.with("on_update"), control_w, &mut fk.on_update);
    });
    row(ui, "On Delete", false, |ui| {
        action_combo(ui, id.with("on_delete"), control_w, &mut fk.on_delete);
    });

    let source_n = split(&fk.columns_raw).len();
    let ref_n = split(&fk.ref_columns_raw).len();
    let problem = if source_n == 0 {
        Some("Tick at least one column")
    } else if fk.ref_table.trim().is_empty() {
        Some("Choose the referenced table")
    } else if ref_n == 0 {
        Some("Tick the referenced column")
    } else if source_n != ref_n {
        Some("Both sides need the same number of columns")
    } else {
        None
    };

    ui.add_space(6.0);
    let footer = egui::vec2(ui.available_width(), crate::style::CONTROL_H);
    ui.allocate_ui_with_layout(
        footer,
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let ok = components::Btn::primary("OK")
                .enabled(problem.is_none())
                .show(ui);
            let ok = match problem {
                Some(text) => ok.on_disabled_hover_text(text),
                None => ok,
            };
            if ok.clicked() {
                actions.push(Action::ConfirmForeignKeyEdit);
            }
            if components::Btn::new("Delete").show(ui).clicked() {
                actions.push(Action::DeleteForeignKeyEdit);
            }
        },
    );
}

/// One form line: a right-aligned label, then the controls.
fn row(ui: &mut egui::Ui, label: &str, _top: bool, add: impl FnOnce(&mut egui::Ui)) {
    // Labels sit on the first line of a tall control (a column list), so top-align always.
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_W, crate::style::CONTROL_H),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new(label).color(palette::TEXT()));
            },
        );
        ui.add_space(LABEL_GAP - 8.0);
        add(ui);
    });
}

fn action_combo(ui: &mut egui::Ui, id: egui::Id, width: f32, value: &mut FkAction) {
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(value.label())
        .show_ui(ui, |ui| {
            for action in FkAction::ALL {
                ui.selectable_value(value, *action, action.label());
            }
        });
}

/// A boxed, scrollable list of column names; clicking a name ticks or unticks it. Ticks keep
/// click order because composite keys pair the two sides positionally. Returns true on change.
fn column_list(
    ui: &mut egui::Ui,
    id: egui::Id,
    width: f32,
    names: &[String],
    picked: &mut Vec<String>,
    placeholder: &str,
) -> bool {
    let mut changed = false;
    let box_h = (names.len().clamp(2, 4) as f32) * LIST_ROW_H + 8.0;
    egui::Frame::new()
        .fill(palette::CODE_BG())
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.set_width(width - 8.0);
            ui.set_min_height(box_h - 8.0);
            if names.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(placeholder)
                        .color(palette::TEXT_FAINT())
                        .size(13.0),
                );
                return;
            }
            // The row sits in a horizontal layout; the list itself stacks downwards.
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt(id)
                    .max_height(LIST_MAX_H)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for name in names {
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), LIST_ROW_H),
                                egui::Sense::click(),
                            );
                            let on = picked.iter().any(|p| p.eq_ignore_ascii_case(name));
                            let painter = ui.painter();
                            if response.hovered() {
                                painter.rect_filled(
                                    rect,
                                    egui::CornerRadius::same(5),
                                    palette::SURFACE_HOVER(),
                                );
                            }
                            // Only the checkbox carries the state: a ring when off, a filled
                            // primary box with a tick when on.
                            let box_rect = egui::Rect::from_center_size(
                                egui::pos2(rect.left() + 16.0, rect.center().y),
                                egui::vec2(14.0, 14.0),
                            );
                            if on {
                                painter.rect_filled(
                                    box_rect,
                                    egui::CornerRadius::same(3),
                                    palette::ACCENT(),
                                );
                                let c = box_rect.center();
                                let tick = egui::Stroke::new(1.8_f32, palette::ON_ACCENT());
                                painter.line_segment(
                                    [c + egui::vec2(-3.5, 0.0), c + egui::vec2(-1.0, 3.0)],
                                    tick,
                                );
                                painter.line_segment(
                                    [c + egui::vec2(-1.0, 3.0), c + egui::vec2(4.0, -3.0)],
                                    tick,
                                );
                            } else {
                                painter.rect_stroke(
                                    box_rect,
                                    egui::CornerRadius::same(3),
                                    egui::Stroke::new(1.2_f32, palette::TEXT_FAINT()),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            painter.text(
                                egui::pos2(rect.left() + 32.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                name,
                                egui::TextStyle::Body.resolve(ui.style()),
                                palette::TEXT(),
                            );
                            if response.clicked() {
                                if on {
                                    picked.retain(|p| !p.eq_ignore_ascii_case(name));
                                } else {
                                    picked.push(name.clone());
                                }
                                changed = true;
                            }
                        }
                    });
            });
        });
    changed
}

/// The notch joining the popover to its cell, drawn over the popover's own border so the
/// two read as one shape.
fn paint_arrow(
    ctx: &egui::Context,
    layer: egui::LayerId,
    rect: egui::Rect,
    anchor: egui::Rect,
    below: bool,
    fill: egui::Color32,
    stroke: egui::Stroke,
) {
    let x = anchor
        .center()
        .x
        .clamp(rect.left() + 26.0, rect.right() - 26.0);
    let painter = ctx.layer_painter(layer);
    let (base_y, tip_y) = if below {
        (rect.top(), rect.top() - ARROW_H)
    } else {
        (rect.bottom(), rect.bottom() + ARROW_H)
    };
    let left = egui::pos2(x - ARROW_HALF_W, base_y);
    let tip = egui::pos2(x, tip_y);
    let right = egui::pos2(x + ARROW_HALF_W, base_y);
    painter.add(egui::Shape::convex_polygon(
        vec![left, tip, right],
        fill,
        egui::Stroke::NONE,
    ));
    // Hide the frame's border under the notch, then outline only its two slanted sides.
    painter.line_segment(
        [
            egui::pos2(left.x + 0.5, base_y),
            egui::pos2(right.x - 0.5, base_y),
        ],
        egui::Stroke::new(stroke.width + 1.0, fill),
    );
    painter.line_segment([left, tip], stroke);
    painter.line_segment([tip, right], stroke);
}

fn split(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn find_table<'a>(tables: &'a [TableInfo], schema: &str, name: &str) -> Option<&'a TableInfo> {
    if name.is_empty() {
        return None;
    }
    tables
        .iter()
        .find(|table| table.name == name && table.schema.as_deref().unwrap_or("") == schema)
}

fn table_names(tables: &[TableInfo], schema: &str) -> Vec<String> {
    let mut names: Vec<String> = tables
        .iter()
        .filter(|table| table.schema.as_deref().unwrap_or("") == schema)
        .map(|table| table.name.clone())
        .collect();
    names.sort();
    names
}
