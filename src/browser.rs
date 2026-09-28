//! The file browser (`^T` at the Read File and Write Out prompts): nano's
//! src/browser.c, minus the curses drawing, which lives in `ui.rs`.
//!
//! A [`Browser`] is one directory's listing -- sorted, with its selection
//! -- plus the pure layout and navigation math nano's `browse()` loop
//! does on it. Everything that depends on the screen size takes the
//! column/row counts as parameters rather than storing them, so a
//! terminal resize just redraws with the new numbers.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keymap::Action;

/// One name in the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The directory's own path joined with `name` -- not resolved any
    /// further, the way nano's `filelist` holds `path` + `d_name`.
    pub path: PathBuf,
    pub name: String,
    /// Whether it is (or, for a symlink, points to) a directory: what
    /// nano's `diralphasort` sorts by and what Enter descends into.
    pub is_dir: bool,
    /// The right-aligned column: a size, "(dir)", "(parent dir)" or "--".
    pub info: String,
}

#[derive(Debug, Clone)]
pub struct Browser {
    /// nano's `present_path`: absolute, symlinks resolved.
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    /// Index into `entries`; always valid while `entries` is non-empty.
    pub selected: usize,
    /// The widest name, in columns (nano derives `gauge` from it).
    widest: usize,
}

/// What a filename search (`findfile`) turned up, for its status message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindOutcome {
    Found,
    /// Found, after going past the end (or start) of the list.
    Wrapped,
    /// Only the name that was already selected matches.
    OnlyOccurrence,
    NotFound,
}

impl Browser {
    /// Read the directory `dir` (which should already be absolute and
    /// resolved -- see [`full_dir_path`]), with nothing selected but the
    /// first entry. nano's `read_the_list`.
    pub fn read(dir: &Path) -> std::io::Result<Browser> {
        let mut entries = Vec::new();
        // readdir(3) hands nano ".." (and "."), which it keeps (and skips);
        // `read_dir` gives neither, so ".." is put back by hand.
        entries.push(make_entry(dir, ".."));
        for item in std::fs::read_dir(dir)? {
            let Ok(item) = item else { continue };
            let name = item.file_name().to_string_lossy().into_owned();
            entries.push(make_entry(dir, &name));
        }
        entries.sort_by(diralphasort);
        let widest = entries.iter().map(|e| e.name.width()).max().unwrap_or(0);
        Ok(Browser {
            dir: dir.to_path_buf(),
            entries,
            selected: 0,
            widest,
        })
    }

    /// The width of one column of the listing: the widest name plus ten
    /// (blanks plus the size), at least room for ".. (parent dir)", and
    /// never wider than the screen.
    pub fn gauge(&self, cols: usize) -> usize {
        (self.widest + 10).max(15).min(cols).max(1)
    }

