//! plusplus — native database GUI. This crate is just the eframe entry point: it sets up
//! the window, installs the fonts, and hands control to [`ui::DbGuiApp`].

// On Windows, don't pop up a console window alongside the GUI in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// SF Pro Text (Apple's system UI typeface), embedded as the primary proportional font so
/// the UI looks native and crisp at small sizes regardless of the host's installed fonts.
const SF_PRO_REGULAR: &[u8] = include_bytes!("../assets/SF-Pro-Text-Regular.otf");
/// SF Pro Text Semibold, used for headings.
const SF_PRO_SEMIBOLD: &[u8] = include_bytes!("../assets/SF-Pro-Text-Semibold.otf");

/// Anuphan (loopless Thai, OFL-licensed) — SF Pro has no Thai glyphs, so these cover the
/// Thai script and pair cleanly with SF Pro. Embedded so the binary is self-contained.
const THAI_REGULAR: &[u8] = include_bytes!("../assets/Anuphan-Regular.ttf");
const THAI_SEMIBOLD: &[u8] = include_bytes!("../assets/Anuphan-SemiBold.ttf");

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("plusplus"),
        ..Default::default()
    };

    eframe::run_native(
        "plusplus",
        native_options,
        Box::new(|cc| {
            ui::install_fonts(
                &cc.egui_ctx,
                &ui::AppFonts {
                    sf_regular: SF_PRO_REGULAR,
                    sf_semibold: SF_PRO_SEMIBOLD,
                    thai_regular: THAI_REGULAR,
                    thai_semibold: THAI_SEMIBOLD,
                },
            );
            Ok(Box::new(ui::DbGuiApp::new(cc)))
        }),
    )
}
