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
let quotesRange = "1d"; // period the current quotes were fetched for
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
	return {
		symbol,
		price,
		base,
		time: meta.regularMarketTime,
		points,
		currency: meta.currency || "USD",
		gmtoffset: Number.isFinite(meta.gmtoffset) ? meta.gmtoffset : -5 * 3600,
	};
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
	quotesRange = range;
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

// Portfolio value over the period as [{t, v}]: the base value, then at each bar time shares × last
// known close (base before a ticker's first bar), then the current value "now".
function portfolioSeries(rows) {
	const ok = rows.filter((r) => r.q && !r.q.error);
	if (ok.length === 0) return [];
	const times = [...new Set(ok.flatMap((r) => r.q.points.map((p) => p[0])))].sort((a, b) => a - b);
	const cursor = ok.map(() => 0);
	const last = ok.map((r) => r.q.base);
	const nowSec = Math.floor(Date.now() / 1000);
	const series = [{ t: times.length ? times[0] : nowSec, v: ok.reduce((s, r) => s + r.shares * r.q.base, 0) }];
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
		series.push({ t, v: total });
	}
	series.push({ t: Math.max(nowSec, times.length ? times[times.length - 1] : nowSec), v: ok.reduce((s, r) => s + r.value, 0) });
	return series;
}

// ---------- timeline ----------

const ET = "America/New_York";
const etFormat = (options) => new Intl.DateTimeFormat("en-US", { timeZone: ET, ...options });
const etDay = etFormat({ year: "numeric", month: "2-digit", day: "2-digit" });
const etMonth = etFormat({ year: "numeric", month: "2-digit" });
const LABELS = {
	hour: etFormat({ hour: "numeric" }),
	weekday: etFormat({ weekday: "short" }),
	monthDay: etFormat({ month: "short", day: "numeric" }),
	month: etFormat({ month: "short" }),
	monthYear: etFormat({ month: "short", year: "numeric" }),
};
const at = (t) => new Date(t * 1000);

// Monday-based week number of an ET calendar date.
function etWeek(t) {
	const [m, d, y] = etDay.format(at(t)).split("/").map(Number);
	const days = Math.floor(Date.UTC(y, m - 1, d) / 86400000);
	return Math.floor((days + 3) / 7); // 1970-01-01 was a Thursday
}

// x position (0..1000) of each series point. 1D uses real session time (9:30–16:00 ET of the
// bars' day) so a mid-session line stops at "now"; longer periods space bars evenly, which
// compresses nights and weekends as finance charts do.
function xScale(series, range, gmtoffset) {
	if (range === "1d" && series.length > 2) {
		const local = series[1].t + gmtoffset;
		const midnight = Math.floor(local / 86400) * 86400 - gmtoffset;
		const start = midnight + 9.5 * 3600;
		const end = midnight + 16 * 3600;
		series[0].t = start;
		series[series.length - 1].t = Math.min(series[series.length - 1].t, end);
		const x = (p) => (Math.min(Math.max((p.t - start) / (end - start), 0), 1)) * 1000;
		return { x, byTime: true, start, end };
	}
	const n = series.length;
	return { x: (_p, i) => (n > 1 ? (i / (n - 1)) * 1000 : 0), byTime: false };
}

// Tick marks [{x, label}] for the period, thinned to fit `width` pixels.
function timelineTicks(series, scale, range, width) {
	const ticks = [];
	if (scale.byTime) {
		for (let h = 10; h <= 16; h += 1) {
			const t = scale.start + (h - 9.5) * 3600;
			ticks.push({ x: scale.x({ t }), label: LABELS.hour.format(at(t)) });
		}
	} else if (series.length > 1) {
		// Months show the year on the first tick and wherever the year changes ("Oct 2025 … Feb 2026 …").
		const year = (t) => etMonth.format(at(t)).slice(-4);
		const monthLabel = (t, previousT) =>
			(previousT === undefined || year(t) !== year(previousT) ? LABELS.monthYear : LABELS.month).format(at(t));
		const [keyOf, label] = {
			"5d": [(t) => etDay.format(at(t)), (t) => LABELS.weekday.format(at(t))],
			"1mo": [etWeek, (t) => LABELS.monthDay.format(at(t))],
		}[range] || [(t) => etMonth.format(at(t)), monthLabel];
		let previous = null;
		// Skip the base point (index 0): it belongs to the period before.
		for (let i = 1; i < series.length - 1; i += 1) {
			const key = keyOf(series[i].t);
			if (key !== previous) {
				ticks.push({ x: scale.x(series[i], i), t: series[i].t });
				previous = key;
			}
		}
		const kept = thin(ticks, width);
		return kept.map((tick, i) => ({ x: tick.x, label: label(tick.t, i === 0 ? undefined : kept[i - 1].t) }));
	}
	return thin(ticks, width);
}

