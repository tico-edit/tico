//! Loading/saving files, detecting when the on-disk file has changed
//! underneath us, and three-way merging local edits with external changes.

use crate::buffer::{Buffer, DiskState, LineFormat};
use crate::options::Options;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// True if the current process can actually write to `path` (a file or a
/// directory), matching nano's own `access(path, W_OK)` checks in
/// src/files.c. This is deliberately not the same thing as "some write bit
/// is set in the mode" (`std::fs::Permissions::readonly()`): a file like
/// `/etc/passwd`, owned by root with mode 644, has *a* write bit set (the
/// owner's) but a non-root user still can't write to it, and
/// `Permissions::readonly()` would wrongly say it's writable.
#[cfg(unix)]
pub fn path_writable(path: &Path) -> bool {
    rustix::fs::access(path, rustix::fs::Access::WRITE_OK).is_ok()
}

#[cfg(not(unix))]
pub fn path_writable(path: &Path) -> bool {
    // No portable equivalent of access(W_OK) wired up yet for non-Unix
    // targets; fall back to the mode-bit check, which at least catches the
    // common "no write bit at all" case.
    std::fs::metadata(path)
        .map(|m| !m.permissions().readonly())
        .unwrap_or(true)
}

/// Expand a leading `~` in `path` to a home directory, the same as nano's
/// `expand_leading_tilde()`: `~` or `~/...` expands to the current user's
/// home directory; `~user` or `~user/...` expands to *that* user's home
/// directory (looked up via the system's user database). A path that
/// doesn't start with `~`, or a `~user` for an unknown user, is returned
/// unchanged — nano does the same for an unknown user (leaving `~baduser/x`
/// as a literal, normally-nonexistent relative path) rather than erroring.
pub fn expand_leading_tilde(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    if rest.is_empty() || rest.starts_with('/') {
        return match dirs::home_dir() {
            Some(home) => format!("{}{rest}", home.display()),
            None => path.to_string(),
        };
    }
    let (name, tail) = match rest.split_once('/') {
        Some((n, t)) => (n, format!("/{t}")),
        None => (rest, String::new()),
    };
    match user_home_dir(name) {
        Some(home) => format!("{}{tail}", home.display()),
        None => path.to_string(),
    }
}

