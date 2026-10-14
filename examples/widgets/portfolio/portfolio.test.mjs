// Minimal fake-DOM test for the Stock Portfolio widget.
// Run with: node --test examples/widgets/portfolio/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("./scripts/portfolio.js", import.meta.url), "utf8");

function makeWidgetEnv() {
	const byId = new Map();
	const storage = new Map();
	const fetchCalls = [];

	function makeElement(tag, id) {
		const eventListeners = {};
		const children = [];
		const attrs = {};
		const el = {
			tagName: tag.toUpperCase(),
			id: id || "",
			className: "",
			textContent: "",
			hidden: false,
			type: "",
			value: "",
			placeholder: "",
			autocapitalize: "",
			spellcheck: false,
			inputMode: "",
			colSpan: 1,
			_parent: null,
			_attrs: attrs,
			style: {
				_properties: {},
				setProperty(k, v) {
					this._properties[k] = String(v);
				},
				getPropertyValue(k) {
					return this._properties[k] || "";
				},
			},
			get children() {
				return children;
			},
			append(...nodes) {
				for (const n of nodes) {
					if (n == null) continue;
					children.push(n);
					n._parent = el;
				}
			},
			replaceChildren(...nodes) {
				for (const c of children) c._parent = null;
				children.length = 0;
				el.append(...nodes);
			},
			remove() {
				if (el._parent) el._parent.removeChild(el);
			},
			removeChild(n) {
				const i = children.indexOf(n);
				if (i >= 0) {
					children.splice(i, 1);
					n._parent = null;
				}
			},
			appendChild(n) {
				el.append(n);
			},
			setAttribute(k, v) {
				attrs[k] = String(v);
			},
			getAttribute(k) {
				return Object.prototype.hasOwnProperty.call(attrs, k) ? attrs[k] : null;
			},
			addEventListener(type, fn) {
				(eventListeners[type] ||= []).push(fn);
			},
			querySelector(sel) {
				for (const c of descendants(el)) {
					if (matches(c, sel)) return c;
				}
				return null;
			},
			querySelectorAll(sel) {
				const out = [];
				for (const c of descendants(el)) {
					if (matches(c, sel)) out.push(c);
				}
				return out;
			},
			focus() {},
			click() {
				for (const fn of eventListeners.click || []) fn({});
			},
		};
		return el;
	}

	function descendants(n) {
		const out = [];
		function walk(node) {
			for (const c of node.children) {
				out.push(c);
				walk(c);
			}
		}
		walk(n);
		return out;
	}

	function matches(el, sel) {
		if (sel.startsWith(".")) return el.className.split(/\s+/).includes(sel.slice(1));
		if (sel.startsWith("#")) return el.id === sel.slice(1);
		return el.tagName.toLowerCase() === sel.toLowerCase();
	}

	const documentElement = makeElement("html");
	const body = makeElement("body");
	const document = {
		createElement(tag) {
			return makeElement(tag);
		},
		getElementById(id) {
			if (!byId.has(id)) {
				const el = makeElement("div", id);
				byId.set(id, el);
			}
			return byId.get(id);
		},
		documentElement,
		body,
		addEventListener() {},
	};

	const localStorage = {
		getItem(k) {
			return storage.has(k) ? storage.get(k) : null;
		},
		setItem(k, v) {
			storage.set(k, String(v));
		},
		removeItem(k) {
			storage.delete(k);
		},
	};

	const fetch = async (url) => {
		fetchCalls.push(String(url));
		return {
			status: 200,
			json: async () => ({
				chart: {
					result: [
						{
							meta: { regularMarketPrice: 100, chartPreviousClose: 95, currency: "USD" },
							indicators: { quote: [{ close: [95, 96, 100] }] },
							timestamp: [1, 2, 3],
						},
					],
				},
			}),
		};
	};

	const timers = [];
	const context = {
		console,
		setInterval: (fn, ms) => {
			const id = setInterval(fn, ms);
			timers.push(id);
			return id;
		},
		clearInterval: (id) => clearInterval(id),
		localStorage,
		fetch,
		document,
		addEventListener() {},
		iCUE: { iCUELanguage: "en", defaultTemperatureUnit: () => "°C" },
		device: { deviceId: "d-1" },
		uniqueId: "u-1",
		holdings: "AAPL:10,MSFT:5",
		period: "1d",
		refreshSeconds: 60,
		textColor: "#F2F4F8",
		accentColor: "#FFD200",
		backgroundColor: "#12151C",
		upColor: "#2ECC71",
		downColor: "#FF5C5C",
		iCUE_initialized: false,
	};
	context.window = context;

	vm.createContext(context);
	vm.runInContext(SOURCE, context);

	function clear() {
		for (const id of timers) clearInterval(id);
		timers.length = 0;
	}

	return {
		context,
		byId,
		fetchCalls,
		storage,
		clear,
	};
}

test("portfolio initializes and period taps change state", () => {
	const env = makeWidgetEnv();
	assert.equal(typeof env.context.icueEvents.onICUEInitialized, "function");
	env.context.icueEvents.onICUEInitialized();

	const nav = env.byId.get("periods");
	assert.ok(nav && nav.children.length === 6, "six period buttons rendered");
	assert.equal(nav.children[0].getAttribute("aria-pressed"), "true");

	// Tap the third button (1M).
	nav.children[2].click();
	assert.equal(nav.children[2].getAttribute("aria-pressed"), "true");
	assert.equal(nav.children[0].getAttribute("aria-pressed"), "false");
	env.clear();
});

test("edit button opens the holdings editor", () => {
	const env = makeWidgetEnv();
	env.context.icueEvents.onICUEInitialized();

	const editor = env.byId.get("editor");
	assert.equal(editor.hidden, true);
	env.byId.get("edit").click();
	assert.equal(editor.hidden, false);
	assert.ok(env.byId.get("editor-rows").children.length >= 1);
	env.clear();
});

test("saving the editor updates holdings and refreshes", () => {
	const env = makeWidgetEnv();
	env.context.icueEvents.onICUEInitialized();
	env.byId.get("edit").click();

	const rows = env.byId.get("editor-rows").children;
	assert.ok(rows.length >= 1);
	const first = rows[0];
	first.querySelector(".ticker").value = "TSLA";
	first.querySelector(".shares").value = "7";

	env.byId.get("save").click();
	assert.equal(env.byId.get("editor").hidden, true);
	// Holdings table should now have a TSLA row.
	const rowTexts = env.byId.get("rows").children.map((tr) => tr.textContent);
	assert.ok(rowTexts.some((t) => t.includes("TSLA")));
	env.clear();
});
