//! Code folding for the SQL editor: which parts of a script can collapse, and the machinery
//! that lets egui's `TextEdit` show a collapsed version of a buffer it is still editing.
//!
//! egui always renders the whole buffer, so folding is done by handing the editor a *folded
//! view* — the real SQL with each collapsed region swapped for a short `⋯ N lines` marker —
//! through a [`Buffer`] that maps every insertion and deletion back onto the real string.
//! With nothing folded the view is the text verbatim and every mapping is the identity, so
//! the editor behaves exactly as it did before folding existed.
//!
//! What can fold is decided by [`regions`], a single lexical pass that understands the parts
//! of SQL that actually nest: whole statements, bracketed groups (subqueries, column lists,
//! `VALUES`), `BEGIN`/`CASE` … `END` blocks, and runs of comment lines. One region per line
//! — the widest one, matching how editors offer a single chevron per line.

use std::collections::BTreeSet;
use std::ops::Range;

/// The construct a fold covers. Kept alongside each region for the editor's tooling (and to
/// keep the detection rules readable); the collapsed marker looks the same for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A multi-line statement, delimited by the `;` separators outside strings and comments.
    Statement,
    /// A bracketed group: subquery, column list, `VALUES` tuple, function argument list.
    Paren,
    /// A `BEGIN … END` or `CASE … END` block.
    Block,
    /// A run of `--` comment lines, or one `/* … */` spanning several lines.
    Comment,
}

/// One foldable range of the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Char index of the start of the header line — the line that keeps its chevron and stays
    /// visible when the region collapses. Doubles as the region's stable identity: the app
    /// stores folded regions by this offset and shifts it along with edits.
    pub anchor: usize,
    /// 0-based source line of the header.
    pub header_line: usize,
    /// Char range hidden while collapsed: from the end of the header line to the end of the
    /// last covered line, so the newline that ends the header is swallowed by the marker.
    pub hide: Range<usize>,
    /// How many lines disappear when collapsed.
    pub lines: usize,
    pub kind: Kind,
}

/// Every foldable region of `sql`, sorted by position, at most one per line.
pub fn regions(sql: &str) -> Vec<Region> {
    let chars: Vec<char> = sql.chars().collect();
    let lines = line_starts(&chars);
    let mut out = Vec::new();
    scan(&chars, &lines, &mut out);

    // A line can carry only one chevron. Where several regions start on the same line — a
    // statement whose first line also opens a subquery, say — offer the widest, which is
    // what editors do: the outer fold is the one the reader means.
    out.sort_by_key(|r| (r.anchor, std::cmp::Reverse(r.hide.end)));
    out.dedup_by_key(|r| r.anchor);
    out
}

