// Tests for the Roborev widget: the view model and the polling/render wiring.
// Fixtures are trimmed captures from a live roborev v0.71.0 daemon (prompts and ids removed).
// Run with: node --test examples/widgets/roborev/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const MODEL = readFileSync(new URL("./scripts/roborev-model.js", import.meta.url), "utf8");
const WIDGET = readFileSync(new URL("./scripts/roborev.js", import.meta.url), "utf8");
const FX = JSON.parse(readFileSync(new URL("./fixtures.json", import.meta.url), "utf8"));
const plain = (v) => JSON.parse(JSON.stringify(v));

const model = () => {
	const ctx = {};
	vm.createContext(ctx);
	vm.runInContext(MODEL, ctx);
	return ctx.RoborevModel;
};

const NOW = Date.parse("2026-10-17T12:00:00Z");
const S = 1000;
const iso = (ms) => new Date(ms).toISOString();
const job = (more = {}) => ({
	id: 1,
	repo_name: "icue-rs",
	commit_subject: "Add the roborev widget",
	git_ref: "99dd1c478347310aba48588542c35953bf42f347",
	branch: "main",
	agent: "claude-code",
	job_type: "review",
	status: "done",
	verdict: "P",
	enqueued_at: iso(NOW - 10 * 60 * S),
	started_at: iso(NOW - 10 * 60 * S),
	finished_at: iso(NOW - 9 * 60 * S),
	...more,
});

test("span, secs and windowArg", () => {
	const M = model();
	assert.equal(M.span(45 * S), "45s");
	assert.equal(M.span(12 * 60 * S), "12m");
	assert.equal(M.span(26 * 3600 * S), "1d 2h");
	assert.equal(M.secs(0.688), "0.7s");
	assert.equal(M.secs(37.67), "38s");
	assert.equal(M.secs(78.65), "1m 19s");
	assert.equal(M.secs(938.7), "15m");
	assert.equal(M.secs(0), "–");
	assert.equal(M.windowArg("30d"), "30d");
	assert.equal(M.windowArg(""), "7d");
	assert.equal(M.windowArg("bogus"), "7d");
});

test("resolveRepo accepts a name, an absolute path or ~/…", () => {
	const M = model();
	const repos = [
		{ name: "alpha", root_path: "/Users/dev/src/alpha" },
		{ name: "Beta-Service", root_path: "/Users/dev/work/beta-service" },
	];
	assert.deepEqual(plain(M.resolveRepo("", repos)), { path: null });
	assert.deepEqual(plain(M.resolveRepo("beta-service", repos)), { path: "/Users/dev/work/beta-service" });
	assert.deepEqual(plain(M.resolveRepo("/some/where/", repos)), { path: "/some/where" });
	assert.deepEqual(plain(M.resolveRepo("~/src/alpha", repos)), { path: "/Users/dev/src/alpha" });
	assert.match(M.resolveRepo("nope", repos).error, /named "nope"/);
	assert.match(M.resolveRepo("~/nope", []).error, /at ~\/nope/);
});

test("paths always omit prompts and encode the repo", () => {
	const M = model();
	const p = M.paths("7d", "/Users/dev/my repo", 8);
	assert.equal(p.summary, "/api/summary?since=7d&repo=%2FUsers%2Fdev%2Fmy%20repo");
	assert.equal(p.jobs, "/api/jobs?limit=8&omit_prompt=true&repo=%2FUsers%2Fdev%2Fmy%20repo");
	assert.equal(p.daily.length, M.DAILY_DAYS);
	assert.equal(p.daily[0], "/api/summary?since=1d&repo=%2FUsers%2Fdev%2Fmy%20repo");
	assert.equal(M.paths("1d", null, 8).jobs, "/api/jobs?limit=8&omit_prompt=true");
});

test("jobRow maps statuses and verdicts to states", () => {
	const M = model();
	const r = (more) => plain(M.jobRow(job(more), NOW));
	assert.deepEqual(
		[r({}), r({ verdict: "F" }), r({ status: "failed", verdict: undefined }), r({ status: "canceled" }), r({ job_type: "insights", verdict: undefined })].map((x) => [x.state, x.chip]),
		[
			["pass", "PASS"],
			["fail", "FAIL"],
			["error", "ERROR"],
			["canceled", "SKIP"],
			["done", "DONE"],
		],
	);
	assert.equal(r({}).age, "9m ago");
	const running = r({ status: "running", verdict: undefined, started_at: iso(NOW - 75 * S), finished_at: undefined });
	assert.deepEqual([running.state, running.age], ["running", "1m"]);
	assert.equal(r({ status: "queued", enqueued_at: iso(NOW - 5 * S) }).age, "5s queued");
	assert.equal(r({ job_type: "compact", commit_subject: "" }).subject, "compact: 99dd1c47");
	assert.equal(r({ closed: true }).closed, true);
});

