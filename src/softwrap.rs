//! `set softwrap`: splitting an overlong line into screen-row "chunks",
//! and moving through the buffer a chunk at a time -- a port of nano
//! 8.7.1's `get_softwrap_breakpoint`, `get_chunk_and_edge`,
//! `go_back_chunks` and `go_forward_chunks` (src/winio.c). All columns
//! here are display columns (tabs expanded, wide characters counting
//! two), like nano's own.

use crate::app::Editor;
use crate::buffer::{Buffer, Pos, char_width};

/// The parameters that decide where a line wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wrap {
    /// Columns available to the text on each row (nano's `editwincols`).
    pub width: usize,
    pub tabsize: usize,
    /// `set atblanks`: prefer breaking after a blank to mid-word.
    pub atblanks: bool,
}

/// nano's `is_blank_char` (`iswblank`): a tab or a non-breaking-free
/// horizontal space.
fn is_blank(c: char) -> bool {
    c == '\t'
        || (c.is_whitespace()
            && !c.is_control()
            && !matches!(
                c,
                '\u{a0}' | '\u{2007}' | '\u{202f}' | '\u{2028}' | '\u{2029}'
            ))
}

impl Wrap {
    fn advance(&self, c: char, column: &mut usize) {
        *column += if c == '\t' {
            self.tabsize - (*column % self.tabsize)
        } else {
            char_width(c)
        };
    }

    /// The `(from, to)` display-column span of each chunk of `line`, in
    /// order. There is always at least one; the last ends at the line's
    /// full width -- and a line exactly filling its rows gets an empty
    /// final chunk to hold the cursor at its end, as in nano.
    pub fn spans(&self, line: &str) -> Vec<(usize, usize)> {
        let chars: Vec<char> = line.chars().collect();
        let width = self.width.max(1);
        let mut spans = Vec::new();
        // nano keeps these two across calls (statics) while walking one
        // line, so a chunk's scan resumes where the last one stopped.
        let mut idx = 0;
        let mut column = 0;
        let mut from = 0;
        loop {
            let rightside = from + width;
            let mut breaking_col = rightside;
            let mut last_blank: Option<(usize, usize)> = None;

            while idx < chars.len() && column < from {
                self.advance(chars[idx], &mut column);
                idx += 1;
            }
            while idx < chars.len() && column <= rightside {
                let c = chars[idx];
                // When breaking at blanks, do it *before* the target column.
                if self.atblanks && is_blank(c) && column < rightside {
                    last_blank = Some((idx, column));
                }
                breaking_col = if c == '\t' { rightside } else { column };
                self.advance(c, &mut column);
                idx += 1;
            }

            if column <= rightside {
                // Reached the end of the line without overshooting.
                spans.push((from, column));
                if column < rightside {
                    return spans;
                }
                from = column;
                continue;
            }

            let mut to = None;
            if let Some((blank_idx, blank_col)) = last_blank {
                let mut after = blank_col;
                self.advance(chars[blank_idx], &mut after);
                if after <= rightside {
                    idx = blank_idx + 1;
                    column = after;
                    to = Some(after);
                } else if chars[blank_idx] == '\t' {
                    breaking_col = rightside;
                }
            }
            let to = to.unwrap_or(if width > 1 { breaking_col } else { column - 1 });
            // Never stall on a character wider than the whole row.
            let to = if to > from { to } else { column };
            spans.push((from, to));
            from = to;
        }
    }

    /// nano's `get_chunk_and_edge`: which chunk of `line` (0-based) holds
    /// display column `column`, and that chunk's span. A column past the
    /// end of the line lands on the last chunk.
    pub fn chunk_and_span(&self, line: &str, column: usize) -> (usize, (usize, usize)) {
        let spans = self.spans(line);
        let last = spans.len() - 1;
        let i = spans
            .iter()
            .position(|&(from, to)| from <= column && column < to)
            .unwrap_or(last);
        (i, spans[i])
    }

