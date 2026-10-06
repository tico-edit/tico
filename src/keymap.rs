//! Key bindings: the set of rebindable editor functions ("actions"), the menus
//! they apply to, and the table mapping keystrokes to actions per menu.
//!
//! The function names, menu names and default bindings mirror those of GNU
//! nano 8.7.1 (see `nanorc(5)` and nano's built-in help), so that `~/.nanorc`
//! and `~/.ticorc` `bind`/`unbind` directives written for nano work unchanged.

use std::collections::HashMap;

/// A rebindable editor function, matching nano's `bind`/`unbind` function names
/// verbatim (see `nanorc(5)`, section "REBINDING KEYS").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Help,
    Cancel,
    Exit,
    WriteOut,
    SaveFile,
    Insert,
    WhereIs,
    WhereWas,
    FindPrevious,
    FindNext,
    Replace,
    Cut,
    Copy,
    Paste,
    Zap,
    ChopWordLeft,
    ChopWordRight,
    CutRestOfFile,
    Mark,
    Location,
    WordCount,
    Execute,
    Speller,
    Formatter,
    Linter,
    Justify,
    FullJustify,
    Indent,
    Unindent,
    Comment,
    Complete,
    Left,
    Right,
    Up,
    Down,
    ScrollUp,
    ScrollDown,
    Center,
    Cycle,
    PrevWord,
    NextWord,
    Home,
    End,
    BeginPara,
    EndPara,
    PrevBlock,
    NextBlock,
    TopRow,
    BottomRow,
    PageUp,
    PageDown,
    FirstLine,
    LastLine,
    GotoLine,
    FindBracket,
    Anchor,
    PrevAnchor,
    NextAnchor,
    PrevBuf,
    NextBuf,
    Verbatim,
    Tab,
    Enter,
    Delete,
    Backspace,
    RecordMacro,
    RunMacro,
    Undo,
    Redo,
    Refresh,
    Suspend,
    /// What plain `^Z` does in the main menu by default: nano's
    /// `suggest_ctrlT_ctrlZ`, which just says "To suspend, type ^T^Z".
    /// Not a nanorc-bindable function (nano has no name for it either);
    /// it exists as an action so `bind ^Z ... main` / `unbind ^Z main`
    /// displace it exactly like nano's shortcut-list entry, and so
    /// `--modernbindings` can put Undo on `^Z` instead.
    SuggestSuspend,
    CaseSens,
    Regexp,
    Backwards,
    Older,
    Newer,
    FlipReplace,
    FlipGoto,
    FlipExecute,
    FlipPipe,
    FlipNewBuffer,
    FlipConvert,
    DosFormat,
    MacFormat,
    Append,
    Prepend,
    Backup,
    DiscardBuffer,
    Browser,
    GotoDir,
    FirstFile,
    LastFile,
    NoHelp,
    Zero,
    ConstantShow,
    SoftWrap,
    LineNumbers,
    WhitespaceDisplay,
    NoSyntax,
    SmartHome,
    AutoIndent,
    CutFromCursor,
    BreakLongLines,
    TabsToSpaces,
    Mouse,
}

impl Action {
    /// Parse a nano `bind`/`unbind` function name (lowercase, as written in
    /// nanorc files) into an [`Action`].
    pub fn from_name(name: &str) -> Option<Action> {
        use Action::*;
        Some(match name {
            "help" => Help,
            "cancel" => Cancel,
            "exit" => Exit,
            "writeout" => WriteOut,
            "savefile" => SaveFile,
            "insert" => Insert,
            "whereis" => WhereIs,
            "wherewas" => WhereWas,
            "findprevious" => FindPrevious,
            "findnext" => FindNext,
            "replace" => Replace,
            "cut" => Cut,
            "copy" => Copy,
            "paste" => Paste,
            "zap" => Zap,
            "chopwordleft" => ChopWordLeft,
            "chopwordright" => ChopWordRight,
            "cutrestoffile" => CutRestOfFile,
            "mark" => Mark,
            "location" => Location,
            "wordcount" => WordCount,
            "execute" => Execute,
            "speller" => Speller,
            "formatter" => Formatter,
            "linter" => Linter,
            "justify" => Justify,
            "fulljustify" => FullJustify,
            "indent" => Indent,
            "unindent" => Unindent,
            "comment" => Comment,
            "complete" => Complete,
            "left" => Left,
            "right" => Right,
            "up" => Up,
            "down" => Down,
            "scrollup" => ScrollUp,
            "scrolldown" => ScrollDown,
            "center" => Center,
            "cycle" => Cycle,
            "prevword" => PrevWord,
            "nextword" => NextWord,
            "home" => Home,
            "end" => End,
            "beginpara" => BeginPara,
            "endpara" => EndPara,
            "prevblock" => PrevBlock,
            "nextblock" => NextBlock,
            "toprow" => TopRow,
            "bottomrow" => BottomRow,
            "pageup" => PageUp,
            "pagedown" => PageDown,
            "firstline" => FirstLine,
            "lastline" => LastLine,
            "gotoline" => GotoLine,
            "findbracket" => FindBracket,
            "anchor" => Anchor,
            "prevanchor" => PrevAnchor,
            "nextanchor" => NextAnchor,
            "prevbuf" => PrevBuf,
            "nextbuf" => NextBuf,
            "verbatim" => Verbatim,
            "tab" => Tab,
            "enter" => Enter,
            "delete" => Delete,
            "backspace" => Backspace,
            "recordmacro" => RecordMacro,
            "runmacro" => RunMacro,
            "undo" => Undo,
            "redo" => Redo,
            "refresh" => Refresh,
            "suspend" => Suspend,
            "casesens" => CaseSens,
            "regexp" => Regexp,
            "backwards" => Backwards,
            "older" => Older,
            "newer" => Newer,
            "flipreplace" => FlipReplace,
            "flipgoto" => FlipGoto,
            "flipexecute" => FlipExecute,
            "flippipe" => FlipPipe,
            "flipnewbuffer" => FlipNewBuffer,
            "flipconvert" => FlipConvert,
            "dosformat" => DosFormat,
            "macformat" => MacFormat,
            "append" => Append,
            "prepend" => Prepend,
            "backup" => Backup,
            "discardbuffer" => DiscardBuffer,
            "browser" => Browser,
            "gotodir" => GotoDir,
            "firstfile" => FirstFile,
            "lastfile" => LastFile,
            "nohelp" => NoHelp,
            "zero" => Zero,
            "constantshow" => ConstantShow,
            "softwrap" => SoftWrap,
            "linenumbers" => LineNumbers,
            "whitespacedisplay" => WhitespaceDisplay,
            "nosyntax" => NoSyntax,
            "smarthome" => SmartHome,
            "autoindent" => AutoIndent,
            "cutfromcursor" => CutFromCursor,
            "breaklonglines" => BreakLongLines,
            "tabstospaces" => TabsToSpaces,
            "mouse" => Mouse,
            _ => return None,
        })
    }

