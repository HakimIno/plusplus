//! Global fuzzy switcher for schema objects, tabs, saved queries, and app commands.

use super::*;
use crate::style::palette;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

const MAX_RESULTS: usize = 80;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaletteCommand {
    NewQuery,
    NewConnection,
    SaveQuery,
    Refresh,
    Settings,
    DatabaseDiagram,
    ToggleSchema,
    ToggleDetails,
    ToggleConsole,
    ToggleLiveLog,
}

#[derive(Clone)]
enum OpenAnythingTarget {
    Tab(u64),
    Favorite(String),
    Object {
        conn_id: String,
        schema: Option<String>,
        name: String,
        kind: crate::components::QueryTabKind,
        column: Option<String>,
    },
    Command(PaletteCommand),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenAnythingKind {
    Table,
    View,
    Column,
    Tab,
    SavedQuery,
    Command,
}

impl OpenAnythingKind {
    fn label(self) -> &'static str {
        match self {
            Self::Table => "Table",
            Self::View => "View",
            Self::Column => "Column",
            Self::Tab => "Tab",
            Self::SavedQuery => "Saved query",
            Self::Command => "Command",
        }
    }

    fn icon(self) -> egui::ImageSource<'static> {
        match self {
            Self::Table => crate::icons::table(),
            Self::View => crate::icons::view(),
            Self::Column => crate::icons::column(),
            Self::Tab => crate::icons::file(),
            Self::SavedQuery => crate::icons::star(),
            Self::Command => crate::icons::keyboard_command(),
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Self::Table | Self::Command => palette::ACCENT(),
            Self::View => palette::SUCCESS(),
            Self::Column | Self::SavedQuery => palette::WARNING(),
            Self::Tab => palette::TEXT_WEAK(),
        }
    }

    fn rank_boost(self) -> i32 {
        match self {
            Self::Tab => 550,
            Self::Table => 500,
            Self::View => 450,
            Self::SavedQuery => 300,
            Self::Command => 200,
            Self::Column => 0,
        }
    }
}

#[derive(Clone)]
struct OpenAnythingItem {
    label: String,
    detail: String,
    kind: OpenAnythingKind,
    search: String,
    target: OpenAnythingTarget,
}

pub(super) struct OpenAnythingState {
    query: String,
    selected: usize,
    focus_pending: bool,
    index: Vec<OpenAnythingItem>,
    results: Vec<usize>,
}

impl DbGuiApp {
    pub(super) fn open_open_anything(&mut self) {
        let index = self.build_open_anything_index();
        let results = filtered_indices(&index, "");
        self.open_anything = Some(OpenAnythingState {
            query: String::new(),
            selected: 0,
            focus_pending: true,
            index,
            results,
        });
    }

