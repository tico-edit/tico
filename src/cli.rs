//! Command-line argument parsing, matching GNU nano 8.7.1's option set
//! (short/long flags captured verbatim from the installed `nano --help`).

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "tico",
    version,
    about = "Yet Another Text Editor (a nano-compatible TUI editor)",
    // clap's default (`args_override_self = false`) treats a repeated flag
    // as a conflict-with-itself error; nano (like most CLI tools) just
    // tolerates `-z -z`/`-v -v`/etc. as redundant, not an error.
    args_override_self = true
)]
pub struct Cli {
    #[arg(short = 'A', long = "smarthome", help = "Enable smart home key")]
    pub smarthome: bool,
    #[arg(short = 'B', long = "backup", help = "Save backups of existing files")]
    pub backup: bool,
    #[arg(
        short = 'C',
        long = "backupdir",
        value_name = "dir",
        help = "Directory for saving unique backup files"
    )]
    pub backupdir: Option<String>,
    #[arg(
        short = 'D',
        long = "boldtext",
        help = "Use bold instead of reverse video text"
    )]
    pub boldtext: bool,
    #[arg(
        short = 'E',
        long = "tabstospaces",
        help = "Convert typed tabs to spaces"
    )]
    pub tabstospaces: bool,
    #[arg(
        short = 'F',
        long = "multibuffer",
        help = "Read a file into a new buffer by default"
    )]
    pub multibuffer: bool,
    #[arg(short = 'G', long = "locking", help = "Use (vim-style) lock files")]
    pub locking: bool,
    #[arg(
        short = 'H',
        long = "historylog",
        help = "Save & reload old search/replace strings"
    )]
    pub historylog: bool,
    #[arg(
        short = 'I',
        long = "ignorercfiles",
        help = "Don't look at nanorc files"
    )]
    pub ignorercfiles: bool,
    #[arg(
        short = 'J',
        long = "guidestripe",
        value_name = "number",
        help = "Show a guiding bar at this column"
    )]
    pub guidestripe: Option<u32>,
    #[arg(
        short = 'K',
        long = "rawsequences",
        help = "Fix numeric keypad key confusion problem"
    )]
    pub rawsequences: bool,
    #[arg(
        short = 'L',
        long = "nonewlines",
        help = "Don't add an automatic newline"
    )]
    pub nonewlines: bool,
    #[arg(
        short = 'M',
        long = "trimblanks",
        help = "Trim tail spaces when hard-wrapping"
    )]
    pub trimblanks: bool,
    #[arg(
        short = 'N',
        long = "noconvert",
        help = "Don't convert files from DOS/Mac format"
    )]
    pub noconvert: bool,
    #[arg(
        short = 'O',
        long = "bookstyle",
        help = "Leading whitespace means new paragraph"
    )]
    pub bookstyle: bool,
    #[arg(
        short = 'P',
        long = "positionlog",
        help = "Save & restore position of the cursor"
    )]
    pub positionlog: bool,
    #[arg(
        short = 'Q',
        long = "quotestr",
        value_name = "regex",
        help = "Regular expression to match quoting"
    )]
    pub quotestr: Option<String>,
    #[arg(
        short = 'R',
        long = "restricted",
        help = "Restrict access to the filesystem"
    )]
    pub restricted: bool,
    #[arg(
        short = 'S',
        long = "softwrap",
        help = "Display overlong lines on multiple rows"
    )]
    pub softwrap: bool,
    #[arg(
        short = 'T',
        long = "tabsize",
        value_name = "number",
        help = "Make a tab this number of columns wide"
    )]
    pub tabsize: Option<u32>,
    #[arg(
        short = 'U',
        long = "quickblank",
        help = "Wipe status bar upon next keystroke"
    )]
    pub quickblank: bool,
    #[arg(
        short = 'W',
        long = "wordbounds",
        help = "Detect word boundaries more accurately"
    )]
    pub wordbounds: bool,
    #[arg(
        short = 'X',
        long = "wordchars",
        value_name = "string",
        help = "Which other characters are word parts"
    )]
    pub wordchars: Option<String>,
    #[arg(
        short = 'Y',
        long = "syntax",
        value_name = "name",
        help = "Syntax definition to use for coloring"
    )]
    pub syntax: Option<String>,
    #[arg(
        short = 'Z',
        long = "zap",
        help = "Let Bsp and Del erase a marked region"
    )]
    pub zap: bool,
    #[arg(
        short = 'a',
        long = "atblanks",
        help = "When soft-wrapping, do it at whitespace"
    )]
    pub atblanks: bool,
    #[arg(
        short = 'b',
        long = "breaklonglines",
        overrides_with = "nowrap",
        help = "Automatically hard-wrap overlong lines"
    )]
    pub breaklonglines: bool,
    #[arg(
        short = 'c',
        long = "constantshow",
        help = "Constantly show cursor position"
    )]
    pub constantshow: bool,
    #[arg(
        short = 'd',
        long = "rebinddelete",
        help = "Fix Backspace/Delete confusion problem"
    )]
    pub rebinddelete: bool,
    #[arg(
        short = 'e',
        long = "emptyline",
        help = "Keep the line below the title bar empty"
    )]
    pub emptyline: bool,
    #[arg(
        short = 'f',
        long = "rcfile",
        value_name = "file",
        help = "Use only this file for configuring tico"
    )]
    pub rcfile: Option<String>,
    #[arg(
        short = 'g',
        long = "showcursor",
        help = "Show cursor in file browser & help text"
    )]
    pub showcursor: bool,
    #[arg(
        short = 'i',
        long = "autoindent",
        help = "Automatically indent new lines"
    )]
    pub autoindent: bool,
    #[arg(
        short = 'j',
        long = "jumpyscrolling",
        help = "Scroll per half-screen, not per line"
    )]
    pub jumpyscrolling: bool,
    #[arg(
        short = 'k',
        long = "cutfromcursor",
        help = "Cut from cursor to end of line"
    )]
    pub cutfromcursor: bool,
    #[arg(
        short = 'l',
        long = "linenumbers",
        help = "Show line numbers in front of the text"
    )]
    pub linenumbers: bool,
    #[arg(short = 'm', long = "mouse", help = "Enable the use of the mouse")]
    pub mouse: bool,
    #[arg(
        short = 'n',
        long = "noread",
        help = "Do not read the file (only write it)"
    )]
    pub noread: bool,
    #[arg(
        short = 'o',
        long = "operatingdir",
        value_name = "dir",
        help = "Set operating directory"
    )]
    pub operatingdir: Option<String>,
    #[arg(
        short = 'p',
        long = "preserve",
        help = "Preserve XON (^Q) and XOFF (^S) keys"
    )]
    pub preserve: bool,
    #[arg(
        short = 'q',
        long = "indicator",
        help = "Show a position+portion indicator"
    )]
    pub indicator: bool,
    #[arg(
        short = 'r',
        long = "fill",
        value_name = "number",
        help = "Set width for hard-wrap and justify"
    )]
    pub fill: Option<i32>,
    #[arg(
        short = 's',
        long = "speller",
        value_name = "program",
        help = "Use this alternative spell checker"
    )]
    pub speller: Option<String>,
    #[arg(
        short = 't',
        long = "saveonexit",
        help = "Save changes on exit, don't prompt"
    )]
    pub saveonexit: bool,
    #[arg(
        short = 'u',
        long = "unix",
        help = "Save a file by default in Unix format"
    )]
    pub unix: bool,
    #[arg(short = 'v', long = "view", help = "View mode (read-only)")]
    pub view: bool,
    #[arg(
        short = 'w',
        long = "nowrap",
        overrides_with = "breaklonglines",
        help = "Don't hard-wrap long lines [default]"
    )]
    pub nowrap: bool,
    #[arg(short = 'x', long = "nohelp", help = "Don't show the two help lines")]
    pub nohelp: bool,
    #[arg(
        short = 'y',
        long = "afterends",
        help = "Make Ctrl+Right stop at word ends"
    )]
    pub afterends: bool,
    #[arg(
        short = 'z',
        long = "listsyntaxes",
        help = "List the names of available syntaxes"
    )]
    pub listsyntaxes: bool,
    #[arg(
        short = '@',
        long = "colonparsing",
        help = "Accept 'filename:linenumber' notation"
    )]
    pub colonparsing: bool,
    #[arg(
        short = '%',
        long = "stateflags",
        help = "Show some states on the title bar"
    )]
    pub stateflags: bool,
    #[arg(
        short = '_',
        long = "minibar",
        help = "Show a feedback bar at the bottom"
    )]
    pub minibar: bool,
    #[arg(short = '0', long = "zero", help = "Hide all bars, use whole terminal")]
    pub zero: bool,
    #[arg(
        short = '/',
        long = "modernbindings",
        help = "Use better-known key bindings"
    )]
    pub modernbindings: bool,

    // tico-only options (no nano equivalent): long-form only and prefixed
    // `--tico-`, so they can never collide with a nano flag, present or
    // future.
    #[arg(
        long = "tico-theme",
        value_name = "[lang.]name",
        action = clap::ArgAction::Append,
        help = "Syntax-highlighting theme, overriding [syntax]'s `theme` (or, as `perl.NAME`, \
                only that language's); a built-in (tico-builtin-*), a Helix theme name, or \
                a .toml path. May be repeated."
    )]
    pub tico_theme: Vec<String>,
    #[arg(
        long = "tico-list-themes",
        help = "List the built-in and on-disk themes, and which ones the current configuration uses"
    )]
    pub tico_list_themes: bool,

    /// Files to edit, optionally preceded by +LINE[,COLUMN]. A name of `-`
    /// reads from standard input.
    #[arg(trailing_var_arg = true)]
    pub files: Vec<String>,
}

