// War Thunder HUD: reads War Thunder's own local telemetry server and shows it on the XENEON EDGE.
//
// LEGAL / FAIR-PLAY BOUNDARY (Gaijin rules; overlays of your OWN localhost telemetry are tolerated,
// ESP-like information about other units is not):
//   ALLOWED endpoints only: /state, /indicators, /hudmsg?lastEvt=N&lastDmg=N, /gamechat?lastId=N,
//   /mission.json.  Every request goes through httpGet(), which refuses any other path.
//   NEVER used: /map_obj.json, /map.img, /map_info.json, positions/distance/bearing of any other
//   unit (friend or foe), screen capture, audio analysis, memory reading, or anything that
//   modifies the game. The widget is read-only and only ever sends GET requests to localhost:8111.
//
// Field names come from https://github.com/lucasvmx/WarThunder-localhost-documentation
// (State.md, Indicators.md, Hudmsg.md, GameChat.md, Mission.md). Tank-only indicator fields are
// not in that documentation and are read defensively (see TANK_* below).

icueEvents = {
	onICUEInitialized: init,
	onDataUpdated: onSettingsChanged,
};

// ---------------------------------------------------------------------------------------------
// Network (allow-list enforced here)
// ---------------------------------------------------------------------------------------------
const WT_BASE = "http://localhost:8111";
const ALLOWED_PATHS = ["/state", "/indicators", "/hudmsg", "/gamechat", "/mission.json"];

function isAllowedPath(path) {
	return ALLOWED_PATHS.includes(String(path));
}

async function httpGet(path, query) {
	if (!isAllowedPath(path)) throw new Error("endpoint not allowed: " + path);
	const r = await fetch(WT_BASE + path + (query || ""));
	if (r.ok === false) throw new Error("HTTP " + r.status);
	return r.json();
}

// ---------------------------------------------------------------------------------------------
// Pure helpers (global so tests can call them)
// ---------------------------------------------------------------------------------------------
const KMH_PER_KNOT = 1.852;
const KMH_PER_MPH = 1.609344;
const FT_PER_M = 3.28084;
const LB_PER_KG = 2.20462;

function kmhToKnots(kmh) {
	return kmh / KMH_PER_KNOT;
}
function kmhToMph(kmh) {
	return kmh / KMH_PER_MPH;
}
function metersToFeet(m) {
	return m * FT_PER_M;
}
function msToFtMin(ms) {
	return ms * FT_PER_M * 60;
}
function kgToLb(kg) {
	return kg * LB_PER_KG;
}
function celsiusToFahrenheit(c) {
	return (c * 9) / 5 + 32;
}

function isNum(x) {
	return typeof x === "number" && Number.isFinite(x);
}

function fmt(x, digits) {
	return isNum(x) ? x.toFixed(digits || 0) : "—";
}

// Airspeed: km/h or knots.
function airspeed(kmh, units) {
	if (!isNum(kmh)) return { v: null, u: units === "imperial" ? "kn" : "km/h" };
	return units === "imperial" ? { v: kmhToKnots(kmh), u: "kn" } : { v: kmh, u: "km/h" };
}
// Ground speed: km/h or mph.
function groundSpeed(kmh, units) {
	if (!isNum(kmh)) return { v: null, u: units === "imperial" ? "mph" : "km/h" };
	return units === "imperial" ? { v: kmhToMph(kmh), u: "mph" } : { v: kmh, u: "km/h" };
}
function altitude(m, units) {
	if (!isNum(m)) return { v: null, u: units === "imperial" ? "ft" : "m" };
	return units === "imperial" ? { v: metersToFeet(m), u: "ft" } : { v: m, u: "m" };
}
function verticalSpeed(ms, units) {
	if (!isNum(ms)) return { v: null, u: units === "imperial" ? "ft/min" : "m/s" };
	return units === "imperial" ? { v: msToFtMin(ms), u: "ft/min" } : { v: ms, u: "m/s" };
}
function temperature(c, units) {
	if (!isNum(c)) return { v: null, u: units === "imperial" ? "°F" : "°C" };
	return units === "imperial" ? { v: celsiusToFahrenheit(c), u: "°F" } : { v: c, u: "°C" };
}
function fuelMass(kg, units) {
	if (!isNum(kg)) return { v: null, u: units === "imperial" ? "lb" : "kg" };
	return units === "imperial" ? { v: kgToLb(kg), u: "lb" } : { v: kg, u: "kg" };
}

