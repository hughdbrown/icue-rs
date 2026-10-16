//! Gemini app (consumer AI Pro/Ultra): the usage page's GraphQL query, authenticated by the
//! browser's cookie. Gemini CLI's own quota API stopped serving these accounts on 2026-06-18.
//!
//! The response may start with `)]}'` and is an array whose element with `data.viewer` holds
//! `rpcs.getUsageInfo.pools[]`: `{poolType: "GXU_FIVE_HOURLY" | "GXU_WEEKLY", percentageUsed
//! (fraction), gxu: {windowRefreshesAt}}`. `viewer: null` means the cookie is signed out.

use serde_json::Value;

use super::{FIVE_HOURS, WEEK, num, ts};
use crate::error::FetchError;
use crate::http::{Request, Response};
use crate::model::{Usage, Window};

const HOST: &str = "gemini.google.com";
/// Persisted `UsagePageQuery` signature from gemini.google.com's `UsagePageSurface` bundle.
const QUERY_SIGNATURE: &str = "2/dWbCvkVB0X16r9NPnxifeOAAqdCqMf0xeWvau9yPy7g=";
const VARIABLES: &str = r#"{"g1Request":{"entrypoints":["GEMINI_USAGE_SETTINGS_UPGRADE_BANNER"]},"workspaceRequest":{"recommendationsForSurfaceRequests":[]}}"#;
const SIGN_IN: &str =
    "sign in at gemini.google.com/usage and paste a fresh Cookie header into the config";

/// `application/x-www-form-urlencoded`-safe percent encoding of a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            b => format!("%{b:02X}"),
        })
        .collect()
}

/// The query URL.
#[must_use]
pub fn url() -> String {
    format!(
        "https://{HOST}/_/BardChatUi/graphql/schemas/GEMINI_WEB_GRAPHQL/executeQuery?query_signature={}&variables={}",
        encode(QUERY_SIGNATURE),
        encode(VARIABLES)
    )
}

/// The request with the browser's `Cookie` header value.
#[must_use]
pub fn request(cookie: &str) -> Request {
    Request::get(url())
        .header("Cookie", cookie.trim().trim_start_matches("Cookie:").trim())
        .header("Accept", "application/json")
        .header("Referer", "https://gemini.google.com/usage")
}

/// Usage from a response.
///
/// # Errors
///
/// Error status, a signed-out cookie, or an unknown shape.
pub fn finish(r: &Response) -> Result<Usage, FetchError> {
    if !(200..300).contains(&r.status) {
        return Err(FetchError::from_status(HOST, r, SIGN_IN));
    }
    let body = r.body.trim_start();
    if body.starts_with('<') {
        // A sign-in page instead of JSON.
        return Err(FetchError::SignInAgain(format!(
            "{HOST} did not accept the cookie; {SIGN_IN}"
        )));
    }
    let body = body.strip_prefix(")]}'").unwrap_or(body);
    let v: Value = serde_json::from_str(body)
        .map_err(|e| FetchError::parse(HOST, format!("not JSON ({e})")))?;
    parse(&v)
}

/// Usage from the JSON body.
///
/// # Errors
///
/// Signed out, or no usage pools.
pub fn parse(v: &Value) -> Result<Usage, FetchError> {
    let results = match v {
        Value::Array(a) => a.as_slice(),
        other => std::slice::from_ref(other),
    };
    let data = results
        .iter()
        .filter_map(|r| r.get("data"))
        .find(|d| d.get("viewer").is_some())
        .ok_or_else(|| {
            FetchError::parse(HOST, "no data.viewer; the usage query may have changed")
        })?;
    let viewer = data.get("viewer").filter(|v| !v.is_null()).ok_or_else(|| {
        FetchError::SignInAgain(format!("the Gemini cookie is signed out; {SIGN_IN}"))
    })?;
    let info = viewer.get("rpcs").and_then(|r| r.get("getUsageInfo"));
    let pools = info
        .and_then(|i| i.get("pools"))
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::parse(HOST, "no getUsageInfo.pools"))?;
    let windows: Vec<Window> = [
        ("GXU_FIVE_HOURLY", "5 h", FIVE_HOURS),
        ("GXU_WEEKLY", "Week", WEEK),
    ]
    .into_iter()
    .filter_map(|(kind, label, len)| {
        let p = pools
            .iter()
            .find(|p| p.get("poolType").and_then(Value::as_str) == Some(kind))?;
        let used = num(p.get("percentageUsed"))?;
        let reset = ts(p.get("gxu").and_then(|g| g.get("windowRefreshesAt")));
        Some(Window::new(label, used, reset, Some(len)))
    })
    .collect();
    if windows.is_empty() {
        return Err(FetchError::parse(
            HOST,
            "this account reports no usage windows",
        ));
    }
    let plan = match info
        .and_then(|i| i.get("subscriptionTier"))
        .and_then(Value::as_str)
    {
        Some("TIER1") => Some("Pro".to_owned()),
        _ => None,
    };
    Ok(Usage {
        plan,
        detail: None,
        windows,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn resp(body: &str) -> Response {
        Response {
            status: 200,
            body: body.into(),
            retry_after: None,
        }
    }

    #[test]
    fn parses_pools_behind_the_xssi_prefix() {
        let body = json!([
            {"data": {"other": 1}},
            {"data": {"viewer": {"rpcs": {"getUsageInfo": {
                "subscriptionTier": "TIER1",
                "pools": [
                    {"poolType": "GXU_WEEKLY", "percentageUsed": 0.4, "gxu": {"windowRefreshesAt": "2026-10-20T07:00:00Z"}},
                    {"poolType": "GXU_FIVE_HOURLY", "percentageUsed": 0.05, "gxu": {"windowRefreshesAt": "2026-10-16T15:00:00Z"}},
                    {"poolType": "OTHER", "percentageUsed": 1}
                ]
            }}}}}
        ]);
        let u = finish(&resp(&format!(")]}}'\n{body}"))).unwrap();
        assert_eq!(u.plan.as_deref(), Some("Pro"));
        assert_eq!(u.windows.len(), 2);
        assert_eq!(u.windows[0].label, "5 h");
        assert!((u.windows[0].used - 0.05).abs() < 1e-9);
        assert_eq!(
            u.windows[1].resets_at.unwrap().to_string(),
            "2026-10-20T07:00:00Z"
        );
    }

    #[test]
    fn signed_out_cookie_asks_to_sign_in() {
        let null_viewer = json!([{"data": {"viewer": null}}]).to_string();
        assert!(matches!(
            finish(&resp(&null_viewer)),
            Err(FetchError::SignInAgain(_))
        ));
        assert!(matches!(
            finish(&resp("<!doctype html>")),
            Err(FetchError::SignInAgain(_))
        ));
        assert!(matches!(finish(&resp("[]")), Err(FetchError::Parse { .. })));
    }

    #[test]
    fn url_encodes_the_query() {
        let u = url();
        assert!(u.contains("query_signature=2%2FdWbCvkVB0X16r9NPnxifeOAAqdCqMf0xeWvau9yPy7g%3D"));
        assert!(u.contains("variables=%7B%22g1Request%22"));
        let r = request("Cookie: SID=1; HSID=2");
        assert!(r.headers.contains(&("Cookie", "SID=1; HSID=2".into())));
    }
}
