# Vendored tree-sitter highlight queries

One `highlights.scm` per supported language, referenced from the
`LanguageDef` table in `../languages.rs`. They're vendored rather than
pulled from each `tree-sitter-*` crate at run time because the crates
expose them inconsistently (differently named constants, sometimes not at
all); see the module docs in `../mod.rs`.

Capture names in these files are a mix of conventions (tree-sitter CLI,
nvim-treesitter, grammar-specific); `normalize_capture()` in `../mod.rs`
translates them all into Helix's scope vocabulary, which is what themes are
written against. When adding a query, run the tests: any capture that
doesn't land in a Helix top-level scope fails
`all_captures_normalize_into_helix_scopes`.

## Provenance and licensing

| File | Source | License |
|---|---|---|
| `perl.scm` | The `ts-parser-perl` crate (v2.0.0, [tree-sitter-perl/tree-sitter-perl](https://github.com/tree-sitter-perl/tree-sitter-perl)) `queries/highlights.scm`, with the changes noted in its header | MIT |
| `diff.scm` | [Helix](https://github.com/helix-editor/helix) `runtime/queries/diff/highlights.scm`, commit `737ab17` | **MPL-2.0** — see `LICENSE-MPL-2.0` in the repository root |
| `vcl.scm` | Started from [ntsk/tree-sitter-vcl](https://github.com/ntsk/tree-sitter-vcl) v0.4.1 `queries/highlights.scm`, extended for the Fastly constructs in tico's fork of the grammar (`grammars/tree-sitter-vcl/`) | MIT (upstream's and tico's) |
| `groovy.scm` | The [dekobon fork of tree-sitter-groovy](https://github.com/dekobon/tree-sitter-groovy) (the `dekobon-tree-sitter-groovy` crate, not `tree-sitter-groovy` — that one's crates.io package omits its `highlights.scm`), commit `436a405`, `queries/groovy/highlights.scm`, unmodified | MIT (upstream dual-licenses MIT/Apache-2.0; used here under the MIT option) |
| `tcl.scm` | [tree-sitter-grammars/tree-sitter-tcl](https://github.com/tree-sitter-grammars/tree-sitter-tcl) commit `850a72a`, `queries/tcl/highlights.scm`, with the stacked fallback captures dropped as noted in its header (the grammar is a git dependency, not a crate) | MIT |
| `dockerfile.scm` | The `tree-sitter-containerfile` crate (v0.9.2, [wharflab/tree-sitter-containerfile](https://github.com/wharflab/tree-sitter-containerfile), a maintained fork of camdencheek's tree-sitter-dockerfile) `queries/highlights.scm`, unmodified | MIT |
| `tt2.scm` | The [Template Toolkit Zed extension](https://github.com/RuvimSypa/template-toolkit-zed), commit `93854ef`, `languages/template-toolkit/highlights.scm`, unmodified (the grammar itself is vendored under `grammars/tree-sitter-template-toolkit/`) | MIT |
| `batch.scm` | The `tree-sitter-batch` crate (v0.11.1, [wharflab/tree-sitter-batch](https://github.com/wharflab/tree-sitter-batch)) `queries/highlights.scm`, with two patterns reordered as noted in its header | MIT |
| `cue.scm` | [eonpatapon/tree-sitter-cue](https://github.com/eonpatapon/tree-sitter-cue) commit `dd7b90e`, `queries/highlights.scm`, with two patterns moved earlier as noted in its header (the grammar itself is vendored under `grammars/tree-sitter-cue/`) | MIT |
| `pascal.scm` | [Helix](https://github.com/helix-editor/helix) `runtime/queries/pascal/highlights.scm`, commit `33c18b3`, reordered and extended as noted in its header (the grammar is tico's fork under `grammars/tree-sitter-pascal/`) | **MPL-2.0** — see `LICENSE-MPL-2.0` in the repository root |
| `hcl.scm` | [Helix](https://github.com/helix-editor/helix) `runtime/queries/hcl/highlights.scm`, commit `90b126d` (the `tree-sitter-hcl` crate ships no query) | **MPL-2.0** — see `LICENSE-MPL-2.0` in the repository root |
| `v.scm` | [vlang/v-analyzer](https://github.com/vlang/v-analyzer) commit `925d457` (the revision the `tree-sitter-vlang` crate pins), `tree_sitter_v/queries/helix.highlights.scm`, reordered and adjusted as noted in its header (the crate ships no query) | MIT |
| everything else | The corresponding `tree-sitter-<lang>` crate's `queries/highlights.scm`, unmodified except where noted in a file's header | That grammar's license (MIT for all current ones) |

`diff.scm` and `hcl.scm` are the only files here not under tico's MIT
license. MPL-2.0 is a file-scoped copyleft: each file must stay under
MPL-2.0 and carry its notice (both do, in their header comments), but
that has no effect on the license of the rest of tico.
