//! Details rendering and interaction.

use crate::app::{Action, DbGuiApp, QueryTab, TabView};
use crate::components;
use crate::icons;
use crate::style;
use crate::style::palette;

/// Width the body scroll's bar overlays on the right edge of its content. Section-header rows
/// right-align things into it, so they reserve this much.
pub(super) const SCROLLBAR_GUTTER: f32 = 14.0;

pub(super) fn kind_color(kind: crate::edit::EditorKind) -> egui::Color32 {
    use crate::edit::EditorKind as K;
    match kind {
        K::Int | K::Float | K::Decimal => palette::WARNING(),
        K::Bool => palette::SUCCESS(),
        K::Date | K::Time | K::DateTime => palette::ACCENT(),
        K::Text => palette::TEXT_FAINT(),
    }
}

/// Render one Details-panel value as an input-styled box (TablePlus-look): a bordered
/// full-width field showing the value, with a ⌄ actions menu at its right edge. While the
/// cell is actively being edited the box is replaced by the validated text editor.
///
/// Height of a Details-panel value box (display and edit modes share this).
const DETAILS_VALUE_H: f32 = 26.0;

/// Fixed height of one name/type/value field. A stable height lets `ScrollArea::show_rows`
/// build only the fields intersecting the viewport, even for very wide result schemas.
const DETAILS_FIELD_H: f32 = 64.0;

const DETAILS_IMAGE_H: f32 = 176.0;

/// More than can fit in the panel, but bounded so a multi-megabyte JSON/text cell does not
/// get cloned and shaped in full on every frame.
const DETAILS_PREVIEW_CHARS: usize = 256;

#[allow(clippy::too_many_arguments)]
fn details_field(
    ui: &mut egui::Ui,
    edits: &mut crate::edit::Edits,
    row_idx: usize,
    c: usize,
    col: &dbcore::ColumnMeta,
    value: &dbcore::Value,
    editable: bool,
    date_pick: &mut Option<(usize, usize)>,
    image_preview: &mut crate::value_viewer::ImagePreviewCache,
    tab_id: u64,
    actions: &mut Vec<Action>,
) {
    let kind = edits.col_kind(c);
    let shown = edits.staged(row_idx, c).unwrap_or(value);
    let has_image = crate::value_viewer::ValueViewer::kind(&col.type_name, shown)
        == Some(crate::value_viewer::ViewerKind::Image);
    let mut row_h = DETAILS_FIELD_H;
    if *date_pick == Some((row_idx, c)) {
        row_h += ui.spacing().interact_size.y + ui.spacing().item_spacing.y + 2.0;
    }
    if has_image {
        row_h += DETAILS_IMAGE_H + ui.spacing().item_spacing.y;
    }
    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::hover(),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(row_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.add_space(4.0);
            // Header: column name on the left, with a quiet colour-coded type label pinned to
            // the right edge. Details deliberately avoids badge chrome so values stay dominant.
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&col.name)
                        .strong()
                        .color(palette::TEXT()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(col.type_name.to_uppercase())
                            .size(10.0)
                            .strong()
                            .color(kind_color(kind)),
                    );
                });
            });
            details_value_box(
                ui, edits, kind, row_idx, c, col, value, editable, date_pick, actions,
            );
            let shown = edits.staged(row_idx, c).unwrap_or(value);
            if let dbcore::Value::Bytes(bytes) = shown {
                if crate::value_viewer::ValueViewer::kind(&col.type_name, shown)
                    == Some(crate::value_viewer::ViewerKind::Image)
                {
                    ui.add_space(ui.spacing().item_spacing.y);
                    details_image_thumbnail(
                        ui,
                        image_preview,
                        crate::value_viewer::ImagePreviewKey::new(tab_id, row_idx, c, bytes),
                        bytes,
                        col,
                        shown,
                        actions,
                    );
                }
            }
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn details_image_thumbnail(
    ui: &mut egui::Ui,
    cache: &mut crate::value_viewer::ImagePreviewCache,
    key: crate::value_viewer::ImagePreviewKey,
    bytes: &[u8],
    col: &dbcore::ColumnMeta,
    value: &dbcore::Value,
    actions: &mut Vec<Action>,
) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), DETAILS_IMAGE_H),
        egui::Sense::click(),
    );
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Open full-size image");
    let stroke = if response.hovered() {
        palette::ACCENT()
    } else {
        palette::BORDER()
    };
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(5),
        palette::CODE_BG(),
        egui::Stroke::new(1.0_f32, stroke),
        egui::StrokeKind::Inside,
    );

    match cache.get(ui.ctx(), key, bytes) {
        Ok(preview) => {
            let caption_h = 24.0;
            let image_area = egui::Rect::from_min_max(
                rect.min + egui::vec2(8.0, 8.0),
                egui::pos2(rect.right() - 8.0, rect.bottom() - caption_h),
            );
            let source = egui::vec2(preview.width as f32, preview.height as f32);
            let scale = (image_area.width() / source.x)
                .min(image_area.height() / source.y)
                .min(1.0);
            let image_rect = egui::Rect::from_center_size(image_area.center(), source * scale);
            ui.painter().image(
                preview.texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            let caption = format!(
                "{}  ·  {} × {} px  ·  {}",
                preview.format.to_ascii_uppercase(),
                preview.width,
                preview.height,
                crate::value_viewer::format_bytes(preview.bytes_len as u64)
            );
            ui.painter().text(
                egui::pos2(rect.center().x, rect.bottom() - 10.0),
                egui::Align2::CENTER_CENTER,
                caption,
                egui::FontId::monospace(10.0),
                palette::TEXT_FAINT(),
            );
        }
        Err(error) => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                error,
                egui::FontId::proportional(11.0),
                palette::TEXT_FAINT(),
            );
        }
    }

    if response.clicked() {
        if let Some(viewer) =
            crate::value_viewer::ValueViewer::new(&col.name, &col.type_name, value)
        {
            actions.push(Action::OpenValueViewer(viewer));
        }
    }
}

