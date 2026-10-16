//! The CLIs' own logins: Claude Code (Keychain or `.credentials.json`) and Codex (`auth.json`).
//!
//! Both rotate refresh tokens, so a refresh here must be written back or the CLI's saved login
//! stops working. The rules:
//! - use the stored access token while it is valid;
//! - when it has expired (or the service rejects it), take [`REFRESH_LOCK`] and an advisory file
//!   lock beside the store (so `edgectl` and EdgeHost do not refresh at once), **re-read** the
//!   store (the CLI may have refreshed it meanwhile), and only then refresh;
//! - write back the whole stored document with just the token fields replaced, so fields this
//!   crate does not know survive;
//! - if that write fails, keep the new document in memory, use it, and retry the write on the next
//!   read: the old refresh token is already spent.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};

use base64::Engine as _;
use jiff::Timestamp;
use serde_json::{Map, Value, json};

use crate::error::FetchError;
use crate::http::{Body, Http, Request, host_of};

/// One refresh at a time in this process, so two widgets cannot spend the same refresh token.
pub static REFRESH_LOCK: Mutex<()> = Mutex::new(());

/// Refreshed documents that could not be written yet: (store description, document).
static UNSAVED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Refresh this long before the stored expiry.
const EXPIRY_MARGIN_MS: i64 = 60_000;

/// Somewhere a credential document is kept.
pub trait SecretStore: Send + Sync {
    /// The document, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// The store could not be read.
    fn read(&self) -> Result<Option<String>, FetchError>;

    /// Replace the document.
    ///
    /// # Errors
    ///
    /// The store could not be written.
    fn write(&self, value: &str) -> Result<(), FetchError>;

    /// Where it is, for messages.
    fn describe(&self) -> String;

    /// The advisory lock file that serialises refreshes across processes.
    fn lock_path(&self) -> PathBuf;

    /// Fail early, before a refresh spends the refresh token, when the store cannot be written.
    ///
    /// # Errors
    ///
    /// The store is read-only.
    fn check_writable(&self) -> Result<(), FetchError> {
        Ok(())
    }
}

/// The stored document, preferring one refreshed earlier that could not be saved (whose save is
/// retried now).
fn read_doc(store: &dyn SecretStore) -> Result<Option<String>, FetchError> {
    let key = store.describe();
    let mut unsaved = UNSAVED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(i) = unsaved.iter().position(|(k, _)| *k == key) {
        let doc = unsaved.get(i).map(|(_, d)| d.clone()).unwrap_or_default();
        if store.write(&doc).is_ok() {
            unsaved.remove(i);
        }
        return Ok(Some(doc));
    }
    drop(unsaved);
    store.read()
}

/// Write a refreshed `doc`; on failure keep it for [`read_doc`] and report it on stderr, since the
/// refresh already happened and the new tokens are the only working ones.
fn save_doc(store: &dyn SecretStore, doc: &str) {
    if let Err(e) = store.write(doc) {
        eprintln!(
            "llm-usage: refreshed the login but could not save it to {} ({e}); will retry",
            store.describe()
        );
        let key = store.describe();
        let mut unsaved = UNSAVED.lock().unwrap_or_else(PoisonError::into_inner);
        unsaved.retain(|(k, _)| *k != key);
        unsaved.push((key, doc.to_owned()));
    }
}

/// Held while refreshing: this process's [`REFRESH_LOCK`] and the store's advisory file lock.
struct RefreshGuard {
    _thread: std::sync::MutexGuard<'static, ()>,
    _file: std::fs::File,
}

fn refresh_guard(store: &dyn SecretStore) -> Result<RefreshGuard, FetchError> {
    let thread = REFRESH_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let path = store.lock_path();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| FetchError::Store(format!("opening lock {}: {e}", path.display())))?;
    file.lock()
        .map_err(|e| FetchError::Store(format!("locking {}: {e}", path.display())))?;
    Ok(RefreshGuard {
        _thread: thread,
        _file: file,
    })
}

