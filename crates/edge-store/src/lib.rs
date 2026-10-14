//! EdgeHost's on-disk data.
//!
//! ```text
//! <root>/                      ~/Library/Application Support/EdgeHost (macOS), ~/.local/share/edgehost (Linux), or $EDGEHOST_HOME
//! ├── widgets/<id>/<version>/  installed, validated widget packages
//! ├── common/                  optional shared folder served at /common/ (iCUE's widgets/common)
//! ├── layout.json              pages, widget instances and their property values
//! └── tmp/                     staging for installs (same volume, so installs are a rename)
//! ```

mod install;
mod layout;

use std::cmp::Ordering;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub use install::{InstallError, InstallOutcome, install, unpack};
pub use layout::{Instance, InstanceId, Layout, LayoutError, Page};

use icuew_manifest::{WidgetDef, WidgetId};

/// Errors reading or writing the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A file or directory could not be read or written.
    #[error("{action} {path}")]
    Io {
        /// What was being done ("reading", "writing", …).
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// `layout.json` is not valid.
    #[error("parsing {path}")]
    Layout {
        /// The layout file.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// Neither `$EDGEHOST_HOME` nor `$HOME` is set.
    #[error("cannot locate the data folder: neither EDGEHOST_HOME nor HOME is set")]
    NoHome,
    /// The requested widget is not installed.
    #[error("widget {0} is not installed")]
    NotInstalled(String),
}

/// An installed widget version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Widget id.
    pub id: WidgetId,
    /// Version string (directory name).
    pub version: String,
    /// Display name from the manifest.
    pub name: String,
    /// Directory holding the widget.
    pub path: PathBuf,
}