/// Look up another user's home directory by name (for `~user` expansion),
/// via the system's user database (`getpwnam_r(3)`).
#[cfg(unix)]
fn user_home_dir(name: &str) -> Option<std::path::PathBuf> {
    use std::ffi::{CStr, CString};

    let cname = CString::new(name).ok()?;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // getpwnam(3) uses a static buffer and isn't thread-safe; getpwnam_r
    // wants a caller-supplied buffer instead. 16KiB comfortably covers any
    // real /etc/passwd (or NSS-backed) entry.
    let mut buf: Vec<libc::c_char> = vec![0; 16 * 1024];
    let rc = unsafe {
        libc::getpwnam_r(
            cname.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr(pwd.pw_dir) };
    Some(std::path::PathBuf::from(dir.to_str().ok()?))
}

/// Windows has no user database to query by name, so only `~` followed by
/// the current user's own name (`%USERNAME%`, compared case-insensitively
/// as Windows does) expands, to the same directory as a bare `~`. Other
/// users' names are left as-is rather than guessed at.
#[cfg(not(unix))]
fn user_home_dir(name: &str) -> Option<std::path::PathBuf> {
    let me = current_username()?;
    if name.eq_ignore_ascii_case(&me) {
        dirs::home_dir()
    } else {
        None
    }
}

#[cfg(not(unix))]
fn current_username() -> Option<String> {
    std::env::var("USERNAME").ok().filter(|s| !s.is_empty())
}

/// All usernames on the system, for `~user<Tab>` completion at the `^R`
/// Read File prompt — matches nano's `username_completion`, which walks
/// the same database with `getpwent(3)`.
#[cfg(unix)]
pub fn list_usernames() -> Vec<String> {
    use std::ffi::CStr;
    use std::sync::Mutex;

    // getpwent/setpwent/endpwent share one static, process-wide iteration
    // cursor and aren't thread-safe. tico's own event loop only ever calls
    // this from the single main UI thread, but the test suite runs tests
    // concurrently, so guard the sequence explicitly rather than relying
    // on that.
    static GETPWENT_LOCK: Mutex<()> = Mutex::new(());
    let _guard = GETPWENT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut names = Vec::new();
    unsafe {
        libc::setpwent();
        loop {
            let entry = libc::getpwent();
            if entry.is_null() {
                break;
            }
            if let Ok(name) = CStr::from_ptr((*entry).pw_name).to_str() {
                names.push(name.to_string());
            }
        }
        libc::endpwent();
    }
    names
}

/// Just the current user on Windows, the only name `~user` expands for
/// there (see the non-Unix `user_home_dir`).
#[cfg(not(unix))]
pub fn list_usernames() -> Vec<String> {
    current_username().into_iter().collect()
}

fn hash_content(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub fn stat_disk_state(path: &Path) -> Option<DiskState> {
    let meta = std::fs::metadata(path).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    Some(DiskState {
        mtime: meta.modified().ok(),
        len: meta.len(),
        content_hash: hash_content(&content),
        file_id: file_id(&meta),
    })
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// Whole seconds since the epoch, the resolution of nano's `st_mtime`.
fn mtime_secs(mtime: Option<std::time::SystemTime>) -> Option<u64> {
    mtime?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// nano's save-time "File on disk has changed" test (in `write_it_out`):
/// the file at `path` is newer than when it was last read or written, or
/// is a different file altogether (another device or inode).
pub fn changed_on_disk_since(known: &DiskState, path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let newer = match (mtime_secs(known.mtime), mtime_secs(meta.modified().ok())) {
        (Some(then), Some(now)) => then < now,
        _ => false,
    };
    let replaced = match (known.file_id, file_id(&meta)) {
        (Some(then), Some(now)) => then != now,
        _ => false,
    };
    newer || replaced
}

/// nano's `get_full_path`: `path` made absolute with symlinks resolved --
/// or, for a file that doesn't exist yet, its directory resolved with the
/// last component re-added. `None` when even the directory can't be
/// resolved. Used to tell whether a Write Out name is the buffer's own.
pub fn full_path(path: &Path) -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(expand_leading_tilde(&path.to_string_lossy()));
    if let Ok(target) = std::fs::canonicalize(&path) {
        return Some(target);
    }
    let name = path.file_name()?;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    Some(std::fs::canonicalize(dir).ok()?.join(name))
}

/// A human-readable summary of a freshly loaded file's size, matching
/// nano's post-load status message (`Read %zu line(s)` in src/files.c),
/// with nano's "(converted from DOS format)" / "(converted from Mac
/// format)" suffix per the format `convert_line_endings` detected.
/// nano counts actual lines of content, not ropey's `len_lines()` (which
/// counts a trailing empty line after a final newline).
pub fn describe_read(text: &str, detected: LineFormat) -> String {
    let n = nano_style_line_count(text);
    let suffix = match detected {
        LineFormat::Dos => " (converted from DOS format)",
        LineFormat::Mac => " (converted from Mac format)",
        LineFormat::Unix | LineFormat::Unspecified => "",
    };
    if n == 1 {
        format!("Read 1 line{suffix}")
    } else {
        format!("Read {n} lines{suffix}")
    }
}

/// nano 8.7's line-ending conversion on read (the byte loop of its
/// `read_file`), returning the converted text and the format it found.
/// Unless `noconvert` (`-N`/`set noconvert`, which returns the text
/// untouched and reports Unix):
///
/// - a CR directly before an LF is always dropped, and the file is DOS
///   format when its *first* line break was such a CR LF;
/// - a bare CR followed by another byte is a line break -- but only on
///   the first line, or once the file is already known to be Mac format
///   (so a stray CR later in a Unix or DOS file is ordinary content);
/// - a CR ending the file is a line break too, and makes an otherwise
///   unterminated last line terminated (nano's `mac_line_needs_newline`).
pub fn convert_line_endings(raw: &str, noconvert: bool) -> (String, LineFormat) {
    if noconvert {
        return (raw.to_string(), LineFormat::Unix);
    }
    let mut out = String::with_capacity(raw.len());
    let mut line = String::new();
    let mut num_lines = 0usize;
    let mut format = LineFormat::Unix;
    for c in raw.chars() {
        if c == '\n' {
            if line.ends_with('\r') {
                if num_lines == 0 {
                    format = LineFormat::Dos;
                }
                line.pop();
            }
        } else if (num_lines == 0 || format == LineFormat::Mac) && line.ends_with('\r') {
            format = LineFormat::Mac;
            line.pop();
        } else {
            line.push(c);
            continue;
        }
        out.push_str(&line);
        out.push('\n');
        num_lines += 1;
        line.clear();
        // After a Mac line break, the byte that revealed it starts the
        // next line.
        if c != '\n' {
            line.push(c);
        }
    }
    if !line.is_empty() {
        if line.ends_with('\r') {
            if num_lines == 0 {
                format = LineFormat::Mac;
            }
            line.pop();
            out.push_str(&line);
            out.push('\n');
        } else {
            out.push_str(&line);
        }
    }
    (out, format)
}

/// What `load_file` hands back: the buffer, plus the format the file was
/// read in (for the "converted from ... format" blurb -- reported even
/// under `set unix`, which forces the buffer's own format to Unix).
pub struct LoadedFile {
    pub buffer: Buffer,
    pub detected: LineFormat,
}

pub(crate) fn nano_style_line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let newlines = text.matches('\n').count();
    if text.ends_with('\n') {
        newlines
    } else {
        newlines + 1
    }
}

/// Give freshly read `text` nano's magic line (see
/// `Buffer::lacks_magic_line`) unless `nonewlines`: nano's `read_file`
/// puts an empty line after a last line with no newline on it, without
/// that counting as a modification.
pub fn with_magic_line(text: &mut String, nonewlines: bool) {
    if !nonewlines && !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
}

/// Read `path` into a fresh buffer, converting line endings per
/// `convert_line_endings` and settling the buffer's format per
/// `Buffer::adopt_format`.
pub fn load_file(path: &Path, opts: &Options) -> std::io::Result<LoadedFile> {
    let raw = std::fs::read_to_string(path)?;
    let (mut text, detected) = convert_line_endings(&raw, opts.noconvert);
    with_magic_line(&mut text, opts.nonewlines);
    let mut buffer = Buffer::from_text(&text, Some(path.to_path_buf()));
    buffer.adopt_format(detected, opts.unix);
    buffer.disk_state = stat_disk_state(path);
    Ok(LoadedFile { buffer, detected })
}

/// The bytes `save_file` writes for `buffer`: its text, with every line
/// break written as CR LF for a DOS-format buffer or a bare CR for a Mac
/// one (nano 8.7's `write_file`: a CR before each `'\n'` for both, and
/// the `'\n'` itself only when not Mac).
pub fn serialized(buffer: &Buffer) -> String {
    with_line_breaks(buffer.to_string(), buffer.format)
}

/// `text` with each `'\n'` written as `format` breaks lines on disk.
pub fn with_line_breaks(text: String, format: LineFormat) -> String {
    match format {
        LineFormat::Dos => text.replace('\n', "\r\n"),
        LineFormat::Mac => text.replace('\n', "\r"),
        LineFormat::Unix | LineFormat::Unspecified => text,
    }
}

pub fn save_file(buffer: &mut Buffer, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, serialized(buffer))?;
    buffer.path = Some(path.to_path_buf());
    buffer.modified = false;
    buffer.disk_state = stat_disk_state(path);
    buffer.original_content = buffer.to_string();
    Ok(())
}

/// nano's `outside_of_confinement`: whether `path` lies outside the
/// operating directory `opdir` (`set operatingdir`, already resolved by
/// `resolve_directory`'s rules; `None` confines nothing). A path whose
/// directory doesn't exist counts as inside -- unless `tabbing`, where it
/// counts as outside, but a directory on the way down *to* `opdir`
/// counts as inside, so that completion can reach it.
pub fn outside_of_confinement(opdir: Option<&str>, path: &Path, tabbing: bool) -> bool {
    let Some(opdir) = opdir else {
        return false;
    };
    let Some(full) = full_path(path) else {
        return tabbing;
    };
    let opdir = Path::new(opdir);
    let is_inside = full.starts_with(opdir);
    let begins_to_be = tabbing && opdir.starts_with(&full);
    !is_inside && !begins_to_be
}

/// How the Write Out prompt puts text into the file (nano's
/// `kind_of_writing_type`, toggled there with `M-A` and `M-P`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMethod {
    Overwrite,
    Append,
    Prepend,
}