    /// How many names fit on one screen row: "feigning room for two spaces
    /// beyond the right edge, and adding two spaces of padding between
    /// columns".
    pub fn piles(&self, cols: usize) -> usize {
        ((cols + 2) / (self.gauge(cols) + 2)).max(1)
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Select the entry whose path is `path`, if it is (still) listed;
    /// otherwise nudge the selection back one so the change gets noticed,
    /// staying in range. nano's `reselect`.
    pub fn reselect(&mut self, path: &Path) {
        if let Some(i) = self.entries.iter().position(|e| e.path == path) {
            self.selected = i;
        } else {
            self.selected = self
                .selected
                .saturating_sub(1)
                .min(self.entries.len().saturating_sub(1));
        }
    }

    /// Select `path` if it is listed (without the fallback nudge of
    /// [`reselect`](Self::reselect)) -- Go To Directory's highlighting of a
    /// directory it then fails to enter.
    pub fn select_if_listed(&mut self, path: &Path) {
        if let Some(i) = self.entries.iter().position(|e| e.path == path) {
            self.selected = i;
        }
    }

    /// Apply one of the browser's movement functions, with `piles` names
    /// per row and `rows` rows per screen -- the arithmetic of nano's
    /// `browse()` loop, verbatim. Returns false for an action that isn't a
    /// movement.
    pub fn navigate(&mut self, action: Action, piles: usize, rows: usize) -> bool {
        let len = self.entries.len();
        if len == 0 {
            return matches!(
                action,
                Action::Left
                    | Action::Right
                    | Action::PrevWord
                    | Action::NextWord
                    | Action::Up
                    | Action::Down
                    | Action::PrevBlock
                    | Action::NextBlock
                    | Action::PageUp
                    | Action::PageDown
                    | Action::FirstFile
                    | Action::LastFile
            );
        }
        let piles = piles.max(1);
        let page = rows.max(1) * piles;
        let last = len - 1;
        let s = &mut self.selected;
        match action {
            Action::Left => *s = s.saturating_sub(1),
            Action::Right => {
                if *s < last {
                    *s += 1;
                }
            }
            // "Left Column" / "Right Column".
            Action::PrevWord => *s -= *s % piles,
            Action::NextWord => *s = (*s + piles - 1 - *s % piles).min(last),
            Action::Up => {
                if *s >= piles {
                    *s -= piles;
                }
            }
            Action::Down => {
                if *s + piles <= last {
                    *s += piles;
                }
            }
            // "Top Row" / "Bottom Row" of the current screenful.
            Action::PrevBlock => *s = (*s / page) * page + *s % piles,
            Action::NextBlock => {
                *s = (*s / page) * page + *s % piles + page - piles;
                if *s >= len {
                    *s = (len / piles) * piles + *s % piles;
                }
                if *s >= len {
                    *s -= piles;
                }
            }
            Action::PageUp => {
                if *s < piles {
                    *s = 0;
                } else if *s < page {
                    *s %= piles;
                } else {
                    *s -= page;
                }
            }
            Action::PageDown => {
                if *s + piles >= last {
                    *s = last;
                } else if *s + page >= len {
                    *s = (*s + page - len) % piles + len - piles;
                } else {
                    *s += page;
                }
            }
            Action::FirstFile => *s = 0,
            Action::LastFile => *s = last,
            _ => return false,
        }
        true
    }

    /// Look for `needle` (case-insensitively, in the names only) starting
    /// just past the selection and wrapping around, selecting the first
    /// match. nano's `findfile`.
    pub fn find(&mut self, needle: &str, forwards: bool) -> FindOutcome {
        let len = self.entries.len();
        if len == 0 {
            return FindOutcome::NotFound;
        }
        let needle = needle.to_lowercase();
        let began_at = self.selected;
        let mut wrapped = false;
        loop {
            if forwards {
                if self.selected == len - 1 {
                    self.selected = 0;
                    wrapped = true;
                } else {
                    self.selected += 1;
                }
            } else if self.selected == 0 {
                self.selected = len - 1;
                wrapped = true;
            } else {
                self.selected -= 1;
            }

            if self.entries[self.selected]
                .name
                .to_lowercase()
                .contains(&needle)
            {
                return if self.selected == began_at {
                    FindOutcome::OnlyOccurrence
                } else if wrapped {
                    FindOutcome::Wrapped
                } else {
                    FindOutcome::Found
                };
            }
            if self.selected == began_at {
                return FindOutcome::NotFound;
            }
        }
    }

    /// The index of the first name on the screenful holding the selection.
    pub fn first_shown(&self, cols: usize, rows: usize) -> usize {
        let page = rows.max(1) * self.piles(cols);
        self.selected - self.selected % page
    }

    /// The screen rows of the listing (at most `rows` of them, each exactly
    /// `cols` columns wide), as runs of text with whether each run is the
    /// highlighted selection. nano's `browser_refresh`.
    pub fn render_rows(&self, cols: usize, rows: usize) -> Vec<Vec<(String, bool)>> {
        let gauge = self.gauge(cols);
        let piles = self.piles(cols);
        let first = self.first_shown(cols, rows);
        let mut out = Vec::new();
        for row in 0..rows {
            let mut segments: Vec<(String, bool)> = Vec::new();
            let mut used = 0;
            for pile in 0..piles {
                let index = first + row * piles + pile;
                let Some(entry) = self.entries.get(index) else {
                    break;
                };
                if pile > 0 {
                    push_plain(&mut segments, "  ");
                    used += 2;
                }
                let cell = entry_cell(entry, gauge, cols);
                if index == self.selected {
                    segments.push((cell, true));
                } else {
                    push_plain(&mut segments, &cell);
                }
                used += gauge;
            }
            if used < cols {
                push_plain(&mut segments, &" ".repeat(cols - used));
            }
            out.push(segments);
        }
        out
    }

    /// Where the selected name's cell starts on screen (row within the
    /// listing, column) -- for `set showcursor`.
    pub fn selected_cell(&self, cols: usize, rows: usize) -> (usize, usize) {
        let piles = self.piles(cols);
        let offset = self.selected - self.first_shown(cols, rows);
        (offset / piles, (offset % piles) * (self.gauge(cols) + 2))
    }

    /// The entry a mouse click at (`row`, `col`) within the listing picks:
    /// nano's click arithmetic in `browse()`.
    pub fn index_at(&self, cols: usize, rows: usize, row: usize, col: usize) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }
        let gauge = self.gauge(cols);
        let piles = self.piles(cols);
        let mut index = self.first_shown(cols, rows) + row * piles + col / (gauge + 2);
        // Beyond the end of a row: the row's last name.
        if col > piles * (gauge + 2) {
            index = index.saturating_sub(1);
        }
        Some(index.min(len - 1))
    }
}

