//! `ttee`: behaves like POSIX `tee`, except that its stdout copy gets
//! tico's own syntax highlighting when stdout is a terminal -- same config
//! file (`~/.ticorc`), same theme resolution, same heredoc/injection
//! theming as the `tico` editor and `tcat`.
//!
//! The two output streams are treated differently, deliberately: every
//! file operand always gets raw, unhighlighted bytes, streamed as they
//! arrive, exactly like real `tee` (so a `ttee`'d file is always clean,
//! reusable text -- never polluted with ANSI codes -- and can be `tail
//! -f`'d live from elsewhere while `ttee` is still running). stdout, when
//! it's a terminal, is fully buffered until stdin closes and then printed
//! once, highlighted -- the main use case is a code generator piped
//! straight into a file (`generate-perl | ttee out.pl`), where you want
//! both the clean saved file and a highlighted look at what was produced;
//! `ttee`'s own stdout just won't update live for that one case. Piped or
//! redirected stdout is the ordinary, immediate streaming passthrough,
//! unless `--color=always` asks for the buffered, highlighted copy anyway
//! (`generate-perl | ttee out.pl --color=always | less -R`);
//! `--color=never` makes a terminal behave like a pipe.
//!
//! There's no input filename at all (`tee`'s operands are output
//! destinations), so language detection uses the *first* file operand's
//! name for the extension step, same shebang/modeline sniffing of the
//! actual content as `tcat`'s stdin case, and the same `-Y`/`--syntax`
//! override.
//!
//! POSIX `tee` only (`-a`, `-i`) -- no GNU extensions.

use clap::Parser;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use tico::theme::ColorWhen;

#[derive(Parser, Debug)]
#[command(
    name = "ttee",
    version,
    about = "tee, with tico's syntax highlighting on the stdout copy when it's a terminal"
)]
struct Cli {
    #[arg(short = 'a', help = "Append to files rather than overwriting them")]
    append: bool,
    #[arg(short = 'i', help = "Ignore the SIGINT signal")]
    ignore_interrupts: bool,
    #[arg(
        short = 'Y',
        long = "syntax",
        value_name = "name",
        help = "Force this language for the stdout copy (\"none\" disables highlighting)"
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
        help = "When to colorize the stdout copy: auto (only if stdout is a terminal), \
                always, or never. A bare --color means always."
    )]
    color: ColorWhen,
    /// Output files. The first one's name (if any) feeds language
    /// detection for the stdout copy, same as a `tcat` operand's own name.
    #[arg(trailing_var_arg = true)]
    files: Vec<String>,
}

#[cfg(unix)]
fn ignore_sigint() {
    // SAFETY: installing the ignore-disposition for SIGINT via a single,
    // well-defined libc call, matching real `tee -i` exactly; no signal
    // handler closure is involved, so there's nothing unsound here beyond
    // the FFI call itself.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
}

#[cfg(not(unix))]
fn ignore_sigint() {}

fn main() {
    let cli = Cli::parse();

    if cli.ignore_interrupts {
        ignore_sigint();
    }

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
        eprintln!("ttee: {w}");
    }

    // Opened up front, same as real `tee`: a file that can't be opened is
    // reported immediately, but the others (and stdin/stdout) still work.
    let mut had_error = false;
    let mut files: Vec<(String, File)> = Vec::new();
    for path in &cli.files {
        match open_destination(path, cli.append) {
            Ok(f) => files.push((path.clone(), f)),
            Err(e) => {
                eprintln!("ttee: {path}: {e}");
                had_error = true;
            }
        }
    }

    let detect_path = cli.files.first().map(PathBuf::from);
    let syntax_override = options.syntax_name.as_deref();
    let colorize = cli.color.colorize(io::stdout().is_terminal());
    let mut stdout = io::stdout();

    // Only accumulated when highlighting (so it can be done as one whole,
    // once stdin closes); otherwise stdout is written
    // straight through in the same loop as the files, exactly like real
    // `tee`, and this stays empty and unused.
    let mut pending = Vec::new();

    let mut stdin = io::stdin();
    let mut buf = [0u8; 65536];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                eprintln!("ttee: stdin: {e}");
                had_error = true;
                break;
            }
        };
        let chunk = &buf[..n];

        for (path, f) in &mut files {
            if let Err(e) = f.write_all(chunk) {
                eprintln!("ttee: {path}: {e}");
                had_error = true;
            }
        }

        if colorize {
            pending.extend_from_slice(chunk);
        } else if let Err(e) = stdout.write_all(chunk) {
            // Matches real `tee`'s default-SIGPIPE behavior: a downstream
            // reader that closed early (`producer | ttee file | head`)
            // ends the whole run quietly, the same as `tcat` handles it --
            // Rust ignores SIGPIPE by default and hands back a normal
            // error instead of dying the way a C program would.
            if e.kind() == io::ErrorKind::BrokenPipe {
                let _ = stdout.flush();
                std::process::exit(if had_error { 1 } else { 0 });
            }
            eprintln!("ttee: stdout: {e}");
            had_error = true;
        }
    }

    if colorize
        && let Err(e) = tico::theme::highlight_or_plain(
            &pending,
            detect_path.as_deref(),
            &options,
            syntax_override,
            &theme,
            &language_themes,
            &mut stdout,
        )
    {
        if e.kind() == io::ErrorKind::BrokenPipe {
            let _ = stdout.flush();
            std::process::exit(if had_error { 1 } else { 0 });
        }
        eprintln!("ttee: stdout: {e}");
        had_error = true;
    }

    let _ = stdout.flush();
    std::process::exit(if had_error { 1 } else { 0 });
}

/// Open one destination the way real `tee` does: create it if it doesn't
/// exist, and either truncate it or append to it depending on `-a`.
fn open_destination(path: &str, append: bool) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ttee_test_{}_{name}", std::process::id()))
    }

    #[test]
    fn open_destination_truncates_existing_content_by_default() {
        let path = temp_path("truncate.txt");
        std::fs::write(&path, "old content\n").unwrap();
        open_destination(path.to_str().unwrap(), false)
            .unwrap()
            .write_all(b"new\n")
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_destination_appends_when_requested() {
        let path = temp_path("append.txt");
        std::fs::write(&path, "first\n").unwrap();
        open_destination(path.to_str().unwrap(), true)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_destination_creates_a_new_file() {
        let path = temp_path("new.txt");
        std::fs::remove_file(&path).ok();
        open_destination(path.to_str().unwrap(), false)
            .unwrap()
            .write_all(b"content\n")
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content\n");
        std::fs::remove_file(&path).ok();
    }
}