// Least-squares slope of y over x for [{x, y}] points; null if degenerate.
function linregSlope(points) {
	const n = points.length;
	if (n < 2) return null;
	let sx = 0;
	let sy = 0;
	for (const p of points) {
		sx += p.x;
		sy += p.y;
	}
	const mx = sx / n;
	const my = sy / n;
	let num = 0;
	let den = 0;
	for (const p of points) {
		num += (p.x - mx) * (p.y - my);
		den += (p.x - mx) * (p.x - mx);
	}
	return den === 0 ? null : num / den;
}

// Fuel burn from samples [{t: ms, kg}] over a sliding window (default 60 s).
// Returns { burnKgPerMin, minutes }; minutes is null when fuel is not decreasing.
function estimateFuel(samples, windowMs) {
	const win = windowMs || 60000;
	if (!samples || samples.length < 2) return { burnKgPerMin: null, minutes: null };
	const last = samples[samples.length - 1];
	const recent = samples.filter((s) => last.t - s.t <= win);
	if (recent.length < 2 || last.t - recent[0].t < 5000) return { burnKgPerMin: null, minutes: null };
	const slope = linregSlope(recent.map((s) => ({ x: s.t / 60000, y: s.kg }))); // kg per minute
	if (slope === null || slope >= -0.001) return { burnKgPerMin: 0, minutes: null };
	const burn = -slope;
	return { burnKgPerMin: burn, minutes: last.kg / burn };
}

// Seconds until `limit` is reached, from the last ~10 s of samples [{t: ms, v}] by linear
// regression. 0 if already at/over the limit; null if not rising, too little data, or > 1 hour.
function estimateTimeToLimit(samples, limit, windowMs) {
	if (!samples || samples.length === 0 || !isNum(limit)) return null;
	const last = samples[samples.length - 1];
	if (last.v >= limit) return 0;
	const win = windowMs || 10000;
	const recent = samples.filter((s) => last.t - s.t <= win);
	if (recent.length < 3 || last.t - recent[0].t < 2000) return null;
	const slope = linregSlope(recent.map((s) => ({ x: s.t / 1000, y: s.v }))); // per second
	if (slope === null || slope <= 0.01) return null;
	const secs = (limit - last.v) / slope;
	return secs > 3600 ? null : secs;
}

// Append a sample and drop those older than keepMs.
function pushSample(list, sample, keepMs) {
	list.push(sample);
	while (list.length > 1 && sample.t - list[0].t > keepMs) list.shift();
	return list;
}

// Incremental merge of /hudmsg or /gamechat entries into a newest-first list, deduplicated by
// (kind, id). Returns { list, added } where `added` are the new entries in ascending id order.
function mergeHud(existing, incoming, kind, max, startSeq) {
	const limit = max || 30;
	const seen = new Set(existing.map((e) => e.kind + ":" + e.id));
	const fresh = [];
	const sorted = (incoming || []).filter((m) => m && isNum(m.id)).sort((a, b) => a.id - b.id);
	let seq = startSeq || (existing.length ? existing[0].seq + 1 : 1);
	for (const m of sorted) {
		const key = kind + ":" + m.id;
		if (seen.has(key)) continue;
		seen.add(key);
		fresh.push({
			kind,
			id: m.id,
			seq: seq++,
			msg: String(m.msg == null ? "" : m.msg),
			sender: String(m.sender == null ? "" : m.sender),
			mode: String(m.mode == null ? "" : m.mode),
		});
	}
	const list = fresh.slice().reverse().concat(existing).slice(0, limit);
	return { list, added: fresh };
}

