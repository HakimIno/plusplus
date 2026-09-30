//! Saved queries rendering and interaction.

use super::live_log::live_log_clock;
use super::sidebar::object_group;
use super::tree::drop_after;
use super::tree::paint_drop_line;
use super::tree::paint_tree_guide;
use super::tree::paint_tree_row_fill;
use super::tree::rename_callout;
use super::tree::sql_preview_callout;
use super::tree::tree_file_row;
use super::tree::CalloutEdit;
use super::tree::TREE_CHILD_INDENT;
use super::tree::TREE_ICON;
use super::tree::TREE_ROW_H;
use crate::app::{Action, DbGuiApp, SavedQueryDrag};
use crate::components;
use crate::icons;
use crate::style::palette;

#[derive(Debug, PartialEq)]
pub(in crate::app) struct HistoryDay {
    pub(in crate::app) key: String,
    pub(in crate::app) label: String,
    pub(in crate::app) entries: Vec<usize>,
}

fn history_moment(timestamp: &str) -> (String, String, String) {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        let day = timestamp.get(..10).unwrap_or(timestamp).to_string();
        return (day.clone(), day, live_log_clock(timestamp).to_string());
    };
    let local = parsed.with_timezone(&chrono::Local);
    let key = local.format("%Y-%m-%d").to_string();
    let label = local.format("%e %B %Y").to_string().trim().to_string();
    let clock = local
        .format("%I:%M:%S %p")
        .to_string()
        .trim_start_matches('0')
        .to_string();
    (key, label, clock)
}

pub(in crate::app) fn grouped_history(
    entries: &[dbcore::history::HistoryEntry],
    filter: &str,
    conn_id: Option<&str>,
) -> Vec<HistoryDay> {
    let Some(conn_id) = conn_id else {
        return Vec::new();
    };
    let filter = filter.trim().to_lowercase();
    let mut days: Vec<HistoryDay> = Vec::new();
    for idx in (0..entries.len()).rev() {
        let entry = &entries[idx];
        if entry.conn_id != conn_id {
            continue;
        }
        let (key, label, _) = history_moment(&entry.at);
        if !filter.is_empty()
            && !entry.sql.to_lowercase().contains(&filter)
            && !entry.conn_name.to_lowercase().contains(&filter)
            && !entry
                .error
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&filter)
            && !label.to_lowercase().contains(&filter)
        {
            continue;
        }
        if let Some(day) = days.last_mut().filter(|day| day.key == key) {
            day.entries.push(idx);
        } else {
            days.push(HistoryDay {
                key,
                label,
                entries: vec![idx],
            });
        }
    }
    days
}

/// Group saved queries for one connection while preserving indices into the full cache.
/// Favorites from older versions have no connection id, so they remain globally visible.
pub(in crate::app) fn grouped_favorites_for_connection(
    queries: &[dbcore::Favorite],
    folders: &[String],
    filter: &str,
    keep_empty: bool,
    conn_id: Option<&str>,
) -> Vec<(String, Vec<usize>)> {
    let global_indices: Vec<usize> = queries
        .iter()
        .enumerate()
        .filter(|(_, query)| match conn_id {
            Some(conn_id) => query
                .conn_id
                .as_deref()
                .is_none_or(|saved_conn_id| saved_conn_id == conn_id),
            None => query.conn_id.is_none(),
        })
        .map(|(idx, _)| idx)
        .collect();
    let scoped_queries: Vec<dbcore::Favorite> = global_indices
        .iter()
        .map(|&idx| queries[idx].clone())
        .collect();

    dbcore::favorites::grouped(&scoped_queries, folders, filter, keep_empty)
        .into_iter()
        // Folder names are stored globally for backward compatibility. An empty group here
        // means that the folder belongs only to another connection, so it must not leak into
        // the current connection's Queries tree.
        .filter(|(_, local_indices)| !local_indices.is_empty())
        .map(|(folder, local_indices)| {
            let indices = local_indices
                .into_iter()
                .map(|local_idx| global_indices[local_idx])
                .collect();
            (folder, indices)
        })
        .collect()
}

