//! The language registry and detection cascade: filename extension (or
//! exact filename, for things like `Makefile`) -> shebang line -> modeline
//! (vim- or Emacs-style) -> a peek at the leading lines (JSON's `{ "`,
//! YAML's `---`), matching the priority order other editors use.
//! All on by default; see `crate::options::Options::syntax_highlighting`
//! for the master on/off toggle.

use std::path::Path;

pub struct LanguageDef {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    /// Exact basenames (`Makefile`, `cpanfile`), plus glob-style entries
    /// with a single `*` standing for any run of characters: `Makefile.*`
    /// matches `Makefile.in`, `log4perl.*.conf` matches `log4perl.debug.conf`.
    /// Glob entries are consulted only after the extension table, so
    /// `Makefile.PL` stays Perl rather than makefile; see
    /// `FILENAME_EXCEPTIONS` for basenames carved out of a glob.
    pub filenames: &'static [&'static str],
    pub shebangs: &'static [&'static str],
    /// Extra names recognized in a modeline besides `name` itself.
    pub modeline_aliases: &'static [&'static str],
    pub language: fn() -> tree_sitter::Language,
    pub highlights_query: &'static str,
    /// A per-syntax external linter command (nano nanorc 'linter' directive default), split on whitespace with the buffer's path appended as the final argument.
    pub linter: Option<&'static str>,
    /// A per-syntax external formatter command (nanorc 'formatter' directive default): run on a temp copy of the buffer, which replaces the buffer's content if the tool modified it.
    pub formatter: Option<&'static str>,
    /// What `M-3` Comment/Uncomment prefixes lines with (nanorc `comment`
    /// directive default, taken from nano's shipped syntax files where it
    /// has one): a plain prefix like `#`, or `PREFIX|POSTFIX` for
    /// bracketing comments like `<!--|-->`. Empty means the language has no
    /// line comments and `M-3` reports so; a buffer with no language at
    /// all uses nano's general default, `#`.
    pub comment: &'static str,
}

macro_rules! lang_fn {
    ($fn_name:ident, $krate:ident) => {
        fn $fn_name() -> tree_sitter::Language {
            $krate::LANGUAGE.into()
        }
    };
    ($fn_name:ident, $krate:ident, $konst:ident) => {
        fn $fn_name() -> tree_sitter::Language {
            $krate::$konst.into()
        }
    };
}

/// A handful of grammar crates still expose the older `fn language() ->
/// Language` API instead of the newer `const LANGUAGE: LanguageFn`.
macro_rules! lang_fn_old_api {
    ($fn_name:ident, $krate:ident) => {
        fn $fn_name() -> tree_sitter::Language {
            $krate::language()
        }
    };
}

