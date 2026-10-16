//! The subscriptions file, `llm-usage.toml` (see `examples/llm-usage/llm-usage.example.toml`).
//!
//! ```toml
//! poll_seconds = 300
//!
//! [[subscription]]
//! provider = "claude"          # uses Claude Code's login
//!
//! [[subscription]]
//! provider = "ollama"
//! api_key = "…"
//! ```
//!
//! It holds secrets, so it is refused when anyone but its owner can read it (`chmod 600`).

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::model::ProviderKind;

/// File name inside the EdgeHost data folder.
pub const FILE_NAME: &str = "llm-usage.toml";

/// Default and smallest poll interval, in seconds.
pub const DEFAULT_POLL_SECS: u64 = 300;
/// Smallest poll interval; the services rate-limit faster polling.
pub const MIN_POLL_SECS: u64 = 60;

/// Kimi Code's API (international accounts: `https://api.kimi.ai/coding/v1`).
pub const KIMI_BASE_URL: &str = "https://api.kimi.com/coding/v1";

/// A credential: never printed.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wrap a value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for a request header.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// Where one subscription's credentials come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Claude Code's login (Keychain on macOS, else `~/.claude/.credentials.json`), refreshed and
    /// written back when it expires.
    ClaudeCode,
    /// A Claude OAuth access token given in the config; never refreshed.
    ClaudeToken(Secret),
    /// The Codex CLI's `auth.json`, refreshed and written back when it expires.
    CodexAuthFile(PathBuf),
    /// A `ChatGPT` access token and account id given in the config; never refreshed.
    CodexToken {
        /// Bearer token.
        access_token: Secret,
        /// `ChatGPT-Account-Id`.
        account_id: String,
    },
    /// The `Cookie` header of a signed-in gemini.google.com tab.
    GeminiCookie(Secret),
    /// A Kimi Code API key.
    Kimi {
        /// Bearer key.
        api_key: Secret,
        /// API root, without a trailing slash.
        base_url: String,
    },
    /// An Ollama API key.
    Ollama(Secret),
}

impl Source {
    /// The service this source belongs to.
    #[must_use]
    pub const fn provider(&self) -> ProviderKind {
        match self {
            Self::ClaudeCode | Self::ClaudeToken(_) => ProviderKind::Claude,
            Self::CodexAuthFile(_) | Self::CodexToken { .. } => ProviderKind::Codex,
            Self::GeminiCookie(_) => ProviderKind::Gemini,
            Self::Kimi { .. } => ProviderKind::Kimi,
            Self::Ollama(_) => ProviderKind::Ollama,
        }
    }
}

/// One subscription to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Unique id (defaults to the provider key, then `key-2`, …).
    pub id: String,
    /// Display name.
    pub label: String,
    /// Credentials.
    pub source: Source,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Poll interval in seconds, at least [`MIN_POLL_SECS`].
    pub poll_secs: u64,
    /// Enabled subscriptions, in file order.
    pub entries: Vec<Entry>,
}

/// Why the config file cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// No file: the widget explains how to create one.
    #[error("{} does not exist; create it with `edgectl llm-usage init`", path.display())]
    Missing {
        /// Expected location.
        path: PathBuf,
    },
    /// Others can read the secrets in it.
    #[error("{} is readable by other users (mode {mode:o}); run `chmod 600` on it", path.display())]
    TooOpen {
        /// The file.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
    },
    /// Reading failed.
    #[error("reading {}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// Not valid TOML, or unknown keys. Only the reason and position are kept: the parser's own
    /// message quotes the offending line, which may hold a secret.
    #[error("{}:{line}:{column}: {message}", path.display())]
    Syntax {
        /// The file.
        path: PathBuf,
        /// 1-based line.
        line: usize,
        /// 1-based column.
        column: usize,
        /// The reason, without the line's text.
        message: String,
    },
    /// A `[[subscription]]` entry is incomplete or contradictory.
    #[error("{}: subscription {index} ({provider}): {reason}", path.display())]
    Entry {
        /// The file.
        path: PathBuf,
        /// 1-based position in the file.
        index: usize,
        /// Its provider.
        provider: ProviderKind,
        /// What is wrong.
        reason: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    poll_seconds: Option<u64>,
    #[serde(default)]
    subscription: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    provider: ProviderKind,
    id: Option<String>,
    label: Option<String>,
    enabled: Option<bool>,
    access_token: Option<Secret>,
    account_id: Option<String>,
    auth_file: Option<PathBuf>,
    cookie: Option<Secret>,
    api_key: Option<Secret>,
    base_url: Option<String>,
}

