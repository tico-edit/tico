//! Terminal rendering and the interactive event loop, built on crossterm.
//! The on-screen layout (title bar, buffer, status line, two-line shortcut
//! bar) mirrors GNU nano's, with "tico" shown wherever nano would show its
//! own name.

use crate::app::{DiffOutcome, Editor, Mode, Prompt, PromptKind};
use crate::buffer::Pos;
use crate::keymap::{Action, Binding, Key as TKey, KeyMap, Menu};
use crate::theme::{Style, print_styled};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{
    Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use crossterm::{execute, queue};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> io::Result<RawModeGuard> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(RawModeGuard)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // Harmless even if mouse capture was never turned on -- terminals
        // silently ignore a mode-reset escape they weren't in.
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

/// Keeps a `watch::FileWatcher` pointed at whichever file the current
/// buffer has open, recreating it whenever that changes (buffer switch,
/// load, save-as, ...). When native watching isn't available for the
/// current path (unsupported platform, no path yet, ...), `watcher` is
/// `None` and the caller should keep doing its own periodic check.
struct DiskWatch {
    watcher: Option<crate::watch::FileWatcher>,
    path: Option<std::path::PathBuf>,
}

impl DiskWatch {
    fn new() -> DiskWatch {
        DiskWatch {
            watcher: None,
            path: None,
        }
    }

    fn sync(&mut self, current: Option<&std::path::Path>) {
        if self.path.as_deref() != current {
            self.path = current.map(|p| p.to_path_buf());
            self.watcher = current.and_then(crate::watch::FileWatcher::new);
        }
    }
}

pub fn run(editor: &mut Editor) -> io::Result<()> {
    let _guard = RawModeGuard::new()?;
    if let Ok((cols, rows)) = size() {
        editor.screen_cols = cols as usize;
        editor.screen_rows = rows as usize;
    }

    // Full clear happens exactly once (here) and again on resize; every
    // other render overwrites each row's full width in place, so nothing
    // ever needs re-blanking (which is what caused the visible flicker:
    // clearing the whole screen before every redraw, even when idle).
    execute!(io::stdout(), Clear(ClearType::All))?;
    // `set mouse` (`-m`, or `M-M` live): tracked separately from
    // `editor.options.mouse` itself, since that's just a plain bool the
    // rest of the editor toggles freely -- this is the one place that
    // needs to know whether the *terminal* is currently in mouse-capture
    // mode, so it can tell when to (de)activate it.
    let mut mouse_capture_enabled = false;
    sync_mouse_capture(editor, &mut mouse_capture_enabled)?;
    render_and_ring(editor)?;

    let mut disk_watch = DiskWatch::new();

    loop {
        if matches!(editor.mode, Mode::Quit) {
            break;
        }

        let mut dirty = false;

        if event::poll(Duration::from_millis(600))? {
            // A terminal delivers a clipboard paste as a burst of individual
            // synthetic keystrokes, not one chunk (nano has no bracketed-paste
            // support to ask it to do otherwise). Redrawing after every single
            // one made a paste of any size crawl, especially on Windows where
            // each terminal write costs more than on a Unix pty. So drain
            // whatever's already queued and render once for the whole batch,
            // capped so a very long paste (or a held-down repeating key)
            // still redraws periodically instead of looking frozen.
            const MAX_BATCHED_EVENTS: u32 = 512;
            let mut batched = 0u32;
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        handle_key(editor, key);
                        dirty = true;
                    }
                    Event::Mouse(mev) => {
                        handle_mouse(editor, mev);
                        dirty = true;
                    }
                    Event::Resize(cols, rows) => {
                        editor.screen_cols = cols as usize;
                        editor.screen_rows = rows as usize;
                        execute!(io::stdout(), Clear(ClearType::All))?;
                        dirty = true;
                    }
                    _ => {}
                }
                if matches!(editor.mode, Mode::Quit) {
                    break;
                }
                batched += 1;
                if batched >= MAX_BATCHED_EVENTS || !event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
        } else if matches!(editor.mode, Mode::Editing) {
            let watched_path = if editor.buf().ignore_external_changes {
                None
            } else {
                editor.buf().path.as_deref()
            };
            disk_watch.sync(watched_path);
            // With a live native watcher, only bother running the (still
            // cheap, but not free) stat-based check when it actually
            // flagged something; without one (unsupported platform, or
            // setup failed for this path), fall back to the previous
            // behavior of checking on every idle poll.
            let should_check = match &disk_watch.watcher {
                Some(w) => w.take_changed(),
                None => true,
            };
            if should_check {
                dirty = maybe_check_external_change(editor);
            }
            dirty |= editor.tick_spotlight_deadline();
        }

        // Check Quit before rendering: an action (e.g. Exit with no
        // unsaved changes) may have just closed the last buffer, and
        // render() assumes there's always at least one to draw.
        if matches!(editor.mode, Mode::Quit) {
            break;
        }

        sync_mouse_capture(editor, &mut mouse_capture_enabled)?;

        show_brief_warning(editor)?;

        if dirty {
            render_and_ring(editor)?;
        }
    }
    Ok(())
}

/// (De)activate the terminal's mouse-tracking mode to match
/// `editor.options.mouse`, whenever it's just been toggled (`M-M`, or a
/// nanorc/ticorc reload) -- a no-op otherwise.
fn sync_mouse_capture(editor: &Editor, enabled: &mut bool) -> io::Result<()> {
    if editor.options.mouse == *enabled {
        return Ok(());
    }
    if editor.options.mouse {
        execute!(io::stdout(), EnableMouseCapture)?;
    } else {
        execute!(io::stdout(), DisableMouseCapture)?;
    }
    *enabled = editor.options.mouse;
    Ok(())
}

/// Render, then ring the terminal bell exactly once if an Alert-level
/// message was just posted (matching nano's beep() for ALERT-importance
/// statusline() calls).
fn render_and_ring(editor: &mut Editor) -> io::Result<()> {
    maybe_warn_highlighting_disabled_for_size(editor);
    render(editor)?;
    ring_pending_bell(editor)
}

fn ring_pending_bell(editor: &mut Editor) -> io::Result<()> {
    if editor.bell_pending {
        editor.bell_pending = false;
        print!("\x07");
        io::stdout().flush()?;
    }
    Ok(())
}

/// nano's `warn_and_briefly_pause`: blank the shortcut bars, show the
/// queued `brief_warning` as an Alert (errorcolor plus the bell), and hold
/// it on screen for 1.5s so it can be read before the mode set up behind
/// it (e.g. the "Save modified buffer?" prompt) is drawn. Keys typed
/// meanwhile stay queued for that mode, as they do across nano's `napms`.
/// The message is dropped afterward, like nano's `lastmessage = VACUUM`.
fn show_brief_warning(editor: &mut Editor) -> io::Result<()> {
    let Some(msg) = editor.brief_warning.take() else {
        return Ok(());
    };
    let mode = std::mem::replace(&mut editor.mode, Mode::Editing);
    editor.set_status_alert(msg);
    render_frame(editor, true)?;
    ring_pending_bell(editor)?;
    std::thread::sleep(Duration::from_millis(1500));
    editor.mode = mode;
    editor.status = None;
    Ok(())
}

/// One-time "syntax highlighting disabled: file too large" notice for the
/// current buffer, the first time it's found over
/// `options.max_syntax_highlight_bytes` (checked on every render, but the
/// check itself is just a length read, and `highlighting_size_warning_shown`
/// keeps it from repeating). Runs before render() so the notice shows up
/// in the very frame that would otherwise have silently skipped
/// highlighting -- including the first frame right after opening a large
/// file, or right after switching to one.
fn maybe_warn_highlighting_disabled_for_size(editor: &mut Editor) {
    if !editor.options.syntax_highlighting || editor.buf().language.is_none() {
        return;
    }
    let max = editor.options.max_syntax_highlight_bytes;
    if editor.buf().rope.len_bytes() as u64 <= max || editor.buf().highlighting_size_warning_shown {
        return;
    }
    editor.buf_mut().highlighting_size_warning_shown = true;
    editor.set_status_mild(format!(
        "Syntax highlighting disabled: file is larger than {}",
        format_byte_size(max)
    ));
}

