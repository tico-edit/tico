//! Editor application state and action dispatch: ties together buffers,
//! options, the keymap, cut/paste, search, and the various single-line
//! prompts (search, goto, save-as, yes/no, ...), independent of any
//! particular terminal backend.

use crate::buffer::{Buffer, Pos};
use crate::keymap::{Action, Menu};
use crate::options::Options;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    WhereIs,
    Replace1, // search term
    Replace2 {
        search: String,
    }, // replacement term
    ReplaceConfirm(ReplaceLoopState),
    GotoLine,
    WriteOut {
        flow: WriteFlow,
    },
    /// One of the Yes/No questions nano's `write_it_out` may ask once a
    /// filename is settled (see `WriteQuestion`). `answer` is the name as
    /// typed, offered again if the Write Out prompt has to be reshown.
    WriteConfirm {
        question: WriteQuestion,
        answer: String,
        flow: WriteFlow,
    },
    Exit {
        discard_and_quit: bool,
    },
    ExternalChangeConflict,
    /// Someone else appears to be editing this file (a vim/nano-style lock
    /// file exists for it). `lock_path` is where to write our own lock if
    /// the user chooses to open anyway; `target` is the display path
    /// recorded inside it.
    LockConflict {
        lock_path: std::path::PathBuf,
        target: String,
    },
    /// `^R` Read File / `^T` Execute Command: `new_buffer` mirrors nano's
    /// `NEW_BUFFER` flag, toggled live by `M-F` within this one prompt
    /// (seeded from `set multibuffer`, reset back to that baseline the next
    /// time the prompt opens) — when set, the file/command output opens as
    /// a separate buffer instead of being inserted into the current one at
    /// the cursor. `execute` mirrors nano's own toggle between "insert a
    /// file" and "run a command", flipped in place by `^X` (both prompts
    /// share one underlying UI in nano, `insert_a_file_or()`).
    InsertFile {
        new_buffer: bool,
        execute: bool,
    },
    /// The Execute-Command prompt's `^T` (no `set speller`/`--speller`
    /// configured): one step of nano's own word-by-word spell-fix loop.
    /// `word` is the currently spotlighted misspelling, pre-filled into the
    /// "Edit a replacement" prompt; `remaining` holds the still-unchecked
    /// words after it, already sorted and deduplicated like nano's own
    /// `hunspell -l | sort -f | uniq` pipeline.
    SpellFix {
        word: String,
        remaining: Vec<String>,
    },
    /// The linter's interactive result viewer (nano's `MLINTER` menu):
    /// `^Y`/PageUp and `^V`/PageDown step through `messages`, moving the
    /// cursor to each one's reported location; `index` is the currently
    /// shown message.
    Linter {
        messages: Vec<LintMessage>,
        index: usize,
    },
    /// The file browser's Search prompt (nano's `MWHEREISFILE`), shown
    /// over the listing in `Editor::browser`.
    BrowserSearch {
        forwards: bool,
    },
    /// The file browser's Go To Directory prompt (nano's `MGOTODIR`).
    GotoDir,
}

/// An open file browser (`^T` at the Read File / Write Out prompts): the
/// listing, plus the prompt it was opened from, which gets the chosen
/// filename -- or is simply shown again when the browser is left.
#[derive(Debug, Clone)]
pub struct BrowserSession {
    pub list: crate::browser::Browser,
    pub return_to: Prompt,
}

/// The context of one nano `write_it_out` call, carried through the Write
/// Out prompt and its follow-up questions: nano keeps these in parameters
/// and locals across its loop's `continue`s, but tico's prompts are modal,
/// so each step hands them on to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteFlow {
    /// Saving from `^X` (nano's `exiting`): always the whole buffer, and a
    /// successful write closes it.
    pub exiting: bool,
    /// False for `^S` (nano's `do_savefile`): a named buffer is written
    /// under its own name without showing the prompt.
    pub withprompt: bool,
    /// nano's `maychange`: writing under a name other than the buffer's
    /// own is fine -- the buffer has no name, or "Save file under
    /// DIFFERENT NAME?" was already answered Yes.
    pub maychange: bool,
}

/// The questions nano's `write_it_out` asks before writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteQuestion {
    /// "Save file under DIFFERENT NAME? "
    DifferentName,
    /// "File \"NAME\" exists; OVERWRITE? "
    Overwrite,
    /// "File was modified since you opened it; continue saving? "
    DiskChanged,
}

/// An answer to one of nano's `ask_user` Yes/No questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YesNo {
    Yes,
    No,
    Cancel,
}

/// One parsed line of linter output: `filename:line[:col]: msg` (or
/// `filename:line,col: msg`), matching nano's own linter-output parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintMessage {
    pub filename: String,
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

/// State threaded through an in-progress interactive replace, one match at
/// a time — matches nano's `do_replace_loop()` in src/search.c: find the
/// next occurrence, ask "Replace this instance?" (Yes/No/All/Cancel), act,
/// and repeat, wrapping around the buffer once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceLoopState {
    pub search: String,
    pub replacement: String,
    pub match_pos: Pos,
    pub match_len: usize,
    /// Where this replace operation began (the cursor position when it was
    /// kicked off); once the search wraps around and reaches here again,
    /// the loop stops rather than repeating forever.
    pub session_start: Pos,
    pub wrapped: bool,
    pub count: usize,
    /// When a region was marked at the start of this replace, its end
    /// position — matches never wrap and are only reported before this
    /// point (nano's `INREGION` mode). Adjusted as replacements on the
    /// same line change its length, mirroring nano's own `mark_x` upkeep
    /// in `do_replace_loop`.
    pub region_end: Option<Pos>,
}

pub enum ReplaceChoice {
    Yes,
    No,
    All,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub menu: Menu,
    pub label: String,
    pub input: String,
    pub cursor: usize,
    /// Which history entry is currently shown (0 = most recent), while
    /// browsing history with Older/Newer; `None` means `input` is
    /// live-typed text, not a history entry.
    pub history_pos: Option<usize>,
    /// What `input` was before history browsing started, restored when
    /// Newer is pressed past the most recent entry.
    pub saved_input: Option<String>,
}

pub enum Mode {
    Editing,
    Prompt(Prompt),
    /// The `^G` help viewer. `top` is the first scrolled-to body line
    /// (index into `lines`, which are wrapped and ready to draw as-is).
    /// `return_to` is the prompt to restore on close, when help was opened
    /// from one (e.g. `^G` inside a Search prompt) — `None` means it was
    /// opened from the main editing window, so closing goes back there.
    Help {
        lines: Vec<String>,
        top: usize,
        cursor: HelpCursor,
        return_to: Option<Box<Prompt>>,
    },
    /// A full-screen, scrollable diff viewer — currently used only for
    /// previewing a three-way merge (`^X` reload-conflict -> `[M]erge`)
    /// before applying it, since the diff can easily run to many lines and
    /// doesn't fit in a one-line prompt label (cramming it in there, with
    /// embedded newlines, used to scramble the display). `top` is the
    /// first scrolled-to body line (index into `lines[1..]`).
    Diff {
        lines: Vec<String>,
        top: usize,
        outcome: DiffOutcome,
    },
    /// The file browser, whose state is `Editor::browser` -- kept there
    /// rather than here so that it survives while one of its own prompts
    /// (`PromptKind::BrowserSearch`/`GotoDir`) or its help is up.
    Browser,
    Quit,
}

/// Where the cursor is in the help viewer's body (an index into its
/// `lines[1..]`), which only matters with `set showcursor`: nano's help
/// viewer is a buffer, and with the cursor shown the arrow keys move it
/// through the text instead of scrolling. `want` is the column Up/Down
/// aim for (nano's `placewewant`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HelpCursor {
    pub line: usize,
    pub col: usize,
    pub want: usize,
}

/// What happens when the diff viewer (`Mode::Diff`) is dismissed.
#[derive(Debug, Clone)]
pub enum DiffOutcome {
    /// A clean three-way merge is ready; accepting replaces the buffer's
    /// content with `merged_text`.
    ApplyMerge { merged_text: String },
    /// The merge had overlapping changes and couldn't be resolved
    /// automatically; dismissing returns to the reload/keep/cancel choice.
    Conflict,
}

/// Severity of a status-bar message, matching the subset of nano's message
/// importance levels (src/definitions.h: VACUUM/HUSH/REMARK/INFO/NOTICE/
/// AHEM/MILD/ALERT) that affect rendering here: most messages are `Normal`
/// (nano's default STATUS_BAR color, reverse video); errors like "is a
/// directory" or "is unwritable" are `Alert` (nano's ERROR_MESSAGE color,
/// bold white-on-red, plus a bell); `Mild` warnings like "Directory is not
/// writable" use the same ERROR_MESSAGE color (MILD > NOTICE in nano's
/// enum) but without the bell (only importance == ALERT beeps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusLevel {
    #[default]
    Normal,
    Mild,
    Alert,
}

/// `set minibar`'s "(N lines[, DOS/Mac])" note text for a buffer with
/// `count` lines in the given format -- shared by `Editor::note_buffer_linecount`
/// and the startup file-loading path in `main.rs`, which needs it before the
/// buffer is pushed onto `Editor::buffers` (so `note_buffer_linecount`,
/// which reads the *current* buffer, isn't usable yet). Singular "line" and
/// the format tag only appear when they apply, confirmed against the
/// installed nano's own escape-code output.
pub fn minibar_linecount_note(count: usize, format: crate::buffer::LineFormat) -> String {
    use crate::buffer::LineFormat;
    let word = if count == 1 { "line" } else { "lines" };
    match format {
        LineFormat::Dos => format!("({count} {word}, DOS)"),
        LineFormat::Mac => format!("({count} {word}, Mac)"),
        LineFormat::Unix | LineFormat::Unspecified => format!("({count} {word})"),
    }
}

#[derive(Default)]
pub struct SearchState {
    pub last_pattern: Option<String>,
    pub case_sensitive: bool,
    pub use_regex: bool,
    pub backwards: bool,
}

pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub current: usize,
    pub options: Options,
    pub keymap: crate::keymap::KeyMap,
    /// The syntax-highlighting theme (see `crate::theme`). Starts as the
    /// built-in default; `main` swaps in the configured one after loading.
    pub theme: crate::theme::Theme,
    /// Per-language overrides of `theme`, keyed by `LanguageDef::name`
    /// (`[syntax]`'s `perl.theme = ...`). Use `theme_for()` rather than
    /// reading either field directly.
    pub language_themes: std::collections::HashMap<String, crate::theme::Theme>,
    pub cutbuffer: String,
    pub cut_was_consecutive: bool,
    /// nano's `also_the_last`: whether a marked region that ends exactly
    /// at column 0 should nonetheless include that last line when
    /// indenting/unindenting/commenting. Set once such an action has run on a region
    /// that *did* reach into its last line, and cleared as soon as the
    /// cursor moves to another line -- so repeated `M-}`/`M-{` presses
    /// keep acting on the same set of lines. See `marked_line_range`.
    pub also_the_last: bool,
    /// nano's `shift_held`: an action that moves the cursor/mark columns
    /// but wants a soft (Shift-selected) mark kept anyway -- indent,
    /// unindent and comment -- sets this, and ui.rs's post-keystroke soft-mark drop
    /// skips that keystroke. Cleared before each keystroke is handled.
    pub shift_held: bool,
    pub search: SearchState,
    pub status: Option<String>,
    pub status_level: StatusLevel,
    /// Keystrokes remaining before the status message is wiped, mirroring
    /// nano's `countdown` in src/winio.c: a status message is cleared after
    /// 20 keystrokes (or 1, with `quickblank`) in the main editing window —
    /// it is not a timer.
    status_countdown: u32,
    /// Set when an Alert-level message was just posted; the UI layer rings
    /// the terminal bell once and clears this, matching nano's beep() in
    /// statusline() for ALERT-importance messages.
    pub bell_pending: bool,
    /// A warning to flash before the next frame, nano's
    /// `warn_and_briefly_pause`: the UI layer shows it as an Alert with the
    /// shortcut bars blanked, holds it for 1.5s, then clears it and draws
    /// whatever mode was set up behind it (e.g. the prompt that follows).
    pub brief_warning: Option<String>,
    /// The currently highlighted search/replace match, if any (position +
    /// length in characters), rendered black-on-yellow like nano's
    /// `spotlightcolor` (confirmed against the installed nano's own
    /// escape-code output).
    pub spotlight: Option<(Pos, usize)>,
    /// When the spotlight should be cleared on its own — `None` means it
    /// persists until something else clears it (used for the "Replace
    /// this instance?" match, which stays lit for as long as that prompt
    /// is up); `Some(deadline)` means a plain search match, which nano
    /// auto-clears after ~1.5s (or ~0.8s with quickblank) of no input —
    /// confirmed by timing the installed nano directly.
    pub spotlight_deadline: Option<std::time::Instant>,
    /// Search/Replace/Execute history, recalled with Up/Down or ^P/^N at
    /// those prompts. Built up in-session regardless of settings; only
    /// loaded from and saved to disk when `historylog` is on.
    pub history: crate::history::HistoryStore,
    pub mode: Mode,
    pub screen_rows: usize,
    pub screen_cols: usize,
    /// The `^R` Read File prompt's `Tab`-completion listing, when more than
    /// one filename matches the typed fragment — shown as a grid in place
    /// of the buffer, matching nano's `input_tab`. Cleared by any other
    /// keystroke.
    pub file_completions: Option<Vec<String>>,
    /// `set minibar`'s one-shot "(N lines[, DOS/Mac])" note, shown after the
    /// filename in place of an `[i/n]` buffer counter right after a file is
    /// loaded or saved, or a buffer is switched to (nano's `report_size`;
    /// see `note_buffer_linecount`). Cleared by the next keystroke handled
    /// in the main editing window, same as `shift_held`/the search
    /// spotlight — confirmed against the installed nano's own escape-code
    /// output.
    pub minibar_note: Option<String>,
    /// The open file browser, if any (see `Mode::Browser`).
    pub browser: Option<BrowserSession>,
}

impl Editor {
    /// The theme to paint text of language `language` (a `LanguageDef::name`)
    /// with: its override from `[syntax]` if it has one, otherwise the
    /// global theme. Resolved per highlight span, not per buffer, so a
    /// heredoc body injected with another language gets that language's
    /// theme.
    pub fn theme_for(&self, language: &str) -> &crate::theme::Theme {
        self.language_themes.get(language).unwrap_or(&self.theme)
    }

    pub fn new(options: Options, keymap: crate::keymap::KeyMap) -> Editor {
        // `set casesensitive` / `set regexp` in nanorc/ticorc set the
        // default search mode, same as nano; there's no CLI flag for
        // either (nano doesn't have one), only the config item.
        let search = SearchState {
            case_sensitive: options.casesensitive,
            use_regex: options.regexp,
            ..SearchState::default()
        };
        let history = if options.historylog {
            crate::history::HistoryStore::load()
        } else {
            crate::history::HistoryStore::new()
        };
        Editor {
            buffers: vec![Buffer::empty()],
            current: 0,
            options,
            keymap,
            theme: crate::theme::Theme::builtin_default(),
            language_themes: std::collections::HashMap::new(),
            cutbuffer: String::new(),
            cut_was_consecutive: false,
            also_the_last: false,
            shift_held: false,
            search,
            status: None,
            status_level: StatusLevel::Normal,
            status_countdown: 0,
            bell_pending: false,
            brief_warning: None,
            spotlight: None,
            spotlight_deadline: None,
            history,
            mode: Mode::Editing,
            screen_rows: 24,
            screen_cols: 80,
            file_completions: None,
            minibar_note: None,
            browser: None,
        }
    }

    pub fn buf(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    pub fn buf_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_level = StatusLevel::Normal;
        self.status_countdown = if self.options.quickblank { 1 } else { 20 };
    }

    /// Like `set_status`, but for error-class messages (unwritable file,
    /// "is a directory", ...): rendered bold white-on-red instead of plain
    /// reverse video, and rings the terminal bell, matching nano's
    /// ALERT-importance messages.
    pub fn set_status_alert(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_level = StatusLevel::Alert;
        self.status_countdown = if self.options.quickblank { 1 } else { 20 };
        self.bell_pending = true;
    }

    /// Like `set_status_alert`, but for MILD-importance warnings (e.g.
    /// "Directory is not writable"): same coloring, no bell.
    pub fn set_status_mild(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_level = StatusLevel::Mild;
        self.status_countdown = if self.options.quickblank { 1 } else { 20 };
    }