impl RawEntry {
    /// The keys set on this entry besides the common ones.
    fn set_keys(&self) -> Vec<&'static str> {
        [
            ("access_token", self.access_token.is_some()),
            ("account_id", self.account_id.is_some()),
            ("auth_file", self.auth_file.is_some()),
            ("cookie", self.cookie.is_some()),
            ("api_key", self.api_key.is_some()),
            ("base_url", self.base_url.is_some()),
        ]
        .into_iter()
        .filter_map(|(k, set)| set.then_some(k))
        .collect()
    }

    fn source(self, home: Option<&Path>) -> Result<Source, String> {
        let allowed: &[&str] = match self.provider {
            ProviderKind::Claude => &["access_token"],
            ProviderKind::Codex => &["access_token", "account_id", "auth_file"],
            ProviderKind::Gemini => &["cookie"],
            ProviderKind::Kimi => &["api_key", "base_url"],
            ProviderKind::Ollama => &["api_key"],
        };
        if let Some(bad) = self.set_keys().into_iter().find(|k| !allowed.contains(k)) {
            return Err(format!(
                "`{bad}` does not apply; use {}",
                allowed.join(" / ")
            ));
        }
        let required = |v: Option<Secret>, key: &str| {
            v.filter(|s| !s.expose().trim().is_empty())
                .ok_or_else(|| format!("`{key}` is required"))
        };
        Ok(match self.provider {
            ProviderKind::Claude => match self.access_token {
                None => Source::ClaudeCode,
                token => Source::ClaudeToken(required(token, "access_token")?),
            },
            ProviderKind::Codex => match (self.access_token, self.account_id, self.auth_file) {
                (Some(access_token), Some(account_id), None) => Source::CodexToken {
                    access_token,
                    account_id,
                },
                (None, None, file) => Source::CodexAuthFile(match file {
                    Some(f) => expand_home(&f, home),
                    None => {
                        codex_auth_file(home).ok_or("cannot locate ~/.codex: $HOME is not set")?
                    }
                }),
                _ => {
                    return Err(
                        "give either `auth_file` (or nothing) or both `access_token` and `account_id`".into(),
                    );
                }
            },
            ProviderKind::Gemini => Source::GeminiCookie(required(self.cookie, "cookie")?),
            ProviderKind::Kimi => {
                let base_url = self
                    .base_url
                    .as_deref()
                    .unwrap_or(KIMI_BASE_URL)
                    .trim_end_matches('/')
                    .to_owned();
                if !base_url.starts_with("https://") {
                    return Err(
                        "`base_url` must start with https:// (the API key is sent to it)".into(),
                    );
                }
                Source::Kimi {
                    api_key: required(self.api_key, "api_key")?,
                    base_url,
                }
            }
            ProviderKind::Ollama => Source::Ollama(required(self.api_key, "api_key")?),
        })
    }
}

/// `~/x` → `<home>/x`.
fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// `$CODEX_HOME/auth.json`, else `~/.codex/auth.json`.
fn codex_auth_file(home: Option<&Path>) -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".codex")))
        .map(|d| d.join("auth.json"))
}

