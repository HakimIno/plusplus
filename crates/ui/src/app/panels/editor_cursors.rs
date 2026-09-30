//! Editor cursors rendering and interaction.

use crate::app::DbGuiApp;
use crate::style::palette;

/// Byte offset of the `char_idx`-th character in `s` (its length when out of range), for
/// turning the editor's char-based caret indices into `str` slice bounds.
pub(super) fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MultiCursorEdit {
    Insert(String),
    Backspace,
    Delete,
}

/// Apply one edit to all selections and return their collapsed post-edit carets in the same
/// order. Exact duplicate/overlapping ranges coalesce so text is never edited twice.
fn apply_multi_cursor_edit(
    sql: &str,
    ranges: &[std::ops::Range<usize>],
    edit: &MultiCursorEdit,
) -> (String, Vec<std::ops::Range<usize>>) {
    let chars: Vec<char> = sql.chars().collect();
    let mut operations = ranges
        .iter()
        .enumerate()
        .map(|(index, range)| {
            let mut start = range.start.min(chars.len());
            let mut end = range.end.min(chars.len()).max(start);
            if start == end {
                match edit {
                    MultiCursorEdit::Backspace if start > 0 => start -= 1,
                    MultiCursorEdit::Delete if end < chars.len() => end += 1,
                    _ => {}
                }
            }
            let replacement = match edit {
                MultiCursorEdit::Insert(text) => text.as_str(),
                MultiCursorEdit::Backspace | MultiCursorEdit::Delete => "",
            };
            (start, end, index, replacement)
        })
        .collect::<Vec<_>>();
    operations.sort_by_key(|(start, end, _, _)| (*start, *end));

    let mut out = String::with_capacity(sql.len());
    let mut source_cursor = 0usize;
    let mut last_caret = 0usize;
    let mut carets = vec![0..0; ranges.len()];
    for (start, end, original_index, replacement) in operations {
        if start < source_cursor {
            carets[original_index] = last_caret..last_caret;
            continue;
        }
        out.extend(chars[source_cursor..start].iter());
        out.push_str(replacement);
        last_caret = out.chars().count();
        carets[original_index] = last_caret..last_caret;
        source_cursor = end;
    }
    out.extend(chars[source_cursor..].iter());
    (out, carets)
}

fn word_range_at(sql: &str, caret: usize) -> std::ops::Range<usize> {
    let chars: Vec<char> = sql.chars().collect();
    let is_word = |ch: char| ch == '_' || ch.is_alphanumeric();
    let mut start = caret.min(chars.len());
    let mut end = start;
    if start == chars.len() || !chars.get(start).is_some_and(|ch| is_word(*ch)) {
        if start > 0 && is_word(chars[start - 1]) {
            start -= 1;
            end = start + 1;
        } else {
            return caret.min(chars.len())..caret.min(chars.len());
        }
    }
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }
    start..end
}

fn next_occurrence(
    sql: &str,
    selected: &std::ops::Range<usize>,
    after: usize,
) -> Option<std::ops::Range<usize>> {
    if selected.is_empty() {
        return None;
    }
    let chars: Vec<char> = sql.chars().collect();
    let needle = chars.get(selected.clone())?;
    if needle.is_empty() || needle.len() > chars.len() {
        return None;
    }
    let find = |range: std::ops::Range<usize>| {
        range
            .filter(|start| *start + needle.len() <= chars.len())
            .find(|start| chars[*start..*start + needle.len()] == *needle)
            .map(|start| start..start + needle.len())
    };
    find(after.min(chars.len())..chars.len()).or_else(|| find(0..selected.start.min(chars.len())))
}

/// One highlighted run of editor text (a find match, an extra cursor's selection): a
/// rounded fill with a thin border, inset a point top and bottom so runs on adjacent lines
/// stay separate instead of merging into one block.
pub(super) fn highlight_shapes(
    out: &mut Vec<egui::Shape>,
    rect: egui::Rect,
    fill: egui::Color32,
    border: egui::Color32,
) {
    let rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() - 1.0, rect.top() + 1.0),
        egui::pos2(rect.right() + 1.0, rect.bottom() - 1.0),
    );
    let radius = egui::CornerRadius::same(3);
    out.push(egui::Shape::rect_filled(rect, radius, fill));
    out.push(egui::Shape::rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    ));
}

