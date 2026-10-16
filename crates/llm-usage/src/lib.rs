//! How much of each LLM subscription's quota is left, when it resets, and which to use next.
//!
//! - [`config`]: the 0600 `llm-usage.toml` listing the subscriptions and their credentials.
//! - [`creds`]: reuse (and refresh) the logins of Claude Code and the Codex CLI.
//! - [`providers`]: one fetcher and parser per service.
//! - [`pace`]: pace against straight-line use, run-out projection, the "use next" suggestion.
//! - [`Service`]: fetches every subscription at most once per poll interval, keeps the last good
//!   numbers when a fetch fails, and honours `Retry-After`. EdgeHost's `LlmUsage` plugin and
//!   `edgectl llm-usage` both use it.

pub mod config;
pub mod creds;
pub mod error;
pub mod http;
pub mod model;
pub mod pace;
pub mod providers;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};

pub use crate::config::{Config, ConfigError};
pub use crate::error::FetchError;
pub use crate::model::{Snapshot, Status, Subscription, Usage, Window};

use crate::http::{Http, UreqHttp};

/// After a forced refresh, further forced refreshes within this time reuse its result.
const MIN_FORCED_GAP: Duration = Duration::from_secs(15);

/// Longest back-off honoured, whatever `Retry-After` says.
const MAX_BACKOFF: Duration = Duration::from_hours(6);

/// Back-off after a 429 that named no `Retry-After`.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(600);

#[derive(Default)]
struct State {
    /// The last snapshot and when it was built.
    last: Option<(Instant, Snapshot)>,
    /// Per subscription id: the last good fetch and its time.
    good: HashMap<String, (Usage, Timestamp)>,
    /// Per subscription id: no requests before this time.
    backoff: HashMap<String, Timestamp>,
}

/// Fetches and caches snapshots. One per process is enough: see [`Service::shared`].
pub struct Service {
    http: Box<dyn Http>,
    path: Option<PathBuf>,
    state: Mutex<State>,
    /// Held while fetching, so concurrent callers wait for one fetch instead of starting more.
    fetching: Mutex<()>,
}

/// Whether to fetch even though the last snapshot is younger than the poll interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Freshness {
    /// Reuse a snapshot younger than the poll interval.
    Cached,
    /// Fetch now (unless one was just forced).
    Forced,
}

impl Service {
    /// A service reading `path` (none: no `$HOME`) and fetching through `http`.
    #[must_use]
    pub fn new(path: Option<PathBuf>, http: Box<dyn Http>) -> Self {
        Self {
            http,
            path,
            state: Mutex::default(),
            fetching: Mutex::new(()),
        }
    }