function maxId(entries, kind, fallback) {
	let m = fallback;
	for (const e of entries) if (e.kind === kind && e.id > m) m = e.id;
	return m;
}

// Kill-feed heuristics. HUD damage lines look like (Hudmsg.md):
//   "ActorName (Vehicle) <verb> TargetName (Vehicle)"   and   "Name (Vehicle) has crashed."
// Classification:
//   kill    verbs "shot down", "destroyed"
//   damage  verbs "set afire", "severely damaged", "critically damaged", "damaged"
//   crash   "<victim> has crashed" / "has been wrecked" / "bailed out" / "disconnected" (no killer)
//   assist  message contains the word "assist" (not seen in the docs; best effort)
//   other   anything else
// `mine` is true when playerName appears in the actor or victim (case-insensitive, ignoring a
// leading "*" and clan tags such as "[TAG]"). Counters: actor+kill => kills; victim+kill or
// own crash => deaths; own assist => assists. Damage verbs are not assists.
const KILL_RE = /^(.*?)\s+\(([^)]*)\)\s+(shot down|destroyed)\s+(.*?)\s*(?:\(([^)]*)\))?\.?$/i;
const DAMAGE_RE = /^(.*?)\s+\(([^)]*)\)\s+(set afire|severely damaged|critically damaged|damaged)\s+(.*?)\s*(?:\(([^)]*)\))?\.?$/i;
const CRASH_RE = /^(.*?)\s+\(([^)]*)\)\s+(has crashed|has been wrecked|bailed out|has disconnected)/i;

function normName(s) {
	return String(s || "")
		.replace(/^\*+/, "")
		.replace(/^[\[(<][^\])>]*[\])>]\s*/, "")
		.trim()
		.toLowerCase();
}

function nameMatches(candidate, player) {
	const p = normName(player);
	if (!p) return false;
	const c = normName(candidate);
	return c === p || c.endsWith(" " + p) || c.startsWith(p + " ");
}

function parseKillLine(msg, player) {
	const text = String(msg || "").trim();
	let m = KILL_RE.exec(text);
	if (m) {
		const actorMine = nameMatches(m[1], player);
		const victimMine = nameMatches(m[4], player);
		return { kind: "kill", actor: m[1], victim: m[4], mine: actorMine || victimMine, actorMine, victimMine };
	}
	m = DAMAGE_RE.exec(text);
	if (m) {
		const actorMine = nameMatches(m[1], player);
		const victimMine = nameMatches(m[4], player);
		return { kind: "damage", actor: m[1], victim: m[4], mine: actorMine || victimMine, actorMine, victimMine };
	}
	m = CRASH_RE.exec(text);
	if (m) {
		const victimMine = nameMatches(m[1], player);
		return { kind: "crash", actor: "", victim: m[1], mine: victimMine, actorMine: false, victimMine };
	}
	const pn = normName(player);
	const mine = pn ? text.toLowerCase().includes(pn) : false;
	if (/\bassist/i.test(text)) return { kind: "assist", actor: "", victim: "", mine, actorMine: mine, victimMine: false };
	return { kind: "other", actor: "", victim: "", mine, actorMine: false, victimMine: false };
}

function emptyCounters() {
	return { kills: 0, assists: 0, deaths: 0 };
}

function applyCounters(counters, entries, player) {
	const c = { kills: counters.kills, assists: counters.assists, deaths: counters.deaths };
	if (!normName(player)) return c;
	for (const e of entries) {
		const p = parseKillLine(e.msg, player);
		if (p.kind === "kill") {
			if (p.actorMine) c.kills++;
			else if (p.victimMine) c.deaths++;
		} else if (p.kind === "crash" && p.victimMine) c.deaths++;
		else if (p.kind === "assist" && p.mine) c.assists++;
	}
	return c;
}

