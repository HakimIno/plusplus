//! Import rendering and interaction.

use super::details::kind_color;
use super::details::SCROLLBAR_GUTTER;
use crate::app::{Action, Busy, DbGuiApp};
use crate::components;
use crate::icons;
use crate::style::palette;

/// Semantic colour for a column's editor kind, used by the Details panel's type badges:
/// numbers amber, booleans green, dates/times blue, free text neutral.
/// Longest preview value shown before eliding. A `Label::truncate()` would shrink to whatever
/// width the grid cell happened to get, which collapsed timestamps to `2026-07…`; a fixed
/// character budget keeps every column legible and the layout predictable.
const IMPORT_PREVIEW_CHARS: usize = 28;

/// One cell of the import dialog's file preview: elided past [`IMPORT_PREVIEW_CHARS`], with the
/// full value on hover, and dimmed when no target column reads this source column.
fn preview_cell(ui: &mut egui::Ui, text: &str, dim: bool) {
    let color = if dim {
        palette::TEXT_FAINT()
    } else {
        palette::TEXT()
    };
    let short: String = text.chars().take(IMPORT_PREVIEW_CHARS).collect();
    let elided = short.chars().count() < text.chars().count();
    let shown = if elided { format!("{short}…") } else { short };
    let resp = ui.add(
        egui::Label::new(egui::RichText::new(shown).color(color))
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Extend),
    );
    if elided {
        resp.on_hover_text(text);
    }
}

impl DbGuiApp {
    /// Map a CSV/JSON file's columns onto the target table's, preview the result, and confirm.
    ///
    /// The dialog is built to answer three questions at a glance: *what file*, *which columns
    /// land where*, and *what is about to be written*. A leading status dot per row makes the
    /// mapped/skipped split scannable, the type badge matches the Details panel's colour
    /// language, and the preview dims the source columns nothing reads from.
    ///
    /// The source column list only ever chooses an *index*; the identifiers in the generated
    /// `INSERT` come from the table's introspected columns, so nothing in the file can reach
    /// the SQL as an identifier.
    pub(in crate::app) fn import_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(draft) = self.import_pending.as_ref() else {
            return;
        };
        let busy = self.busy;
        let target = draft.table_label();

        let binary = draft.binary_conflicts();
        let required = draft.unmapped_required();
        let mapped = draft.mapping.iter().filter(|m| m.is_some()).count();
        let can_import = busy == Busy::Idle && binary.is_empty() && mapped > 0;

        // Source columns that feed at least one target. The rest are dimmed in the preview so
        // the user can see exactly what the import ignores.
        let mut used = vec![false; draft.headers.len()];
        for src in draft.mapping.iter().flatten() {
            if let Some(slot) = used.get_mut(*src) {
                *slot = true;
            }
        }

        let mut open = true;
        components::dialog_window(format!("Import into {target}"))
            .open(&mut open)
            .resizable(true)
            // Width is fixed by design; height hugs the content and stops growing once the body
            // scroll hits its cap, so a six-column table gets a short dialog and a sixty-column
            // one doesn't run off the screen.
            .default_width(820.0)
            .frame(components::dialog_frame(ctx))
            .show(ctx, |ui| {
                // Everything except the footer lives in ONE vertical scroll — the file name,
                // the header switch, the callouts, the mapping, and the preview all move
                // together. Nothing above the buttons is pinned, so a long warning or a wide
                // table never squeezes the form. `auto_shrink` vertically means a short form
                // leaves no dead space above the footer; a long one scrolls at `MAX_BODY_H`.
                const MAX_BODY_H: f32 = 520.0;
                let has_columns = !draft.headers.is_empty();

                egui::ScrollArea::vertical()
                    .id_salt("import_body_scroll")
                    .max_height(MAX_BODY_H)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // ── Source file ────────────────────────────────────────
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            icons::show_weak(ui, icons::code(), icons::SIZE);
                            ui.label(egui::RichText::new(draft.file_name()).strong());
                            components::type_badge(ui, draft.format.label(), palette::ACCENT());
                        });