    /// The process-wide service at [`config::default_path`], so several widgets share one set of
    /// requests.
    #[must_use]
    pub fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<Service>> = OnceLock::new();
        Arc::clone(SHARED.get_or_init(|| {
            Arc::new(Self::new(
                config::default_path(),
                Box::new(UreqHttp::default()),
            ))
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The last snapshot, without fetching.
    #[must_use]
    pub fn cached(&self) -> Option<Snapshot> {
        self.lock().last.as_ref().map(|(_, s)| s.clone())
    }

    /// The poll interval of the last snapshot (the default before the first).
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(
            self.lock()
                .last
                .as_ref()
                .map_or(config::DEFAULT_POLL_SECS, |(_, s)| s.poll_secs),
        )
    }

    /// The last snapshot if it is younger than the poll interval, else a new one.
    pub fn current(&self) -> Snapshot {
        self.get(Freshness::Cached)
    }

    /// A new snapshot (or one forced less than 15 s ago). Services in back-off are not asked.
    pub fn refresh(&self) -> Snapshot {
        self.get(Freshness::Forced)
    }

    fn reusable(&self, freshness: Freshness) -> Option<Snapshot> {
        let state = self.lock();
        let (at, snap) = state.last.as_ref()?;
        let max_age = match freshness {
            // A little early, so a poll timer firing on the interval finds the snapshot stale.
            Freshness::Cached => Duration::from_secs(snap.poll_secs.saturating_sub(5)),
            Freshness::Forced => MIN_FORCED_GAP,
        };
        (at.elapsed() < max_age).then(|| snap.clone())
    }

    fn get(&self, freshness: Freshness) -> Snapshot {
        if let Some(s) = self.reusable(freshness) {
            return s;
        }
        let _fetching = self.fetching.lock().unwrap_or_else(PoisonError::into_inner);
        // Another caller may have fetched while this one waited.
        if let Some(s) = self.reusable(Freshness::Forced) {
            return s;
        }
        let snap = self.fetch(Timestamp::now());
        self.lock().last = Some((Instant::now(), snap.clone()));
        snap
    }

    fn fetch(&self, now: Timestamp) -> Snapshot {
        let path_text = self
            .path
            .as_ref()
            .map_or_else(|| "(no $HOME)".to_owned(), |p| p.display().to_string());
        let loaded = self.path.as_deref().map_or_else(
            || Err("cannot locate the EdgeHost data folder: $HOME is not set".to_owned()),
            |p| Config::load(p).map_err(|e| e.to_string()),
        );
        let config = match loaded {
            Ok(c) => c,
            Err(e) => {
                return Snapshot {
                    generated_at: now,
                    config_path: path_text,
                    config_error: Some(e),
                    poll_secs: config::DEFAULT_POLL_SECS,
                    subscriptions: Vec::new(),
                    suggestion: None,
                };
            }
        };
        let waiting: HashMap<String, Timestamp> = {
            let state = self.lock();
            state
                .backoff
                .iter()
                .filter(|(_, until)| **until > now)
                .map(|(id, until)| (id.clone(), *until))
                .collect()
        };
        let results: Vec<Result<Usage, FetchError>> = std::thread::scope(|s| {
            let handles: Vec<_> = config
                .entries
                .iter()
                .map(|entry| {
                    let until = waiting.get(&entry.id).copied();
                    s.spawn(move || match until {
                        Some(until) => Err(FetchError::RateLimited {
                            retry_after: Some(until.duration_since(now).unsigned_abs()),
                        }),
                        None => providers::fetch(entry, &*self.http, now),
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(FetchError::Store("the fetcher panicked".into())))
                })
                .collect()
        });
        let mut state = self.lock();
        let mut subscriptions: Vec<Subscription> = config
            .entries
            .iter()
            .zip(results)
            .map(|(entry, result)| record(&mut state, entry, result, &waiting, now))
            .collect();
        drop(state);
        let suggestion = pace::annotate(&mut subscriptions, now);
        Snapshot {
            generated_at: now,
            config_path: path_text,
            config_error: None,
            poll_secs: config.poll_secs,
            subscriptions,
            suggestion,
        }
    }
}

/// Turn one fetch result into a subscription, remembering good numbers and back-off in `state`.
fn record(
    state: &mut State,
    entry: &config::Entry,
    result: Result<Usage, FetchError>,
    waiting: &HashMap<String, Timestamp>,
    now: Timestamp,
) -> Subscription {
    let mut sub = Subscription {
        id: entry.id.clone(),
        provider: entry.source.provider(),
        label: entry.label.clone(),
        plan: None,
        detail: None,
        status: Status::Ok,
        message: None,
        windows: Vec::new(),
        fetched_at: None,
        blocked_until: None,
        urgency: None,
    };
    match result {
        Ok(usage) => {
            state.backoff.remove(&entry.id);
            fill(&mut sub, &usage, now);
            state.good.insert(entry.id.clone(), (usage, now));
        }
        Err(e) => {
            if let FetchError::RateLimited { retry_after } = &e
                && !waiting.contains_key(&entry.id)
            {
                let wait = retry_after.unwrap_or(DEFAULT_BACKOFF).min(MAX_BACKOFF);
                let until = SignedDuration::try_from(wait)
                    .ok()
                    .and_then(|d| now.checked_add(d).ok())
                    .unwrap_or(now);
                state.backoff.insert(entry.id.clone(), until);
            }
            if let Some((usage, at)) = state.good.get(&entry.id) {
                fill(&mut sub, usage, *at);
                sub.windows = pace::age(&sub.windows, now);
                sub.status = Status::Stale;
            } else {
                sub.status = Status::Error;
            }
            sub.message = Some(e.to_string());
        }
    }
    sub
}

fn fill(sub: &mut Subscription, usage: &Usage, at: Timestamp) {
    sub.plan.clone_from(&usage.plan);
    sub.detail.clone_from(&usage.detail);
    sub.windows.clone_from(&usage.windows);
    sub.fetched_at = Some(at);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::http::fake::FakeHttp;

    fn service(dir: &Path, toml: &str, http: FakeHttp) -> Service {
        let path = dir.join(config::FILE_NAME);
        std::fs::write(&path, toml).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        Service::new(Some(path), Box::new(http))
    }

    const OLLAMA_AND_KIMI: &str = r#"
        [[subscription]]
        provider = "ollama"
        api_key = "o"
        [[subscription]]
        provider = "kimi"
        api_key = "k"
    "#;

    const BALANCE: &str = r#"{"included":{"balance_usd":10,"allowance_usd":100,"period":{"from":"2026-10-01T00:00:00Z","until":"2099-10-31T00:00:00Z"}}}"#;

    #[test]
    fn missing_config_is_reported_in_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let s = Service::new(
            Some(dir.path().join("none.toml")),
            Box::new(FakeHttp::default()),
        );
        let snap = s.current();
        assert!(
            snap.config_error
                .unwrap()
                .contains("edgectl llm-usage init")
        );
        assert_eq!(snap.subscriptions, []);
    }

    #[test]
    fn fetches_every_subscription_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let http =
            FakeHttp::default()
                .on("ollama.com", 200, BALANCE)
                .on("api.kimi.com", 500, "boom");
        let s = service(dir.path(), OLLAMA_AND_KIMI, http);
        let snap = s.current();
        let [ollama, kimi] = snap.subscriptions.as_slice() else {
            panic!("two subscriptions")
        };
        assert_eq!(ollama.status, Status::Ok);
        assert_eq!(kimi.status, Status::Error);
        assert_eq!(
            kimi.message.as_deref(),
            Some("HTTP 500 from api.kimi.com: boom")
        );
        assert_eq!(
            snap.suggestion.as_ref().map(|x| x.id.as_str()),
            Some("ollama")
        );
        // Cached: no new requests within the poll interval.
        assert_eq!(s.current(), snap);
        assert_eq!(s.cached(), Some(snap));
    }

    #[test]
    fn failures_keep_the_last_good_numbers_and_rate_limits_back_off() {
        let dir = tempfile::tempdir().unwrap();
        let toml = "[[subscription]]\nprovider = \"ollama\"\napi_key = \"o\"\n";
        let http = FakeHttp::default()
            .reply(200, BALANCE)
            .reply(429, "slow down");
        let s = service(dir.path(), toml, http);
        let now = Timestamp::now();
        let first = s.fetch(now);
        assert_eq!(first.subscriptions[0].status, Status::Ok);
        assert_eq!(
            first.suggestion.as_ref().map(|x| x.id.as_str()),
            Some("ollama")
        );

        let second = s.fetch(now);
        let sub = &second.subscriptions[0];
        assert_eq!(sub.status, Status::Stale);
        assert!(sub.message.as_deref().unwrap().starts_with("rate limited"));
        assert!(
            (sub.windows[0].used - 0.9).abs() < 1e-9,
            "last good numbers kept"
        );

        // In back-off: no request is made (the fake has no replies left and would fail
        // differently).
        let third = s.fetch(now);
        assert!(
            third.subscriptions[0]
                .message
                .as_deref()
                .unwrap()
                .contains("retrying in")
        );
    }
}