    /// A short human description of what this action does, for the `^G`
    /// help screen's shortcut listing. Phrasing follows nano's own
    /// `*_gist` strings (src/global.c) where one exists, for the actions
    /// nano itself documents; the rest (tico-internal toggles nano lacks a
    /// public name for, plus a few tico-specific ones) get an equivalent
    /// phrase in the same style.
    pub fn description(&self) -> &'static str {
        use Action::*;
        match self {
            Help => "Display this help text",
            Cancel => "Cancel the current function",
            Exit => "Close the current buffer / Exit from tico",
            WriteOut => "Write the current buffer (or the marked region) to disk",
            SaveFile => "Save file without prompting",
            Insert => "Insert another file into current buffer (or into new buffer)",
            WhereIs => "Search forward for a string or a regular expression",
            WhereWas => "Search backward for a string or a regular expression",
            FindPrevious => "Search next occurrence backward",
            FindNext => "Search next occurrence forward",
            Replace => "Replace a string or a regular expression",
            Cut => "Cut current line (or marked region) and store it in cutbuffer",
            Copy => "Copy current line (or marked region) and store it in cutbuffer",
            Paste => "Paste the contents of cutbuffer at current cursor position",
            Zap => "Throw away the current line (or marked region)",
            ChopWordLeft => "Delete backward from cursor to word start",
            ChopWordRight => "Delete forward from cursor to next word start",
            CutRestOfFile => "Cut from the cursor position to the end of the file",
            Mark => "Mark text starting from the cursor position",
            Location => "Display the position of the cursor",
            WordCount => "Count the number of lines, words, and characters",
            Execute => "Execute a function or an external command",
            Speller => "Invoke the spell checker, if available",
            Formatter => "Invoke a program to format/arrange/manipulate the buffer",
            Linter => "Invoke the linter, if available",
            Justify => "Justify the current paragraph",
            FullJustify => "Justify the entire file",
            Indent => "Indent the current line (or marked lines)",
            Unindent => "Unindent the current line (or marked lines)",
            Comment => "Comment/uncomment the current line (or marked lines)",
            Complete => "Try and complete the current word",
            Left => "Go back one character",
            Right => "Go forward one character",
            Up => "Go to previous line",
            Down => "Go to next line",
            ScrollUp => "Scroll up one line without moving the cursor textually",
            ScrollDown => "Scroll down one line without moving the cursor textually",
            Center => "Center the line where the cursor is",
            Cycle => "Push the cursor line to the center, then top, then bottom",
            PrevWord => "Go back one word",
            NextWord => "Go forward one word",
            Home => "Go to beginning of current line",
            End => "Go to end of current line",
            BeginPara => "Go to beginning of paragraph; then of previous paragraph",
            EndPara => "Go just beyond end of paragraph; then of next paragraph",
            PrevBlock => "Go to previous block of text",
            NextBlock => "Go to next block of text",
            TopRow => "Go to first row in the viewport",
            BottomRow => "Go to last row in the viewport",
            PageUp => "Go one screenful up",
            PageDown => "Go one screenful down",
            FirstLine => "Go to the first line of the file",
            LastLine => "Go to the last line of the file",
            GotoLine => "Go to line and column number",
            FindBracket => "Go to the matching bracket",
            Anchor => "Place or remove an anchor at the current line",
            PrevAnchor => "Jump backward to the nearest anchor",
            NextAnchor => "Jump forward to the nearest anchor",
            PrevBuf => "Switch to the previous file buffer",
            NextBuf => "Switch to the next file buffer",
            Verbatim => "Insert the next keystroke verbatim",
            Tab => "Insert a tab at the cursor position (or indent marked lines)",
            Enter => "Insert a newline at the cursor position",
            Delete => "Delete the character under the cursor",
            Backspace => "Delete the character to the left of the cursor",
            RecordMacro => "Start/stop recording a macro",
            RunMacro => "Run the last recorded macro",
            Undo => "Undo the last operation",
            Redo => "Redo the last undone operation",
            Refresh => "Refresh (redraw) the current screen",
            Suspend => "Suspend the editor (return to the shell)",
            SuggestSuspend => "Say how to suspend the editor",
            CaseSens => "Toggle the case sensitivity of the search",
            Regexp => "Toggle the use of regular expressions",
            Backwards => "Reverse the direction of the search",
            Older => "Recall the previous search/replace/command string",
            Newer => "Recall the next search/replace/command string",
            FlipReplace => "Switch between searching and replacing",
            FlipGoto => "Switch between searching and going to a line",
            FlipExecute => "Switch between inserting a file and running a command",
            FlipPipe => "Pipe the current buffer (or marked region) to the command",
            FlipNewBuffer => "Toggle the use of a new buffer",
            FlipConvert => "Do not convert from DOS/Mac format",
            DosFormat => "Toggle the use of DOS format",
            MacFormat => "Toggle the use of Mac format",
            Append => "Toggle appending",
            Prepend => "Toggle prepending",
            Backup => "Toggle backing up of the original file",
            DiscardBuffer => "Close buffer without saving it",
            Browser => "Go to file browser",
            GotoDir => "Go to directory",
            FirstFile => "Go to the first file in the list",
            LastFile => "Go to the last file in the list",
            NoHelp => "Toggle the display of the shortcut lists",
            Zero => "Toggle the use of the title bar and status bar",
            ConstantShow => "Toggle constant cursor position display",
            SoftWrap => "Toggle the displaying of overlong lines on multiple screen lines",
            LineNumbers => "Toggle the display of line numbers",
            WhitespaceDisplay => "Toggle the visibility of whitespace",
            NoSyntax => "Toggle syntax highlighting",
            SmartHome => "Toggle the smartness of the Home key",
            AutoIndent => "Toggle auto-indent",
            CutFromCursor => "Toggle cutting from cursor to end of line, instead of whole line",
            BreakLongLines => "Toggle whether the overlong part of a line is hard-wrapped",
            TabsToSpaces => "Toggle whether typed tabs are converted to spaces",
            Mouse => "Toggle mouse support",
        }
    }
}

impl Action {
    /// [`description`](Self::description), except where `menu` gives the
    /// action a meaning of its own: the file browser reuses the main
    /// menu's movement and search functions, and nano's help there uses
    /// the browser's own phrasing (its `browser*_gist` strings).
    pub fn description_in(&self, menu: Menu) -> &'static str {
        use Action::*;
        if menu == Menu::Browser {
            match self {
                Exit => return "Exit from the file browser",
                WhereIs => return "Search forward for a string",
                WhereWas => return "Search backward for a string",
                Left => return "Go to the previous file in the list",
                Right => return "Go to the next file in the list",
                PrevWord => return "Go to lefthand column",
                NextWord => return "Go to righthand column",
                PrevBlock => return "Go to first row in this column",
                NextBlock => return "Go to last row in this column",
                Refresh => return "Refresh the file list",
                _ => {}
            }
        }
        self.description()
    }
}