    pub fn chunk_for(&self, line: &str, column: usize) -> usize {
        self.chunk_and_span(line, column).0
    }

    pub fn leftedge_for(&self, line: &str, column: usize) -> usize {
        self.chunk_and_span(line, column).1.0
    }

    /// How many rows beyond the first `line` takes.
    pub fn extra_chunks(&self, line: &str) -> usize {
        self.spans(line).len() - 1
    }

    /// nano's `go_back_chunks`: move `(line, leftedge)` up `nrows`
    /// chunks, leaving `leftedge` at the start of the chunk reached.
    /// Returns how many rows it could not move (nonzero only at the top
    /// of the buffer, where -- as in nano -- `line` stays put but
    /// `leftedge` drops to 0, and the count isn't reduced by the chunks
    /// that were available on that first line).
    pub fn go_back_chunks(
        &self,
        buf: &Buffer,
        nrows: usize,
        line: &mut usize,
        leftedge: &mut usize,
    ) -> usize {
        let mut i = nrows;
        while i > 0 {
            let text = buf.line(*line);
            let spans = self.spans(&text);
            let chunk = chunk_index(&spans, *leftedge);
            if chunk >= i {
                *leftedge = spans[chunk - i].0;
                return 0;
            }
            *leftedge = 0;
            if *line == 0 {
                return i;
            }
            i -= chunk + 1;
            *line -= 1;
            *leftedge = usize::MAX;
        }
        if *leftedge == usize::MAX {
            *leftedge = self.leftedge_for(&buf.line(*line), usize::MAX);
        }
        0
    }

    /// nano's `go_forward_chunks`: move `(line, leftedge)` down `nrows`
    /// chunks. Returns how many rows it could not move (nonzero only on
    /// reaching the buffer's last chunk); `leftedge` changes only when
    /// at least one row was moved.
    pub fn go_forward_chunks(
        &self,
        buf: &Buffer,
        nrows: usize,
        line: &mut usize,
        leftedge: &mut usize,
    ) -> usize {
        let last_line = buf.line_count().saturating_sub(1);
        let mut current = *leftedge;
        let mut spans = self.spans(&buf.line(*line));
        let mut i = nrows;
        while i > 0 {
            let chunk = chunk_index(&spans, current);
            if chunk + 1 < spans.len() {
                current = spans[chunk + 1].0;
                i -= 1;
                continue;
            }
            if *line >= last_line {
                break;
            }
            *line += 1;
            current = 0;
            spans = self.spans(&buf.line(*line));
            i -= 1;
        }
        if i < nrows {
            *leftedge = current;
        }
        i
    }

    /// The display column within `line`'s chunk starting at `leftedge`
    /// that `target` (a column relative to that leftedge) actually lands
    /// on -- nano's `actual_last_column`: past a non-final chunk's end it
    /// is pulled back to that chunk's last column, so the cursor doesn't
    /// spill onto the following row.
    pub fn actual_last_column(&self, line: &str, leftedge: usize, target: usize) -> usize {
        let spans = self.spans(line);
        let chunk = chunk_index(&spans, leftedge);
        let (from, to) = spans[chunk];
        let mut end = to - from;
        if chunk + 1 < spans.len() {
            end = end.saturating_sub(1);
        }
        leftedge + target.min(end)
    }
}

/// The index of the chunk in `spans` containing `column` (the last one
/// when past the end).
fn chunk_index(spans: &[(usize, usize)], column: usize) -> usize {
    spans
        .iter()
        .position(|&(from, to)| from <= column && column < to)
        .unwrap_or(spans.len() - 1)
}

