//! db-gui — native database GUI. This crate is just the eframe entry point: it sets up
//! the window, installs the Thai font, and hands control to [`ui::DbGuiApp`].

// On Windows, don't pop up a console window alongside the GUI in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Noto Sans Thai, embedded so the binary is self-contained and Thai text renders
/// everywhere without relying on a system font.
const THAI_FONT: &[u8] = include_bytes!("../assets/NotoSansThai-Regular.ttf");

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("db-gui"),
        ..Default::default()
    };

    eframe::run_native(
        "db-gui",
        native_options,
        Box::new(|cc| {
            ui::install_fonts(&cc.egui_ctx, THAI_FONT);
            Ok(Box::new(ui::DbGuiApp::new(cc)))
        }),
    )
}
