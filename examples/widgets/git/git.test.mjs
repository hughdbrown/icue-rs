// Tests for the Git widget. Run with: node --test examples/widgets/git/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("./scripts/git.js", import.meta.url), "utf8");
const NOW = Math.floor(Date.now() / 1000);

const STATUS = {
	root: "/Users/me/projects/app",
	name: "app",
	branch: "main",
	detached: false,
	head: { hash: "9f2c", short: "9f2c1ab", subject: "Fix tests", author: "Me", time: NOW - 300 },
	upstream: "origin/main",
	ahead: 2,
	behind: 1,
	staged: 1,
	modified: 3,
	untracked: 0,
	conflicted: 0,
};
const LOG = {
	commits: [
		{ hash: "9f2c0000", short: "9f2c1ab", author: "Me", time: NOW - 300, subject: "Fix tests", refs: ["HEAD -> main", "origin/main"] },
		{ hash: "8e1d0000", short: "8e1d2cd", author: "You", time: NOW - 3 * 3600, subject: "Add widget", refs: [] },
		{ hash: "7d0c0000", short: "7d0c3ef", author: "Me", time: NOW - 2 * 86400, subject: "Initial", refs: [] },
	],
};
const SHOW = {
	hash: "9f2c0000",
	short: "9f2c1ab",
	author: "Me",
	time: NOW - 300,
	subject: "Fix tests",
	body: "Longer description\nsecond line",
	files: [
		{ path: "src/lib.rs", additions: 12, deletions: 3 },
		{ path: "logo.png", additions: null, deletions: null },
	],
};
const TERMINAL = { cwd: "/Users/me/projects/app/src", repo: "/Users/me/projects/app", updated: NOW };

function makeEnv({ routes, props = {} } = {}) {
	const byId = new Map();
	const storage = new Map();
	const fetchCalls = [];
	const timers = [];

	function makeElement(tag, id) {
		const listeners = {};
		const children = [];
		const attrs = {};
		const el = {
			tagName: tag.toUpperCase(),
			id: id || "",
			className: "",
			textContent: "",
			hidden: false,
			type: "",
			_attrs: attrs,
			style: { _p: {}, setProperty(k, v) { this._p[k] = String(v); } },
			get children() {
				return children;
			},
			append(...nodes) {
				for (const n of nodes) if (n != null) children.push(n);
			},
			replaceChildren(...nodes) {
				children.length = 0;
				el.append(...nodes);
			},
			setAttribute(k, v) {
				attrs[k] = String(v);
			},
			getAttribute(k) {
				return k in attrs ? attrs[k] : null;
			},
			addEventListener(type, fn) {
				(listeners[type] ||= []).push(fn);
			},
			click() {
				for (const fn of listeners.click || []) fn({});
			},
		};
		return el;
	}
	const text = (n) => [n.textContent, ...n.children.map(text)].join(" ");
	const all = (n) => n.children.flatMap((c) => [c, ...all(c)]);
	const byClass = (n, cls) => all(n).filter((c) => c.className.split(/\s+/).includes(cls));

	const document = {
		visibilityState: "visible",
		createElement: (t) => makeElement(t),
		getElementById(id) {
			if (!byId.has(id)) byId.set(id, makeElement("div", id));
			return byId.get(id);
		},
		documentElement: makeElement("html"),
		addEventListener() {},
	};
	const defaultRoutes = {
		"/__edgehost/git/status": () => ({ body: STATUS }),
		"/__edgehost/git/log": () => ({ body: LOG }),
		"/__edgehost/git/show": () => ({ body: SHOW }),
		"/__edgehost/git/terminal": () => ({ body: TERMINAL }),
	};
	const table = { ...defaultRoutes, ...(routes || {}) };
	const fetch = async (url) => {
		fetchCalls.push(String(url));
		const u = new URL(String(url), "http://x");
		const route = table[u.pathname];
		if (!route) return { ok: false, status: 404, text: async () => "", json: async () => ({}) };
		const r = route(u);
		const status = r.status || 200;
		return { ok: status < 400, status, text: async () => r.text || "", json: async () => r.body };
	};
	const context = {
		console,
		setInterval: (fn, ms) => {
			const id = setInterval(fn, ms);
			timers.push(id);
			return id;
		},
		clearInterval,
		setTimeout: (fn, ms) => {
			const id = setTimeout(fn, ms);
			timers.push(id);
			return id;
		},
		clearTimeout,
		localStorage: {
			getItem: (k) => (storage.has(k) ? storage.get(k) : null),
			setItem: (k, v) => storage.set(k, String(v)),
		},
		fetch,
		document,
		addEventListener() {},
		uniqueId: "u-1",
		repos: "/Users/me/projects/app",
		followTerminal: true,
		commitCount: 20,
		refreshSeconds: 15,
		showAuthor: true,
		iCUE_initialized: false,
		...props,
	};
	context.window = context;
	vm.createContext(context);
	vm.runInContext(SOURCE, context);
	return {
		context,
		byId,
		fetchCalls,
		storage,
		text,
		byClass,
		all,
		clear: () => timers.forEach((t) => (clearInterval(t), clearTimeout(t))),
	};
}