// Army detection. /indicators carries "army": "air" | "tank" (community-observed); fall back to
// field presence. Returns "none" (not in a vehicle), "air", "tank" or null (unknown).
function detectArmy(indicators, state) {
	if (!indicators && !state) return null;
	if (indicators && indicators.valid === false) return "none";
	if (indicators && typeof indicators.army === "string") {
		const a = indicators.army.toLowerCase();
		if (a === "air" || a === "tank") return a;
	}
	if (state && state.valid === true && isNum(state["IAS, km/h"])) return "air";
	if (indicators && indicators.valid === true) {
		if ("gears" in indicators || "airbrake_lever" in indicators || "altitude_hour" in indicators) return "air";
		if ("crew_total" in indicators || "gear" in indicators || "cruise_control" in indicators) return "tank";
	}
	if (state && state.valid === false) return "none";
	return null;
}

// Largest value of a per-engine key family such as "water temp N, C".
function maxEngine(state, fn) {
	let best = null;
	for (let n = 1; n <= 8; n++) {
		const v = state[fn(n)];
		if (isNum(v) && (best === null || v > best)) best = v;
	}
	return best;
}
function firstEngine(state, fn) {
	for (let n = 1; n <= 8; n++) {
		const v = state[fn(n)];
		if (isNum(v)) return v;
	}
	return null;
}
function sumEngine(state, fn) {
	let s = null;
	for (let n = 1; n <= 8; n++) {
		const v = state[fn(n)];
		if (isNum(v)) s = (s === null ? 0 : s) + v;
	}
	return s;
}

function gearText(g) {
	if (!isNum(g)) return "—";
	if (g < 0.05) return "UP";
	if (g > 0.95) return "DOWN";
	return "MOVING";
}

// Raw aircraft readings (SI/metric) from /state and /indicators.
function readAir(state, ind) {
	const s = state || {};
	const i = ind || {};
	return {
		ias: s["IAS, km/h"],
		tas: s["TAS, km/h"],
		mach: isNum(s["M"]) ? s["M"] : i.mach,
		alt: s["H, m"],
		vy: s["Vy, m/s"],
		ny: isNum(s["Ny"]) ? s["Ny"] : i.g_meter,
		aoa: s["AoA, deg"],
		throttle: firstEngine(s, (n) => "throttle " + n + ", %") ?? (isNum(i.throttle) ? i.throttle * 100 : null),
		rpm: firstEngine(s, (n) => "RPM " + n) ?? (isNum(i.rpm_min) ? i.rpm_min : null),
		power: sumEngine(s, (n) => "power " + n + ", hp"),
		manifold: firstEngine(s, (n) => "manifold pressure " + n + ", atm"),
		water: maxEngine(s, (n) => "water temp " + n + ", C"),
		oil: maxEngine(s, (n) => "oil temp " + n + ", C"),
		fuel: s["Mfuel, kg"],
		fuel0: s["Mfuel0, kg"],
		flaps: s["flaps, %"],
		gears: i.gears,
		airbrake: i.airbrake_indicator,
	};
}

// Tank fields: NOT in the lucasvmx docs. Names below are community-observed (/indicators on a
// tank) and every read is guarded; missing ones render as an em dash.
const TANK_FLAG_KEYS = ["lws", "ircm_state", "roll_indicators_state"];

function readTank(ind) {
	const i = ind || {};
	const flags = [];
	for (const k of TANK_FLAG_KEYS) {
		const v = i[k];
		if (v === true || (isNum(v) && v > 0)) flags.push(k);
	}
	return {
		speed: i.speed,
		gear: i.gear,
		rpm: isNum(i.rpm) ? i.rpm : i.rpm_min,
		cruise: i.cruise_control,
		stabilizer: i.stabilizer,
		crewCurrent: i.crew_current,
		crewTotal: i.crew_total,
		flags,
	};
}

function gearLabel(g) {
	if (!isNum(g)) return "—";
	if (g === 0) return "N";
	if (g === -1) return "R";
	return String(g);
}

function onOff(v) {
	if (v === true || (isNum(v) && v > 0)) return "ON";
	if (v === false || v === 0) return "OFF";
	return "—";
}

