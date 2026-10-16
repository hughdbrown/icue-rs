//! Pace, run-out projection and the "use next" ranking.
//!
//! For a window of length `L` that resets at `r`, used fraction `u` at time `t`:
//! - `elapsed = 1 − (r − t) / L`, clamped to 0–1;
//! - `ahead = u − elapsed`: above 0 the quota runs out before the reset at this rate;
//! - run-out time at the average rate so far: `t + (1 − u) · (elapsed · L) / u`.
//!
//! A subscription is **blocked** while any window is used up. Among the others, **urgency** is
//! the worst use-it-or-lose-it ratio over its windows: for each window, the quota it can still
//! really use (its own remaining, capped by the remaining of every longer window, since using it
//! spends those too) divided by the share of its time left. 62% of a week left with 13% of the
//! week to go scores 4.8; a fresh window scores at most 1. The suggestion is the most urgent
//! subscription.

use jiff::{SignedDuration, Timestamp};

use crate::model::{Pace, Status, Subscription, Suggestion, Window};

/// Smallest share of time left used as a divisor, so a window about to reset does not divide by 0.
const MIN_TIME_LEFT: f64 = 0.01;

/// Pace of `w` at `now`, if its reset time and length are known.
#[must_use]
pub fn pace(w: &Window, now: Timestamp) -> Option<Pace> {
    let reset = w.resets_at?;
    let len = w.length_secs.filter(|&l| l > 0)?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "window lengths are far below 2^52 s"
    )]
    let len = len as f64;
    let left = reset.duration_since(now).as_secs_f64().clamp(0.0, len);
    let elapsed = 1.0 - left / len;
    let runs_out_at = (w.used > 0.0 && w.used < 1.0 && elapsed > 0.0)
        .then(|| (1.0 - w.used) * elapsed * len / w.used)
        .filter(|&secs| secs < left)
        .and_then(|secs| SignedDuration::try_from_secs_f64(secs).ok())
        .and_then(|d| now.checked_add(d).ok());
    Some(Pace {
        elapsed,
        ahead: w.used - elapsed,
        runs_out_at,
    })
}

/// Share of `w`'s time still to come at `now` (1 when unknown).
fn time_left(w: &Window, now: Timestamp) -> f64 {
    pace(w, now).map_or(1.0, |p| 1.0 - p.elapsed)
}

/// When `windows` stop blocking: the latest reset among used-up windows that report one. `None`
/// when no window is used up, or when none of the used-up ones reports a reset time.
#[must_use]
pub fn blocked_until(windows: &[Window]) -> Option<Timestamp> {
    windows
        .iter()
        .filter(|w| w.is_exhausted())
        .map(|w| w.resets_at)
        .max()
        .flatten()
}

/// How much quota would expire unused without use soon. `None` when there are no windows or one
/// is used up.
#[must_use]
pub fn urgency(windows: &[Window], now: Timestamp) -> Option<f64> {
    if windows.is_empty() || windows.iter().any(Window::is_exhausted) {
        return None;
    }
    // A window of unknown length counts as the longest.
    let len = |w: &Window| w.length_secs.unwrap_or(i64::MAX);
    windows
        .iter()
        .map(|w| {
            let usable = windows
                .iter()
                .filter(|o| len(o) >= len(w))
                .map(Window::remaining)
                .fold(1.0, f64::min);
            usable / time_left(w, now).max(MIN_TIME_LEFT)
        })
        .reduce(f64::max)
}