/// Lexical pass collecting statement, bracket, block and comment regions in one walk. Strings
/// and quoted identifiers are skipped wholesale so a `;` or `(` inside them never counts.
fn scan(chars: &[char], lines: &[usize], out: &mut Vec<Region>) {
    let n = chars.len();
    let mut i = 0;
    // Open brackets and blocks, innermost last.
    let mut stack: Vec<(usize, Kind)> = Vec::new();
    // Start of the statement being scanned, and its first non-blank char (the header).
    let mut stmt_head: Option<usize> = None;
    let mut last_solid = 0usize;
    // Lines that hold nothing but a `--` comment, for grouping runs afterwards.
    let mut comment_lines: Vec<usize> = Vec::new();

    while i < n {
        let c = chars[i];
        let solid = !c.is_whitespace();
        if solid {
            last_solid = i;
        }
        match c {
            '\'' => {
                stmt_head.get_or_insert(i);
                i = skip_string(chars, i);
                last_solid = i.saturating_sub(1);
                continue;
            }
            '"' | '`' | '[' => {
                stmt_head.get_or_insert(i);
                let close = match c {
                    '"' => '"',
                    '`' => '`',
                    _ => ']',
                };
                i = skip_delimited(chars, i, close);
                last_solid = i.saturating_sub(1);
                continue;
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                let line = line_of(lines, i);
                if is_blank_before(chars, lines[line], i) {
                    comment_lines.push(line);
                }
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let end = skip_block_comment(chars, i);
                push(
                    out,
                    region(chars, lines, i, end.saturating_sub(1), Kind::Comment, false),
                );
                last_solid = end.saturating_sub(1);
                i = end;
                continue;
            }
            '(' => {
                stmt_head.get_or_insert(i);
                stack.push((i, Kind::Paren));
            }
            ')' => {
                // Pop past any unclosed block inside the brackets rather than pairing this
                // `)` with a `BEGIN` that never ended.
                if let Some(pos) = stack.iter().rposition(|(_, k)| *k == Kind::Paren) {
                    let (open, kind) = stack[pos];
                    stack.truncate(pos);
                    push(out, region(chars, lines, open, i, kind, true));
                }
            }
            ';' => {
                if let Some(head) = stmt_head.take() {
                    push(out, region(chars, lines, head, i, Kind::Statement, false));
                }
                stack.clear();
            }
            c if is_word_start(c) => {
                let start = i;
                while i < n && is_word_char(chars[i]) {
                    i += 1;
                }
                last_solid = i - 1;
                stmt_head.get_or_insert(start);
                let word = upper(&chars[start..i]);
                if word == "CASE" || (word == "BEGIN" && !opens_transaction(chars, i)) {
                    stack.push((start, Kind::Block));
                } else if word == "END" {
                    if let Some(pos) = stack.iter().rposition(|(_, k)| *k == Kind::Block) {
                        let (open, kind) = stack[pos];
                        stack.truncate(pos);
                        push(out, region(chars, lines, open, start, kind, true));
                    }
                }
                continue;
            }
            _ => {
                if solid {
                    stmt_head.get_or_insert(i);
                }
            }
        }
        i += 1;
    }

    // A script whose last statement has no trailing `;` still folds.
    if let Some(head) = stmt_head {
        push(
            out,
            region(chars, lines, head, last_solid, Kind::Statement, false),
        );
    }

    // Consecutive comment-only lines fold as one block, from the first line of the run.
    comment_lines.dedup();
    let mut run = 0;
    while run < comment_lines.len() {
        let mut end = run;
        while end + 1 < comment_lines.len() && comment_lines[end + 1] == comment_lines[end] + 1 {
            end += 1;
        }
        if end > run {
            push(
                out,
                region(
                    chars,
                    lines,
                    lines[comment_lines[run]],
                    lines[comment_lines[end]],
                    Kind::Comment,
                    false,
                ),
            );
        }
        run = end + 1;
    }
}

fn push(out: &mut Vec<Region>, region: Option<Region>) {
    out.extend(region);
}

/// Build the region between the header at `head` and the closing token at `end`, or `None`
/// when it would hide nothing. `dedent_close` keeps a closing token that starts its own line
/// (`)` or `END`) visible, so a collapsed block still shows how it finishes.
fn region(
    chars: &[char],
    lines: &[usize],
    head: usize,
    end: usize,
    kind: Kind,
    dedent_close: bool,
) -> Option<Region> {
    let header_line = line_of(lines, head);
    let mut end_line = line_of(lines, end);
    if dedent_close && end_line > header_line && is_blank_before(chars, lines[end_line], end) {
        end_line -= 1;
    }
    if end_line <= header_line {
        return None;
    }
    let hide = line_end(chars, lines, header_line)..line_end(chars, lines, end_line);
    Some(Region {
        anchor: lines[header_line],
        header_line,
        lines: end_line - header_line,
        hide,
        kind,
    })
}

/// `BEGIN TRANSACTION` / `BEGIN WORK` open a transaction, not a block, so they must not eat
/// the `END` that closes a later `CASE`.
fn opens_transaction(chars: &[char], after_begin: usize) -> bool {
    let n = chars.len();
    let mut i = after_begin;
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    let start = i;
    while i < n && is_word_char(chars[i]) {
        i += 1;
    }
    matches!(
        upper(&chars[start..i]).as_str(),
        "TRANSACTION" | "TRAN" | "WORK" | ""
    )
}

