//! Path macros and `file` / `folder` permission grants.
//!
//! iCUE writes the same macros two ways: `<Documents>` in manifest permissions and `{Documents}` in
//! `watchPath` arguments. Both resolve through [`Macros`]; [`Grants`] then decides whether a resolved
//! path is covered by a declared permission.

use std::path::{Component, Path, PathBuf};

use icuew_manifest::{PathPermission, PathPermissionKind};

/// Why a path could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// `<Name>` / `{Name}` is not a known macro.
    #[error("unknown path macro {0}")]
    UnknownMacro(String),
    /// After macro expansion the path is not absolute.
    #[error("{0} is not an absolute path")]
    NotAbsolute(String),
    /// The path climbs out with `..`.
    #[error("{0} contains `..`")]
    ParentDir(String),
}

/// Which bracket style a path uses for its macros.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroStyle {
    /// `<Home>`: manifest permissions.
    Angle,
    /// `{Home}`: File System provider arguments.
    Curly,
}

impl MacroStyle {
    const fn brackets(self) -> (char, char) {
        match self {
            Self::Angle => ('<', '>'),
            Self::Curly => ('{', '}'),
        }
    }
}

/// The folders path macros expand to (macOS locations, SPEC §8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macros {
    home: PathBuf,
    temp: PathBuf,
}

impl Macros {
    /// Macros for `home` and the temporary folder `temp`.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>, temp: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            temp: temp.into(),
        }
    }

    /// Macros for the current user (`$HOME`, `$TMPDIR`); `None` without `$HOME`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        Some(Self::new(home, std::env::temp_dir()))
    }

    fn expand(&self, name: &str) -> Option<PathBuf> {
        let home = &self.home;
        Some(match name {
            "Home" => home.clone(),
            "Desktop" => home.join("Desktop"),
            "Documents" => home.join("Documents"),
            "Downloads" => home.join("Downloads"),
            "Music" => home.join("Music"),
            "Pictures" => home.join("Pictures"),
            "Movies" => home.join("Movies"),
            "Temp" => self.temp.clone(),
            "AppData" => home.join("Library/Application Support/EdgeHost/appdata"),
            "Runtime" => home.join("Library/Application Support"),
            _ => return None,
        })
    }

    /// Expand a leading macro in `raw` and normalise the result.
    ///
    /// Only a macro at the start is expanded (`{Home}/x`); iCUE documents no other position.
    ///
    /// # Errors
    ///
    /// An unknown macro, a relative result, or a `..` component.
    pub fn resolve(&self, raw: &str, style: MacroStyle) -> Result<PathBuf, PathError> {
        let (open, close) = style.brackets();
        let raw = raw.trim();
        let expanded = match raw
            .strip_prefix(open)
            .and_then(|rest| rest.split_once(close))
        {
            Some((name, rest)) => {
                let base = self
                    .expand(name)
                    .ok_or_else(|| PathError::UnknownMacro(format!("{open}{name}{close}")))?;
                base.join(rest.trim_start_matches(['/', '\\']))
            }
            None => PathBuf::from(raw),
        };
        normalise(&expanded).ok_or_else(|| {
            if expanded.is_absolute() {
                PathError::ParentDir(raw.to_owned())
            } else {
                PathError::NotAbsolute(raw.to_owned())
            }
        })
    }
}

/// Drop `.` components and trailing slashes; `None` for relative paths or any `..`.
fn normalise(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => return None,
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    Some(out)
}

/// Resolve symlinks where the path exists, so a link cannot point outside a grant.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Grant {
    File(PathBuf),
    Folder(PathBuf),
}

/// The `file` and `folder` permissions a widget declared, resolved to absolute paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants(Vec<Grant>);