// Keep every k-th tick so labels (~84 px each) fit in `width`.
function thin(ticks, width) {
	const maxLabels = Math.max(3, Math.floor(width / 84));
	if (ticks.length <= maxLabels) return ticks;
	const step = Math.ceil(ticks.length / maxLabels);
	return ticks.filter((_, i) => i % step === 0);
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

// "Nice" value-axis ticks: a step of 1, 2, 2.5 or 5 × 10^k giving about `count` intervals.
function niceTicks(min, max, count) {
	const span = max - min || Math.abs(max) * 0.01 || 1;
	const raw = span / count;
	const magnitude = 10 ** Math.floor(Math.log10(raw));
	const step = [1, 2, 2.5, 5, 10].map((m) => m * magnitude).find((m) => m >= raw) || 10 * magnitude;
	const lo = Math.floor(min / step) * step;
	const hi = Math.max(Math.ceil(max / step) * step, lo + step);
	const ticks = [];
	for (let v = lo; v <= hi + step / 2; v += step) ticks.push(Number(v.toFixed(10)));
	return { lo, hi, step, ticks };
}

const axisUsd = (step) =>
	new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: step < 1 ? 2 : 0, minimumFractionDigits: step < 1 ? 2 : 0 });

// The last drawn chart, for the scrubbing cursor.
let chart = null;

function renderChart(series, trend, range) {
	const line = $("line");
	const area = $("area");
	const baseline = $("baseline");
	const grid = $("grid");
	const hgrid = $("hgrid");
	const timeline = $("timeline");
	const yaxis = $("yaxis");
	document.documentElement.style.setProperty("--trend", trend < 0 ? "var(--down)" : "var(--up)");
	hideCursor();
	if (series.length < 2) {
		chart = null;
		line.setAttribute("points", "");
		area.setAttribute("d", "");
		for (const el of [grid, hgrid, timeline, yaxis]) el.replaceChildren();
		return;
	}
	const gmtoffset = (Object.values(quotes).find((q) => !q.error) || {}).gmtoffset ?? -5 * 3600;
	const scale = xScale(series, range, gmtoffset);
	const values = series.map((p) => p.v);
	const plotHeight = $("plot").clientHeight || 300;
	const axis = niceTicks(Math.min(...values), Math.max(...values), Math.max(2, Math.min(5, Math.floor(plotHeight / 55))));
	const y = (v) => 296 - ((v - axis.lo) / (axis.hi - axis.lo)) * 292;
	const xs = series.map((p, i) => scale.x(p, i));
	const pts = series.map((p, i) => `${xs[i].toFixed(1)},${y(p.v).toFixed(1)}`);
	line.setAttribute("points", pts.join(" "));
	area.setAttribute("d", `M${xs[0].toFixed(1)},300 L${pts.join(" L")} L${xs[xs.length - 1].toFixed(1)},300 Z`);
	const by = y(series[0].v).toFixed(1);
	baseline.setAttribute("y1", by);
	baseline.setAttribute("y2", by);
	chart = { series, xs, ys: series.map((p) => y(p.v)), range, base: series[0].v };

	const svgNs = "http://www.w3.org/2000/svg";
	const svgLine = (x1, y1, x2, y2) => {
		const l = document.createElementNS(svgNs, "line");
		l.setAttribute("x1", x1);
		l.setAttribute("y1", y1);
		l.setAttribute("x2", x2);
		l.setAttribute("y2", y2);
		return l;
	};
	const ticks = timelineTicks(series, scale, range, timeline.clientWidth || 600);
	grid.replaceChildren(...ticks.map((tick) => svgLine(tick.x.toFixed(1), 0, tick.x.toFixed(1), 300)));
	hgrid.replaceChildren(...axis.ticks.map((v) => svgLine(0, y(v).toFixed(1), 1000, y(v).toFixed(1))));
	timeline.replaceChildren(
		...ticks.map((tick) => {
			const span = document.createElement("span");
			span.textContent = tick.label;
			span.style.left = `${tick.x / 10}%`;
			if (tick.x < 40) span.className = "first";
			else if (tick.x > 960) span.className = "last";
			return span;
		}),
	);
	const fmt = axisUsd(axis.step);
	yaxis.replaceChildren(
		...axis.ticks.map((v) => {
			const span = document.createElement("span");
			span.textContent = fmt.format(v);
			span.style.top = `${(y(v) / 300) * 100}%`;
			return span;
		}),
	);
}

