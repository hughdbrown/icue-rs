//! Serving widget files over `icuew://<instance>/<path>`.
//!
//! [`resolve`] is a pure, lexical mapping from URL path to a path under the widget root;
//! [`read_asset`] then canonicalises the result so symlinks cannot escape the root either.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Errors while resolving or reading a widget asset.
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    /// The URL path tries to leave the widget directory.
    #[error("path escapes the widget directory: {0:?}")]
    Escapes(String),
    /// The URL path has invalid percent-encoding or is not UTF-8.
    #[error("invalid percent-encoding in {0:?}")]
    BadEncoding(String),
    /// The file could not be read.
    #[error("reading {path}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
}

impl AssetError {
    /// HTTP status that best describes the error.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::Escapes(_) => 403,
            Self::BadEncoding(_) => 400,
            Self::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => 404,
            Self::Io { .. } => 500,
        }
    }
}

/// Map a URL path to a file path under `root`, lexically. An empty path means `index.html`.
///
/// ```
/// use std::path::Path;
/// use widget_runtime::assets::resolve;
/// let root = Path::new("/w");
/// assert_eq!(resolve(root, "/").unwrap(), Path::new("/w/index.html"));
/// assert_eq!(resolve(root, "/styles/a%20b.css").unwrap(), Path::new("/w/styles/a b.css"));
/// assert!(resolve(root, "/../etc/passwd").is_err());
/// ```
///
/// # Errors
///
/// [`AssetError::BadEncoding`] for malformed `%XX` escapes or non-UTF-8 paths, and
/// [`AssetError::Escapes`] for `..` segments, backslashes or NUL bytes.
pub fn resolve(root: &Path, url_path: &str) -> Result<PathBuf, AssetError> {
    let decoded =
        percent_decode(url_path).ok_or_else(|| AssetError::BadEncoding(url_path.to_owned()))?;
    let mut out = root.to_path_buf();
    let mut any = false;
    for seg in decoded.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if seg == ".." || seg.contains(['\\', '\0']) {
            return Err(AssetError::Escapes(url_path.to_owned()));
        }
        out.push(seg);
        any = true;
    }
    if !any {
        out.push("index.html");
    }
    Ok(out)
}

/// Read the asset at `url_path`, refusing anything outside `root` after resolving symlinks.
///
/// # Errors
///
/// The errors of [`resolve`], [`AssetError::Escapes`] if the canonical path leaves the root, and
/// [`AssetError::Io`] if the file cannot be read.
pub fn read_asset(root: &Path, url_path: &str) -> Result<Vec<u8>, AssetError> {
    let canonical_root = root.canonicalize().map_err(|e| io_err(root, e))?;
    let candidate = resolve(&canonical_root, url_path)?;
    let canonical = candidate
        .canonicalize()
        .map_err(|e| io_err(&candidate, e))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(AssetError::Escapes(url_path.to_owned()));
    }
    fs::read(&canonical).map_err(|e| io_err(&canonical, e))
}

fn io_err(path: &Path, source: io::Error) -> AssetError {
    AssetError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// MIME type for a widget asset, by extension.
#[must_use]
pub fn mime_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("bmp") => "image/bmp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rejects_traversal_and_bad_encoding() {
        let root = Path::new("/w");
        for bad in ["/..", "/a/../../b", "/%2e%2e/x", "/a\\b", "/a%00b"] {
            assert!(
                matches!(resolve(root, bad), Err(AssetError::Escapes(_))),
                "{bad}"
            );
        }
        for bad in ["/%zz", "/%4", "/%ff"] {
            assert!(
                matches!(resolve(root, bad), Err(AssetError::BadEncoding(_))),
                "{bad}"
            );
        }
        assert_eq!(resolve(root, "/./a//b.js").unwrap(), Path::new("/w/a/b.js"));
    }

    #[test]
    fn read_asset_serves_files_and_blocks_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "nope").unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "<p>hi</p>").unwrap();
        assert_eq!(read_asset(root.path(), "/").unwrap(), b"<p>hi</p>");
        assert_eq!(
            read_asset(root.path(), "/missing.js").unwrap_err().status(),
            404
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                outside.path().join("secret.txt"),
                root.path().join("link.txt"),
            )
            .unwrap();
            assert!(matches!(
                read_asset(root.path(), "/link.txt"),
                Err(AssetError::Escapes(_))
            ));
        }
    }

    #[test]
    fn mime_types() {
        assert_eq!(
            mime_for(Path::new("modules/a.MJS")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_for(Path::new("icon.svg")), "image/svg+xml");
        assert_eq!(mime_for(Path::new("blob")), "application/octet-stream");
    }
}
