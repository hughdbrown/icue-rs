// Tests for the LLM Usage widget: the view model and the plugin wiring.
// Run with: node --test examples/widgets/llm-usage/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const MODEL = readFileSync(new URL("./scripts/llm-model.js", import.meta.url), "utf8");
const WIDGET = readFileSync(new URL("./scripts/llm-usage.js", import.meta.url), "utf8");
const plain = (v) => JSON.parse(JSON.stringify(v));

const model = () => {
	const ctx = {};
	vm.createContext(ctx);
	vm.runInContext(MODEL, ctx);
	return ctx.LlmModel;
};

const NOW = Date.parse("2026-10-16T12:00:00Z");
const H = 3600 * 1000;
const iso = (ms) => new Date(ms).toISOString();
const win = (label, used, resetIn, lengthSecs, extra = {}) => ({
	label,
	used,
	resetsAt: resetIn === null ? null : iso(NOW + resetIn),
	lengthSecs,
	...extra,
});
const FIVE = 5 * 3600;
const WEEK = 7 * 86400;

const snapshot = (subs, suggestion = null) => ({
	generatedAt: iso(NOW - 2 * 60 * 1000),
	configPath: "/x/llm-usage.toml",
	configError: null,
	pollSecs: 300,
	subscriptions: subs,
	suggestion,
});
const sub = (id, windows, more = {}) => ({
	id,
	provider: id,
	label: id[0].toUpperCase() + id.slice(1),
	plan: null,
	detail: null,
	status: "ok",
	message: null,
	windows,
	fetchedAt: iso(NOW),
	...more,
});

test("span and elapsed", () => {
	const M = model();
	assert.equal(M.span(45 * 1000), "45s");
	assert.equal(M.span(12 * 60 * 1000), "12m");
	assert.equal(M.span(2 * H + 13 * 60 * 1000), "2h 13m");
	assert.equal(M.span(4 * 24 * H + 6 * H), "4d 6h");
	assert.equal(M.span(-5), "0s");
	assert.equal(M.elapsed(win("5 h", 0.5, 2.5 * H, FIVE), NOW), 0.5);
	assert.equal(M.elapsed(win("5 h", 0.5, 10 * H, FIVE), NOW), 0);
	assert.equal(M.elapsed(win("5 h", 0.5, -H, FIVE), NOW), 1);
	assert.equal(M.elapsed(win("Month", 0.5, H, null), NOW), null);
});

test("pace states, run-out and waste", () => {
	const M = model();
	assert.equal(M.state(win("5 h", 1, H, FIVE), NOW), "out");
	assert.equal(M.state(win("5 h", 0, null, FIVE), NOW), "idle");
	assert.equal(M.state(win("5 h", 0.8, 2.5 * H, FIVE), NOW), "ahead");
	assert.equal(M.state(win("5 h", 0.5, 2.5 * H, FIVE), NOW), "on");
	assert.equal(M.state(win("5 h", 0.1, H, FIVE), NOW), "behind");
	assert.equal(M.state(win("5 h", 0, 4.5 * H, FIVE), NOW), "on", "too early to tell");
	// 80% used halfway: the last 20% goes in 37.5 minutes.
	assert.equal(M.runsOutAt(win("5 h", 0.8, 2.5 * H, FIVE), NOW).getTime(), NOW + 37.5 * 60 * 1000);
	assert.equal(M.runsOutAt(win("5 h", 0.3, 2.5 * H, FIVE), NOW), null);
	// 10% used with 80% of the time gone: 87.5% will be left over.
	assert.equal(M.projectedWaste(win("5 h", 0.1, H, FIVE), NOW), 0.875);
});

test("the most urgent window is the expiring week, capped by longer windows", () => {
	const M = model();
	const windows = [win("5 h", 0.14, 2.37 * H, FIVE), win("Week", 0.38, 21.87 * H, WEEK)];
	const best = M.mostUrgent(windows, NOW);
	assert.equal(best.w.label, "Week");
	assert.ok(Math.abs(best.ratio - 0.62 / (21.87 / 168)) < 1e-3, String(best.ratio));
	const capped = M.mostUrgent([win("5 h", 0.1, 0.5 * H, FIVE), win("Week", 0.95, 60 * H, WEEK)], NOW);
	assert.ok(Math.abs(capped.usable - 0.05) < 1e-9);
});

test("view: setup messages", () => {
	const M = model();
	assert.equal(M.view(null, NOW).title, "Fetching usage…");
	const err = M.view({ ...snapshot([]), configError: "x does not exist; create it with `edgectl llm-usage init`" }, NOW);
	assert.equal(err.kind, "message");
	assert.match(err.detail, /edgectl llm-usage init/);
	assert.match(M.view(snapshot([]), NOW).title, /No subscriptions/);
});

