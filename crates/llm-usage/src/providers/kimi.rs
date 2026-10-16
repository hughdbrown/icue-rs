//! Kimi Code membership: `GET <base>/usages` with a Kimi Code API key (what kimi-cli's `/usage`
//! calls). Two shapes are in the wild:
//!
//! ```json
//! {"usage": {"limit": "2048", "used": "214", "remaining": "1834", "resetTime": "2026-01-09T15:23:13.7168393Z"},
//!  "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
//!              "detail": {"limit": "200", "used": "139", "remaining": "61", "resetTime": "…"}}]}
//! ```
//! (`usage` is the weekly quota), and
//! ```json
//! {"usages": {"limit_5h": {"used_ratio": 0.625, "reset_time": "…"},
//!             "limit_7d": {"used_ratio": 0.125, "reset_time": "…"},
//!             "limit_month_total": {"used_ratio": 0.0056, "reset_time": "…"}},
//!  "user": {"membership": {"level": "LEVEL_BASIC"}}}
//! ```

use serde_json::Value;

use super::{FIVE_HOURS, WEEK, json_body, label_for, num, ts};
use crate::error::FetchError;
use crate::http::{Request, Response};
use crate::model::{Usage, Window};

const SIGN_IN: &str = "check `api_key` (create one at kimi.com/code/console)";

/// The request for `api_key` against `base_url`.
#[must_use]
pub fn request(base_url: &str, api_key: &str) -> Request {
    Request::get(format!("{base_url}/usages"))
        .bearer(api_key)
        .header("Accept", "application/json")
}

/// Usage from a response.
///
/// # Errors
///
/// Error status or unknown shape.
pub fn finish(base_url: &str, r: &Response) -> Result<Usage, FetchError> {
    parse(&json_body(base_url, r, SIGN_IN)?)
}

/// Fraction used of a `{limit, used, remaining}` object.
fn used_fraction(d: &Value) -> Option<f64> {
    let limit = num(d.get("limit")).filter(|&l| l > 0.0)?;
    let used = num(d.get("used")).or_else(|| num(d.get("remaining")).map(|r| limit - r))?;
    Some(used / limit)
}

fn window_secs(w: &Value) -> Option<i64> {
    #[expect(clippy::cast_possible_truncation, reason = "window lengths are small")]
    let n = num(w.get("duration"))? as i64;
    let unit = match w.get("timeUnit").and_then(Value::as_str)? {
        "TIME_UNIT_SECOND" => 1,
        "TIME_UNIT_MINUTE" => 60,
        "TIME_UNIT_HOUR" => 3600,
        "TIME_UNIT_DAY" => 86400,
        _ => return None,
    };
    n.checked_mul(unit)
}

fn plan(v: &Value) -> Option<String> {
    let level = v.get("user")?.get("membership")?.get("level")?.as_str()?;
    let name = match level {
        "LEVEL_FREE" => "Adagio",
        "LEVEL_TRIAL" => "Andante",
        "LEVEL_BASIC" => "Moderato",
        "LEVEL_INTERMEDIATE" => "Allegretto",
        "LEVEL_ADVANCED" => "Allegro",
        other => other.strip_prefix("LEVEL_").unwrap_or(other),
    };
    Some(name.to_owned())
}

/// Usage from the JSON body.
///
/// # Errors
///
/// No known window.
pub fn parse(v: &Value) -> Result<Usage, FetchError> {
    let mut windows = Vec::new();
    if let Some(u) = v.get("usages").filter(|u| u.is_object()) {
        for (key, label, len) in [
            ("limit_5h", "5 h", Some(FIVE_HOURS)),
            ("limit_7d", "Week", Some(WEEK)),
            ("limit_month_total", "Month", None),
        ] {
            if let Some(w) = u.get(key).filter(|w| w.is_object()) {
                let used = num(w.get("used_ratio")).unwrap_or(0.0);
                windows.push(Window::new(label, used, ts(w.get("reset_time")), len));
            }
        }
    } else {
        for l in v
            .get("limits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(d), len) = (l.get("detail"), l.get("window").and_then(window_secs)) else {
                continue;
            };
            if let Some(used) = used_fraction(d) {
                let label = len.map_or_else(|| "Window".into(), label_for);
                windows.push(Window::new(label, used, ts(d.get("resetTime")), len));
            }
        }
        if let Some((u, used)) = v.get("usage").and_then(|u| Some((u, used_fraction(u)?))) {
            windows.push(Window::new(
                "Week",
                used,
                ts(u.get("resetTime")),
                Some(WEEK),
            ));
        }
    }
    if windows.is_empty() {
        return Err(FetchError::parse("api.kimi.com", "no usage windows"));
    }
    windows.sort_by_key(|w| w.length_secs.unwrap_or(i64::MAX));
    Ok(Usage {
        plan: plan(v),
        detail: None,
        windows,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_the_older_shape() {
        let v = json!({
            "usage": {"limit": "2048", "used": "214", "remaining": "1834", "resetTime": "2026-01-09T15:23:13.716839300Z"},
            "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                        "detail": {"limit": "200", "used": "139", "remaining": "61", "resetTime": "2026-01-06T13:33:02.717Z"}}]
        });
        let u = parse(&v).unwrap();
        assert_eq!(u.windows.len(), 2);
        assert_eq!(u.windows[0].label, "5 h");
        assert!((u.windows[0].used - 0.695).abs() < 1e-9);
        assert_eq!(u.windows[1].label, "Week");
        assert!((u.windows[1].used - 214.0 / 2048.0).abs() < 1e-9);
    }

    #[test]
    fn parses_the_newer_shape() {
        let v = json!({
            "usages": {
                "limit_5h": {"used_ratio": 0.625, "reset_time": "2026-10-16T15:00:00Z"},
                "limit_7d": {"used_ratio": 0.125, "reset_time": "2026-10-20T00:00:00Z"},
                "limit_month_total": {"used_ratio": 0.0056, "reset_time": "2026-10-17T00:00:00Z"}
            },
            "user": {"membership": {"level": "LEVEL_BASIC"}},
            "version": "GOODS_VERSION_V1"
        });
        let u = parse(&v).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Moderato"));
        let labels: Vec<_> = u.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5 h", "Week", "Month"]);
        assert_eq!(u.windows[2].length_secs, None);
    }

    #[test]
    fn remaining_without_used_and_empty_bodies() {
        let v = json!({"limits": [{"window": {"duration": 5, "timeUnit": "TIME_UNIT_HOUR"},
                                   "detail": {"limit": 100, "remaining": 25}}]});
        assert!((parse(&v).unwrap().windows[0].used - 0.75).abs() < 1e-9);
        assert!(parse(&json!({})).is_err());
    }
}
