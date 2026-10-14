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
		addEventListener() {},
		ipc: { postMessage: (m) => posted.push(JSON.parse(m)) },
	};
	context.window = context;
	vm.createContext(context);
	vm.runInContext(SOURCE.replace("__PAYLOAD__", JSON.stringify(payload)), context);
	vm.runInContext(widgetScript, context);
	return { context, posted, listeners };
}

const BASE = {
	lang: "de",
	fpsLimit: 30,
	preview: false,
	widgetId: "w-1",
	instanceId: "i-1",
	deviceId: "d-1",
	tempUnit: "°C",
	props: [
		{ name: "textColor", defaultExpr: "'#FFFFFF'" },
		{ name: "size", defaultExpr: "40 + 2" },
		{ name: "label", defaultExpr: "tr('Hello')" },
		{ name: "fromModule", defaultExpr: "Later.value()" },
		{ name: "none", defaultExpr: null },
	],
	tr: { Hello: "Hallo" },
	modules: [],
	plugins: ["Sensorsdataprovider"],
};

test("globals exist before widget scripts run", () => {
	const { context } = run(BASE, "globalThis.seen = [textColor, size, label, uniqueId, device.deviceId];");
	assert.deepEqual(plain(context.seen), ["#FFFFFF", 42, "Hallo", "i-1", "d-1"]);
	assert.equal(context.iCUE.iCUELanguage, "de");
	assert.equal(context.iCUE.defaultTemperatureUnit(), "°C");
	assert.equal(context.iCUE_initialized, false);
	assert.equal(context.pluginSensorsdataprovider_initialized, false);
	assert.equal(context.fromModule, undefined);
	assert.ok("none" in context);
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

test("deferred defaults are retried during boot", async () => {
	const { context, listeners } = run(BASE, "globalThis.Later = { value: () => 'late' };");
	assert.equal(context.fromModule, undefined);
	listeners.DOMContentLoaded();
	await new Promise((r) => setImmediate(r));
	assert.equal(context.fromModule, "late");
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