/// The menu (keystroke context) a binding applies to, matching nano's menu
/// names from `nanorc(5)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Menu {
    Main,
    Help,
    Search,
    Replace,
    ReplaceWith,
    YesNo,
    GotoLine,
    WriteOut,
    Insert,
    Browser,
    WhereIsFile,
    GotoDir,
    Execute,
    Spell,
    Linter,
}

impl Menu {
    pub fn from_name(name: &str) -> Option<Menu> {
        use Menu::*;
        Some(match name {
            "main" => Main,
            "help" => Help,
            "search" => Search,
            "replace" => Replace,
            "replacewith" => ReplaceWith,
            "yesno" => YesNo,
            "gotoline" => GotoLine,
            "writeout" => WriteOut,
            "insert" => Insert,
            "browser" => Browser,
            "whereisfile" => WhereIsFile,
            "gotodir" => GotoDir,
            "execute" => Execute,
            "spell" => Spell,
            "linter" => Linter,
            _ => return None,
        })
    }

    pub const ALL: &'static [Menu] = &[
        Menu::Main,
        Menu::Help,
        Menu::Search,
        Menu::Replace,
        Menu::ReplaceWith,
        Menu::YesNo,
        Menu::GotoLine,
        Menu::WriteOut,
        Menu::Insert,
        Menu::Browser,
        Menu::WhereIsFile,
        Menu::GotoDir,
        Menu::Execute,
        Menu::Spell,
        Menu::Linter,
    ];
}

/// A single keystroke, normalized from terminal input. Covers both the
/// rebindable keys (Ctrl/Meta/Shift-Meta/function keys/Ins/Del) described in
/// `nanorc(5)`'s "REBINDING KEYS" section, and the dedicated cursor-moving
/// keys which nano documents as *not* rebindable, but which we still route
/// through the same dispatch table (config code refuses to rebind them, to
/// match nano's documented behavior).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// Ctrl+X. `ch` is the uppercase letter, or one of `@ ] \ ^ _`, or `' '`
    /// for the word "Space".
    Ctrl(char),
    /// Meta (Alt)+X. `ch` is any ASCII character except `[`, or `' '` for Space.
    Meta(char),
    /// Shift+Meta+letter.
    ShiftMeta(char),
    /// Function key F1..F24.
    F(u8),
    Ins,
    Del,
    /// The physical Backspace key (nano's "Bsp"/`KEY_BACKSPACE`), kept
    /// distinct from a literal Ctrl+H keypress: most terminals send a
    /// different byte for Backspace than for Ctrl+H, and `--modernbindings`
    /// depends on that distinction (it repurposes Ctrl+H for Help while
    /// Backspace keeps deleting).
    Backspace,
    ShiftTab,
    // Dedicated, non-rebindable navigation keys.
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    CtrlLeft,
    CtrlRight,
    CtrlUp,
    CtrlDown,
    CtrlHome,
    CtrlEnd,
    CtrlDel,
    ShiftCtrlDel,
    MetaLeft,
    MetaRight,
    MetaUp,
    MetaDown,
    MetaHome,
    MetaEnd,
    MetaPgUp,
    MetaPgDn,
    MetaIns,
    MetaDel,
}

/// Keys that nano documents as not being reassignable via `bind`/`unbind`.
pub fn is_rebindable(key: &Key) -> bool {
    !matches!(
        key,
        Key::Left
            | Key::Right
            | Key::Up
            | Key::Down
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
    )
}

impl Key {
    /// Render a key the way nano's help viewer and shortcut bars do
    /// (`^X`, `M-x`, `Sh-M-X`, `F2`, ...) — the inverse of `parse`, roughly;
    /// used only for display, so it doesn't need to round-trip exactly.
    pub fn describe(&self) -> String {
        match self {
            Key::Ctrl(' ') => "^Space".to_string(),
            Key::Ctrl(c) => format!("^{c}"),
            Key::Meta(' ') => "M-Space".to_string(),
            Key::Meta(c) => format!("M-{c}"),
            Key::ShiftMeta(c) => format!("Sh-M-{c}"),
            Key::F(n) => format!("F{n}"),
            Key::Ins => "Ins".to_string(),
            Key::Del => "Del".to_string(),
            Key::Backspace => "Bsp".to_string(),
            Key::ShiftTab => "Sh-Tab".to_string(),
            Key::Left => "Left".to_string(),
            Key::Right => "Right".to_string(),
            Key::Up => "Up".to_string(),
            Key::Down => "Down".to_string(),
            Key::Home => "Home".to_string(),
            Key::End => "End".to_string(),
            Key::PageUp => "PgUp".to_string(),
            Key::PageDown => "PgDn".to_string(),
            Key::CtrlLeft => "^Left".to_string(),
            Key::CtrlRight => "^Right".to_string(),
            Key::CtrlUp => "^Up".to_string(),
            Key::CtrlDown => "^Down".to_string(),
            Key::CtrlHome => "^Home".to_string(),
            Key::CtrlEnd => "^End".to_string(),
            Key::CtrlDel => "^Del".to_string(),
            Key::ShiftCtrlDel => "Sh-^Del".to_string(),
            Key::MetaLeft => "M-Left".to_string(),
            Key::MetaRight => "M-Right".to_string(),
            Key::MetaUp => "M-Up".to_string(),
            Key::MetaDown => "M-Down".to_string(),
            Key::MetaHome => "M-Home".to_string(),
            Key::MetaEnd => "M-End".to_string(),
            Key::MetaPgUp => "M-PgUp".to_string(),
            Key::MetaPgDn => "M-PgDn".to_string(),
            Key::MetaIns => "M-Ins".to_string(),
            Key::MetaDel => "M-Del".to_string(),
        }
    }

    /// Sort key for showing the "primary" binding of an action before its
    /// alternates, roughly matching nano's own convention (Ctrl first, then
    /// function keys, then Meta).
    pub(crate) fn display_rank(&self) -> (u8, i32) {
        match self {
            Key::Ctrl(c) => (0, *c as i32),
            Key::CtrlLeft
            | Key::CtrlRight
            | Key::CtrlUp
            | Key::CtrlDown
            | Key::CtrlHome
            | Key::CtrlEnd
            | Key::CtrlDel => (0, 0),
            Key::F(n) => (1, *n as i32),
            Key::Meta(c) => (2, *c as i32),
            Key::ShiftMeta(c) => (3, *c as i32),
            _ => (4, 0),
        }
    }

