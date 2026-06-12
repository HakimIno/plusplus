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

/// macOS: with `FullSizeContentView` + a transparent titlebar, AppKit still treats a
/// mouse-down in the titlebar strip as a window drag, because the content view's default
/// `mouseDownCanMoveWindow` answers YES. The drag then swallows the mouse-up, so egui
/// buttons drawn into that strip never register a click.
///
/// Swap the content view's class for a runtime subclass whose `mouseDownCanMoveWindow`
/// answers NO. Clicks in the titlebar strip then reach egui; window dragging stays
/// explicit via `ViewportCommand::StartDrag` on the breadcrumb (which uses
/// `performWindowDragWithEvent:` and is unaffected by this override).
#[cfg(target_os = "macos")]
fn fix_titlebar_click_through(cc: &eframe::CreationContext<'_>) {
    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::sel;
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use std::ffi::CString;

    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };

    extern "C-unwind" fn no(_this: &AnyObject, _sel: Sel) -> Bool {
        Bool::NO
    }

    unsafe {
        let view: &AnyObject = appkit.ns_view.cast::<AnyObject>().as_ref();
        let superclass = view.class();
        let Ok(name) = CString::new(format!("{}_NoTitlebarDrag", superclass.name().to_string_lossy()))
        else {
            return;
        };
        // Register once; reuse on subsequent windows.
        let subclass = AnyClass::get(&name).unwrap_or_else(|| {
            let mut builder = ClassBuilder::new(&name, superclass)
                .expect("titlebar-drag subclass name already taken");
            builder.add_method(
                sel!(mouseDownCanMoveWindow),
                no as extern "C-unwind" fn(_, _) -> _,
            );
            builder.register()
        });
        // No ivars added, so the instance size is unchanged and the swap is safe.
        AnyObject::set_class(view, subclass);
    }
}

const APP_ICON: &[u8] = include_bytes!("../assets/icon/png/icon-256.png");

fn main() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(APP_ICON).expect("valid app icon PNG");


    
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title(format!("plusplus v{}", env!("CARGO_PKG_VERSION")))
        .with_icon(icon);

    // Native macOS traffic lights/titlebar, with egui drawing into the titlebar space.
    // Note: eframe cannot put egui controls above AppKit hit-testing; this is best-effort.
    #[cfg(target_os = "macos")]
    {
        viewport = viewport
            .with_decorations(true)
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(true)
            .with_movable_by_background(false);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "plusplus",
        native_options,
        Box::new(|cc| {
            #[cfg(target_os = "macos")]
            fix_titlebar_click_through(cc);
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
