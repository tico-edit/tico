//! Search/Replace/Execute history: an in-memory list per category, built up
//! during the session regardless of settings, and — only when `historylog`
//! is on — loaded from and saved to disk in nano's exact format (see
//! `~/dev/nano`'s `src/history.c`): a single `search_history` file in the
//! state directory, holding three blank-line-separated sections (search,
//! replace, execute), each oldest-to-newest, capped at 100 entries with
//! duplicates moved to the end rather than repeated.

use std::path::PathBuf;

const MAX_HISTORY: usize = 100;

#[derive(Debug, Default)]
pub struct HistoryStore {
    pub search: Vec<String>,
    pub replace: Vec<String>,
    pub execute: Vec<String>,
    changed: bool,
}

impl HistoryStore {
    pub fn new() -> HistoryStore {
        HistoryStore::default()
    }

    pub fn add_search(&mut self, text: &str) {
        Self::add(&mut self.search, text);
        self.changed = true;
    }

    pub fn add_replace(&mut self, text: &str) {
        Self::add(&mut self.replace, text);
        self.changed = true;
    }

    #[allow(dead_code)] // execute-command history: wired for round-tripping the file; the feature itself isn't implemented yet.
    pub fn add_execute(&mut self, text: &str) {
        Self::add(&mut self.execute, text);
        self.changed = true;
    }

    fn add(list: &mut Vec<String>, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(i) = list.iter().position(|s| s == text) {
            list.remove(i);
        }
        list.push(text.to_string());
        if list.len() > MAX_HISTORY {
            list.remove(0);
        }
    }

    /// nano's state directory: `~/.nano/` if that already exists as a
    /// directory (legacy location), else `$XDG_DATA_HOME/nano/` (or
    /// `~/.local/share/nano/` if that's unset), creating it (and its
    /// parents, for the default path) with owner-only permissions if
    /// missing. Returns `None` if no home directory can be found at all.
    pub(crate) fn state_dir() -> Option<PathBuf> {
        let home = dirs::home_dir()?;

        let legacy = home.join(".nano");
        if legacy.is_dir() {
            return Some(legacy);
        }

        let dir = match std::env::var("XDG_DATA_HOME") {
            Ok(xdg) if !xdg.is_empty() => PathBuf::from(xdg).join("nano"),
            _ => home.join(".local/share/nano"),
        };
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        if dir.is_dir() { Some(dir) } else { None }
    }

    fn history_path() -> Option<PathBuf> {
        Self::state_dir().map(|d| d.join("search_history"))
    }

    /// Load history from disk (only meaningful when `historylog` is set;
    /// callers should gate the call on that themselves).
    pub fn load() -> HistoryStore {
        let mut store = HistoryStore::new();
        let Some(path) = Self::history_path() else {
            return store;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return store;
        };

        let mut section = 0u8; // 0 = search, 1 = replace, 2 = execute
        for line in text.lines() {
            if line.is_empty() {
                section = section.saturating_add(1);
                continue;
            }
            match section {
                0 => Self::add(&mut store.search, line),
                1 => Self::add(&mut store.replace, line),
                _ => Self::add(&mut store.execute, line),
            }
        }
        // Loading marks the lists as "changed" as a side effect of using
        // the same add() path as live searches; undo that, matching nano
        // (only save if something *new* happened this session).
        store.changed = false;
        store
    }

    /// Save history to disk if it changed this session (only meaningful
    /// when `historylog` is set; callers should gate the call themselves).
    pub fn save(&self) {
        if !self.changed {
            return;
        }
        let Some(path) = Self::history_path() else {
            return;
        };
        let mut text = String::new();
        for line in &self.search {
            text.push_str(line);
            text.push('\n');
        }
        text.push('\n');
        for line in &self.replace {
            text.push_str(line);
            text.push('\n');
        }
        text.push('\n');
        for line in &self.execute {
            text.push_str(line);
            text.push('\n');
        }
        if std::fs::write(&path, text).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_deduplicates_by_moving_to_end() {
        let mut h = HistoryStore::new();
        h.add_search("one");
        h.add_search("two");
        h.add_search("one");
        assert_eq!(h.search, vec!["two".to_string(), "one".to_string()]);
    }

    #[test]
    fn add_caps_at_max_history() {
        let mut h = HistoryStore::new();
        for i in 0..(MAX_HISTORY + 10) {
            h.add_search(&format!("item{i}"));
        }
        assert_eq!(h.search.len(), MAX_HISTORY);
        assert_eq!(h.search.first(), Some(&"item10".to_string()));
        assert_eq!(h.search.last(), Some(&format!("item{}", MAX_HISTORY + 9)));
    }

    #[test]
    fn empty_entries_are_ignored() {
        let mut h = HistoryStore::new();
        h.add_search("");
        assert!(h.search.is_empty());
    }
}
