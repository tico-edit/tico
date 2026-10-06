# TODO

Features nano 8.7.1 has that tico does not yet. Remove an item once it is
implemented (and, where it applies, its "not yet implemented" status
message and test in `src/app.rs` / `src/ui.rs`).

Syntax highlighting is intentionally *not* nano-compatible; see
`DIFFERENCES.md`. Nothing about nanorc `color`/`syntax` directives belongs
here.

## Bound keys that report "not yet implemented"

- [ ] Word completion (`^]`)
- [ ] Block navigation: previous/next block (`^Up` / `^Down`, `M-7` / `M-8`)
- [ ] Top/bottom row of screen (`M-Home` / `M-End`)
- [ ] Find matching bracket (`M-]`)
- [ ] Anchors: set, previous, next (`M-Ins` / `M-"`, `M-PgUp`, `M-PgDn` / `M-'`)
- [ ] Macros: record and replay (`M-:` / `M-;`)
- [ ] Pipe Text (`M-\`) at the Execute prompt
- [ ] Cut Till End (`^V`) from the Execute menu
- [ ] Full Justify (`^J`) from the Execute menu — main-menu `M-J` already
      works; this path is stubbed separately in `src/ui.rs`
- [ ] Verbatim input (`M-V`)
- [ ] Center (`^L`) / Cycle (`M-%`)
- [ ] Write Out prompt toggles: Append (`M-A`), Prepend (`M-P`)

## Options parsed but never consulted

### Display

- [ ] `softwrap` — toggle flips the flag, nothing wraps
- [ ] `constantshow`
- [ ] `matchbrackets`
- [ ] `stateflags`
- [ ] `bookstyle`
- [ ] `jumpyscrolling`
- [ ] `emptyline`
- [ ] `rawsequences`
- [ ] `atblanks`
- [ ] `afterends`

### Files and safety

- [ ] `positionlog` (no position-log machinery exists yet)
- [ ] `restricted` (which, among other things, should hide the file
      browser's `^T`)
- [ ] `operatingdir` (which should also confine the file browser)

### Editing

- [ ] `wordchars` / `wordbounds`
- [ ] `zap` (the setting: Backspace/Delete erase the marked region)
- [ ] `colonparsing`
- [ ] `rebinddelete`
- [ ] `preserve`