/// Write `bytes` to `path` by `method`, the way nano's `write_file` does:
/// overwriting (creating the file if needed), appending (likewise), or
/// prepending -- which needs an existing, readable, non-FIFO file. A
/// failure comes back as nano's status message for it.
pub fn write_by_method(path: &Path, bytes: &[u8], method: WriteMethod) -> Result<(), String> {
    use std::io::Write;
    let name = path.display();
    let writing =
        |e: std::io::Error| format!("Error writing {name}: {}", crate::browser::strerror(&e));
    let mut bytes = std::borrow::Cow::Borrowed(bytes);
    if method == WriteMethod::Prepend {
        if std::fs::metadata(path).is_ok_and(|m| is_fifo(&m)) {
            return Err(format!("Error writing {name}: FIFO"));
        }
        let existing = std::fs::read(path)
            .map_err(|e| format!("Error reading {name}: {}", crate::browser::strerror(&e)))?;
        bytes.to_mut().extend_from_slice(&existing);
    }
    let mut open = std::fs::OpenOptions::new();
    open.create(true);
    if method == WriteMethod::Append {
        open.append(true);
    } else {
        open.write(true).truncate(true);
    }
    let mut file = open.open(path).map_err(writing)?;
    file.write_all(&bytes).map_err(writing)?;
    if !std::fs::metadata(path).is_ok_and(|m| is_fifo(&m)) {
        file.sync_all().map_err(writing)?;
    }
    Ok(())
}

/// nano's `init_backup_dir` (and, for `set operatingdir`,
/// `init_operating_dir`): `dir` made absolute with a trailing slash, or
/// `None` when it doesn't name an existing directory -- which nano treats
/// as fatal at startup.
pub fn resolve_directory(dir: &str) -> Option<String> {
    let full = full_path(Path::new(dir))?;
    if !full.is_dir() {
        return None;
    }
    let mut s = full.to_string_lossy().into_owned();
    if !s.ends_with('/') {
        s.push('/');
    }
    Some(s)
}

/// nano's `get_next_filename`: `name` plus `suffix`, or failing that the
/// first of `.1`, `.2`, ... appended that doesn't exist yet -- `None` once
/// a hundred thousand of them do.
fn next_free_name(name: &str, suffix: &str) -> Option<String> {
    let base = format!("{name}{suffix}");
    (0..100_000u32)
        .map(|i| {
            if i == 0 {
                base.clone()
            } else {
                format!("{base}.{i}")
            }
        })
        .find(|candidate| std::fs::metadata(candidate).is_err())
}