// ---------- scrubbing cursor ----------

const TIP_INTRADAY = etFormat({ weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
const TIP_DAILY = etFormat({ weekday: "short", month: "short", day: "numeric", year: "numeric" });
let hideTimer = null;
let dragging = false;

function hideCursor() {
	clearTimeout(hideTimer);
	$("cursor").hidden = true;
}

function showCursorAt(clientX) {
	if (!chart) return;
	const plot = $("plot");
	const box = plot.getBoundingClientRect();
	if (box.width <= 0) return;
	const target = ((clientX - box.left) / box.width) * 1000;
	let best = 0;
	chart.xs.forEach((x, i) => {
		if (Math.abs(x - target) < Math.abs(chart.xs[best] - target)) best = i;
	});
	const point = chart.series[best];
	const last = chart.series.length - 1;
	const px = (chart.xs[best] / 1000) * box.width;
	const py = (chart.ys[best] / 300) * box.height;
	const intraday = chart.range === "1d" || chart.range === "5d";
	let when;
	if (best === 0) when = chart.range === "1d" ? "Previous close" : "Start of period";
	else when = (intraday ? `${TIP_INTRADAY.format(at(point.t))} ET` : TIP_DAILY.format(at(point.t))) + (best === last ? " · now" : "");
	const delta = point.v - chart.base;

	const cursor = $("cursor");
	cursor.hidden = false;
	cursor.querySelector(".cursor-line").style.left = `${px}px`;
	const dot = cursor.querySelector(".cursor-dot");
	dot.style.left = `${px}px`;
	dot.style.top = `${py}px`;
	const tip = cursor.querySelector(".cursor-tip");
	tip.querySelector(".tip-time").textContent = when;
	tip.querySelector(".tip-value").textContent = usd.format(point.v);
	const change = tip.querySelector(".tip-change");
	change.textContent = best === 0 ? "baseline" : `${signedUsd(delta)} (${signedPct(chart.base ? delta / chart.base : NaN)})`;
	change.className = `tip-change ${best === 0 ? "flat" : trendClass(delta)}`;
	const flip = px > box.width * 0.6;
	tip.style.left = flip ? "" : `${px + 12}px`;
	tip.style.right = flip ? `${box.width - px + 12}px` : "";
}

function wireCursor() {
	const plot = $("plot");
	plot.addEventListener("pointerdown", (e) => {
		dragging = true;
		try {
			plot.setPointerCapture(e.pointerId);
		} catch (_) {
			// Capture is best-effort; the drag still works inside the plot.
		}
		clearTimeout(hideTimer);
		showCursorAt(e.clientX);
	});
	plot.addEventListener("pointermove", (e) => {
		if (dragging || e.pointerType === "mouse") {
			clearTimeout(hideTimer);
			showCursorAt(e.clientX);
		}
	});
	const release = () => {
		dragging = false;
		clearTimeout(hideTimer);
		hideTimer = setTimeout(hideCursor, 3000);
	};
	plot.addEventListener("pointerup", release);
	plot.addEventListener("pointercancel", release);
	plot.addEventListener("pointerleave", () => {
		if (!dragging) release();
	});
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

	renderChart(portfolioSeries(rows), change, quotesRange);

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
	wireCursor();
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