    /// Parse a key specification as written in a nanorc `bind`/`unbind` line:
    /// `^X`, `M-X`, `Sh-M-X`, `FN` (F1..F24), `Ins`, `Del`, or `Bsp`.
    pub fn parse(spec: &str) -> Option<Key> {
        if spec.eq_ignore_ascii_case("ins") {
            return Some(Key::Ins);
        }
        if spec.eq_ignore_ascii_case("del") {
            return Some(Key::Del);
        }
        if spec.eq_ignore_ascii_case("bsp") {
            return Some(Key::Backspace);
        }
        if let Some(rest) = spec.strip_prefix("F").or_else(|| spec.strip_prefix('f'))
            && let Ok(n) = rest.parse::<u8>()
            && (1..=24).contains(&n)
        {
            return Some(Key::F(n));
        }
        if let Some(rest) = spec
            .strip_prefix("Sh-M-")
            .or_else(|| spec.strip_prefix("sh-m-"))
        {
            let ch = normalize_letter(rest)?;
            return Some(Key::ShiftMeta(ch));
        }
        if let Some(rest) = spec.strip_prefix("M-").or_else(|| spec.strip_prefix("m-")) {
            let ch = normalize_meta_char(rest)?;
            return Some(Key::Meta(ch));
        }
        if let Some(rest) = spec.strip_prefix('^') {
            let ch = normalize_ctrl_char(rest)?;
            return Some(Key::Ctrl(ch));
        }
        None
    }
}

fn normalize_letter(rest: &str) -> Option<char> {
    let mut chars = rest.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c.is_ascii_alphabetic() {
        Some(c.to_ascii_uppercase())
    } else {
        None
    }
}

fn normalize_ctrl_char(rest: &str) -> Option<char> {
    if rest.eq_ignore_ascii_case("space") {
        return Some(' ');
    }
    let mut chars = rest.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c.is_ascii_alphabetic() {
        Some(c.to_ascii_uppercase())
    } else if matches!(c, '@' | ']' | '\\' | '^' | '_') {
        Some(c)
    } else {
        None
    }
}

fn normalize_meta_char(rest: &str) -> Option<char> {
    if rest.eq_ignore_ascii_case("space") {
        return Some(' ');
    }
    let mut chars = rest.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c == '[' { None } else { Some(c) }
}

/// A binding target: either an [`Action`], or a literal string to type
/// (nano's `bind key "string" menu` form), which may itself reference
/// actions by name in `{braces}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Action(Action),
    Macro(String),
}

/// The full key-binding table: (menu, key) -> binding, built from nano's
/// defaults and then overridden by `bind`/`unbind` directives from
/// `~/.nanorc` and `~/.ticorc`, in that precedence order.
#[derive(Debug, Clone, Default)]
pub struct KeyMap {
    table: HashMap<(Menu, Key), Binding>,
}

/// What nano leaves out of `menu` in restricted mode -- unbound, and so
/// missing from its shortcut bar and help text: at the Write Out prompt
/// Append, Prepend, Backup File and Browse; at the Read File and Execute
/// Command prompts New Buffer, Execute Command, Pipe Text and Browse; and
/// on the main menu Read File (whose key stays, to report "disabled").
pub fn hidden_when_restricted(menu: Menu, action: Action) -> bool {
    use Action as A;
    match menu {
        Menu::WriteOut => matches!(action, A::Append | A::Prepend | A::Backup | A::Browser),
        Menu::Insert | Menu::Execute => matches!(
            action,
            A::FlipNewBuffer | A::FlipExecute | A::FlipPipe | A::Browser
        ),
        Menu::Main => action == A::Insert,
        _ => false,
    }
}

impl KeyMap {
    pub fn new() -> KeyMap {
        KeyMap {
            table: HashMap::new(),
        }
    }

    pub fn bind(&mut self, menu: Menu, key: Key, binding: Binding) {
        self.table.insert((menu, key), binding);
    }

    pub fn bind_all_menus(&mut self, menus: &[Menu], key: Key, binding: Binding) {
        for &menu in menus {
            self.bind(menu, key, binding.clone());
        }
    }

    pub fn unbind(&mut self, menu: Menu, key: Key) {
        self.table.remove(&(menu, key));
    }

    pub fn unbind_all_menus(&mut self, key: Key) {
        for &menu in Menu::ALL {
            self.unbind(menu, key);
        }
    }

    /// Look up a key, falling back to the [`Menu::Main`] binding for menus
    /// that don't have their own entry for a given key (mirrors nano's
    /// "all menus" semantics for many global keys like ^C=cancel, arrows, etc).
    pub fn lookup(&self, menu: Menu, key: Key) -> Option<&Binding> {
        self.table
            .get(&(menu, key))
            .or_else(|| self.table.get(&(Menu::Main, key)))
    }

    /// Like `lookup`, but without the Main-menu fallback: only a binding
    /// registered for exactly this menu counts. Prompt menus intentionally
    /// have their own small, curated set of bindings (see
    /// `install_prompt_defaults`); falling back to Main's editing
    /// shortcuts there would apply the wrong action to an unrelated key
    /// (e.g. Main's `Ctrl+Home` = FirstLine leaking into the WriteOut
    /// prompt and unexpectedly aborting a save).
    pub fn lookup_menu_only(&self, menu: Menu, key: Key) -> Option<&Binding> {
        self.table.get(&(menu, key))
    }

    /// All (menu, key) -> binding entries, e.g. for building a help screen's
    /// shortcut listing straight from the live bindings (so a `bind`/
    /// `unbind` in nanorc/ticorc is reflected automatically).
    pub fn entries(&self) -> impl Iterator<Item = (&(Menu, Key), &Binding)> {
        self.table.iter()
    }

    /// Build the default keybinding table, matching GNU nano 8.7.1's
    /// compiled-in defaults for the main editing menu (captured directly
    /// from the running `nano` binary's help viewer), plus conventional
    /// bindings for the prompt/list menus. `modern` mirrors nano's
    /// `-/`/`--modernbindings`: CLI-only in nano (no `set` equivalent — not
    /// in nano's own rcfile.c), so it must be known before any nanorc/
    /// ticorc `bind`/`unbind` directives are layered on, exactly as it
    /// would be for nano's own `global_init()`.
    pub fn defaults(modern: bool) -> KeyMap {
        let mut km = KeyMap::new();
        km.install_main_defaults();
        km.install_prompt_defaults();
        if modern {
            km.install_modern_overrides();
        }
        km
    }