                        // JSON objects are always keyed by name, so the switch is CSV-only.
                        if draft.format == dbcore::ImportFormat::Csv {
                            ui.add_space(4.0);
                            // `accent_checkbox` allocates the box then the label in sequence, so
                            // it needs a horizontal layout or the label drops to the next line.
                            ui.horizontal(|ui| {
                                let mut has_header = draft.has_header;
                                if components::accent_checkbox(
                                    ui,
                                    busy == Busy::Idle,
                                    &mut has_header,
                                    Some("First row is a header"),
                                )
                                .on_hover_text("Uncheck if the file's first row is already data")
                                .changed()
                                {
                                    actions.push(Action::SetImportHasHeader(has_header));
                                }
                            });
                        }

                        // ── Problems ───────────────────────────────────────────
                        if !binary.is_empty() {
                            ui.add_space(8.0);
                            components::callout(
                                ui,
                                icons::warning(),
                                &format!(
                                    "Binary columns can't be imported: {}. Set them to “Skip”.",
                                    binary.join(", ")
                                ),
                                palette::DANGER(),
                            );
                        }
                        if !required.is_empty() {
                            ui.add_space(8.0);
                            components::callout(
                                ui,
                                icons::warning(),
                                &format!(
                                    "Not null and unmapped: {}. The import will fail unless the \
                                     database supplies a default.",
                                    required.join(", ")
                                ),
                                palette::WARNING(),
                            );
                        }

                        // A file with no columns has nothing to map — say so instead of drawing
                        // two empty grids. The footer still renders, outside this scroll.
                        if !has_columns {
                            components::empty_state(
                                ui,
                                icons::table(),
                                "This file has no columns",
                                "It looks empty. Pick another file, or uncheck “First row is a \
                                 header”.",
                            );
                            return;
                        }

                        ui.add_space(10.0);

