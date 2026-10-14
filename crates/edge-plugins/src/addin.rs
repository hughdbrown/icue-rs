//! Third-party add-ins: Rhai scripts that provide a plugin namespace.
//!
//! An add-in is a folder holding `addin.toml` and `main.rhai`:
//!
//! ```toml
//! id = "dev.example.coin-prices"     # folder name in the store's addins/ directory
//! module = "edgehost.coinprices"     # with name and version: "edgehost.coinprices:CoinPrices:1.0"
//! name = "CoinPrices"
//! version = "1.0"
//! description = "Spot prices from Coinbase"
//! methods = ["getPrice", "watch"]    # become window.plugins.Coinprices.getPrice(…) etc.
//! signals = ["priceChanged"]         # become …priceChanged.connect(handler)
//! poll_ms = 30000                    # optional: run poll() this often
//!
//! [permissions]
//! http = ["api.coinbase.com"]        # hosts http_get() may reach (and their subdomains)
//! ```
//!
//! The script itself runs in [`crate::rhai_provider`]. [`Registry::load`] reads every add-in in a
//! folder and compiles its script, so mistakes surface at install time rather than in a widget.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use icuew_manifest::PluginRef;
use serde::Deserialize;

use crate::rhai_provider;

/// The descriptor file of an add-in.
pub const DESCRIPTOR_FILE: &str = "addin.toml";
/// The script file of an add-in.
pub const SCRIPT_FILE: &str = "main.rhai";
/// Shortest accepted `poll_ms`.
pub const MIN_POLL: Duration = Duration::from_millis(100);
/// Longest accepted `poll_ms` (one hour).
pub const MAX_POLL: Duration = Duration::from_secs(3600);
/// Largest accepted `main.rhai`.
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

/// Why an add-in could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum AddinError {
    /// A file could not be read.
    #[error("reading {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// `addin.toml` is not valid TOML or has unknown or mistyped keys.
    #[error("{DESCRIPTOR_FILE}: {0}")]
    Descriptor(String),
    /// A value in `addin.toml` is out of range or malformed.
    #[error("{DESCRIPTOR_FILE}: {0}")]
    Invalid(String),
    /// `main.rhai` does not compile, or lacks a function the descriptor needs.
    #[error("{SCRIPT_FILE}: {0}")]
    Script(String),
}

/// Raw `addin.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescriptorToml {
    id: String,
    module: String,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    signals: Vec<String>,
    poll_ms: Option<u64>,
    #[serde(default)]
    permissions: PermissionsToml,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionsToml {
    #[serde(default)]
    http: Vec<String>,
}

/// A host `http_get` may reach: the host and its subdomains, on `port` if given (otherwise the
/// URL's default port). Loopback hosts need an explicit port, as widget `url` permissions do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpGrant {
    /// Lower-case host name or IP address.
    pub host: String,
    /// Required port, if any.
    pub port: Option<u16>,
}

impl HttpGrant {
    fn parse(entry: &str) -> Result<Self, AddinError> {
        let bad = || {
            AddinError::Invalid(format!(
                "permissions.http entry {entry:?} must be a host name or IP, optionally with :port"
            ))
        };
        let entry = entry.trim().to_ascii_lowercase();
        let (host, port) = match entry.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_owned(), Some(p.parse().map_err(|_| bad())?)),
            _ => (entry.clone(), None),
        };
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && !host.starts_with(['.', '-'])
            && !host.ends_with(['.', '-']);
        if !valid {
            return Err(bad());
        }
        if is_loopback(&host) && port.is_none() {
            return Err(AddinError::Invalid(format!(
                "permissions.http entry {entry:?} is a loopback host and needs a port"
            )));
        }
        Ok(Self { host, port })
    }

    /// Whether this grant covers `host` (lower-case) on `port`.
    #[must_use]
    pub fn covers(&self, host: &str, port: u16, default_port: u16) -> bool {
        let host_ok = host == self.host || host.ends_with(&format!(".{}", self.host));
        let port_ok = match self.port {
            Some(p) => p == port,
            None => port == default_port && !is_loopback(host),
        };
        host_ok && port_ok
    }
}

