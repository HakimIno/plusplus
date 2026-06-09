//! `ui` — egui views, widgets and application state for db-gui.
//!
//! The entry point is [`DbGuiApp`], which implements [`eframe::App`]. The `app` crate
//! constructs it and runs it; this crate owns all rendering and UI state.

mod app;
mod grid;
mod highlight;
mod icons;
mod style;
mod theme;

pub use app::DbGuiApp;

/// Install a Thai-capable font as a fallback for the proportional and monospace families,
/// so Thai glyphs render correctly in headers, cells, and the SQL editor.
///
/// `thai_font` is the raw bytes of a TTF/OTF that covers the Thai script (the `app` crate
/// embeds Noto Sans Thai). We append it as a fallback rather than replacing the default
/// fonts, so Latin text keeps egui's default look while Thai falls through to Noto.
pub fn install_fonts(ctx: &egui::Context, thai_font: &[u8]) {
    use egui::{FontData, FontDefinitions, FontFamily};

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "noto_thai".to_owned(),
        std::sync::Arc::new(FontData::from_owned(thai_font.to_vec())),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("noto_thai".to_owned());
    }
    ctx.set_fonts(fonts);
}
