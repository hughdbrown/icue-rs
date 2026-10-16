//! What the widget is sent: one [`Snapshot`] of every configured [`Subscription`].
//!
//! Serialised with camelCase keys and RFC 3339 timestamps, so the page can use `new Date(…)`.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// A supported LLM service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Anthropic Claude Pro/Max, through Claude Code's login.
    Claude,
    /// `OpenAI` Codex with a `ChatGPT` plan, through the Codex CLI's login.
    Codex,
    /// Google Gemini app (consumer AI Pro/Ultra), through a browser cookie.
    Gemini,
    /// Kimi Code membership, through an API key.
    Kimi,
    /// Ollama Cloud, through an API key.
    Ollama,
}

impl ProviderKind {
    /// Every provider, in display order.
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Kimi,
        Self::Ollama,
    ];

    /// The config file's name for it (`claude`, `codex`, …).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Kimi => "kimi",
            Self::Ollama => "ollama",
        }
    }

    /// Human name, used as the default label.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::Kimi => "Kimi",
            Self::Ollama => "Ollama",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// Money spent against an allowance (Ollama's monthly credits).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Amount {
    /// Spent so far.
    pub used: f64,
    /// Allowance for the window.
    pub limit: f64,
    /// Currency code, e.g. `USD`.
    pub unit: String,
}

/// Where usage stands against straight-line use of the whole quota by the reset time.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pace {
    /// Fraction of the window's time already gone, 0–1.
    pub elapsed: f64,
    /// `used − elapsed`: above 0 the quota runs out before the reset, below 0 some will be wasted.
    pub ahead: f64,
    /// When the quota runs out at the average rate so far, if that is before the reset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runs_out_at: Option<Timestamp>,
}

/// One quota window of a subscription, such as Claude's 5-hour block or a weekly cap.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// Short name: `5 h`, `Week`, `Month`.
    pub label: String,
    /// Fraction used, 0–1.
    pub used: f64,
    /// Money used against an allowance, when the service counts in money.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Amount>,
    /// When the window resets.
    pub resets_at: Option<Timestamp>,
    /// Length of the window in seconds, when known.
    pub length_secs: Option<i64>,
    /// Filled in by [`crate::pace`] when both the reset time and the length are known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
}

impl Window {
    /// A window `used` (clamped to 0–1) of a quota that resets at `resets_at`.
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        used: f64,
        resets_at: Option<Timestamp>,
        length_secs: Option<i64>,
    ) -> Self {
        Self {
            label: label.into(),
            used: if used.is_finite() {
                used.clamp(0.0, 1.0)
            } else {
                0.0
            },
            amount: None,
            resets_at,
            length_secs,
            pace: None,
        }
    }

    /// Fraction left, 0–1.
    #[must_use]
    pub fn remaining(&self) -> f64 {
        1.0 - self.used
    }

    /// Whether the quota is used up.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.used >= 0.995
    }
}

/// What one fetch of a service returned.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    /// Plan name, e.g. `Max` or `Plus`.
    pub plan: Option<String>,
    /// Extra line, e.g. purchased credits.
    pub detail: Option<String>,
    /// Quota windows, shortest first.
    pub windows: Vec<Window>,
}

/// How fresh a subscription's numbers are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Fetched on the last attempt.
    Ok,
    /// The last attempt failed; the windows are from an earlier fetch.
    Stale,
    /// Never fetched successfully; see the message.
    Error,
}

/// One configured subscription and its quota.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// Config id, unique within the snapshot.
    pub id: String,
    /// Which service.
    pub provider: ProviderKind,
    /// Display name.
    pub label: String,
    /// Plan name, when the service reports it.
    pub plan: Option<String>,
    /// Extra line, e.g. purchased credits.
    pub detail: Option<String>,
    /// Freshness.
    pub status: Status,
    /// Why the last attempt failed (for `stale` and `error`).
    pub message: Option<String>,
    /// Quota windows, shortest first.
    pub windows: Vec<Window>,
    /// When the windows were fetched.
    pub fetched_at: Option<Timestamp>,
    /// While some window is used up: when the last such window resets.
    pub blocked_until: Option<Timestamp>,
    /// How much quota would go to waste without use soon (see [`crate::pace::urgency`]).
    pub urgency: Option<f64>,
}

/// Which subscription to use next.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// [`Subscription::id`].
    pub id: String,
    /// Its urgency.
    pub urgency: f64,
}

/// Everything the widget shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// When it was put together.
    pub generated_at: Timestamp,
    /// Config file path, for messages.
    pub config_path: String,
    /// Why the config could not be used, if it could not.
    pub config_error: Option<String>,
    /// How often EdgeHost fetches, in seconds.
    pub poll_secs: u64,
    /// Subscriptions in config order.
    pub subscriptions: Vec<Subscription>,
    /// Use this one next.
    pub suggestion: Option<Suggestion>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_used_is_clamped() {
        assert!((Window::new("x", 1.7, None, None).used - 1.0).abs() < f64::EPSILON);
        assert!(Window::new("x", -0.2, None, None).used.abs() < f64::EPSILON);
        assert!(Window::new("x", f64::NAN, None, None).used.abs() < f64::EPSILON);
        assert!(Window::new("x", 0.999, None, None).is_exhausted());
    }

    #[test]
    fn serialises_camel_case() {
        let w = Window::new(
            "5 h",
            0.25,
            Some("2026-10-16T12:00:00Z".parse().unwrap()),
            Some(18000),
        );
        let v = serde_json::to_value(&w).unwrap();
        assert_eq!(v["resetsAt"], "2026-10-16T12:00:00Z");
        assert_eq!(v["lengthSecs"], 18000);
        assert!(v.get("pace").is_none());
    }
}