/// The editor's side of softwrapping: where the cursor sits among the
/// viewport's rows, keeping it on screen, and the movements that go by
/// chunk rather than by line (nano's `do_up`/`do_down`, `do_page_up`/
/// `do_page_down`, `to_top_row`/`to_bottom_row`, `do_home`/`do_end` and
/// `do_scroll_up`/`do_scroll_down`, each in its `SOFTWRAP` branch). These
/// are only called while `softwrap` is on, with `w` from
/// `Editor::softwrap()`.
impl Editor {
    /// The cursor's display column within its line (nano's `xplustabs`).
    pub fn cursor_display_col(&self) -> usize {
        let buf = self.buf();
        crate::buffer::display_width(
            &buf.line(buf.cursor.line),
            buf.cursor.col,
            self.options.tabsize as usize,
        )
    }

    /// The viewport row the cursor is on (nano's `cursor_row`, as
    /// `place_the_cursor` computes it) -- negative when it's above the
    /// viewport, and possibly `>= text_rows()` when below.
    pub fn wrapped_cursor_row(&self, w: &Wrap) -> isize {
        let buf = self.buf();
        let top = buf.top_line.min(buf.line_count().saturating_sub(1));
        let rows_in = |l: usize| 1 + w.extra_chunks(&buf.line(l)) as isize;
        let mut row = -(w.chunk_for(&buf.line(top), buf.first_col) as isize);
        if buf.cursor.line >= top {
            row += (top..buf.cursor.line).map(rows_in).sum::<isize>();
        } else {
            row -= (buf.cursor.line..top).map(rows_in).sum::<isize>();
        }
        row + w.chunk_for(&buf.line(buf.cursor.line), self.cursor_display_col()) as isize
    }

    /// nano's `current_is_above_screen` / `current_is_below_screen`.
    fn wrapped_cursor_offscreen(&self, w: &Wrap) -> Option<bool> {
        let buf = self.buf();
        let col = self.cursor_display_col();
        if buf.cursor.line < buf.top_line
            || (buf.cursor.line == buf.top_line && col < buf.first_col)
        {
            return Some(true);
        }
        let (mut line, mut edge) = (buf.top_line, buf.first_col);
        let below = w.go_forward_chunks(buf, self.text_rows() - 1, &mut line, &mut edge) == 0
            && (line < buf.cursor.line
                || (line == buf.cursor.line
                    && edge < w.leftedge_for(&buf.line(buf.cursor.line), col)));
        below.then_some(false)
    }

    /// nano's `adjust_viewport`: scroll so the cursor's chunk lands on
    /// viewport row `goal` (or as near as the start of the buffer allows).
    pub fn wrapped_adjust_viewport(&mut self, w: &Wrap, goal: usize) {
        let col = self.cursor_display_col();
        let buf = self.buf_mut();
        let mut line = buf.cursor.line;
        let mut edge = w.leftedge_for(&buf.line(line), col);
        w.go_back_chunks(buf, goal, &mut line, &mut edge);
        buf.top_line = line;
        buf.first_col = edge;
    }

    /// Softwrap's `scroll_to_cursor`: when the cursor has left the
    /// viewport, bring it back on the top or bottom row (or, `centered`,
    /// the middle one).
    pub fn wrapped_scroll_to_cursor(&mut self, w: &Wrap, centered: bool) {
        // nano's `ensure_firstcolumn_is_aligned`: a resize, an edit or
        // turning softwrap on may have moved where the chunks fall.
        let buf = self.buf_mut();
        buf.left_col = 0;
        buf.top_line = buf.top_line.min(buf.line_count().saturating_sub(1));
        buf.first_col = w.leftedge_for(&buf.line(buf.top_line), buf.first_col);
        if let Some(above) = self.wrapped_cursor_offscreen(w) {
            let rows = self.text_rows();
            let goal = if centered {
                rows / 2
            } else if above {
                0
            } else {
                rows - 1
            };
            self.wrapped_adjust_viewport(w, goal);
        }
    }

