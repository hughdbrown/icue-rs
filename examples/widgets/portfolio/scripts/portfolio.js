// Stock Portfolio widget (iCUE Widget API 1.x).
//
// Quotes come from Yahoo Finance's chart API. Under EdgeHost, fetch() to the domain declared in
// manifest.json is performed by the host, so the API's missing CORS headers do not matter.
//
// State (holdings edited on the widget, selected period) is kept in localStorage under uniqueId.
// The "holdings" and "period" settings seed it; changing those settings re-seeds it.

icueEvents = {
	onICUEInitialized: init,
	onDataUpdated: onSettingsChanged,
};

const PERIODS = [
	["1d", "1D", "5m"],
	["5d", "5D", "30m"],
	["1mo", "1M", "1d"],
	["6mo", "6M", "1d"],
	["ytd", "YTD", "1d"],
	["1y", "1Y", "1d"],
];
const INTERVAL = Object.fromEntries(PERIODS.map(([key, , interval]) => [key, interval]));
const TICKER = /^[A-Z][A-Z0-9.\-]{0,9}$/;
const MAX_HOLDINGS = 40;

const $ = (id) => document.getElementById(id);
const usd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const sharesFmt = new Intl.NumberFormat("en-US", { maximumFractionDigits: 4 });
const signedUsd = (v) => `${v > 0 ? "+" : v < 0 ? "−" : ""}${usd.format(Math.abs(v))}`;
const signedPct = (v) => (Number.isFinite(v) ? `${v > 0 ? "+" : v < 0 ? "−" : ""}${Math.abs(v * 100).toFixed(2)}%` : "");
const trendClass = (v) => (v > 0 ? "up" : v < 0 ? "down" : "flat");
const setting = (name, fallback) => (typeof window[name] === "undefined" || window[name] === null ? fallback : window[name]);

let state = null;
let quotes = {};
let timer = null;
let requestSeq = 0;

// ---------- state ----------

function parseHoldings(text) {
	const out = [];
	for (const entry of String(text || "").split(/[,;\n]+/)) {
		const [sym, qty] = entry.trim().split(/[\s:=]+/);
		const symbol = (sym || "").toUpperCase();
		const shares = Number(qty);
		if (TICKER.test(symbol) && Number.isFinite(shares) && shares > 0) out.push({ symbol, shares });
	}
	return out.slice(0, MAX_HOLDINGS);
}

function loadStored() {
	try {
		return JSON.parse(localStorage.getItem(uniqueId) || "null");
	} catch (_) {
		return null;
	}
}

function save() {
	try {
		localStorage.setItem(uniqueId, JSON.stringify(state));
	} catch (e) {
		console.warn("could not save portfolio", e);
	}
}

function seededState() {
	const seed = String(setting("holdings", ""));
	const seedPeriod = INTERVAL[setting("period", "1d")] ? setting("period", "1d") : "1d";
	const stored = loadStored();
	const fresh = { seed, seedPeriod, holdings: parseHoldings(seed), period: seedPeriod };
	if (!stored || !Array.isArray(stored.holdings)) return fresh;
	// Settings changed since the widget last saw them: they win.
	if (stored.seed !== seed) return { ...fresh, period: stored.period };
	if (stored.seedPeriod !== seedPeriod) return { ...stored, seedPeriod, period: seedPeriod };
	return stored;
}

// ---------- data ----------

async function fetchSeries(symbol, range) {
	const url =
		`https://query1.finance.yahoo.com/v8/finance/chart/${encodeURIComponent(symbol)}` +
		`?range=${range}&interval=${INTERVAL[range]}&includePrePost=false`;
	const response = await fetch(url);
	let body = null;
	try {
		body = await response.json();
	} catch (_) {
		throw new Error(`HTTP ${response.status}`);
	}
	const result = body && body.chart && body.chart.result && body.chart.result[0];
	if (!result) {
		const err = body && body.chart && body.chart.error;
		throw new Error((err && err.description) || `HTTP ${response.status}`);
	}
	const meta = result.meta || {};
	const quote = (result.indicators && result.indicators.quote && result.indicators.quote[0]) || {};
	const closes = quote.close || [];
	const points = (result.timestamp || []).map((t, i) => [t, closes[i]]).filter((p) => Number.isFinite(p[1]));
	const last = points.length ? points[points.length - 1][1] : NaN;
	const price = Number.isFinite(meta.regularMarketPrice) ? meta.regularMarketPrice : last;
	let base = meta.chartPreviousClose;
	if (range === "1d" && Number.isFinite(meta.previousClose)) base = meta.previousClose;
	if (!Number.isFinite(base)) base = points.length ? points[0][1] : price;
	if (!Number.isFinite(price)) throw new Error("no price");
	return { symbol, price, base, time: meta.regularMarketTime, points, currency: meta.currency || "USD" };
}