fn push_plain(segments: &mut Vec<(String, bool)>, text: &str) {
    match segments.last_mut() {
        Some((s, false)) => s.push_str(text),
        _ => segments.push((text.to_string(), false)),
    }
}

/// One name as shown in the listing, exactly `gauge` columns: the name at
/// the left (or "..." plus its tail, when it would run into the info
/// column), and the info right-aligned.
fn entry_cell(entry: &Entry, gauge: usize, cols: usize) -> String {
    const INFOMAXLEN: usize = 7;
    let namelen = entry.name.width();
    // No room is wasted on dots when there are fewer than 15 columns.
    let dots = cols >= 15 && namelen + INFOMAXLEN >= gauge;
    let mut cell: Vec<char> = Vec::with_capacity(gauge);
    if dots {
        cell.extend("...".chars());
        // Skip leading columns so that the rest, plus the dots and one
        // space of padding, leaves room for the info.
        let skip = namelen + INFOMAXLEN + 4 - gauge;
        push_columns(&mut cell, &entry.name, skip, gauge);
    } else {
        push_columns(&mut cell, &entry.name, 0, gauge);
    }
    cell.truncate(gauge);
    while cell.len() < gauge {
        cell.push(' ');
    }

    // "(parent dir)" may take up to twelve columns; anything else, seven.
    let infomax = if entry.name == ".." { 12 } else { INFOMAXLEN };
    let info: String = take_columns(&entry.info, infomax);
    let infolen = info.width();
    let start = gauge.saturating_sub(infolen);
    let info_chars: Vec<char> = info.chars().collect();
    for (i, c) in info_chars.into_iter().enumerate() {
        if start + i < gauge {
            cell[start + i] = c;
        }
    }
    cell.into_iter().collect()
}

/// Append `text` to `cell`, starting `skip` columns into it and stopping
/// once `cell` would exceed `limit` columns; a wide character is replaced
/// by blanks so that every `char` in `cell` stays one column (the info is
/// overlaid by index).
fn push_columns(cell: &mut Vec<char>, text: &str, skip: usize, limit: usize) {
    let mut col = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(1);
        if col >= skip {
            if cell.len() + w > limit {
                break;
            }
            if w == 1 {
                cell.push(c);
            } else {
                cell.extend(std::iter::repeat_n(' ', w));
            }
        }
        col += w;
    }
}

fn take_columns(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(1);
        if used + w > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out
}

fn make_entry(dir: &Path, name: &str) -> Entry {
    let path = dir.join(name);
    let (is_dir, info) = match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.file_type().is_symlink() => {
            if meta.is_dir() {
                let info = if name == ".." {
                    "(parent dir)"
                } else {
                    "(dir)"
                };
                (true, info.to_string())
            } else {
                (false, size_info(meta.len()))
            }
        }
        // A symlink (or something that vanished): "(dir)" only when it
        // leads to a directory, "--" otherwise.
        _ => {
            if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                (true, "(dir)".to_string())
            } else {
                (false, "--".to_string())
            }
        }
    };
    Entry {
        path,
        name: name.to_string(),
        is_dir,
        info,
    }
}

/// A file size the way the browser shows it: `"%4ju %cB"` after scaling
/// to the largest unit that keeps the number under 1024, or "(huge)" from
/// a terabyte up.
pub fn size_info(size: u64) -> String {
    let (value, unit) = if size < 1 << 10 {
        (size, ' ')
    } else if size < 1 << 20 {
        (size >> 10, 'K')
    } else if size < 1 << 30 {
        (size >> 20, 'M')
    } else {
        (size >> 30, 'G')
    };
    if value < 1 << 10 {
        format!("{value:>4} {unit}B")
    } else {
        "(huge)".to_string()
    }
}

