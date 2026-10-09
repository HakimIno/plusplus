//! Dialogs rendering and interaction.

use super::saved_queries::first_line;
use crate::app::{Action, Busy, DbGuiApp, ProductionGuardContinuation};
use crate::components;
use crate::icons;
use crate::style::palette;

/// Keep the review dialog responsive when a BLOB is represented by a large hexadecimal
/// literal. The complete statement remains in `commit_pending` and is what gets executed.
fn commit_statement_preview(statement: &str) -> std::borrow::Cow<'_, str> {
    const HEAD_BYTES: usize = 24 * 1024;
    const TAIL_BYTES: usize = 2 * 1024;
    if statement.len() <= HEAD_BYTES + TAIL_BYTES {
        return std::borrow::Cow::Borrowed(statement);
    }

    let mut head_end = HEAD_BYTES;
    while !statement.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = statement.len() - TAIL_BYTES;
    while !statement.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let omitted = tail_start - head_end;
    std::borrow::Cow::Owned(format!(
        "{}\n/* … {} bytes omitted from preview … */\n{}",
        &statement[..head_end],
        omitted,
        &statement[tail_start..]
    ))
}

fn review_title(count: usize) -> String {
    format!("Review {count} Change{}", if count == 1 { "" } else { "s" })
}

fn review_sql<'a>(
    ui: &mut egui::Ui,
    id: &'static str,
    max_height: f32,
    statements: impl IntoIterator<Item = &'a str>,
) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Frame::new()
                .fill(palette::CODE_BG())
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 10.0;
                    for statement in statements {
                        let preview = commit_statement_preview(statement);
                        let mut job = crate::highlight::highlight_sql_cached(
                            ui.ctx(),
                            preview.as_ref(),
                            font.clone(),
                        );
                        job.wrap.max_width = ui.available_width().max(40.0);
                        ui.label(job);
                    }
                });
        });
}

