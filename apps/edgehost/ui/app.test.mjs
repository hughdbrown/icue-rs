// Fake-DOM tests for the configurator UI.
// Run with: node --test apps/edgehost/ui/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("./app.js", import.meta.url), "utf8");

// ---------------------------------------------------------------- fixture

function fixtureState() {
	const styler = {
		id: "styler",
		version: "1.0.0",
		name: "Styler",
		description: "A widget with every kind of property.",
		author: "test",
		icon: "edgecfg://widget/styler/1.0.0/icon.png",
		supports_edge: true,
		interactive: false,
		required_plugins: [],
		properties: [
			{ name: "accent", kind: "color", label: "Accent", default_expr: "'#ff0000'" },
			{ name: "size", kind: "slider", label: "Size", default_expr: "50", min: 0, max: 100, step: 5, unit: "px" },
			{ name: "shadow", kind: "switch", label: "Shadow", default_expr: "true" },
			{ name: "title", kind: "textfield", label: "Title", default_expr: "'Hi'" },
			{ name: "mode", kind: "combobox", label: "Mode", default_expr: "'a'", options: [{ key: "a", label: "Alpha" }, { key: "b", label: "Beta" }] },
			{ name: "period", kind: "tab-buttons", label: "Period", default_expr: "'1d'", options: [{ key: "1d", label: "1D" }, { key: "5d", label: "5D" }] },
			{ name: "extra", kind: "media-selector", label: "Media", default_expr: null },
		],
		groups: [{ title: "Look", properties: ["accent", "size", "shadow"], info: "Visual options" }],
	};
	return {
		orientation: "landscape",
		grid: { cols: 3, rows: 2 },
		slots: { S: { cols: 1, rows: 1 }, M: { cols: 1, rows: 2 }, L: { cols: 2, rows: 2 }, XL: { cols: 3, rows: 2 } },
		pages: [
			{
				instances: [
					{ id: "i1", widget: "clock", version: "1.0.0", slot: "S", origin: { col: 0, row: 0 }, values: {}, user_set: [], defaults_pending: false },
					{
						id: "i3", widget: "styler", version: "1.0.0", slot: "S", origin: { col: 2, row: 0 },
						values: { accent: "#ff0000", size: 50, shadow: true, title: "Hi", mode: "a", period: "1d", extra: null },
						user_set: ["accent"], defaults_pending: false,
					},
				],
			},
			{ instances: [{ id: "i2", widget: "weather", version: "2.0.0", slot: "M", origin: { col: 1, row: 0 }, values: {}, user_set: [], defaults_pending: true }] },
		],
		display: "auto",
		displays: [
			{ index: 0, name: "Main", pixel_size: [1920, 1080], edge: false, native: true },
			{ index: 1, name: "Xeneon Edge", pixel_size: [2560, 720], edge: true, native: false },
		],
		widgets: [
			{ id: "clock", version: "1.0.0", name: "Clock", description: "Shows the time.", author: "a", icon: null, supports_edge: true, interactive: false, required_plugins: [], properties: [], groups: [] },
			{ id: "weather", version: "2.0.0", name: "Weather", description: "Forecast.", author: "a", icon: null, supports_edge: false, interactive: true, required_plugins: ["ns:Weather:1.0"], properties: [], groups: [] },
			styler,
		],
		data_dir: "/Users/test/EdgeHost",
	};
}

// ---------------------------------------------------------------- fake DOM