    pub(super) fn open_anything_shortcut(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
            self.open_open_anything();
        }
    }

    fn build_open_anything_index(&self) -> Vec<OpenAnythingItem> {
        let mut items = Vec::new();

        for (index, tab) in self.tabs.iter().enumerate() {
            let label = self.tab_label(index);
            let connection = tab
                .conn_id
                .as_deref()
                .and_then(|id| {
                    self.connections
                        .iter()
                        .find(|connection| connection.id == id)
                })
                .map(|connection| connection.name.as_str())
                .unwrap_or("No connection");
            items.push(OpenAnythingItem {
                search: format!("{label} {connection} tab").to_lowercase(),
                label,
                detail: connection.to_string(),
                kind: OpenAnythingKind::Tab,
                target: OpenAnythingTarget::Tab(tab.id),
            });
        }

        for favorite in &self.favorites_cache {
            items.push(OpenAnythingItem {
                search: format!(
                    "{} {} {} saved query favorite",
                    favorite.name,
                    favorite.folder_key(),
                    favorite.sql
                )
                .to_lowercase(),
                label: favorite.name.clone(),
                detail: favorite.folder_key().to_string(),
                kind: OpenAnythingKind::SavedQuery,
                target: OpenAnythingTarget::Favorite(favorite.id.clone()),
            });
        }

        for connection in &self.active_connections {
            for table in &connection.schema.tables {
                let qualified = qualified_name(table.schema.as_deref(), &table.name);
                items.push(OpenAnythingItem {
                    search: format!("{qualified} {} table", connection.name).to_lowercase(),
                    label: qualified.clone(),
                    detail: connection.name.clone(),
                    kind: OpenAnythingKind::Table,
                    target: OpenAnythingTarget::Object {
                        conn_id: connection.config_id.clone(),
                        schema: table.schema.clone(),
                        name: table.name.clone(),
                        kind: crate::components::QueryTabKind::Table,
                        column: None,
                    },
                });
                for column in &table.columns {
                    items.push(OpenAnythingItem {
                        search: format!(
                            "{} {qualified} {} {} column",
                            column.name, column.data_type, connection.name
                        )
                        .to_lowercase(),
                        label: column.name.clone(),
                        detail: format!("{qualified} · {} · {}", column.data_type, connection.name),
                        kind: OpenAnythingKind::Column,
                        target: OpenAnythingTarget::Object {
                            conn_id: connection.config_id.clone(),
                            schema: table.schema.clone(),
                            name: table.name.clone(),
                            kind: crate::components::QueryTabKind::Table,
                            column: Some(column.name.clone()),
                        },
                    });
                }
            }
            for view in &connection.schema.views {
                let qualified = qualified_name(view.schema.as_deref(), &view.name);
                items.push(OpenAnythingItem {
                    search: format!("{qualified} {} view", connection.name).to_lowercase(),
                    label: qualified.clone(),
                    detail: connection.name.clone(),
                    kind: OpenAnythingKind::View,
                    target: OpenAnythingTarget::Object {
                        conn_id: connection.config_id.clone(),
                        schema: view.schema.clone(),
                        name: view.name.clone(),
                        kind: crate::components::QueryTabKind::View,
                        column: None,
                    },
                });
                for column in &view.columns {
                    items.push(OpenAnythingItem {
                        search: format!(
                            "{} {qualified} {} {} column view",
                            column.name, column.data_type, connection.name
                        )
                        .to_lowercase(),
                        label: column.name.clone(),
                        detail: format!("{qualified} · {} · {}", column.data_type, connection.name),
                        kind: OpenAnythingKind::Column,
                        target: OpenAnythingTarget::Object {
                            conn_id: connection.config_id.clone(),
                            schema: view.schema.clone(),
                            name: view.name.clone(),
                            kind: crate::components::QueryTabKind::View,
                            column: Some(column.name.clone()),
                        },
                    });
                }
            }
        }

        for (command, label, detail, keywords) in [
            (PaletteCommand::NewQuery, "New query", "⌘T", "new tab sql"),
            (
                PaletteCommand::NewConnection,
                "New connection",
                "",
                "database connect provider",
            ),
            (
                PaletteCommand::SaveQuery,
                "Save query",
                "",
                "favorite saved query",
            ),
            (
                PaletteCommand::Refresh,
                "Refresh current tab",
                "⌘R",
                "reload run query",
            ),
            (
                PaletteCommand::Settings,
                "Open settings",
                "",
                "preferences options",
            ),
            (
                PaletteCommand::DatabaseDiagram,
                "Show database diagram",
                "",
                "erd schema",
            ),
            (
                PaletteCommand::ToggleSchema,
                "Toggle schema sidebar",
                "",
                "items panel",
            ),
            (
                PaletteCommand::ToggleDetails,
                "Toggle details panel",
                "",
                "fields inspector",
            ),
            (
                PaletteCommand::ToggleConsole,
                "Toggle query console",
                "",
                "sql editor",
            ),
            (
                PaletteCommand::ToggleLiveLog,
                "Toggle live log",
                "",
                "history output",
            ),
        ] {
            items.push(OpenAnythingItem {
                label: label.to_string(),
                detail: detail.to_string(),
                kind: OpenAnythingKind::Command,
                search: format!("{label} {keywords} command").to_lowercase(),
                target: OpenAnythingTarget::Command(command),
            });
        }

        items
    }

    pub(super) fn open_anything_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.open_anything.take() else {
            return;
        };
        let mut keep_open = true;
        let mut activate = None;
        let before = state.query.clone();

        egui::Window::new("Open Anything")
            .id(egui::Id::new("open_anything"))
            .anchor(egui::Align2::CENTER_TOP, [0.0, 72.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .fixed_size([620.0, 430.0])
            .frame(crate::components::dialog_frame(ctx).inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                let response = egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 6,
                        bottom: 1,
                    })
                    .show(ui, |ui| {
                        crate::components::icon_text_input(
                            ui,
                            &mut state.query,
                            "Search tables, columns, views, tabs, queries, commands…",
                            crate::icons::search(),
                            ui.available_width(),
                        )
                    })
                    .inner;
                if state.focus_pending {
                    response.request_focus();
                    state.focus_pending = false;
                }
                if state.query != before {
                    state.selected = 0;
                    state.results = filtered_indices(&state.index, &state.query);
                }
                if state.results.is_empty() {
                    state.selected = 0;
                } else {
                    state.selected = state.selected.min(state.results.len() - 1);
                }

                let (up, down, enter, escape) = ui.input_mut(|input| {
                    (
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                    )
                });
                if up && !state.results.is_empty() {
                    state.selected = state
                        .selected
                        .checked_sub(1)
                        .unwrap_or(state.results.len() - 1);
                }
                if down && !state.results.is_empty() {
                    state.selected = (state.selected + 1) % state.results.len();
                }
                if enter {
                    activate = state
                        .results
                        .get(state.selected)
                        .and_then(|index| state.index.get(*index))
                        .map(|item| item.target.clone());
                }
                if escape {
                    keep_open = false;
                }

                ui.separator();
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(if state.query.trim().is_empty() {
                                    "Recently opened"
                                } else {
                                    "Best matches"
                                })
                                .small()
                                .color(palette::TEXT_WEAK()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} results",
                                            state.results.len()
                                        ))
                                        .small()
                                        .color(palette::TEXT_FAINT()),
                                    );
                                },
                            );
                        });
                    });
                egui::ScrollArea::vertical()
                    .id_salt("open_anything_results")
                    .max_height(365.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if state.results.is_empty() {
                            ui.add_space(28.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("No matching items")
                                        .color(palette::TEXT_WEAK()),
                                );
                            });
                        }
                        for (index, item_index) in state.results.iter().copied().enumerate() {
                            let item = &state.index[item_index];
                            let selected = index == state.selected;
                            let response = egui::Frame::new()
                                .fill(if selected {
                                    palette::SELECTION()
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::symmetric(12, 3))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.add(
                                            egui::Image::new(item.kind.icon())
                                                .fit_to_exact_size(egui::vec2(17.0, 17.0))
                                                .tint(item.kind.color()),
                                        );
                                        ui.add_space(8.0);
                                        ui.label(highlighted_label(ui, &item.label, &state.query));
                                        if !item.detail.is_empty() {
                                            ui.add_space(8.0);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&item.detail)
                                                        .small()
                                                        .color(palette::TEXT_WEAK()),
                                                )
                                                .truncate(),
                                            );
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(item.kind.label())
                                                        .small()
                                                        .color(palette::TEXT_FAINT()),
                                                );
                                            },
                                        );
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());
                            if response.hovered() {
                                state.selected = index;
                            }
                            if response.clicked() {
                                activate = Some(item.target.clone());
                            }
                        }
                    });
            });

        if let Some(target) = activate {
            self.activate_open_anything(target);
        } else if keep_open {
            self.open_anything = Some(state);
        }
    }

    fn activate_open_anything(&mut self, target: OpenAnythingTarget) {
        match target {
            OpenAnythingTarget::Tab(id) => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                    self.settings_open = false;
                    self.select_tab(index);
                }
            }
            OpenAnythingTarget::Favorite(id) => {
                if let Some(index) = self.favorites_cache.iter().position(|query| query.id == id) {
                    self.settings_open = false;
                    self.apply_action(Action::UseFavorite(index));
                }
            }
            OpenAnythingTarget::Object {
                conn_id,
                schema,
                name,
                kind,
                column,
            } => {
                self.settings_open = false;
                self.open_catalog_object(&conn_id, schema, &name, kind);
                if let Some(column) = column {
                    self.status_msg = format!("Opened {name} for column {column}");
                }
            }
            OpenAnythingTarget::Command(command) => match command {
                PaletteCommand::NewQuery => self.apply_action(Action::NewTab),
                PaletteCommand::NewConnection => self.apply_action(Action::NewConnection),
                PaletteCommand::SaveQuery if !self.tabs.is_empty() => {
                    self.apply_action(Action::SaveCurrentAsFavorite)
                }
                PaletteCommand::Refresh if !self.tabs.is_empty() => {
                    self.apply_action(Action::RunQuery)
                }
                PaletteCommand::Settings => self.apply_action(Action::OpenSettings),
                PaletteCommand::DatabaseDiagram if !self.tabs.is_empty() => {
                    self.apply_action(Action::ShowDatabaseDiagram)
                }
                PaletteCommand::ToggleSchema => self.show_schema_panel = !self.show_schema_panel,
                PaletteCommand::ToggleDetails => self.show_details_panel = !self.show_details_panel,
                PaletteCommand::ToggleConsole => self.show_query_console = !self.show_query_console,
                PaletteCommand::ToggleLiveLog => self.show_live_log = !self.show_live_log,
                PaletteCommand::SaveQuery
                | PaletteCommand::Refresh
                | PaletteCommand::DatabaseDiagram => {}
            },
        }
    }
}

