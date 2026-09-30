//! Tree rendering and interaction.

use crate::components;
use crate::icons;
use crate::style::palette;

/// Shared sidebar-tree metrics so Items and Queries read as one language.
pub(super) const TREE_ROW_H: f32 = 26.0;

pub(super) const TREE_CHILD_INDENT: f32 = 28.0;

const TREE_GUIDE_INSET: f32 = 22.0;

pub(super) const TREE_ICON: f32 = 16.0;

pub(super) fn paint_tree_row_fill(
    ui: &egui::Ui,
    row_rect: egui::Rect,
    selected: bool,
    hovered: bool,
) {
    if !ui.is_rect_visible(row_rect) {
        return;
    }
    let radius = egui::CornerRadius::same(7);
    if selected {
        ui.painter()
            .rect_filled(row_rect, radius, palette::SELECTION());
    } else if hovered {
        ui.painter()
            .rect_filled(row_rect, radius, palette::SURFACE_HOVER());
    }
}

pub(super) fn paint_tree_guide(ui: &egui::Ui, folder_rect: egui::Rect, body_rect: egui::Rect) {
    let guide_x = folder_rect.left() + TREE_GUIDE_INSET;
    ui.painter().vline(
        guide_x,
        egui::Rangef::new(
            body_rect.top(),
            (body_rect.bottom() - 4.0).max(body_rect.top()),
        ),
        egui::Stroke::new(1.0_f32, palette::BORDER()),
    );
}

pub(super) fn paint_drop_line(ui: &egui::Ui, row_rect: egui::Rect, after: bool) {
    if !ui.is_rect_visible(row_rect) {
        return;
    }
    let y = if after {
        row_rect.bottom()
    } else {
        row_rect.top()
    };
    ui.painter().hline(
        row_rect.x_range(),
        y,
        egui::Stroke::new(2.0_f32, palette::ACCENT()),
    );
}

pub(super) fn drop_after(ui: &egui::Ui, row_rect: egui::Rect) -> bool {
    ui.ctx()
        .pointer_interact_pos()
        .is_some_and(|pointer| pointer.y > row_rect.center().y)
}

/// File-style leaf in the sidebar tree: full-width hover pill, indented icon + name.
pub(super) fn tree_file_row(
    ui: &mut egui::Ui,
    indent: f32,
    icon: egui::ImageSource<'static>,
    icon_color: egui::Color32,
    name: &str,
    selected: bool,
    sense: egui::Sense,
) -> (egui::Rect, egui::Response) {
    let (row_rect, row_resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), TREE_ROW_H), sense);
    paint_tree_row_fill(ui, row_rect, selected, row_resp.hovered());
    let content = egui::Rect::from_min_max(
        egui::pos2(row_rect.left() + indent, row_rect.top()),
        row_rect.max,
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (icon_rect, _) =
                ui.allocate_exact_size(egui::vec2(16.0, TREE_ROW_H), egui::Sense::hover());
            egui::Image::new(icon).tint(icon_color).paint_at(
                ui,
                egui::Rect::from_center_size(icon_rect.center(), egui::Vec2::splat(TREE_ICON)),
            );
            ui.add(
                egui::Label::new(egui::RichText::new(name).color(palette::TEXT()))
                    .truncate()
                    .selectable(false)
                    .sense(egui::Sense::hover()),
            );
        });
    });
    (row_rect, row_resp)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CalloutEdit {
    Idle,
    Confirm,
    Cancel,
}