/// Render a byte count the way it was most likely configured -- whichever
/// of B/KB/MB/GB divides it evenly (falling back to plain bytes), so a
/// `max_syntax_highlight_size = 4MB` setting is echoed back as "4MB", not
/// "4194304 bytes".
fn format_byte_size(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;
    if bytes != 0 && bytes.is_multiple_of(GB) {
        format!("{}GB", bytes / GB)
    } else if bytes != 0 && bytes.is_multiple_of(MB) {
        format!("{}MB", bytes / MB)
    } else if bytes != 0 && bytes.is_multiple_of(KB) {
        format!("{}KB", bytes / KB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Build the "file changed on disk, you have unsaved edits" choice prompt —
/// shared by the initial detection and by backing out of the merge-diff
/// viewer, so both offer the same [R]eload/[K]eep/[M]erge/[I]gnore choice.
/// No separate [C]ancel: it was always identical to Keep mine in effect, so
/// Esc still works as the usual escape hatch (see handle_conflict_choice)
/// without being advertised as its own, redundant option.
fn external_conflict_prompt() -> Prompt {
    Prompt {
        kind: PromptKind::ExternalChangeConflict,
        menu: Menu::YesNo,
        label: "File changed on disk and you have unsaved edits: [R]eload  [K]eep mine  [M]erge  [I]gnore All"
            .to_string(),
        input: String::new(),
        cursor: 0,
        history_pos: None,
        saved_input: None,
    }
}

/// Returns true if editor state changed (and so needs a redraw).
fn maybe_check_external_change(editor: &mut Editor) -> bool {
    use crate::fileio::ExternalChange;
    if editor.buf().ignore_external_changes {
        return false;
    }
    match crate::fileio::check_external_change(editor.buf()) {
        ExternalChange::Unchanged => return false,
        ExternalChange::ChangedNoLocalEdits => {
            let (noconvert, unix) = (editor.options.noconvert, editor.options.unix);
            let _ = crate::fileio::reload(editor.buf_mut(), noconvert, unix);
            editor.set_status("File reloaded (changed on disk)");
        }
        ExternalChange::ChangedWithLocalEdits => {
            editor.mode = Mode::Prompt(external_conflict_prompt());
        }
    }
    true
}

// ---------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------

fn handle_key(editor: &mut Editor, key: KeyEvent) {
    match std::mem::replace(&mut editor.mode, Mode::Editing) {
        Mode::Editing => {
            editor.mode = Mode::Editing;
            // Matches nano's get_kbinput(): the countdown ticks once per
            // keystroke read from the main edit window, *before* that
            // keystroke is dispatched (so if the dispatch itself shows a
            // fresh message, this tick doesn't immediately eat into it).
            editor.tick_status_countdown();
            // A search match's highlight is cleared by the very next
            // keystroke, same as its status-message countdown above —
            // confirmed by timing the installed nano (a key press ends its
            // half-delay wait immediately, rather than waiting out the
            // ~1.5s timeout). Any spotlight still around here is a timed
            // one; the persistent replace-confirm kind only exists while
            // in Mode::Prompt, not here.
            editor.clear_spotlight();
            // `set minibar`'s one-shot line-count note is likewise good for
            // exactly one keystroke -- cleared here, before dispatch, so a
            // fresh note the dispatch itself sets (e.g. `M->` switching
            // buffers) survives to be shown, same ordering as the two
            // clears above.
            editor.minibar_note = None;
            handle_editing_key(editor, key);
        }
        Mode::Help {
            lines,
            top,
            return_to,
        } => {
            handle_help_key(editor, lines, top, return_to, key);
        }
        Mode::Diff {
            lines,
            top,
            outcome,
        } => {
            handle_diff_key(editor, lines, top, outcome, key);
        }
        Mode::Prompt(prompt) => handle_prompt_key(editor, prompt, key),
        Mode::Browser => {
            editor.mode = Mode::Browser;
            handle_browser_key(editor, key);
        }
        Mode::Quit => editor.mode = Mode::Quit,
    }
}

// ---------------------------------------------------------------------
// Mouse handling (`set mouse`)
// ---------------------------------------------------------------------

/// Handle a mouse event, matching nano's own `get_mouseinput`/
/// `process_click` (confirmed against the installed nano's own source):
/// left-clicks place the cursor (or toggle the mark, when the click
/// resolves to the cursor's own position unchanged), clicks on the
/// scrollbar (`set indicator`) jump proportionally, clicks on a shortcut
/// in the two-line bar activate it, and the wheel scrolls two lines per
/// notch. Everything else (drags, other buttons, plain motion, ...) is
/// ignored, same as nano -- in particular, this leaves Shift+drag free for
/// the terminal's own native text selection, which is how nano itself
/// expects dragging to work (see `set mouse` in `nanorc(5)`): nano's own
/// mouse handling has no drag/motion case at all.
fn handle_mouse(editor: &mut Editor, mev: MouseEvent) {
    if !editor.options.mouse {
        return;
    }
    // In the browser the wheel moves the selection a row per notch-line.
    if matches!(editor.mode, Mode::Browser) {
        let action = match mev.kind {
            MouseEventKind::ScrollUp => Some(Action::Up),
            MouseEventKind::ScrollDown => Some(Action::Down),
            _ => None,
        };
        if let Some(action) = action {
            apply_browser_action(editor, action);
            apply_browser_action(editor, action);
            return;
        }
    }
    match mev.kind {
        // "One bump of the mouse wheel should scroll two lines" (nano's
        // own comment in get_mouseinput).
        MouseEventKind::ScrollUp => {
            editor.execute(Action::ScrollUp);
            editor.execute(Action::ScrollUp);
        }
        MouseEventKind::ScrollDown => {
            editor.execute(Action::ScrollDown);
            editor.execute(Action::ScrollDown);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            handle_click(editor, mev.row as usize, mev.column as usize);
        }
        _ => {}
    }
}

/// Dispatch a left-click at 0-based screen `(row, col)` to whichever
/// region it landed in. The main editing screen and every prompt share one
/// layout (`main_screen_layout`); the full-screen Help and Diff viewers
/// only expose their own bottom shortcut bar.
fn handle_click(editor: &mut Editor, row: usize, col: usize) {
    if editor.browser.is_some() && matches!(&editor.mode, Mode::Browser | Mode::Prompt(_)) {
        handle_browser_click(editor, row, col);
        return;
    }
    if matches!(&editor.mode, Mode::Editing | Mode::Prompt(_)) {
        handle_main_screen_click(editor, row, col);
        return;
    }
    let bar_row = editor.screen_rows.saturating_sub(2);
    if row < bar_row {
        return;
    }
    let entries = match &editor.mode {
        Mode::Help { .. } => resolve_shortcuts(&editor.keymap, Menu::Help, HELP_SHORTCUTS),
        Mode::Diff { outcome, .. } => diff_shortcut_entries(outcome),
        _ => return,
    };
    activate_shortcut_click(editor, &entries, row - bar_row, col);
}

/// A click within the main editing screen's own layout (shared by
/// `Mode::Editing` and every `Mode::Prompt`, since a prompt overlays only
/// the status row -- clicking the buffer or the shortcut bar still works
/// while one is open, matching nano's own `process_click`, which only
/// special-cases the prompt row itself into a no-op).
fn handle_main_screen_click(editor: &mut Editor, row: usize, col: usize) {
    if editor.options.zero {
        handle_buffer_click(editor, row, col, editor.screen_rows);
        return;
    }
    let layout = main_screen_layout(editor);
    if row >= layout.text_start_row && row < layout.text_start_row + layout.text_rows {
        handle_buffer_click(editor, row - layout.text_start_row, col, layout.text_rows);
    } else if layout.help_rows > 0 && row > layout.status_row {
        let row_in_bar = row - layout.status_row - 1;
        let prompt = if let Mode::Prompt(p) = &editor.mode {
            Some(p)
        } else {
            None
        };
        let entries = shortcut_bar_entries(&editor.keymap, prompt);
        activate_shortcut_click(editor, &entries, row_in_bar, col);
    }
    // A click on the title row or the status/prompt row itself is a
    // no-op, matching nano exactly.
}

/// A click within the buffer area proper (0-based `row_in_buffer` counts
/// from the top of the visible text, not the whole screen). `editwinrows`
/// is however many rows that area has, matching what `render_buffer` was
/// given -- needed for the scrollbar's own proportion math.
fn handle_buffer_click(editor: &mut Editor, row_in_buffer: usize, col: usize, editwinrows: usize) {
    let cols = editor.screen_cols;
    let sidebar = usize::from(editor.options.indicator && cols > 9 && editor.screen_rows > 5);

    if sidebar == 1 && col + 1 == cols {
        // Clicking the scrollbar jumps to the roughly corresponding line,
        // matching nano's own click-to-scrollbar math exactly (`row 0` ->
        // the very top; any other row rounds up by one first).
        let total = editor.buf().line_count().max(1);
        let adjusted_row = if row_in_buffer == 0 {
            0
        } else {
            row_in_buffer + 1
        };
        let target_line = (total * adjusted_row / editwinrows.max(1)).min(total - 1);
        editor.buf_mut().cursor.line = target_line;
        let len = editor.buf().line(target_line).chars().count();
        editor.buf_mut().cursor.col = editor.buf().cursor.col.min(len);
        editor.scroll_to_cursor_centered();
        return;
    }

    let gutter = editor.gutter_width();
    let tabsize = editor.options.tabsize as usize;
    let buf = editor.buf();
    let line_idx = (buf.top_line + row_in_buffer).min(buf.line_count().saturating_sub(1));
    let is_cursor_line = line_idx == buf.cursor.line;
    // Only the cursor's own line is ever horizontally scrolled (see
    // `render_buffer`), so a click on any other row starts counting
    // columns from display column 0.
    let content_col = col.saturating_sub(gutter);
    let target_display_col = if is_cursor_line {
        buf.left_col + content_col
    } else {
        content_col
    };
    let raw = buf.line(line_idx);
    let char_col = crate::buffer::char_col_for_display(&raw, target_display_col, tabsize);
    let was_line = buf.cursor.line;
    let was_col = buf.cursor.col;

    editor.buf_mut().cursor = Pos::new(line_idx, char_col);
    editor.scroll_to_cursor();

    // Clicking exactly where the cursor already was toggles the mark,
    // matching nano's own `process_click` (it's not click-timing based —
    // literally just "the click didn't move the cursor").
    if line_idx == was_line && char_col == was_col {
        editor.execute(Action::Mark);
    }
}

/// Resolve a click at `(row_in_bar, col)` -- 0-based, relative to the
/// shortcut bar's own top-left corner -- against `entries`, and, if it
/// lands on a real one, activate it by converting its displayed key label
/// back into the equivalent keystroke and dispatching that through the
/// exact same path a real keypress would take (matching nano's own
/// "put the keystroke back" mechanism).
fn activate_shortcut_click(
    editor: &mut Editor,
    entries: &[(String, &str)],
    row_in_bar: usize,
    col: usize,
) {
    let cols = editor.screen_cols;
    if let Some(idx) = shortcut_bar_click_index(cols, entries, row_in_bar, col)
        && let Some(kev) = synthetic_key_event_for_label(&entries[idx].0)
    {
        handle_key(editor, kev);
    }
}

/// The crossterm `KeyEvent` that, fed through `handle_key`, has the same
/// effect as the shortcut bar's displayed label for one entry -- either a
/// real `Key` spec (`^G`, `M-U`, `F1`, ...; parsed the same way a nanorc
/// `bind` line would be, then converted with `key_to_event`) or a bare
/// single character (`Y`, `N`, `A`, ...), which the Y/N/A-style
/// confirmation prompts match directly as a raw keystroke, outside the
/// rebindable keymap entirely.
fn synthetic_key_event_for_label(label: &str) -> Option<KeyEvent> {
    let mut chars = label.chars();
    let first = chars.next()?;
    if chars.next().is_none() && first != '^' {
        return Some(KeyEvent::new(KeyCode::Char(first), KeyModifiers::NONE));
    }
    TKey::parse(label).map(key_to_event)
}

/// The inverse of `normalize_key`: the crossterm `KeyEvent` that
/// `normalize_key` would turn back into `key`. Needed to replay a
/// shortcut-bar click as the keystroke it represents.
fn key_to_event(key: TKey) -> KeyEvent {
    let (code, modifiers) = match key {
        TKey::Ctrl(c) => (KeyCode::Char(c.to_ascii_lowercase()), KeyModifiers::CONTROL),
        TKey::Meta(c) => (KeyCode::Char(c), KeyModifiers::ALT),
        TKey::ShiftMeta(c) => (KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::SHIFT),
        TKey::F(n) => (KeyCode::F(n), KeyModifiers::NONE),
        TKey::Ins => (KeyCode::Insert, KeyModifiers::NONE),
        TKey::Del => (KeyCode::Delete, KeyModifiers::NONE),
        TKey::Backspace => (KeyCode::Backspace, KeyModifiers::NONE),
        TKey::ShiftTab => (KeyCode::BackTab, KeyModifiers::NONE),
        TKey::Left => (KeyCode::Left, KeyModifiers::NONE),
        TKey::Right => (KeyCode::Right, KeyModifiers::NONE),
        TKey::Up => (KeyCode::Up, KeyModifiers::NONE),
        TKey::Down => (KeyCode::Down, KeyModifiers::NONE),
        TKey::Home => (KeyCode::Home, KeyModifiers::NONE),
        TKey::End => (KeyCode::End, KeyModifiers::NONE),
        TKey::PageUp => (KeyCode::PageUp, KeyModifiers::NONE),
        TKey::PageDown => (KeyCode::PageDown, KeyModifiers::NONE),
        TKey::CtrlLeft => (KeyCode::Left, KeyModifiers::CONTROL),
        TKey::CtrlRight => (KeyCode::Right, KeyModifiers::CONTROL),
        TKey::CtrlUp => (KeyCode::Up, KeyModifiers::CONTROL),
        TKey::CtrlDown => (KeyCode::Down, KeyModifiers::CONTROL),
        TKey::CtrlHome => (KeyCode::Home, KeyModifiers::CONTROL),
        TKey::CtrlEnd => (KeyCode::End, KeyModifiers::CONTROL),
        TKey::CtrlDel => (KeyCode::Delete, KeyModifiers::CONTROL),
        TKey::ShiftCtrlDel => (KeyCode::Delete, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        TKey::MetaLeft => (KeyCode::Left, KeyModifiers::ALT),
        TKey::MetaRight => (KeyCode::Right, KeyModifiers::ALT),
        TKey::MetaUp => (KeyCode::Up, KeyModifiers::ALT),
        TKey::MetaDown => (KeyCode::Down, KeyModifiers::ALT),
        TKey::MetaHome => (KeyCode::Home, KeyModifiers::ALT),
        TKey::MetaEnd => (KeyCode::End, KeyModifiers::ALT),
        TKey::MetaPgUp => (KeyCode::PageUp, KeyModifiers::ALT),
        TKey::MetaPgDn => (KeyCode::PageDown, KeyModifiers::ALT),
        TKey::MetaIns => (KeyCode::Insert, KeyModifiers::ALT),
        TKey::MetaDel => (KeyCode::Delete, KeyModifiers::ALT),
    };
    KeyEvent::new(code, modifiers)
}

/// Handle a keystroke while the `^G` help viewer is open: scroll its body,
/// or close it (via `^X`/`^C`/Esc) and return to whatever was active
/// before — the main editing window, or the prompt help was opened from.
fn handle_help_key(
    editor: &mut Editor,
    lines: Vec<String>,
    top: usize,
    return_to: Option<Box<Prompt>>,
    key: KeyEvent,
) {
    let body_len = lines.len().saturating_sub(1);
    let body_rows = help_body_rows(editor);
    let max_top = body_len.saturating_sub(body_rows);
    let mut top = top.min(max_top);
    let mut close = false;

    if matches!(key.code, KeyCode::Esc) {
        close = true;
    } else if let Some(tkey) = normalize_key(key)
        && let Some(Binding::Action(action)) =
            editor.keymap.lookup_menu_only(Menu::Help, tkey).cloned()
    {
        match action {
            Action::Cancel => close = true,
            Action::Up => top = top.saturating_sub(1),
            Action::Down => top = (top + 1).min(max_top),
            Action::PageUp => top = top.saturating_sub(body_rows),
            Action::PageDown => top = (top + body_rows).min(max_top),
            Action::FirstLine => top = 0,
            Action::LastLine => top = max_top,
            _ => {}
        }
    }

    editor.mode = if close {
        match return_to {
            Some(prompt) => Mode::Prompt(*prompt),
            None if editor.browser.is_some() => Mode::Browser,
            None => Mode::Editing,
        }
    } else {
        Mode::Help {
            lines,
            top,
            return_to,
        }
    };
}

/// The movement actions Shift-selection applies to -- matches nano's
/// `wanted_to_move()`, the set of functions its own shift-held handling
/// treats as "just moving the cursor".
fn is_movement_action(action: Action) -> bool {
    matches!(
        action,
        Action::Left
            | Action::Right
            | Action::Up
            | Action::Down
            | Action::Home
            | Action::End
            | Action::PrevWord
            | Action::NextWord
            | Action::BeginPara
            | Action::EndPara
            | Action::PrevBlock
            | Action::NextBlock
            | Action::PageUp
            | Action::PageDown
            | Action::FirstLine
            | Action::LastLine
    )
}

fn handle_editing_key(editor: &mut Editor, key: KeyEvent) {
    // Shift-selection (nano's "soft mark"): crossterm reports Shift as a
    // modifier on the same key codes as plain movement (no separate
    // Shift+Left binding needed), so the resolved action is identical
    // either way -- only whether to also manage a mark around it differs.
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    editor.shift_held = false;
    let tkey = normalize_key(key);
    let binding = tkey.and_then(|tk| editor.keymap.lookup(Menu::Main, tk).cloned());
    let is_movement = matches!(&binding, Some(Binding::Action(a)) if is_movement_action(*a));

    if shift && is_movement && editor.buf().mark.is_none() {
        let cur = editor.buf().cursor;
        editor.buf_mut().mark = Some(cur);
        editor.buf_mut().softmark = true;
    }
    let before = editor.buf().cursor;

    if let Some(binding) = binding {
        apply_binding(editor, binding);
    } else if let KeyCode::Char(c) = key.code
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        if editor.options.view {
            editor.set_status_mild("Key is invalid in view mode");
        } else {
            editor.insert_char(c);
            // Plain self-insertion bypasses execute(), which is what
            // normally keeps the cursor in view (vertically and, for a
            // long line, horizontally) after an action.
            editor.scroll_to_cursor();
        }
    }
    editor.maybe_update_lock_modified_flag();

    // Any plain (non-Shift) movement or edit drops a soft mark -- matches
    // nano's own post-dispatch check (a hard mark, set via `^^`/`M-A`,
    // isn't touched here at all). An action that asked for the mark to be
    // kept despite shifting it (`shift_held`: indent/unindent) is exempt.
    if !editor.buffers.is_empty()
        && !shift
        && !editor.shift_held
        && editor.buf().softmark
        && editor.buf().mark.is_some()
        && (editor.buf().cursor != before || is_movement)
    {
        editor.buf_mut().mark = None;
        editor.buf_mut().softmark = false;
    }
}

fn apply_binding(editor: &mut Editor, binding: Binding) {
    match binding {
        // These need to run an external process (and, for the alt-speller
        // and formatter, hand the terminal over to it), which the
        // UI-agnostic `Editor::execute` can't do, so intercept them here
        // rather than dispatching through it (e.g. the default keymap's
        // `F12` for Speller, matching nano's own direct Main-menu binding).
        Binding::Action(Action::Speller) if editor.blocked_in_view_mode(Action::Speller) => {
            editor.set_status_mild("Key is invalid in view mode");
        }
        Binding::Action(Action::Formatter) if editor.blocked_in_view_mode(Action::Formatter) => {
            editor.set_status_mild("Key is invalid in view mode");
        }
        Binding::Action(Action::Speller) => run_speller(editor),
        Binding::Action(Action::Formatter) => run_formatter(editor),
        Binding::Action(Action::Linter) => run_linter(editor),
        // Only reachable via a `bind ... suspend main` in nanorc: nano's
        // default main-menu ^Z is the ^T^Z hint (Action::SuggestSuspend).
        Binding::Action(Action::Suspend) => suspend_editor(editor),
        Binding::Action(action) => editor.execute(action),
        Binding::Macro(text) => {
            // Literal-string bindings; `{function}` substitution is not yet
            // implemented, so braces are inserted literally.
            for c in text.chars() {
                editor.insert_char(c);
            }
        }
    }
}

fn handle_prompt_key(editor: &mut Editor, mut prompt: Prompt, key: KeyEvent) {
    // Single-keystroke choice prompts (yes/no/conflict resolution) are
    // handled directly, without going through the text-editing path.
    match &prompt.kind {
        PromptKind::Exit { .. } => return handle_exit_choice(editor, prompt, key),
        PromptKind::WriteConfirm { .. } => return handle_write_confirm_choice(editor, prompt, key),
        PromptKind::ExternalChangeConflict => return handle_conflict_choice(editor, prompt, key),
        PromptKind::LockConflict { .. } => return handle_lock_conflict_choice(editor, prompt, key),
        PromptKind::ReplaceConfirm(_) => return handle_replace_confirm_choice(editor, prompt, key),
        PromptKind::Linter { .. } => return handle_linter_choice(editor, prompt, key),
        _ => {}
    }

    // Any keystroke other than Tab drops a filename-completion listing
    // that's currently shown (matches nano: typing something else clears
    // the "(more)" grid rather than leaving it stale on screen).
    if !matches!(normalize_key(key), Some(TKey::Ctrl('I'))) {
        editor.file_completions = None;
    }

    if let Some(tkey) = normalize_key(key) {
        if tkey == TKey::Ctrl('C') || matches!(key.code, KeyCode::Esc) {
            editor.mode = Mode::Editing;
            // The file browser's own prompts go back to the browser.
            if matches!(
                prompt.kind,
                PromptKind::BrowserSearch { .. } | PromptKind::GotoDir
            ) {
                editor.mode = Mode::Browser;
            }
            // Canceling out of a spell-fix prompt stops the word-by-word
            // loop, but (matching nano's `fix_spello`/`spell_check`) still
            // reports success rather than "Cancelled".
            if matches!(prompt.kind, PromptKind::SpellFix { .. }) {
                editor.set_status("Finished checking spelling");
            } else {
                editor.set_status("Cancelled");
            }
            return;
        }
        if tkey == TKey::Ctrl('M') {
            submit_prompt(editor, prompt);
            return;
        }
        if tkey == TKey::Ctrl('H') || tkey == TKey::Backspace {
            // In `--modernbindings`, Ctrl+H is Help for most prompt menus
            // (physical Backspace is unaffected — always deletes); check
            // the live keymap rather than hardcoding editing here always,
            // so this stays correct if that binding is customized further.
            if tkey == TKey::Ctrl('H')
                && editor.keymap.lookup_menu_only(prompt.menu, tkey)
                    == Some(&Binding::Action(Action::Help))
            {
                apply_prompt_action(editor, &mut prompt, Action::Help);
                return;
            }
            if prompt.cursor > 0 {
                let idx = prompt
                    .input
                    .char_indices()
                    .nth(prompt.cursor - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                prompt.input.remove(idx);
                prompt.cursor -= 1;
            }
            prompt.history_pos = None;
            prompt.saved_input = None;
            editor.mode = Mode::Prompt(prompt);
            return;
        }
        // `Tab` at the `^R` Read File prompt (only in file-insert mode,
        // not Execute Command — matches nano's `MINSERTFILE` gate), and at
        // the browser's Go To Directory prompt, where it completes only
        // directories, relative to the one being browsed (`MGOTODIR`).
        if tkey == TKey::Ctrl('I') {
            let gotodir_base = match prompt.kind {
                PromptKind::GotoDir => editor.browser.as_ref().map(|b| b.list.dir.clone()),
                _ => None,
            };
            let completes = matches!(prompt.kind, PromptKind::InsertFile { execute: false, .. })
                || gotodir_base.is_some();
            if completes {
                apply_filename_completion(editor, &mut prompt, gotodir_base.as_deref());
                editor.mode = Mode::Prompt(prompt);
                return;
            }
        }
        if tkey == TKey::Left {
            prompt.cursor = prompt.cursor.saturating_sub(1);
            editor.mode = Mode::Prompt(prompt);
            return;
        }
        if tkey == TKey::Right {
            prompt.cursor = (prompt.cursor + 1).min(prompt.input.chars().count());
            editor.mode = Mode::Prompt(prompt);
            return;
        }
        // Menu-specific bindings (e.g. ^Y/^V to jump straight to the first
        // or last line from the Search/GotoLine prompts, without needing
        // to type anything) — see keymap.rs's install_prompt_defaults for
        // the full, source-verified list.
        if let Some(Binding::Action(action)) =
            editor.keymap.lookup_menu_only(prompt.menu, tkey).cloned()
        {
            if apply_prompt_action(editor, &mut prompt, action) {
                return;
            }
            editor.mode = Mode::Prompt(prompt);
            return;
        }
    }
    if let KeyCode::Char(c) = key.code
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        let idx = prompt
            .input
            .char_indices()
            .nth(prompt.cursor)
            .map(|(i, _)| i)
            .unwrap_or(prompt.input.len());
        prompt.input.insert(idx, c);
        prompt.cursor += 1;
        prompt.history_pos = None;
        prompt.saved_input = None;
    }
    editor.mode = Mode::Prompt(prompt);
}

/// Apply an Action bound within a prompt menu, which generally means
/// something different from its effect while editing the buffer directly.
/// Returns true if the prompt was closed (editor.mode has already been
/// set); false if it should stay open (the caller restores
/// `Mode::Prompt(prompt)`).
///
/// Only the bindings that make sense to act on immediately are handled
/// here; anything else recognized by the keymap but not listed below is
/// ignored rather than silently doing the wrong thing.
fn apply_prompt_action(editor: &mut Editor, prompt: &mut Prompt, action: Action) -> bool {
    match action {
        // `^G` opens the help screen for whichever prompt is currently up
        // (Search and Replace get their own text — see help.rs); closing
        // it (via handle_help_key) returns here to the same prompt.
        Action::Help => {
            let lines = crate::help::build(prompt.menu, &editor.keymap, editor.screen_cols);
            editor.mode = Mode::Help {
                lines,
                top: 0,
                return_to: Some(Box::new(prompt.clone())),
            };
            true
        }
        Action::FirstLine => {
            editor.buf_mut().cursor = Pos::new(0, 0);
            editor.scroll_to_cursor();
            editor.mode = Mode::Editing;
            true
        }
        Action::LastLine => {
            let last = editor.buf().line_count().saturating_sub(1);
            editor.buf_mut().cursor = Pos::new(last, 0);
            editor.scroll_to_cursor();
            editor.mode = Mode::Editing;
            true
        }
        // nano runs a bound movement function from the prompt and is then
        // done with it (`ask_for_line_and_column`: "when a function was
        // run, we're done"), so these leave the prompt like ^Y/^V do.
        Action::BeginPara => {
            editor.move_para_begin();
            editor.mode = Mode::Editing;
            true
        }
        Action::EndPara => {
            editor.move_para_end();
            editor.mode = Mode::Editing;
            true
        }
        Action::CaseSens => {
            editor.search.case_sensitive = !editor.search.case_sensitive;
            refresh_search_label(editor, prompt);
            false
        }
        Action::Regexp => {
            editor.search.use_regex = !editor.search.use_regex;
            refresh_search_label(editor, prompt);
            false
        }
        Action::Backwards => {
            editor.search.backwards = !editor.search.backwards;
            refresh_search_label(editor, prompt);
            false
        }
        // ^R in the Search prompt switches it into a replace operation
        // (keeping whatever was typed); ^R again from there switches back
        // to plain search — matches nano's flip_replace, bound to MWHEREIS
        // and MREPLACE both.
        Action::FlipReplace => {
            match prompt.menu {
                Menu::Search => {
                    prompt.kind = PromptKind::Replace1;
                    prompt.menu = Menu::Replace;
                }
                Menu::Replace => {
                    prompt.kind = PromptKind::WhereIs;
                    prompt.menu = Menu::Search;
                }
                _ => return false,
            }
            refresh_search_label(editor, prompt);
            false
        }
        // ^T flips between the Search and GotoLine prompts (MWHEREIS and
        // MGOTOLINE both bind it to flip_goto).
        Action::FlipGoto => {
            match prompt.menu {
                Menu::Search => {
                    prompt.kind = PromptKind::GotoLine;
                    prompt.menu = Menu::GotoLine;
                    prompt.label = "Enter line number, column number".to_string();
                }
                Menu::GotoLine => {
                    prompt.kind = PromptKind::WhereIs;
                    prompt.menu = Menu::Search;
                    refresh_search_label(editor, prompt);
                }
                _ => return false,
            }
            false
        }
        Action::Older => {
            cycle_history(editor, prompt, true);
            false
        }
        Action::Newer => {
            cycle_history(editor, prompt, false);
            false
        }
        // `M-F` at the Read File prompt: only the label wording changes
        // (confirmed against the installed nano — the shortcut-bar entry
        // itself always just reads "New Buffer", not a toggle-state pair
        // like FlipReplace's "Replace"/"No Replace").
        Action::FlipNewBuffer => {
            let PromptKind::InsertFile {
                new_buffer,
                execute,
            } = &mut prompt.kind
            else {
                return false;
            };
            *new_buffer = !*new_buffer;
            prompt.label =
                crate::app::insert_prompt_label(*new_buffer, *execute, editor.options.noconvert);
            false
        }
        // `^X` flips the Insert-File/Execute-Command prompt between its two
        // modes in place, keeping whatever was already typed (matches
        // nano's `flip_execute`, bound to the same key in both MINSERTFILE
        // and MEXECUTE).
        Action::FlipExecute => {
            let PromptKind::InsertFile {
                new_buffer,
                execute,
            } = &mut prompt.kind
            else {
                return false;
            };
            *execute = !*execute;
            prompt.menu = if *execute {
                Menu::Execute
            } else {
                Menu::Insert
            };
            prompt.label =
                crate::app::insert_prompt_label(*new_buffer, *execute, editor.options.noconvert);
            false
        }
        // `M-N` No Conversion: nano's `flip_convert` toggles the *global*
        // NO_CONVERT flag (it persists for later reads too, not just this
        // one), and the prompt's label says "unconverted" while it's on.
        Action::FlipConvert => {
            let PromptKind::InsertFile {
                new_buffer,
                execute,
            } = &prompt.kind
            else {
                return false;
            };
            editor.options.noconvert = !editor.options.noconvert;
            prompt.label =
                crate::app::insert_prompt_label(*new_buffer, *execute, editor.options.noconvert);
            false
        }
        // `M-D` DOS Format / `M-M` Mac Format at the Write Out prompt:
        // nano's `dos_format`/`mac_format` flip the buffer's own format
        // (so it sticks for later saves too) -- to that format, or back
        // to Unix if it already was that -- and re-show the prompt with
        // its " [DOS Format]"/" [Mac Format]" tag updated.
        Action::DosFormat | Action::MacFormat => {
            let PromptKind::WriteOut { flow } = prompt.kind else {
                return false;
            };
            use crate::buffer::LineFormat;
            let wanted = if action == Action::DosFormat {
                LineFormat::Dos
            } else {
                LineFormat::Mac
            };
            let buf = editor.buf_mut();
            buf.format = if buf.format == wanted {
                LineFormat::Unix
            } else {
                wanted
            };
            prompt.label = editor.writeout_prompt_label(flow.exiting);
            false
        }
        // `^T` Browse, at the Read File and Write Out prompts: nano's
        // `to_files`. The prompt waits behind the browser, to get the
        // chosen filename or to be shown again when the browser is left.
        Action::Browser => {
            let from_file_prompt = matches!(
                prompt.kind,
                PromptKind::InsertFile { execute: false, .. } | PromptKind::WriteOut { .. }
            );
            from_file_prompt && open_browser(editor, prompt)
        }
        // `^Y`/`^V` (`M-\`/`M-/`) at the browser's Search prompt select the
        // first/last name without searching -- though what was typed is
        // still remembered as the search string, as nano's
        // `search_filename` does for any function key.
        Action::FirstFile | Action::LastFile => {
            if !matches!(prompt.kind, PromptKind::BrowserSearch { .. }) {
                return false;
            }
            remember_browser_search(editor, &prompt.input);
            if let Some(session) = editor.browser.as_mut() {
                session.list.navigate(action, 1, 1);
            }
            editor.status = None;
            editor.mode = Mode::Browser;
            true
        }
        // Recognized (bound, shown in the shortcut bar and ^G help) but not
        // actually implemented yet: report that plainly rather than either
        // hiding the option or silently doing nothing when pressed. Status
        // messages don't show while a prompt is up (the status line is the
        // prompt itself), so this closes the prompt to make the message
        // visible, same as a real result would.
        Action::Append => {
            editor.mode = Mode::Editing;
            editor.set_status("Append: not yet implemented");
            true
        }
        Action::Prepend => {
            editor.mode = Mode::Editing;
            editor.set_status("Prepend: not yet implemented");
            true
        }
        Action::Backup => {
            editor.mode = Mode::Editing;
            editor.set_status("Backup File: not yet implemented");
            true
        }
        // `^T`/`^Y`/`^O` from within the Insert-File/Execute-Command
        // prompt run the tool immediately, ignoring whatever was typed —
        // matches nano's `ran_a_tool` flag, which makes `insert_a_file_or`
        // break out of its loop (closing the prompt) as soon as one of
        // these fires.
        Action::Speller => {
            editor.mode = Mode::Editing;
            if editor.blocked_in_view_mode(Action::Speller) {
                editor.set_status_mild("Key is invalid in view mode");
            } else {
                run_speller(editor);
            }
            true
        }
        Action::Formatter => {
            editor.mode = Mode::Editing;
            if editor.blocked_in_view_mode(Action::Formatter) {
                editor.set_status_mild("Key is invalid in view mode");
            } else {
                run_formatter(editor);
            }
            true
        }
        Action::Linter => {
            editor.mode = Mode::Editing;
            run_linter(editor);
            true
        }
        // Bound (matching nano's full MEXECUTE menu, so the shortcut bar
        // and ^G help show them) but not actually implemented: same
        // plain-report convention as FlipConvert/Browser above.
        Action::FullJustify => {
            editor.mode = Mode::Editing;
            editor.set_status("Full Justify: not yet implemented");
            true
        }
        Action::CutRestOfFile => {
            editor.mode = Mode::Editing;
            editor.set_status("Cut Till End: not yet implemented");
            true
        }
        Action::FlipPipe => {
            editor.mode = Mode::Editing;
            editor.set_status("Pipe Text: not yet implemented");
            true
        }
        // `^Z` from the Execute prompt: the real thing. Like the tools
        // above, nano's `ran_a_tool` closes the prompt first.
        Action::Suspend => {
            editor.mode = Mode::Editing;
            suspend_editor(editor);
            true
        }
        _ => false,
    }
}

/// `Tab` at the `^R` Read File prompt: complete the typed fragment to the
/// longest common prefix among matching directory entries (nano's
/// `input_tab`/`filename_completion`), or, when the fragment starts with
/// `~` and contains no `/` yet, among system usernames instead (nano's
/// `username_completion`) — and list them all in `editor.file_completions`
/// when there's more than one, rendered as a grid in place of the buffer
/// (`render_completions_grid`).
///
/// With `browsed` (the browser's Go To Directory prompt), a relative
/// fragment is looked up in that directory rather than the working one,
/// and only directories are candidates.
fn apply_filename_completion(
    editor: &mut Editor,
    prompt: &mut Prompt,
    browsed: Option<&std::path::Path>,
) {
    // Matches nano: completion only applies at the end of the input.
    if prompt.cursor != prompt.input.chars().count() {
        return;
    }
    let morsel = prompt.input.clone();

    if morsel.starts_with('~') && !morsel.contains('/') {
        apply_username_completion(editor, prompt, &morsel);
        return;
    }

    let (dir_part, fragment) = match morsel.rfind('/') {
        Some(i) => (morsel[..=i].to_string(), morsel[i + 1..].to_string()),
        None => (String::new(), morsel.clone()),
    };
    // The directory is resolved with `~` expanded, but the completed text
    // keeps whatever the user actually typed (so `~/Doc<Tab>` completes to
    // `~/Documents/`, not the expanded home path).
    let expanded_dir = crate::fileio::expand_leading_tilde(&dir_part);
    let base = browsed.unwrap_or(std::path::Path::new("."));
    let dir_path = if std::path::Path::new(&expanded_dir).is_absolute() {
        std::path::PathBuf::from(&expanded_dir)
    } else {
        base.join(&expanded_dir)
    };
    let Ok(entries) = std::fs::read_dir(&dir_path) else {
        return;
    };
    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(&fragment))
        .filter(|name| browsed.is_none() || dir_path.join(name).is_dir())
        .collect();
    if matches.is_empty() {
        return;
    }
    matches.sort();

    let mut common = matches[0].clone();
    for m in &matches[1..] {
        common = common_prefix(&common, m);
    }

    let mut new_input = format!("{dir_part}{common}");
    if matches.len() == 1 && dir_path.join(&common).is_dir() {
        new_input.push('/');
    }
    if new_input != morsel {
        prompt.input = new_input;
        prompt.cursor = prompt.input.chars().count();
    }
    if matches.len() > 1 {
        editor.file_completions = Some(matches);
    }
}

/// `Tab` on a bare `~fragment` (no `/` yet): complete against system
/// usernames instead of filenames — matches nano's `username_completion`.
/// Unlike plain filename completion, a single match never gets a trailing
/// `/` appended (nano doesn't either: the completed `~name` isn't itself a
/// real path to check `is_dir` against), so finishing into that user's
/// home directory still takes one more keystroke plus a further Tab.
fn apply_username_completion(editor: &mut Editor, prompt: &mut Prompt, morsel: &str) {
    let matches = username_completion_matches(&crate::fileio::list_usernames(), &morsel[1..]);
    if matches.is_empty() {
        return;
    }

    let mut common = matches[0].clone();
    for m in &matches[1..] {
        common = common_prefix(&common, m);
    }
    if common != morsel {
        prompt.input = common;
        prompt.cursor = prompt.input.chars().count();
    }
    if matches.len() > 1 {
        editor.file_completions = Some(matches);
    }
}

/// The `~`-prefixed usernames (sorted) among `users` that start with
/// `fragment` — the pure matching logic behind `apply_username_completion`,
/// kept separate from `list_usernames()`'s real system lookup so it can be
/// tested against a fixed, portable list instead of the actual (and
/// environment-dependent) `/etc/passwd`.
fn username_completion_matches(users: &[String], fragment: &str) -> Vec<String> {
    let mut matches: Vec<String> = users
        .iter()
        .filter(|name| name.starts_with(fragment))
        .map(|name| format!("~{name}"))
        .collect();
    matches.sort();
    matches
}

/// The longest common leading substring of `a` and `b`.
fn common_prefix(a: &str, b: &str) -> String {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect()
}

/// Recall history at a Search/Replace/ReplaceWith/Execute prompt with
/// Older (Up/^P) or Newer (Down/^N) — matches nano's get_older_item()/
/// get_newer_item(), confirmed against the installed nano: Older steps
/// backward through that menu's history (most recent first), Newer steps
/// forward, and stepping Newer past the most recent entry restores
/// whatever was live-typed before browsing started.
fn cycle_history(editor: &mut Editor, prompt: &mut Prompt, older: bool) {
    let list: &[String] = match prompt.menu {
        Menu::Search | Menu::Replace | Menu::WhereIsFile => &editor.history.search,
        Menu::ReplaceWith => &editor.history.replace,
        Menu::Execute => &editor.history.execute,
        _ => return,
    };
    if list.is_empty() {
        return;
    }
    if older {
        let next = match prompt.history_pos {
            None => 0,
            Some(i) if i + 1 < list.len() => i + 1,
            Some(i) => i,
        };
        if prompt.history_pos.is_none() {
            prompt.saved_input = Some(prompt.input.clone());
        }
        prompt.history_pos = Some(next);
        prompt.input = list[list.len() - 1 - next].clone();
        prompt.cursor = prompt.input.chars().count();
    } else {
        match prompt.history_pos {
            None => {}
            Some(0) => {
                prompt.history_pos = None;
                prompt.input = prompt.saved_input.take().unwrap_or_default();
                prompt.cursor = prompt.input.chars().count();
            }
            Some(i) => {
                let next = i - 1;
                prompt.history_pos = Some(next);
                prompt.input = list[list.len() - 1 - next].clone();
                prompt.cursor = prompt.input.chars().count();
            }
        }
    }
}

/// Rebuild a Search/Replace prompt's label from the current toggle state,
/// preserving whichever suffix belongs to its menu (see
/// `app::search_prompt_label`).
fn refresh_search_label(editor: &Editor, prompt: &mut Prompt) {
    let suffix = if prompt.menu == Menu::Replace {
        " (to replace)"
    } else {
        ""
    };
    prompt.label = crate::app::search_prompt_label("Search", suffix, &editor.search);
}

/// nano's `ask_user` for a Yes/No question: Y/y and N/n, the menu's
/// Cancel key (or Esc), plus nano's control-key shortcuts -- ^Y for Yes,
/// and ^N or ^Q (^X under `--modernbindings`) for No, "to allow exiting in
/// anger". Anything else leaves the question up.
fn ask_user_choice(editor: &Editor, key: KeyEvent) -> Option<crate::app::YesNo> {
    use crate::app::YesNo;
    if matches!(key.code, KeyCode::Esc) {
        return Some(YesNo::Cancel);
    }
    if !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        match key.code {
            KeyCode::Char('y' | 'Y') => return Some(YesNo::Yes),
            KeyCode::Char('n' | 'N') => return Some(YesNo::No),
            _ => {}
        }
    }
    let tkey = normalize_key(key)?;
    if editor.keymap.lookup_menu_only(Menu::YesNo, tkey) == Some(&Binding::Action(Action::Cancel)) {
        return Some(YesNo::Cancel);
    }
    let modern = editor.options.modernbindings;
    match tkey {
        TKey::Ctrl('Y') => Some(YesNo::Yes),
        TKey::Ctrl('N') => Some(YesNo::No),
        TKey::Ctrl('Q') if !modern => Some(YesNo::No),
        TKey::Ctrl('X') if modern => Some(YesNo::No),
        _ => None,
    }
}

fn handle_exit_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    use crate::app::YesNo;
    let PromptKind::Exit { .. } = &prompt.kind else {
        return;
    };
    match ask_user_choice(editor, key) {
        Some(YesNo::Yes) => {
            editor.mode = Mode::Editing;
            editor.begin_writeout_for_exit();
        }
        Some(YesNo::No) => {
            editor.close_current_buffer();
            if !matches!(editor.mode, Mode::Quit) {
                editor.mode = Mode::Editing;
            }
        }
        Some(YesNo::Cancel) => {
            editor.mode = Mode::Editing;
            editor.set_status("Cancelled");
        }
        None => editor.mode = Mode::Prompt(prompt),
    }
}

fn handle_write_confirm_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let Some(choice) = ask_user_choice(editor, key) else {
        editor.mode = Mode::Prompt(prompt);
        return;
    };
    let PromptKind::WriteConfirm {
        question,
        answer,
        flow,
    } = prompt.kind
    else {
        return;
    };
    editor.answer_write_question(question, answer, flow, choice);
}

fn handle_conflict_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    // Get Help is normally ^G, but moves to ^H under `--modernbindings`
    // like every other prompt menu (see install_modern_overrides) — check
    // the live keymap rather than hardcoding ^G, so this stays correct
    // there and for any further nanorc/ticorc customization.
    if let Some(tkey) = normalize_key(key)
        && editor.keymap.lookup_menu_only(Menu::YesNo, tkey) == Some(&Binding::Action(Action::Help))
    {
        let lines = crate::help::build_conflict_help(editor.screen_cols);
        editor.mode = Mode::Help {
            lines,
            top: 0,
            return_to: Some(Box::new(prompt)),
        };
        return;
    }
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let (noconvert, unix) = (editor.options.noconvert, editor.options.unix);
            let _ = crate::fileio::reload(editor.buf_mut(), noconvert, unix);
            editor.mode = Mode::Editing;
            editor.set_status("Reloaded from disk; local edits discarded");
        }
        // Esc is folded in here rather than removed outright: it and
        // [C]ancel used to be identical to Keep mine in every way but the
        // status message, so dropping Cancel as a separately-advertised
        // (redundant) choice still leaves Esc as the usual escape hatch.
        KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Esc => {
            if let Some(path) = editor.buf().path.clone() {
                editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
            }
            editor.mode = Mode::Editing;
            editor.set_status("Kept your local edits");
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            editor.begin_merge_preview();
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            editor.buf_mut().ignore_external_changes = true;
            if let Some(path) = editor.buf().path.clone() {
                editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
            }
            editor.mode = Mode::Editing;
            editor.set_status("Ignoring further on-disk changes to this file");
        }
        _ => editor.mode = Mode::Prompt(prompt),
    }
}

