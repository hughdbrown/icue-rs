// Coin Ticker: live spot prices from the CoinPrices Rhai add-in
// (edgehost.coinprices:CoinPrices:1.0, examples/addins/dev.edgehost.coin-prices).
//
// Settings: pairs ("BTC-USD, ETH-USD"), colours. The add-in polls Coinbase every 30 s and emits
// priceChanged(pair, price); this widget only watches pairs and draws rows.
(function () {
	"use strict";

	const MAX_PAIRS = 6;
	const $ = (id) => document.getElementById(id);
	const api = () => window.plugins && window.plugins.Coinprices;

	/** "btc-usd, ETH-USD;bad" → ["BTC-USD", "ETH-USD"]: valid, unique, at most MAX_PAIRS. */
	function parsePairs(raw) {
		const out = [];
		for (const part of String(raw || "").split(/[\s,;]+/)) {
			const p = part.trim().toUpperCase();
			if (/^[A-Z0-9]+-[A-Z0-9]+$/.test(p) && !out.includes(p)) out.push(p);
		}
		return out.slice(0, MAX_PAIRS);
	}

	/** Enough digits to see movement: 81766.45 → "81,766.45", 0.0123456 → "0.012346". */
	function formatPrice(n) {
		if (typeof n !== "number" || !Number.isFinite(n)) return "—";
		const digits = n >= 100 ? 2 : n >= 1 ? 4 : 6;
		return n.toLocaleString("en-US", { minimumFractionDigits: Math.min(digits, 2), maximumFractionDigits: digits });
	}

	let pairs = [];
	const rows = new Map(); // pair → { price, trend: "up" | "down" | "", error }

	function render() {
		const list = $("list");
		const items = pairs.map((pair) => {
			const r = rows.get(pair) || { price: null, trend: "", error: "" };
			const li = document.createElement("li");
			li.className = [r.trend, r.error ? "error" : ""].filter(Boolean).join(" ");
			const cell = (cls, text) => {
				const span = document.createElement("span");
				span.className = cls;
				span.textContent = text;
				return span;
			};
			li.append(cell("pair", pair), cell("price", formatPrice(r.price)), cell("arrow", r.trend === "up" ? "▲" : r.trend === "down" ? "▼" : ""));
			if (r.error) li.title = r.error;
			return li;
		});
		list.replaceChildren(...items);
	}

	function message(text) {
		$("message").textContent = text;
		$("message").hidden = !text;
	}

	function onPrice(pair, price) {
		const old = rows.get(pair);
		const trend = old && typeof old.price === "number" && price !== old.price ? (price > old.price ? "up" : "down") : old ? old.trend : "";
		rows.set(pair, { price, trend, error: "" });
		render();
	}

	function onError(pair, text) {
		const old = rows.get(pair) || { price: null, trend: "" };
		rows.set(pair, { ...old, error: String(text) });
		render();
	}

	function connect() {
		const p = api();
		if (!p || connect.done) return;
		connect.done = true;
		p.priceChanged.connect(onPrice);
		p.priceError.connect(onError);
	}

	function applyColors() {
		const root = document.documentElement.style;
		if (typeof textColor !== "undefined") root.setProperty("--fg", textColor);
		if (typeof upColor !== "undefined") root.setProperty("--up", upColor);
		if (typeof downColor !== "undefined") root.setProperty("--down", downColor);
		if (typeof backgroundColor !== "undefined") root.setProperty("--bg", backgroundColor);
	}

	function onReady() {
		applyColors();
		const p = api();
		if (!p) {
			pairs = [];
			render();
			message("Needs the CoinPrices add-in: edgectl addin install examples/addins/dev.edgehost.coin-prices");
			return;
		}
		connect();
		const wanted = parsePairs(typeof window.pairs === "string" ? window.pairs : "");
		for (const old of pairs) if (!wanted.includes(old)) p.unwatch(old).catch(() => {});
		for (const pair of wanted) {
			if (!pairs.includes(pair)) p.watch(pair).catch((e) => onError(pair, e.message || e));
		}
		for (const old of rows.keys()) if (!wanted.includes(old)) rows.delete(old);
		pairs = wanted;
		message(pairs.length ? "" : "Add pairs such as BTC-USD in the widget settings.");
		render();
	}

	window.pluginCoinpricesEvents = { onInitialized: connect };
	window.icueEvents = { onICUEInitialized: onReady, onDataUpdated: onReady };
	window.CoinTicker = { parsePairs, formatPrice };
})();
