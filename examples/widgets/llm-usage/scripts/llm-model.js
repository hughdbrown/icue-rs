// LLM Usage widget model: turns the LlmUsage plugin's snapshot into plain view data (texts and
// state names). Pure: no DOM, the clock is passed in. Loaded before llm-usage.js; tested by
// llm-usage.test.mjs.
(function (root) {
	"use strict";

	const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
	const MIN_TIME_LEFT = 0.01;
	const EXHAUSTED = 0.995;

	/** Milliseconds → "45s", "12m", "2h 13m", "4d 6h". */
	function span(ms) {
		const s = Math.max(0, Math.floor(ms / 1000));
		if (s < 60) return `${s}s`;
		if (s < 3600) return `${Math.floor(s / 60)}m`;
		if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
		return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
	}

	const pad = (n) => String(n).padStart(2, "0");

	/** Local time to the nearest minute: "19:30" today, "Sat 15:00" on another day. */
	function clock(date, now) {
		// Services report resets a moment before the minute (14:59:59.9).
		const d = new Date(new Date(date).getTime() + 30 * 1000);
		const n = new Date(now);
		const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
		return d.toDateString() === n.toDateString() ? hm : `${DAYS[d.getDay()]} ${hm}`;
	}

	const ms = (t) => (t ? new Date(t).getTime() : NaN);

	/** Share of the window's time gone at `now` (0–1), or null when unknown. */
	function elapsed(w, now) {
		const reset = ms(w.resetsAt);
		if (!Number.isFinite(reset) || !(w.lengthSecs > 0)) return null;
		const left = Math.min(Math.max((reset - now) / 1000, 0), w.lengthSecs);
		return 1 - left / w.lengthSecs;
	}

	/**
	 * Pace state of a window:
	 * - "out": used up;
	 * - "ahead": runs out before the reset at this rate;
	 * - "behind": quota will be left over at the reset;
	 * - "on": about right, or too early to tell;
	 * - "idle": not started (no reset time yet).
	 */
	function state(w, now) {
		if (w.used >= EXHAUSTED) return "out";
		if (!w.resetsAt) return "idle";
		const e = elapsed(w, now);
		if (e === null) return "on";
		const ahead = w.used - e;
		if (ahead > 0.1) return "ahead";
		if (ahead < -0.15 && e > 0.2) return "behind";
		return "on";
	}

	/** Quota expected to be left unused at the reset at the average rate so far (0–1), or null. */
	function projectedWaste(w, now) {
		const e = elapsed(w, now);
		if (e === null || e <= 0) return null;
		return Math.max(0, 1 - w.used / e);
	}

	/** When the window runs out at the average rate so far, if before the reset. */
	function runsOutAt(w, now) {
		const e = elapsed(w, now);
		if (e === null || e <= 0 || w.used <= 0 || w.used >= 1) return null;
		const len = w.lengthSecs * 1000;
		const toFull = ((1 - w.used) * e * len) / w.used;
		const left = ms(w.resetsAt) - now;
		return toFull < left ? new Date(now + toFull) : null;
	}

	function leftText(w) {
		if (w.amount) return `$${(w.amount.limit - w.amount.used).toFixed(2)} left`;
		return `${Math.round((1 - w.used) * 100)}% left`;
	}

	function windowView(w, now) {
		const reset = ms(w.resetsAt);
		return {
			label: w.label,
			used: w.used,
			tick: elapsed(w, now),
			state: state(w, now),
			left: leftText(w),
			reset: Number.isFinite(reset) ? `${span(reset - now)} · ${clock(reset, now)}` : "not started",
		};
	}

	/** The one line worth saying about a subscription, or "". */
	function note(windows, now) {
		const out = windows.filter((w) => w.used >= EXHAUSTED && w.resetsAt);
		if (out.length) {
			const back = Math.max(...out.map((w) => ms(w.resetsAt)));
			return { kind: "out", text: `Used up · back in ${span(back - now)} (${clock(back, now)})` };
		}
		for (const w of windows) {
			const t = runsOutAt(w, now);
			if (t && state(w, now) === "ahead") return { kind: "ahead", text: `${w.label}: runs out ≈ ${clock(t, now)}, before its reset` };
		}
		for (const w of [...windows].reverse()) {
			const waste = projectedWaste(w, now);
			if (waste !== null && state(w, now) === "behind") {
				return { kind: "behind", text: `${w.label}: ≈ ${Math.round(waste * 100)}% will go unused at this pace` };
			}
		}
		return { kind: "", text: "" };
	}

	/** Window with the worst use-it-or-lose-it ratio (mirrors llm_usage::pace::urgency). */
	function mostUrgent(windows, now) {
		const len = (w) => (w.lengthSecs > 0 ? w.lengthSecs : Infinity);
		let best = null;
		for (const w of windows) {
			const usable = Math.min(1, ...windows.filter((o) => len(o) >= len(w)).map((o) => 1 - o.used));
			const e = elapsed(w, now);
			const ratio = usable / Math.max(e === null ? 1 : 1 - e, MIN_TIME_LEFT);
			if (!best || ratio > best.ratio) best = { w, usable, ratio };
		}
		return best;
	}

	function suggestionText(sub, now) {
		const best = mostUrgent(sub.windows, now);
		if (!best) return `Use ${sub.label} next`;
		const reset = ms(best.w.resetsAt);
		const what = best.w.amount ? leftText(best.w) : `${Math.round(best.usable * 100)}% of its ${best.w.label.toLowerCase()}`;
		const when = Number.isFinite(reset) ? ` expires in ${span(reset - now)}` : " is left";
		return `Use ${sub.label} next: ${what}${when}`;
	}

	function subView(s, now, suggestedId) {
		return {
			id: s.id,
			label: s.label,
			plan: s.plan || "",
			detail: s.detail || "",
			status: s.status,
			message: s.message || "",
			suggested: s.id === suggestedId,
			windows: s.windows.map((w) => windowView(w, now)),
			note: s.status === "error" ? { kind: "", text: "" } : note(s.windows, now),
		};
	}

	/**
	 * Snapshot → view:
	 * - { kind: "message", title, detail } before data, on a config error, or with nothing enabled;
	 * - { kind: "list", subs, suggestion, updated }.
	 */
	function view(snapshot, now) {
		if (!snapshot) return { kind: "message", title: "Fetching usage…", detail: "" };
		if (snapshot.configError) {
			return { kind: "message", title: "Set up your subscriptions", detail: snapshot.configError };
		}
		if (!snapshot.subscriptions.length) {
			return {
				kind: "message",
				title: "No subscriptions enabled",
				detail: `Enable some in ${snapshot.configPath}, then check them with \`edgectl llm-usage\`.`,
			};
		}
		const suggestedId = snapshot.suggestion ? snapshot.suggestion.id : null;
		const suggested = snapshot.subscriptions.find((s) => s.id === suggestedId);
		const generated = ms(snapshot.generatedAt);
		return {
			kind: "list",
			subs: snapshot.subscriptions.map((s) => subView(s, now, suggestedId)),
			suggestion: suggested ? suggestionText(suggested, now) : "Every subscription is used up for now",
			updated: Number.isFinite(generated) ? `Updated ${clock(generated, now)}` : "",
		};
	}

	root.LlmModel = { span, clock, elapsed, state, projectedWaste, runsOutAt, mostUrgent, view };
})(typeof window !== "undefined" ? window : globalThis);