    /// nano's `get_edge_and_target`: the cursor chunk's leftedge, and the
    /// column within a chunk that vertical movement aims for.
    fn wrapped_edge_and_target(&self, w: &Wrap) -> (usize, usize) {
        let buf = self.buf();
        let width = w.width.max(1);
        let shim = width * (1 + w.tabsize / width);
        let edge = w.leftedge_for(&buf.line(buf.cursor.line), self.cursor_display_col());
        let target = (self.buf().goal_column(w.tabsize) + shim - edge) % width;
        (edge, target)
    }

    /// nano's `proper_x`: the character index on `line` at display column
    /// `column` within the chunk at `edge` -- stepping past a tab that
    /// straddles a row boundary so the cursor doesn't fall back (or skip)
    /// a row -- with `edge` updated to the chunk that index is on.
    fn proper_x(
        &self,
        w: &Wrap,
        text: &str,
        edge: &mut usize,
        forward: bool,
        column: usize,
    ) -> (usize, bool) {
        let ts = w.tabsize;
        let mut index = crate::buffer::char_col_for_display(text, column, ts);
        let mut shifted = false;
        if text.chars().nth(index) == Some('\t')
            && ((forward && crate::buffer::display_width(text, index, ts) < *edge)
                || (!forward
                    && edge.checked_sub(1).is_some_and(|e| column / ts == e / ts)
                    && column / ts < (*edge + w.width - 1) / ts))
        {
            index += 1;
            shifted = true;
        }
        *edge = w.leftedge_for(text, crate::buffer::display_width(text, index, ts));
        (index, shifted)
    }

    /// nano's `set_proper_index_and_pww`: put the cursor on `line` at
    /// `target` columns into its chunk at `edge`, and make that the goal.
    fn wrapped_land(&mut self, w: &Wrap, line: usize, mut edge: usize, target: usize, fwd: bool) {
        let text = self.buf().line(line);
        let was_edge = edge;
        let mut column = w.actual_last_column(&text, edge, target);
        let (mut x, shifted) = self.proper_x(w, &text, &mut edge, fwd, column);
        if shifted || edge < was_edge {
            column = w.actual_last_column(&text, edge, target);
            x = self.proper_x(w, &text, &mut edge, fwd, column).0;
        }
        let buf = self.buf_mut();
        buf.cursor = Pos::new(line, x);
        buf.goal_col = Some((edge + target, buf.cursor));
    }

    /// Up/Down by `nrows` chunks; false (nothing moved) at either end.
    pub fn wrapped_vertical(&mut self, w: &Wrap, nrows: usize, forward: bool) -> bool {
        let (mut edge, target) = self.wrapped_edge_and_target(w);
        let mut line = self.buf().cursor.line;
        let left = if forward {
            w.go_forward_chunks(self.buf(), nrows, &mut line, &mut edge)
        } else {
            w.go_back_chunks(self.buf(), nrows, &mut line, &mut edge)
        };
        if left > 0 {
            return false;
        }
        self.wrapped_land(w, line, edge, target, forward);
        true
    }

    /// nano's `do_page_up`/`do_page_down` under softwrap: almost a
    /// screenful of chunks, keeping the cursor on its screen row.
    pub fn wrapped_page(&mut self, w: &Wrap, forward: bool) {
        let rows = self.text_rows();
        let mustmove = if rows < 3 { 1 } else { rows - 2 };
        let (mut line, mut edge, target, row) = if self.options.jumpyscrolling {
            (self.buf().top_line, self.buf().first_col, 0, 0)
        } else {
            let (edge, target) = self.wrapped_edge_and_target(w);
            let row = self.wrapped_cursor_row(w).clamp(0, rows as isize - 1) as usize;
            (self.buf().cursor.line, edge, target, row)
        };
        let left = if forward {
            w.go_forward_chunks(self.buf(), mustmove, &mut line, &mut edge)
        } else {
            w.go_back_chunks(self.buf(), mustmove, &mut line, &mut edge)
        };
        if left > 0 {
            let buf = self.buf_mut();
            buf.cursor = if forward {
                let last = buf.line_count().saturating_sub(1);
                Pos::new(last, buf.line(last).chars().count())
            } else {
                Pos::new(0, 0)
            };
            return;
        }
        self.wrapped_land(w, line, edge, target, forward);
        self.wrapped_adjust_viewport(w, row);
    }

