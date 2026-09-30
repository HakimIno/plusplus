//! Updates rendering and interaction.

use crate::app::{Action, DbGuiApp};
use crate::components;
use crate::icons;
use crate::style::palette;

impl DbGuiApp {
    pub(in crate::app) fn update_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.update_dialog_open {
            return;
        }

        let current = crate::update::CURRENT_VERSION;
        let mut open = true;
        let mut close = false;
        let mut dismiss = false;
        let mut download = false;
        let mut install = false;

        let (title, version, notes, progress, ready, failed, downloading) = match &self.update {
            crate::update::UpdatePhase::Available(offer) => (
                "Update available",
                offer.version.clone(),
                offer.notes.clone(),
                None,
                false,
                None,
                false,
            ),
            crate::update::UpdatePhase::Downloading { offer, progress } => (
                "Downloading update",
                offer.version.clone(),
                offer.notes.clone(),
                Some(*progress),
                false,
                None,
                true,
            ),
            crate::update::UpdatePhase::Ready { offer, .. } => (
                "Ready to install",
                offer.version.clone(),
                offer.notes.clone(),
                Some(1.0),
                true,
                None,
                false,
            ),
            crate::update::UpdatePhase::Failed(msg) => (
                "Update failed",
                String::new(),
                String::new(),
                None,
                false,
                Some(msg.clone()),
                false,
            ),
            _ => return,
        };

        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                if !version.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("plusplus v{version}"))
                            .strong()
                            .size(16.0),
                    );
                    ui.label(
                        egui::RichText::new(format!("Current version: v{current}"))
                            .color(palette::TEXT_WEAK()),
                    );
                }
                ui.add_space(8.0);

                if let Some(p) = progress {
                    ui.add(egui::ProgressBar::new(p).show_percentage());
                    ui.add_space(8.0);
                }

                if let Some(err) = &failed {
                    ui.colored_label(palette::DANGER(), err);
                    ui.add_space(8.0);
                } else if !notes.trim().is_empty() {
                    components::section_header(ui, "Release notes");
                    egui::ScrollArea::vertical()
                        .id_salt("update_notes_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.label(notes.trim());
                        });
                    ui.add_space(8.0);
                }

                components::dialog_footer(ui, |ui| {
                    if ready {
                        if components::primary_button(ui, icons::save(), "Install & Restart", true)
                            .clicked()
                        {
                            install = true;
                        }
                    } else if downloading {
                        ui.add_enabled(false, egui::Button::new("Downloading…"));
                    } else if failed.is_some() {
                        if components::button(ui, icons::play(), "Retry download", true).clicked() {
                            download = true;
                        }
                    } else if components::primary_button(ui, icons::play(), "Download update", true)
                        .clicked()
                    {
                        download = true;
                    }

                    if components::button(ui, icons::close(), "Later", true).clicked() {
                        dismiss = true;
                    }
                    if components::button(ui, icons::close(), "Close", true).clicked() {
                        close = true;
                    }
                });
            });

        if install {
            actions.push(Action::InstallUpdate);
        }
        if download {
            actions.push(Action::DownloadUpdate);
        }
        if dismiss {
            actions.push(Action::DismissUpdate);
        }
        if !open || close {
            actions.push(Action::CloseUpdateDialog);
        }
    }

    pub(in crate::app) fn whats_new_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        if !self.show_whats_new {
            return;
        }

        let mut open = true;
        let mut close = false;

        components::dialog_window("What's New")
            .open(&mut open)
            .resizable(false)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    egui::RichText::new(format!("plusplus v{}", crate::update::CURRENT_VERSION))
                        .strong()
                        .size(16.0),
                );
                ui.add_space(8.0);

                components::section_header(ui, "Release notes");
                egui::ScrollArea::vertical()
                    .id_salt("whats_new_notes_scroll")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.label("• Implement query history feature with local audit log\n• Refactor dialog UI components for improved consistency and layout\n• Improve light-mode readability\n• Added \"What's New\" dialog on update");
                    });
                ui.add_space(8.0);

                components::dialog_footer(ui, |ui| {
                    if components::primary_button(ui, icons::play(), "Awesome", true).clicked() {
                        close = true;
                    }
                });
            });

        if !open || close {
            actions.push(Action::DismissWhatsNew);
        }
    }
}
