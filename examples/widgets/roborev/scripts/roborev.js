// Roborev widget: review queue, pass rate and latest verdicts from the local roborev daemon
// (https://github.com/kenn-io/roborev), read over its JSON API on 127.0.0.1:7373. The manifest's
// url permission routes these fetches through EdgeHost, so the daemon needs no CORS headers.
// Settings: repoFilter (a repo name, absolute path or "~/…"; empty = all), timeWindow (1d/7d/30d).
(function () {
	"use strict";

	const M = window.RoborevModel;
	const BASE = "http://127.0.0.1:7373";
	const $ = (id) => document.getElementById(id);
	const FAST_MS = 5 * 1000; // queue status
	const DOWN_MS = 10 * 1000; // retry while the daemon is unreachable
	const SLOW_MS = 30 * 1000; // summary + latest jobs (sooner when the queue changes)
	const HEALTH_MS = 30 * 1000;
	const DAILY_MS = 30 * 60 * 1000;
	const ROWS = 8;
	const RING = 2 * Math.PI * 52; // circumference of the pass-rate ring (r = 52)

	const st = {
		down: false,
		status: null,
		health: null,
		summary: null,
		jobs: null,
		daily: null,
		repos: null,
		repoError: null,
		since: "7d",
		updatedAt: 0,
	};
	let settingsKey = null;
	let queueKey = null;
	let lastSlow = 0;
	let lastHealth = 0;
	let lastDaily = 0;
	let slowBusy = false;
	let fastTimer = null;

	async function get(path) {
		const r = await fetch(BASE + path);
		if (r.ok === false) throw new Error(`HTTP ${r.status}`);
		return r.json();
	}

	const setting = (name) => (typeof window[name] === "undefined" ? "" : window[name]);
	// Only the L and XL slots (either orientation) show the 14-day chart, so only they fetch it.
	const wantsDaily = () => Math.max(window.innerWidth || 0, window.innerHeight || 0) >= 1500;

	function make(tag, className, text) {
		const e = document.createElement(tag);
		if (className) e.className = className;
		if (text !== undefined) e.textContent = text;
		return e;
	}

	function jobEl(r) {
		const row = make("div", `job state-${r.state}${r.closed ? " closed" : ""}`);
		row.append(make("span", "chip", r.chip), make("span", "repo", r.repo), make("span", "subject", r.subject), make("span", "age", r.age));
		return row;
	}

	function repoEl(r) {
		const row = make("div", "repo-row");
		const bar = make("div", "bar");
		const pass = make("div", "pass");
		const fail = make("div", "fail");
		const sum = r.passed + r.failed || 1;
		pass.style.width = `${((r.passed / sum) * 100).toFixed(1)}%`;
		fail.style.width = `${((r.failed / sum) * 100).toFixed(1)}%`;
		bar.append(pass, fail);
		row.append(make("span", "name", r.name), bar, make("span", "count", `${r.passed}/${r.total}`));
		return row;
	}

	function agentEl(a) {
		const row = make("div", "agent-row");
		row.append(make("span", "name", a.name), make("span", "n", String(a.total)), make("span", "n", a.passText), make("span", "n", a.median));
		return row;
	}

	function dayEl(d) {
		const col = make("div", `day${d.total ? "" : " empty"}`);
		const stack = make("div", "stack");
		stack.style.height = `${(d.scale * 100).toFixed(1)}%`;
		const fail = make("div", "fail");
		const pass = make("div", "pass");
		fail.style.flexGrow = String(d.failed);
		pass.style.flexGrow = String(d.passed);
		stack.append(fail, pass);
		col.append(make("span", "n", d.total ? String(d.total) : ""), stack);
		return col;
	}

	function render() {
		const v = M.view(st, Date.now());
		const isDash = v.kind === "dash";
		$("dash").hidden = !isDash;
		$("message").hidden = isDash;
		if (!isDash) {
			$("message-title").textContent = v.title;
			$("message-detail").textContent = v.detail;
			return;
		}
		$("dash").dataset.level = v.level;
		$("ring-fill").style.strokeDasharray = `${v.passRate === null ? 0 : (v.passRate * RING).toFixed(1)} ${RING.toFixed(1)}`;
		$("rate").textContent = v.passText;
		$("rate-label").textContent = `pass · ${v.windowLabel}`;
		$("reviewed").textContent = `${v.passed} passed · ${v.failed} failed`;
		$("queued").textContent = String(v.queued);
		$("running").textContent = String(v.running);
		$("workers").textContent = v.workers;
		$("fails").textContent = String(v.openFails);
		$("fails-tile").classList.toggle("alert", v.openFails > 0);
		$("updated").textContent = v.updated;
		$("banner").textContent = v.banner;
		$("banner").hidden = !v.banner;
		$("jobs").replaceChildren(...(v.rows.length ? v.rows.map(jobEl) : [make("div", "empty", "No reviews yet")]));
		$("repos").replaceChildren(...v.repos.slice(0, 6).map(repoEl));
		$("p50").textContent = v.p50;
		$("p90").textContent = v.p90;
		$("cost").textContent = v.cost;
		$("agents").replaceChildren(...v.agents.slice(0, 4).map(agentEl));
		$("daily").replaceChildren(...v.daily.map(dayEl));
	}

	/** Summary and latest jobs; also resolves the repository setting (by name needs /api/repos). */
	async function refreshSlow() {
		if (slowBusy) return;
		slowBusy = true;
		try {
			const filter = setting("repoFilter");
			let repoPath = null;
			if (String(filter).trim()) {
				if (!st.repos && !String(filter).trim().startsWith("/")) st.repos = (await get("/api/repos")).repos || [];
				const r = M.resolveRepo(filter, st.repos);
				st.repoError = r.error || null;
				repoPath = r.path || null;
				if (r.error) return;
			} else {
				st.repoError = null;
			}
			const p = M.paths(st.since, repoPath, ROWS);
			const [summary, jobs] = await Promise.all([get(p.summary), get(p.jobs)]);
			st.summary = summary;
			st.jobs = (jobs && jobs.jobs) || [];
			st.updatedAt = Date.now();
			lastSlow = Date.now();
			if (wantsDaily() && Date.now() - lastDaily > DAILY_MS) {
				lastDaily = Date.now();
				st.daily = await Promise.all(p.daily.map((d) => get(d).catch(() => null)));
			}
		} catch (e) {
			console.warn("roborev: refresh failed", e);
		} finally {
			slowBusy = false;
			render();
		}
	}

	/** Queue status every few seconds; refreshes the rest when the queue changes or is due. */
	async function tick() {
		let next = FAST_MS;
		try {
			const status = await get("/api/status");
			const wasDown = st.down;
			st.status = status;
			st.down = false;
			if (Date.now() - lastHealth > HEALTH_MS) {
				lastHealth = Date.now();
				st.health = await get("/api/health").catch(() => st.health);
			}
			const key = [status.queued_jobs, status.running_jobs, status.completed_jobs, status.failed_jobs, status.canceled_jobs].join(",");
			const changed = key !== queueKey;
			queueKey = key;
			if (wasDown || changed || Date.now() - lastSlow > SLOW_MS) await refreshSlow();
			else render();
		} catch (_e) {
			st.down = true;
			next = DOWN_MS;
			render();
		}
		fastTimer = setTimeout(tick, next);
	}

	function applyColors() {
		const root = document.documentElement.style;
		const set = (name, v) => {
			if (typeof v !== "undefined" && v) root.setProperty(name, v);
		};
		set("--fg", window.textColor);
		set("--accent", window.accentColor);
		set("--pass", window.passColor);
		set("--fail", window.failColor);
		set("--bg", window.backgroundColor);
	}

	// Boot calls onICUEInitialized then onDataUpdated; refetch only when a data setting changes.
	function onSettings() {
		applyColors();
		const since = M.windowArg(setting("timeWindow"));
		const key = `${since}|${String(setting("repoFilter")).trim()}`;
		if (key === settingsKey) return;
		const first = settingsKey === null;
		settingsKey = key;
		st.since = since;
		st.summary = null;
		st.jobs = null;
		st.daily = null;
		st.repos = null;
		lastDaily = 0;
		if (first) {
			render();
			tick();
		} else {
			refreshSlow();
		}
	}

	window.icueEvents = { onICUEInitialized: onSettings, onDataUpdated: onSettings };
	// Keep ages ("3m ago", running time) current between fetches.
	setInterval(() => {
		if (st.status && !st.down) render();
	}, SLOW_MS);
	window.addEventListener("resize", () => {
		if (wantsDaily() && !st.daily) refreshSlow();
	});
	// For tests: stop the poll loop.
	window.__roborevStop = () => clearTimeout(fastTimer);
})();
