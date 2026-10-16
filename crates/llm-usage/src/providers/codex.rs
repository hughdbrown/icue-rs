//! Codex with a `ChatGPT` plan: `GET https://chatgpt.com/backend-api/wham/usage`.
//!
//! ```json
//! {"plan_type": "plus",
//!  "rate_limit": {"primary_window": {"used_percent": 22, "reset_at": 1766948068, "limit_window_seconds": 18000},
//!                 "secondary_window": {"used_percent": 43, "reset_at": 1767407914, "limit_window_seconds": 604800}},
//!  "credits": {"has_credits": false, "unlimited": false, "balance": "0"}}
//! ```

use serde_json::Value;

use super::{json_body, label_for, num, ts};
use crate::creds::capitalise;
use crate::error::FetchError;
use crate::http::{Request, Response};
use crate::model::{Usage, Window};

/// The usage endpoint.
pub const URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const SIGN_IN: &str = "run `codex login` again";

/// The request for `token` and `account_id`.
#[must_use]
pub fn request(token: &str, account_id: &str) -> Request {
    Request::get(URL)
        .bearer(token)
        .header("ChatGPT-Account-Id", account_id)
        .header("Accept", "application/json")
        .header("User-Agent", "codex-cli")
}

/// Usage from a response.
///
/// # Errors
///
/// Error status or unknown shape.
pub fn finish(r: &Response) -> Result<Usage, FetchError> {
    parse(&json_body(URL, r, SIGN_IN)?)
}

/// Usage from the JSON body.
///
/// # Errors
///
/// No rate-limit window.
pub fn parse(v: &Value) -> Result<Usage, FetchError> {
    let limits = v.get("rate_limit");
    let mut windows: Vec<Window> = ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|key| {
            let w = limits?.get(key).filter(|w| w.is_object())?;
            #[expect(clippy::cast_possible_truncation, reason = "window lengths are small")]
            let len = num(w.get("limit_window_seconds")).map(|s| s as i64);
            let label = len.map_or_else(|| "Window".into(), label_for);
            let used = num(w.get("used_percent")).unwrap_or(0.0) / 100.0;
            Some(Window::new(label, used, ts(w.get("reset_at")), len))
        })
        .collect();
    if windows.is_empty() {
        return Err(FetchError::parse("chatgpt.com", "no rate_limit windows"));
    }
    windows.sort_by_key(|w| w.length_secs.unwrap_or(i64::MAX));
    let credits = v.get("credits");
    let detail = credits
        .filter(|c| c.get("has_credits").and_then(Value::as_bool) == Some(true))
        .and_then(|c| num(c.get("balance")))
        .filter(|&b| b > 0.0)
        .map(|b| format!("{b:.0} credits"));
    Ok(Usage {
        plan: v.get("plan_type").and_then(Value::as_str).map(capitalise),
        detail,
        windows,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_windows_plan_and_credits() {
        let v = json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "secondary_window": {"used_percent": 43, "reset_at": 1_767_407_914, "limit_window_seconds": 604_800},
                "primary_window": {"used_percent": 22, "reset_at": 1_766_948_068, "limit_window_seconds": 18000}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": "120"}
        });
        let u = parse(&v).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Plus"));
        assert_eq!(u.detail.as_deref(), Some("120 credits"));
        assert_eq!(u.windows[0].label, "5 h", "shortest first");
        assert!((u.windows[0].used - 0.22).abs() < 1e-9);
        assert_eq!(u.windows[0].resets_at.unwrap().as_second(), 1_766_948_068);
        assert_eq!(u.windows[1].label, "Week");
    }

    #[test]
    fn missing_windows_fail() {
        assert!(parse(&json!({"plan_type": "free", "rate_limit": null})).is_err());
    }

    #[test]
    fn request_names_the_account() {
        let r = request("tok", "acct");
        assert!(r.headers.contains(&("ChatGPT-Account-Id", "acct".into())));
    }
}
