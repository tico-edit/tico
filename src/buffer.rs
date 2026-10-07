//! A single edited file: its text (as a rope), cursor/mark, undo/redo
//! history, and the on-disk metadata needed to detect external changes.

use ropey::Rope;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize, // character offset within the line (not display width)
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }
}

/// nano's `control_mbrep`: the character shown after a `^` for a control
/// character in the text -- `^@`..`^_` for C0 codes, `^?` for DEL, and
/// for the C1 codes U+0080..U+009F, `^``..`^~` with U+009F as `^=`.
/// `None` for anything else, a tab included (that expands to spaces).
pub fn control_rep(c: char) -> Option<char> {
    match c as u32 {
        0x09 => None,
        n @ 0x00..=0x1F => char::from_u32(n + 0x40),
        0x7F => Some('?'),
        0x9F => Some('='),
        n @ 0x80..=0x9E => char::from_u32(n - 0x20),
        _ => None,
    }
}

/// Columns a (non-tab) character takes on screen: two for a control
/// character's `^X` form, otherwise its Unicode width.
pub fn char_width(c: char) -> usize {
    if control_rep(c).is_some() {
        2
    } else {
        unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
    }
}

/// Display width (in columns, with tabs expanded) of `line` up to (but not
/// including) its `up_to_col`'th character. Shared by rendering (tab
/// expansion, spotlight positioning) and horizontal-scroll math, both of
/// which need to convert a character offset into a screen column.
pub fn display_width(line: &str, up_to_col: usize, tabsize: usize) -> usize {
    let mut w = 0;
    for (i, c) in line.chars().enumerate() {
        if i >= up_to_col {
            break;
        }
        if c == '\t' {
            w += tabsize - (w % tabsize);
        } else {
            w += char_width(c);
        }
    }
    w
}

/// The inverse of `display_width`: the character offset whose on-screen
/// cell contains display column `target_col` (or `line`'s length, if
/// `target_col` is past the line's own display width) -- used to turn a
/// mouse click's screen column back into a buffer column.
pub fn char_col_for_display(line: &str, target_col: usize, tabsize: usize) -> usize {
    let mut w = 0;
    for (i, c) in line.chars().enumerate() {
        let cw = if c == '\t' {
            tabsize - (w % tabsize)
        } else {
            char_width(c)
        };
        if w + cw > target_col {
            return i;
        }
        w += cw;
    }
    line.chars().count()
}

/// One undoable edit: replacing the text in `[start, end)` (in the buffer
/// *before* the edit) with `inserted`. Undo restores `removed` at `start`;
/// redo re-applies `inserted`.
#[derive(Debug, Clone)]
pub struct Edit {
    pub start_char: usize,
    pub removed: String,
    pub inserted: String,
    pub cursor_before: Pos,
    pub cursor_after: Pos,
}

/// Snapshot of a file's on-disk state at the time it was last loaded or
/// saved by us, used to detect concurrent external modification.
#[derive(Debug, Clone)]
pub struct DiskState {
    pub mtime: Option<SystemTime>,
    pub len: u64,
    pub content_hash: u64,
    /// The file's (device, inode) where the platform has them: part of
    /// nano's "File on disk has changed" test at save time.
    pub file_id: Option<(u64, u64)>,
}

/// A memoized `syntax::highlight()` result, valid as long as `version`
/// matches the buffer's `content_version` and `language` is still the same
/// language (compared by identity — languages live in a `'static` table,
/// so pointer equality is exact and free).
#[derive(Debug, Clone)]
struct HighlightCache {
    version: u64,
    language: *const crate::syntax::LanguageDef,
    spans: Vec<crate::syntax::HighlightSpan>,
}

/// A buffer's line-ending format, nano 8.7's `format_type`: what
/// `save_file` writes after each line -- LF, CR LF (DOS) or a bare CR
/// (old Mac). `Unspecified` is a buffer nothing has been read into yet
/// (nano's UNSPECIFIED): it writes as Unix, but the first file read into
/// it decides the format (see `adopt_format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineFormat {
    #[default]
    Unspecified,
    Unix,
    Dos,
    Mac,
}

