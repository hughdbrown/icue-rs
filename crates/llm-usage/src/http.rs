//! The network seam: a small [`Http`] trait, implemented with `ureq` and faked in tests.

use std::time::Duration;

/// Timeout of one request, connection included.
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// Largest response body read.
const MAX_BODY: u64 = 2 * 1024 * 1024;

/// A request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// No body (GET).
    None,
    /// `application/x-www-form-urlencoded` pairs (POST).
    Form(Vec<(String, String)>),
    /// A JSON document (POST).
    Json(serde_json::Value),
}

/// One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Absolute URL.
    pub url: String,
    /// Extra headers.
    pub headers: Vec<(&'static str, String)>,
    /// Body; anything but [`Body::None`] makes it a POST.
    pub body: Body,
}

impl Request {
    /// A GET of `url`.
    #[must_use]
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            body: Body::None,
        }
    }

    /// A POST of `body` to `url`.
    #[must_use]
    pub fn post(url: impl Into<String>, body: Body) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            body,
        }
    }

    /// Add a header.
    #[must_use]
    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    /// Add `Authorization: Bearer <token>`.
    #[must_use]
    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", format!("Bearer {token}"))
    }
}

/// A response of any status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP status.
    pub status: u16,
    /// Body as text (lossy UTF-8).
    pub body: String,
    /// `Retry-After` in seconds, when sent.
    pub retry_after: Option<Duration>,
}

/// The network failed before a status arrived.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NetworkError(pub String);

/// Sends requests.
pub trait Http: Send + Sync {
    /// Send `req`; any status is a response.
    ///
    /// # Errors
    ///
    /// DNS, connection, TLS or timeout failures.
    fn send(&self, req: &Request) -> Result<Response, NetworkError>;
}

/// [`Http`] over `ureq`. Redirects are not followed: none of the services redirect an API call,
/// and following one would send credentials to wherever it points.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl Default for UreqHttp {
    fn default() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("EdgeHost-llm-usage/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Http for UreqHttp {
    fn send(&self, req: &Request) -> Result<Response, NetworkError> {
        let net = |e: ureq::Error| NetworkError(format!("{}: {e}", host_of(&req.url)));
        let result = match &req.body {
            Body::None => {
                let mut r = self.agent.get(&req.url);
                for (k, v) in &req.headers {
                    r = r.header(*k, v);
                }
                r.call()
            }
            Body::Form(pairs) => {
                let mut r = self.agent.post(&req.url);
                for (k, v) in &req.headers {
                    r = r.header(*k, v);
                }
                r.send_form(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            }
            Body::Json(value) => {
                let mut r = self
                    .agent
                    .post(&req.url)
                    .header("Content-Type", "application/json");
                for (k, v) in &req.headers {
                    r = r.header(*k, v);
                }
                r.send(value.to_string())
            }
        };
        let mut response = result.map_err(net)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(net)?;
        Ok(Response {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
            retry_after,
        })
    }
}

/// `https://host/path` → `host`, for messages that must not echo query strings.
#[must_use]
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest)
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted [`Http`] that records what it was sent.

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::{Http, NetworkError, Request, Response};

    #[derive(Default)]
    pub struct FakeHttp {
        replies: Mutex<VecDeque<Result<Response, NetworkError>>>,
        /// Fixed replies for URLs containing a substring, checked before the queue.
        routes: Vec<(String, Response)>,
        pub sent: Mutex<Vec<Request>>,
    }

    impl FakeHttp {
        pub fn on(mut self, url_part: &str, status: u16, body: &str) -> Self {
            self.routes.push((
                url_part.into(),
                Response {
                    status,
                    body: body.into(),
                    retry_after: None,
                },
            ));
            self
        }

        pub fn reply(self, status: u16, body: &str) -> Self {
            self.replies.lock().unwrap().push_back(Ok(Response {
                status,
                body: body.into(),
                retry_after: None,
            }));
            self
        }

        pub fn urls(&self) -> Vec<String> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .map(|r| r.url.clone())
                .collect()
        }

        pub fn header(&self, i: usize, name: &str) -> Option<String> {
            self.sent.lock().unwrap()[i]
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        }
    }

    impl Http for FakeHttp {
        fn send(&self, req: &Request) -> Result<Response, NetworkError> {
            self.sent.lock().unwrap().push(req.clone());
            if let Some((_, r)) = self
                .routes
                .iter()
                .find(|(part, _)| req.url.contains(part.as_str()))
            {
                return Ok(r.clone());
            }
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(NetworkError(format!("no scripted reply for {}", req.url))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_strips_scheme_path_and_query() {
        assert_eq!(
            host_of("https://api.example.com/v1/x?k=secret"),
            "api.example.com"
        );
        assert_eq!(host_of("example.com?x"), "example.com");
    }
}
