//! Sidebar rendering and interaction.

use super::tree::drop_after;
use super::tree::paint_drop_line;
use super::tree::paint_tree_guide;
use super::tree::paint_tree_row_fill;
use super::tree::tree_file_row;
use super::tree::TREE_CHILD_INDENT;
use super::tree::TREE_ICON;
use super::tree::TREE_ROW_H;
use crate::app::{Action, ActiveConnection, Busy, DbGuiApp, SchemaTableDrag, SidebarTab};
use crate::components;
use crate::icons;
use crate::style;
use crate::style::palette;

/// The expandable body of a table row in the explorer: its columns (PK marked), then any
/// indexes and foreign keys. Shared by the "Pinned" group and the main table list.
fn schema_table_body(ui: &mut egui::Ui, table: &dbcore::TableInfo) {
    for col in &table.columns {
        let glyph = if col.primary_key {
            icons::key()
        } else {
            icons::column()
        };
        let nn = if col.nullable { "" } else { " · not null" };
        let meta = format!("{}{nn}", col.data_type);
        let (_, resp) = tree_file_row(
            ui,
            TREE_CHILD_INDENT,
            glyph,
            palette::TEXT_WEAK(),
            &col.name,
            false,
            egui::Sense::hover(),
        );
        let _ = resp.on_hover_text(meta);
    }
    if !table.indexes.is_empty() {
        for idx in &table.indexes {
            let u = if idx.unique { "unique " } else { "" };
            let detail = format!("{u}{} ({})", idx.name, idx.columns.join(", "));
            let (_, resp) = tree_file_row(
                ui,
                TREE_CHILD_INDENT,
                icons::index(),
                palette::TEXT_WEAK(),
                &idx.name,
                false,
                egui::Sense::hover(),
            );
            let _ = resp.on_hover_text(detail);
        }
    }
    if !table.foreign_keys.is_empty() {
        for fk in &table.foreign_keys {
            let detail = fk.display();
            let hover = if fk.name.is_empty() {
                format!("{detail} · on delete {}", fk.on_delete)
            } else {
                format!("{} · {detail} · on delete {}", fk.name, fk.on_delete)
            };
            let name = if fk.name.is_empty() {
                detail.clone()
            } else {
                fk.name.clone()
            };
            let (_, resp) = tree_file_row(
                ui,
                TREE_CHILD_INDENT,
                icons::connect(),
                palette::TEXT_WEAK(),
                &name,
                false,
                egui::Sense::hover(),
            );
            let _ = resp.on_hover_text(hover);
        }
    }
}

/// The table actions menu (pin, edit, clone, export, truncate, drop) shared by the row's
/// right-click context menu. `pinned` selects the pin/unpin wording.
fn table_actions_menu(
    ui: &mut egui::Ui,
    table: &dbcore::TableInfo,
    pinned: bool,
    kind: dbcore::DbKind,
    conn_id: &str,
    actions: &mut Vec<Action>,
) {
    ui.set_min_width(180.0);
    let pin_label = if pinned {
        "Unpin from Top"
    } else {
        "Pin to Top"
    };
    if components::button(ui, icons::star(), pin_label, true).clicked() {
        actions.push(Action::ToggleBookmark {
            schema: table.schema.clone(),
            table: table.name.clone(),
        });
        ui.close();
    }
    if components::button(ui, icons::diagram(), "Show Diagram", true)
        .on_hover_text("Diagram of this table and the tables it links to")
        .clicked()
    {
        actions.push(Action::ShowTableDiagram {
            schema: table.schema.clone(),
            table: table.name.clone(),
        });
        ui.close();
    }
    if components::button(ui, icons::copy(), "Copy name", true)
        .on_hover_text("Copy the table name to the clipboard")
        .clicked()
    {
        ui.ctx().copy_text(table.name.clone());
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::edit(), "Edit Structure…", true).clicked() {
        actions.push(Action::OpenEditTable(table.clone()));
        ui.close();
    }
    // CQL has no `CREATE TABLE … LIKE` / `INSERT … SELECT`, so a table can't be cloned as
    // a statement sequence; hide the action rather than offer one that produces no SQL.
    if !kind.is_cql()
        && components::button(ui, icons::table(), "Clone Table…", true)
            .on_hover_text("Copy this table's structure and rows into a new table")
            .clicked()
    {
        actions.push(Action::CloneTable(table.clone()));
        ui.close();
    }
    let export_label = egui::Image::new(icons::save())
        .fit_to_exact_size(egui::vec2(icons::SIZE, icons::SIZE))
        .tint(ui.visuals().widgets.inactive.fg_stroke.color);
    ui.menu_button((export_label, "Export Table…"), |ui| {
        ui.set_min_width(160.0);
        for fmt in [dbcore::ExportFormat::Csv, dbcore::ExportFormat::Json] {
            if ui
                .button(format!("Export as {}…", fmt.label()))
                .on_hover_text("Stream every row of this table to a file")
                .clicked()
            {
                actions.push(Action::ExportTable {
                    table: table.clone(),
                    format: fmt,
                });
                ui.close();
            }
        }
        if matches!(
            kind,
            dbcore::DbKind::Postgres
                | dbcore::DbKind::MySql
                | dbcore::DbKind::MariaDb
                | dbcore::DbKind::SqlServer
        ) {
            ui.separator();
            if ui
                .button("Export as SQL Dump…")
                .on_hover_text("Back up this table's structure and rows to a local .sql file")
                .clicked()
            {
                actions.push(Action::ExportTableDump {
                    conn_id: conn_id.to_owned(),
                    table: table.clone(),
                });
                ui.close();
            }
        }
    });
    if components::button(ui, icons::table(), "Import Data…", true)
        .on_hover_text("Load rows into this table from a CSV or JSON file")
        .clicked()
    {
        actions.push(Action::ImportIntoTable(table.clone()));
        ui.close();
    }
    ui.separator();
    if components::button(ui, icons::warning(), "Truncate Table…", true)
        .on_hover_text("Remove all rows but keep the table")
        .clicked()
    {
        actions.push(Action::TruncateTable(table.clone()));
        ui.close();
    }
    if components::button(ui, icons::trash(), "Drop Table…", true)
        .on_hover_text("Delete this table and all of its data")
        .clicked()
    {
        actions.push(Action::DropTable(table.clone()));
        ui.close();
    }
}

