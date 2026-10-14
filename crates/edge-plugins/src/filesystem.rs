//! `widgetbuilder.filesystemprovider:FileSystem:1.0`.
//!
//! Methods: `watchPath(path)`, `unwatchPath(path)`, `clearPaths()`. Signals: `fileChanged(path,
//! content)`, `fileContentDiff(path, addedLines, removedLines)`, `fileAdded(path, content)`,
//! `fileRemoved(path)`. Paths in signals are absolute.
//!
//! EdgeHost behaviour where iCUE's docs are silent:
//! - `watchPath` first reports the current state: `fileChanged` for an existing file, `fileAdded`
//!   for each file directly inside a folder. Without it a widget could not show anything until the
//!   first change.
//! - Folder watches are not recursive. Files directly inside also report `fileChanged` (and
//!   `fileContentDiff`) when their content changes.
//! - Text files only (UTF-8); a file larger than [`MAX_BYTES`] is reported as its last
//!   [`MAX_BYTES`], starting at a line boundary, which suits log-style files. In a folder, a file
//!   that is not text is reported with empty content.
//!
//! Each watched path is followed through its parent folder rather than its inode, so files that
//! are replaced by renaming (git's `HEAD.lock` → `HEAD`) keep reporting.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::paths::{Grants, MacroStyle, Macros};
use crate::{Descriptor, Emitter, PluginError, Provider};

/// Largest amount of a file that is read.
pub const MAX_BYTES: u64 = 1024 * 1024;

pub(crate) const DESCRIPTOR: Descriptor = Descriptor {
    module: "widgetbuilder.filesystemprovider",
    name: "FileSystem",
    major: 1,
    minor: 0,
    methods: &["watchPath", "unwatchPath", "clearPaths"],
    signals: &["fileChanged", "fileContentDiff", "fileAdded", "fileRemoved"],
};

/// A change to report, built while the state is locked and emitted after.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Signal {
    Changed(PathBuf, String),
    Diff(PathBuf, Vec<String>, Vec<String>),
    Added(PathBuf, String),
    Removed(PathBuf),
}

#[derive(Default)]
struct Snapshots {
    /// Watched file → last content (`None`: missing or not text).
    files: HashMap<PathBuf, Option<String>>,
    /// Watched folder → its files and their last content.
    folders: HashMap<PathBuf, BTreeMap<PathBuf, String>>,
}

#[derive(Default)]
struct Watches {
    watcher: Option<notify::RecommendedWatcher>,
    /// Folders registered with the OS watcher → number of watched paths that need them.
    dirs: HashMap<PathBuf, usize>,
}

struct Inner {
    macros: Macros,
    grants: Grants,
    emit: Emitter,
    // Separate locks: the OS watcher's callback takes only `snapshots`, and registering a folder
    // with the OS (under `watches`) may wait for that callback to return.
    snapshots: Mutex<Snapshots>,
    watches: Mutex<Watches>,
}

/// The File System provider for one widget instance.
pub struct FileSystem {
    inner: Arc<Inner>,
}

impl FileSystem {
    /// A provider that may read what `grants` allow and reports through `emit`.
    #[must_use]
    pub fn new(macros: Macros, grants: Grants, emit: Emitter) -> Self {
        Self {
            inner: Arc::new(Inner {
                macros,
                grants,
                emit,
                snapshots: Mutex::default(),
                watches: Mutex::default(),
            }),
        }
    }