function makeEnv() {
	let focused = null;
	const byId = new Map();

	function makeNode(tag) {
		const listeners = {};
		const attrs = {};
		const children = [];
		const el = {
			tagName: tag.toUpperCase(),
			className: "",
			_text: "",
			_parent: null,
			hidden: false,
			disabled: false,
			checked: false,
			value: "",
			style: { setProperty(k, v) { this[k] = v; } },
			children,
			get textContent() {
				return el._text;
			},
			set textContent(v) {
				el._text = String(v);
				for (const c of children) c._parent = null;
				children.length = 0;
			},
			append(...nodes) {
				for (const n of nodes) {
					n._parent = el;
					children.push(n);
				}
			},
			replaceChildren(...nodes) {
				for (const c of children) c._parent = null;
				children.length = 0;
				el.append(...nodes.flat());
			},
			remove() {
				if (el._parent) {
					const i = el._parent.children.indexOf(el);
					if (i >= 0) el._parent.children.splice(i, 1);
					el._parent = null;
				}
			},
			replaceWith(n) {
				const p = el._parent;
				if (p) { p.children[p.children.indexOf(el)] = n; n._parent = p; el._parent = null; }
			},
			setAttribute(k, v) { attrs[k] = String(v); },
			getAttribute(k) { return k in attrs ? attrs[k] : null; },
			addEventListener(t, fn) { (listeners[t] ||= []).push(fn); },
			focus() { focused = el; },
			getBoundingClientRect() {
				return el.className.split(/\s+/).includes("board") ? env.boardRect : { left: 0, top: 0, width: 0, height: 0 };
			},
			setPointerCapture() {},
			releasePointerCapture() {},
			// Test helper: bubble an event up the parent chain.
			fire(type, init = {}) {
				const ev = { type, target: el, key: undefined, button: 0, pointerId: 1, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...init };
				for (let n = el; n; n = n._parent) for (const fn of n.__listeners[type] || []) fn(ev);
				return ev;
			},
			__listeners: listeners,
		};
		return el;
	}

	function all(node, out = []) {
		for (const c of node.children) { out.push(c); all(c, out); }
		return out;
	}
	const cls = (n, c) => n.className.split(/\s+/).includes(c);
	function text(n) { return n._text + n.children.map(text).join(""); }

	const body = makeNode("body");
	for (const id of ["topbar", "library", "editor", "settings", "toast"]) {
		const n = makeNode(id === "topbar" ? "header" : "div");
		byId.set(id, n);
		body.append(n);
	}
	const docListeners = {};
	const document = {
		body,
		get activeElement() { return focused; },
		createElement: (t) => makeNode(t),
		createTextNode: (t) => { const n = makeNode("#text"); n._text = String(t); return n; },
		getElementById: (id) => byId.get(id),
		addEventListener: (t, fn) => (docListeners[t] ||= []).push(fn),
	};

	// virtual timers
	let now = 0, nextTimer = 1;
	const timers = new Map();
	const context = {
		console, document, Promise, JSON, Math, Number, Object, String, Array, Set, Map, Error,
		setTimeout: (fn, ms) => { const id = nextTimer++; timers.set(id, { at: now + ms, fn }); return id; },
		clearTimeout: (id) => timers.delete(id),
		confirm: () => env.confirmAnswer,
	};
	context.window = context;
	context.globalThis = context;

	const env = {
		context, document, body, state: fixtureState(), calls: [], failNext: null, confirmAnswer: true,
		boardRect: { left: 0, top: 0, width: 900, height: 300 },
		el: (id) => byId.get(id),
		all: (root = body) => all(root),
		find: (root, pred) => all(root).find(pred),
		findAll: (root, pred) => all(root).filter(pred),
		cls, text,
		mutationCalls: () => env.calls.filter((c) => c.method !== "state.get"),
		tick(ms) {
			now += ms;
			for (const [id, t] of [...timers]) if (t.at <= now) { timers.delete(id); t.fn(); }
		},
		async settle() { for (let i = 0; i < 8; i++) await new Promise((r) => setImmediate(r)); },
	};

	// fake host
	context.ipc = {
		postMessage(raw) {
			const msg = JSON.parse(raw);
			env.calls.push({ method: msg.method, params: msg.params });
			Promise.resolve().then(() => {
				if (env.failNext && msg.method !== "state.get") {
					const message = env.failNext;
					env.failNext = null;
					return context.__configurator.reply({ id: msg.id, error: { message } });
				}
				context.__configurator.reply({ id: msg.id, result: handle(msg.method, msg.params) });
			});
		},
	};
	function instOf(id) {
		for (const p of env.state.pages) { const i = p.instances.find((x) => x.id === id); if (i) return i; }
	}
	function handle(method, p) {
		const st = env.state;
		switch (method) {
			case "state.get": return JSON.parse(JSON.stringify(st));
			case "layout.add": {
				const w = st.widgets.find((x) => x.id === p.widget);
				if (!st.pages[p.page]) st.pages[p.page] = { instances: [] };
				const inst = { id: "new" + st.pages[p.page].instances.length, widget: w.id, version: w.version, slot: p.slot, origin: { col: 1, row: 1 }, values: {}, user_set: [], defaults_pending: false };
				st.pages[p.page].instances.push(inst);
				return { instance: inst.id };
			}
			case "layout.move": { const i = instOf(p.instance); i.origin = p.origin; i.slot = p.slot; return {}; }
			case "layout.remove": for (const pg of st.pages) pg.instances = pg.instances.filter((x) => x.id !== p.instance); return {};
			case "layout.set": { const i = instOf(p.instance); i.values[p.key] = p.value; if (!i.user_set.includes(p.key)) i.user_set.push(p.key); return {}; }
			case "layout.unset": { const i = instOf(p.instance); i.user_set = i.user_set.filter((k) => k !== p.key); return {}; }
			case "layout.display": st.display = p.choice; return {};
			case "layout.orientation": st.orientation = p.orientation; return {};
			default: throw new Error("unexpected " + method);
		}
	}

	vm.createContext(context);
	vm.runInContext(SOURCE, context, { filename: "app.js" });
	return env;
}

