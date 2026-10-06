use clap::Parser;
use tico::{
    app, browser, buffer, cli, config, fileio, keymap, lockfile, options, syntax, theme, ui,
};

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    if cli.listsyntaxes {
        print_syntax_names();
        return Ok(());
    }
    let ignore_rcfiles = cli.ignorercfiles;
    let explicit_rcfile = cli.rcfile.as_deref();
    let loaded = config::load(explicit_rcfile, ignore_rcfiles, cli.modernbindings);
    let mut options = loaded.options;
    cli.apply(&mut options);

    // After config + CLI, so the summary reflects what an editing session
    // started with these same flags would actually use.
    if cli.tico_list_themes {
        print_themes(&options);
        return Ok(());
    }

    // Like nano (rnano), a name starting with 'r' means restricted mode.
    if std::env::args_os()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_name()
                .map(|n| n.to_string_lossy().starts_with('r'))
        })
        .unwrap_or(false)
    {
        options.restricted = true;
    }

    // nano: restricted mode turns off backups and both history files,
    // since they would write to files not named on the command line, and
    // takes an operating directory only from the command line; otherwise
    // a backup directory must exist, or nano refuses to start.
    if options.restricted {
        options.backup = false;
        options.historylog = false;
        options.positionlog = false;
        options.operatingdir = cli.operatingdir.clone();
    } else if let Some(dir) = options.backupdir.take() {
        match fileio::resolve_directory(&dir) {
            Some(full) => options.backupdir = Some(full),
            None => {
                print_config_warnings(&loaded.warnings);
                eprintln!("Invalid backup directory: {dir}");
                std::process::exit(1);
            }
        }
    }
    // nano's `init_operating_dir`: everything happens from inside it.
    if let Some(dir) = options.operatingdir.take() {
        match fileio::resolve_directory(&dir) {
            Some(full) if std::env::set_current_dir(&full).is_ok() => {
                options.operatingdir = Some(full);
            }
            _ => {
                print_config_warnings(&loaded.warnings);
                eprintln!("Invalid operating directory: {dir}");
                std::process::exit(1);
            }
        }
    }

    let mut warnings = loaded.warnings;
    let (theme, language_themes) = theme::resolve_themes(&options, &mut warnings);
    // nano reports the gist on the status bar at startup and the details
    // on stderr once the screen is restored. A theme problem has no file
    // to name, so it stands for itself.
    let startup_problem = loaded.startup_problem.or_else(|| warnings.first().cloned());

    let file_args = cli::parse_file_args(&cli.files);
    let mut editor = app::Editor::new(options, loaded.keymap);
    editor.theme = theme;
    editor.language_themes = language_themes;
    editor.buffers.clear();
    // Known up front (ui::run re-reads it) so a `+LINE` below can place
    // its viewport for the real screen height.
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        editor.screen_cols = cols as usize;
        editor.screen_rows = rows as usize;
    }

    let syntax_override = editor.options.syntax_name.clone();
    if file_args.is_empty() {
        let mut buf = buffer::Buffer::empty();
        buf.language = syntax::detect_with_override(None, "", syntax_override.as_deref());
        editor.buffers.push(buf);
    } else {
        for (i, fa) in file_args.iter().enumerate() {
            // A bare `-` reads standard input into an unnamed buffer,
            // matching nano: `echo foo | nano -`.
            // `set operatingdir`: a file outside it isn't opened at all.
            let opdir = editor.options.operatingdir.clone();
            if fa.path != "-"
                && fileio::outside_of_confinement(
                    opdir.as_deref(),
                    std::path::Path::new(&fa.path),
                    false,
                )
            {
                let opdir = opdir.unwrap_or_default();
                editor.set_status_alert(format!("Can't read file from outside of {opdir}"));
                continue;
            }
            let (mut buf, message, level) = if fa.path == "-" {
                open_stdin(&editor.options, syntax_override.as_deref())
            } else {
                let path = std::path::PathBuf::from(&fa.path);
                open_one(&path, &editor.options, syntax_override.as_deref())
            };
            if let Some(line) = fa.line {
                let target = (line.max(1) as usize) - 1;
                buf.cursor.line = target.min(buf.line_count().saturating_sub(1));
                if let Some(col) = fa.column {
                    buf.cursor.col = (col.max(1) as usize) - 1;
                }
            }
            // The status message reflects whichever buffer ends up focused
            // (the first one), matching nano showing the "Read N lines"
            // blurb for the file that lands in the active edit window.
            if i == 0 {
                // `set minibar`'s one-shot line-count note mirrors nano's
                // own `report_size = TRUE`: it fires for a real, existing
                // file that was read, but not for a brand-new (nonexistent)
                // file, which gets no blurb of any kind under minibar.
                // And under minibar, a startup load shows *only* that note
                // -- not also the ordinary "Read N lines" status blurb,
                // confirmed against the installed nano's own escape-code
                // output (`we_are_running` is false at nano's own startup,
                // which unconditionally suppresses that blurb there).
                let is_fresh_read = level == app::StatusLevel::Normal && message != "New File";
                if is_fresh_read {
                    editor.minibar_note = Some(app::minibar_linecount_note(
                        buf.nano_line_count(),
                        buf.format,
                    ));
                }
                if !(is_fresh_read && editor.options.minibar) {
                    match level {
                        app::StatusLevel::Alert => editor.set_status_alert(message),
                        app::StatusLevel::Mild => editor.set_status_mild(message),
                        app::StatusLevel::Normal => editor.set_status(message),
                    }
                }
            }
            // Only the focused buffer gets an interactive "someone else is
            // editing this" prompt; for any others opened at the same time,
            // this mirrors nano's non-interactive ask_the_user=false path
            // (warn-lessly take over the lock) rather than chaining several
            // blocking prompts before the UI has even started.
            if let Some(prompt) = acquire_lock(&mut editor, &mut buf, i == 0) {
                editor.mode = app::Mode::Prompt(prompt);
            }
            editor.buffers.push(buf);
            if fa.line.is_some() {
                editor.current = editor.buffers.len() - 1;
                editor.place_viewport_for_cli_goto();
            } else if fa.path != "-" {
                // `set positionlog`; a `+LINE` replaces it, as in nano.
                editor.current = editor.buffers.len() - 1;
                editor.restore_position();
            }
        }
    }
    if editor.buffers.is_empty() {
        let mut buf = buffer::Buffer::empty();
        buf.language = syntax::detect_with_override(None, "", syntax_override.as_deref());
        editor.buffers.push(buf);
    }
    editor.current = 0;
    if let Some(problem) = startup_problem {
        editor.set_status_alert(problem);
    }

    let result = ui::run(&mut editor);
    print_config_warnings(&warnings);
    result?;

    if editor.options.historylog {
        editor.history.save();
    }
    Ok(())
}

