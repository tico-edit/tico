# tico

A friendly terminal text editor that keeps nano's key bindings and
configuration, and adds syntax-aware (tree-sitter) highlighting.

![tico editing hello.cxx, a C++ program, on a white CRT monitor](https://ticoedit.org/images/hello-cxx-crt.png)

Downloads, documentation and the FAQ are at
**[ticoedit.org](https://ticoedit.org)**.

## License

tico is released under the MIT License (see `LICENSE`), with these
exceptions, all copied from the [Helix](https://github.com/helix-editor/helix)
editor and remaining under the Mozilla Public License 2.0 (see
`LICENSE-MPL-2.0`):

- `src/syntax/queries/diff.scm`, `src/syntax/queries/hcl.scm` and
  `src/syntax/queries/pascal.scm` — see `src/syntax/queries/README.md`
- the built-in themes under `themes/`, other than `tico-builtin-default.toml`
  — see `themes/README.md`

Both READMEs record the provenance of every vendored file.

A few tree-sitter grammars are vendored too, under `grammars/`, each with
a README recording its provenance: `tree-sitter-vcl/`, a fork of
[ntsk/tree-sitter-vcl](https://github.com/ntsk/tree-sitter-vcl) (MIT)
extended to cover Fastly's VCL dialect; `tree-sitter-pascal/`, a fork of
[Isopod/tree-sitter-pascal](https://github.com/Isopod/tree-sitter-pascal)
(MIT) extended to cover Turbo Pascal; and unmodified copies of
`tree-sitter-template-toolkit/` and `tree-sitter-cue/` (both MIT), whose
upstreams publish no usable crate. Every other grammar is a crates.io
or git dependency.
