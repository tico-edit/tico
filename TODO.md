# TODO

Features nano 8.7.1 has that tico does not yet. Remove an item once it is
implemented (and, where it applies, its "not yet implemented" status
message and test in `src/app.rs` / `src/ui.rs`).

Syntax highlighting is intentionally *not* nano-compatible; see
`DIFFERENCES.md`. Nothing about nanorc `color`/`syntax` directives belongs
here.

## Options parsed but never consulted

### Editing

- [ ] `colonparsing`
- [ ] `rebinddelete`