/// nano's `diralphasort`: directories before everything else, then by
/// name ignoring case, with names that are equal but for case in byte
/// order.
fn diralphasort(a: &Entry, b: &Entry) -> Ordering {
    b.is_dir.cmp(&a.is_dir).then_with(|| {
        let folded = a
            .name
            .chars()
            .flat_map(char::to_lowercase)
            .cmp(b.name.chars().flat_map(char::to_lowercase));
        folded.then_with(|| a.name.cmp(&b.name))
    })
}

/// nano's `get_full_path` for a directory: `path` made absolute with
/// symlinks and `..` resolved.
pub fn full_dir_path(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify_verbatim)
}

/// `canonicalize` on Windows hands back a `\\?\C:\...` verbatim path;
/// show (and keep) the ordinary form when there is one.
fn simplify_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return PathBuf::from(rest.to_string());
        }
    }
    path
}

/// Where browsing starts for a prompt answer: the answer itself when it
/// names a directory, else the directory part of it, else the working
/// directory. nano's `browse_in`. `Err` holds the message for when not
/// even the working directory can be found.
pub fn start_dir(answer: &str) -> Result<PathBuf, String> {
    let path = crate::fileio::expand_leading_tilde(answer);
    if Path::new(&path).is_dir() {
        return Ok(PathBuf::from(path));
    }
    let stripped = strip_last_component(&path);
    if Path::new(stripped).is_dir() {
        return Ok(PathBuf::from(stripped));
    }
    std::env::current_dir().map_err(|_| "The working directory has disappeared".to_string())
}

/// `path` up to (not including) its last `/`; the whole of it when there
/// is none.
fn strip_last_component(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => path,
    }
}

/// The Go To Directory answer made into a path: `~` expanded, relative to
/// the browsed directory when not absolute, trailing slashes snipped.
pub fn goto_dir_target(browsed: &Path, answer: &str) -> PathBuf {
    let expanded = crate::fileio::expand_leading_tilde(answer);
    let mut path = if Path::new(&expanded).is_absolute() {
        expanded
    } else {
        let mut base = browsed.to_string_lossy().into_owned();
        if !base.ends_with(std::path::MAIN_SEPARATOR) {
            base.push(std::path::MAIN_SEPARATOR);
        }
        base + &expanded
    };
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    PathBuf::from(path)
}

/// A directory as the browser's title bar shows it: with a trailing
/// separator, like nano's `present_path`.
pub fn display_dir(dir: &Path) -> String {
    let mut s = dir.to_string_lossy().into_owned();
    if !s.ends_with(std::path::MAIN_SEPARATOR) {
        s.push(std::path::MAIN_SEPARATOR);
    }
    s
}

/// The browser's title bar text, `cols` columns wide: "DIR:" and the path,
/// centered, giving up first the side margins, then the "DIR:", then the
/// start of the path (for "..."). nano's `titlebar(path)`.
pub fn title_line(path: &str, cols: usize) -> String {
    let prefix = "DIR:";
    let prefixlen = prefix.len() + 1;
    let pathlen = path.width();
    let mut verlen = 3;
    let mut statelen = 2;
    let fits = |verlen: usize, statelen: usize| verlen + prefixlen + pathlen + statelen <= cols;
    if !fits(verlen, statelen) {
        verlen = 2;
        if !fits(verlen, statelen) {
            verlen = 0;
            statelen = 0;
        }
    }
    let offset = if verlen > 0 {
        verlen + (cols - (verlen + statelen) - (prefixlen + pathlen)) / 2
    } else {
        0
    };

    let mut line = " ".repeat(offset);
    if fits(verlen, statelen) {
        line.push_str(prefix);
        line.push(' ');
    }
    if pathlen + statelen <= cols {
        line.push_str(path);
    } else if 5 + statelen <= cols {
        line.push_str("...");
        let room = cols - statelen - 3;
        let skip = pathlen - room;
        let mut col = 0;
        for c in path.chars() {
            if col >= skip {
                line.push(c);
            }
            col += c.width().unwrap_or(1);
        }
    }
    let mut line = take_columns(&line, cols);
    let width = line.width();
    line.push_str(&" ".repeat(cols.saturating_sub(width)));
    line
}

