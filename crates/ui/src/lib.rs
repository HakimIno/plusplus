//! `ui` — egui views, widgets and application state for plusplus.
//!
//! The entry point is [`DbGuiApp`], which implements [`eframe::App`]. The `app` crate
//! constructs it and runs it; this crate owns all rendering and UI state.

mod app;
mod autocomplete;
mod components;
mod edit;
mod emoji;
mod erd;
mod filter;
mod format;
mod ghost;
mod grid;
mod highlight;
mod icons;
mod pet;
mod schema;
mod style;
mod theme;
mod title_bar;
mod update;

pub use app::DbGuiApp;

/// The custom font family used for headings, rendered with Inter Semibold.
///
/// Register it via [`install_fonts`] and select it from a [`egui::FontId`] with
/// `FontFamily::Name(HEADING_FAMILY.into())`.
pub const HEADING_FAMILY: &str = "heading";

/// Raw bytes of the fonts the app embeds.
pub struct AppFonts<'a> {
    /// Inter Regular — the primary UI font.
    pub ui_regular: &'a [u8],
    /// Inter Semibold — the weight for the [`HEADING_FAMILY`] family.
    pub ui_semibold: &'a [u8],
    /// JetBrains Mono Regular — SQL/code font.
    pub code_regular: &'a [u8],
    /// Anuphan Regular — Thai fallback for proportional and monospace families.
    pub thai_regular: &'a [u8],
    /// Anuphan Semibold — Thai weight for the [`HEADING_FAMILY`] family.
    pub thai_semibold: &'a [u8],
}

/// Install Inter for UI, JetBrains Mono for SQL/code, and Anuphan as Thai fallback.
pub fn install_fonts(ctx: &egui::Context, app_fonts: &AppFonts) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;

    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.ui_regular.to_vec())),
    );
    fonts.font_data.insert(
        "inter_semibold".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.ui_semibold.to_vec())),
    );
    fonts.font_data.insert(
        "jetbrains_mono".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.code_regular.to_vec())),
    );
    fonts.font_data.insert(
        "thai".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.thai_regular.to_vec())),
    );
    fonts.font_data.insert(
        "thai_semibold".to_owned(),
        Arc::new(FontData::from_owned(app_fonts.thai_semibold.to_vec())),
    );

    // Inter leads the proportional family; Anuphan trails as the Thai fallback.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "inter".to_owned());
    proportional.push("thai".to_owned());

    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.insert(0, "jetbrains_mono".to_owned());
    monospace.push("thai".to_owned());

    // A heavier family for headings: Inter Semibold first, Anuphan Semibold for Thai.
    fonts.families.insert(
        FontFamily::Name(HEADING_FAMILY.into()),
        vec![
            "inter_semibold".to_owned(),
            "thai_semibold".to_owned(),
            "inter".to_owned(),
            "thai".to_owned(),
        ],
    );

    ctx.set_fonts(fonts);
}