/// Handle a keystroke while the merge-diff viewer (`Mode::Diff`) is open:
/// scroll it, or act on it — Apply/Cancel for a clean-merge preview, any
/// key to dismiss an unmergeable-conflict preview (returning to the
/// reload/keep/cancel choice, same as before this became a full-screen
/// view).
fn handle_diff_key(
    editor: &mut Editor,
    lines: Vec<String>,
    top: usize,
    outcome: DiffOutcome,
    key: KeyEvent,
) {
    let body_len = lines.len().saturating_sub(1);
    let body_rows = help_body_rows(editor);
    let max_top = body_len.saturating_sub(body_rows);
    let mut top = top.min(max_top);

    match &outcome {
        DiffOutcome::Conflict => {
            // No automatic resolution possible; any key returns to the
            // main conflict choice so the user can pick reload/keep
            // instead — matches the prior prompt-based behavior.
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::ExternalChangeConflict,
                menu: Menu::YesNo,
                label: "Could not merge automatically: [R]eload  [K]eep mine  [I]gnore All"
                    .to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
            return;
        }
        DiffOutcome::ApplyMerge { merged_text } => match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') => {
                let text = merged_text.clone();
                editor.buf_mut().rope = ropey::Rope::from_str(&text);
                editor.buf_mut().invalidate_highlight_cache();
                editor.buf_mut().modified = true;
                if let Some(path) = editor.buf().path.clone() {
                    editor.buf_mut().disk_state = crate::fileio::stat_disk_state(&path);
                }
                editor.mode = Mode::Editing;
                editor.set_status("Merged");
                return;
            }
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                // Back out to the reload/keep/merge/cancel choice, not
                // straight to editing — this is a step within resolving
                // the conflict, not a dismissal of it.
                editor.mode = Mode::Prompt(external_conflict_prompt());
                return;
            }
            KeyCode::Up => top = top.saturating_sub(1),
            KeyCode::Down => top = (top + 1).min(max_top),
            KeyCode::PageUp => top = top.saturating_sub(body_rows),
            KeyCode::PageDown => top = (top + body_rows).min(max_top),
            KeyCode::Home => top = 0,
            KeyCode::End => top = max_top,
            _ => {}
        },
    }
    editor.mode = Mode::Diff {
        lines,
        top,
        outcome,
    };
}

fn handle_lock_conflict_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::LockConflict { lock_path, target } = &prompt.kind else {
        return;
    };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            let _ = crate::lockfile::write_lock(lock_path, target, false);
            editor.buf_mut().lock_filename = Some(lock_path.clone());
            editor.mode = Mode::Editing;
        }
        KeyCode::Char('n')
        | KeyCode::Char('N')
        | KeyCode::Char('c')
        | KeyCode::Char('C')
        | KeyCode::Esc => {
            // Matches nano: declining leaves this buffer unopened. If it
            // was the only one, fall back to a blank buffer rather than
            // quitting (nano's read_files_from_cmdline() does the same
            // when every given file was declined or invalid).
            editor.buffers.remove(editor.current);
            if editor.buffers.is_empty() {
                editor.buffers.push(crate::buffer::Buffer::empty());
                editor.current = 0;
            } else if editor.current >= editor.buffers.len() {
                editor.current = editor.buffers.len() - 1;
            }
            editor.mode = Mode::Editing;
        }
        _ => editor.mode = Mode::Prompt(prompt),
    }
}

/// The "Replace this instance?" prompt: Y/y = Yes, N/n = No, A/a = All,
/// ^C/Esc = Cancel — matches nano's `ask_user(YESORALLORNO, ...)` exactly
/// (src/prompt.c), including that Esc cancels there too (Cancel is bound
/// to whatever key the MYESNO menu's cancel function has, which includes
/// Esc via the generic Ctrl-C/Esc handling used throughout this UI).
fn handle_replace_confirm_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::ReplaceConfirm(state) = prompt.kind else {
        return;
    };
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            editor.replace_choice(state, crate::app::ReplaceChoice::Yes)
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            editor.replace_choice(state, crate::app::ReplaceChoice::No)
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            editor.replace_choice(state, crate::app::ReplaceChoice::All)
        }
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
            editor.replace_choice(state, crate::app::ReplaceChoice::Cancel)
        }
        _ => {
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::ReplaceConfirm(state),
                ..prompt
            });
        }
    }
}

fn submit_prompt(editor: &mut Editor, prompt: Prompt) {
    let text = prompt.input.clone();
    match prompt.kind {
        PromptKind::WhereIs => {
            // Pressing Enter with nothing typed reuses the remembered last
            // search term (shown bracketed in the label) - confirmed
            // against the installed nano.
            let text = if text.is_empty() {
                editor.search.last_pattern.clone().unwrap_or_default()
            } else {
                text
            };
            editor.mode = Mode::Editing;
            if text.is_empty() {
                return;
            }
            editor.history.add_search(&text);
            let backwards = editor.search.backwards;
            editor.run_search(&text, backwards);
        }
        PromptKind::Replace1 => {
            let text = if text.is_empty() {
                editor.search.last_pattern.clone().unwrap_or_default()
            } else {
                text
            };
            if text.is_empty() {
                editor.mode = Mode::Editing;
                return;
            }
            editor.history.add_search(&text);
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::Replace2 { search: text },
                menu: Menu::ReplaceWith,
                label: "Replace with".to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
        }
        PromptKind::Replace2 { search } => {
            editor.history.add_replace(&text);
            editor.begin_replace_loop(search, text);
        }
        PromptKind::GotoLine => {
            editor.mode = Mode::Editing;
            let (line_s, col_s) = text.split_once(',').unwrap_or((text.as_str(), ""));
            if let Ok(line) = line_s.trim().parse::<i64>() {
                let total = editor.buf().line_count() as i64;
                let target_line = if line < 0 {
                    (total + line).max(0)
                } else {
                    (line - 1).max(0)
                };
                let col = col_s.trim().parse::<i64>().unwrap_or(1).max(1) as usize - 1;
                editor.buf_mut().cursor = Pos::new(target_line as usize, col);
            }
        }
        PromptKind::InsertFile {
            new_buffer,
            execute,
        } => {
            editor.mode = Mode::Editing;
            if text.is_empty() {
                // Matches nano: an empty filename/command with New Buffer
                // on opens a blank buffer instead of canceling; off, it
                // cancels.
                if new_buffer {
                    editor.buffers.push(crate::buffer::Buffer::empty());
                    editor.current = editor.buffers.len() - 1;
                } else {
                    editor.set_status("Cancelled");
                }
                return;
            }
            if execute {
                submit_execute_command(editor, &text, new_buffer);
                return;
            }
            // `~`/`~/rest` expands to the current user's home directory;
            // `~user`/`~user/rest` to that user's (matching nano exactly —
            // see expand_leading_tilde).
            let path = std::path::PathBuf::from(crate::fileio::expand_leading_tilde(&text));
            if path.is_dir() {
                editor.set_status_alert(format!("'{}' is a directory", path.display()));
                return;
            }
            if new_buffer {
                let syntax_override = editor.options.syntax_name.clone();
                if !path.exists() {
                    // A nonexistent filename also yields a blank buffer,
                    // per nano's own hint text for this prompt.
                    let mut buf = crate::buffer::Buffer::from_text("", Some(path));
                    // Detected the same way as a file given on the command
                    // line (extension -> shebang -> modeline, or -Y/--syntax)
                    // — a buffer opened via ^R shouldn't get plain text just
                    // because it didn't come from argv.
                    buf.language = crate::syntax::detect_with_override(
                        buf.path.as_deref(),
                        &buf.to_string(),
                        syntax_override.as_deref(),
                    );
                    editor.buffers.push(buf);
                    editor.current = editor.buffers.len() - 1;
                    editor.set_status("New File");
                } else {
                    match crate::fileio::load_file(&path, &editor.options) {
                        Ok(crate::fileio::LoadedFile {
                            buffer: mut buf,
                            detected,
                        }) => {
                            let msg = crate::fileio::describe_read(&buf.to_string(), detected);
                            buf.language = crate::syntax::detect_with_override(
                                buf.path.as_deref(),
                                &buf.to_string(),
                                syntax_override.as_deref(),
                            );
                            editor.buffers.push(buf);
                            editor.current = editor.buffers.len() - 1;
                            editor.note_buffer_linecount();
                            // Loading into a *new* buffer this way is never
                            // an undoable insert into the current one, so
                            // nano suppresses the ordinary blurb here too
                            // whenever minibar is on (only the persistent
                            // note shows) -- confirmed against the
                            // installed nano's own escape-code output.
                            if !editor.options.minibar {
                                editor.set_status(msg);
                            }
                        }
                        Err(e) => editor
                            .set_status_alert(format!("Error reading {}: {e}", path.display())),
                    }
                }
            } else {
                match std::fs::read_to_string(&path) {
                    Ok(raw) => {
                        let (content, detected) =
                            crate::fileio::convert_line_endings(&raw, editor.options.noconvert);
                        let msg = crate::fileio::describe_read(&content, detected);
                        let unix = editor.options.unix;
                        editor.buf_mut().insert_str(&content);
                        editor.buf_mut().adopt_format(detected, unix);
                        editor.set_status(msg);
                        editor.note_buffer_linecount();
                    }
                    Err(e) => {
                        editor.set_status_alert(format!("Error reading {}: {e}", path.display()))
                    }
                }
            }
        }
        PromptKind::SpellFix { word, remaining } => {
            editor.mode = Mode::Editing;
            if text != word && !text.is_empty() {
                replace_whole_word(editor.buf_mut(), &word, &text);
                editor.buf_mut().modified = true;
            }
            advance_spell_fix(editor, remaining);
        }
        PromptKind::WriteOut { flow } => editor.submit_write_answer(text, flow),
        PromptKind::BrowserSearch { forwards } => {
            editor.mode = Mode::Browser;
            // Enter on a blank answer, with nothing searched for yet this
            // session, is the same as cancelling.
            if text.is_empty() && editor.search.last_pattern.is_none() {
                editor.set_status("Cancelled");
                return;
            }
            remember_browser_search(editor, &text);
            let needle = editor.search.last_pattern.clone().unwrap_or_default();
            browser_find(editor, &needle, forwards);
        }
        PromptKind::GotoDir => {
            editor.mode = Mode::Browser;
            if text.is_empty() {
                editor.set_status("Cancelled");
                return;
            }
            let Some(session) = editor.browser.as_mut() else {
                return;
            };
            let target = crate::browser::goto_dir_target(&session.list.dir, &text);
            // Highlighted if listed, in case it then can't be entered.
            session.list.select_if_listed(&target);
            browser_enter_dir(editor, &target, None);
        }
        _ => {
            editor.mode = Mode::Editing;
        }
    }
}

// ---------------------------------------------------------------------
// Execute Command / Speller / Formatter / Linter
// ---------------------------------------------------------------------

/// A whole-word (alphanumeric-or-underscore-bounded) match, case-sensitive.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The position of the first whole-word occurrence of `word` in `buf`,
/// scanning from the top — used to seed the spell-fix loop's spotlight,
/// matching nano's own `fix_spello` (which likewise searches for the exact,
/// case-sensitive word).
fn find_whole_word(buf: &crate::buffer::Buffer, word: &str) -> Option<Pos> {
    let wchars = word.chars().count();
    if wchars == 0 {
        return None;
    }
    for line_idx in 0..buf.line_count() {
        let chars: Vec<char> = buf.line(line_idx).chars().collect();
        let mut col = 0;
        while col + wchars <= chars.len() {
            if chars[col..col + wchars].iter().collect::<String>() == word {
                let before_ok = col == 0 || !is_word_char(chars[col - 1]);
                let after_ok = col + wchars == chars.len() || !is_word_char(chars[col + wchars]);
                if before_ok && after_ok {
                    return Some(Pos::new(line_idx, col));
                }
            }
            col += 1;
        }
    }
    None
}

/// Replace every whole-word, case-sensitive occurrence of `word` in `buf`
/// with `replacement` — matches nano's `fix_spello`, which (via
/// `do_replace_loop`) fixes every instance of a misspelling at once rather
/// than asking per-occurrence. Returns whether anything changed.
fn replace_whole_word(buf: &mut crate::buffer::Buffer, word: &str, replacement: &str) -> bool {
    let wchars = word.chars().count();
    if wchars == 0 {
        return false;
    }
    let mut matches: Vec<Pos> = Vec::new();
    for line_idx in 0..buf.line_count() {
        let chars: Vec<char> = buf.line(line_idx).chars().collect();
        let mut col = 0;
        while col + wchars <= chars.len() {
            if chars[col..col + wchars].iter().collect::<String>() == word {
                let before_ok = col == 0 || !is_word_char(chars[col - 1]);
                let after_ok = col + wchars == chars.len() || !is_word_char(chars[col + wchars]);
                if before_ok && after_ok {
                    matches.push(Pos::new(line_idx, col));
                    col += wchars;
                    continue;
                }
            }
            col += 1;
        }
    }
    if matches.is_empty() {
        return false;
    }
    for start in matches.into_iter().rev() {
        let end = Pos::new(start.line, start.col + wchars);
        buf.delete_range(start, end);
        buf.cursor = start;
        buf.insert_str(replacement);
    }
    true
}

/// Split a configured command (`set speller`/`--speller`, or a syntax's
/// built-in `linter`/`formatter`) into a program and its arguments on
/// whitespace — matching nano's own `construct_argument_list`, which uses
/// `strtok(..., " ")` and likewise has no quoting support.
fn split_command(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(str::to_string).collect()
}

/// Write `text` to a fresh temp file, for handing to an external
/// speller/formatter (matches nano's `safe_tempfile`).
fn write_temp_file(text: &str) -> io::Result<std::path::PathBuf> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("tico.{}.{unique}.tmp", std::process::id()));
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Hand the terminal over to `cmd` for the duration of its run — leaving
/// the alternate screen and raw mode exactly like nano's `endwin()` before
/// `treat()`/spawning a program, then restoring both (and forcing a full
/// repaint, since the program may have written anything to the screen)
/// once it exits.
fn run_suspended(mut cmd: std::process::Command) -> io::Result<std::process::ExitStatus> {
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    let result = cmd.status();
    let _ = enable_raw_mode();
    let _ = execute!(
        io::stdout(),
        EnterAlternateScreen,
        Hide,
        Clear(ClearType::All)
    );
    result
}

/// `^T^Z` Suspend: matches nano's `do_suspend`/`suspend_nano`. Hand the
/// terminal back (leave the alternate screen and raw mode, show the
/// cursor), print the same reminder nano prints, then stop our whole
/// process group with SIGSTOP the way nano (and mutt) do, so the shell's
/// job control takes over. When the shell resumes us (`fg`), execution
/// carries on right here: back into raw mode and the alternate screen with
/// a full repaint, and the terminal size re-read since the window may have
/// changed meanwhile (nano's `continue_nano` flags a resize for the same
/// reason).
///
/// Only the keystroke path is covered: crossterm's raw mode turns off the
/// terminal's ISIG, so a typed ^Z arrives as a key rather than a SIGTSTP,
/// and nano's SIGTSTP/SIGCONT handlers (for a `kill -TSTP` from outside)
/// have no equivalent here.
fn suspend_editor(editor: &mut Editor) {
    // nano comes back with a blank status bar (its `lastmessage = HUSH`),
    // not whatever was showing before the ^T prompt replaced it.
    editor.status = None;
    let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    println!("\n\nUse \"fg\" to return to tico.");
    let _ = io::stdout().flush();

    #[cfg(unix)]
    let stopped = rustix::process::kill_current_process_group(rustix::process::Signal::STOP);
    // Windows has no SIGSTOP/process-group job control to hand off to a
    // shell the way Unix does, so there's nothing to stop the process with.
    #[cfg(not(unix))]
    let stopped: Result<(), &str> = Err("suspend is not supported on this platform");

    let _ = enable_raw_mode();
    let _ = execute!(
        io::stdout(),
        EnterAlternateScreen,
        Hide,
        Clear(ClearType::All)
    );
    if let Ok((cols, rows)) = size() {
        editor.screen_cols = cols as usize;
        editor.screen_rows = rows as usize;
    }
    if let Err(e) = stopped {
        editor.set_status_alert(format!("Could not suspend: {e}"));
    }
}

/// `^T` Execute Command's submit: run `text` in the shell, and insert its
/// captured output into the buffer at the cursor (or, with New Buffer on,
/// into a fresh blank buffer) — matches nano's `execute_command` (the
/// plain, non-pipe case; nano's `|command` pipe-to-stdin form isn't
/// implemented).
fn submit_execute_command(editor: &mut Editor, command: &str, new_buffer: bool) {
    if new_buffer {
        editor.buffers.push(crate::buffer::Buffer::empty());
        editor.current = editor.buffers.len() - 1;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    editor.set_status("Executing...");
    match std::process::Command::new(&shell)
        .arg("-c")
        .arg(command)
        .output()
    {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            if !text.is_empty() {
                editor.buf_mut().insert_str(&text);
                editor.buf_mut().modified = true;
            }
            editor.history.add_execute(command);
            editor.set_status("Executing...");
        }
        Err(e) => editor.set_status_alert(format!("Could not fork: {e}")),
    }
}

/// `F12` (Main menu) / `^T` from within the Execute-Command prompt: spell
/// check the current buffer (or, if a region is marked, just that region).
fn run_speller(editor: &mut Editor) {
    match editor.options.speller.clone() {
        Some(cmd) if !cmd.is_empty() => run_alt_speller(editor, &cmd),
        _ => run_internal_speller(editor),
    }
}

/// The configured `set speller`/`--speller` program: an interactive tool
/// (aspell -c, ispell, ...) that edits a temp copy of the text directly,
/// matching nano's `treat()` — the terminal is handed over to it, and the
/// buffer is replaced with the temp file's contents if it changed.
fn run_alt_speller(editor: &mut Editor, speller_cmd: &str) {
    let text = editor.tool_input_text();
    let tmp = match write_temp_file(&text) {
        Ok(p) => p,
        Err(e) => {
            editor.set_status_alert(format!("Error writing temp file: {e}"));
            return;
        }
    };
    let before = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
    let mut argv = split_command(speller_cmd);
    if argv.is_empty() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let program = argv.remove(0);
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&argv).arg(&tmp);
    match run_suspended(cmd) {
        Ok(status) => {
            let code = status.code();
            if !code.is_some_and(|c| (0..=2).contains(&c)) {
                editor.set_status_alert(format!("Error invoking '{speller_cmd}'"));
                let _ = std::fs::remove_file(&tmp);
                return;
            }
            // Exit code 1 or 2 means the program is unhappy about
            // something; nano shows that ALERT-level complaint and, per
            // its status-bar importance rule (a lower-priority message
            // never overwrites a still-showing higher one), leaves it up
            // instead of replacing it with the routine "Nothing
            // changed"/"Finished..." message that follows.
            let complained = code != Some(0);
            if complained {
                editor.set_status_alert(format!("Program '{speller_cmd}' complained"));
            }
            let after = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
            if after != before {
                match std::fs::read_to_string(&tmp) {
                    Ok(new_text) => {
                        editor.replace_tool_input(&new_text);
                        if !complained {
                            editor.set_status("Finished checking spelling");
                        }
                    }
                    Err(e) => editor.set_status_alert(format!("Error reading temp file: {e}")),
                }
            } else if !complained {
                editor.set_status("Nothing changed");
            }
        }
        Err(e) => editor.set_status_alert(format!("Error invoking '{speller_cmd}': {e}")),
    }
    let _ = std::fs::remove_file(&tmp);
}

/// No `set speller`/`--speller` configured: nano's own default — run
/// `hunspell -l` (falling back to `spell`) over the text, sort and dedupe
/// the misspelled words it lists (`sort -f | uniq`), then offer each one
/// in turn via the `SpellFix` prompt (see `advance_spell_fix`).
fn run_internal_speller(editor: &mut Editor) {
    let text = editor.tool_input_text();
    let words = match run_word_lister("hunspell", &["-l"], &text)
        .or_else(|| run_word_lister("spell", &[], &text))
    {
        Some(w) => w,
        None => {
            editor.set_status_alert("Error invoking spell checker");
            return;
        }
    };
    let mut sorted = words;
    sorted.sort_by_key(|w| w.to_lowercase());
    let mut deduped: Vec<String> = Vec::with_capacity(sorted.len());
    for w in sorted {
        if deduped.last() != Some(&w) {
            deduped.push(w);
        }
    }
    advance_spell_fix(editor, deduped);
}

/// Run `program args...` with `text` piped to its stdin, returning its
/// stdout split into non-blank lines — `None` if the program couldn't be
/// spawned (e.g. not installed), so the caller can fall back to the next
/// one in nano's own preference order.
fn run_word_lister(program: &str, args: &[&str], text: &str) -> Option<Vec<String>> {
    use std::io::Write as _;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Advance the internal spell-fix loop: pop words off the front of
/// `remaining` until one is actually found in the buffer (a speller can
/// report a word that doesn't literally occur, e.g. across a line
/// boundary), open the `SpellFix` prompt spotlighting it, or, once the
/// list is exhausted, report done — matches nano's `spell_check`.
fn advance_spell_fix(editor: &mut Editor, mut remaining: Vec<String>) {
    while !remaining.is_empty() {
        let word = remaining.remove(0);
        if let Some(pos) = find_whole_word(editor.buf(), &word) {
            editor.buf_mut().cursor = pos;
            editor.scroll_to_cursor();
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::SpellFix {
                    word: word.clone(),
                    remaining,
                },
                menu: Menu::Spell,
                label: "Edit a replacement".to_string(),
                cursor: word.chars().count(),
                input: word,
                history_pos: None,
                saved_input: None,
            });
            return;
        }
    }
    editor.mode = Mode::Editing;
    editor.set_status("Finished checking spelling");
}

/// `^O` from the Execute-Command prompt (or a rebound Main-menu key): run
/// the current buffer's configured formatter (a per-language default, e.g.
/// `gofmt -w`, matching nano's shipped nanorc `formatter` directives).
/// Same terminal-handoff/temp-file contract as `run_alt_speller` (nano
/// implements both through the shared `treat()`).
fn run_formatter(editor: &mut Editor) {
    let Some(formatter_cmd) = editor.buf().language.and_then(|l| l.formatter) else {
        editor.set_status_mild("No formatter is defined for this type of file");
        return;
    };
    let text = editor.tool_input_text();
    let tmp = match write_temp_file(&text) {
        Ok(p) => p,
        Err(e) => {
            editor.set_status_alert(format!("Error writing temp file: {e}"));
            return;
        }
    };
    let before = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
    let mut argv = split_command(formatter_cmd);
    if argv.is_empty() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let program = argv.remove(0);
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&argv).arg(&tmp);
    match run_suspended(cmd) {
        Ok(status) => {
            let code = status.code();
            if !code.is_some_and(|c| (0..=2).contains(&c)) {
                editor.set_status_alert(format!("Error invoking '{formatter_cmd}'"));
                let _ = std::fs::remove_file(&tmp);
                return;
            }
            // See run_alt_speller: a "complained" ALERT outranks the
            // routine messages that follow, so it stays up instead of
            // being overwritten by them.
            let complained = code != Some(0);
            if complained {
                editor.set_status_alert(format!("Program '{formatter_cmd}' complained"));
            }
            let after = std::fs::metadata(&tmp).and_then(|m| m.modified()).ok();
            if after != before {
                match std::fs::read_to_string(&tmp) {
                    Ok(new_text) => {
                        editor.replace_tool_input(&new_text);
                        if !complained {
                            editor.set_status("Buffer has been processed");
                        }
                    }
                    Err(e) => editor.set_status_alert(format!("Error reading temp file: {e}")),
                }
            } else if !complained {
                editor.set_status("Nothing changed");
            }
        }
        Err(e) => editor.set_status_alert(format!("Error invoking '{formatter_cmd}': {e}")),
    }
    let _ = std::fs::remove_file(&tmp);
}

/// `^Y` from the Execute-Command prompt (or a rebound Main-menu key): run
/// the current buffer's configured linter and open the interactive result
/// viewer (`PromptKind::Linter`) on whatever it reports — matches nano's
/// `do_linter`, without the "jump to a different open buffer" case (tico's
/// linter only ever targets the current buffer's own file).
fn run_linter(editor: &mut Editor) {
    let Some(linter_cmd) = editor.buf().language.and_then(|l| l.linter) else {
        editor.set_status_mild("No linter is defined for this type of file");
        return;
    };
    let Some(path) = editor.buf().path.clone() else {
        editor.set_status_mild("No linter is defined for this type of file");
        return;
    };
    let mut argv = split_command(linter_cmd);
    if argv.is_empty() {
        return;
    }
    let program = argv.remove(0);
    editor.set_status("Invoking linter...");
    let output = std::process::Command::new(&program)
        .args(&argv)
        .arg(&path)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            editor.set_status_alert(format!("Error invoking '{linter_cmd}': {e}"));
            return;
        }
    };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    let messages = parse_linter_output(&combined);
    if messages.is_empty() {
        editor.set_status(format!("Got 0 parsable lines from command: {linter_cmd}"));
        return;
    }
    goto_lint_message(editor, &messages[0]);
    editor.mode = Mode::Prompt(Prompt {
        kind: PromptKind::Linter { messages, index: 0 },
        menu: Menu::Linter,
        label: String::new(),
        input: String::new(),
        cursor: 0,
        history_pos: None,
        saved_input: None,
    });
    if let Mode::Prompt(prompt) = &mut editor.mode
        && let PromptKind::Linter { messages, index } = &prompt.kind
    {
        prompt.label = messages[*index].msg.clone();
    }
}

/// Parse `filename:line:col: message` (or `filename:line,col: message`, or
/// bare `filename:line: message` with the column defaulting to 1) lines —
/// matches nano's own linter-output parser in `do_linter`.
/// Parse a leading (optionally signed) decimal integer, ignoring any
/// trailing non-digit text — matches C's `strtol(s, NULL, 10)`, which
/// nano's linter parser relies on to tolerate e.g. `12,2` as a line number
/// (reading `12` and leaving the rest for a separate comma-split).
fn parse_leading_int(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let neg = s.starts_with('-');
    let digits_start = if neg || s.starts_with('+') { 1 } else { 0 };
    let digit_len = s[digits_start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .count();
    if digit_len == 0 {
        return None;
    }
    s[..digits_start + digit_len].parse().ok()
}

fn parse_linter_output(output: &str) -> Vec<crate::app::LintMessage> {
    let mut messages = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        // The message is everything after the first space anywhere in the
        // line, independent of how the fields before it are split — matches
        // nano's `spacer = strstr(complaint, " ")`.
        let Some(spacer) = line.find(' ') else {
            continue;
        };
        let Some((filename, after_filename)) = line.split_once(':') else {
            continue;
        };
        let Some((linestring, after_line)) = after_filename.split_once(':') else {
            continue;
        };
        let Some(lineno) = parse_leading_int(linestring).filter(|&n| n > 0) else {
            continue;
        };
        // `strtok(NULL, " ")` on the remainder: skips any leading spaces,
        // then reads up to the next one.
        let colstring = after_line.trim_start_matches(' ').split(' ').next();
        let mut colno = colstring
            .and_then(parse_leading_int)
            .filter(|&c| c > 0)
            .unwrap_or(0);
        if colno <= 0 {
            colno = 1;
            // "line,column" form: the part after a comma in `linestring`.
            if let Some((_, colpart)) = linestring.split_once(',')
                && let Some(c) = parse_leading_int(colpart)
            {
                colno = c;
            }
        }
        messages.push(crate::app::LintMessage {
            filename: filename.to_string(),
            line: lineno as usize,
            col: colno as usize,
            msg: line[spacer + 1..].to_string(),
        });
    }
    messages
}