// --- text helpers ---------------------------------------------------------------------

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn upper(chars: &[char]) -> String {
    chars.iter().collect::<String>().to_ascii_uppercase()
}

fn line_starts(chars: &[char]) -> Vec<usize> {
    let mut out = vec![0];
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            out.push(i + 1);
        }
    }
    out
}

fn line_of(lines: &[usize], idx: usize) -> usize {
    match lines.binary_search(&idx) {
        Ok(line) => line,
        Err(next) => next - 1,
    }
}

/// Char index of the end of `line`, before its newline (and before a `\r\n` carriage return,
/// so a Windows script does not fold a stray `\r` into the marker).
fn line_end(chars: &[char], lines: &[usize], line: usize) -> usize {
    let end = if line + 1 < lines.len() {
        lines[line + 1] - 1
    } else {
        chars.len()
    };
    if end > 0 && chars.get(end - 1) == Some(&'\r') {
        end - 1
    } else {
        end
    }
}

/// Whether everything from `from` up to `idx` is whitespace.
fn is_blank_before(chars: &[char], from: usize, idx: usize) -> bool {
    chars[from..idx].iter().all(|c| c.is_whitespace())
}

fn skip_string(chars: &[char], mut i: usize) -> usize {
    let n = chars.len();
    i += 1;
    while i < n {
        if chars[i] == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    n
}

fn skip_delimited(chars: &[char], mut i: usize, close: char) -> usize {
    let n = chars.len();
    i += 1;
    while i < n && chars[i] != close {
        i += 1;
    }
    (i + 1).min(n)
}

fn skip_block_comment(chars: &[char], mut i: usize) -> usize {
    let n = chars.len();
    i += 2;
    while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
        i += 1;
    }
    (i + 2).min(n)
}

// --- the folded view ------------------------------------------------------------------

/// A collapsed span in the display text: `hidden` source chars shown as a `marker`-long
/// placeholder. Positions are char indices in their respective strings.
#[derive(Debug, Clone, Copy)]
struct Gap {
    display: usize,
    source: usize,
    hidden: usize,
    marker: usize,
}

/// The text the editor shows while some regions are collapsed, plus the mapping back to the
/// real SQL. Rebuilt every frame from the tab's text and its set of folded anchors, so it can
/// never drift out of sync with either.
#[derive(Debug, Default, Clone)]
pub struct View {
    /// What the editor renders and edits.
    pub text: String,
    /// For each display line, the 0-based source line it stands for. Drives the gutter.
    pub source_lines: Vec<usize>,
    /// For each display line, the char index it starts at in [`Self::text`], so the gutter can
    /// ask the galley where that line ended up on screen.
    pub line_starts: Vec<usize>,
    gaps: Vec<Gap>,
}

/// Placeholder for a collapsed region. The `⋯` reads as "more here" at any font size, and the
/// line count says how much is hidden without opening it.
fn marker_text(lines: usize) -> String {
    if lines == 1 {
        " ⋯ 1 line ".to_string()
    } else {
        format!(" ⋯ {lines} lines ")
    }
}

impl View {
    /// Collapse every region of `sql` whose anchor is in `folded`.
    pub fn build(sql: &str, regions: &[Region], folded: &BTreeSet<usize>) -> Self {
        if folded.is_empty() {
            return Self::whole(sql);
        }
        let chars: Vec<char> = sql.chars().collect();
        let mut applied: Vec<&Region> = regions
            .iter()
            .filter(|r| folded.contains(&r.anchor))
            .collect();
        applied.sort_by_key(|r| r.hide.start);

        let mut text = String::new();
        let mut gaps: Vec<Gap> = Vec::new();
        let mut cursor = 0usize;
        let mut display = 0usize;
        for region in applied {
            // A region nested inside one that is already collapsed is not shown at all.
            if region.hide.start < cursor {
                continue;
            }
            text.extend(&chars[cursor..region.hide.start]);
            display += region.hide.start - cursor;
            let marker = marker_text(region.lines);
            let marker_len = marker.chars().count();
            text.push_str(&marker);
            gaps.push(Gap {
                display,
                source: region.hide.start,
                hidden: region.hide.end - region.hide.start,
                marker: marker_len,
            });
            display += marker_len;
            cursor = region.hide.end;
        }
        text.extend(&chars[cursor..]);

        let (source_lines, line_starts) = display_lines(&chars, &gaps, &text);
        Self {
            text,
            source_lines,
            line_starts,
            gaps,
        }
    }

