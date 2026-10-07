//! `tcat`: behaves like POSIX `cat`, except that when stdout is a terminal
//! it colorizes each operand with tico's own syntax highlighting -- same
//! config file (`~/.ticorc`), same theme resolution, same language
//! detection (filename extension -> shebang -> modeline) as the `tico`
//! editor itself. Piped/redirected output is byte-identical to plain
//! `cat`, so `tcat` is safe to use anywhere `cat` is. `--color=always`
//! overrides that terminal check (for `tcat --color=always foo.c | less
//! -R`), `--color=never` disables highlighting -- GNU `ls`/`grep` style.
//!
//! Per operand, not per run: `tcat foo.pl foo.c` highlights each in its own
//! language. `-Y`/`--syntax NAME` overrides detection for every operand at
//! once (matching tico's own, not-per-file, `-Y` semantics); `-Y none`
//! disables highlighting outright. Standard input has no filename, so it's
//! only highlighted when `-Y` names a real language -- which means fully
//! buffering it first (giving up `cat`'s usual streaming for that case).
//!
//! GNU `cat` extensions (`-n`, `-s`, ...) are intentionally out of scope.

use clap::Parser;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use tico::theme::{ColorWhen, Theme};

#[derive(Parser, Debug)]
#[command(
    name = "tcat",
    version,
    about = "cat, with tico's syntax highlighting when stdout is a terminal"
)]
struct Cli {
    #[arg(
        short = 'u',
        help = "Accepted for POSIX cat compatibility; output is never extra-buffered here"
    )]
    _unbuffered: bool,
    #[arg(
        short = 'Y',
        long = "syntax",
        value_name = "name",
        help = "Force this language for every operand (\"none\" disables highlighting)"
    )]
    syntax: Option<String>,
    #[arg(
        long = "tico-theme",
        value_name = "[lang.]name",
        action = clap::ArgAction::Append,
        help = "Syntax-highlighting theme, overriding [syntax]'s `theme` (or, as `lang.NAME`, \
                only that language's); a built-in (tico-builtin-*), a Helix theme name, or \
                a .toml path. May be repeated."
    )]
    tico_theme: Vec<String>,
    #[arg(
        short = 'f',
        long = "rcfile",
        value_name = "file",
        help = "Use only this file for configuring tico"
    )]
    rcfile: Option<String>,
    #[arg(
        short = 'I',
        long = "ignorercfiles",
        help = "Don't look at nanorc/ticorc files"
    )]
    ignorercfiles: bool,
    #[arg(
        long = "color",
        value_name = "WHEN",
        value_enum,
        default_value_t = ColorWhen::Auto,
        default_missing_value = "always",
        num_args = 0..=1,
        require_equals = true,
        help = "When to colorize: auto (only if stdout is a terminal), always, or never. \
                A bare --color means always."
    )]
    color: ColorWhen,
    /// Files to print. `-`, or no operands at all, means standard input.
    #[arg(trailing_var_arg = true)]
    files: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    let loaded = tico::config::load(cli.rcfile.as_deref(), cli.ignorercfiles, false, false);
    let mut options = loaded.options;
    if let Some(v) = &cli.syntax {
        options.syntax_name = Some(v.clone());
    }
    for v in &cli.tico_theme {
        match tico::theme::split_language_theme(v) {
            Some((lang, name)) => options
                .language_themes
                .push((lang.to_ascii_lowercase(), name.to_string())),
            None => options.theme = Some(v.clone()),
        }
    }

    let mut warnings = loaded.warnings;
    let (theme, language_themes) = tico::theme::resolve_themes(&options, &mut warnings);
    for w in &warnings {
        eprintln!("tcat: {w}");
    }

    let colorize = cli.color.colorize(io::stdout().is_terminal());
    let syntax_override = options.syntax_name.as_deref();
    let operands: &[String] = if cli.files.is_empty() {
        &["-".to_string()]
    } else {
        &cli.files
    };

    let mut out = io::stdout().lock();
    let mut exit_code = 0i32;
    for operand in operands {
        if let Err(e) = cat_one(
            operand,
            &options,
            colorize,
            syntax_override,
            &theme,
            &language_themes,
            &mut out,
        ) {
            // Rust ignores SIGPIPE by default and hands back a normal
            // error instead of dying outright the way a C program (real
            // `cat`) would -- so a downstream reader closing early (`tcat
            // bigfile | head`) needs to be handled explicitly the same
            // way: exit quietly, not with a "Broken pipe" error message.
            if e.kind() == io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
            eprintln!("tcat: {operand}: {e}");
            exit_code = 1;
        }
    }
    // `process::exit` below skips normal drop glue, so anything crossterm
    // queued onto `out` but hasn't written yet needs an explicit flush.
    let _ = out.flush();
    std::process::exit(exit_code);
}

/// Print one operand (`-` for standard input, else a filename), matching
/// `cat`'s own byte-for-byte passthrough -- no line-ending or whitespace
/// transformation -- plus tico's own syntax coloring, layered on top only
/// when it's safe and meaningful: `colorize` (stdout is a terminal, or
/// `--color=always`), `syntax_highlighting` is on, the content is valid
/// UTF-8, it's under the configured size limit, and a language was
/// actually detected (or forced).
fn cat_one(
    operand: &str,
    options: &tico::options::Options,
    colorize: bool,
    syntax_override: Option<&str>,
    theme: &Theme,
    language_themes: &HashMap<String, Theme>,
    out: &mut impl Write,
) -> io::Result<()> {
    let (bytes, path): (Vec<u8>, Option<PathBuf>) = if operand == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        (buf, None)
    } else {
        let path = PathBuf::from(operand);
        let bytes = std::fs::read(&path)?;
        (bytes, Some(path))
    };

    if !colorize {
        return out.write_all(&bytes);
    }
    tico::theme::highlight_or_plain(
        &bytes,
        path.as_deref(),
        options,
        syntax_override,
        theme,
        language_themes,
        out,
    )
}