/// An I/O error's text without Rust's " (os error N)" tail -- what nano's
/// `strerror` shows.
pub fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, info: &str) -> Entry {
        Entry {
            path: PathBuf::from("/d").join(name),
            name: name.to_string(),
            is_dir,
            info: info.to_string(),
        }
    }

    fn browser(names: &[&str]) -> Browser {
        let entries: Vec<Entry> = names.iter().map(|n| entry(n, false, "0  B")).collect();
        let widest = entries.iter().map(|e| e.name.width()).max().unwrap_or(0);
        Browser {
            dir: PathBuf::from("/d"),
            entries,
            selected: 0,
            widest,
        }
    }

    fn row_text(row: &[(String, bool)]) -> String {
        row.iter().map(|(s, _)| s.as_str()).collect()
    }

    #[test]
    fn sizes_match_nanos_format() {
        assert_eq!(size_info(6), "   6  B");
        assert_eq!(size_info(5000), "   4 KB");
        assert_eq!(size_info(3_000_000), "   2 MB");
        assert_eq!(size_info(5 << 30), "   5 GB");
        assert_eq!(size_info(2 << 40), "(huge)");
    }

    #[test]
    fn directories_sort_first_then_case_insensitively() {
        let mut v = [
            entry("B.txt", false, ""),
            entry("sub", true, ""),
            entry("a.txt", false, ""),
            entry("..", true, ""),
            entry("Another", true, ""),
            entry("b.txt", false, ""),
        ];
        v.sort_by(diralphasort);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "Another", "sub", "a.txt", "B.txt", "b.txt"]);
    }

    #[test]
    fn rows_match_nanos_listing() {
        // The layout of the installed nano's own browser at 80 columns
        // for this directory: widest name 11, so gauge 21 and 3 per row.
        let mut b = browser(&[]);
        b.entries = vec![
            entry("..", true, "(parent dir)"),
            entry("another_dir", true, "(dir)"),
            entry("dlink", true, "(dir)"),
            entry("sub", true, "(dir)"),
            entry("a.txt", false, "   6  B"),
            entry("B.txt", false, "   0  B"),
            entry("big.bin", false, "   4 KB"),
            entry("broken", false, "--"),
        ];
        b.widest = 11;
        let rows = b.render_rows(80, 4);
        assert_eq!(
            row_text(&rows[0]).trim_end(),
            "..       (parent dir)  another_dir     (dir)  dlink           (dir)"
        );
        assert_eq!(
            row_text(&rows[1]).trim_end(),
            "sub             (dir)  a.txt            6  B  B.txt            0  B"
        );
        assert_eq!(
            row_text(&rows[2]).trim_end(),
            "big.bin          4 KB  broken             --"
        );
        assert_eq!(row_text(&rows[3]), " ".repeat(80));
        for row in &rows {
            assert_eq!(row_text(row).chars().count(), 80);
        }
        // Only the selected cell is highlighted, and only its own width.
        assert_eq!(rows[0][0], ("..       (parent dir)".to_string(), true));
        assert!(rows[0][1..].iter().all(|(_, hl)| !hl));
    }

    #[test]
    fn long_names_are_dotted_from_the_left() {
        let mut b = browser(&["short", "a_really_long_filename_here.txt"]);
        // Squeeze the gauge below the widest name + 10.
        let cols = 30;
        assert_eq!(b.gauge(cols), 30);
        b.entries[1].info = "   0  B".into();
        let rows = b.render_rows(cols, 2);
        // gauge - 11 columns of the name's tail: the dots, the tail and
        // one blank leave exactly seven columns for the info.
        assert_eq!(row_text(&rows[1]), "...g_filename_here.txt    0  B");
    }

    #[test]
    fn navigation_follows_nanos_arithmetic() {
        let mut b = browser(&["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        // 3 piles, 2 rows: a page is 6 names.
        let (p, r) = (3, 2);
        b.selected = 4;
        b.navigate(Action::Up, p, r);
        assert_eq!(b.selected, 1);
        b.navigate(Action::Up, p, r);
        assert_eq!(b.selected, 1, "no row above");
        b.navigate(Action::Down, p, r);
        b.navigate(Action::Down, p, r);
        b.navigate(Action::Down, p, r);
        assert_eq!(b.selected, 7);
        b.navigate(Action::Down, p, r);
        assert_eq!(b.selected, 7, "no name below");
        b.navigate(Action::PrevWord, p, r);
        assert_eq!(b.selected, 6);
        b.navigate(Action::NextWord, p, r);
        assert_eq!(b.selected, 8);
        b.navigate(Action::PrevBlock, p, r);
        assert_eq!(b.selected, 8, "top row of the second page");
        b.selected = 1;
        b.navigate(Action::NextBlock, p, r);
        assert_eq!(b.selected, 4);
        b.navigate(Action::PageDown, p, r);
        assert_eq!(b.selected, 7, "same column, on the last page");
        b.navigate(Action::PageUp, p, r);
        assert_eq!(b.selected, 1);
        b.navigate(Action::PageUp, p, r);
        assert_eq!(b.selected, 0);
        b.navigate(Action::LastFile, p, r);
        assert_eq!(b.selected, 9);
        b.navigate(Action::Right, p, r);
        assert_eq!(b.selected, 9);
        b.navigate(Action::FirstFile, p, r);
        b.navigate(Action::Left, p, r);
        assert_eq!(b.selected, 0);
        assert!(!b.navigate(Action::Enter, p, r));
    }

    #[test]
    fn find_wraps_and_reports() {
        let mut b = browser(&["..", "alpha", "Beta", "gamma", "alphabet"]);
        assert_eq!(b.find("BET", true), FindOutcome::Found);
        assert_eq!(b.selected, 2);
        assert_eq!(b.find("bet", true), FindOutcome::Found);
        assert_eq!(b.selected, 4);
        assert_eq!(b.find("bet", true), FindOutcome::Wrapped);
        assert_eq!(b.selected, 2);
        assert_eq!(b.find("gam", false), FindOutcome::Wrapped);
        assert_eq!(b.selected, 3);
        assert_eq!(b.find("gam", true), FindOutcome::OnlyOccurrence);
        assert_eq!(b.find("zzz", true), FindOutcome::NotFound);
        assert_eq!(b.selected, 3);
    }

    #[test]
    fn title_centers_then_dottifies() {
        let line = title_line("/home/u/", 40);
        assert_eq!(line.chars().count(), 40);
        assert_eq!(line.trim(), "DIR: /home/u/");
        // Offset as nano computes it: 3 + (40 - 5 - (5 + 8)) / 2 = 14.
        assert!(line.starts_with(&" ".repeat(14)));
        assert!(!line.starts_with(&" ".repeat(15)));

        let long = "/a/very/long/directory/name/that/cannot/fit/";
        let line = title_line(long, 30);
        assert_eq!(line.chars().count(), 30);
        assert!(line.starts_with("..."), "{line:?}");
        assert!(line.trim_end().ends_with("cannot/fit/"), "{line:?}");
    }

    #[test]
    fn start_dir_falls_back_like_browse_in() {
        let tmp = std::env::temp_dir().join(format!("tico-browse-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        let t = tmp.to_string_lossy().into_owned();
        assert_eq!(start_dir(&t).unwrap(), tmp);
        assert_eq!(
            start_dir(&format!("{t}/sub/newfile")).unwrap(),
            PathBuf::from(format!("{t}/sub"))
        );
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(start_dir("").unwrap(), cwd);
        assert_eq!(start_dir("no-such-file-here").unwrap(), cwd);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn reading_a_directory_lists_parent_and_resolves_links() {
        let tmp = std::env::temp_dir().join(format!("tico-browse-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("f.txt"), "hello\n").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.join("sub"), tmp.join("dlink")).unwrap();
            std::os::unix::fs::symlink(tmp.join("f.txt"), tmp.join("flink")).unwrap();
        }
        let dir = full_dir_path(&tmp).unwrap();
        let b = Browser::read(&dir).unwrap();
        let listed: Vec<(&str, &str)> = b
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.info.as_str()))
            .collect();
        #[cfg(unix)]
        assert_eq!(
            listed,
            [
                ("..", "(parent dir)"),
                ("dlink", "(dir)"),
                ("sub", "(dir)"),
                ("f.txt", "   6  B"),
                ("flink", "--"),
            ]
        );
        #[cfg(not(unix))]
        assert_eq!(
            listed,
            [
                ("..", "(parent dir)"),
                ("sub", "(dir)"),
                ("f.txt", "   6  B")
            ]
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn click_picks_the_name_under_the_mouse() {
        let b = browser(&["a", "b", "c", "d", "e"]);
        // gauge 15, 80 columns: 4 piles of 17.
        assert_eq!(b.index_at(80, 3, 0, 0), Some(0));
        assert_eq!(b.index_at(80, 3, 0, 18), Some(1));
        assert_eq!(b.index_at(80, 3, 1, 0), Some(4));
        assert_eq!(b.index_at(80, 3, 2, 0), Some(4), "past the end");
        assert_eq!(b.index_at(80, 3, 0, 79), Some(3), "past the last pile");
    }
}