/// Where `make_backup_of` first tries to put the backup of `realname`:
/// without a backup directory, `realname~` alongside it; with one (`dir`,
/// as `resolve_directory` left it), a numbered name in that directory
/// built from the file's full path with each `/` turned into `!`.
fn backup_name(realname: &Path, dir: Option<&str>) -> Option<String> {
    let Some(dir) = dir else {
        return Some(format!("{}~", realname.display()));
    };
    let thename = match full_path(realname) {
        Some(full) => full.to_string_lossy().replace('/', "!"),
        None => realname
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    next_free_name(&format!("{dir}{thename}"), "~")
}

/// Whether writing to `realname` makes a backup first (the test at the
/// top of nano's `write_file`): only under `set backup`, of an existing
/// file that isn't a FIFO, and -- unless `forced`, as for a selection or
/// an append/prepend -- only when the file's mtime still matches what was
/// last read or written (`known`), i.e. not after "continue saving?" for
/// a file that was changed on disk. Returns the file's metadata for the
/// backup.
pub fn needs_backup(
    opts: &Options,
    realname: &Path,
    known: Option<&DiskState>,
    forced: bool,
) -> Option<std::fs::Metadata> {
    if !opts.backup {
        return None;
    }
    let meta = std::fs::metadata(realname).ok()?;
    if is_fifo(&meta) {
        return None;
    }
    // nano stats a file it hasn't seen before right here, so an unknown
    // file always matches itself.
    let unchanged = known.is_none_or(|k| mtime_secs(k.mtime) == mtime_secs(meta.modified().ok()));
    (unchanged || forced).then_some(meta)
}

#[cfg(unix)]
fn is_fifo(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt;
    meta.file_type().is_fifo()
}

#[cfg(not(unix))]
fn is_fifo(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Why `make_backup_of` didn't make a backup.
#[derive(Debug)]
pub enum BackupError {
    /// Every numbered name in the backup directory is taken; nano gives
    /// up on the save without asking.
    TooMany,
    /// Copying failed (in place and then in the home directory):
    /// `warnings` are what nano flashes one after another, and `error`
    /// the last failure, which decides whether nano asks to save anyway.
    Failed {
        warnings: Vec<String>,
        error: std::io::Error,
    },
}

/// How one attempt at writing the backup copy went wrong: `Problem`s get
/// a second try in the home directory, an unreadable original doesn't.
enum CopyError {
    Problem(std::io::Error),
    CannotRead(std::io::Error),
}

/// nano 8.7.1's `make_backup_of`: copy `realname` (whose metadata is
/// `meta`) to its backup name, with the original's owner, permissions and
/// timestamps. A backup that can't be made where it belongs is tried
/// again as `~/NAME~XXXXXX`.
pub fn make_backup_of(
    realname: &Path,
    meta: &std::fs::Metadata,
    backup_dir: Option<&str>,
    insecure: bool,
) -> Result<(), BackupError> {
    let Some(backupname) = backup_name(realname, backup_dir) else {
        return Err(BackupError::TooMany);
    };
    let mut warnings = Vec::new();
    let first = remove_old_backup(&backupname, insecure)
        .and_then(|()| create_backup_file(&backupname, insecure))
        .map_err(CopyError::Problem)
        .and_then(|file| fill_backup(file, realname, meta));
    let error = match first {
        Ok(()) => return Ok(()),
        Err(CopyError::CannotRead(e)) => {
            warnings.push("Cannot read original file".to_string());
            e
        }
        Err(CopyError::Problem(e)) => match dirs::home_dir() {
            None => {
                warnings.push("Cannot make backup".to_string());
                e
            }
            Some(home) => {
                let _ = std::fs::remove_file(&backupname);
                warnings.push("Cannot make regular backup".to_string());
                warnings.push("Trying again in your home directory".to_string());
                let tail = realname
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let second = create_unique_file(&home.join(format!("{tail}~")))
                    .map_err(CopyError::Problem)
                    .and_then(|file| fill_backup(file, realname, meta));
                match second {
                    Ok(()) => return Ok(()),
                    Err(CopyError::CannotRead(e)) => {
                        warnings.push("Cannot read original file".to_string());
                        e
                    }
                    Err(CopyError::Problem(e)) => {
                        warnings.push("Cannot make backup".to_string());
                        e
                    }
                }
            }
        },
    };
    warnings.push(crate::browser::strerror(&error));
    Err(BackupError::Failed { warnings, error })
}

/// Delete a previous backup, so that the new one is created afresh rather
/// than written through whatever is there (a symlink, say) -- a failure
/// to do so only matters without `allow_insecure_backup`.
fn remove_old_backup(backupname: &str, insecure: bool) -> std::io::Result<()> {
    match std::fs::remove_file(backupname) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound && !insecure => Err(e),
        _ => Ok(()),
    }
}

/// Open the backup for writing, owner read/write only until its real
/// permissions are copied over: newly created (`O_EXCL`), or under
/// `allow_insecure_backup` truncated if it's there anyway.
fn create_backup_file(backupname: &str, insecure: bool) -> std::io::Result<std::fs::File> {
    let mut open = std::fs::OpenOptions::new();
    open.write(true);
    if insecure {
        open.create(true).truncate(true);
    } else {
        open.create_new(true);
    }
    owner_only(&mut open);
    open.open(backupname)
}

#[cfg(unix)]
fn owner_only(open: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    open.mode(0o600);
}

#[cfg(not(unix))]
fn owner_only(_open: &mut std::fs::OpenOptions) {}

/// `mkstemp(3)` on `prefix` + six random characters: a new file that
/// didn't exist before.
fn create_unique_file(prefix: &Path) -> std::io::Result<std::fs::File> {
    use std::hash::BuildHasher;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let state = std::collections::hash_map::RandomState::new();
    let mut last_error = None;
    for attempt in 0u32..100 {
        let mut n = state.hash_one(attempt);
        let suffix: String = (0..6)
            .map(|_| {
                let c = CHARS[(n % CHARS.len() as u64) as usize] as char;
                n /= CHARS.len() as u64;
                c
            })
            .collect();
        let mut name = prefix.as_os_str().to_owned();
        name.push(suffix);
        match create_backup_file(&name.to_string_lossy(), false) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last_error = Some(e),
            result => return result,
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::ErrorKind::AlreadyExists.into()))
}

