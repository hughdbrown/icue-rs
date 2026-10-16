//! Claude Pro/Max: `GET https://api.anthropic.com/api/oauth/usage` with Claude Code's OAuth token.
//!
//! ```json
//! {"five_hour": {"utilization": 12.5, "resets_at": "2025-12-25T12:00:00.000Z"},
//!  "seven_day": {"utilization": 30, "resets_at": "2025-12-31T00:00:00.000Z"},
//!  "seven_day_opus": null, "extra_usage": {…}}
//! ```
//! `utilization` is a percentage. Model-scoped weekly limits (`seven_day_opus`, `limits[]`) are
//! left out: they cap one model, not the subscription.

use serde_json::Value;

use super::{FIVE_HOURS, WEEK, json_body, num, ts};
use crate::error::FetchError;
use crate::http::{Request, Response};
use crate::model::{Usage, Window};

/// The usage endpoint.
pub const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const SIGN_IN: &str = "run `claude` and sign in again (/login)";

/// The request for `token`.
#[must_use]
pub fn request(token: &str) -> Request {
    Request::get(URL)
        .bearer(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Accept", "application/json")
        .header("User-Agent", "claude-code/2.1.0")
}

/// Usage from a response.
///
/// # Errors
///
/// Error status or unknown shape.
pub fn finish(r: &Response, plan: Option<String>) -> Result<Usage, FetchError> {
    let mut u = parse(&json_body(URL, r, SIGN_IN)?)?;
    u.plan = plan;
    Ok(u)
}

/// Usage from the JSON body.
///
/// # Errors
///
/// Neither window is present.
pub fn parse(v: &Value) -> Result<Usage, FetchError> {
    let windows: Vec<Window> = [
        ("five_hour", "5 h", FIVE_HOURS),
        ("seven_day", "Week", WEEK),
    ]
    .into_iter()
    .filter_map(|(key, label, len)| {
        let o = v.get(key).filter(|o| o.is_object())?;
        let pct = num(o.get("utilization")).unwrap_or(0.0);
        Some(Window::new(
            label,
            pct / 100.0,
            ts(o.get("resets_at")),
            Some(len),
        ))
    })
    .collect();
    if windows.is_empty() {
        return Err(FetchError::parse(
            "api.anthropic.com",
            "no five_hour or seven_day window",
        ));
    }
    Ok(Usage {
        windows,
        ..Usage::default()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_both_windows() {
        let v = json!({
            "five_hour": {"utilization": 12.5, "resets_at": "2025-12-25T12:00:00.000Z"},
            "seven_day": {"utilization": 30, "resets_at": "2025-12-31T00:00:00.000Z"},
            "seven_day_sonnet": {"utilization": 5},
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": true, "monthly_limit": 2000, "used_credits": 500, "utilization": 25}
        });
        let u = parse(&v).unwrap();
        assert_eq!(u.windows.len(), 2);
        assert_eq!(u.windows[0].label, "5 h");
        assert!((u.windows[0].used - 0.125).abs() < 1e-9);
        assert_eq!(
            u.windows[0].resets_at.unwrap().to_string(),
            "2025-12-25T12:00:00Z"
        );
        assert_eq!(u.windows[1].length_secs, Some(WEEK));
        assert!((u.windows[1].used - 0.3).abs() < 1e-9);
    }

    #[test]
    fn idle_window_has_no_reset_and_unknown_shapes_fail() {
        let u =
            parse(&json!({"five_hour": {"utilization": 0, "resets_at": null}, "seven_day": null}))
                .unwrap();
        assert_eq!(u.windows.len(), 1);
        assert_eq!(u.windows[0].resets_at, None);
        assert!(parse(&json!({"error": "x"})).is_err());
    }

    #[test]
    fn request_carries_the_beta_header() {
        let r = request("tok");
        assert!(r.headers.contains(&("Authorization", "Bearer tok".into())));
        assert!(
            r.headers
                .contains(&("anthropic-beta", "oauth-2025-04-20".into()))
        );
    }

    #[test]
    fn rejected_token_asks_to_sign_in() {
        let r = Response {
            status: 401,
            body: String::new(),
            retry_after: None,
        };
        assert!(matches!(finish(&r, None), Err(FetchError::SignInAgain(_))));
    }
}
