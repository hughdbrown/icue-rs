//! Installing widgets from `.icuewidget` / `.zip` packages or folders.
//!
//! Untrusted input: entries that escape the destination, symlinks, and oversized or overlong
//! archives are rejected before anything reaches the widgets folder. A package is staged in the
//! store's `tmp/`, validated, and only then renamed into `widgets/<id>/<version>/`.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use icuew_manifest::{Report, WidgetId, validate_dir};

use crate::Store;

/// Maximum total uncompressed size of a package.
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum number of entries in a package.
pub const MAX_ENTRIES: usize = 10_000;

/// Errors installing a widget.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// A file could not be read or written.
    #[error("{action} {path}")]
    Io {
        /// What was being done.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// The archive is not a readable zip file.
    #[error("reading archive {path}")]
    Zip {
        /// The archive.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: zip::result::ZipError,
    },
    /// The package contains something unsafe (path escape, symlink).
    #[error("refusing unsafe package entry {0:?}")]
    Unsafe(String),
    /// The package exceeds [`MAX_TOTAL_BYTES`] or [`MAX_ENTRIES`].
    #[error("package is too large (limits: {MAX_ENTRIES} entries, {} MiB)", MAX_TOTAL_BYTES / 1024 / 1024)]
    TooLarge,
    /// No `manifest.json` at the top level or in a single top-level folder.
    #[error("no manifest.json found in the package")]
    NoManifest,
    /// Validation found errors.
    #[error("the widget failed validation")]
    Invalid(Box<Report>),
}

/// A successful install.
#[derive(Debug)]
pub struct InstallOutcome {
    /// Widget id.
    pub id: WidgetId,
    /// Installed version.
    pub version: String,
    /// Where it was installed.
    pub path: PathBuf,
    /// Whether an existing copy of this version was replaced.
    pub replaced: bool,
    /// Validation report (warnings only).
    pub report: Report,
}

/// Install a package file or widget folder into `store`.
///
/// # Errors
///
/// See [`InstallError`]; validation errors come back as [`InstallError::Invalid`] with the report.
pub fn install(store: &Store, source: &Path) -> Result<InstallOutcome, InstallError> {
    let staging = tempfile::Builder::new()
        .prefix("install-")
        .tempdir_in(store.tmp_dir())
        .map_err(|e| io_err("creating staging folder in", &store.tmp_dir(), e))?;
    let root = unpack(source, staging.path())?;
    let mut report = validate_dir(&root);
    if report.has_errors() {
        return Err(InstallError::Invalid(Box::new(report)));
    }
    let Some(def) = report.widget.take() else {
        return Err(InstallError::Invalid(Box::new(report)));
    };
    let id = def.manifest.id;
    let version = def.manifest.version;
    let dest = store.widget_dir(&id, &version);
    let replaced = dest.exists();
    if replaced {
        fs::remove_dir_all(&dest).map_err(|e| io_err("removing old copy", &dest, e))?;
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err("creating", parent, e))?;
    }
    fs::rename(&root, &dest).map_err(|e| io_err("moving widget into", &dest, e))?;
    Ok(InstallOutcome {
        id,
        version,
        path: dest,
        replaced,
        report,
    })
}

/// Unpack `source` (archive or folder) into `dest` and return the widget's root inside it: `dest`
/// itself, or the single top-level folder that holds `manifest.json`.
///
/// # Errors
///
/// I/O, zip, safety and size errors, and [`InstallError::NoManifest`].
pub fn unpack(source: &Path, dest: &Path) -> Result<PathBuf, InstallError> {
    let meta = fs::metadata(source).map_err(|e| io_err("reading", source, e))?;
    let mut budget = Budget::default();
    if meta.is_dir() {
        copy_tree(source, dest, &mut budget)?;
    } else {
        extract_zip(source, dest, &mut budget)?;
    }
    if dest.join("manifest.json").is_file() {
        return Ok(dest.to_path_buf());
    }
    let mut dirs = fs::read_dir(dest)
        .map_err(|e| io_err("listing", dest, e))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path());
    match (dirs.next(), dirs.next()) {
        (Some(only), None) if only.join("manifest.json").is_file() => Ok(only),
        _ => Err(InstallError::NoManifest),
    }
}

#[derive(Default)]
struct Budget {
    entries: usize,
    bytes: u64,
}

impl Budget {
    fn entry(&mut self) -> Result<(), InstallError> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            Err(InstallError::TooLarge)
        } else {
            Ok(())
        }
    }

    fn remaining(&self) -> u64 {
        MAX_TOTAL_BYTES.saturating_sub(self.bytes)
    }

    fn spend(&mut self, n: u64) -> Result<(), InstallError> {
        self.bytes = self.bytes.saturating_add(n);
        if self.bytes > MAX_TOTAL_BYTES {
            Err(InstallError::TooLarge)
        } else {
            Ok(())
        }
    }
}