async function refresh() {
	if (!state || state.holdings.length === 0) {
		quotes = {};
		render();
		return;
	}
	const seq = ++requestSeq;
	setStatus("Updating…");
	const range = state.period;
	const results = await Promise.allSettled(state.holdings.map((h) => fetchSeries(h.symbol, range)));
	if (seq !== requestSeq) return; // a newer refresh (e.g. period change) superseded this one
	quotes = {};
	results.forEach((r, i) => {
		const symbol = state.holdings[i].symbol;
		quotes[symbol] = r.status === "fulfilled" ? r.value : { symbol, error: String((r.reason && r.reason.message) || r.reason) };
	});
	const failed = Object.values(quotes).filter((q) => q.error);
	console.log(`portfolio ${range}: ${state.holdings.length - failed.length}/${state.holdings.length} quotes` + (failed.length ? `; failed: ${failed.map((q) => `${q.symbol} (${q.error})`).join(", ")}` : ""));
	render();
}

function rowsWithQuotes() {
	return state.holdings
		.map((h) => {
			const q = quotes[h.symbol];
			if (!q || q.error) return { ...h, q, value: NaN, change: NaN };
			const value = h.shares * q.price;
			const change = h.shares * (q.price - q.base);
			return { ...h, q, value, change, pct: q.base ? q.price / q.base - 1 : NaN };
		})
		.sort((a, b) => (Number.isFinite(b.value) ? b.value : -1) - (Number.isFinite(a.value) ? a.value : -1));
}

// Portfolio value over the period: at each timestamp, shares × last known close (base before the
// first bar), starting from the period's base value and ending at the current value.
function portfolioSeries(rows) {
	const ok = rows.filter((r) => r.q && !r.q.error);
	if (ok.length === 0) return [];
	const times = [...new Set(ok.flatMap((r) => r.q.points.map((p) => p[0])))].sort((a, b) => a - b);
	const cursor = ok.map(() => 0);
	const last = ok.map((r) => r.q.base);
	const series = [ok.reduce((s, r) => s + r.shares * r.q.base, 0)];
	for (const t of times) {
		let total = 0;
		ok.forEach((r, k) => {
			const pts = r.q.points;
			while (cursor[k] < pts.length && pts[cursor[k]][0] <= t) {
				last[k] = pts[cursor[k]][1];
				cursor[k] += 1;
			}
			total += r.shares * last[k];
		});
		series.push(total);
	}
	series.push(ok.reduce((s, r) => s + r.value, 0));
	return series;
}

// ---------- rendering ----------

function setStatus(text) {
	$("status").textContent = text;
}

function applyTheme() {
	const s = document.documentElement.style;
	s.setProperty("--fg", setting("textColor", "#F2F4F8"));
	s.setProperty("--accent", setting("accentColor", "#FFD200"));
	s.setProperty("--bg", setting("backgroundColor", "#12151C"));
	s.setProperty("--up", setting("upColor", "#2ECC71"));
	s.setProperty("--down", setting("downColor", "#FF5C5C"));
}

function renderPeriods() {
	const nav = $("periods");
	nav.replaceChildren(
		...PERIODS.map(([key, label]) => {
			const b = document.createElement("button");
			b.type = "button";
			b.textContent = label;
			b.setAttribute("aria-pressed", String(key === state.period));
			b.addEventListener("click", () => {
				if (state.period === key) return;
				state.period = key;
				save();
				renderPeriods();
				refresh();
			});
			return b;
		}),
	);
}

