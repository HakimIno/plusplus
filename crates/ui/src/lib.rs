//! `ui` — egui views, widgets and application state for plusplus.
//!
//! The entry point is [`DbGuiApp`], which implements [`eframe::App`]. The `app` crate
//! constructs it and runs it; this crate owns all rendering and UI state.

mod app;
mod appearance;
mod catalog;
mod components;
mod editor;
mod platform;
mod results;

// Stable module facade; implementations are grouped by responsibility above.
use appearance::{emoji, fonts, icons, style, theme};
use catalog::{erd, schema};
use editor::{autocomplete, editor_tools, fold, ghost, highlight, hover, query_error, sqlctx};
use platform::{pet, title_bar, update};
use results::{chart, edit, filter, format, grid, value_viewer};

pub use app::{open_connection_url, DbGuiApp};
pub use appearance::style::set_double_click_interval;
#[cfg(target_os = "macos")]
pub use app::NativeMenuCommand;

/// The custom font family used for headings, rendered with Inter Semibold.
///
/// Register it via [`install_fonts`] and select it from a [`egui::FontId`] with
/// `FontFamily::Name(HEADING_FAMILY.into())`.
pub const HEADING_FAMILY: &str = "heading";

/// IBM Plex Sans weight families for `FontFamily::Name(..)`. `FONT_REGULAR` matches
/// `FontFamily::Proportional`.
pub const FONT_THIN: &str = "plex-thin";
pub const FONT_REGULAR: &str = "plex-regular";
pub const FONT_MEDIUM: &str = "plex-medium";
pub const FONT_SEMIBOLD: &str = "plex-semibold";
pub const FONT_BOLD: &str = "plex-bold";

/// Raw bytes of the embedded fallback font. The Latin and Thai families (IBM Plex Sans and
/// Plex Sans Thai, five weights) are bundled directly by `fonts::install`.
#[derive(Clone, Copy)]
pub struct AppFonts {
    /// GNU Unifont — broad Unicode fallback used only when the fonts above lack a glyph.
    pub universal_regular: &'static [u8],
}

/// Install IBM Plex Sans, its Thai companion and the broad Unicode fallback.
pub fn install_fonts(ctx: &egui::Context, app_fonts: &AppFonts) {
    fonts::install(ctx, *app_fonts, None, None).expect("embedded fonts are valid");
}