impl Config {
    /// Parse `text` (the contents of `path`, which is only used in messages).
    ///
    /// # Errors
    ///
    /// [`ConfigError::Syntax`] or [`ConfigError::Entry`].
    pub fn parse(text: &str, path: &Path, home: Option<&Path>) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| {
            let at = e.span().map_or(0, |s| s.start).min(text.len());
            let before = text.get(..at).unwrap_or_default();
            ConfigError::Syntax {
                path: path.to_path_buf(),
                line: before.matches('\n').count() + 1,
                column: before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1,
                message: e.message().trim().to_owned(),
            }
        })?;
        let mut ids = HashSet::new();
        let mut entries = Vec::with_capacity(raw.subscription.len());
        for (i, e) in raw.subscription.into_iter().enumerate() {
            let provider = e.provider;
            let err = |reason: String| ConfigError::Entry {
                path: path.to_path_buf(),
                index: i + 1,
                provider,
                reason,
            };
            if e.enabled == Some(false) {
                continue;
            }
            let label = e
                .label
                .clone()
                .unwrap_or_else(|| provider.display_name().to_owned());
            let id = match e.id.clone() {
                Some(id) if ids.contains(&id) => {
                    return Err(err(format!("id {id:?} is used twice")));
                }
                Some(id) => id,
                None => {
                    let key = provider.key();
                    // There are fewer ids than numbers, so the first free one is close.
                    (1..=ids.len() + 1)
                        .map(|n| match n {
                            1 => key.to_owned(),
                            n => format!("{key}-{n}"),
                        })
                        .find(|id| !ids.contains(id))
                        .unwrap_or_else(|| key.to_owned())
                }
            };
            let source = e.source(home).map_err(err)?;
            ids.insert(id.clone());
            entries.push(Entry { id, label, source });
        }
        Ok(Self {
            poll_secs: raw
                .poll_seconds
                .unwrap_or(DEFAULT_POLL_SECS)
                .max(MIN_POLL_SECS),
            entries,
        })
    }

    /// Read and parse `path`, refusing a file others can read.
    ///
    /// # Errors
    ///
    /// Any [`ConfigError`].
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let io = |source: std::io::Error| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        };
        let meta = match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::Missing {
                    path: path.to_path_buf(),
                });
            }
            other => other.map_err(io)?,
        };
        check_private(path, &meta)?;
        let text = std::fs::read_to_string(path).map_err(io)?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::parse(&text, path, home.as_deref())
    }
}

#[cfg(unix)]
fn check_private(path: &Path, meta: &std::fs::Metadata) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o777;
    if mode.trailing_zeros() >= 6 {
        Ok(())
    } else {
        Err(ConfigError::TooOpen {
            path: path.to_path_buf(),
            mode,
        })
    }
}

#[cfg(not(unix))]
fn check_private(_: &Path, _: &std::fs::Metadata) -> Result<(), ConfigError> {
    Ok(())
}

/// The data folder the app was started with (`edgehost --home`), if set.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Use `dir` as the EdgeHost data folder in [`default_path`]. Call it before the first fetch; later
/// calls are ignored.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// `$EDGEHOST_LLM_USAGE_CONFIG`, else `llm-usage.toml` in the EdgeHost data folder (the one given
/// to [`set_data_dir`], else `$EDGEHOST_HOME`, else `~/Library/Application Support/EdgeHost` on macOS or
/// `$XDG_DATA_HOME/edgehost` / `~/.local/share/edgehost` elsewhere). `None` without `$HOME`.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("EDGEHOST_LLM_USAGE_CONFIG").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(dir) = DATA_DIR.get() {
        return Some(dir.join(FILE_NAME));
    }
    if let Some(home) = std::env::var_os("EDGEHOST_HOME").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(home).join(FILE_NAME));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(platform_data_dir(&home).join(FILE_NAME))
}

#[cfg(target_os = "macos")]
fn platform_data_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/EdgeHost")
}

#[cfg(not(target_os = "macos"))]
fn platform_data_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from)
        .join("edgehost")
}