/// nano's `display_rcfile_errors`: once the terminal is back to normal
/// (or before bailing out early), list every configuration problem on
/// stderr, one per line.
fn print_config_warnings(warnings: &[String]) {
    for w in warnings {
        eprintln!("{w}");
    }
}

/// Print the names of tico's built-in syntax-highlighting languages, for
/// `-z`/`--listsyntaxes`. Unlike nano, these aren't read from nanorc `syntax`
/// definitions (tico intentionally ignores those; see the syntax module),
/// so this lists the fixed, compiled-in language registry instead, wrapped
/// to the real terminal width rather than nano's hardcoded 45 columns.
/// `--tico-list-themes`: the built-in themes, any found on the search
/// path, and a summary of the active `[syntax]` configuration with each
/// configured theme test-loaded so a typo shows up here rather than as a
/// warning flashing past at editor startup.
fn print_themes(options: &options::Options) {
    let loader = theme::Loader::with_default_dirs();

    println!("Built-in themes:");
    for name in theme::builtin_names() {
        println!("  {name}");
    }

    let on_disk = loader.disk_themes();
    if !on_disk.is_empty() {
        println!("\nThemes found on disk:");
        let width = on_disk.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (name, path) in &on_disk {
            println!("  {name:width$}  {}", path.display());
        }
    }

    println!("\nActive configuration:");
    let status = |name: &str| -> String {
        let mut warnings = Vec::new();
        match loader.load(name, &mut warnings) {
            Some(_) if warnings.is_empty() => String::new(),
            Some(_) => format!("  (loads with {} warning(s))", warnings.len()),
            None => "  (ERROR: cannot be loaded; the default will be used instead)".to_string(),
        }
    };
    let width = options
        .language_themes
        .iter()
        .map(|(l, _)| l.len())
        .max()
        .unwrap_or(0)
        .max("default".len());
    match options.theme.as_deref() {
        Some(name) => println!("  {:width$}  {name}{}", "default", status(name)),
        None => println!(
            "  {:width$}  {}  (nothing configured)",
            "default",
            theme::DEFAULT_THEME_NAME
        ),
    }
    // Later lines for the same language override earlier ones, so show
    // only the effective entry for each, in the order first mentioned.
    let mut seen: Vec<&str> = Vec::new();
    for (lang, _) in &options.language_themes {
        if seen.contains(&lang.as_str()) {
            continue;
        }
        seen.push(lang);
        let name = options
            .language_themes
            .iter()
            .rev()
            .find(|(l, _)| l == lang)
            .map(|(_, n)| n.as_str())
            .unwrap_or_default();
        println!("  {lang:width$}  {name}{}", status(name));
    }
}

