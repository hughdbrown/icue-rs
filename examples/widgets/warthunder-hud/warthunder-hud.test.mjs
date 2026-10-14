// Tests for the War Thunder HUD widget: pure functions plus a fake-DOM init test.
// Run with: node --test examples/widgets/warthunder-hud/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("./scripts/warthunder-hud.js", import.meta.url), "utf8");

// Fixtures in the documented formats (State.md, Indicators.md, Hudmsg.md, GameChat.md, Mission.md).
const STATE_AIR = {
	valid: true,
	"H, m": 4936,
	"TAS, km/h": 237,
	"IAS, km/h": 185,
	M: 0.2,
	"AoA, deg": 12.5,
	Ny: 1.0,
	"Vy, m/s": 3.2,
	"Mfuel, kg": 750,
	"Mfuel0, kg": 2620,
	"throttle 1, %": 100,
	"power 1, hp": 484.0,
	"RPM 1": 1957,
	"manifold pressure 1, atm": 0.81,
	"water temp 1, C": 95,
	"oil temp 1, C": 73,
	"flaps, %": 0,
};
const IND_AIR = { valid: true, army: "air", type: "spitfire", gears: 0, airbrake_indicator: 0, mach: 0.2, g_meter: 1 };
const IND_TANK = { valid: true, army: "tank", speed: 24, gear: 3, rpm: 1800, cruise_control: 0, stabilizer: 1, crew_current: 3, crew_total: 4 };
const HUD = {
	events: [],
	damage: [
		{ id: 161, msg: "percusiones1r (Spitfire) set afire *shino_rs (Spitfire)", sender: "", enemy: false, mode: "" },
		{ id: 162, msg: "*shino_rs (Spitfire) has crashed.", sender: "", enemy: false, mode: "" },
	],
};

function load(extra = {}) {
	const byId = new Map();
	function el(id) {
		const attrs = {};
		const children = [];
		const e = {
			id,
			className: "",
			textContent: "",
			hidden: false,
			children,
			style: { _p: {}, setProperty(k, v) { this._p[k] = String(v); } },
			replaceChildren(...n) { children.length = 0; children.push(...n); },
			append(...n) { children.push(...n); },
			setAttribute(k, v) { attrs[k] = v; },
			getAttribute(k) { return attrs[k] ?? null; },
			addEventListener() {},
		};
		return e;
	}
	const document = {
		getElementById(id) {
			if (!byId.has(id)) byId.set(id, el(id));
			return byId.get(id);
		},
		createElement() {
			return el("");
		},
		documentElement: el("html"),
		addEventListener() {},
	};
	const timeouts = [];
	const fetchCalls = [];
	const ctx = {
		console,
		document,
		setTimeout: (fn, ms) => { const t = setTimeout(fn, ms); timeouts.push(t); return t; },
		clearTimeout: (t) => clearTimeout(t),
		fetch: async () => { throw new Error("no fetch"); },
		playerName: "Hugh",
		units: "metric",
		pollMs: 200,
		fuelWarnMinutes: 3,
		waterTempLimit: 110,
		oilTempLimit: 120,
		criticalAoA: 15,
		showChat: true,
		iCUE_initialized: false,
		...extra,
	};
	const origFetch = ctx.fetch;
	ctx.fetch = async (url) => { fetchCalls.push(String(url)); return origFetch(url); };
	ctx.window = ctx;
	vm.createContext(ctx);
	vm.runInContext(SOURCE, ctx);
	return { ctx, byId, fetchCalls, stop: () => timeouts.forEach(clearTimeout) };
}

const { ctx: F } = load();
const near = (a, b, eps = 0.01) => assert.ok(Math.abs(a - b) <= eps, `${a} !~ ${b}`);

test("unit conversions", () => {
	near(F.kmhToKnots(185.2), 100);
	near(F.kmhToMph(160.9344), 100);
	near(F.metersToFeet(1000), 3280.84);
	near(F.msToFtMin(1), 196.85, 0.01);
	near(F.kgToLb(100), 220.462);
	assert.equal(F.airspeed(185.2, "imperial").u, "kn");
	assert.equal(F.altitude(100, "imperial").u, "ft");
	assert.equal(F.verticalSpeed(1, "imperial").u, "ft/min");
	assert.equal(F.airspeed(100, "metric").v, 100);
	assert.equal(F.airspeed(undefined, "metric").v, null);
});

