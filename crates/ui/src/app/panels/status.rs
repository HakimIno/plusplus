//! Status rendering and interaction.

use crate::app::{Action, DbGuiApp, QueryTab};
use crate::icons;
use crate::style::palette;

const ROW_H: f32 = 18.0;
/// Past this many selected rows the numeric summary is skipped: it is recomputed every frame.
const MAX_STAT_ROWS: usize = 200_000;

/// Count / sum / average of the numbers in the cursor's column across the selected rows.
#[derive(Debug, PartialEq)]
struct SelectionStats {
    count: usize,
    sum: f64,
}

impl SelectionStats {
    fn average(&self) -> f64 {
        self.sum / self.count as f64
    }
}

/// `1234567.5` → `1,234,567.5`; whole numbers drop the fraction, others keep two places.
fn format_number(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    let mut text = format!("{:.2}", rounded.abs());
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    let (int, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let mut grouped = String::new();
    for (i, digit) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if rounded < 0.0 { "-" } else { "" };
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

/// `3.2 ms`, `128 ms`, `1.40 s`.
fn format_duration_ms(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

/// 1-based `(line, column, selected chars)` of the SQL editor's primary cursor. Counts chars,
/// not bytes, so Thai text reports the position a person would count.
fn caret_position(sql: &str, cursor: &std::ops::Range<usize>) -> (usize, usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for ch in sql.chars().take(cursor.start) {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column, cursor.end.saturating_sub(cursor.start))
}

fn as_number(value: &dbcore::Value) -> Option<f64> {
    let number = match value {
        dbcore::Value::Int(i) => *i as f64,
        dbcore::Value::Float(f) => *f,
        // NUMERIC / DECIMAL arrive as text to keep their precision.
        dbcore::Value::Text(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

fn selection_stats(tab: &QueryTab) -> Option<SelectionStats> {
    let result = tab.result.as_ref()?;
    let (_, col) = tab.selection.cursor()?;
    if tab.selection.len() < 2 || tab.selection.len() > MAX_STAT_ROWS || col >= result.column_count()
    {
        return None;
    }
    let mut stats = SelectionStats { count: 0, sum: 0.0 };
    for disp in tab.selection.iter() {
        let Some(raw) = crate::edit::disp_to_raw(&tab.row_order, tab.edits.new_rows, disp) else {
            continue;
        };
        // What the grid shows: the staged edit when there is one.
        let number = match tab.edits.staged(raw, col) {
            Some(value) => as_number(value),
            None => crate::edit::original_value(result, raw, col)
                .as_ref()
                .and_then(as_number),
        };
        if let Some(number) = number {
            stats.count += 1;
            stats.sum += number;
        }
    }
    (stats.count > 0).then_some(stats)
}

fn dot(ui: &mut egui::Ui) {
    ui.colored_label(palette::TEXT_FAINT(), "·");
}

fn chip(ui: &mut egui::Ui, text: impl Into<String>, color: egui::Color32) -> egui::Response {
    ui.label(egui::RichText::new(text.into()).size(11.0).color(color))
}

impl DbGuiApp {
    /// Thin status strip pinned to the very bottom edge. Left: what just happened, the filter
    /// and the selection (with a numeric summary). Right, from the edge: version, the tab's
    /// connection, its last result's size and time, the editor caret, staged changes, and a
    /// running query's clock with a cancel button.
    pub(in crate::app) fn status_bar(&mut self, root: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::bottom("status_bar").show_inside(root, |ui| {
            ui.add_space(2.0);
            let row = egui::Rect::from_min_size(
                ui.cursor().min,
                egui::vec2(ui.available_width(), ROW_H),
            );
            ui.allocate_rect(row, egui::Sense::hover());

            // The right cluster is laid out first so the left one can take what it leaves.
            let right_rect = row.shrink2(egui::vec2(8.0, 0.0));
            let right = ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(right_rect)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    self.status_right(ui, actions);
                },
            );
            let left_rect = egui::Rect::from_min_max(
                row.min,
                egui::pos2((right.response.rect.left() - 12.0).max(row.min.x), row.max.y),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(left_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.set_clip_rect(left_rect.intersect(ui.clip_rect()));
                    ui.add_space(4.0);
                    self.status_left(ui);
                },
            );
            ui.add_space(3.0);
        });
    }

    fn status_left(&self, ui: &mut egui::Ui) {
        if let Some(err) = &self.error {
            icons::show_colored(ui, icons::warning(), 13.0, palette::DANGER());
            ui.label(egui::RichText::new(err).size(11.0).color(palette::DANGER()));
            return;
        }
        icons::show_native(ui, icons::table(), 12.0);
        chip(ui, &self.status_msg, palette::TEXT_WEAK());
        let Some(tab) = self.tabs.get(self.active_query_tab) else {
            return;
        };
        if tab.sort.is_some() {
            ui.colored_label(palette::TEXT_WEAK(), "· Sorted loaded rows only");
        }
        let Some(res) = &tab.result else {
            return;
        };
        if tab.filter.is_active() && tab.row_order.len() != res.row_count() {
            dot(ui);
            icons::show_colored(ui, icons::filter(), 13.0, palette::ACCENT());
            ui.colored_label(
                palette::ACCENT(),
                format!("{} of {} rows", tab.row_order.len(), res.row_count()),
            );
        }
        if !tab.selection.is_empty() {
            dot(ui);
            let n = tab.selection.len();
            let label = if n > 1 {
                format!("{n} rows selected")
            } else if let Some(lead) = tab.selection.lead() {
                format!("row {}", lead + 1)
            } else {
                String::new()
            };
            ui.colored_label(palette::TEXT_WEAK(), label);
            if let Some(stats) = selection_stats(tab) {
                dot(ui);
                ui.colored_label(
                    palette::TEXT_WEAK(),
                    format!(
                        "Count {} · Sum {} · Avg {}",
                        stats.count,
                        format_number(stats.sum),
                        format_number(stats.average())
                    ),
                );
            }
        }
    }

    /// Items are added right-to-left, so the first one lands on the window edge.
    fn status_right(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        chip(
            ui,
            format!("v{}", crate::update::CURRENT_VERSION),
            palette::TEXT_FAINT(),
        );
        let Some(tab) = self.tabs.get(self.active_query_tab) else {
            return;
        };
        let tab_id = tab.id;

        // The tab's own connection, with a flag when it can't be written to.
        let connection = tab.conn_id.as_deref().and_then(|id| {
            self.active_connections
                .iter()
                .find(|conn| conn.config_id == id)
        });
        if let Some(conn) = connection {
            let read_only = self.tab_connection_is_read_only(self.active_query_tab);
            let label = if read_only {
                format!("{} · READ-ONLY", conn.name)
            } else {
                conn.name.clone()
            };
            chip(ui, label, palette::TEXT_WEAK()).on_hover_text(format!(
                "{} · {}",
                conn.db.kind().label(),
                conn.schema.database_name
            ));
        }

        // The last result's size and time.
        if let Some(result) = tab.result.as_ref().filter(|_| !self.is_tab_querying(tab_id)) {
            let size = match result.stats.rows_affected {
                Some(n) => format!("{n} affected"),
                None => format!(
                    "{} row{}{}",
                    result.row_count(),
                    if result.row_count() == 1 { "" } else { "s" },
                    if result.truncated { "+" } else { "" }
                ),
            };
            chip(
                ui,
                format!("{size} · {}", format_duration_ms(result.stats.elapsed_ms)),
                palette::TEXT_WEAK(),
            )
            .on_hover_text("Rows returned and how long the last query took");
        }

        // The SQL editor's caret.
        if self.tab_has_sql_editor() {
            let (line, column, selected) = caret_position(&tab.sql, &tab.primary_cursor);
            let mut text = format!("Ln {line}, Col {column}");
            if selected > 0 {
                text.push_str(&format!(" ({selected} selected)"));
            }
            chip(ui, text, palette::TEXT_WEAK());
        }

        // Staged changes, one click from their preview.
        if tab.edits.has_pending() {
            let shortcut = if cfg!(target_os = "macos") {
                "⌘S"
            } else {
                "Ctrl+S"
            };
            let count = tab.edits.pending_count();
            let response = chip(
                ui,
                format!(
                    "{count} pending change{} · {shortcut}",
                    if count == 1 { "" } else { "s" }
                ),
                palette::ACCENT(),
            )
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Review and save the staged changes");
            if response.clicked() {
                actions.push(Action::PreviewEdits);
            }
        }

        // A running query: its clock, and a way to stop it.
        if self.is_tab_querying(tab_id) {
            let started = self.query_jobs.get(&tab_id).map(|job| job.started);
            // Right-to-left: the cancel button first, so it sits right of the clock.
            let cancel = ui
                .add(
                    egui::Image::new(icons::close())
                        .fit_to_exact_size(egui::Vec2::splat(12.0))
                        .tint(palette::TEXT_WEAK())
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Cancel query");
            if cancel.clicked() {
                actions.push(Action::CancelTabQuery(tab_id));
            }
            if let Some(started) = started {
                let seconds = started.elapsed().as_secs_f64();
                chip(ui, format!("Running {seconds:.1}s"), palette::ACCENT());
                // Keep the clock ticking while nothing else repaints.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_thousands_and_trim_zeros() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(4310.0), "4,310");
        assert_eq!(format_number(1234567.5), "1,234,567.5");
        assert_eq!(format_number(359.1666), "359.17");
        assert_eq!(format_number(-1234.0), "-1,234");
        assert_eq!(format_number(-0.004), "0");
    }

    #[test]
    fn durations_pick_a_readable_unit() {
        assert_eq!(format_duration_ms(3.21), "3.2 ms");
        assert_eq!(format_duration_ms(128.4), "128 ms");
        assert_eq!(format_duration_ms(1400.0), "1.40 s");
    }

    #[test]
    fn caret_counts_lines_and_columns_in_chars() {
        let sql = "SELECT 1\nFROM ก่อน\nWHERE x";
        assert_eq!(caret_position(sql, &(0..0)), (1, 1, 0));
        assert_eq!(caret_position(sql, &(9..9)), (2, 1, 0));
        // Past the five Thai chars of line 2, counted as chars rather than bytes.
        assert_eq!(caret_position(sql, &(19..22)), (3, 1, 3));
    }

    #[test]
    fn numbers_come_from_ints_floats_and_numeric_text_only() {
        use dbcore::Value;
        assert_eq!(as_number(&Value::Int(7)), Some(7.0));
        assert_eq!(as_number(&Value::Text(" 12.50 ".into())), Some(12.5));
        assert_eq!(as_number(&Value::Text("abc".into())), None);
        assert_eq!(as_number(&Value::Text("NaN".into())), None);
        assert_eq!(as_number(&Value::Null), None);
        assert_eq!(as_number(&Value::Bool(true)), None);
    }
}