/// Move the cursor to a lint message's reported location, matching nano's
/// `goto_line_posx` + `adjust_viewport(CENTERING)` in `do_linter`.
fn goto_lint_message(editor: &mut Editor, msg: &crate::app::LintMessage) {
    let line = msg
        .line
        .saturating_sub(1)
        .min(editor.buf().line_count().saturating_sub(1));
    let col = msg
        .col
        .saturating_sub(1)
        .min(editor.buf().line(line).chars().count());
    editor.buf_mut().cursor = Pos::new(line, col);
    editor.scroll_to_cursor();
}

/// Step through linter results with PageUp/PageDown ("Previous/Next Linter
/// message"), or close the viewer with Cancel/Enter — matches nano's
/// `MLINTER` navigation loop in `do_linter`.
fn handle_linter_choice(editor: &mut Editor, prompt: Prompt, key: KeyEvent) {
    let PromptKind::Linter {
        messages,
        mut index,
    } = prompt.kind
    else {
        unreachable!()
    };
    if matches!(key.code, KeyCode::Esc) {
        editor.mode = Mode::Editing;
        editor.status = None;
        return;
    }
    let Some(tkey) = normalize_key(key) else {
        editor.mode = Mode::Prompt(Prompt {
            kind: PromptKind::Linter { messages, index },
            ..prompt
        });
        return;
    };
    // "At first/last message" briefly replaces the current message when a
    // boundary is hit (nano flashes it for ~600ms then restores the
    // message; tico's synchronous input loop has no timed flash, so it
    // just shows until the next keystroke instead).
    let mut boundary_label = None;
    match editor.keymap.lookup_menu_only(Menu::Linter, tkey) {
        Some(Binding::Action(Action::Cancel)) => {
            editor.mode = Mode::Editing;
            editor.status = None;
            return;
        }
        Some(Binding::Action(Action::PageUp)) => {
            if index > 0 {
                index -= 1;
            } else {
                boundary_label = Some("At first message");
                editor.bell_pending = true;
            }
        }
        Some(Binding::Action(Action::PageDown)) => {
            if index + 1 < messages.len() {
                index += 1;
            } else {
                boundary_label = Some("At last message");
                editor.bell_pending = true;
            }
        }
        _ => {}
    }
    goto_lint_message(editor, &messages[index]);
    let label = boundary_label
        .map(str::to_string)
        .unwrap_or_else(|| messages[index].msg.clone());
    editor.mode = Mode::Prompt(Prompt {
        kind: PromptKind::Linter { messages, index },
        menu: Menu::Linter,
        label,
        input: String::new(),
        cursor: 0,
        history_pos: None,
        saved_input: None,
    });
}

// ---------------------------------------------------------------------
// Crossterm key normalization
// ---------------------------------------------------------------------

fn normalize_key(key: KeyEvent) -> Option<TKey> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        KeyCode::Backspace => Some(TKey::Backspace),
        KeyCode::Tab => Some(TKey::Ctrl('I')),
        KeyCode::BackTab => Some(TKey::ShiftTab),
        KeyCode::Enter => Some(TKey::Ctrl('M')),
        KeyCode::Esc => None,
        KeyCode::Left if ctrl => Some(TKey::CtrlLeft),
        KeyCode::Right if ctrl => Some(TKey::CtrlRight),
        KeyCode::Up if ctrl => Some(TKey::CtrlUp),
        KeyCode::Down if ctrl => Some(TKey::CtrlDown),
        KeyCode::Left if alt => Some(TKey::MetaLeft),
        KeyCode::Right if alt => Some(TKey::MetaRight),
        KeyCode::Up if alt => Some(TKey::MetaUp),
        KeyCode::Down if alt => Some(TKey::MetaDown),
        KeyCode::Left => Some(TKey::Left),
        KeyCode::Right => Some(TKey::Right),
        KeyCode::Up => Some(TKey::Up),
        KeyCode::Down => Some(TKey::Down),
        KeyCode::Home if ctrl => Some(TKey::CtrlHome),
        KeyCode::End if ctrl => Some(TKey::CtrlEnd),
        KeyCode::Home if alt => Some(TKey::MetaHome),
        KeyCode::End if alt => Some(TKey::MetaEnd),
        KeyCode::Home => Some(TKey::Home),
        KeyCode::End => Some(TKey::End),
        KeyCode::PageUp if alt => Some(TKey::MetaPgUp),
        KeyCode::PageDown if alt => Some(TKey::MetaPgDn),
        KeyCode::PageUp => Some(TKey::PageUp),
        KeyCode::PageDown => Some(TKey::PageDown),
        KeyCode::Delete if ctrl && shift => Some(TKey::ShiftCtrlDel),
        KeyCode::Delete if ctrl => Some(TKey::CtrlDel),
        KeyCode::Delete if alt => Some(TKey::MetaDel),
        KeyCode::Delete => Some(TKey::Ctrl('D')),
        KeyCode::Insert if alt => Some(TKey::MetaIns),
        KeyCode::Insert => Some(TKey::Ins),
        KeyCode::F(n) => Some(TKey::F(n)),
        KeyCode::Char(c) if ctrl => {
            // crossterm's unix parser reports Ctrl+\, Ctrl+], Ctrl+^ and
            // Ctrl+_ (raw bytes 0x1C-0x1F) as Char('4')..Char('7') with
            // CONTROL set — those bytes are indistinguishable on the wire
            // from an actual Ctrl+digit, and crossterm picks the digit
            // form. Map back to the symbol form nano's docs, our keymap
            // defaults, and nanorc `bind` lines all use.
            let mapped = match c {
                '4' => '\\',
                '5' => ']',
                '6' => '^',
                '7' => '_',
                other => other.to_ascii_uppercase(),
            };
            Some(TKey::Ctrl(mapped))
        }
        KeyCode::Char(c) if alt && shift && c.is_ascii_alphabetic() => {
            Some(TKey::ShiftMeta(c.to_ascii_uppercase()))
        }
        // Meta+letter is case-insensitive by default in nano (a bare
        // Meta+letter keystroke does the same as Shift+Meta+letter unless
        // a specific Sh-M- binding overrides it), and our keymap stores
        // Meta letter bindings uppercase, so normalize here too.
        KeyCode::Char(c) if alt => Some(TKey::Meta(c.to_ascii_uppercase())),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// File browser
// ---------------------------------------------------------------------

/// Open the file browser from `prompt` (Read File or Write Out), starting
/// where its answer points -- nano's `browse_in`. Returns false, with the
/// reason flashed as a brief warning, when there is nothing to browse; the
/// prompt then simply stays up, as it does in nano.
fn open_browser(editor: &mut Editor, prompt: &Prompt) -> bool {
    use crate::browser::{Browser, full_dir_path, start_dir, strerror};
    let start = match start_dir(&prompt.input) {
        Ok(dir) => dir,
        Err(msg) => {
            editor.brief_warning = Some(msg);
            return false;
        }
    };
    match full_dir_path(&start).and_then(|dir| Browser::read(&dir)) {
        Ok(list) => {
            editor.browser = Some(crate::app::BrowserSession {
                list,
                return_to: prompt.clone(),
            });
            editor.file_completions = None;
            editor.status = None;
            editor.mode = Mode::Browser;
            true
        }
        Err(e) => {
            editor.brief_warning = Some(format!("Cannot open directory: {}", strerror(&e)));
            false
        }
    }
}

/// Leave the browser without choosing anything: back to the prompt it
/// was opened from, as it was.
fn close_browser(editor: &mut Editor) {
    editor.status = None;
    editor.mode = match editor.browser.take() {
        Some(session) => Mode::Prompt(session.return_to),
        None => Mode::Editing,
    };
}

/// Show `path` in the browser: nano's `read_directory_contents` step.
/// With `reselect`, that entry (the directory just left, for "..") is
/// selected again; otherwise the first. A directory that can't be read
/// leaves the current listing up (re-read, keeping its selection) under
/// an alert.
fn browser_enter_dir(editor: &mut Editor, path: &std::path::Path, reselect: Option<PathBuf>) {
    use crate::browser::{Browser, full_dir_path, strerror};
    let loaded = full_dir_path(path).and_then(|dir| Browser::read(&dir));
    let Some(session) = editor.browser.as_mut() else {
        return;
    };
    match loaded {
        Ok(mut list) => {
            if let Some(name) = reselect {
                list.selected = session.list.selected;
                list.reselect(&name);
            }
            session.list = list;
        }
        Err(e) => {
            let msg = format!("Cannot open directory: {}", strerror(&e));
            browser_reread(session);
            editor.set_status_alert(msg);
        }
    }
}

/// Re-read the directory being shown, keeping the selection on the same
/// name when it's still there (`^L`, and after failing to enter another).
fn browser_reread(session: &mut crate::app::BrowserSession) {
    let current = session.list.selected_entry().map(|e| e.path.clone());
    if let Ok(mut list) = crate::browser::Browser::read(&session.list.dir) {
        list.selected = session.list.selected;
        if let Some(path) = current {
            list.reselect(&path);
        }
        session.list = list;
    }
}

/// The number of listing rows, and the screen row the listing starts on.
fn browser_list_area(editor: &Editor) -> (usize, usize) {
    if editor.options.zero {
        (editor.screen_rows.saturating_sub(1).max(1), 0)
    } else {
        let layout = main_screen_layout(editor);
        (layout.text_rows.max(1), layout.text_start_row)
    }
}

/// The browser's (piles, rows) for the current screen size.
fn browser_geometry(editor: &Editor) -> (usize, usize) {
    let rows = browser_list_area(editor).0;
    let piles = editor
        .browser
        .as_ref()
        .map_or(1, |b| b.list.piles(editor.screen_cols));
    (piles, rows)
}

/// The plain keys that nano's `interpret()` gives a meaning in the browser
/// (and help viewer) ahead of any binding -- Pico's and `less`'s.
fn browser_plain_key(key: KeyEvent) -> Option<Action> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    match c {
        'N' => return Some(Action::FindPrevious),
        'n' => return Some(Action::FindNext),
        _ => {}
    }
    Some(match c.to_ascii_lowercase() {
        'b' | '-' => Action::PageUp,
        ' ' => Action::PageDown,
        'w' | '/' => Action::WhereIs,
        'g' => Action::GotoDir,
        '?' => Action::Help,
        's' => Action::Enter,
        'e' | 'q' | 'x' => Action::Exit,
        _ => return None,
    })
}

fn handle_browser_key(editor: &mut Editor, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc) {
        close_browser(editor);
        return;
    }
    let tkey = normalize_key(key);
    let action = browser_plain_key(key).or_else(|| {
        match tkey.and_then(|k| editor.keymap.lookup_menu_only(Menu::Browser, k)) {
            Some(Binding::Action(a)) => Some(*a),
            _ => None,
        }
    });
    let Some(action) = action else {
        let name = match (key.code, tkey) {
            (_, Some(k)) => k.describe(),
            (KeyCode::Char(c), None) => c.to_string(),
            _ => return,
        };
        editor.set_status_mild(format!("Unbound key: {name}"));
        return;
    };
    apply_browser_action(editor, action);
}

fn apply_browser_action(editor: &mut Editor, action: Action) {
    let (piles, rows) = browser_geometry(editor);
    let Some(session) = editor.browser.as_mut() else {
        editor.mode = Mode::Editing;
        return;
    };
    if session.list.navigate(action, piles, rows) {
        return;
    }
    match action {
        Action::Help => {
            let lines = crate::help::build(Menu::Browser, &editor.keymap, editor.screen_cols);
            editor.mode = Mode::Help {
                lines,
                top: 0,
                return_to: None,
            };
        }
        Action::Exit => close_browser(editor),
        Action::Enter => browser_choose(editor),
        Action::WhereIs | Action::WhereWas => {
            let forwards = action == Action::WhereIs;
            let label = browser_search_label(editor, forwards);
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::BrowserSearch { forwards },
                menu: Menu::WhereIsFile,
                label,
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
        }
        Action::FindNext | Action::FindPrevious => {
            // With nothing searched for yet, the newest search history
            // entry stands in.
            if editor.search.last_pattern.is_none() {
                editor.search.last_pattern = editor.history.search.last().cloned();
            }
            match editor.search.last_pattern.clone() {
                Some(needle) => {
                    editor.status = None;
                    browser_find(editor, &needle, action == Action::FindNext);
                }
                None => editor.set_status("No current search pattern"),
            }
        }
        Action::GotoDir => {
            editor.mode = Mode::Prompt(Prompt {
                kind: PromptKind::GotoDir,
                menu: Menu::GotoDir,
                label: "Go To Directory".to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            });
        }
        Action::Refresh => browser_reread(session),
        Action::NoHelp => editor.options.nohelp = !editor.options.nohelp,
        _ => {}
    }
}

/// Enter on the selected name: descend into a directory, or pick a file
/// -- handing it to the prompt the browser was opened from, as if typed
/// there and accepted.
fn browser_choose(editor: &mut Editor) {
    let Some(session) = editor.browser.as_ref() else {
        return;
    };
    let Some(entry) = session.list.selected_entry().cloned() else {
        return;
    };
    let dir = session.list.dir.clone();
    let going_up = entry.name == "..";
    if going_up && dir.parent().is_none() {
        editor.set_status_alert("Can't move up a directory");
        return;
    }
    let meta = match std::fs::metadata(&entry.path) {
        Ok(meta) => meta,
        Err(e) => {
            editor.set_status_alert(format!(
                "Error reading {}: {}",
                entry.path.display(),
                crate::browser::strerror(&e)
            ));
            return;
        }
    };
    if meta.is_dir() {
        // Going up, the directory just left is selected again, so that
        // it's easily re-entered.
        let reselect = going_up.then_some(dir);
        browser_enter_dir(editor, &entry.path, reselect);
        return;
    }

    let Some(session) = editor.browser.take() else {
        return;
    };
    let mut prompt = session.return_to;
    prompt.input = entry.path.to_string_lossy().into_owned();
    prompt.cursor = prompt.input.chars().count();
    editor.status = None;
    submit_prompt(editor, prompt);
}

/// The browser's Search prompt label: "Search", " [Backwards]" when so,
/// and the previous search string in brackets (clipped to a third of the
/// screen). nano's `search_filename`.
fn browser_search_label(editor: &Editor, forwards: bool) -> String {
    let mut label = "Search".to_string();
    if !forwards {
        label.push_str(" [Backwards]");
    }
    if let Some(last) = editor.search.last_pattern.as_deref()
        && !last.is_empty()
    {
        let room = editor.screen_cols / 3;
        let shown: String = last.chars().take(room).collect();
        let dots = if last.chars().count() > room {
            "..."
        } else {
            ""
        };
        label.push_str(&format!(" [{shown}{dots}]"));
    }
    label
}

/// An answer typed at the browser's Search prompt becomes the search
/// string -- shared with the editor's own searches, as in nano -- and goes
/// into the search history.
fn remember_browser_search(editor: &mut Editor, text: &str) {
    if !text.is_empty() {
        editor.search.last_pattern = Some(text.to_string());
        editor.history.add_search(text);
    }
}

fn browser_find(editor: &mut Editor, needle: &str, forwards: bool) {
    use crate::browser::FindOutcome;
    let Some(session) = editor.browser.as_mut() else {
        return;
    };
    match session.list.find(needle, forwards) {
        FindOutcome::Found => editor.status = None,
        FindOutcome::Wrapped => editor.set_status("Search Wrapped"),
        FindOutcome::OnlyOccurrence => editor.set_status("This is the only occurrence"),
        FindOutcome::NotFound => editor.set_status(format!("\"{needle}\" not found")),
    }
}

/// A left-click while the browser (or one of its prompts) is up: on a
/// name, select it -- or, when it already was, choose it, as nano does;
/// on the shortcut bar, that shortcut.
fn handle_browser_click(editor: &mut Editor, row: usize, col: usize) {
    let (rows, start) = browser_list_area(editor);
    if matches!(editor.mode, Mode::Browser) && row >= start && row < start + rows {
        let cols = editor.screen_cols;
        let Some(session) = editor.browser.as_mut() else {
            return;
        };
        if let Some(index) = session.list.index_at(cols, rows, row - start, col) {
            if index == session.list.selected {
                browser_choose(editor);
            } else {
                session.list.selected = index;
            }
        }
        return;
    }
    if editor.options.zero {
        return;
    }
    let layout = main_screen_layout(editor);
    if layout.help_rows > 0 && row > layout.status_row {
        let entries = browser_bar_entries(editor);
        activate_shortcut_click(editor, &entries, row - layout.status_row - 1, col);
    }
}

/// The shortcut bar under the browser: its own, or its current prompt's.
fn browser_bar_entries(editor: &Editor) -> Vec<(String, &'static str)> {
    match &editor.mode {
        Mode::Prompt(p) => shortcut_bar_entries(&editor.keymap, Some(p)),
        _ => {
            // Only as many as nano's `shown_entries_for` would show, so the
            // longer entries further down don't widen every column.
            let maximum = ((editor.screen_cols + 40) / 20) * 2;
            let shown = &BROWSER_SHORTCUTS[..maximum.min(BROWSER_SHORTCUTS.len())];
            resolve_shortcuts(&editor.keymap, Menu::Browser, shown)
        }
    }
}