                        // ── Column mapping ─────────────────────────────────────
                        ui.horizontal(|ui| {
                            components::section_header(ui, "Column mapping");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // The body's scrollbar is drawn over the content's right
                                    // edge; in a right-to-left layout the first thing added is
                                    // the rightmost, so this reserves the gutter.
                                    ui.add_space(SCROLLBAR_GUTTER);
                                    if components::button(
                                        ui,
                                        icons::close(),
                                        "Skip all",
                                        mapped > 0,
                                    )
                                    .on_hover_text("Unmap every column")
                                    .clicked()
                                    {
                                        actions.push(Action::ClearImportMapping);
                                    }
                                    if components::button(ui, icons::redo(), "Match by name", true)
                                        .on_hover_text(
                                            "Re-match columns by name, discarding manual choices",
                                        )
                                        .clicked()
                                    {
                                        actions.push(Action::AutoMapImport);
                                    }
                                },
                            );
                        });

                        egui::Grid::new("import_mapping_grid")
                            .num_columns(4)
                            .spacing([10.0, 7.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (i, col) in draft.table.columns.iter().enumerate() {
                                    let source = draft.mapping.get(i).copied().flatten();
                                    let is_binary = dbcore::import::is_binary_type(&col.data_type);
                                    let blocked = is_binary && source.is_some();

                                    // Dot: red = blocked, accent = mapped, faint = skipped.
                                    let dot = if blocked {
                                        palette::DANGER()
                                    } else if source.is_some() {
                                        palette::ACCENT()
                                    } else {
                                        palette::TEXT_FAINT()
                                    };
                                    components::status_dot(ui, dot);

                                    let name = egui::RichText::new(&col.name);
                                    ui.label(if source.is_some() {
                                        name.strong().color(palette::TEXT())
                                    } else {
                                        name.color(palette::TEXT_WEAK())
                                    });

                                    let kind = dbcore::EditorKind::classify(&col.data_type);
                                    let badge_color = if is_binary {
                                        palette::DANGER()
                                    } else {
                                        kind_color(kind)
                                    };
                                    components::type_badge(ui, &col.data_type, badge_color);

                                    let selected = source
                                        .and_then(|s| draft.headers.get(s))
                                        .map_or("Skip", String::as_str);
                                    // `ui.available_width()` here is the row's *remaining* width,
                                    // which on a wide dialog is enormous. A picker for one column
                                    // name has no business being 700px, so cap it — the space to
                                    // its right stays empty on purpose.
                                    let combo_w = (ui.available_width() - 4.0).clamp(180.0, 320.0);
                                    if let Some(source) = components::searchable_combo_box(
                                        ui,
                                        ("import_map", i),
                                        selected,
                                        combo_w,
                                        &draft.headers,
                                        source,
                                        Some("Skip"),
                                    ) {
                                        actions
                                            .push(Action::SetImportMapping { target: i, source });
                                    }
                                    ui.end_row();
                                }
                            });

                        // ── File preview ───────────────────────────────────────
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            components::section_header(ui, "File preview");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(SCROLLBAR_GUTTER);
                                    let note = if draft.more {
                                        format!("first {} rows", draft.preview_rows.len())
                                    } else {
                                        format!(
                                            "{} row{}",
                                            draft.preview_rows.len(),
                                            if draft.preview_rows.len() == 1 {
                                                ""
                                            } else {
                                                "s"
                                            }
                                        )
                                    };
                                    ui.label(
                                        egui::RichText::new(note).color(palette::TEXT_FAINT()),
                                    );
                                },
                            );
                        });

                        // Horizontal only: vertical scrolling belongs to the body above, so a
                        // wide file pans sideways without trapping the wheel.
                        egui::ScrollArea::horizontal()
                            .id_salt("import_preview_scroll")
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                egui::Grid::new("import_preview_grid")
                                    .striped(true)
                                    .spacing([16.0, 4.0])
                                    .show(ui, |ui| {
                                        for (s, header) in draft.headers.iter().enumerate() {
                                            let text = egui::RichText::new(header).strong();
                                            // Unused source columns read as ignored, not missing.
                                            ui.label(if used.get(s).copied().unwrap_or(false) {
                                                text.color(palette::TEXT())
                                            } else {
                                                text.color(palette::TEXT_FAINT()).strikethrough()
                                            });
                                        }
                                        ui.end_row();

                                        for row in &draft.preview_rows {
                                            for (s, field) in row.iter().enumerate() {
                                                let dim = !used.get(s).copied().unwrap_or(false);
                                                match field {
                                                    // A JSON null, not an empty string.
                                                    None => {
                                                        ui.label(
                                                            egui::RichText::new("NULL")
                                                                .italics()
                                                                .color(palette::TEXT_FAINT()),
                                                        );
                                                    }
                                                    Some(text) => preview_cell(ui, text, dim),
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                    });

                // ── Commit ─────────────────────────────────────────────────────
                // Pinned below the scroll, so the buttons are always reachable.
                components::dialog_footer(ui, |ui| {
                    if !has_columns {
                        if components::button(ui, icons::close(), "Cancel", true).clicked() {
                            actions.push(Action::CancelImport);
                        }
                        return;
                    }
                    let label = if mapped == 0 {
                        "Import".to_string()
                    } else {
                        format!(
                            "Import {mapped} column{}",
                            if mapped == 1 { "" } else { "s" }
                        )
                    };
                    let resp = components::primary_button(ui, icons::save(), &label, can_import);
                    // Say *why* the button is dead rather than leaving the user guessing.
                    let hint = if busy != Busy::Idle {
                        "Waiting for the current operation to finish"
                    } else if !binary.is_empty() {
                        "Skip the binary columns first"
                    } else if mapped == 0 {
                        "Map at least one column first"
                    } else {
                        "Read the whole file and insert every row in one transaction"
                    };
                    if resp.on_hover_text(hint).clicked() {
                        actions.push(Action::ConfirmImport);
                    }
                    if components::button(ui, icons::close(), "Cancel", true).clicked() {
                        actions.push(Action::CancelImport);
                    }
                    if busy == Busy::Importing {
                        ui.add(components::spinner(14.0));
                    }
                    // The title already names the table; say what gets left behind instead.
                    let skipped = draft.table.columns.len() - mapped;
                    if skipped > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{skipped} column{} skipped",
                                if skipped == 1 { "" } else { "s" }
                            ))
                            .color(palette::TEXT_FAINT()),
                        );
                    }
                });
            });

        if !open {
            actions.push(Action::CancelImport);
        }
    }
}