/// Fill in each window's pace and each subscription's block and urgency, then pick the
/// suggestion among fetched (or stale) subscriptions that are not blocked.
pub fn annotate(subs: &mut [Subscription], now: Timestamp) -> Option<Suggestion> {
    for s in subs.iter_mut() {
        for w in &mut s.windows {
            w.pace = pace(w, now);
        }
        s.blocked_until = blocked_until(&s.windows);
        s.urgency = urgency(&s.windows, now);
    }
    subs.iter()
        .filter(|s| s.status != Status::Error)
        .filter_map(|s| s.urgency.map(|u| (s, u)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(s, urgency)| Suggestion {
            id: s.id.clone(),
            urgency,
        })
}

/// Windows from an earlier fetch, as of `now`: one whose reset has passed has refilled.
#[must_use]
pub fn age(windows: &[Window], now: Timestamp) -> Vec<Window> {
    windows
        .iter()
        .map(|w| match w.resets_at {
            Some(r) if r <= now => Window {
                used: 0.0,
                resets_at: None,
                amount: w.amount.clone().map(|mut a| {
                    a.used = 0.0;
                    a
                }),
                ..w.clone()
            },
            _ => w.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderKind;

    fn t(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    const NOW: &str = "2026-10-16T12:00:00Z";

    fn five_hour(used: f64, reset: &str) -> Window {
        Window::new("5 h", used, Some(t(reset)), Some(5 * 3600))
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn halfway_through_and_half_used_is_on_pace() {
        // Resets in 2.5 h of 5 h: half the time gone.
        let p = pace(&five_hour(0.5, "2026-10-16T14:30:00Z"), t(NOW)).unwrap();
        assert!(approx(p.elapsed, 0.5));
        assert!(approx(p.ahead, 0.0));
        assert_eq!(
            p.runs_out_at, None,
            "runs out exactly at the reset, not before"
        );
    }

    #[test]
    fn ahead_of_pace_projects_a_run_out() {
        // 80% used in the first 2.5 h → 20% more takes 2.5·0.2/0.8 h = 37.5 min.
        let p = pace(&five_hour(0.8, "2026-10-16T14:30:00Z"), t(NOW)).unwrap();
        assert!(approx(p.ahead, 0.3));
        assert_eq!(p.runs_out_at, Some(t("2026-10-16T12:37:30Z")));
    }

    #[test]
    fn behind_pace_and_edge_cases() {
        let p = pace(&five_hour(0.1, "2026-10-16T13:00:00Z"), t(NOW)).unwrap();
        assert!(approx(p.elapsed, 0.8));
        assert!(approx(p.ahead, -0.7));
        assert_eq!(p.runs_out_at, None);
        // Reset already passed: all time elapsed; reset farther than the length: none elapsed.
        assert!(approx(
            pace(&five_hour(0.3, "2026-10-16T11:00:00Z"), t(NOW))
                .unwrap()
                .elapsed,
            1.0
        ));
        assert!(approx(
            pace(&five_hour(0.0, "2026-10-17T12:00:00Z"), t(NOW))
                .unwrap()
                .elapsed,
            0.0
        ));
        // Unknown length or reset: no pace.
        assert_eq!(
            pace(&Window::new("x", 0.5, Some(t(NOW)), None), t(NOW)),
            None
        );
        assert_eq!(pace(&Window::new("x", 0.5, None, Some(60)), t(NOW)), None);
        assert_eq!(
            pace(&Window::new("x", 0.5, Some(t(NOW)), Some(0)), t(NOW)),
            None
        );
    }

    #[test]
    fn blocked_until_the_last_exhausted_window_resets() {
        let week = Window::new(
            "Week",
            1.0,
            Some(t("2026-10-20T00:00:00Z")),
            Some(7 * 86400),
        );
        let five = five_hour(1.0, "2026-10-16T13:00:00Z");
        assert_eq!(
            blocked_until(&[five.clone(), week.clone()]),
            Some(t("2026-10-20T00:00:00Z"))
        );
        assert_eq!(blocked_until(&[five_hour(0.4, NOW)]), None);
        assert_eq!(urgency(&[five, week], t(NOW)), None);
    }

    #[test]
    fn urgency_favours_quota_about_to_expire() {
        // 90% left with 10% of the time to go: use it now.
        let expiring = [five_hour(0.1, "2026-10-16T12:30:00Z")];
        // 90% left with all the time to go.
        let fresh = [five_hour(0.1, "2026-10-16T17:00:00Z")];
        let a = urgency(&expiring, t(NOW)).unwrap();
        let b = urgency(&fresh, t(NOW)).unwrap();
        assert!(approx(a, 9.0), "{a}");
        assert!(approx(b, 0.9), "{b}");
        // The weekly window limits what is left even though the 5 h one is nearly empty.
        let tight_week = [
            five_hour(0.1, "2026-10-16T12:30:00Z"),
            Window::new(
                "Week",
                0.95,
                Some(t("2026-10-19T00:00:00Z")),
                Some(7 * 86400),
            ),
        ];
        assert!(approx(urgency(&tight_week, t(NOW)).unwrap(), 0.5));
        // Much of the week left with little of the week to go dominates a fresh 5 h window.
        let expiring_week = [
            five_hour(0.14, "2026-10-16T14:22:00Z"),
            Window::new(
                "Week",
                0.38,
                Some(t("2026-10-17T09:52:00Z")),
                Some(7 * 86400),
            ),
        ];
        let u = urgency(&expiring_week, t(NOW)).unwrap();
        assert!((u - 0.62 / (21.866_666 / 168.0)).abs() < 1e-3, "{u}");
        // A window about to reset does not divide by zero.
        assert!(urgency(&[five_hour(0.5, NOW)], t(NOW)).unwrap().is_finite());
    }

    fn sub(id: &str, status: Status, windows: Vec<Window>) -> Subscription {
        Subscription {
            id: id.into(),
            provider: ProviderKind::Claude,
            label: id.into(),
            plan: None,
            detail: None,
            status,
            message: None,
            windows,
            fetched_at: None,
            blocked_until: None,
            urgency: None,
        }
    }

    #[test]
    fn annotate_suggests_the_most_urgent_usable_subscription() {
        let mut subs = vec![
            sub(
                "fresh",
                Status::Ok,
                vec![five_hour(0.1, "2026-10-16T17:00:00Z")],
            ),
            sub(
                "expiring",
                Status::Stale,
                vec![five_hour(0.1, "2026-10-16T12:30:00Z")],
            ),
            sub(
                "blocked",
                Status::Ok,
                vec![five_hour(1.0, "2026-10-16T12:10:00Z")],
            ),
            sub("broken", Status::Error, vec![]),
        ];
        let s = annotate(&mut subs, t(NOW)).unwrap();
        assert_eq!(s.id, "expiring");
        assert!(subs[0].windows[0].pace.is_some());
        assert_eq!(subs[2].blocked_until, Some(t("2026-10-16T12:10:00Z")));
        assert_eq!(subs[3].urgency, None);
        assert_eq!(annotate(&mut [], t(NOW)), None);
    }

    #[test]
    fn aged_windows_refill_after_their_reset() {
        let old = [
            five_hour(0.7, "2026-10-16T11:00:00Z"),
            five_hour(0.7, "2026-10-16T13:00:00Z"),
        ];
        let aged = age(&old, t(NOW));
        assert!(approx(aged[0].used, 0.0));
        assert_eq!(aged[0].resets_at, None);
        assert_eq!(aged[1], old[1]);
    }
}