fn print_syntax_names() {
    println!("Available syntaxes:");
    // nano wraps this listing at a hardcoded 45 columns regardless of the
    // actual terminal size; wrap to the real width instead (falling back
    // to 80 when it can't be queried, e.g. output is piped to a file).
    let width = crossterm::terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(80)
        .max(10);
    let mut line = String::new();
    for name in syntax::names() {
        let extra = 1 + name.chars().count(); // leading space + the name itself
        if !line.is_empty() && line.chars().count() + extra > width {
            println!("{line}");
            line.clear();
        }
        line.push(' ');
        line.push_str(name);
    }
    if !line.is_empty() {
        println!("{line}");
    }
}

/// When `locking` is on, check for (and take) a vim-style lock on `buf`'s
/// file, matching nano's `do_lockfile()`. If another lock is already held
/// and `interactive` is true, returns a confirmation prompt instead of
/// acquiring the lock immediately (the caller must set it as the editor's
/// mode); otherwise a conflicting lock is taken over anyway, as nano itself
/// does in its non-interactive (`ask_the_user = FALSE`) path.
fn acquire_lock(
    editor: &mut app::Editor,
    buf: &mut buffer::Buffer,
    interactive: bool,
) -> Option<app::Prompt> {
    if !editor.options.locking || editor.options.view {
        return None;
    }
    let path = buf.path.clone()?;
    let lock_path = lockfile::lock_path(&path);
    let target = path.display().to_string();
    match lockfile::check_lock(&lock_path) {
        lockfile::LockCheck::None => {
            let _ = lockfile::write_lock(&lock_path, &target, false);
            buf.lock_filename = Some(lock_path);
            None
        }
        lockfile::LockCheck::Bad => {
            // nano warns and leaves the file open without taking a lock,
            // rather than overwriting a lock file it doesn't understand.
            editor.set_status_alert(format!("Bad lock file is ignored: {}", lock_path.display()));
            None
        }
        lockfile::LockCheck::Held(info) if interactive => Some(app::Prompt {
            kind: app::PromptKind::LockConflict { lock_path, target },
            menu: keymap::Menu::YesNo,
            label: format!(
                "File {} is being edited by {} (with {}, PID {}); open anyway?",
                path.display(),
                info.user,
                info.program,
                info.pid
            ),
            input: String::new(),
            cursor: 0,
            history_pos: None,
            saved_input: None,
        }),
        lockfile::LockCheck::Held(_) => {
            let _ = lockfile::write_lock(&lock_path, &target, false);
            buf.lock_filename = Some(lock_path);
            None
        }
    }
}