    /// Highlight a plain search match, auto-clearing after ~1.5s (0.8s with
    /// quickblank) of no further input.
    fn set_spotlight_timed(&mut self, pos: Pos, len: usize) {
        self.spotlight = Some((pos, len));
        let ms = if self.options.quickblank { 800 } else { 1500 };
        self.spotlight_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(ms));
    }

    /// Highlight the match currently up for replace confirmation; persists
    /// until explicitly cleared (when the prompt is answered), not timed.
    pub fn set_spotlight_persistent(&mut self, pos: Pos, len: usize) {
        self.spotlight = Some((pos, len));
        self.spotlight_deadline = None;
    }

    pub fn clear_spotlight(&mut self) {
        self.spotlight = None;
        self.spotlight_deadline = None;
    }

    /// If a timed spotlight's deadline has passed, clear it. Returns true
    /// if it just got cleared (so the caller knows to redraw).
    pub fn tick_spotlight_deadline(&mut self) -> bool {
        if let Some(deadline) = self.spotlight_deadline
            && std::time::Instant::now() >= deadline
        {
            self.clear_spotlight();
            return true;
        }
        false
    }

    /// Call once per keystroke handled while focused on the main edit
    /// window (not while a prompt is active), matching nano's
    /// `blank_it_when_expired()`. Wipes the status message once its
    /// countdown reaches zero. Returns true if the message was just wiped
    /// (so the caller knows a redraw is needed).
    pub fn tick_status_countdown(&mut self) -> bool {
        if self.status_countdown == 0 {
            return false;
        }
        self.status_countdown -= 1;
        if self.status_countdown == 0 {
            self.status = None;
            return true;
        }
        false
    }

    /// Number of rows available for buffer text (screen minus title bar,
    /// status line, and the two-line shortcut help unless `nohelp`).
    pub fn text_rows(&self) -> usize {
        let mut used = 2; // title bar + status/prompt/minibar line
        if !self.options.nohelp {
            used += 2;
        }
        if self.options.zero {
            used = 0;
        } else if self.options.minibar {
            // `set minibar` suppresses just the title bar row; the minibar
            // itself takes over the status row, already counted above.
            used -= 1;
        }
        self.screen_rows.saturating_sub(used).max(1)
    }

    /// Width, in columns, of the line-number margin (0 when `linenumbers`
    /// is off) — the text area proper is `screen_cols - gutter_width()`
    /// wide.
    pub fn gutter_width(&self) -> usize {
        if !self.options.linenumbers {
            return 0;
        }
        let digits = self.buf().line_count().to_string().len();
        digits + 1
    }

    pub fn scroll_to_cursor(&mut self) {
        let rows = self.text_rows();
        let buf = self.buf_mut();
        if buf.cursor.line < buf.top_line {
            buf.top_line = buf.cursor.line;
        } else if buf.cursor.line >= buf.top_line + rows {
            buf.top_line = buf.cursor.line + 1 - rows;
        }
        self.scroll_horizontal_to_cursor();
    }

    /// Horizontal counterpart of `scroll_to_cursor`, for lines too long to
    /// fit the screen (relevant only when `softwrap` is off, since a
    /// soft-wrapped line never needs sideways scrolling). Adjusts
    /// `buf.left_col` — the display-column offset applied when rendering
    /// just the cursor's current line — using the same "cushion" scheme
    /// nano uses when not soft-wrapping (its `united_sidescroll`, see
    /// `get_page_start()` in nano's src/utils.c): scrolling only kicks in
    /// within a few columns of either edge, and then jumps just enough to
    /// restore that margin, rather than moving one column at a time.
    fn scroll_horizontal_to_cursor(&mut self) {
        const CUSHION: usize = 3;
        let tabsize = self.options.tabsize as usize;
        let width = self.screen_cols.saturating_sub(self.gutter_width());
        let buf = self.buf_mut();
        let cursor_col =
            crate::buffer::display_width(&buf.line(buf.cursor.line), buf.cursor.col, tabsize);
        let left = buf.left_col;
        buf.left_col = if width <= 2 * CUSHION + 1 {
            // Too narrow for a cushioned scroll; just keep the cursor in
            // view.
            cursor_col.saturating_sub(width.saturating_sub(1))
        } else if cursor_col < CUSHION {
            0
        } else if cursor_col < left + CUSHION {
            cursor_col - CUSHION
        } else if cursor_col > left + width - CUSHION - 1 {
            cursor_col + CUSHION + 1 - width
        } else {
            left
        };
    }

    /// Like `scroll_to_cursor`, but when the cursor is off-screen, centers
    /// it in the viewport instead of scrolling just enough to reveal it.
    /// Matches nano's `edit_redraw(..., CENTERING)`, which it uses
    /// specifically for search/find-next/find-previous and replace jumps
    /// (confirmed directly against the installed nano for both) — ordinary
    /// cursor movement (arrows, page up/down, ...) keeps the minimal-scroll
    /// behavior of plain `scroll_to_cursor`.
    pub fn scroll_to_cursor_centered(&mut self) {
        let rows = self.text_rows();
        let buf = self.buf_mut();
        if buf.cursor.line < buf.top_line || buf.cursor.line >= buf.top_line + rows {
            buf.top_line = buf.cursor.line.saturating_sub(rows / 2);
        }
        self.scroll_horizontal_to_cursor();
    }

    /// Keep nano's magic line (see `Buffer::lacks_magic_line`) under the
    /// current buffer's text unless `nonewlines`; run after every event
    /// that might have edited it.
    pub fn ensure_magic_line(&mut self) {
        if !self.options.nonewlines && !self.buffers.is_empty() {
            self.buf_mut().add_magic_line();
        }
    }

    /// Whether the cursor sits at the end of a line with text on it that
    /// is directly above the magic line: nano's do_delete does nothing
    /// there (joining the two would only have the magic line re-added).
    fn at_end_above_magic_line(&self) -> bool {
        let buf = self.buf();
        let len = buf.line(buf.cursor.line).chars().count();
        !self.options.nonewlines
            && len > 0
            && buf.cursor.col == len
            && buf.cursor.line + 2 == buf.line_count()
    }

    fn do_delete(&mut self) {
        if !self.at_end_above_magic_line() {
            self.buf_mut().delete_forward();
        }
    }

    /// nano's do_backspace is a do_left followed by a do_delete, so on
    /// the magic line under a line with text it only moves the cursor.
    fn do_backspace(&mut self) {
        let cursor = self.buf().cursor;
        if cursor.col == 0 && cursor.line > 0 {
            self.buf_mut().move_left();
            if self.at_end_above_magic_line() {
                return;
            }
            self.buf_mut().cursor = cursor;
        }
        self.buf_mut().backspace();
    }

    /// Dispatch one editing action. Returns true if the caller should
    /// re-render (essentially always, but kept for future use).
    pub fn execute(&mut self, action: Action) {
        use Action::*;
        if self.blocked_in_view_mode(action) {
            self.set_status_mild("Key is invalid in view mode");
            return;
        }
        // Any action other than Cut clears nano's "consecutive cuts append
        // to the same cutbuffer" chain.
        if !matches!(action, Cut | CutRestOfFile) {
            self.cut_was_consecutive = false;
        }
        let was_line = self.buffers.get(self.current).map(|b| b.cursor.line);
        match action {
            Help => {
                let lines = crate::help::build(Menu::Main, &self.keymap, self.screen_cols);
                self.mode = Mode::Help {
                    lines,
                    top: 0,
                    cursor: HelpCursor::default(),
                    return_to: None,
                };
            }
            Cancel => {
                self.mode = Mode::Editing;
            }
            Exit => self.begin_exit(),
            WriteOut => self.write_it_out(false, true),
            SaveFile => self.write_it_out(false, false),
            Insert => self.begin_insert(),
            WhereIs => self.begin_search(),
            WhereWas => {
                self.search.backwards = true;
                self.begin_search();
            }
            FindNext => self.repeat_search(false),
            FindPrevious => self.repeat_search(true),
            Replace => self.begin_replace(),
            Cut => self.do_cut(),
            CutRestOfFile => self.do_cut_rest_of_file(),
            Copy => self.do_copy(),
            Paste => self.do_paste(),
            Mark => self.toggle_mark(),
            Location => self.report_location(),
            WordCount => self.report_word_count(),
            Undo => {
                if self.buf_mut().undo() {
                    self.set_status("Undo");
                } else {
                    self.set_status("Nothing to undo");
                }
            }
            Redo => {
                if self.buf_mut().redo() {
                    self.set_status("Redo");
                } else {
                    self.set_status("Nothing to redo");
                }
            }
            Left => self.buf_mut().move_left(),
            Right => self.buf_mut().move_right(),
            Up => self.buf_mut().move_up(),
            Down => self.buf_mut().move_down(),
            Home => self.buf_mut().move_home(),
            End => self.buf_mut().move_end(),
            PrevWord => self.move_prev_word(),
            NextWord => self.move_next_word(),
            PageUp => self.page_up(),
            PageDown => self.page_down(),
            FirstLine => {
                self.buf_mut().cursor = Pos::new(0, 0);
            }
            LastLine => {
                let last = self.buf().line_count().saturating_sub(1);
                self.buf_mut().cursor = Pos::new(last, 0);
            }
            GotoLine => self.begin_goto_line(),
            Tab => self.buf_mut().insert_char('\t'),
            Enter => self.do_enter(),
            Delete => self.do_delete(),
            Backspace => self.do_backspace(),
            Zap => self.do_zap(),
            ChopWordLeft => self.chop_word_left(),
            ChopWordRight => self.chop_word_right(),
            Complete => self.set_status("complete: not yet implemented"),
            Justify => self.run_justify(false),
            FullJustify => self.run_justify(true),
            Indent => self.do_indent(),
            Unindent => self.do_unindent(),
            Comment => self.do_comment(),
            Center => self.set_status("center: not yet implemented"),
            Cycle => self.set_status("cycle: not yet implemented"),
            ScrollUp => self.scroll_view(-1),
            ScrollDown => self.scroll_view(1),
            BeginPara => self.move_para_begin(),
            EndPara => self.move_para_end(),
            PrevBlock | NextBlock | TopRow | BottomRow => {
                self.set_status("block navigation: not yet implemented");
            }
            FindBracket => self.set_status("find-bracket: not yet implemented"),
            Anchor | PrevAnchor | NextAnchor => self.set_status("anchors: not yet implemented"),
            PrevBuf => self.switch_buffer(-1),
            NextBuf => self.switch_buffer(1),
            Verbatim => self.set_status("verbatim input: not yet implemented"),
            RecordMacro | RunMacro => self.set_status("macros: not yet implemented"),
            Refresh => {}
            SuggestSuspend => self.suggest_ctrl_t_ctrl_z(),
            Execute => self.begin_execute(),
            // Speller/Formatter/Linter need to run an external process (and,
            // for the alt-speller/formatter, hand the terminal over to it),
            // and Suspend hands the terminal back to the shell outright --
            // which this UI-agnostic dispatcher can't do — ui.rs's
            // apply_binding/apply_prompt_action intercept them before they
            // would ever reach here.
            Speller | Formatter | Linter | Suspend => {}
            NoHelp => self.options.nohelp = !self.options.nohelp,
            Zero => self.options.zero = !self.options.zero,
            ConstantShow => self.options.constantshow = !self.options.constantshow,
            SoftWrap => self.options.softwrap = !self.options.softwrap,
            LineNumbers => self.options.linenumbers = !self.options.linenumbers,
            WhitespaceDisplay => {
                self.options.whitespacedisplay = !self.options.whitespacedisplay;
                // nano's do_toggle reports every flag flip this way; tico
                // does so for this one (the others are still silent).
                self.set_status(if self.options.whitespacedisplay {
                    "Whitespace display enabled"
                } else {
                    "Whitespace display disabled"
                });
            }
            NoSyntax => self.options.syntax_highlighting = !self.options.syntax_highlighting,
            SmartHome => self.options.smarthome = !self.options.smarthome,
            AutoIndent => self.options.autoindent = !self.options.autoindent,
            CutFromCursor => self.options.cutfromcursor = !self.options.cutfromcursor,
            BreakLongLines => self.options.breaklonglines = !self.options.breaklonglines,
            TabsToSpaces => self.options.tabstospaces = !self.options.tabstospaces,
            Mouse => self.options.mouse = !self.options.mouse,
            CaseSens => self.search.case_sensitive = !self.search.case_sensitive,
            Regexp => self.search.use_regex = !self.search.use_regex,
            Backwards => self.search.backwards = !self.search.backwards,
            _ => {}
        }
        // An action (e.g. Exit with no unsaved changes) may have just
        // closed the last buffer and set Mode::Quit; nothing left to
        // scroll in that case.
        if !self.buffers.is_empty() {
            // nano resets its "last line too" flag whenever the current
            // line changes (see the end of its `process_a_keystroke`).
            if Some(self.buf().cursor.line) != was_line {
                self.also_the_last = false;
            }
            // `ScrollUp`/`ScrollDown` (`M-Up`/`M-Down`, `M--`/`M-+`, the
            // mouse wheel) exist specifically to slide the viewport while
            // "keeping the cursor in the same text position" (nanorc(5));
            // an unconditional scroll_to_cursor() here would immediately
            // clamp `top_line` right back, since the cursor itself never
            // moves for these two.
            if !matches!(action, ScrollUp | ScrollDown) {
                self.scroll_to_cursor();
            }
        }
    }

    /// Rewrite this buffer's lock file (if any) with the "modified" flag
    /// set, the first time it becomes modified in this session — matching
    /// nano's `set_modified()`, which does the same only on the
    /// false->true transition rather than on every keystroke. Called once
    /// per keystroke handled in the main edit window, since edits happen
    /// via several different paths (`execute()`'s actions, plain character
    /// self-insertion, ...).
    pub fn maybe_update_lock_modified_flag(&mut self) {
        if self.buffers.is_empty() {
            // The action just closed the last buffer (e.g. Exit with no
            // unsaved changes), which already set Mode::Quit; nothing left
            // to update.
            return;
        }
        let target = self.buf().path.as_ref().map(|p| p.display().to_string());
        let buf = self.buf_mut();
        if buf.modified
            && !buf.lock_modified_written
            && let (Some(lock), Some(target)) = (&buf.lock_filename, target)
        {
            let _ = crate::lockfile::write_lock(lock, &target, true);
            buf.lock_modified_written = true;
        }
    }

    /// Close the current buffer (deleting its lock file, if any) and, if it
    /// was the last one, quit — matching nano's normal `close_and_go()`.
    pub fn close_current_buffer(&mut self) {
        if let Some(lock) = self.buf_mut().lock_filename.take() {
            crate::lockfile::delete_lock(&lock);
        }
        self.buffers.remove(self.current);
        if self.buffers.is_empty() {
            self.mode = Mode::Quit;
        } else if self.current >= self.buffers.len() {
            self.current = self.buffers.len() - 1;
        }
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.do_enter();
            return;
        }
        if c == '\t' && self.options.tabstospaces {
            let n = self.options.tabsize as usize;
            for _ in 0..n {
                self.buf_mut().insert_char(' ');
            }
        } else {
            self.buf_mut().insert_char(c);
        }
    }

    fn do_enter(&mut self) {
        let indent = if self.options.autoindent {
            let line = self.buf().line(self.buf().cursor.line);
            line.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect::<String>()
        } else {
            String::new()
        };
        self.buf_mut().insert_char('\n');
        if !indent.is_empty() {
            self.buf_mut().insert_str(&indent);
        }
        self.also_the_last = false;
    }

    /// nano's `get_range()`: the lines an indent/unindent/comment acts on
    /// -- just the cursor's line without a mark, otherwise every line the
    /// marked region touches. A region ending exactly at column 0 of a
    /// later line doesn't reach into that line, so it's left out... unless
    /// a previous such action already included it (`also_the_last`), which
    /// keeps the set of lines stable across repeated presses even as the
    /// cursor's column shifts.
    fn marked_line_range(&mut self) -> (usize, usize) {
        let Some((start, end)) = self.selection_range() else {
            let line = self.buf().cursor.line;
            return (line, line);
        };
        if end.col == 0 && end.line != start.line && !self.also_the_last {
            (start.line, end.line - 1)
        } else {
            self.also_the_last = true;
            (start.line, end.line)
        }
    }

    /// `M-}` Indent: matches nano's `do_indent` -- prefix each non-empty
    /// line of the range with one tab (or `tabsize` spaces under
    /// `tabstospaces`), as a single undo step. Empty lines are left alone,
    /// and if every line is empty nothing happens at all (no message). The
    /// cursor and mark shift right with their line's text, except when
    /// sitting at column 0.
    fn do_indent(&mut self) {
        let (mut top, bot) = self.marked_line_range();
        while top <= bot && self.buf().line(top).is_empty() {
            top += 1;
        }
        if top > bot {
            return;
        }
        let indentation = if self.options.tabstospaces {
            " ".repeat(self.options.tabsize.max(1) as usize)
        } else {
            "\t".to_string()
        };
        let indent_len = indentation.chars().count();

        let mut new_lines = Vec::with_capacity(bot - top + 1);
        let mut indented = Vec::with_capacity(bot - top + 1);
        for i in top..=bot {
            let line = self.buf().line(i);
            indented.push(!line.is_empty());
            if line.is_empty() {
                new_lines.push(line);
            } else {
                new_lines.push(format!("{indentation}{line}"));
            }
        }
        let shifted = |p: Pos| {
            if (top..=bot).contains(&p.line) && p.col > 0 && indented[p.line - top] {
                Pos::new(p.line, p.col + indent_len)
            } else {
                p
            }
        };
        let cursor_after = shifted(self.buf().cursor);
        let mark_after = self.buf().mark.map(shifted);
        self.buf_mut()
            .replace_lines(top, bot, &new_lines.join("\n"), cursor_after);
        self.buf_mut().mark = mark_after;
        self.shift_held = true;
    }

    /// `M-{`/`Shift-Tab` Unindent: matches nano's `do_unindent` -- strip
    /// one tab's worth of leading whitespace (see `length_of_white`) from
    /// each line of the range that has any, as a single undo step. If no
    /// line has any, nothing happens at all (no message). The cursor and
    /// mark shift left with their line's text, stopping at column 0.
    fn do_unindent(&mut self) {
        let tabsize = self.options.tabsize.max(1) as usize;
        let (mut top, bot) = self.marked_line_range();
        while top <= bot && length_of_white(&self.buf().line(top), tabsize) == 0 {
            top += 1;
        }
        if top > bot {
            return;
        }

        let mut new_lines = Vec::with_capacity(bot - top + 1);
        let mut removed = Vec::with_capacity(bot - top + 1);
        for i in top..=bot {
            let line = self.buf().line(i);
            let n = length_of_white(&line, tabsize);
            removed.push(n);
            new_lines.push(line.chars().skip(n).collect::<String>());
        }
        let shifted = |p: Pos| {
            if (top..=bot).contains(&p.line) {
                Pos::new(p.line, p.col.saturating_sub(removed[p.line - top]))
            } else {
                p
            }
        };
        let cursor_after = shifted(self.buf().cursor);
        let mark_after = self.buf().mark.map(shifted);
        self.buf_mut()
            .replace_lines(top, bot, &new_lines.join("\n"), cursor_after);
        self.buf_mut().mark = mark_after;
        self.shift_held = true;
    }

    /// `M-3` Comment/Uncomment: matches nano's `do_comment`. The comment
    /// sequence is the language's (`LanguageDef::comment`; `#` with no
    /// language; a `PREFIX|POSTFIX` pair brackets the line). If any
    /// non-blank line in the range isn't already commented -- or all of
    /// them are blank -- every line gets commented; otherwise the commented
    /// ones get uncommented. The buffer's last line (nano's "magic line")
    /// is never touched unless `nonewlines` is set, and selecting only it
    /// is refused. One undo step; cursor and mark shift with their text.
    fn do_comment(&mut self) {
        let comment_seq = self.buf().language.map(|l| l.comment).unwrap_or("#");
        if comment_seq.is_empty() {
            self.set_status_mild("Commenting is not supported for this file type");
            return;
        }
        let (pre, post) = comment_seq.split_once('|').unwrap_or((comment_seq, ""));
        let pre_len = pre.chars().count();

        let (top, bot) = self.marked_line_range();
        let filebot = self.buf().line_count().saturating_sub(1);
        let protect_last = !self.options.nonewlines;
        if top == bot && bot == filebot && protect_last {
            self.set_status_mild("Cannot comment past end of file");
            return;
        }
        let untouchable = |i: usize| protect_last && i == filebot;

        // Comment everything unless every non-blank line is already
        // commented (and there is at least one non-blank line).
        let mut add = false;
        let mut all_blank = true;
        for i in top..=bot {
            let line = self.buf().line(i);
            let blank = line.chars().all(|c| c == ' ' || c == '\t' || c == '\r');
            if !blank && (untouchable(i) || !is_commented(&line, pre, post)) {
                add = true;
                break;
            }
            all_blank &= blank;
        }
        let add = add || all_blank;

        let mut new_lines = Vec::with_capacity(bot - top + 1);
        let mut changed = Vec::with_capacity(bot - top + 1);
        for i in top..=bot {
            let line = self.buf().line(i);
            if untouchable(i) {
                changed.push(false);
                new_lines.push(line);
            } else if add {
                changed.push(true);
                new_lines.push(format!("{pre}{line}{post}"));
            } else if is_commented(&line, pre, post) {
                changed.push(true);
                let inner: String = line.chars().skip(pre_len).collect();
                let keep = inner.chars().count() - post.chars().count();
                new_lines.push(inner.chars().take(keep).collect());
            } else {
                changed.push(false);
                new_lines.push(line);
            }
        }
        if !changed.iter().any(|&c| c) {
            return;
        }
        let shifted = |p: Pos| {
            if !(top..=bot).contains(&p.line) || !changed[p.line - top] {
                return p;
            }
            let col = if add {
                if p.col > 0 { p.col + pre_len } else { 0 }
            } else {
                p.col.saturating_sub(pre_len)
            };
            // A removed postfix can leave a column past the new end.
            Pos::new(p.line, col.min(new_lines[p.line - top].chars().count()))
        };
        let cursor_after = shifted(self.buf().cursor);
        let mark_after = self.buf().mark.map(shifted);
        self.buf_mut()
            .replace_lines(top, bot, &new_lines.join("\n"), cursor_after);
        self.buf_mut().mark = mark_after;
        self.shift_held = true;
    }

    /// Plain `^Z` in the main menu: nano's `suggest_ctrlT_ctrlZ`. Tells the
    /// user how suspension actually works -- but only while the keys the
    /// hint names still do that (`^T` is Execute in the main menu and `^Z`
    /// is Suspend in the Execute menu); with either rebound nano says
    /// nothing rather than give a wrong hint. AHEM-level in nano: the
    /// error coloring, no bell.
    fn suggest_ctrl_t_ctrl_z(&mut self) {
        use crate::keymap::{Binding, Key};
        let ctrl_t_executes = matches!(
            self.keymap.lookup(Menu::Main, Key::Ctrl('T')),
            Some(Binding::Action(Action::Execute))
        );
        let ctrl_z_suspends = matches!(
            self.keymap.lookup(Menu::Execute, Key::Ctrl('Z')),
            Some(Binding::Action(Action::Suspend))
        );
        if ctrl_t_executes && ctrl_z_suspends {
            self.set_status_mild("To suspend, type ^T^Z");
        }
    }

    /// The marked region, normalized to (earlier, later) regardless of
    /// which end the mark or the cursor is on -- `None` when no mark is
    /// set. `pub(crate)` so the renderer can highlight it, not just the
    /// tools (Cut/Copy/Speller/...) that already act on it.
    pub(crate) fn selection_range(&self) -> Option<(Pos, Pos)> {
        let buf = self.buf();
        buf.mark.map(|m| {
            if (m.line, m.col) <= (buf.cursor.line, buf.cursor.col) {
                (m, buf.cursor)
            } else {
                (buf.cursor, m)
            }
        })
    }

    /// Whether `--view`/`set view` should block `action` — matches nano's
    /// `ISSET(VIEW_MODE) && changes_something(function)` check. `pub(crate)`
    /// so ui.rs's Speller/Formatter interception (which never reaches
    /// `execute()`, since those need real terminal I/O) can honor it too.
    pub(crate) fn blocked_in_view_mode(&self, action: Action) -> bool {
        self.options.view && action_changes_something(action)
    }

    /// What the spell checker and formatter operate on: the marked
    /// selection if one is active, otherwise the whole buffer — matching
    /// nano's own `write_region_to_file`/`write_file` choice in `do_spell`.
    pub(crate) fn tool_input_text(&self) -> String {
        match self.selection_range() {
            Some((start, end)) => self.buf().text_range(start, end),
            None => self.buf().to_string(),
        }
    }

    /// Replace whatever `tool_input_text` returned with `new_text`,
    /// matching nano's `replace_buffer` (used by `treat()` for the
    /// alt-speller and the formatter).
    pub(crate) fn replace_tool_input(&mut self, new_text: &str) {
        let range = self.selection_range();
        let start = match range {
            Some((start, end)) => {
                self.buf_mut().delete_range(start, end);
                start
            }
            None => {
                let last_line = self.buf().line_count().saturating_sub(1);
                let last_col = self.buf().line(last_line).chars().count();
                self.buf_mut()
                    .delete_range(Pos::new(0, 0), Pos::new(last_line, last_col));
                Pos::new(0, 0)
            }
        };
        self.buf_mut().cursor = start;
        self.buf_mut().insert_str(new_text);
        self.buf_mut().modified = true;
        self.scroll_to_cursor();
    }

    /// `^J` Justify (one paragraph) / `M-J` Full Justify (the whole
    /// buffer) — matches nano's `justify_text`. A marked region, when
    /// present, always wins over either of those (nano's own "treat all
    /// marked text as one paragraph" behavior), regardless of which key
    /// was pressed.
    fn run_justify(&mut self, whole_buffer: bool) {
        if let Some((start, end)) = self.selection_range() {
            self.run_justify_selection(start, end);
            return;
        }
        let quote_re = regex::Regex::new(&self.options.quotestr).ok();
        let quote_re = quote_re.as_ref();
        let wrap_at = crate::justify::wrap_at(self.options.fill, self.screen_cols);
        let punct = self.options.punct.clone();
        let brackets = self.options.brackets.clone();
        let trim_blanks = self.options.trimblanks;

        let lines = self.char_lines();

        if whole_buffer {
            let mut result = lines;
            let mut search_start = 0;
            let mut touched = false;
            while let Some((start, count)) =
                crate::justify::find_paragraph(&result, search_start, quote_re)
            {
                let new_lines = crate::justify::justify_paragraph(
                    &result,
                    start,
                    count,
                    quote_re,
                    &punct,
                    &brackets,
                    wrap_at,
                    trim_blanks,
                );
                let new_count = new_lines.len();
                result.splice(start..start + count, new_lines);
                search_start = start + new_count;
                touched = true;
            }
            if !touched {
                self.set_status("Nothing to justify");
                return;
            }
            let was_line = self.buf().cursor.line;
            let new_text = join_lines(&result);
            self.replace_tool_input(&new_text);
            let target = was_line.min(self.buf().line_count().saturating_sub(1));
            self.buf_mut().cursor = Pos::new(target, 0);
            self.scroll_to_cursor();
            self.set_status("Justified file");
        } else {
            let cursor_line = self.buf().cursor.line;
            let search_start = if crate::justify::in_mid_paragraph(&lines, cursor_line, quote_re) {
                crate::justify::para_begin(&lines, cursor_line, quote_re)
            } else {
                cursor_line
            };
            let Some((start, count)) =
                crate::justify::find_paragraph(&lines, search_start, quote_re)
            else {
                self.set_status("Nothing to justify");
                return;
            };
            let new_lines = crate::justify::justify_paragraph(
                &lines,
                start,
                count,
                quote_re,
                &punct,
                &brackets,
                wrap_at,
                trim_blanks,
            );
            let new_count = new_lines.len();
            let new_text = join_lines(&new_lines);

            let end_line = start + count - 1;
            let end_col = self.buf().line(end_line).chars().count();
            self.buf_mut()
                .delete_range(Pos::new(start, 0), Pos::new(end_line, end_col));
            self.buf_mut().cursor = Pos::new(start, 0);
            self.buf_mut().insert_str(&new_text);
            self.buf_mut().modified = true;

            // Matches nano: the cursor ends up on whatever line follows
            // the justified paragraph (often a blank separator line), not
            // on the paragraph's own last line -- `justify_text` extends
            // the cut region one line further before re-pasting it, which
            // is where this comes from.
            let next_line = start + new_count;
            if next_line < self.buf().line_count() {
                self.buf_mut().cursor = Pos::new(next_line, 0);
            } else {
                let last_line = self.buf().line_count().saturating_sub(1);
                let last_col = self.buf().line(last_line).chars().count();
                self.buf_mut().cursor = Pos::new(last_line, last_col);
            }
            self.scroll_to_cursor();
            self.set_status("Justified paragraph");
        }
    }

    /// `^J`/`M-J` with a marked region: nano's "treat all marked text as
    /// one paragraph" (Pico behavior) — justifies the marked lines as a
    /// single unit regardless of paragraph boundaries within them. This
    /// snaps to whole lines rather than replicating nano's exact mid-line
    /// lead-trimming and its backward search past the selection's start
    /// for the "true" paragraph beginning; for a selection that already
    /// starts/ends at a paragraph's own boundaries (the common case) the
    /// result is identical.
    fn run_justify_selection(&mut self, start: Pos, end: Pos) {
        if start == end {
            self.set_status_mild("Selection is empty");
            return;
        }
        let quote_re = regex::Regex::new(&self.options.quotestr).ok();
        let quote_re = quote_re.as_ref();
        let wrap_at = crate::justify::wrap_at(self.options.fill, self.screen_cols);
        let punct = self.options.punct.clone();
        let brackets = self.options.brackets.clone();
        let trim_blanks = self.options.trimblanks;

        // A selection ending right at column 0 doesn't reach into that
        // line, so treat the line above as the last one covered.
        let end_line = if end.col == 0 && end.line > start.line {
            end.line - 1
        } else {
            end.line
        };
        let count = end_line - start.line + 1;

        let lines: Vec<Vec<char>> = (0..self.buf().line_count())
            .map(|i| self.buf().line(i).chars().collect())
            .collect();
        let new_lines = crate::justify::justify_paragraph(
            &lines,
            start.line,
            count,
            quote_re,
            &punct,
            &brackets,
            wrap_at,
            trim_blanks,
        );
        let new_count = new_lines.len();
        let new_text = join_lines(&new_lines);

        let end_col = self.buf().line(end_line).chars().count();
        self.buf_mut()
            .delete_range(Pos::new(start.line, 0), Pos::new(end_line, end_col));
        self.buf_mut().cursor = Pos::new(start.line, 0);
        self.buf_mut().insert_str(&new_text);
        self.buf_mut().modified = true;
        self.buf_mut().mark = None;
        self.buf_mut().softmark = false;

        let final_line = start.line + new_count - 1;
        let final_col = self.buf().line(final_line).chars().count();
        self.buf_mut().cursor = Pos::new(final_line, final_col);
        self.scroll_to_cursor();
        self.set_status("Justified selection");
    }

    /// `^^`/`M-A` Set Mark: matches nano's `do_mark` -- toggles a *hard*
    /// mark (as opposed to the "soft" one Shift+movement sets), which
    /// persists until toggled off again rather than being cleared by the
    /// next plain movement.
    fn toggle_mark(&mut self) {
        let cur = self.buf().cursor;
        if self.buf().mark.is_some() {
            self.buf_mut().mark = None;
            self.buf_mut().softmark = false;
            self.set_status("Mark Unset");
        } else {
            self.buf_mut().mark = Some(cur);
            self.buf_mut().softmark = false;
            self.set_status("Mark Set");
        }
    }

    fn do_cut(&mut self) {
        if let Some((start, end)) = self.selection_range() {
            let text = self.buf_mut().delete_range(start, end);
            self.cutbuffer = text;
            self.buf_mut().mark = None;
        } else {
            let line = self.buf().cursor.line;
            let line_len = self.buf().line(line).chars().count();
            let has_next = line + 1 < self.buf().line_count();
            let end = if has_next {
                Pos::new(line + 1, 0)
            } else {
                Pos::new(line, line_len)
            };
            let start = Pos::new(line, 0);
            let text = self.buf_mut().delete_range(start, end);
            if self.cut_was_consecutive {
                self.cutbuffer.push_str(&text);
            } else {
                self.cutbuffer = text;
            }
        }
        self.cut_was_consecutive = true;
        self.set_status("Cut");
    }

    fn do_cut_rest_of_file(&mut self) {
        let start = self.buf().cursor;
        let last_line = self.buf().line_count().saturating_sub(1);
        let end = Pos::new(last_line, self.buf().line(last_line).chars().count());
        self.cutbuffer = self.buf_mut().delete_range(start, end);
        self.set_status("Cut to end of file");
    }

    fn do_copy(&mut self) {
        if let Some((start, end)) = self.selection_range() {
            self.cutbuffer = self.buf().text_range(start, end);
        } else {
            let line = self.buf().cursor.line;
            self.cutbuffer = format!("{}\n", self.buf().line(line));
        }
        self.set_status("Copied");
    }

    fn do_paste(&mut self) {
        if self.cutbuffer.is_empty() {
            return;
        }
        let text = self.cutbuffer.clone();
        self.buf_mut().insert_str(&text);
        self.set_status("Pasted");
    }

    fn do_zap(&mut self) {
        if let Some((start, end)) = self.selection_range() {
            self.buf_mut().delete_range(start, end);
            self.buf_mut().mark = None;
            self.set_status("Zapped");
        } else {
            let line = self.buf().cursor.line;
            let has_next = line + 1 < self.buf().line_count();
            let start = Pos::new(line, 0);
            let end = if has_next {
                Pos::new(line + 1, 0)
            } else {
                Pos::new(line, self.buf().line(line).chars().count())
            };
            self.buf_mut().delete_range(start, end);
        }
    }

    fn chop_word_left(&mut self) {
        let end = self.buf().cursor;
        let start = word_left_pos(self.buf(), end);
        self.buf_mut().delete_range(start, end);
    }

    fn chop_word_right(&mut self) {
        let start = self.buf().cursor;
        let end = word_right_pos(self.buf(), start);
        self.buf_mut().delete_range(start, end);
    }

    fn move_prev_word(&mut self) {
        let pos = word_left_pos(self.buf(), self.buf().cursor);
        self.buf_mut().cursor = pos;
    }

    fn move_next_word(&mut self) {
        let pos = word_right_pos(self.buf(), self.buf().cursor);
        self.buf_mut().cursor = pos;
    }

    /// The buffer as the `justify` module's line vectors.
    fn char_lines(&self) -> Vec<Vec<char>> {
        (0..self.buf().line_count())
            .map(|i| self.buf().line(i).chars().collect())
            .collect()
    }

    /// nano's `to_para_begin` (`M-(`, `M-9`; `^W` in the Go To Line
    /// prompt): to the first line of the paragraph the cursor is in, or
    /// of the previous one when already on that line. A paragraph here
    /// is what justify would treat as one, `quotestr` included. nano
    /// redraws with CENTERING, so an off-screen landing is centered.
    pub fn move_para_begin(&mut self) {
        let quote_re = regex::Regex::new(&self.options.quotestr).ok();
        let lines = self.char_lines();
        let from = self.buf().cursor.line;
        let line = crate::justify::para_begin(&lines, from, quote_re.as_ref());
        self.buf_mut().cursor = Pos::new(line, 0);
        self.scroll_to_cursor_centered();
    }

    /// nano's `to_para_end` (`M-)`, `M-0`; `^O` in the Go To Line
    /// prompt): to just beyond the end of the paragraph the cursor is in
    /// or before, i.e. the start of the line after it; or the end of the
    /// buffer's last line when that is where the paragraph ends.
    pub fn move_para_end(&mut self) {
        let quote_re = regex::Regex::new(&self.options.quotestr).ok();
        let lines = self.char_lines();
        let from = self.buf().cursor.line;
        let line = crate::justify::para_end(&lines, from, quote_re.as_ref());
        let cursor = if line + 1 < lines.len() {
            Pos::new(line + 1, 0)
        } else {
            Pos::new(line, lines[line].len())
        };
        self.buf_mut().cursor = cursor;
        self.scroll_to_cursor_centered();
    }

    fn page_up(&mut self) {
        let rows = self.text_rows();
        for _ in 0..rows {
            self.buf_mut().move_up();
        }
    }

    fn page_down(&mut self) {
        let rows = self.text_rows();
        for _ in 0..rows {
            self.buf_mut().move_down();
        }
    }

    fn scroll_view(&mut self, delta: isize) {
        let buf = self.buf_mut();
        if delta < 0 {
            buf.top_line = buf.top_line.saturating_sub((-delta) as usize);
        } else {
            buf.top_line = buf.top_line.saturating_add(delta as usize);
        }
    }

    fn switch_buffer(&mut self, delta: isize) {
        let n = self.buffers.len() as isize;
        if n <= 1 {
            return;
        }
        let cur = self.current as isize;
        self.current = ((cur + delta).rem_euclid(n)) as usize;
        self.note_buffer_linecount();
    }

    /// Set `minibar_note` to the current buffer's line count (plus a
    /// `DOS`/`Mac` tag when applicable), for `set minibar`'s one-shot
    /// display right after a load, a save, or a buffer switch -- matches
    /// nano's `report_size = TRUE` plus the wording `minibar()` builds from
    /// it, confirmed against the installed nano's own escape-code output
    /// (singular "line" and the format tag only show up when they apply).
    pub fn note_buffer_linecount(&mut self) {
        let buf = self.buf();
        self.minibar_note = Some(minibar_linecount_note(buf.nano_line_count(), buf.format));
    }

    fn report_location(&mut self) {
        let buf = self.buf();
        self.set_status(format!(
            "line {}/{}, col {}",
            buf.cursor.line + 1,
            buf.line_count(),
            buf.cursor.col + 1
        ));
    }

    fn report_word_count(&mut self) {
        let text = self.buf().to_string();
        let words = text.split_whitespace().count();
        let lines = self.buf().line_count();
        let chars = text.chars().count();
        self.set_status(format!("{lines} lines, {words} words, {chars} characters"));
    }

    fn begin_search(&mut self) {
        // The last search term is shown as a bracketed default in the
        // label (see search_prompt_label), not pre-filled into the input -
        // confirmed against the installed nano, which leaves the field
        // empty and reuses the bracketed default only if Enter is pressed
        // with nothing typed.
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::WhereIs,
            menu: Menu::Search,
            label: search_prompt_label("Search", "", &self.search),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    fn begin_replace(&mut self) {
        let suffix = if self.buf().mark.is_some() {
            " (to replace) in selection"
        } else {
            " (to replace)"
        };
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::Replace1,
            menu: Menu::Replace,
            label: search_prompt_label("Search", suffix, &self.search),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    /// `^R` Read File. `new_buffer` starts from `set multibuffer` each time
    /// this prompt opens (matching nano: the `M-F` toggle only lives for
    /// the duration of one prompt, not permanently).
    fn begin_insert(&mut self) {
        let new_buffer = self.options.multibuffer;
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::InsertFile {
                new_buffer,
                execute: false,
            },
            menu: Menu::Insert,
            label: insert_prompt_label(new_buffer, false, self.options.noconvert),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    /// `^T` Execute Command: the same prompt as `^R`, just starting in
    /// "run a command" mode (nano's `do_execute` -> `insert_a_file_or(TRUE)`,
    /// versus `do_insertfile` -> `insert_a_file_or(FALSE)`).
    fn begin_execute(&mut self) {
        let new_buffer = self.options.multibuffer;
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::InsertFile {
                new_buffer,
                execute: true,
            },
            menu: Menu::Execute,
            label: insert_prompt_label(new_buffer, true, self.options.noconvert),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    fn begin_goto_line(&mut self) {
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::GotoLine,
            menu: Menu::GotoLine,
            label: "Enter line number, column number".to_string(),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    /// Start nano's `write_it_out(exiting, withprompt)`: `^O` is
    /// (false, true), `^S` (false, false), and `^X`'s save (true, true).
    fn write_it_out(&mut self, exiting: bool, withprompt: bool) {
        // A marked region offers to write just the selection instead of
        // the whole buffer, and (to reduce the chance of clobbering the
        // real file with just a fragment) starts with a blank filename
        // rather than defaulting to the current one — matches nano, and
        // only applies outside of the exit-time save prompt.
        let given = if !exiting && self.buf().mark.is_some() {
            String::new()
        } else {
            self.buf()
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        };
        let flow = WriteFlow {
            exiting,
            withprompt,
            maychange: self.buf().path.is_none(),
        };
        self.prompt_for_write(flow, given);
    }

    /// The top of `write_it_out`'s loop: without a prompt to show (`^S`,
    /// or `saveonexit` when exiting) a named buffer goes straight on under
    /// its own name; otherwise ask for the filename, offering `given`.
    fn prompt_for_write(&mut self, flow: WriteFlow, given: String) {
        if (!flow.withprompt || (self.options.saveonexit && flow.exiting))
            && let Some(path) = self.buf().path.clone()
        {
            self.check_write_answer(path.display().to_string(), flow);
            return;
        }
        let label = self.writeout_prompt_label(flow.exiting);
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::WriteOut { flow },
            menu: Menu::WriteOut,
            label,
            cursor: given.chars().count(),
            input: given,
            history_pos: None,
            saved_input: None,
        });
    }

    /// Enter at the Write Out prompt. An empty answer cancels, as nano's
    /// `do_prompt` returns -2 for it.
    pub(crate) fn submit_write_answer(&mut self, answer: String, flow: WriteFlow) {
        self.mode = Mode::Editing;
        if answer.is_empty() {
            self.set_status("Cancelled");
            return;
        }
        self.check_write_answer(answer, flow);
    }

    /// `write_it_out`'s checks on a settled filename: a name other than
    /// the buffer's own needs "Save file under DIFFERENT NAME?" (unless
    /// writing a selection) and, when taken, "File exists; OVERWRITE?"; the
    /// buffer's own name needs "continue saving?" when the file changed on
    /// disk since it was read or written.
    fn check_write_answer(&mut self, answer: String, flow: WriteFlow) {
        let expanded = crate::fileio::expand_leading_tilde(&answer);
        let path = std::path::Path::new(&expanded);
        let name_exists = std::fs::metadata(path).is_ok();
        let do_warning = match &self.buf().path {
            None => name_exists,
            Some(own) => crate::fileio::full_path(path) != crate::fileio::full_path(own),
        };
        if do_warning {
            if !flow.maychange && (flow.exiting || self.buf().mark.is_none()) {
                self.ask_write_question(WriteQuestion::DifferentName, answer, flow);
            } else if name_exists {
                self.ask_write_question(WriteQuestion::Overwrite, answer, flow);
            } else {
                self.finish_write(&answer, flow);
            }
        } else if name_exists
            && self
                .buf()
                .disk_state
                .as_ref()
                .is_some_and(|known| crate::fileio::changed_on_disk_since(known, path))
        {
            self.brief_warning = Some("File on disk has changed".to_string());
            self.ask_write_question(WriteQuestion::DiskChanged, answer, flow);
        } else {
            self.finish_write(&answer, flow);
        }
    }

    fn ask_write_question(&mut self, question: WriteQuestion, answer: String, flow: WriteFlow) {
        let label = match question {
            WriteQuestion::DifferentName => "Save file under DIFFERENT NAME? ".to_string(),
            WriteQuestion::Overwrite => {
                // nano's crop_to_fit(answer, COLS - breadth(question) + 1),
                // where the question's "%s" counts as two columns.
                let room = self
                    .screen_cols
                    .saturating_sub(overwrite_question("").chars().count() + 1);
                overwrite_question(&crop_to_fit(&answer, room))
            }
            WriteQuestion::DiskChanged => {
                "File was modified since you opened it; continue saving? ".to_string()
            }
        };
        self.mode = Mode::Prompt(Prompt {
            kind: PromptKind::WriteConfirm {
                question,
                answer,
                flow,
            },
            menu: Menu::YesNo,
            label,
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
    }

    /// Act on the answer to a `WriteConfirm` question, following
    /// `write_it_out` -- a "No" to either name question reshows the Write
    /// Out prompt with the name as typed (nano's `continue`).
    pub(crate) fn answer_write_question(
        &mut self,
        question: WriteQuestion,
        answer: String,
        mut flow: WriteFlow,
        choice: YesNo,
    ) {
        self.mode = Mode::Editing;
        match question {
            WriteQuestion::DifferentName => {
                if choice != YesNo::Yes {
                    return self.prompt_for_write(flow, answer);
                }
                flow.maychange = true;
                if std::fs::metadata(crate::fileio::expand_leading_tilde(&answer)).is_ok() {
                    self.ask_write_question(WriteQuestion::Overwrite, answer, flow);
                } else {
                    self.finish_write(&answer, flow);
                }
            }
            WriteQuestion::Overwrite => {
                if choice != YesNo::Yes {
                    return self.prompt_for_write(flow, answer);
                }
                self.finish_write(&answer, flow);
            }
            // nano's `write_it_out` returns 0 (failure), 1 (success) or 2
            // (discard the buffer); `do_exit` closes the buffer on 1 or 2,
            // `do_writeout`/`do_savefile` only on 2.
            WriteQuestion::DiskChanged => {
                if self.options.saveonexit && flow.withprompt {
                    // In "tool mode": Yes overwrites without updating the
                    // buffer's bookkeeping (nano's NONOTES), No discards
                    // the buffer, Cancel does nothing.
                    match choice {
                        YesNo::Yes => {
                            if self.write_buffer_plainly(&answer) && flow.exiting {
                                self.close_current_buffer();
                            }
                        }
                        YesNo::No => self.close_current_buffer(),
                        YesNo::Cancel => {}
                    }
                } else if choice == YesNo::Cancel && flow.exiting {
                    self.prompt_for_write(flow, answer);
                } else if choice != YesNo::Yes {
                    // Not writing still counts as success, so ^X then
                    // closes the buffer unsaved.
                    if flow.exiting {
                        self.close_current_buffer();
                    }
                } else {
                    self.finish_write(&answer, flow);
                }
            }
        }
    }

    /// The end of `write_it_out`: the marked region when one was prompted
    /// for (and not exiting), else the whole buffer.
    fn finish_write(&mut self, answer: &str, flow: WriteFlow) {
        // nano's write_file expands a leading ~ or ~user.
        let path = std::path::PathBuf::from(crate::fileio::expand_leading_tilde(answer));
        // A marked region writes just the selection to `path` as a
        // standalone file -- it doesn't touch the current buffer's own
        // path/modified/disk-state, matching nano's write_region_to_file,
        // and doesn't clear the mark (confirmed against the installed
        // nano: the selection stays highlighted afterward).
        if flow.withprompt
            && !flow.exiting
            && let Some((start, end)) = self.selection_range()
        {
            let mut selected = self.buf().text_range(start, end);
            // nano's write_region_to_file: a region that ends partway into
            // a line gets an empty line after it (so the file ends with a
            // newline) unless `nonewlines` -- and, like any write, line
            // breaks in the buffer's own format.
            if end.col > 0 && !self.options.nonewlines {
                selected.push('\n');
            }
            let bytes = crate::fileio::with_line_breaks(selected.clone(), self.buf().format);
            match std::fs::write(&path, bytes) {
                Ok(()) => {
                    self.set_status(wrote_lines(crate::fileio::nano_style_line_count(&selected)))
                }
                Err(e) => self.set_status_alert(format!("Error writing {}: {e}", path.display())),
            }
            return;
        }
        self.write_buffer_to(&path, flow.exiting);
    }

    /// nano's `write_file(..., NONOTES)`: write the whole buffer to `path`
    /// without marking it saved or renaming it. tico still refreshes the
    /// buffer's disk snapshot, which only its own external-change watcher
    /// reads, so that watcher doesn't then report tico's own write.
    fn write_buffer_plainly(&mut self, path: &str) -> bool {
        let path = std::path::Path::new(path);
        match std::fs::write(path, crate::fileio::serialized(self.buf())) {
            Ok(()) => {
                self.buf_mut().disk_state = crate::fileio::stat_disk_state(path);
                // Unlike a normal save, nano reports this one even under
                // minibar: its line-count note is only for annotated writes.
                self.set_status(wrote_lines(self.buf().nano_line_count()));
                true
            }
            Err(e) => {
                self.set_status_alert(format!("Error writing {}: {e}", path.display()));
                false
            }
        }
    }

    fn begin_exit(&mut self) {
        // `--saveonexit`/`set saveonexit`: a modified buffer that has a
        // name is written without asking (nano's `do_exit`); an unnamed
        // one first flashes "No file name", then falls through to the
        // usual "Save modified buffer?".
        if self.buf().modified && self.options.saveonexit {
            if self.buf().path.is_some() {
                self.write_it_out(true, true);
                return;
            }
            self.brief_warning = Some("No file name".to_string());
        }
        if self.buf().modified {
            self.mode = Mode::Prompt(Prompt {
                kind: PromptKind::Exit {
                    discard_and_quit: false,
                },
                menu: Menu::YesNo,
                label: "Save modified buffer? ".to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
        } else {
            self.close_current_buffer();
        }
    }

    fn repeat_search(&mut self, backwards: bool) {
        let Some(pattern) = self.search.last_pattern.clone() else {
            self.set_status("No search pattern in memory");
            return;
        };
        self.run_search(&pattern, backwards);
    }

    /// The Write Out prompt's label (nano 8.7's `do_writeout`), rebuilt
    /// whenever `M-D`/`M-M` toggles the buffer's format: " [DOS Format]"
    /// or " [Mac Format]" is appended for those formats (and " [Backup]"
    /// under `set backup`, which tico doesn't have).
    pub(crate) fn writeout_prompt_label(&self, exiting: bool) -> String {
        use crate::buffer::LineFormat;
        let selecting = !exiting && self.buf().mark.is_some();
        let base = if selecting {
            "Write Selection to File"
        } else {
            "Write to File"
        };
        match self.buf().format {
            LineFormat::Dos => format!("{base} [DOS Format]"),
            LineFormat::Mac => format!("{base} [Mac Format]"),
            LineFormat::Unix | LineFormat::Unspecified => base.to_string(),
        }
    }

    /// Write the whole current buffer to `path` (the Write Out prompt's
    /// non-selection case) and report it; when `exiting`, a successful
    /// write then closes the buffer, as nano's `do_exit` does.
    pub(crate) fn write_buffer_to(&mut self, path: &std::path::Path, exiting: bool) {
        match crate::fileio::save_file(self.buf_mut(), path) {
            Ok(()) => {
                self.note_buffer_linecount();
                // nano suppresses the ordinary "Wrote N lines" blurb
                // under minibar too -- only the persistent note shows
                // (confirmed against the installed nano's own
                // escape-code output).
                if !self.options.minibar {
                    self.set_status(wrote_lines(self.buf().nano_line_count()));
                }
                if exiting {
                    self.close_current_buffer();
                }
            }
            Err(e) => self.set_status_alert(format!("Error writing {}: {e}", path.display())),
        }
    }

    pub fn begin_writeout_for_exit(&mut self) {
        self.write_it_out(true, true);
    }

    /// Kick off a three-way-merge preview for the current buffer against
    /// its on-disk contents, using the buffer's originally-loaded content
    /// as the merge base.
    pub fn begin_merge_preview(&mut self) {
        let Some(path) = self.buf().path.clone() else {
            self.mode = Mode::Editing;
            return;
        };
        let Ok(theirs) = std::fs::read_to_string(&path) else {
            self.set_status("Could not re-read file from disk");
            self.mode = Mode::Editing;
            return;
        };
        // Compare like with like: the buffer holds converted text.
        let (theirs, _) = crate::fileio::convert_line_endings(&theirs, self.options.noconvert);
        let base = self.buf().original_content.clone();
        let ours = self.buf().to_string();
        match crate::fileio::three_way_merge(&base, &ours, &theirs) {
            crate::fileio::MergeResult::Clean { text, diff } => {
                let mut lines = vec!["Merge preview -- [A]pply  [C]ancel".to_string()];
                lines.extend(diff.lines().map(str::to_string));
                self.mode = Mode::Diff {
                    lines,
                    top: 0,
                    outcome: DiffOutcome::ApplyMerge { merged_text: text },
                };
            }
            crate::fileio::MergeResult::Conflict { diff } => {
                let mut lines = vec!["Could not merge automatically -- press any key".to_string()];
                lines.extend(diff.lines().map(str::to_string));
                self.mode = Mode::Diff {
                    lines,
                    top: 0,
                    outcome: DiffOutcome::Conflict,
                };
            }
        }
    }

    /// Kick off an interactive replace: find the first match (from the
    /// current cursor, wrapping once around the buffer) and, if found,
    /// open the "Replace this instance?" confirmation prompt. Matches
    /// nano's do_replace() / do_replace_loop().
    pub fn begin_replace_loop(&mut self, search: String, replacement: String) {
        if search.is_empty() {
            self.mode = Mode::Editing;
            return;
        }
        self.search.last_pattern = Some(search.clone());
        // A marked region restricts the replace to just that text (nano's
        // "treat all marked text as one region" for replace) and is a
        // one-shot restriction: the mark itself is cleared here, matching
        // nano's do_replace_loop.
        let region = self.selection_range();
        if region.is_some() {
            self.buf_mut().mark = None;
            self.buf_mut().softmark = false;
        }
        let session_start = region.map(|(start, _)| start).unwrap_or(self.buf().cursor);
        let region_end = region.map(|(_, end)| end);
        let found = find_next_match_for_replace(
            self.buf(),
            session_start,
            session_start,
            false,
            region_end,
            &search,
            self.search.case_sensitive,
            self.search.use_regex,
        );
        match found {
            Ok(Some((pos, len, wrapped))) => {
                self.buf_mut().cursor = pos;
                self.scroll_to_cursor_centered();
                self.set_spotlight_persistent(pos, len);
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::ReplaceConfirm(ReplaceLoopState {
                        search,
                        replacement,
                        match_pos: pos,
                        match_len: len,
                        session_start,
                        wrapped,
                        count: 0,
                        region_end,
                    }),
                    menu: Menu::YesNo,
                    label: "Replace this instance?".to_string(),
                    input: String::new(),
                    cursor: 0,
                    history_pos: None,
                    saved_input: None,
                });
            }
            Ok(None) => {
                self.mode = Mode::Editing;
                self.clear_spotlight();
                self.set_status(format!("\"{search}\" not found"));
            }
            Err(e) => {
                self.mode = Mode::Editing;
                self.clear_spotlight();
                self.set_status(format!("Invalid regex: {e}"));
            }
        }
    }

    /// Act on the user's Yes/No/All/Cancel answer for the current match,
    /// then either advance to the next one (opening a fresh confirmation
    /// prompt) or finish the loop.
    pub fn replace_choice(&mut self, mut state: ReplaceLoopState, choice: ReplaceChoice) {
        if matches!(choice, ReplaceChoice::Cancel) {
            self.mode = Mode::Editing;
            self.clear_spotlight();
            self.report_replace_count(state.count);
            return;
        }
        let mut do_replace = matches!(choice, ReplaceChoice::Yes | ReplaceChoice::All);
        let replace_all = matches!(choice, ReplaceChoice::All);
        loop {
            let next_from = if do_replace {
                let expanded = self.expand_replacement(
                    &state.search,
                    &state.replacement,
                    state.match_pos,
                    state.match_len,
                );
                let end = Pos::new(state.match_pos.line, state.match_pos.col + state.match_len);
                self.buf_mut().delete_range(state.match_pos, end);
                self.buf_mut().cursor = state.match_pos;
                let expanded_len = expanded.chars().count();
                self.buf_mut().insert_str(&expanded);
                state.count += 1;
                // Keep the region boundary in step with a length change on
                // its own line, matching nano's own `mark_x` adjustment.
                if let Some(region_end) = &mut state.region_end
                    && region_end.line == state.match_pos.line
                    && region_end.col >= end.col
                {
                    let delta = expanded_len as isize - state.match_len as isize;
                    region_end.col = (region_end.col as isize + delta).max(0) as usize;
                }
                Pos::new(state.match_pos.line, state.match_pos.col + expanded_len)
            } else {
                // Skip past this match (at least one character, so a
                // zero-length regex match can't be found again forever).
                Pos::new(
                    state.match_pos.line,
                    state.match_pos.col + state.match_len.max(1),
                )
            };
            let found = find_next_match_for_replace(
                self.buf(),
                next_from,
                state.session_start,
                state.wrapped,
                state.region_end,
                &state.search,
                self.search.case_sensitive,
                self.search.use_regex,
            );
            match found {
                Ok(Some((pos, len, wrapped))) => {
                    state.match_pos = pos;
                    state.match_len = len;
                    state.wrapped = wrapped;
                    if replace_all {
                        do_replace = true;
                        continue;
                    }
                    self.buf_mut().cursor = pos;
                    self.scroll_to_cursor_centered();
                    self.set_spotlight_persistent(pos, len);
                    self.mode = Mode::Prompt(Prompt {
                        kind: PromptKind::ReplaceConfirm(state),
                        menu: Menu::YesNo,
                        label: "Replace this instance?".to_string(),
                        input: String::new(),
                        cursor: 0,
                        history_pos: None,
                        saved_input: None,
                    });
                    return;
                }
                Ok(None) => {
                    self.mode = Mode::Editing;
                    self.clear_spotlight();
                    self.report_replace_count(state.count);
                    return;
                }
                Err(e) => {
                    self.mode = Mode::Editing;
                    self.clear_spotlight();
                    self.set_status(format!("Invalid regex: {e}"));
                    return;
                }
            }
        }
    }

    fn report_replace_count(&mut self, count: usize) {
        match count {
            0 => self.set_status("No replacements made"),
            1 => self.set_status("Replaced 1 occurrence"),
            n => self.set_status(format!("Replaced {n} occurrences")),
        }
    }

    /// Build the literal text to insert for one match: for a regex search,
    /// expands nano-style `\1`-`\9` backreferences (verified against
    /// nano's `replace_regexp()` in src/search.c); a literal-string search
    /// uses the replacement text as-is, with no backreference processing —
    /// nano does the same (`replace_line()` only calls `replace_regexp()`
    /// when `ISSET(USE_REGEXP)`).
    fn expand_replacement(
        &self,
        search: &str,
        replacement: &str,
        match_pos: Pos,
        match_len: usize,
    ) -> String {
        if !self.search.use_regex {
            return replacement.to_string();
        }
        let pat = if self.search.case_sensitive {
            search.to_string()
        } else {
            format!("(?i){search}")
        };
        let Ok(re) = regex::Regex::new(&pat) else {
            return replacement.to_string();
        };
        let line_chars: Vec<char> = self.buf().line(match_pos.line).chars().collect();
        let end = (match_pos.col + match_len).min(line_chars.len());
        let matched_text: String = line_chars[match_pos.col..end].iter().collect();
        let Some(caps) = re.captures(&matched_text) else {
            return replacement.to_string();
        };
        expand_backreferences(replacement, &caps)
    }

    pub fn run_search(&mut self, pattern: &str, backwards: bool) {
        if pattern.is_empty() {
            return;
        }
        let text = self.buf().to_string();
        let hay: Vec<&str> = text.split_inclusive('\n').collect();
        let found = find_in_lines(
            &hay,
            self.buf().cursor,
            pattern,
            backwards,
            self.search.case_sensitive,
            self.search.use_regex,
        );
        match found {
            Some((pos, len)) => {
                self.buf_mut().cursor = pos;
                self.scroll_to_cursor_centered();
                self.search.last_pattern = Some(pattern.to_string());
                self.set_spotlight_timed(pos, len);
            }
            None => self.set_status(format!("\"{pattern}\" not found")),
        }
    }
}

fn word_left_pos(buf: &Buffer, from: Pos) -> Pos {
    let mut line = from.line;
    let mut chars: Vec<char> = buf.line(line).chars().collect();
    let mut col = from.col;
    loop {
        while col > 0 && !chars[col - 1].is_alphanumeric() && chars[col - 1] != '_' {
            col -= 1;
        }
        while col > 0 && (chars[col - 1].is_alphanumeric() || chars[col - 1] == '_') {
            col -= 1;
        }
        if col > 0 || line == 0 {
            return Pos::new(line, col);
        }
        line -= 1;
        chars = buf.line(line).chars().collect();
        col = chars.len();
        if col == 0 {
            return Pos::new(line, 0);
        }
    }
}

fn word_right_pos(buf: &Buffer, from: Pos) -> Pos {
    let mut line = from.line;
    let mut chars: Vec<char> = buf.line(line).chars().collect();
    let mut col = from.col;
    loop {
        while col < chars.len() && (chars[col].is_alphanumeric() || chars[col] == '_') {
            col += 1;
        }
        while col < chars.len() && !chars[col].is_alphanumeric() && chars[col] != '_' {
            col += 1;
        }
        if col < chars.len() || line + 1 >= buf.line_count() {
            return Pos::new(line, col);
        }
        line += 1;
        chars = buf.line(line).chars().collect();
        col = 0;
        if chars.is_empty() {
            return Pos::new(line, 0);
        }
    }
}

/// Build a Search/Replace prompt's label the way nano does: the base text,
/// then a bracketed flag for each active toggle in this exact order —
/// `[Case Sensitive]`, `[Regexp]`, `[Backwards]` — then an optional suffix
/// like `" (to replace)"`. Confirmed against the installed nano's actual
/// prompt text (e.g. `Search [Case Sensitive] [Regexp] (to replace):`).
pub fn search_prompt_label(base: &str, suffix: &str, search: &SearchState) -> String {
    let mut label = base.to_string();
    if search.case_sensitive {
        label.push_str(" [Case Sensitive]");
    }
    if search.use_regex {
        label.push_str(" [Regexp]");
    }
    if search.backwards {
        label.push_str(" [Backwards]");
    }
    label.push_str(suffix);
    // The remembered last search term is shown in brackets at the very
    // end, after any suffix (e.g. "Search [Case Sensitive] (to replace)
    // [apple]:") - confirmed against the installed nano's exact wording.
    // Pressing Enter with nothing typed reuses this as the search text.
    if let Some(default) = &search.last_pattern
        && !default.is_empty()
    {
        label.push_str(&format!(" [{default}]"));
    }
    label
}

/// The `^R` Read File prompt's label, matching the installed nano's exact
/// wording for both states (it toggles with `M-F`, no other wording change).
pub fn insert_prompt_label(new_buffer: bool, execute: bool, noconvert: bool) -> String {
    match (execute, new_buffer, noconvert) {
        (true, true, _) => "Command to execute in new buffer".to_string(),
        (true, false, _) => "Command to execute".to_string(),
        (false, true, false) => "File to read into new buffer [from ./]".to_string(),
        (false, true, true) => "File to read unconverted into new buffer [from ./]".to_string(),
        (false, false, false) => "File to insert [from ./]".to_string(),
        (false, false, true) => "File to insert unconverted [from ./]".to_string(),
    }
}

/// Whether `action` would modify the buffer — matches nano's own
/// `changes_something()`, the exact gate its main dispatch loop uses to
/// decide what `--view` blocks (with "Key is invalid in view mode")
/// versus what it still allows (movement, search, Copy, Set Mark, the
/// Insert-File prompt, ...).
/// Whether `line` carries the comment sequence nano's way: `pre` at column
/// 0 exactly (no leading whitespace allowed) and, for a bracketing
/// sequence, `post` at the very end.
fn is_commented(line: &str, pre: &str, post: &str) -> bool {
    line.len() >= pre.len() + post.len() && line.starts_with(pre) && line.ends_with(post)
}

/// nano's `length_of_white()`: how much leading whitespace one unindent
/// removes from `text` -- at most a tab's worth: up to `tabsize` spaces,
/// or any spaces up to and including a tab.
fn length_of_white(text: &str, tabsize: usize) -> usize {
    let mut count = 0;
    for c in text.chars() {
        match c {
            '\t' => return count + 1,
            ' ' => {
                count += 1;
                if count == tabsize {
                    return tabsize;
                }
            }
            _ => break,
        }
    }
    count
}

fn action_changes_something(action: Action) -> bool {
    use Action::*;
    matches!(
        action,
        WriteOut
            | SaveFile
            | Enter
            | Tab
            | Delete
            | Backspace
            | Cut
            | Paste
            | ChopWordLeft
            | ChopWordRight
            | Zap
            | CutRestOfFile
            | Execute
            | Indent
            | Unindent
            | Justify
            | FullJustify
            | Comment
            | Speller
            | Formatter
            | Complete
            | Replace
    )
}

/// Join `lines` (each a char vector) into buffer text with `\n` between
/// them, ready to hand to `insert_str`.
fn join_lines(lines: &[Vec<char>]) -> String {
    lines
        .iter()
        .map(|l| l.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Expand nano-style `\1`-`\9` backreferences in `template` using `caps`
/// (a valid group number that didn't participate in the match expands to
/// nothing; a `\` followed by anything else — including a digit that isn't
/// a valid group number for this pattern — is copied through literally).
fn expand_backreferences(template: &str, caps: &regex::Captures) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\'
            && i + 1 < chars.len()
            && chars[i + 1].is_ascii_digit()
            && chars[i + 1] != '0'
        {
            let n = chars[i + 1].to_digit(10).unwrap() as usize;
            if n < caps.len() {
                if let Some(m) = caps.get(n) {
                    out.push_str(m.as_str());
                }
                i += 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Find the next match of `pattern` at or after `from`, wrapping around the
/// buffer once (but not past `session_start`, if already wrapped) —
/// matches nano's search wraparound ("came_full_circle") so a replace loop
/// can't repeat forever. When `region_end` is set (a marked region was
/// active when the replace began), the search never wraps and stops
/// reporting matches once it reaches that position — matches nano's
/// `INREGION` mode ("only matches in the selected text will be replaced").
/// Returns (match position, match length in chars, whether the search has
/// now wrapped).
#[allow(clippy::too_many_arguments)]
fn find_next_match_for_replace(
    buf: &Buffer,
    from: Pos,
    session_start: Pos,
    already_wrapped: bool,
    region_end: Option<Pos>,
    pattern: &str,
    case_sensitive: bool,
    use_regex: bool,
) -> Result<Option<(Pos, usize, bool)>, String> {
    let re = if use_regex {
        let pat = if case_sensitive {
            pattern.to_string()
        } else {
            format!("(?i){pattern}")
        };
        Some(regex::Regex::new(&pat).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let matches_at = |line: &str| -> Vec<(usize, usize)> {
        if let Some(re) = &re {
            re.find_iter(line)
                .map(|m| {
                    (
                        line[..m.start()].chars().count(),
                        line[m.start()..m.end()].chars().count(),
                    )
                })
                .collect()
        } else if case_sensitive {
            line.char_indices()
                .filter(|(i, _)| line[*i..].starts_with(pattern))
                .map(|(i, _)| (line[..i].chars().count(), pattern.chars().count()))
                .collect()
        } else {
            let lower_line = line.to_lowercase();
            let lower_needle = pattern.to_lowercase();
            lower_line
                .char_indices()
                .filter(|(i, _)| lower_line[*i..].starts_with(&lower_needle))
                .map(|(i, _)| {
                    (
                        lower_line[..i].chars().count(),
                        lower_needle.chars().count(),
                    )
                })
                .collect()
        }
    };

    let n = buf.line_count();
    for line_idx in from.line..n {
        let raw = buf.line(line_idx);
        for (c, len) in matches_at(&raw) {
            if line_idx != from.line || c >= from.col {
                let pos = Pos::new(line_idx, c);
                if let Some(end) = region_end
                    && (pos.line, pos.col) >= (end.line, end.col)
                {
                    return Ok(None);
                }
                return Ok(Some((pos, len, already_wrapped)));
            }
        }
    }
    if already_wrapped || region_end.is_some() {
        return Ok(None);
    }
    for line_idx in 0..=session_start.line.min(n.saturating_sub(1)) {
        let raw = buf.line(line_idx);
        for (c, len) in matches_at(&raw) {
            if line_idx < session_start.line || c < session_start.col {
                return Ok(Some((Pos::new(line_idx, c), len, true)));
            }
        }
    }
    Ok(None)
}

fn find_in_lines(
    lines: &[&str],
    from: Pos,
    pattern: &str,
    backwards: bool,
    case_sensitive: bool,
    use_regex: bool,
) -> Option<(Pos, usize)> {
    let re = if use_regex {
        let pat = if case_sensitive {
            pattern.to_string()
        } else {
            format!("(?i){pattern}")
        };
        regex::Regex::new(&pat).ok()
    } else {
        None
    };
    let matches_at = |line: &str, needle: &str| -> Vec<(usize, usize)> {
        if let Some(re) = &re {
            re.find_iter(line)
                .map(|m| {
                    (
                        line[..m.start()].chars().count(),
                        line[m.start()..m.end()].chars().count(),
                    )
                })
                .collect()
        } else if case_sensitive {
            line.char_indices()
                .filter(|(i, _)| line[*i..].starts_with(needle))
                .map(|(i, _)| (line[..i].chars().count(), needle.chars().count()))
                .collect()
        } else {
            let lower_line = line.to_lowercase();
            let lower_needle = needle.to_lowercase();
            lower_line
                .char_indices()
                .filter(|(i, _)| lower_line[*i..].starts_with(&lower_needle))
                .map(|(i, _)| {
                    (
                        lower_line[..i].chars().count(),
                        lower_needle.chars().count(),
                    )
                })
                .collect()
        }
    };

    let n = lines.len();
    let order: Vec<usize> = if backwards {
        (0..n).rev().collect()
    } else {
        (0..n).collect()
    };
    // Rotate so we start searching from the current line, wrapping around.
    let start_idx = order.iter().position(|&l| l == from.line).unwrap_or(0);
    let rotated = order[start_idx..].iter().chain(order[..start_idx].iter());

    for &line_idx in rotated {
        let raw = lines[line_idx].trim_end_matches(['\n', '\r']);
        let mut cols = matches_at(raw, pattern);
        if !backwards {
            cols.retain(|&(c, _)| line_idx != from.line || c > from.col);
        } else {
            cols.reverse();
            cols.retain(|&(c, _)| line_idx != from.line || c < from.col);
        }
        if let Some(&(c, len)) = cols.first() {
            return Some((Pos::new(line_idx, c), len));
        }
    }
    None
}

/// nano's report after writing a file: "Wrote 1 line" / "Wrote N lines".
fn wrote_lines(count: usize) -> String {
    if count == 1 {
        "Wrote 1 line".to_string()
    } else {
        format!("Wrote {count} lines")
    }
}

/// nano's "File \"%s\" exists; OVERWRITE? " question.
fn overwrite_question(name: &str) -> String {
    format!("File \"{name}\" exists; OVERWRITE? ")
}

/// nano's `crop_to_fit`: `name` if it fits in `room` columns, else its
/// tail behind "...", or just "_" when there's no room for that.
fn crop_to_fit(name: &str, room: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let width = |c: char| c.width().unwrap_or(0);
    if name.chars().map(width).sum::<usize>() <= room {
        return name.to_string();
    }
    if room < 4 {
        return "_".to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for c in name.chars().rev() {
        if used + width(c) > room - 3 {
            break;
        }
        used += width(c);
        tail.push(c);
    }
    format!("...{}", tail.into_iter().rev().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::LineFormat;
    use crate::keymap::KeyMap;
    use crate::options::Options;

    fn test_editor(text: &str) -> Editor {
        let mut ed = Editor::new(Options::default(), KeyMap::new());
        ed.buffers[0] = Buffer::from_text(text, None);
        ed
    }

    #[test]
    fn exiting_the_last_unmodified_buffer_does_not_panic() {
        // execute() used to end with an unconditional scroll_to_cursor(),
        // which - like a couple of other post-action steps - assumed there
        // was always still a buffer to look at. Exiting with nothing to
        // save closes the last buffer and sets Mode::Quit in the same
        // call, leaving `buffers` empty.
        let mut ed = test_editor("hello");
        ed.execute(Action::Exit);
        assert!(matches!(ed.mode, Mode::Quit));
        assert!(ed.buffers.is_empty());
    }

    #[test]
    fn saveonexit_writes_a_named_modified_buffer_without_asking() {
        let path =
            std::env::temp_dir().join(format!("tico_test_saveonexit.{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut ed = test_editor("hello");
        ed.options.saveonexit = true;
        ed.buf_mut().path = Some(path.clone());
        ed.buf_mut().modified = true;
        ed.execute(Action::Exit);
        assert!(matches!(ed.mode, Mode::Quit));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn saveonexit_still_asks_for_an_unnamed_buffer() {
        let mut ed = test_editor("hello");
        ed.options.saveonexit = true;
        ed.buf_mut().modified = true;
        ed.execute(Action::Exit);
        assert!(matches!(
            ed.mode,
            Mode::Prompt(Prompt {
                kind: PromptKind::Exit { .. },
                ..
            })
        ));
        assert_eq!(ed.brief_warning.as_deref(), Some("No file name"));
    }

    #[test]
    fn exit_without_saveonexit_has_no_warning() {
        let mut ed = test_editor("hello");
        ed.buf_mut().modified = true;
        ed.execute(Action::Exit);
        assert_eq!(ed.brief_warning, None);
    }

    /// A fresh scratch directory for one write-question test.
    fn write_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tico_wq_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An editor holding `path` as if just read from disk, then edited.
    fn editor_on_file(path: &std::path::Path, disk: &str, text: &str) -> Editor {
        std::fs::write(path, disk).unwrap();
        let mut ed = test_editor(text);
        ed.screen_cols = 80;
        ed.buf_mut().path = Some(path.to_path_buf());
        ed.buf_mut().disk_state = crate::fileio::stat_disk_state(path);
        ed.buf_mut().modified = true;
        ed
    }

    /// Make `path` look changed on disk since it was read: new contents
    /// and an mtime well ahead of the recorded one.
    fn touch_on_disk(path: &std::path::Path, text: &str) {
        std::fs::write(path, text).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    /// Type `answer` at the Write Out prompt that's up.
    fn submit_write(ed: &mut Editor, answer: &str) {
        let Mode::Prompt(Prompt {
            kind: PromptKind::WriteOut { flow },
            ..
        }) = std::mem::replace(&mut ed.mode, Mode::Editing)
        else {
            panic!("expected the Write Out prompt");
        };
        ed.submit_write_answer(answer.to_string(), flow);
    }

    /// The pending write question and its label, if one is up.
    fn write_question(ed: &Editor) -> Option<(WriteQuestion, String)> {
        match &ed.mode {
            Mode::Prompt(Prompt {
                kind: PromptKind::WriteConfirm { question, .. },
                label,
                ..
            }) => Some((*question, label.clone())),
            _ => None,
        }
    }

    fn answer(ed: &mut Editor, choice: YesNo) {
        let Mode::Prompt(Prompt {
            kind:
                PromptKind::WriteConfirm {
                    question,
                    answer,
                    flow,
                },
            ..
        }) = std::mem::replace(&mut ed.mode, Mode::Editing)
        else {
            panic!("expected a write question");
        };
        ed.answer_write_question(question, answer, flow, choice);
    }

    #[test]
    fn write_out_under_another_name_asks_different_name_then_overwrite() {
        let dir = write_test_dir("different_name");
        let own = dir.join("own.txt");
        let other = dir.join("other.txt");
        std::fs::write(&other, "theirs").unwrap();
        let mut ed = editor_on_file(&own, "old", "new");
        let other_s = other.display().to_string();

        ed.execute(Action::WriteOut);
        submit_write(&mut ed, &other_s);
        assert_eq!(
            write_question(&ed),
            Some((
                WriteQuestion::DifferentName,
                "Save file under DIFFERENT NAME? ".to_string()
            ))
        );

        // No: back to the Write Out prompt, offering the name as typed.
        answer(&mut ed, YesNo::No);
        match &ed.mode {
            Mode::Prompt(p) => {
                assert!(matches!(p.kind, PromptKind::WriteOut { .. }));
                assert_eq!(p.input, other_s);
            }
            _ => panic!("expected the Write Out prompt again"),
        }

        submit_write(&mut ed, &other_s);
        answer(&mut ed, YesNo::Yes);
        let (question, label) = write_question(&ed).unwrap();
        assert_eq!(question, WriteQuestion::Overwrite);
        assert!(label.starts_with("File \""), "{label}");
        assert!(label.ends_with("\" exists; OVERWRITE? "), "{label}");
        assert!(label.chars().count() <= 80, "{label}");

        answer(&mut ed, YesNo::Yes);
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "old");
        assert_eq!(ed.buf().path.as_deref(), Some(other.as_path()));
        assert!(!ed.buf().modified);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn declining_overwrite_writes_nothing_and_reprompts() {
        let dir = write_test_dir("decline_overwrite");
        let other = dir.join("other.txt");
        std::fs::write(&other, "theirs").unwrap();
        let mut ed = test_editor("mine");
        ed.screen_cols = 80;
        ed.buf_mut().modified = true;

        // An unnamed buffer may take any name, so only OVERWRITE is asked.
        ed.execute(Action::WriteOut);
        submit_write(&mut ed, &other.display().to_string());
        assert_eq!(write_question(&ed).unwrap().0, WriteQuestion::Overwrite);
        answer(&mut ed, YesNo::Cancel);
        assert!(matches!(
            ed.mode,
            Mode::Prompt(Prompt {
                kind: PromptKind::WriteOut { .. },
                ..
            })
        ));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "theirs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unnamed_buffer_to_a_new_name_is_written_without_questions() {
        let dir = write_test_dir("new_name");
        let path = dir.join("fresh.txt");
        let mut ed = test_editor("mine");
        ed.buf_mut().modified = true;
        ed.execute(Action::WriteOut);
        submit_write(&mut ed, &path.display().to_string());
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        assert_eq!(ed.status.as_deref(), Some("Wrote 1 line"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrote_lines_matches_nanos_wording() {
        assert_eq!(wrote_lines(0), "Wrote 0 lines");
        assert_eq!(wrote_lines(1), "Wrote 1 line");
        assert_eq!(wrote_lines(3), "Wrote 3 lines");
    }

    #[test]
    fn exit_question_matches_nano_even_for_a_named_buffer() {
        let mut ed = test_editor("x");
        ed.buf_mut().path = Some("named.txt".into());
        ed.buf_mut().modified = true;
        ed.execute(Action::Exit);
        let Mode::Prompt(p) = &ed.mode else {
            panic!("expected the exit question");
        };
        assert_eq!(p.label, "Save modified buffer? ");
    }

    #[test]
    fn a_selection_under_another_name_skips_the_different_name_question() {
        let dir = write_test_dir("selection");
        let own = dir.join("own.txt");
        let other = dir.join("part.txt");
        let mut ed = editor_on_file(&own, "old", "hello world");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(0, 5);
        ed.execute(Action::WriteOut);
        submit_write(&mut ed, &other.display().to_string());
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "hello\n");
        assert_eq!(ed.status.as_deref(), Some("Wrote 1 line"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Write `text`'s region `from`..`to` to a fresh file via ^O, as
    /// `format`, and return what landed on disk.
    fn write_region(
        text: &str,
        from: Pos,
        to: Pos,
        format: LineFormat,
        nonewlines: bool,
    ) -> String {
        let dir = write_test_dir(&format!("region_{}_{}_{nonewlines}", to.line, to.col));
        let out = dir.join(format!("{format:?}.txt"));
        let mut ed = test_editor(text);
        ed.options.nonewlines = nonewlines;
        ed.buf_mut().format = format;
        ed.buf_mut().mark = Some(from);
        ed.buf_mut().cursor = to;
        ed.execute(Action::WriteOut);
        submit_write(&mut ed, &out.display().to_string());
        let written = std::fs::read_to_string(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        written
    }

    #[test]
    fn a_region_write_ends_with_a_newline_like_nano() {
        let text = "one\ntwo\nthree\n";
        // Ending mid-line: nano adds the newline.
        assert_eq!(
            write_region(
                text,
                Pos::new(0, 0),
                Pos::new(1, 2),
                LineFormat::Unix,
                false
            ),
            "one\ntw\n"
        );
        // Ending at the start of a line: already ends with one.
        assert_eq!(
            write_region(
                text,
                Pos::new(0, 0),
                Pos::new(2, 0),
                LineFormat::Unix,
                false
            ),
            "one\ntwo\n"
        );
        // `nonewlines`: nothing added.
        assert_eq!(
            write_region(text, Pos::new(0, 0), Pos::new(1, 2), LineFormat::Unix, true),
            "one\ntw"
        );
        // Line breaks follow the buffer's format.
        assert_eq!(
            write_region(text, Pos::new(0, 0), Pos::new(1, 2), LineFormat::Dos, false),
            "one\r\ntw\r\n"
        );
        assert_eq!(
            write_region(text, Pos::new(0, 0), Pos::new(1, 2), LineFormat::Mac, false),
            "one\rtw\r"
        );
    }

    #[test]
    fn empty_write_out_answer_cancels() {
        let mut ed = test_editor("mine");
        ed.execute(Action::WriteOut);
        submit_write(&mut ed, "");
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(ed.status.as_deref(), Some("Cancelled"));
    }

    #[test]
    fn saving_over_a_file_changed_on_disk_warns_and_asks() {
        let dir = write_test_dir("disk_changed");
        let own = dir.join("own.txt");
        let mut ed = editor_on_file(&own, "old", "mine");
        touch_on_disk(&own, "theirs");

        ed.execute(Action::SaveFile);
        assert_eq!(
            ed.brief_warning.as_deref(),
            Some("File on disk has changed")
        );
        assert_eq!(
            write_question(&ed),
            Some((
                WriteQuestion::DiskChanged,
                "File was modified since you opened it; continue saving? ".to_string()
            ))
        );
        // ^S answered No: nothing written, the buffer stays as it was.
        answer(&mut ed, YesNo::No);
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "theirs");
        assert!(ed.buf().modified);

        ed.brief_warning = None;
        ed.execute(Action::SaveFile);
        answer(&mut ed, YesNo::Yes);
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "mine");
        assert!(!ed.buf().modified);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_file_on_disk_saves_without_asking() {
        let dir = write_test_dir("disk_unchanged");
        let own = dir.join("own.txt");
        let mut ed = editor_on_file(&own, "old", "mine");
        ed.execute(Action::SaveFile);
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(ed.brief_warning, None);
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "mine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exiting_over_a_changed_file_no_closes_unsaved_cancel_reprompts() {
        let dir = write_test_dir("exit_disk_changed");
        let own = dir.join("own.txt");
        let own_s = own.display().to_string();

        // Cancel goes back to the Write Out prompt.
        let mut ed = editor_on_file(&own, "old", "mine");
        touch_on_disk(&own, "theirs");
        ed.begin_writeout_for_exit();
        submit_write(&mut ed, &own_s);
        assert_eq!(write_question(&ed).unwrap().0, WriteQuestion::DiskChanged);
        answer(&mut ed, YesNo::Cancel);
        assert!(matches!(
            ed.mode,
            Mode::Prompt(Prompt {
                kind: PromptKind::WriteOut { .. },
                ..
            })
        ));

        // No counts as done: the buffer is closed without writing.
        submit_write(&mut ed, &own_s);
        answer(&mut ed, YesNo::No);
        assert!(matches!(ed.mode, Mode::Quit));
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "theirs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saveonexit_over_a_changed_file() {
        let dir = write_test_dir("saveonexit_disk_changed");
        let own = dir.join("own.txt");

        // Cancel: nothing happens, the buffer stays open.
        let mut ed = editor_on_file(&own, "old", "mine");
        ed.options.saveonexit = true;
        touch_on_disk(&own, "theirs");
        ed.execute(Action::Exit);
        assert_eq!(write_question(&ed).unwrap().0, WriteQuestion::DiskChanged);
        answer(&mut ed, YesNo::Cancel);
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(ed.buffers.len(), 1);

        // No: the buffer is discarded.
        ed.execute(Action::Exit);
        answer(&mut ed, YesNo::No);
        assert!(matches!(ed.mode, Mode::Quit));
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "theirs");

        // Yes: written, then closed.
        let mut ed = editor_on_file(&own, "old", "mine");
        ed.options.saveonexit = true;
        touch_on_disk(&own, "theirs");
        ed.execute(Action::Exit);
        answer(&mut ed, YesNo::Yes);
        assert!(matches!(ed.mode, Mode::Quit));
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "mine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crop_to_fit_matches_nano() {
        assert_eq!(crop_to_fit("short", 10), "short");
        assert_eq!(crop_to_fit("/a/long/path/name.txt", 10), "...ame.txt");
        assert_eq!(crop_to_fit("abcdef", 3), "_");
    }

    #[test]
    fn lock_flag_update_on_empty_buffers_does_not_panic() {
        let mut ed = test_editor("hello");
        ed.buffers.clear();
        ed.maybe_update_lock_modified_flag(); // must not panic
    }

    #[test]
    fn justify_leaves_cursor_on_the_line_after_the_paragraph_not_its_last_line() {
        // Confirmed against the installed nano: justify_text extends the
        // cut region one line further than the paragraph itself before
        // pasting the result back, so the cursor ends up on whatever
        // follows (typically a blank separator line), not on the
        // paragraph's own last line.
        let mut ed = test_editor("one two three four five six seven eight\n\nSecond paragraph.\n");
        ed.options.fill = -65; // wrap_at = 80 - 65 = 15: forces a rewrap into multiple lines
        ed.execute(Action::Justify);
        assert_eq!(ed.buf().cursor.col, 0);
        assert_eq!(ed.buf().line(ed.buf().cursor.line), "");
        assert!(
            ed.buf().cursor.line > 1,
            "the one-line paragraph should have been rewrapped into several lines first"
        );
    }

    #[test]
    fn toggle_mark_sets_and_unsets_with_status_messages() {
        let mut ed = test_editor("hello world");
        ed.buf_mut().cursor = Pos::new(0, 3);
        ed.execute(Action::Mark);
        assert_eq!(ed.buf().mark, Some(Pos::new(0, 3)));
        assert!(!ed.buf().softmark, "^^ sets a hard mark, not a soft one");
        assert_eq!(ed.status.as_deref(), Some("Mark Set"));

        ed.execute(Action::Mark);
        assert_eq!(ed.buf().mark, None);
        assert_eq!(ed.status.as_deref(), Some("Mark Unset"));
    }

    #[test]
    fn replace_loop_restricted_to_marked_region_only() {
        // "foo" appears on all three lines; marking just the middle one
        // must mean only that occurrence gets replaced.
        let mut ed = test_editor("foo one\nfoo two\nfoo three\n");
        ed.buf_mut().cursor = Pos::new(1, 0);
        ed.buf_mut().mark = Some(Pos::new(2, 0)); // selects line 1 (0-indexed) whole
        ed.begin_replace_loop("foo".to_string(), "bar".to_string());
        let Mode::Prompt(prompt) = &ed.mode else {
            panic!("expected the replace-confirm prompt to be open");
        };
        let Prompt {
            kind: PromptKind::ReplaceConfirm(state),
            ..
        } = prompt.clone()
        else {
            panic!("expected PromptKind::ReplaceConfirm");
        };
        assert_eq!(state.match_pos, Pos::new(1, 0));
        assert!(
            ed.buf().mark.is_none(),
            "the mark is a one-shot restriction, cleared once replacing begins"
        );
        ed.replace_choice(state, ReplaceChoice::All);
        assert_eq!(ed.buf().to_string(), "foo one\nbar two\nfoo three\n");
    }

    #[test]
    fn justify_selection_treats_marked_lines_as_one_paragraph() {
        let mut ed = test_editor("one two three\nfour five six\n\nnot selected\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 0); // selects the first two lines whole
        ed.execute(Action::Justify);
        assert_eq!(
            ed.buf().to_string(),
            "one two three four five six\n\nnot selected\n"
        );
        assert!(ed.buf().mark.is_none());
        assert_eq!(ed.status.as_deref(), Some("Justified selection"));
    }

    #[test]
    fn justify_selection_reports_when_empty() {
        let mut ed = test_editor("hello");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(0, 0); // mark == cursor: nothing selected
        ed.execute(Action::Justify);
        assert_eq!(ed.status.as_deref(), Some("Selection is empty"));
        assert_eq!(ed.buf().to_string(), "hello");
    }

    #[test]
    fn view_mode_blocks_edits_but_allows_movement_and_search() {
        let mut ed = test_editor("hello world");
        ed.options.view = true;

        ed.execute(Action::Cut);
        assert_eq!(ed.buf().to_string(), "hello world");
        assert_eq!(ed.status.as_deref(), Some("Key is invalid in view mode"));

        ed.execute(Action::Backspace);
        assert_eq!(ed.buf().to_string(), "hello world");

        ed.execute(Action::Delete);
        assert_eq!(ed.buf().to_string(), "hello world");

        // Movement, Copy, and Set Mark are all read-only and must still work.
        ed.execute(Action::Right);
        assert_eq!(ed.buf().cursor, Pos::new(0, 1));
        ed.execute(Action::Copy);
        assert_eq!(ed.cutbuffer, "hello world\n");
        ed.execute(Action::Mark);
        assert_eq!(ed.status.as_deref(), Some("Mark Set"));
    }

    #[test]
    fn view_mode_allows_read_file_but_blocks_execute_command() {
        let mut ed = test_editor("hello");
        ed.options.view = true;
        ed.execute(Action::Insert);
        assert!(
            matches!(ed.mode, Mode::Prompt(_)),
            "^R Read File isn't in nano's changes_something list, so it stays available"
        );
        ed.mode = Mode::Editing;

        ed.execute(Action::Execute);
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(ed.status.as_deref(), Some("Key is invalid in view mode"));
    }

    #[test]
    fn backreference_expansion_basic() {
        let re = regex::Regex::new(r"(\w+)@(\w+)").unwrap();
        let caps = re.captures("alice@example").unwrap();
        assert_eq!(expand_backreferences(r"\2:\1", &caps), "example:alice");
    }

    #[test]
    fn backreference_nonparticipating_group_is_empty() {
        let re = regex::Regex::new(r"(a)|(b)").unwrap();
        let caps = re.captures("b").unwrap();
        assert_eq!(expand_backreferences(r"[\1][\2]", &caps), "[][b]");
    }

    #[test]
    fn backreference_out_of_range_is_literal() {
        let re = regex::Regex::new(r"(a)").unwrap();
        let caps = re.captures("a").unwrap();
        // Only group 1 exists; \5 isn't a valid group so stays literal.
        assert_eq!(expand_backreferences(r"\5-\1", &caps), r"\5-a");
    }

    #[test]
    fn search_scrolls_offscreen_match_into_view() {
        let text = (0..50).map(|i| format!("line{i}\n")).collect::<String>();
        let mut ed = test_editor(&text);
        ed.screen_rows = 24; // text_rows() ~= 20 with default title/status/help rows
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.buf_mut().top_line = 0;
        ed.run_search("line45".to_string().as_str(), false);
        assert_eq!(ed.buf().cursor.line, 45);
        let rows = ed.text_rows();
        assert!(
            ed.buf().top_line <= 45 && 45 < ed.buf().top_line + rows,
            "match at line 45 should be within the scrolled viewport (top_line={}, rows={rows})",
            ed.buf().top_line
        );
    }

    #[test]
    fn search_centers_offscreen_match_like_nano() {
        // nano's edit_redraw(..., CENTERING) puts the match at row
        // editwinrows/2 rather than just barely scrolling it into view -
        // confirmed directly against the installed nano.
        let text = (0..50).map(|i| format!("line{i}\n")).collect::<String>();
        let mut ed = test_editor(&text);
        ed.screen_rows = 24;
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.buf_mut().top_line = 0;
        ed.run_search("line45".to_string().as_str(), false);
        let rows = ed.text_rows();
        assert_eq!(ed.buf().top_line, 45 - rows / 2);
    }

    #[test]
    fn search_does_not_scroll_when_match_already_visible() {
        let text = (0..50).map(|i| format!("line{i}\n")).collect::<String>();
        let mut ed = test_editor(&text);
        ed.screen_rows = 24;
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.buf_mut().top_line = 0;
        ed.run_search("line3".to_string().as_str(), false); // well within the first screenful
        assert_eq!(
            ed.buf().top_line,
            0,
            "already-visible match shouldn't move the viewport"
        );
    }

    #[test]
    fn config_options_seed_initial_search_state() {
        let opts = Options {
            casesensitive: true,
            regexp: true,
            ..Default::default()
        };
        let ed = Editor::new(opts, KeyMap::new());
        assert!(ed.search.case_sensitive);
        assert!(ed.search.use_regex);
    }

    #[test]
    fn replace_all_literal_case_insensitive() {
        let mut ed = test_editor("Foo bar foo BAR foo");
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.begin_replace_loop("foo".to_string(), "X".to_string());
        // First match should be found and awaiting confirmation.
        let Mode::Prompt(Prompt {
            kind: PromptKind::ReplaceConfirm(state),
            ..
        }) = &ed.mode
        else {
            panic!("expected a replace-confirm prompt");
        };
        assert_eq!(state.match_pos, Pos::new(0, 0));
        let state = state.clone();
        ed.replace_choice(state, ReplaceChoice::All);
        assert_eq!(ed.buf().to_string(), "X bar X BAR X");
        assert!(matches!(ed.mode, Mode::Editing));
    }

    #[test]
    fn replace_yes_no_skips_and_replaces_selectively() {
        let mut ed = test_editor("cat cat cat");
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.begin_replace_loop("cat".to_string(), "dog".to_string());
        let state = match &ed.mode {
            Mode::Prompt(Prompt {
                kind: PromptKind::ReplaceConfirm(s),
                ..
            }) => s.clone(),
            _ => panic!("expected prompt"),
        };
        ed.replace_choice(state, ReplaceChoice::No); // skip first "cat"
        let state = match &ed.mode {
            Mode::Prompt(Prompt {
                kind: PromptKind::ReplaceConfirm(s),
                ..
            }) => s.clone(),
            _ => panic!("expected prompt after No"),
        };
        ed.replace_choice(state, ReplaceChoice::Yes); // replace second "cat"
        assert_eq!(ed.buf().to_string(), "cat dog cat");
        let state = match &ed.mode {
            Mode::Prompt(Prompt {
                kind: PromptKind::ReplaceConfirm(s),
                ..
            }) => s.clone(),
            _ => panic!("expected prompt after Yes"),
        };
        ed.replace_choice(state, ReplaceChoice::Cancel);
        assert_eq!(ed.buf().to_string(), "cat dog cat");
        assert!(matches!(ed.mode, Mode::Editing));
    }

    #[test]
    fn replace_regex_with_backreferences() {
        let mut ed = test_editor("2024-01-15");
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.search.use_regex = true;
        ed.begin_replace_loop(r"(\d+)-(\d+)-(\d+)".to_string(), r"\3/\2/\1".to_string());
        let state = match &ed.mode {
            Mode::Prompt(Prompt {
                kind: PromptKind::ReplaceConfirm(s),
                ..
            }) => s.clone(),
            _ => panic!("expected prompt"),
        };
        ed.replace_choice(state, ReplaceChoice::Yes);
        assert_eq!(ed.buf().to_string(), "15/01/2024");
    }

    #[test]
    fn replace_wraps_around_once_then_stops() {
        // Cursor starts on the second "x"; with wraparound both should be
        // found (in order: second, then first on wrap), but not forever.
        let mut ed = test_editor("x y x");
        ed.buf_mut().cursor = Pos::new(0, 4); // at the second 'x'
        ed.begin_replace_loop("x".to_string(), "Z".to_string());
        let state = match &ed.mode {
            Mode::Prompt(Prompt {
                kind: PromptKind::ReplaceConfirm(s),
                ..
            }) => s.clone(),
            _ => panic!("expected prompt"),
        };
        assert_eq!(state.match_pos, Pos::new(0, 4));
        ed.replace_choice(state, ReplaceChoice::All);
        assert_eq!(ed.buf().to_string(), "Z y Z");
        assert!(matches!(ed.mode, Mode::Editing));
    }

    #[test]
    fn replace_not_found_reports_status() {
        let mut ed = test_editor("hello world");
        ed.begin_replace_loop("zzz".to_string(), "y".to_string());
        assert!(matches!(ed.mode, Mode::Editing));
        assert_eq!(ed.status.as_deref(), Some("\"zzz\" not found"));
    }

    #[test]
    fn invalid_regex_reports_status_not_panic() {
        let mut ed = test_editor("hello world");
        ed.search.use_regex = true;
        ed.begin_replace_loop("(unclosed".to_string(), "y".to_string());
        assert!(matches!(ed.mode, Mode::Editing));
        assert!(
            ed.status
                .as_deref()
                .unwrap_or("")
                .starts_with("Invalid regex")
        );
    }

    #[test]
    fn short_line_never_scrolls_horizontally() {
        let mut ed = test_editor("short");
        ed.screen_cols = 60;
        ed.buf_mut().cursor.col = 5;
        ed.scroll_to_cursor();
        assert_eq!(ed.buf().left_col, 0);
    }

    #[test]
    fn long_line_scrolls_to_keep_cursor_visible() {
        // Regression test: the first cut of this formula underflowed
        // (`cursor_col - width + CUSHION + 1` computed left-to-right in
        // usize) for exactly this kind of case, panicking in a debug
        // build the moment the cursor crossed the scroll threshold.
        let mut ed = test_editor(&"x".repeat(200));
        ed.screen_cols = 60;
        for _ in 0..65 {
            ed.execute(Action::Right);
        }
        assert_eq!(ed.buf().cursor.col, 65);
        assert!(
            ed.buf().left_col > 0,
            "cursor at column 65 in a 60-wide view should have scrolled"
        );
        // The cursor itself must still land within the visible window
        // (leaving room for the '<' marker its scroll implies).
        assert!(
            ed.buf().cursor.col > ed.buf().left_col
                && ed.buf().cursor.col < ed.buf().left_col + ed.screen_cols,
            "cursor (col={}) should be inside the scrolled window (left_col={})",
            ed.buf().cursor.col,
            ed.buf().left_col
        );
    }

    #[test]
    fn scroll_resets_when_cursor_returns_near_start() {
        let mut ed = test_editor(&"x".repeat(200));
        ed.screen_cols = 60;
        for _ in 0..65 {
            ed.execute(Action::Right);
        }
        assert!(ed.buf().left_col > 0);
        ed.buf_mut().cursor.col = 0;
        ed.scroll_to_cursor();
        assert_eq!(ed.buf().left_col, 0);
    }

    #[test]
    fn narrow_screen_does_not_panic() {
        // width <= 2*CUSHION+1 takes the "too narrow to cushion" fallback;
        // make sure it doesn't underflow either.
        let mut ed = test_editor(&"x".repeat(50));
        ed.screen_cols = 3;
        for _ in 0..20 {
            ed.execute(Action::Right); // must not panic
        }
    }

    // ----- Indent / Unindent (nano's do_indent / do_unindent) -----

    #[test]
    fn indent_prefixes_the_cursor_line_with_a_tab_and_shifts_the_cursor() {
        let mut ed = test_editor("abc\ndef\n");
        ed.buf_mut().cursor = Pos::new(0, 2);
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().line(0), "\tabc");
        assert_eq!(
            ed.buf().line(1),
            "def",
            "only the cursor's line is indented"
        );
        assert_eq!(ed.buf().cursor, Pos::new(0, 3));
        assert!(ed.buf().modified);
        assert_eq!(ed.status, None, "nano shows no message for an indent");
    }

    #[test]
    fn indent_leaves_a_cursor_at_column_zero_where_it_is() {
        let mut ed = test_editor("abc\n");
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().line(0), "\tabc");
        assert_eq!(ed.buf().cursor, Pos::new(0, 0));
    }

    #[test]
    fn indent_uses_tabsize_spaces_under_tabstospaces() {
        let mut ed = test_editor("abc\n");
        ed.options.tabstospaces = true;
        ed.options.tabsize = 4;
        ed.buf_mut().cursor = Pos::new(0, 1);
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().line(0), "    abc");
        assert_eq!(ed.buf().cursor, Pos::new(0, 5));
    }

    #[test]
    fn indent_of_a_marked_region_skips_empty_lines_and_keeps_the_mark() {
        let mut ed = test_editor("one\n\nthree\nfour\n");
        ed.buf_mut().mark = Some(Pos::new(0, 1));
        ed.buf_mut().cursor = Pos::new(2, 2);
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().line(0), "\tone");
        assert_eq!(ed.buf().line(1), "", "empty lines get no indentation");
        assert_eq!(ed.buf().line(2), "\tthree");
        assert_eq!(ed.buf().line(3), "four", "past the region");
        assert_eq!(
            ed.buf().mark,
            Some(Pos::new(0, 2)),
            "mark shifts with its text"
        );
        assert_eq!(ed.buf().cursor, Pos::new(2, 3));
    }

    #[test]
    fn indent_of_only_empty_lines_does_nothing_at_all() {
        let mut ed = test_editor("\n\n\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 0);
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().to_string(), "\n\n\n");
        assert!(!ed.buf().modified);
        assert!(ed.buf().undo_stack.is_empty(), "no undo record for a no-op");
    }

    #[test]
    fn indent_region_ending_at_column_zero_excludes_that_line() {
        let mut ed = test_editor("one\ntwo\nthree\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 0);
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().line(0), "\tone");
        assert_eq!(ed.buf().line(1), "\ttwo");
        assert_eq!(
            ed.buf().line(2),
            "three",
            "a region ending at col 0 doesn't reach into that line"
        );
    }

    #[test]
    fn indent_keeps_including_the_last_line_until_the_cursor_changes_line() {
        // nano's `also_the_last`: once an indent has acted on a region that
        // reached into its last line, a follow-up press keeps that line
        // even if the cursor has since moved to column 0 of it...
        let mut ed = test_editor("one\ntwo\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(1, 2);
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().line(1), "\ttwo");
        ed.execute(Action::Home);
        assert_eq!(ed.buf().cursor, Pos::new(1, 0));
        ed.execute(Action::Indent);
        assert_eq!(
            ed.buf().line(1),
            "\t\ttwo",
            "still included: same line as before"
        );

        // ...but not once the cursor has been on a different line.
        ed.execute(Action::Up);
        ed.execute(Action::Down);
        assert_eq!(ed.buf().cursor, Pos::new(1, 0));
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().line(0), "\t\t\tone");
        assert_eq!(
            ed.buf().line(1),
            "\t\ttwo",
            "excluded again after a line change"
        );
    }

    #[test]
    fn indent_is_a_single_undo_step() {
        let mut ed = test_editor("one\ntwo\nthree\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 3);
        ed.execute(Action::Indent);
        assert_eq!(ed.buf().to_string(), "\tone\n\ttwo\n\tthree\n");
        ed.execute(Action::Undo);
        assert_eq!(ed.buf().to_string(), "one\ntwo\nthree\n");
        assert_eq!(ed.buf().cursor, Pos::new(2, 3));
        ed.execute(Action::Redo);
        assert_eq!(ed.buf().to_string(), "\tone\n\ttwo\n\tthree\n");
        assert_eq!(ed.buf().cursor, Pos::new(2, 4));
    }

    #[test]
    fn unindent_removes_a_tabs_worth_of_leading_whitespace() {
        // tabsize 8 (the default): up to 8 spaces, or spaces up to and
        // including a tab, but never more than that per press.
        let mut ed = test_editor("\t\tone\n    two\n  \tthree\n          four\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(3, 10);
        ed.execute(Action::Unindent);
        assert_eq!(ed.buf().line(0), "\tone");
        assert_eq!(ed.buf().line(1), "two");
        assert_eq!(ed.buf().line(2), "three", "spaces plus a tab go together");
        assert_eq!(ed.buf().line(3), "  four", "only tabsize of ten spaces");
        assert_eq!(ed.buf().cursor, Pos::new(3, 2));
        assert_eq!(ed.status, None);
    }

    #[test]
    fn unindent_clamps_cursor_and_mark_at_column_zero() {
        let mut ed = test_editor("    one\n    two\n");
        ed.buf_mut().mark = Some(Pos::new(0, 2));
        ed.buf_mut().cursor = Pos::new(1, 6);
        ed.options.tabsize = 4;
        ed.execute(Action::Unindent);
        assert_eq!(ed.buf().line(0), "one");
        assert_eq!(ed.buf().line(1), "two");
        assert_eq!(
            ed.buf().mark,
            Some(Pos::new(0, 0)),
            "mark inside the indent lands at col 0"
        );
        assert_eq!(ed.buf().cursor, Pos::new(1, 2));
    }

    #[test]
    fn unindent_with_nothing_to_remove_is_a_silent_no_op() {
        let mut ed = test_editor("one\ntwo\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(1, 3);
        ed.execute(Action::Unindent);
        assert_eq!(ed.buf().to_string(), "one\ntwo\n");
        assert!(!ed.buf().modified);
        assert!(ed.buf().undo_stack.is_empty());
        assert_eq!(ed.status, None, "nano gives no feedback here either");
    }

    #[test]
    fn unindent_only_touches_lines_that_have_an_indent() {
        let mut ed = test_editor("one\n\ttwo\nthree\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 5);
        ed.execute(Action::Unindent);
        assert_eq!(ed.buf().to_string(), "one\ntwo\nthree\n");
        assert_eq!(
            ed.buf().cursor,
            Pos::new(2, 5),
            "unchanged: its line lost nothing"
        );
        ed.execute(Action::Undo);
        assert_eq!(ed.buf().to_string(), "one\n\ttwo\nthree\n");
    }

    #[test]
    fn indent_and_unindent_are_blocked_in_view_mode() {
        let mut ed = test_editor("\tone\n");
        ed.options.view = true;
        ed.execute(Action::Indent);
        ed.execute(Action::Unindent);
        assert_eq!(ed.buf().to_string(), "\tone\n");
        assert_eq!(ed.status.as_deref(), Some("Key is invalid in view mode"));
    }

    #[test]
    fn length_of_white_matches_nano() {
        assert_eq!(length_of_white("abc", 4), 0);
        assert_eq!(length_of_white("  abc", 4), 2);
        assert_eq!(length_of_white("      abc", 4), 4);
        assert_eq!(length_of_white("\tabc", 4), 1);
        assert_eq!(length_of_white("  \tabc", 4), 3);
        assert_eq!(
            length_of_white("   \t\tabc", 4),
            4,
            "reaches tabsize before the tab"
        );
        assert_eq!(length_of_white("  ", 4), 2, "an all-blank short line");
        assert_eq!(length_of_white("", 4), 0);
    }

    // ----- Comment / Uncomment (nano's do_comment) -----

    fn editor_for_language(text: &str, lang: &str) -> Editor {
        let mut ed = test_editor(text);
        ed.buf_mut().language =
            Some(crate::syntax::find_by_name(lang).unwrap_or_else(|| panic!("no language {lang}")));
        ed
    }

    #[test]
    fn comment_uses_hash_when_the_buffer_has_no_language() {
        let mut ed = test_editor("abc\n");
        ed.buf_mut().cursor = Pos::new(0, 2);
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().line(0), "#abc");
        assert_eq!(ed.buf().cursor, Pos::new(0, 3));
        assert_eq!(ed.status, None);
        assert!(ed.buf().modified);
    }

    #[test]
    fn comment_uses_the_languages_sequence_and_toggles_back() {
        let mut ed = editor_for_language("let x = 1;\n", "rust");
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().line(0), "//let x = 1;");
        assert_eq!(
            ed.buf().cursor,
            Pos::new(0, 0),
            "a cursor at column 0 stays"
        );
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().line(0), "let x = 1;");
    }

    #[test]
    fn comment_brackets_the_line_for_a_prefix_postfix_sequence() {
        let mut ed = editor_for_language("<p>hi</p>\n", "html");
        ed.buf_mut().cursor = Pos::new(0, 9);
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().line(0), "<!--<p>hi</p>-->");
        assert_eq!(
            ed.buf().cursor,
            Pos::new(0, 13),
            "shifted by the prefix only"
        );
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().line(0), "<p>hi</p>");
        assert_eq!(ed.buf().cursor, Pos::new(0, 9));
    }

    #[test]
    fn uncomment_clamps_a_cursor_left_stranded_by_the_removed_postfix() {
        let mut ed = editor_for_language("<!--x-->\n", "html");
        ed.buf_mut().cursor = Pos::new(0, 8);
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().line(0), "x");
        assert_eq!(ed.buf().cursor, Pos::new(0, 1));
    }

    #[test]
    fn comment_is_refused_for_a_language_without_one() {
        let mut ed = editor_for_language("{}\n", "json");
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().line(0), "{}");
        assert_eq!(
            ed.status.as_deref(),
            Some("Commenting is not supported for this file type")
        );
        assert!(!ed.buf().modified);
    }

    #[test]
    fn comment_refuses_the_magic_last_line_alone_but_skips_it_in_a_range() {
        let mut ed = test_editor("one\ntwo\n");
        ed.buf_mut().cursor = Pos::new(2, 0);
        ed.execute(Action::Comment);
        assert_eq!(
            ed.status.as_deref(),
            Some("Cannot comment past end of file")
        );
        assert_eq!(ed.buf().to_string(), "one\ntwo\n");

        ed.status = None;
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 0);
        ed.also_the_last = true; // make the range reach the last line
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "#one\n#two\n", "last line left alone");
        assert_eq!(ed.status, None);
    }

    #[test]
    fn comment_touches_the_last_line_under_nonewlines() {
        let mut ed = test_editor("one");
        ed.options.nonewlines = true;
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "#one");
    }

    #[test]
    fn mixed_range_gets_commented_and_fully_commented_range_uncommented() {
        let mut ed = test_editor("#one\ntwo\n\nthree\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(3, 5);
        ed.execute(Action::Comment);
        assert_eq!(
            ed.buf().to_string(),
            "##one\n#two\n#\n#three\n",
            "one uncommented line means comment all, blank lines included"
        );
        assert_eq!(ed.buf().cursor, Pos::new(3, 6));
        assert_eq!(ed.buf().mark, Some(Pos::new(0, 0)));

        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "#one\ntwo\n\nthree\n");
        assert_eq!(ed.buf().cursor, Pos::new(3, 5));
    }

    #[test]
    fn uncomment_leaves_blank_lines_alone_and_only_strips_column_zero_prefixes() {
        let mut ed = test_editor("#one\n\n#two\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 4);
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "one\n\ntwo\n");

        // An indented "#" isn't a comment prefix to nano, so this line
        // counts as uncommented and the range gets commented instead.
        let mut ed = test_editor("#one\n  #two\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(1, 6);
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "##one\n#  #two\n");
    }

    #[test]
    fn all_blank_range_gets_commented() {
        let mut ed = test_editor("\n  \nx\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 0);
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "#\n#  \nx\n");
    }

    #[test]
    fn comment_is_a_single_undo_step() {
        let mut ed = editor_for_language("a\nb\nc\n", "c");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 1);
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "//a\n//b\n//c\n");
        ed.execute(Action::Undo);
        assert_eq!(ed.buf().to_string(), "a\nb\nc\n");
        assert_eq!(ed.buf().cursor, Pos::new(2, 1));
        ed.execute(Action::Redo);
        assert_eq!(ed.buf().to_string(), "//a\n//b\n//c\n");
        assert_eq!(ed.buf().cursor, Pos::new(2, 3));
    }

    #[test]
    fn comment_is_blocked_in_view_mode() {
        let mut ed = test_editor("one\n");
        ed.options.view = true;
        ed.execute(Action::Comment);
        assert_eq!(ed.buf().to_string(), "one\n");
        assert_eq!(ed.status.as_deref(), Some("Key is invalid in view mode"));
    }

    // ----- Suspend hint (nano's suggest_ctrlT_ctrlZ) -----

    /// nano's `to_para_begin`/`to_para_end`, checked against the installed
    /// nano: a paragraph is a run of non-blank lines (as justify sees it);
    /// begin goes to the paragraph's first line, then to the previous
    /// paragraph's; end goes to the start of the line after the paragraph,
    /// then past the next one, and to the end of the last line at the end
    /// of the buffer.
    #[test]
    fn paragraph_begin_and_end_move_like_nano() {
        let text = "one a\none b\none c\n\ntwo a\ntwo b\n\n\nthree a\nthree b";
        let mut ed = test_editor(text);

        // Begin: from mid-paragraph to its first line, then back one
        // paragraph at a time, stopping at the first line.
        ed.buf_mut().cursor = Pos::new(5, 3);
        ed.execute(Action::BeginPara);
        assert_eq!(ed.buf().cursor, Pos::new(4, 0));
        ed.execute(Action::BeginPara);
        assert_eq!(ed.buf().cursor, Pos::new(0, 0));
        ed.execute(Action::BeginPara);
        assert_eq!(ed.buf().cursor, Pos::new(0, 0));
        // From a blank line, to the start of the paragraph before it.
        ed.buf_mut().cursor = Pos::new(7, 0);
        ed.execute(Action::BeginPara);
        assert_eq!(ed.buf().cursor, Pos::new(4, 0));

        // End: from mid-paragraph to the line after it, then on past the
        // next paragraph (skipping the blank lines before it), and at the
        // last paragraph to the end of the buffer's last line.
        ed.buf_mut().cursor = Pos::new(1, 2);
        ed.execute(Action::EndPara);
        assert_eq!(ed.buf().cursor, Pos::new(3, 0));
        ed.execute(Action::EndPara);
        assert_eq!(ed.buf().cursor, Pos::new(6, 0));
        ed.execute(Action::EndPara);
        assert_eq!(ed.buf().cursor, Pos::new(9, "three b".len()));
        ed.execute(Action::EndPara);
        assert_eq!(ed.buf().cursor, Pos::new(9, "three b".len()));
    }

    /// `quotestr` decides paragraph membership for these moves as it does
    /// for justify: a change of quote prefix starts a new paragraph.
    #[test]
    fn paragraph_moves_respect_quotestr() {
        let text = "> q1\n> q2\nplain\n";
        let mut ed = test_editor(text);
        ed.buf_mut().cursor = Pos::new(2, 3);
        ed.execute(Action::BeginPara);
        assert_eq!(
            ed.buf().cursor,
            Pos::new(0, 0),
            "the quoted lines are one paragraph"
        );
        ed.execute(Action::EndPara);
        assert_eq!(
            ed.buf().cursor,
            Pos::new(2, 0),
            "the quoted paragraph ends before `plain`"
        );
        ed.execute(Action::EndPara);
        assert_eq!(ed.buf().cursor, Pos::new(3, 0));
    }

    /// A paragraph jump that lands off-screen centers the cursor, as
    /// nano's `edit_redraw(..., CENTERING)` does for these moves.
    #[test]
    fn paragraph_end_centers_offscreen_landing() {
        let text = (0..60).map(|i| format!("line{i}\n")).collect::<String>();
        let mut ed = test_editor(&text);
        ed.screen_rows = 24;
        ed.buf_mut().cursor = Pos::new(0, 0);
        ed.buf_mut().top_line = 0;
        ed.execute(Action::EndPara);
        let rows = ed.text_rows();
        assert_eq!(ed.buf().cursor, Pos::new(60, 0));
        assert_eq!(ed.buf().top_line, 60 - rows / 2);
    }

    fn editor_with_default_keys(modern: bool) -> Editor {
        let mut ed = test_editor("x");
        ed.keymap = KeyMap::defaults(modern);
        ed
    }

    #[test]
    fn plain_ctrl_z_hints_at_ctrl_t_ctrl_z_with_the_default_keys() {
        use crate::keymap::{Binding, Key};
        let mut ed = editor_with_default_keys(false);
        assert!(matches!(
            ed.keymap.lookup(Menu::Main, Key::Ctrl('Z')),
            Some(Binding::Action(Action::SuggestSuspend))
        ));
        assert!(matches!(
            ed.keymap.lookup(Menu::Execute, Key::Ctrl('Z')),
            Some(Binding::Action(Action::Suspend))
        ));
        ed.execute(Action::SuggestSuspend);
        assert_eq!(ed.status.as_deref(), Some("To suspend, type ^T^Z"));
        assert!(matches!(ed.status_level, StatusLevel::Mild));
        assert!(!ed.bell_pending, "AHEM in nano: no beep");
    }

    #[test]
    fn modern_bindings_put_undo_on_ctrl_z_instead_of_the_hint() {
        use crate::keymap::{Binding, Key};
        let ed = editor_with_default_keys(true);
        assert!(matches!(
            ed.keymap.lookup(Menu::Main, Key::Ctrl('Z')),
            Some(Binding::Action(Action::Undo))
        ));
    }

    #[test]
    fn suspend_hint_is_withheld_once_either_key_is_rebound() {
        use crate::keymap::{Binding, Key};
        let mut ed = editor_with_default_keys(false);
        ed.keymap.bind(
            Menu::Main,
            Key::Ctrl('T'),
            Binding::Action(Action::GotoLine),
        );
        ed.execute(Action::SuggestSuspend);
        assert_eq!(ed.status, None, "^T no longer opens the Execute menu");

        let mut ed = editor_with_default_keys(false);
        ed.keymap.unbind(Menu::Execute, Key::Ctrl('Z'));
        ed.execute(Action::SuggestSuspend);
        assert_eq!(ed.status, None, "^Z no longer suspends from that menu");
    }

    #[test]
    fn whitespace_display_toggle_reports_like_nanos_do_toggle() {
        let mut ed = test_editor("x");
        assert!(!ed.options.whitespacedisplay);
        ed.execute(Action::WhitespaceDisplay);
        assert!(ed.options.whitespacedisplay);
        assert_eq!(ed.status.as_deref(), Some("Whitespace display enabled"));
        assert!(matches!(ed.status_level, StatusLevel::Normal));
        ed.execute(Action::WhitespaceDisplay);
        assert!(!ed.options.whitespacedisplay);
        assert_eq!(ed.status.as_deref(), Some("Whitespace display disabled"));
    }

    #[test]
    fn genuinely_inert_actions_report_plainly_instead_of_doing_nothing() {
        for (action, expected) in [
            (Action::Center, "center: not yet implemented"),
            (Action::Cycle, "cycle: not yet implemented"),
            (Action::Verbatim, "verbatim input: not yet implemented"),
        ] {
            let mut ed = test_editor("hello");
            ed.execute(action);
            assert_eq!(ed.status.as_deref(), Some(expected), "{action:?}");
        }
    }

    // `set minibar`

    #[test]
    fn minibar_note_says_lines_singular_and_tags_dos_mac_format() {
        assert_eq!(
            minibar_linecount_note(1, crate::buffer::LineFormat::Unix),
            "(1 line)"
        );
        assert_eq!(
            minibar_linecount_note(3, crate::buffer::LineFormat::Unspecified),
            "(3 lines)"
        );
        assert_eq!(
            minibar_linecount_note(3, crate::buffer::LineFormat::Dos),
            "(3 lines, DOS)"
        );
        assert_eq!(
            minibar_linecount_note(1, crate::buffer::LineFormat::Mac),
            "(1 line, Mac)"
        );
    }

    #[test]
    fn minibar_shrinks_text_rows_by_the_title_bar_only() {
        let mut ed = test_editor("x");
        ed.screen_rows = 24;
        let baseline = ed.text_rows(); // title(1) + status(1) + help(2)
        ed.options.minibar = true;
        // Losing just the title bar gains the buffer exactly one row back:
        // the minibar itself still occupies the status row, and the
        // shortcut bar is untouched (unlike nano's own `--zero`).
        assert_eq!(ed.text_rows(), baseline + 1);

        ed.options.nohelp = true;
        let baseline_nohelp = {
            let mut plain = test_editor("x");
            plain.screen_rows = 24;
            plain.options.nohelp = true;
            plain.text_rows()
        };
        assert_eq!(ed.text_rows(), baseline_nohelp + 1);
    }

    #[test]
    fn switching_buffers_sets_the_minibar_note_to_the_new_buffers_linecount() {
        let mut ed = test_editor("one\ntwo\nthree\n");
        ed.buffers.push(Buffer::from_text("only one line", None));
        ed.minibar_note = None;
        ed.execute(Action::NextBuf);
        assert_eq!(ed.current, 1);
        assert_eq!(ed.minibar_note.as_deref(), Some("(1 line)"));
    }

    #[test]
    fn switching_with_only_one_buffer_open_leaves_the_note_untouched() {
        let mut ed = test_editor("x");
        ed.minibar_note = Some("unchanged".to_string());
        ed.execute(Action::NextBuf);
        assert_eq!(ed.minibar_note.as_deref(), Some("unchanged"));
    }
}