fn qualified_name(schema: Option<&str>, name: &str) -> String {
    schema.map_or_else(|| name.to_string(), |schema| format!("{schema}.{name}"))
}

fn filtered_indices(index: &[OpenAnythingItem], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return index
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                matches!(
                    item.kind,
                    OpenAnythingKind::Tab
                        | OpenAnythingKind::SavedQuery
                        | OpenAnythingKind::Command
                )
            })
            .take(MAX_RESULTS)
            .map(|(index, _)| index)
            .collect();
    }
    let mut best = BinaryHeap::with_capacity(MAX_RESULTS + 1);
    for (index, item) in index.iter().enumerate() {
        if let Some(score) = fuzzy_score_lowercase(&query, &item.search) {
            best.push(Reverse((score + item.kind.rank_boost(), index)));
            if best.len() > MAX_RESULTS {
                best.pop();
            }
        }
    }
    let mut ranked: Vec<(i32, usize)> = best
        .into_iter()
        .map(|Reverse((score, index))| (score, index))
        .collect();
    ranked.sort_unstable_by(|(left_score, left), (right_score, right)| {
        right_score.cmp(left_score).then_with(|| {
            index[*left]
                .label
                .to_lowercase()
                .cmp(&index[*right].label.to_lowercase())
        })
    });
    ranked
        .into_iter()
        .map(|(_, item_index)| item_index)
        .collect()
}

