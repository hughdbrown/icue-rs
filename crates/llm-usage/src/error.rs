//! Why fetching one subscription failed. Messages are shown in the widget, so they say what to do.

use std::time::Duration;

use crate::http::{NetworkError, Response};

/// Why a subscription could not be fetched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// No credentials where they were expected.
    #[error("{0}")]
    MissingCredential(String),
    /// The credentials were rejected or have expired for good; the user must sign in again.
    #[error("{0}")]
    SignInAgain(String),
    /// The service asked us to slow down.
    #[error("rate limited{}", .retry_after.map(|d| format!("; retrying in {} s", d.as_secs())).unwrap_or_default())]
    RateLimited {
        /// How long it asked us to wait.
        retry_after: Option<Duration>,
    },
    /// An unexpected HTTP status.
    #[error("HTTP {status} from {host}{}", if detail.is_empty() { String::new() } else { format!(": {detail}") })]
    Status {
        /// Service host.
        host: String,
        /// HTTP status.
        status: u16,
        /// Start of the body, if it explains anything.
        detail: String,
    },
    /// The network failed.
    #[error("{0}")]
    Network(#[from] NetworkError),
    /// The response was not what the parser knows; the service may have changed.
    #[error("unexpected response from {host}: {reason}")]
    Parse {
        /// Service host.
        host: String,
        /// What was wrong.
        reason: String,
    },
    /// Reading or writing a credential store failed.
    #[error("{0}")]
    Store(String),
}

impl FetchError {
    /// A parse failure for `host`.
    pub(crate) fn parse(host: &str, reason: impl Into<String>) -> Self {
        Self::Parse {
            host: host.to_owned(),
            reason: reason.into(),
        }
    }

    /// The error for a non-2xx `r` from `host`; 401/403 become [`FetchError::SignInAgain`] with
    /// `sign_in` as the advice.
    pub(crate) fn from_status(host: &str, r: &Response, sign_in: &str) -> Self {
        match r.status {
            401 | 403 => Self::SignInAgain(format!("{host} refused the credentials; {sign_in}")),
            429 => Self::RateLimited {
                retry_after: r.retry_after,
            },
            status => Self::Status {
                host: host.to_owned(),
                status,
                detail: excerpt(&r.body),
            },
        }
    }
}

/// The first line of a body, shortened, when it is not HTML.
fn excerpt(body: &str) -> String {
    let line = body.trim().lines().next().unwrap_or_default();
    if line.starts_with('<') {
        return String::new();
    }
    let mut s: String = line.chars().take(120).collect();
    if s.len() < line.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: u16, body: &str) -> Response {
        Response {
            status,
            body: body.into(),
            retry_after: Some(Duration::from_secs(30)),
        }
    }

    #[test]
    fn statuses_map_to_actionable_errors() {
        assert_eq!(
            FetchError::from_status("h", &resp(401, ""), "run `x`").to_string(),
            "h refused the credentials; run `x`"
        );
        assert_eq!(
            FetchError::from_status("h", &resp(429, ""), "").to_string(),
            "rate limited; retrying in 30 s"
        );
        assert_eq!(
            FetchError::from_status("h", &resp(500, "<html>"), "").to_string(),
            "HTTP 500 from h"
        );
        let long = "x".repeat(300);
        assert!(
            FetchError::from_status("h", &resp(502, &long), "")
                .to_string()
                .ends_with('…')
        );
    }
}