pub struct Buffer {
    pub rope: Rope,
    /// Line endings to write this buffer with; see `LineFormat`.
    pub format: LineFormat,
    pub path: Option<PathBuf>,
    pub cursor: Pos,
    pub mark: Option<Pos>,
    /// Whether `mark` was auto-set by Shift+movement (nano's "soft mark")
    /// rather than explicitly toggled on with `^^`/`M-A` (a "hard" mark).
    /// A soft mark is auto-cleared by the next plain (non-Shift) movement
    /// or edit; a hard one persists until toggled off again.
    pub softmark: bool,
    pub modified: bool,
    pub undo_stack: Vec<Edit>,
    pub redo_stack: Vec<Edit>,
    pub disk_state: Option<DiskState>,
    /// Original content as loaded, used as the merge base for three-way
    /// merges when the file changes on disk while we have local edits.
    pub original_content: String,
    pub top_line: usize,
    /// Horizontal scroll offset (in display columns) applied when rendering
    /// the cursor's current line, when it's too long to fit the screen and
    /// `softwrap` is off. Other lines always render from column 0 — nano
    /// scrolls only the current line sideways, not the whole viewport.
    pub left_col: usize,
    pub language: Option<&'static crate::syntax::LanguageDef>,
    /// Remembered column for consecutive up/down movement through shorter
    /// lines (nano's `placewewant`), together with where the cursor was
    /// left by that movement. It only applies while the cursor is still
    /// there: any other way of moving it -- a jump, a search, a click, an
    /// edit -- makes the next Up/Down start from the cursor's own column,
    /// as in nano, without every such site having to clear it.
    goal_col: Option<(usize, Pos)>,
    /// Path of this buffer's vim-style lock file (`set locking`), if one is
    /// currently held.
    pub lock_filename: Option<PathBuf>,
    /// Whether the lock file has already been rewritten with the "modified"
    /// flag set, so it's only rewritten once per edit session (matches
    /// nano's `set_modified()`, which does this only on the false->true
    /// transition).
    pub lock_modified_written: bool,
    /// Set by `[I]gnore All` at the "file changed on disk" prompt: skips
    /// external-change detection for this buffer entirely (not just the
    /// change that was showing), until the buffer is closed. Sticky across
    /// saves — the user asked to stop being asked about this file, not
    /// just about the one change already on screen.
    pub ignore_external_changes: bool,
    /// Bumped on every edit (see `replace_range`/`undo`/`redo`) — lets
    /// `highlighted_spans_cached` tell whether its cache is still valid
    /// without re-hashing or re-stringifying the whole buffer.
    content_version: u64,
    /// Cache for `syntax::highlight()`: reparsing the whole file with
    /// tree-sitter and re-running its query is too expensive to redo on
    /// every render, but every render (including pure cursor movement) used
    /// to do exactly that. Interior mutability lets the read-only render
    /// pass populate it.
    highlight_cache: std::cell::RefCell<Option<HighlightCache>>,
    /// Whether the "syntax highlighting disabled: file too large" notice
    /// has already been shown for this buffer, so it's a one-time heads-up
    /// rather than repeated on every render.
    pub highlighting_size_warning_shown: bool,
}

impl Buffer {
    pub fn empty() -> Buffer {
        Buffer {
            rope: Rope::new(),
            format: LineFormat::Unspecified,
            path: None,
            cursor: Pos::new(0, 0),
            mark: None,
            softmark: false,
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            disk_state: None,
            original_content: String::new(),
            top_line: 0,
            left_col: 0,
            language: None,
            goal_col: None,
            lock_filename: None,
            lock_modified_written: false,
            ignore_external_changes: false,
            content_version: 0,
            highlight_cache: std::cell::RefCell::new(None),
            highlighting_size_warning_shown: false,
        }
    }