/// First non-empty line of a SQL string, for one-line list displays.
pub(super) fn first_line(sql: &str) -> &str {
    sql.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// Right-click actions for one saved-query folder.
fn favorite_folder_menu(ui: &mut egui::Ui, folder: &str, actions: &mut Vec<Action>) {
    ui.set_min_width(180.0);
    if components::button(ui, icons::edit(), "Rename Folder", true).clicked() {
        actions.push(Action::RenameFavoriteFolder(folder.to_string()));
        ui.close();
    }
    if components::button(ui, icons::trash(), "Delete Folder", true).clicked() {
        actions.push(Action::DeleteFavoriteFolder(folder.to_string()));
        ui.close();
    }
}

/// Right-click actions for one Saved Queries row.
fn favorite_entry_menu(
    ui: &mut egui::Ui,
    idx: usize,
    sql: &str,
    fav_id: &str,
    folders: &[String],
    actions: &mut Vec<Action>,
) {
    ui.set_min_width(180.0);
    if components::button(ui, icons::play(), "Run", true).clicked() {
        actions.push(Action::RunFavorite(idx));
        ui.close();
    }
    if components::button(ui, icons::edit(), "Open in Editor", true).clicked() {
        actions.push(Action::UseFavorite(idx));
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::copy(), "Copy", true).clicked() {
        ui.ctx().copy_text(sql.to_string());
        ui.close();
    }
    if components::button(ui, icons::edit(), "Rename", true).clicked() {
        actions.push(Action::RenameFavorite(idx));
        ui.close();
    }
    components::menu_button(ui, icons::folder(), "Move to", |ui| {
        if ui.button(dbcore::favorites::UNGROUPED).clicked() {
            actions.push(Action::MoveFavorite { idx, folder: None });
            ui.close();
        }
        for folder in folders {
            if ui.button(folder).clicked() {
                actions.push(Action::MoveFavorite {
                    idx,
                    folder: Some(folder.clone()),
                });
                ui.close();
            }
        }
        ui.separator();
        if ui.button("New Folder…").clicked() {
            actions.push(Action::NewFavoriteFolder {
                move_id: Some(fav_id.to_string()),
            });
            ui.close();
        }
    });
    ui.separator();
    if components::button(ui, icons::trash(), "Delete", true).clicked() {
        actions.push(Action::DeleteFavorite(idx));
        ui.close();
    }
}

/// Right-click actions for one History row, matching the TablePlus history menu.
fn history_entry_menu(ui: &mut egui::Ui, idx: usize, sql: &str, actions: &mut Vec<Action>) {
    ui.set_min_width(200.0);
    if components::button(ui, icons::play(), "Run", true).clicked() {
        actions.push(Action::RunHistorySql(idx));
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::copy(), "Copy", true).clicked() {
        ui.ctx().copy_text(sql.to_string());
        ui.close();
    }
    if components::button(ui, icons::save(), "Save As…", true).clicked() {
        actions.push(Action::SaveHistorySqlAs(idx));
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::edit(), "Insert into SQL Editor", true).clicked() {
        actions.push(Action::UseHistorySql(idx));
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::arrow_up_right(), reveal_history_label(), true).clicked() {
        actions.push(Action::RevealHistoryFile);
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::star(), "Add to Queries", true).clicked() {
        actions.push(Action::SaveFavoriteFromHistory(idx));
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::trash(), "Delete", true).clicked() {
        actions.push(Action::DeleteHistory(idx));
        ui.close();
    }
    if ui.button("Clear connection history").clicked() {
        actions.push(Action::ClearHistory);
        ui.close();
    }
}

fn reveal_history_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else if cfg!(target_os = "windows") {
        "Show in Explorer"
    } else {
        "Show in folder"
    }
}

