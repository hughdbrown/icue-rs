//! Host-side fetch for widgets.
//!
//! Widgets are served from `icuew://<instance>/`, so a `fetch()` to an API that sends no CORS
//! headers (for example Yahoo Finance) is blocked by WebKit. For domains a widget declares in its
//! manifest (`{"type": "url", "domain": …}`), the shim routes `fetch()` to
//! `icuew://<instance>/__edgehost/fetch?url=…`, which is same-origin, and EdgeHost performs the
//! request here on a background thread.
//!
//! This is also the enforcement point for `url` permissions: only declared domains are fetched.
//! Redirects are followed here, not by the HTTP client, so every hop is checked against the same
//! permissions: a declared host cannot redirect a widget to an undeclared one.
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
/// Redirect hops followed before giving up.
const MAX_REDIRECTS: usize = 5;
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
    /// A permitted URL redirected to one the widget did not declare.
    #[error("{from} redirects to {to}, which is not in the widget's declared url permissions")]
    RedirectNotPermitted {
        /// The URL that answered with the redirect.
        from: String,
        /// Where it pointed.
        to: String,
    },
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
            Self::NotPermitted(_) | Self::RedirectNotPermitted { .. } => 403,
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
    let loopback = edge_plugins::net::is_loopback_host(&host);
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
    let mut url = target_url(query).ok_or(ProxyError::BadRequest)?;
    if !is_allowed(permissions, &url) {
        return Err(ProxyError::NotPermitted(url));
    }
    let mut hops = 0;
    let mut resp = loop {
        let upstream = |message: String| ProxyError::Upstream {
            url: url.clone(),
            message,
        };
        // A host written as loopback got past is_allowed only through an explicit permission; any
        // other name that resolves to this machine is refused when connecting.
        let local = url
            .parse::<Uri>()
            .ok()
            .and_then(|u| u.host().map(edge_plugins::net::is_loopback_host))
            .unwrap_or(false);
        let resp = agent(local)
            .get(&url)
            .call()
            .map_err(|e| upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok());
        let (true, Some(location)) = (is_redirect(status), location) else {
            break resp;
        };
        let next = resolve_location(&url, location)
            .ok_or_else(|| upstream(format!("unusable redirect location {location:?}")))?;
        if !is_allowed(permissions, &next) {
            return Err(ProxyError::RedirectNotPermitted {
                from: url,
                to: next,
            });
        }
        hops += 1;
        if hops > MAX_REDIRECTS {
            return Err(upstream(format!(
                "too many redirects (more than {MAX_REDIRECTS})"
            )));
        }
        url = next;
    };
    let upstream = |message: String| ProxyError::Upstream {
        url: url.clone(),
        message,
    };
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

/// Statuses whose `Location` the proxy follows (as a GET, like browsers do for a GET).
fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// `location` (absolute, scheme-relative, absolute-path or relative-path) resolved against `base`.
/// `None` unless the result is an http(s) URL.
fn resolve_location(base: &str, location: &str) -> Option<String> {
    let location = location.trim();
    let lower = location.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(location.to_owned());
    }
    let (scheme, rest) = base.split_once("://")?;
    if let Some(after) = location.strip_prefix("//") {
        return Some(format!("{scheme}://{after}"));
    }
    if location.contains(':') && !location.starts_with(['/', '.', '?', '#']) {
        // Another scheme (javascript:, file:, …).
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if location.starts_with('/') {
        return Some(format!("{scheme}://{authority}{location}"));
    }
    let path = rest
        .strip_prefix(authority)
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
    Some(format!("{scheme}://{authority}{dir}{location}"))
}

fn error_response(e: &ProxyError) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(e.status())
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Cow::Owned(e.to_string().into_bytes()))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"proxy error".as_slice())))
}