/// A collapsible folder group matching Saved Queries: chevron, folder icon, 26px hover pill.
pub(super) fn object_group(
    ui: &mut egui::Ui,
    id_key: &str,
    title: &str,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    use egui::collapsing_header::CollapsingState;
    let id = ui.make_persistent_id(("obj_group", id_key));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);

    let (row_rect, row_resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TREE_ROW_H),
        egui::Sense::click(),
    );
    paint_tree_row_fill(ui, row_rect, false, row_resp.hovered());
    let mut toggle_open = row_resp.clicked();
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
                        egui::Rect::from_center_size(chev_rect.center(), egui::Vec2::splat(12.0)),
                    );
                if chev_resp.clicked() {
                    toggle_open = true;
                }
                let (icon_rect, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, TREE_ROW_H), egui::Sense::hover());
                egui::Image::new(icons::folder())
                    .tint(palette::TEXT_WEAK())
                    .paint_at(
                        ui,
                        egui::Rect::from_center_size(
                            icon_rect.center(),
                            egui::Vec2::splat(TREE_ICON),
                        ),
                    );
                ui.add(
                    egui::Label::new(egui::RichText::new(title).color(palette::TEXT()))
                        .truncate()
                        .selectable(false),
                );
            });
        },
    );
    if toggle_open {
        state.toggle(ui);
    }
    let shown = state.show_body_unindented(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add_space(1.0);
        body(ui);
        ui.add_space(1.0);
    });
    if let Some(inner) = shown {
        paint_tree_guide(ui, row_rect, inner.response.rect);
    }
}

const OBJECT_ROW_HEIGHT: f32 = TREE_ROW_H;

/// Reserve the full height of a large object list, but build widgets only for rows intersecting
/// the outer schema scroll area's clip rectangle. This keeps expanding a group with thousands of
/// routines cheap without introducing a nested scrollbar.
fn virtualized_object_rows<T>(
    ui: &mut egui::Ui,
    items: &[T],
    mut show_row: impl FnMut(&mut egui::Ui, usize, &T),
) {
    if items.is_empty() {
        return;
    }

    let spacing = ui.spacing().item_spacing.y;
    let stride = OBJECT_ROW_HEIGHT + spacing;
    let full_height = stride * items.len() as f32 - spacing;
    let (_, full_rect) = ui.allocate_space(egui::vec2(ui.available_width().max(0.0), full_height));
    let visible = visible_object_row_range(full_rect, ui.clip_rect(), stride, items.len());
    if visible.is_empty() {
        return;
    }

    let rows_rect = egui::Rect::from_min_max(
        egui::pos2(
            full_rect.left(),
            full_rect.top() + visible.start as f32 * stride,
        ),
        egui::pos2(
            full_rect.right(),
            full_rect.top() + visible.end as f32 * stride - spacing,
        ),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(rows_rect), |ui| {
        ui.skip_ahead_auto_ids(visible.start);
        for index in visible {
            show_row(ui, index, &items[index]);
        }
    });
}

fn visible_object_row_range(
    full_rect: egui::Rect,
    clip_rect: egui::Rect,
    stride: f32,
    total: usize,
) -> std::ops::Range<usize> {
    if total == 0 || stride <= 0.0 || !full_rect.intersects(clip_rect) {
        return 0..0;
    }
    let first = ((clip_rect.top() - full_rect.top()) / stride)
        .floor()
        .max(0.0) as usize;
    let end = (((clip_rect.bottom() - full_rect.top()) / stride).ceil() as usize + 1).min(total);
    first.min(end)..end
}

/// A not-yet-created object, shown in the explorer while its New … editor is open, so the
/// list already reads as it will after Apply. Green marks a pending addition, as in the grids;
/// the name follows what is typed in the editor.
fn draft_object_row(
    ui: &mut egui::Ui,
    indent: f32,
    icon: egui::ImageSource<'static>,
    name: &str,
    placeholder: &str,
    selected: bool,
) -> bool {
    let (row_rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TREE_ROW_H),
        egui::Sense::click(),
    );
    // Every draft is green; the one open in the active tab is a shade stronger.
    ui.painter().rect_filled(
        row_rect,
        egui::CornerRadius::same(6),
        palette::SUCCESS().linear_multiply(if selected || response.hovered() {
            0.4
        } else {
            0.28
        }),
    );
    let content = egui::Rect::from_min_max(
        egui::pos2(row_rect.left() + indent, row_rect.top()),
        row_rect.max,
    );
    let shown = name.trim();
    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (icon_rect, _) =
                ui.allocate_exact_size(egui::vec2(16.0, TREE_ROW_H), egui::Sense::hover());
            egui::Image::new(icon).tint(palette::TEXT()).paint_at(
                ui,
                egui::Rect::from_center_size(icon_rect.center(), egui::Vec2::splat(TREE_ICON)),
            );
            let (text, color) = if shown.is_empty() {
                (placeholder, palette::TEXT_WEAK())
            } else {
                (shown, palette::TEXT())
            };
            ui.add(
                egui::Label::new(egui::RichText::new(text).color(color))
                    .truncate()
                    .selectable(false)
                    .sense(egui::Sense::hover()),
            );
        });
    });
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Not created yet — click to open its tab, apply to create it");
    response.clicked()
}