    fn install_main_defaults(&mut self) {
        use Action as A;
        use Key as K;
        let m = Menu::Main;
        let mut b = |k: Key, a: Action| self.bind(m, k, Binding::Action(a));

        b(K::Ctrl('G'), A::Help);
        b(K::F(1), A::Help);
        b(K::Ctrl('X'), A::Exit);
        b(K::F(2), A::Exit);
        b(K::Ctrl('O'), A::WriteOut);
        b(K::F(3), A::WriteOut);
        b(K::Ctrl('R'), A::Insert);
        b(K::Ins, A::Insert);
        b(K::Ctrl('F'), A::WhereIs);
        b(K::Ctrl('W'), A::WhereIs);
        b(K::F(6), A::WhereIs);
        b(K::Ctrl('B'), A::WhereWas);
        b(K::Ctrl('Q'), A::WhereWas);
        b(K::Meta('B'), A::FindPrevious);
        b(K::Meta('Q'), A::FindPrevious);
        b(K::Meta('F'), A::FindNext);
        b(K::Meta('W'), A::FindNext);
        b(K::Ctrl('\\'), A::Replace);
        b(K::Meta('R'), A::Replace);
        b(K::Ctrl('K'), A::Cut);
        b(K::F(9), A::Cut);
        b(K::Ctrl('U'), A::Paste);
        b(K::F(10), A::Paste);
        b(K::Meta('T'), A::CutRestOfFile);
        b(K::Meta('6'), A::Copy);
        b(K::Meta('^'), A::Copy);
        b(K::Meta('A'), A::Mark);
        b(K::Ctrl('6'), A::Mark);
        b(K::Ctrl('^'), A::Mark);
        b(K::MetaDel, A::Zap);
        b(K::ShiftCtrlDel, A::ChopWordLeft);
        b(K::CtrlDel, A::ChopWordRight);
        b(K::Ctrl('T'), A::Execute);
        b(K::F(12), A::Speller);
        b(K::Ctrl('J'), A::Justify);
        b(K::F(4), A::Justify);
        b(K::Meta('J'), A::FullJustify);
        b(K::Meta('}'), A::Indent);
        b(K::Meta('{'), A::Unindent);
        b(K::ShiftTab, A::Unindent);
        b(K::Meta('3'), A::Comment);
        b(K::Ctrl(']'), A::Complete);
        b(K::Left, A::Left);
        b(K::Right, A::Right);
        b(K::Up, A::Up);
        b(K::Down, A::Down);
        b(K::Ctrl('P'), A::Up);
        b(K::Ctrl('N'), A::Down);
        b(K::Meta('-'), A::ScrollUp);
        b(K::Meta('_'), A::ScrollUp);
        b(K::MetaUp, A::ScrollUp);
        b(K::Meta('+'), A::ScrollDown);
        b(K::Meta('='), A::ScrollDown);
        b(K::MetaDown, A::ScrollDown);
        b(K::Ctrl('L'), A::Center);
        b(K::Meta('%'), A::Cycle);
        b(K::CtrlLeft, A::PrevWord);
        b(K::Meta(' '), A::PrevWord);
        b(K::CtrlRight, A::NextWord);
        b(K::Ctrl(' '), A::NextWord);
        b(K::Ctrl('A'), A::Home);
        b(K::Home, A::Home);
        b(K::Ctrl('E'), A::End);
        b(K::End, A::End);
        b(K::Meta('('), A::BeginPara);
        b(K::Meta('9'), A::BeginPara);
        b(K::Meta(')'), A::EndPara);
        b(K::Meta('0'), A::EndPara);
        b(K::CtrlUp, A::PrevBlock);
        b(K::Meta('7'), A::PrevBlock);
        b(K::CtrlDown, A::NextBlock);
        b(K::Meta('8'), A::NextBlock);
        b(K::MetaHome, A::TopRow);
        b(K::MetaEnd, A::BottomRow);
        b(K::Ctrl('Y'), A::PageUp);
        b(K::PageUp, A::PageUp);
        b(K::Ctrl('V'), A::PageDown);
        b(K::PageDown, A::PageDown);
        b(K::Meta('\\'), A::FirstLine);
        b(K::CtrlHome, A::FirstLine);
        b(K::Meta('/'), A::LastLine);
        b(K::CtrlEnd, A::LastLine);
        b(K::Ctrl('_'), A::GotoLine);
        b(K::Meta('G'), A::GotoLine);
        b(K::Meta(']'), A::FindBracket);
        b(K::Meta('"'), A::Anchor);
        b(K::MetaIns, A::Anchor);
        b(K::MetaPgUp, A::PrevAnchor);
        b(K::MetaPgDn, A::NextAnchor);
        b(K::Meta('\''), A::NextAnchor);
        b(K::MetaLeft, A::PrevBuf);
        b(K::Meta('<'), A::PrevBuf);
        b(K::MetaRight, A::NextBuf);
        b(K::Meta('>'), A::NextBuf);
        b(K::Meta('V'), A::Verbatim);
        b(K::Ctrl('I'), A::Tab);
        b(K::Ctrl('M'), A::Enter);
        b(K::Ctrl('D'), A::Delete);
        b(K::Ctrl('H'), A::Backspace);
        b(K::Backspace, A::Backspace);
        b(K::Meta(':'), A::RecordMacro);
        b(K::Meta(';'), A::RunMacro);
        b(K::Meta('U'), A::Undo);
        b(K::Meta('E'), A::Redo);
        // nano binds plain ^Z in the main menu to a hint about ^T^Z, not
        // to suspend itself (that lives in the Execute menu).
        b(K::Ctrl('Z'), A::SuggestSuspend);
        b(K::Ctrl('S'), A::SaveFile);
        b(K::Ctrl('C'), A::Location);
        b(K::F(11), A::Location);
        b(K::Meta('D'), A::WordCount);
        b(K::Meta('Z'), A::Zero);
        b(K::Meta('X'), A::NoHelp);
        b(K::Meta('C'), A::ConstantShow);
        b(K::Meta('S'), A::SoftWrap);
        b(K::Meta('N'), A::LineNumbers);
        b(K::Meta('P'), A::WhitespaceDisplay);
        b(K::Meta('Y'), A::NoSyntax);
        b(K::Meta('H'), A::SmartHome);
        b(K::Meta('I'), A::AutoIndent);
        b(K::Meta('K'), A::CutFromCursor);
        b(K::Meta('L'), A::BreakLongLines);
        b(K::Meta('O'), A::TabsToSpaces);
        b(K::Meta('M'), A::Mouse);
    }