async function boot() {
	const env = makeEnv();
	await env.context.__configuratorApp.start();
	await env.settle();
	return env;
}

const isBtn = (n) => n.tagName === "BUTTON";

// ---------------------------------------------------------------- tests

test("does not auto-start without window.ipc and exposes the app hook", () => {
	const env = makeEnv();
	assert.equal(typeof env.context.__configuratorApp.start, "function");
	assert.equal(typeof env.context.__configuratorApp.render, "function");
	assert.equal(typeof env.context.__configuratorApp.rpc, "function");
	assert.equal(env.calls.length, 0);
});

test("start defines the reply hook and fetches state", async () => {
	const env = await boot();
	assert.equal(typeof env.context.__configurator.reply, "function");
	assert.deepEqual(env.calls.map((c) => c.method), ["state.get"]);
});

test("library renders all widgets with badges and four slot buttons each", async () => {
	const env = await boot();
	const lib = env.el("library");
	const cards = env.findAll(lib, (n) => n.getAttribute("data-widget-card"));
	assert.deepEqual(cards.map((c) => c.getAttribute("data-widget-card")), ["clock", "weather", "styler"]);
	for (const c of cards) assert.equal(env.findAll(c, (n) => env.cls(n, "slot-btn")).length, 4);
	const weather = env.text(cards[1]);
	assert.match(weather, /needs plugins/);
	assert.match(weather, /not for Edge/);
	assert.doesNotMatch(env.text(cards[0]), /needs plugins|not for Edge/);
	// icon vs placeholder
	assert.ok(env.find(cards[2], (n) => n.tagName === "IMG"));
	assert.ok(env.find(cards[0], (n) => env.cls(n, "w-icon-ph")));
});

test("clicking a slot button sends layout.add for the current page and selects the new tile", async () => {
	const env = await boot();
	const btn = env.find(env.el("library"), (n) => isBtn(n) && n.getAttribute("data-widget") === "clock" && n.getAttribute("data-slot") === "M");
	btn.fire("click");
	await env.settle();
	assert.deepEqual(env.mutationCalls()[0], { method: "layout.add", params: { widget: "clock", version: "1.0.0", slot: "M", page: 0 } });
	assert.equal(env.calls.at(-1).method, "state.get"); // re-fetched after the mutation
	assert.equal(env.context.__configuratorApp.getState().selected, "new2");
});

test("adding on the '+ Page' tab targets page == page count", async () => {
	const env = await boot();
	env.find(env.el("editor"), (n) => env.cls(n, "page-add")).fire("click");
	const btn = env.find(env.el("library"), (n) => isBtn(n) && n.getAttribute("data-widget") === "clock" && n.getAttribute("data-slot") === "S");
	btn.fire("click");
	await env.settle();
	assert.equal(env.mutationCalls()[0].params.page, 2);
});