test("daily turns cumulative summaries into per-day counts, oldest first", () => {
	const M = model();
	const cum = (total, passed) => ({ verdicts: { total, passed } });
	const d = plain(M.daily([cum(2, 1), cum(2, 1), cum(6, 5), null]));
	assert.deepEqual(
		d.map((x) => [x.daysAgo, x.total, x.passed, x.failed, x.rate, x.scale]),
		[
			[3, 0, 0, 0, null, 0],
			[2, 4, 4, 0, 1, 1],
			[1, 0, 0, 0, null, 0],
			[0, 2, 1, 1, 0.5, 0.5],
		],
	);
	assert.deepEqual(plain(M.daily(undefined)), []);
	// A failed fetch in the middle doesn't inflate the following day.
	const gap = plain(M.daily([cum(2, 1), null, cum(5, 3)]));
	assert.deepEqual(
		gap.map((x) => x.total),
		[3, 0, 2],
	);
});

test("view from live fixtures", () => {
	const M = model();
	const v = plain(M.view({ status: FX.status, health: FX.health, summary: FX.summary7d, jobs: FX.jobs, since: "7d", updatedAt: NOW - 3 * S }, NOW));
	assert.equal(v.kind, "dash");
	assert.equal(v.level, "ok");
	assert.equal(v.banner, "");
	assert.equal(v.windowLabel, "7 days");
	const vd = FX.summary7d.verdicts;
	assert.equal(v.passText, `${Math.round(vd.pass_rate * 100)}%`);
	assert.equal(v.openFails, vd.failed - vd.addressed);
	assert.equal(v.workers, `${FX.status.active_workers}/${FX.status.max_workers}`);
	assert.equal(v.rows.length, FX.jobs.length);
	assert.ok(v.rows.every((r) => ["pass", "fail"].includes(r.state)));
	assert.equal(v.repos[0].name, FX.summary7d.repos[0].name);
	assert.match(v.cost, /^\$\d+\.\d\d$/);
	assert.equal(v.updated, "3s ago");
});

test("view handles an empty window, pauses, ill health and a down daemon", () => {
	const M = model();
	const empty = plain(M.view({ status: FX.status, summary: FX.summaryEmpty, jobs: [], since: "1d" }, NOW));
	assert.equal(empty.passText, "–");
	assert.equal(empty.passRate, null);
	assert.deepEqual(empty.repos, []);
	assert.equal(empty.cost, "–");
	assert.equal(empty.windowLabel, "24 h");

	const paused = plain(M.view({ status: { ...FX.status, queue_paused: true }, summary: FX.summary7d }, NOW));
	assert.equal(paused.level, "paused");
	assert.match(paused.banner, /Queue paused/);

	const sick = plain(
		M.view({ status: { ...FX.status, queue_paused: true }, health: { healthy: false, components: [{ name: "database", healthy: false }, { name: "workers", healthy: true }], error_count_24h: 3 } }, NOW),
	);
	assert.equal(sick.level, "error");
	assert.equal(sick.banner, "Daemon unhealthy: database · Queue paused — `roborev unpause` resumes it · 3 daemon errors in 24 h");

	assert.equal(M.view({ down: true }, NOW).title, "roborev isn't running");
	assert.equal(M.view({}, NOW).title, "Connecting to roborev…");
	assert.match(M.view({ status: FX.status, repoError: 'No roborev repository named "x"' }, NOW).title, /named "x"/);
});

// ── Widget wiring with a fake DOM, fake fetch and manual timers ──

function widgetEnv({ responses = {}, settings = {}, width = 840, height = 696 } = {}) {
	const elements = new Map();
	const node = (id) => {
		const classes = new Set();
		return {
			id,
			hidden: false,
			textContent: "",
			className: "",
			title: "",
			style: {},
			dataset: {},
			children: [],
			classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)), has: (c) => classes.has(c) },
			replaceChildren(...nodes) {
				this.children = nodes;
			},
			append(...nodes) {
				this.children.push(...nodes);
			},
		};
	};
	const el = (id) => {
		if (!elements.has(id)) elements.set(id, node(id));
		return elements.get(id);
	};
	const fetched = [];
	const timers = [];
	const ctx = {
		document: { getElementById: el, createElement: (tag) => node(tag), documentElement: { style: { setProperty() {} } } },
		fetch: async (url) => {
			const path = url.replace("http://127.0.0.1:7373", "");
			fetched.push(path);
			const route = path.split("?")[0];
			const body = typeof responses[path] !== "undefined" ? responses[path] : responses[route];
			if (body === undefined) throw new TypeError("Failed to fetch");
			return { ok: true, status: 200, json: async () => body };
		},
		setTimeout: (fn, ms) => (timers.push({ fn, ms }), timers.length),
		clearTimeout() {},
		setInterval() {},
		addEventListener() {},
		innerWidth: width,
		innerHeight: height,
		console: { warn() {} },
		Date,
		Promise,
		Math,
		...settings,
	};
	ctx.window = ctx;
	vm.createContext(ctx);
	vm.runInContext(MODEL, ctx);
	vm.runInContext(WIDGET, ctx);
	return { ctx, el, fetched, timers };
}

