//! Backup / Restore database dialog.

use crate::app::{Action, DbGuiApp};
use crate::components;
use crate::icons;
use crate::style::palette;
use dbcore::backup::{DumpFormat, Method};

impl DbGuiApp {
    /// The Backup / Restore dialog: what will be written or replaced, how, and where.
    ///
    /// It reads top to bottom as the questions the user answers — which database, which
    /// file, which options — and, for a restore, ends with the one thing that can't be
    /// undone spelled out and gated behind typing the database's name.
    pub(in crate::app) fn backup_dialog(
        &mut self,
        ctx: &egui::Context,
        _actions: &mut Vec<Action>,
    ) {
        let Some(dialog) = self.backup_dialog.as_mut() else {
            return;
        };
        let restore = dialog.restore;
        let running = dialog.running.is_some();
        let blocker = dialog.blocker();
        let (mut start, mut cancel, mut close, mut choose, mut reveal) =
            (false, false, false, false, false);
        let mut open = true;
        let title = if restore {
            "Restore Database"
        } else {
            "Backup Database"
        };

        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .default_width(600.0)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_width(600.0);
                egui::ScrollArea::vertical()
                    .id_salt("backup_dialog_body")
                    .max_height((ctx.content_rect().height() - 220.0).max(260.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_min_width(600.0);
                        ui.add_space(8.0);

                        // ── Which database ─────────────────────────────────────────────
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            icons::show_weak(
                                ui,
                                if restore {
                                    icons::database_import()
                                } else {
                                    icons::database_export()
                                },
                                18.0,
                            );
                            ui.label(
                                egui::RichText::new(&dialog.database)
                                    .strong()
                                    .size(15.0)
                                    .color(palette::TEXT()),
                            );
                            ui.label(egui::RichText::new(dialog.kind.label()).size(11.0).color(palette::TEXT_WEAK()));
                            if dialog.production {
                                ui.label(egui::RichText::new("Production").size(11.0).color(palette::DANGER()));
                            }
                        });
                        ui.label(
                            egui::RichText::new(format!("{} · {}", dialog.conn_name, dialog.target))
                                .size(11.5)
                                .color(palette::TEXT_WEAK()),
                        );
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(8.0);

                        // ── How ─────────────────────────────────────────────────────────
                        match (&dialog.tools, dialog.method) {
                            (_, Method::Unsupported) => message(
                                ui,
                                icons::warning(),
                                dbcore::backup::unsupported_reason(dialog.kind),
                                palette::WARNING(),
                            ),
                            (Some(Err(hint)), _) => {
                                message(ui, icons::warning(), hint, palette::DANGER())
                            }
                            (Some(Ok(tools)), _) => {
                                let tool = if restore {
                                    if dialog.archive || dialog.kind != dbcore::DbKind::Postgres {
                                        &tools.restore
                                    } else {
                                        &tools.load
                                    }
                                } else {
                                    &tools.dump
                                };
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Uses {}{}",
                                        tool.display(),
                                        if tools.version.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" · {}", tools.version)
                                        }
                                    ))
                                    .size(11.5)
                                    .color(palette::TEXT_FAINT()),
                                );
                            }
                            (None, _) => {}
                        }
                        if dialog.method == Method::Unsupported {
                            return;
                        }

                        // SQL Server: a local script (tables can be chosen) or a native .bak, which
                        // the server writes on its own disk.
                        if dialog.method == Method::Server {
                            section_label(ui, "Format");
                            section_frame(ui, |ui| {
                                let mut format = dialog.format;
                                ui.add_enabled_ui(!running, |ui| {
                                    egui::ComboBox::from_id_salt("sql_server_backup_format")
                                        .selected_text(match format {
                                            DumpFormat::PlainSql => "SQL script (.sql) — on this computer",
                                            DumpFormat::Archive => "Full backup (.bak) — on the server",
                                        })
                                        .width(ui.available_width())
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut format,
                                                DumpFormat::PlainSql,
                                                "SQL script (.sql) — on this computer",
                                            )
                                            .on_hover_text("CREATE TABLE and INSERT statements; choose tables, open it in SSMS");
                                            ui.selectable_value(
                                                &mut format,
                                                DumpFormat::Archive,
                                                "Full backup (.bak) — on the server",
                                            )
                                            .on_hover_text("BACKUP DATABASE … WITH COPY_ONLY: the whole database");
                                        });
                                });
                                if format != dialog.format {
                                    dialog.format = format;
                                    dialog.outcome = None;
                                    dialog.path = None;
                                }
                            });
                            if dialog.uses_server_path() {
                                ui.add_space(6.0);
                                message(
                                    ui,
                                    icons::database(),
                                    "SQL Server reads and writes .bak files on the database server's own \
                                     disk, so the path below is a path on the server.",
                                    palette::ACCENT(),
                                );
                            }
                        }

                        // ── Where ───────────────────────────────────────────────────────
                        section_label(ui, if restore { "Backup file" } else { "Save to" });
                        section_frame(ui, |ui| {
                            if dialog.uses_server_path() {
                                ui.add_enabled_ui(!running, |ui| {
                                    components::text_input(
                                        ui,
                                        &mut dialog.server_path,
                                        "D:\\Backups\\sales.bak",
                                        ui.available_width(),
                                    );
                                });
                            } else {
                                ui.horizontal(|ui| {
                                    let shown = dialog.path.as_ref().map_or_else(
                                        || "No file chosen".to_string(),
                                        |p| p.display().to_string(),
                                    );
                                    let width = (ui.available_width() - 96.0).max(120.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(width, 26.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.set_min_width(width);
                                            ui.add(
                                                egui::Label::new(egui::RichText::new(shown).color(
                                                    if dialog.path.is_some() {
                                                        palette::TEXT()
                                                    } else {
                                                        palette::TEXT_FAINT()
                                                    },
                                                ))
                                                .truncate(),
                                            );
                                        },
                                    );
                                    choose |= components::button(ui, icons::folder(), "Choose…", !running)
                                        .clicked();
                                });
                            }
                        });
                        ui.add_space(10.0);

                        // ── Options ─────────────────────────────────────────────────────
                        let kind = dialog.kind;
                        let pg = kind == dbcore::DbKind::Postgres;
                        let tool_based = dialog.method == Method::Tool;
                        if !restore && pg {
                            section_label(ui, "Format");
                            section_frame(ui, |ui| {
                                let mut format = dialog.format;
                                ui.add_enabled_ui(!running, |ui| {
                                    egui::ComboBox::from_id_salt("postgres_backup_format")
                                        .selected_text(match format {
                                            DumpFormat::Archive => "Archive (.dump)",
                                            DumpFormat::PlainSql => "Plain SQL (.sql)",
                                        })
                                        .width(ui.available_width())
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut format,
                                                DumpFormat::Archive,
                                                "Archive (.dump)",
                                            )
                                            .on_hover_text("Compressed; restore all or part of it with pg_restore");
                                            ui.selectable_value(
                                                &mut format,
                                                DumpFormat::PlainSql,
                                                "Plain SQL (.sql)",
                                            )
                                            .on_hover_text("A readable script, replayed with psql");
                                        });
                                });
                                if format != dialog.format {
                                    dialog.format = format;
                                    // The chosen file's extension follows the format.
                                    if let Some(path) = dialog.path.as_mut() {
                                        path.set_extension(dbcore::backup::default_extension(kind, format));
                                    }
                                }
                            });
                        }
                        let script = dialog.method == Method::Server && dialog.format == DumpFormat::PlainSql;
                        if (!restore && (tool_based || script)) || (restore && pg && dialog.archive) {
                            section_label(ui, "Options");
                            section_frame(ui, |ui| {
                                if !restore && (tool_based || script) {
                                    ui.horizontal(|ui| {
                                        components::accent_checkbox(
                                            ui,
                                            !running,
                                            &mut dialog.schema_only,
                                            Some("Structure only (no rows)"),
                                        );
                                    });
                                }
                                if restore && pg && dialog.archive {
                                    ui.horizontal(|ui| {
                                        components::accent_checkbox(
                                            ui,
                                            !running,
                                            &mut dialog.clean,
                                            Some("Drop existing objects first"),
                                        )
                                        .on_hover_text("pg_restore --clean --if-exists");
                                    });
                                }
                            });
                        }

                        // ── Tables ──────────────────────────────────────────────────────
                        if dialog.can_choose_tables() {
                            ui.add_space(10.0);
                            section_label(ui, "Tables");
                            section_frame(ui, |ui| table_picker(ui, dialog, running));
                        } else if !restore && dialog.method == Method::Server {
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("Choose SQL script to pick tables.").color(palette::TEXT_FAINT()));
                        }

                        // ── Restore: the consequence, then the gate ─────────────────────
                        if restore {
                            ui.add_space(6.0);
                            section_label(ui, "Confirm restore");
                            section_frame(ui, |ui| {
                                if dialog.read_only {
                                    message(
                                        ui,
                                        icons::warning(),
                                        "This connection is read-only, so it can't be restored into.",
                                        palette::WARNING(),
                                    );
                                } else {
                                    message(
                                        ui,
                                        icons::warning(),
                                        &format!(
                                            "Restoring replaces the data in {}{}. This can't be undone — \
                                             back it up first if you may need it.",
                                            dialog.database,
                                            if dialog.production {
                                                ", a production database"
                                            } else {
                                                ""
                                            }
                                        ),
                                        palette::DANGER(),
                                    );
                                    // A file restore swaps the whole database: name what it would drop.
                                    match &dialog.restore_missing {
                                        Some(Ok(missing)) if !missing.is_empty() => {
                                            ui.add_space(6.0);
                                            let shown: Vec<&str> =
                                                missing.iter().take(8).map(String::as_str).collect();
                                            let more = missing.len().saturating_sub(shown.len());
                                            message(
                                                ui,
                                                icons::warning(),
                                                &format!(
                                                    "This backup doesn't contain {} table{} that exist now; \
                                                     restoring removes {}: {}{}",
                                                    missing.len(),
                                                    if missing.len() == 1 { "" } else { "s" },
                                                    if missing.len() == 1 { "it" } else { "them" },
                                                    shown.join(", "),
                                                    if more > 0 {
                                                        format!(" and {more} more")
                                                    } else {
                                                        String::new()
                                                    }
                                                ),
                                                palette::DANGER(),
                                            );
                                        }
                                        Some(Err(error)) => {
                                            ui.add_space(6.0);
                                            message(
                                                ui,
                                                icons::warning(),
                                                &format!("Couldn't read the backup file: {error}"),
                                                palette::WARNING(),
                                            );
                                        }
                                        _ => {}
                                    }
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(format!("Type {} to confirm", dialog.database))
                                            .size(11.5)
                                            .color(palette::TEXT_WEAK()),
                                    );
                                    ui.add_enabled_ui(!running, |ui| {
                                        components::text_input(
                                            ui,
                                            &mut dialog.confirm,
                                            &dialog.database,
                                            ui.available_width(),
                                        );
                                    });
                                }
                            });
                        }

                        // ── Progress / outcome ──────────────────────────────────────────
                        if let Some(run) = &dialog.running {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0).color(palette::TEXT_WEAK()));
                                let secs = run.started.elapsed().as_secs();
                                let written = (!restore && !dialog.uses_server_path())
                                    .then_some(dialog.path.as_ref())
                                    .flatten()
                                    .and_then(|p| std::fs::metadata(p).ok())
                                    .map(|m| {
                                        format!(
                                            " · {}",
                                            crate::results::value_viewer::format_bytes(m.len())
                                        )
                                    })
                                    .unwrap_or_default();
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} {}:{:02}{written}",
                                        if restore {
                                            "Restoring…"
                                        } else {
                                            "Backing up…"
                                        },
                                        secs / 60,
                                        secs % 60
                                    ))
                                    .color(palette::TEXT_WEAK()),
                                );
                            });
                            // Elapsed time and file size tick without input.
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(500));
                        }
                        match &dialog.outcome {
                            Some(Ok(text)) => {
                                ui.add_space(10.0);
                                message(ui, icons::check(), text, palette::SUCCESS());
                            }
                            Some(Err(error)) => {
                                ui.add_space(10.0);
                                egui::ScrollArea::vertical()
                                    .id_salt("backup_error")
                                    .max_height(140.0)
                                    .show(ui, |ui| {
                                        message(ui, icons::warning(), error, palette::DANGER());
                                    });
                            }
                            None => {}
                        }
                    });

                // ── Actions ─────────────────────────────────────────────────────
                components::dialog_footer(ui, |ui| {
                    if running {
                        let cancellable = dialog.running.as_ref().is_some_and(|r| r.cancellable);
                        cancel |= components::button(ui, icons::close(), "Stop", cancellable)
                            .on_hover_text(if cancellable {
                                "Stop the tool; a partial backup file is removed"
                            } else {
                                "This step runs on the database and can't be stopped midway"
                            })
                            .clicked();
                        return;
                    }
                    let label = if restore { "Restore" } else { "Back Up" };
                    let hint = blocker.clone().unwrap_or_else(|| {
                        if restore {
                            "Replace the database with this backup".into()
                        } else {
                            "Write the backup file".into()
                        }
                    });
                    let resp = if restore {
                        components::Btn::danger(label)
                            .icon(icons::database_import())
                            .enabled(blocker.is_none())
                            .show(ui)
                    } else {
                        components::primary_button(
                            ui,
                            icons::database_export(),
                            label,
                            blocker.is_none(),
                        )
                    };
                    start |= resp.on_hover_text(hint).clicked();
                    let local_backup_done = !restore
                        && !dialog.uses_server_path()
                        && dialog.outcome.as_ref().is_some_and(|o| o.is_ok());
                    if local_backup_done {
                        reveal |=
                            components::button(ui, icons::folder(), "Show File", true).clicked();
                    }
                    close |= components::button(ui, icons::close(), "Close", true).clicked();
                });
            });

        if choose {
            self.choose_backup_file();
        }
        if start {
            self.start_backup_job();
        }
        if cancel {
            self.cancel_backup_job();
        }
        if reveal {
            if let Some(path) = self.backup_dialog.as_ref().and_then(|d| d.path.clone()) {
                let _ = crate::app::actions::reveal_in_file_manager(&path);
            }
        }
        // Closing while a job runs only hides nothing: the dialog stays until it finishes,
        // so its outcome (and a restore's reconnect) is never lost.
        if (close || !open) && !running {
            self.backup_dialog = None;
        }
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(10.0)
            .color(palette::TEXT_FAINT()),
    );
    ui.add_space(3.0);
}