    /// The unfolded view: what the editor shows when nothing is collapsed.
    pub fn whole(sql: &str) -> Self {
        let chars: Vec<char> = sql.chars().collect();
        let (source_lines, line_starts) = display_lines(&chars, &[], sql);
        Self {
            text: sql.to_string(),
            source_lines,
            line_starts,
            gaps: Vec::new(),
        }
    }

    /// Display char ranges of the collapsed markers, for painting them as placeholders
    /// rather than as SQL.
    pub fn markers(&self) -> Vec<Range<usize>> {
        self.gaps
            .iter()
            .map(|g| g.display..g.display + g.marker)
            .collect()
    }

    /// The source offset a marker stands for, when `display` lands inside one. Lets a click on
    /// the `⋯ N lines` stand-in open the region it hides, the way clicking one does anywhere
    /// else — the caller matches this against [`Region::hide`] to find the region.
    pub fn marker_hiding(&self, display: usize) -> Option<usize> {
        self.gaps
            .iter()
            .find(|g| display > g.display && display < g.display + g.marker)
            .map(|g| g.source)
    }

    /// Source index for a display index. A position inside a marker resolves to the start of
    /// what it hides, so text typed there lands before the collapsed block.
    pub fn to_source(&self, display: usize) -> usize {
        to_source(&self.gaps, display, false)
    }

    /// Like [`Self::to_source`], but a position inside a marker resolves past the hidden
    /// text — the right end for a deletion, which then takes the collapsed block with it.
    pub fn to_source_end(&self, display: usize) -> usize {
        to_source(&self.gaps, display, true)
    }

    /// Display index for a source index, or `None` when that text is currently hidden.
    pub fn to_display(&self, source: usize) -> Option<usize> {
        to_display(&self.gaps, source)
    }

    /// Display index for a source index, falling back to the marker that hides it.
    pub fn to_display_clamped(&self, source: usize) -> usize {
        to_display(&self.gaps, source).unwrap_or_else(|| {
            self.gaps
                .iter()
                .find(|g| source < g.source + g.hidden)
                .map_or(source, |g| g.display)
        })
    }
}

/// For every display line: the source line it shows, and the display char index it starts at.
fn display_lines(chars: &[char], gaps: &[Gap], text: &str) -> (Vec<usize>, Vec<usize>) {
    let source_starts = line_starts(chars);
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut display = 0usize;
    for line in text.split('\n') {
        lines.push(line_of(&source_starts, to_source(gaps, display, false)));
        starts.push(display);
        display += line.chars().count() + 1;
    }
    (lines, starts)
}

fn to_source(gaps: &[Gap], display: usize, past_hidden: bool) -> usize {
    let mut delta = 0usize;
    for gap in gaps {
        if display < gap.display {
            break;
        }
        if display < gap.display + gap.marker {
            return if past_hidden {
                gap.source + gap.hidden
            } else {
                gap.source
            };
        }
        delta += gap.hidden - gap.marker;
    }
    display + delta
}

fn to_display(gaps: &[Gap], source: usize) -> Option<usize> {
    let mut delta = 0usize;
    for gap in gaps {
        if source < gap.source {
            break;
        }
        if source < gap.source + gap.hidden {
            return None;
        }
        delta += gap.hidden - gap.marker;
    }
    Some(source - delta)
}