test("page editor renders cells and positioned tiles", async () => {
	const env = await boot();
	const board = env.find(env.el("editor"), (n) => env.cls(n, "board"));
	assert.ok(env.cls(board, "landscape"));
	assert.equal(env.findAll(board, (n) => env.cls(n, "cell")).length, 6);
	const tiles = env.findAll(board, (n) => n.getAttribute("data-instance"));
	assert.deepEqual(tiles.map((t) => t.getAttribute("data-instance")), ["i1", "i3"]);
	assert.equal(tiles[1].style.left, `${(2 / 3) * 100}%`);
	assert.equal(tiles[1].style.width, `${(1 / 3) * 100}%`);
});

test("selecting a tile renders its generated controls, grouped, ungrouped last", async () => {
	const env = await boot();
	assert.match(env.text(env.el("settings")), /Select a tile/);
	env.find(env.el("editor"), (n) => n.getAttribute("data-instance") === "i3").fire("pointerdown", { clientX: 700, clientY: 50 });
	env.el("editor") && env.find(env.el("editor"), (n) => n.getAttribute("data-instance") === "i3").fire("pointerup", { clientX: 700, clientY: 50 });
	await env.settle();
	const s = env.el("settings");
	assert.match(env.text(s), /Styler/);
	const names = env.findAll(s, (n) => n.getAttribute("data-prop")).map((n) => n.getAttribute("data-prop"));
	assert.deepEqual(names, ["accent", "size", "shadow", "title", "mode", "period", "extra"]);
	assert.match(env.text(s), /Look/);
	assert.match(env.text(s), /Other/);
	assert.ok(env.find(s, (n) => n.tagName === "INPUT" && n.type === "color"));
	assert.ok(env.find(s, (n) => n.tagName === "INPUT" && n.type === "range"));
	assert.ok(env.find(s, (n) => n.tagName === "INPUT" && n.type === "checkbox"));
	assert.ok(env.find(s, (n) => n.tagName === "SELECT"));
	assert.equal(env.findAll(s, (n) => isBtn(n) && n.getAttribute("data-key") && n.getAttribute("aria-pressed")).length, 2);
	assert.ok(env.find(s, (n) => n.tagName === "TEXTAREA"));
	// no click happened beyond selection: no mutation
	assert.equal(env.mutationCalls().length, 0);
});

async function selectStyler(env) {
	env.context.__configuratorApp.select("i3");
	await env.settle();
	return env.el("settings");
}

test("changing a color sends layout.set; reset sends layout.unset only for user_set keys", async () => {
	const env = await boot();
	let s = await selectStyler(env);
	const color = env.find(s, (n) => n.type === "color");
	color.value = "#00ff00";
	color.fire("change");
	await env.settle();
	assert.deepEqual(env.mutationCalls()[0], { method: "layout.set", params: { instance: "i3", key: "accent", value: "#00ff00" } });

	s = env.el("settings");
	const resets = env.findAll(s, (n) => env.cls(n, "reset"));
	assert.deepEqual(resets.map((r) => r.getAttribute("data-key")), ["accent"]);
	resets[0].fire("click");
	await env.settle();
	assert.deepEqual(env.mutationCalls()[1], { method: "layout.unset", params: { instance: "i3", key: "accent" } });
	assert.equal(env.findAll(env.el("settings"), (n) => env.cls(n, "reset")).length, 0);
});

test("switch, combobox and tab-buttons send immediately", async () => {
	const env = await boot();
	const s = await selectStyler(env);
	const sw = env.find(s, (n) => n.type === "checkbox");
	sw.checked = false;
	sw.fire("change");
	await env.settle();
	const sel = env.find(env.el("settings"), (n) => n.tagName === "SELECT");
	sel.value = "b";
	sel.fire("change");
	await env.settle();
	env.find(env.el("settings"), (n) => isBtn(n) && n.getAttribute("data-key") === "5d").fire("click");
	await env.settle();
	assert.deepEqual(env.mutationCalls().map((c) => c.params.value), [false, "b", "5d"]);
});