lang_fn!(lang_perl, ts_parser_perl);
lang_fn!(lang_c, tree_sitter_c);
lang_fn!(lang_cpp, tree_sitter_cpp);
lang_fn!(lang_html, tree_sitter_html);
lang_fn!(lang_css, tree_sitter_css);
lang_fn!(lang_sql, tree_sitter_sequel);
lang_fn!(lang_python, tree_sitter_python);
lang_fn!(lang_rust, tree_sitter_rust);
lang_fn!(lang_bash, tree_sitter_bash);
lang_fn!(lang_json, tree_sitter_json);
lang_fn!(lang_yaml, tree_sitter_yaml);
lang_fn!(lang_v, tree_sitter_vlang);
lang_fn!(lang_toml, tree_sitter_toml_ng);
lang_fn!(lang_make, tree_sitter_make);
lang_fn!(lang_fortran, tree_sitter_fortran);
lang_fn!(lang_go, tree_sitter_go);
lang_fn!(lang_javascript, tree_sitter_javascript);
lang_fn!(lang_typescript, tree_sitter_typescript, LANGUAGE_TYPESCRIPT);
lang_fn!(lang_java, tree_sitter_java);
lang_fn!(lang_ruby, tree_sitter_ruby);
lang_fn!(lang_php, tree_sitter_php, LANGUAGE_PHP);
lang_fn!(lang_csharp, tree_sitter_c_sharp);
lang_fn!(lang_swift, tree_sitter_swift);
lang_fn!(lang_haskell, tree_sitter_haskell);
lang_fn!(lang_scala, tree_sitter_scala);
lang_fn!(lang_objc, tree_sitter_objc);
lang_fn!(lang_r, tree_sitter_r);
lang_fn!(lang_julia, tree_sitter_julia);
lang_fn!(lang_xml, tree_sitter_xml, LANGUAGE_XML);
lang_fn!(lang_diff, tree_sitter_diff);
lang_fn!(lang_ini, tree_sitter_ini);
lang_fn!(lang_elixir, tree_sitter_elixir);
lang_fn!(lang_elm, tree_sitter_elm);
lang_fn!(lang_zig, tree_sitter_zig);
lang_fn!(lang_dart, tree_sitter_dart);
lang_fn_old_api!(lang_scss, tree_sitter_scss);
lang_fn!(lang_proto, tree_sitter_proto);
lang_fn!(lang_cmake, tree_sitter_cmake);
lang_fn!(lang_nix, tree_sitter_nix);
lang_fn_old_api!(lang_vim, tree_sitter_vim);
lang_fn!(lang_lua, tree_sitter_lua);
lang_fn!(lang_markdown, tree_sitter_md);
lang_fn!(lang_groovy, dekobon_tree_sitter_groovy);
lang_fn!(lang_properties, tree_sitter_properties);
lang_fn!(lang_tcl, tree_sitter_tcl);
lang_fn!(lang_dockerfile, tree_sitter_containerfile);
lang_fn!(lang_powershell, tree_sitter_powershell);
lang_fn!(lang_batch, tree_sitter_batch);
lang_fn!(lang_hcl, tree_sitter_hcl);

// The VCL grammar is not a crate: build.rs compiles it from
// `grammars/tree-sitter-vcl/` (a fork of ntsk/tree-sitter-vcl extended for
// Fastly's dialect), so its C entry point is declared here.
unsafe extern "C" {
    fn tree_sitter_vcl() -> *const ();
}
const VCL_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_vcl) };
fn lang_vcl() -> tree_sitter::Language {
    VCL_LANGUAGE.into()
}

// Likewise Template Toolkit, from `grammars/tree-sitter-template-toolkit/`
// (upstream ships no crate).
unsafe extern "C" {
    fn tree_sitter_template_toolkit() -> *const ();
}
const TT2_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_template_toolkit) };
fn lang_tt2() -> tree_sitter::Language {
    TT2_LANGUAGE.into()
}

// And CUE, from `grammars/tree-sitter-cue/` (upstream's Rust bindings
// still pin tree-sitter 0.20).
unsafe extern "C" {
    fn tree_sitter_cue() -> *const ();
}
const CUE_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_cue) };
fn lang_cue() -> tree_sitter::Language {
    CUE_LANGUAGE.into()
}

// And Pascal, from `grammars/tree-sitter-pascal/` (a fork of
// Isopod/tree-sitter-pascal extended for Turbo Pascal).
unsafe extern "C" {
    fn tree_sitter_pascal() -> *const ();
}
const PASCAL_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_pascal) };
fn lang_pascal() -> tree_sitter::Language {
    PASCAL_LANGUAGE.into()
}