/// Open a folder group the first time a draft appears in it, so the draft isn't hidden inside
/// a collapsed "Views" / "Triggers" folder. Later the user can collapse it again.
fn reveal_group_for_draft(ui: &egui::Ui, id_key: &str, draft_id: egui::Id) {
    use egui::collapsing_header::CollapsingState;
    let seen = ui.data(|d| d.get_temp::<bool>(draft_id)).unwrap_or(false);
    if seen {
        return;
    }
    ui.data_mut(|d| d.insert_temp(draft_id, true));
    let id = ui.make_persistent_id(("obj_group", id_key));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
    state.set_open(true);
    state.store(ui.ctx());
}

/// A single clickable leaf row (a routine or trigger) in the sidebar tree: an icon, the object
/// name, and `detail` shown as a hover tooltip. The full row is interactive so a click beside a
/// short name still opens the intended object.
fn object_leaf_row(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'static>,
    color: egui::Color32,
    name: &str,
    detail: &str,
) -> egui::Response {
    tree_file_row(
        ui,
        TREE_CHILD_INDENT,
        icon,
        color,
        name,
        false,
        egui::Sense::click(),
    )
    .1
    .on_hover_text(detail)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod object_row_tests {
    use super::visible_object_row_range;

    #[test]
    fn large_object_lists_only_build_visible_rows() {
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(300.0, 30_000.0));
        let top = egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(300.0, 300.0));
        let middle = egui::Rect::from_min_size(egui::pos2(0.0, 15_100.0), egui::vec2(300.0, 300.0));

        assert_eq!(visible_object_row_range(full, top, 30.0, 1_024), 0..11);
        assert_eq!(
            visible_object_row_range(full, middle, 30.0, 1_024),
            500..511
        );
    }

    #[test]
    fn offscreen_object_lists_build_no_rows() {
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 500.0), egui::vec2(300.0, 300.0));
        let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 400.0));
        assert_eq!(visible_object_row_range(full, clip, 30.0, 10), 0..0);
    }
}