/// A file, replaced atomically and kept at mode 0600.
pub struct FileStore(pub PathBuf);

impl SecretStore for FileStore {
    fn read(&self) -> Result<Option<String>, FetchError> {
        match std::fs::read_to_string(&self.0) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(FetchError::Store(format!(
                "reading {}: {e}",
                self.0.display()
            ))),
        }
    }

    fn write(&self, value: &str) -> Result<(), FetchError> {
        write_private(&self.0, value)
            .map_err(|e| FetchError::Store(format!("writing {}: {e}", self.0.display())))
    }

    fn describe(&self) -> String {
        self.0.display().to_string()
    }

    fn lock_path(&self) -> PathBuf {
        sibling(&self.0, "edgehost-lock")
    }

    fn check_writable(&self) -> Result<(), FetchError> {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&self.0)
            .map(drop)
            .map_err(|e| FetchError::Store(format!("{} is not writable: {e}", self.0.display())))
    }
}

/// `dir/.name.<suffix>` beside `path`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().map_or_else(
        || "credentials".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    dir.join(format!(".{name}.{suffix}"))
}

/// Write `value` to a sibling temporary file with mode 0600, then rename it over `path`.
fn write_private(path: &Path, value: &str) -> std::io::Result<()> {
    // Per process, so two processes writing at once cannot interleave in one temp file.
    let tmp = sibling(path, &format!("edgehost-tmp-{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = opts
        .open(&tmp)
        .and_then(|mut f| f.write_all(value.as_bytes()).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A macOS Keychain generic password, read and written with `/usr/bin/security` (the tool Claude
/// Code itself uses, so its access list already allows it).
pub struct KeychainStore {
    /// Service name, e.g. `Claude Code-credentials`.
    pub service: String,
}

const SECURITY: &str = "/usr/bin/security";

impl KeychainStore {
    fn run(args: &[&str]) -> Result<std::process::Output, FetchError> {
        Command::new(SECURITY)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| FetchError::Store(format!("running {SECURITY}: {e}")))
    }

    /// The item's account name, which an update must repeat.
    fn account(&self) -> Result<Option<String>, FetchError> {
        let out = Self::run(&["find-generic-password", "-s", &self.service])?;
        if !out.status.success() {
            return Ok(None);
        }
        // The attributes go to stdout: `    "acct"<blob>="ada"`.
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text.lines().find_map(|l| {
            l.trim()
                .strip_prefix("\"acct\"<blob>=\"")
                .and_then(|v| v.strip_suffix('"'))
                .map(str::to_owned)
        }))
    }
}

/// Exit status of `security find-generic-password` when there is no such item.
const ITEM_NOT_FOUND: i32 = 44;

impl SecretStore for KeychainStore {
    fn read(&self) -> Result<Option<String>, FetchError> {
        let out = Self::run(&["find-generic-password", "-s", &self.service, "-w"])?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
            return Ok(Some(text));
        }
        if out.status.code() == Some(ITEM_NOT_FOUND) {
            return Ok(None);
        }
        Err(FetchError::Store(format!(
            "reading Keychain item {:?}: {}",
            self.service,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }

    fn write(&self, value: &str) -> Result<(), FetchError> {
        let account = match self.account()? {
            Some(a) => a,
            None => {
                std::env::var("USER").map_err(|_| FetchError::Store("$USER is not set".into()))?
            }
        };
        keychain_set(&self.service, &account, value.as_bytes())
    }

    fn describe(&self) -> String {
        format!("Keychain item {:?}", self.service)
    }

    fn lock_path(&self) -> PathBuf {
        let name: String = self
            .service
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        std::env::temp_dir().join(format!("edgehost-{name}.lock"))
    }
}

/// Create or update a generic password through the Security framework. (`security -i` cuts input
/// lines at about 4 KB, which a hex-encoded credentials document easily exceeds.)
#[cfg(target_os = "macos")]
fn keychain_set(service: &str, account: &str, value: &[u8]) -> Result<(), FetchError> {
    security_framework::passwords::set_generic_password(service, account, value)
        .map_err(|e| FetchError::Store(format!("updating Keychain item {service:?}: {e}")))
}

#[cfg(not(target_os = "macos"))]
fn keychain_set(service: &str, _: &str, _: &[u8]) -> Result<(), FetchError> {
    Err(FetchError::Store(format!(
        "Keychain item {service:?}: there is no Keychain on this system"
    )))
}

/// Where Claude Code keeps its login on this machine: the Keychain on macOS;
/// `$CLAUDE_CONFIG_DIR/.credentials.json` or `~/.claude/.credentials.json` elsewhere.
///
/// # Errors
///
/// Off macOS, when neither `$CLAUDE_CONFIG_DIR` nor `$HOME` is set.
pub fn claude_code_store() -> Result<Box<dyn SecretStore>, FetchError> {
    if cfg!(target_os = "macos") {
        return Ok(Box::new(KeychainStore {
            service: "Claude Code-credentials".into(),
        }));
    }
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
        .map(|d| Box::new(FileStore(d.join(".credentials.json"))) as Box<dyn SecretStore>)
        .ok_or_else(|| FetchError::Store("cannot locate ~/.claude: $HOME is not set".into()))
}

/// An access token in hand, and whether it was just refreshed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Bearer token.
    pub access: String,
    /// Extra id the request needs (Codex's account id).
    pub account_id: Option<String>,
    /// Plan, when the store records it (Claude's `subscriptionType`).
    pub plan: Option<String>,
}

/// When the token is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// The stored token unless it has expired.
    Valid,
    /// A new one: the service rejected the stored token.
    Fresh,
}

// ── Claude Code ─────────────────────────────────────────────────────────────────────────────

const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_SIGN_IN: &str = "run `claude` and sign in again (/login)";

struct ClaudeDoc {
    doc: Value,
    access: String,
    refresh: Option<String>,
    expires_at_ms: Option<i64>,
    plan: Option<String>,
}

fn claude_doc(store: &dyn SecretStore) -> Result<ClaudeDoc, FetchError> {
    let missing = || {
        FetchError::MissingCredential(format!(
            "Claude Code is not signed in ({}); {CLAUDE_SIGN_IN}",
            store.describe()
        ))
    };
    let text = read_doc(store)?.ok_or_else(missing)?;
    let doc: Value = serde_json::from_str(&text)
        .map_err(|e| FetchError::Store(format!("{} is not JSON: {e}", store.describe())))?;
    let oauth = doc.get("claudeAiOauth").ok_or_else(missing)?;
    let access = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(missing)?
        .to_owned();
    Ok(ClaudeDoc {
        refresh: oauth
            .get("refreshToken")
            .and_then(Value::as_str)
            .map(str::to_owned),
        expires_at_ms: oauth.get("expiresAt").and_then(Value::as_i64),
        plan: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(capitalise),
        access,
        doc,
    })
}

fn is_valid(expires_at_ms: Option<i64>, now: Timestamp) -> bool {
    expires_at_ms.is_none_or(|e| e - EXPIRY_MARGIN_MS > now.as_millisecond())
}

/// Claude Code's access token, refreshed and written back to `store` when needed.
///
/// # Errors
///
/// Not signed in, the refresh was refused, or the store failed.
pub fn claude_token(
    store: &dyn SecretStore,
    http: &dyn Http,
    want: Want,
    now: Timestamp,
) -> Result<Token, FetchError> {
    let d = claude_doc(store)?;
    if want == Want::Valid && is_valid(d.expires_at_ms, now) {
        return Ok(Token {
            access: d.access,
            account_id: None,
            plan: d.plan,
        });
    }
    let _guard = refresh_guard(store)?;
    let fresh = claude_doc(store)?;
    if fresh.access != d.access && is_valid(fresh.expires_at_ms, now) {
        // Someone (the CLI, another widget) refreshed it meanwhile.
        return Ok(Token {
            access: fresh.access,
            account_id: None,
            plan: fresh.plan,
        });
    }
    let refresh = fresh.refresh.clone().ok_or_else(|| {
        FetchError::SignInAgain(format!("Claude Code's login has expired; {CLAUDE_SIGN_IN}"))
    })?;
    store.check_writable()?;
    let body = Body::Form(vec![
        ("grant_type".into(), "refresh_token".into()),
        ("refresh_token".into(), refresh),
        ("client_id".into(), CLAUDE_CLIENT_ID.into()),
    ]);
    let tokens = post_refresh(http, &Request::post(CLAUDE_TOKEN_URL, body), CLAUDE_SIGN_IN)?;
    let mut doc = fresh.doc;
    let oauth = doc
        .get_mut("claudeAiOauth")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| FetchError::Store("claudeAiOauth vanished".into()))?;
    oauth.insert("accessToken".into(), tokens.access.clone().into());
    if let Some(r) = tokens.refresh {
        oauth.insert("refreshToken".into(), r.into());
    }
    match tokens.expires_in {
        Some(secs) => {
            oauth.insert(
                "expiresAt".into(),
                (now.as_millisecond() + secs * 1000).into(),
            );
        }
        // The old expiry has passed; leaving it would refresh again on every poll.
        None => {
            oauth.remove("expiresAt");
        }
    }
    save_doc(store, &doc.to_string());
    Ok(Token {
        access: tokens.access,
        account_id: None,
        plan: fresh.plan,
    })
}

// ── Codex ───────────────────────────────────────────────────────────────────────────────────

const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_SIGN_IN: &str = "run `codex login` again";

struct CodexDoc {
    doc: Value,
    access: String,
    refresh: Option<String>,
    account_id: String,
}

fn codex_doc(store: &dyn SecretStore) -> Result<CodexDoc, FetchError> {
    let missing = || {
        FetchError::MissingCredential(format!(
            "Codex is not signed in with ChatGPT ({}); run `codex login`",
            store.describe()
        ))
    };
    let text = read_doc(store)?.ok_or_else(missing)?;
    let doc: Value = serde_json::from_str(&text)
        .map_err(|e| FetchError::Store(format!("{} is not JSON: {e}", store.describe())))?;
    let tokens = doc
        .get("tokens")
        .filter(|t| t.is_object())
        .ok_or_else(missing)?;
    let field = |k: &str| {
        tokens
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let access = field("access_token").ok_or_else(missing)?.to_owned();
    let account_id = field("account_id")
        .map(str::to_owned)
        .or_else(|| jwt_claim(&access, "chatgpt_account_id"))
        .ok_or_else(missing)?;
    Ok(CodexDoc {
        refresh: field("refresh_token").map(str::to_owned),
        account_id,
        access,
        doc,
    })
}

/// A JWT's payload as JSON (unverified: only used to read the expiry and account of our own token).
fn jwt_payload(jwt: &str) -> Option<Map<String, Value>> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    match serde_json::from_slice(&bytes).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn jwt_claim(jwt: &str, claim: &str) -> Option<String> {
    let p = jwt_payload(jwt)?;
    p.get(claim)
        .or_else(|| p.get("https://api.openai.com/auth")?.get(claim))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn jwt_expiry_ms(jwt: &str) -> Option<i64> {
    jwt_payload(jwt)?.get("exp")?.as_i64().map(|s| s * 1000)
}

/// The Codex CLI's access token, refreshed and written back to `store` when needed.
///
/// # Errors
///
/// Not signed in, the refresh was refused, or the store failed.
pub fn codex_token(
    store: &dyn SecretStore,
    http: &dyn Http,
    want: Want,
    now: Timestamp,
) -> Result<Token, FetchError> {
    let d = codex_doc(store)?;
    let token = |d: CodexDoc| Token {
        access: d.access,
        account_id: Some(d.account_id),
        plan: None,
    };
    if want == Want::Valid && is_valid(jwt_expiry_ms(&d.access), now) {
        return Ok(token(d));
    }
    let _guard = refresh_guard(store)?;
    let fresh = codex_doc(store)?;
    if fresh.access != d.access && is_valid(jwt_expiry_ms(&fresh.access), now) {
        return Ok(token(fresh));
    }
    let refresh = fresh.refresh.clone().ok_or_else(|| {
        FetchError::SignInAgain(format!("Codex's login has expired; {CODEX_SIGN_IN}"))
    })?;
    store.check_writable()?;
    let body = Body::Json(json!({
        "grant_type": "refresh_token",
        "client_id": CODEX_CLIENT_ID,
        "refresh_token": refresh,
        "scope": "openid profile email",
    }));
    let tokens = post_refresh(http, &Request::post(CODEX_TOKEN_URL, body), CODEX_SIGN_IN)?;
    let mut doc = fresh.doc;
    let slot = doc
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| FetchError::Store("tokens vanished".into()))?;
    slot.insert("access_token".into(), tokens.access.clone().into());
    if let Some(r) = tokens.refresh {
        slot.insert("refresh_token".into(), r.into());
    }
    if let Some(id) = tokens.id_token {
        slot.insert("id_token".into(), id.into());
    }
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("last_refresh".into(), now.to_string().into());
    }
    let text = serde_json::to_string_pretty(&doc)
        .map_err(|e| FetchError::Store(format!("encoding auth.json: {e}")))?;
    save_doc(store, &text);
    Ok(Token {
        access: tokens.access,
        account_id: Some(fresh.account_id),
        plan: None,
    })
}

// ── Shared ──────────────────────────────────────────────────────────────────────────────────────

/// Access tokens that were just refreshed and still got 401: refreshing again would not help.
static REJECTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Remember that `access`, fresh from a refresh, was still rejected.
pub(crate) fn mark_rejected(access: &str) {
    let mut r = REJECTED.lock().unwrap_or_else(PoisonError::into_inner);
    if r.len() >= 16 {
        r.remove(0);
    }
    r.push(access.to_owned());
}

/// Whether `access` was refreshed and still rejected before.
pub(crate) fn was_rejected(access: &str) -> bool {
    REJECTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|a| a == access)
}

struct Refreshed {
    access: String,
    refresh: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

fn post_refresh(http: &dyn Http, req: &Request, sign_in: &str) -> Result<Refreshed, FetchError> {
    let host = host_of(&req.url).to_owned();
    let r = http.send(req)?;
    if !(200..300).contains(&r.status) {
        // 400 invalid_grant, refresh_token_expired, refresh_token_reused: the login is gone.
        if matches!(r.status, 400 | 401 | 403) {
            return Err(FetchError::SignInAgain(format!(
                "{host} refused to refresh the login; {sign_in}"
            )));
        }
        return Err(FetchError::from_status(&host, &r, sign_in));
    }
    let v: Value =
        serde_json::from_str(&r.body).map_err(|e| FetchError::parse(&host, e.to_string()))?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Ok(Refreshed {
        access: s("access_token").ok_or_else(|| FetchError::parse(&host, "no access_token"))?,
        refresh: s("refresh_token"),
        id_token: s("id_token"),
        expires_in: v.get("expires_in").and_then(Value::as_i64),
    })
}

/// `max` → `Max`, `prolite` → `Prolite`.
#[must_use]
pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod fake {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::SecretStore;
    use crate::error::FetchError;

    /// An in-memory store that counts writes and can refuse them.
    pub struct MemStore {
        pub value: Mutex<Option<String>>,
        pub writes: Mutex<usize>,
        pub fail_writes: Mutex<bool>,
        name: String,
        lock: tempfile::NamedTempFile,
    }

    impl MemStore {
        pub fn new(v: Option<&str>) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            Self {
                value: Mutex::new(v.map(str::to_owned)),
                writes: Mutex::new(0),
                fail_writes: Mutex::new(false),
                // Unique: the unsaved-document cache is keyed by the description.
                name: format!("memory-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
                lock: tempfile::NamedTempFile::new().unwrap(),
            }
        }

        pub fn json(&self) -> serde_json::Value {
            serde_json::from_str(self.value.lock().unwrap().as_deref().unwrap()).unwrap()
        }
    }

    impl SecretStore for MemStore {
        fn read(&self) -> Result<Option<String>, FetchError> {
            Ok(self.value.lock().unwrap().clone())
        }

        fn write(&self, value: &str) -> Result<(), FetchError> {
            if *self.fail_writes.lock().unwrap() {
                return Err(FetchError::Store("read-only".into()));
            }
            *self.value.lock().unwrap() = Some(value.to_owned());
            *self.writes.lock().unwrap() += 1;
            Ok(())
        }

        fn describe(&self) -> String {
            self.name.clone()
        }

        fn lock_path(&self) -> PathBuf {
            self.lock.path().to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::MemStore;
    use super::*;
    use crate::http::fake::FakeHttp;

    const NOW: &str = "2026-10-16T12:00:00Z";

    fn now() -> Timestamp {
        NOW.parse().unwrap()
    }

    fn claude_json(access: &str, expires_at: i64) -> String {
        json!({
            "claudeAiOauth": {
                "accessToken": access, "refreshToken": "rt-old", "expiresAt": expires_at,
                "scopes": ["user:inference", "user:profile"], "subscriptionType": "max"
            },
            "mcpOAuth": {"keep": "me"}
        })
        .to_string()
    }

    #[test]
    fn claude_uses_a_valid_token_without_network() {
        let store = MemStore::new(Some(&claude_json(
            "at-1",
            now().as_millisecond() + 3_600_000,
        )));
        let http = FakeHttp::default();
        let t = claude_token(&store, &http, Want::Valid, now()).unwrap();
        assert_eq!(t.access, "at-1");
        assert_eq!(t.plan.as_deref(), Some("Max"));
        assert_eq!(http.urls(), Vec::<String>::new());
    }

    #[test]
    fn claude_refreshes_an_expired_token_and_writes_it_back() {
        let store = MemStore::new(Some(&claude_json("at-1", now().as_millisecond() - 1)));
        let http = FakeHttp::default().reply(
            200,
            r#"{"access_token":"at-2","refresh_token":"rt-new","expires_in":28800,"token_type":"Bearer"}"#,
        );
        let t = claude_token(&store, &http, Want::Valid, now()).unwrap();
        assert_eq!(t.access, "at-2");
        assert_eq!(http.urls(), [CLAUDE_TOKEN_URL]);
        let saved = store.json();
        assert_eq!(saved["claudeAiOauth"]["accessToken"], "at-2");
        assert_eq!(saved["claudeAiOauth"]["refreshToken"], "rt-new");
        assert_eq!(
            saved["claudeAiOauth"]["expiresAt"],
            now().as_millisecond() + 28_800_000
        );
        assert_eq!(
            saved["claudeAiOauth"]["subscriptionType"], "max",
            "other fields kept"
        );
        assert_eq!(saved["mcpOAuth"]["keep"], "me");
        let sent = http.sent.lock().unwrap();
        let Body::Form(form) = &sent[0].body else {
            panic!("form body")
        };
        assert!(form.contains(&("refresh_token".into(), "rt-old".into())));
    }

    #[test]
    fn claude_keeps_the_refresh_token_when_none_comes_back() {
        let store = MemStore::new(Some(&claude_json("at-1", 0)));
        let http = FakeHttp::default().reply(200, r#"{"access_token":"at-2"}"#);
        claude_token(&store, &http, Want::Valid, now()).unwrap();
        assert_eq!(store.json()["claudeAiOauth"]["refreshToken"], "rt-old");
    }

    #[test]
    fn claude_without_expires_in_drops_the_stale_expiry() {
        let store = MemStore::new(Some(&claude_json("at-1", 1)));
        let http = FakeHttp::default().reply(200, r#"{"access_token":"at-2"}"#);
        claude_token(&store, &http, Want::Valid, now()).unwrap();
        assert!(store.json()["claudeAiOauth"].get("expiresAt").is_none());
        // Valid from now on (until a 401), so no refresh on the next poll.
        let t = claude_token(&store, &FakeHttp::default(), Want::Valid, now()).unwrap();
        assert_eq!(t.access, "at-2");
    }

    #[test]
    fn a_failed_save_keeps_the_refreshed_login_and_retries() {
        let store = MemStore::new(Some(&claude_json("at-1", 0)));
        *store.fail_writes.lock().unwrap() = true;
        let http = FakeHttp::default().reply(
            200,
            r#"{"access_token":"at-2","refresh_token":"rt-2","expires_in":3600}"#,
        );
        assert_eq!(
            claude_token(&store, &http, Want::Valid, now())
                .unwrap()
                .access,
            "at-2"
        );
        assert_eq!(
            store.json()["claudeAiOauth"]["accessToken"],
            "at-1",
            "not saved yet"
        );
        // Next poll: uses the unsaved login without refreshing (no scripted reply would fail).
        let t = claude_token(&store, &FakeHttp::default(), Want::Valid, now()).unwrap();
        assert_eq!(t.access, "at-2");
        // Once the store accepts writes, the retry saves it.
        *store.fail_writes.lock().unwrap() = false;
        claude_token(&store, &FakeHttp::default(), Want::Valid, now()).unwrap();
        assert_eq!(store.json()["claudeAiOauth"]["refreshToken"], "rt-2");
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_file_is_not_refreshed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let expired = jwt(&json!({"exp": now().as_second() - 10}));
        std::fs::write(&path, codex_json(&expired)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        let http = FakeHttp::default();
        let err = codex_token(&FileStore(path), &http, Want::Valid, now()).unwrap_err();
        assert!(err.to_string().contains("not writable"), "{err}");
        assert_eq!(http.urls(), Vec::<String>::new(), "refresh token not spent");
    }

    /// Writes, reads back and deletes a throwaway 6 KB Keychain item. Run by hand on macOS:
    /// `cargo test -p llm-usage keychain_round_trip -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "touches the login Keychain"]
    fn keychain_round_trip_of_a_large_item() {
        use security_framework::passwords::{delete_generic_password, get_generic_password};
        let service = "EdgeHost llm-usage test item";
        let account = std::env::var("USER").unwrap();
        let store = KeychainStore {
            service: service.into(),
        };
        let big = json!({"claudeAiOauth": {"accessToken": "x".repeat(6000)}}).to_string();
        store.write(&big).unwrap();
        // Read back through the framework: this test binary created the item, so `security`
        // reading it would raise an access dialog.
        assert_eq!(
            get_generic_password(service, &account).unwrap(),
            big.as_bytes()
        );
        store.write("{}").unwrap();
        assert_eq!(get_generic_password(service, &account).unwrap(), b"{}");
        delete_generic_password(service, &account).unwrap();
    }

    #[test]
    fn claude_refused_refresh_asks_to_sign_in_and_writes_nothing() {
        let store = MemStore::new(Some(&claude_json("at-1", 0)));
        let http = FakeHttp::default().reply(400, r#"{"error":"invalid_grant"}"#);
        let err = claude_token(&store, &http, Want::Valid, now()).unwrap_err();
        assert!(matches!(err, FetchError::SignInAgain(_)), "{err:?}");
        assert_eq!(*store.writes.lock().unwrap(), 0);
    }

    #[test]
    fn claude_not_signed_in() {
        for doc in [None, Some(r#"{"mcpOAuth":{}}"#)] {
            let err = claude_token(
                &MemStore::new(doc),
                &FakeHttp::default(),
                Want::Valid,
                now(),
            )
            .unwrap_err();
            assert!(matches!(err, FetchError::MissingCredential(_)), "{err:?}");
        }
        let err = claude_token(
            &MemStore::new(Some("nope")),
            &FakeHttp::default(),
            Want::Valid,
            now(),
        )
        .unwrap_err();
        assert!(matches!(err, FetchError::Store(_)));
    }

    fn jwt(claims: &Value) -> String {
        let b64 = |v: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        format!(
            "{}.{}.sig",
            b64(r#"{"alg":"none"}"#),
            b64(&claims.to_string())
        )
    }

    fn codex_json(access: &str) -> String {
        json!({
            "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
            "tokens": {"id_token": "id-old", "access_token": access, "refresh_token": "rt-old", "account_id": "acct-1"},
            "last_refresh": "2026-10-01T00:00:00Z"
        })
        .to_string()
    }

    #[test]
    fn codex_reads_expiry_from_the_jwt() {
        let valid = jwt(&json!({"exp": now().as_second() + 3600}));
        let store = MemStore::new(Some(&codex_json(&valid)));
        let t = codex_token(&store, &FakeHttp::default(), Want::Valid, now()).unwrap();
        assert_eq!(t.access, valid);
        assert_eq!(t.account_id.as_deref(), Some("acct-1"));
    }

    #[test]
    fn codex_refreshes_and_writes_back() {
        let expired = jwt(&json!({"exp": now().as_second() - 10}));
        let store = MemStore::new(Some(&codex_json(&expired)));
        let http = FakeHttp::default().reply(
            200,
            r#"{"access_token":"at-2","refresh_token":"rt-2","id_token":"id-2"}"#,
        );
        let t = codex_token(&store, &http, Want::Valid, now()).unwrap();
        assert_eq!(t.access, "at-2");
        let saved = store.json();
        assert_eq!(saved["tokens"]["access_token"], "at-2");
        assert_eq!(saved["tokens"]["refresh_token"], "rt-2");
        assert_eq!(saved["tokens"]["id_token"], "id-2");
        assert_eq!(saved["tokens"]["account_id"], "acct-1");
        assert_eq!(saved["last_refresh"], NOW);
        assert_eq!(saved["auth_mode"], "chatgpt");
        let sent = http.sent.lock().unwrap();
        let Body::Json(body) = &sent[0].body else {
            panic!("json body")
        };
        assert_eq!(body["client_id"], CODEX_CLIENT_ID);
        assert_eq!(body["refresh_token"], "rt-old");
    }

    #[test]
    fn a_rejected_token_is_refreshed_even_if_not_expired() {
        let valid = jwt(&json!({"exp": now().as_second() + 3600}));
        let store = MemStore::new(Some(&codex_json(&valid)));
        let http = FakeHttp::default().reply(200, r#"{"access_token":"at-2"}"#);
        assert_eq!(
            codex_token(&store, &http, Want::Fresh, now())
                .unwrap()
                .access,
            "at-2"
        );
    }

    #[test]
    fn codex_account_id_falls_back_to_the_jwt_claim() {
        let access = jwt(
            &json!({"exp": now().as_second() + 3600, "https://api.openai.com/auth": {"chatgpt_account_id": "acct-jwt"}}),
        );
        let doc = json!({"tokens": {"access_token": access}}).to_string();
        let t = codex_token(
            &MemStore::new(Some(&doc)),
            &FakeHttp::default(),
            Want::Valid,
            now(),
        )
        .unwrap();
        assert_eq!(t.account_id.as_deref(), Some("acct-jwt"));
        let api_key_only = json!({"auth_mode": "apikey", "OPENAI_API_KEY": "sk"}).to_string();
        let err = codex_token(
            &MemStore::new(Some(&api_key_only)),
            &FakeHttp::default(),
            Want::Valid,
            now(),
        )
        .unwrap_err();
        assert!(matches!(err, FetchError::MissingCredential(_)));
    }

    #[cfg(unix)]
    #[test]
    fn file_store_writes_atomically_at_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore(dir.path().join("auth.json"));
        assert_eq!(store.read().unwrap(), None);
        store.write("{}").unwrap();
        assert_eq!(store.read().unwrap().as_deref(), Some("{}"));
        let mode = std::fs::metadata(&store.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp file left"
        );
    }
}