fn fuzzy_score_lowercase(query: &str, candidate: &str) -> Option<i32> {
    if let Some(position) = candidate.find(query) {
        return Some(10_000 - position as i32 * 4 - candidate.len() as i32);
    }
    let mut score = 0;
    let mut next = 0;
    let mut previous = None;
    for needle in query.chars().filter(|character| !character.is_whitespace()) {
        let (offset, _) = candidate[next..]
            .char_indices()
            .find(|(_, value)| *value == needle)?;
        let position = next + offset;
        score += if previous == Some(position.saturating_sub(1)) {
            12
        } else {
            2
        };
        previous = Some(position);
        next = position + needle.len_utf8();
    }
    Some(score - candidate.len() as i32)
}

fn highlighted_label(ui: &egui::Ui, label: &str, query: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};

    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let normal = TextFormat {
        font_id: font_id.clone(),
        color: palette::TEXT(),
        ..Default::default()
    };
    let matched = TextFormat {
        font_id,
        color: palette::ACCENT(),
        ..Default::default()
    };
    let needles: Vec<char> = query
        .trim()
        .to_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let mut needle = 0;
    let mut job = LayoutJob::default();
    for character in label.chars() {
        let is_match = needles
            .get(needle)
            .is_some_and(|wanted| character.to_lowercase().next() == Some(*wanted));
        let mut bytes = [0; 4];
        job.append(
            character.encode_utf8(&mut bytes),
            0.0,
            if is_match {
                matched.clone()
            } else {
                normal.clone()
            },
        );
        if is_match {
            needle += 1;
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_prefers_contiguous_and_supports_subsequences() {
        assert!(fuzzy_score_lowercase("acct", "account table").is_some());
        assert!(fuzzy_score_lowercase("pvdt", "ac_pv_detail table").is_some());
        assert!(
            fuzzy_score_lowercase("account", "account table").unwrap()
                > fuzzy_score_lowercase("acnt", "account table").unwrap()
        );
        assert!(fuzzy_score_lowercase("missing", "account table").is_none());
    }

    #[test]
    fn empty_palette_includes_commands_tabs_and_saved_queries() {
        let mut app = DbGuiApp::construct();
        app.favorites_cache.push(dbcore::Favorite {
            id: "daily".into(),
            name: "Daily report".into(),
            sql: "SELECT 1".into(),
            conn_id: None,
            conn_name: None,
            folder: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        });
        let index = app.build_open_anything_index();
        let items = filtered_indices(&index, "");
        assert!(items
            .iter()
            .any(|item| index[*item].kind == OpenAnythingKind::Tab));
        assert!(items
            .iter()
            .any(|item| index[*item].kind == OpenAnythingKind::SavedQuery));
        assert!(items
            .iter()
            .any(|item| index[*item].kind == OpenAnythingKind::Command));
        assert!(!items
            .iter()
            .any(|item| index[*item].kind == OpenAnythingKind::Table));
    }
}
