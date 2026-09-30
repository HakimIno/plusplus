//! Commands shared by the macOS menu bar and the existing application actions.
use super::*;

#[derive(Clone, Copy)]
pub enum NativeMenuCommand {
    NewTab,
    CloseTab,
    NewConnection,
    Settings,
    OpenAnything,
    BeautifySql,
    Help,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
}

impl NativeMenuCommand {
    pub fn enqueue(self, ctx: &egui::Context) {
        ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Vec<Self>>(egui::Id::new("native_menu"))
                .push(self);
        });
        ctx.request_repaint();
    }
}

impl DbGuiApp {
    pub(super) fn native_menu_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let commands = ctx.data_mut(|data| {
            data.remove_temp::<Vec<NativeMenuCommand>>(egui::Id::new("native_menu"))
                .unwrap_or_default()
        });
        for command in commands {
            use NativeMenuCommand::*;
            // The welcome screen has no workspace yet.
            if self.show_welcome
                && !matches!(command, Help | Copy | Cut | Paste | SelectAll | Undo | Redo)
            {
                continue;
            }
            match command {
                NewTab => self.apply_action(if self.split_tab.is_some() {
                    Action::NewSplitPaneTab(self.split_focus)
                } else {
                    Action::NewTab
                }),
                CloseTab if self.settings_open => self.settings_open = false,
                CloseTab if !self.tabs.is_empty() => {
                    let action = if let Some(split) = self.split_tab {
                        Action::CloseSplitPaneTab {
                            idx: if self.split_focus {
                                split
                            } else {
                                self.active_query_tab
                            },
                            right: self.split_focus,
                        }
                    } else {
                        Action::CloseTab(self.active_query_tab)
                    };
                    self.apply_action(action);
                }
                CloseTab => {}
                NewConnection => {
                    self.settings_open = false;
                    self.apply_action(Action::NewConnection);
                }
                Settings => self.apply_action(Action::OpenSettings),
                OpenAnything => self.open_open_anything(),
                BeautifySql if !self.tabs.is_empty() => self.apply_action(Action::BeautifySql),
                BeautifySql => {}
                Help => ctx.open_url(egui::OpenUrl::new_tab(
                    "https://github.com/HakimIno/plusplus#readme",
                )),
                Copy => input.events.push(egui::Event::Copy),
                Cut => input.events.push(egui::Event::Cut),
                Paste => {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            input.events.push(egui::Event::Paste(text));
                        }
                    }
                }
                SelectAll | Undo | Redo => {
                    let key = if matches!(command, SelectAll) {
                        egui::Key::A
                    } else {
                        egui::Key::Z
                    };
                    // egui-winit reports Cmd on macOS as both `mac_cmd` and the portable
                    // `command`; widgets (TextEdit select-all/undo) test only the latter.
                    let modifiers = egui::Modifiers {
                        shift: matches!(command, Redo),
                        command: true,
                        ..egui::Modifiers::MAC_CMD
                    };
                    input.modifiers = modifiers;
                    for pressed in [true, false] {
                        input.events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_menu_reopens_an_empty_workspace_and_delivers_copy_once() {
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        app.tabs.clear();
        let ctx = egui::Context::default();
        NativeMenuCommand::CloseTab.enqueue(&ctx);
        NativeMenuCommand::NewTab.enqueue(&ctx);
        NativeMenuCommand::Copy.enqueue(&ctx);
        let mut input = egui::RawInput::default();
        app.native_menu_input(&ctx, &mut input);
        assert_eq!(app.tabs.len(), 1);
        assert!(matches!(input.events.as_slice(), [egui::Event::Copy]));
        let mut next = egui::RawInput::default();
        app.native_menu_input(&ctx, &mut next);
        assert!(next.events.is_empty());
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn menu_select_all_selects_the_focused_sql_editor() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        crate::style::apply(&ctx);
        let mut app = DbGuiApp::construct();
        app.show_welcome = false;
        let tab = app.tab_mut();
        tab.kind = crate::components::QueryTabKind::Query;
        tab.sql = "SELECT 1; SELECT 2".into();
        tab.mark_sql_changed();
        let frame =
            |ctx: &egui::Context, app: &mut DbGuiApp, command: Option<NativeMenuCommand>| {
                let mut raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 700.0),
                    )),
                    ..Default::default()
                };
                if let Some(command) = command {
                    command.enqueue(ctx);
                    app.native_menu_input(ctx, &mut raw);
                }
                ctx.run_ui(raw, |ui| app.draw(ui, None));
            };
        frame(&ctx, &mut app, None);
        let editor = egui::Id::new(("sql_editor", app.tab().id, "primary"));
        ctx.memory_mut(|m| m.request_focus(editor));
        frame(&ctx, &mut app, None);
        frame(&ctx, &mut app, Some(NativeMenuCommand::SelectAll));
        assert!(
            ctx.memory(|m| m.has_focus(editor)),
            "focus must stay in the editor"
        );
        assert_eq!(app.tab().primary_cursor, 0..18);
    }
}
