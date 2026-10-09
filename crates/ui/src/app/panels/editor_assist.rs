//! Editor assist rendering and interaction.

use super::editor_cursors::char_to_byte;
use crate::app::DbGuiApp;
use crate::icons;
use crate::style::palette;

/// Width of the SQL gutter's fold chevron column, in points.
pub(super) const FOLD_CHEVRON_W: f32 = 12.0;

/// Whether a syntax error covers the caret — the token the user is in the middle of typing,
/// which is half-written by definition and must not be flagged yet. The range is inclusive at
/// both ends: a caret sitting immediately after the token is still "inside" the word being
/// typed, and one sitting immediately before it is about to extend it.
pub(in crate::app) fn error_under_caret(
    range: &std::ops::Range<usize>,
    caret: Option<usize>,
) -> bool {
    caret.is_some_and(|caret| range.start <= caret && caret <= range.end)
}

/// Pure core of the Cmd/Ctrl+/ comment toggle. Given the buffer and a sorted **char** range,
/// returns the byte range to replace and its replacement — or `None` when there's nothing to
/// do (an all-blank selection). VS Code semantics: the selection is grown to whole lines, and
/// if every non-blank line it touches already starts (after its indent) with `--`, the markers
/// are stripped; otherwise a `-- ` is inserted on each non-blank line at the shallowest indent
/// so the markers line up. Only the touched slice is scanned and rebuilt, in a single pass.
pub(in crate::app) fn toggle_comment_edit(
    text: &str,
    sel: std::ops::Range<usize>,
) -> Option<(std::ops::Range<usize>, String)> {
    let bytes = text.as_bytes();
    let sel_start = char_to_byte(text, sel.start);
    let sel_end = char_to_byte(text, sel.end);

    // Grow the range to whole lines: back up to the char after the previous newline, and
    // forward to the newline ending the last touched line. A non-empty selection ending exactly
    // at a line start doesn't really reach that line, so drop it (matching VS Code).
    let first_line_start = bytes[..sel_start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let mut region_end = sel_end;
    if region_end > sel_start && region_end > 0 && bytes[region_end - 1] == b'\n' {
        region_end -= 1;
    }
    let last_line_end = bytes[region_end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(text.len(), |i| region_end + i);

    let region = &text[first_line_start..last_line_end];
    let lines: Vec<&str> = region.split('\n').collect();

    let is_blank = |l: &str| l.trim().is_empty();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let commented = |l: &str| l.trim_start().starts_with("--");

    // Nothing meaningful to comment on an all-blank selection.
    if lines.iter().all(|l| is_blank(l)) {
        return None;
    }
    // Uncomment only when every non-blank line already carries a marker; a single bare line
    // means the toggle adds markers instead.
    let uncomment = lines.iter().filter(|l| !is_blank(l)).all(|l| commented(l));
    // Insert every marker at the shallowest indent so they line up under the code.
    let col = lines
        .iter()
        .filter(|l| !is_blank(l))
        .map(|l| indent(l))
        .min()
        .unwrap_or(0);

    let mut out = String::with_capacity(region.len() + lines.len() * 3);
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if uncomment {
            let ind = indent(line);
            match line[ind..].strip_prefix("--") {
                // Drop the marker and, if present, the single space we insert after it.
                Some(rest) => {
                    out.push_str(&line[..ind]);
                    out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
                None => out.push_str(line),
            }
        } else if is_blank(line) {
            out.push_str(line);
        } else {
            out.push_str(&line[..col]);
            out.push_str("-- ");
            out.push_str(&line[col..]);
        }
    }

    Some((first_line_start..last_line_end, out))
}

/// The char span of the statement under `caret`, trimmed of surrounding whitespace — or
/// `None` while the buffer holds a single statement, where tinting it would light up the
/// whole editor for no reason.
pub(in crate::app) fn current_statement_span(
    sql: &str,
    caret: usize,
) -> Option<std::ops::Range<usize>> {
    let chars: Vec<char> = sql.chars().collect();
    let range = crate::sqlctx::statement_range(&chars, caret);
    let first = (range.start..range.end).find(|&i| !chars[i].is_whitespace())?;
    let last = (range.start..range.end)
        .rev()
        .find(|&i| !chars[i].is_whitespace())
        .unwrap_or(first);
    let blank = |span: std::ops::Range<usize>| chars[span].iter().all(|c| c.is_whitespace());
    // `range` stops short of the `;`, so look past it for another statement.
    let alone = blank(0..first) && blank((last + 2).min(chars.len())..chars.len());
    (!alone).then_some(first..last + 1)
}

impl DbGuiApp {
    /// Paint the SQL editor's gutter — line numbers and fold chevrons — and apply a click on
    /// one of them.
    ///
    /// Runs after the editor so it can place every row from the galley itself: a line that
    /// wrapped is two rows tall, and its number must sit beside the first of them rather than
    /// where a fixed row-height count would put it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fold_gutter(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        view: &crate::fold::View,
        regions: &[crate::fold::Region],
        galley: &egui::Galley,
        galley_pos: egui::Pos2,
        font: &egui::FontId,
        row_height: f32,
    ) {
        let painter = ui.painter();
        painter.vline(
            rect.right(),
            rect.y_range(),
            egui::Stroke::new(1.0_f32, palette::BORDER()),
        );

        // Open regions only show their chevron while the pointer is over the gutter, the way
        // every editor does it: the numbers stay quiet until you go looking for a fold.
        let gutter_hovered = ui.rect_contains_pointer(rect);
        let clip = ui.clip_rect();
        let idx = self.active_query_tab;
        let mut toggle = None;

        // Walk the galley's rows once instead of asking it for each line's position: a cursor
        // lookup scans rows from the top, which made this loop quadratic in the line count
        // (~100 ms a frame at 5,000 lines). A row starts a display line when the one above it
        // ended with a newline; a wrapped line's later rows belong to the line already placed.
        let mut display_line = 0usize;
        let mut starts_line = true;
        for placed in &galley.rows {
            let begins = std::mem::replace(&mut starts_line, placed.ends_with_newline);
            if !begins {
                continue;
            }
            let line = display_line;
            display_line += 1;
            let Some(&source_line) = view.source_lines.get(line) else {
                break;
            };
            let top = galley_pos.y + placed.pos.y;
            if top > clip.bottom() {
                break;
            }
            if top + row_height < clip.top() {
                continue;
            }
            // Number first, then the chevron column hard against the code — the fold marker
            // belongs beside the line it opens, not out at the far edge of the gutter.
            painter.text(
                egui::pos2(rect.right() - FOLD_CHEVRON_W - 6.0, top),
                egui::Align2::RIGHT_TOP,
                source_line + 1,
                font.clone(),
                palette::TEXT_FAINT(),
            );

            let Some(region) = regions.iter().find(|r| r.header_line == source_line) else {
                continue;
            };
            let folded = self.tabs[idx].folds.contains(&region.anchor);
            let chevron_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - FOLD_CHEVRON_W - 1.0, top),
                egui::vec2(FOLD_CHEVRON_W, row_height),
            );
            let resp = ui.interact(
                chevron_rect,
                ui.id().with(("sql_fold", region.anchor)),
                egui::Sense::click(),
            );
            if resp.clicked() {
                toggle = Some(region.anchor);
            }
            if folded || gutter_hovered {
                let icon = if folded {
                    icons::chevron_right()
                } else {
                    icons::chevron_down()
                };
                let tint = if folded || resp.hovered() {
                    palette::TEXT_WEAK()
                } else {
                    palette::TEXT_FAINT()
                };
                egui::Image::new(icon)
                    .fit_to_exact_size(egui::Vec2::splat(FOLD_CHEVRON_W))
                    .tint(tint)
                    .paint_at(
                        ui,
                        egui::Rect::from_center_size(
                            chevron_rect.center(),
                            egui::Vec2::splat(FOLD_CHEVRON_W),
                        ),
                    );
            }
        }

        if let Some(anchor) = toggle {
            if !self.tabs[idx].folds.remove(&anchor) {
                self.tabs[idx].folds.insert(anchor);
            }
        }
    }

    /// Drive the editor's inline ghost-text suggestion for one frame: recompute it from the
    /// caret, paint the greyed remainder, and apply a pending Tab acceptance. While the
    /// autocomplete popup is open (they share the Tab key) it previews the popup's selected
    /// row instead.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_ghost(
        &mut self,
        ui: &egui::Ui,
        ctx: &egui::Context,
        editor_id: egui::Id,
        focused: bool,
        cursor_char: Option<usize>,
        cursor_rect: Option<egui::Rect>,
        font: &egui::FontId,
        accept: bool,
    ) {
        let idx = self.active_query_tab;
        // The popup owns the Tab key while it's up; only its selected row is previewed then.
        let (Some(cursor_char), Some(cursor_rect)) = (cursor_char, cursor_rect) else {
            self.tabs[idx].editor_assist.ghost_suggestion = None;
            self.tabs[idx].editor_assist.ghost_key = None;
            return;
        };
        if !focused || self.tabs[idx].editor_assist.autocomplete.open {
            self.tabs[idx].editor_assist.ghost_suggestion = None;
            self.tabs[idx].editor_assist.ghost_key = None;
            // While the popup is up, its highlighted row previews inline instead, so the
            // caret shows what Enter/Tab will produce. Painted only at the end of a line:
            // ghost text is drawn over the editor, not inserted, and would overlap anything
            // already after the caret.
            let assist = &self.tabs[idx].editor_assist;
            let line_rest_blank = self.tabs[idx]
                .sql
                .chars()
                .skip(cursor_char)
                .take_while(|&c| c != '\n')
                .all(char::is_whitespace);
            // A schema-qualified insertion rewrites the start of the word, which an appended
            // preview can't show.
            let qualified = self.editor_options.prefix_schema
                && assist
                    .autocomplete
                    .items
                    .get(assist.autocomplete.selected)
                    .is_some_and(|item| item.schema.is_some());
            if focused && assist.autocomplete.open && line_rest_blank && !qualified {
                if let Some(tail) = crate::autocomplete::selected_tail(&assist.autocomplete) {
                    ui.painter().text(
                        egui::pos2(cursor_rect.left(), cursor_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        tail,
                        font.clone(),
                        palette::TEXT_FAINT(),
                    );
                }
            }
            return;
        }

        // Recompute only when the text or caret moved since the cached suggestion. While the
        // focused editor merely repaints — e.g. the result grid is scrolling — reuse the cached
        // value instead of re-scanning history and the schema every frame.
        let tab_id = self.tabs[idx].id;
        let cache_hit =
            self.tabs[idx]
                .editor_assist
                .ghost_key
                .as_ref()
                .is_some_and(|(id, sql, caret)| {
                    *id == tab_id && *caret == cursor_char && sql == &self.tabs[idx].sql
                });
        if !cache_hit {
            let suggestion = {
                let (schema, kind) = match self.active() {
                    Some(c) => (Some(&c.schema), Some(c.db.kind())),
                    None => (None, None),
                };
                // Only complete from history this tab's own connection produced — never from
                // another database's queries (whose tables, and SQL dialect, won't match).
                let conn_id = self.tabs[idx].conn_id.as_deref();
                let pool: Vec<&str> = match conn_id {
                    Some(id) => self
                        .suggest_pool
                        .iter()
                        .filter(|q| q.conn_id == id)
                        .map(|q| q.sql.as_str())
                        .collect(),
                    None => Vec::new(),
                };
                let sql = &self.tabs[idx].sql;
                let known = self.tabs[idx]
                    .editor_assist
                    .autocomplete
                    .inline_hint
                    .as_ref()
                    .filter(|(key, _)| *key == (self.tabs[idx].sql_revision, cursor_char))
                    .map(|(_, hint)| hint.clone());
                crate::ghost::suggest_with(
                    sql,
                    cursor_char,
                    &pool,
                    schema,
                    kind,
                    &self.editor_options,
                    known,
                )
            };
            self.tabs[idx].editor_assist.ghost_suggestion = suggestion;
            self.tabs[idx].editor_assist.ghost_key =
                Some((tab_id, self.tabs[idx].sql.clone(), cursor_char));
        }

        let Some(remainder) = self.tabs[idx].editor_assist.ghost_suggestion.clone() else {
            return;
        };

        if accept {
            self.accept_ghost(ctx, editor_id, cursor_char, &remainder);
            self.tabs[idx].editor_assist.ghost_suggestion = None;
            self.tabs[idx].editor_assist.ghost_key = None;
            return;
        }

        // Paint the single-line remainder in a faint colour, flush against the caret.
        // `ghost::suggest` excludes multi-line history entries so this never looks like
        // another query layered over the editor.
        ui.painter().text(
            egui::pos2(cursor_rect.left(), cursor_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &remainder,
            font.clone(),
            palette::TEXT_FAINT(),
        );
        self.tabs[idx].editor_assist.ghost_suggestion = Some(remainder);
    }

    pub(super) fn finish_external_editor_edit(&mut self, idx: usize) {
        let tab = &mut self.tabs[idx];
        tab.folds.clear();
        tab.extra_cursors.clear();
        tab.snippet_placeholders.clear();
        tab.mark_sql_changed();
        tab.edits.source = None;
        tab.preview = false;
        self.workspace_dirty = true;
    }

    fn select_editor_range(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        range: std::ops::Range<usize>,
    ) {
        self.set_editor_selection(ctx, editor_id, range, true);
    }

    /// Select `range` (char offsets into the SQL) in the editor, optionally focusing it.
    pub(super) fn set_editor_selection(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        range: std::ops::Range<usize>,
        focus: bool,
    ) {
        let idx = self.active_query_tab;
        self.tabs[idx].folds.clear();
        self.tabs[idx].primary_cursor = range.clone();
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(range.start),
                    egui::text::CCursor::new(range.end),
                )));
            state.store(ctx, editor_id);
        }
        if focus {
            ctx.memory_mut(|memory| memory.request_focus(editor_id));
        }
    }

    /// `sel<Tab>`, `ins<Tab>`, … expand built-in SQL templates. Placeholder ranges remain
    /// selections, making each subsequent Tab jump to the next value to fill.
    pub(super) fn expand_snippet_at_caret(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
    ) -> bool {
        let idx = self.active_query_tab;
        let caret = self.tabs[idx].primary_cursor.end;
        if self.tabs[idx].primary_cursor.start != caret {
            return false;
        }
        let chars: Vec<char> = self.tabs[idx].sql.chars().collect();
        let start = chars[..caret.min(chars.len())]
            .iter()
            .rposition(|c| !c.is_ascii_alphanumeric() && *c != '_')
            .map_or(0, |at| at + 1);
        let trigger: String = chars[start..caret.min(chars.len())].iter().collect();
        let Some(snippet) = crate::editor_tools::SNIPPETS
            .iter()
            .find(|snippet| snippet.trigger.eq_ignore_ascii_case(&trigger))
        else {
            return false;
        };
        let (expanded, ranges) = crate::editor_tools::expand_snippet(snippet.body);
        let byte_start = char_to_byte(&self.tabs[idx].sql, start);
        let byte_end = char_to_byte(&self.tabs[idx].sql, caret);
        self.tabs[idx]
            .sql
            .replace_range(byte_start..byte_end, &expanded);
        self.tabs[idx].snippet_placeholders = ranges
            .into_iter()
            .map(|range| start + range.start..start + range.end)
            .collect();
        self.tabs[idx].snippet_placeholder = 0;
        self.finish_external_editor_edit_preserving_snippet(idx);
        let range = self.tabs[idx]
            .snippet_placeholders
            .first()
            .cloned()
            .unwrap_or_else(|| {
                let end = start + expanded.chars().count();
                end..end
            });
        self.select_editor_range(ctx, editor_id, range);
        self.status_msg = format!("Inserted {} snippet", snippet.label);
        true
    }

    pub(super) fn advance_snippet_placeholder(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
    ) -> bool {
        let idx = self.active_query_tab;
        if self.tabs[idx].snippet_placeholders.is_empty() {
            return false;
        }
        let next = self.tabs[idx].snippet_placeholder + 1;
        if next >= self.tabs[idx].snippet_placeholders.len() {
            self.tabs[idx].snippet_placeholders.clear();
            return true;
        }
        self.tabs[idx].snippet_placeholder = next;
        let range = self.tabs[idx].snippet_placeholders[next].clone();
        self.select_editor_range(ctx, editor_id, range);
        true
    }

    fn finish_external_editor_edit_preserving_snippet(&mut self, idx: usize) {
        let placeholders = std::mem::take(&mut self.tabs[idx].snippet_placeholders);
        self.finish_external_editor_edit(idx);
        self.tabs[idx].snippet_placeholders = placeholders;
    }

    /// Add the small pieces TextEdit intentionally leaves to code editors: matching closers
    /// and indentation after Enter. Multi-character text events and paste are ignored so IME
    /// composition and bulk insertion remain entirely owned by egui.
    pub(super) fn apply_typing_assist(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        caret: usize,
    ) -> bool {
        let events = ctx.input(|input| input.events.clone());
        let typed = events.iter().rev().find_map(|event| match event {
            egui::Event::Text(text) if text.chars().count() == 1 => text.chars().next(),
            _ => None,
        });
        let entered = events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    ..
                }
            )
        });
        let idx = self.active_query_tab;

        let pairs = self.editor_options.auto_close_pairs;
        // Typing a closer that already sits after the caret steps over it instead of doubling
        // it: `(` gives `(|)`, and typing `)` then lands on `()|`, not `())|`. TextEdit has
        // already inserted the typed character, so the one to drop is right after it.
        if pairs
            && !entered
            && matches!(typed, Some(')' | ']' | '}' | '\'' | '"'))
            && self.tabs[idx].sql.chars().nth(caret) == typed
        {
            let byte = char_to_byte(&self.tabs[idx].sql, caret);
            let width = typed.map_or(1, char::len_utf8);
            self.tabs[idx].sql.replace_range(byte..byte + width, "");
            self.tabs[idx].mark_sql_changed();
            self.shift_folds(idx, caret, -1);
            self.workspace_dirty = true;
            self.select_editor_range(ctx, editor_id, caret..caret);
            return true;
        }
        let insertion = match typed {
            Some('(') if pairs => Some(")".to_string()),
            Some('[') if pairs => Some("]".to_string()),
            Some('{') if pairs => Some("}".to_string()),
            Some('\'') if pairs => Some("'".to_string()),
            Some('"') if pairs => Some("\"".to_string()),
            _ if entered => {
                let indent =
                    crate::editor_tools::indentation_after_newline(&self.tabs[idx].sql, caret);
                (!indent.is_empty()).then_some(indent)
            }
            _ => None,
        };
        let Some(insertion) = insertion else {
            return false;
        };

        // Do not double a closer that already follows the caret (common while editing inside
        // an existing pair). Indentation is allowed to repeat whitespace by design.
        if !entered
            && self.tabs[idx]
                .sql
                .chars()
                .nth(caret)
                .is_some_and(|next| insertion.starts_with(next))
        {
            return false;
        }
        let byte = char_to_byte(&self.tabs[idx].sql, caret);
        self.tabs[idx].sql.insert_str(byte, &insertion);
        self.tabs[idx].mark_sql_changed();
        self.shift_folds(idx, caret, insertion.chars().count() as isize);
        self.workspace_dirty = true;
        // For pairs the caret stays between the characters; after Enter it moves through the
        // inserted indentation.
        let target = if entered {
            caret + insertion.chars().count()
        } else {
            caret
        };
        self.select_editor_range(ctx, editor_id, target..target);
        true
    }

    pub(super) fn paint_matching_brackets(
        ui: &egui::Ui,
        sql: &str,
        caret: usize,
        view: &crate::fold::View,
        galley: &egui::Galley,
        galley_pos: egui::Pos2,
    ) {
        let Some((left, right)) = crate::editor_tools::matching_bracket(sql, caret) else {
            return;
        };
        for source in [left, right] {
            let (Some(start), Some(end)) = (view.to_display(source), view.to_display(source + 1))
            else {
                continue;
            };
            let offset = galley_pos.to_vec2();
            let from = galley
                .pos_from_cursor(egui::text::CCursor::new(start))
                .translate(offset);
            let to = galley
                .pos_from_cursor(egui::text::CCursor::new(end))
                .translate(offset);
            let rect = egui::Rect::from_min_max(
                egui::pos2(from.left(), from.top()),
                egui::pos2(to.left().max(from.left() + 3.0), from.bottom()),
            );
            ui.painter().rect_stroke(
                rect.shrink(0.5),
                egui::CornerRadius::same(2),
                egui::Stroke::new(1.0_f32, palette::ACCENT()),
                egui::StrokeKind::Inside,
            );
        }
    }

    /// Tint the lines of the statement under the caret — what Run Current would execute — so
    /// the boundary between statements in a long script is visible. Finding the statement
    /// scans the whole buffer, so its char span is cached per `(revision, caret)` and a
    /// repaint that moved neither reuses it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_current_statement(
        &mut self,
        idx: usize,
        under: &mut Vec<egui::Shape>,
        caret: usize,
        view: &crate::fold::View,
        galley: &egui::Galley,
        galley_pos: egui::Pos2,
        x_range: egui::Rangef,
    ) {
        let key = (self.tabs[idx].sql_revision, caret);
        let span = match &self.tabs[idx].editor_assist.statement_span {
            Some((cached, span)) if *cached == key => span.clone(),
            _ => {
                let span = current_statement_span(&self.tabs[idx].sql, caret);
                self.tabs[idx].editor_assist.statement_span = Some((key, span.clone()));
                span
            }
        };
        let Some(span) = span else {
            return;
        };
        let (Some(start), end) = (
            view.to_display(span.start),
            view.to_display_clamped(span.end),
        ) else {
            return;
        };
        let offset = galley_pos.to_vec2();
        let top = galley
            .pos_from_cursor(egui::text::CCursor::new(start))
            .translate(offset)
            .top();
        let bottom = galley
            .pos_from_cursor(egui::text::CCursor::new(end))
            .translate(offset)
            .bottom();
        under.push(egui::Shape::rect_filled(
            egui::Rect::from_x_y_ranges(x_range, top..=bottom),
            egui::CornerRadius::same(3),
            palette::ACCENT().gamma_multiply(0.07),
        ));
    }

    /// Draw whitespace the way code editors show it: `·` for a space, `→` for a tab. Only the
    /// rows on screen are walked, so a long script costs nothing extra off-screen.
    pub(super) fn paint_invisibles(
        ui: &egui::Ui,
        galley: &egui::Galley,
        galley_pos: egui::Pos2,
        font: &egui::FontId,
    ) {
        let clip = ui.clip_rect();
        let color = palette::TEXT_FAINT().gamma_multiply(0.7);
        let painter = ui.painter();
        for placed in &galley.rows {
            let row = placed.rect().translate(galley_pos.to_vec2());
            if row.bottom() < clip.top() {
                continue;
            }
            if row.top() > clip.bottom() {
                break;
            }
            for glyph in &placed.row.glyphs {
                // Glyph positions are relative to the row's origin, not its bounding rect.
                let x = galley_pos.x + placed.pos.x + glyph.pos.x + glyph.advance_width / 2.0;
                match glyph.chr {
                    ' ' => {
                        painter.circle_filled(egui::pos2(x, row.center().y), 1.1, color);
                    }
                    '\t' => {
                        painter.text(
                            egui::pos2(x, row.center().y),
                            egui::Align2::CENTER_CENTER,
                            "→",
                            font.clone(),
                            color,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    /// Tooltip for the table/column name under the pointer: type, key role, nullability,
    /// default, comment and foreign keys from the connected schema.
    ///
    /// Registered *before* [`Self::update_diagnostics`] so that when a name is also underlined
    /// as an error, the error's tooltip (registered later, so on top) is the one shown.
    pub(super) fn show_symbol_hover(
        &mut self,
        ui: &egui::Ui,
        galley: &egui::Galley,
        galley_pos: egui::Pos2,
        view: &crate::fold::View,
    ) {
        let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let text_rect = galley.rect.translate(galley_pos.to_vec2());
        if !text_rect.contains(pointer) {
            return;
        }
        let idx = self.active_query_tab;
        let Some(conn) = self.tabs[idx]
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
        else {
            return;
        };
        let display = galley.cursor_from_pos(pointer - galley_pos).index;
        let source = view.to_source(display);
        let Some(hover) = crate::hover::describe_at(&self.tabs[idx].sql, source, &conn.schema)
        else {
            return;
        };
        let (Some(start), Some(end)) = (
            view.to_display(hover.range.start),
            view.to_display(hover.range.end),
        ) else {
            return;
        };
        let offset = galley_pos.to_vec2();
        let from = galley
            .pos_from_cursor(egui::text::CCursor::new(start))
            .translate(offset);
        let to = galley
            .pos_from_cursor(egui::text::CCursor::new(end))
            .translate(offset);
        // A name wrapped over two rows would make a nonsense rect; hover only single-row names.
        if (from.top() - to.top()).abs() > 1.0 {
            return;
        }
        let rect = egui::Rect::from_min_max(
            egui::pos2(from.left(), from.top()),
            egui::pos2(to.left().max(from.left() + 2.0), from.bottom()),
        );
        ui.interact(rect, ui.id().with("sql_symbol_hover"), egui::Sense::hover())
            .on_hover_ui(|ui| {
                ui.set_max_width(360.0);
                ui.label(
                    egui::RichText::new(&hover.title)
                        .strong()
                        .color(palette::TEXT()),
                );
                for line in &hover.lines {
                    ui.label(
                        egui::RichText::new(line)
                            .monospace()
                            .size(crate::style::font::CAPTION)
                            .color(palette::TEXT_WEAK()),
                    );
                }
            });
    }

    /// Re-check the editor's SQL and mark what's wrong: the first syntax error, or — once the
    /// SQL parses — every table/column the connected schema doesn't have. Each is a red squiggle
    /// under the token, explained in a tooltip on hover.
    ///
    /// Two rules keep it from nagging while the query is still being written. The parse runs
    /// only after a short pause in typing (every half-typed keyword is a "syntax error", and
    /// a squiggle that flickers on every keystroke is noise), and an error covering the caret
    /// is never drawn — that token is the one the user is in the middle of typing.
    pub(super) fn update_diagnostics(
        &mut self,
        ui: &egui::Ui,
        galley: &egui::Galley,
        galley_pos: egui::Pos2,
        text_changed: bool,
        cursor_char: Option<usize>,
        view: &crate::fold::View,
    ) {
        /// Pause in typing before the buffer is re-parsed.
        const DEBOUNCE: f64 = 0.35;
        /// Height and half-period of the squiggle, in points.
        const WAVE: f32 = 1.7;

        let idx = self.active_query_tab;
        let now = ui.input(|i| i.time);
        // The dialect of the connection this tab runs on — not whichever connection happens
        // to be selected elsewhere in the app.
        let kind = self.tabs[idx]
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
            .map(|c| c.db.kind());
        // A schema refresh (or a first load) can change what counts as an unknown name, so it
        // re-checks unchanged text just as a typed character does.
        let schema_stamp = self.tabs[idx]
            .conn_id
            .as_deref()
            .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
            .map_or((0, 0, 0), |c| {
                (
                    c.schema.tables.len(),
                    c.schema.views.len(),
                    c.schema
                        .tables
                        .iter()
                        .map(|t| t.columns.len())
                        .sum::<usize>(),
                )
            });
        let stale = self.tabs[idx].sql != self.tabs[idx].editor_assist.syntax_checked
            || kind != self.tabs[idx].editor_assist.syntax_checked_kind
            || schema_stamp != self.tabs[idx].editor_assist.schema_stamp;
        if text_changed || (stale && self.tabs[idx].editor_assist.syntax_dirty_at.is_none()) {
            self.tabs[idx].editor_assist.syntax_dirty_at = Some(now);
        }
        if let Some(since) = self.tabs[idx].editor_assist.syntax_dirty_at {
            let waited = now - since;
            if waited >= DEBOUNCE {
                let sql = self.tabs[idx].sql.clone();
                let syntax_error = dbcore::check_syntax(kind, &sql);
                // Unknown names only matter once the SQL parses; until then the syntax
                // error is the one thing worth saying.
                let semantic_issues = if syntax_error.is_none() {
                    self.tabs[idx]
                        .conn_id
                        .as_deref()
                        .and_then(|id| self.active_connections.iter().find(|c| c.config_id == id))
                        .map(|c| dbcore::check_semantics(kind, &sql, &c.schema))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let assist = &mut self.tabs[idx].editor_assist;
                assist.syntax_error = syntax_error;
                assist.semantic_issues = semantic_issues;
                assist.syntax_checked = sql;
                assist.syntax_checked_kind = kind;
                assist.schema_stamp = schema_stamp;
                assist.syntax_dirty_at = None;
            } else {
                // Nothing else would repaint an idle editor, so ask for the frame that runs
                // the check once the pause is long enough.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(DEBOUNCE - waited));
                return;
            }
        }

        // The check ran against this exact text (the debounce above guarantees it), so the
        // ranges still index the buffer on screen.
        let assist = &self.tabs[idx].editor_assist;
        let marks: Vec<(std::ops::Range<usize>, &str, &str)> = assist
            .syntax_error
            .iter()
            .map(|e| (e.range.clone(), "Syntax error", e.message.as_str()))
            .chain(assist.semantic_issues.iter().map(|issue| {
                let title = match issue.kind {
                    dbcore::semantic::IssueKind::Table => "Unknown table",
                    dbcore::semantic::IssueKind::Column => "Unknown column",
                };
                (issue.range.clone(), title, issue.message.as_str())
            }))
            .filter(|(range, _, _)| !error_under_caret(range, cursor_char))
            .collect();
        let sql = &self.tabs[idx].sql;

        for (n, (range, title, message)) in marks.into_iter().enumerate() {
            // Keep the mark on one row: a range that runs past a newline (an unterminated
            // string swallowing the rest of the query) is clipped to the line it starts on.
            let line_end = sql
                .chars()
                .skip(range.start)
                .position(|c| c == '\n')
                .map_or(usize::MAX, |n| range.start + n);
            let end = range.end.min(line_end).max(range.start.saturating_add(1));
            // Nothing to mark when the offending token is inside a collapsed region: the
            // galley has no position for text it isn't showing.
            let (Some(start), Some(end)) = (view.to_display(range.start), view.to_display(end))
            else {
                continue;
            };
            let offset = galley_pos.to_vec2();
            let from = galley
                .pos_from_cursor(egui::text::CCursor::new(start))
                .translate(offset);
            let to = galley
                .pos_from_cursor(egui::text::CCursor::new(end))
                .translate(offset);
            let x0 = from.left();
            let x1 = to.left().max(x0 + 2.0 * WAVE);
            let y = from.bottom() - WAVE;

            // A hand-drawn wave rather than a straight rule: it reads as "this is wrong"
            // without competing with the caret or the selection, exactly like every code
            // editor's.
            let mut points = Vec::new();
            let mut x = x0;
            let mut down = false;
            while x < x1 {
                points.push(egui::pos2(x, if down { y + WAVE } else { y }));
                x += WAVE;
                down = !down;
            }
            points.push(egui::pos2(x1, if down { y + WAVE } else { y }));
            ui.painter().add(egui::Shape::line(
                points,
                egui::Stroke::new(1.0_f32, palette::DANGER()),
            ));

            // Hover the *token*, not just the two pixels of squiggle under it. Registered
            // after the editor so it wins the hover, and hover-only so clicks and drags
            // still reach the text beneath.
            let hover = egui::Rect::from_min_max(
                egui::pos2(x0, from.top()),
                egui::pos2(x1, y + WAVE + 1.0),
            );
            ui.interact(
                hover,
                ui.id().with(("sql_diagnostic", n)),
                egui::Sense::hover(),
            )
            .on_hover_ui(|ui| {
                ui.set_max_width(340.0);
                ui.label(
                    egui::RichText::new(title)
                        .size(crate::style::font::CAPTION)
                        .color(palette::DANGER()),
                );
                ui.label(message);
            });
        }
    }

    /// Insert an accepted ghost suggestion at the source caret and move the caret past it.
    fn accept_ghost(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        cursor_char: usize,
        remainder: &str,
    ) {
        let tab = &mut self.tabs[self.active_query_tab];
        let byte_cursor = char_to_byte(&tab.sql, cursor_char);
        if byte_cursor > tab.sql.len() {
            return;
        }
        tab.sql.insert_str(byte_cursor, remainder);
        tab.mark_sql_changed();
        tab.edits.source = None;
        tab.preview = false;
        self.workspace_dirty = true;
        self.shift_folds(
            self.active_query_tab,
            cursor_char,
            remainder.chars().count() as isize,
        );

        let new_cursor = self.editor_caret(cursor_char + remainder.chars().count());
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(new_cursor),
                )));
            state.store(ctx, editor_id);
        }
        ctx.memory_mut(|m| m.request_focus(editor_id));
    }

    /// Toggle `-- ` line comments over the selected lines (Cmd/Ctrl+/), VS Code style, then
    /// restore a caret/selection over the same text. The buffer rewrite itself lives in the
    /// pure [`toggle_comment_edit`] so it can be unit-tested without an egui context.
    ///
    /// `chars` is a sorted char range into the tab's real SQL (the caller maps it out of the
    /// folded view first).
    pub(super) fn toggle_line_comment(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        chars: std::ops::Range<usize>,
    ) {
        let idx = self.active_query_tab;
        let Some((byte_range, out)) = toggle_comment_edit(&self.tabs[idx].sql, chars.clone())
        else {
            return;
        };

        // Restore a caret/selection over the same text. For a bare caret, keep its distance
        // from the end of its line (markers land near the start, so this tracks the caret
        // through the shift); for a real selection, re-cover the rewritten lines.
        let sql = &self.tabs[idx].sql;
        let first_line_start_char = sql[..byte_range.start].chars().count();
        let old_chars = sql[byte_range.clone()].chars().count();
        let new_chars = out.chars().count();
        let new_range = if chars.start == chars.end {
            let tail = old_chars.saturating_sub(chars.start - first_line_start_char);
            let offset = new_chars.saturating_sub(tail);
            [first_line_start_char + offset; 2]
        } else {
            [first_line_start_char, first_line_start_char + new_chars]
        };

        let tab = &mut self.tabs[idx];
        tab.sql.replace_range(byte_range, &out);
        tab.mark_sql_changed();
        tab.edits.source = None;
        tab.preview = false;
        self.workspace_dirty = true;
        self.shift_folds(
            idx,
            first_line_start_char,
            new_chars as isize - old_chars as isize,
        );

        let [from, to] = new_range.map(|at| self.editor_caret(at));
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(from),
                    egui::text::CCursor::new(to),
                )));
            state.store(ctx, editor_id);
        }
        ctx.memory_mut(|m| m.request_focus(editor_id));
    }

    /// Where the editor's caret must go to sit at char index `at` of the active tab's SQL.
    /// The two differ only while something is folded, when the visible text is shorter than
    /// the query it stands for.
    fn editor_caret(&self, at: usize) -> usize {
        let tab = &self.tabs[self.active_query_tab];
        if tab.folds.is_empty() {
            return at;
        }
        let regions = crate::fold::regions(&tab.sql);
        crate::fold::View::build(&tab.sql, &regions, &tab.folds).to_display_clamped(at)
    }

    /// Replay an edit made outside the editor — a completion, a comment toggle — onto the
    /// tab's fold anchors, so collapsed regions keep covering the same lines.
    fn shift_folds(&mut self, idx: usize, at: usize, delta: isize) {
        if delta == 0 || self.tabs[idx].folds.is_empty() {
            return;
        }
        self.tabs[idx].folds = self.tabs[idx]
            .folds
            .iter()
            .map(|anchor| {
                if *anchor > at {
                    anchor.saturating_add_signed(delta)
                } else {
                    *anchor
                }
            })
            .collect();
    }

    /// Drive the SQL editor's autocomplete popup for one frame: recompute suggestions from
    /// the caret position, apply navigation/accept, and draw the popup. Called from
    /// [`Self::query_console`] right after the editor renders.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_autocomplete(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        focused: bool,
        text_changed: bool,
        force: bool,
        cursor_char: Option<usize>,
        cursor_rect: Option<egui::Rect>,
        nav: crate::autocomplete::NavKeys,
    ) {
        let tab_idx = self.active_query_tab;
        // Esc dismisses without touching the text; the editor never saw the keystroke.
        if nav.dismiss {
            self.tabs[tab_idx].editor_assist.autocomplete.open = false;
            return;
        }

        // While the editor is focused it reports a live caret; cache it so a click on the
        // popup — which strips the editor's focus that same frame, nulling the caret — can
        // still recompute and resolve the insertion point.
        if let (Some(cc), Some(cr)) = (cursor_char, cursor_rect) {
            self.tabs[tab_idx].editor_assist.autocomplete.caret_char = cc;
            self.tabs[tab_idx].editor_assist.autocomplete.anchor = cr;
        }

        // Open while actively typing (or on a forced trigger); a caret that merely sits in a
        // word — e.g. after a click — shouldn't pop the menu back up on its own.
        let typing = text_changed || force;
        // Only scan the schema when there's a reason to: the user is typing/forcing, or the
        // popup is already open and following the prefix. Recomputing every focused frame meant
        // re-scanning the whole schema even while idle — e.g. when the grid scrolls and the
        // still-focused editor keeps repainting — which made scrolling janky on big schemas.
        // Matching runs over every table and column in the schema, so an open popup recomputes
        // only when the text or the caret moved — never on a repaint that changed neither
        // (the grid scrolling, a spinner, the caret blinking).
        let key = (
            self.tabs[tab_idx].sql_revision,
            self.tabs[tab_idx].editor_assist.autocomplete.caret_char,
        );
        let stale = self.tabs[tab_idx].editor_assist.autocomplete.computed_for != Some(key);
        if focused && (typing || (self.tabs[tab_idx].editor_assist.autocomplete.open && stale)) {
            self.tabs[tab_idx].editor_assist.autocomplete.computed_for = Some(key);
            // Recompute against the live text. Borrow the connection's schema and the tab's
            // SQL immutably together, then hand ownership back so the borrows end.
            let completion = {
                let (schema, kind) = match self.active() {
                    Some(c) => (Some(&c.schema), Some(c.db.kind())),
                    None => (None, None),
                };
                let sql = &self.tabs[tab_idx].sql;
                crate::autocomplete::complete_with(
                    sql,
                    self.tabs[tab_idx].editor_assist.autocomplete.caret_char,
                    schema,
                    kind,
                    force,
                    &self.editor_options,
                )
            };

            if !force {
                self.tabs[tab_idx].editor_assist.autocomplete.inline_hint = Some((
                    key,
                    completion
                        .as_ref()
                        .and_then(crate::autocomplete::inline_suffix),
                ));
            }
            match completion {
                Some(c) => {
                    // A single append-only match reads more naturally as inline ghost text;
                    // Ctrl+Space still force-opens the popup when the user explicitly asks.
                    if !force && crate::autocomplete::inline_suffix(&c).is_some() {
                        self.tabs[tab_idx].editor_assist.autocomplete.open = false;
                        return;
                    }
                    self.tabs[tab_idx].editor_assist.autocomplete.items = c.items;
                    self.tabs[tab_idx].editor_assist.autocomplete.replace_start = c.replace_start;
                    self.tabs[tab_idx].editor_assist.autocomplete.prefix = c.prefix;
                    self.tabs[tab_idx].editor_assist.autocomplete.open = true;
                    self.tabs[tab_idx].editor_assist.autocomplete.selected = self.tabs[tab_idx]
                        .editor_assist
                        .autocomplete
                        .selected
                        .min(self.tabs[tab_idx].editor_assist.autocomplete.items.len() - 1);
                }
                None => {
                    self.tabs[tab_idx].editor_assist.autocomplete.open = false;
                }
            }
        }
        // When not focused we leave `open`/`items` as they were: the popup keeps showing so a
        // click in progress can land on a row. The click-outside check below closes it.

        if !self.tabs[tab_idx].editor_assist.autocomplete.open {
            return;
        }

        // Apply list navigation consumed before the editor rendered.
        let len = self.tabs[tab_idx].editor_assist.autocomplete.items.len();
        if nav.down {
            self.tabs[tab_idx].editor_assist.autocomplete.selected =
                (self.tabs[tab_idx].editor_assist.autocomplete.selected + 1) % len;
        }
        if nav.up {
            self.tabs[tab_idx].editor_assist.autocomplete.selected =
                (self.tabs[tab_idx].editor_assist.autocomplete.selected + len - 1) % len;
        }

        let anchor = self.tabs[tab_idx].editor_assist.autocomplete.anchor;
        let (event, popup_rect) = crate::autocomplete::show_popup(
            ctx,
            &self.tabs[tab_idx].editor_assist.autocomplete,
            anchor,
            nav.up || nav.down,
        );

        let accept = if nav.accept {
            Some(self.tabs[tab_idx].editor_assist.autocomplete.selected)
        } else if let crate::autocomplete::Event::Accept(i) = event {
            Some(i)
        } else {
            None
        };
        if let Some(idx) = accept {
            self.accept_completion(
                ctx,
                editor_id,
                idx,
                self.tabs[tab_idx].editor_assist.autocomplete.caret_char,
            );
            return;
        }

        // The editor lost focus without a row being chosen. Keep the popup only while the
        // pointer is pressing inside it (the press half of a click on a row, before the
        // release that fires `clicked()`); any other focus loss — a click elsewhere, Tab
        // away — dismisses it.
        if !focused {
            let keep = ctx.input(|i| {
                (i.pointer.any_down() || i.pointer.any_pressed())
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| popup_rect.contains(p))
            });
            if !keep {
                self.tabs[tab_idx].editor_assist.autocomplete.open = false;
            }
        }
    }

    /// Insert suggestion `idx` over the prefix at the caret, then move the caret past it.
    fn accept_completion(
        &mut self,
        ctx: &egui::Context,
        editor_id: egui::Id,
        idx: usize,
        cursor_char: usize,
    ) {
        let tab_idx = self.active_query_tab;
        let start = self.tabs[tab_idx].editor_assist.autocomplete.replace_start;
        let after_dot = start > 0 && self.tabs[tab_idx].sql.chars().nth(start - 1) == Some('.');
        let kind = self.active().map(|c| c.db.kind());
        let Some(mut suggestion) = self.tabs[tab_idx]
            .editor_assist
            .autocomplete
            .items
            .get(idx)
            .map(|s| crate::autocomplete::insertion_text(s, after_dot, kind, &self.editor_options))
        else {
            return;
        };
        // "Add a space after completing": only where the caret ends the line, so a name
        // completed in the middle of existing text never pushes it apart.
        if self.editor_options.add_space_after_completion
            && self.tabs[tab_idx]
                .sql
                .chars()
                .skip(cursor_char)
                .take_while(|&c| c != '\n')
                .all(char::is_whitespace)
        {
            suggestion.push(' ');
        }
        let tab = &mut self.tabs[tab_idx];
        let byte_start = char_to_byte(&tab.sql, start);
        let byte_cursor = char_to_byte(&tab.sql, cursor_char);
        // Guard against indices that went stale if the text shifted under us this frame.
        if byte_start > byte_cursor || byte_cursor > tab.sql.len() {
            self.tabs[tab_idx].editor_assist.autocomplete.open = false;
            return;
        }
        tab.sql.replace_range(byte_start..byte_cursor, &suggestion);
        tab.mark_sql_changed();
        tab.edits.source = None;
        tab.preview = false;
        self.workspace_dirty = true;
        self.shift_folds(
            tab_idx,
            start,
            suggestion.chars().count() as isize - (cursor_char - start) as isize,
        );

        // Move the editor's caret to just after the inserted text.
        let new_cursor = self.editor_caret(start + suggestion.chars().count());
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor_id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(new_cursor),
                )));
            state.store(ctx, editor_id);
        }
        ctx.memory_mut(|m| m.request_focus(editor_id));
        self.tabs[tab_idx].editor_assist.autocomplete.open = false;
    }
}