/// Skip hidden files and macOS archive metadata, as `icuewidget package` does.
fn is_ignored(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            name.starts_with('.') || name == "__MACOSX" || name == "__pycache__"
        }
        _ => false,
    })
}

fn extract_zip(archive_path: &Path, dest: &Path, budget: &mut Budget) -> Result<(), InstallError> {
    let zip_err = |source| InstallError::Zip {
        path: archive_path.to_path_buf(),
        source,
    };
    let file = File::open(archive_path).map_err(|e| io_err("opening", archive_path, e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(zip_err)?;
    if archive.len() > MAX_ENTRIES {
        return Err(InstallError::TooLarge);
    }
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(zip_err)?;
        let raw_name = entry
            .name()
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default();
        let Some(rel) = entry.enclosed_name() else {
            return Err(InstallError::Unsafe(raw_name));
        };
        if is_ignored(&rel) {
            continue;
        }
        if entry.is_symlink() {
            return Err(InstallError::Unsafe(raw_name));
        }
        budget.entry()?;
        let out = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| io_err("creating", &out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err("creating", parent, e))?;
        }
        let mut file = File::create(&out).map_err(|e| io_err("writing", &out, e))?;
        // Read one byte past the budget so an oversized entry is detected without trusting headers.
        let limit = budget.remaining().saturating_add(1);
        let written = io::copy(&mut (&mut entry).take(limit), &mut file)
            .map_err(|e| io_err("extracting", &out, e))?;
        budget.spend(written)?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path, budget: &mut Budget) -> Result<(), InstallError> {
    fs::create_dir_all(to).map_err(|e| io_err("creating", to, e))?;
    for entry in fs::read_dir(from).map_err(|e| io_err("listing", from, e))? {
        let entry = entry.map_err(|e| io_err("listing", from, e))?;
        let path = entry.path();
        if is_ignored(Path::new(&entry.file_name())) {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|e| io_err("inspecting", &path, e))?;
        if kind.is_symlink() {
            return Err(InstallError::Unsafe(path.display().to_string()));
        }
        budget.entry()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&path, &target, budget)?;
        } else {
            let n = fs::copy(&path, &target).map_err(|e| io_err("copying", &path, e))?;
            budget.spend(n)?;
        }
    }
    Ok(())
}

fn io_err(action: &'static str, path: &Path, source: io::Error) -> InstallError {
    InstallError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn hello() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/widgets/hello-edge"
        ))
    }

    fn zip_dir(dir: &Path, prefix: &str, out: &Path) {
        let mut zw = zip::ZipWriter::new(File::create(out).unwrap());
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p
                        .strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    zw.start_file(format!("{prefix}{rel}"), opts).unwrap();
                    zw.write_all(&fs::read(&p).unwrap()).unwrap();
                }
            }
        }
        zw.finish().unwrap();
    }

    #[test]
    fn installs_from_folder_and_replaces() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        let first = install(&store, &hello()).unwrap();
        assert_eq!(first.id.as_str(), "dev.edgehost.hello-edge");
        assert!(!first.replaced && first.path.join("index.html").is_file());
        assert!(install(&store, &hello()).unwrap().replaced);
        assert_eq!(store.installed().unwrap().len(), 1);
        assert_eq!(store.latest(&first.id).unwrap().version, first.version);
        assert_eq!(
            store.uninstall(&first.id, None).unwrap(),
            vec![first.version]
        );
        assert_eq!(store.installed().unwrap(), Vec::new());
    }

    #[test]
    fn installs_from_zip_with_top_level_folder() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        let pkg = home.path().join("hello.icuewidget");
        zip_dir(&hello(), "hello-edge/", &pkg);
        let out = install(&store, &pkg).unwrap();
        assert!(out.path.join("modules/Greeting.mjs").is_file());
    }

    #[test]
    fn rejects_traversal_entries() {
        let home = tempfile::tempdir().unwrap();
        let pkg = home.path().join("evil.zip");
        let mut zw = zip::ZipWriter::new(File::create(&pkg).unwrap());
        zw.start_file("../escape.txt", SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"x").unwrap();
        zw.finish().unwrap();
        let dest = tempfile::tempdir().unwrap();
        assert!(matches!(
            unpack(&pkg, dest.path()),
            Err(InstallError::Unsafe(_))
        ));
        assert!(!home.path().join("escape.txt").exists());
    }

    #[test]
    fn invalid_widget_is_not_installed() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("manifest.json"), "{}").unwrap();
        fs::write(src.path().join("index.html"), "<html></html>").unwrap();
        assert!(matches!(
            install(&store, src.path()),
            Err(InstallError::Invalid(_))
        ));
        assert_eq!(store.installed().unwrap(), Vec::new());
    }
}