    fn install_prompt_defaults(&mut self) {
        use Action as A;
        use Key as K;
        // Bindings that make sense (and are standard in nano) across every
        // prompt/list menu: Cancel, and basic line editing on the prompt.
        for &menu in Menu::ALL {
            if menu == Menu::Main {
                continue;
            }
            self.bind(menu, K::Ctrl('C'), Binding::Action(A::Cancel));
            self.bind(menu, K::Ctrl('G'), Binding::Action(A::Help));
            self.bind(menu, K::Left, Binding::Action(A::Left));
            self.bind(menu, K::Right, Binding::Action(A::Right));
            self.bind(menu, K::Home, Binding::Action(A::Home));
            self.bind(menu, K::End, Binding::Action(A::End));
            self.bind(menu, K::Ctrl('H'), Binding::Action(A::Backspace));
            self.bind(menu, K::Backspace, Binding::Action(A::Backspace));
            self.bind(menu, K::Ctrl('D'), Binding::Action(A::Delete));
        }
        // Verified against nano's src/global.c (add_to_sclist calls), not
        // guessed: MWHEREIS|MREPLACE share case/regex/backwards toggles and
        // ^R (flip to replace); MWHEREIS|MGOTOLINE|MFINDINHELP share ^Y/^V
        // to jump straight to the first/last line (this is the behavior
        // that prompted double-checking all of these); history recall
        // (^P/^N) is shared much more broadly, across every prompt that
        // remembers previous entries.
        self.bind(Menu::Search, K::Ctrl('M'), Binding::Action(A::WhereIs));
        self.bind(Menu::Search, K::Ctrl('R'), Binding::Action(A::FlipReplace));
        self.bind(Menu::Search, K::Ctrl('T'), Binding::Action(A::FlipGoto));
        self.bind(Menu::Search, K::Ctrl('Y'), Binding::Action(A::FirstLine));
        self.bind(Menu::Search, K::Ctrl('V'), Binding::Action(A::LastLine));
        self.bind(Menu::Search, K::Meta('C'), Binding::Action(A::CaseSens));
        self.bind(Menu::Search, K::Meta('R'), Binding::Action(A::Regexp));
        self.bind(Menu::Search, K::Meta('B'), Binding::Action(A::Backwards));

        self.bind(Menu::Replace, K::Ctrl('M'), Binding::Action(A::Replace));
        self.bind(Menu::Replace, K::Ctrl('R'), Binding::Action(A::FlipReplace));
        self.bind(Menu::Replace, K::Meta('C'), Binding::Action(A::CaseSens));
        self.bind(Menu::Replace, K::Meta('R'), Binding::Action(A::Regexp));
        self.bind(Menu::Replace, K::Meta('B'), Binding::Action(A::Backwards));
        self.bind(Menu::ReplaceWith, K::Ctrl('M'), Binding::Action(A::Replace));

        for &menu in &[
            Menu::Search,
            Menu::Replace,
            Menu::ReplaceWith,
            Menu::Execute,
        ] {
            self.bind(menu, K::Ctrl('P'), Binding::Action(A::Older));
            self.bind(menu, K::Ctrl('N'), Binding::Action(A::Newer));
            // nano also binds the Up/Down arrows themselves to history
            // recall in these menus (not cursor movement, since there's
            // nothing to move to above/below a single-line prompt) —
            // confirmed against the installed nano.
            self.bind(menu, K::Up, Binding::Action(A::Older));
            self.bind(menu, K::Down, Binding::Action(A::Newer));
        }

        self.bind(Menu::GotoLine, K::Ctrl('M'), Binding::Action(A::GotoLine));
        self.bind(Menu::GotoLine, K::Ctrl('T'), Binding::Action(A::FlipGoto));
        self.bind(Menu::GotoLine, K::Ctrl('Y'), Binding::Action(A::FirstLine));
        self.bind(Menu::GotoLine, K::Ctrl('V'), Binding::Action(A::LastLine));
        self.bind(Menu::GotoLine, K::Ctrl('W'), Binding::Action(A::BeginPara));
        self.bind(Menu::GotoLine, K::Ctrl('O'), Binding::Action(A::EndPara));

        // 'Y'es/'N'o/'A'll at yesno prompts are handled specially by the
        // prompt code (they read the literal character), not via the keymap.

        self.bind(Menu::WriteOut, K::Ctrl('M'), Binding::Action(A::WriteOut));
        self.bind(
            Menu::WriteOut,
            K::Ctrl('Q'),
            Binding::Action(A::DiscardBuffer),
        );
        self.bind(Menu::WriteOut, K::Meta('D'), Binding::Action(A::DosFormat));
        self.bind(Menu::WriteOut, K::Meta('M'), Binding::Action(A::MacFormat));
        // nano's `to_files` (Browse) is bound in both the Write Out and
        // Read File menus.
        self.bind(Menu::WriteOut, K::Ctrl('T'), Binding::Action(A::Browser));
        self.bind(Menu::WriteOut, K::Meta('A'), Binding::Action(A::Append));
        self.bind(Menu::WriteOut, K::Meta('P'), Binding::Action(A::Prepend));
        self.bind(Menu::WriteOut, K::Meta('B'), Binding::Action(A::Backup));

        self.bind(Menu::Insert, K::Ctrl('M'), Binding::Action(A::Insert));
        self.bind(
            Menu::Insert,
            K::Meta('F'),
            Binding::Action(A::FlipNewBuffer),
        );
        self.bind(Menu::Insert, K::Meta('N'), Binding::Action(A::FlipConvert));
        self.bind(Menu::Insert, K::Ctrl('X'), Binding::Action(A::FlipExecute));
        self.bind(Menu::Insert, K::Ctrl('T'), Binding::Action(A::Browser));

        self.bind(Menu::Execute, K::Ctrl('M'), Binding::Action(A::Execute));
        self.bind(Menu::Execute, K::Ctrl('G'), Binding::Action(A::Help));
        // nano binds both ^S and ^T to the speller here (^S only when not
        // `set preserve`, which tico doesn't implement, so unconditionally)
        // — ^S sorts first and is what the shortcut bar shows.
        self.bind(Menu::Execute, K::Ctrl('S'), Binding::Action(A::Speller));
        self.bind(Menu::Execute, K::Ctrl('T'), Binding::Action(A::Speller));
        self.bind(Menu::Execute, K::Ctrl('Y'), Binding::Action(A::Linter));
        self.bind(Menu::Execute, K::Ctrl('O'), Binding::Action(A::Formatter));
        self.bind(
            Menu::Execute,
            K::Ctrl('V'),
            Binding::Action(A::CutRestOfFile),
        );
        self.bind(Menu::Execute, K::Ctrl('Z'), Binding::Action(A::Suspend));
        self.bind(Menu::Execute, K::Ctrl('J'), Binding::Action(A::FullJustify));
        self.bind(Menu::Execute, K::Ctrl('X'), Binding::Action(A::FlipExecute));
        self.bind(
            Menu::Execute,
            K::Meta('F'),
            Binding::Action(A::FlipNewBuffer),
        );
        self.bind(Menu::Execute, K::Meta('\\'), Binding::Action(A::FlipPipe));

        self.bind(Menu::Help, K::Home, Binding::Action(A::FirstLine));
        self.bind(Menu::Help, K::End, Binding::Action(A::LastLine));
        self.bind(Menu::Help, K::Up, Binding::Action(A::Up));
        self.bind(Menu::Help, K::Down, Binding::Action(A::Down));
        self.bind(Menu::Help, K::Ctrl('P'), Binding::Action(A::Up));
        self.bind(Menu::Help, K::Ctrl('N'), Binding::Action(A::Down));
        self.bind(Menu::Help, K::PageUp, Binding::Action(A::PageUp));
        self.bind(Menu::Help, K::PageDown, Binding::Action(A::PageDown));
        self.bind(Menu::Help, K::Ctrl('Y'), Binding::Action(A::PageUp));
        self.bind(Menu::Help, K::Ctrl('V'), Binding::Action(A::PageDown));
        self.bind(Menu::Help, K::Meta('\\'), Binding::Action(A::FirstLine));
        self.bind(Menu::Help, K::Meta('/'), Binding::Action(A::LastLine));
        self.bind(Menu::Help, K::Ctrl('X'), Binding::Action(A::Cancel));

        self.install_browser_defaults();

        self.bind(Menu::Linter, K::Ctrl('X'), Binding::Action(A::Cancel));
        self.bind(Menu::Linter, K::Ctrl('C'), Binding::Action(A::Cancel));
        self.bind(Menu::Linter, K::Ctrl('M'), Binding::Action(A::Cancel));
        self.bind(Menu::Linter, K::PageUp, Binding::Action(A::PageUp));
        self.bind(Menu::Linter, K::PageDown, Binding::Action(A::PageDown));
    }

