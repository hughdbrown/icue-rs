//! Host-side fetch for widgets.
//!
//! Widgets are served from `icuew://<instance>/`, so a `fetch()` to an API that sends no CORS
//! headers (for example Yahoo Finance) is blocked by WebKit. For domains a widget declares in its
//! manifest (`{"type": "url", "domain": …}`), the shim routes `fetch()` to
//! `icuew://<instance>/__edgehost/fetch?url=…`, which is same-origin, and EdgeHost performs the
//! request here on a background thread.
//!
//! This is also the enforcement point for `url` permissions: only declared domains are fetched.
//! (Milestone M3 adds user approval of declared permissions.)

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use icuew_manifest::UrlPermission;
use wry::RequestAsyncResponder;
use wry::http::{Response, Uri, header};

use crate::assets::percent_decode;

/// Path of the proxy route inside a widget's origin.
pub const ROUTE: &str = "/__edgehost/fetch";
/// Largest response body passed back to a widget.
const MAX_BODY_BYTES: u64 = 5 * 1024 * 1024;
/// Whole-request timeout.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Some APIs reject library user agents.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) EdgeHost";

/// Why a proxied request was not performed.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// No `url` query parameter, or it is not an absolute http(s) URL.
    #[error("missing or invalid url parameter")]
    BadRequest,
    /// The widget did not declare the target domain.
    #[error("{0} is not in the widget's declared url permissions")]
    NotPermitted(String),
    /// The request failed.
    #[error("fetching {url}: {message}")]
    Upstream {
        /// Target URL.
        url: String,
        /// Error text.
        message: String,
    },
}

impl ProxyError {
    fn status(&self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::NotPermitted(_) => 403,
            Self::Upstream { .. } => 502,
        }
    }
}

/// Whether `url` may be fetched under `permissions`.
///
/// A permission matches its domain and any subdomain; a declared port must match the URL's
/// (default 443/80). `any` matches every host except loopback; loopback hosts must be declared
/// with a port, as in iCUE.
///
/// ```
/// use icuew_manifest::UrlPermission;
/// use widget_runtime::proxy::is_allowed;
/// let p = [UrlPermission { domain: "finance.yahoo.com".into(), port: None }];
/// assert!(is_allowed(&p, "https://query1.finance.yahoo.com/v8/finance/chart/AAPL"));
/// assert!(!is_allowed(&p, "https://evil-yahoo.com/"));
/// ```
#[must_use]
pub fn is_allowed(permissions: &[UrlPermission], url: &str) -> bool {
    let Ok(uri) = url.parse::<Uri>() else {
        return false;
    };
    let default_port = match uri.scheme_str() {
        Some("https") => 443,
        Some("http") => 80,
        _ => return false,
    };
    let Some(host) = uri
        .host()
        .map(|h| h.trim_matches(['[', ']']).to_ascii_lowercase())
    else {
        return false;
    };
    let port = uri.port_u16().unwrap_or(default_port);
    let loopback = host == "localhost" || host.starts_with("127.") || host == "::1";
    permissions.iter().any(|p| {
        let host_ok = if p.domain == "any" {
            !loopback
        } else {
            host == p.domain || host.ends_with(&format!(".{}", p.domain))
        };
        let port_ok = match p.port {
            Some(declared) => declared == port,
            None => !loopback,
        };
        host_ok && port_ok
    })
}

/// The decoded `url` parameter of a proxy request's query string.
#[must_use]
pub fn target_url(query: Option<&str>) -> Option<String> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("url="))
        .and_then(percent_decode)
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
}

/// Answer a proxy request asynchronously: the network call runs on its own thread.
pub fn handle(
    permissions: Arc<Vec<UrlPermission>>,
    query: Option<String>,
    responder: RequestAsyncResponder,
) {
    std::thread::spawn(move || {
        let response = match fetch_permitted(&permissions, query.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("icuew fetch: {e}");
                error_response(&e)
            }
        };
        responder.respond(response);
    });
}

fn fetch_permitted(
    permissions: &[UrlPermission],
    query: Option<&str>,
) -> Result<Response<Cow<'static, [u8]>>, ProxyError> {
    let url = target_url(query).ok_or(ProxyError::BadRequest)?;
    if !is_allowed(permissions, &url) {
        return Err(ProxyError::NotPermitted(url));
    }
    let upstream = |message: String| ProxyError::Upstream {
        url: url.clone(),
        message,
    };
    let mut resp = agent()
        .get(&url)
        .call()
        .map_err(|e| upstream(e.to_string()))?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_vec()
        .map_err(|e| upstream(e.to_string()))?;
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Owned(body))
        .map_err(|e| upstream(e.to_string()))
}

fn error_response(e: &ProxyError) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(e.status())
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Cow::Owned(e.to_string().into_bytes()))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"proxy error".as_slice())))
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .new_agent()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perm(domain: &str, port: Option<u16>) -> UrlPermission {
        UrlPermission {
            domain: domain.into(),
            port,
        }
    }

    #[test]
    fn domain_and_subdomain_matching() {
        let p = [perm("query1.finance.yahoo.com", Some(443))];
        assert!(is_allowed(
            &p,
            "https://query1.finance.yahoo.com/v8/finance/chart/AAPL?range=1d"
        ));
        assert!(
            !is_allowed(&p, "http://query1.finance.yahoo.com/x"),
            "port 80 not declared"
        );
        assert!(!is_allowed(&p, "https://query2.finance.yahoo.com/x"));
        assert!(!is_allowed(
            &p,
            "https://query1.finance.yahoo.com.evil.com/x"
        ));
        assert!(!is_allowed(&p, "file:///etc/passwd"));
        assert!(!is_allowed(&[], "https://example.com/"));
    }

    #[test]
    fn any_and_loopback_rules() {
        let any = [perm("any", None)];
        assert!(is_allowed(&any, "https://api.open-meteo.com/v1"));
        assert!(!is_allowed(&any, "http://localhost:8080/"));
        assert!(!is_allowed(&any, "http://127.0.0.1/"));
        let local = [perm("localhost", Some(8080))];
        assert!(is_allowed(&local, "http://localhost:8080/data"));
        assert!(!is_allowed(&local, "http://localhost:9090/data"));
        assert!(!is_allowed(&[perm("localhost", None)], "http://localhost/"));
    }

    #[test]
    fn target_url_parsing() {
        assert_eq!(
            target_url(Some("url=https%3A%2F%2Fa.example%2Fx%3Fy%3D1")).as_deref(),
            Some("https://a.example/x?y=1")
        );
        assert_eq!(
            target_url(Some("other=1&url=http%3A%2F%2Fb%2F")).as_deref(),
            Some("http://b/")
        );
        assert_eq!(target_url(Some("url=javascript%3Aalert(1)")), None);
        assert_eq!(target_url(None), None);
    }

    /// Real HTTPS request through the configured agent (`cargo test -- --ignored`).
    #[test]
    #[ignore = "needs network access"]
    fn fetches_over_https() {
        let p = [perm("query1.finance.yahoo.com", Some(443))];
        let resp = fetch_permitted(
            &p,
            Some("url=https%3A%2F%2Fquery1.finance.yahoo.com%2Fv8%2Ffinance%2Fchart%2FAAPL%3Frange%3D1d%26interval%3D1d"),
        )
        .unwrap();
        assert_eq!(resp.status(), 200);
        assert!(String::from_utf8_lossy(resp.body()).contains("\"symbol\":\"AAPL\""));
    }
}
