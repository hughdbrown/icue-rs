// Tests for the Piano widget: pure helpers plus a fake-DOM init/pointer test.
// Run with: node --test examples/widgets/piano/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("./scripts/piano.js", import.meta.url), "utf8");

function makeParam() {
	const p = { value: 0 };
	for (const m of ["setValueAtTime", "linearRampToValueAtTime", "exponentialRampToValueAtTime", "setTargetAtTime", "cancelScheduledValues"]) p[m] = () => p;
	return p;
}

class StubNode {
	constructor(ctx) {
		this.ctx = ctx;
		this.gain = makeParam();
		this.frequency = makeParam();
		this.Q = makeParam();
		this.threshold = makeParam();
		this.knee = makeParam();
		this.ratio = makeParam();
		this.attack = makeParam();
		this.release = makeParam();
		this.started = false;
		this.stopped = false;
	}
	connect(n) {
		return n;
	}
	disconnect() {}
	start() {
		this.started = true;
		this.ctx.oscillatorsStarted++;
	}
	stop() {
		this.stopped = true;
	}
}

class StubAudioContext {
	constructor() {
		this.currentTime = 0;
		this.state = "suspended";
		this.destination = {};
		this.oscillatorsStarted = 0;
		this.resumeCalls = 0;
		StubAudioContext.instances.push(this);
	}
	createGain() {
		return new StubNode(this);
	}
	createOscillator() {
		return new StubNode(this);
	}
	createBiquadFilter() {
		return new StubNode(this);
	}
	createDynamicsCompressor() {
		return new StubNode(this);
	}
	resume() {
		this.resumeCalls++;
		this.state = "running";
		return Promise.resolve();
	}
}
StubAudioContext.instances = [];

function makeEnv(settings = {}) {
	const storage = new Map();
	const byId = new Map();
	const docListeners = {};

	function makeElement(tag, id) {
		const listeners = {};
		const attrs = {};
		const children = [];
		const el = {
			tagName: tag.toUpperCase(),
			id: id || "",
			className: "",
			textContent: "",
			disabled: false,
			parentNode: null,
			children,
			style: { setProperty(k, v) { this[k] = v; } },
			setAttribute(k, v) {
				attrs[k] = String(v);
			},
			getAttribute(k) {
				return Object.prototype.hasOwnProperty.call(attrs, k) ? attrs[k] : null;
			},
			addEventListener(type, fn) {
				(listeners[type] ||= []).push(fn);
			},
			replaceChildren(...nodes) {
				for (const c of children) c.parentNode = null;
				children.length = 0;
				for (const n of nodes) {
					n.parentNode = el;
					children.push(n);
				}
			},
			dispatch(type, event = {}) {
				for (const fn of listeners[type] || []) fn({ preventDefault() {}, ...event });
			},
		};
		return el;
	}

	const document = {
		createElement: (tag) => makeElement(tag),
		getElementById(id) {
			if (!byId.has(id)) byId.set(id, makeElement("div", id));
			return byId.get(id);
		},
		documentElement: makeElement("html"),
		addEventListener(type, fn) {
			(docListeners[type] ||= []).push(fn);
		},
		elementFromPoint: () => null,
	};
	const logs = [];
	const context = {
		console: { log: (...a) => logs.push(a.join(" ")), warn() {}, error() {} },
		document,
		localStorage: {
			getItem: (k) => (storage.has(k) ? storage.get(k) : null),
			setItem: (k, v) => storage.set(k, String(v)),
		},
		addEventListener() {},
		AudioContext: StubAudioContext,
		uniqueId: "u-1",
		octaves: "3",
		startOctave: "3",
		sound: "piano",
		volume: 70,
		labels: "c",
		sustain: false,
		accentColor: "#FFB300",
		backgroundColor: "#14161C",
		iCUE_initialized: false,
		...settings,
	};
	context.window = context;
	vm.createContext(context);
	vm.runInContext(SOURCE, context);
	const call = (expr) => vm.runInContext(expr, context);
	return { context, byId, storage, logs, docListeners, call };
}

const keyEls = (env) => env.byId.get("keyboard").children;
const hasClass = (el, name) => el.className.split(/\s+/).includes(name);

test("midiToFrequency anchors A4 = 440 Hz", () => {
	const { call } = makeEnv();
	assert.equal(call("midiToFrequency(69)"), 440);
	assert.equal(call("midiToFrequency(81)"), 880);
	assert.ok(Math.abs(call("midiToFrequency(60)") - 261.6256) < 1e-3);
});

test("noteName names notes with octave numbers", () => {
	const { call } = makeEnv();
	assert.equal(call("noteName(60)"), "C4");
	assert.equal(call("noteName(61)"), "C♯4");
	assert.equal(call("noteName(69)"), "A4");
	assert.equal(call("noteName(48)"), "C3");
});