/// Handle to the data folder.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// `$EDGEHOST_HOME`, else `~/Library/Application Support/EdgeHost` (macOS) or
    /// `$XDG_DATA_HOME/edgehost` / `~/.local/share/edgehost` (Linux).
    ///
    /// # Errors
    ///
    /// [`StoreError::NoHome`] if neither variable is set.
    pub fn default_root() -> Result<PathBuf, StoreError> {
        if let Some(home) = std::env::var_os("EDGEHOST_HOME") {
            return Ok(PathBuf::from(home));
        }
        let home = std::env::var_os("HOME").ok_or(StoreError::NoHome)?;
        Ok(platform_data_dir(Path::new(&home)))
    }

    /// Open (creating if needed) the store at `root`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the directories cannot be created.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let store = Self { root: root.into() };
        for dir in [store.widgets_dir(), store.tmp_dir()] {
            fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
                action: "creating",
                path: dir,
                source,
            })?;
        }
        Ok(store)
    }

    /// The data folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where installed widgets live.
    #[must_use]
    pub fn widgets_dir(&self) -> PathBuf {
        self.root.join("widgets")
    }

    /// Staging area for installs.
    #[must_use]
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The shared `common/` folder, if the user has put one there.
    #[must_use]
    pub fn common_dir(&self) -> Option<PathBuf> {
        Some(self.root.join("common")).filter(|d| d.is_dir())
    }

    /// The layout file.
    #[must_use]
    pub fn layout_path(&self) -> PathBuf {
        self.root.join("layout.json")
    }

    /// Directory of one installed widget version.
    #[must_use]
    pub fn widget_dir(&self, id: &WidgetId, version: &str) -> PathBuf {
        self.widgets_dir().join(id.as_str()).join(version)
    }

    /// All installed widget versions, sorted by id then version (oldest first).
    ///
    /// Directories that do not hold a loadable widget are skipped.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the widgets folder cannot be listed.
    pub fn installed(&self) -> Result<Vec<Installed>, StoreError> {
        let mut out = Vec::new();
        for id_dir in read_dirs(&self.widgets_dir())? {
            for version_dir in read_dirs(&id_dir)? {
                if let Ok(def) = WidgetDef::load(&version_dir) {
                    let version = file_name(&version_dir);
                    out.push(Installed {
                        id: def.manifest.id,
                        version,
                        name: def.manifest.name,
                        path: version_dir,
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            a.id.as_str()
                .cmp(b.id.as_str())
                .then_with(|| compare_versions(&a.version, &b.version))
        });
        Ok(out)
    }

    /// The newest installed version of `id`.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotInstalled`] if no version is installed, or a listing error.
    pub fn latest(&self, id: &WidgetId) -> Result<Installed, StoreError> {
        self.installed()?
            .into_iter()
            .filter(|w| &w.id == id)
            .max_by(|a, b| compare_versions(&a.version, &b.version))
            .ok_or_else(|| StoreError::NotInstalled(id.to_string()))
    }

    /// Delete one version, or every version when `version` is `None`.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotInstalled`] if nothing matched, or [`StoreError::Io`].
    pub fn uninstall(
        &self,
        id: &WidgetId,
        version: Option<&str>,
    ) -> Result<Vec<String>, StoreError> {
        let targets: Vec<Installed> = self
            .installed()?
            .into_iter()
            .filter(|w| &w.id == id && version.is_none_or(|v| v == w.version))
            .collect();
        if targets.is_empty() {
            return Err(StoreError::NotInstalled(
                version.map_or_else(|| id.to_string(), |v| format!("{id} {v}")),
            ));
        }
        for w in &targets {
            fs::remove_dir_all(&w.path).map_err(|source| StoreError::Io {
                action: "removing",
                path: w.path.clone(),
                source,
            })?;
        }
        let id_dir = self.widgets_dir().join(id.as_str());
        if read_dirs(&id_dir).is_ok_and(|v| v.is_empty()) {
            // Best effort: an empty id folder is harmless if it cannot be removed.
            let _ = fs::remove_dir(&id_dir);
        }
        Ok(targets.into_iter().map(|w| w.version).collect())
    }

    /// Load `layout.json`, or an empty layout if it does not exist yet.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] or [`StoreError::Layout`].
    pub fn load_layout(&self) -> Result<Layout, StoreError> {
        let path = self.layout_path();
        match fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|source| StoreError::Layout { path, source })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Layout::default()),
            Err(source) => Err(StoreError::Io {
                action: "reading",
                path,
                source,
            }),
        }
    }

    /// Write `layout.json` atomically (temporary file, then rename).
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`].
    pub fn save_layout(&self, layout: &Layout) -> Result<(), StoreError> {
        let path = self.layout_path();
        let io_err = |source| StoreError::Io {
            action: "writing",
            path: path.clone(),
            source,
        };
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root).map_err(io_err)?;
        let json = serde_json::to_string_pretty(layout).map_err(|e| io_err(io::Error::other(e)))?;
        tmp.write_all(json.as_bytes()).map_err(io_err)?;
        tmp.write_all(b"\n").map_err(io_err)?;
        tmp.persist(&path).map_err(|e| io_err(e.error))?;
        Ok(())
    }
}

/// macOS: `~/Library/Application Support/EdgeHost`.
#[cfg(target_os = "macos")]
fn platform_data_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/EdgeHost")
}

/// Linux: `$XDG_DATA_HOME/edgehost`, else `~/.local/share/edgehost`.
#[cfg(not(target_os = "macos"))]
fn platform_data_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from)
        .join("edgehost")
}

/// Compare dotted version strings numerically where possible (`1.10.0` > `1.9.2`).
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parts = |v: &str| {
        v.split('.')
            .map(|p| p.parse::<u64>().ok())
            .collect::<Vec<_>>()
    };
    let (pa, pb) = (parts(a), parts(b));
    if pa.iter().chain(&pb).all(Option::is_some) {
        pa.cmp(&pb)
    } else {
        a.cmp(b)
    }
}

fn read_dirs(dir: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(StoreError::Io {
                action: "listing",
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    Ok(entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert_eq!(compare_versions("1.10.0", "1.9.2"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(
            compare_versions("1.0.0-beta", "1.0.0"),
            "1.0.0-beta".cmp("1.0.0")
        );
    }

    #[test]
    fn layout_round_trip_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.load_layout().unwrap(), Layout::default());
        let layout = Layout {
            display: Some("windowed".into()),
            ..Layout::default()
        };
        store.save_layout(&layout).unwrap();
        assert_eq!(store.load_layout().unwrap(), layout);
    }
}