    pub fn from_text(text: &str, path: Option<PathBuf>) -> Buffer {
        let mut b = Buffer::empty();
        b.rope = Rope::from_str(text);
        b.original_content = text.to_string();
        b.path = path;
        b
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// The line count the way nano itself reports one (`"Read N lines"`,
    /// the minibar's `(N lines)`, ...): `line_count()` minus one when the
    /// text ends with a newline, since ropey's own convention (like
    /// nano's linked list of lines) counts the empty "line" after a final
    /// `\n` as a line of its own, which nano's messages never count.
    /// Matches `fileio::nano_style_line_count`, which takes raw text
    /// instead, for wherever a `Buffer` is already at hand.
    pub fn nano_line_count(&self) -> usize {
        let total = self.rope.len_lines();
        if total > 0 && self.rope.line(total - 1).len_chars() == 0 {
            total - 1
        } else {
            total
        }
    }

    /// Settle this buffer's line-ending format after reading a file into
    /// it, matching the end of nano's `read_file`: `set unix` forces Unix
    /// regardless; otherwise only a buffer that has no format yet takes
    /// the file's `detected` one (from `fileio::convert_line_endings`), so
    /// inserting a DOS file into an existing buffer doesn't flip it.
    pub fn adopt_format(&mut self, detected: LineFormat, unix: bool) {
        if unix {
            self.format = LineFormat::Unix;
        } else if self.format == LineFormat::Unspecified {
            self.format = detected;
        }
    }

    pub fn line(&self, idx: usize) -> String {
        if idx >= self.rope.len_lines() {
            return String::new();
        }
        let s = self.rope.line(idx).to_string();
        s.trim_end_matches('\n').to_string()
    }

    /// Byte offset of the start of line `idx` within the buffer's full
    /// text (as returned by `to_string()`) — for mapping whole-buffer byte
    /// ranges (e.g. tree-sitter highlight spans) to a specific line.
    pub fn line_start_byte(&self, idx: usize) -> usize {
        let idx = idx.min(self.rope.len_lines().saturating_sub(1));
        self.rope.line_to_byte(idx)
    }

    fn char_idx(&self, pos: Pos) -> usize {
        let line_start = self
            .rope
            .line_to_char(pos.line.min(self.rope.len_lines().saturating_sub(1)));
        line_start + pos.col
    }

    fn clamp_pos(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.rope.len_lines().saturating_sub(1));
        let line_len = self.line(line).chars().count();
        Pos::new(line, pos.col.min(line_len))
    }

    /// Replace `[start,end)` with `text`, recording an undo entry. Returns
    /// the new cursor position (end of the inserted text).
    fn replace_range(&mut self, start: Pos, end: Pos, text: &str, cursor_after: Pos) {
        let start_c = self.char_idx(start);
        let end_c = self.char_idx(end);
        let removed: String = self.rope.slice(start_c..end_c).to_string();
        self.rope.remove(start_c..end_c);
        self.rope.insert(start_c, text);
        self.undo_stack.push(Edit {
            start_char: start_c,
            removed,
            inserted: text.to_string(),
            cursor_before: self.cursor,
            cursor_after,
        });
        self.redo_stack.clear();
        self.modified = true;
        self.cursor = cursor_after;
        self.goal_col = None;
        self.content_version = self.content_version.wrapping_add(1);
    }

    /// Whether the text ends partway into a line -- missing the empty
    /// last line nano calls the "magic line", which (unless `nonewlines`)
    /// it keeps below the text at all times, so that the cursor can
    /// always move down past a last line that has text on it and a saved
    /// file ends with a newline.
    pub fn lacks_magic_line(&self) -> bool {
        let len = self.rope.len_chars();
        len > 0 && self.rope.char(len - 1) != '\n'
    }

    /// Restore the magic line (see `lacks_magic_line`) after an edit left
    /// the text ending mid-line: nano adds a fresh one as soon as the old
    /// one gets text typed, pasted, ... onto it, and undoing that edit
    /// takes the added line away with it -- so the newline joins the undo
    /// entry of the edit that reached the end of the text.
    pub fn add_magic_line(&mut self) {
        if !self.lacks_magic_line() {
            return;
        }
        let len = self.rope.len_chars();
        self.rope.insert_char(len, '\n');
        if let Some(edit) = self.undo_stack.last_mut()
            && edit.start_char + edit.inserted.chars().count() == len
        {
            edit.inserted.push('\n');
        }
        self.content_version = self.content_version.wrapping_add(1);
    }

    /// Invalidate the highlight cache after replacing `rope` wholesale
    /// (reload from disk, applying a merge, ...) — those bypass
    /// `replace_range`/`undo`/`redo`, the usual places that bump
    /// `content_version`.
    pub fn invalidate_highlight_cache(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
    }

    pub fn insert_char(&mut self, c: char) {
        let pos = self.cursor;
        let mut s = String::new();
        s.push(c);
        let after = if c == '\n' {
            Pos::new(pos.line + 1, 0)
        } else {
            Pos::new(pos.line, pos.col + 1)
        };
        self.replace_range(pos, pos, &s, after);
    }

    pub fn insert_str(&mut self, text: &str) {
        let pos = self.cursor;
        self.replace_text(pos, pos, text);
    }