fn details_value_preview(value: &dbcore::Value, kind: crate::edit::EditorKind) -> String {
    use crate::edit::EditorKind as K;

    if kind == K::Bool && !value.is_null() {
        return if crate::edit::as_bool(value) {
            "TRUE".to_string()
        } else {
            "FALSE".to_string()
        };
    }
    let dbcore::Value::Text(text) = value else {
        return value.display();
    };

    let mut chars = text.chars();
    let mut preview = String::with_capacity(text.len().min(DETAILS_PREVIEW_CHARS + 1));
    for _ in 0..DETAILS_PREVIEW_CHARS {
        let Some(ch) = chars.next() else {
            return preview;
        };
        preview.push(if ch == '\n' || ch == '\r' || ch == '\t' {
            ' '
        } else {
            ch
        });
    }
    if chars.next().is_some() {
        preview.push('…');
    }
    preview
}

/// Type-aware behaviour:
/// - clicking the box starts editing (booleans toggle instead);
/// - the ⌄ menu offers Copy plus, when editable: Edit, type-specific quick-sets
///   (TRUE/FALSE, Today/Now, an inline calendar picker for DATE), Set NULL, and Revert;
/// - numbers and date/times render monospace; NULL/bytes render faint; staged (unsaved)
///   values render green with a green border until saved.
#[allow(clippy::too_many_arguments)]
fn details_value_box(
    ui: &mut egui::Ui,
    edits: &mut crate::edit::Edits,
    kind: crate::edit::EditorKind,
    row_idx: usize,
    c: usize,
    col: &dbcore::ColumnMeta,
    value: &dbcore::Value,
    editable: bool,
    date_pick: &mut Option<(usize, usize)>,
    actions: &mut Vec<Action>,
) {
    use crate::edit::EditorKind as K;

    if edits.is_active_from(row_idx, c, crate::edit::EditOrigin::Details) {
        // Keep the same painted box as display mode; only swap the inner label for a
        // frameless editor so focus doesn't add a second border and resize the row.
        let h = DETAILS_VALUE_H;
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
        // Border turns red while the typed value is invalid for the column, matching the
        // red text — clear feedback that the edit can't be committed yet.
        let valid = edits.active.as_ref().is_none_or(|a| a.check().is_ok());
        let border = if valid {
            palette::ACCENT()
        } else {
            palette::DANGER()
        };
        if ui.is_rect_visible(rect) {
            ui.painter().rect(
                rect,
                egui::CornerRadius::same(5),
                palette::CODE_BG(),
                egui::Stroke::new(1.0_f32, border),
                egui::StrokeKind::Inside,
            );
        }
        let mut outcome = crate::edit::EditOutcome::Continue;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                ui.set_clip_rect(rect);
                if let Some(active) = edits.active.as_mut() {
                    outcome = crate::edit::render_editor(ui, active, Some(rect.size()));
                }
            },
        );
        match outcome {
            // Tab-advance is a grid affordance; in the Details panel it just commits.
            crate::edit::EditOutcome::Commit { .. } => {
                let _ = edits.commit_active(value);
            }
            crate::edit::EditOutcome::Cancel => edits.cancel_active(),
            _ => {}
        }
        return;
    }

    let staged = edits.staged(row_idx, c).cloned();
    let is_staged = staged.is_some();
    let shown = staged.as_ref().unwrap_or(value);
    let can_edit = editable && !matches!(value, dbcore::Value::Bytes(_));
    // A BLOB column may currently be NULL; it must still be possible to add the
    // first file. Determine this from the declared column type, not the runtime
    // value, because NULL carries no type information by itself.
    let can_replace_blob = editable && dbcore::import::is_binary_type(&col.type_name);

    // --- the box: one allocation, a separate hit zone for the ⌄ at the right edge ---
    let h = DETAILS_VALUE_H;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    let chev_w = 20.0;
    let chev_rect =
        egui::Rect::from_min_max(egui::pos2(rect.right() - chev_w, rect.top()), rect.max);
    let chev_resp = ui.interact(chev_rect, resp.id.with("actions"), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() || chev_resp.hovered();
        let stroke_color = if is_staged {
            palette::SUCCESS()
        } else if hovered {
            palette::BORDER_STRONG()
        } else {
            palette::BORDER()
        };
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(5),
            palette::CODE_BG(),
            egui::Stroke::new(1.0_f32, stroke_color),
            egui::StrokeKind::Inside,
        );

        // Value text, single line, clipped before the chevron zone.
        let text_color = if is_staged {
            palette::SUCCESS()
        } else if shown.is_null() || matches!(shown, dbcore::Value::Bytes(_)) {
            palette::TEXT_FAINT()
        } else {
            palette::TEXT()
        };
        let font = if kind.monospace_value() && !shown.is_null() {
            egui::TextStyle::Monospace.resolve(ui.style())
        } else {
            egui::TextStyle::Body.resolve(ui.style())
        };
        let display = details_value_preview(shown, kind);
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &display,
            0.0,
            egui::TextFormat {
                font_id: font,
                color: text_color,
                italics: shown.is_null(),
                ..Default::default()
            },
        );
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let text_clip =
            egui::Rect::from_min_max(rect.min, egui::pos2(chev_rect.left() - 2.0, rect.bottom()));
        ui.painter().with_clip_rect(text_clip).galley(
            egui::pos2(
                rect.left() + crate::edit::DETAILS_VALUE_PAD_X,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            text_color,
        );

        // ⌄ glyph (slightly emphasised on hover).
        let chev_color = if chev_resp.hovered() {
            palette::TEXT()
        } else {
            palette::TEXT_WEAK()
        };
        let cc = chev_rect.center();
        let r = 3.0;
        let s = egui::Stroke::new(1.3_f32, chev_color);
        ui.painter().line_segment(
            [cc + egui::vec2(-r, -r * 0.5), cc + egui::vec2(0.0, r * 0.5)],
            s,
        );
        ui.painter().line_segment(
            [cc + egui::vec2(0.0, r * 0.5), cc + egui::vec2(r, -r * 0.5)],
            s,
        );
    }

    // Click-to-edit, like a real input. Booleans toggle instead of opening an editor.
    if can_edit {
        let resp = resp.on_hover_cursor(egui::CursorIcon::Text);
        if resp.clicked() {
            if kind == K::Bool {
                edits.toggle_bool(row_idx, c, value);
            } else {
                // Prefill from the staged value (if any) so editing continues from it.
                edits.begin(row_idx, c, shown, crate::edit::EditOrigin::Details);
            }
        }
    } else if resp.double_clicked()
        && crate::value_viewer::ValueViewer::kind(&col.type_name, shown).is_some()
    {
        if let Some(viewer) =
            crate::value_viewer::ValueViewer::new(&col.name, &col.type_name, shown)
        {
            actions.push(Action::OpenValueViewer(viewer));
        }
    }

    // The ⌄ actions menu: Copy always; mutating actions only when editable.
    egui::Popup::menu(&chev_resp).show(|ui| {
        ui.set_min_width(150.0);
        if let Some(viewer_kind) = crate::value_viewer::ValueViewer::kind(&col.type_name, shown) {
            if ui.button(viewer_kind.action_label()).clicked() {
                if let Some(viewer) =
                    crate::value_viewer::ValueViewer::new(&col.name, &col.type_name, shown)
                {
                    actions.push(Action::OpenValueViewer(viewer));
                }
                ui.close();
            }
            ui.separator();
        }
        if can_replace_blob {
            if ui.button("Add file…").clicked() {
                actions.push(Action::ReplaceBlobFromFile {
                    row: row_idx,
                    col: c,
                });
                ui.close();
            }
            ui.separator();
        }
        if ui.button("Copy value").clicked() {
            ui.ctx().copy_text(shown.as_text());
        }
        if can_edit {
            if kind != K::Bool && ui.button("Edit").clicked() {
                edits.begin(row_idx, c, shown, crate::edit::EditOrigin::Details);
            }
            ui.separator();
            match kind {
                K::Bool => {
                    if ui.button("Set TRUE").clicked() {
                        edits.stage(row_idx, c, dbcore::Value::Bool(true), value);
                    }
                    if ui.button("Set FALSE").clicked() {
                        edits.stage(row_idx, c, dbcore::Value::Bool(false), value);
                    }
                }
                K::Date => {
                    if ui.button("Pick date…").clicked() {
                        *date_pick = Some((row_idx, c));
                    }
                    if ui.button("Today").clicked() {
                        let today = jiff::Zoned::now().date().to_string();
                        edits.stage(row_idx, c, dbcore::Value::Text(today), value);
                    }
                }
                // The click test stays inside the arm (not a match guard) to match the sibling
                // Bool/Date arms, and because `ui.button` draws as a side effect.
                #[allow(clippy::collapsible_match)]
                K::DateTime => {
                    if ui.button("Now").clicked() {
                        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                        edits.stage(row_idx, c, dbcore::Value::Text(now), value);
                    }
                }
                _ => {}
            }
            if !shown.is_null() && ui.button("Set NULL").clicked() {
                edits.stage(row_idx, c, dbcore::Value::Null, value);
            }
            if is_staged && ui.button("Revert").clicked() {
                // Staging the original value clears the staged edit.
                edits.stage(row_idx, c, value.clone(), value);
            }
        }
    });

    // Inline calendar opened from the menu: a plain widget below the box, so its own
    // popup behaves normally (a calendar nested inside the menu would close with it).
    if *date_pick == Some((row_idx, c)) {
        let mut date = shown
            .display()
            .trim()
            .parse::<jiff::civil::Date>()
            .unwrap_or_else(|_| jiff::Zoned::now().date());
        ui.add_space(2.0);
        let salt = format!("details_date_{c}");
        let picker = ui.add(egui_extras::DatePickerButton::new(&mut date).id_salt(&salt));
        if picker.changed() {
            edits.stage(row_idx, c, dbcore::Value::Text(date.to_string()), value);
            *date_pick = None;
        }
    }
}