// --- the editable buffer --------------------------------------------------------------

/// What the editor did to the buffer this frame, in source coordinates: the char index the
/// edit started at and how many chars the text grew (or shrank) by. The app replays these
/// onto its fold anchors so collapsed regions keep covering the same text.
pub type Shift = (usize, isize);

/// An [`egui::TextBuffer`] that presents the folded [`View`] while editing the real SQL
/// underneath.
///
/// Insertions and deletions are translated through the gaps: typing lands in the source at
/// the mapped offset, and a deletion that swallows a marker takes the whole collapsed block
/// with it — the same bargain every editor makes.
///
/// Undo and redo are the one thing that cannot be translated: egui's undoer replays whole
/// snapshots of the *displayed* string, which carry markers instead of the text they hide.
/// Those are refused and reported through [`Self::finish`], so the app can open every fold
/// and let the next undo run against the real text.
pub struct Buffer<'a> {
    source: &'a mut String,
    display: String,
    gaps: Vec<Gap>,
    shifts: Vec<Shift>,
    refused_bulk: bool,
}

impl<'a> Buffer<'a> {
    /// Wrap `source`, showing `view` (which must have been built from that same text).
    pub fn new(source: &'a mut String, view: &View) -> Self {
        Self {
            source,
            display: view.text.clone(),
            gaps: view.gaps.clone(),
            shifts: Vec::new(),
            refused_bulk: false,
        }
    }

    /// Edits to replay onto the fold anchors, and whether an undo/redo snapshot was refused
    /// (in which case the caller should unfold everything).
    pub fn finish(self) -> (Vec<Shift>, bool) {
        (self.shifts, self.refused_bulk)
    }

    /// Move every gap that sits after an edit at `display`/`source` by `delta` chars, and
    /// record the edit for the fold anchors.
    fn shift(&mut self, display: usize, source: usize, delta: isize) {
        for gap in &mut self.gaps {
            if gap.display >= display {
                gap.display = gap.display.saturating_add_signed(delta);
                gap.source = gap.source.saturating_add_signed(delta);
            }
        }
        self.shifts.push((source, delta));
    }
}

fn byte_of(s: &str, char_index: usize) -> usize {
    s.char_indices()
        .nth(char_index)
        .map_or(s.len(), |(byte, _)| byte)
}

impl egui::TextBuffer for Buffer<'_> {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        &self.display
    }

    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        let source = self.to_source_index(char_index);
        let byte = byte_of(self.source, source);
        self.source.insert_str(byte, text);
        let byte = byte_of(&self.display, char_index);
        self.display.insert_str(byte, text);
        let count = text.chars().count();
        self.shift(char_index, source, count as isize);
        count
    }

    fn delete_char_range(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let from = self.to_source_index(range.start);
        let to = to_source(&self.gaps, range.end, true);
        // Markers wholly inside the deleted span go with it: their hidden text was just
        // removed from the source, so the fold has nothing left to cover.
        self.gaps
            .retain(|g| g.display < range.start || g.display >= range.end);

        let bytes = byte_of(self.source, from)..byte_of(self.source, to);
        self.source.replace_range(bytes, "");
        let bytes = byte_of(&self.display, range.start)..byte_of(&self.display, range.end);
        self.display.replace_range(bytes, "");

        let display_delta = -((range.end - range.start) as isize);
        let source_delta = -((to - from) as isize);
        for gap in &mut self.gaps {
            if gap.display >= range.end {
                gap.display = gap.display.saturating_add_signed(display_delta);
                gap.source = gap.source.saturating_add_signed(source_delta);
            }
        }
        self.shifts.push((from, source_delta));
    }

    fn clear(&mut self) {
        // The default implementation passes a byte length as a char range; take the whole
        // buffer directly instead.
        let chars = self.display.chars().count();
        if chars > 0 {
            self.delete_char_range(0..chars);
        }
    }

    fn replace_with(&mut self, text: &str) {
        if self.gaps.is_empty() {
            let before = self.source.chars().count() as isize;
            self.source.clear();
            self.source.push_str(text);
            self.display.clear();
            self.display.push_str(text);
            self.shifts
                .push((0, text.chars().count() as isize - before));
        } else {
            // An undo/redo snapshot of the folded text cannot be mapped back onto the real
            // SQL (the markers stand in for text the snapshot never saw). Refuse it and ask
            // the app to unfold, rather than writing `⋯ 3 lines` into the user's query.
            self.refused_bulk = true;
        }
    }

    fn type_id(&self) -> std::any::TypeId {
        // `TypeId::of` needs a `'static` type; the borrowed lifetime is irrelevant to the
        // identity egui uses this for (downcasting a `dyn TextBuffer`).
        std::any::TypeId::of::<Buffer<'static>>()
    }
}