fn section_frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette::SURFACE())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui);
        });
}

fn message(ui: &mut egui::Ui, icon: egui::ImageSource<'static>, text: &str, color: egui::Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add(
            egui::Image::new(icon)
                .fit_to_exact_size(egui::vec2(14.0, 14.0))
                .tint(color),
        );
        ui.label(egui::RichText::new(text).color(color));
    });
}

/// "All tables" or a searchable checklist of the connection's tables.
fn table_picker(ui: &mut egui::Ui, dialog: &mut crate::app::backup::BackupDialog, running: bool) {
    ui.add_enabled_ui(!running, |ui| {
        let total = dialog.available.len();
        let picked = dialog.selected.len();
        ui.horizontal(|ui| {
            components::accent_radio(
                ui,
                &mut dialog.all_tables,
                true,
                &format!("All tables ({total})"),
            );
            ui.add_space(12.0);
            components::accent_radio(
                ui,
                &mut dialog.all_tables,
                false,
                &format!("Selected tables ({picked})"),
            );
        });
        if dialog.all_tables {
            return;
        }
        ui.add_space(6.0);
        let filter = dialog.table_filter.to_lowercase();
        let label = |(schema, name): &(Option<String>, String)| match schema {
            Some(schema) => format!("{schema}.{name}"),
            None => name.clone(),
        };
        let visible: Vec<(Option<String>, String)> = dialog
            .available
            .iter()
            .filter(|t| filter.is_empty() || label(t).to_lowercase().contains(&filter))
            .cloned()
            .collect();
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 150.0).max(120.0);
            components::icon_text_input(
                ui,
                &mut dialog.table_filter,
                "Search tables…",
                icons::search(),
                width,
            );
            if ui.small_button("Select all").clicked() {
                dialog.selected.extend(visible.iter().cloned());
            }
            if ui.small_button("None").clicked() {
                dialog.selected.clear();
            }
        });
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(palette::CODE_BG())
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("backup_tables")
                    .max_height(160.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.label(
                                egui::RichText::new("No tables match").color(palette::TEXT_FAINT()),
                            );
                        }
                        for table in visible {
                            let mut on = dialog.selected.contains(&table);
                            if ui
                                .horizontal(|ui| {
                                    components::accent_checkbox(
                                        ui,
                                        true,
                                        &mut on,
                                        Some(&label(&table)),
                                    )
                                })
                                .inner
                                .changed()
                            {
                                if on {
                                    dialog.selected.insert(table);
                                } else {
                                    dialog.selected.remove(&table);
                                }
                            }
                        }
                    });
            });
    });
}