const LANGUAGES: &[LanguageDef] = &[
    LanguageDef {
        name: "perl",
        extensions: &["pl", "pm", "t", "xs"],
        filenames: &["cpanfile", "cpanfile.*"],
        shebangs: &["perl", "perl5"],
        modeline_aliases: &["cperl"],
        language: lang_perl,
        highlights_query: include_str!("queries/perl.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "c",
        extensions: &["c", "h"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_c,
        highlights_query: include_str!("queries/c.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "cpp",
        extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "c++", "h++"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["c++"],
        language: lang_cpp,
        // Upstream's cpp query only covers what C++ adds to C, and is
        // meant to be layered over the C query (tree-sitter-cpp's
        // tree-sitter.json lists both, C first) -- later patterns win.
        highlights_query: concat!(
            include_str!("queries/c.scm"),
            include_str!("queries/cpp.scm")
        ),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "html",
        extensions: &["html", "htm"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_html,
        highlights_query: include_str!("queries/html.scm"),
        linter: None,
        formatter: Some("tidy -m -q"),
        comment: "<!--|-->",
    },
    LanguageDef {
        name: "css",
        extensions: &["css"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_css,
        highlights_query: include_str!("queries/css.scm"),
        linter: None,
        formatter: None,
        comment: "/*|*/",
    },
    LanguageDef {
        name: "sql",
        extensions: &["sql"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_sql,
        highlights_query: include_str!("queries/sequel.scm"),
        linter: None,
        formatter: None,
        comment: "-- ",
    },
    LanguageDef {
        name: "python",
        extensions: &["py", "pyw", "pyi"],
        filenames: &[],
        shebangs: &["python", "python2", "python3"],
        modeline_aliases: &["py"],
        language: lang_python,
        highlights_query: include_str!("queries/python.scm"),
        linter: Some("pyflakes"),
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "rust",
        extensions: &["rs"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_rust,
        highlights_query: include_str!("queries/rust.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "bash",
        extensions: &["sh", "bash", "zsh", "ksh"],
        filenames: &[".bashrc", ".bash_profile", ".zshrc", ".profile"],
        shebangs: &["bash", "sh", "zsh", "ksh", "dash"],
        modeline_aliases: &["sh", "zsh"],
        language: lang_bash,
        highlights_query: include_str!("queries/bash.scm"),
        linter: Some("dash -n"),
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "json",
        extensions: &["json"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_json,
        highlights_query: include_str!("queries/json.scm"),
        linter: None,
        formatter: None,
        comment: "",
    },
    LanguageDef {
        name: "yaml",
        extensions: &["yaml", "yml"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_yaml,
        highlights_query: include_str!("queries/yaml.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "toml",
        extensions: &["toml"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_toml,
        highlights_query: include_str!("queries/toml.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "make",
        extensions: &["mk", "mak"],
        filenames: &[
            "Makefile",
            "makefile",
            "GNUmakefile",
            "Makefile.*",
            "makefile.*",
            "GNUmakefile.*",
        ],
        shebangs: &["make"],
        modeline_aliases: &["makefile"],
        language: lang_make,
        highlights_query: include_str!("queries/make.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "fortran",
        extensions: &["f", "for", "f90", "f95", "f03", "f08"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["f90"],
        language: lang_fortran,
        highlights_query: include_str!("queries/fortran.scm"),
        linter: None,
        formatter: None,
        comment: "!",
    },
    LanguageDef {
        name: "go",
        extensions: &["go"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_go,
        highlights_query: include_str!("queries/go.scm"),
        linter: None,
        formatter: Some("gofmt -w"),
        comment: "//",
    },
    LanguageDef {
        name: "javascript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        filenames: &[],
        shebangs: &["node", "nodejs"],
        modeline_aliases: &["js"],
        language: lang_javascript,
        highlights_query: include_str!("queries/javascript.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "typescript",
        extensions: &["ts", "mts", "cts"],
        filenames: &[],
        shebangs: &["ts-node", "deno"],
        modeline_aliases: &["ts"],
        language: lang_typescript,
        highlights_query: include_str!("queries/typescript.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "java",
        extensions: &["java"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_java,
        highlights_query: include_str!("queries/java.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "ruby",
        extensions: &["rb", "rake", "gemspec"],
        filenames: &["Rakefile", "Gemfile"],
        shebangs: &["ruby"],
        modeline_aliases: &["rb"],
        language: lang_ruby,
        highlights_query: include_str!("queries/ruby.scm"),
        linter: Some("ruby -w -c"),
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "php",
        extensions: &["php", "php3", "php4", "php5", "phtml"],
        filenames: &[],
        shebangs: &["php"],
        modeline_aliases: &[],
        language: lang_php,
        highlights_query: include_str!("queries/php.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "csharp",
        extensions: &["cs"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["cs", "c#"],
        language: lang_csharp,
        highlights_query: include_str!("queries/c-sharp.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "swift",
        extensions: &["swift"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_swift,
        highlights_query: include_str!("queries/swift.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "haskell",
        extensions: &["hs", "lhs"],
        filenames: &[],
        shebangs: &["runghc", "runhaskell"],
        modeline_aliases: &[],
        language: lang_haskell,
        highlights_query: include_str!("queries/haskell.scm"),
        linter: None,
        formatter: None,
        comment: "--",
    },
    LanguageDef {
        name: "scala",
        extensions: &["scala", "sc"],
        filenames: &[],
        shebangs: &["scala"],
        modeline_aliases: &[],
        language: lang_scala,
        highlights_query: include_str!("queries/scala.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "objc",
        extensions: &["m", "mm"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["objc", "objective-c"],
        language: lang_objc,
        highlights_query: include_str!("queries/objc.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "r",
        extensions: &["r", "R"],
        filenames: &[],
        shebangs: &["Rscript"],
        modeline_aliases: &[],
        language: lang_r,
        highlights_query: include_str!("queries/r.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "julia",
        extensions: &["jl"],
        filenames: &[],
        shebangs: &["julia"],
        modeline_aliases: &[],
        language: lang_julia,
        highlights_query: include_str!("queries/julia.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "xml",
        extensions: &["xml", "xsd", "xsl", "svg", "plist"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_xml,
        highlights_query: include_str!("queries/xml.scm"),
        linter: None,
        formatter: None,
        comment: "<!--|-->",
    },
    LanguageDef {
        name: "diff",
        extensions: &["diff", "patch"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_diff,
        highlights_query: include_str!("queries/diff.scm"),
        linter: None,
        formatter: None,
        comment: "",
    },
    LanguageDef {
        name: "ini",
        extensions: &["ini", "cfg"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["cfg", "dosini"],
        language: lang_ini,
        highlights_query: include_str!("queries/ini.scm"),
        linter: None,
        formatter: None,
        comment: ";",
    },
    LanguageDef {
        name: "elixir",
        extensions: &["ex", "exs"],
        filenames: &[],
        shebangs: &["elixir"],
        modeline_aliases: &[],
        language: lang_elixir,
        highlights_query: include_str!("queries/elixir.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "elm",
        extensions: &["elm"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_elm,
        highlights_query: include_str!("queries/elm.scm"),
        linter: None,
        formatter: None,
        comment: "--",
    },
    LanguageDef {
        name: "zig",
        extensions: &["zig"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_zig,
        highlights_query: include_str!("queries/zig.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "dart",
        extensions: &["dart"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_dart,
        highlights_query: include_str!("queries/dart.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "scss",
        extensions: &["scss"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_scss,
        highlights_query: include_str!("queries/scss.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "proto",
        extensions: &["proto"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["protobuf"],
        language: lang_proto,
        highlights_query: include_str!("queries/proto.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "cmake",
        extensions: &["cmake"],
        filenames: &["CMakeLists.txt"],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_cmake,
        highlights_query: include_str!("queries/cmake.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "nix",
        extensions: &["nix"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_nix,
        highlights_query: include_str!("queries/nix.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "vim",
        extensions: &["vim"],
        filenames: &[".vimrc", "vimrc"],
        shebangs: &[],
        modeline_aliases: &["viml"],
        language: lang_vim,
        highlights_query: include_str!("queries/vim.scm"),
        linter: None,
        formatter: None,
        comment: r#"""#,
    },
    LanguageDef {
        name: "v",
        extensions: &["v", "vsh"],
        filenames: &["v.mod"],
        shebangs: &["v"],
        modeline_aliases: &["vlang"],
        language: lang_v,
        highlights_query: include_str!("queries/v.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "lua",
        extensions: &["lua"],
        filenames: &[],
        shebangs: &["lua"],
        modeline_aliases: &[],
        language: lang_lua,
        highlights_query: include_str!("queries/lua.scm"),
        linter: Some("luacheck --no-color"),
        formatter: None,
        comment: "--",
    },
    LanguageDef {
        name: "markdown",
        extensions: &["md", "markdown"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["md"],
        language: lang_markdown,
        highlights_query: include_str!("queries/markdown.scm"),
        linter: None,
        formatter: None,
        comment: "<!--|-->",
    },
    LanguageDef {
        name: "groovy",
        extensions: &["groovy", "gvy", "gy", "gsh", "gradle"],
        filenames: &["Jenkinsfile"],
        shebangs: &["groovy"],
        modeline_aliases: &[],
        language: lang_groovy,
        highlights_query: include_str!("queries/groovy.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        name: "properties",
        extensions: &["properties", "prefs"],
        filenames: &["log4perl.conf", "log4perl.*.conf", "log4j.properties"],
        shebangs: &[],
        modeline_aliases: &["jproperties"],
        language: lang_properties,
        highlights_query: include_str!("queries/properties.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "tcl",
        extensions: &["tcl", "exp"],
        filenames: &[],
        shebangs: &["tclsh", "wish", "expect"],
        modeline_aliases: &["expect"],
        language: lang_tcl,
        highlights_query: include_str!("queries/tcl.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        // The names Helix and Neovim both recognize: the bare file, a
        // suffixed variant (`Dockerfile.dev`), Podman's `Containerfile`
        // spelling, and a `.dockerfile` extension (`app.Dockerfile` too,
        // since extensions match case-insensitively).
        name: "dockerfile",
        extensions: &["dockerfile"],
        filenames: &[
            "Dockerfile",
            "dockerfile",
            "Containerfile",
            "containerfile",
            "Dockerfile.*",
            "dockerfile.*",
            "Containerfile.*",
            "containerfile.*",
        ],
        shebangs: &[],
        modeline_aliases: &["docker", "containerfile"],
        language: lang_dockerfile,
        highlights_query: include_str!("queries/dockerfile.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        // Only the `[% ... %]` directives are highlighted; the text between
        // them stays plain whatever it is (HTML, VCL, config), since a
        // template's output language isn't knowable from its name.
        name: "tt2",
        extensions: &["tt", "tt2", "tmpl"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["tt", "tt2html", "template-toolkit"],
        language: lang_tt2,
        highlights_query: include_str!("queries/tt2.scm"),
        linter: None,
        formatter: None,
        comment: "[%#|%]",
    },
    LanguageDef {
        name: "powershell",
        extensions: &["ps1", "psm1", "psd1", "pscc", "psrc"],
        filenames: &[],
        shebangs: &["pwsh", "powershell"],
        modeline_aliases: &["ps1", "pwsh"],
        language: lang_powershell,
        highlights_query: include_str!("queries/powershell.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        // `.btm` is JP Software's (4DOS/4NT/Take Command) extended batch;
        // the core syntax is CMD's, so it gets the same grammar, as in
        // Helix. `REM ` rather than `::` for M-3: `::` is a label trick
        // that misbehaves inside parenthesized blocks.
        name: "batch",
        extensions: &["bat", "cmd", "btm"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["dosbatch", "bat", "cmd"],
        language: lang_batch,
        highlights_query: include_str!("queries/batch.scm"),
        linter: None,
        formatter: None,
        comment: "REM ",
    },
    LanguageDef {
        name: "cue",
        extensions: &["cue"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &[],
        language: lang_cue,
        highlights_query: include_str!("queries/cue.scm"),
        linter: None,
        formatter: None,
        comment: "//",
    },
    LanguageDef {
        // Terraform (.tf/.tfvars) and Nomad job files are HCL; upstream's
        // "terraform" dialect is the same grammar under another name.
        name: "hcl",
        extensions: &["hcl", "tf", "tfvars", "nomad"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["terraform", "tf", "tfvars"],
        language: lang_hcl,
        highlights_query: include_str!("queries/hcl.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        name: "vcl",
        extensions: &["vcl"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["varnish"],
        language: lang_vcl,
        highlights_query: include_str!("queries/vcl.scm"),
        linter: None,
        formatter: None,
        comment: "#",
    },
    LanguageDef {
        // Turbo Pascal, Free Pascal and Delphi. `.inc` include files are
        // not claimed by extension (assembler, PHP and C use it too); a
        // `.inc` whose first line looks like Pascal is sniffed instead,
        // see `detect_by_content`. Brace comments are the toggle because
        // Turbo Pascal has no `//` line comment; every dialect accepts
        // `{ }`.
        name: "pascal",
        extensions: &["pas", "pp", "dpr", "lpr", "dpk"],
        filenames: &[],
        shebangs: &[],
        modeline_aliases: &["delphi"],
        language: lang_pascal,
        highlights_query: include_str!("queries/pascal.scm"),
        linter: None,
        formatter: None,
        comment: "{|}",
    },
];

/// Detect a buffer's language: by filename extension or exact filename
/// first, then by shebang line, then by a vim- or Emacs-style modeline
/// found in the first or last few lines, and finally by sniffing the
/// leading bytes — matching how other editors layer these signals,
/// most-specific (and cheapest to check) first. The content sniff is the
/// counterpart of nano's `header` directive: it only gets a say when the
/// filename told us nothing, so `foo.conf` holding JSON is highlighted
/// as JSON while `foo.ini` never is. One sniff is extension-specific:
/// only a `.inc` file is checked for Pascal.
pub fn detect(path: Option<&Path>, text: &str) -> Option<&'static LanguageDef> {
    if let Some(path) = path
        && let Some(lang) = detect_by_filename(path)
    {
        return Some(lang);
    }
    let first_line = text.lines().next().unwrap_or("");
    if let Some(lang) = detect_by_shebang(first_line) {
        return Some(lang);
    }
    if let Some(lang) = detect_by_modeline(text) {
        return Some(lang);
    }
    if let Some(lang) = detect_by_content(path, text) {
        return Some(lang);
    }
    None
}

/// Guess from the first line or two, without parsing anything. Only
/// formats with a distinctive opening are sniffed, the way nano's
/// `header` lines do it. The Pascal sniff is the one that looks at the
/// filename: `.inc` is shared with assembler, PHP and C includes, so it
/// isn't registered as a Pascal extension, but a `.inc` that opens like
/// Pascal is Pascal.
fn detect_by_content(path: Option<&Path>, text: &str) -> Option<&'static LanguageDef> {
    let text = text.trim_start_matches('\u{feff}');
    let is_inc = path
        .and_then(Path::extension)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("inc"));
    if looks_like_json(text) {
        find_by_name("json")
    } else if looks_like_diff(text) {
        find_by_name("diff")
    } else if looks_like_yaml(text) {
        find_by_name("yaml")
    } else if is_inc && looks_like_pascal(text) {
        find_by_name("pascal")
    } else {
        None
    }
}

/// The first non-blank line opens a compiler directive (`{$mode ...}`,
/// `{$ifdef ...}`, `{$I ...}`: how nearly every Free Pascal include
/// starts), a comment (`(*`, or `{` followed by a letter or a space; JSON's
/// `{"` was ruled out before this runs), a section header standing alone
/// on its line (`const`, `type`, `var`, which tells a Pascal `const`
/// section from a JavaScript `const x = 1`), a module-level keyword or
/// `label`, or a routine header (`function` only when the line ends in
/// `;`, since a JavaScript or PHP `function` line ends in `{`).
fn looks_like_pascal(text: &str) -> bool {
    let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return false;
    };
    if line.starts_with("{$") || line.starts_with("(*") {
        return true;
    }
    if let Some(rest) = line.strip_prefix('{')
        && rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == ' ')
    {
        return true;
    }
    let mut words = line.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    let alone = words.next().is_none();
    match first.to_ascii_lowercase().as_str() {
        "unit" | "program" | "library" | "uses" | "label" | "resourcestring" | "procedure"
        | "constructor" | "destructor" => true,
        "const" | "type" | "var" | "interface" | "implementation" => alone,
        "function" => line.ends_with(';'),
        _ => false,
    }
}

/// `{` followed by `"` or `}` (objects must have string keys, which
/// rules out C blocks, Perl hashes and Tcl), or `[` followed by the
/// start of a JSON value (which rules out an ini `[section]` header, the
/// one real collision among common `.conf` formats). Leading `//` and
/// `/* */` comments are skipped so JSONC qualifies; a `#` comment is not,
/// since that's the strongest sign a file isn't JSON.
fn looks_like_json(text: &str) -> bool {
    let rest = skip_c_style_comments(text);
    let mut chars = rest.chars().skip_while(|c| c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    let second = chars.find(|c| !c.is_whitespace()).unwrap_or('\0');
    match first {
        '{' => matches!(second, '"' | '}'),
        '[' => {
            matches!(second, '{' | '[' | '"' | ']' | '-' | 't' | 'f' | 'n')
                || second.is_ascii_digit()
        }
        _ => false,
    }
}

/// A unified diff's `--- old` / `+++ new` header pair. Checked before
/// YAML because `--- a/file` also satisfies nano's YAML header rule.
fn looks_like_diff(text: &str) -> bool {
    let mut lines = text.lines();
    lines.next().is_some_and(|l| l.starts_with("--- "))
        && lines.next().is_some_and(|l| l.starts_with("+++ "))
}

/// nano's yaml.nanorc header rule, `^%YAML |^---( |$)`, applied to the
/// first line that isn't blank or a `#` comment (YAML files routinely
/// open with a comment block before the document marker).
fn looks_like_yaml(text: &str) -> bool {
    text.lines()
        .map(str::trim_end)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .is_some_and(|l| l.starts_with("%YAML ") || l == "---" || l.starts_with("--- "))
}

/// Skip any run of leading whitespace and `//` or `/* ... */` comments.
fn skip_c_style_comments(mut text: &str) -> &str {
    loop {
        text = text.trim_start();
        if let Some(rest) = text.strip_prefix("//") {
            text = rest.split_once('\n').map_or("", |(_, after)| after);
        } else if let Some(rest) = text.strip_prefix("/*") {
            let Some((_, after)) = rest.split_once("*/") else {
                return "";
            };
            text = after;
        } else {
            return text;
        }
    }
}

/// Basenames that would match a `NAME.*` pattern below but aren't that
/// language: `cpanfile.snapshot` is Carton's lockfile, not Perl source.
const FILENAME_EXCEPTIONS: &[&str] = &["cpanfile.snapshot"];

fn detect_by_filename(path: &Path) -> Option<&'static LanguageDef> {
    let filename = path.file_name().and_then(|f| f.to_str());
    if let Some(filename) = filename {
        if FILENAME_EXCEPTIONS.contains(&filename) {
            return None;
        }
        for lang in LANGUAGES {
            if lang.filenames.contains(&filename) {
                return Some(lang);
            }
        }
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && let Some(lang) = LANGUAGES
            .iter()
            .find(|lang| lang.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
    {
        return Some(lang);
    }
    // Glob patterns last, so a recognized extension wins over the family
    // name: `Makefile.PL` is Perl, `Makefile.in` is a makefile.
    let filename = filename?;
    LANGUAGES.iter().find(|lang| {
        lang.filenames
            .iter()
            .any(|pattern| glob_matches(pattern, filename))
    })
}

/// Match a `filenames` entry containing one `*` against a basename: the
/// text before the star must be a prefix and the text after it a suffix,
/// without overlapping. An entry with no `*` never matches here (exact
/// names are handled separately).
fn glob_matches(pattern: &str, filename: &str) -> bool {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return false;
    };
    filename.len() >= prefix.len() + suffix.len()
        && filename.starts_with(prefix)
        && filename.ends_with(suffix)
}

/// Parse a shebang line, following `env` indirection (e.g.
/// `#!/usr/bin/env perl` -> `perl`), and match it against each language's
/// known interpreter names.
fn detect_by_shebang(first_line: &str) -> Option<&'static LanguageDef> {
    let rest = first_line.strip_prefix("#!")?;
    let mut parts = rest.split_whitespace();
    let mut interpreter = parts.next()?;
    let interpreter_base = interpreter.rsplit('/').next().unwrap_or(interpreter);
    if interpreter_base == "env" {
        interpreter = parts.next()?;
        // `env -S` splits the rest of the line into arguments, which is how
        // a shebang passes the interpreter its own options
        // (`#!/usr/bin/env -S v run`).
        if interpreter == "-S" {
            interpreter = parts.next()?;
        }
    } else {
        interpreter = interpreter_base;
    }
    // Strip a trailing version number, e.g. "python3" already handled by
    // exact shebang lists below, but "perl5.34" or "python3.11" isn't.
    let interpreter_trimmed =
        interpreter.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    LANGUAGES.iter().find(|lang| {
        lang.shebangs
            .iter()
            .any(|s| *s == interpreter || *s == interpreter_trimmed)
    })
}

/// Scan the first and last few lines for a vim modeline (`vim: set ft=X`,
/// `vim: syntax=X`, `vim: ft=X`) or an Emacs one (`-*- mode: X -*-` or the
/// shorthand `-*- X -*-`), matching each language's canonical name or one
/// of its aliases.
fn detect_by_modeline(text: &str) -> Option<&'static LanguageDef> {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    let head = lines.iter().take(5);
    let tail = lines.iter().rev().take(5);
    for line in head.chain(tail) {
        if let Some(name) = parse_vim_modeline(line).or_else(|| parse_emacs_modeline(line))
            && let Some(lang) = find_by_name(&name)
        {
            return Some(lang);
        }
    }
    let _ = n;
    None
}

fn parse_vim_modeline(line: &str) -> Option<String> {
    // Matches "vim: ft=perl", "vim: set ft=perl:", "vim: syntax=perl",
    // "ex: syntax=perl", each optionally followed by more ":"-separated
    // options and/or a trailing ":".
    for marker in ["vim:", "vi:", "ex:"] {
        if let Some(pos) = line.find(marker) {
            let rest = &line[pos + marker.len()..];
            let rest = rest
                .strip_prefix(" set ")
                .or_else(|| rest.strip_prefix("set "))
                .unwrap_or(rest);
            for field in rest.split(|c: char| c == ':' || c.is_whitespace()) {
                if let Some(v) = field
                    .strip_prefix("ft=")
                    .or_else(|| field.strip_prefix("filetype="))
                {
                    return Some(v.to_string());
                }
                if let Some(v) = field.strip_prefix("syntax=") {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn parse_emacs_modeline(line: &str) -> Option<String> {
    let start = line.find("-*-")?;
    let rest = &line[start + 3..];
    let end = rest.find("-*-")?;
    let inner = rest[..end].trim();
    if let Some(pos) = inner.find("mode:").or_else(|| inner.find("Mode:")) {
        let after = &inner[pos + "mode:".len()..];
        let name = after.split(';').next().unwrap_or(after).trim();
        return Some(name.to_string());
    }
    // Shorthand form: "-*- Perl -*-" (just the mode name, no "mode:" key).
    if !inner.is_empty() && !inner.contains(':') {
        return Some(inner.trim().to_string());
    }
    None
}

/// The canonical name of every registered language, sorted alphabetically —
/// for `-z`/`--listsyntaxes`.
pub fn names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = LANGUAGES.iter().map(|l| l.name).collect();
    names.sort_unstable();
    names
}

/// Look up a language by its canonical `name` or one of its
/// `modeline_aliases`, case-insensitively. Used for modeline detection, for
/// matching a heredoc terminator (e.g. `<<SQL`) to a language, and for the
/// `-Y`/`--syntax` CLI override.
pub fn find_by_name(name: &str) -> Option<&'static LanguageDef> {
    let name = name.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|l| l.name == name || l.modeline_aliases.iter().any(|a| *a == name))
}