    /// Replace `[start,end)` with `text` as a single undo step, leaving
    /// the cursor at the end of the inserted text.
    pub fn replace_text(&mut self, start: Pos, end: Pos, text: &str) {
        let newlines = text.matches('\n').count();
        let after = if newlines == 0 {
            Pos::new(start.line, start.col + text.chars().count())
        } else {
            let last_line_len = text.rsplit('\n').next().unwrap_or("").chars().count();
            Pos::new(start.line + newlines, last_line_len)
        };
        self.replace_range(start, end, text, after);
    }

    pub fn backspace(&mut self) {
        let pos = self.cursor;
        if pos.col == 0 && pos.line == 0 {
            return;
        }
        let before = if pos.col == 0 {
            let prev_len = self.line(pos.line - 1).chars().count();
            Pos::new(pos.line - 1, prev_len)
        } else {
            Pos::new(pos.line, pos.col - 1)
        };
        self.replace_range(before, pos, "", before);
    }

    pub fn delete_forward(&mut self) {
        let pos = self.cursor;
        let line_len = self.line(pos.line).chars().count();
        let after_pos = if pos.col >= line_len {
            if pos.line + 1 >= self.rope.len_lines() {
                return;
            }
            Pos::new(pos.line + 1, 0)
        } else {
            Pos::new(pos.line, pos.col + 1)
        };
        self.replace_range(pos, after_pos, "", pos);
    }

    /// Replace whole lines `first..=last` (the newlines between them
    /// included, the one after `last` not) with `text`, as a single undo
    /// step -- for line-oriented edits like indent/unindent that touch
    /// several non-adjacent spots at once but must undo together.
    pub fn replace_lines(&mut self, first: usize, last: usize, text: &str, cursor_after: Pos) {
        let end_col = self.line(last).chars().count();
        self.replace_range(
            Pos::new(first, 0),
            Pos::new(last, end_col),
            text,
            cursor_after,
        );
    }

    /// Cut and return the text of a line range (used by cut-line and
    /// cut-marked-region).
    pub fn delete_range(&mut self, start: Pos, end: Pos) -> String {
        let (start, end) = if (start.line, start.col) <= (end.line, end.col) {
            (start, end)
        } else {
            (end, start)
        };
        let start_c = self.char_idx(start);
        let end_c = self.char_idx(end);
        let removed = self.rope.slice(start_c..end_c).to_string();
        self.replace_range(start, end, "", start);
        removed
    }

