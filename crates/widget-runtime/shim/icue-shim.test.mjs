// Run with: node --test crates/widget-runtime/shim/
// Executes the shim in a vm context with a minimal fake window/document.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Values created inside the vm context have their own Array prototype; compare plain copies.
const plain = (v) => JSON.parse(JSON.stringify(v));

const SOURCE = readFileSync(new URL("./icue-shim.js", import.meta.url), "utf8");

function run(payload, widgetScript = "") {
	const posted = [];
	const listeners = {};
	const context = {
		console: { log() {}, info() {}, warn() {}, error() {} },
		document: { readyState: "loading", addEventListener: (t, f) => (listeners[t] = f) },
		addEventListener: (t, f) => (listeners[`window:${t}`] = f),
		ipc: { postMessage: (m) => posted.push(JSON.parse(m)) },
	};
	context.window = context;
	vm.createContext(context);
	vm.runInContext(SOURCE.replace("__PAYLOAD__", JSON.stringify(payload)), context);
	vm.runInContext(widgetScript, context);
	return { context, posted, listeners };
}

const BASE = {
	mode: "runtime",
	values: { textColor: "#FFFFFF", size: 42, label: "Hallo" },
	lang: "de",
	fpsLimit: 30,
	preview: false,
	widgetId: "w-1",
	instanceId: "i-1",
	deviceId: "d-1",
	tempUnit: "°C",
	props: [{ name: "textColor" }, { name: "size" }, { name: "label" }, { name: "none" }],
	tr: { Hello: "Hallo" },
	modules: [],
	plugins: [{ name: "Sensorsdataprovider", available: false, methods: [], signals: [] }],
	proxy: [],
};

const EVALUATE = {
	...BASE,
	mode: "evaluate",
	values: null,
	props: [
		{ name: "textColor", defaultExpr: "'#FFFFFF'" },
		{ name: "size", defaultExpr: "40 + 2" },
		{ name: "label", defaultExpr: "tr('Hello')" },
		{ name: "fromModule", defaultExpr: "Later.value()" },
		{ name: "broken", defaultExpr: "plugins.Missing.getBlock()" },
		{ name: "none", defaultExpr: null },
	],
};

test("runtime globals exist before widget scripts run", () => {
	const { context } = run(BASE, "globalThis.seen = [textColor, size, label, uniqueId, device.deviceId];");
	assert.deepEqual(plain(context.seen), ["#FFFFFF", 42, "Hallo", "i-1", "d-1"]);
	assert.equal(context.iCUE.iCUELanguage, "de");
	assert.equal(context.iCUE.defaultTemperatureUnit(), "°C");
	assert.equal(context.iCUE_initialized, false);
	assert.equal(context.pluginSensorsdataprovider_initialized, false);
	assert.ok("none" in context);
	assert.equal(context.none, undefined);
});

test("evaluate mode posts concrete defaults and never runs lifecycle hooks", async () => {
	const { context, posted, listeners } = run(
		EVALUATE,
		`globalThis.Later = { value: () => 'late' };
		 globalThis.hooks = 0;
		 icueEvents = { onICUEInitialized: () => hooks++ };`,
	);
	listeners.DOMContentLoaded();
	await new Promise((r) => setImmediate(r));
	const evaluated = posted.find((m) => m.k === "evaluated");
	assert.deepEqual(evaluated.values, { textColor: "#FFFFFF", size: 42, label: "Hallo", fromModule: "late" });
	assert.equal(context.hooks, 0);
	assert.ok(!posted.some((m) => m.k === "booted"));
});

test("boot fires lifecycle hooks once, after DOMContentLoaded", async () => {
	const { context, posted, listeners } = run(
		BASE,
		`globalThis.calls = [];
		 icueEvents = { onICUEInitialized: () => calls.push("init:" + iCUE_initialized),
		                onDataUpdated: () => calls.push("data") };`,
	);
	assert.deepEqual(plain(context.calls), []);
	listeners.DOMContentLoaded();
	await context.__edgehost.boot(); // second call is a no-op
	await new Promise((r) => setImmediate(r));
	assert.deepEqual(plain(context.calls), ["init:true", "data"]);
	assert.ok(posted.some((m) => m.k === "booted"));
});

test("hooks declared with const are still found", async () => {
	const { context, listeners } = run(
		BASE,
		"globalThis.hit = 0; const icueEvents = { onICUEInitialized: () => { globalThis.hit++; } };",
	);
	listeners.DOMContentLoaded();
	await new Promise((r) => setImmediate(r));
	assert.equal(context.hit, 1);
});

test("console output is forwarded to the host", () => {
	const { context, posted } = run(BASE);
	context.console.error("boom", { a: 1 });
	assert.ok(posted.some((m) => m.k === "console" && m.level === "error" && m.msg === 'boom {"a":1}'));
});

test("Escape asks the host to close only when enabled", () => {
	const off = run(BASE);
	assert.equal(off.listeners["window:keydown"], undefined);
	const on = run({ ...BASE, escapeCloses: true });
	on.listeners["window:keydown"]({ key: "a" });
	on.listeners["window:keydown"]({ key: "Escape" });
	assert.deepEqual(plain(on.posted.filter((m) => m.k === "close")), [{ k: "close" }]);
});