test("fuel minutes estimator", () => {
	const samples = [];
	for (let i = 0; i <= 12; i++) samples.push({ t: i * 5000, kg: 600 - i * 5 }); // 1 kg/s = 60 kg/min
	const r = F.estimateFuel(samples);
	near(r.burnKgPerMin, 60, 0.5);
	near(r.minutes, 540 / 60, 0.05);
	assert.equal(F.estimateFuel([{ t: 0, kg: 10 }]).minutes, null);
	const steady = [{ t: 0, kg: 500 }, { t: 6000, kg: 500 }, { t: 12000, kg: 500 }];
	assert.equal(F.estimateFuel(steady).minutes, null);
	assert.equal(F.estimateFuel(steady).burnKgPerMin, 0);
});

test("temperature time-to-limit estimator", () => {
	const rising = [];
	for (let i = 0; i <= 10; i++) rising.push({ t: i * 1000, v: 90 + i }); // +1 C/s, now 100
	near(F.estimateTimeToLimit(rising, 110), 10, 0.1);
	assert.equal(F.estimateTimeToLimit(rising, 100), 0);
	const flat = [0, 1, 2, 3, 4].map((i) => ({ t: i * 1000, v: 80 }));
	assert.equal(F.estimateTimeToLimit(flat, 110), null);
	const falling = [0, 1, 2, 3, 4].map((i) => ({ t: i * 1000, v: 90 - i }));
	assert.equal(F.estimateTimeToLimit(falling, 110), null);
	assert.equal(F.estimateTimeToLimit([{ t: 0, v: 50 }], 110), null);
	// old samples outside the 10 s window do not skew the slope
	const old = [{ t: 0, v: 20 }, ...rising.map((s) => ({ t: s.t + 60000, v: s.v }))];
	near(F.estimateTimeToLimit(old, 110), 10, 0.1);
});

test("temperature bands and stall margin", () => {
	assert.equal(F.tempBand(80, 110), "ok");
	assert.equal(F.tempBand(100, 110), "warn");
	assert.equal(F.tempBand(110, 110), "bad");
	near(F.stallMargin(7.5, 15), 0.5);
	assert.equal(F.stallMargin(30, 15), 0);
	assert.equal(F.stallMargin(null, 15), null);
});

test("hudmsg incremental merge dedupes by id and keeps newest first", () => {
	const a = F.mergeHud([], HUD.damage, "dmg", 30, 1);
	assert.equal(a.list.length, 2);
	assert.equal(a.list[0].id, 162);
	const again = F.mergeHud(a.list, HUD.damage, "dmg", 30, 10);
	assert.equal(again.added.length, 0);
	assert.equal(again.list.length, 2);
	const more = F.mergeHud(a.list, [{ id: 163, msg: "x" }, { id: 162, msg: "dup" }], "dmg", 30, 10);
	assert.equal(more.added.map((e) => e.id).join(), "163");
	assert.equal(more.list[0].id, 163);
	assert.equal(F.maxId(more.list, "dmg", 0), 163);
	// same id under another kind is distinct; cap is honoured
	assert.equal(F.mergeHud(a.list, [{ id: 162, msg: "e" }], "evt", 30, 10).added.length, 1);
	const many = F.mergeHud([], Array.from({ length: 50 }, (_, i) => ({ id: i + 1, msg: "m" })), "dmg", 30, 1);
	assert.equal(many.list.length, 30);
	assert.equal(many.list[0].id, 50);
});