    /// nano's `to_top_row`/`to_bottom_row` under softwrap.
    pub fn wrapped_to_edge_row(&mut self, w: &Wrap, bottom: bool) {
        let (_, offset) = self.wrapped_edge_and_target(w);
        let (mut line, mut edge) = (self.buf().top_line, self.buf().first_col);
        if bottom {
            let rows = self.text_rows();
            w.go_forward_chunks(self.buf(), rows - 1, &mut line, &mut edge);
        }
        self.wrapped_land(w, line, edge, offset, bottom);
    }

    /// nano's `do_home` under softwrap: to the start of the cursor's
    /// chunk, or from there to the start of the line.
    pub fn wrapped_home(&mut self, w: &Wrap) {
        let text = self.buf().line(self.buf().cursor.line);
        let mut edge = w.leftedge_for(&text, self.cursor_display_col());
        let chunk_edge = edge;
        let (left_x, _) = self.proper_x(w, &text, &mut edge, false, chunk_edge);
        let buf = self.buf_mut();
        if buf.cursor.col == left_x {
            buf.move_home();
            buf.goal_col = None;
        } else {
            buf.cursor.col = left_x;
            buf.goal_col = Some((chunk_edge, buf.cursor));
        }
    }

    /// nano's `do_end` under softwrap: to the end of the cursor's chunk,
    /// or from there to the end of the line.
    pub fn wrapped_end(&mut self, w: &Wrap) {
        let text = self.buf().line(self.buf().cursor.line);
        let spans = w.spans(&text);
        let chunk = chunk_index(&spans, self.cursor_display_col());
        let mut rightedge = spans[chunk].1;
        if chunk + 1 < spans.len() {
            rightedge -= 1;
        }
        let right_x = crate::buffer::char_col_for_display(&text, rightedge, w.tabsize);
        let buf = self.buf_mut();
        if buf.cursor.col == right_x {
            buf.move_end();
            buf.goal_col = None;
        } else {
            buf.cursor.col = right_x;
            buf.goal_col = Some((rightedge, buf.cursor));
        }
    }

    /// nano's `do_scroll_up`/`do_scroll_down` under softwrap: slide the
    /// viewport one chunk, moving the cursor along only when it would
    /// otherwise fall off the edge.
    pub fn wrapped_scroll(&mut self, w: &Wrap, forward: bool) {
        let rows = self.text_rows();
        let row = self.wrapped_cursor_row(w);
        let buf = self.buf();
        if forward {
            if row == 0 {
                self.wrapped_vertical(w, 1, true);
            }
            let buf = self.buf();
            let top = buf.line(buf.top_line);
            if rows > 1
                && (buf.top_line + 1 < buf.line_count()
                    || w.extra_chunks(&top) > w.chunk_for(&top, buf.first_col))
            {
                let (mut line, mut edge) = (buf.top_line, buf.first_col);
                w.go_forward_chunks(buf, 1, &mut line, &mut edge);
                let buf = self.buf_mut();
                buf.top_line = line;
                buf.first_col = edge;
            }
        } else {
            if buf.top_line == 0 && buf.first_col == 0 {
                return;
            }
            if row == rows as isize - 1 {
                self.wrapped_vertical(w, 1, false);
            }
            if rows > 1 {
                let buf = self.buf();
                let (mut line, mut edge) = (buf.top_line, buf.first_col);
                w.go_back_chunks(buf, 1, &mut line, &mut edge);
                let buf = self.buf_mut();
                buf.top_line = line;
                buf.first_col = edge;
            }
        }
    }

