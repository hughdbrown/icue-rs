// Roborev widget model: turns the roborev daemon's JSON (/api/status, /api/summary, /api/jobs,
// /api/health, /api/repos) into plain view data. Pure: no DOM, no fetch, the clock is passed in.
// Loaded before roborev.js; tested by roborev.test.mjs.
(function (root) {
	"use strict";

	const WINDOWS = { "1d": "24 h", "7d": "7 days", "30d": "30 days" };
	const DAILY_DAYS = 14;

	/** Milliseconds → "45s", "12m", "2h 13m", "4d 6h". */
	function span(ms) {
		const s = Math.max(0, Math.floor(ms / 1000));
		if (s < 60) return `${s}s`;
		if (s < 3600) return `${Math.floor(s / 60)}m`;
		if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
		return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
	}

	/** Seconds (a duration statistic) → "0.7s", "38s", "1m 19s", "15m". */
	function secs(n) {
		if (!(n > 0)) return "–";
		if (n < 10) return `${n.toFixed(1)}s`;
		if (n < 60) return `${Math.round(n)}s`;
		const m = Math.floor(n / 60);
		const r = Math.round(n % 60);
		return m >= 10 || r === 0 ? `${m}m` : `${m}m ${r}s`;
	}

	const pct = (rate) => `${Math.round(rate * 100)}%`;
	const time = (t) => (t ? new Date(t).getTime() : NaN);

	/** The window setting → the daemon's `since` value ("1d", "7d", "30d"); 7d when unset. */
	function windowArg(setting) {
		const s = String(setting || "").trim();
		return WINDOWS[s] ? s : "7d";
	}

	/**
	 * The repository setting → the root path the daemon filters by.
	 * Accepts a repo name ("icue-rs"), an absolute path, or "~/…" (matched by path suffix, since
	 * the page cannot know the home folder). Returns { path } (null = all repos) or { error }.
	 */
	function resolveRepo(setting, repos) {
		const s = String(setting || "").trim().replace(/\/+$/, "");
		if (!s) return { path: null };
		const list = Array.isArray(repos) ? repos : [];
		if (s.startsWith("/")) return { path: s };
		if (s.startsWith("~/")) {
			const tail = s.slice(1);
			const hit = list.find((r) => String(r.root_path || "").endsWith(tail));
			return hit ? { path: hit.root_path } : { error: `No roborev repository at ${s}` };
		}
		const lower = s.toLowerCase();
		const hit = list.find((r) => String(r.name || "").toLowerCase() === lower);
		return hit ? { path: hit.root_path } : { error: `No roborev repository named "${s}"` };
	}

	/** "/api/x" plus query parameters, skipping null/empty ones. */
	function url(path, params) {
		const q = Object.entries(params || {})
			.filter(([, v]) => v !== null && v !== undefined && v !== "")
			.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`);
		return q.length ? `${path}?${q.join("&")}` : path;
	}

	/** Request paths for one refresh. `repoPath` null = all repositories. */
	function paths(since, repoPath, rows) {
		return {
			summary: url("/api/summary", { since, repo: repoPath }),
			// omit_prompt: each job otherwise carries its full prompt (~60 KB a job).
			jobs: url("/api/jobs", { limit: rows, omit_prompt: "true", repo: repoPath }),
			daily: Array.from({ length: DAILY_DAYS }, (_, i) => url("/api/summary", { since: `${i + 1}d`, repo: repoPath })),
		};
	}

	/** What a job row shows: state name, chip text, labels and age. */
	function jobRow(job, now) {
		let state;
		switch (job.status) {
			case "running":
				state = "running";
				break;
			case "queued":
				state = "queued";
				break;
			case "failed":
				state = "error";
				break;
			case "canceled":
			case "skipped":
				state = "canceled";
				break;
			default:
				state = job.verdict === "P" ? "pass" : job.verdict === "F" ? "fail" : "done";
		}
		const chip = { pass: "PASS", fail: "FAIL", error: "ERROR", running: "RUN", queued: "WAIT", canceled: "SKIP", done: "DONE" }[state];
		const type = job.job_type && job.job_type !== "review" ? job.job_type : "";
		const subject = job.commit_subject || (job.git_ref ? job.git_ref.slice(0, 8) : "");
		let age;
		if (state === "running") age = span(now - time(job.started_at));
		else if (state === "queued") age = `${span(now - time(job.enqueued_at))} queued`;
		else age = `${span(now - time(job.finished_at || job.enqueued_at))} ago`;
		return {
			id: job.id,
			state,
			chip,
			repo: job.repo_name || "",
			subject: type ? `${type}${subject ? `: ${subject}` : ""}` : subject,
			branch: job.branch || "",
			agent: job.agent || "",
			age,
			closed: job.closed === true,
		};
	}

	/**
	 * Reviews per day, oldest first, from summaries over the last 1…N days. The daemon only
	 * counts back from now, so day i is the difference of the i-day and (i-1)-day totals.
	 * `scale` is the day's share of the busiest day (0–1), for bar heights.
	 */
	function daily(summaries) {
		if (!Array.isArray(summaries) || !summaries.length) return [];
		// A failed fetch (null) repeats the previous totals: that day shows empty rather than
		// inflating the next one.
		const counts = [];
		summaries.forEach((s, i) => {
			const v = s && s.verdicts;
			counts.push(v ? { total: v.total || 0, passed: v.passed || 0 } : i ? counts[i - 1] : { total: 0, passed: 0 });
		});
		const days = counts.map((c, i) => {
			const prev = i ? counts[i - 1] : { total: 0, passed: 0 };
			const total = Math.max(0, c.total - prev.total);
			const passed = Math.min(total, Math.max(0, c.passed - prev.passed));
			return { daysAgo: i, total, passed, failed: total - passed, rate: total ? passed / total : null };
		});
		const busiest = Math.max(1, ...days.map((d) => d.total));
		days.forEach((d) => (d.scale = d.total / busiest));
		return days.reverse();
	}

	function message(title, detail) {
		return { kind: "message", title, detail };
	}

	/**
	 * The whole widget view.
	 * `s` = { down, status, health, summary, jobs, daily, repoError, since, updatedAt }.
	 */
	function view(s, now) {
		if (!s || s.down) {
			return message("roborev isn't running", "Start the daemon with `roborev daemon start`; it serves 127.0.0.1:7373.");
		}
		if (!s.status) return message("Connecting to roborev…", "");
		if (s.repoError) return message(s.repoError, "Set Repository to a name or folder roborev reviews, or leave it empty for all.");

		const st = s.status;
		const sum = s.summary || {};
		const v = sum.verdicts || {};
		const total = v.total || 0;
		const openFails = Math.max(0, (v.failed || 0) - (v.addressed || 0));
		const health = s.health || {};

		let level = "ok";
		const banners = [];
		if (health.healthy === false) {
			level = "error";
			const bad = (health.components || []).filter((c) => !c.healthy).map((c) => c.name);
			banners.push(`Daemon unhealthy${bad.length ? `: ${bad.join(", ")}` : ""}`);
		}
		if (st.queue_paused) {
			if (level === "ok") level = "paused";
			banners.push("Queue paused — `roborev unpause` resumes it");
		}
		if (st.update_draining) {
			if (level === "ok") level = "paused";
			banners.push("Updating roborev…");
		}
		if (health.error_count_24h > 0) banners.push(`${health.error_count_24h} daemon errors in 24 h`);

		const cost = sum.cost || {};
		const dur = sum.duration || {};
		return {
			kind: "dash",
			level,
			banner: banners.join(" · "),
			windowLabel: WINDOWS[s.since] || WINDOWS["7d"],
			passRate: total ? v.pass_rate : null,
			passText: total ? pct(v.pass_rate) : "–",
			reviewed: total,
			passed: v.passed || 0,
			failed: v.failed || 0,
			openFails,
			queued: st.queued_jobs || 0,
			running: st.running_jobs || 0,
			workers: `${st.active_workers || 0}/${st.max_workers || 0}`,
			rows: (s.jobs || []).map((j) => jobRow(j, now)),
			repos: (sum.repos || [])
				.slice()
				.sort((a, b) => b.total - a.total)
				.map((r) => ({ name: r.name, total: r.total, passed: r.passed, failed: r.failed, open: Math.max(0, r.failed - r.addressed) })),
			agents: (sum.agents || []).map((a) => ({ name: a.agent, total: a.total, passText: a.total ? pct(a.pass_rate) : "–", median: secs(a.median_duration_secs) })),
			p50: secs(dur.review_p50_secs),
			p90: secs(dur.review_p90_secs),
			cost: cost.jobs_with_cost ? `${cost.complete ? "" : "≥ "}$${(cost.total_usd || 0).toFixed(2)}` : "–",
			daily: daily(s.daily),
			updated: s.updatedAt ? `${span(now - s.updatedAt)} ago` : "",
		};
	}

	root.RoborevModel = { span, secs, windowArg, resolveRepo, url, paths, jobRow, daily, view, DAILY_DAYS };
})(typeof window !== "undefined" ? window : globalThis);