test("kill-feed parsing heuristics and counters", () => {
	const p = F.parseKillLine("Hugh (Spitfire) shot down Bf109 (Bf 109 F-4)", "Hugh");
	assert.equal(p.kind, "kill");
	assert.ok(p.actorMine && !p.victimMine);
	const d = F.parseKillLine("[TAG] Hugh (Bf 109) has crashed.", "hugh");
	assert.equal(d.kind, "crash");
	assert.ok(d.victimMine);
	assert.equal(F.parseKillLine("percusiones1r (Spitfire) set afire *shino_rs (Spitfire)", "Hugh").kind, "damage");
	assert.equal(F.parseKillLine("percusiones1r (Spitfire) set afire *shino_rs (Spitfire)", "Hugh").mine, false);
	assert.equal(F.parseKillLine("Enemy (Tiger) destroyed *Hugh (T-34)", "Hugh").victimMine, true);
	assert.equal(F.parseKillLine("something odd", "Hugh").kind, "other");
	assert.equal(F.parseKillLine("anything", "").mine, false);
	assert.equal(F.parseKillLine("Hughes (A) shot down Z (B)", "Hugh").actorMine, false);

	const entries = [
		{ msg: "Hugh (Spitfire) shot down A (Bf 109)" },
		{ msg: "Hugh (Spitfire) destroyed B (Tiger)" },
		{ msg: "C (Bf 109) shot down Hugh (Spitfire)" },
		{ msg: "Hugh (Spitfire) has crashed." },
		{ msg: "Hugh assist on D" },
		{ msg: "Hugh (Spitfire) set afire E (Bf 109)" },
		{ msg: "X (A) shot down Y (B)" },
	];
	assert.deepEqual({ ...F.applyCounters(F.emptyCounters(), entries, "Hugh") }, { kills: 2, assists: 1, deaths: 2 });
	assert.deepEqual({ ...F.applyCounters(F.emptyCounters(), entries, "") }, { kills: 0, assists: 0, deaths: 0 });
});

test("army detection", () => {
	assert.equal(F.detectArmy(IND_AIR, STATE_AIR), "air");
	assert.equal(F.detectArmy(IND_TANK, { valid: false }), "tank");
	assert.equal(F.detectArmy({ valid: false }, { valid: false }), "none");
	assert.equal(F.detectArmy({ valid: true, gears: 0 }, null), "air");
	assert.equal(F.detectArmy({ valid: true, crew_total: 4 }, null), "tank");
	assert.equal(F.detectArmy(null, null), null);
	assert.equal(F.detectArmy(null, STATE_AIR), "air");
});

test("endpoint allow-list excludes map and position endpoints", () => {
	for (const ok of ["/state", "/indicators", "/hudmsg", "/gamechat", "/mission.json"]) assert.ok(F.isAllowedPath(ok), ok);
	for (const bad of ["/map_obj.json", "/map.img", "/map_info.json", "/", "/state/x"]) assert.ok(!F.isAllowedPath(bad), bad);
	return assert.rejects(F.httpGet("/map_obj.json"), /not allowed/);
});

test("warnings", () => {
	const cfg = { criticalAoA: 15, waterTempLimit: 110, oilTempLimit: 120, fuelWarnMinutes: 3 };
	const air = { aoa: 14, water: 112, oil: 80, fuel: 10, fuel0: 100 };
	const w = F.computeWarnings(air, { waterSecs: 0, oilSecs: 20, fuelMinutes: 1 }, cfg);
	assert.ok(w.includes("STALL") && w.includes("WATER OVERHEAT") && w.includes("LOW FUEL"));
	assert.ok(w.some((x) => x.startsWith("OIL ")));
});

test("normalizeHost accepts host names, IPs and pasted URLs, else localhost", () => {
	const { normalizeHost } = load().ctx;
	const cases = [
		["", "localhost"],
		[undefined, "localhost"],
		["  GamePC.local ", "gamepc.local"],
		["192.168.1.20", "192.168.1.20"],
		["192.168.1.20:8111", "192.168.1.20"],
		["http://192.168.1.20:8111/state", "192.168.1.20"],
		["::1", "[::1]"],
		["[fe80::1]:8111", "[fe80::1]"],
		["bad host!", "localhost"],
		["-x.local", "localhost"],
	];
	for (const [raw, want] of cases) assert.equal(normalizeHost(raw), want, String(raw));
});

