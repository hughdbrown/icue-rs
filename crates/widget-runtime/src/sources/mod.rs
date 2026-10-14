//! Host data sources served on a widget's own origin at `/__edgehost/<source>/<operation>`
//! (see `docs/data-sources.md`). Requests are answered on a background thread; responses are JSON
//! (200) or a plain-text error with a meaningful status.
//!
//! Access is limited to the folders the widget declares (`{"type": "folder", "path": …}`).

pub mod git;

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use wry::RequestAsyncResponder;
use wry::http::{Response, header};

use crate::assets::percent_decode;
use crate::platform;

/// Path prefix of every data-source route.
pub const PREFIX: &str = "/__edgehost/";

/// A data-source failure, mapped to an HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// Missing or malformed parameters.
    #[error("{0}")]
    BadRequest(String),
    /// The path is outside the widget's declared folders.
    #[error("{0}")]
    Forbidden(String),
    /// No such source, operation, folder or repository.
    #[error("{0}")]
    NotFound(String),
    /// The underlying tool failed.
    #[error("{0}")]
    Failed(String),
}

impl SourceError {
    fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Failed(_) => 502,
        }
    }
}

/// Folders a widget may read, resolved and canonicalised.
#[derive(Debug, Clone, Default)]
pub struct Access {
    folders: Vec<PathBuf>,
}

/// Where the path macros point.
#[derive(Debug, Clone)]
pub struct Env {
    /// The user's home folder.
    pub home: PathBuf,
    /// EdgeHost's data folder (`<AppData>`).
    pub app_data: PathBuf,
}

impl Env {
    /// Folder for a path macro name (without angle brackets).
    #[must_use]
    pub fn macro_dir(&self, name: &str) -> Option<PathBuf> {
        Some(match name {
            "Home" => self.home.clone(),
            "Desktop" => self.home.join("Desktop"),
            "Documents" => self.home.join("Documents"),
            "Downloads" => self.home.join("Downloads"),
            "Music" => self.home.join("Music"),
            "Pictures" => self.home.join("Pictures"),
            "Movies" => platform::videos_dir(&self.home),
            "Temp" => std::env::temp_dir(),
            "AppData" => self.app_data.clone(),
            "Runtime" => platform::runtime_dir(&self.home),
            _ => return None,
        })
    }

    /// Expand a leading `<Macro>` or `~/`; other paths must already be absolute.
    #[must_use]
    pub fn expand(&self, raw: &str) -> Option<PathBuf> {
        let raw = raw.trim();
        if let Some(rest) = raw.strip_prefix('<') {
            let (name, tail) = rest.split_once('>')?;
            let base = self.macro_dir(name)?;
            return Some(base.join(tail.trim_start_matches(['/', '\\'])));
        }
        if raw == "~" {
            return Some(self.home.clone());
        }
        if let Some(tail) = raw.strip_prefix("~/") {
            return Some(self.home.join(tail));
        }
        let p = PathBuf::from(raw);
        p.is_absolute().then_some(p)
    }
}

impl Access {
    /// Resolve the widget's declared folder permissions. Entries that do not exist are skipped.
    #[must_use]
    pub fn new(declared: &[String], env: &Env) -> Self {
        let folders = declared
            .iter()
            .filter_map(|raw| env.expand(raw))
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        Self { folders }
    }

    /// Canonical form of `path` if it lies inside a declared folder.
    ///
    /// # Errors
    ///
    /// [`SourceError::NotFound`] if it does not exist, [`SourceError::Forbidden`] if it is outside
    /// every declared folder.
    pub fn check(&self, path: &Path) -> Result<PathBuf, SourceError> {
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(SourceError::BadRequest(format!(
                "{} must not contain ..",
                path.display()
            )));
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| SourceError::NotFound(format!("{} does not exist", path.display())))?;
        if self.folders.iter().any(|root| canonical.starts_with(root)) {
            Ok(canonical)
        } else {
            Err(SourceError::Forbidden(format!(
                "{} is not inside a folder this widget may read",
                canonical.display()
            )))
        }
    }
}

/// Everything a data-source request needs.
#[derive(Debug, Clone)]
pub struct Context {
    /// Folders the widget may read.
    pub access: Access,
    /// Macro resolution.
    pub env: Env,
}

/// Decoded query parameters.
#[derive(Debug, Default)]
pub struct Query(Vec<(String, String)>);