impl Grants {
    /// Resolve declared permissions; entries with bad paths are reported on stderr and skipped.
    #[must_use]
    pub fn new(declared: &[PathPermission], macros: &Macros) -> Self {
        let grants = declared
            .iter()
            .filter_map(|p| match macros.resolve(&p.path, MacroStyle::Angle) {
                Ok(path) => {
                    let path = canonical(&path);
                    Some(match p.kind {
                        PathPermissionKind::File => Grant::File(path),
                        PathPermissionKind::Folder => Grant::Folder(path),
                    })
                }
                Err(e) => {
                    eprintln!("ignoring {:?} permission: {e}", p.kind);
                    None
                }
            })
            .collect();
        Self(grants)
    }

    /// Whether `path` (absolute, normalised) may be read: it is a granted file, or inside (or equal
    /// to) a granted folder. Symlinks are resolved first.
    #[must_use]
    pub fn allows(&self, path: &Path) -> bool {
        let path = canonical(path);
        self.0.iter().any(|g| match g {
            Grant::File(f) => *f == path,
            Grant::Folder(d) => path.starts_with(d),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macros() -> Macros {
        Macros::new("/Users/u", "/tmp/t")
    }

    fn perm(kind: PathPermissionKind, path: &str) -> PathPermission {
        PathPermission {
            kind,
            path: path.into(),
        }
    }

    #[test]
    fn expands_both_macro_styles() {
        let m = macros();
        assert_eq!(
            m.resolve("{Documents}/w/a.txt", MacroStyle::Curly),
            Ok(PathBuf::from("/Users/u/Documents/w/a.txt"))
        );
        assert_eq!(
            m.resolve("<Home>", MacroStyle::Angle),
            Ok(PathBuf::from("/Users/u"))
        );
        assert_eq!(
            m.resolve("<Runtime>/x", MacroStyle::Angle),
            Ok(PathBuf::from("/Users/u/Library/Application Support/x"))
        );
        assert_eq!(
            m.resolve("{Temp}/./x/", MacroStyle::Curly),
            Ok(PathBuf::from("/tmp/t/x"))
        );
        assert_eq!(
            m.resolve("/abs/path", MacroStyle::Curly),
            Ok(PathBuf::from("/abs/path"))
        );
    }

    #[test]
    fn rejects_bad_paths() {
        let m = macros();
        assert_eq!(
            m.resolve("{Nope}/x", MacroStyle::Curly),
            Err(PathError::UnknownMacro("{Nope}".into()))
        );
        assert!(matches!(
            m.resolve("relative/x", MacroStyle::Curly),
            Err(PathError::NotAbsolute(_))
        ));
        assert!(matches!(
            m.resolve("{Home}/../other", MacroStyle::Curly),
            Err(PathError::ParentDir(_))
        ));
        // Angle macros are not expanded in curly style (and vice versa): the result is relative.
        assert!(m.resolve("<Home>/x", MacroStyle::Curly).is_err());
    }

    #[test]
    fn grants_cover_files_and_folders() {
        let g = Grants::new(
            &[
                perm(PathPermissionKind::File, "<Documents>/w/data.txt"),
                perm(PathPermissionKind::Folder, "<Home>/projects"),
                perm(PathPermissionKind::Folder, "<Bogus>/x"),
            ],
            &macros(),
        );
        assert!(g.allows(Path::new("/Users/u/Documents/w/data.txt")));
        assert!(!g.allows(Path::new("/Users/u/Documents/w/other.txt")));
        assert!(g.allows(Path::new("/Users/u/projects")));
        assert!(g.allows(Path::new("/Users/u/projects/a/.git/HEAD")));
        assert!(!g.allows(Path::new("/Users/u/projects-evil/x")));
        assert!(!Grants::default().allows(Path::new("/Users/u/projects")));
    }

    #[test]
    fn symlink_out_of_a_grant_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let granted = root.path().join("granted");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&granted).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, granted.join("link")).unwrap();
        let g = Grants::new(
            &[perm(PathPermissionKind::Folder, granted.to_str().unwrap())],
            &macros(),
        );
        assert!(!g.allows(&granted.join("link/secret.txt")));
        std::fs::write(granted.join("ok.txt"), "y").unwrap();
        assert!(g.allows(&granted.join("ok.txt")));
    }
}
