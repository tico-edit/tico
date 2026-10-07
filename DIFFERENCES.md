# Known differences from nano

tico aims to match GNU nano's behavior, verified against the installed
`nano` binary and its source. This file records the places where it
knowingly does not. These are either deliberate design choices or
accepted limitations, not work that is planned: unimplemented features
live in `TODO.md`. When a difference here is later removed, delete its
entry.

## Syntax highlighting is not nano's

tico highlights with tree-sitter grammars (`src/syntax/`) rather than
nano's regex-based `color`/`icolor` engine. nanorc's highlighting
directives — `syntax`, `color`, `icolor`, `header`, `magic`, `include`,
`extendsyntax`, and the per-syntax `formatter`/`linter`/`comment`/
`tabgives` lines — are parsed so a real-world nanorc still loads, but
have no effect. The built-in language table in `src/syntax/languages.rs`
supplies what those per-syntax lines would have (the linter/formatter
commands and the `M-3` comment sequence), sourced from nano's shipped
syntax files where nano has one. Language detection is likewise built
in: filename, then shebang, then a vim/Emacs modeline, then a peek at
the leading lines (a `.conf` file that opens with `{ "` or a JSON array
is highlighted as JSON, one that opens with `---` or `%YAML` as YAML,
a `--- `/`+++ ` pair as a diff, and a `.inc` file that opens with a
`{$...}` directive, a Pascal comment, a section header or a routine
header as Pascal), which stands in for nano's `header` and `magic` lines
but is not configurable.

Colors come from a theme in Helix's format (`src/theme.rs`), selected in
`~/.ticorc` or with `--tico-theme`, and only the syntax scopes of a theme
are honored. A theme's `ui.*` entries are ignored: bars, line numbers and
the selection are nano's domain (`set titlecolor` and friends; see
`TODO.md` for which of those tico consults so far).

Consequences worth knowing:

- A user's custom `syntax`/`color` definitions do nothing, and there is
  no way to add highlighting for a language without adding a tree-sitter
  grammar to tico itself.
- nano's per-syntax `tabgives` string cannot override what `M-}` indents
  with; tico always uses a tab, or `tabsize` spaces under `tabstospaces`.
- `M-3` cannot comment a file whose syntax only a nanorc defines; a
  buffer with no recognized language uses nano's general default, `#`.

The trade is deliberate: tree-sitter parses the language rather than
pattern-matching lines, so highlighting is accurate across multi-line
constructs and heredocs, and any Helix theme works unmodified.

## Title bar: `[x/y]` with multiple buffers, and "View"

When more than one buffer is open, nano replaces the version text in the
upper-left corner with the `[current/total]` buffer indicator. tico keeps
`tico VERSION` on the left and puts `[x/y]` in the upper-right corner
instead, so neither is lost. `--view`'s "View" marker shares that corner,
and the two show together when both apply.

## Suspend: no SIGTSTP/SIGCONT handling

`^T^Z` suspends exactly as nano does: the terminal is restored and the
whole process group is stopped with SIGSTOP, and `fg` resumes in place.
But nano also installs SIGTSTP and SIGCONT handlers so that a stop sent
from outside (`kill -TSTP`, or a shell or multiplexer stopping the job)
restores the terminal first and re-initializes it on resume. tico has no
SIGTSTP/SIGCONT handlers, so an externally sent SIGTSTP stops it with the terminal
still in raw mode and on the alternate screen.

A typed `^Z` is unaffected: both editors disable the terminal's ISIG in
raw mode, so it arrives as a keystroke (nano's "To suspend, type ^T^Z"
hint, or the Execute menu's suspend) rather than as a signal.

On Windows there is no SIGSTOP/process-group job control to hand off to
a shell at all, so `^T^Z` there just reports "Could not suspend:
suspend is not supported on this platform" and leaves the process
running.

## Cancelling a command with `^C`: not on Windows

While an Execute command (`^T`) runs, `^C` cancels it exactly as in
nano: the terminal's ISIG is turned back on for the duration and a
SIGINT handler SIGKILLs the command, which is then reported as
"Cancelled" and has its changes undone. As in nano, the SIGINT the
terminal generates goes to tico's whole foreground process group. Run
from an interactive shell that is just tico and the command, but when
tico is started from a non-interactive wrapper sharing its process
group (`sh -c 'tico; ...'`), that wrapper receives the SIGINT too and
abandons the rest of its script.

On Windows none of this is set up (`src/interrupt.rs` is Unix-only), so
`^C` cannot cancel a running command there; tico waits until the
command finishes on its own.

## `set preserve`: no effect on Windows

`preserve` (`-p`) turns the terminal's XON/XOFF flow control back on, so
`^S` and `^Q` stop and resume output instead of reaching tico, as in
nano. Windows consoles have no XON/XOFF, so there it only unbinds the
keys (with `-p`); `^S` and `^Q` still arrive as ordinary keystrokes.

## Help listing order and contents

nano's `^G` help lists a menu's functions in its fixed registration
order, and lists a function even when it has no key in that menu (in the
main menu, Suspend appears with an empty key column). tico builds the
listing from the live keymap, so only bound actions appear, sorted by
description rather than by nano's order.

## File browser: Esc, unbound keys, and the shortcut bar

In the file browser (`^T` at the Read File / Write Out prompts), Esc
leaves the browser, the same as it cancels any tico prompt; in nano a
lone Esc just starts a Meta/Escape sequence. An unbound key reports
"Unbound key: KEY" using tico's own key names (`Left`, `^Up`, ...)
rather than nano's arrow glyphs. The bottom bar lists the same entries
in the same order as nano's (capped at the count nano would show for
the screen width), but laid out with tico's usual column sizing, so at
80 columns it shows eight of nano's twelve.

## Comment toggle: cursor after removing a postfix

For a bracketing comment sequence such as HTML's `<!--|-->`, uncommenting
a line with the cursor near its end can leave the cursor past the new
line end once the postfix is gone. nano leaves that stale column in
place; tico clamps the cursor (and mark) to the end of the line.

## Mac (bare-CR) line endings are kept on purpose

nano removed Mac format -- reading and writing files whose lines end in
a bare CR, the `M-M Mac Format` toggle at the Write Out prompt, and the
"converted from Mac format" message -- from its master branch in April
2026, so nano releases after 8.7.1 have only Unix and DOS. tico keeps
it: vintage machines still produce and consume such files, and tico is
used with them. Do not remove Mac format to track newer nano; when the
installed nano no longer has it, this becomes an intentional difference
rather than a bug.

## Verbatim input (`M-V`): special keys are re-encoded

nano reads the raw bytes a keystroke sends and inserts those. tico's
terminal layer hands it decoded keys instead, so for a key that sends an
escape sequence (arrows, Home/End, Insert/Delete, PageUp/PageDown, F-keys,
Shift-Tab) tico inserts the sequence an xterm-style terminal sends for that
key in normal cursor mode -- `^[[A` for Up, `^[[1;5A` for Ctrl+Up,
`^[[15~` for F5. That matches nano wherever the terminal agrees with
xterm, but not where it doesn't: under tmux or screen, for example, Home
sends `^[[1~`, which nano inserts and tico renders as `^[[H`. Plain
characters, control codes, Tab, Enter, Backspace, Esc and Alt-combinations
come out exactly as in nano, as does Unicode input by hex code.