use crate::net::is_loopback_host as is_loopback;

/// A loaded, compiled add-in.
#[derive(Debug)]
pub struct Addin {
    /// Folder name in the store.
    pub id: String,
    /// Where it was loaded from.
    pub dir: PathBuf,
    /// What it provides, in manifest form (`module:Name:X.Y`).
    pub provides: PluginRef,
    /// Free text.
    pub description: String,
    /// Method names.
    pub methods: Vec<String>,
    /// Signal names.
    pub signals: Vec<String>,
    /// `poll()` interval, if the add-in polls.
    pub poll: Option<Duration>,
    /// Hosts `http_get` may reach.
    pub http: Vec<HttpGrant>,
    /// `main.rhai`.
    pub source: String,
}

impl Addin {
    /// Read and check the add-in in `dir`, compiling its script.
    ///
    /// # Errors
    ///
    /// See [`AddinError`].
    pub fn load(dir: &Path) -> Result<Self, AddinError> {
        let descriptor_path = dir.join(DESCRIPTOR_FILE);
        let text = read(&descriptor_path)?;
        let raw: DescriptorToml =
            toml::from_str(&text).map_err(|e| AddinError::Descriptor(e.message().to_owned()))?;
        if raw.id.is_empty()
            || !raw
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
            || raw.id.starts_with('.')
        {
            return Err(AddinError::Invalid(format!(
                "id {:?} may use only a-z, 0-9, '.', '_' and '-'",
                raw.id
            )));
        }
        let provides: PluginRef = format!("{}:{}:{}", raw.module, raw.name, raw.version)
            .parse()
            .map_err(|_| {
                AddinError::Invalid(format!(
                    "module {:?}, name {:?} and version {:?} must form module:Name:X.Y (version like \"1.0\")",
                    raw.module, raw.name, raw.version
                ))
            })?;
        if provides.facade_name().is_empty()
            || !provides
                .facade_name()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(AddinError::Invalid(format!(
                "the last part of module {:?} must be letters, digits or '_' (it names window.plugins.{})",
                raw.module,
                provides.facade_name()
            )));
        }
        check_names("methods", &raw.methods)?;
        check_names("signals", &raw.signals)?;
        let poll = raw.poll_ms.map(Duration::from_millis);
        if let Some(p) = poll {
            if !(MIN_POLL..=MAX_POLL).contains(&p) {
                return Err(AddinError::Invalid(format!(
                    "poll_ms must be between {} and {}",
                    MIN_POLL.as_millis(),
                    MAX_POLL.as_millis()
                )));
            }
        }
        let http = raw
            .permissions
            .http
            .iter()
            .map(|h| HttpGrant::parse(h))
            .collect::<Result<Vec<_>, _>>()?;

        let script_path = dir.join(SCRIPT_FILE);
        let size = fs::metadata(&script_path)
            .map_err(|source| AddinError::Io {
                path: script_path.clone(),
                source,
            })?
            .len();
        if size > MAX_SCRIPT_BYTES {
            return Err(AddinError::Script(format!(
                "is larger than {MAX_SCRIPT_BYTES} bytes"
            )));
        }
        let source = read(&script_path)?;
        let functions = rhai_provider::script_functions(&source).map_err(AddinError::Script)?;
        let has = |name: &str, params: usize| functions.contains(&(name.to_owned(), params));
        if !raw.methods.is_empty() && !has("handle", 2) {
            return Err(AddinError::Script(
                "declares methods but has no `fn handle(method, args)`".into(),
            ));
        }
        if poll.is_some() && !has("poll", 0) {
            return Err(AddinError::Script(
                "sets poll_ms but has no `fn poll()`".into(),
            ));
        }
        if poll.is_none() && has("poll", 0) {
            return Err(AddinError::Script(
                "has `fn poll()` but addin.toml sets no poll_ms".into(),
            ));
        }
        Ok(Self {
            id: raw.id,
            dir: dir.to_path_buf(),
            provides,
            description: raw.description,
            methods: raw.methods,
            signals: raw.signals,
            poll,
            http,
            source,
        })
    }

    /// Whether this add-in satisfies a widget's `required_plugins` entry: same module and name,
    /// same major version, and a minor version at least as new.
    #[must_use]
    pub fn satisfies(&self, wanted: &PluginRef) -> bool {
        let p = &self.provides;
        wanted.module == p.module
            && wanted.name == p.name
            && wanted.major == p.major
            && wanted.minor <= p.minor
    }
}