/// Give the freshly created backup `file` the original's owner and
/// permissions (as far as allowed), copy `realname`'s bytes into it, sync
/// it to disk, and set its timestamps to the original's.
fn fill_backup(
    mut file: std::fs::File,
    realname: &Path,
    meta: &std::fs::Metadata,
) -> Result<(), CopyError> {
    use std::io::{Read, Write};
    let permitted = |r: std::io::Result<()>| match r {
        Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => Err(CopyError::Problem(e)),
        _ => Ok(()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        permitted(std::os::unix::fs::fchown(
            &file,
            Some(meta.uid()),
            Some(meta.gid()),
        ))?;
    }
    permitted(file.set_permissions(meta.permissions()))?;
    let mut original = std::fs::File::open(realname).map_err(CopyError::CannotRead)?;
    let mut chunk = vec![0; 64 * 1024];
    loop {
        let n = match original.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(CopyError::CannotRead(e)),
        };
        file.write_all(&chunk[..n]).map_err(CopyError::Problem)?;
    }
    file.sync_all().map_err(CopyError::Problem)?;
    // Like nano, whole seconds only, and a failure here doesn't matter.
    let whole_secs = |t: std::io::Result<std::time::SystemTime>| {
        let secs = mtime_secs(t.ok())?;
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
    };
    let mut times = std::fs::FileTimes::new();
    if let Some(t) = whole_secs(meta.accessed()) {
        times = times.set_accessed(t);
    }
    if let Some(t) = whole_secs(meta.modified()) {
        times = times.set_modified(t);
    }
    let _ = file.set_times(times);
    Ok(())
}

/// The result of checking whether the file changed on disk since we loaded
/// or last saved it.
pub enum ExternalChange {
    Unchanged,
    /// Disk changed, buffer has no local edits: safe to silently reload.
    ChangedNoLocalEdits,
    /// Disk changed *and* we have local edits: caller must prompt.
    ChangedWithLocalEdits,
}

pub fn check_external_change(buffer: &Buffer) -> ExternalChange {
    let Some(path) = &buffer.path else {
        return ExternalChange::Unchanged;
    };
    let Some(known) = &buffer.disk_state else {
        return ExternalChange::Unchanged;
    };
    // Cheap pre-check: this runs on every idle poll (roughly every 600ms),
    // so skip re-reading and re-hashing the whole file when the mtime and
    // size we last saw still match — avoids a full read for anything but
    // a tiny file when nothing has actually changed.
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() == known.len
        && meta.modified().ok() == known.mtime
    {
        return ExternalChange::Unchanged;
    }
    let Some(current) = stat_disk_state(path) else {
        return ExternalChange::Unchanged;
    };
    if current.content_hash == known.content_hash {
        return ExternalChange::Unchanged;
    }
    if buffer.modified {
        ExternalChange::ChangedWithLocalEdits
    } else {
        ExternalChange::ChangedNoLocalEdits
    }
}

/// Reload a buffer from disk in place, discarding any (already-established
/// to be nonexistent-or-ignorable) local state. Preserves cursor position
/// where possible by clamping.
pub fn reload(
    buffer: &mut Buffer,
    noconvert: bool,
    unix: bool,
    nonewlines: bool,
) -> std::io::Result<()> {
    let Some(path) = buffer.path.clone() else {
        return Ok(());
    };
    let raw = std::fs::read_to_string(&path)?;
    let (mut text, detected) = convert_line_endings(&raw, noconvert);
    with_magic_line(&mut text, nonewlines);
    buffer.adopt_format(detected, unix);
    let cursor = buffer.cursor;
    buffer.rope = ropey::Rope::from_str(&text);
    buffer.invalidate_highlight_cache();
    buffer.original_content = text;
    buffer.modified = false;
    buffer.undo_stack.clear();
    buffer.redo_stack.clear();
    buffer.anchors.clear();
    buffer.disk_state = stat_disk_state(&path);
    let max_line = buffer.line_count().saturating_sub(1);
    buffer.cursor.line = cursor.line.min(max_line);
    let line_len = buffer.line(buffer.cursor.line).chars().count();
    buffer.cursor.col = cursor.col.min(line_len);
    Ok(())
}

/// The outcome of attempting a three-way merge of local edits against a
/// changed-on-disk file.
pub enum MergeResult {
    /// Merged cleanly; `text` is the merged content, `diff` a human-readable
    /// preview of what changed, to show before applying.
    Clean { text: String, diff: String },
    /// Could not merge automatically (overlapping conflicting edits).
    Conflict { diff: String },
}

