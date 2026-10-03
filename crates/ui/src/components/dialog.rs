//! Dialog chrome shared by modal surfaces.

use egui::{Margin, Vec2};

use crate::style::CONTROL_H;

pub(crate) fn dialog_window(title: impl Into<egui::WidgetText>) -> egui::Window<'static> {
    egui::Window::new(title)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
}

pub(crate) fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    egui::Frame::window(&style).inner_margin(Margin::symmetric(12, 4))
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
        // at the far right. A trailing gap keeps the cluster off the resize grip.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(12.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            add_buttons(ui);
        });
    });
}
