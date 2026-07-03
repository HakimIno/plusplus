//! Loading and empty states.

use crate::style::palette;

pub(crate) fn spinner(size: f32) -> egui::Spinner {
    egui::Spinner::new().size(size).color(palette::ACCENT())
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
