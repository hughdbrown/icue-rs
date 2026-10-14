// Run with: node --test crates/widget-runtime/shim/
// Executes the swipe script in a vm context with a minimal fake window.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Values created inside the vm context have their own Object prototype; compare plain copies.
const plain = (v) => JSON.parse(JSON.stringify(v));

const SOURCE = readFileSync(new URL("./edge-swipe.js", import.meta.url), "utf8");

function run(styles = new Map()) {
	const posted = [];
	const listeners = {};
	const timers = [];
	const context = {
		addEventListener: (t, f) => (listeners[t] = f),
		setTimeout: (f) => timers.push(f),
		getComputedStyle: (el) => ({ touchAction: styles.get(el) || "auto" }),
	};
	context.window = context;
	vm.createContext(context);
	const api = vm.runInContext(`(${SOURCE})((m) => posted.push(m))`, Object.assign(context, { posted }));
	return { api, posted, listeners, flush: () => timers.splice(0).forEach((f) => f()) };
}

const element = (tagName, parentElement = null, attrs = []) => ({
	nodeType: 1,
	tagName,
	parentElement,
	isContentEditable: false,
	hasAttribute: (a) => attrs.includes(a),
});

const pointer = (x, y, t, target, extra = {}) => ({
	clientX: x,
	clientY: y,
	timeStamp: t,
	target,
	isPrimary: true,
	button: 0,
	...extra,
});

function drag(listeners, target, from, to, ms = 200) {
	listeners.pointerdown(pointer(from[0], from[1], 1000, target));
	listeners.pointerup(pointer(to[0], to[1], 1000 + ms, target));
}

test("classify needs distance, a mostly horizontal path and speed", () => {
	const { classify } = run().api;
	assert.equal(classify(-200, 10, 300), 1);
	assert.equal(classify(200, -10, 300), -1);
	assert.equal(classify(-100, 0, 300), 0, "too short");
	assert.equal(classify(-200, 150, 300), 0, "too diagonal");
	assert.equal(classify(-200, 0, 1200), 0, "too slow");
});

test("a left swipe posts next, a right swipe previous, a tap nothing", () => {
	const { posted, listeners } = run();
	const body = element("BODY");
	drag(listeners, body, [600, 100], [300, 120]);
	drag(listeners, body, [300, 100], [700, 90]);
	drag(listeners, body, [300, 100], [302, 101]);
	assert.deepEqual(plain(posted), [
		{ k: "swipe", dir: 1 },
		{ k: "swipe", dir: -1 },
	]);
});

test("drags from controls, touch-action:none or the opt-out attribute are ignored", () => {
	const keys = element("DIV");
	const key = element("DIV", keys);
	const slider = element("INPUT");
	const marked = element("SPAN", element("DIV", null, ["data-edgehost-no-swipe"]));
	const { posted, listeners, api } = run(new Map([[keys, "none"]]));
	for (const target of [key, slider, marked]) drag(listeners, target, [600, 100], [200, 100]);
	assert.deepEqual(plain(posted), []);
	assert.equal(api.optedOut(element("DIV")), false);
});

test("a cancelled pointer or a secondary button is not a swipe", () => {
	const { posted, listeners } = run();
	const body = element("BODY");
	listeners.pointerdown(pointer(600, 100, 0, body));
	listeners.pointercancel({});
	listeners.pointerup(pointer(100, 100, 100, body));
	listeners.pointerdown(pointer(600, 100, 0, body, { button: 2 }));
	listeners.pointerup(pointer(100, 100, 100, body));
	assert.deepEqual(plain(posted), []);
});

test("only the click that directly follows a swipe is swallowed", () => {
	const { listeners, flush } = run();
	const body = element("BODY");
	let stopped = 0;
	const click = () => ({ stopPropagation: () => stopped++, preventDefault() {} });
	drag(listeners, body, [600, 100], [100, 100]);
	listeners.click(click());
	assert.equal(stopped, 1);
	flush();
	listeners.click(click());
	assert.equal(stopped, 1, "a later tap still clicks");
	drag(listeners, body, [600, 100], [100, 100]);
	flush();
	listeners.click(click());
	assert.equal(stopped, 1, "no click arrived after the swipe: the next one is not eaten");
});
