//! Configuration loading: `~/.nanorc` (nano-format, syntax-highlighting
//! directives ignored) and `~/.ticorc` (tico's own `[main]`/`[keybindings]`/
//! `[syntax]` INI-style format), applied in that precedence order (ticorc
//! wins on conflicts), and finally overridden by CLI flags.

pub mod nanorc;
mod settings;
pub mod ticorc;

use crate::keymap::KeyMap;
use crate::options::Options;
use std::path::{Path, PathBuf};

pub struct LoadedConfig {
    pub options: Options,
    pub keymap: KeyMap,
    /// Every problem found, worded like nano's own `jot_error` lines
    /// (`Error in FILE on line N: ...`), to be printed on exit.
    pub warnings: Vec<String>,
    /// nano's `startup_problem`: "Mistakes in 'FILE'" for the first file
    /// that had any, shown as an alert on the status bar at startup.
    pub startup_problem: Option<String>,
}

/// Reword the warnings `parse` just added for `path` -- tagged
/// `nanorc:N: ...` / `ticorc:N: ...` -- the way nano reports them
/// (`Error in FILE on line N: ...`), and note the file as the startup
/// problem if it's the first one with any.
fn attribute_warnings(
    warnings: &mut [String],
    path: &std::path::Path,
    startup_problem: &mut Option<String>,
) {
    if warnings.is_empty() {
        return;
    }
    let file = path.display();
    for w in warnings.iter_mut() {
        let lineno = w
            .split_once(':')
            .and_then(|(_, rest)| rest.split_once(": "))
            .filter(|(n, _)| n.parse::<usize>().is_ok());
        *w = match lineno {
            Some((n, msg)) => format!("Error in {file} on line {n}: {msg}"),
            None => format!("Error in {file}: {w}"),
        };
    }
    startup_problem.get_or_insert_with(|| format!("Mistakes in '{file}'"));
}

/// The system config directory, baked in at build time by `build.rs`
/// (mirrors nano's `--sysconfdir` configure option). Empty means system-wide
/// config lookup was disabled at build time (`TICO_SYSCONFDIR=` when
/// building); see `build.rs` for how to override it.
const SYSCONFDIR: &str = env!("TICO_SYSCONFDIR");

/// The system-wide nanorc path (`$(sysconfdir)/nanorc`, e.g. `/etc/nanorc`),
/// or `None` if system-wide config lookup was disabled at build time.
fn system_nanorc_path() -> Option<PathBuf> {
    if SYSCONFDIR.is_empty() {
        None
    } else {
        Some(PathBuf::from(SYSCONFDIR).join("nanorc"))
    }
}

fn user_nanorc_paths() -> Vec<PathBuf> {
    nanorc_search_order(
        dirs::home_dir().as_deref(),
        std::env::var_os("XDG_CONFIG_HOME")
            .as_deref()
            .map(Path::new),
    )
}

/// Where nano 8.7.1's `do_rcfiles` looks for the user's nanorc, in order
/// (the first one that exists is the only one read): `~/.nanorc`, then
/// `$XDG_CONFIG_HOME/nano/nanorc` when that variable is set, then
/// `~/.config/nano/nanorc` -- the last checked even with
/// `$XDG_CONFIG_HOME` pointing elsewhere.
fn nanorc_search_order(home: Option<&Path>, xdg_config_home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home {
        paths.push(home.join(".nanorc"));
    }
    if let Some(xdg) = xdg_config_home {
        paths.push(xdg.join("nano/nanorc"));
    }
    if let Some(home) = home {
        paths.push(home.join(".config/nano/nanorc"));
    }
    paths
}

fn ticorc_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg).join("tico/ticorc");
        if p.exists() {
            return Some(p);
        }
    }
    dirs::home_dir().map(|h| h.join(".ticorc"))
}

