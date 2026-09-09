//! `ui` — egui views, widgets and application state for plusplus.
//!
//! The entry point is [`DbGuiApp`], which implements [`eframe::App`]. The `app` crate
//! constructs it and runs it; this crate owns all rendering and UI state.

mod app;
mod autocomplete;
mod chart;
mod components;
mod edit;
mod editor_tools;
mod emoji;
mod erd;
mod filter;
mod fold;
mod fonts;
mod format;
mod ghost;
mod grid;
mod highlight;
mod icons;
mod pet;
mod query_error;
mod schema;
mod sqlctx;
mod style;
mod theme;
mod title_bar;
mod update;
mod value_viewer;

pub use app::DbGuiApp;

/// The custom font family used for headings, rendered with Inter Semibold.
///
/// Register it via [`install_fonts`] and select it from a [`egui::FontId`] with
/// `FontFamily::Name(HEADING_FAMILY.into())`.
pub const HEADING_FAMILY: &str = "heading";

/// Raw bytes of the legacy/fallback fonts the app embeds. The default Latin and
/// Thai families are installed by `fonts::install` from the bundled Geist and
/// Noto Sans Thai assets.
#[derive(Clone, Copy)]
pub struct AppFonts {
    /// Inter Regular — legacy Latin fallback.
    pub ui_regular: &'static [u8],
    /// Inter Semibold — legacy heading fallback.
    pub ui_semibold: &'static [u8],
    /// Anuphan Regular — Thai fallback for proportional and monospace families.
    pub thai_regular: &'static [u8],
    /// Anuphan Semibold — Thai weight for the [`HEADING_FAMILY`] family.
    pub thai_semibold: &'static [u8],
    /// GNU Unifont — broad Unicode fallback used only when the fonts above lack a glyph.
    pub universal_regular: &'static [u8],
}

/// Install the primary UI font followed by Thai and broad Unicode fallbacks.
pub fn install_fonts(ctx: &egui::Context, app_fonts: &AppFonts) {
    fonts::install(ctx, *app_fonts, None, None).expect("embedded fonts are valid");
}
