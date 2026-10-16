//! One fetcher and parser per service. Parsers are pure (`parse(body) → Usage`) and tested on
//! recorded response shapes; fetchers add credentials and map HTTP statuses to [`FetchError`].
//!
//! Every service here except Ollama's is private and undocumented, so parsers accept numbers or
//! numeric strings, missing fields, and both of Kimi's known shapes.

use jiff::Timestamp;
use serde_json::Value;

use crate::config::{Entry, Source};
use crate::creds::{self, Token, Want};
use crate::error::FetchError;
use crate::http::{Http, Response, host_of};
use crate::model::Usage;

pub mod claude;
pub mod codex;
pub mod gemini;
pub mod kimi;
pub mod ollama;

/// Seconds in the windows the services use.
pub(crate) const FIVE_HOURS: i64 = 5 * 3600;
pub(crate) const WEEK: i64 = 7 * 86400;

/// Fetch one subscription's usage.
///
/// # Errors
///
/// Missing or refused credentials, network failure, or an unexpected response.
pub fn fetch(entry: &Entry, http: &dyn Http, now: Timestamp) -> Result<Usage, FetchError> {
    match &entry.source {
        Source::ClaudeCode => {
            let store = creds::claude_code_store()?;
            let (token, r) = with_token(
                |want| creds::claude_token(&*store, http, want, now),
                |t| http.send(&claude::request(&t.access)),
            )?;
            claude::finish(&r, token.plan)
        }
        Source::ClaudeToken(t) => claude::finish(&http.send(&claude::request(t.expose()))?, None),
        Source::CodexAuthFile(path) => {
            let store = creds::FileStore(path.clone());
            let (_, r) = with_token(
                |want| creds::codex_token(&store, http, want, now),
                |t| {
                    http.send(&codex::request(
                        &t.access,
                        t.account_id.as_deref().unwrap_or_default(),
                    ))
                },
            )?;
            codex::finish(&r)
        }
        Source::CodexToken {
            access_token,
            account_id,
        } => codex::finish(&http.send(&codex::request(access_token.expose(), account_id))?),
        Source::GeminiCookie(cookie) => {
            gemini::finish(&http.send(&gemini::request(cookie.expose()))?)
        }
        Source::Kimi { api_key, base_url } => {
            kimi::finish(&http.send(&kimi::request(base_url, api_key.expose()))?)
        }
        Source::Ollama(key) => ollama::finish(&http.send(&ollama::request(key.expose()))?),
    }
}

/// Send with the stored token; on 401, once more with a refreshed one.
fn with_token(
    token: impl Fn(Want) -> Result<Token, FetchError>,
    send: impl Fn(&Token) -> Result<Response, crate::http::NetworkError>,
) -> Result<(Token, Response), FetchError> {
    let t = token(Want::Valid)?;
    let r = send(&t)?;
    if r.status != 401 {
        return Ok((t, r));
    }
    let t = token(Want::Fresh)?;
    let r = send(&t)?;
    Ok((t, r))
}

/// The JSON body of a 2xx `r` from `url`, or the error its status means.
pub(crate) fn json_body(url: &str, r: &Response, sign_in: &str) -> Result<Value, FetchError> {
    let host = host_of(url);
    if !(200..300).contains(&r.status) {
        return Err(FetchError::from_status(host, r, sign_in));
    }
    serde_json::from_str(&r.body).map_err(|e| FetchError::parse(host, format!("not JSON ({e})")))
}

/// A number, or a string holding one.
pub(crate) fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f: &f64| f.is_finite())
}

/// An RFC 3339 string, or epoch seconds (milliseconds when that large).
pub(crate) fn ts(v: Option<&Value>) -> Option<Timestamp> {
    match v? {
        Value::String(s) => s.trim().parse().ok(),
        n @ Value::Number(_) => {
            let x = num(Some(n))?;
            #[expect(clippy::cast_possible_truncation, reason = "epoch values fit i64")]
            if x > 1e12 {
                Timestamp::from_millisecond(x as i64).ok()
            } else {
                Timestamp::from_second(x as i64).ok()
            }
        }
        _ => None,
    }
}

/// Short label for a window of `secs`: `5 h`, `Day`, `Week`, `3 d`.
pub(crate) fn label_for(secs: i64) -> String {
    match secs {
        FIVE_HOURS => "5 h".into(),
        86400 => "Day".into(),
        WEEK => "Week".into(),
        s if s % 86400 == 0 => format!("{} d", s / 86400),
        s => format!("{} h", (s + 1800) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn lenient_numbers_and_times() {
        assert_eq!(num(Some(&json!("12.5"))), Some(12.5));
        assert_eq!(num(Some(&json!(3))), Some(3.0));
        assert_eq!(num(Some(&json!("x"))), None);
        assert_eq!(num(Some(&json!(null))), None);
        let t: Timestamp = "2026-01-09T15:23:13Z".parse().unwrap();
        assert_eq!(ts(Some(&json!(1_767_972_193))), Some(t));
        assert_eq!(ts(Some(&json!(1_767_972_193_000_i64))), Some(t));
        assert_eq!(
            ts(Some(&json!("2026-01-09T15:23:13.716839300Z"))).map(Timestamp::as_second),
            Some(t.as_second())
        );
        assert_eq!(ts(Some(&json!("soon"))), None);
    }

    #[test]
    fn codex_retries_once_with_a_refreshed_token_after_a_401() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let doc =
            json!({"tokens": {"access_token": "old", "refresh_token": "rt", "account_id": "acct"}});
        std::fs::write(&path, doc.to_string()).unwrap();
        let usage = json!({"plan_type": "pro", "rate_limit": {"primary_window":
            {"used_percent": 10, "reset_at": 1_767_000_000, "limit_window_seconds": 18000}}});
        let http = crate::http::fake::FakeHttp::default()
            .reply(401, "")
            .reply(200, r#"{"access_token":"new","refresh_token":"rt2"}"#)
            .reply(200, &usage.to_string());
        let entry = Entry {
            id: "codex".into(),
            label: "Codex".into(),
            source: Source::CodexAuthFile(path.clone()),
        };
        let u = fetch(&entry, &http, "2026-10-16T12:00:00Z".parse().unwrap()).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Pro"));
        assert_eq!(
            http.urls(),
            [
                codex::URL,
                "https://auth.openai.com/oauth/token",
                codex::URL
            ]
        );
        assert_eq!(
            http.header(2, "authorization").as_deref(),
            Some("Bearer new")
        );
        assert_eq!(
            http.header(2, "chatgpt-account-id").as_deref(),
            Some("acct")
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["tokens"]["refresh_token"], "rt2");
    }

    #[test]
    fn a_fixed_token_is_not_retried() {
        let http = crate::http::fake::FakeHttp::default().reply(401, "");
        let entry = Entry {
            id: "c".into(),
            label: "C".into(),
            source: Source::ClaudeToken(crate::config::Secret::new("t")),
        };
        let err = fetch(&entry, &http, Timestamp::now()).unwrap_err();
        assert!(matches!(err, FetchError::SignInAgain(_)));
        assert_eq!(http.urls().len(), 1);
    }

    #[test]
    fn window_labels() {
        assert_eq!(label_for(18000), "5 h");
        assert_eq!(label_for(604_800), "Week");
        assert_eq!(label_for(3 * 86400), "3 d");
        assert_eq!(label_for(7200), "2 h");
    }
}