const settle = async () => {
	for (let i = 0; i < 20; i++) await new Promise((r) => setImmediate(r));
};

async function withEnv(opts, fn) {
	const env = makeEnv(opts);
	try {
		await fn(env);
	} finally {
		env.clear();
	}
}

test("parseRepoList trims, splits, dedupes, drops empties", () => {
	const { context: c } = makeEnv();
	assert.deepEqual(Array.from(c.parseRepoList(" a , b\n\nc,,a \r\n")), ["a", "b", "c"]);
	assert.deepEqual(Array.from(c.parseRepoList("")), []);
	assert.deepEqual(Array.from(c.parseRepoList(null)), []);
});

test("basename", () => {
	const { context: c } = makeEnv();
	assert.equal(c.basename("/Users/me/app"), "app");
	assert.equal(c.basename("/Users/me/app/"), "app");
	assert.equal(c.basename("C:\\x\\proj"), "proj");
	assert.equal(c.basename("~"), "~");
	assert.equal(c.basename("/"), "/");
});

test("relativeTime", () => {
	const { context: c } = makeEnv();
	const now = NOW;
	assert.equal(c.relativeTime(now - 10, now), "now");
	assert.equal(c.relativeTime(now - 300, now), "5 m");
	assert.equal(c.relativeTime(now - 3 * 3600, now), "3 h");
	assert.equal(c.relativeTime(now - 2 * 86400, now), "2 d");
	assert.match(c.relativeTime(now - 90 * 86400, now), /^\d{4}-\d{2}-\d{2}$/);
});

test("formatAheadBehind", () => {
	const { context: c } = makeEnv();
	assert.equal(c.formatAheadBehind(2, 1, "origin/main"), "↑2 ↓1");
	assert.equal(c.formatAheadBehind(0, 3, "origin/main"), "↓3");
	assert.equal(c.formatAheadBehind(0, 0, "origin/main"), "in sync");
	assert.equal(c.formatAheadBehind(2, 1, null), "");
});