impl DbGuiApp {
    /// Right-hand Details panel: the selected row's columns and values.
    pub(in crate::app) fn right_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        // The details panel only makes sense for a selected row; with nothing selected we
        // hide it entirely so the grid gets the full width (rather than showing an empty
        // placeholder panel).
        let idx = self.active_query_tab;
        let tab_id = self.tabs[idx].id;
        let tab = &mut self.tabs[idx];
        // The selected row belongs to the data grid, which every other result surface hides.
        if tab.view != TabView::Data {
            return;
        }
        let row_idx = match (tab.result.as_ref(), tab.selection.lead()) {
            (Some(_), Some(disp)) if disp < tab.row_order.len() => tab.row_order[disp],
            _ => return,
        };
        let editable = tab.edits.editable();
        // Split the borrow so the closure can hold the result immutably and edits mutably.
        let QueryTab { result, edits, .. } = tab;
        let res = result.as_ref().expect("row_idx implies a result");
        // Disjoint field borrows alongside `tab` above.
        let details_filter = &mut self.details_filter;
        let details_date_pick = &mut self.details_date_pick;
        let details_image_preview = &mut self.details_image_preview;

        egui::Panel::right("details_panel")
            .resizable(true)
            .default_size(260.0)
            .frame(style::workspace_frame(palette::PANEL()))
            .show_separator_line(false)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                components::section_header(ui, "Details");
                // Live field filter, TablePlus-style: typing narrows the stacked fields
                // below by column name. Icon sits inside the field via `icon_text_input`.
                components::icon_text_input(
                    ui,
                    details_filter,
                    "Search for field…",
                    icons::search(),
                    ui.available_width(),
                );
                ui.add_space(4.0);