    fn resolve(&self, args: &[Value]) -> Result<PathBuf, PluginError> {
        let raw = args
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| PluginError::BadArguments("expected a path string".into()))?;
        let path = self
            .inner
            .macros
            .resolve(raw, MacroStyle::Curly)
            .map_err(|e| PluginError::BadArguments(e.to_string()))?;
        if self.inner.grants.allows(&path) {
            Ok(path)
        } else {
            // iCUE blocks silently; the widget sees no events. Say why on the host log.
            Err(PluginError::NotPermitted(path.display().to_string()))
        }
    }

    fn watch(&self, path: &Path) -> Result<(), PluginError> {
        let is_folder = path.is_dir();
        let dir = if is_folder {
            path.to_path_buf()
        } else {
            path.parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| PluginError::BadArguments("cannot watch /".into()))?
        };
        let initial = {
            let mut snaps = lock(&self.inner.snapshots);
            if snaps.files.contains_key(path) || snaps.folders.contains_key(path) {
                return Ok(());
            }
            if is_folder {
                let listing = list_folder(path);
                let added = listing
                    .iter()
                    .map(|(p, c)| Signal::Added(p.clone(), c.clone()))
                    .collect();
                snaps.folders.insert(path.to_path_buf(), listing);
                added
            } else {
                let content = read_text(path);
                let changed = content
                    .clone()
                    .map(|c| Signal::Changed(path.to_path_buf(), c))
                    .into_iter()
                    .collect();
                snaps.files.insert(path.to_path_buf(), content);
                changed
            }
        };
        if let Err(e) = self.add_dir(&dir) {
            let mut snaps = lock(&self.inner.snapshots);
            snaps.files.remove(path);
            snaps.folders.remove(path);
            return Err(e);
        }
        self.inner.send(initial);
        Ok(())
    }

    fn add_dir(&self, dir: &Path) -> Result<(), PluginError> {
        let mut w = lock(&self.inner.watches);
        if let Some(count) = w.dirs.get_mut(dir) {
            *count += 1;
            return Ok(());
        }
        if w.watcher.is_none() {
            let weak = Arc::downgrade(&self.inner);
            let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                on_event(&weak, res);
            })
            .map_err(|e| PluginError::Failed(format!("starting the file watcher: {e}")))?;
            w.watcher = Some(watcher);
        }
        if let Some(watcher) = w.watcher.as_mut() {
            // A missing folder cannot be watched; the path then stays silent, as in iCUE.
            if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                eprintln!("FileSystem: not watching {}: {e}", dir.display());
            }
        }
        w.dirs.insert(dir.to_path_buf(), 1);
        Ok(())
    }

    fn remove_dir(&self, dir: &Path) {
        let mut w = lock(&self.inner.watches);
        let Some(count) = w.dirs.get_mut(dir) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            w.dirs.remove(dir);
            if let Some(watcher) = w.watcher.as_mut() {
                let _ = watcher.unwatch(dir);
            }
        }
    }

    fn unwatch(&self, path: &Path) {
        let dir = {
            let mut snaps = lock(&self.inner.snapshots);
            if snaps.folders.remove(path).is_some() {
                Some(path.to_path_buf())
            } else if snaps.files.remove(path).is_some() {
                path.parent().map(Path::to_path_buf)
            } else {
                None
            }
        };
        if let Some(dir) = dir {
            self.remove_dir(&dir);
        }
    }

    fn clear(&self) {
        let mut snaps = lock(&self.inner.snapshots);
        let mut w = lock(&self.inner.watches);
        *snaps = Snapshots::default();
        // Dropping the watcher stops it; a new one starts on the next watchPath.
        *w = Watches::default();
    }
}

impl Provider for FileSystem {
    fn call(&self, method: &str, args: &[Value]) -> Result<Value, PluginError> {
        match method {
            "watchPath" => self.watch(&self.resolve(args)?)?,
            "unwatchPath" => self.unwatch(&self.resolve(args)?),
            "clearPaths" => self.clear(),
            other => return Err(PluginError::UnknownMethod(other.to_owned())),
        }
        Ok(Value::Null)
    }

    fn reset(&self) {
        self.clear();
    }
}

impl Inner {
    fn send(&self, signals: Vec<Signal>) {
        for s in signals {
            let (name, args) = match s {
                Signal::Changed(p, c) => ("fileChanged", vec![path_value(&p), Value::from(c)]),
                Signal::Diff(p, added, removed) => (
                    "fileContentDiff",
                    vec![path_value(&p), Value::from(added), Value::from(removed)],
                ),
                Signal::Added(p, c) => ("fileAdded", vec![path_value(&p), Value::from(c)]),
                Signal::Removed(p) => ("fileRemoved", vec![path_value(&p)]),
            };
            (self.emit)(name, args);
        }
    }

    /// Re-read everything an OS event under `paths` may have touched and report the differences.
    fn refresh(&self, paths: &[PathBuf]) -> Vec<Signal> {
        let mut snaps = lock(&self.snapshots);
        let Snapshots { files, folders } = &mut *snaps;
        let touches = |watched: &Path| {
            paths
                .iter()
                .any(|p| p == watched || Some(p.as_path()) == watched.parent())
        };
        let mut out = Vec::new();
        for (path, old) in files.iter_mut() {
            if touches(path) {
                let new = read_text(path);
                file_signals(path, old.as_deref(), new.as_deref(), &mut out);
                *old = new;
            }
        }
        for (folder, old) in folders.iter_mut() {
            if paths
                .iter()
                .any(|p| p == folder || p.parent() == Some(folder))
            {
                let new = list_folder(folder);
                folder_signals(old, &new, &mut out);
                *old = new;
            }
        }
        out
    }
}

