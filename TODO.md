# TODO

Features nano 8.7.1 has that tico does not yet. Remove an item once it is
implemented (and, where it applies, its "not yet implemented" status
message and test in `src/app.rs` / `src/ui.rs`).

Syntax highlighting is intentionally *not* nano-compatible; see
`DIFFERENCES.md`. Nothing about nanorc `color`/`syntax` directives belongs
here.

## Bound keys that report "not yet implemented"

- [ ] Top/bottom row of screen (`M-Home` / `M-End`)
- [ ] Anchors: set, previous, next (`M-Ins` / `M-"`, `M-PgUp`, `M-PgDn` / `M-'`)
- [ ] Macros: record and replay (`M-:` / `M-;`)

## Options parsed but never consulted

### Display

- [ ] `softwrap` — toggle flips the flag, nothing wraps
- [ ] `stateflags`
- [ ] `bookstyle`
- [ ] `emptyline`
- [ ] `rawsequences`
- [ ] `atblanks`
- [ ] `afterends`

### Editing

- [ ] `breaklonglines` — the `M-L` toggle flips the flag; typing never
      hard-wraps
- [ ] `nowrap` (nano's legacy alias for `unset breaklonglines`)
- [ ] `wordchars` / `wordbounds` (word completion honors `wordchars`;
      word movement and deletion consult neither)
- [ ] `zap` (the setting: Backspace/Delete erase the marked region)
- [ ] `colonparsing`
- [ ] `rebinddelete`