impl DbGuiApp {
    pub(super) fn split_sql_editor(&mut self, ui: &mut egui::Ui, font: &egui::FontId) {
        let idx = self.active_query_tab;
        let tab_id = self.tabs[idx].id;
        let wrap_lines = self.editor_wrap_lines;
        egui::Frame::new()
            .fill(palette::CODE_BG())
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                ui.painter().vline(
                    ui.min_rect().left(),
                    ui.min_rect().y_range(),
                    egui::Stroke::new(1.0_f32, palette::BORDER_STRONG()),
                );
                egui::ScrollArea::vertical()
                    .id_salt(("sql_scroll", tab_id, "split"))
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let row_height = ui.fonts_mut(|fonts| fonts.row_height(font));
                        let rows = (ui.available_height() / row_height).floor().max(5.0) as usize;
                        let mut layouter =
                            |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                                let mut job = crate::highlight::highlight_sql_folded(
                                    buf.as_str(),
                                    font.clone(),
                                    &[],
                                );
                                job.wrap.max_width = if wrap_lines {
                                    wrap_width
                                } else {
                                    f32::INFINITY
                                };
                                ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
                            };
                        if self.tabs[idx].split_sql.is_none() {
                            self.tabs[idx].split_sql = Some(self.tabs[idx].sql.clone());
                        }
                        let split_sql = self.tabs[idx]
                            .split_sql
                            .as_mut()
                            .expect("split buffer initialized above");
                        let output = egui::TextEdit::multiline(split_sql)
                            .id_source(("sql_editor", tab_id, "split"))
                            .code_editor()
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::symmetric(10, 0))
                            .desired_rows(rows)
                            .desired_width(f32::INFINITY)
                            .layouter(&mut layouter)
                            .show(ui);
                        output.response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::TextEdit,
                                true,
                                "Split SQL editor",
                            )
                        });
                        if output.response.has_focus() || output.response.clicked() {
                            if let Some(range) = output.cursor_range {
                                self.tabs[idx].primary_cursor = range.as_sorted_char_range();
                            }
                        }
                        if output.response.changed() {
                            let tab = &mut self.tabs[idx];
                            tab.editor_pane = crate::app::EditorPane::Split;
                            tab.edits.source = None;
                            tab.preview = false;
                            tab.extra_cursors.clear();
                            tab.mark_sql_changed();
                            self.workspace_dirty = true;
                        }
                        if output.response.has_focus() {
                            self.tabs[idx].editor_pane = crate::app::EditorPane::Split;
                        }
                        if self.tabs[idx].restore_editor_focus
                            == Some((tab_id, crate::app::EditorPane::Split))
                        {
                            output.response.request_focus();
                            self.tabs[idx].restore_editor_focus = None;
                        }
                    });
            });
    }

    /// Consume text/delete events while secondary carets are active and apply one edit to every
    /// range. egui's TextEdit owns one native cursor, so this small interception keeps its normal
    /// keyboard behavior until the user explicitly enables another caret.
    pub(super) fn apply_multi_cursor_input(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
    ) -> bool {
        let idx = self.active_query_tab;
        if self.tabs[idx].extra_cursors.is_empty()
            || !ctx.memory(|memory| memory.has_focus(editor_id))
        {
            return false;
        }
        let mut edit = None;
        ctx.input_mut(|input| {
            let event_index = input.events.iter().position(|event| match event {
                egui::Event::Text(text) if !text.is_empty() => true,
                egui::Event::Paste(text) if !text.is_empty() => true,
                egui::Event::Key {
                    key: egui::Key::Backspace | egui::Key::Delete | egui::Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } if *modifiers == egui::Modifiers::NONE => true,
                _ => false,
            });
            let Some(event_index) = event_index else {
                return;
            };
            let event = input.events.remove(event_index);
            edit = match event {
                egui::Event::Text(text) | egui::Event::Paste(text) => {
                    Some(MultiCursorEdit::Insert(text))
                }
                egui::Event::Key { key, .. } => match key {
                    egui::Key::Backspace => Some(MultiCursorEdit::Backspace),
                    egui::Key::Delete => Some(MultiCursorEdit::Delete),
                    egui::Key::Enter => Some(MultiCursorEdit::Insert("\n".into())),
                    _ => None,
                },
                _ => None,
            };
        });
        let Some(edit) = edit else { return false };

        let tab = &self.tabs[idx];
        let old_sql = tab.sql.clone();
        let old_primary = tab.primary_cursor.clone();
        let mut ranges = vec![old_primary];
        ranges.extend(tab.extra_cursors.iter().cloned());
        let (new_sql, carets) = apply_multi_cursor_edit(&old_sql, &ranges, &edit);
        let tab = &mut self.tabs[idx];
        tab.sql = new_sql;
        tab.mark_sql_changed();
        tab.edits.source = None;
        tab.preview = false;
        tab.primary_cursor = carets.first().cloned().unwrap_or(0..0);
        tab.extra_cursors = carets.into_iter().skip(1).collect();
        self.workspace_dirty = true;
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            let old_range = egui::text::CCursorRange::two(
                egui::text::CCursor::new(ranges[0].start),
                egui::text::CCursor::new(ranges[0].end),
            );
            let mut undoer = state.undoer();
            undoer.add_undo(&(old_range, old_sql));
            state.set_undoer(undoer);
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(tab.primary_cursor.start),
                )));
            state.store(ctx, editor_id);
        }
        true
    }

    pub(super) fn add_next_cursor(&mut self, ctx: &egui::Context, editor_id: egui::Id) {
        let idx = self.active_query_tab;
        if !ctx.memory(|memory| memory.has_focus(editor_id)) {
            return;
        }
        let current = self.tabs[idx].primary_cursor.clone();
        let sql = self.tabs[idx].sql.clone();
        let selected = if current.is_empty() {
            word_range_at(&sql, current.start)
        } else {
            current.clone()
        };
        if selected.is_empty() {
            return;
        }
        if current.is_empty() {
            self.tabs[idx].primary_cursor = selected.clone();
            self.tabs[idx].folds.clear();
        } else {
            let after = self.tabs[idx]
                .extra_cursors
                .iter()
                .map(|range| range.end)
                .chain(std::iter::once(current.end))
                .max()
                .unwrap_or(current.end);
            let Some(next) = next_occurrence(&sql, &selected, after) else {
                return;
            };
            let existing = std::iter::once(&current)
                .chain(self.tabs[idx].extra_cursors.iter())
                .any(|range| *range == next);
            if !existing {
                self.tabs[idx].extra_cursors.push(next);
            }
            self.tabs[idx].folds.clear();
        }
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            let caret = self.tabs[idx].primary_cursor.clone();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(caret.start),
                    egui::text::CCursor::new(caret.end),
                )));
            state.store(ctx, editor_id);
        }
        ctx.request_repaint();
    }

    pub(super) fn clear_extra_cursors(&mut self) {
        self.tabs[self.active_query_tab].extra_cursors.clear();
    }
}

