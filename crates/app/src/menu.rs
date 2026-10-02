//! Native macOS menus; workspace commands reuse the egui application's action handlers.
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSProcessInfo, NSString};
use std::cell::RefCell;
use ui::NativeMenuCommand;

thread_local! {
    // NSMenuItem targets are not retained by AppKit.
    static TARGET: RefCell<Option<(Retained<AnyObject>, egui::Context)>> = const { RefCell::new(None) };
}

const COMMANDS: &[NativeMenuCommand] = &[
    NativeMenuCommand::NewTab,
    NativeMenuCommand::CloseTab,
    NativeMenuCommand::NewConnection,
    NativeMenuCommand::Settings,
    NativeMenuCommand::OpenAnything,
    NativeMenuCommand::BeautifySql,
    NativeMenuCommand::Help,
    NativeMenuCommand::Copy,
    NativeMenuCommand::Cut,
    NativeMenuCommand::Paste,
    NativeMenuCommand::SelectAll,
    NativeMenuCommand::Undo,
    NativeMenuCommand::Redo,
    NativeMenuCommand::Quit,
];

extern "C-unwind" fn dispatch(_this: &AnyObject, _sel: Sel, item: &NSMenuItem) {
    if let Some(command) = COMMANDS.get(item.tag() as usize) {
        TARGET.with(|target| {
            if let Some((_, ctx)) = target.borrow().as_ref() {
                command.enqueue(ctx);
            }
        });
    }
}

fn item(mtm: MainThreadMarker, title: &str, action: Sel, key: &str) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    }
}

fn submenu(mtm: MainThreadMarker, bar: &NSMenu, title: &str) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setTitle(&NSString::from_str(title));
    let entry = NSMenuItem::new(mtm);
    entry.setTitle(&NSString::from_str(title));
    entry.setSubmenu(Some(&menu));
    bar.addItem(&entry);
    menu
}

fn command(mtm: MainThreadMarker, menu: &NSMenu, title: &str, key: &str, tag: usize) {
    // Text-editing commands (tags 7..=12) get no key equivalent: AppKit would swallow the
    // keystroke, flash the "Edit" title as if clicked, and hand egui a synthetic event.
    // Without one, winit delivers Cmd+A/Z/C/X/V straight to the focused widget.
    let key = if (7..=12).contains(&tag) { "" } else { key };
    let entry = item(mtm, title, sel!(plusplusMenuCommand:), key);
    entry.setTag(tag as isize);
    TARGET.with(|target| unsafe {
        entry.setTarget(target.borrow().as_ref().map(|(object, _)| &**object));
    });
    menu.addItem(&entry);
}

pub fn install(ctx: &egui::Context) {
    let mtm = MainThreadMarker::new().expect("menus must be installed on the main thread");
    NSProcessInfo::processInfo().setProcessName(&NSString::from_str("Plusplus"));
    let class = AnyClass::get(c"PlusplusMenuTarget").unwrap_or_else(|| {
        let mut builder =
            ClassBuilder::new(c"PlusplusMenuTarget", AnyClass::get(c"NSObject").unwrap()).unwrap();
        unsafe {
            builder.add_method(
                sel!(plusplusMenuCommand:),
                dispatch as extern "C-unwind" fn(_, _, _),
            );
        }
        builder.register()
    });
    let target: Retained<AnyObject> = unsafe { msg_send![class, new] };
    TARGET.with(|slot| *slot.borrow_mut() = Some((target, ctx.clone())));

    let app = NSApplication::sharedApplication(mtm);
    // Preserve winit's About, Services, Hide and Quit items.
    let bar = app.mainMenu().expect("winit installs the application menu");
    if let Some(root) = bar.itemAtIndex(0) {
        root.setTitle(&NSString::from_str("Plusplus"));
        if let Some(menu) = root.submenu() {
            menu.setTitle(&NSString::from_str("Plusplus"));
            for (index, title) in [
                (0, "About Plusplus"),
                (3, "Hide Plusplus"),
                (7, "Quit Plusplus"),
            ] {
                if let Some(entry) = menu.itemAtIndex(index) {
                    entry.setTitle(&NSString::from_str(title));
                    if index == 7 {
                        // Route Cmd+Q through the same unsaved-work guard as window close.
                        unsafe {
                            entry.setAction(Some(sel!(plusplusMenuCommand:)));
                            TARGET.with(|target| {
                                entry.setTarget(
                                    target.borrow().as_ref().map(|(object, _)| &**object),
                                );
                            });
                        }
                        entry.setTag(13);
                    }
                }
            }
            command(mtm, &menu, "Settings…", ",", 3);
        }
    }
    let file = submenu(mtm, &bar, "File");
    command(mtm, &file, "New Query Tab", "t", 0);
    command(mtm, &file, "New Connection…", "n", 2);
    command(mtm, &file, "Close Tab", "w", 1);
    let edit = submenu(mtm, &bar, "Edit");
    for (title, key, tag) in [
        ("Undo", "z", 11),
        ("Redo", "z", 12),
        ("Cut", "x", 8),
        ("Copy", "c", 7),
        ("Paste", "v", 9),
        ("Select All", "a", 10),
    ] {
        command(mtm, &edit, title, key, tag);
    }
    let view = submenu(mtm, &bar, "View");
    view.addItem(&item(
        mtm,
        "Toggle Full Screen",
        sel!(toggleFullScreen:),
        "",
    ));
    let tools = submenu(mtm, &bar, "Tools");
    command(mtm, &tools, "Beautify SQL", "i", 5);
    let navigate = submenu(mtm, &bar, "Navigate");
    command(mtm, &navigate, "Open Anything…", "p", 4);
    let window = submenu(mtm, &bar, "Window");
    window.addItem(&item(mtm, "Minimize", sel!(performMiniaturize:), "m"));
    window.addItem(&item(mtm, "Zoom", sel!(performZoom:), ""));
    app.setWindowsMenu(Some(&window));
    let help = submenu(mtm, &bar, "Help");
    command(mtm, &help, "Plusplus Help", "", 6);
    app.setHelpMenu(Some(&help));
}