fn on_event(weak: &Weak<Inner>, res: notify::Result<notify::Event>) {
    let Some(inner) = weak.upgrade() else {
        return;
    };
    match res {
        Ok(event) => {
            let signals = inner.refresh(&event.paths);
            inner.send(signals);
        }
        Err(e) => eprintln!("FileSystem: watcher error: {e}"),
    }
}

fn file_signals(path: &Path, old: Option<&str>, new: Option<&str>, out: &mut Vec<Signal>) {
    match (old, new) {
        (Some(o), Some(n)) if o != n => {
            out.push(Signal::Changed(path.to_path_buf(), n.to_owned()));
            let (added, removed) = line_diff(o, n);
            if !added.is_empty() || !removed.is_empty() {
                out.push(Signal::Diff(path.to_path_buf(), added, removed));
            }
        }
        (None, Some(n)) => out.push(Signal::Changed(path.to_path_buf(), n.to_owned())),
        (Some(_), None) => out.push(Signal::Removed(path.to_path_buf())),
        _ => {}
    }
}

fn folder_signals(
    old: &BTreeMap<PathBuf, String>,
    new: &BTreeMap<PathBuf, String>,
    out: &mut Vec<Signal>,
) {
    for (path, content) in new {
        match old.get(path) {
            None => out.push(Signal::Added(path.clone(), content.clone())),
            Some(before) => file_signals(path, Some(before), Some(content), out),
        }
    }
    for path in old.keys().filter(|p| !new.contains_key(*p)) {
        out.push(Signal::Removed(path.clone()));
    }
}

