//! Dialog chrome shared by modal surfaces.

use egui::{Margin, Vec2};

use crate::style::{self, palette, CONTROL_H};

pub(crate) fn dialog_window(title: impl Into<egui::WidgetText>) -> egui::Window<'static> {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
}

pub(crate) fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    egui::Frame::window(&style)
        .fill(style::mix(palette::BASE(), palette::PANEL(), 0.45))
        .stroke(egui::Stroke::new(
            1.0_f32,
            style::mix(palette::BASE(), palette::BORDER(), 0.55),
        ))
        .inner_margin(Margin::symmetric(12, 4))
}

/// Blocking confirmation with the same title, frame, and content spacing as dialog windows.
pub(crate) fn dialog_modal<T>(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    width: f32,
    add_content: impl FnOnce(&mut egui::Ui) -> T,
) -> egui::ModalResponse<T> {
    egui::Modal::new(egui::Id::new(id))
        .frame(dialog_frame(ctx).inner_margin(Margin::ZERO))
        .show(ctx, |ui| {
            ui.set_width(width + 24.0);
            let (header, _) =
                ui.allocate_exact_size(egui::vec2(width + 24.0, 32.0), egui::Sense::hover());
            ui.scope_builder(egui::UiBuilder::new().max_rect(header), |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(title)
                            .text_style(egui::TextStyle::Heading)
                            .color(palette::TEXT()),
                    );
                });
            });
            let close_rect = egui::Rect::from_center_size(
                egui::pos2(header.right() - 16.0, header.center().y),
                egui::Vec2::splat(16.0),
            );
            let close = ui.interact(
                close_rect,
                ui.id().with("dialog_close"),
                egui::Sense::click(),
            );
            let stroke = ui.style().interact(&close).fg_stroke;
            let cross = close_rect.shrink(2.0);
            ui.painter()
                .line_segment([cross.left_top(), cross.right_bottom()], stroke);
            ui.painter()
                .line_segment([cross.right_top(), cross.left_bottom()], stroke);
            if close.clicked() {
                ui.close();
            }
            ui.painter().hline(
                header.x_range(),
                header.bottom(),
                egui::Stroke::new(
                    1.0_f32,
                    style::mix(palette::BASE(), palette::BORDER(), 0.55),
                ),
            );
            ui.add_space(4.0);
            egui::Frame::new()
                .inner_margin(Margin::symmetric(12, 4))
                .show(ui, |ui| {
                    ui.set_width(width);
                    add_content(ui)
                })
                .inner
        })
}

/// Footer row for a dialog: plain, with no bar or rule of its own, like VS Code's. Buttons
/// are right-aligned; the first widget added is the rightmost (put the primary action first,
/// then cancel / secondary).
pub(crate) fn dialog_footer(ui: &mut egui::Ui, add_buttons: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(10.0);

    let body_w = ui.min_rect().width();
    let (row_rect, _) =
        ui.allocate_exact_size(egui::vec2(body_w, CONTROL_H + 6.0), egui::Sense::hover());

    ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
        // Right-to-left so the first widget in `add_buttons` (the primary action) sits
        // at the far right. The dialog frame already provides the edge padding.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            add_buttons(ui);
        });
    });
}