impl DbGuiApp {
    pub(in crate::app) fn left_panel(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(280.0)
            .min_size(200.0)
            .max_size(360.0)
            .frame(
                style::workspace_frame(palette::PANEL()).outer_margin(egui::Margin {
                    // The right seam combines this card and the central card's gutters.
                    // When the connection rail is present it has no card margin of its own,
                    // so this panel supplies the whole seam. Without the rail, the workspace
                    // inset supplies the other half just like the other outside edges.
                    left: if self.show_connection_tabs {
                        style::WORKSPACE_GUTTER * 2
                    } else {
                        style::WORKSPACE_GUTTER
                    },
                    right: style::WORKSPACE_GUTTER,
                    top: style::WORKSPACE_GUTTER_Y,
                    bottom: style::WORKSPACE_GUTTER_Y,
                }),
            )
            .show_separator_line(false)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                // A quiet, unboxed tab rail keeps navigation visible without spending a
                // second surface layer at the top of the narrow sidebar.
                let tabs = [SidebarTab::Items, SidebarTab::Queries, SidebarTab::History];
                let selected = tabs
                    .iter()
                    .position(|t| *t == self.sidebar_tab)
                    .unwrap_or(0);
                let labels = ["Items", "Queries", "History"];
                let mut choice = selected;
                let tab_width = ui.available_width() / tabs.len() as f32;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (index, label) in labels.iter().enumerate() {
                        let active = index == selected;
                        let color = if active {
                            palette::TEXT()
                        } else {
                            palette::TEXT_WEAK()
                        };
                        let button =
                            egui::Button::new(egui::RichText::new(*label).size(12.0).color(color))
                                .frame(true)
                                .frame_when_inactive(false)
                                .corner_radius(egui::CornerRadius::same(4));
                        let response = ui.add_sized(egui::vec2(tab_width, 24.0), button);
                        if active && ui.is_rect_visible(response.rect) {
                            ui.painter().hline(
                                response.rect.center().x - 12.0..=response.rect.center().x + 12.0,
                                response.rect.bottom() - 1.0,
                                egui::Stroke::new(2.0_f32, palette::ACCENT()),
                            );
                        }
                        if response.clicked() {
                            choice = index;
                        }
                    }
                });
                if choice != selected {
                    actions.push(Action::SetSidebarTab(tabs[choice]));
                }
                ui.add_space(4.0);
                match self.sidebar_tab {
                    SidebarTab::Items => self.sidebar_items(ui, actions),
                    SidebarTab::Queries => self.favorites_tab(ui, actions),
                    SidebarTab::History => self.sidebar_history(ui, actions),
                }
            });
        style::workspace_resize_grip(root, egui::Id::new("left_panel"), false);
    }

    /// The Items tab: create-object menu, table filter, and the schema tree.
    fn sidebar_items(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Keep the create menu visually subordinate to the filter: only its resting
                // chrome is removed, so the whole toolbar fits on one 24-point row.
                ui.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
                let inactive_button = ui.visuals().widgets.inactive;
                let inactive = &mut ui.visuals_mut().widgets.inactive;
                inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                inactive.bg_stroke = egui::Stroke::NONE;
                let connected = self.active().is_some();
                let active_kind = self.active().map(|a| a.db.kind());
                // An object the driver can't create stays listed but disabled, with the reason
                // on hover, rather than vanishing from the menu.
                let supports = |check: fn(dbcore::DbKind) -> bool| active_kind.is_none_or(check);
                let supports_routines = supports(dbcore::DbKind::supports_routines);
                let supports_views = supports(dbcore::DbKind::supports_views);
                let supports_triggers = supports(dbcore::DbKind::supports_triggers);
                let driver = active_kind.map_or("this driver", dbcore::DbKind::label);
                let unavailable = format!("Not available for {driver}");
                let menu = ui.add_enabled_ui(connected, |ui| {
                    let plus = egui::Image::new(icons::plus())
                        .fit_to_exact_size(egui::vec2(icons::SIZE, icons::SIZE))
                        .tint(ui.visuals().widgets.inactive.fg_stroke.color);
                    ui.menu_button(plus, |ui| {
                        ui.set_min_width(180.0);
                        if components::button(ui, icons::table(), "New Table…", true).clicked() {
                            actions.push(Action::OpenNewTable);
                            ui.close();
                        }
                        if components::button(ui, icons::view(), "New View…", supports_views)
                            .on_disabled_hover_text(&unavailable)
                            .clicked()
                        {
                            actions.push(Action::OpenNewView);
                            ui.close();
                        }
                        if components::button(ui, icons::play(), "New Trigger…", supports_triggers)
                            .on_disabled_hover_text(&unavailable)
                            .clicked()
                        {
                            actions.push(Action::OpenNewTrigger);
                            ui.close();
                        }
                        ui.separator();
                        if components::button(
                            ui,
                            icons::function(),
                            "New Function…",
                            supports_routines,
                        )
                        .on_disabled_hover_text(&unavailable)
                        .clicked()
                        {
                            actions.push(Action::OpenNewRoutine(dbcore::RoutineKind::Function));
                            ui.close();
                        }
                        if components::button(
                            ui,
                            icons::function(),
                            "New Procedure…",
                            supports_routines,
                        )
                        .on_disabled_hover_text(&unavailable)
                        .clicked()
                        {
                            actions.push(Action::OpenNewRoutine(dbcore::RoutineKind::Procedure));
                            ui.close();
                        }
                        // The ER designer lives here rather than behind its own toolbar icon;
                        // a single table's diagram is also on its right-click menu.
                        ui.separator();
                        if components::button(ui, icons::diagram(), "Open ER Designer", true)
                            .clicked()
                        {
                            actions.push(Action::ShowDatabaseDiagram);
                            ui.close();
                        }
                        if components::button(ui, icons::database(), "Import ER Design…", true)
                            .clicked()
                        {
                            actions.push(Action::ImportErd);
                            ui.close();
                        }
                    })
                    .response
                    .on_hover_text("Create a new object");
                });
                if !connected {
                    let _ = menu
                        .response
                        .on_disabled_hover_text("Connect to a database first");
                }
                // The filter remains a visible input well; only the icon actions are ghosted.
                ui.visuals_mut().widgets.inactive = inactive_button;
                components::icon_text_input(
                    ui,
                    &mut self.schema_filter,
                    "Filter tables…",
                    icons::search(),
                    ui.available_width(),
                );
            });
        });
        ui.add_space(4.0);

        if self.active().is_some() {
            // The tree fills the tab; the schema picker (when there's a choice) sits under it.
            let schemas = self.sidebar_schemas();
            let picker_h = if schemas.is_empty() {
                0.0
            } else {
                style::CONTROL_H + 10.0
            };
            egui::ScrollArea::vertical()
                .id_salt("schema_scroll")
                .auto_shrink([false, false])
                .max_height((ui.available_height() - picker_h).max(0.0))
                .show(ui, |ui| {
                    // Keep tree content within the panel — long names must not widen it.
                    ui.set_width(ui.available_width());
                    self.schema_tree(ui, actions);
                });
            if !schemas.is_empty() {
                self.sidebar_schema_picker(ui, &schemas);
            }
        } else {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                let avail = ui.available_height();
                ui.add_space(avail * 0.4);
                if self.busy == Busy::Connecting {
                    ui.add(components::spinner(32.0));
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("Connecting...")
                            .color(palette::TEXT_WEAK())
                            .size(14.0),
                    );
                } else {
                    ui.add(
                        egui::Image::new(icons::plug_off())
                            .fit_to_exact_size(egui::Vec2::splat(40.0))
                            .tint(palette::TEXT_FAINT())
                            .alt_text("Connect to a database to browse its schema."),
                    )
                    .on_hover_text("Connect to a database to browse its schema.");
                    ui.add_space(12.0);
                    if components::primary_button(ui, icons::connect(), "Connect a database", true)
                        .clicked()
                    {
                        actions.push(Action::NewConnection);
                    }
                }
            });
        }
    }

    /// The History tab: search, then the executed-statement list.
    fn sidebar_history(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let has_connection = self.active_connection_config().is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let trash_w = 28.0;
            let search_w = (ui.available_width() - trash_w).max(80.0);
            components::icon_text_input(
                ui,
                &mut self.history_filter,
                "Search history…",
                icons::search(),
                search_w,
            );
            let clear =
                components::icon_button(ui, icons::trash(), "Delete history for this connection");
            if clear.clicked() && has_connection {
                actions.push(Action::ClearHistory);
            }
        });
        ui.add_space(6.0);
        self.history_list(ui, actions);
    }

    /// Every schema the active connection's objects live in, sorted. Empty when there's no
    /// choice to make (SQLite and MySQL report none; a single schema needs no picker).
    pub(in crate::app) fn sidebar_schemas(&self) -> Vec<String> {
        let Some(active) = self.active() else {
            return Vec::new();
        };
        let tree = &active.schema;
        let names: std::collections::BTreeSet<&str> = tree
            .tables
            .iter()
            .map(|t| t.schema.as_deref())
            .chain(tree.views.iter().map(|v| v.schema.as_deref()))
            .chain(tree.routines.iter().map(|r| r.schema.as_deref()))
            .flatten()
            .collect();
        if names.len() < 2 {
            return Vec::new();
        }
        names.into_iter().map(str::to_string).collect()
    }

    /// The schema the sidebar is scoped to, if one is chosen and still exists.
    pub(in crate::app) fn sidebar_schema_scope(&self) -> Option<&str> {
        let active = self.active()?;
        let chosen = self.sidebar_schema.get(&active.config_id)?.as_str();
        let tree = &active.schema;
        let exists = tree
            .tables
            .iter()
            .any(|t| t.schema.as_deref() == Some(chosen))
            || tree
                .views
                .iter()
                .any(|v| v.schema.as_deref() == Some(chosen))
            || tree
                .routines
                .iter()
                .any(|r| r.schema.as_deref() == Some(chosen));
        exists.then_some(chosen)
    }

    /// The schema picker under the Items tree: "All schemas" or one schema.
    fn sidebar_schema_picker(&mut self, ui: &mut egui::Ui, schemas: &[String]) {
        let Some(conn_id) = self.active().map(|a| a.config_id.clone()) else {
            return;
        };
        let current = self.sidebar_schema_scope().map(str::to_string);
        let mut choice = current.clone();
        ui.add_space(6.0);
        egui::ComboBox::from_id_salt(("sidebar_schema", conn_id.as_str()))
            .width(ui.available_width())
            .selected_text(current.as_deref().unwrap_or("All schemas"))
            .icon(components::combo_chevron_icon)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, None, "All schemas");
                for schema in schemas {
                    ui.selectable_value(&mut choice, Some(schema.clone()), schema);
                }
            })
            .response
            .on_hover_text("Show one schema's objects");
        if choice != current {
            match choice {
                Some(schema) => {
                    self.sidebar_schema.insert(conn_id, schema);
                }
                None => {
                    self.sidebar_schema.remove(&conn_id);
                }
            }
        }
    }

    fn schema_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(active) = self.active() else {
            return;
        };
        let scope = self.sidebar_schema_scope();
        let in_scope = |schema: Option<&str>| scope.is_none_or(|s| schema == Some(s));

        // The connection rail and the title bar already say which driver this is.
        components::truncated_label(
            ui,
            &active.schema.database_name,
            None,
            false,
            egui::Sense::hover(),
        )
        .on_hover_text(active.db.kind().label());
        ui.add_space(2.0);

        let conn_id = active.config_id.as_str();
        let filter = self.schema_filter.to_lowercase();
        let visible = |t: &dbcore::TableInfo| {
            in_scope(t.schema.as_deref())
                && (filter.is_empty() || t.name.to_lowercase().contains(&filter))
        };

        // One continuous list: pinned tables always sort to the top, while the saved custom
        // order controls positions within the pinned and unpinned groups.
        let custom_order = self.schema_table_order.get(conn_id);
        let custom_ranks: std::collections::HashMap<(&str, &str), usize> = custom_order
            .into_iter()
            .flat_map(|order| order.iter().enumerate())
            .filter_map(|(rank, key)| {
                key.split_once('\0')
                    .map(|(schema, table)| ((schema, table), rank))
            })
            .collect();
        let bookmark_ranks: std::collections::HashMap<(Option<&str>, &str), usize> = self
            .bookmarks
            .iter()
            .enumerate()
            .filter(|(_, bookmark)| bookmark.conn_id == conn_id)
            .map(|(rank, bookmark)| ((bookmark.schema.as_deref(), bookmark.table.as_str()), rank))
            .collect();
        let mut tables: Vec<(&dbcore::TableInfo, bool, usize, usize)> = active
            .schema
            .tables
            .iter()
            .filter(|table| visible(table))
            .map(|table| {
                let bookmark_key = (table.schema.as_deref(), table.name.as_str());
                let custom_key = (
                    table.schema.as_deref().unwrap_or_default(),
                    table.name.as_str(),
                );
                let bookmark_rank = bookmark_ranks
                    .get(&bookmark_key)
                    .copied()
                    .unwrap_or(usize::MAX);
                let custom_rank = custom_ranks.get(&custom_key).copied().unwrap_or(usize::MAX);
                (
                    table,
                    bookmark_rank != usize::MAX,
                    custom_rank,
                    bookmark_rank,
                )
            })
            .collect();
        tables.sort_by_key(|(_, pinned, custom_rank, bookmark_rank)| {
            (!*pinned, *custom_rank, *bookmark_rank)
        });

        for (table, pinned, _, _) in tables {
            self.schema_table_row(ui, active, table, pinned, "tbl", actions);
        }
        for (kind, name, idx) in self.sidebar_drafts() {
            if kind == crate::components::QueryTabKind::Table
                // Same inset as a table row: its padding, chevron slot and gap.
                && draft_object_row(
                    ui,
                    22.0,
                    kind.icon(),
                    &name,
                    "untitled_table",
                    idx == self.active_query_tab,
                )
            {
                actions.push(Action::SelectTab(idx));
            }
        }

        // Views, functions, procedures, and triggers follow the tables.
        self.schema_object_tree(ui, actions);
    }

    /// One table entry in the schema explorer: a modern full-width row — a rounded
    /// selection/hover pill, an accent table icon, the name, and a pin (star) toggle — with
    /// the table's columns / indexes / foreign keys as a collapsible body. Used by both the
    /// "Pinned" group and the main table list; `id_salt` keeps their expand state independent.
    fn schema_table_row(
        &self,
        ui: &mut egui::Ui,
        active: &ActiveConnection,
        table: &dbcore::TableInfo,
        pinned: bool,
        id_salt: &str,
        actions: &mut Vec<Action>,
    ) {
        use egui::collapsing_header::CollapsingState;

        // Selected = this table is what the active tab is currently showing.
        let selected = self.tab().edits.source.as_ref().is_some_and(|s| {
            s.schema.as_deref() == table.schema.as_deref() && s.table == table.name
        });

        let id = ui.make_persistent_id((id_salt, table.schema.as_deref(), table.name.as_str()));
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, false);

        let full_w = ui.available_width();
        let (row_rect, row_resp) = ui.allocate_exact_size(
            egui::vec2(full_w, TREE_ROW_H),
            egui::Sense::click_and_drag(),
        );
        // Keep the row's height in the outer scroll area, but avoid building its icons,
        // interactions, and menus when a collapsed table is outside the viewport. Large schemas
        // commonly contain thousands of tables, so this removes most per-frame widget work.
        if !ui.is_rect_visible(row_rect) && state.openness(ui.ctx()) <= 0.0 {
            return;
        }
        let row_resp = row_resp
            .on_hover_text("Click to preview · drag to reorder or split · double-click to open");
        let payload = SchemaTableDrag {
            conn_id: active.config_id.clone(),
            schema: table.schema.clone(),
            table: table.name.clone(),
            pinned,
        };
        row_resp.dnd_set_drag_payload(payload);
        if row_resp.dragged() {
            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                egui::Area::new(id.with("drag_ghost"))
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .fixed_pos(pointer + egui::vec2(14.0, 14.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::new()
                            .fill(palette::SURFACE())
                            .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()))
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(9, 6))
                            .show(ui, |ui| {
                                ui.horizontal_centered(|ui| {
                                    ui.add(
                                        egui::Image::new(icons::table())
                                            .fit_to_exact_size(egui::Vec2::splat(14.0))
                                            .tint(palette::ACCENT()),
                                    );
                                    ui.label(
                                        egui::RichText::new(&table.name)
                                            .strong()
                                            .color(palette::TEXT()),
                                    );
                                });
                            });
                    });
            }
        }
        if let Some(source) = row_resp.dnd_hover_payload::<SchemaTableDrag>() {
            let same_table = source.conn_id == active.config_id
                && source.schema == table.schema
                && source.table == table.name;
            let compatible =
                source.conn_id == active.config_id && source.pinned == pinned && !same_table;
            if compatible && ui.is_rect_visible(row_rect) {
                let after = drop_after(ui, row_rect);
                paint_drop_line(ui, row_rect, after);
                if let Some(source) = row_resp.dnd_release_payload::<SchemaTableDrag>() {
                    actions.push(Action::MoveSchemaTable {
                        conn_id: active.config_id.clone(),
                        source_schema: source.schema.clone(),
                        source_table: source.table.clone(),
                        target_schema: table.schema.clone(),
                        target_table: table.name.clone(),
                        after,
                    });
                }
            }
        }

        paint_tree_row_fill(ui, row_rect, selected, row_resp.hovered());

        // Row content, painted on top of the pill. The chevron and the star are their own
        // interactive widgets layered above `row_resp`, so they capture their own clicks while
        // the rest of the row drives preview/open.
        let mut toggle_open = false;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(row_rect.shrink2(egui::vec2(6.0, 0.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    // Disclosure chevron uses the shared Tabler icon set.
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
                    if chev_resp.clicked() {
                        toggle_open = true;
                    }

                    // Shared table glyph, with the list's primary colour and selected contrast.
                    let icon_color = if selected {
                        palette::TEXT()
                    } else {
                        palette::ACCENT()
                    };
                    let (icon_rect, _) =
                        ui.allocate_exact_size(egui::vec2(16.0, TREE_ROW_H), egui::Sense::hover());
                    egui::Image::new(icons::table()).tint(icon_color).paint_at(
                        ui,
                        egui::Rect::from_center_size(
                            icon_rect.center(),
                            egui::Vec2::splat(TREE_ICON),
                        ),
                    );

                    // Split the strip left of the row edge: the pin sits flush right, the name
                    // fills everything to its left (left-aligned right after the icon). Computing
                    // the rects directly keeps the star pinned to the edge regardless of name
                    // length, and reserves its slot so hovering never reflows the row.
                    let rest = ui.available_rect_before_wrap();
                    ui.allocate_rect(rest, egui::Sense::hover());
                    let star_rect = egui::Rect::from_min_size(
                        egui::pos2(rest.right() - 20.0, rest.top()),
                        egui::vec2(20.0, TREE_ROW_H),
                    );
                    let label_rect = egui::Rect::from_min_max(
                        rest.min,
                        egui::pos2(star_rect.left() - 4.0, rest.bottom()),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(label_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&table.name).color(palette::TEXT()),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        },
                    );

                    // Pin (star) toggle: always shown when pinned, otherwise only on hover.
                    let star_resp = ui.interact(
                        star_rect,
                        ui.make_persistent_id((
                            id_salt,
                            "star",
                            table.schema.as_deref(),
                            table.name.as_str(),
                        )),
                        egui::Sense::click(),
                    );
                    if (pinned || selected || row_resp.hovered()) && ui.is_rect_visible(star_rect) {
                        let color = if pinned {
                            palette::ACCENT()
                        } else if star_resp.hovered() {
                            palette::TEXT()
                        } else {
                            palette::TEXT_FAINT()
                        };
                        // Solid star once pinned so the "on" state reads at a glance; a hollow
                        // outline for the hover-to-pin affordance.
                        let star = if pinned {
                            icons::star_filled()
                        } else {
                            icons::star()
                        };
                        egui::Image::new(star).tint(color).paint_at(
                            ui,
                            egui::Rect::from_center_size(
                                star_rect.center(),
                                egui::vec2(14.0, 14.0),
                            ),
                        );
                    }
                    if star_resp.clicked() {
                        actions.push(Action::ToggleBookmark {
                            schema: table.schema.clone(),
                            table: table.name.clone(),
                        });
                    }
                    let _ = star_resp.on_hover_text(if pinned { "Unpin" } else { "Pin to top" });
                });
            },
        );

        // Right-click anywhere on the row opens the full table actions menu.
        let kind = active.db.kind();
        row_resp.context_menu(|ui| {
            table_actions_menu(ui, table, pinned, kind, &active.config_id, actions)
        });

        // Single-click previews (reuses the italic preview tab); double-click pins a tab.
        let open_pin = row_resp.double_clicked();
        if row_resp.clicked() || open_pin {
            // Carry the table + its primary key so the previewed rows become editable.
            let source = crate::edit::EditSource {
                schema: table.schema.clone(),
                table: table.name.clone(),
                pk_cols: table
                    .edit_key_candidates()
                    .into_iter()
                    .next()
                    .map(|(_, columns)| columns)
                    .unwrap_or_default(),
            };
            actions.push(Action::OpenTable {
                sql: active
                    .db
                    .kind()
                    .preview_query(&table.qualified(active.db.kind()), 100),
                source,
                pin: open_pin,
                kind: crate::components::QueryTabKind::Table,
            });
        }

        if toggle_open {
            state.toggle(ui);
        }
        // Children stay full-width like Saved Queries: the hover pill is a row, the icon+name
        // sit past the chevron, and a hairline spine ties them to the parent.
        let body = state.show_body_unindented(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add_space(1.0);
            schema_table_body(ui, table);
            ui.add_space(1.0);
        });
        if let Some(inner) = body {
            paint_tree_guide(ui, row_rect, inner.response.rect);
        }
    }

    /// Render the non-table schema objects — views, functions, procedures, triggers — as
    /// collapsible groups beneath the tables. Each group appears only when it has objects
    /// matching the sidebar filter, and is collapsed by default to keep the tree compact.
    fn schema_object_tree(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(active) = self.active() else {
            return;
        };
        let kind = active.db.kind();
        let filter = self.schema_filter.to_lowercase();
        let scope = self.sidebar_schema_scope();
        let in_scope = |schema: Option<&str>| scope.is_none_or(|s| schema == Some(s));
        let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);

        // ── Views: a folder of file-style rows, matching Saved Queries. ──
        let views: Vec<&dbcore::ViewInfo> = active
            .schema
            .views
            .iter()
            .filter(|v| in_scope(v.schema.as_deref()) && matches(&v.name))
            .collect();
        let drafts = self.sidebar_drafts();
        let view_drafts: Vec<(&String, usize)> = drafts
            .iter()
            .filter(|(kind, _, _)| *kind == crate::components::QueryTabKind::View)
            .map(|(_, name, idx)| (name, *idx))
            .collect();
        if !views.is_empty() || !view_drafts.is_empty() {
            if !view_drafts.is_empty() {
                reveal_group_for_draft(
                    ui,
                    "views_group",
                    ui.id().with(("view_draft", self.tab().id)),
                );
            }
            object_group(ui, "views_group", "Views", false, |ui| {
                virtualized_object_rows(ui, &views, |ui, index, view| {
                    let tab_kind = crate::components::QueryTabKind::View;
                    let label = if view.materialized {
                        format!("{} · materialized", view.name)
                    } else {
                        view.name.clone()
                    };
                    ui.push_id(
                        ("view", index, view.schema.as_deref(), view.name.as_str()),
                        |ui| {
                            let row = object_leaf_row(
                                ui,
                                tab_kind.icon(),
                                palette::TEXT_WEAK(),
                                &label,
                                "Click to preview rows · right-click for actions",
                            );
                            row.context_menu(|ui| {
                                ui.set_min_width(170.0);
                                if components::button(ui, icons::edit(), "Edit View…", true)
                                    .clicked()
                                {
                                    actions.push(Action::OpenEditView((*view).clone()));
                                    ui.close();
                                }
                                if components::button(ui, icons::trash(), "Drop View…", true)
                                    .on_hover_text("Delete this view")
                                    .clicked()
                                {
                                    actions.push(Action::DropView((*view).clone()));
                                    ui.close();
                                }
                            });
                            if row.clicked() || row.double_clicked() {
                                let source = crate::edit::EditSource {
                                    schema: view.schema.clone(),
                                    table: view.name.clone(),
                                    pk_cols: Vec::new(),
                                };
                                actions.push(Action::OpenTable {
                                    sql: kind.preview_query(&view.qualified(kind), 100),
                                    source,
                                    pin: row.double_clicked(),
                                    kind: tab_kind,
                                });
                            }
                        },
                    );
                });
                for (name, idx) in &view_drafts {
                    if draft_object_row(
                        ui,
                        TREE_CHILD_INDENT,
                        crate::components::QueryTabKind::View.icon(),
                        name,
                        "untitled_view",
                        *idx == self.active_query_tab,
                    ) {
                        actions.push(Action::SelectTab(*idx));
                    }
                }
            });
        }

        // ── Functions & Procedures: leaf rows; click opens the definition for reading. ──
        for (rk, key, title) in [
            (dbcore::RoutineKind::Function, "fn_group", "Functions"),
            (dbcore::RoutineKind::Procedure, "proc_group", "Procedures"),
        ] {
            let routines: Vec<&dbcore::RoutineInfo> = active
                .schema
                .routines
                .iter()
                .filter(|r| r.kind == rk && in_scope(r.schema.as_deref()) && matches(&r.name))
                .collect();
            let tab_kind = match rk {
                dbcore::RoutineKind::Function => crate::components::QueryTabKind::Function,
                dbcore::RoutineKind::Procedure => crate::components::QueryTabKind::Procedure,
            };
            let routine_drafts: Vec<(&String, usize)> = drafts
                .iter()
                .filter(|(kind, _, _)| *kind == tab_kind)
                .map(|(_, name, idx)| (name, *idx))
                .collect();
            if routines.is_empty() && routine_drafts.is_empty() {
                continue;
            }
            if !routine_drafts.is_empty() {
                reveal_group_for_draft(ui, key, ui.id().with((key, "draft", self.tab().id)));
            }
            object_group(ui, key, title, false, |ui| {
                virtualized_object_rows(ui, &routines, |ui, index, r| {
                    let signature = r.signature();
                    let tab_kind = match r.kind {
                        dbcore::RoutineKind::Function => crate::components::QueryTabKind::Function,
                        dbcore::RoutineKind::Procedure => {
                            crate::components::QueryTabKind::Procedure
                        }
                    };
                    ui.push_id(
                        (
                            "routine",
                            index,
                            r.schema.as_deref(),
                            r.name.as_str(),
                            signature.as_str(),
                        ),
                        |ui| {
                            let row = object_leaf_row(
                                ui,
                                tab_kind.icon(),
                                palette::TEXT_WEAK(),
                                &r.name,
                                &signature,
                            );
                            row.context_menu(|ui| {
                                ui.set_min_width(170.0);
                                if components::button(ui, icons::edit(), "Edit…", true).clicked()
                                {
                                    actions.push(Action::OpenEditRoutine((*r).clone()));
                                    ui.close();
                                }
                                if components::button(ui, icons::trash(), "Drop…", true).clicked()
                                {
                                    actions.push(Action::DropRoutine((*r).clone()));
                                    ui.close();
                                }
                            });
                            if row.clicked() || row.double_clicked() {
                                actions.push(Action::OpenDefinition {
                                    title: r.name.clone(),
                                    sql: r.body.clone(),
                                    kind: tab_kind,
                                });
                            }
                        },
                    );
                });
                for (name, idx) in &routine_drafts {
                    if draft_object_row(
                        ui,
                        TREE_CHILD_INDENT,
                        tab_kind.icon(),
                        name,
                        "untitled_routine",
                        *idx == self.active_query_tab,
                    ) {
                        actions.push(Action::SelectTab(*idx));
                    }
                }
            });
        }

        // ── Triggers: leaf rows; click opens the trigger's CREATE text for reading. ──
        let triggers: Vec<&dbcore::TriggerInfo> = active
            .schema
            .triggers
            .iter()
            .filter(|t| in_scope(t.schema.as_deref()) && matches(&t.name))
            .collect();
        let trigger_drafts: Vec<(&String, usize)> = drafts
            .iter()
            .filter(|(kind, _, _)| *kind == crate::components::QueryTabKind::Trigger)
            .map(|(_, name, idx)| (name, *idx))
            .collect();
        if !triggers.is_empty() || !trigger_drafts.is_empty() {
            if !trigger_drafts.is_empty() {
                reveal_group_for_draft(
                    ui,
                    "trig_group",
                    ui.id().with(("trigger_draft", self.tab().id)),
                );
            }
            object_group(ui, "trig_group", "Triggers", false, |ui| {
                virtualized_object_rows(ui, &triggers, |ui, index, t| {
                    let tab_kind = crate::components::QueryTabKind::Trigger;
                    ui.push_id(
                        (
                            "trigger",
                            index,
                            t.schema.as_deref(),
                            t.table.as_str(),
                            t.name.as_str(),
                        ),
                        |ui| {
                            let row = object_leaf_row(
                                ui,
                                tab_kind.icon(),
                                palette::TEXT_WEAK(),
                                &t.name,
                                &t.display(),
                            );
                            row.context_menu(|ui| {
                                ui.set_min_width(170.0);
                                if components::button(ui, icons::edit(), "Edit Trigger…", true)
                                    .clicked()
                                {
                                    actions.push(Action::OpenEditTrigger((*t).clone()));
                                    ui.close();
                                }
                                if components::button(ui, icons::trash(), "Drop Trigger…", true)
                                    .clicked()
                                {
                                    actions.push(Action::DropTrigger((*t).clone()));
                                    ui.close();
                                }
                            });
                            if row.clicked() || row.double_clicked() {
                                actions.push(Action::OpenDefinition {
                                    title: t.name.clone(),
                                    sql: t.action.clone(),
                                    kind: tab_kind,
                                });
                            }
                        },
                    );
                });
                for (name, idx) in &trigger_drafts {
                    if draft_object_row(
                        ui,
                        TREE_CHILD_INDENT,
                        crate::components::QueryTabKind::Trigger.icon(),
                        name,
                        "untitled_trigger",
                        *idx == self.active_query_tab,
                    ) {
                        actions.push(Action::SelectTab(*idx));
                    }
                }
            });
        }
    }

    /// The objects drafted in open New Table / View / Trigger tabs, with the name typed so far —
    /// only tabs on the connection the explorer shows.
    pub(in crate::app) fn sidebar_drafts(&self) -> Vec<(crate::components::QueryTabKind, String, usize)> {
        use crate::components::QueryTabKind;
        use crate::schema::{ObjectEditor, ObjectMode, SchemaEditorMode};
        let Some(active) = self.active() else {
            return Vec::new();
        };
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.conn_id.as_deref() == Some(active.config_id.as_str()))
            .filter_map(|(idx, tab)| match tab.schema_editor.as_ref()? {
                ObjectEditor::Table(e) if e.mode == SchemaEditorMode::New => {
                    Some((QueryTabKind::Table, e.table_name.clone(), idx))
                }
                ObjectEditor::View(e) if e.mode == ObjectMode::Create => {
                    Some((QueryTabKind::View, e.name.clone(), idx))
                }
                ObjectEditor::Trigger(e) if e.mode == ObjectMode::Create => {
                    Some((QueryTabKind::Trigger, e.name.clone(), idx))
                }
                ObjectEditor::Routine(e) if e.mode == ObjectMode::Create => {
                    let kind = match e.kind {
                        dbcore::RoutineKind::Function => QueryTabKind::Function,
                        dbcore::RoutineKind::Procedure => QueryTabKind::Procedure,
                    };
                    Some((kind, e.name.clone(), idx))
                }
                _ => None,
            })
            .collect()
    }
}