/// The agent for public hosts, or for an explicitly permitted loopback host (`local`).
fn agent(local: bool) -> &'static ureq::Agent {
    static PUBLIC: OnceLock<ureq::Agent> = OnceLock::new();
    static LOCAL: OnceLock<ureq::Agent> = OnceLock::new();
    let make = || {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            // Redirects are followed in `fetch_permitted`, which checks each hop.
            .max_redirects(0)
            .user_agent(USER_AGENT)
            .build();
        edge_plugins::net::agent(config, local)
    };
    if local {
        LOCAL.get_or_init(make)
    } else {
        PUBLIC.get_or_init(make)
    }
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

    /// An HTTP server on 127.0.0.1 answering each request with `response(path)`.
    fn stub_server(response: impl Fn(&str) -> String + Send + 'static) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(16) {
                let mut s = stream.unwrap();
                let mut buf = [0_u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let _ = s.write_all(response(&path).as_bytes());
            }
        });
        port
    }

    fn body(text: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
            text.len()
        )
    }

    fn redirect(location: &str) -> String {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }

    fn fetch(
        perms: &[UrlPermission],
        url: &str,
    ) -> Result<Response<Cow<'static, [u8]>>, ProxyError> {
        let query = format!("url={}", url.replace(':', "%3A").replace('/', "%2F"));
        fetch_permitted(perms, Some(&query))
    }

    fn local(port: u16) -> UrlPermission {
        perm("127.0.0.1", Some(port))
    }

    #[test]
    fn any_cannot_reach_this_machine_through_other_spellings() {
        let port = stub_server(|_| body("local secret"));
        let any = [perm("any", Some(port))];
        for host in ["0.0.0.0", "2130706433"] {
            let err = fetch(&any, &format!("http://{host}:{port}/")).unwrap_err();
            assert!(
                err.to_string().contains("resolves to this machine"),
                "{host}: {err}"
            );
        }
        // A redirect from an allowed public-looking host to this machine is caught the same way.
        let bounce = stub_server(move |_| redirect(&format!("http://0.0.0.0:{port}/")));
        let perms = [local(bounce), perm("any", Some(port))];
        let err = fetch(&perms, &format!("http://127.0.0.1:{bounce}/")).unwrap_err();
        assert!(
            err.to_string().contains("resolves to this machine"),
            "{err}"
        );
        // Declared explicitly, the local service is reachable.
        let ok = fetch(&[local(port)], &format!("http://127.0.0.1:{port}/")).unwrap();
        assert_eq!(ok.body().as_ref(), b"local secret");
    }

    /// `localtest.me` resolves to 127.0.0.1 in public DNS (`cargo test -- --ignored`).
    #[test]
    #[ignore = "needs DNS"]
    fn any_cannot_reach_this_machine_through_dns() {
        let port = stub_server(|_| body("local secret"));
        let err = fetch(
            &[perm("any", Some(port))],
            &format!("http://localtest.me:{port}/"),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("resolves to this machine"),
            "{err}"
        );
    }

    #[test]
    fn redirect_to_an_undeclared_host_is_refused() {
        let secret = stub_server(|_| body("secret"));
        let open = stub_server(move |_| redirect(&format!("http://127.0.0.1:{secret}/data")));
        let err = fetch(&[local(open)], &format!("http://127.0.0.1:{open}/")).unwrap_err();
        assert_eq!(err.status(), 403);
        let text = err.to_string();
        assert!(
            text.contains(&format!("redirects to http://127.0.0.1:{secret}/data")),
            "{text}"
        );
    }

    #[test]
    fn redirects_within_the_permissions_are_followed() {
        let end = stub_server(|path| {
            if path == "/final" {
                body("done")
            } else {
                body("wrong path")
            }
        });
        let start = stub_server(move |path| match path {
            "/" => redirect("/next"), // relative
            _ => redirect(&format!("http://127.0.0.1:{end}/final")),
        });
        let resp = fetch(
            &[local(start), local(end)],
            &format!("http://127.0.0.1:{start}/"),
        )
        .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.body().as_ref(), b"done");

        let looping = stub_server(|_| redirect("/again"));
        let err = fetch(&[local(looping)], &format!("http://127.0.0.1:{looping}/")).unwrap_err();
        assert!(err.to_string().contains("too many redirects"), "{err}");
    }

    #[test]
    fn relative_locations_resolve_against_the_current_url() {
        assert_eq!(
            resolve_location("https://a.example/x/y?q", "/z").as_deref(),
            Some("https://a.example/z")
        );
        assert_eq!(
            resolve_location("https://a.example/x/y?q", "w").as_deref(),
            Some("https://a.example/x/w")
        );
        assert_eq!(
            resolve_location("https://a.example:8443/x", "//b.example/p").as_deref(),
            Some("https://b.example/p")
        );
        assert_eq!(
            resolve_location("http://a.example/", "https://c.example/").as_deref(),
            Some("https://c.example/")
        );
        assert_eq!(
            resolve_location("http://a.example/", "javascript:alert(1)"),
            None
        );
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
