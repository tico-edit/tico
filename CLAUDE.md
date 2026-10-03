# tico

A nano-compatible TUI text editor written in Rust. tico matches GNU nano's
keybindings, configuration, and on-screen behavior as closely as practical,
verified against the installed `nano` binary and the nano source (checked
out separately, if available, for reference) rather than assumptions.

`TODO.md` lists nano features tico does not have yet; `DIFFERENCES.md`
records the known, intentional differences from nano. Keep both current:
remove a TODO entry when its feature lands, and add a DIFFERENCES entry
whenever a deliberate deviation is introduced.

## Keep the website's configuration doc in sync

The project website (`ticoedit.org`) has `docs/configuration.md`
(built to `docs/configuration.html` by that repo's `./build.pl`)
documenting every nanorc/ticorc configuration item -- every `set`/`unset`
name, `~/.ticorc`'s sections, and the built-in themes -- in tico's own
words rather than by pointing at nano's docs. When a change here adds,
removes, renames, or changes the behavior of one of those (including
moving something from "parsed but not yet consulted" to actually working,
or adding/renaming a built-in theme), update that page to match, and
rebuild it. On this system that website repo is checked out at
`~/www/ticoedit.org`; if it isn't present, say the config doc needs a
follow-up update instead of silently skipping it.

## Never commit or push without explicit instructions

Do not run `git commit` or `git push` (or anything else that lands work in
git history or a shared remote) unless the user explicitly asks for it in
that message — e.g. "commit and push", "commit this". Being asked to
implement or fix something is not permission to commit or push it, even
when the change is small and obviously correct; leave the working tree as
it is and say what's ready. This applies equally when the trigger for the
work was a bug report, a pasted error, or a failing CI link — investigate
and fix, but don't commit/push on your own judgment. Offering to commit
("want me to commit this?") is fine; proceeding without a yes is not.

## Never merge a pull request

Do not merge a pull request (`gh pr merge`, the GitHub web UI, or any
other route) — not even if explicitly asked to. If asked to merge a PR,
say that this is intentionally outside what you'll do here and that the
user needs to merge it themselves. This one holds even with a direct,
explicit instruction to merge; it isn't a "confirm once" situation like
commit/push above.

## Every package ships every binary

This repo builds three binaries: `tico`, `tcat`, `ttee`. Whenever a
release-worthy package is produced, all three must be in it — not just
`tico`. That currently means two separate places, both of which need
updating together when a new binary is added:

- `Cargo.toml`'s `[package.metadata.deb]` `assets` list (drives the `.deb`
  built by `cargo-deb`).
- `.github/workflows/release.yml`'s `extra-bin-names` input (space-separated,
  alongside the implicit primary `bin-name`), which drives the tar.gz/zip
  archives and the Windows NSIS installer via the reusable
  `uperl/action-rust-release-bin` workflow.

`extra-bin-names` was added to that shared action specifically to support
this (see its git history) — it doesn't need `--bin`-restricted builds or
per-target special-casing, since `cargo build --release` already builds
every binary in the crate in one pass. If you add a fourth binary, add it
to both lists; don't let a new binary end up in `.deb` but missing from
the tarball/zip/installer, or vice versa.

The same two-places rule applies to license files. Every package must
carry both `LICENSE` (MIT, tico itself) and `LICENSE-MPL-2.0` (the
vendored Helix themes): they're listed in the `.deb` `assets` and in the
workflow's `extra-files` input (which replaces the action's default of
`README.md LICENSE`, so keep `README.md` and `LICENSE` in it too).

## Keep Mac (bare-CR) line-ending support

nano dropped Mac format (bare-CR line endings, the `M-M Mac Format`
toggle, "converted from Mac format") after 8.7.1. tico deliberately keeps
it because the user works with vintage computers that still use it. Do
not remove it to match a newer nano, and don't treat its presence as a
nano-compatibility bug; see `DIFFERENCES.md`. Note that the nano source
checkout under `~/dev/nano` is git master, which is newer than the
installed 8.7.1 binary -- when they disagree, the installed binary is the
reference, and `git show v8.7.1:src/...` gives the matching source.

## Syntax highlighting: an intentional exception to nano-compatibility

Everywhere else, the goal is to match nano's real behavior. Syntax
highlighting is a deliberate exception:

- tico implements its own syntax highlighting using **tree-sitter**
  grammars (`src/syntax/`), not nano's regex-based `color`/`icolor` engine.
- nanorc's syntax-highlighting directives — `syntax`, `color`, `icolor`,
  `header`, `magic`, `include`, `extendsyntax`, and the per-syntax
  `formatter`/`linter`/`comment`/`tabgives` lines inside a `syntax` block —
  are **parsed but ignored** (see `SYNTAX_BODY_COMMANDS` in
  `src/config/nanorc.rs`). They're recognized only so the rest of a
  real-world `~/.nanorc`/`/etc/nanorc` still parses correctly; a user's
  custom syntax/color definitions have no effect in tico.
- Do not implement nanorc `color`/`icolor` parsing to actually drive
  highlighting, and do not treat "nanorc syntax directives don't work" as a
  bug to fix. If asked to make nanorc-defined highlighting work, point back
  to this file rather than implementing it.
- The built-in language list, including any per-language `linter`/
  `formatter` defaults, lives in `src/syntax/languages.rs` as a
  `LanguageDef` table (tree-sitter grammar + highlight query + optional
  linter/formatter command), sourced from nano's own shipped nanorc files
  where applicable (see git history for `justify`/`Speller-Formatter-
  Linter-Execute` work). Adding support for a new language means adding an
  entry there and a corresponding `.scm` highlight query under
  `src/syntax/queries/`, not adding nanorc color-directive support.
- Syntax colors come from a **theme in Helix's theme format**
  (`src/theme.rs`): TOML keyed by dotted scope names (`keyword.control.import`,
  `constant.numeric`, ...) resolved by longest prefix, with `inherits` and
  `[palette]` support, so any Helix theme file works unmodified. Nine
  themes are compiled into the binary from `themes/*.toml` (`BUILTIN_THEMES`
  in `src/theme.rs`), all named with the reserved `tico-builtin-` prefix:
  tico's own 16-color `tico-builtin-default` plus eight vendored from
  Helix (MPL-2.0 — keep their headers, and record any addition in
  `themes/README.md`, `README.md` and the `Cargo.toml` license comment).
  Any other name is looked up on disk: `~/.config/tico/themes/`, then an
  installed Helix's theme directories. Selection is in `~/.ticorc`'s
  `[syntax]` section — `theme = NAME` globally, `LANGUAGE.theme = NAME`
  per language (`perl.theme = tico-builtin-nord`) — or `--tico-theme NAME` / `--tico-theme LANG.NAME` (repeatable);
  `Editor::theme_for(lang)` is the one place that resolves which applies.
  `--tico-list-themes` lists what's available and summarizes the active configuration. Only syntax scopes are honored;
  a theme's `ui.*` entries are parsed but ignored, since bars/line
  numbers/selection follow nano's `set titlecolor` & co.
- Consequently the highlighter's output is Helix's scope vocabulary, not
  an internal enum: `syntax::HighlightSpan` carries an interned `Scope`,
  and `normalize_capture()` in `src/syntax/mod.rs` translates each
  vendored query's capture names (a mix of tree-sitter-CLI and
  nvim-treesitter conventions) into Helix scope names. When adding a
  language, run the tests: `all_captures_normalize_into_helix_scopes`
  fails on any capture name that doesn't land in a Helix top-level scope,
  and the fix is a new alias in `normalize_capture()`, not a new color.