// Colour band for a temperature: ok < 85% of limit, warn up to the limit, bad at/over it.
function tempBand(c, limit) {
	if (!isNum(c) || !isNum(limit)) return "";
	if (c >= limit) return "bad";
	if (c >= limit * 0.85) return "warn";
	return "ok";
}

// Stall margin in 0..1 (1 = lots of margin, 0 = at the critical angle).
function stallMargin(aoa, critical) {
	if (!isNum(aoa) || !isNum(critical) || critical <= 0) return null;
	return Math.max(0, Math.min(1, 1 - Math.abs(aoa) / critical));
}

function formatDuration(secs) {
	if (!isNum(secs)) return "—";
	const s = Math.max(0, Math.round(secs));
	return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
}

// Warning strings for the compact (S) layout and the warning strip.
function computeWarnings(air, est, cfg) {
	const w = [];
	if (isNum(air.aoa) && isNum(cfg.criticalAoA) && Math.abs(air.aoa) >= cfg.criticalAoA * 0.9) w.push("STALL");
	if (isNum(air.water) && air.water >= cfg.waterTempLimit) w.push("WATER OVERHEAT");
	else if (est.waterSecs !== null && est.waterSecs !== undefined && est.waterSecs < 30) w.push("WATER " + formatDuration(est.waterSecs));
	if (isNum(air.oil) && air.oil >= cfg.oilTempLimit) w.push("OIL OVERHEAT");
	else if (est.oilSecs !== null && est.oilSecs !== undefined && est.oilSecs < 30) w.push("OIL " + formatDuration(est.oilSecs));
	if (est.fuelMinutes !== null && est.fuelMinutes !== undefined && est.fuelMinutes < cfg.fuelWarnMinutes) w.push("LOW FUEL");
	else if (isNum(air.fuel) && air.fuel === 0 && isNum(air.fuel0) && air.fuel0 > 0) w.push("NO FUEL");
	return w;
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------
function readSettings() {
	const num = (v, d, lo, hi) => (isNum(v) ? Math.max(lo, Math.min(hi, v)) : d);
	return {
		playerName: typeof playerName === "string" ? playerName : "",
		units: typeof units === "string" && units === "imperial" ? "imperial" : "metric",
		pollMs: num(typeof pollMs === "undefined" ? undefined : pollMs, 200, 100, 1000),
		fuelWarnMinutes: num(typeof fuelWarnMinutes === "undefined" ? undefined : fuelWarnMinutes, 3, 1, 15),
		waterTempLimit: num(typeof waterTempLimit === "undefined" ? undefined : waterTempLimit, 110, 50, 200),
		oilTempLimit: num(typeof oilTempLimit === "undefined" ? undefined : oilTempLimit, 120, 50, 200),
		criticalAoA: num(typeof criticalAoA === "undefined" ? undefined : criticalAoA, 15, 5, 45),
		showChat: typeof showChat === "undefined" ? true : showChat !== false && showChat !== "false",
		textColor: typeof textColor === "string" ? textColor : "#E8ECF2",
		accentColor: typeof accentColor === "string" ? accentColor : "#F2A900",
		backgroundColor: typeof backgroundColor === "string" ? backgroundColor : "#0E1117",
		warningColor: typeof warningColor === "string" ? warningColor : "#FF4D4D",
	};
}

// ---------------------------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------------------------
const rt = {
	started: false,
	cfg: null,
	online: false,
	army: null,
	inflight: null,
	slowInflight: null,
	fastTimer: null,
	slowTimer: null,
	fuelSamples: [],
	waterSamples: [],
	oilSamples: [],
	lastEvt: 0,
	lastDmg: 0,
	lastChat: 0,
	feed: [],
	chat: [],
	seq: 1,
	firstHud: true,
	firstChat: true,
	counters: emptyCounters(),
	sessionStart: null,
	missionTick: 0,
	mission: null,
	feedDirty: true,
	chatDirty: true,
};

function $(id) {
	return document.getElementById(id);
}
function setText(id, text) {
	const el = $(id);
	if (el.textContent !== text) el.textContent = text;
}
function setBand(id, base, band) {
	$(id).className = base + (band ? " " + band : "");
}

function applyTheme() {
	const c = rt.cfg;
	const st = document.documentElement.style;
	st.setProperty("--fg", c.textColor);
	st.setProperty("--accent", c.accentColor);
	st.setProperty("--bg", c.backgroundColor);
	st.setProperty("--warn", c.warningColor);
}

// ---------------------------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------------------------
function pollFast() {
	if (rt.inflight) return rt.inflight; // never pile up requests
	rt.inflight = (async () => {
		let state = null;
		let ind = null;
		try {
			[state, ind] = await Promise.all([httpGet("/state").catch(() => null), httpGet("/indicators").catch(() => null)]);
		} catch (_e) {
			state = ind = null;
		}
		if (!state && !ind) {
			rt.online = false;
			rt.army = null;
		} else {
			rt.online = true;
			rt.army = detectArmy(ind, state);
			ingest(state, ind);
		}
		renderAll(state, ind);
	})()
		.catch((e) => console.warn("poll failed", e))
		.finally(() => {
			rt.inflight = null;
		});
	return rt.inflight;
}

function pollSlow() {
	if (rt.slowInflight) return rt.slowInflight;
	rt.slowInflight = (async () => {
		if (!rt.online) return;
		const tasks = [];
		tasks.push(
			httpGet("/hudmsg", "?lastEvt=" + rt.lastEvt + "&lastDmg=" + rt.lastDmg).then(ingestHud, () => {}),
		);
		if (rt.cfg.showChat) tasks.push(httpGet("/gamechat", "?lastId=" + rt.lastChat).then(ingestChat, () => {}));
		if (rt.missionTick++ % 5 === 0) tasks.push(httpGet("/mission.json").then((m) => (rt.mission = m), () => {}));
		await Promise.all(tasks);
		renderFeeds();
	})()
		.catch((e) => console.warn("slow poll failed", e))
		.finally(() => {
			rt.slowInflight = null;
		});
	return rt.slowInflight;
}

function scheduleFast() {
	clearTimeout(rt.fastTimer);
	rt.fastTimer = setTimeout(async () => {
		await pollFast();
		scheduleFast();
	}, rt.online ? rt.cfg.pollMs : 2000);
}
function scheduleSlow() {
	clearTimeout(rt.slowTimer);
	rt.slowTimer = setTimeout(async () => {
		await pollSlow();
		scheduleSlow();
	}, rt.online ? 1000 : 2000);
}

function ingest(state, ind) {
	if (rt.army === "air" && state) {
		const t = Date.now();
		const air = readAir(state, ind);
		if (isNum(air.fuel)) pushSample(rt.fuelSamples, { t, kg: air.fuel }, 90000);
		if (isNum(air.water)) pushSample(rt.waterSamples, { t, v: air.water }, 15000);
		if (isNum(air.oil)) pushSample(rt.oilSamples, { t, v: air.oil }, 15000);
	} else {
		rt.fuelSamples.length = 0;
		rt.waterSamples.length = 0;
		rt.oilSamples.length = 0;
	}
	if (rt.army === "air" || rt.army === "tank") {
		if (rt.sessionStart === null) rt.sessionStart = Date.now();
	}
}

function ingestHud(data) {
	if (!data) return;
	const evt = mergeHud(rt.feed, data.events || [], "evt", 30, rt.seq);
	rt.seq += evt.added.length;
	const dmg = mergeHud(evt.list, data.damage || [], "dmg", 30, rt.seq);
	rt.seq += dmg.added.length;
	rt.feed = dmg.list.sort((a, b) => b.seq - a.seq).slice(0, 30);
	rt.lastEvt = maxId(evt.added, "evt", rt.lastEvt);
	rt.lastDmg = maxId(dmg.added, "dmg", rt.lastDmg);
	// The first response replays history (possibly earlier matches): show it, do not count it.
	if (!rt.firstHud) rt.counters = applyCounters(rt.counters, evt.added.concat(dmg.added), rt.cfg.playerName);
	rt.firstHud = false;
	rt.feedDirty = true;
}

function ingestChat(data) {
	if (!Array.isArray(data)) return;
	const r = mergeHud(rt.chat, data, "chat", 30, rt.seq);
	rt.seq += r.added.length;
	rt.chat = r.list;
	rt.lastChat = maxId(r.added, "chat", rt.lastChat);
	rt.chatDirty = true;
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------
function renderAll(state, ind) {
	const root = $("root");
	let mode = "wait";
	if (rt.online) mode = rt.army === "air" ? "air" : rt.army === "tank" ? "tank" : "hangar";
	root.className = "state-" + mode;
	if (mode === "wait") {
		setText("banner-title", "Waiting for War Thunder on localhost:8111");
		setText("banner-hint", "Make sure the game is running and its telemetry server is enabled (localhost:8111 should open in a browser).");
	} else if (mode === "hangar") {
		setText("banner-title", "In hangar / not in vehicle");
		setText("banner-hint", "Telemetry connected. Flight and vehicle data appear once you spawn.");
	}
	$("air").hidden = mode !== "air";
	$("ground").hidden = mode !== "tank";
	$("chatbox").hidden = !rt.cfg.showChat;
	if (mode === "air") renderAir(state, ind);
	else if (mode === "tank") renderTank(ind);
	else setText("warnings", "");
	renderFeeds();
}

function put(id, quantity, digits) {
	setText("v-" + id, fmt(quantity.v, digits));
	setText("u-" + id, quantity.u);
}

function renderAir(state, ind) {
	const cfg = rt.cfg;
	const air = readAir(state, ind);
	const u = cfg.units;
	put("ias", airspeed(air.ias, u), 0);
	put("tas", airspeed(air.tas, u), 0);
	put("alt", altitude(air.alt, u), 0);
	put("vs", verticalSpeed(air.vy, u), u === "imperial" ? 0 : 1);
	setText("v-g", fmt(air.ny, 1));
	setText("v-mach", fmt(air.mach, 2));
	setText("v-aoa", fmt(air.aoa, 1));
	const margin = stallMargin(air.aoa, cfg.criticalAoA);
	$("b-aoa").style.setProperty("--p", margin === null ? "0" : String(Math.round(margin * 100)));
	setBand("c-aoa", "card core", margin === null ? "" : margin < 0.1 ? "bad" : margin < 0.35 ? "warn" : "ok");
	setText("v-thr", fmt(air.throttle, 0));
	$("b-thr").style.setProperty("--p", isNum(air.throttle) ? String(Math.max(0, Math.min(100, Math.round(air.throttle)))) : "0");
	setText("v-rpm", fmt(air.rpm, 0));
	setText("v-pow", fmt(air.power, 0));
	setText("s-man", isNum(air.manifold) ? fmt(air.manifold, 2) + " atm" : "");

	const wq = temperature(air.water, u);
	const oq = temperature(air.oil, u);
	put("water", wq, 0);
	put("oil", oq, 0);
	const waterSecs = estimateTimeToLimit(rt.waterSamples, cfg.waterTempLimit);
	const oilSecs = estimateTimeToLimit(rt.oilSamples, cfg.oilTempLimit);
	setText("s-water", subTemp(waterSecs, air.water, cfg.waterTempLimit, u));
	setText("s-oil", subTemp(oilSecs, air.oil, cfg.oilTempLimit, u));
	setBand("c-water", "card core", tempBand(air.water, cfg.waterTempLimit));
	setBand("c-oil", "card core", tempBand(air.oil, cfg.oilTempLimit));

	const f = estimateFuel(rt.fuelSamples);
	put("fuel", fuelMass(air.fuel, u), 0);
	setText("s-fuel", f.minutes === null ? (f.burnKgPerMin === 0 ? "steady" : "") : formatDuration(f.minutes * 60) + " left");
	setBand("c-fuel", "card core", f.minutes !== null && f.minutes < cfg.fuelWarnMinutes ? "bad" : f.minutes !== null && f.minutes < cfg.fuelWarnMinutes * 2 ? "warn" : "");

	const cfgParts = ["Gear " + gearText(air.gears), "Flaps " + (isNum(air.flaps) ? Math.round(air.flaps) + "%" : "—"), "Brake " + (isNum(air.airbrake) ? (air.airbrake > 0.05 ? "OUT" : "IN") : "—")];
	setText("v-cfg", cfgParts.join("  ·  "));

	const warnings = computeWarnings(air, { waterSecs, oilSecs, fuelMinutes: f.minutes }, cfg);
	setText("warnings", warnings.join("   "));
	$("warnings").className = warnings.length ? "on" : "";
}

function subTemp(secs, now, limit, units) {
	if (!isNum(now)) return "";
	const lim = temperature(limit, units);
	if (secs === 0) return "over limit " + fmt(lim.v, 0);
	if (secs === null) return "limit " + fmt(lim.v, 0);
	return formatDuration(secs) + " to " + fmt(lim.v, 0);
}

function renderTank(ind) {
	const t = readTank(ind);
	put("gspeed", groundSpeed(isNum(t.speed) ? Math.abs(t.speed) : null, rt.cfg.units), 0);
	setText("v-ggear", gearLabel(t.gear));
	setText("v-grpm", fmt(t.rpm, 0));
	setText("v-gcruise", onOff(t.cruise));
	setText("v-gstab", onOff(t.stabilizer));
	setText("v-gcrew", isNum(t.crewCurrent) && isNum(t.crewTotal) ? t.crewCurrent + " / " + t.crewTotal : "—");
	setText("v-gflags", t.flags.length ? t.flags.join(", ") : "none");
	const low = isNum(t.crewCurrent) && isNum(t.crewTotal) && t.crewCurrent < t.crewTotal;
	setBand("c-gcrew", "card core", low ? (t.crewCurrent <= 1 ? "bad" : "warn") : "");
	setText("warnings", low ? "CREW LOSS" : "");
	$("warnings").className = low ? "on" : "";
}

function lineEl(entry, mine, text) {
	const row = document.createElement("div");
	row.className = "line" + (mine ? " mine" : "");
	row.textContent = text;
	return row;
}

function renderFeeds() {
	const pn = rt.cfg.playerName;
	if (rt.feedDirty) {
		rt.feedDirty = false;
		$("feed-list").replaceChildren(
			...rt.feed.map((e) => {
				const p = parseKillLine(e.msg, pn);
				return lineEl(e, p.mine, e.msg);
			}),
		);
	}
	if (rt.chatDirty) {
		rt.chatDirty = false;
		$("chat-list").replaceChildren(
			...rt.chat.map((e) => lineEl(e, !!normName(pn) && nameMatches(e.sender, pn), (e.mode ? "[" + e.mode + "] " : "") + e.sender + ": " + e.msg)),
		);
	}
	setText("cnt-kills", String(rt.counters.kills));
	setText("cnt-assists", String(rt.counters.assists));
	setText("cnt-deaths", String(rt.counters.deaths));
	const m = rt.mission;
	let text = "";
	if (rt.sessionStart !== null) text = formatDuration((Date.now() - rt.sessionStart) / 1000);
	if (m && typeof m.status === "string") text += (text ? "  ·  " : "") + m.status;
	if (m && Array.isArray(m.objectives)) {
		const prim = m.objectives.find((o) => o && o.primary);
		if (prim && prim.status) text += "  ·  " + prim.status.replace("_", " ");
	}
	setText("mission", text);
}

// ---------------------------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------------------------
function init() {
	if (rt.started) return;
	rt.started = true;
	rt.cfg = readSettings();
	applyTheme();
	renderAll(null, null);
	pollFast();
	pollSlow();
	scheduleFast();
	scheduleSlow();
}

function onSettingsChanged() {
	rt.cfg = readSettings();
	if (rt.started) applyTheme();
}

// Late-load guard: the script may run after boot finished.
if (typeof iCUE_initialized !== "undefined" && iCUE_initialized) init();