impl Cli {
    /// Apply the CLI flags on top of already-loaded config-file options
    /// (CLI overrides nanorc/ticorc, per nanorc(5): "command-line options
    /// override nanorc settings").
    pub fn apply(&self, options: &mut crate::options::Options) {
        macro_rules! flag {
            ($field:ident) => {
                if self.$field {
                    options.$field = true;
                }
            };
        }
        flag!(smarthome);
        flag!(backup);
        flag!(boldtext);
        flag!(tabstospaces);
        flag!(multibuffer);
        flag!(locking);
        flag!(historylog);
        flag!(rawsequences);
        flag!(nonewlines);
        flag!(trimblanks);
        flag!(noconvert);
        flag!(bookstyle);
        flag!(positionlog);
        flag!(restricted);
        flag!(softwrap);
        flag!(quickblank);
        flag!(wordbounds);
        flag!(zap);
        flag!(atblanks);
        flag!(constantshow);
        flag!(rebinddelete);
        flag!(emptyline);
        flag!(showcursor);
        flag!(autoindent);
        flag!(jumpyscrolling);
        flag!(cutfromcursor);
        flag!(linenumbers);
        flag!(mouse);
        flag!(preserve);
        flag!(indicator);
        flag!(saveonexit);
        flag!(unix);
        flag!(view);
        flag!(nohelp);
        flag!(afterends);
        flag!(colonparsing);
        flag!(minibar);
        flag!(zero);
        flag!(modernbindings);

        // nano's legacy `nowrap` is on by default; a nanorc that unsets
        // it asks for `breaklonglines`. Then `-w`/`-b` on the command line
        // have the last word, overriding the nanorc either way (when both
        // are given, whichever comes last: they override each other).
        if !options.nowrap {
            options.breaklonglines = true;
        }
        if self.nowrap {
            options.nowrap = true;
            options.breaklonglines = false;
        } else if self.breaklonglines {
            options.breaklonglines = true;
        }

        if self.stateflags {
            options.stateflags = true;
        }
        if let Some(v) = &self.backupdir {
            options.backupdir = Some(v.clone());
        }
        if let Some(v) = self.guidestripe {
            options.guidestripe = Some(v);
        }
        if let Some(v) = &self.quotestr {
            options.quotestr = v.clone();
        }
        if let Some(v) = self.tabsize {
            options.tabsize = v;
        }
        if let Some(v) = &self.wordchars {
            options.wordchars = Some(v.clone());
        }
        if let Some(v) = &self.syntax {
            options.syntax_name = Some(v.clone());
        }
        // `NAME` overrides `[syntax]`'s global `theme` only (per-language
        // overrides from the file still apply on top of it); `LANG.NAME`
        // overrides just that language, replacing any `LANG.theme` line.
        for v in &self.tico_theme {
            match crate::theme::split_language_theme(v) {
                Some((lang, name)) => options
                    .language_themes
                    .push((lang.to_ascii_lowercase(), name.to_string())),
                None => options.theme = Some(v.clone()),
            }
        }
        if let Some(v) = &self.operatingdir {
            options.operatingdir = Some(v.clone());
        }
        if let Some(v) = self.fill {
            options.fill = v;
        }
        if let Some(v) = &self.speller {
            options.speller = Some(v.clone());
        }
        options.ignorercfiles = self.ignorercfiles;
        options.rcfile = self.rcfile.clone();
    }
}