impl DbGuiApp {
    /// The query-history list (newest first): date groups, then time + highlighted SQL.
    /// Rendered inside the sidebar's History tab. (The append-only compliance record is
    /// separate — see `dbcore::audit`.)
    pub(super) fn history_list(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let conn_id = self.tab().conn_id.clone();
        if conn_id.is_none() {
            ui.label(
                egui::RichText::new("Select a connection to view its history.")
                    .color(palette::TEXT_WEAK()),
            );
            return;
        }

        let mut view_cache = std::mem::take(&mut self.history_view_cache);
        if view_cache.revision != self.history_revision
            || view_cache.filter != self.history_filter
            || view_cache.connection != conn_id
        {
            view_cache.days = grouped_history(
                &self.history_cache,
                &self.history_filter,
                conn_id.as_deref(),
            );
            view_cache.revision = self.history_revision;
            view_cache.filter.clone_from(&self.history_filter);
            view_cache.connection.clone_from(&conn_id);
        }
        if view_cache.days.is_empty() {
            self.history_view_cache = view_cache;
            let message = if self.history_filter.trim().is_empty() {
                "No history for this connection."
            } else {
                "No history matches this search."
            };
            ui.label(egui::RichText::new(message).color(palette::TEXT_WEAK()));
            return;
        }

        let mut font = egui::TextStyle::Monospace.resolve(ui.style());
        font.size = (font.size - 1.5).max(10.0);
        egui::ScrollArea::vertical()
            .id_salt("history_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for (day_index, day) in view_cache.days.iter().enumerate() {
                    let id_key = format!("history_day:{}", day.key);
                    let label = day.label.clone();
                    object_group(ui, &id_key, &label, day_index == 0, |ui| {
                        ui.spacing_mut().item_spacing.y = 12.0;
                        for &idx in &day.entries {
                            let entry = &self.history_cache[idx];
                            let (_, _, clock) = history_moment(&entry.at);
                            let sql = entry.sql.clone();
                            let error = entry.error.clone();
                            let rows = entry.rows;
                            let elapsed_ms = entry.elapsed_ms;
                            let width = (ui.available_width() - TREE_CHILD_INDENT).max(40.0);

                            let row_resp = ui
                                .horizontal_top(|ui| {
                                    ui.add_space(TREE_CHILD_INDENT);
                                    ui.vertical(|ui| {
                                        ui.set_min_width(width);
                                        ui.set_max_width(width);
                                        ui.spacing_mut().item_spacing.y = 3.0;
                                        ui.label(
                                            egui::RichText::new(&clock)
                                                .small()
                                                .monospace()
                                                .color(palette::TEXT_FAINT()),
                                        );
                                        let mut job = crate::highlight::highlight_sql_cached(
                                            ui.ctx(),
                                            &sql,
                                            font.clone(),
                                        );
                                        job.wrap.max_width = ui.available_width().max(40.0);
                                        job.wrap.max_rows = 12;
                                        ui.add(
                                            egui::Label::new(job)
                                                .selectable(false)
                                                .sense(egui::Sense::hover()),
                                        );
                                        if let Some(error) = &error {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(error)
                                                        .small()
                                                        .color(palette::DANGER()),
                                                )
                                                .truncate()
                                                .selectable(false)
                                                .sense(egui::Sense::hover()),
                                            );
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            let meta = format!(
                                "{} rows · {:.0} ms",
                                rows.map_or_else(|| "—".to_string(), |n| n.to_string()),
                                elapsed_ms
                            );
                            sql_preview_callout(ui, &row_resp, &clock, &sql, Some(&meta));
                            row_resp.context_menu(|ui| {
                                history_entry_menu(ui, idx, &sql, actions);
                            });
                            if row_resp.clicked() {
                                actions.push(Action::UseHistorySql(idx));
                            }
                        }
                    });
                }
            });
        self.history_view_cache = view_cache;
    }

    /// The Queries tab: search above a folder tree of named queries. Rows show only the
    /// query name (SQL stays in the editor / a hover tooltip). Click opens a Query tab;
    /// folders and move/rename/delete sit on the right-click menu.
    pub(super) fn favorites_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let plus_w = 28.0;
            let search_w = (ui.available_width() - plus_w).max(80.0);
            components::icon_text_input(
                ui,
                &mut self.favorites_filter,
                "Search for query…",
                icons::search(),
                search_w,
            );
            if components::icon_button(ui, icons::plus(), "New folder").clicked() {
                actions.push(Action::NewFavoriteFolder { move_id: None });
            }
        });
        ui.add_space(6.0);

        let keep_empty = self.favorites_filter.trim().is_empty();
        let groups = grouped_favorites_for_connection(
            &self.favorites_cache,
            &self.favorite_folders,
            &self.favorites_filter,
            keep_empty,
            self.tab().conn_id.as_deref(),
        );
        if groups.is_empty() {
            let message = if self.favorites_filter.trim().is_empty() {
                "No saved queries for this connection."
            } else {
                "No queries match the search."
            };
            ui.label(egui::RichText::new(message).color(palette::TEXT_FAINT()));
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("saved_queries_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (folder, idxs) in groups {
                    self.favorite_folder_block(ui, &folder, &idxs, actions);
                }
            });
    }

    fn favorite_folder_block(
        &mut self,
        ui: &mut egui::Ui,
        folder: &str,
        idxs: &[usize],
        actions: &mut Vec<Action>,
    ) {
        use egui::collapsing_header::CollapsingState;
        let ungrouped = folder.eq_ignore_ascii_case(dbcore::favorites::UNGROUPED);
        let id = ui.make_persistent_id(("fav_folder", folder));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, true);

        let renaming_this = self
            .folder_pending
            .as_ref()
            .and_then(|draft| draft.from.as_deref())
            .is_some_and(|from| from.eq_ignore_ascii_case(folder));
        let sense = if ungrouped || renaming_this {
            egui::Sense::click()
        } else {
            egui::Sense::click_and_drag()
        };
        let (row_rect, row_resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), TREE_ROW_H), sense);
        let dots_rect = egui::Rect::from_center_size(
            egui::pos2(row_rect.right() - 16.0, row_rect.center().y),
            egui::vec2(20.0, 20.0),
        );
        let dots_resp = ui.interact(
            dots_rect,
            ui.make_persistent_id(("fav_folder_menu", folder)),
            egui::Sense::click(),
        );
        let hovered = row_resp.hovered()
            || dots_resp.hovered()
            || row_resp.context_menu_opened()
            || dots_resp.context_menu_opened();
        paint_tree_row_fill(ui, row_rect, false, hovered);

        if !ungrouped && !renaming_this {
            row_resp.dnd_set_drag_payload(SavedQueryDrag::Folder(folder.to_string()));
        }
        if let Some(source) = row_resp.dnd_hover_payload::<SavedQueryDrag>() {
            match source.as_ref() {
                SavedQueryDrag::Folder(source_name)
                    if !ungrouped && !source_name.eq_ignore_ascii_case(folder) =>
                {
                    let after = drop_after(ui, row_rect);
                    paint_drop_line(ui, row_rect, after);
                    if let Some(released) = row_resp.dnd_release_payload::<SavedQueryDrag>() {
                        if let SavedQueryDrag::Folder(source_name) = released.as_ref() {
                            actions.push(Action::ReorderFavoriteFolder {
                                source: source_name.clone(),
                                target: folder.to_string(),
                                after,
                            });
                        }
                    }
                }
                SavedQueryDrag::Query(_) => {
                    paint_tree_row_fill(ui, row_rect, true, true);
                    if let Some(released) = row_resp.dnd_release_payload::<SavedQueryDrag>() {
                        if let SavedQueryDrag::Query(id) = released.as_ref() {
                            actions.push(Action::DropFavoriteOnFolder {
                                id: id.clone(),
                                folder: if ungrouped {
                                    None
                                } else {
                                    Some(folder.to_string())
                                },
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        let mut toggle_open = row_resp.clicked()
            && !row_resp.double_clicked()
            && !dots_resp.clicked()
            && !renaming_this
            && !row_resp.dragged();
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(row_rect.shrink2(egui::vec2(6.0, 0.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let (chev_rect, chev_resp) =
                        ui.allocate_exact_size(egui::vec2(12.0, TREE_ROW_H), egui::Sense::click());
                    let chevron = if state.openness(ui.ctx()) < 0.5 {
                        icons::chevron_right()
                    } else {
                        icons::chevron_down()
                    };
                    egui::Image::new(chevron)
                        .fit_to_exact_size(egui::Vec2::splat(12.0))
                        .tint(palette::TEXT_FAINT())
                        .paint_at(
                            ui,
                            egui::Rect::from_center_size(
                                chev_rect.center(),
                                egui::Vec2::splat(12.0),
                            ),
                        );
                    if chev_resp.clicked() && !renaming_this {
                        toggle_open = true;
                    }
                    let (icon_rect, _) =
                        ui.allocate_exact_size(egui::vec2(16.0, TREE_ROW_H), egui::Sense::hover());
                    egui::Image::new(icons::folder())
                        .tint(palette::ACCENT())
                        .paint_at(
                            ui,
                            egui::Rect::from_center_size(
                                icon_rect.center(),
                                egui::Vec2::splat(TREE_ICON),
                            ),
                        );

                    let rest = ui.available_rect_before_wrap();
                    ui.allocate_rect(rest, egui::Sense::hover());
                    let label_right = if ungrouped {
                        rest.right()
                    } else {
                        (dots_rect.left() - 4.0).max(rest.left())
                    };
                    let label_rect =
                        egui::Rect::from_min_max(rest.min, egui::pos2(label_right, rest.bottom()));
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(label_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(folder).color(palette::TEXT()),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        },
                    );
                    if !ungrouped && hovered && ui.is_rect_visible(dots_rect) {
                        let color = if dots_resp.hovered() {
                            palette::TEXT()
                        } else {
                            palette::TEXT_FAINT()
                        };
                        egui::Image::new(icons::more_vert()).tint(color).paint_at(
                            ui,
                            egui::Rect::from_center_size(
                                dots_rect.center(),
                                egui::vec2(16.0, 16.0),
                            ),
                        );
                    }
                });
            },
        );

        let row_resp = if ungrouped {
            row_resp.on_hover_text("Drop queries here to ungroup. Create a folder with +.")
        } else {
            row_resp.on_hover_text(
                "Drag to reorder · drop queries here · double-click to rename · ⋮ to delete",
            )
        };
        if !ungrouped {
            if row_resp.double_clicked() && !renaming_this {
                actions.push(Action::RenameFavoriteFolder(folder.to_string()));
            }
            row_resp.context_menu(|ui| {
                favorite_folder_menu(ui, folder, actions);
            });
            egui::Popup::menu(&dots_resp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    favorite_folder_menu(ui, folder, actions);
                });
        }
        if renaming_this {
            if let Some(draft) = self.folder_pending.as_mut() {
                match rename_callout(ui, &row_resp, &mut draft.name, "Folder name") {
                    CalloutEdit::Confirm => actions.push(Action::ConfirmFavoriteFolder),
                    CalloutEdit::Cancel => actions.push(Action::CancelFavoriteFolder),
                    CalloutEdit::Idle => {}
                }
            }
        }

        if toggle_open {
            state.toggle(ui);
        }
        if idxs.is_empty() {
            return;
        }
        // Same nesting language as Items: children stay full-width so the hover/selection
        // pill reads as a row, while the icon+name sit past the folder chevron.
        let body = state.show_body_unindented(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add_space(1.0);
            for &idx in idxs {
                self.favorite_query_row(ui, idx, TREE_CHILD_INDENT, actions);
            }
            ui.add_space(1.0);
        });
        if let Some(inner) = body {
            paint_tree_guide(ui, row_rect, inner.response.rect);
        }
    }

    fn favorite_query_row(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        tree_indent: f32,
        actions: &mut Vec<Action>,
    ) {
        let Some(fav) = self.favorites_cache.get(idx) else {
            return;
        };
        let name = fav.name.clone();
        let sql = fav.sql.clone();
        let fav_id = fav.id.clone();
        let selected = self.favorites_selected.as_deref() == Some(fav_id.as_str());

        let is_renaming_this = self
            .favorite_pending
            .as_ref()
            .and_then(|d| d.editing_id.as_ref())
            .is_some_and(|id| id == &fav_id);

        let icon_color = if selected {
            palette::TEXT()
        } else {
            palette::TEXT_WEAK()
        };
        let sense = if is_renaming_this {
            egui::Sense::click()
        } else {
            egui::Sense::click_and_drag()
        };
        let (row_rect, row_resp) = tree_file_row(
            ui,
            tree_indent,
            icons::file(),
            icon_color,
            &name,
            selected,
            sense,
        );
        if !is_renaming_this {
            row_resp.dnd_set_drag_payload(SavedQueryDrag::Query(fav_id.clone()));
            if let Some(source) = row_resp.dnd_hover_payload::<SavedQueryDrag>() {
                if let SavedQueryDrag::Query(source_id) = source.as_ref() {
                    if source_id != &fav_id {
                        let after = drop_after(ui, row_rect);
                        paint_drop_line(ui, row_rect, after);
                        if let Some(released) = row_resp.dnd_release_payload::<SavedQueryDrag>() {
                            if let SavedQueryDrag::Query(source_id) = released.as_ref() {
                                actions.push(Action::DropFavoriteOnQuery {
                                    source_id: source_id.clone(),
                                    target_id: fav_id.clone(),
                                    after,
                                });
                            }
                        }
                    }
                }
            }
        }

        let folders = self.favorite_folders.clone();
        let row_resp = row_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if !is_renaming_this && !row_resp.dragged() {
            sql_preview_callout(ui, &row_resp, &name, &sql, None);
        }
        row_resp.context_menu(|ui| {
            favorite_entry_menu(ui, idx, &sql, &fav_id, &folders, actions);
        });
        if is_renaming_this {
            if let Some(draft) = self.favorite_pending.as_mut() {
                match rename_callout(ui, &row_resp, &mut draft.name, "Query name") {
                    CalloutEdit::Confirm => actions.push(Action::ConfirmSaveFavorite),
                    CalloutEdit::Cancel => actions.push(Action::CancelSaveFavorite),
                    CalloutEdit::Idle => {}
                }
            }
        } else if row_resp.clicked() && !row_resp.dragged() {
            actions.push(Action::UseFavorite(idx));
        }
    }
}