test("describeError", () => {
	const { context: c } = makeEnv();
	assert.match(c.describeError(403, "forbidden"), /not permitted/);
	assert.match(c.describeError(403, ""), /folder permissions/);
	assert.equal(c.describeError(502, "git error: boom"), "git error: boom");
	assert.equal(c.describeError(502, "boom"), "git error: boom");
	assert.match(c.describeError(0, ""), /Needs EdgeHost's git data source/);
	assert.match(c.describeError(404, "<html>"), /Needs EdgeHost's git data source/);
	assert.equal(c.describeError(404, "repository not found"), "repository not found");
	assert.equal(c.describeError(400, "bad rev"), "bad rev");
});

test("cleanRefs", () => {
	const { context: c } = makeEnv();
	assert.deepEqual(Array.from(c.cleanRefs(["HEAD -> main", "origin/main", "main", "", "tag: v1"])), ["main", "origin/main", "tag: v1"]);
	assert.deepEqual(Array.from(c.cleanRefs(undefined)), []);
});

test("renders header and commits, and tapping a commit shows files", async () => {
	await withEnv({}, async (env) => {
		env.context.icueEvents.onICUEInitialized();
		await settle();
		const header = env.text(env.byId.get("header"));
		assert.match(header, /app/);
		assert.match(header, /main/);
		assert.match(header, /↑2 ↓1/);
		assert.match(header, /1 staged/);
		assert.match(header, /3 modified/);
		assert.doesNotMatch(header, /untracked|clean/);
		const chips = env.byId.get("chips").children;
		assert.equal(chips.length, 2);
		assert.match(chips[0].textContent, /Terminal: app/);

		const rows = env.byClass(env.byId.get("commits"), "commit");
		assert.equal(rows.length, 3);
		assert.match(env.text(rows[0]), /Fix tests/);
		assert.match(env.text(rows[0]), /5 m/);
		assert.match(env.text(rows[0]), /Me/);
		assert.equal(env.byClass(rows[0], "tag").map((t) => t.textContent).join(","), "main,origin/main");

		rows[0].click();
		await settle();
		assert.ok(env.fetchCalls.some((u) => u.startsWith("/__edgehost/git/show?") && u.includes("rev=9f2c0000")));
		const files = env.byClass(env.byId.get("detail"), "file");
		assert.equal(files.length, 2);
		assert.match(env.text(files[0]), /src\/lib\.rs/);
		assert.match(env.text(files[0]), /\+12/);
		assert.match(env.text(files[0]), /−3/);
		assert.match(env.text(files[1]), /binary/);
		assert.match(env.text(env.byId.get("detail")), /Longer description/);

		// Back returns to the list.
		env.byClass(env.byId.get("detail"), "back")[0].click();
		assert.match(env.byId.get("root").className, /narrow/);
		assert.doesNotMatch(env.byId.get("root").className, /detail-open/);
	});
});

test("showAuthor false hides authors; selection persists", async () => {
	await withEnv({ props: { showAuthor: false, repos: "/a/one, /b/two" } }, async (env) => {
		env.context.icueEvents.onICUEInitialized();
		await settle();
		assert.equal(env.byClass(env.byId.get("commits"), "author").length, 0);
		const chips = env.byId.get("chips").children;
		assert.equal(chips.length, 3);
		chips[2].click();
		await settle();
		assert.equal(JSON.parse(env.storage.get("u-1")).selection, "/b/two");
		assert.ok(env.fetchCalls.some((u) => u.includes("status?repo=%2Fb%2Ftwo")));
	});
});

test("403 shows the permission message", async () => {
	await withEnv(
		{ routes: { "/__edgehost/git/status": () => ({ status: 403, text: "forbidden" }), "/__edgehost/git/log": () => ({ status: 403, text: "forbidden" }) } },
		async (env) => {
			env.context.icueEvents.onICUEInitialized();
			await settle();
			const banner = env.byId.get("banner");
			assert.equal(banner.hidden, false);
			assert.match(banner.textContent, /This folder is not permitted — add it to the widget's folder permissions/);
		},
	);
});

test("missing routes show the EdgeHost-only message", async () => {
	await withEnv({ props: { followTerminal: false }, routes: { "/__edgehost/git/status": () => ({ status: 404 }), "/__edgehost/git/log": () => ({ status: 404 }) } }, async (env) => {
		env.context.icueEvents.onICUEInitialized();
		await settle();
		assert.match(env.byId.get("banner").textContent, /Needs EdgeHost's git data source/);
	});
});

test("terminal with null cwd explains the shell hook", async () => {
	await withEnv({ routes: { "/__edgehost/git/terminal": () => ({ body: { cwd: null, repo: null, updated: 0 } }) } }, async (env) => {
		env.context.icueEvents.onICUEInitialized();
		await settle();
		// Without a terminal repo the widget starts on the first configured repo; pick Terminal.
		env.byId.get("chips").children[0].click();
		await settle();
		assert.match(env.text(env.byId.get("commits")), /shell hook.*docs\/data-sources\.md/);
		assert.match(env.byId.get("chips").children[0].textContent, /no repo/);
	});
});

test("errors after a successful load keep the last data and mark it stale", async () => {
	let fail = false;
	const routes = {
		"/__edgehost/git/status": () => (fail ? { status: 502, text: "git error: lock" } : { body: STATUS }),
		"/__edgehost/git/log": () => ({ body: LOG }),
	};
	await withEnv({ routes }, async (env) => {
		env.context.icueEvents.onICUEInitialized();
		await settle();
		fail = true;
		env.context.icueEvents.onDataUpdated();
		await settle();
		assert.equal(env.byClass(env.byId.get("commits"), "commit").length, 3);
		assert.match(env.text(env.byId.get("header")), /stale/);
		assert.match(env.byId.get("banner").textContent, /git error: lock/);
	});
});

test("never more than one refresh in flight", async () => {
	let inflight = 0;
	let maxInflight = 0;
	const slow = (body) => async () => body;
	const routes = {};
	for (const [p, body] of [["status", STATUS], ["log", LOG]]) routes[`/__edgehost/git/${p}`] = slow(body);
	await withEnv({ props: { followTerminal: false } }, async (env) => {
		const realFetch = env.context.fetch;
		env.context.fetch = async (u) => {
			inflight++;
			maxInflight = Math.max(maxInflight, inflight);
			await new Promise((r) => setImmediate(r));
			inflight--;
			return realFetch(u);
		};
		env.context.icueEvents.onICUEInitialized();
		env.context.icueEvents.onDataUpdated();
		env.context.icueEvents.onDataUpdated();
		await settle();
		assert.ok(maxInflight <= 2, `status+log only, saw ${maxInflight}`);
	});
});

test("manifest-declared properties are all defined and files exist", () => {
	const html = readFileSync(new URL("./index.html", import.meta.url), "utf8");
	for (const p of ["repos", "followTerminal", "commitCount", "refreshSeconds", "showAuthor", "accentColor", "backgroundColor", "textColor"]) {
		assert.ok(html.includes(`content="${p}"`), p);
	}
});
