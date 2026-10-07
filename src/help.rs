//! Text for the `^G` help viewer (`Action::Help`). Each menu context gets
//! its own introductory blurb — most notably, Search and Replace have
//! their own (nano gives the search-term prompt and the replacement-term
//! prompt separate help screens, and so does tico) — followed by a
//! shortcut listing generated straight from the live `KeyMap`, so it
//! automatically reflects whatever `bind`/`unbind` a nanorc/ticorc applied
//! rather than a hardcoded table like nano's own (per-function) phrases.
//!
//! Intro wording is adapted from nano's own help text (src/help.c), which
//! is itself the closest thing to a spec for what each prompt does.

use crate::keymap::{Action, Binding, Key, KeyMap, Menu};
use std::collections::BTreeMap;

/// Build the help screen for `menu`. Element 0 is the title, shown on its
/// own reverse-video row and never scrolled; the rest is the scrollable
/// body, already word-wrapped to `width` columns.
/// `restricted` leaves out what nano's restricted mode doesn't offer (see
/// `keymap::hidden_when_restricted`).
pub fn build(menu: Menu, keymap: &KeyMap, width: usize, restricted: bool) -> Vec<String> {
    let width = width.max(20);
    let (title, paragraphs) = intro_for(menu);

    let mut lines = vec![title.to_string()];
    lines.push(String::new());
    for para in paragraphs {
        lines.extend(wrap(para, width.saturating_sub(1)));
        lines.push(String::new());
    }
    lines.extend(shortcut_lines(menu, keymap, restricted));
    lines
}

/// Help for the "file changed on disk, you have unsaved edits" choice
/// prompt. This one's bespoke, not built from `intro_for`/`shortcut_lines`
/// like the rest: it's tico-original (no nano equivalent), and its R/K/M/C
/// responses are raw keystrokes the prompt matches directly rather than
/// rebindable keymap actions, so there'd be nothing for the usual
/// keymap-driven shortcut listing to draw from.
pub fn build_conflict_help(width: usize) -> Vec<String> {
    let width = width.max(20).saturating_sub(1);
    let mut lines = vec!["File Changed On Disk Help Text".to_string(), String::new()];
    for para in [
        "tico detected that the file's contents on disk no longer match what \
         was last loaded or saved, and this buffer also has edits of its own \
         that haven't been saved. Reloading would discard your edits; saving \
         would discard the on-disk change; so tico asks which you want.",
        "[R]eload discards your local edits and loads the file's current \
         on-disk contents.",
        "[K]eep mine ignores the on-disk change and keeps editing as if it \
         hadn't happened. tico won't ask again about this particular change \
         (a further change to the file will still prompt).",
        "[M]erge previews a three-way merge of both sets of changes in a \
         scrollable diff screen, which you can then apply or back out of \
         (back out returns here).",
        "[I]gnore All stops checking this file for on-disk changes \
         entirely, for the rest of this session -- unlike Keep, which only \
         silences the one change already showing, a further change to the \
         file won't prompt either.",
    ] {
        lines.extend(wrap(para, width));
        lines.push(String::new());
    }
    lines
}

