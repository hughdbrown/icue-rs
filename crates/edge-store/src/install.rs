//! Installing widgets from `.icuewidget` / `.zip` packages or folders.
//!
//! Untrusted input: entries that escape the destination, symlinks, and oversized or overlong
//! archives are rejected before anything reaches the widgets folder. A package is staged in the
//! store's `tmp/`, validated, and only then renamed into `widgets/<id>/<version>/`.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use edge_plugins::addin::{Addin, Registry};
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
    /// The add-in folder is not a usable add-in.
    #[error("{0}")]
    InvalidAddin(String),
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
    let replaced = replace_dir(&root, &dest, &store.tmp_dir())?;
    Ok(InstallOutcome {
        id,
        version,
        path: dest,
        replaced,
        report,
    })
}

/// A successful add-in install.
#[derive(Debug)]
pub struct AddinOutcome {
    /// The installed add-in, loaded from its new place.
    pub addin: Addin,
    /// Whether an existing copy was replaced.
    pub replaced: bool,
}

/// Install the add-in folder `source` into `store`'s `addins/<id>/`, replacing an older copy.
///
/// # Errors
///
/// I/O, safety and size errors, and [`InstallError::InvalidAddin`] when the add-in does not load,
/// provides a built-in plugin, or provides the same plugin as another installed add-in.
pub fn install_addin(store: &Store, source: &Path) -> Result<AddinOutcome, InstallError> {
    let staging = tempfile::Builder::new()
        .prefix("addin-")
        .tempdir_in(store.tmp_dir())
        .map_err(|e| io_err("creating staging folder in", &store.tmp_dir(), e))?;
    let root = staging.path().join("addin");
    copy_tree(source, &root, &mut Budget::default())?;
    let addin = Addin::load(&root).map_err(|e| InstallError::InvalidAddin(e.to_string()))?;
    if edge_plugins::builtin(&addin.provides).is_some() {
        return Err(InstallError::InvalidAddin(format!(
            "{}:{} is built into EdgeHost",
            addin.provides.module, addin.provides.name
        )));
    }
    let (installed, _) = Registry::load(&store.addins_dir());
    if let Some(other) = installed.iter().find(|o| {
        o.id != addin.id
            && o.provides.module == addin.provides.module
            && o.provides.name == addin.provides.name
    }) {
        return Err(InstallError::InvalidAddin(format!(
            "add-in {} already provides {}:{}",
            other.id, addin.provides.module, addin.provides.name
        )));
    }
    let dest = store.addins_dir().join(&addin.id);
    let replaced = replace_dir(&root, &dest, &store.tmp_dir())?;
    let addin = Addin::load(&dest).map_err(|e| InstallError::InvalidAddin(e.to_string()))?;
    Ok(AddinOutcome { addin, replaced })
}

/// Move the staged folder `new` to `dest`, keeping any existing `dest` until the move has
/// succeeded: the old copy is first moved aside into a fresh folder under `tmp` (the store's own
/// `tmp/`, so both moves are renames on one file system), and put back if the new copy cannot be
/// moved in. The aside folder is separate from the staging folder, because `new` may be the
/// staging folder itself. Returns whether an existing copy was replaced.
fn replace_dir(new: &Path, dest: &Path, tmp: &Path) -> Result<bool, InstallError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err("creating", parent, e))?;
    }
    let aside = tempfile::Builder::new()
        .prefix("previous-")
        .tempdir_in(tmp)
        .map_err(|e| io_err("creating a folder in", tmp, e))?;
    let backup = aside.path().join("copy");
    let replaced = dest.exists();
    if replaced {
        fs::rename(dest, &backup)
            .map_err(|e| io_err("moving the installed copy aside", dest, e))?;
    }
    if let Err(e) = fs::rename(new, dest) {
        if replaced {
            if let Err(restore) = fs::rename(&backup, dest) {
                eprintln!(
                    "could not restore {} from {}: {restore}",
                    dest.display(),
                    backup.display()
                );
            }
        }
        return Err(io_err("moving the new copy into", dest, e));
    }
    // Dropping `aside` deletes the old copy.
    Ok(replaced)
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

    fn coin_prices() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/addins/dev.edgehost.coin-prices"
        ))
    }

    #[test]
    fn installs_and_replaces_an_addin_and_refuses_a_second_provider() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        let first = install_addin(&store, &coin_prices()).unwrap();
        assert_eq!(first.addin.id, "dev.edgehost.coin-prices");
        assert!(!first.replaced);
        assert_eq!(
            first.addin.dir,
            store.addins_dir().join("dev.edgehost.coin-prices")
        );
        assert!(install_addin(&store, &coin_prices()).unwrap().replaced);

        // Same plugin under another id.
        let copy = home.path().join("copy");
        copy_tree(&coin_prices(), &copy, &mut Budget::default()).unwrap();
        let toml = fs::read_to_string(copy.join("addin.toml")).unwrap();
        fs::write(
            copy.join("addin.toml"),
            toml.replace("dev.edgehost.coin-prices", "dev.other.prices"),
        )
        .unwrap();
        let err = install_addin(&store, &copy).unwrap_err().to_string();
        assert!(err.contains("already provides"), "{err}");

        fs::write(copy.join("main.rhai"), "fn handle(m, a) {").unwrap();
        assert!(matches!(
            install_addin(&store, &copy),
            Err(InstallError::InvalidAddin(_))
        ));
    }

    #[test]
    fn a_failed_replace_keeps_the_installed_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("widgets/w/1.0");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("index.html"), "old").unwrap();
        let scratch = tmp.path().join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        // The staged copy is missing, so moving it in fails.
        let err = replace_dir(&tmp.path().join("missing"), &dest, &scratch);
        assert!(matches!(err, Err(InstallError::Io { .. })), "{err:?}");
        assert_eq!(fs::read_to_string(dest.join("index.html")).unwrap(), "old");

        let new = tmp.path().join("new");
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("index.html"), "new").unwrap();
        assert!(replace_dir(&new, &dest, &scratch).unwrap());
        assert_eq!(fs::read_to_string(dest.join("index.html")).unwrap(), "new");
        assert_eq!(
            fs::read_dir(&scratch).unwrap().count(),
            0,
            "old copy deleted"
        );
        assert_eq!(
            fs::read_dir(&dest).unwrap().count(),
            1,
            "nothing extra in the new copy"
        );
        let fresh = tmp.path().join("fresh");
        fs::create_dir_all(&fresh).unwrap();
        assert!(!replace_dir(&fresh, &tmp.path().join("widgets/x/1.0"), &scratch).unwrap());
    }

    #[test]
    fn installs_from_folder_and_replaces() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        let first = install(&store, &hello()).unwrap();
        assert_eq!(first.id.as_str(), "dev.edgehost.hello-edge");
        assert!(!first.replaced && first.path.join("index.html").is_file());
        let again = install(&store, &hello()).unwrap();
        assert!(again.replaced);
        let names = |dir: &Path| -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            names(&again.path),
            names(&hello()),
            "the replaced copy holds only the widget"
        );
        assert_eq!(
            names(&store.tmp_dir()),
            Vec::<String>::new(),
            "no leftovers in tmp/"
        );
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