    /// The buffer position shown at viewport row `row`, display column
    /// `col` (relative to the text area) -- for mouse clicks.
    pub fn wrapped_pos_at(&self, w: &Wrap, row: usize, col: usize) -> Pos {
        let buf = self.buf();
        let (mut line, mut edge) = (buf.top_line, buf.first_col);
        w.go_forward_chunks(buf, row, &mut line, &mut edge);
        let text = buf.line(line);
        let column = w.actual_last_column(&text, edge, col);
        Pos::new(
            line,
            crate::buffer::char_col_for_display(&text, column, w.tabsize),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(width: usize, atblanks: bool) -> Wrap {
        Wrap {
            width,
            tabsize: 8,
            atblanks,
        }
    }

    #[test]
    fn short_lines_are_a_single_chunk() {
        assert_eq!(wrap(10, false).spans(""), vec![(0, 0)]);
        assert_eq!(wrap(10, false).spans("hello"), vec![(0, 5)]);
    }

    #[test]
    fn long_lines_break_at_the_width() {
        let w = wrap(10, false);
        assert_eq!(
            w.spans("abcdefghij klmnopqrst uvw"),
            vec![(0, 10), (10, 20), (20, 25)]
        );
    }

    #[test]
    fn a_line_exactly_filling_its_rows_gets_an_empty_last_chunk() {
        assert_eq!(wrap(5, false).spans("abcde"), vec![(0, 5), (5, 5)]);
    }

    #[test]
    fn atblanks_breaks_after_the_last_blank_that_fits() {
        let w = wrap(10, true);
        assert_eq!(
            w.spans("one two three fours"),
            vec![(0, 8), (8, 14), (14, 19)]
        );
        // A word longer than the row still breaks mid-word.
        assert_eq!(w.spans("abcdefghijklmno"), vec![(0, 10), (10, 15)]);
    }

    #[test]
    fn a_wide_character_is_never_split() {
        // Five two-column characters on rows of width 5.
        assert_eq!(
            wrap(5, false).spans("日本語日本"),
            vec![(0, 4), (4, 8), (8, 10)]
        );
    }

    #[test]
    fn a_tab_spans_the_row_boundary() {
        // "abcdef\tx": the tab runs from column 6 to 8, the row is 7 wide.
        assert_eq!(wrap(7, false).spans("abcdef\tx"), vec![(0, 7), (7, 9)]);
    }

    #[test]
    fn chunk_lookup_and_last_column() {
        let w = wrap(10, false);
        let line = "abcdefghijklmnopqrstuvwxy";
        assert_eq!(w.chunk_and_span(line, 0), (0, (0, 10)));
        assert_eq!(w.chunk_and_span(line, 15), (1, (10, 20)));
        assert_eq!(w.chunk_and_span(line, 25), (2, (20, 25)));
        assert_eq!(w.extra_chunks(line), 2);
        assert_eq!(w.actual_last_column(line, 10, 15), 19);
        assert_eq!(w.actual_last_column(line, 20, 15), 25);
    }

    #[test]
    fn chunk_navigation_crosses_lines() {
        let w = wrap(10, false);
        let buf = Buffer::from_text("abcdefghijklmnopqrstuvwxy\nshort\n", None);
        let (mut line, mut edge) = (0, 0);
        assert_eq!(w.go_forward_chunks(&buf, 3, &mut line, &mut edge), 0);
        assert_eq!((line, edge), (1, 0));
        assert_eq!(w.go_back_chunks(&buf, 2, &mut line, &mut edge), 0);
        assert_eq!((line, edge), (0, 10));
        assert_eq!(w.go_back_chunks(&buf, 5, &mut line, &mut edge), 5);
        assert_eq!((line, edge), (0, 0));
        let (mut line, mut edge) = (1, 0);
        assert_eq!(w.go_forward_chunks(&buf, 5, &mut line, &mut edge), 4);
        assert_eq!((line, edge), (2, 0));
    }

    /// A 20-column, 3-text-row editor (7 rows minus title, status and
    /// the two shortcut rows) softwrapping `text`.
    fn wrapped_editor(text: &str) -> Editor {
        let mut ed = Editor::new(
            crate::options::Options::default(),
            crate::keymap::KeyMap::new(),
        );
        ed.buffers[0] = Buffer::from_text(text, None);
        ed.screen_cols = 20;
        ed.screen_rows = 7;
        ed.options.softwrap = true;
        ed
    }

    fn press(ed: &mut Editor, action: crate::keymap::Action) {
        ed.begin_keystroke();
        ed.execute(action);
        ed.end_keystroke();
    }

    #[test]
    fn up_and_down_move_by_chunk_keeping_the_column() {
        use crate::keymap::Action::{Down, Right, Up};
        let line = "a".repeat(50);
        let mut ed = wrapped_editor(&format!("{line}\nxy\n"));
        for _ in 0..3 {
            press(&mut ed, Right);
        }
        press(&mut ed, Down);
        assert_eq!(ed.buf().cursor, Pos::new(0, 23));
        press(&mut ed, Down);
        assert_eq!(ed.buf().cursor, Pos::new(0, 43));
        press(&mut ed, Down);
        assert_eq!(ed.buf().cursor, Pos::new(1, 2), "clamped to the short line");
        press(&mut ed, Up);
        assert_eq!(ed.buf().cursor, Pos::new(0, 43), "the goal column survives");
    }

    #[test]
    fn moving_past_the_bottom_row_scrolls_one_chunk() {
        use crate::keymap::Action::Down;
        let line = "a".repeat(50);
        let mut ed = wrapped_editor(&format!("{line}\nxy\n"));
        press(&mut ed, Down);
        press(&mut ed, Down);
        assert_eq!((ed.buf().top_line, ed.buf().first_col), (0, 0));
        let w = ed.softwrap().unwrap();
        assert_eq!(ed.wrapped_cursor_row(&w), 2);
        press(&mut ed, Down);
        assert_eq!(ed.buf().cursor.line, 1);
        assert_eq!((ed.buf().top_line, ed.buf().first_col), (0, 20));
        assert_eq!(ed.wrapped_cursor_row(&w), 2);
    }

    #[test]
    fn home_and_end_go_to_the_chunk_edge_first() {
        use crate::keymap::Action::{Down, End, Home, Right};
        let line = "a".repeat(50);
        let mut ed = wrapped_editor(&line);
        press(&mut ed, Down);
        press(&mut ed, Right);
        press(&mut ed, End);
        assert_eq!(
            ed.buf().cursor.col,
            39,
            "the last column of the middle chunk"
        );
        press(&mut ed, End);
        assert_eq!(ed.buf().cursor.col, 50);
        press(&mut ed, Home);
        assert_eq!(ed.buf().cursor.col, 40, "the start of the last chunk");
        press(&mut ed, Home);
        assert_eq!(ed.buf().cursor.col, 0);
    }

    #[test]
    fn turning_softwrap_off_drops_the_first_column() {
        use crate::keymap::Action::{Down, SoftWrap};
        let line = "a".repeat(50);
        let mut ed = wrapped_editor(&format!("{line}\nb\nc\n"));
        for _ in 0..4 {
            press(&mut ed, Down);
        }
        assert_eq!((ed.buf().top_line, ed.buf().first_col), (0, 40));
        press(&mut ed, SoftWrap);
        assert_eq!(ed.buf().first_col, 0);
    }

    #[test]
    fn a_click_lands_on_the_chunk_under_it() {
        let line = "a".repeat(50);
        let mut ed = wrapped_editor(&format!("{line}\nxy\n"));
        ed.buf_mut().first_col = 20;
        let w = ed.softwrap().unwrap();
        assert_eq!(ed.wrapped_pos_at(&w, 0, 5), Pos::new(0, 25));
        assert_eq!(ed.wrapped_pos_at(&w, 1, 15), Pos::new(0, 50));
        assert_eq!(ed.wrapped_pos_at(&w, 2, 15), Pos::new(1, 2));
    }
}