/// `set colonparsing` (`-@`): nano's reading of a `NAME:LINE` or
/// `NAME:LINE:COLUMN` argument naming a file that doesn't exist, as
/// `NAME` opened at that position -- but only when `NAME` itself does
/// exist (`exists`), trying the shorter `NAME:LINE` as the name first.
/// `Ok(None)` when the argument is to be taken as the plain name it is;
/// `Err` (nano dies with "Invalid number") when the digits don't make a
/// number.
pub fn split_colon_position(
    name: &str,
    exists: impl Fn(&str) -> bool,
) -> Result<Option<(String, i64, Option<i64>)>, std::num::ParseIntError> {
    if !name.contains(':') || exists(name) {
        return Ok(None);
    }
    let bytes = name.as_bytes();
    let mut coda = bytes.len();
    loop {
        // Back over the trailing digits, though never to the name's
        // first two bytes.
        loop {
            coda -= 1;
            if !(coda > 1 && bytes[coda].is_ascii_digit()) {
                break;
            }
        }
        if bytes[coda] != b':' || !bytes.get(coda + 1).is_some_and(u8::is_ascii_digit) {
            return Ok(None);
        }
        let base = &name[..coda];
        let numbers = &name[coda + 1..];
        if exists(base) {
            let mut parts = numbers.splitn(2, ':');
            let line = parts.next().unwrap_or_default().parse::<i64>()?;
            let column = parts.next().map(str::parse::<i64>).transpose()?;
            return Ok(Some((base.to_string(), line, column)));
        }
        // When this was the last colon, look for one before it.
        if numbers.contains(':') {
            return Ok(None);
        }
    }
}

