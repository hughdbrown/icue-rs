//! Ollama Cloud: `GET https://ollama.com/api/balance` (documented at docs.ollama.com/api/balance).
//!
//! Credit plans:
//! ```json
//! {"included": {"balance_usd": 72.5, "allowance_usd": 100,
//!               "period": {"from": "2026-09-15T09:30:00Z", "until": "2026-10-15T09:30:00Z"}},
//!  "purchased": {"balance_usd": 25}}
//! ```
//! Legacy plans keep percentage windows:
//! ```json
//! {"included": {"session": {"remaining_percent": 75, "resets_at": "…"},
//!               "weekly": {"remaining_percent": 40, "resets_at": "…"}}}
//! ```

use serde_json::Value;

use super::{FIVE_HOURS, WEEK, json_body, num, ts};
use crate::error::FetchError;
use crate::http::{Request, Response};
use crate::model::{Amount, Usage, Window};

/// The balance endpoint.
pub const URL: &str = "https://ollama.com/api/balance";
const SIGN_IN: &str = "check `api_key` (create one at ollama.com/settings/keys)";

/// The request for `api_key`.
#[must_use]
pub fn request(api_key: &str) -> Request {
    Request::get(URL)
        .bearer(api_key)
        .header("Accept", "application/json")
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
/// Neither shape is present.
pub fn parse(v: &Value) -> Result<Usage, FetchError> {
    let inc = v
        .get("included")
        .ok_or_else(|| FetchError::parse("ollama.com", "no `included` balance"))?;
    let mut windows = Vec::new();
    if let (Some(balance), Some(allowance)) = (
        num(inc.get("balance_usd")),
        num(inc.get("allowance_usd")).filter(|&a| a > 0.0),
    ) {
        let period = inc.get("period");
        let from = ts(period.and_then(|p| p.get("from")));
        let until = ts(period.and_then(|p| p.get("until")));
        let len = from
            .zip(until)
            .map(|(f, u)| u.as_second() - f.as_second())
            .filter(|&s| s > 0);
        let used = (allowance - balance).max(0.0);
        let mut w = Window::new("Month", used / allowance, until, len);
        w.amount = Some(Amount {
            used,
            limit: allowance,
            unit: "USD".into(),
        });
        windows.push(w);
    } else {
        for (key, label, len) in [("session", "5 h", FIVE_HOURS), ("weekly", "Week", WEEK)] {
            if let Some((w, left)) = inc
                .get(key)
                .and_then(|w| Some((w, num(w.get("remaining_percent"))?)))
            {
                windows.push(Window::new(
                    label,
                    1.0 - left / 100.0,
                    ts(w.get("resets_at")),
                    Some(len),
                ));
            }
        }
    }
    if windows.is_empty() {
        return Err(FetchError::parse(
            "ollama.com",
            "no allowance or usage windows",
        ));
    }
    let detail = num(v.get("purchased").and_then(|p| p.get("balance_usd")))
        .filter(|&b| b > 0.0)
        .map(|b| format!("+${b:.2} purchased"));
    Ok(Usage {
        plan: None,
        detail,
        windows,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_a_credit_plan() {
        let v = json!({
            "included": {"balance_usd": 72.5, "allowance_usd": 100,
                         "period": {"from": "2026-09-15T09:30:00Z", "until": "2026-10-15T09:30:00Z"}},
            "purchased": {"balance_usd": 25}
        });
        let u = parse(&v).unwrap();
        let w = &u.windows[0];
        assert_eq!(w.label, "Month");
        assert!((w.used - 0.275).abs() < 1e-9);
        assert_eq!(w.length_secs, Some(30 * 86400));
        assert_eq!(w.amount.as_ref().unwrap().limit, 100.0);
        assert_eq!(u.detail.as_deref(), Some("+$25.00 purchased"));
    }

    #[test]
    fn parses_a_legacy_plan_and_decimal_strings() {
        let v = json!({"included": {
            "session": {"remaining_percent": "75", "resets_at": "2026-10-01T07:00:00Z"},
            "weekly": {"remaining_percent": 40, "resets_at": "2026-10-05T00:00:00Z"}}});
        let u = parse(&v).unwrap();
        assert!((u.windows[0].used - 0.25).abs() < 1e-9);
        assert!((u.windows[1].used - 0.6).abs() < 1e-9);
        assert_eq!(u.detail, None);
        assert!(parse(&json!({"included": {}})).is_err());
        assert!(parse(&json!({})).is_err());
    }
}