test("a remote War Thunder PC is polled and named in the waiting banner", async () => {
	const env = load({ wtHost: "http://192.168.1.20:8111/" });
	try {
		env.ctx.icueEvents.onICUEInitialized();
		await env.ctx.pollFast();
		assert.equal(env.byId.get("banner-title").textContent, "Waiting for War Thunder on 192.168.1.20:8111");
		assert.match(env.byId.get("banner-hint").textContent, /firewall allows incoming TCP 8111/);
		assert.ok(env.fetchCalls.length > 0);
		assert.ok(env.fetchCalls.every((u) => u.startsWith("http://192.168.1.20:8111/")));
	} finally {
		env.stop();
	}
});

test("init renders the waiting state when fetch rejects", async () => {
	const env = load();
	try {
		env.ctx.icueEvents.onICUEInitialized();
		await env.ctx.pollFast();
		assert.equal(env.byId.get("banner-title").textContent, "Waiting for War Thunder on localhost:8111");
		assert.match(env.byId.get("banner-hint").textContent, /running/);
		assert.equal(env.byId.get("root").className, "state-wait");
		assert.ok(env.fetchCalls.every((u) => u.startsWith("http://localhost:8111/")));
	} finally {
		env.stop();
	}
});

test("init renders aircraft numbers when fetch returns fixtures", async () => {
	const env = load({
		units: "imperial",
		fetch: async (url) => {
			const u = String(url);
			const body = u.includes("/state") ? STATE_AIR : u.includes("/indicators") ? IND_AIR : u.includes("/hudmsg") ? HUD : u.includes("/gamechat") ? [{ id: 1, msg: "hi", sender: "Bob", enemy: false, mode: "All" }] : { objectives: [], status: "running" };
			return { ok: true, status: 200, json: async () => body };
		},
	});
	try {
		env.ctx.icueEvents.onICUEInitialized();
		await env.ctx.pollFast();
		await env.ctx.pollSlow();
		const t = (id) => env.byId.get(id).textContent;
		assert.equal(env.byId.get("root").className, "state-air");
		assert.equal(t("v-ias"), "100"); // 185 km/h in knots
		assert.equal(t("u-ias"), "kn");
		assert.equal(t("v-alt"), "16194"); // 4936 m in ft
		assert.equal(t("u-alt"), "ft");
		assert.equal(t("v-g"), "1.0");
		assert.equal(t("v-aoa"), "12.5");
		assert.equal(t("v-water"), "203"); // 95 C in F
		assert.equal(t("v-fuel"), "1653"); // 750 kg in lb
		assert.equal(t("v-rpm"), "1957");
		assert.match(t("v-cfg"), /Gear UP/);
		assert.equal(env.byId.get("feed-list").children.length, 2);
		assert.equal(env.byId.get("chat-list").children.length, 1);
		assert.ok(env.fetchCalls.some((u) => u.includes("/hudmsg?lastEvt=0&lastDmg=0")));
	} finally {
		env.stop();
	}
});

test("hangar state when valid is false", async () => {
	const env = load({
		fetch: async (url) => ({ ok: true, status: 200, json: async () => (String(url).includes("/state") ? { valid: false } : { valid: false }) }),
	});
	try {
		env.ctx.icueEvents.onICUEInitialized();
		await env.ctx.pollFast();
		assert.equal(env.byId.get("banner-title").textContent, "In hangar / not in vehicle");
		assert.equal(env.byId.get("root").className, "state-hangar");
	} finally {
		env.stop();
	}
});

test("ground panel renders tank fields", async () => {
	const env = load({
		fetch: async (url) => ({ ok: true, status: 200, json: async () => (String(url).includes("/state") ? { valid: false } : IND_TANK) }),
	});
	try {
		env.ctx.icueEvents.onICUEInitialized();
		await env.ctx.pollFast();
		assert.equal(env.byId.get("root").className, "state-tank");
		assert.equal(env.byId.get("v-gspeed").textContent, "24");
		assert.equal(env.byId.get("v-gcrew").textContent, "3 / 4");
		assert.equal(env.byId.get("v-gstab").textContent, "ON");
		assert.equal(env.byId.get("v-gcruise").textContent, "OFF");
	} finally {
		env.stop();
	}
});
