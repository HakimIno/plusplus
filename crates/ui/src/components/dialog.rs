//! Dialog chrome shared by modal surfaces.

use egui::{Margin, Stroke, Vec2};

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

pub(crate) fn dialog_footer(ui: &mut egui::Ui, add_buttons: impl FnOnce(&mut egui::Ui)) {
    let t = crate::theme::current();
    let margin = ui.style().spacing.window_margin;
    let bar_h = CONTROL_H + margin.topf() + margin.bottomf();
    let bleed_x = margin.leftf() + margin.rightf();

    ui.add_space(8.0);

    let body_w = ui.min_rect().width();
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(body_w, bar_h), egui::Sense::hover());
    let paint_rect = egui::Rect::from_min_size(
        row_rect.min - egui::vec2(margin.leftf(), 0.0),
        egui::vec2(row_rect.width() + bleed_x, row_rect.height() + margin.bottomf()),
    );
    if ui.is_rect_visible(paint_rect) {
        let mut round = ui.style().visuals.window_corner_radius;
        round.nw = 0;
        round.ne = 0;
        ui.painter().rect_filled(paint_rect, round, t.base);
        ui.painter()
            .hline(paint_rect.x_range(), paint_rect.top(), Stroke::new(1.0, t.border));
    }

    ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            add_buttons(ui);
        });
    });
}