    pub fn text_range(&self, start: Pos, end: Pos) -> String {
        let (start, end) = if (start.line, start.col) <= (end.line, end.col) {
            (start, end)
        } else {
            (end, start)
        };
        let start_c = self.char_idx(start);
        let end_c = self.char_idx(end);
        self.rope.slice(start_c..end_c).to_string()
    }

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo_stack.pop() else {
            return false;
        };
        let inserted_len = edit.inserted.chars().count();
        self.rope
            .remove(edit.start_char..edit.start_char + inserted_len);
        self.rope.insert(edit.start_char, &edit.removed);
        self.cursor = edit.cursor_before;
        self.redo_stack.push(edit);
        self.modified = !self.undo_stack.is_empty();
        self.goal_col = None;
        self.content_version = self.content_version.wrapping_add(1);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo_stack.pop() else {
            return false;
        };
        let removed_len = edit.removed.chars().count();
        self.rope
            .remove(edit.start_char..edit.start_char + removed_len);
        self.rope.insert(edit.start_char, &edit.inserted);
        self.cursor = edit.cursor_after;
        self.undo_stack.push(edit.clone());
        self.modified = true;
        self.goal_col = None;
        self.content_version = self.content_version.wrapping_add(1);
        true
    }

    pub fn move_left(&mut self) {
        self.goal_col = None;
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line(self.cursor.line).chars().count();
        }
    }

    pub fn move_right(&mut self) {
        self.goal_col = None;
        let len = self.line(self.cursor.line).chars().count();
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.rope.len_lines() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    /// The column Up/Down should aim for: the remembered one if the cursor
    /// is still where the last vertical move left it, else its own.
    pub fn goal_column(&self) -> usize {
        match self.goal_col {
            Some((goal, at)) if at == self.cursor => goal,
            _ => self.cursor.col,
        }
    }

    /// Put the cursor on `line` as near to column `goal` as that line
    /// allows, and keep aiming for `goal` on following Up/Down moves --
    /// nano's Go To Line, which sets `placewewant` to the requested column
    /// even when the line is too short to reach it.
    pub fn goto_line_aiming_at(&mut self, line: usize, goal: usize) {
        self.cursor = self.clamp_pos(Pos::new(line, goal));
        self.goal_col = Some((goal, self.cursor));
    }

    pub fn move_up(&mut self) {
        if self.cursor.line > 0 {
            let goal = self.goal_column();
            self.cursor = self.clamp_pos(Pos::new(self.cursor.line - 1, goal));
            self.goal_col = Some((goal, self.cursor));
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.line + 1 < self.rope.len_lines() {
            let goal = self.goal_column();
            self.cursor = self.clamp_pos(Pos::new(self.cursor.line + 1, goal));
            self.goal_col = Some((goal, self.cursor));
        }
    }

    pub fn move_home(&mut self) {
        self.goal_col = None;
        self.cursor.col = 0;
    }

    pub fn move_end(&mut self) {
        self.goal_col = None;
        self.cursor.col = self.line(self.cursor.line).chars().count();
    }

    /// `syntax::highlight(&self.to_string(), lang)`, memoized against
    /// `content_version` and `lang` — recomputing that on every render
    /// (a full tree-sitter reparse plus a full query run) is what made
    /// moving the cursor through a large file with syntax highlighting on
    /// dramatically slower than with it off, or than nano's own (much
    /// cheaper, regex-based) highlighting.
    pub fn highlighted_spans_cached(
        &self,
        lang: &'static crate::syntax::LanguageDef,
    ) -> Vec<crate::syntax::HighlightSpan> {
        let lang_ptr: *const crate::syntax::LanguageDef = lang;
        {
            let cache = self.highlight_cache.borrow();
            if let Some(c) = &*cache
                && c.version == self.content_version
                && c.language == lang_ptr
            {
                return c.spans.clone();
            }
        }
        let spans = crate::syntax::highlight(&self.to_string(), lang);
        *self.highlight_cache.borrow_mut() = Some(HighlightCache {
            version: self.content_version,
            language: lang_ptr,
            spans: spans.clone(),
        });
        spans
    }
}

impl std::fmt::Display for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.rope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jump_drops_the_remembered_column() {
        // Confirmed against the installed nano 8.7.1: Down onto a short
        // line, M-\ to the top, then Down twice stays in column 0.
        let mut b = Buffer::from_text("abcdefgh\nab\nabcdefgh\nabcdefgh\n", None);
        b.cursor = Pos::new(0, 6);
        b.move_down();
        assert_eq!(b.cursor, Pos::new(1, 2));
        b.move_down();
        assert_eq!(b.cursor, Pos::new(2, 6), "still aiming for column 6");
        b.cursor = Pos::new(0, 0);
        b.move_down();
        b.move_down();
        assert_eq!(b.cursor, Pos::new(2, 0));
    }

    #[test]
    fn goto_line_keeps_aiming_at_its_column() {
        let mut b = Buffer::from_text("abcdefgh\nab\nabcdefgh\n", None);
        b.goto_line_aiming_at(1, 6);
        assert_eq!(b.cursor, Pos::new(1, 2));
        assert_eq!(b.goal_column(), 6);
        b.move_down();
        assert_eq!(b.cursor, Pos::new(2, 6));
    }

    #[test]
    fn insert_and_backspace() {
        let mut b = Buffer::from_text("hello\nworld", None);
        b.cursor = Pos::new(0, 5);
        b.insert_str(" there");
        assert_eq!(b.to_string(), "hello there\nworld");
        b.backspace();
        assert_eq!(b.to_string(), "hello ther\nworld");
        assert_eq!(b.cursor, Pos::new(0, 10));
    }

    #[test]
    fn insert_newline_moves_to_next_line_col0() {
        let mut b = Buffer::from_text("ab", None);
        b.cursor = Pos::new(0, 1);
        b.insert_char('\n');
        assert_eq!(b.to_string(), "a\nb");
        assert_eq!(b.cursor, Pos::new(1, 0));
    }

    #[test]
    fn backspace_at_line_start_joins_lines() {
        let mut b = Buffer::from_text("foo\nbar", None);
        b.cursor = Pos::new(1, 0);
        b.backspace();
        assert_eq!(b.to_string(), "foobar");
        assert_eq!(b.cursor, Pos::new(0, 3));
    }

    #[test]
    fn delete_forward_at_eol_joins_next_line() {
        let mut b = Buffer::from_text("foo\nbar", None);
        b.cursor = Pos::new(0, 3);
        b.delete_forward();
        assert_eq!(b.to_string(), "foobar");
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut b = Buffer::from_text("abc", None);
        b.cursor = Pos::new(0, 3);
        b.insert_str("def");
        assert_eq!(b.to_string(), "abcdef");
        assert!(b.undo());
        assert_eq!(b.to_string(), "abc");
        assert_eq!(b.cursor, Pos::new(0, 3));
        assert!(b.redo());
        assert_eq!(b.to_string(), "abcdef");
    }

    #[test]
    fn highlighted_spans_cached_is_stable_without_edits() {
        let lang = crate::syntax::detect_with_override(None, "", Some("rust")).unwrap();
        let b = Buffer::from_text("fn main() { let x = 1; }\n", None);
        let first = b.highlighted_spans_cached(lang);
        let second = b.highlighted_spans_cached(lang);
        assert!(!first.is_empty());
        assert_eq!(first, second, "repeated calls with no edit must agree");
    }

    #[test]
    fn highlighted_spans_cached_reflects_a_subsequent_edit() {
        let lang = crate::syntax::detect_with_override(None, "", Some("rust")).unwrap();
        let mut b = Buffer::from_text("fn main() {}\n", None);
        let before = b.highlighted_spans_cached(lang);

        b.cursor = Pos::new(0, 0);
        b.insert_str("// a comment\n");
        let after = b.highlighted_spans_cached(lang);

        assert_ne!(
            before, after,
            "an edit must invalidate the cache, not return stale spans"
        );
    }

    #[test]
    fn highlighted_spans_cached_reflects_undo_and_redo() {
        let lang = crate::syntax::detect_with_override(None, "", Some("rust")).unwrap();
        let mut b = Buffer::from_text("fn main() {}\n", None);
        let original = b.highlighted_spans_cached(lang);

        b.cursor = Pos::new(0, 0);
        b.insert_str("// a comment\n");
        let edited = b.highlighted_spans_cached(lang);
        assert_ne!(original, edited);

        b.undo();
        assert_eq!(
            b.highlighted_spans_cached(lang),
            original,
            "undo must invalidate the cache too, not just replace_range edits"
        );

        b.redo();
        assert_eq!(b.highlighted_spans_cached(lang), edited);
    }

    #[test]
    fn delete_range_marked_region() {
        let mut b = Buffer::from_text("hello world", None);
        let removed = b.delete_range(Pos::new(0, 0), Pos::new(0, 6));
        assert_eq!(removed, "hello ");
        assert_eq!(b.to_string(), "world");
    }

    #[test]
    fn move_up_down_clamps_column() {
        let mut b = Buffer::from_text("longline\nhi", None);
        b.cursor = Pos::new(0, 8);
        b.move_down();
        assert_eq!(b.cursor, Pos::new(1, 2));
        b.move_up();
        assert_eq!(b.cursor, Pos::new(0, 8));
    }

    #[test]
    fn nano_line_count_ignores_the_trailing_empty_line_after_a_final_newline() {
        // A trailing newline leaves ropey's own `line_count()` one higher
        // than nano ever reports (`(N lines)` in the minibar, "Read N
        // lines", ...) -- confirmed against the installed nano.
        let with_trailing_newline = Buffer::from_text("a\nb\nc\n", None);
        assert_eq!(with_trailing_newline.line_count(), 4);
        assert_eq!(with_trailing_newline.nano_line_count(), 3);

        let without_trailing_newline = Buffer::from_text("a\nb\nc", None);
        assert_eq!(without_trailing_newline.line_count(), 3);
        assert_eq!(without_trailing_newline.nano_line_count(), 3);

        let empty = Buffer::from_text("", None);
        assert_eq!(empty.line_count(), 1);
        assert_eq!(empty.nano_line_count(), 0);
    }

    #[test]
    fn char_col_for_display_inverts_display_width() {
        assert_eq!(char_col_for_display("hello", 0, 8), 0);
        assert_eq!(char_col_for_display("hello", 2, 8), 2);
        assert_eq!(
            char_col_for_display("hello", 100, 8),
            5,
            "past the end clamps to the line's length"
        );

        // A tab expands to the next 8-column stop: clicking anywhere within
        // its cell (0..7) should land on the tab itself (char index 0), and
        // column 8 is the first cell of the following character.
        for col in 0..8 {
            assert_eq!(char_col_for_display("\tx", col, 8), 0, "column {col}");
        }
        assert_eq!(char_col_for_display("\tx", 8, 8), 1);
    }
}