test("keyboardLayout counts keys and positions them", () => {
	const { call } = makeEnv();
	for (const [oct, whites, blacks] of [[2, 15, 10], [3, 22, 15]]) {
		const layout = call(`keyboardLayout(${oct}, 3)`);
		assert.equal(layout.whiteCount, whites);
		assert.equal(layout.keys.filter((k) => !k.black).length, whites);
		assert.equal(layout.keys.filter((k) => k.black).length, blacks);
		assert.equal(layout.keys[0].midi, 48);
		assert.equal(layout.keys[layout.keys.length - 1].midi, 48 + 12 * oct);
	}
	const layout = call("keyboardLayout(2, 3)");
	const w = 1 / 15;
	assert.ok(Math.abs(layout.keys[0].x) < 1e-12);
	assert.ok(Math.abs(layout.keys[0].w - w) < 1e-12);
	const cSharp = layout.keys[1];
	assert.equal(cSharp.black, true);
	assert.ok(Math.abs(cSharp.x + cSharp.w / 2 - w) < 1e-12, "black key centred on the C/D boundary");
	const last = layout.keys[layout.keys.length - 1];
	assert.ok(Math.abs(last.x + last.w - 1) < 1e-9, "white keys fill the width");
});

test("computerKeyToMidi maps both rows from the start octave", () => {
	const { call } = makeEnv();
	assert.equal(call("computerKeyToMidi('z', 3)"), 48);
	assert.equal(call("computerKeyToMidi('s', 3)"), 49);
	assert.equal(call("computerKeyToMidi(',', 3)"), 60);
	assert.equal(call("computerKeyToMidi('q', 3)"), 60);
	assert.equal(call("computerKeyToMidi('Q', 3)"), 60);
	assert.equal(call("computerKeyToMidi('i', 3)"), 72);
	assert.equal(call("computerKeyToMidi('p', 3)"), null);
});

test("init builds 3 octaves: 22 white + 15 black, and logs once", () => {
	const env = makeEnv();
	env.context.icueEvents.onICUEInitialized();
	env.context.icueEvents.onDataUpdated();
	const keys = keyEls(env);
	assert.equal(keys.filter((k) => hasClass(k, "white")).length, 22);
	assert.equal(keys.filter((k) => hasClass(k, "black")).length, 15);
	assert.deepEqual(env.logs, ["piano: 3 octaves from C3, sound piano"]);
});

test("init builds 2 octaves: 15 white + 10 black", () => {
	const env = makeEnv({ octaves: "2", startOctave: "4" });
	env.context.icueEvents.onICUEInitialized();
	const keys = keyEls(env);
	assert.equal(keys.filter((k) => hasClass(k, "white")).length, 15);
	assert.equal(keys.filter((k) => hasClass(k, "black")).length, 10);
	assert.equal(keys[0].getAttribute("data-midi"), "60");
});

test("pointerdown marks a key pressed, pointerup releases it", () => {
	const env = makeEnv();
	env.context.icueEvents.onICUEInitialized();
	const key = keyEls(env)[0];
	key.dispatch("pointerdown", { pointerId: 1, button: 0 });
	assert.ok(hasClass(key, "pressed"));
	assert.equal(StubAudioContext.instances.at(-1).resumeCalls >= 1, true, "audio context resumed on first gesture");
	for (const fn of env.docListeners.pointerup) fn({ pointerId: 1 });
	assert.ok(!hasClass(key, "pressed"));
});

test("two pointers play two notes; glissando moves the note", () => {
	const env = makeEnv();
	env.context.icueEvents.onICUEInitialized();
	const keys = keyEls(env);
	keys[0].dispatch("pointerdown", { pointerId: 1, button: 0 });
	keys[2].dispatch("pointerdown", { pointerId: 2, button: 0 });
	assert.ok(hasClass(keys[0], "pressed") && hasClass(keys[2], "pressed"));
	env.context.document.elementFromPoint = () => keys[4];
	for (const fn of env.docListeners.pointermove) fn({ pointerId: 1, clientX: 0, clientY: 0 });
	assert.ok(!hasClass(keys[0], "pressed"));
	assert.ok(hasClass(keys[4], "pressed"));
	assert.ok(hasClass(keys[2], "pressed"), "other finger unaffected");
	env.context.document.elementFromPoint = () => null; // pointer left the keyboard
	for (const fn of env.docListeners.pointermove) fn({ pointerId: 1, clientX: 0, clientY: 0 });
	assert.ok(!hasClass(keys[4], "pressed"));
});

test("octave buttons change and persist on-widget state; settings re-seed", () => {
	const env = makeEnv();
	env.context.icueEvents.onICUEInitialized();
	env.byId.get("oct-up").dispatch("click");
	assert.equal(env.byId.get("oct-label").textContent, "C4–C7");
	assert.equal(JSON.parse(env.storage.get("u-1")).startOctave, 4);
	assert.equal(keyEls(env)[0].getAttribute("data-midi"), "60");
	// A data update with unchanged settings keeps the on-widget choice.
	env.context.icueEvents.onDataUpdated();
	assert.equal(env.byId.get("oct-label").textContent, "C4–C7");
	// Changing the startOctave setting wins and rebuilds.
	env.context.startOctave = "2";
	env.context.octaves = "2";
	env.context.icueEvents.onDataUpdated();
	assert.equal(env.byId.get("oct-label").textContent, "C2–C4");
	assert.equal(keyEls(env).length, 25);
});

test("computer keyboard plays notes and ignores repeats", () => {
	const env = makeEnv();
	env.context.icueEvents.onICUEInitialized();
	const keys = keyEls(env);
	for (const fn of env.docListeners.keydown) fn({ key: "z" });
	assert.ok(hasClass(keys[0], "pressed"));
	for (const fn of env.docListeners.keydown) fn({ key: "z", repeat: true });
	for (const fn of env.docListeners.keyup) fn({ key: "z" });
	assert.ok(!hasClass(keys[0], "pressed"));
});