function renderChart(series, trend) {
	const line = $("line");
	const area = $("area");
	const baseline = $("baseline");
	document.documentElement.style.setProperty("--trend", trend < 0 ? "var(--down)" : "var(--up)");
	if (series.length < 2) {
		line.setAttribute("points", "");
		area.setAttribute("d", "");
		return;
	}
	const min = Math.min(...series);
	const max = Math.max(...series);
	const span = max - min || 1;
	const x = (i) => (i / (series.length - 1)) * 1000;
	const y = (v) => 290 - ((v - min) / span) * 280;
	const pts = series.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`);
	line.setAttribute("points", pts.join(" "));
	area.setAttribute("d", `M0,300 L${pts.join(" L")} L1000,300 Z`);
	const by = y(series[0]).toFixed(1);
	baseline.setAttribute("y1", by);
	baseline.setAttribute("y2", by);
}

function cell(text, className) {
	const td = document.createElement("td");
	td.textContent = text;
	if (className) td.className = className;
	return td;
}

function render() {
	const rows = rowsWithQuotes();
	const ok = rows.filter((r) => Number.isFinite(r.value));
	const total = ok.reduce((s, r) => s + r.value, 0);
	const change = ok.reduce((s, r) => s + r.change, 0);
	const start = total - change;
	const pct = start ? change / start : NaN;
	const periodLabel = (PERIODS.find(([k]) => k === state.period) || [, ""])[1];

	$("total").textContent = ok.length ? usd.format(total) : "—";
	const ch = $("change");
	ch.textContent = ok.length ? `${signedUsd(change)} (${signedPct(pct)}) · ${periodLabel}` : state.holdings.length ? "No quotes" : "Tap Edit to add holdings";
	ch.className = ok.length ? trendClass(change) : "flat";

	$("rows").replaceChildren(
		...rows.map((r) => {
			const tr = document.createElement("tr");
			tr.append(cell(r.symbol, "sym"), cell(sharesFmt.format(r.shares), "num"));
			if (r.q && r.q.error) {
				const err = cell(r.q.error, "err");
				err.colSpan = 3;
				tr.append(err);
			} else if (!r.q) {
				tr.append(cell("…", "num"), cell("", "num"), cell("", "num"));
			} else {
				tr.append(
					cell(usd.format(r.q.price), "num"),
					cell(usd.format(r.value), "num"),
					cell(`${signedUsd(r.change)}  ${signedPct(r.pct)}`, `num ${trendClass(r.change)}`),
				);
			}
			return tr;
		}),
	);

	renderChart(portfolioSeries(rows), change);

	const latest = Math.max(0, ...ok.map((r) => r.q.time || 0));
	const failed = rows.length - ok.length;
	const asOf = latest
		? new Date(latest * 1000).toLocaleString("en-US", { timeZone: "America/New_York", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }) + " ET"
		: "";
	setStatus([asOf && `As of ${asOf}`, failed ? `${failed} ticker(s) failed` : "", "Yahoo Finance"].filter(Boolean).join(" · "));
}

// ---------- editor ----------

function editorRow(symbol = "", shares = "") {
	const row = document.createElement("div");
	row.className = "editor-row";
	const t = document.createElement("input");
	t.className = "ticker";
	t.placeholder = "Ticker";
	t.value = symbol;
	t.autocapitalize = "characters";
	t.spellcheck = false;
	const s = document.createElement("input");
	s.className = "shares";
	s.placeholder = "Shares";
	s.inputMode = "decimal";
	s.value = shares === "" ? "" : String(shares);
	const del = document.createElement("button");
	del.type = "button";
	del.textContent = "✕";
	del.title = "Remove";
	del.addEventListener("click", () => row.remove());
	row.append(t, s, del);
	return row;
}

function openEditor(holdings) {
	$("editor-error").textContent = "";
	$("editor-rows").replaceChildren(...holdings.map((h) => editorRow(h.symbol, h.shares)), editorRow());
	$("editor").hidden = false;
}

function readEditor() {
	const holdings = [];
	const problems = [];
	const seen = new Set();
	for (const row of $("editor-rows").querySelectorAll(".editor-row")) {
		const symbol = row.querySelector(".ticker").value.trim().toUpperCase();
		const sharesText = row.querySelector(".shares").value.trim().replace(/,/g, "");
		if (!symbol && !sharesText) continue;
		const shares = Number(sharesText);
		if (!TICKER.test(symbol)) problems.push(`“${symbol || "?"}” is not a ticker`);
		else if (!Number.isFinite(shares) || shares <= 0) problems.push(`${symbol}: shares must be a positive number`);
		else if (seen.has(symbol)) problems.push(`${symbol} is listed twice`);
		else {
			seen.add(symbol);
			holdings.push({ symbol, shares });
		}
	}
	if (holdings.length > MAX_HOLDINGS) problems.push(`at most ${MAX_HOLDINGS} holdings`);
	return { holdings, problems };
}

function wireEditor() {
	$("edit").addEventListener("click", () => openEditor(state.holdings));
	$("add-row").addEventListener("click", () => {
		const row = editorRow();
		$("editor-rows").append(row);
		row.querySelector("input").focus();
	});
	$("reset").addEventListener("click", () => openEditor(parseHoldings(setting("holdings", ""))));
	$("cancel").addEventListener("click", () => ($("editor").hidden = true));
	$("save").addEventListener("click", () => {
		const { holdings, problems } = readEditor();
		if (problems.length) {
			$("editor-error").textContent = problems.join(" · ");
			return;
		}
		state.holdings = holdings;
		save();
		$("editor").hidden = true;
		quotes = {};
		render();
		refresh();
	});
}

// ---------- lifecycle ----------

function restartTimer() {
	clearInterval(timer);
	const seconds = Math.max(15, Number(setting("refreshSeconds", 60)) || 60);
	timer = setInterval(refresh, seconds * 1000);
}

let initialized = false;

function init() {
	if (initialized) return;
	initialized = true;
	applyTheme();
	state = seededState();
	save();
	wireEditor();
	renderPeriods();
	render();
	refresh();
	restartTimer();
}

function onSettingsChanged() {
	if (!initialized) return;
	applyTheme();
	const next = seededState();
	const changed = next.seed !== state.seed || next.period !== state.period;
	state = next;
	save();
	renderPeriods();
	restartTimer();
	if (changed) refresh();
	else render();
}

if (typeof iCUE_initialized !== "undefined" && iCUE_initialized) {
	init();
}