const settle = () => new Promise((r) => setTimeout(r, 0));
const text = (n) => (n.children.length ? n.children.map(text).join("|") : n.textContent);
const live = { "/api/status": FX.status, "/api/health": FX.health, "/api/summary": FX.summary7d, "/api/jobs": { jobs: FX.jobs }, "/api/repos": { repos: FX.repos } };

test("widget polls status, then summary and jobs, and renders the dashboard", async () => {
	const { ctx, el, fetched, timers } = widgetEnv({ responses: live });
	ctx.icueEvents.onICUEInitialized();
	ctx.icueEvents.onDataUpdated(); // boot calls both; the second must not start another loop
	await settle();
	assert.deepEqual(fetched, ["/api/status", "/api/health", "/api/summary?since=7d", "/api/jobs?limit=8&omit_prompt=true"]);
	assert.equal(el("dash").hidden, false);
	assert.equal(el("message").hidden, true);
	assert.equal(el("rate").textContent, `${Math.round(FX.summary7d.verdicts.pass_rate * 100)}%`);
	assert.equal(el("jobs").children.length, FX.jobs.length);
	assert.match(text(el("jobs").children[0]), new RegExp(`^PASS\\|${FX.jobs[0].repo_name}\\|`));
	assert.equal(timers.length, 1);
	assert.equal(timers[0].ms, 5000);

	// An unchanged queue only refetches status.
	fetched.length = 0;
	timers[0].fn();
	await settle();
	assert.deepEqual(fetched, ["/api/status"]);

	// A finished job changes the counters, so summary and jobs refresh at once.
	ctx.window.__roborevStop();
	const { ctx: c2, fetched: f2, timers: t2 } = widgetEnv({ responses: live });
	c2.icueEvents.onICUEInitialized();
	await settle();
	f2.length = 0;
	live["/api/status"] = { ...FX.status, completed_jobs: FX.status.completed_jobs + 1 };
	t2[0].fn();
	await settle();
	live["/api/status"] = FX.status;
	assert.deepEqual(f2, ["/api/status", "/api/summary?since=7d", "/api/jobs?limit=8&omit_prompt=true"]);
});

test("widget shows the daemon as down and retries more slowly", async () => {
	const { ctx, el, timers } = widgetEnv({ responses: {} });
	ctx.icueEvents.onICUEInitialized();
	await settle();
	assert.equal(el("dash").hidden, true);
	assert.equal(el("message-title").textContent, "roborev isn't running");
	assert.equal(timers[0].ms, 10000);
});

test("widget resolves a repo name through /api/repos and filters by its path", async () => {
	const name = FX.repos[1].name;
	const path = FX.repos[1].root_path;
	const { ctx, fetched } = widgetEnv({ responses: live, settings: { repoFilter: name, timeWindow: "30d" } });
	ctx.icueEvents.onICUEInitialized();
	await settle();
	const enc = encodeURIComponent(path);
	assert.deepEqual(fetched.slice(2), ["/api/repos", `/api/summary?since=30d&repo=${enc}`, `/api/jobs?limit=8&omit_prompt=true&repo=${enc}`]);
});

test("widget reports an unknown repo instead of fetching", async () => {
	const { ctx, el, fetched } = widgetEnv({ responses: live, settings: { repoFilter: "no-such-repo" } });
	ctx.icueEvents.onICUEInitialized();
	await settle();
	assert.ok(!fetched.some((p) => p.startsWith("/api/summary")));
	assert.equal(el("message-title").textContent, 'No roborev repository named "no-such-repo"');
});

test("widget fetches the 14-day series only in the L and XL slots", async () => {
	const summaries = (f) => f.filter((p) => p.startsWith("/api/summary?since=")).length;
	for (const [width, height] of [
		[1688, 696],
		[2536, 696],
		[696, 1688],
	]) {
		const { ctx, el, fetched } = widgetEnv({ responses: live, width, height });
		ctx.icueEvents.onICUEInitialized();
		await settle();
		assert.equal(summaries(fetched), 1 + 14, `${width}×${height}`);
		assert.equal(el("daily").children.length, 14);
	}
	const { ctx, fetched } = widgetEnv({ responses: live, width: 840, height: 696 });
	ctx.icueEvents.onICUEInitialized();
	await settle();
	assert.equal(summaries(fetched), 1, "M fetches no series");
});
