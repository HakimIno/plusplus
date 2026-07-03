//! plusplus — native database GUI. This crate is just the eframe entry point: it sets up
//! the window, installs the fonts, and hands control to [`ui::DbGuiApp`].

// On Windows, don't pop up a console window alongside the GUI in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Inter, embedded as the primary UI font so the interface stays crisp and portable.
const INTER_REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
/// Inter Semibold, used for headings and compact emphasis.
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/Inter-SemiBold.ttf");

/// JetBrains Mono, embedded for SQL editors, result values, and code-like metadata.
const JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");

/// Anuphan (loopless Thai, OFL-licensed) covers Thai glyphs and pairs cleanly with Inter.
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

/// Append every panic (UI thread or a background query worker) to a crash log beside the
/// app's config, with a full backtrace, then run the default hook. Launched from Finder the
/// process has no terminal, so without this a panic just vanishes — this turns "the app
/// disappeared" into a file we can actually read. The default hook still runs after, so
/// behaviour (abort/unwind) is unchanged.
fn install_crash_logger() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(dir) = dbcore::config::config_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("crash.log");
            let backtrace = std::backtrace::Backtrace::force_capture();
            let thread = std::thread::current();
            let entry = format!(
                "\n===== plusplus v{} crash @ {} (thread {:?}) =====\n{info}\n{backtrace}\n",
                env!("CARGO_PKG_VERSION"),
                dbcore::history::now_rfc3339(),
                thread.name().unwrap_or("unnamed"),
            );
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = f.write_all(entry.as_bytes());
            }
        }
        default(info);
    }));
}

fn main() -> eframe::Result<()> {
    install_crash_logger();
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

    #[cfg(not(target_os = "macos"))]
    {
        viewport = viewport
            .with_decorations(false)
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
                    ui_regular: INTER_REGULAR,
                    ui_semibold: INTER_SEMIBOLD,
                    code_regular: JETBRAINS_MONO_REGULAR,
                    thai_regular: THAI_REGULAR,
                    thai_semibold: THAI_SEMIBOLD,
                },
            );
            Ok(Box::new(ui::DbGuiApp::new(cc)))
        }),
    )
}
