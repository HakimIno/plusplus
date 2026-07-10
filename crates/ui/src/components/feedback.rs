//! Loading, empty, and inline-message states.

use crate::style::palette;

pub(crate) fn spinner(size: f32) -> egui::Spinner {
    egui::Spinner::new().size(size).color(palette::ACCENT())
}

/// An inline banner carrying a warning or error inside a dialog or panel: an icon and wrapped
/// text on a tinted, bordered background. `color` drives border, icon, and text — pass
/// `palette::DANGER()` for something that blocks, `palette::WARNING()` for something that
/// merely might. The tint alphas match [`super::type_badge`] so the two read as one system.
pub(crate) fn callout(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    text: &str,
    color: egui::Color32,
) {
    let tint = |a: u8| egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a);
    egui::Frame::new()
        .fill(tint(22))
        .stroke(egui::Stroke::new(1.0, tint(64)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add(
                    egui::Image::new(icon)
                        .fit_to_exact_size(egui::vec2(14.0, 14.0))
                        .tint(color),
                );
                ui.label(egui::RichText::new(text).color(color));
            });
        });
}

pub(crate) fn loading_state(ui: &mut egui::Ui, message: &str) {
    ui.add_space((ui.available_height() * 0.30).max(24.0));
    ui.vertical_centered(|ui| {
        ui.add(spinner(32.0));
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new(message)
                .size(14.5)
                .color(palette::TEXT_WEAK()),
        );
    });
}

pub(crate) fn empty_state(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    title: &str,
    hint: &str,
) {
    ui.add_space((ui.available_height() * 0.30).max(24.0));
    ui.vertical_centered(|ui| {
        ui.add(
            egui::Image::new(icon)
                .fit_to_exact_size(egui::vec2(40.0, 40.0))
                .tint(palette::TEXT_FAINT()),
        );
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(title)
                .size(14.5)
                .color(palette::TEXT_WEAK()),
        );
        ui.add_space(3.0);
        ui.label(egui::RichText::new(hint).color(palette::TEXT_FAINT()));
    });
}

pub(crate) fn empty_illustration(ui: &mut egui::Ui) {
    crate::pet::show(ui);
}