fn read(path: &Path) -> Result<String, AddinError> {
    fs::read_to_string(path).map_err(|source| AddinError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Method and signal names become JS property names: identifiers, no duplicates.
fn check_names(field: &str, names: &[String]) -> Result<(), AddinError> {
    let mut seen = BTreeSet::new();
    for n in names {
        let mut chars = n.chars();
        let ident = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        if !ident {
            return Err(AddinError::Invalid(format!(
                "{field} entry {n:?} is not a JavaScript identifier"
            )));
        }
        if !seen.insert(n) {
            return Err(AddinError::Invalid(format!("{field} lists {n:?} twice")));
        }
    }
    Ok(())
}

/// The add-ins in one folder.
#[derive(Debug, Default)]
pub struct Registry {
    addins: Vec<Arc<Addin>>,
}

/// An add-in folder that could not be used.
#[derive(Debug)]
pub struct Rejected {
    /// The folder.
    pub dir: PathBuf,
    /// Why.
    pub reason: String,
}

impl Registry {
    /// Load every add-in under `dir` (one folder each). A missing `dir` is an empty registry.
    /// Folders that fail to load, whose id differs from their folder name, that provide a
    /// built-in plugin, or that repeat another add-in's plugin are returned as [`Rejected`].
    #[must_use]
    pub fn load(dir: &Path) -> (Self, Vec<Rejected>) {
        let mut registry = Self::default();
        let mut rejected = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return (registry, rejected);
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| {
                !p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        dirs.sort();
        for d in dirs {
            let reject = |reason: String| Rejected {
                dir: d.clone(),
                reason,
            };
            match Addin::load(&d) {
                Err(e) => rejected.push(reject(e.to_string())),
                Ok(a) if d.file_name().is_none_or(|n| n.to_string_lossy() != a.id) => {
                    rejected.push(reject(format!(
                        "its id {:?} does not match its folder name",
                        a.id
                    )));
                }
                Ok(a) if crate::builtin(&a.provides).is_some() => {
                    rejected.push(reject(format!(
                        "{} is built into EdgeHost",
                        a.provides.facade_name()
                    )));
                }
                Ok(a) => match registry.duplicate_of(&a) {
                    Some(other) => rejected.push(reject(format!(
                        "it provides {}:{} like add-in {other}",
                        a.provides.module, a.provides.name
                    ))),
                    None => registry.addins.push(Arc::new(a)),
                },
            }
        }
        (registry, rejected)
    }

    fn duplicate_of(&self, a: &Addin) -> Option<&str> {
        self.addins
            .iter()
            .find(|o| o.provides.module == a.provides.module && o.provides.name == a.provides.name)
            .map(|o| o.id.as_str())
    }

    /// The add-in that satisfies `wanted`, if any.
    #[must_use]
    pub fn find(&self, wanted: &PluginRef) -> Option<&Arc<Addin>> {
        self.addins.iter().find(|a| a.satisfies(wanted))
    }

    /// Every loaded add-in, by folder name.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Addin>> {
        self.addins.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = r#"
id = "dev.test.counter"
module = "edgehost.counter"
name = "Counter"
version = "1.2"
methods = ["get", "add"]
signals = ["changed"]
poll_ms = 1000

[permissions]
http = ["api.example.com", "localhost:9090"]
"#;
    const SCRIPT: &str = "fn handle(method, args) { 1 } fn poll() { }";

    fn write(dir: &Path, id: &str, toml: &str, script: &str) -> PathBuf {
        let d = dir.join(id);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(DESCRIPTOR_FILE), toml).unwrap();
        fs::write(d.join(SCRIPT_FILE), script).unwrap();
        d
    }

    #[test]
    fn loads_a_valid_addin() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Addin::load(&write(tmp.path(), "dev.test.counter", TOML, SCRIPT)).unwrap();
        assert_eq!(a.provides.to_string(), "edgehost.counter:Counter:1.2");
        assert_eq!(a.provides.facade_name(), "Counter");
        assert_eq!(a.poll, Some(Duration::from_secs(1)));
        assert_eq!(a.http[1].port, Some(9090));
        let wanted = |s: &str| s.parse::<PluginRef>().unwrap();
        assert!(a.satisfies(&wanted("edgehost.counter:Counter:1.0")));
        assert!(!a.satisfies(&wanted("edgehost.counter:Counter:1.3")));
        assert!(!a.satisfies(&wanted("edgehost.counter:Counter:2.0")));
    }

    #[test]
    fn rejects_bad_descriptors_and_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        let cases = [
            (
                TOML.replace("version = \"1.2\"", "version = \"one\""),
                SCRIPT,
                "module:Name:X.Y",
            ),
            (
                TOML.replace("\"get\"", "\"no-dash\""),
                SCRIPT,
                "JavaScript identifier",
            ),
            (TOML.replace("\"add\"", "\"get\""), SCRIPT, "twice"),
            (
                TOML.replace("poll_ms = 1000", "poll_ms = 5"),
                SCRIPT,
                "poll_ms",
            ),
            (
                TOML.replace("localhost:9090", "localhost"),
                SCRIPT,
                "needs a port",
            ),
            (
                TOML.replace("api.example.com", "https://x"),
                SCRIPT,
                "host name",
            ),
            (format!("{TOML}\nextra = 1"), SCRIPT, "unknown field"),
            (TOML.to_owned(), "fn poll() { }", "fn handle"),
            (TOML.to_owned(), "fn handle(m, a) { 1 }", "fn poll"),
            (TOML.to_owned(), "fn handle(m, a) { ", "main.rhai"),
            (TOML.replace("poll_ms = 1000", ""), SCRIPT, "no poll_ms"),
        ];
        for (toml, script, want) in cases {
            let d = write(tmp.path(), "x", &toml, script);
            let err = Addin::load(&d).unwrap_err().to_string();
            assert!(err.contains(want), "{want:?} not in {err:?}");
        }
    }

    #[test]
    fn http_grants_match_hosts_ports_and_subdomains() {
        let api = HttpGrant::parse("API.example.com").unwrap();
        assert!(api.covers("api.example.com", 443, 443));
        assert!(api.covers("eu.api.example.com", 80, 80));
        assert!(!api.covers("example.com", 443, 443));
        assert!(!api.covers("api.example.com", 8443, 443));
        let local = HttpGrant::parse("localhost:9090").unwrap();
        assert!(local.covers("localhost", 9090, 80));
        assert!(!local.covers("localhost", 80, 80));
    }

    #[test]
    fn registry_rejects_mismatched_ids_builtins_and_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "dev.test.counter", TOML, SCRIPT);
        write(tmp.path(), "wrong-folder", TOML, SCRIPT);
        let dup = TOML.replace("dev.test.counter", "dev.test.zcopy");
        write(tmp.path(), "dev.test.zcopy", &dup, SCRIPT);
        let fs_toml = "id = \"dev.test.fs\"\nmodule = \"widgetbuilder.filesystemprovider\"\nname = \"FileSystem\"\nversion = \"1.0\"\n";
        write(tmp.path(), "dev.test.fs", fs_toml, "");
        let (reg, rejected) = Registry::load(tmp.path());
        assert_eq!(
            reg.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["dev.test.counter"]
        );
        let reasons: Vec<&str> = rejected.iter().map(|r| r.reason.as_str()).collect();
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(reasons.iter().any(|r| r.contains("built into")));
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("like add-in dev.test.counter"))
        );
        assert!(reasons.iter().any(|r| r.contains("folder name")));
        let (empty, none) = Registry::load(&tmp.path().join("missing"));
        assert!(empty.iter().next().is_none() && none.is_empty());
    }
}