/// The browser's screen: "DIR:" title bar, the listing (or a Go To
/// Directory tab-completion grid), the status/prompt line and the
/// shortcut bar. nano's `browser_refresh` plus its `titlebar(path)`.
fn render_browser_screen(
    editor: &Editor,
    session: &crate::app::BrowserSession,
    out: &mut impl Write,
    blank_bars: bool,
) -> io::Result<()> {
    let cols = editor.screen_cols;
    let (rows, start) = browser_list_area(editor);
    if start > 0 {
        queue!(out, MoveTo(0, 0))?;
        let title =
            crate::browser::title_line(&crate::browser::display_dir(&session.list.dir), cols);
        queue_bar_segment(out, title_bar_style(editor), &title)?;
    }

    if let Some(matches) = &editor.file_completions {
        render_completions_grid(out, start as u16, rows, cols, matches)?;
    } else {
        let highlight = bar_style(&editor.options.selectedcolor, BarStyle::Reverse);
        for (r, segments) in session.list.render_rows(cols, rows).into_iter().enumerate() {
            queue!(out, MoveTo(0, (start + r) as u16))?;
            for (text, selected) in segments {
                if selected {
                    queue_bar_segment(out, highlight, &text)?;
                } else {
                    queue!(out, Print(text))?;
                }
            }
        }
    }

    let status_row = start + rows;
    if status_row < editor.screen_rows {
        // No minibar here: with no message, the status line is blank.
        if matches!(editor.mode, Mode::Browser) && editor.status.is_none() {
            queue!(out, MoveTo(0, status_row as u16), Print(" ".repeat(cols)))?;
        } else {
            render_status_line(editor, out, status_row as u16, cols)?;
        }
    }
    if !editor.options.zero && !editor.options.nohelp {
        let entries = if blank_bars {
            Vec::new()
        } else {
            browser_bar_entries(editor)
        };
        render_shortcut_bar(editor, out, (status_row + 1) as u16, cols, &entries)?;
    }

    match &editor.mode {
        Mode::Prompt(_) => finish_cursor(editor, out, start as u16),
        // `set showcursor`: the cursor sits on the selected name.
        _ if editor.options.showcursor => {
            let (r, c) = session.list.selected_cell(cols, rows);
            queue!(out, MoveTo(c as u16, (start + r) as u16), Show)
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

fn render(editor: &Editor) -> io::Result<()> {
    render_frame(editor, false)
}

/// `render`, optionally with the shortcut bars left blank (nano's
/// `blank_bottombars`, for `show_brief_warning`).
fn render_frame(editor: &Editor, blank_bars: bool) -> io::Result<()> {
    // No full-screen Clear here: every row below is redrawn at its full
    // width, so nothing needs re-blanking first (a per-frame Clear was the
    // cause of visible flicker). The screen is cleared once at startup and
    // again on resize, in `run()`.
    let mut out = io::stdout();
    // Hide the cursor for the duration of the redraw: otherwise the
    // terminal's real hardware cursor stays visible and visibly jumps to
    // every intermediate MoveTo position used while painting each row,
    // instead of moving straight to its final spot.
    queue!(out, Hide)?;

    if let Mode::Help { lines, top, .. } = &editor.mode {
        render_help_screen(editor, &mut out, lines, *top)?;
        return out.flush();
    }
    if let Mode::Diff {
        lines,
        top,
        outcome,
    } = &editor.mode
    {
        render_diff_screen(editor, &mut out, lines, *top, outcome)?;
        return out.flush();
    }

    if let Some(session) = &editor.browser
        && matches!(editor.mode, Mode::Browser | Mode::Prompt(_))
    {
        render_browser_screen(editor, session, &mut out, blank_bars)?;
        return out.flush();
    }

    let cols = editor.screen_cols;
    if editor.options.zero {
        render_buffer(editor, &mut out, 0, editor.screen_rows)?;
        finish_cursor(editor, &mut out, 0)?;
        return out.flush();
    }

    let layout = main_screen_layout(editor);
    let text_start_row = layout.text_start_row as u16;
    // `set minibar` suppresses the title bar entirely -- its own summary
    // takes over the status row instead (see `render_minibar`), so the
    // buffer gets that row back.
    if text_start_row > 0 {
        queue!(out, MoveTo(0, 0))?;
        render_title_bar(editor, &mut out, cols)?;
    }

    if let Some(matches) = &editor.file_completions {
        render_completions_grid(&mut out, text_start_row, layout.text_rows, cols, matches)?;
    } else {
        render_buffer(editor, &mut out, text_start_row, layout.text_rows)?;
    }

    let status_row = layout.status_row as u16;
    render_status_line(editor, &mut out, status_row, cols)?;

    if layout.help_rows > 0 {
        let prompt = if let Mode::Prompt(p) = &editor.mode {
            Some(p)
        } else {
            None
        };
        let entries = if blank_bars {
            Vec::new()
        } else {
            shortcut_bar_entries(&editor.keymap, prompt)
        };
        render_shortcut_bar(editor, &mut out, status_row + 1, cols, &entries)?;
    }

    finish_cursor(editor, &mut out, text_start_row)?;
    out.flush()
}

/// The main editing screen's row layout (everything but `Mode::Help` /
/// `Mode::Diff`, which have their own bespoke full-screen layout, and
/// `set zero`, which hides all chrome and just needs `screen_rows`):
/// shared by `render` and by mouse click resolution (`handle_mouse`), so
/// the two can never drift the way `Editor::text_rows` and `render`'s own
/// row math once did.
struct MainLayout {
    /// 0 normally, 1 when `set minibar` drops the title bar.
    text_start_row: usize,
    /// Rows available to the buffer (or the `^R` tab-completion grid).
    text_rows: usize,
    /// The status/prompt/minibar row, directly below the buffer.
    status_row: usize,
    /// 0 under `set nohelp`, else 2 (the shortcut bar's own two rows,
    /// directly below `status_row`).
    help_rows: usize,
}

fn main_screen_layout(editor: &Editor) -> MainLayout {
    let text_start_row = if editor.options.minibar { 0 } else { 1 };
    let help_rows = if editor.options.nohelp { 0 } else { 2 };
    let text_rows = editor
        .screen_rows
        .saturating_sub(text_start_row + 1 + help_rows);
    let status_row = text_start_row + text_rows;
    MainLayout {
        text_start_row,
        text_rows,
        status_row,
        help_rows,
    }
}

/// Number of rows available for the help viewer's scrollable body: the
/// whole screen minus the title row and the (always-shown, regardless of
/// `nohelp`) two-line shortcut bar.
fn help_body_rows(editor: &Editor) -> usize {
    editor.screen_rows.saturating_sub(3).max(1)
}

/// The `^G` help viewer takes over the whole screen: a centered,
/// reverse-video title (`lines[0]`) where the title bar would normally be,
/// the scrollable body starting at `top` (an index into `lines[1..]`), and
/// its own shortcut bar in place of the usual status line + shortcuts.
fn render_help_screen(
    editor: &Editor,
    out: &mut impl Write,
    lines: &[String],
    top: usize,
) -> io::Result<()> {
    let cols = editor.screen_cols;
    let rows = editor.screen_rows;

    render_centered_title_row(
        editor,
        out,
        cols,
        lines.first().map(|s| s.as_str()).unwrap_or("Help"),
    )?;
    render_scrollable_body(
        out,
        cols,
        &lines[1.min(lines.len())..],
        top,
        help_body_rows(editor),
        None,
    )?;
    let entries = resolve_shortcuts(&editor.keymap, Menu::Help, HELP_SHORTCUTS);
    render_shortcut_bar(editor, out, rows.saturating_sub(2) as u16, cols, &entries)
}

/// The merge-diff viewer (`Mode::Diff`): same full-screen layout as the
/// help viewer (title row, scrollable body, bottom bar), but the bottom
/// bar offers Apply/Cancel for a clean-merge preview instead of just
/// closing, since dismissing this screen is itself a decision.
fn render_diff_screen(
    editor: &Editor,
    out: &mut impl Write,
    lines: &[String],
    top: usize,
    outcome: &DiffOutcome,
) -> io::Result<()> {
    let cols = editor.screen_cols;
    let rows = editor.screen_rows;

    render_centered_title_row(
        editor,
        out,
        cols,
        lines.first().map(|s| s.as_str()).unwrap_or("Diff"),
    )?;
    let body = &lines[1.min(lines.len())..];
    let styles = if editor.options.syntax_highlighting {
        diff_line_styles(body, editor)
    } else {
        None
    };
    render_scrollable_body(
        out,
        cols,
        body,
        top,
        help_body_rows(editor),
        styles.as_deref(),
    )?;

    let entries = diff_shortcut_entries(outcome);
    render_shortcut_bar(editor, out, rows.saturating_sub(2) as u16, cols, &entries)
}

/// The merge-diff viewer's bottom-bar entries for the given outcome --
/// shared by `render_diff_screen` and mouse click resolution
/// (`handle_click`), so the two can never disagree about what a click
/// lands on.
fn diff_shortcut_entries(outcome: &DiffOutcome) -> Vec<(String, &'static str)> {
    let shortcuts: &[(&str, &str)] = match outcome {
        DiffOutcome::ApplyMerge { .. } => &[
            ("A", "Apply merge"),
            ("C", "Cancel"),
            ("^P", "Prev Line"),
            ("^N", "Next Line"),
            ("^Y", "Prev Page"),
            ("^V", "Next Page"),
        ],
        DiffOutcome::Conflict => &[("(any key)", "Continue")],
    };
    shortcuts.iter().map(|&(k, d)| (k.to_string(), d)).collect()
}

/// Center `title` on its own title-bar-colored row at the top of the
/// screen — shared by the help and merge-diff full-screen viewers, and
/// confirmed against the installed nano to follow `titlecolor` exactly the
/// same as the ordinary title bar does.
fn render_centered_title_row(
    editor: &Editor,
    out: &mut impl Write,
    cols: usize,
    title: &str,
) -> io::Result<()> {
    queue!(out, MoveTo(0, 0))?;
    let mut title_row = vec![' '; cols];
    let start = cols.saturating_sub(title.chars().count()) / 2;
    for (i, c) in title.chars().enumerate() {
        if start + i < cols {
            title_row[start + i] = c;
        }
    }
    let title_line: String = title_row.into_iter().collect();
    let style = title_bar_style(editor);
    queue_bar_segment(out, style, &title_line)
}

/// Draw `body_rows` rows of `body` starting at `top`, one screen row per
/// line, below the title row — shared by the help and merge-diff viewers.
fn render_scrollable_body(
    out: &mut impl Write,
    cols: usize,
    body: &[String],
    top: usize,
    body_rows: usize,
    styles: Option<&[Vec<Option<Style>>]>,
) -> io::Result<()> {
    for r in 0..body_rows {
        queue!(out, MoveTo(0, 1 + r as u16))?;
        let idx = top + r;
        let text = body.get(idx).map(|s| s.as_str()).unwrap_or("");
        let chars: Vec<char> = text.chars().take(cols).collect();
        let line_styles = styles.and_then(|k| k.get(idx));
        let len = chars.len();

        if let Some(line_styles) = line_styles.filter(|k| k.iter().any(Option::is_some)) {
            let mut i = 0;
            while i < len {
                let style = line_styles.get(i).copied().flatten();
                let mut j = i + 1;
                while j < len && line_styles.get(j).copied().flatten() == style {
                    j += 1;
                }
                let segment: String = chars[i..j].iter().collect();
                print_styled(out, &segment, style)?;
                i = j;
            }
            if len < cols {
                queue!(out, Print(" ".repeat(cols - len)))?;
            }
        } else {
            let s: String = chars.into_iter().collect();
            queue!(out, Print(format!("{s:<cols$}", cols = cols)))?;
        }
    }
    Ok(())
}

fn finish_cursor(editor: &Editor, out: &mut impl Write, text_start_row: u16) -> io::Result<()> {
    if let Mode::Prompt(prompt) = &editor.mode {
        let row = editor
            .screen_rows
            .saturating_sub(if editor.options.nohelp { 1 } else { 3 });
        let col = if prompt.menu == Menu::YesNo {
            prompt.label.chars().count()
        } else {
            prompt.label.chars().count() + 2 + prompt.cursor
        };
        queue!(
            out,
            MoveTo(
                col.min(editor.screen_cols.saturating_sub(1)) as u16,
                row as u16
            ),
            Show
        )?;
    } else {
        let buf = editor.buf();
        let screen_line = buf.cursor.line.saturating_sub(buf.top_line);
        let gutter = editor.gutter_width();
        let cursor_col = crate::buffer::display_width(
            &buf.line(buf.cursor.line),
            buf.cursor.col,
            editor.options.tabsize as usize,
        );
        // `left_col` is only ever nonzero for the cursor's own line (see
        // `render_buffer`), and a `<` marker takes up one column whenever
        // it's scrolled, shifting everything after it right by one.
        let show_left = buf.left_col > 0;
        let col = gutter + if show_left { 1 } else { 0 } + cursor_col.saturating_sub(buf.left_col);
        queue!(
            out,
            MoveTo(
                col.min(editor.screen_cols.saturating_sub(1)) as u16,
                (text_start_row as usize + screen_line) as u16
            ),
            Show
        )?;
    }
    Ok(())
}

fn render_title_bar(editor: &Editor, out: &mut impl Write, cols: usize) -> io::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let left = format!("  tico {version}");
    let name = editor
        .buf()
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "New Buffer".to_string());
    let modified = if editor.buf().modified { " *" } else { "" };
    let mut center_text = format!("{name}{modified}");

    // When more than one buffer is open, show "[i/n]" (1-based current
    // buffer index / total buffer count) in the upper-right corner. Nano
    // shows this same indicator but replaces its version text with it; we
    // keep the "tico version" text on the left and add the indicator on
    // the right instead so neither is lost. `--view`'s "View" (nano's own
    // right-aligned "state" word) shares that same corner; the two are
    // independent, so both can show together.
    let mut indicator = String::new();
    if editor.options.view {
        indicator.push_str("View");
    }
    if editor.buffers.len() > 1 {
        if !indicator.is_empty() {
            indicator.push(' ');
        }
        indicator.push_str(&format!(
            "[{}/{}]",
            editor.current + 1,
            editor.buffers.len()
        ));
    }
    let right_w = if indicator.is_empty() {
        0
    } else {
        indicator.chars().count() + 2
    };

    // Reserve space for the left prefix and the right indicator (plus one
    // column of separation on each side); if the filename doesn't fit,
    // truncate it, keeping the tail (the most identifying part of a long
    // path) and prefixing "...".
    let left_w = left.chars().count();
    let available = cols.saturating_sub(left_w + right_w + 2);
    if center_text.chars().count() > available {
        if available > 3 {
            let tail: String = center_text
                .chars()
                .rev()
                .take(available - 3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            center_text = format!("...{tail}");
        } else {
            center_text.clear();
        }
    }

    let mut line = vec![' '; cols];
    for (i, c) in left.chars().enumerate() {
        if i < cols {
            line[i] = c;
        }
    }
    let start = (cols.saturating_sub(center_text.chars().count()) / 2).max(left_w + 1);
    for (i, c) in center_text.chars().enumerate() {
        if start + i < cols {
            line[start + i] = c;
        }
    }
    if !indicator.is_empty() {
        let ind_start = cols.saturating_sub(indicator.chars().count() + 2);
        for (i, c) in indicator.chars().enumerate() {
            let pos = ind_start + i;
            if pos < cols {
                line[pos] = c;
            }
        }
    }
    let s: String = line.into_iter().collect();
    let style = bar_style(&editor.options.titlecolor, BarStyle::Reverse);
    queue_bar_segment(out, style, &s)
}

/// The resolved style for the title bar, used directly by the title bar
/// itself and as the fallback for `promptcolor`/`minicolor` when those are
/// unset (nano: "the colors of the title bar are used").
fn title_bar_style(editor: &Editor) -> BarStyle {
    bar_style(&editor.options.titlecolor, BarStyle::Reverse)
}

fn render_status_line(
    editor: &Editor,
    out: &mut impl Write,
    row: u16,
    cols: usize,
) -> io::Result<()> {
    queue!(out, MoveTo(0, row))?;
    if let Mode::Prompt(prompt) = &editor.mode {
        // nano's promptcolor defaults to the title bar's colors (reverse
        // video unless titlecolor is set), confirmed against the installed
        // nano's own escape-code output for both the Search and WriteOut
        // prompts.
        let style = bar_style(&editor.options.promptcolor, title_bar_style(editor));
        // nano's `ask_user` shows a Yes/No question bare; only `do_prompt`
        // adds ": " and the answer being typed.
        let text = if prompt.menu == Menu::YesNo {
            prompt.label.clone()
        } else {
            format!(
                "{}: {}",
                prompt.label,
                prompt_input_for_display(editor, &prompt.input)
            )
        };
        let mut s: String = text.chars().take(cols).collect();
        while s.chars().count() < cols {
            s.push(' ');
        }
        queue_bar_segment(out, style, &s)
    } else if let Some(msg) = &editor.status {
        // nano shows ordinary status-bar messages in reverse video by
        // default, and Alert/Mild-level ones (unwritable file, "is a
        // directory", ...) in `errorcolor` (bold white-on-red by default)
        // instead, confirmed against the installed nano's own escape-code
        // output for both cases.
        let bracketed = format!("[ {msg} ]");
        let pad = cols.saturating_sub(bracketed.chars().count()) / 2;
        if pad > 0 {
            queue!(out, Print(" ".repeat(pad)))?;
        }
        let remaining = cols.saturating_sub(pad);
        let shown: String = bracketed.chars().take(remaining).collect();
        let shown_len = shown.chars().count();
        match editor.status_level {
            crate::app::StatusLevel::Normal => {
                let style = bar_style(&editor.options.statuscolor, BarStyle::Reverse);
                queue_bar_segment(out, style, &shown)?;
            }
            crate::app::StatusLevel::Mild | crate::app::StatusLevel::Alert => {
                // nano uses this same ERROR_MESSAGE color for both MILD and
                // ALERT messages; only ALERT also rings the bell (handled
                // via `bell_pending`, which `set_status_mild` never sets).
                let style = bar_style(&editor.options.errorcolor, BarStyle::Reverse);
                queue_bar_segment(out, style, &shown)?;
            }
        }
        let used = pad + shown_len;
        if used < cols {
            queue!(out, Print(" ".repeat(cols - used)))?;
        }
        Ok(())
    } else if editor.options.minibar {
        render_minibar(editor, out, cols)
    } else {
        queue!(out, Print(" ".repeat(cols)))
    }
}

/// `set minibar`'s condensed one-line summary of the current buffer, shown
/// where the status bar normally goes once no prompt or status message is
/// active: the filename (plus `*` if modified) on the left, then either the
/// one-shot `minibar_note` (right after a load/save/buffer-switch) or,
/// once that's gone, an `[i/n]` counter when multiple buffers are open, and
/// finally the cursor's percentage into the file, right-aligned. Colored
/// with `minicolor` (falling back to the title bar's own colors, same as
/// `promptcolor`) -- confirmed layout and colors against the installed
/// nano's own escape-code output.
fn render_minibar(editor: &Editor, out: &mut impl Write, cols: usize) -> io::Result<()> {
    let buf = editor.buf();
    let name = buf
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "New Buffer".to_string());
    let mark = if buf.modified { '*' } else { ' ' };
    let left = format!("  {name} {mark} ");

    let mut line: Vec<char> = vec![' '; cols];
    let mut cursor = 0;
    for c in left.chars() {
        if cursor >= cols {
            break;
        }
        line[cursor] = c;
        cursor += 1;
    }

    let right_note = editor.minibar_note.clone().or_else(|| {
        (editor.buffers.len() > 1)
            .then(|| format!("[{}/{}]", editor.current + 1, editor.buffers.len()))
    });
    if let Some(note) = right_note {
        for c in note.chars() {
            if cursor >= cols {
                break;
            }
            line[cursor] = c;
            cursor += 1;
        }
    }

    // Right-aligned, 2 columns in from the edge -- the same margin the
    // title bar's own `[i/n]`/"View" indicator uses.
    let pct = 100 * (buf.cursor.line + 1) / buf.line_count().max(1);
    let pct_str = format!("{pct}%");
    let pct_start = cols.saturating_sub(pct_str.chars().count() + 2);
    for (i, c) in pct_str.chars().enumerate() {
        if pct_start + i < cols {
            line[pct_start + i] = c;
        }
    }

    let s: String = line.into_iter().collect();
    let style = bar_style(&editor.options.minicolor, title_bar_style(editor));
    queue_bar_segment(out, style, &s)
}

/// The default main-menu shortcut priority list, in the exact order GNU
/// nano 8.7.1 lays them out (captured directly from the installed binary),
/// as (action, description) pairs — resolved against the *live* keymap at
/// render time (via `key_label_for`) rather than baking in a fixed key
/// label, so a `bind`/`unbind` in nanorc/ticorc, or `--modernbindings`,
/// shows up here immediately instead of leaving the bar showing stale
/// defaults.
const SHORTCUT_PRIORITY: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Exit, "Exit"),
    (Action::WriteOut, "Write Out"),
    (Action::Insert, "Read File"),
    (Action::WhereIs, "Where Is"),
    (Action::Replace, "Replace"),
    (Action::Cut, "Cut"),
    (Action::Paste, "Paste"),
    (Action::Execute, "Execute"),
    (Action::Justify, "Justify"),
    (Action::Location, "Location"),
    (Action::GotoLine, "Go To Line"),
    (Action::Undo, "Undo"),
    (Action::Redo, "Redo"),
    (Action::Mark, "Set Mark"),
    (Action::Copy, "Copy"),
    (Action::FindBracket, "To Bracket"),
    (Action::WhereWas, "Where Was"),
    (Action::FindPrevious, "Previous"),
    (Action::FindNext, "Next"),
];

/// The Search (WhereIs) prompt's shortcut list, captured the same way.
const SEARCH_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::CaseSens, "Case Sens"),
    (Action::Regexp, "Reg.exp."),
    (Action::Backwards, "Backwards"),
    (Action::FlipReplace, "Replace"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
    (Action::FlipGoto, "Go To Line"),
];

/// The "Search (to replace)" prompt: same as Search but without ^T
/// (MREPLACE isn't bound to flip_goto in nano) and ^R now offers to flip
/// *back* to plain search.
const REPLACE1_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::CaseSens, "Case Sens"),
    (Action::Regexp, "Reg.exp."),
    (Action::Backwards, "Backwards"),
    (Action::FlipReplace, "No Replace"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
];

const REPLACEWITH_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
];

const GOTOLINE_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::BeginPara, "Begin of Paragr."),
    (Action::EndPara, "End of Paragraph"),
    (Action::FirstLine, "First Line"),
    (Action::LastLine, "Last Line"),
    (Action::FlipGoto, "Go To Text"),
];

/// The `^R` Read File prompt's shortcut list, matching nano's full menu.
const INSERT_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::FlipNewBuffer, "New Buffer"),
    (Action::FlipConvert, "No Conversion"),
    (Action::FlipExecute, "Execute Command"),
    (Action::Browser, "Browse"),
];

/// The `^O` Write Out prompt's shortcut list, matching nano 8.7's
/// MWRITEFILE bar (confirmed against the installed nano). Append,
/// Prepend and Backup File aren't implemented yet but are listed rather
/// than silently omitted.
const WRITEOUT_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::DosFormat, "DOS Format"),
    (Action::MacFormat, "Mac Format"),
    (Action::Append, "Append"),
    (Action::Prepend, "Prepend"),
    (Action::Backup, "Backup File"),
    (Action::DiscardBuffer, "Discard buffer"),
    (Action::Browser, "Browse"),
];

/// The `^T` Execute Command prompt's shortcut list, matching nano's full
/// MEXECUTE menu (confirmed against the installed nano's own bottom bar).
/// Full Justify (`^J`), Cut Till End (`^V`), and Pipe Text (`M-\`) aren't
/// actually implemented yet — see apply_prompt_action's arms for them —
/// but are still listed rather than silently omitted.
const EXECUTE_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
    (Action::FlipNewBuffer, "New Buffer"),
    (Action::FlipPipe, "Pipe Text"),
    (Action::Speller, "Spell Check"),
    (Action::Linter, "Linter"),
    (Action::FullJustify, "Full Justify"),
    (Action::Formatter, "Formatter"),
    (Action::CutRestOfFile, "Cut Till End"),
    (Action::Suspend, "Suspend"),
];

/// The linter's interactive result viewer (`MLINTER`): Cancel plus
/// PageUp/PageDown to step to the previous/next reported message.
const LINTER_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Cancel, "Cancel"),
    (Action::PageUp, "Previous Linter message"),
    (Action::PageDown, "Next Linter message"),
];

/// The file browser's bar, in the order of nano's MBROWSER functions
/// (src/global.c; the installed nano shows the first twelve at 80
/// columns).
const BROWSER_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Exit, "Close"),
    (Action::GotoDir, "Go To Dir"),
    (Action::Refresh, "Refresh"),
    (Action::WhereIs, "Where Is"),
    (Action::WhereWas, "Where Was"),
    (Action::FindPrevious, "Previous"),
    (Action::FindNext, "Next"),
    (Action::Left, "Back"),
    (Action::Right, "Forward"),
    (Action::Up, "Prev Line"),
    (Action::Down, "Next Line"),
    (Action::PageUp, "Prev Page"),
    (Action::PageDown, "Next Page"),
    (Action::FirstFile, "First File"),
    (Action::LastFile, "Last File"),
    (Action::PrevWord, "Left Column"),
    (Action::NextWord, "Right Column"),
    (Action::PrevBlock, "Top Row"),
    (Action::NextBlock, "Bottom Row"),
];

/// The browser's Search prompt (`MWHEREISFILE`), confirmed against the
/// installed nano.
const WHEREISFILE_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Help, "Help"),
    (Action::Cancel, "Cancel"),
    (Action::Older, "Older"),
    (Action::Newer, "Newer"),
    (Action::FirstFile, "First File"),
    (Action::LastFile, "Last File"),
];

/// The browser's Go To Directory prompt (`MGOTODIR`), likewise.
const GOTODIR_SHORTCUTS: &[(Action, &str)] = &[(Action::Help, "Help"), (Action::Cancel, "Cancel")];

/// The `^G` help viewer's own bottom bar (confirmed against the installed
/// nano's help screen).
const HELP_SHORTCUTS: &[(Action, &str)] = &[
    (Action::Up, "Prev Line"),
    (Action::PageUp, "Prev Page"),
    (Action::FirstLine, "First Line"),
    (Action::Cancel, "Close"),
    (Action::Down, "Next Line"),
    (Action::PageDown, "Next Page"),
    (Action::LastLine, "Last Line"),
];

/// The "file changed on disk, you have unsaved edits" choice prompt: R/K/M
/// aren't keymap-driven (this prompt is tico-original and matches those
/// raw keystrokes directly — see `handle_conflict_choice`), so only the
/// `^G` entry is resolved against the live keymap; the rest stay literal.
const EXTERNAL_CONFLICT_SHORTCUTS: &[(&str, &str)] = &[
    ("R", "Reload"),
    ("K", "Keep mine"),
    ("M", "Merge"),
    ("I", "Ignore All"),
];

/// nano's plain Yes/No questions ("Save modified buffer?", and Write
/// Out's follow-up questions) — confirmed against the installed nano's own
/// bar. The blank third entry keeps Cancel in the bottom-right slot,
/// matching nano's layout (its Y/N/^C bar isn't a plain fill-in-order
/// grid: Yes/No stack in the left column, Cancel sits alone at bottom
/// right).
const YES_NO_SHORTCUTS: &[(&str, &str)] = &[("Y", "Yes"), ("N", "No"), ("", ""), ("^C", "Cancel")];

/// "Replace this instance?" — confirmed against the installed nano's own
/// bar; unlike the exit prompt this one fills all four slots, so no blank
/// padding is needed.
const REPLACE_CONFIRM_SHORTCUTS: &[(&str, &str)] =
    &[("Y", "Yes"), ("N", "No"), ("A", "All"), ("^C", "Cancel")];

/// The lock-conflict prompt ("File is being edited by ...; open anyway?")
/// is tico-original (no nano equivalent — nano has no interactive
/// lock-file prompt). ^C/N both decline identically (see
/// handle_lock_conflict_choice), so — matching the same reasoning that
/// dropped the deconflict prompt's redundant Cancel — only Yes/No are
/// advertised here.
const LOCK_CONFLICT_SHORTCUTS: &[(&str, &str)] = &[("Y", "Yes"), ("N", "No")];

/// Which shortcut list to show at the bottom for the current prompt (or
/// the main editing window, for `None`), with each entry's key label
/// resolved against the *live* keymap — nano rebuilds its two help lines
/// per-menu the same way (see e.g. `bottombars()` in its winio.c, which
/// looks up each function's current binding rather than a fixed table);
/// menus/prompts not yet curated here fall back to Main's list rather than
/// showing nothing.
fn shortcut_bar_entries(keymap: &KeyMap, prompt: Option<&Prompt>) -> Vec<(String, &'static str)> {
    let Some(p) = prompt else {
        return resolve_shortcuts(keymap, Menu::Main, SHORTCUT_PRIORITY);
    };
    if matches!(p.kind, PromptKind::ExternalChangeConflict) {
        let mut entries: Vec<(String, &str)> = EXTERNAL_CONFLICT_SHORTCUTS
            .iter()
            .map(|&(k, d)| (k.to_string(), d))
            .collect();
        entries.push((key_label_for(keymap, Menu::YesNo, Action::Help), "Get Help"));
        return entries;
    }
    // The other Y/N-style choice prompts: none of Y/N/A/^C go through the
    // keymap (they're raw keystrokes each handler matches directly — see
    // handle_exit_choice/handle_lock_conflict_choice/
    // handle_replace_confirm_choice), so these stay literal too.
    let literal: Option<&[(&str, &str)]> = match &p.kind {
        PromptKind::Exit { .. } | PromptKind::WriteConfirm { .. } => Some(YES_NO_SHORTCUTS),
        PromptKind::LockConflict { .. } => Some(LOCK_CONFLICT_SHORTCUTS),
        PromptKind::ReplaceConfirm(_) => Some(REPLACE_CONFIRM_SHORTCUTS),
        _ => None,
    };
    if let Some(table) = literal {
        return table.iter().map(|&(k, d)| (k.to_string(), d)).collect();
    }
    let table: &[(Action, &str)] = match p.menu {
        Menu::Search => SEARCH_SHORTCUTS,
        Menu::Replace => REPLACE1_SHORTCUTS,
        Menu::ReplaceWith => REPLACEWITH_SHORTCUTS,
        Menu::GotoLine => GOTOLINE_SHORTCUTS,
        Menu::Help => HELP_SHORTCUTS,
        Menu::Insert => INSERT_SHORTCUTS,
        Menu::WriteOut => WRITEOUT_SHORTCUTS,
        Menu::Execute => EXECUTE_SHORTCUTS,
        Menu::Linter => LINTER_SHORTCUTS,
        Menu::WhereIsFile => WHEREISFILE_SHORTCUTS,
        Menu::GotoDir => GOTODIR_SHORTCUTS,
        _ => return resolve_shortcuts(keymap, Menu::Main, SHORTCUT_PRIORITY),
    };
    resolve_shortcuts(keymap, p.menu, table)
}