test("function and promise defaults are resolved; search-combobox keeps the key", async () => {
	const payload = {
		...EVALUATE,
		props: [
			{ name: "city", kind: "search-combobox", defaultExpr: "Geo.getDefault" },
			{ name: "later", kind: "textfield", defaultExpr: "Promise.resolve('x')" },
		],
	};
	const { posted, listeners } = run(payload, "globalThis.Geo = { getDefault: () => Promise.resolve(['5368361', 'Los Angeles']) };");
	listeners.DOMContentLoaded();
	await new Promise((r) => setTimeout(r, 10));
	const evaluated = posted.find((m) => m.k === "evaluated");
	assert.deepEqual(evaluated.values, { city: "5368361", later: "x" });
});

test("fetch to declared domains is routed through the EdgeHost proxy", async () => {
	const calls = [];
	const payload = { ...BASE, proxy: [{ domain: "finance.yahoo.com", port: 443 }] };
	const posted = [];
	const context = {
		console: { log() {}, info() {}, warn() {}, error() {} },
		document: { readyState: "complete", addEventListener() {} },
		addEventListener() {},
		ipc: { postMessage: (m) => posted.push(JSON.parse(m)) },
		location: { href: "icuew://abc/index.html" },
		URL,
		fetch: (input, init) => {
			calls.push(String(input));
			return Promise.resolve("ok");
		},
	};
	context.window = context;
	vm.createContext(context);
	vm.runInContext(SOURCE.replace("__PAYLOAD__", JSON.stringify(payload)), context);
	await context.fetch("https://query1.finance.yahoo.com/v8/finance/chart/AAPL?range=1d");
	await context.fetch("https://example.com/data.json");
	await context.fetch("https://query1.finance.yahoo.com/x", { method: "POST" });
	assert.deepEqual(calls, [
		"/__edgehost/fetch?url=https%3A%2F%2Fquery1.finance.yahoo.com%2Fv8%2Ffinance%2Fchart%2FAAPL%3Frange%3D1d",
		"https://example.com/data.json",
		"https://query1.finance.yahoo.com/x",
	]);
});

test("focusing a text field asks for the keyboard", () => {
	const { posted, listeners } = run(BASE);
	listeners["window:focusin"]({ target: { tagName: "DIV" } });
	listeners["window:focusin"]({ target: { tagName: "INPUT" } });
	assert.deepEqual(plain(posted.filter((m) => m.k === "keyboard")), [{ k: "keyboard" }]);
});

test("available plugins get a facade: calls reach the host and signals reach handlers", async () => {
	const requests = [];
	let deliver; // resolves the pending events request
	const replies = {
		reset: () => ({ seq: 7 }),
		call: () => ({ value: null }),
		events: (url) =>
			url.includes("after=7")
				? new Promise((r) => (deliver = r))
				: new Promise(() => {}), // later polls stay pending
	};
	const context = {
		console: { log() {}, info() {}, warn() {}, error() {} },
		document: { readyState: "loading", addEventListener: (t, f) => (context.ready = f) },
		addEventListener() {},
		ipc: { postMessage() {} },
		setTimeout,
		fetch: (url) => {
			requests.push(url);
			const route = url.replace("/__edgehost/plugin/", "").split("?")[0];
			return Promise.resolve(replies[route](url)).then((body) => ({ json: () => body }));
		},
	};
	context.window = context;
	const payload = {
		...BASE,
		plugins: [
			{ name: "Filesystemprovider", available: true, methods: ["watchPath"], signals: ["fileChanged"] },
			{ name: "Sensorsdataprovider", available: false, methods: [], signals: [] },
		],
	};
	vm.createContext(context);
	vm.runInContext(SOURCE.replace("__PAYLOAD__", JSON.stringify(payload)), context);
	vm.runInContext(
		`globalThis.got = [];
		 globalThis.order = [];
		 pluginFilesystemproviderEvents = { onInitialized: () => order.push("plugin:" + pluginFilesystemprovider_initialized) };
		 icueEvents = { onICUEInitialized: () => order.push("icue") };
		 plugins.Filesystemprovider.fileChanged.connect((path, content) => got.push([path, content]));
		 plugins.Filesystemprovider.watchPath("{Home}/repo/.git/HEAD");
		 plugins.Filesystemprovider.watchPath("{Home}/repo/.git/config");`,
		context,
	);
	assert.equal(context.pluginFilesystemprovider_initialized, false);
	assert.equal(context.plugins.Sensorsdataprovider, undefined);
	context.ready();
	await new Promise((r) => setTimeout(r, 10));
	assert.deepEqual(plain(context.order), ["plugin:true", "icue"]);
	assert.equal(requests[0], "/__edgehost/plugin/reset");
	assert.ok(
		requests.includes(
			"/__edgehost/plugin/call?p=Filesystemprovider&m=watchPath&a=" + encodeURIComponent(JSON.stringify(["{Home}/repo/.git/HEAD"])),
		),
		"calls made before boot wait for the reset, then go through",
	);
	const calls = requests.filter((u) => u.includes("/call?"));
	assert.equal(calls.length, 2);
	assert.ok(calls[0].includes("HEAD") && calls[1].includes("config"), "calls keep their order");
	deliver([{ seq: 8, plugin: "Filesystemprovider", signal: "fileChanged", args: ["/r/.git/HEAD", "ref: refs/heads/main\n"] }]);
	await new Promise((r) => setTimeout(r, 10));
	assert.deepEqual(plain(context.got), [["/r/.git/HEAD", "ref: refs/heads/main\n"]]);
	assert.ok(requests.some((u) => u.endsWith("events?after=8")), "the next poll starts after the delivered event");
});