/// (title, body paragraphs) for each menu — the last paragraph always ends
/// by introducing the shortcut listing that `build` appends after it.
fn intro_for(menu: Menu) -> (&'static str, &'static [&'static str]) {
    match menu {
        Menu::Search | Menu::Replace => (
            "Search Command Help Text",
            &[
                "Enter the words or characters you would like to search for, and \
                 then press Enter. If there is a match for the text you entered, \
                 the screen will be updated to the location of the nearest match \
                 for the search string.",
                "The previous search string will be shown in brackets after the \
                 search prompt. Hitting Enter without entering any text will \
                 perform the previous search. If you have selected text with the \
                 mark and then search to replace, only matches in the selected \
                 text will be replaced.",
                "The following function keys are available in Search mode:",
            ],
        ),
        Menu::ReplaceWith => (
            "=== Replacement ===",
            &[
                "Type the characters that should replace what you typed at the \
                 previous prompt, and press Enter.",
                "The following function keys are available at this prompt:",
            ],
        ),
        Menu::GotoLine => (
            "Go To Line Help Text",
            &[
                "Enter the line number that you wish to go to and hit Enter. If \
                 there are fewer lines of text than the number you entered, you \
                 will be brought to the last line of the file.",
                "The following function keys are available in Go To Line mode:",
            ],
        ),
        Menu::Insert => (
            "Insert File Help Text",
            &[
                "Type in the name of a file to be inserted into the current file \
                 buffer at the current cursor location.",
                "If you need another blank buffer, do not enter any filename, or \
                 type in a nonexistent filename at the prompt and press Enter.",
                "The following function keys are available in Insert File mode:",
            ],
        ),
        Menu::WriteOut => (
            "Write File Help Text",
            &[
                "Type the name that you wish to save the current file as and \
                 press Enter to save the file.",
                "If you have selected text with the mark, you will be prompted to \
                 save only the selected portion to a separate file. To reduce the \
                 chance of overwriting the current file with just a portion of \
                 it, the current filename is not the default in this mode.",
                "The following function keys are available in Write File mode:",
            ],
        ),
        Menu::Browser => (
            "File Browser Help Text",
            &[
                "The file browser is used to visually browse the directory \
                 structure to select a file for reading or writing. You may use \
                 the arrow keys or Page Up/Down to browse through the files, and \
                 S or Enter to choose the selected file or enter the selected \
                 directory. To move up one level, select the directory called \
                 \"..\" at the top of the file list.",
                "The following function keys are available in the file browser:",
            ],
        ),
        Menu::WhereIsFile => (
            "Browser Search Command Help Text",
            &[
                "Enter the words or characters you would like to search for, and \
                 then press Enter. If there is a match for the text you entered, \
                 the screen will be updated to the location of the nearest match \
                 for the search string.",
                "The previous search string will be shown in brackets after the \
                 search prompt. Hitting Enter without entering any text will \
                 perform the previous search.",
                "The following function keys are available at this prompt:",
            ],
        ),
        Menu::GotoDir => (
            "Browser Go To Directory Help Text",
            &[
                "Enter the name of the directory you would like to browse to.",
                "The following function keys are available in Browser Go To \
                 Directory mode:",
            ],
        ),
        Menu::Spell => (
            "=== Spelling correction ===",
            &[
                "The spell checker has examined the spelling of all text in the \
                 current buffer or marked region. An unknown word has been \
                 encountered -- it is highlighted and a replacement can now be \
                 edited. After this you will be asked whether to replace each \
                 instance of that unknown word.",
                "The following function keys are available at this prompt:",
            ],
        ),
        Menu::Execute => (
            "Execute Command Help Text",
            &[
                "This mode allows you to insert the output of a command run by \
                 the shell into the current buffer (or into a new buffer). If \
                 the command is preceded by '|' (the pipe symbol), the current \
                 contents of the buffer (or marked region) will be piped to the \
                 command.",
                "If you just need another blank buffer, do not enter any \
                 command. You can also pick one of four tools, or cut a large \
                 piece of the buffer, or put the editor to sleep.",
                "The following function keys are available at this prompt:",
            ],
        ),
        Menu::Linter => (
            "=== Linter ===",
            &[
                "In this mode, the status bar shows an error message or warning, \
                 and the cursor is put at the corresponding position in the \
                 file. With PageUp and PageDown you can switch to earlier and \
                 later messages.",
                "The following function keys are available in Linter mode:",
            ],
        ),
        Menu::Main => (
            "Main tico help text",
            &[
                "tico is designed to be compatible with GNU nano's keybindings, \
                 configuration, and on-screen layout. There are four main \
                 sections of the editor. The top line shows the program \
                 version, the current filename being edited, and whether or not \
                 the file has been modified. Next is the main editor window \
                 showing the file being edited. The status line is the third \
                 line from the bottom and shows important messages.",
                "The bottom two lines show the most commonly used shortcuts in \
                 the editor. Shortcuts are written as follows: Control-key \
                 sequences are notated with a '^' and can be entered either by \
                 using the Ctrl key or pressing the Esc key twice. Meta-key \
                 sequences are notated with 'M-' and can be entered using \
                 either the Alt, Cmd, or Esc key, depending on your keyboard \
                 setup.",
                "The following keystrokes are available in the main editor \
                 window:",
            ],
        ),
        // YesNo and anything else not called out above: a generic screen
        // (still populated with that menu's real bindings below).
        _ => (
            "Help Text",
            &["The following function keys are available at this prompt:"],
        ),
    }
}

/// Word-wrap `text` (a single paragraph — internal newlines aren't
/// expected) to `width` columns, breaking only at whitespace.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let extra = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if extra > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// One formatted line per action bound in `menu` ("^G     (F1)      Display
/// this help text"), built from the live keymap rather than a static
/// table — grouping every key bound to the same action onto one line, the
/// way nano shows a primary key plus its alternates in parentheses.
fn shortcut_lines(menu: Menu, keymap: &KeyMap, restricted: bool) -> Vec<String> {
    let mut by_description: BTreeMap<&'static str, Vec<Key>> = BTreeMap::new();
    for ((m, key), binding) in keymap.entries() {
        if *m != menu {
            continue;
        }
        if let Binding::Action(action) = binding {
            // nano's main-menu ^Z entry (the "type ^T^Z" hint) isn't a
            // listed function there either.
            if *action == Action::SuggestSuspend {
                continue;
            }
            // Nor is Enter, anywhere but the main menu: it's how a prompt
            // (or the browser's selection) is accepted, not a function.
            if *action == Action::Enter && menu != Menu::Main {
                continue;
            }
            // `M-V` works at the prompts too, but nano lists it only in
            // the main menu's help.
            if *action == Action::Verbatim && menu != Menu::Main {
                continue;
            }
            if restricted && crate::keymap::hidden_when_restricted(menu, *action) {
                continue;
            }
            by_description
                .entry(action.description_in(menu))
                .or_default()
                .push(*key);
        }
    }

    let mut rows: Vec<(String, &'static str)> = by_description
        .into_iter()
        .map(|(desc, mut keys)| {
            keys.sort_by_key(|k| k.display_rank());
            let primary = keys[0].describe();
            let key_col = if keys.len() > 1 {
                let alts: Vec<String> = keys[1..].iter().map(Key::describe).collect();
                format!("{primary}     ({})", alts.join(", "))
            } else {
                primary
            };
            (key_col, desc)
        })
        .collect();
    // No binding order is inherently "correct" here (unlike nano's fixed
    // function-registration order); sort by description so the listing is
    // at least stable and alphabetically browsable.
    rows.sort_by_key(|(_, desc)| *desc);

    // Always at least one space before the description, even when the key
    // column (a key plus a long list of alternates) overruns the normal
    // 17-column field.
    rows.into_iter()
        .map(|(key_col, desc)| format!("{key_col:<17} {desc}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeyMap;

    #[test]
    fn search_and_replace_get_distinct_intros() {
        let km = KeyMap::defaults(false);
        let search = build(Menu::Search, &km, 80, false);
        let replace_with = build(Menu::ReplaceWith, &km, 80, false);
        assert_eq!(search[0], "Search Command Help Text");
        assert_eq!(replace_with[0], "=== Replacement ===");
        assert_ne!(search[0], replace_with[0]);
    }

    #[test]
    fn shortcut_listing_reflects_live_keymap() {
        let km = KeyMap::defaults(false);
        let lines = build(Menu::Main, &km, 80, false);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("^G") && l.contains("Display this help text"))
        );
        assert!(lines.iter().any(|l| l.contains("Search forward")));
        assert!(
            !lines.iter().any(|l| l.contains("Say how to suspend")),
            "nano's main help doesn't list the ^Z hint entry"
        );
    }

    #[test]
    fn wrapping_respects_width() {
        // The shortcut-listing lines (key column + description) are a
        // separate, deliberately un-wrapped format — like nano's own, they
        // just get truncated at render time in a narrow terminal — so this
        // only checks the word-wrapped intro paragraphs.
        let wrapped = wrap(
            "This is a long sentence that should be wrapped across several lines \
             once it exceeds the requested width.",
            20,
        );
        for line in &wrapped {
            assert!(line.chars().count() <= 20, "line exceeds width: {line:?}");
        }
        assert!(wrapped.len() > 1);
    }
}