/// Resolve each `(action, description)` pair in `table` to
/// `(current key label for that action in `menu`, description)`.
fn resolve_shortcuts(
    keymap: &KeyMap,
    menu: Menu,
    table: &[(Action, &'static str)],
) -> Vec<(String, &'static str)> {
    table
        .iter()
        .map(|&(action, desc)| (key_label_for(keymap, menu, action), desc))
        .collect()
}

/// The best key currently bound to `action` within `menu` — "best" meaning
/// the one `Key::display_rank` would show first among alternates (Ctrl
/// before function keys before Meta), matching how the `^G` help screen
/// already picks a primary key to display. Empty if nothing is bound
/// (e.g. the user `unbind`-ed it), rather than showing a stale label.
fn key_label_for(keymap: &KeyMap, menu: Menu, action: Action) -> String {
    keymap
        .entries()
        .filter(|((m, _), binding)| *m == menu && **binding == Binding::Action(action))
        .map(|((_, k), _)| *k)
        .min_by_key(|k| k.display_rank())
        .map(|k| k.describe())
        .unwrap_or_default()
}

/// The shortcut bar's column grid: shared by the renderer and by mouse
/// click resolution, so the two can never drift apart the way `text_rows`
/// and `render`'s own row math once did (see `note_buffer_linecount`'s
/// history). `col_width` is the width, in columns, of one (key, desc)
/// cell; `n_pairs` is how many such cells fit across `cols`, each holding
/// up to two entries (one per row of the two-line bar).
struct ShortcutBarLayout {
    col_width: usize,
    n_pairs: usize,
}

fn shortcut_bar_layout(cols: usize, entries: &[(String, &str)]) -> ShortcutBarLayout {
    let max_label = entries
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(2);
    let max_desc = entries
        .iter()
        .map(|(_, d)| d.chars().count())
        .max()
        .unwrap_or(4);
    let col_width = max_label + 1 + max_desc + 2;
    let n_cols = (cols / col_width).max(1);
    let n_pairs = n_cols.min(entries.len().div_ceil(2));
    ShortcutBarLayout { col_width, n_pairs }
}

/// The entry (if any) a mouse click at `(row_in_bar, col)` -- 0-based
/// coordinates relative to the shortcut bar's own top-left corner -- would
/// activate, matching nano's own `get_mouseinput`'s shortcut-click math
/// (adapted to tico's own column layout, which sizes columns to the
/// longest label/description rather than nano's fixed `COLS /
/// ((number+1)/2)` grid).
fn shortcut_bar_click_index(
    cols: usize,
    entries: &[(String, &str)],
    row_in_bar: usize,
    col: usize,
) -> Option<usize> {
    if row_in_bar > 1 {
        return None;
    }
    let layout = shortcut_bar_layout(cols, entries);
    let c = col / layout.col_width;
    if c >= layout.n_pairs {
        return None;
    }
    let idx = c * 2 + row_in_bar;
    let (key, _) = entries.get(idx)?;
    if key.is_empty() {
        return None;
    }
    Some(idx)
}

fn render_shortcut_bar(
    editor: &Editor,
    out: &mut impl Write,
    row: u16,
    cols: usize,
    entries: &[(String, &str)],
) -> io::Result<()> {
    let key_style = bar_style(&editor.options.keycolor, BarStyle::Reverse);
    let desc_style = bar_style(&editor.options.functioncolor, BarStyle::Plain);
    let ShortcutBarLayout { col_width, n_pairs } = shortcut_bar_layout(cols, entries);

    for r in 0..2u16 {
        queue!(out, MoveTo(0, row + r))?;
        let mut written = 0usize;
        for c in 0..n_pairs {
            let idx = c * 2 + r as usize;
            if let Some((key, desc)) = entries.get(idx).filter(|(k, _)| !k.is_empty()) {
                // As in nano's `post_one_key`: just the key combo itself in
                // `keycolor` (reverse video by default) -- not padded out to
                // the widest key in the bar -- then one blank and the
                // description in `functioncolor` (the terminal's normal
                // colors by default); the rest of the column is plain.
                queue_bar_segment(out, key_style, key)?;
                queue!(out, Print(" "))?;
                queue_bar_segment(out, desc_style, desc)?;
                let used = key.chars().count() + 1 + desc.chars().count();
                let pad = col_width.saturating_sub(used);
                queue!(out, Print(" ".repeat(pad)))?;
                written += used + pad;
            } else {
                let pad = " ".repeat(col_width);
                queue!(out, Print(&pad))?;
                written += pad.chars().count();
            }
        }
        if written < cols {
            queue!(out, Print(" ".repeat(cols - written)))?;
        }
    }
    Ok(())
}

/// The `^R` Read File prompt's `Tab`-completion listing, shown in place of
/// the buffer — matches nano's `input_tab` (`blank_edit()` + a sorted,
/// multi-column grid, bottom-aligned within the edit window, with
/// `"(more)"` in the last cell when it doesn't all fit).
fn render_completions_grid(
    out: &mut impl Write,
    start_row: u16,
    rows: usize,
    cols: usize,
    matches: &[String],
) -> io::Result<()> {
    for r in 0..rows {
        queue!(
            out,
            MoveTo(0, start_row + r as u16),
            Print(" ".repeat(cols))
        )?;
    }
    if matches.is_empty() || rows == 0 || cols == 0 {
        return Ok(());
    }
    let longest = matches
        .iter()
        .map(|m| m.chars().count())
        .max()
        .unwrap_or(0)
        .min(cols.saturating_sub(1));
    let col_width = longest + 2;
    let ncols = ((cols + 1) / col_width).max(1);
    let nrows = matches.len().div_ceil(ncols);
    let top_row = rows.saturating_sub(nrows);

    let mut row = top_row;
    for (i, name) in matches.iter().enumerate() {
        if row >= rows {
            break;
        }
        let col_idx = i % ncols;
        let is_last_row = row == rows - 1;
        let fills_row = (i + 1) % ncols == 0;
        let more_remain = i + 1 < matches.len();
        if is_last_row && fills_row && more_remain {
            queue!(
                out,
                MoveTo((col_width * col_idx) as u16, start_row + row as u16),
                Print("(more)")
            )?;
            break;
        }
        let display: String = name.chars().take(longest).collect();
        queue!(
            out,
            MoveTo((col_width * col_idx) as u16, start_row + row as u16),
            Print(&display)
        )?;
        if fills_row {
            row += 1;
        }
    }
    Ok(())
}

fn render_buffer(
    editor: &Editor,
    out: &mut impl Write,
    start_row: u16,
    rows: usize,
) -> io::Result<()> {
    let buf = editor.buf();
    let gutter = editor.gutter_width();
    let cols = editor.screen_cols;
    let tabsize = editor.options.tabsize as usize;
    let whitespace = editor
        .options
        .whitespacedisplay
        .then_some(editor.options.whitespace);

    // Memoized on the buffer itself, invalidated only by an actual edit or
    // language change (see `Buffer::highlighted_spans_cached`) -- a full
    // tree-sitter reparse plus query run is too expensive to redo on every
    // render, which used to happen even for pure cursor movement. Buffers
    // over `max_syntax_highlight_bytes` skip highlighting altogether
    // (tico-only safety valve; see `maybe_warn_highlighting_disabled_for_size`
    // for the one-time status notice) -- checking that is just a length
    // read, no parsing attempted.
    let too_large_to_highlight =
        buf.rope.len_bytes() as u64 > editor.options.max_syntax_highlight_bytes;
    let spans: Vec<crate::syntax::HighlightSpan> =
        if editor.options.syntax_highlighting && !too_large_to_highlight {
            buf.language
                .map(|lang| buf.highlighted_spans_cached(lang))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
    let selection = editor.selection_range();
    let number_style = numbercolor_style(&editor.options.numbercolor);

    // `set indicator`: a one-column "scrollbar" on the right edge, showing
    // the viewport's position and extent within the buffer -- suppressed
    // on a too-small screen, matching nano's own `sidebar` guard exactly.
    let sidebar = usize::from(editor.options.indicator && cols > 9 && editor.screen_rows > 5);
    let (thumb_lowest, thumb_highest) = if sidebar == 1 && rows > 0 {
        scrollbar_thumb_range(buf.top_line, rows, buf.line_count())
    } else {
        (0, 0)
    };
    let scrollbar_track_style = scrollbar_cell_style(&editor.options.scrollercolor, false);
    let scrollbar_thumb_style = scrollbar_cell_style(&editor.options.scrollercolor, true);
    // `set guidestripe`: a one-column vertical guide recoloring whatever
    // character (or blank) already sits at that column, so it moves with
    // horizontal scroll -- the given column number is 1-based.
    let stripe_col = editor
        .options
        .guidestripe
        .map(|n| (n as usize).saturating_sub(1));
    let stripe_style = stripecolor_style(&editor.options.stripecolor);

    for r in 0..rows {
        queue!(out, MoveTo(0, start_row + r as u16))?;
        let line_idx = buf.top_line + r;
        let mut rendered = String::new();
        // Syntax-highlight style, one entry per char of `rendered`.
        let mut styles: Vec<Option<Style>> = Vec::new();
        // Character range within `rendered` (post-gutter, post-tab-expansion,
        // pre-horizontal-scroll) to paint with spotlightcolor, taking
        // precedence over syntax colors, if the active search/replace match
        // is on this line.
        let mut highlight: Option<(usize, usize)> = None;
        // Character range covering the marked selection on this line, if
        // any -- lower priority than `highlight` (an active search/replace
        // match), matching nano's own SELECTED_TEXT vs. spotlight layering.
        let mut selected: Option<(usize, usize)> = None;
        let mut gutter_chars = 0;
        let is_real_line = line_idx < buf.line_count();

        if is_real_line {
            if gutter > 0 {
                let prefix = format!("{:>width$} ", line_idx + 1, width = gutter - 1);
                styles.extend(prefix.chars().map(|_| Some(number_style)));
                rendered.push_str(&prefix);
            }
            let raw = buf.line(line_idx);
            gutter_chars = rendered.chars().count();

            let line_start = buf.line_start_byte(line_idx);
            let char_styles = map_spans_to_line(&raw, line_start, &spans, editor);

            let (expanded, expanded_styles) =
                expand_tabs_with_styles(&raw, &char_styles, tabsize, whitespace);
            rendered.push_str(&expanded);
            styles.extend(expanded_styles);

            if let Some((pos, len)) = editor.spotlight
                && pos.line == line_idx
            {
                let start = gutter_chars + crate::buffer::display_width(&raw, pos.col, tabsize);
                let end = gutter_chars + crate::buffer::display_width(&raw, pos.col + len, tabsize);
                if end > start {
                    highlight = Some((start, end));
                }
            }

            if let Some((sel_start, sel_end)) = selection
                && line_idx >= sel_start.line
                && line_idx <= sel_end.line
            {
                let start_col = if line_idx == sel_start.line {
                    sel_start.col
                } else {
                    0
                };
                let end_col = if line_idx == sel_end.line {
                    sel_end.col
                } else {
                    raw.chars().count()
                };
                let start = gutter_chars + crate::buffer::display_width(&raw, start_col, tabsize);
                let end = gutter_chars + crate::buffer::display_width(&raw, end_col, tabsize);
                if end > start {
                    selected = Some((start, end));
                }
            }
        } else if gutter > 0 {
            rendered.push('~');
            styles.push(None);
        }

        // Horizontal scroll: only the cursor's own line ever gets a nonzero
        // offset (nano scrolls just the current line sideways, not the
        // whole viewport — confirmed against the installed nano). A `<`
        // marker appears once scrolled; a `>` marker appears whenever the
        // line's text still overflows the available width, on any line.
        let full_chars: Vec<char> = rendered.chars().collect();
        let text_total = full_chars.len().saturating_sub(gutter_chars);
        let left = if is_real_line && line_idx == buf.cursor.line {
            buf.left_col.min(text_total)
        } else {
            0
        };
        let content_width = cols.saturating_sub(gutter_chars + sidebar);
        let show_left = left > 0;
        let mut capacity = content_width.saturating_sub(if show_left { 1 } else { 0 });
        let show_right = left + capacity < text_total;
        if show_right {
            capacity = capacity.saturating_sub(1);
        }
        let vis_start = gutter_chars + left;
        let vis_end = (vis_start + capacity).min(full_chars.len());

        let mut chars: Vec<char> = full_chars[..gutter_chars].to_vec();
        if show_left {
            chars.push('<');
        }
        chars.extend_from_slice(&full_chars[vis_start..vis_end]);
        if show_right {
            chars.push('>');
        }
        let mut windowed_styles: Vec<Option<Style>> = styles[..gutter_chars].to_vec();
        if show_left {
            windowed_styles.push(None);
        }
        windowed_styles.extend_from_slice(&styles[vis_start..vis_end]);
        if show_right {
            windowed_styles.push(None);
        }
        let mut styles = windowed_styles;
        let marker_shift = gutter_chars + if show_left { 1 } else { 0 };
        let clamp_to_view = |(s, e): (usize, usize)| {
            let clamp = |x: usize| marker_shift + x.clamp(vis_start, vis_end) - vis_start;
            (clamp(s), clamp(e))
        };
        let highlight = highlight.map(clamp_to_view);
        let selected = selected.map(clamp_to_view);

        // The stripe recolors whatever's already at its column, so it
        // scrolls along with the line -- hidden once scrolled past it
        // (`stripe_col < left`) or off the right edge of the content area.
        // It can land past the line's actual text (a short line, or past
        // end-of-buffer's own blank rows), in which case `chars`/`styles`
        // are extended with a plain space to carry it, matching nano's own
        // fallback of painting a space there.
        let stripe_idx = stripe_col
            .and_then(|col| stripe_content_offset(col, left, content_width))
            .map(|offset| marker_shift + offset);
        if let Some(idx) = stripe_idx
            && idx >= chars.len()
        {
            chars.resize(idx + 1, ' ');
            styles.resize(idx + 1, None);
        }

        let len = chars.len();
        let spot = highlight
            .map(|(s, e)| (s.min(len), e.min(len)))
            .filter(|(s, e)| s < e);
        let sel = selected
            .map(|(s, e)| (s.min(len), e.min(len)))
            .filter(|(s, e)| s < e);

        let (spot_fg, spot_bg) = spotlight_colors(&editor.options.spotlightcolor);
        let selection_style = selection_render_style(&editor.options.selectedcolor);
        let mut i = 0;
        while i < len {
            let in_spot = spot.is_some_and(|(s, e)| i >= s && i < e);
            let in_sel = !in_spot && sel.is_some_and(|(s, e)| i >= s && i < e);
            let in_stripe = !in_spot && !in_sel && stripe_idx == Some(i);
            let style = if in_spot || in_sel || in_stripe {
                None
            } else {
                styles.get(i).copied().flatten()
            };
            let mut j = i + 1;
            while j < len {
                let j_in_spot = spot.is_some_and(|(s, e)| j >= s && j < e);
                let j_in_sel = !j_in_spot && sel.is_some_and(|(s, e)| j >= s && j < e);
                let j_in_stripe = !j_in_spot && !j_in_sel && stripe_idx == Some(j);
                if j_in_spot != in_spot || j_in_sel != in_sel || j_in_stripe != in_stripe {
                    break;
                }
                let j_style = if j_in_spot || j_in_sel || j_in_stripe {
                    None
                } else {
                    styles.get(j).copied().flatten()
                };
                if j_style != style {
                    break;
                }
                j += 1;
            }
            let segment: String = chars[i..j].iter().collect();
            if in_spot {
                queue!(
                    out,
                    SetForegroundColor(spot_fg),
                    SetBackgroundColor(spot_bg),
                    Print(segment),
                    SetAttribute(Attribute::Reset)
                )?;
            } else if in_sel {
                match selection_style {
                    SelectionStyle::Reverse => {
                        queue!(
                            out,
                            SetAttribute(Attribute::Reverse),
                            Print(segment),
                            SetAttribute(Attribute::Reset)
                        )?;
                    }
                    SelectionStyle::Colored(fg, bg) => {
                        queue!(
                            out,
                            SetForegroundColor(fg),
                            SetBackgroundColor(bg),
                            Print(segment),
                            SetAttribute(Attribute::Reset)
                        )?;
                    }
                }
            } else if in_stripe {
                print_styled(out, &segment, Some(stripe_style))?;
            } else {
                print_styled(out, &segment, style)?;
            }
            i = j;
        }
        let fill_width = cols.saturating_sub(sidebar);
        if len < fill_width {
            queue!(out, Print(" ".repeat(fill_width - len)))?;
        }
        if sidebar == 1 {
            let in_thumb = r >= thumb_lowest && r <= thumb_highest;
            let style = if in_thumb {
                scrollbar_thumb_style
            } else {
                scrollbar_track_style
            };
            print_styled(out, " ", Some(style))?;
        }
    }
    Ok(())
}

/// The "plain reverse video by default, `cp`'s own colors when configured"
/// pattern shared by `numbercolor` and `stripecolor`: confirmed against the
/// installed nano's own escape-code output for both the line-number margin
/// (`set linenumbers`) and the vertical guide (`set guidestripe`) -- nano's
/// own color-pair setup gives both LINE_NUMBER and GUIDE_STRIPE plain
/// `A_REVERSE` when unconfigured, unlike `scrollercolor`/`functioncolor`'s
/// `A_NORMAL` default.
fn reverse_default_style(cp: &crate::options::ColorPair) -> Style {
    if cp.fg.is_none() && cp.bg.is_none() {
        Style {
            modifiers: crate::theme::Modifiers::any(false, false, true),
            ..Style::default()
        }
    } else {
        Style {
            fg: cp.fg.map(map_named_color),
            bg: cp.bg.map(map_named_color),
            modifiers: crate::theme::Modifiers::any(cp.bold, cp.italic, false),
            ..Style::default()
        }
    }
}

fn numbercolor_style(cp: &crate::options::ColorPair) -> Style {
    reverse_default_style(cp)
}

/// `stripecolor`'s resolved style for `set guidestripe`'s vertical guide.
fn stripecolor_style(cp: &crate::options::ColorPair) -> Style {
    reverse_default_style(cp)
}

/// Where `set guidestripe`'s vertical guide falls within one row's visible
/// content, if at all -- `stripe_col` and `left` (the row's horizontal
/// scroll offset) are both 0-based display columns, and `content_width` is
/// how many display columns the content area itself has. The result is an
/// offset from the content area's own start (so a caller windowing the row
/// still needs to add its own left-marker shift) -- `None` when the
/// configured column has been scrolled past on either side, matching
/// nano's own `draw_row`'s bounds check exactly (confirmed against the
/// installed nano's own escape-code output at both edges).
fn stripe_content_offset(stripe_col: usize, left: usize, content_width: usize) -> Option<usize> {
    let offset = stripe_col.checked_sub(left)?;
    (offset < content_width).then_some(offset)
}

/// The row range (inclusive, 0-based within the viewport) of `set
/// indicator`'s scrollbar "thumb" -- the portion representing the current
/// viewport within the whole buffer. Matches nano's own `draw_scrollbar`
/// exactly (`lowest`/`highest`, for the no-softwrap case, since tico
/// doesn't support `softwrap` yet): `top_line` is 0-based, `viewport_rows`
/// is the edit window's height, and `total_lines` is the buffer's own
/// (ropey-native) line count, consistent with nano's `filebot->lineno`.
fn scrollbar_thumb_range(
    top_line: usize,
    viewport_rows: usize,
    total_lines: usize,
) -> (usize, usize) {
    let total_lines = total_lines.max(1);
    let lowest = (top_line * viewport_rows) / total_lines;
    let mut highest = lowest + (viewport_rows * viewport_rows) / total_lines;
    if viewport_rows > total_lines {
        // The whole buffer already fits without scrolling: the thumb
        // covers the entire bar.
        highest = viewport_rows;
    }
    (lowest, highest)
}

/// `scrollercolor`'s resolved style for one cell of `set indicator`'s
/// scrollbar column: unlike the other bars, its unset default is no color
/// at all (matching `functioncolor`, not the usual reverse-video default),
/// and the "thumb" (the portion representing the current viewport) is
/// always additionally reverse-video, on top of whatever `scrollercolor`
/// resolves to -- nano ORs `A_REVERSE` onto the color pair rather than
/// treating it as a separate, mutually exclusive style. Confirmed against
/// the installed nano's own escape-code output, unconfigured and with an
/// explicit `set scrollercolor`.
fn scrollbar_cell_style(cp: &crate::options::ColorPair, thumb: bool) -> Style {
    Style {
        fg: cp.fg.map(map_named_color),
        bg: cp.bg.map(map_named_color),
        modifiers: crate::theme::Modifiers::any(cp.bold, cp.italic, thumb),
        ..Style::default()
    }
}

/// Map a nanorc color spec to the crossterm colors that produce the same
/// escape codes as nano itself (crossterm's naming is inverted from
/// nano's: `Color::Red` is bright/light red, `Color::DarkRed` is the
/// standard-intensity red nano means by plain "red" — confirmed against
/// crossterm's own SGR-generation source).
fn spotlight_colors(cp: &crate::options::ColorPair) -> (Color, Color) {
    let fg = cp.fg.map(map_named_color).unwrap_or(Color::Black);
    let bg = cp.bg.map(map_named_color).unwrap_or(Color::Yellow);
    (fg, bg)
}

/// How to paint a title/status/prompt bar, an error message, or the
/// key-combo half of a shortcut-bar entry. nano's own default, whenever the
/// corresponding `set XXXcolor` is unset, is plain reverse video (confirmed
/// against the installed nano's own escape-code output); an explicit
/// setting instead paints with exactly the given colors, bold and italic.
/// `functioncolor`'s unset default is no color at all (`Plain`), matching
/// nano's own behavior for the shortcut bar's descriptions.
#[derive(Clone, Copy)]
enum BarStyle {
    Reverse,
    Plain,
    Colored {
        fg: Color,
        bg: Color,
        bold: bool,
        italic: bool,
    },
}

/// Resolve a `set XXXcolor` option to how it should actually be painted,
/// falling back to `default` (nano's own per-option default: `Reverse` for
/// most bars, `Plain` for `functioncolor`, or another bar's already-resolved
/// style for `promptcolor`/`minicolor`, which fall back to `titlecolor`)
/// when the option was never configured.
fn bar_style(cp: &crate::options::ColorPair, default: BarStyle) -> BarStyle {
    if cp.fg.is_none() && cp.bg.is_none() {
        default
    } else {
        BarStyle::Colored {
            fg: cp.fg.map(map_named_color).unwrap_or(Color::Reset),
            bg: cp.bg.map(map_named_color).unwrap_or(Color::Reset),
            bold: cp.bold,
            italic: cp.italic,
        }
    }
}

/// Print `text` in `style`, resetting afterwards (a no-op for `Plain`).
fn queue_bar_segment(out: &mut impl Write, style: BarStyle, text: &str) -> io::Result<()> {
    match style {
        BarStyle::Plain => queue!(out, Print(text)),
        BarStyle::Reverse => queue!(
            out,
            SetAttribute(Attribute::Reverse),
            Print(text),
            SetAttribute(Attribute::Reset)
        ),
        BarStyle::Colored {
            fg,
            bg,
            bold,
            italic,
        } => {
            queue!(out, SetForegroundColor(fg), SetBackgroundColor(bg))?;
            if bold {
                queue!(out, SetAttribute(Attribute::Bold))?;
            }
            if italic {
                queue!(out, SetAttribute(Attribute::Italic))?;
            }
            queue!(out, Print(text), SetAttribute(Attribute::Reset))
        }
    }
}

/// How to paint the marked selection (`buf.mark`).
#[derive(Clone, Copy)]
enum SelectionStyle {
    /// nano's own default (`hilite_attribute`, `A_REVERSE`): plain reverse
    /// video, used whenever `selectedcolor` hasn't been configured.
    Reverse,
    /// An explicit `set selectedcolor` — a real color pair, like spotlight.
    Colored(Color, Color),
}

fn selection_render_style(cp: &crate::options::ColorPair) -> SelectionStyle {
    if cp.fg.is_none() && cp.bg.is_none() {
        return SelectionStyle::Reverse;
    }
    let fg = cp.fg.map(map_named_color).unwrap_or(Color::Reset);
    let bg = cp.bg.map(map_named_color).unwrap_or(Color::Reset);
    SelectionStyle::Colored(fg, bg)
}

fn map_named_color(nc: crate::options::NamedColor) -> Color {
    use crate::options::Color as OC;
    match nc.color {
        OC::Black => {
            if nc.light {
                Color::DarkGrey
            } else {
                Color::Black
            }
        }
        OC::Red => {
            if nc.light {
                Color::Red
            } else {
                Color::DarkRed
            }
        }
        OC::Green => {
            if nc.light {
                Color::Green
            } else {
                Color::DarkGreen
            }
        }
        OC::Yellow => {
            if nc.light {
                Color::Yellow
            } else {
                Color::DarkYellow
            }
        }
        OC::Blue => {
            if nc.light {
                Color::Blue
            } else {
                Color::DarkBlue
            }
        }
        OC::Magenta => {
            if nc.light {
                Color::Magenta
            } else {
                Color::DarkMagenta
            }
        }
        OC::Cyan => {
            if nc.light {
                Color::Cyan
            } else {
                Color::DarkCyan
            }
        }
        OC::White => {
            if nc.light {
                Color::White
            } else {
                Color::Grey
            }
        }
        OC::Normal => Color::Reset,
        OC::Rgb(r, g, b) => Color::Rgb { r, g, b },
        // nano's extended hue names (`lime`, `pink`, ...) are already a
        // literal 256-color palette index; `light` doesn't apply to them.
        OC::Indexed(i) => Color::AnsiValue(i),
    }
}

/// Map whole-buffer byte-offset highlight spans onto one line's characters
/// as resolved theme styles: `raw` is that line's text, `line_start` its
/// byte offset in the text `spans` were computed from. Shared by the main
/// editor buffer and the merge-diff viewer, which both highlight a block of
/// text line-by-line.
///
/// Spans whose scope the theme says nothing about are skipped rather than
/// painted as "no style", so an inner capture a theme doesn't cover (say
/// `punctuation.bracket` inside a string) leaves the enclosing span's style
/// showing through — the same layering Helix produces.
///
/// The theme is chosen per span from the language that produced it
/// (`editor.theme_for`), so an injected heredoc body follows its own
/// language's `[syntax]` override rather than the enclosing buffer's.
fn map_spans_to_line(
    raw: &str,
    line_start: usize,
    spans: &[crate::syntax::HighlightSpan],
    editor: &Editor,
) -> Vec<Option<Style>> {
    let mut char_styles = vec![None; raw.chars().count()];
    if spans.is_empty() {
        return char_styles;
    }
    let line_end = line_start + raw.len();
    for span in spans {
        if span.end <= line_start || span.start >= line_end {
            continue;
        }
        let Some(style) = editor.theme_for(span.language).style(span.scope) else {
            continue;
        };
        let rel_start = span.start.max(line_start) - line_start;
        let rel_end = span.end.min(line_end) - line_start;
        let cs = raw[..rel_start].chars().count();
        let ce = raw[..rel_end].chars().count();
        for k in &mut char_styles[cs..ce] {
            *k = Some(style);
        }
    }
    char_styles
}

/// Like tab expansion alone, but carries each source character's syntax
/// style along to every column it expands to, so a tab adjacent to a
/// highlighted token doesn't break the highlighting. With `whitespace`
/// (the `set whitespace` pair, passed when `whitespacedisplay` is on) a
/// tab shows as its marker followed by the usual fill to the next tab
/// stop, and a space as its marker -- nano's `display_string`, whose
/// markers take exactly the columns the blanks did.
fn expand_tabs_with_styles(
    line: &str,
    styles: &[Option<Style>],
    tabsize: usize,
    whitespace: Option<(char, char)>,
) -> (String, Vec<Option<Style>>) {
    let mut out = String::new();
    let mut out_styles = Vec::new();
    let mut w = 0;
    for (c, k) in line.chars().zip(styles.iter().copied()) {
        if c == '\t' {
            let n = tabsize - (w % tabsize);
            out.push(whitespace.map_or(' ', |(tab, _)| tab));
            out_styles.push(k);
            for _ in 1..n {
                out.push(' ');
                out_styles.push(k);
            }
            w += n;
        } else if c == ' ' {
            out.push(whitespace.map_or(' ', |(_, space)| space));
            out_styles.push(k);
            w += 1;
        } else {
            out.push(c);
            out_styles.push(k);
            w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
        }
    }
    (out, out_styles)
}

/// Prompt input with whitespace made visible when `whitespacedisplay` is
/// on -- nano runs the answer through the same `display_string` as the
/// edit rows, so `Search: a·b` there too (only status messages are
/// exempt).
fn prompt_input_for_display(editor: &Editor, input: &str) -> String {
    if !editor.options.whitespacedisplay {
        return input.to_string();
    }
    let styles = vec![None; input.chars().count()];
    let tabsize = editor.options.tabsize as usize;
    expand_tabs_with_styles(input, &styles, tabsize, Some(editor.options.whitespace)).0
}

/// Highlight `body` (already-split lines of a unified diff) with the
/// vendored "diff" tree-sitter grammar, one style array per line — mirrors
/// how `render_buffer` highlights the main editor buffer, just without a
/// gutter, tabs, or horizontal scroll to account for. The grammar's query
/// uses Helix's `diff.plus`/`diff.minus` scopes, so the theme decides the
/// colors (green/red in every sane theme, including the built-in one).
/// Returns `None` if the "diff" language somehow isn't registered (never
/// happens in practice; guards against a future registry change more than
/// anything).
fn diff_line_styles(body: &[String], editor: &Editor) -> Option<Vec<Vec<Option<Style>>>> {
    let lang = crate::syntax::find_by_name("diff")?;
    let text = body.join("\n");
    let spans = crate::syntax::highlight(&text, lang);
    let mut line_start = 0usize;
    let mut out = Vec::with_capacity(body.len());
    for line in body {
        out.push(map_spans_to_line(line, line_start, &spans, editor));
        line_start += line.len() + 1; // +1 for the '\n' joiner
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_editor(text: &str) -> Editor {
        let mut ed = Editor::new(crate::options::Options::default(), KeyMap::defaults(false));
        ed.buffers[0] = crate::buffer::Buffer::from_text(text, None);
        ed
    }

    #[test]
    fn yes_no_questions_take_nanos_keys() {
        use crate::app::YesNo;
        let ed = test_editor("");
        let k = |c, m| ask_user_choice(&ed, KeyEvent::new(KeyCode::Char(c), m));
        assert_eq!(k('y', KeyModifiers::NONE), Some(YesNo::Yes));
        assert_eq!(k('N', KeyModifiers::SHIFT), Some(YesNo::No));
        assert_eq!(k('c', KeyModifiers::CONTROL), Some(YesNo::Cancel));
        assert_eq!(k('y', KeyModifiers::CONTROL), Some(YesNo::Yes));
        assert_eq!(k('n', KeyModifiers::CONTROL), Some(YesNo::No));
        assert_eq!(k('q', KeyModifiers::CONTROL), Some(YesNo::No));
        // A plain "c" isn't Cancel in nano; it's ignored.
        assert_eq!(k('c', KeyModifiers::NONE), None);
        assert_eq!(
            ask_user_choice(&ed, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(YesNo::Cancel)
        );
    }

    #[test]
    fn yes_no_questions_render_without_a_colon() {
        let mut ed = test_editor("x");
        ed.screen_cols = 40;
        ed.buf_mut().modified = true;
        ed.execute(Action::Exit);
        let mut out = Vec::new();
        render_status_line(&ed, &mut out, 0, 40).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("Save modified buffer? "), "{text}");
        assert!(!text.contains("? :"), "{text}");
    }

    #[test]
    fn shift_right_sets_a_soft_mark_and_extends_the_selection() {
        let mut ed = test_editor("hello world");
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(ed.buf().mark, Some(Pos::new(0, 0)));
        assert!(ed.buf().softmark);
        assert_eq!(ed.buf().cursor, Pos::new(0, 1));

        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(
            ed.buf().mark,
            Some(Pos::new(0, 0)),
            "the mark's anchor shouldn't move on further shift-movement"
        );
        assert_eq!(ed.buf().cursor, Pos::new(0, 2));
    }

    #[test]
    fn plain_movement_after_shift_selection_drops_the_soft_mark() {
        let mut ed = test_editor("hello world");
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert!(ed.buf().mark.is_some());

        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            ed.buf().mark,
            None,
            "a plain movement key should collapse the selection"
        );
        assert!(!ed.buf().softmark);
        assert_eq!(ed.buf().cursor, Pos::new(0, 2), "the cursor still moves");
    }

    #[test]
    fn hard_mark_survives_plain_movement() {
        let mut ed = test_editor("hello world");
        ed.execute(Action::Mark);
        assert!(ed.buf().mark.is_some());
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(
            ed.buf().mark.is_some(),
            "^^/M-A sets a hard mark, unaffected by plain movement"
        );
    }

    #[test]
    fn typing_a_character_drops_a_soft_mark() {
        let mut ed = test_editor("hello world");
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert!(ed.buf().mark.is_some());
        handle_editing_key(
            &mut ed,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(ed.buf().mark.is_none());
        assert_eq!(ed.buf().to_string(), "hxello world");
    }

    #[test]
    fn view_mode_blocks_plain_character_insertion() {
        let mut ed = test_editor("hello");
        ed.options.view = true;
        handle_editing_key(
            &mut ed,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(ed.buf().to_string(), "hello");
        assert_eq!(ed.status.as_deref(), Some("Key is invalid in view mode"));
    }

    #[test]
    fn write_selection_to_file_writes_only_the_marked_text() {
        let path = std::env::temp_dir().join("tico_test_write_selection.txt");
        std::fs::remove_file(&path).ok();
        let mut ed = test_editor("hello\nworld\nagain\n");
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(2, 0); // selects the first two lines whole
        let prompt = Prompt {
            kind: PromptKind::WriteOut {
                flow: crate::app::WriteFlow {
                    exiting: false,
                    withprompt: true,
                    maychange: true,
                },
            },
            menu: Menu::WriteOut,
            label: "Write Selection to File".to_string(),
            input: path.to_str().unwrap().to_string(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        };
        submit_prompt(&mut ed, prompt);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\nworld\n");
        assert!(
            ed.buf().mark.is_some(),
            "writing the selection shouldn't clear it (confirmed against the installed nano)"
        );
        assert!(
            ed.buf().path.is_none(),
            "the buffer's own path/state shouldn't change from a selection-only write"
        );
        std::fs::remove_file(&path).ok();
    }

    fn insert_prompt(new_buffer: bool, input: &str) -> Prompt {
        Prompt {
            kind: PromptKind::InsertFile {
                new_buffer,
                execute: false,
            },
            menu: Menu::Insert,
            label: crate::app::insert_prompt_label(new_buffer, false, false),
            input: input.to_string(),
            cursor: input.chars().count(),
            history_pos: None,
            saved_input: None,
        }
    }

    #[test]
    fn insert_file_reads_content_into_current_buffer_at_cursor() {
        let path = std::env::temp_dir().join("tico_test_insert_at_cursor.txt");
        std::fs::write(&path, "INSERTED\n").unwrap();
        let mut ed = test_editor("hello\nworld\n");
        ed.buf_mut().cursor = crate::buffer::Pos::new(1, 0); // start of "world"
        submit_prompt(&mut ed, insert_prompt(false, path.to_str().unwrap()));
        assert_eq!(ed.buf().to_string(), "hello\nINSERTED\nworld\n");
        assert_eq!(ed.buffers.len(), 1, "should not have opened a new buffer");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn common_prefix_of_strings() {
        assert_eq!(common_prefix("foobar", "foobaz"), "fooba");
        assert_eq!(common_prefix("foo", "bar"), "");
        assert_eq!(common_prefix("foo", "foo"), "foo");
        assert_eq!(common_prefix("foo", "foobar"), "foo");
    }

    fn tab_complete_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tab_completion_single_match_completes_fully() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_single");
        std::fs::write(dir.join("readme.txt"), "").unwrap();
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/rea", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt, None);
        assert_eq!(prompt.input, format!("{}/readme.txt", dir.display()));
        assert_eq!(prompt.cursor, prompt.input.chars().count());
        assert!(ed.file_completions.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_directory_match_appends_slash() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_dir");
        std::fs::create_dir(dir.join("subdir")).unwrap();
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/sub", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt, None);
        assert_eq!(prompt.input, format!("{}/subdir/", dir.display()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_multiple_matches_completes_common_prefix_and_lists() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_multi");
        std::fs::write(dir.join("foo_alpha.txt"), "").unwrap();
        std::fs::write(dir.join("foo_beta.txt"), "").unwrap();
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/foo_", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt, None);
        assert_eq!(prompt.input, format!("{}/foo_", dir.display()));
        let matches = ed
            .file_completions
            .expect("should list the ambiguous matches");
        let mut sorted = matches.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["foo_alpha.txt", "foo_beta.txt"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_no_matches_leaves_input_unchanged() {
        let dir = tab_complete_test_dir("tico_test_tabcomplete_none");
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, &format!("{}/nope", dir.display()));
        apply_filename_completion(&mut ed, &mut prompt, None);
        assert_eq!(prompt.input, format!("{}/nope", dir.display()));
        assert!(ed.file_completions.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tab_completion_not_offered_in_execute_command_mode() {
        // Matches nano's MINSERTFILE-only gate: Execute Command (^T)
        // doesn't get filename completion.
        let dir = tab_complete_test_dir("tico_test_tabcomplete_execute");
        std::fs::write(dir.join("readme.txt"), "").unwrap();
        let mut ed = test_editor("x");
        let original_input = format!("{}/rea", dir.display());
        let prompt = Prompt {
            kind: PromptKind::InsertFile {
                new_buffer: false,
                execute: true,
            },
            menu: Menu::Execute,
            label: "Command to execute".to_string(),
            input: original_input.clone(),
            cursor: original_input.chars().count(),
            history_pos: None,
            saved_input: None,
        };
        handle_prompt_key(
            &mut ed,
            prompt,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        // The Tab key normalizes to Ctrl('I'), which has no binding in
        // Menu::Execute, so the prompt is left completely unchanged.
        if let Mode::Prompt(p) = &ed.mode {
            assert_eq!(p.input, original_input);
        } else {
            panic!("expected prompt to still be open");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn username_completion_matches_filters_by_prefix_and_sorts() {
        let users: Vec<String> = ["bob", "alice", "alicia"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            username_completion_matches(&users, "ali"),
            vec!["~alice", "~alicia"]
        );
        assert_eq!(username_completion_matches(&users, "bob"), vec!["~bob"]);
        assert!(username_completion_matches(&users, "nope").is_empty());
        // An empty fragment (a bare "~") matches everyone.
        assert_eq!(
            username_completion_matches(&users, ""),
            vec!["~alice", "~alicia", "~bob"]
        );
    }

    #[test]
    fn apply_username_completion_no_match_leaves_input_unchanged() {
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "~tico_test_no_such_user_xyz");
        let morsel = prompt.input.clone();
        apply_username_completion(&mut ed, &mut prompt, &morsel);
        assert_eq!(prompt.input, "~tico_test_no_such_user_xyz");
        assert!(ed.file_completions.is_none());
    }

    #[test]
    fn apply_username_completion_current_user_stays_stable_when_already_complete() {
        // Portable against whatever /etc/passwd actually contains: typing
        // the current user's full name is already the longest common
        // prefix of every matching entry (itself, and anything else that
        // happens to start with the same string), so completion must
        // leave it exactly as-is regardless of the environment.
        let Ok(user) = std::env::var("USER") else {
            return; // not set in this environment; skip rather than fail
        };
        let mut ed = test_editor("x");
        let fragment = format!("~{user}");
        let mut prompt = insert_prompt(false, &fragment);
        apply_username_completion(&mut ed, &mut prompt, &fragment);
        assert_eq!(prompt.input, fragment);
    }

    #[test]
    fn apply_username_completion_lists_real_matches_when_ambiguous() {
        // Find two real usernames on this system sharing a nonempty common
        // prefix, to exercise the >1-match listing path against the
        // actual system database rather than only the pure matcher above.
        let users = crate::fileio::list_usernames();
        let common = users.iter().enumerate().find_map(|(i, u)| {
            users[i + 1..]
                .iter()
                .map(|v| common_prefix(u, v))
                .find(|c| !c.is_empty())
        });
        let Some(common) = common else {
            return; // no ambiguous pair of usernames on this system; skip
        };
        let mut ed = test_editor("x");
        let fragment = format!("~{common}");
        let mut prompt = insert_prompt(false, &fragment);
        apply_username_completion(&mut ed, &mut prompt, &fragment);
        let matches = ed
            .file_completions
            .expect("an ambiguous fragment should list its matches");
        assert!(matches.len() > 1);
        assert!(matches.iter().all(|m| m.starts_with(&fragment)));
    }

    #[test]
    fn tab_completion_tilde_fragment_with_slash_is_not_username_completion() {
        // "~/..." contains a slash, so it must go through plain filename
        // completion (against the real home directory) rather than
        // username completion, even though it starts with `~`.
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "~/tico_test_no_such_dir_xyz123/rea");
        apply_filename_completion(&mut ed, &mut prompt, None);
        assert_eq!(prompt.input, "~/tico_test_no_such_dir_xyz123/rea");
    }

    #[test]
    fn insert_file_new_buffer_opens_a_separate_buffer() {
        let path = std::env::temp_dir().join("tico_test_insert_new_buffer.txt");
        std::fs::write(&path, "SEPARATE CONTENT\n").unwrap();
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.current, 1);
        assert_eq!(ed.buf().to_string(), "SEPARATE CONTENT\n");
        assert_eq!(
            ed.buffers[0].to_string(),
            "original\n",
            "original buffer untouched"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn insert_file_new_buffer_under_minibar_suppresses_the_blurb_but_notes_the_linecount() {
        // Loading into a *new* buffer is never an undoable insert into the
        // current one, so nano suppresses the ordinary "Read N lines"
        // blurb under minibar too, showing only the persistent note --
        // confirmed against the installed nano's own escape-code output.
        let path = std::env::temp_dir().join("tico_test_insert_new_buffer_minibar.txt");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let mut ed = test_editor("original\n");
        ed.options.minibar = true;
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.status, None, "blurb suppressed under minibar");
        assert_eq!(ed.minibar_note.as_deref(), Some("(3 lines)"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn insert_file_into_current_buffer_under_minibar_still_shows_the_blurb() {
        // Unlike loading into a new buffer, an interactive insert into the
        // *current* buffer is undoable, so nano does NOT suppress its
        // ordinary blurb even under minibar (only startup loads and
        // new-buffer loads are silenced) -- confirmed against the
        // installed nano's own escape-code output.
        let path = std::env::temp_dir().join("tico_test_insert_current_minibar.txt");
        std::fs::write(&path, "x\n").unwrap();
        let mut ed = test_editor("original\n");
        ed.options.minibar = true;
        submit_prompt(&mut ed, insert_prompt(false, path.to_str().unwrap()));
        assert!(
            ed.status.is_some(),
            "blurb still shown for an in-place insert"
        );
        assert_eq!(ed.minibar_note.as_deref(), Some("(2 lines)"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_out_under_minibar_suppresses_the_blurb_but_notes_the_linecount() {
        let path = std::env::temp_dir().join("tico_test_write_out_minibar.txt");
        std::fs::remove_file(&path).ok();
        let mut ed = test_editor("one\ntwo\n");
        ed.options.minibar = true;
        let prompt = Prompt {
            kind: PromptKind::WriteOut {
                flow: crate::app::WriteFlow {
                    exiting: false,
                    withprompt: true,
                    maychange: true,
                },
            },
            menu: Menu::WriteOut,
            label: "Write Out".to_string(),
            input: path.to_str().unwrap().to_string(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        };
        submit_prompt(&mut ed, prompt);
        assert_eq!(ed.status, None, "blurb suppressed under minibar");
        assert_eq!(ed.minibar_note.as_deref(), Some("(2 lines)"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn insert_file_new_buffer_gets_syntax_highlighting_like_argv() {
        let path = std::env::temp_dir().join("tico_test_insert_new_buffer.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buf().language.map(|l| l.name), Some("rust"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn insert_file_nonexistent_path_new_buffer_gets_syntax_highlighting_too() {
        let path = std::env::temp_dir().join("tico_test_insert_does_not_exist.py");
        std::fs::remove_file(&path).ok();
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buf().language.map(|l| l.name), Some("python"));
    }

    #[test]
    fn insert_file_empty_input_new_buffer_opens_blank_buffer() {
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, ""));
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.current, 1);
        assert_eq!(ed.buf().to_string(), "");
    }

    #[test]
    fn insert_file_empty_input_without_new_buffer_cancels() {
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(false, ""));
        assert_eq!(ed.buffers.len(), 1);
        assert_eq!(ed.buf().to_string(), "original\n");
        assert_eq!(ed.status.as_deref(), Some("Cancelled"));
    }

    #[test]
    fn insert_file_nonexistent_path_new_buffer_gives_blank_named_buffer() {
        let path = std::env::temp_dir().join("tico_test_insert_does_not_exist.txt");
        std::fs::remove_file(&path).ok(); // just in case a prior run left it
        let mut ed = test_editor("original\n");
        submit_prompt(&mut ed, insert_prompt(true, path.to_str().unwrap()));
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.buf().to_string(), "");
        assert_eq!(ed.buf().path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn flip_new_buffer_toggles_label_and_state() {
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipNewBuffer
        ));
        assert_eq!(
            prompt.kind,
            PromptKind::InsertFile {
                new_buffer: true,
                execute: false
            }
        );
        assert!(prompt.label.contains("new buffer"));
    }

    /// A scratch directory for the browser tests: `sub/` (holding
    /// `inner.txt`), `alpha.txt` and `beta.txt`.
    fn browser_fixture(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tico-browser-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/inner.txt"), "inner\n").unwrap();
        std::fs::write(dir.join("alpha.txt"), "alpha\n").unwrap();
        std::fs::write(dir.join("beta.txt"), "beta\n").unwrap();
        crate::browser::full_dir_path(&dir).unwrap()
    }

    fn press(ed: &mut Editor, code: KeyCode, modifiers: KeyModifiers) {
        handle_key(ed, KeyEvent::new(code, modifiers));
    }

    fn type_text(ed: &mut Editor, text: &str) {
        for c in text.chars() {
            press(ed, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn browser_names(ed: &Editor) -> Vec<String> {
        let session = ed.browser.as_ref().expect("browser open");
        session
            .list
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    fn selected_name(ed: &Editor) -> String {
        let session = ed.browser.as_ref().expect("browser open");
        session.list.selected_entry().unwrap().name.clone()
    }

    fn open_test_browser(dir: &std::path::Path) -> Editor {
        let mut ed = test_editor("x");
        ed.screen_cols = 80;
        ed.screen_rows = 24;
        let input = format!("{}/", dir.display());
        ed.mode = Mode::Prompt(insert_prompt(false, &input));
        press(&mut ed, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert!(matches!(ed.mode, Mode::Browser));
        ed
    }

    #[test]
    fn browser_lists_the_prompts_directory_and_inserts_the_chosen_file() {
        let dir = browser_fixture("insert");
        let mut ed = open_test_browser(&dir);
        assert_eq!(browser_names(&ed), ["..", "sub", "alpha.txt", "beta.txt"]);
        assert_eq!(selected_name(&ed), "..");
        // Right three times, then Enter: beta.txt is read into the buffer.
        press(&mut ed, KeyCode::Right, KeyModifiers::NONE);
        press(&mut ed, KeyCode::Right, KeyModifiers::NONE);
        press(&mut ed, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(selected_name(&ed), "beta.txt");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(ed.mode, Mode::Editing));
        assert!(ed.browser.is_none());
        assert_eq!(ed.buf().to_string(), "beta\nx");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn browser_enters_directories_and_reselects_on_the_way_back_up() {
        let dir = browser_fixture("updown");
        let mut ed = open_test_browser(&dir);
        press(&mut ed, KeyCode::Right, KeyModifiers::NONE);
        // `s` is Enter in the browser, as in nano.
        type_text(&mut ed, "s");
        assert_eq!(ed.browser.as_ref().unwrap().list.dir, dir.join("sub"));
        assert_eq!(browser_names(&ed), ["..", "inner.txt"]);
        assert_eq!(selected_name(&ed), "..");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ed.browser.as_ref().unwrap().list.dir, dir);
        assert_eq!(selected_name(&ed), "sub");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn leaving_the_browser_restores_the_prompt_untouched() {
        let dir = browser_fixture("leave");
        for (code, modifiers) in [
            (KeyCode::Char('x'), KeyModifiers::CONTROL),
            (KeyCode::Char('t'), KeyModifiers::CONTROL),
            (KeyCode::Char('q'), KeyModifiers::NONE),
            (KeyCode::Esc, KeyModifiers::NONE),
        ] {
            let mut ed = open_test_browser(&dir);
            press(&mut ed, code, modifiers);
            assert!(ed.browser.is_none(), "{code:?}");
            let Mode::Prompt(prompt) = &ed.mode else {
                panic!("{code:?} should return to the prompt");
            };
            assert_eq!(prompt.menu, Menu::Insert);
            assert_eq!(prompt.input, format!("{}/", dir.display()));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn browser_search_selects_matches_and_shares_the_search_string() {
        let dir = browser_fixture("search");
        let mut ed = open_test_browser(&dir);
        press(&mut ed, KeyCode::Char('w'), KeyModifiers::CONTROL);
        let Mode::Prompt(prompt) = &ed.mode else {
            panic!("search prompt expected");
        };
        assert_eq!(prompt.menu, Menu::WhereIsFile);
        assert_eq!(prompt.label, "Search");
        type_text(&mut ed, "TXT");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(ed.mode, Mode::Browser));
        assert_eq!(selected_name(&ed), "alpha.txt");
        assert_eq!(ed.search.last_pattern.as_deref(), Some("TXT"));
        // `n` repeats it; past the end it wraps.
        type_text(&mut ed, "n");
        assert_eq!(selected_name(&ed), "beta.txt");
        assert_eq!(ed.status, None);
        type_text(&mut ed, "n");
        assert_eq!(selected_name(&ed), "alpha.txt");
        assert_eq!(ed.status.as_deref(), Some("Search Wrapped"));
        // The previous string is offered in the label, and Enter reuses it.
        type_text(&mut ed, "w");
        let Mode::Prompt(prompt) = &ed.mode else {
            panic!("search prompt expected");
        };
        assert_eq!(prompt.label, "Search [TXT]");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(selected_name(&ed), "beta.txt");
        // Cancelling goes back to the browser.
        press(&mut ed, KeyCode::Char('b'), KeyModifiers::CONTROL);
        press(&mut ed, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(ed.mode, Mode::Browser));
        assert_eq!(ed.status.as_deref(), Some("Cancelled"));
        type_text(&mut ed, "wzzz");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ed.status.as_deref(), Some("\"zzz\" not found"));
        assert_eq!(selected_name(&ed), "beta.txt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn go_to_directory_enters_or_complains() {
        let dir = browser_fixture("gotodir");
        let mut ed = open_test_browser(&dir);
        press(&mut ed, KeyCode::Char('7'), KeyModifiers::CONTROL); // ^_
        let Mode::Prompt(prompt) = &ed.mode else {
            panic!("go-to-dir prompt expected");
        };
        assert_eq!(prompt.menu, Menu::GotoDir);
        // Tab completes relative to the browsed directory, directories only.
        type_text(&mut ed, "s");
        press(&mut ed, KeyCode::Tab, KeyModifiers::NONE);
        let Mode::Prompt(prompt) = &ed.mode else {
            panic!("go-to-dir prompt expected");
        };
        assert_eq!(prompt.input, "sub/");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(ed.mode, Mode::Browser));
        assert_eq!(ed.browser.as_ref().unwrap().list.dir, dir.join("sub"));

        type_text(&mut ed, "g");
        type_text(&mut ed, "../nowhere");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(ed.mode, Mode::Browser));
        assert_eq!(ed.browser.as_ref().unwrap().list.dir, dir.join("sub"));
        assert_eq!(
            ed.status.as_deref(),
            Some("Cannot open directory: No such file or directory")
        );
        assert_eq!(ed.status_level, crate::app::StatusLevel::Alert);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn browsing_from_write_out_asks_before_overwriting_the_chosen_file() {
        let dir = browser_fixture("writeout");
        let mut ed = test_editor("new text");
        ed.screen_cols = 80;
        ed.screen_rows = 24;
        let flow = crate::app::WriteFlow {
            exiting: false,
            withprompt: true,
            maychange: true,
        };
        ed.mode = Mode::Prompt(Prompt {
            kind: PromptKind::WriteOut { flow },
            menu: Menu::WriteOut,
            label: "File Name to Write".to_string(),
            input: dir.join("whatever").to_string_lossy().into_owned(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        });
        press(&mut ed, KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert!(
            matches!(ed.mode, Mode::Browser),
            "a missing name browses its directory"
        );
        press(&mut ed, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(selected_name(&ed), "beta.txt");
        press(&mut ed, KeyCode::Enter, KeyModifiers::NONE);
        let Mode::Prompt(prompt) = &ed.mode else {
            panic!("overwrite question expected");
        };
        assert!(
            matches!(
                prompt.kind,
                PromptKind::WriteConfirm {
                    question: crate::app::WriteQuestion::Overwrite,
                    ..
                }
            ),
            "{:?}",
            prompt.kind
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn browser_screen_shows_the_directory_listing_and_its_bar() {
        let dir = browser_fixture("render");
        let ed = open_test_browser(&dir);
        let mut out = Vec::new();
        render_browser_screen(&ed, ed.browser.as_ref().unwrap(), &mut out, false).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("DIR: "), "{text}");
        assert!(text.contains("(parent dir)"), "{text}");
        assert!(text.contains("alpha.txt"), "{text}");
        assert!(text.contains("Go To Dir"), "{text}");
        assert!(text.contains("Where Was"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unimplemented_write_out_actions_report_plainly_and_close_the_prompt() {
        for (action, expected) in [
            (Action::Append, "Append: not yet implemented"),
            (Action::Prepend, "Prepend: not yet implemented"),
            (Action::Backup, "Backup File: not yet implemented"),
        ] {
            let mut ed = test_editor("x");
            let mut prompt = Prompt {
                kind: PromptKind::WriteOut {
                    flow: crate::app::WriteFlow {
                        exiting: false,
                        withprompt: true,
                        maychange: true,
                    },
                },
                menu: Menu::WriteOut,
                label: "Write Out".to_string(),
                input: String::new(),
                cursor: 0,
                history_pos: None,
                saved_input: None,
            };
            assert!(
                apply_prompt_action(&mut ed, &mut prompt, action),
                "{action:?}"
            );
            assert!(
                matches!(ed.mode, Mode::Editing),
                "{action:?} should close the prompt"
            );
            assert_eq!(ed.status.as_deref(), Some(expected), "{action:?}");
        }
    }

    #[test]
    fn flip_execute_toggles_mode_menu_and_label() {
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipExecute
        ));
        assert_eq!(
            prompt.kind,
            PromptKind::InsertFile {
                new_buffer: false,
                execute: true
            }
        );
        assert_eq!(prompt.menu, Menu::Execute);
        assert_eq!(prompt.label, "Command to execute");

        // Flipping back restores the Insert-File prompt.
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipExecute
        ));
        assert_eq!(
            prompt.kind,
            PromptKind::InsertFile {
                new_buffer: false,
                execute: false
            }
        );
        assert_eq!(prompt.menu, Menu::Insert);
        assert_eq!(prompt.label, "File to insert [from ./]");
    }

    #[test]
    fn execute_command_inserts_output_at_cursor() {
        let mut ed = test_editor("ab");
        ed.buf_mut().cursor = Pos::new(0, 1);
        submit_execute_command(&mut ed, "echo -n hello", false);
        assert_eq!(ed.buf().to_string(), "ahellob");
        assert!(ed.buf().modified);
        assert_eq!(ed.history.execute, vec!["echo -n hello".to_string()]);
    }

    #[test]
    fn execute_command_new_buffer_opens_a_separate_buffer() {
        let mut ed = test_editor("original");
        submit_execute_command(&mut ed, "echo -n hi", true);
        assert_eq!(ed.buffers.len(), 2);
        assert_eq!(ed.current, 1);
        assert_eq!(ed.buf().to_string(), "hi");
        assert_eq!(ed.buffers[0].to_string(), "original");
    }

    #[test]
    fn replace_whole_word_replaces_only_whole_word_matches() {
        let mut buf = crate::buffer::Buffer::from_text("teh cat sat on teh mat, nateh", None);
        assert!(replace_whole_word(&mut buf, "teh", "the"));
        assert_eq!(
            buf.to_string(),
            "the cat sat on the mat, nateh",
            "the trailing 'nateh' isn't a whole-word match and must be left alone"
        );
    }

    #[test]
    fn find_whole_word_finds_first_occurrence_only() {
        let buf = crate::buffer::Buffer::from_text("one\nteh two\nteh three", None);
        assert_eq!(find_whole_word(&buf, "teh"), Some(Pos::new(1, 0)));
        assert_eq!(find_whole_word(&buf, "missing"), None);
    }

    #[test]
    fn parse_linter_output_handles_colon_and_comma_column_forms() {
        let out = "main.rs:3:5: unused variable\nmain.rs:9: missing semicolon\nmain.rs:12,2: bad indent\nnot a lint line\n";
        let messages = parse_linter_output(out);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].line, 3);
        assert_eq!(messages[0].col, 5);
        assert_eq!(messages[0].msg, "unused variable");
        assert_eq!(messages[1].line, 9);
        assert_eq!(messages[1].col, 1);
        assert_eq!(messages[1].msg, "missing semicolon");
        assert_eq!(messages[2].line, 12);
        assert_eq!(messages[2].col, 2);
        assert_eq!(messages[2].msg, "bad indent");
    }

    #[test]
    fn format_byte_size_uses_the_largest_exact_unit() {
        assert_eq!(format_byte_size(4 * 1024 * 1024), "4MB");
        assert_eq!(format_byte_size(2 * 1024 * 1024 * 1024), "2GB");
        assert_eq!(format_byte_size(4096), "4KB");
        assert_eq!(format_byte_size(4097), "4097 bytes");
        assert_eq!(format_byte_size(0), "0 bytes");
    }

    #[test]
    fn heredoc_body_uses_the_injected_languages_theme() {
        // Perl buffer with an SQL heredoc; Perl keeps the default theme
        // while SQL gets an override whose keyword color is unmistakable.
        let mut ed = test_editor("print <<SQL;\nSELECT 1\nSQL\n");
        ed.buf_mut().language = crate::syntax::find_by_name("perl");
        let dir = std::env::temp_dir().join(format!("tico-ui-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("loud.toml"), "\"keyword\" = \"#123456\"\n").unwrap();
        let mut w = Vec::new();
        let loud_theme = crate::theme::Loader::new(vec![dir.clone()])
            .load("loud", &mut w)
            .unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert!(w.is_empty(), "{w:?}");
        ed.language_themes.insert("sql".to_string(), loud_theme);

        let spans = ed
            .buf()
            .highlighted_spans_cached(ed.buf().language.unwrap());
        let text = ed.buf().to_string();
        let line1_start = ed.buf().line_start_byte(1);
        let styles = map_spans_to_line(&ed.buf().line(1), line1_start, &spans, &ed);
        let loud = Color::Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        };
        assert_eq!(
            styles[0].and_then(|s| s.fg),
            Some(loud),
            "SELECT in the SQL heredoc must use sql's theme: {styles:?} (text {text:?})"
        );
        // And Perl's own `print` on line 0 does not.
        let styles0 = map_spans_to_line(&ed.buf().line(0), 0, &spans, &ed);
        assert_ne!(styles0[0].and_then(|s| s.fg), Some(loud), "{styles0:?}");
    }

    #[test]
    fn warns_once_when_buffer_exceeds_the_size_limit() {
        let mut ed = test_editor(&"x".repeat(100));
        ed.options.max_syntax_highlight_bytes = 10;
        let lang = crate::syntax::detect_with_override(None, "", Some("rust")).unwrap();
        ed.buf_mut().language = Some(lang);

        maybe_warn_highlighting_disabled_for_size(&mut ed);
        assert_eq!(
            ed.status.as_deref(),
            Some("Syntax highlighting disabled: file is larger than 10 bytes")
        );
        assert!(ed.buf().highlighting_size_warning_shown);

        // Doesn't repeat on a later check.
        ed.status = None;
        maybe_warn_highlighting_disabled_for_size(&mut ed);
        assert_eq!(ed.status, None);
    }

    #[test]
    fn no_size_warning_under_the_limit_or_without_a_detected_language() {
        let mut ed = test_editor("small");
        ed.options.max_syntax_highlight_bytes = 1_000_000;
        let lang = crate::syntax::detect_with_override(None, "", Some("rust")).unwrap();
        ed.buf_mut().language = Some(lang);
        maybe_warn_highlighting_disabled_for_size(&mut ed);
        assert_eq!(ed.status, None, "well under the limit: no warning");

        let mut ed2 = test_editor(&"x".repeat(100));
        ed2.options.max_syntax_highlight_bytes = 10;
        maybe_warn_highlighting_disabled_for_size(&mut ed2);
        assert_eq!(
            ed2.status, None,
            "over the limit but no detected language: nothing to warn about"
        );
    }

    #[test]
    fn run_formatter_reports_when_none_configured() {
        let mut ed = test_editor("x");
        run_formatter(&mut ed);
        assert_eq!(
            ed.status.as_deref(),
            Some("No formatter is defined for this type of file")
        );
    }

    #[test]
    fn run_linter_reports_when_none_configured() {
        let mut ed = test_editor("x");
        run_linter(&mut ed);
        assert_eq!(
            ed.status.as_deref(),
            Some("No linter is defined for this type of file")
        );
    }

    #[test]
    fn shortcut_bar_reflects_modern_bindings() {
        let default_km = KeyMap::defaults(false);
        let modern_km = KeyMap::defaults(true);
        assert_eq!(key_label_for(&default_km, Menu::Main, Action::Help), "^G");
        assert_eq!(key_label_for(&modern_km, Menu::Main, Action::Help), "^H");
        assert_eq!(key_label_for(&default_km, Menu::Main, Action::Exit), "^X");
        assert_eq!(key_label_for(&modern_km, Menu::Main, Action::Exit), "^Q");
    }

    #[test]
    fn shortcut_bar_reflects_user_rebind() {
        let mut km = KeyMap::defaults(false);
        km.unbind(Menu::Main, crate::keymap::Key::Ctrl('X'));
        km.bind(
            Menu::Main,
            crate::keymap::Key::Ctrl('Q'),
            Binding::Action(Action::Exit),
        );
        assert_eq!(key_label_for(&km, Menu::Main, Action::Exit), "^Q");
    }

    #[test]
    fn key_label_for_unbound_action_is_empty() {
        let mut km = KeyMap::defaults(false);
        // Help has two default keys (^G and F1); unbind both.
        km.unbind(Menu::Main, crate::keymap::Key::Ctrl('G'));
        km.unbind(Menu::Main, crate::keymap::Key::F(1));
        assert_eq!(key_label_for(&km, Menu::Main, Action::Help), "");
    }

    #[test]
    fn main_shortcut_priority_resolves_to_nonempty_labels_by_default() {
        let km = KeyMap::defaults(false);
        let entries = resolve_shortcuts(&km, Menu::Main, SHORTCUT_PRIORITY);
        assert_eq!(entries.len(), SHORTCUT_PRIORITY.len());
        for (key, desc) in &entries {
            assert!(!key.is_empty(), "no key resolved for {desc:?}");
        }
    }

    #[test]
    fn shift_tab_unindents_and_keeps_a_soft_mark() {
        // Shift+Down sets a soft mark spanning line 0; a following
        // Shift-Tab (reported by crossterm as BackTab, here deliberately
        // without the SHIFT modifier) unindents that line and, unlike a
        // plain edit, must not drop the soft mark just because the
        // cursor/mark columns shifted -- nano's `shift_held` exemption.
        let mut ed = test_editor("\tone\n\ttwo\n");
        ed.buf_mut().cursor = Pos::new(0, 3);
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        assert_eq!(ed.buf().mark, Some(Pos::new(0, 3)));
        assert!(ed.buf().softmark);
        assert_eq!(ed.buf().cursor, Pos::new(1, 3));

        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(ed.buf().line(0), "one");
        assert_eq!(ed.buf().line(1), "two");
        assert_eq!(
            ed.buf().mark,
            Some(Pos::new(0, 2)),
            "soft mark kept, shifted left"
        );
        assert!(ed.buf().softmark);
        assert_eq!(ed.buf().cursor, Pos::new(1, 2));

        // A plain movement afterwards still drops it as usual.
        handle_editing_key(&mut ed, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(ed.buf().mark, None);
    }

    #[test]
    fn dos_and_mac_toggles_at_the_write_out_prompt_flip_label_and_format() {
        use crate::buffer::LineFormat;
        let mut ed = test_editor("x\n");
        let mut prompt = Prompt {
            kind: PromptKind::WriteOut {
                flow: crate::app::WriteFlow {
                    exiting: false,
                    withprompt: true,
                    maychange: true,
                },
            },
            menu: Menu::WriteOut,
            label: ed.writeout_prompt_label(false),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        };
        assert_eq!(prompt.label, "Write to File");
        assert!(
            !apply_prompt_action(&mut ed, &mut prompt, Action::DosFormat),
            "the prompt stays open"
        );
        assert_eq!(ed.buf().format, LineFormat::Dos);
        assert_eq!(prompt.label, "Write to File [DOS Format]");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::MacFormat
        ));
        assert_eq!(ed.buf().format, LineFormat::Mac);
        assert_eq!(prompt.label, "Write to File [Mac Format]");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::MacFormat
        ));
        assert_eq!(
            ed.buf().format,
            LineFormat::Unix,
            "toggling the current format off means Unix"
        );
        assert_eq!(prompt.label, "Write to File");
        assert!(
            !ed.buf().modified,
            "a format flip alone doesn't dirty the buffer"
        );
    }

    #[test]
    fn no_conversion_flips_the_global_option_and_the_prompt_label() {
        let mut ed = test_editor("x");
        let mut prompt = insert_prompt(false, "");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipConvert
        ));
        assert!(
            ed.options.noconvert,
            "nano's flip_convert toggles the global flag"
        );
        assert_eq!(prompt.label, "File to insert unconverted [from ./]");
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipNewBuffer
        ));
        assert_eq!(
            prompt.label,
            "File to read unconverted into new buffer [from ./]"
        );
        assert!(!apply_prompt_action(
            &mut ed,
            &mut prompt,
            Action::FlipConvert
        ));
        assert!(!ed.options.noconvert);
        assert_eq!(prompt.label, "File to read into new buffer [from ./]");
    }

    #[test]
    fn inserting_a_file_converts_it_and_a_fresh_buffer_adopts_its_format() {
        use crate::buffer::LineFormat;
        let path = std::env::temp_dir().join("tico_test_insert_dos.txt");
        std::fs::write(&path, "in\r\n").unwrap();
        let mut ed = test_editor("");
        submit_prompt(&mut ed, insert_prompt(false, path.to_str().unwrap()));
        assert_eq!(ed.buf().to_string(), "in\n");
        assert_eq!(ed.buf().format, LineFormat::Dos);
        assert_eq!(
            ed.status.as_deref(),
            Some("Read 1 line (converted from DOS format)")
        );

        let unix_path = std::env::temp_dir().join("tico_test_insert_unix.txt");
        std::fs::write(&unix_path, "more\n").unwrap();
        submit_prompt(&mut ed, insert_prompt(false, unix_path.to_str().unwrap()));
        assert_eq!(
            ed.buf().format,
            LineFormat::Dos,
            "an existing format sticks"
        );
        assert_eq!(ed.status.as_deref(), Some("Read 1 line"));

        ed.options.noconvert = true;
        submit_prompt(&mut ed, insert_prompt(false, path.to_str().unwrap()));
        assert!(
            ed.buf().to_string().contains("in\r\n"),
            "unconverted bytes are inserted as-is"
        );
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&unix_path).ok();
    }

    #[test]
    fn whitespace_display_substitutes_markers_without_changing_widths() {
        let styles = vec![None; 6];
        let (plain, plain_styles) = expand_tabs_with_styles("a\tb c ", &styles, 8, None);
        assert_eq!(plain, "a       b c ");
        let (marked, marked_styles) =
            expand_tabs_with_styles("a\tb c ", &styles, 8, Some(('\u{bb}', '\u{b7}')));
        assert_eq!(
            marked, "a\u{bb}      b\u{b7}c\u{b7}",
            "as nano 8.7.1 renders it"
        );
        assert_eq!(marked.chars().count(), plain.chars().count());
        assert_eq!(marked_styles.len(), plain_styles.len());

        let (t, _) = expand_tabs_with_styles("\tx", &[None, None], 4, Some(('>', '.')));
        assert_eq!(t, ">   x", "a tab at a stop still fills to the next one");
    }

    #[test]
    fn prompt_input_shows_markers_only_while_whitespace_display_is_on() {
        let mut ed = test_editor("");
        assert_eq!(prompt_input_for_display(&ed, "a b"), "a b");
        ed.options.whitespacedisplay = true;
        assert_eq!(prompt_input_for_display(&ed, "a b"), "a\u{b7}b");
    }

    // Color settings (`set titlecolor` and friends): defaults and fallbacks
    // confirmed against the installed nano 8.6's own escape-code output.

    #[test]
    fn bar_style_falls_back_to_the_given_default_when_unconfigured() {
        let unset = crate::options::ColorPair::default();
        assert!(matches!(
            bar_style(&unset, BarStyle::Reverse),
            BarStyle::Reverse
        ));
        assert!(matches!(
            bar_style(&unset, BarStyle::Plain),
            BarStyle::Plain
        ));
    }

    #[test]
    fn bar_style_uses_the_configured_colors_and_attributes() {
        let cp = crate::options::parse_color_pair("bold,yellow,magenta").unwrap();
        match bar_style(&cp, BarStyle::Reverse) {
            BarStyle::Colored {
                fg,
                bg,
                bold,
                italic,
            } => {
                assert_eq!(fg, Color::DarkYellow);
                assert_eq!(bg, Color::DarkMagenta);
                assert!(bold);
                assert!(!italic);
            }
            _ => panic!("expected a configured color pair to resolve to Colored"),
        }
    }

    #[test]
    fn errorcolor_default_matches_nanos_bold_white_on_red() {
        // nano's own default ERROR_MESSAGE color, captured directly from
        // the installed binary opening a directory as a file.
        let cp = crate::options::Options::default().errorcolor;
        match bar_style(&cp, BarStyle::Reverse) {
            BarStyle::Colored {
                fg,
                bg,
                bold,
                italic,
            } => {
                assert_eq!(fg, Color::Grey);
                assert_eq!(bg, Color::DarkRed);
                assert!(bold);
                assert!(!italic);
            }
            _ => panic!("errorcolor always has fg/bg set, even by default"),
        }
    }

    #[test]
    fn promptcolor_and_minicolor_fall_back_to_titlecolor() {
        let mut ed = test_editor("");
        ed.options.titlecolor = crate::options::parse_color_pair("bold,green,blue").unwrap();
        let title_style = title_bar_style(&ed);
        let prompt_style = bar_style(&ed.options.promptcolor, title_style);
        match prompt_style {
            BarStyle::Colored { fg, bg, bold, .. } => {
                assert_eq!(fg, Color::DarkGreen);
                assert_eq!(bg, Color::DarkBlue);
                assert!(bold);
            }
            _ => panic!("promptcolor should inherit titlecolor when unset"),
        }
    }

    #[test]
    fn numbercolor_defaults_to_reverse_video() {
        let unset = crate::options::ColorPair::default();
        let style = numbercolor_style(&unset);
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
        assert!(style.modifiers.contains(crate::theme::Modifiers::REVERSED));
    }

    #[test]
    fn numbercolor_configured_uses_its_own_colors_not_reverse() {
        let cp = crate::options::parse_color_pair("italic,cyan").unwrap();
        let style = numbercolor_style(&cp);
        assert_eq!(style.fg, Some(Color::DarkCyan));
        assert_eq!(style.bg, None);
        assert!(style.modifiers.contains(crate::theme::Modifiers::ITALIC));
        assert!(!style.modifiers.contains(crate::theme::Modifiers::REVERSED));
    }

    #[test]
    fn functioncolor_defaults_to_plain_not_reverse() {
        let unset = crate::options::ColorPair::default();
        assert!(matches!(
            bar_style(&unset, BarStyle::Plain),
            BarStyle::Plain
        ));
    }

    #[test]
    fn an_extended_256_color_hue_name_like_lime_is_not_rendered_invisibly() {
        // Regression test: `set titlecolor black,lime` used to render as
        // black-on-black, because `lime` failed to parse and left the
        // background unset (-> the terminal's own default, typically
        // black) instead of the lime-green 256-color background nano
        // itself shows.
        let cp = crate::options::parse_color_pair("black,lime").unwrap();
        match bar_style(&cp, BarStyle::Reverse) {
            BarStyle::Colored { fg, bg, .. } => {
                assert_eq!(fg, Color::Black);
                assert_eq!(bg, Color::AnsiValue(148));
                assert_ne!(fg, bg, "must not resolve to the same color");
            }
            _ => panic!("an explicit color pair should never fall back to the default"),
        }
    }

    // `set indicator` (the scrollbar)

    #[test]
    fn scrollbar_thumb_covers_the_whole_bar_when_the_buffer_fits_without_scrolling() {
        assert_eq!(scrollbar_thumb_range(0, 26, 5), (0, 26));
    }

    #[test]
    fn scrollbar_thumb_matches_nano_at_the_top_of_a_100_line_buffer() {
        // 26-row viewport, 100-line buffer, scrolled to the top -- matches
        // the installed nano's own escape-code output exactly (rows 0..6
        // of the viewport highlighted).
        assert_eq!(scrollbar_thumb_range(0, 26, 100), (0, 6));
    }

    #[test]
    fn scrollbar_thumb_matches_nano_at_the_bottom_of_a_100_line_buffer() {
        // Same buffer, scrolled all the way down (top_line = 74) -- matches
        // the installed nano's own escape-code output exactly (rows 19..25).
        assert_eq!(scrollbar_thumb_range(74, 26, 100), (19, 25));
    }

    #[test]
    fn scrollercolor_default_is_plain_not_reverse_but_the_thumb_always_reverses() {
        let unset = crate::options::ColorPair::default();
        let track = scrollbar_cell_style(&unset, false);
        assert_eq!(track.fg, None);
        assert_eq!(track.bg, None);
        assert!(!track.modifiers.contains(crate::theme::Modifiers::REVERSED));

        let thumb = scrollbar_cell_style(&unset, true);
        assert_eq!(thumb.fg, None);
        assert_eq!(thumb.bg, None);
        assert!(thumb.modifiers.contains(crate::theme::Modifiers::REVERSED));
    }

    #[test]
    fn scrollercolor_configured_colors_both_track_and_thumb_reverse_added_to_thumb_only() {
        let cp = crate::options::parse_color_pair("blue,yellow").unwrap();
        let track = scrollbar_cell_style(&cp, false);
        assert_eq!(track.fg, Some(Color::DarkBlue));
        assert_eq!(track.bg, Some(Color::DarkYellow));
        assert!(!track.modifiers.contains(crate::theme::Modifiers::REVERSED));

        let thumb = scrollbar_cell_style(&cp, true);
        assert_eq!(thumb.fg, Some(Color::DarkBlue));
        assert_eq!(thumb.bg, Some(Color::DarkYellow));
        assert!(thumb.modifiers.contains(crate::theme::Modifiers::REVERSED));
    }

    // `set guidestripe` / `stripecolor`

    #[test]
    fn stripe_content_offset_is_visible_within_bounds() {
        assert_eq!(stripe_content_offset(9, 0, 20), Some(9));
        // Scrolled so the stripe's column is now the content area's first
        // visible column.
        assert_eq!(stripe_content_offset(9, 9, 20), Some(0));
    }

    #[test]
    fn stripe_content_offset_hides_once_scrolled_past_on_either_side() {
        // Scrolled past it to the left.
        assert_eq!(stripe_content_offset(9, 10, 20), None);
        // Past the right edge of a narrow content area.
        assert_eq!(stripe_content_offset(25, 0, 20), None);
        // Exactly at the last visible column is still shown.
        assert_eq!(stripe_content_offset(19, 0, 20), Some(19));
    }

    #[test]
    fn stripecolor_defaults_to_reverse_video_like_numbercolor() {
        let unset = crate::options::ColorPair::default();
        let style = stripecolor_style(&unset);
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
        assert!(style.modifiers.contains(crate::theme::Modifiers::REVERSED));
    }

    #[test]
    fn stripecolor_configured_uses_its_own_colors_not_reverse() {
        let cp = crate::options::parse_color_pair("blue,yellow").unwrap();
        let style = stripecolor_style(&cp);
        assert_eq!(style.fg, Some(Color::DarkBlue));
        assert_eq!(style.bg, Some(Color::DarkYellow));
        assert!(!style.modifiers.contains(crate::theme::Modifiers::REVERSED));
    }

    #[test]
    fn guidestripe_recolors_the_configured_column_in_reverse_by_default() {
        // Confirmed against the installed nano's own escape-code output:
        // column 10 recolors the character already there, in plain
        // reverse video, when stripecolor is unset.
        let mut ed = test_editor("xxxxxxxxxxxxxxxxxxxx\n");
        ed.options.guidestripe = Some(10);
        ed.screen_cols = 80;
        let mut out = Vec::new();
        render_buffer(&ed, &mut out, 0, 1).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("\x1b[7mx\x1b[0m"),
            "expected a lone reverse-video 'x' at the stripe column: {text:?}"
        );
    }

    #[test]
    fn guidestripe_paints_a_space_past_a_short_lines_own_text() {
        let mut ed = test_editor("short\n");
        ed.options.guidestripe = Some(10);
        ed.screen_cols = 80;
        let mut out = Vec::new();
        render_buffer(&ed, &mut out, 0, 1).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("\x1b[7m \x1b[0m"),
            "expected a reverse-video space past the end of the line: {text:?}"
        );
    }

    #[test]
    fn selection_takes_priority_over_the_guidestripe_at_the_same_column() {
        let mut ed = test_editor("xxxxxxxxxxxxxxxxxxxx\n");
        ed.options.guidestripe = Some(10);
        ed.screen_cols = 80;
        ed.buf_mut().mark = Some(Pos::new(0, 0));
        ed.buf_mut().cursor = Pos::new(0, 20);
        let mut out = Vec::new();
        render_buffer(&ed, &mut out, 0, 1).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(
            !text.contains("\x1b[7mx\x1b[0m"),
            "the whole line is selected, so no lone reverse 'x' should appear: {text:?}"
        );
    }

    // `set mouse`

    fn mev(kind: MouseEventKind, row: u16, column: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_events_are_ignored_when_the_option_is_off() {
        let mut ed = test_editor("hello\n");
        ed.options.mouse = false;
        handle_mouse(&mut ed, mev(MouseEventKind::Down(MouseButton::Left), 1, 3));
        assert_eq!(ed.buf().cursor, Pos::new(0, 0), "click had no effect");
    }

    #[test]
    fn mouse_click_in_the_buffer_places_the_cursor() {
        let mut ed = test_editor("line one\nline two\nline three\n");
        ed.options.mouse = true;
        // Row 0 is the title bar; row 1 + n is buffer line n.
        handle_mouse(&mut ed, mev(MouseEventKind::Down(MouseButton::Left), 3, 3));
        assert_eq!(ed.buf().cursor, Pos::new(2, 3));
    }

    #[test]
    fn mouse_click_at_the_cursors_own_position_toggles_the_mark() {
        // Matches nano's own process_click: not click-timing based, just
        // "the click didn't move the cursor".
        let mut ed = test_editor("hello world\n");
        ed.options.mouse = true;
        ed.buf_mut().cursor = Pos::new(0, 3);
        assert!(ed.buf().mark.is_none());

        handle_click(&mut ed, 1, 3);
        assert_eq!(ed.buf().cursor, Pos::new(0, 3), "cursor shouldn't move");
        assert_eq!(
            ed.buf().mark,
            Some(Pos::new(0, 3)),
            "click-in-place toggles the mark on"
        );

        handle_click(&mut ed, 1, 3);
        assert!(
            ed.buf().mark.is_none(),
            "clicking again toggles it back off"
        );
    }

    #[test]
    fn mouse_click_on_the_scrollbar_jumps_proportionally() {
        let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let mut ed = test_editor(&text);
        ed.options.mouse = true;
        ed.options.indicator = true;
        ed.screen_cols = 80;
        ed.screen_rows = 24;
        // editwinrows = 24 - title(1) - status(1) - help(2) = 20; clicking
        // the scrollbar's very last row (and very last column) should jump
        // at or near the end of the buffer.
        handle_click(&mut ed, 1 + 19, 79);
        assert_eq!(ed.buf().cursor.line, 100);
    }

    #[test]
    fn mouse_click_on_a_shortcut_activates_it() {
        let mut ed = test_editor("hello\n");
        ed.options.mouse = true;
        let bar_row = main_screen_layout(&ed).status_row + 1;
        // Column 2 lands within the bar's first cell ("^G Help").
        handle_click(&mut ed, bar_row, 2);
        assert!(matches!(ed.mode, Mode::Help { .. }));
    }

    #[test]
    fn mouse_wheel_scrolls_two_lines_without_moving_the_cursor() {
        let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        let mut ed = test_editor(&text);
        ed.options.mouse = true;
        let before = ed.buf().cursor;
        handle_mouse(&mut ed, mev(MouseEventKind::ScrollDown, 0, 0));
        assert_eq!(ed.buf().top_line, 2, "one wheel notch scrolls two lines");
        assert_eq!(ed.buf().cursor, before, "the cursor doesn't move");
        handle_mouse(&mut ed, mev(MouseEventKind::ScrollUp, 0, 0));
        assert_eq!(ed.buf().top_line, 0);
    }

    #[test]
    fn shortcut_bar_reverses_only_the_key_itself() {
        // nano's `post_one_key`: a short key next to a long one isn't
        // padded out inside the reverse-video block, and the description
        // follows it after a single blank.
        let ed = test_editor("");
        let entries: Vec<(String, &str)> =
            vec![("^G".into(), "Help"), ("Right".into(), "Forward")];
        let mut out = Vec::new();
        render_shortcut_bar(&ed, &mut out, 0, 40, &entries).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("\x1b[7m^G\x1b[0m Help"), "{text:?}");
        assert!(text.contains("\x1b[7mRight\x1b[0m Forward"), "{text:?}");
    }

    #[test]
    fn shortcut_bar_click_index_matches_the_rendered_grid() {
        let entries: Vec<(String, &str)> = vec![
            ("^A".into(), "Aaa"),
            ("^B".into(), "Bbb"),
            ("^C".into(), "Ccc"),
            ("^D".into(), "Ddd"),
        ];
        // max_label=2, max_desc=3 -> col_width = 2+1+3+2 = 8;
        // cols=20 -> n_cols=2 -> n_pairs=min(2, 4.div_ceil(2)=2)=2.
        assert_eq!(shortcut_bar_click_index(20, &entries, 0, 0), Some(0));
        assert_eq!(shortcut_bar_click_index(20, &entries, 1, 0), Some(1));
        assert_eq!(shortcut_bar_click_index(20, &entries, 0, 8), Some(2));
        assert_eq!(shortcut_bar_click_index(20, &entries, 1, 8), Some(3));
        assert_eq!(
            shortcut_bar_click_index(20, &entries, 2, 0),
            None,
            "only 2 rows exist"
        );
        assert_eq!(
            shortcut_bar_click_index(20, &entries, 0, 16),
            None,
            "beyond the last column"
        );
    }

    #[test]
    fn key_to_event_round_trips_through_normalize_key() {
        for key in [
            TKey::Ctrl('G'),
            TKey::Meta('U'),
            TKey::ShiftMeta('A'),
            TKey::F(1),
            TKey::Backspace,
            TKey::ShiftTab,
            TKey::Ins,
        ] {
            let ev = key_to_event(key);
            assert_eq!(normalize_key(ev), Some(key), "{key:?}");
        }
    }

    #[test]
    fn synthetic_key_event_for_label_handles_bare_chars_and_key_specs() {
        assert_eq!(synthetic_key_event_for_label(""), None);
        assert_eq!(
            synthetic_key_event_for_label("Y"),
            Some(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::NONE))
        );
        let ev = synthetic_key_event_for_label("^G").unwrap();
        assert_eq!(normalize_key(ev), Some(TKey::Ctrl('G')));
        let ev = synthetic_key_event_for_label("M-U").unwrap();
        assert_eq!(normalize_key(ev), Some(TKey::Meta('U')));
    }
}