/// Parsed positional argument: an optional `+LINE[,COLUMN]` followed by a
/// filename.
pub struct FileArg {
    pub path: String,
    pub line: Option<i64>,
    pub column: Option<i64>,
}

/// Interpret `cli.files` as nano does: a leading `+LINE[,COLUMN]` token
/// attaches to the filename that follows it.
pub fn parse_file_args(files: &[String]) -> Vec<FileArg> {
    let mut out = Vec::new();
    let mut pending: Option<(Option<i64>, Option<i64>)> = None;
    for f in files {
        if let Some(spec) = f.strip_prefix('+') {
            let (line_s, col_s) = match spec.split_once(',') {
                Some((l, c)) => (l, Some(c)),
                None => (spec, None),
            };
            let line = line_s.parse::<i64>().ok();
            let col = col_s.and_then(|c| c.parse::<i64>().ok());
            pending = Some((line, col));
            continue;
        }
        let (line, column) = pending.take().unwrap_or((None, None));
        out.push(FileArg {
            path: f.clone(),
            line,
            column,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(args: &[&str]) -> crate::options::Options {
        let mut argv = vec!["tico"];
        argv.extend_from_slice(args);
        let cli = Cli::try_parse_from(argv).expect("args parse");
        let mut options = crate::options::Options {
            theme: Some("from-rc".into()),
            language_themes: vec![("perl".into(), "rc-perl".into())],
            ..Default::default()
        };
        cli.apply(&mut options);
        options
    }

    #[test]
    fn colon_parsing_splits_off_a_line_and_column_like_nano() {
        let exists = |names: &'static [&'static str]| move |n: &str| names.contains(&n);
        let split = |name, names| split_colon_position(name, exists(names));
        let got = |name: &str, line, column| Ok(Some((name.to_string(), line, column)));
        assert_eq!(split("foo.c:12", &["foo.c"]), got("foo.c", 12, None));
        assert_eq!(split("foo.c:12:5", &["foo.c"]), got("foo.c", 12, Some(5)));
        // The shorter split is tried first.
        assert_eq!(
            split("foo.c:12:5", &["foo.c", "foo.c:12"]),
            got("foo.c:12", 5, None)
        );
        // A name that exists as it is, or whose base doesn't, stays whole.
        assert_eq!(split("foo.c:12", &["foo.c:12", "foo.c"]), Ok(None));
        assert_eq!(split("foo.c:12", &[]), Ok(None));
        assert_eq!(split("foo.c:1:2:3", &["foo.c"]), Ok(None));
        assert_eq!(split("foo.c:", &["foo.c"]), Ok(None));
        assert_eq!(split("foo.c:12x", &["foo.c"]), Ok(None));
        assert_eq!(split("a:5", &["a"]), got("a", 5, None));
        assert_eq!(split(":5", &[""]), Ok(None));
        assert!(split("foo.c:99999999999999999999", &["foo.c"]).is_err());
    }

    #[test]
    fn breaklonglines_follows_nowrap_in_the_nanorc_and_the_last_of_b_and_w() {
        assert!(!apply(&[]).breaklonglines);
        assert!(apply(&["-b"]).breaklonglines);
        assert!(!apply(&["-b", "-w"]).breaklonglines);
        assert!(apply(&["-w", "-b"]).breaklonglines);
        let from_rc = |nowrap, args: &[&str]| {
            let cli = Cli::try_parse_from(std::iter::once(&"tico").chain(args)).unwrap();
            let mut options = crate::options::Options {
                nowrap,
                ..Default::default()
            };
            cli.apply(&mut options);
            options.breaklonglines
        };
        // `unset nowrap` in a nanorc.
        assert!(from_rc(false, &[]));
        assert!(!from_rc(false, &["--nowrap"]));
        assert!(!from_rc(true, &[]));
    }

    #[test]
    fn tico_theme_plain_name_overrides_only_the_global_theme() {
        let o = apply(&["--tico-theme", "tico-builtin-nord"]);
        assert_eq!(o.theme.as_deref(), Some("tico-builtin-nord"));
        assert_eq!(
            o.language_themes,
            vec![("perl".to_string(), "rc-perl".to_string())]
        );
    }

    #[test]
    fn tico_theme_language_prefix_overrides_only_that_language() {
        let o = apply(&["--tico-theme", "perl.tico-builtin-catppuccin_mocha"]);
        assert_eq!(o.theme.as_deref(), Some("from-rc"));
        // Appended after the rc entry, so it wins under last-one-wins.
        assert_eq!(
            o.language_themes.last(),
            Some(&(
                "perl".to_string(),
                "tico-builtin-catppuccin_mocha".to_string()
            ))
        );
    }

    #[test]
    fn tico_theme_may_be_repeated_and_mixed() {
        let o = apply(&[
            "--tico-theme",
            "tico-builtin-gruvbox",
            "--tico-theme",
            "FORTRAN.tico-builtin-nord",
            "--tico-theme",
            "rust.~/mine.toml",
        ]);
        assert_eq!(o.theme.as_deref(), Some("tico-builtin-gruvbox"));
        assert_eq!(
            &o.language_themes[1..],
            &[
                ("fortran".to_string(), "tico-builtin-nord".to_string()),
                ("rust".to_string(), "~/mine.toml".to_string()),
            ]
        );
    }

    #[test]
    fn tico_theme_path_with_dots_is_not_mistaken_for_a_language() {
        for v in [
            "~/mine.toml",
            "./perl.toml",
            "/abs/x.y.toml",
            "klingon.nord",
        ] {
            let o = apply(&["--tico-theme", v]);
            assert_eq!(o.theme.as_deref(), Some(v), "{v}");
            assert_eq!(o.language_themes.len(), 1, "{v}");
        }
    }
}