test("view: cards, suggestion and notes", () => {
	const M = model();
	const snap = snapshot(
		[
			sub("claude", [win("5 h", 0.14, 2.37 * H, FIVE), win("Week", 0.38, 21.87 * H, WEEK)], { plan: "Max" }),
			sub("codex", [win("5 h", 0.9, 2.5 * H, FIVE), win("Week", 0.2, 6 * 24 * H, WEEK)], { plan: "Plus" }),
			sub("kimi", [win("5 h", 1, 1.5 * H, FIVE)]),
			sub("ollama", [win("Month", 0.275, 10 * 24 * H, 30 * 86400, { amount: { used: 27.5, limit: 100, unit: "USD" } })], {
				status: "stale",
				message: "rate limited; retrying in 60 s",
				detail: "+$25.00 purchased",
			}),
			sub("gemini", [], { status: "error", message: "the Gemini cookie is signed out" }),
		],
		{ id: "claude", urgency: 4.76 },
	);
	const v = plain(M.view(snap, NOW));
	assert.equal(v.kind, "list");
	assert.equal(v.suggestion, "Use Claude next: 62% of its week expires in 21h 52m");
	assert.match(v.updated, /^Updated /);
	const [claude, codex, kimi, ollama, gemini] = v.subs;
	assert.equal(claude.suggested, true);
	assert.equal(claude.plan, "Max");
	assert.equal(claude.windows[0].left, "86% left");
	assert.match(claude.windows[0].reset, /^2h 22m · \d\d:\d\d$/);
	assert.equal(claude.note.kind, "behind");
	assert.match(claude.note.text, /^Week: ≈ \d+% will go unused at this pace$/);
	assert.equal(codex.windows[0].state, "ahead");
	assert.match(codex.note.text, /^5 h: runs out ≈ /);
	assert.equal(kimi.note.kind, "out");
	assert.match(kimi.note.text, /^Used up · back in 1h 30m/);
	assert.equal(ollama.windows[0].left, "$72.50 left");
	assert.equal(ollama.status, "stale");
	assert.equal(ollama.detail, "+$25.00 purchased");
	assert.equal(gemini.status, "error");
	assert.equal(gemini.note.text, "");
	assert.equal(M.view(snapshot([sub("kimi", [win("5 h", 1, H, FIVE)])]), NOW).suggestion, "Every subscription is used up for now");
});

// ── Widget wiring with a fake DOM and a fake LlmUsage plugin ──

function widgetEnv({ withPlugin = true, initial = null } = {}) {
	const elements = new Map();
	const listeners = {};
	const node = (id) => {
		const classes = new Set();
		return {
			id,
			hidden: false,
			textContent: "",
			className: "",
			style: {},
			children: [],
			classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), has: (c) => classes.has(c) },
			addEventListener: (type, fn) => (listeners[`${id}:${type}`] = fn),
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
	const calls = [];
	const handlers = {};
	const ctx = {
		document: {
			getElementById: el,
			createElement: (tag) => node(tag),
			documentElement: { style: { setProperty() {} } },
		},
		setTimeout: () => 0,
		setInterval() {},
		Date,
		Promise,
		plugins: withPlugin
			? {
					Llmusage: {
						snapshot: () => (calls.push("snapshot"), Promise.resolve(initial)),
						refresh: () => calls.push("refresh"),
						updated: { connect: (fn) => (handlers.updated = fn), disconnect() {} },
					},
				}
			: {},
	};
	ctx.window = ctx;
	vm.createContext(ctx);
	vm.runInContext(MODEL, ctx);
	vm.runInContext(WIDGET, ctx);
	return { ctx, calls, handlers, el, listeners };
}

const text = (n) => (n.children.length ? n.children.map(text).join("|") : n.textContent);

test("widget renders updates and refreshes on tap", async () => {
	const live = Date.now();
	const snap = {
		...snapshot([sub("claude", [{ label: "5 h", used: 0.25, resetsAt: new Date(live + H + 30000).toISOString(), lengthSecs: FIVE }])], {
			id: "claude",
			urgency: 1,
		}),
		generatedAt: new Date(live).toISOString(),
	};
	const { ctx, calls, handlers, el, listeners } = widgetEnv({ initial: snap });
	ctx.pluginLlmusageEvents.onInitialized();
	ctx.icueEvents.onICUEInitialized();
	await new Promise((r) => setImmediate(r));
	assert.deepEqual(calls, ["snapshot"]);
	assert.equal(el("usage").hidden, false);
	assert.equal(el("message").hidden, true);
	assert.match(el("suggestion").textContent, /^Use Claude next/);
	const card = el("subs").children[0];
	assert.match(card.className, /suggested/);
	assert.match(text(card), /Claude\|Use next\|5 h\|\|\|75% left\|1h 0m · /);

	handlers.updated({ ...snap, configError: "missing file", subscriptions: [] });
	assert.equal(el("usage").hidden, true);
	assert.equal(el("message-title").textContent, "Set up your subscriptions");

	listeners["refresh:click"]();
	assert.deepEqual(calls, ["snapshot", "refresh"]);
});

test("without the plugin the widget says so", () => {
	const { ctx, el } = widgetEnv({ withPlugin: false });
	ctx.icueEvents.onICUEInitialized();
	assert.equal(el("message-title").textContent, "LlmUsage plugin missing");
});