/// The commented example written by `edgectl llm-usage init`.
pub const EXAMPLE: &str = include_str!("../../../examples/llm-usage/llm-usage.example.toml");

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(
            text,
            Path::new("/x/llm-usage.toml"),
            Some(Path::new("/home/ada")),
        )
    }

    #[test]
    fn the_shipped_example_parses() {
        let c = parse(EXAMPLE).unwrap();
        assert_ne!(c.entries, []);
        assert_eq!(c.poll_secs, DEFAULT_POLL_SECS);
    }

    #[test]
    fn entries_get_defaults_and_unique_ids() {
        let c = parse(
            r#"
            poll_seconds = 5
            [[subscription]]
            provider = "claude"
            [[subscription]]
            provider = "claude"
            label = "Work Claude"
            access_token = "tok"
            [[subscription]]
            provider = "codex"
            auth_file = "~/alt/auth.json"
            [[subscription]]
            provider = "kimi"
            api_key = "k"
            base_url = "https://api.kimi.ai/coding/v1/"
            [[subscription]]
            provider = "ollama"
            api_key = "o"
            enabled = false
            "#,
        )
        .unwrap();
        assert_eq!(c.poll_secs, MIN_POLL_SECS);
        let ids: Vec<_> = c.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["claude", "claude-2", "codex", "kimi"]);
        assert_eq!(c.entries[0].label, "Claude");
        assert_eq!(c.entries[0].source, Source::ClaudeCode);
        assert_eq!(c.entries[1].label, "Work Claude");
        assert_eq!(c.entries[1].source, Source::ClaudeToken(Secret::new("tok")));
        assert_eq!(
            c.entries[2].source,
            Source::CodexAuthFile(PathBuf::from("/home/ada/alt/auth.json"))
        );
        assert_eq!(
            c.entries[3].source,
            Source::Kimi {
                api_key: Secret::new("k"),
                base_url: "https://api.kimi.ai/coding/v1".into()
            }
        );
    }

    #[test]
    fn mistakes_name_the_entry() {
        let msg = |text: &str| parse(text).unwrap_err().to_string();
        assert!(
            msg("[[subscription]]\nprovider = \"gemini\"")
                .contains("subscription 1 (gemini): `cookie` is required")
        );
        assert!(
            msg("[[subscription]]\nprovider = \"ollama\"\napi_key = \"  \"")
                .contains("`api_key` is required")
        );
        assert!(
            msg("[[subscription]]\nprovider = \"claude\"\ncookie = \"c\"")
                .contains("`cookie` does not apply")
        );
        assert!(
            msg("[[subscription]]\nprovider = \"codex\"\naccess_token = \"t\"")
                .contains("both `access_token` and `account_id`")
        );
        assert!(msg("[[subscription]]\nprovider = \"kimi\"\nid = \"a\"\napi_key = \"k\"\n[[subscription]]\nprovider = \"ollama\"\nid = \"a\"\napi_key = \"k\"").contains("used twice"));
        assert!(msg("[[subscription]]\nprovider = \"bard\"").contains("unknown variant"));
        assert!(
            msg("[[subscription]]\nprovider = \"claude\"\naccess_token = \"\"")
                .contains("`access_token` is required")
        );
        assert!(
            msg("[[subscription]]\nprovider = \"kimi\"\napi_key = \"k\"\nbase_url = \"http://x\"")
                .contains("https://")
        );
        assert!(msg("[[subscription]]\nprovider = \"claude\"\ntypo = 1").contains("unknown field"));
    }

    #[test]
    fn syntax_errors_do_not_echo_the_line() {
        let err = parse(
            "poll_seconds = 60\n[[subscription]]\nprovider = \"ollama\"\napikey = \"sk-secret\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(!err.contains("sk-secret"), "{err}");
        assert!(err.contains(":4:1: unknown field `apikey`"), "{err}");
        let err = parse("[[subscription]]\napi_key = \"sk-unclosed\n")
            .unwrap_err()
            .to_string();
        assert!(!err.contains("sk-unclosed"), "{err}");
    }

    #[test]
    fn secrets_are_not_printed() {
        assert_eq!(format!("{:?}", Secret::new("hunter2")), "Secret(…)");
    }

    #[cfg(unix)]
    #[test]
    fn load_refuses_a_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        assert!(matches!(
            Config::load(&path),
            Err(ConfigError::Missing { .. })
        ));
        std::fs::write(&path, "[[subscription]]\nprovider = \"claude\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("mode 644"), "{err}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Config::load(&path).unwrap().entries.len(), 1);
    }
}