    /// The file browser (`MBROWSER`) and its two prompts, Search
    /// (`MWHEREISFILE`) and Go To Directory (`MGOTODIR`), from nano's
    /// `add_to_sclist` calls in src/global.c. The browser mostly reuses the
    /// main menu's function names with a browser meaning: `left`/`right`
    /// step through the names, `prevword`/`nextword` go to the first/last
    /// column, `prevblock`/`nextblock` to the top/bottom row. Plain letters
    /// (`s`, `q`, `n`, Space, ...) work too, but aren't bindings: nano
    /// checks them before the keymap (`interpret()`), and so does
    /// `ui::browser_plain_key`.
    fn install_browser_defaults(&mut self) {
        use Action as A;
        use Key as K;
        // The shared prompt-line editing keys (Home/End, Backspace, ^D, ...)
        // mean something else here, or nothing.
        self.table.retain(|(menu, _), _| *menu != Menu::Browser);
        let m = Menu::Browser;
        let mut b = |k: Key, a: Action| self.bind(m, k, Binding::Action(a));
        b(K::Ctrl('M'), A::Enter);
        b(K::Ctrl('G'), A::Help);
        b(K::Ctrl('X'), A::Exit);
        b(K::Ctrl('C'), A::Exit);
        // Leaving with the same key as used for entry.
        b(K::Ctrl('T'), A::Exit);
        b(K::F(2), A::Exit);
        b(K::Ctrl('F'), A::WhereIs);
        b(K::Ctrl('W'), A::WhereIs);
        b(K::F(6), A::WhereIs);
        b(K::Ctrl('B'), A::WhereWas);
        b(K::Ctrl('Q'), A::WhereWas);
        b(K::Meta('B'), A::FindPrevious);
        b(K::Meta('Q'), A::FindPrevious);
        b(K::Meta('F'), A::FindNext);
        b(K::Meta('W'), A::FindNext);
        b(K::Left, A::Left);
        b(K::Right, A::Right);
        b(K::CtrlLeft, A::PrevWord);
        b(K::CtrlRight, A::NextWord);
        b(K::Up, A::Up);
        b(K::Down, A::Down);
        b(K::Ctrl('P'), A::Up);
        b(K::Ctrl('N'), A::Down);
        b(K::CtrlUp, A::PrevBlock);
        b(K::CtrlDown, A::NextBlock);
        b(K::PageUp, A::PageUp);
        b(K::Ctrl('Y'), A::PageUp);
        b(K::F(7), A::PageUp);
        b(K::Backspace, A::PageUp);
        b(K::PageDown, A::PageDown);
        b(K::Ctrl('V'), A::PageDown);
        b(K::F(8), A::PageDown);
        b(K::Meta('\\'), A::FirstFile);
        b(K::Home, A::FirstFile);
        b(K::CtrlHome, A::FirstFile);
        b(K::Meta('/'), A::LastFile);
        b(K::End, A::LastFile);
        b(K::CtrlEnd, A::LastFile);
        // nano shows this one as "^/": the same byte as ^_.
        b(K::Ctrl('_'), A::GotoDir);
        b(K::Meta('G'), A::GotoDir);
        b(K::Ctrl('L'), A::Refresh);
        b(K::Meta('X'), A::NoHelp);

        for menu in [Menu::WhereIsFile, Menu::GotoDir] {
            self.bind(menu, K::Ctrl('M'), Binding::Action(A::Enter));
        }
        let w = Menu::WhereIsFile;
        for (k, a) in [
            (K::Ctrl('P'), A::Older),
            (K::Ctrl('N'), A::Newer),
            (K::Up, A::Older),
            (K::Down, A::Newer),
            (K::Ctrl('Y'), A::FirstFile),
            (K::Ctrl('V'), A::LastFile),
            (K::Meta('\\'), A::FirstFile),
            (K::Meta('/'), A::LastFile),
        ] {
            self.bind(w, k, Binding::Action(a));
        }
    }