/// Lines added and removed going from `old` to `new`, without line endings.
fn line_diff(old: &str, new: &str) -> (Vec<String>, Vec<String>) {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for change in TextDiff::from_lines(old, new).iter_all_changes() {
        let line = change.value().trim_end_matches(['\n', '\r']).to_owned();
        match change.tag() {
            ChangeTag::Insert => added.push(line),
            ChangeTag::Delete => removed.push(line),
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

/// The text of a file: `None` if it is missing, not a regular file, or not UTF-8.
fn read_text(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let truncated = meta.len() > MAX_BYTES;
    if truncated {
        file.seek(SeekFrom::End(-i64::try_from(MAX_BYTES).ok()?))
            .ok()?;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES).read_to_end(&mut bytes).ok()?;
    if truncated {
        // Start at the first complete line.
        let start = bytes.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
        bytes.drain(..start);
    }
    String::from_utf8(bytes).ok()
}

/// Files directly inside `folder` with their text (empty if not text).
fn list_folder(folder: &Path) -> BTreeMap<PathBuf, String> {
    let Ok(entries) = fs::read_dir(folder) else {
        return BTreeMap::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            let path = e.path();
            let content = read_text(&path).unwrap_or_default();
            (path, content)
        })
        .collect()
}

fn path_value(path: &Path) -> Value {
    Value::from(path.to_string_lossy().into_owned())
}

/// Locks stay usable after a panicking holder: the snapshots are only a cache.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use icuew_manifest::{PathPermission, PathPermissionKind};

    use super::*;

    type Received = mpsc::Receiver<(String, Vec<Value>)>;

    fn provider(granted: &Path) -> (FileSystem, Received) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let emit: Emitter = Arc::new(move |name: &str, args: Vec<Value>| {
            let _ = lock(&tx).send((name.to_owned(), args));
        });
        let macros = Macros::new(granted, granted);
        let grants = Grants::new(
            &[PathPermission {
                kind: PathPermissionKind::Folder,
                path: "<Home>".into(),
            }],
            &macros,
        );
        (FileSystem::new(macros, grants, emit), rx)
    }

    /// The next signal named `name`, skipping others (the OS may report extra events).
    fn next(rx: &Received, name: &str) -> Vec<Value> {
        loop {
            let (got, args) = rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("no {name} signal"));
            if got == name {
                return args;
            }
        }
    }

    fn canonical_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        (dir, path)
    }

    #[test]
    fn line_diff_reports_added_and_removed_lines() {
        assert_eq!(
            line_diff("a\nb\nc\n", "a\nc\nd\n"),
            (vec!["d".to_owned()], vec!["b".to_owned()])
        );
        assert_eq!(line_diff("same\n", "same\n"), (vec![], vec![]));
    }

    #[test]
    fn file_signals_cover_each_transition() {
        let p = Path::new("/x");
        let mut out = Vec::new();
        file_signals(p, Some("a\n"), Some("a\nb\n"), &mut out);
        file_signals(p, None, Some("new"), &mut out);
        file_signals(p, Some("gone"), None, &mut out);
        file_signals(p, Some("same"), Some("same"), &mut out);
        file_signals(p, None, None, &mut out);
        assert_eq!(
            out,
            vec![
                Signal::Changed(p.into(), "a\nb\n".into()),
                Signal::Diff(p.into(), vec!["b".into()], vec![]),
                Signal::Changed(p.into(), "new".into()),
                Signal::Removed(p.into()),
            ]
        );
    }

    #[test]
    fn large_files_keep_their_last_whole_lines() {
        let (_tmp, dir) = canonical_dir();
        let path = dir.join("big.log");
        let line = "0123456789abcdef\n";
        let lines = usize::try_from(MAX_BYTES).unwrap() / line.len() + 10;
        let mut text = line.repeat(lines);
        text.push_str("last\n");
        fs::write(&path, &text).unwrap();
        let read = read_text(&path).unwrap();
        assert!(read.len() <= usize::try_from(MAX_BYTES).unwrap());
        assert!(read.starts_with("0123") && read.ends_with("last\n"));
        fs::write(dir.join("bin"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_text(&dir.join("bin")), None);
        assert_eq!(read_text(&dir), None);
    }

    #[test]
    fn refuses_paths_outside_the_grant() {
        let (_tmp, dir) = canonical_dir();
        let (fs_provider, _rx) = provider(&dir.join("granted"));
        let outside = dir.join("other.txt");
        let err = fs_provider
            .call("watchPath", &[Value::from(outside.to_str().unwrap())])
            .unwrap_err();
        assert!(matches!(err, PluginError::NotPermitted(_)), "{err:?}");
        assert!(matches!(
            fs_provider.call("watchPath", &[]),
            Err(PluginError::BadArguments(_))
        ));
        assert!(matches!(
            fs_provider.call("nope", &[]),
            Err(PluginError::UnknownMethod(_))
        ));
    }

    #[test]
    fn watching_a_file_reports_its_content_then_changes() {
        let (_tmp, dir) = canonical_dir();
        let path = dir.join("HEAD");
        fs::write(&path, "ref: refs/heads/main\n").unwrap();
        let (fs_provider, rx) = provider(&dir);
        fs_provider
            .call("watchPath", &[Value::from("{Home}/HEAD")])
            .unwrap();
        let first = next(&rx, "fileChanged");
        assert_eq!(first[0], path_value(&path));
        assert_eq!(first[1], "ref: refs/heads/main\n");

        // Replace by rename, as git does.
        fs::write(dir.join("HEAD.lock"), "ref: refs/heads/dev\n").unwrap();
        fs::rename(dir.join("HEAD.lock"), &path).unwrap();
        assert_eq!(next(&rx, "fileChanged")[1], "ref: refs/heads/dev\n");
        let diff = next(&rx, "fileContentDiff");
        assert_eq!(diff[1], Value::from(vec!["ref: refs/heads/dev"]));
        assert_eq!(diff[2], Value::from(vec!["ref: refs/heads/main"]));

        fs::remove_file(&path).unwrap();
        assert_eq!(next(&rx, "fileRemoved")[0], path_value(&path));
    }

    #[test]
    fn watching_a_missing_file_reports_it_when_created() {
        let (_tmp, dir) = canonical_dir();
        let (fs_provider, rx) = provider(&dir);
        fs_provider
            .call("watchPath", &[Value::from("{Home}/FETCH_HEAD")])
            .unwrap();
        fs::write(dir.join("FETCH_HEAD"), "abc\n").unwrap();
        assert_eq!(next(&rx, "fileChanged")[1], "abc\n");
    }

    #[test]
    fn watching_a_folder_lists_then_follows_its_files() {
        let (_tmp, dir) = canonical_dir();
        fs::write(dir.join("a.txt"), "one").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        let (fs_provider, rx) = provider(&dir);
        fs_provider
            .call("watchPath", &[Value::from("{Home}")])
            .unwrap();
        let added = next(&rx, "fileAdded");
        assert_eq!(added, vec![path_value(&dir.join("a.txt")), "one".into()]);

        fs::write(dir.join("b.txt"), "two").unwrap();
        assert_eq!(next(&rx, "fileAdded")[0], path_value(&dir.join("b.txt")));
        fs::remove_file(dir.join("a.txt")).unwrap();
        assert_eq!(next(&rx, "fileRemoved")[0], path_value(&dir.join("a.txt")));

        fs_provider.call("clearPaths", &[]).unwrap();
        fs::write(dir.join("c.txt"), "three").unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            rx.try_iter()
                .all(|(_, args)| args[0] != path_value(&dir.join("c.txt"))),
            "cleared watches stay silent"
        );
    }
}