test("slider and text edits are debounced by 300 ms", async () => {
	const env = await boot();
	const s = await selectStyler(env);
	const range = env.find(s, (n) => n.type === "range");
	range.value = "60";
	range.fire("input");
	range.value = "65";
	range.fire("input");
	env.tick(299);
	assert.equal(env.mutationCalls().length, 0);
	env.tick(1);
	await env.settle();
	assert.deepEqual(env.mutationCalls(), [{ method: "layout.set", params: { instance: "i3", key: "size", value: 65 } }]);

	const text = env.find(env.el("settings"), (n) => n.type === "text");
	text.value = "Hello";
	text.fire("input");
	env.tick(300);
	await env.settle();
	assert.deepEqual(env.mutationCalls().at(-1), { method: "layout.set", params: { instance: "i3", key: "title", value: "Hello" } });
});

test("JSON fallback rejects invalid JSON client-side", async () => {
	const env = await boot();
	const s = await selectStyler(env);
	const area = env.find(s, (n) => n.tagName === "TEXTAREA");
	area.value = "{oops";
	area.fire("change");
	await env.settle();
	assert.equal(env.mutationCalls().length, 0);
	assert.match(env.text(env.el("toast")), /valid JSON/);
	area.value = '{"a":1}';
	area.fire("change");
	await env.settle();
	assert.deepEqual(env.mutationCalls()[0].params, { instance: "i3", key: "extra", value: { a: 1 } });
});

test("resize buttons send layout.move with the same origin; delete confirms", async () => {
	const env = await boot();
	await selectStyler(env);
	const find = (pred) => env.find(env.el("settings"), pred);
	find((n) => isBtn(n) && n.getAttribute("data-slot") === "M").fire("click");
	await env.settle();
	assert.deepEqual(env.mutationCalls()[0], { method: "layout.move", params: { instance: "i3", page: 0, origin: { col: 2, row: 0 }, slot: "M" } });

	env.confirmAnswer = false;
	find((n) => env.cls(n, "delete-btn")).fire("click");
	await env.settle();
	assert.equal(env.mutationCalls().length, 1);
	env.confirmAnswer = true;
	find((n) => env.cls(n, "delete-btn")).fire("click");
	await env.settle();
	assert.deepEqual(env.mutationCalls()[1], { method: "layout.remove", params: { instance: "i3" } });
	assert.match(env.text(env.el("settings")), /Select a tile/);
});

test("defaults_pending shows a note", async () => {
	const env = await boot();
	env.state.pages[0].instances[0].defaults_pending = true;
	await env.context.__configuratorApp.refresh();
	env.context.__configuratorApp.select("i1");
	assert.match(env.text(env.el("settings")), /Defaults pending/);
});

function dragTile(env, id, from, to) {
	const tile = env.find(env.el("editor"), (n) => n.getAttribute("data-instance") === id);
	tile.fire("pointerdown", { clientX: from[0], clientY: from[1] });
	tile.fire("pointermove", { clientX: to[0], clientY: to[1] });
	return tile;
}

test("drag computes the target cell and sends layout.move", async () => {
	const env = await boot();
	// board 900x300, 3x2 grid: cells are 300x150. i1 at (0,0) -> drop at (1,1).
	const tile = dragTile(env, "i1", [50, 50], [350, 200]);
	const drop = env.find(env.el("editor"), (n) => env.cls(n, "drop"));
	assert.ok(env.cls(drop, "valid"));
	tile.fire("pointerup", { clientX: 350, clientY: 200 });
	await env.settle();
	assert.deepEqual(env.mutationCalls(), [{ method: "layout.move", params: { instance: "i1", page: 0, origin: { col: 1, row: 1 }, slot: "S" } }]);
});