/// Load configuration: system nanorc, then the first user nanorc found
/// (see `nanorc_search_order`), then
/// `~/.ticorc` (which takes precedence over nanorc on conflicting settings).
///
/// If `explicit_rcfile` is `Some`, only that single file is read (mirrors
/// nano's `--rcfile`), and `~/.ticorc` is still applied afterward unless
/// `ignore_ticorc` is set.
///
/// `modern` is `-/`/`--modernbindings`: CLI-only in nano (it has no `set`
/// form), so it's threaded in from the caller rather than read from a
/// parsed option, and applied to the keymap *before* any nanorc/ticorc
/// `bind`/`unbind` directives, so those can still override individual
/// modern-mode bindings — matching nano's own `global_init()`, which bakes
/// modernbindings into the initial shortcut list before `parse_rcfile()`.
/// `preserve` is `-p`/`--preserve` (but not a nanorc's `set preserve`),
/// which leaves `^S`/`^Q` out of that same initial list; see
/// `KeyMap::drop_flow_control_keys`.
pub fn load(
    explicit_rcfile: Option<&str>,
    ignore_rcfiles: bool,
    modern: bool,
    preserve: bool,
) -> LoadedConfig {
    let mut options = Options::default();
    let mut keymap = KeyMap::defaults(modern);
    if preserve && !modern {
        keymap.drop_flow_control_keys();
    }
    let mut warnings = Vec::new();
    let mut startup_problem = None;

    if ignore_rcfiles {
        return LoadedConfig {
            options,
            keymap,
            warnings,
            startup_problem,
        };
    }

    let mut parse_nanorc = |path: &std::path::Path,
                            text: &str,
                            options: &mut Options,
                            keymap: &mut KeyMap,
                            warnings: &mut Vec<String>| {
        let before = warnings.len();
        nanorc::parse(text, options, keymap, warnings);
        attribute_warnings(&mut warnings[before..], path, &mut startup_problem);
    };

    if let Some(path) = explicit_rcfile {
        if let Ok(text) = std::fs::read_to_string(path) {
            parse_nanorc(
                std::path::Path::new(path),
                &text,
                &mut options,
                &mut keymap,
                &mut warnings,
            );
        } else {
            warnings.push(format!("could not read rcfile: {path}"));
        }
    } else {
        // System-wide file (unless disabled at build time), always read if
        // present.
        if let Some(sys_path) = system_nanorc_path()
            && let Ok(text) = std::fs::read_to_string(&sys_path)
        {
            parse_nanorc(&sys_path, &text, &mut options, &mut keymap, &mut warnings);
        }
        // First user nanorc found, in nano's documented search order.
        for path in user_nanorc_paths() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                parse_nanorc(&path, &text, &mut options, &mut keymap, &mut warnings);
                break;
            }
        }
    }

    if let Some(path) = ticorc_path()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let before = warnings.len();
        ticorc::parse(&text, &mut options, &mut keymap, &mut warnings);
        attribute_warnings(&mut warnings[before..], &path, &mut startup_problem);
    }

    LoadedConfig {
        options,
        keymap,
        warnings,
        startup_problem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_nanorc_is_searched_for_in_nanos_order() {
        let home = Path::new("/home/me");
        assert_eq!(
            nanorc_search_order(Some(home), Some(Path::new("/xdg"))),
            [
                PathBuf::from("/home/me/.nanorc"),
                PathBuf::from("/xdg/nano/nanorc"),
                PathBuf::from("/home/me/.config/nano/nanorc"),
            ]
        );
        assert_eq!(
            nanorc_search_order(Some(home), None),
            [
                PathBuf::from("/home/me/.nanorc"),
                PathBuf::from("/home/me/.config/nano/nanorc"),
            ]
        );
        assert_eq!(
            nanorc_search_order(None, Some(Path::new("/xdg"))),
            [PathBuf::from("/xdg/nano/nanorc")]
        );
    }

    #[test]
    fn warnings_are_reworded_like_nanos_and_the_first_file_is_the_problem() {
        // Confirmed against the installed nano 8.7.1's stderr on exit and
        // its startup status-bar alert.
        let mut problem = None;
        let mut first = vec![
            "nanorc:1: Even number of characters required".to_string(),
            "nanorc:3: Unknown option: bogus".to_string(),
        ];
        attribute_warnings(&mut first, std::path::Path::new("/a/.nanorc"), &mut problem);
        assert_eq!(
            first,
            [
                "Error in /a/.nanorc on line 1: Even number of characters required",
                "Error in /a/.nanorc on line 3: Unknown option: bogus",
            ]
        );
        let mut second = vec!["ticorc:2: Unknown option: x".to_string()];
        attribute_warnings(
            &mut second,
            std::path::Path::new("/a/.ticorc"),
            &mut problem,
        );
        assert_eq!(second, ["Error in /a/.ticorc on line 2: Unknown option: x"]);
        assert_eq!(problem.as_deref(), Some("Mistakes in '/a/.nanorc'"));
    }

    #[test]
    fn a_file_without_warnings_is_not_a_problem() {
        let mut problem = None;
        attribute_warnings(&mut [], std::path::Path::new("/a/.nanorc"), &mut problem);
        assert_eq!(problem, None);
    }
}