impl Buffer<'_> {
    fn to_source_index(&self, display: usize) -> usize {
        to_source(&self.gaps, display, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::TextBuffer as _;

    fn kinds(sql: &str) -> Vec<(usize, Kind, usize)> {
        regions(sql)
            .into_iter()
            .map(|r| (r.header_line, r.kind, r.lines))
            .collect()
    }

    #[test]
    fn folds_a_multi_line_statement_from_its_first_line() {
        let sql = "SELECT a,\n       b\nFROM t;\n";
        assert_eq!(kinds(sql), vec![(0, Kind::Statement, 2)]);
    }

    #[test]
    fn single_line_statements_are_not_foldable() {
        assert!(regions("SELECT 1;\nSELECT 2;\n").is_empty());
    }

    #[test]
    fn each_statement_of_a_script_folds_on_its_own() {
        let sql = "SELECT a\nFROM t;\n\nUPDATE u\nSET x = 1;\n";
        assert_eq!(
            kinds(sql),
            vec![(0, Kind::Statement, 1), (3, Kind::Statement, 1)]
        );
    }

    #[test]
    fn a_bracket_group_keeps_its_closing_line_visible() {
        let sql = "INSERT INTO t (\n  a,\n  b\n)\nVALUES (1, 2);\n";
        // The statement wins line 0 (it is wider); the bracket fold covers only its body.
        let all = regions(sql);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind, Kind::Statement);
        assert_eq!(all[0].lines, 4);

        // With the bracket opening on its own line the two no longer collide.
        let sql = "INSERT INTO t\n  (\n  a,\n  b\n)\nVALUES (1, 2);\n";
        assert_eq!(
            kinds(sql),
            vec![(0, Kind::Statement, 5), (1, Kind::Paren, 2)]
        );
    }

    #[test]
    fn case_blocks_fold_and_end_stays_visible() {
        let sql = "SELECT\n  CASE\n    WHEN a THEN 1\n    ELSE 2\n  END\nFROM t;\n";
        assert_eq!(
            kinds(sql),
            vec![(0, Kind::Statement, 5), (1, Kind::Block, 2)]
        );
    }

    #[test]
    fn begin_transaction_does_not_open_a_block() {
        // The `BEGIN` opens a transaction, so the `END` below still belongs to the `CASE`.
        let sql = "BEGIN TRANSACTION;\nSELECT\n  CASE\n    WHEN a THEN 1\n  END\nFROM t;\n";
        let blocks: Vec<_> = regions(sql)
            .into_iter()
            .filter(|r| r.kind == Kind::Block)
            .map(|r| r.header_line)
            .collect();
        assert_eq!(blocks, vec![2]);
    }

    #[test]
    fn a_run_of_comment_lines_folds_as_one() {
        let sql = "-- one\n-- two\n-- three\nSELECT 1;\n";
        assert_eq!(kinds(sql), vec![(0, Kind::Comment, 2)]);
    }

    #[test]
    fn semicolons_inside_strings_do_not_split_statements() {
        let sql = "SELECT 'a;b',\n  c\nFROM t;\n";
        assert_eq!(kinds(sql), vec![(0, Kind::Statement, 2)]);
    }

    /// Char index of `needle` in `haystack` (`str::find` counts bytes, and the marker's `⋯`
    /// is three of them).
    fn char_index_of(haystack: &str, needle: &str) -> usize {
        let byte = haystack.find(needle).expect("needle is present");
        haystack[..byte].chars().count()
    }

    fn folded_view(sql: &str, line: usize) -> View {
        let all = regions(sql);
        let region = all
            .iter()
            .find(|r| r.header_line == line)
            .expect("a foldable region on that line");
        View::build(sql, &all, &BTreeSet::from([region.anchor]))
    }

    #[test]
    fn a_collapsed_region_shows_a_marker_and_keeps_the_rest() {
        let sql = "SELECT a,\n       b\nFROM t;\nSELECT 2;\n";
        let view = folded_view(sql, 0);
        assert_eq!(view.text, "SELECT a, ⋯ 2 lines \nSELECT 2;\n");
        // The gutter still numbers the visible lines with their real line numbers.
        assert_eq!(view.source_lines, vec![0, 3, 4]);
    }

    #[test]
    fn indices_map_across_a_collapsed_region() {
        let sql = "SELECT a,\n       b\nFROM t;\nSELECT 2;\n";
        let view = folded_view(sql, 0);
        // Before the marker the two spaces agree.
        assert_eq!(view.to_source(3), 3);
        assert_eq!(view.to_display(3), Some(3));
        // Hidden text has no display position.
        assert_eq!(view.to_display(12), None);
        // After the marker, indices resume against the real text.
        let display = char_index_of(&view.text, "SELECT 2");
        let source = char_index_of(sql, "SELECT 2");
        assert_eq!(view.to_source(display), source);
        assert_eq!(view.to_display(source), Some(display));
    }

    #[test]
    fn typing_after_a_fold_edits_the_real_text() {
        let sql = "SELECT a,\n       b\nFROM t;\nSELECT 2;\n";
        let view = folded_view(sql, 0);
        let mut text = sql.to_string();
        let mut buffer = Buffer::new(&mut text, &view);
        let at = char_index_of(buffer.as_str(), "SELECT 2") + "SELECT ".len();
        buffer.insert_text("9", at);
        let (shifts, refused) = buffer.finish();
        assert!(!refused);
        assert_eq!(text, "SELECT a,\n       b\nFROM t;\nSELECT 92;\n");
        assert_eq!(shifts, vec![(char_index_of(sql, "SELECT 2") + 7, 1)]);
    }

    #[test]
    fn deleting_a_marker_deletes_what_it_hides() {
        let sql = "SELECT a,\n       b\nFROM t;\nSELECT 2;\n";
        let view = folded_view(sql, 0);
        let mut text = sql.to_string();
        let mut buffer = Buffer::new(&mut text, &view);
        // Select from the start of the line through the marker and delete it.
        let end = char_index_of(buffer.as_str(), "\n");
        buffer.delete_char_range(0..end);
        buffer.finish();
        assert_eq!(text, "\nSELECT 2;\n");
    }

    #[test]
    fn an_unmappable_undo_snapshot_is_refused() {
        let sql = "SELECT a,\n       b\nFROM t;\n";
        let view = folded_view(sql, 0);
        let mut text = sql.to_string();
        let mut buffer = Buffer::new(&mut text, &view);
        buffer.replace_with("SELECT a, ⋯ 2 lines \n");
        let (_, refused) = buffer.finish();
        assert!(refused, "a folded snapshot must never overwrite the SQL");
        assert_eq!(text, sql, "the real query is left untouched");
    }

    #[test]
    fn without_folds_the_buffer_is_a_plain_string() {
        let sql = "SELECT 1;";
        let view = View::whole(sql);
        assert!(view.markers().is_empty(), "nothing is collapsed");
        let mut text = sql.to_string();
        let mut buffer = Buffer::new(&mut text, &view);
        buffer.insert_text("2", 7);
        buffer.finish();
        assert_eq!(text, "SELECT 21;");
    }
}