    /// `-/`/`--modernbindings`: rebinds a batch of Main-menu Ctrl-key
    /// shortcuts to ones considered more familiar outside the Pico/nano
    /// tradition (^S save, ^X cut, ^C copy, ^V paste, ^Z/^Y undo/redo, ...),
    /// at the cost of some of nano's own traditional ones. Verified against
    /// the `ISSET(MODERN_BINDINGS)` branch of nano's `src/global.c`
    /// (`add_to_sclist` calls), not guessed — including the fact that
    /// dedicated keys (arrows, Home/End, PageUp/PageDown, the Bsp/Del keys)
    /// are untouched; only the letter-key Ctrl aliases move.
    ///
    /// One simplification: real nano picks Ctrl+H vs Ctrl+N for the "help"
    /// key via a terminfo query for what the terminal's actual Backspace
    /// key sends (falling back to Ctrl+N if it can't tell); this always
    /// uses Ctrl+H, correct for the overwhelming majority of terminals
    /// (which send a distinct byte for Backspace, not literal Ctrl+H).
    fn install_modern_overrides(&mut self) {
        use Action as A;
        use Key as K;
        let m = Menu::Main;
        let mut b = |k: Key, a: Action| self.bind(m, k, Binding::Action(a));

        b(K::Ctrl('H'), A::Help);
        b(K::Ctrl('G'), A::FindNext);
        b(K::Ctrl('D'), A::FindPrevious);
        b(K::Ctrl('Q'), A::Exit);
        b(K::Ctrl('W'), A::WriteOut);
        b(K::Ctrl('O'), A::Insert);
        b(K::Ctrl('R'), A::Replace);
        b(K::Ctrl('T'), A::GotoLine);
        b(K::Ctrl('P'), A::Location);
        b(K::Ctrl('Z'), A::Undo);
        b(K::Ctrl('Y'), A::Redo);
        b(K::Ctrl('A'), A::Mark);
        b(K::Ctrl('X'), A::Cut);
        b(K::Ctrl('C'), A::Copy);
        b(K::Ctrl('V'), A::Paste);
        b(K::Ctrl('E'), A::Execute);

        // nano's own help-key rebinding isn't Main-only: it applies via a
        // broad `(MMOST|MBROWSER) & ~MFINDINHELP` menu set — "an
        // abbreviation for all menus except Help and Browser and YesNo"
        // (src/definitions.h), plus Browser itself, minus the in-help
        // search submenu tico doesn't have.
        //
        // nano's own bare y/n/c confirmations have no help key at all in
        // *either* mode, which is why nano excludes MYESNO here — but
        // unlike nano, tico's shared prompt defaults give every non-Main
        // menu a working ^G Get Help, YesNo included (see the deconflict
        // screen's own help, which is entirely tico-original — nano has no
        // equivalent prompt to model this exclusion on). So here, YesNo is
        // treated the same as every other prompt menu instead: its
        // existing ^G moves to ^H too, rather than being stranded on the
        // old key while everything else switches.
        for &menu in Menu::ALL {
            if matches!(menu, Menu::Main | Menu::Help) {
                continue;
            }
            self.unbind(menu, K::Ctrl('G'));
            self.bind(menu, K::Ctrl('H'), Binding::Action(A::Help));
        }

        // The browser gets the modern set's ^Q/^D/^G in place of the
        // traditional ^X/^Q/^W/^P/^N/^Y/^V (and ^H for help, above).
        let br = Menu::Browser;
        for k in ['X', 'Q', 'W', 'P', 'N', 'Y', 'V'] {
            self.unbind(br, K::Ctrl(k));
        }
        self.bind(br, K::Ctrl('Q'), Binding::Action(A::Exit));
        self.bind(br, K::Ctrl('D'), Binding::Action(A::FindPrevious));
        self.bind(br, K::Ctrl('G'), Binding::Action(A::FindNext));

        // The help key toggles: pressing it again while help is open closes
        // it, same as nano's `add_to_sclist(MHELP, help_key, 0, do_exit)`.
        self.unbind(Menu::Help, K::Ctrl('G'));
        self.bind(Menu::Help, K::Ctrl('H'), Binding::Action(A::Cancel));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_key_and_literal_ctrl_h_both_work_by_default() {
        let km = KeyMap::defaults(false);
        assert_eq!(
            km.lookup(Menu::Main, Key::Backspace),
            Some(&Binding::Action(Action::Backspace))
        );
        assert_eq!(
            km.lookup(Menu::Main, Key::Ctrl('H')),
            Some(&Binding::Action(Action::Backspace))
        );
    }

    #[test]
    fn modern_bindings_repurpose_ctrl_h_for_help_but_keep_backspace_key_working() {
        let km = KeyMap::defaults(true);
        assert_eq!(
            km.lookup(Menu::Main, Key::Ctrl('H')),
            Some(&Binding::Action(Action::Help))
        );
        assert_eq!(
            km.lookup(Menu::Main, Key::Backspace),
            Some(&Binding::Action(Action::Backspace))
        );
    }

    #[test]
    fn modern_bindings_move_help_to_ctrl_h_in_prompt_menus_too() {
        // nano's help-key rebinding isn't Main-only: it applies to every
        // menu except Help and Browser (see src/definitions.h's MMOST;
        // nano additionally excludes YesNo there, but only because its own
        // bare y/n/c confirmations have no help key in either mode — tico
        // gives every prompt menu a working ^G by default, YesNo included,
        // so YesNo switches to ^H here too, for consistency).
        let km = KeyMap::defaults(true);
        for menu in [
            Menu::Search,
            Menu::Replace,
            Menu::ReplaceWith,
            Menu::GotoLine,
            Menu::WriteOut,
            Menu::Insert,
            Menu::Execute,
            Menu::YesNo,
        ] {
            assert_eq!(
                km.lookup_menu_only(menu, Key::Ctrl('H')),
                Some(&Binding::Action(Action::Help)),
                "{menu:?}"
            );
            assert_ne!(
                km.lookup_menu_only(menu, Key::Ctrl('G')),
                Some(&Binding::Action(Action::Help)),
                "{menu:?} should no longer use ^G for help"
            );
        }
    }

    #[test]
    fn modern_bindings_remap_the_documented_set() {
        // Spot-check against nano's ISSET(MODERN_BINDINGS) branch in
        // src/global.c, not exhaustive.
        let km = KeyMap::defaults(true);
        let cases = [
            (Key::Ctrl('G'), Action::FindNext),
            (Key::Ctrl('Q'), Action::Exit),
            (Key::Ctrl('W'), Action::WriteOut),
            (Key::Ctrl('O'), Action::Insert),
            (Key::Ctrl('R'), Action::Replace),
            (Key::Ctrl('T'), Action::GotoLine),
            (Key::Ctrl('X'), Action::Cut),
            (Key::Ctrl('C'), Action::Copy),
            (Key::Ctrl('V'), Action::Paste),
            (Key::Ctrl('Z'), Action::Undo),
            (Key::Ctrl('Y'), Action::Redo),
            (Key::Ctrl('A'), Action::Mark),
            (Key::Ctrl('E'), Action::Execute),
        ];
        for (key, action) in cases {
            assert_eq!(
                km.lookup(Menu::Main, key),
                Some(&Binding::Action(action)),
                "{key:?}"
            );
        }
    }

    #[test]
    fn non_modern_keeps_traditional_bindings() {
        let km = KeyMap::defaults(false);
        assert_eq!(
            km.lookup(Menu::Main, Key::Ctrl('G')),
            Some(&Binding::Action(Action::Help))
        );
        assert_eq!(
            km.lookup(Menu::Main, Key::Ctrl('X')),
            Some(&Binding::Action(Action::Exit))
        );
        assert_eq!(
            km.lookup(Menu::Main, Key::Ctrl('W')),
            Some(&Binding::Action(Action::WhereIs))
        );
    }

    #[test]
    fn key_parse_recognizes_bsp() {
        assert_eq!(Key::parse("Bsp"), Some(Key::Backspace));
        assert_eq!(Key::parse("bsp"), Some(Key::Backspace));
    }

    #[test]
    fn buffer_switch_uses_the_shifted_m_less_greater_keys_not_unshifted_comma_period() {
        // Confirmed against the installed nano: M-, and M-. do NOT switch
        // buffers there -- only the shifted M-< / M-> (plus the M-Left /
        // M-Right arrow-key aliases) do.
        let km = KeyMap::defaults(false);
        assert_eq!(
            km.lookup(Menu::Main, Key::Meta('<')),
            Some(&Binding::Action(Action::PrevBuf))
        );
        assert_eq!(
            km.lookup(Menu::Main, Key::Meta('>')),
            Some(&Binding::Action(Action::NextBuf))
        );
        assert_eq!(km.lookup(Menu::Main, Key::Meta(',')), None);
        assert_eq!(km.lookup(Menu::Main, Key::Meta('.')), None);
    }
}