test("dropping on an occupied cell is flagged invalid and not sent", async () => {
	const env = await boot();
	const tile = dragTile(env, "i1", [50, 50], [750, 50]); // cell (2,0) holds i3
	assert.ok(env.cls(env.find(env.el("editor"), (n) => env.cls(n, "drop")), "invalid"));
	tile.fire("pointerup", { clientX: 750, clientY: 50 });
	await env.settle();
	assert.equal(env.mutationCalls().length, 0);
	assert.match(env.text(env.el("toast")), /not free/);
});

test("a tiny pointer movement is a click, not a drag", async () => {
	const env = await boot();
	const tile = dragTile(env, "i1", [50, 50], [52, 51]);
	tile.fire("pointerup", { clientX: 52, clientY: 51 });
	await env.settle();
	assert.equal(env.mutationCalls().length, 0);
	assert.equal(env.context.__configuratorApp.getState().selected, "i1");
});

test("arrow keys move the selected tile; Delete removes after confirm", async () => {
	const env = await boot();
	env.context.__configuratorApp.select("i1");
	const board = () => env.find(env.el("editor"), (n) => env.cls(n, "board"));
	board().fire("keydown", { key: "ArrowDown" });
	await env.settle();
	assert.deepEqual(env.mutationCalls()[0], { method: "layout.move", params: { instance: "i1", page: 0, origin: { col: 0, row: 1 }, slot: "S" } });
	board().fire("keydown", { key: "ArrowLeft" }); // would leave the grid
	await env.settle();
	assert.equal(env.mutationCalls().length, 1);
	board().fire("keydown", { key: "Delete" });
	await env.settle();
	assert.deepEqual(env.mutationCalls()[1], { method: "layout.remove", params: { instance: "i1" } });
});

test("a host error is shown in the toast", async () => {
	const env = await boot();
	env.failNext = "The widget does not fit there.";
	env.find(env.el("library"), (n) => isBtn(n) && n.getAttribute("data-widget") === "clock" && n.getAttribute("data-slot") === "XL").fire("click");
	await env.settle();
	const toast = env.el("toast");
	assert.equal(toast.hidden, false);
	assert.match(toast.textContent, /does not fit there/);
	assert.ok(env.cls(toast, "error"));
});

test("top bar: display options, orientation disabled while pages are not empty", async () => {
	const env = await boot();
	const top = env.el("topbar");
	const sel = env.find(top, (n) => n.tagName === "SELECT");
	const labels = env.findAll(sel, (n) => n.tagName === "OPTION").map((o) => env.text(o));
	assert.equal(labels.length, 4);
	assert.match(labels[2], /^0: Main \(1920x1080\).*native/);
	assert.match(labels[3], /^1: Xeneon Edge \(2560x720\).*Edge/);
	assert.match(env.text(top), /\/Users\/test\/EdgeHost/);
	assert.equal(env.find(top, (n) => n.tagName === "BUTTON").disabled, true);

	sel.value = "1";
	sel.fire("change");
	await env.settle();
	assert.deepEqual(env.mutationCalls()[0], { method: "layout.display", params: { choice: "1" } });

	env.state.pages = [];
	await env.context.__configuratorApp.refresh();
	const btn = env.find(env.el("topbar"), (n) => n.tagName === "BUTTON");
	assert.equal(btn.disabled, false);
	btn.fire("click");
	await env.settle();
	assert.deepEqual(env.mutationCalls().at(-1), { method: "layout.orientation", params: { orientation: "portrait" } });
	assert.ok(env.cls(env.find(env.el("editor"), (n) => env.cls(n, "board")), "portrait"));
});

test("the 'changed' event re-fetches state", async () => {
	const env = await boot();
	env.state.widgets[0].name = "Renamed";
	env.context.__configurator.event({ type: "changed" });
	await env.settle();
	assert.match(env.text(env.el("library")), /Renamed/);
});

test("rpc times out after 10 s", async () => {
	const env = makeEnv();
	env.context.ipc.postMessage = () => {};
	env.context.__configurator = undefined;
	const p = env.context.__configuratorApp.rpc("state.get", {});
	const caught = p.then(() => null, (e) => e);
	env.tick(10000);
	const err = await caught;
	assert.match(err.message, /did not answer/);
});