/// Read standard input into an unnamed buffer (`nano -`, `echo foo | tico
/// -`). Matches nano's `scoop_stdin()`: no path is attached (so writing it
/// out needs a filename, same as any other new buffer), and the buffer is
/// marked modified whenever it got any content — read-but-unsaved input
/// isn't "clean" the way a file freshly loaded from disk is.
fn open_stdin(
    opts: &options::Options,
    syntax_override: Option<&str>,
) -> (buffer::Buffer, String, app::StatusLevel) {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        eprintln!("Reading data from keyboard; type ^D or ^D^D to finish.");
    }
    let mut content = String::new();
    let (mut buf, msg, level) = match std::io::stdin().read_to_string(&mut content) {
        Ok(_) => {
            // Same line-ending conversion as a file (nano's scoop_stdin
            // feeds read_file too).
            let (content, detected) = fileio::convert_line_endings(&content, opts.noconvert);
            let msg = fileio::describe_read(&content, detected);
            let mut content = content;
            fileio::with_magic_line(&mut content, opts.nonewlines);
            let mut buf = buffer::Buffer::from_text(&content, None);
            buf.adopt_format(detected, opts.unix);
            if !content.is_empty() {
                buf.modified = true;
            }
            (buf, msg, app::StatusLevel::Normal)
        }
        Err(e) => (
            buffer::Buffer::empty(),
            format!("Failed to open stdin: {e}"),
            app::StatusLevel::Alert,
        ),
    };
    buf.language = syntax::detect_with_override(None, &buf.to_string(), syntax_override);
    (buf, msg, level)
}

/// Load one file argument as nano would: refuse to open directories (an
/// empty "New Buffer" is used instead), warn (but still load) files that
/// exist but aren't writable, and — when `locking` is on, matching nano's
/// `ISSET(LOCKING)`-gated check in `has_valid_path()` — warn when a new
/// file's containing directory isn't writable either. Returns (buffer,
/// status message, message severity).
fn open_one(
    path: &std::path::Path,
    opts: &options::Options,
    syntax_override: Option<&str>,
) -> (buffer::Buffer, String, app::StatusLevel) {
    let (mut buf, msg, level) = open_one_inner(path, opts);
    // Detected once at load time (extension/filename -> shebang -> modeline,
    // all on by default, or forced by -Y/--syntax); the on/off toggle (M-Y)
    // only controls whether rendering actually uses it, so toggling back on
    // doesn't need to re-detect.
    buf.language =
        syntax::detect_with_override(buf.path.as_deref(), &buf.to_string(), syntax_override);
    (buf, msg, level)
}

fn open_one_inner(
    path: &std::path::Path,
    opts: &options::Options,
) -> (buffer::Buffer, String, app::StatusLevel) {
    let locking = opts.locking;
    if path.is_dir() {
        return (
            buffer::Buffer::empty(),
            format!("'{}' is a directory", path.display()),
            app::StatusLevel::Alert,
        );
    }
    if !path.exists() {
        if locking {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            if parent.is_dir() && !fileio::path_writable(parent) {
                return (
                    buffer::Buffer::from_text("", Some(path.to_path_buf())),
                    format!("Directory '{}' is not writable", parent.display()),
                    app::StatusLevel::Mild,
                );
            }
        }
        return (
            buffer::Buffer::from_text("", Some(path.to_path_buf())),
            "New File".to_string(),
            app::StatusLevel::Normal,
        );
    }
    match fileio::load_file(path, opts) {
        Ok(fileio::LoadedFile {
            buffer: buf,
            detected,
        }) => {
            if !fileio::path_writable(path) {
                (
                    buf,
                    format!("File '{}' is unwritable", path.display()),
                    app::StatusLevel::Alert,
                )
            } else {
                let msg = fileio::describe_read(&buf.to_string(), detected);
                (buf, msg, app::StatusLevel::Normal)
            }
        }
        Err(e) => (
            buffer::Buffer::from_text("", Some(path.to_path_buf())),
            format!(
                "Error reading {}: {}",
                path.display(),
                browser::strerror(&e)
            ),
            app::StatusLevel::Alert,
        ),
    }
}