/// Attempt a three-way merge: `base` is the content as originally loaded,
/// `ours` is the current (local, unsaved) buffer content, `theirs` is the
/// current on-disk content.
pub fn three_way_merge(base: &str, ours: &str, theirs: &str) -> MergeResult {
    use similar::{ChangeTag, TextDiff};

    // Diff base->ours and base->theirs, expressed as a common sequence of
    // base-line ranges so we can detect whether both sides touched the same
    // region of the base text.
    let ours_diff = TextDiff::from_lines(base, ours);
    let theirs_diff = TextDiff::from_lines(base, theirs);

    let ours_ops = ours_diff.ops();
    let theirs_ops = theirs_diff.ops();

    // Build, for each side, a map from base-line-index -> replacement lines
    // (only for ranges that actually differ from base).
    let base_lines: Vec<&str> = base.split_inclusive('\n').collect();
    let ours_lines: Vec<&str> = ours.split_inclusive('\n').collect();
    let theirs_lines: Vec<&str> = theirs.split_inclusive('\n').collect();

    #[derive(Clone)]
    struct Change {
        base_range: std::ops::Range<usize>,
        replacement: Vec<String>,
    }

    let extract_changes = |ops: &[similar::DiffOp], new_lines: &[&str]| -> Vec<Change> {
        let mut changes = Vec::new();
        for op in ops {
            match op {
                similar::DiffOp::Equal { .. } => {}
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                } => {
                    changes.push(Change {
                        base_range: *old_index..(*old_index + *old_len),
                        replacement: Vec::new(),
                    });
                }
                similar::DiffOp::Insert {
                    old_index,
                    new_index,
                    new_len,
                } => {
                    changes.push(Change {
                        base_range: *old_index..*old_index,
                        replacement: new_lines[*new_index..*new_index + *new_len]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    });
                }
                similar::DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    changes.push(Change {
                        base_range: *old_index..(*old_index + *old_len),
                        replacement: new_lines[*new_index..*new_index + *new_len]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    });
                }
            }
        }
        changes
    };

    let ours_changes = extract_changes(ours_ops, &ours_lines);
    let theirs_changes = extract_changes(theirs_ops, &theirs_lines);

    let ranges_overlap = |a: &std::ops::Range<usize>, b: &std::ops::Range<usize>| -> bool {
        if a.is_empty() && b.is_empty() {
            a.start == b.start
        } else {
            a.start < b.end && b.start < a.end
        }
    };

    let mut conflict = false;
    for oc in &ours_changes {
        for tc in &theirs_changes {
            if ranges_overlap(&oc.base_range, &tc.base_range) {
                conflict = true;
            }
        }
    }

    let diff_preview = |label: &str, text: &str| -> String {
        let d = TextDiff::from_lines(base, text);
        let mut out = format!("--- base\n+++ {label}\n");
        for change in d.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            out.push_str(sign);
            out.push_str(change.as_str().unwrap_or(""));
            if !change.as_str().unwrap_or("").ends_with('\n') {
                out.push('\n');
            }
        }
        out
    };

    if conflict {
        let mut diff = diff_preview("theirs (on disk)", theirs);
        diff.push('\n');
        diff.push_str(&diff_preview("ours (local edits)", ours));
        return MergeResult::Conflict { diff };
    }

    // No overlap: apply both sets of changes to base, in order.
    let mut all_changes: Vec<(Change, &str)> = ours_changes
        .iter()
        .map(|c| (c.clone(), "ours"))
        .chain(theirs_changes.iter().map(|c| (c.clone(), "theirs")))
        .collect();
    all_changes.sort_by_key(|(c, _)| (c.base_range.start, c.base_range.end));

    let mut merged = String::new();
    let mut pos = 0usize;
    for (change, _side) in &all_changes {
        while pos < change.base_range.start {
            merged.push_str(base_lines[pos]);
            pos += 1;
        }
        for line in &change.replacement {
            merged.push_str(line);
        }
        pos = change.base_range.end;
    }
    while pos < base_lines.len() {
        merged.push_str(base_lines[pos]);
        pos += 1;
    }

    let diff = diff_preview("merged", &merged);
    MergeResult::Clean { text: merged, diff }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_line_count_matches_nano() {
        use LineFormat::*;
        assert_eq!(describe_read("a\nb\n", Unix), "Read 2 lines");
        assert_eq!(describe_read("a\nb", Unix), "Read 2 lines");
        assert_eq!(describe_read("a\nb\nc\n", Unix), "Read 3 lines");
        assert_eq!(describe_read("onlyline", Unix), "Read 1 line");
        assert_eq!(describe_read("", Unix), "Read 0 lines");
        assert_eq!(
            describe_read("a\nb\n", Dos),
            "Read 2 lines (converted from DOS format)"
        );
        assert_eq!(
            describe_read("a", Mac),
            "Read 1 line (converted from Mac format)"
        );
    }

    fn conv(raw: &str) -> (String, LineFormat) {
        convert_line_endings(raw, false)
    }

    #[test]
    fn crlf_is_stripped_and_the_first_line_break_decides_dos_format() {
        use LineFormat::*;
        assert_eq!(conv("a\r\nb\r\n"), ("a\nb\n".to_string(), Dos));
        // Every CR-before-LF goes, but a file whose first break is a bare
        // LF isn't DOS format to nano.
        assert_eq!(conv("a\nb\r\n"), ("a\nb\n".to_string(), Unix));
        // A doubled CR on the first line: nano sees the first CR as a Mac
        // line break (the byte after it isn't an LF), so the file is Mac
        // format with an empty second line.
        assert_eq!(conv("a\r\r\n"), ("a\n\n".to_string(), Mac));
        // ...but later in a DOS file exactly one CR is dropped per break.
        assert_eq!(conv("a\r\nb\r\r\n"), ("a\nb\r\n".to_string(), Dos));
        assert_eq!(conv(""), (String::new(), Unix));
        assert_eq!(conv("no newline"), ("no newline".to_string(), Unix));
        assert_eq!(conv("\r\n"), ("\n".to_string(), Dos));
    }

    #[test]
    fn bare_cr_line_breaks_make_mac_format_only_from_the_first_line() {
        use LineFormat::*;
        assert_eq!(conv("a\rb\r"), ("a\nb\n".to_string(), Mac));
        assert_eq!(conv("a\rb\rc"), ("a\nb\nc".to_string(), Mac));
        // Once Mac, a CR LF still counts as one line break.
        assert_eq!(conv("a\rb\r\nc"), ("a\nb\nc".to_string(), Mac));
        // A stray CR after a Unix or DOS first line is content.
        assert_eq!(conv("a\nb\rc\n"), ("a\nb\rc\n".to_string(), Unix));
        assert_eq!(conv("a\r\nb\rc\r\n"), ("a\nb\rc\n".to_string(), Dos));
        // A lone trailing CR terminates the last line (nano adds the
        // blank line after it) and, on the first line, means Mac.
        assert_eq!(conv("a\r"), ("a\n".to_string(), Mac));
        assert_eq!(conv("\r"), ("\n".to_string(), Mac));
        assert_eq!(conv("a\nb\r"), ("a\nb\n".to_string(), Unix));
    }

    #[test]
    fn noconvert_keeps_the_bytes_and_never_reports_a_format() {
        assert_eq!(
            convert_line_endings("a\r\nb\rc", true),
            ("a\r\nb\rc".to_string(), LineFormat::Unix)
        );
    }

    #[test]
    fn backup_dir_must_be_an_existing_directory() {
        let dir = std::env::temp_dir().join(format!("tico_test_backupdir_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let resolved = resolve_directory(&dir.to_string_lossy()).unwrap();
        assert!(resolved.ends_with('/'), "{resolved}");
        assert!(Path::new(&resolved).is_absolute());
        let file = dir.join("plain");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(resolve_directory(&file.to_string_lossy()), None);
        assert_eq!(
            resolve_directory(&dir.join("missing").to_string_lossy()),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn confinement_follows_nanos_outside_of_confinement() {
        let base = std::env::temp_dir().join(format!("tico_test_confine_{}", std::process::id()));
        let inner = base.join("op");
        std::fs::create_dir_all(inner.join("sub")).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        let opdir = resolve_directory(&inner.to_string_lossy()).unwrap();
        let op = Some(opdir.as_str());
        let out = |p: &Path, tabbing| outside_of_confinement(op, p, tabbing);

        assert!(!outside_of_confinement(
            None,
            Path::new("/etc/passwd"),
            false
        ));
        assert!(!out(&inner, false), "the directory itself is inside");
        assert!(!out(&inner.join("sub/new.txt"), false));
        assert!(out(&base.join("other/x"), false));
        assert!(out(&inner.join("../other"), false), "`..` is resolved");
        // A name in a directory that doesn't exist counts as inside,
        // except when completing.
        assert!(!out(&base.join("nowhere/x"), false));
        assert!(out(&base.join("nowhere/x"), true));
        // Completing, the way down to the operating directory is open.
        assert!(out(&base, false));
        assert!(!out(&base, true));
        assert!(out(&base.join("other"), true));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn next_free_name_counts_up_past_taken_names() {
        let dir = std::env::temp_dir().join(format!("tico_test_nextname_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("f").to_string_lossy().into_owned();
        assert_eq!(next_free_name(&base, "~"), Some(format!("{base}~")));
        std::fs::write(format!("{base}~"), "").unwrap();
        std::fs::write(format!("{base}~.1"), "").unwrap();
        assert_eq!(next_free_name(&base, "~"), Some(format!("{base}~.2")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_backup_copies_permissions_and_timestamps() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tico_test_backupmeta_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.sh");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).unwrap();
        let then = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(then)
            .unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        make_backup_of(&path, &meta, None, false).unwrap();
        let backup = dir.join("f.sh~");
        let bmeta = std::fs::metadata(&backup).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "#!/bin/sh\n");
        assert_eq!(bmeta.permissions().mode() & 0o7777, 0o751);
        assert_eq!(bmeta.modified().unwrap(), then);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn buffers_serialize_with_their_formats_line_endings() {
        let mut buf = Buffer::from_text("a\nb\n", None);
        assert_eq!(serialized(&buf), "a\nb\n", "Unspecified writes as Unix");
        buf.format = LineFormat::Dos;
        assert_eq!(serialized(&buf), "a\r\nb\r\n");
        buf.format = LineFormat::Mac;
        assert_eq!(serialized(&buf), "a\rb\r");
        buf.format = LineFormat::Unix;
        assert_eq!(serialized(&buf), "a\nb\n");
    }

    #[test]
    fn adopt_format_follows_nano() {
        use LineFormat::*;
        let mut buf = Buffer::empty();
        buf.adopt_format(Dos, false);
        assert_eq!(buf.format, Dos, "a fresh buffer takes the file's");
        buf.adopt_format(Mac, false);
        assert_eq!(buf.format, Dos, "an existing format sticks");
        buf.adopt_format(Dos, true);
        assert_eq!(buf.format, Unix, "`set unix` overrides");
        let mut buf = Buffer::empty();
        buf.adopt_format(Unix, false);
        assert_eq!(buf.format, Unix);
    }

    #[test]
    fn loading_a_file_without_a_final_newline_adds_the_magic_line() {
        let path = std::env::temp_dir().join("tico_test_magic_line.txt");
        std::fs::write(&path, "one\ntwo").unwrap();
        let loaded = load_file(&path, &Options::default()).unwrap();
        assert_eq!(loaded.buffer.to_string(), "one\ntwo\n");
        assert!(!loaded.buffer.modified);
        let nonewlines = Options {
            nonewlines: true,
            ..Default::default()
        };
        let loaded = load_file(&path, &nonewlines).unwrap();
        assert_eq!(loaded.buffer.to_string(), "one\ntwo");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dos_file_round_trips_through_load_and_save() {
        let path = std::env::temp_dir().join("tico_test_dos_roundtrip.txt");
        std::fs::write(&path, "one\r\ntwo\r\n").unwrap();
        let opts = Options::default();
        let loaded = load_file(&path, &opts).unwrap();
        assert_eq!(loaded.detected, LineFormat::Dos);
        let mut buf = loaded.buffer;
        assert_eq!(buf.format, LineFormat::Dos);
        assert_eq!(buf.to_string(), "one\ntwo\n", "CRs never reach the buffer");
        buf.cursor = crate::buffer::Pos::new(1, 3);
        buf.insert_str("!");
        save_file(&mut buf, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one\r\ntwo!\r\n");

        let unix = Options {
            unix: true,
            ..Default::default()
        };
        let loaded = load_file(&path, &unix).unwrap();
        assert_eq!(
            loaded.detected,
            LineFormat::Dos,
            "still reported as converted"
        );
        assert_eq!(loaded.buffer.format, LineFormat::Unix, "but saved as Unix");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mac_file_round_trips_through_load_and_save() {
        let path = std::env::temp_dir().join("tico_test_mac_roundtrip.txt");
        std::fs::write(&path, "one\rtwo\r").unwrap();
        let loaded = load_file(&path, &Options::default()).unwrap();
        assert_eq!(loaded.detected, LineFormat::Mac);
        let mut buf = loaded.buffer;
        assert_eq!(buf.to_string(), "one\ntwo\n");
        save_file(&mut buf, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one\rtwo\r");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tilde_expansion_matches_nano() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_leading_tilde("~"), home.display().to_string());
        assert_eq!(
            expand_leading_tilde("~/foo/bar"),
            format!("{}/foo/bar", home.display())
        );
        // No leading tilde: unchanged.
        assert_eq!(expand_leading_tilde("relative/path"), "relative/path");
        assert_eq!(expand_leading_tilde("/absolute/path"), "/absolute/path");
        // A user that (almost certainly) doesn't exist is left as-is,
        // matching nano rather than erroring.
        assert_eq!(
            expand_leading_tilde("~tico_test_no_such_user_xyz/foo"),
            "~tico_test_no_such_user_xyz/foo"
        );
        assert_eq!(
            expand_leading_tilde("~tico_test_no_such_user_xyz"),
            "~tico_test_no_such_user_xyz"
        );
    }

    #[test]
    #[cfg(unix)]
    fn tilde_expansion_for_current_user_by_name() {
        // getpwnam_r should resolve our own username to our passwd entry's
        // home directory. Take both from the passwd database (by uid)
        // rather than $USER/$HOME, which needn't agree with it -- e.g. in
        // a `cross` container $USER is the host user, absent from the
        // container's /etc/passwd, and $HOME is the container's.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let mut buf: Vec<libc::c_char> = vec![0; 16 * 1024];
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc != 0 || result.is_null() {
            return; // no passwd entry for our uid; skip rather than fail
        }
        let (user, home) = unsafe {
            (
                std::ffi::CStr::from_ptr(pwd.pw_name)
                    .to_string_lossy()
                    .into_owned(),
                std::ffi::CStr::from_ptr(pwd.pw_dir)
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        assert_eq!(expand_leading_tilde(&format!("~{user}")), home);
        assert_eq!(
            expand_leading_tilde(&format!("~{user}/x")),
            format!("{home}/x")
        );
    }

    #[test]
    #[cfg(windows)]
    fn tilde_expansion_for_current_user_by_name() {
        // Windows: only our own name expands (case-insensitively), to the
        // same directory as a bare `~`; anyone else's is left alone.
        let Ok(user) = std::env::var("USERNAME") else {
            return; // not set in this environment; skip rather than fail
        };
        let home = dirs::home_dir().unwrap().display().to_string();
        assert_eq!(expand_leading_tilde(&format!("~{user}")), home);
        assert_eq!(
            expand_leading_tilde(&format!("~{user}/x")),
            format!("{home}/x")
        );
        assert_eq!(
            expand_leading_tilde(&format!("~{}", user.to_uppercase())),
            home
        );
        assert_eq!(
            expand_leading_tilde("~tico_test_no_such_user_xyz"),
            "~tico_test_no_such_user_xyz"
        );
        assert_eq!(list_usernames(), vec![user]);
    }

    #[test]
    fn merges_non_overlapping_edits() {
        let base = "one\ntwo\nthree\nfour\n";
        let ours = "one\nTWO\nthree\nfour\n"; // we changed line 2
        let theirs = "one\ntwo\nthree\nFOUR\n"; // they changed line 4
        match three_way_merge(base, ours, theirs) {
            MergeResult::Clean { text, .. } => {
                assert_eq!(text, "one\nTWO\nthree\nFOUR\n");
            }
            MergeResult::Conflict { diff } => panic!("expected clean merge, got conflict:\n{diff}"),
        }
    }

    #[test]
    fn detects_overlapping_conflict() {
        let base = "one\ntwo\nthree\n";
        let ours = "one\nTWO-ours\nthree\n";
        let theirs = "one\nTWO-theirs\nthree\n";
        match three_way_merge(base, ours, theirs) {
            MergeResult::Clean { text, .. } => panic!("expected conflict, got clean merge: {text}"),
            MergeResult::Conflict { .. } => {}
        }
    }

    #[test]
    fn unchanged_when_no_diffs() {
        let base = "same\n";
        match three_way_merge(base, base, base) {
            MergeResult::Clean { text, .. } => assert_eq!(text, base),
            MergeResult::Conflict { diff } => panic!("unexpected conflict:\n{diff}"),
        }
    }
}
