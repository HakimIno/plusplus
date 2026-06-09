//! `ui` — egui views, widgets and application state for plusplus.
//!
//! The entry point is [`DbGuiApp`], which implements [`eframe::App`]. The `app` crate
//! constructs it and runs it; this crate owns all rendering and UI state.

mod app;
mod edit;
mod filter;
mod grid;
mod highlight;
mod icons;
mod style;
mod theme;
mod title_bar;

pub use app::DbGuiApp;

/// The custom font family used for headings, rendered with SF Pro Text Semibold.
///
/// Register it via [`install_fonts`] and select it from a [`egui::FontId`] with
/// `FontFamily::Name(HEADING_FAMILY.into())`.
pub const HEADING_FAMILY: &str = "heading";

/// Raw bytes of the fonts the app embeds. SF Pro Text (Apple's system UI typeface) has no
/// Thai glyphs, so the Thai script is covered by Anuphan — a loopless, geometric Thai face
/// that pairs cleanly with SF Pro — in matching Regular and Semibold weights.
pub struct AppFonts<'a> {
    /// SF Pro Text Regular — the primary proportional/UI font for Latin.
    pub sf_regular: &'a [u8],
    /// SF Pro Text Semibold — the Latin weight for the [`HEADING_FAMILY`] family.
    pub sf_semibold: &'a [u8],
    /// Anuphan Regular — Thai fallback for the proportional and monospace families.
    pub thai_regular: &'a [u8],
    /// Anuphan Semibold — Thai weight for the [`HEADING_FAMILY`] family.
    pub thai_semibold: &'a [u8],
}

/// Install the app's fonts, making SF Pro Text the primary proportional typeface so the
/// UI gets a native, macOS-like look, with Anuphan as the Thai fallback so Thai glyphs
/// still render in headers, cells, and the SQL editor.
///
/// SF Pro replaces egui's default proportional font; the monospace family keeps egui's
/// built-in monospace (the repo we vendor from ships no SF Mono) and only gains the Thai
/// fallback. A dedicated [`HEADING_FAMILY`] family is registered with the Semibold weight
/// of both SF Pro (Latin) and Anuphan (Thai).
pub fn install_fonts(ctx: &egui::Context, app_fonts: &AppFonts) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;

    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "sf_pro".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.sf_regular.to_vec())),
    );
    fonts.font_data.insert(
        "sf_pro_semibold".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.sf_semibold.to_vec())),
    );
    fonts.font_data.insert(
        "thai".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.thai_regular.to_vec())),
    );
    fonts.font_data.insert(
        "thai_semibold".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.thai_semibold.to_vec())),
    );

    // SF Pro Text leads the proportional family; Anuphan trails as the Thai fallback.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "sf_pro".to_owned());
    proportional.push("thai".to_owned());

    // Monospace keeps egui's default font and just gains the Thai fallback.
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push("thai".to_owned());

    // A heavier family for headings: SF Pro Semibold for Latin, Anuphan Semibold for Thai,
    // falling back to the regular weights.
    fonts.families.insert(
        FontFamily::Name(HEADING_FAMILY.into()),
        vec![
            "sf_pro_semibold".to_owned(),
            "thai_semibold".to_owned(),
            "sf_pro".to_owned(),
            "thai".to_owned(),
        ],
    );

    ctx.set_fonts(fonts);
}
