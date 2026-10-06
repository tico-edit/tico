//! `set positionlog`: the cursor position last left in each file, kept in
//! nano's own `filepos_history` file in the state directory (alongside
//! `historylog`'s `search_history`), so tico and nano share it. Ported
//! from nano 8.7.1's `src/history.c` (`load_poshistory`,
//! `save_poshistory`, `update_poshistory`, `restore_cursor_position_if_any`).
//!
//! Each line is `[ANCHORS]PATH LINE COLUMN`: the full path, the 1-based
//! line, and the 1-based display column, most recently closed file first,
//! at most 200 of them. ANCHORS is nano's list of anchored line numbers
//! (`"3 17 "`); tico has no anchors, so it keeps whatever nano wrote there
//! rather than dropping it. A newline in a path is stored as a NUL.

use std::path::PathBuf;

const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    anchors: String,
    filename: String,
    line: usize,
    column: usize,
}

#[derive(Debug)]
pub struct PositionLog {
    path: PathBuf,
    entries: Vec<Entry>,
    /// The file's mtime (whole seconds) as of the last load or save, to
    /// notice another nano or tico having rewritten it since.
    timestamp: Option<u64>,
}

impl PositionLog {
    /// The log in nano's state directory -- `None` when there's no state
    /// directory, or the file exists but can't be read (nano then turns
    /// `positionlog` off, so as not to overwrite it on exit).
    pub fn open() -> Option<PositionLog> {
        let dir = crate::history::HistoryStore::state_dir()?;
        PositionLog::at(dir.join("filepos_history"))
    }

    /// The log kept in `path`, loaded.
    pub fn at(path: PathBuf) -> Option<PositionLog> {
        let mut log = PositionLog {
            path,
            entries: Vec::new(),
            timestamp: None,
        };
        log.load().then_some(log)
    }

    fn load(&mut self) -> bool {
        self.entries.clear();
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) => return e.kind() == std::io::ErrorKind::NotFound,
        };
        let text = String::from_utf8_lossy(&bytes);
        self.entries = text
            .split_inclusive('\n')
            .take(MAX_ENTRIES)
            .take_while(|line| line.len() > 1)
            .filter_map(parse_entry)
            .collect();
        self.timestamp = self.mtime();
        true
    }

    fn mtime(&self) -> Option<u64> {
        let modified = std::fs::metadata(&self.path).ok()?.modified().ok()?;
        Some(
            modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs(),
        )
    }

    /// nano's `reload_positions_if_needed`.
    fn reload_if_changed(&mut self) {
        if let Some(now) = self.mtime()
            && Some(now) != self.timestamp
        {
            self.load();
        }
    }

    /// Where the cursor was last left in the file whose full path is
    /// `fullpath`: its 1-based line and display column.
    pub fn lookup(&mut self, fullpath: &str) -> Option<(usize, usize)> {
        self.reload_if_changed();
        self.entries
            .iter()
            .find(|e| e.filename == fullpath)
            .map(|e| (e.line, e.column))
    }

    /// Record that the file at `fullpath` was left at `line`, `column`
    /// (both 1-based, the column a display column), moving it to the top
    /// of the list, and write the list out.
    pub fn update(&mut self, fullpath: &str, line: usize, column: usize) {
        self.reload_if_changed();
        let anchors = match self.entries.iter().position(|e| e.filename == fullpath) {
            Some(i) => self.entries.remove(i).anchors,
            None => String::new(),
        };
        self.entries.insert(
            0,
            Entry {
                anchors,
                filename: fullpath.to_string(),
                line,
                column,
            },
        );
        self.save();
    }

    /// nano's `save_poshistory`; like nano, problems writing are ignored
    /// beyond losing the update.
    fn save(&mut self) {
        let mut text = String::new();
        for e in self.entries.iter().take(MAX_ENTRIES) {
            text.push_str(&e.anchors);
            text.push_str(&format!("{} {} {}", e.filename, e.line, e.column).replace('\n', "\0"));
            text.push('\n');
        }
        if std::fs::write(&self.path, text).is_ok() {
            // Don't allow others to read or write the history file.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
            }
        }
        self.timestamp = self.mtime();
    }
}

/// One line of the file. The anchors (digits and spaces) run up to the
/// start of the path; the line and column are the last two space-separated
/// numbers (parsed like C's `atoi`, so junk reads as 0).
fn parse_entry(line: &str) -> Option<Entry> {
    let line = line.strip_suffix('\n').unwrap_or(line).replace('\0', "\n");
    let start = line.find(|c: char| !c.is_ascii_digit() && c != ' ')?;
    let (anchors, rest) = line.split_at(start);
    let mut parts = rest.rsplitn(3, ' ');
    let column = atoi(parts.next()?);
    let lineno = atoi(parts.next()?);
    let filename = parts.next()?;
    Some(Entry {
        anchors: anchors.to_string(),
        filename: filename.to_string(),
        line: lineno,
        column,
    })
}

fn atoi(s: &str) -> usize {
    let digits: String = s
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tico_poslog_{name}_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn reads_nanos_format_including_anchors() {
        let path = temp_log("read");
        std::fs::write(
            &path,
            "/a/one.txt 12 5\n3 17 /a/two words.txt 1 9\n\n/a/after 1 1\n",
        )
        .unwrap();
        let mut log = PositionLog::at(path.clone()).unwrap();
        assert_eq!(log.lookup("/a/one.txt"), Some((12, 5)));
        assert_eq!(log.lookup("/a/two words.txt"), Some((1, 9)));
        assert_eq!(log.lookup("/a/after"), None, "nano stops at a blank line");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn update_moves_the_file_to_the_top_and_keeps_its_anchors() {
        let path = temp_log("update");
        std::fs::write(&path, "/a/one 1 1\n3 /a/two 2 2\n").unwrap();
        let mut log = PositionLog::at(path.clone()).unwrap();
        log.update("/a/two", 7, 4);
        log.update("/a/new\nline", 1, 2);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"/a/new\0line 1 2\n3 /a/two 7 4\n/a/one 1 1\n"
        );
        let mut again = PositionLog::at(path.clone()).unwrap();
        assert_eq!(again.lookup("/a/new\nline"), Some((1, 2)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn keeps_at_most_two_hundred_files() {
        let path = temp_log("cap");
        let mut log = PositionLog::at(path.clone()).unwrap();
        for i in 0..205 {
            log.update(&format!("/f{i}"), 1, 1);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 200);
        assert!(text.starts_with("/f204 1 1\n"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn notices_another_editor_rewriting_the_file() {
        let path = temp_log("reload");
        std::fs::write(&path, "/a/x 1 1\n").unwrap();
        let mut log = PositionLog::at(path.clone()).unwrap();
        std::fs::write(&path, "/a/x 9 9\n").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(log.lookup("/a/x"), Some((9, 9)));
        std::fs::remove_file(&path).ok();
    }
}