impl DbGuiApp {
    /// Ask what to do with pending Structure edits before Cmd/Ctrl+R reloads the tab.
    pub(in crate::app) fn schema_reload_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(tab_id) = self.schema_reload_pending else {
            return;
        };
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            self.schema_reload_pending = None;
            return;
        };

        let mut open = true;
        components::dialog_window("Unsaved Structure Changes")
            .open(&mut open)
            .resizable(false)
            .default_size([460.0, 0.0])
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Save or discard your structure changes before reloading.",
                    )
                    .color(palette::TEXT_WEAK()),
                );
                components::dialog_footer(ui, |ui| {
                    if components::primary_button(ui, icons::save(), "Save", true).clicked() {
                        actions.push(Action::SelectTab(tab_index));
                        actions.push(Action::CancelSchemaReload);
                        actions.push(Action::GenerateSchema);
                    }
                    if components::button(ui, icons::trash(), "Discard", true).clicked() {
                        actions.push(Action::SelectTab(tab_index));
                        actions.push(Action::CancelSchemaReload);
                        actions.push(Action::DiscardSchemaChanges);
                        actions.push(Action::ReloadTableStructure);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelSchemaReload);
                    }
                });
            });
        if !open {
            actions.push(Action::CancelSchemaReload);
        }
    }

    /// Compact review modal with an optional SQL preview and Commit/Cancel buttons.
    pub(in crate::app) fn commit_preview_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        if self.danger_pending.as_ref().is_some_and(|pending| {
            matches!(pending.continuation, ProductionGuardContinuation::Edits)
        }) {
            return;
        }
        let Some(plan) = self.commit_pending.as_ref() else {
            return;
        };
        let stmts = &plan.statements;
        let sequential = plan.is_sequential();

        let title = review_title(stmts.len());
        let response = components::dialog_modal(ctx, "commit_preview", &title, 440.0, |ui| {
                egui::CollapsingHeader::new("SQL preview")
                    .id_salt("commit_sql_preview")
                    .default_open(false)
                    .show(ui, |ui| {
                        review_sql(
                            ui,
                            "commit_preview_scroll",
                            280.0,
                            stmts.iter().map(String::as_str),
                        );
                    });

                components::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle;
                    if components::primary_button(ui, icons::save(), "Commit", can_act)
                        .on_hover_text(if sequential { "Execute statements sequentially" } else { "Execute all statements in a single transaction" })
                        .clicked()
                    {
                        actions.push(Action::ConfirmEdits);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelEdits);
                    }
                });
        });

        if response.should_close() {
            actions.push(Action::CancelEdits);
        }
    }

    /// Choose the columns used to identify one row when a table has no primary key,
    /// or when a user prefers one of several unique indexes.
    pub(in crate::app) fn key_chooser_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(chooser) = self.key_chooser.as_ref() else {
            return;
        };
        let mut open = true;
        let mut selected = chooser.selected;
        components::dialog_window("Choose Row Key Columns")
            .open(&mut open)
            .resizable(false)
            .default_size([520.0, 0.0])
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Pick columns that identify a single row.",
                    )
                    .color(palette::TEXT_WEAK()),
                );
                ui.add_space(8.0);
                for (index, (label, columns)) in chooser.candidates.iter().enumerate() {
                    ui.radio_value(&mut selected, index, format!("{label}  ({})", columns.join(", ")));
                }
                components::dialog_footer(ui, |ui| {
                    if components::primary_button(ui, icons::key(), "Use key", true).clicked() {
                        actions.push(Action::SelectKeyCandidate(selected));
                        actions.push(Action::ConfirmKeyChooser);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelKeyChooser);
                    }
                });
            });
        if !open {
            actions.push(Action::CancelKeyChooser);
        } else if selected != chooser.selected {
            actions.push(Action::SelectKeyCandidate(selected));
        }
    }

    /// Small modal to name a query when saving a favorite. Enter or Save
    /// commits; Escape / Cancel / closing the window dismisses it.
    /// Renaming an existing query uses the side callout on the row instead.
    pub(in crate::app) fn favorite_name_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(draft) = self.favorite_pending.as_ref() else {
            return;
        };
        if draft.editing_id.is_some() {
            return;
        }
        let preview = first_line(&draft.sql).to_string();
        let title = "Save query to favorites";

        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .default_size([440.0, 0.0])
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Name").color(palette::TEXT_WEAK()));
                if let Some(draft) = self.favorite_pending.as_mut() {
                    let w = ui.available_width();
                    let resp = components::text_input(ui, &mut draft.name, "My query", w);
                    resp.request_focus();
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                }
                ui.add_space(6.0);
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(preview)
                            .font(font)
                            .color(palette::TEXT_FAINT()),
                    )
                    .truncate(),
                );
                components::dialog_footer(ui, |ui| {
                    if components::primary_button(ui, icons::save(), "Save", true).clicked() {
                        submit = true;
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            actions.push(Action::ConfirmSaveFavorite);
        } else if cancel || !open {
            actions.push(Action::CancelSaveFavorite);
        }
    }

    /// Name a new folder, or rename an existing one.
    pub(in crate::app) fn favorite_folder_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(draft) = self.folder_pending.as_ref() else {
            return;
        };
        if draft.from.is_some() {
            return; // Rename uses the side callout on the folder row.
        }
        let title = "New folder";
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        components::dialog_window(title)
            .open(&mut open)
            .resizable(false)
            .default_size([360.0, 0.0])
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Name").color(palette::TEXT_WEAK()));
                if let Some(draft) = self.folder_pending.as_mut() {
                    let w = ui.available_width();
                    let resp = components::text_input(ui, &mut draft.name, "Reports", w);
                    resp.request_focus();
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                }
                components::dialog_footer(ui, |ui| {
                    if components::primary_button(ui, icons::save(), "Save", true).clicked() {
                        submit = true;
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            actions.push(Action::ConfirmFavoriteFolder);
        } else if cancel || !open {
            actions.push(Action::CancelFavoriteFolder);
        }
    }

    /// Production Guardian review: immutable target context, read-only preflight evidence,
    /// risk per statement, and typed confirmation for Critical operations.
    pub(in crate::app) fn danger_confirm_dialog(
        &mut self,
        ctx: &egui::Context,
        actions: &mut Vec<Action>,
    ) {
        let Some(pending) = self.danger_pending.clone() else {
            return;
        };

        let title = review_title(pending.statements.len());
        let response = components::dialog_modal(ctx, "production_review", &title, 640.0, |ui| {
                let database = if pending.database.is_empty() {
                    "default database"
                } else {
                    &pending.database
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(egui::RichText::new("Production").color(palette::DANGER()));
                    ui.label(egui::RichText::new("·").color(palette::TEXT_FAINT()));
                    icons::show_weak(ui, icons::database(), 14.0);
                    ui.label(
                        egui::RichText::new(&pending.connection_name).color(palette::TEXT_WEAK()),
                    );
                    ui.label(egui::RichText::new("/").color(palette::TEXT_FAINT()));
                    ui.label(egui::RichText::new(database).color(palette::TEXT_WEAK()));
                });
                ui.add_space(8.0);

                review_sql(
                    ui,
                    "production_review_sql",
                    220.0,
                    dbcore::split_query_statements(&pending.sql),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Impact")
                        .strong()
                        .color(palette::TEXT()),
                );

                egui::ScrollArea::vertical()
                    .id_salt("danger_confirm_scroll")
                    .max_height(160.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        for (i, stmt) in pending.statements.iter().enumerate() {
                            let preflight =
                                pending.preflights.as_ref().and_then(|items| items.get(i));
                            let risk = pending.preflights.as_ref().map(|_| pending.risk(i));
                            let risk_label = match risk {
                                Some(dbcore::safety::RiskLevel::Low) => "Low",
                                Some(dbcore::safety::RiskLevel::Medium) => "Medium",
                                Some(dbcore::safety::RiskLevel::Critical) => "Critical",
                                None => "Checking",
                            };
                            let target = if stmt.targets.is_empty() {
                                "unknown target".to_string()
                            } else {
                                stmt.targets.join(", ")
                            };

                            if i > 0 {
                                ui.separator();
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(
                                    egui::RichText::new(stmt.kind.label())
                                        .small()
                                        .monospace()
                                        .color(palette::TEXT_WEAK()),
                                );
                                ui.label(egui::RichText::new(&target).color(palette::TEXT()));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        // Plain words; only Critical borrows the danger colour.
                                        let color = if risk_label == "Critical" {
                                            palette::DANGER()
                                        } else {
                                            palette::TEXT_WEAK()
                                        };
                                        ui.label(
                                            egui::RichText::new(risk_label).small().color(color),
                                        );
                                    },
                                );
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                match preflight {
                                    Some(preflight) => {
                                        let rows = preflight
                                            .affected_rows
                                            .map(|rows| format!("{rows} rows affected"))
                                            .or_else(|| {
                                                preflight.plan.as_ref().and_then(|plan| {
                                                    plan.estimated_rows
                                                        .map(|rows| format!("~{rows} rows estimated"))
                                                })
                                            })
                                            .unwrap_or_else(|| "Impact unknown".to_string());
                                        ui.label(
                                            egui::RichText::new(rows)
                                                .small()
                                                .color(palette::TEXT_FAINT()),
                                        );
                                    }
                                    None => {
                                        ui.spinner();
                                        ui.label(
                                            egui::RichText::new("Checking impact…")
                                                .small()
                                                .color(palette::TEXT_FAINT()),
                                        );
                                    }
                                }
                                let mut flags = Vec::new();
                                if stmt.missing_where {
                                    flags.push("No WHERE");
                                }
                                if preflight
                                    .and_then(|item| item.plan.as_ref())
                                    .is_some_and(|plan| plan.full_scan)
                                {
                                    flags.push("Full scan");
                                }
                                if !flags.is_empty() {
                                    ui.label(
                                        egui::RichText::new(format!("· {}", flags.join(" · ")))
                                            .small()
                                            .color(palette::DANGER()),
                                    );
                                }
                            });

                            let has_details = stmt.analysis_warning.is_some()
                                || preflight
                                    .is_some_and(|item| item.plan.is_some() || !item.warnings.is_empty());
                            if has_details {
                                egui::CollapsingHeader::new(
                                    egui::RichText::new("Details")
                                        .small()
                                        .color(palette::TEXT_WEAK()),
                                )
                                .id_salt(("production_guard_details", i))
                                .show(ui, |ui| {
                                    if let Some(warning) = &stmt.analysis_warning {
                                        ui.label(
                                            egui::RichText::new(warning)
                                                .small()
                                                .color(palette::TEXT_WEAK()),
                                        );
                                    }
                                    if let Some(plan) = preflight.and_then(|item| item.plan.as_ref())
                                    {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Plan · {}{}{}",
                                                plan.scan_type
                                                    .as_deref()
                                                    .unwrap_or("scan type unavailable"),
                                                plan.index
                                                    .as_deref()
                                                    .map(|index| format!(" · index {index}"))
                                                    .unwrap_or_default(),
                                                plan.estimated_rows
                                                    .map(|rows| format!(" · ~{rows} rows"))
                                                    .unwrap_or_default(),
                                            ))
                                            .small()
                                            .color(palette::TEXT_FAINT()),
                                        );
                                    }
                                    if let Some(preflight) = preflight {
                                        for warning in &preflight.warnings {
                                            ui.label(
                                                egui::RichText::new(warning)
                                                    .small()
                                                    .color(palette::TEXT_WEAK()),
                                            );
                                        }
                                    }
                                });
                            }
                        }
                    });

                if let Some(phrase) = pending.confirmation_phrase() {
                    ui.add_space(12.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(
                            egui::RichText::new("Confirm by typing").color(palette::TEXT_WEAK()),
                        );
                        ui.label(
                            egui::RichText::new(phrase)
                                .monospace()
                                .color(palette::TEXT())
                                .background_color(palette::SURFACE()),
                        );
                    });
                    ui.add_space(6.0);
                    let mut confirmation = pending.confirmation.clone();
                    let resp =
                        components::text_input(ui, &mut confirmation, phrase, ui.available_width());
                    let focus_id = ui.id().with("danger_confirm_focus");
                    let already_focused = ui
                        .ctx()
                        .data(|data| data.get_temp::<bool>(focus_id).unwrap_or(false));
                    if !already_focused {
                        resp.request_focus();
                        ui.ctx().data_mut(|data| data.insert_temp(focus_id, true));
                    }
                    if resp.changed() {
                        actions.push(Action::SetDangerConfirmation(confirmation));
                    }
                }

                components::dialog_footer(ui, |ui| {
                    let can_act = self.busy == Busy::Idle && pending.can_confirm();
                    let critical = pending.confirmation_phrase().is_some();
                    let staged_edits =
                        matches!(pending.continuation, ProductionGuardContinuation::Edits);
                    let label = if staged_edits { "Commit" } else { "Run change" };
                    let icon = if staged_edits {
                        icons::save()
                    } else {
                        icons::play()
                    };
                    let run = if critical {
                        components::Btn::danger(label)
                            .icon(icon)
                            .enabled(can_act)
                            .tooltip("Execute against the production connection")
                            .show(ui)
                    } else {
                        components::primary_button(ui, icon, label, can_act)
                            .on_hover_text("Execute against the production connection")
                    };
                    if run.clicked() {
                        actions.push(Action::ConfirmDangerQuery);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelDangerQuery);
                    }
                });
        });

        if response.should_close() {
            actions.push(Action::CancelDangerQuery);
        }
    }
}

#[cfg(test)]
mod commit_statement_preview_tests {
    use super::commit_statement_preview;

    #[test]
    fn large_binary_sql_is_bounded_without_changing_the_statement() {
        let sql = format!(
            "UPDATE t SET image = X'{}' WHERE id = 'ภาษาไทย';",
            "AB".repeat(40_000)
        );
        let preview = commit_statement_preview(&sql);

        assert!(preview.len() < 30_000);
        assert!(preview.contains("bytes omitted from preview"));
        assert!(preview.ends_with("WHERE id = 'ภาษาไทย';"));
        assert!(sql.len() > 80_000);
    }
}