/// Side callout for renaming a saved query or folder: sits to the right of `anchor`
/// with a caret pointing back at the row. Not inline, not a modal.
pub(super) fn rename_callout(
    ui: &mut egui::Ui,
    anchor: &egui::Response,
    name: &mut String,
    hint: &str,
) -> CalloutEdit {
    const ARROW: f32 = 8.0;
    let popup_id = anchor.id.with("rename_callout");
    let focus_id = popup_id.with("focused");
    let frame = egui::Frame::popup(ui.style())
        .fill(palette::PANEL())
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10));

    let mut confirm = false;
    let mut cancel = false;
    let shown = egui::Popup::from_response(anchor)
        .id(popup_id)
        .align(egui::RectAlign::RIGHT)
        .align_alternatives(&[egui::RectAlign::LEFT])
        .gap(ARROW + 1.0)
        .width(268.0)
        .frame(frame)
        .open(true)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .layout(egui::Layout::top_down(egui::Align::Min))
        .show(|ui| {
            ui.set_width(248.0);
            ui.label(
                egui::RichText::new("Rename")
                    .small()
                    .color(palette::TEXT_WEAK()),
            );
            ui.add_space(4.0);
            let resp = components::text_input(ui, name, hint, ui.available_width());
            let already_focused = ui
                .ctx()
                .data(|data| data.get_temp::<bool>(focus_id).unwrap_or(false));
            if !already_focused {
                resp.request_focus();
                ui.ctx().data_mut(|data| data.insert_temp(focus_id, true));
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                confirm = true;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if components::button(ui, icons::close(), "Cancel", true).clicked() {
                    cancel = true;
                }
                if components::primary_button(ui, icons::save(), "Save", true).clicked() {
                    confirm = true;
                }
            });
        });

    if let Some(inner) = shown {
        paint_callout_arrow(
            ui.ctx(),
            inner.response.layer_id,
            inner.response.rect,
            anchor.rect,
        );
        if !confirm && !cancel {
            let pointer_clicked = ui.input(|i| i.pointer.primary_released());
            if pointer_clicked {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let in_popup = inner.response.rect.expand(ARROW).contains(pos);
                    let in_row = anchor.rect.contains(pos);
                    if !in_popup && !in_row {
                        cancel = true;
                    }
                }
            }
        }
    }
    if confirm {
        ui.ctx().data_mut(|data| data.remove::<bool>(focus_id));
        CalloutEdit::Confirm
    } else if cancel {
        ui.ctx().data_mut(|data| data.remove::<bool>(focus_id));
        CalloutEdit::Cancel
    } else {
        CalloutEdit::Idle
    }
}

/// Hover preview beside a sidebar row: same chrome as the rename callout (panel fill,
/// strong border, caret pointing at the anchor).
pub(super) fn sql_preview_callout(
    ui: &mut egui::Ui,
    anchor: &egui::Response,
    title: &str,
    sql: &str,
    meta: Option<&str>,
) {
    const ARROW: f32 = 8.0;
    let sql = sql.trim();
    if sql.is_empty() {
        return;
    }
    let frame = egui::Frame::popup(ui.style())
        .fill(palette::PANEL())
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10));

    let mut tooltip = egui::Tooltip::for_enabled(anchor);
    tooltip.popup = tooltip
        .popup
        .align(egui::RectAlign::RIGHT)
        .align_alternatives(&[egui::RectAlign::LEFT])
        .gap(ARROW + 1.0)
        .width(340.0)
        .frame(frame)
        .layout(egui::Layout::top_down(egui::Align::Min));

    let shown = tooltip.show(|ui| {
        ui.set_max_width(320.0);
        ui.label(egui::RichText::new(title).color(palette::TEXT()).strong());
        ui.add_space(6.0);
        let mut font = egui::TextStyle::Monospace.resolve(ui.style());
        font.size = (font.size - 1.5).max(10.0);
        let mut job = crate::highlight::highlight_sql_cached(ui.ctx(), sql, font);
        job.wrap.max_width = ui.available_width().max(40.0);
        job.wrap.max_rows = 14;
        ui.add(egui::Label::new(job).selectable(false));
        if let Some(meta) = meta.filter(|m| !m.is_empty()) {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(meta)
                    .small()
                    .color(palette::TEXT_FAINT()),
            );
        }
    });
    if let Some(inner) = shown {
        paint_callout_arrow(
            ui.ctx(),
            inner.response.layer_id,
            inner.response.rect,
            anchor.rect,
        );
    }
}

fn paint_callout_arrow(
    ctx: &egui::Context,
    layer: egui::LayerId,
    popup: egui::Rect,
    anchor: egui::Rect,
) {
    let on_right = popup.center().x >= anchor.center().x;
    let y = anchor
        .center()
        .y
        .clamp(popup.top() + 12.0, popup.bottom() - 12.0);
    let (tip, base_a, base_b) = if on_right {
        let x = popup.left();
        (
            egui::pos2(x - 7.0, y),
            egui::pos2(x + 0.5, y - 6.5),
            egui::pos2(x + 0.5, y + 6.5),
        )
    } else {
        let x = popup.right();
        (
            egui::pos2(x + 7.0, y),
            egui::pos2(x - 0.5, y - 6.5),
            egui::pos2(x - 0.5, y + 6.5),
        )
    };
    let painter = ctx.layer_painter(layer);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, base_a, base_b],
        palette::PANEL(),
        egui::Stroke::NONE,
    ));
    let stroke = egui::Stroke::new(1.0_f32, palette::BORDER_STRONG());
    painter.line_segment([tip, base_a], stroke);
    painter.line_segment([tip, base_b], stroke);
}