#[cfg(test)]
mod multi_cursor_tests {
    use super::*;

    #[test]
    fn inserts_at_all_cursors_in_reverse_safe_order() {
        let (sql, carets) = apply_multi_cursor_edit(
            "foo foo",
            &[0..0, 4..4],
            &MultiCursorEdit::Insert("bar".into()),
        );
        assert_eq!(sql, "barfoo barfoo");
        assert_eq!(carets, [3..3, 10..10]);
    }

    #[test]
    fn deletes_selected_ranges_and_handles_unicode_backspace() {
        let (sql, _) = apply_multi_cursor_edit("กขกข", &[1..1, 3..3], &MultiCursorEdit::Backspace);
        assert_eq!(sql, "ขข");
        let (sql, carets) = apply_multi_cursor_edit(
            "one two one",
            &[0..3, 8..11],
            &MultiCursorEdit::Insert("x".into()),
        );
        assert_eq!(sql, "x two x");
        assert_eq!(carets, [1..1, 7..7]);
    }

    #[test]
    fn command_d_finds_word_then_wraps_to_next_match() {
        let sql = "SELECT id FROM ids WHERE id = 1";
        let first = word_range_at(sql, 7);
        assert_eq!(&sql.chars().collect::<Vec<_>>()[first.clone()], &['i', 'd']);
        assert_eq!(next_occurrence(sql, &first, first.end), Some(15..17));
    }
}
