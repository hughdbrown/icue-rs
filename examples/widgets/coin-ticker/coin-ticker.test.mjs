// Tests for the Coin Ticker widget: parsing, formatting and the CoinPrices plugin wiring.
// Run with: node --test examples/widgets/coin-ticker/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("./scripts/coin-ticker.js", import.meta.url), "utf8");
const plain = (v) => JSON.parse(JSON.stringify(v));

function el() {
	const e = {
		className: "",
		textContent: "",
		title: "",
		hidden: false,
		children: [],
		style: { setProperty() {} },
		append: (...c) => e.children.push(...c),
		replaceChildren: (...c) => (e.children = c),
	};
	return e;
}

// A fake window.plugins.Coinprices that records calls and lets tests fire signals.
function fakePlugin() {
	const handlers = { priceChanged: [], priceError: [] };
	const calls = [];
	const signal = (name) => ({ connect: (f) => handlers[name].push(f), disconnect() {} });
	return {
		calls,
		fire: (name, ...args) => handlers[name].forEach((f) => f(...args)),
		api: {
			watch: async (p) => calls.push(["watch", p]),
			unwatch: async (p) => calls.push(["unwatch", p]),
			getPrice: async () => 1,
			priceChanged: signal("priceChanged"),
			priceError: signal("priceError"),
		},
	};
}

function load({ plugin, pairs = "BTC-USD, eth-usd" } = {}) {
	const byId = new Map();
	const document = {
		getElementById: (id) => byId.get(id) || byId.set(id, el()).get(id),
		createElement: () => el(),
		documentElement: el(),
	};
	const ctx = { document, pairs, plugins: plugin ? { Coinprices: plugin.api } : {} };
	ctx.window = ctx;
	vm.createContext(ctx);
	vm.runInContext(SOURCE, ctx);
	const rows = () => byId.get("list").children.map((li) => ({ cls: li.className, text: li.children.map((c) => c.textContent) }));
	return { ctx, byId, rows };
}

test("parsePairs normalises, de-duplicates, drops junk and caps at six", () => {
	const { parsePairs } = load().ctx.CoinTicker;
	assert.deepEqual(plain(parsePairs("btc-usd, ETH-USD;BTC-USD  bad x-y-z")), ["BTC-USD", "ETH-USD"]);
	assert.deepEqual(plain(parsePairs("")), []);
	assert.equal(parsePairs("A-B C-D E-F G-H I-J K-L M-N").length, 6);
});

test("formatPrice keeps enough digits to show movement", () => {
	const { formatPrice } = load().ctx.CoinTicker;
	assert.equal(formatPrice(81766.445), "81,766.45");
	assert.equal(formatPrice(2.5), "2.50");
	assert.equal(formatPrice(0.0123456), "0.012346");
	assert.equal(formatPrice(null), "—");
});

test("watches each pair and draws rising and falling prices", async () => {
	const plugin = fakePlugin();
	const env = load({ plugin });
	env.ctx.icueEvents.onICUEInitialized();
	await Promise.resolve();
	assert.deepEqual(plain(plugin.calls), [["watch", "BTC-USD"], ["watch", "ETH-USD"]]);
	plugin.fire("priceChanged", "BTC-USD", 100);
	plugin.fire("priceChanged", "ETH-USD", 10);
	plugin.fire("priceChanged", "BTC-USD", 120);
	plugin.fire("priceChanged", "ETH-USD", 9);
	assert.deepEqual(plain(env.rows()), [
		{ cls: "up", text: ["BTC-USD", "120.00", "▲"] },
		{ cls: "down", text: ["ETH-USD", "9.00", "▼"] },
	]);
	plugin.fire("priceError", "ETH-USD", "HTTP 503");
	assert.equal(env.rows()[1].cls, "down error");
});

test("changing the setting unwatches removed pairs and watches new ones", async () => {
	const plugin = fakePlugin();
	const env = load({ plugin });
	env.ctx.icueEvents.onICUEInitialized();
	env.ctx.pairs = "ETH-USD, SOL-USD";
	env.ctx.icueEvents.onDataUpdated();
	assert.deepEqual(plain(plugin.calls.slice(2)), [["unwatch", "BTC-USD"], ["watch", "SOL-USD"]]);
	assert.deepEqual(plain(env.rows().map((r) => r.text[0])), ["ETH-USD", "SOL-USD"]);
});

test("without the add-in it explains what to install", () => {
	const env = load();
	env.ctx.icueEvents.onICUEInitialized();
	assert.equal(env.byId.get("message").hidden, false);
	assert.match(env.byId.get("message").textContent, /edgectl addin install/);
});