                // Stacked fields (name + type above an input-styled value box). The box is
                // full-width, so it tracks the panel as it is resized.
                // `auto_shrink([false, _])` keeps the inner ui at the panel width.
                let query = details_filter.trim().to_lowercase();
                let columns: Vec<usize> = res
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, col)| query.is_empty() || col.name.to_lowercase().contains(&query))
                    .map(|(c, _)| c)
                    .collect();
                let scroll = egui::ScrollArea::vertical()
                    .id_salt("details_scroll")
                    .auto_shrink([false, true]);
                let has_image = columns.iter().any(|&c| {
                    let shown = edits.staged(row_idx, c).unwrap_or(&res.rows[row_idx][c]);
                    crate::value_viewer::ValueViewer::kind(&res.columns[c].type_name, shown)
                        == Some(crate::value_viewer::ViewerKind::Image)
                });

                if details_date_pick.is_some() || has_image {
                    // Inline calendars and image thumbnails have variable height, so keep
                    // normal layout while either is visible. The common path stays virtualized.
                    scroll.show(ui, |ui| {
                        for &c in &columns {
                            details_field(
                                ui,
                                edits,
                                row_idx,
                                c,
                                &res.columns[c],
                                &res.rows[row_idx][c],
                                editable,
                                details_date_pick,
                                details_image_preview,
                                tab_id,
                                actions,
                            );
                        }
                    });
                } else {
                    scroll.show_rows(ui, DETAILS_FIELD_H, columns.len(), |ui, range| {
                        for item in range {
                            let c = columns[item];
                            details_field(
                                ui,
                                edits,
                                row_idx,
                                c,
                                &res.columns[c],
                                &res.rows[row_idx][c],
                                editable,
                                details_date_pick,
                                details_image_preview,
                                tab_id,
                                actions,
                            );
                        }
                    });
                }
            });
        style::workspace_resize_grip(root, egui::Id::new("details_panel"), false);
    }
}

#[cfg(test)]
mod details_preview_tests {
    use super::{details_value_preview, DETAILS_PREVIEW_CHARS};

    #[test]
    fn long_multiline_values_get_a_bounded_single_line_preview() {
        let value = dbcore::Value::Text(format!("first\n{}", "x".repeat(10_000)));
        let preview = details_value_preview(&value, crate::edit::EditorKind::Text);

        assert!(!preview.contains('\n'));
        assert_eq!(preview.chars().count(), DETAILS_PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }
}