impl Query {
    /// Parse `a=1&b=x%20y`.
    #[must_use]
    pub fn parse(query: Option<&str>) -> Self {
        Self(
            query
                .unwrap_or_default()
                .split('&')
                .filter(|p| !p.is_empty())
                .filter_map(|pair| {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    Some((percent_decode(k)?, percent_decode(&v.replace('+', " "))?))
                })
                .collect(),
        )
    }

    /// The first value of `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Answer `/__edgehost/<source>/<op>` on a background thread.
pub fn handle(
    ctx: Arc<Context>,
    path: String,
    query: Option<String>,
    responder: RequestAsyncResponder,
) {
    std::thread::spawn(move || {
        let result = route(&ctx, &path, &Query::parse(query.as_deref()));
        match &result {
            Err(e) => eprintln!("data source {path}: {} {e}", e.status()),
            Ok(_) if std::env::var_os("EDGEHOST_DEBUG_SOURCES").is_some() => {
                eprintln!(
                    "data source {path}?{}: ok",
                    query.as_deref().unwrap_or_default()
                );
            }
            Ok(_) => {}
        }
        responder.respond(respond(&result));
    });
}

/// Dispatch a data-source request.
///
/// # Errors
///
/// [`SourceError::NotFound`] for unknown sources, and the source's own errors.
pub fn route(ctx: &Context, path: &str, query: &Query) -> Result<Value, SourceError> {
    let rest = path.strip_prefix(PREFIX).unwrap_or(path);
    match rest.split_once('/') {
        Some(("git", op)) => git::handle(ctx, &git::SystemGit, op, query),
        _ => Err(SourceError::NotFound(format!("no data source at {path}"))),
    }
}

fn respond(result: &Result<Value, SourceError>) -> Response<Cow<'static, [u8]>> {
    let built = match result {
        Ok(value) => Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Cow::Owned(value.to_string().into_bytes())),
        Err(e) => Response::builder()
            .status(e.status())
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Cow::Owned(e.to_string().into_bytes())),
    };
    built.unwrap_or_else(|_| Response::new(Cow::Borrowed(b"internal error".as_slice())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(home: &Path) -> Env {
        Env {
            home: home.to_path_buf(),
            app_data: home.join("data"),
        }
    }

    #[test]
    fn expands_macros_and_tilde() {
        let e = env(Path::new("/h"));
        assert_eq!(
            e.expand("<Home>/projects"),
            Some(PathBuf::from("/h/projects"))
        );
        assert_eq!(e.expand("<Documents>"), Some(PathBuf::from("/h/Documents")));
        assert_eq!(e.expand("~/x"), Some(PathBuf::from("/h/x")));
        assert_eq!(e.expand("<AppData>/a"), Some(PathBuf::from("/h/data/a")));
        assert_eq!(e.expand("relative/path"), None);
        assert_eq!(e.expand("<Nope>/x"), None);
    }

    #[test]
    fn access_is_limited_to_declared_folders() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("projects/app")).unwrap();
        std::fs::create_dir_all(home.path().join("secret")).unwrap();
        let access = Access::new(&["<Home>/projects".to_owned()], &env(home.path()));
        assert!(access.check(&home.path().join("projects/app")).is_ok());
        assert!(matches!(
            access.check(&home.path().join("secret")),
            Err(SourceError::Forbidden(_))
        ));
        assert!(matches!(
            access.check(&home.path().join("projects/missing")),
            Err(SourceError::NotFound(_))
        ));
        assert!(matches!(
            access.check(&home.path().join("projects/../secret")),
            Err(SourceError::BadRequest(_))
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                home.path().join("secret"),
                home.path().join("projects/link"),
            )
            .unwrap();
            assert!(matches!(
                access.check(&home.path().join("projects/link")),
                Err(SourceError::Forbidden(_))
            ));
        }
    }

    #[test]
    fn query_parsing() {
        let q = Query::parse(Some("repo=%2Fa%20b&n=5&flag"));
        assert_eq!(q.get("repo"), Some("/a b"));
        assert_eq!(q.get("n"), Some("5"));
        assert_eq!(q.get("flag"), Some(""));
        assert_eq!(q.get("x"), None);
    }

    #[test]
    fn unknown_source_is_404() {
        let ctx = Context {
            access: Access::default(),
            env: env(Path::new("/h")),
        };
        let err = route(&ctx, "/__edgehost/weather/now", &Query::default()).unwrap_err();
        assert_eq!(err.status(), 404);
    }
}
