// Piano widget (iCUE Widget API 1.x). Web Audio synthesis only: no samples, no network.
//
// The "octaves", "startOctave" and "sound" settings seed the on-widget state, which is kept in
// localStorage under uniqueId. A seed is re-applied only when its setting changes.

icueEvents = {
	onICUEInitialized: init,
	onDataUpdated: onSettingsChanged,
};

// ---------- pure helpers (also exercised by piano.test.mjs) ----------

const SOUNDS = ["piano", "electric", "organ", "synth", "marimba"];
const SOUND_LABELS = { piano: "Piano", electric: "Electric", organ: "Organ", synth: "Synth", marimba: "Marimba" };
const NOTE_NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const BLACK_PITCH_CLASSES = [1, 3, 6, 8, 10];
const BLACK_WIDTH_RATIO = 0.62;
const MIN_START_OCTAVE = 1;
const MAX_START_OCTAVE = 6;
// Computer keyboard: index = semitone above the first C of the keyboard.
const LOWER_ROW = "zsxdcvgbhnjm,";
const UPPER_ROW = "q2w3er5t6y7ui";

// Equal temperament, A4 (MIDI 69) = 440 Hz.
function midiToFrequency(midi) {
	return 440 * Math.pow(2, (midi - 69) / 12);
}

// MIDI 60 -> "C4", 61 -> "C♯4".
function noteName(midi) {
	return NOTE_NAMES[((midi % 12) + 12) % 12] + (Math.floor(midi / 12) - 1);
}

function isBlackKey(midi) {
	return BLACK_PITCH_CLASSES.includes(((midi % 12) + 12) % 12);
}

function octaveStartMidi(octave) {
	return 12 * (octave + 1);
}

// Keys for `octaves` octaves from C<startOctave>, plus the final top C. Positions are fractions of
// the keyboard width: x is the left edge, w the width.
function keyboardLayout(octaves, startOctave) {
	const first = octaveStartMidi(startOctave);
	const last = first + 12 * octaves;
	let whiteCount = 0;
	for (let m = first; m <= last; m++) if (!isBlackKey(m)) whiteCount++;
	const whiteWidth = 1 / whiteCount;
	const keys = [];
	let whiteIndex = 0;
	for (let m = first; m <= last; m++) {
		if (isBlackKey(m)) {
			const width = whiteWidth * BLACK_WIDTH_RATIO;
			keys.push({ midi: m, black: true, x: whiteIndex * whiteWidth - width / 2, w: width });
		} else {
			keys.push({ midi: m, black: false, x: whiteIndex * whiteWidth, w: whiteWidth });
			whiteIndex++;
		}
	}
	return { whiteCount, keys };
}

// Computer-keyboard character -> MIDI note (null if unmapped).
function computerKeyToMidi(key, startOctave) {
	const k = String(key).toLowerCase();
	const base = octaveStartMidi(startOctave);
	const lower = LOWER_ROW.indexOf(k);
	if (lower >= 0) return base + lower;
	const upper = UPPER_ROW.indexOf(k);
	if (upper >= 0) return base + 12 + upper;
	return null;
}

function clampInt(value, min, max, fallback) {
	const n = Math.round(Number(value));
	return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
}

// ---------- settings and state ----------

const $ = (id) => document.getElementById(id);
const setting = (name, fallback) => (typeof window[name] === "undefined" || window[name] === null ? fallback : window[name]);

function readSettings() {
	const snd = String(setting("sound", "piano"));
	const lbl = String(setting("labels", "c"));
	const sus = setting("sustain", false);
	return {
		octaves: clampInt(setting("octaves", 3), 2, 3, 3),
		startOctave: clampInt(setting("startOctave", 3), MIN_START_OCTAVE, MAX_START_OCTAVE, 3),
		sound: SOUNDS.includes(snd) ? snd : "piano",
		volume: clampInt(setting("volume", 70), 0, 100, 70),
		labels: ["none", "c", "all"].includes(lbl) ? lbl : "c",
		sustain: sus === true || sus === "true",
		accentColor: String(setting("accentColor", "#FFB300")),
		backgroundColor: String(setting("backgroundColor", "#14161C")),
	};
}

function loadStored() {
	try {
		return JSON.parse(localStorage.getItem(uniqueId) || "null");
	} catch (_) {
		return null;
	}
}

function saveState() {
	try {
		localStorage.setItem(uniqueId, JSON.stringify(state));
	} catch (_) {
		// Storage can be unavailable; the widget still works, it just forgets.
	}
}

// On-widget state seeded per field from settings; a field follows its setting only when that
// setting changed since the widget last saw it.
function seededState(cfg, stored) {
	const seed = { octaves: cfg.octaves, startOctave: cfg.startOctave, sound: cfg.sound };
	const old = stored && stored.seed ? stored.seed : {};
	const pick = (key) => (stored && old[key] === seed[key] && stored[key] !== undefined ? stored[key] : seed[key]);
	return {
		seed,
		octaves: clampInt(pick("octaves"), 2, 3, seed.octaves),
		startOctave: clampInt(pick("startOctave"), MIN_START_OCTAVE, MAX_START_OCTAVE, seed.startOctave),
		sound: SOUNDS.includes(pick("sound")) ? pick("sound") : seed.sound,
	};
}

let cfg = null;
let state = null;
let initialized = false;
let layoutKey = "";
const keyEls = new Map(); // midi -> element
const held = new Map(); // midi -> { count, voice }
const pointers = new Map(); // pointerId -> midi | null
const computerKeys = new Map(); // lowercase key -> midi

// ---------- audio ----------

let audio = null; // { ctx, master }

function ensureAudio() {
	if (audio) {
		if (audio.ctx.state === "suspended") resumeContext(audio.ctx);
		return audio;
	}
	const Ctor = typeof AudioContext !== "undefined" ? AudioContext : typeof webkitAudioContext !== "undefined" ? webkitAudioContext : null;
	if (!Ctor) return null;
	try {
		const ctx = new Ctor();
		const master = ctx.createGain();
		const limiter = ctx.createDynamicsCompressor();
		limiter.threshold.value = -18;
		limiter.knee.value = 24;
		limiter.ratio.value = 8;
		limiter.attack.value = 0.003;
		limiter.release.value = 0.25;
		master.connect(limiter);
		limiter.connect(ctx.destination);
		audio = { ctx, master };
		applyVolume();
		resumeContext(ctx);
		return audio;
	} catch (_) {
		return null;
	}
}

function resumeContext(ctx) {
	try {
		const p = ctx.resume();
		if (p && p.catch) p.catch(() => {});
	} catch (_) {
		// ignored: a later gesture retries
	}
}

function applyVolume() {
	if (!audio || !cfg) return;
	const level = Math.pow(cfg.volume / 100, 1.5);
	audio.master.gain.setTargetAtTime(level, audio.ctx.currentTime, 0.02);
}

// Builds one voice. Every voice is osc -> env -> out(release stage) -> master, so release is a
// clean exponential fade on `out` that never needs to know the current envelope value.
function buildVoice(sound, ctx, dest, midi) {
	const t = ctx.currentTime;
	const f = midiToFrequency(midi);
	const out = ctx.createGain();
	out.gain.value = 0.22;
	out.connect(dest);
	const env = ctx.createGain();
	env.connect(out);
	const oscs = [];
	const addOsc = (type, freq, target, level) => {
		const o = ctx.createOscillator();
		o.type = type;
		o.frequency.value = freq;
		if (level === undefined) {
			o.connect(target);
		} else {
			const g = ctx.createGain();
			g.gain.value = level;
			o.connect(g);
			g.connect(target);
		}
		o.start(t);
		oscs.push(o);
		return o;
	};
	const pluck = (param, peak, decay) => {
		param.setValueAtTime(0.0001, t);
		param.exponentialRampToValueAtTime(peak, t + 0.006);
		param.exponentialRampToValueAtTime(0.0005, t + decay);
	};
	const cents = (c) => Math.pow(2, c / 1200);
	const voice = { oscs, out, percussive: true, decay: 1, releaseTau: 0.12, sustainTau: 0.5 };

	if (sound === "organ") {
		voice.percussive = false;
		voice.releaseTau = 0.05;
		const bars = [[0.5, 0.5], [1, 1], [2, 0.8], [3, 0.4], [4, 0.25]];
		for (const [ratio, level] of bars) addOsc("sine", f * ratio, env, level / 3);
		env.gain.setValueAtTime(0.0001, t);
		env.gain.linearRampToValueAtTime(1, t + 0.02);
	} else if (sound === "synth") {
		voice.percussive = false;
		voice.releaseTau = 0.12;
		const lp = ctx.createBiquadFilter();
		lp.type = "lowpass";
		lp.Q.value = 4;
		lp.frequency.setValueAtTime(Math.min(f * 8, 12000), t);
		lp.frequency.setTargetAtTime(Math.min(f * 2, 6000), t, 0.18);
		lp.connect(env);
		addOsc("sawtooth", f * cents(-7), lp, 0.5);
		addOsc("sawtooth", f * cents(7), lp, 0.5);
		env.gain.setValueAtTime(0.0001, t);
		env.gain.linearRampToValueAtTime(1, t + 0.01);
		env.gain.setTargetAtTime(0.6, t + 0.01, 0.2);
	} else if (sound === "electric") {
		voice.decay = Math.min(3.5, 1.5 + 260 / f);
		const mod = ctx.createOscillator();
		mod.type = "sine";
		mod.frequency.value = f;
		const index = ctx.createGain();
		index.gain.setValueAtTime(f * 1.8, t);
		index.gain.exponentialRampToValueAtTime(f * 0.12, t + voice.decay * 0.6);
		mod.connect(index);
		const carrier = addOsc("sine", f, env);
		index.connect(carrier.frequency);
		mod.start(t);
		oscs.push(mod);
		const tine = ctx.createGain();
		pluck(tine.gain, 0.06, 0.35);
		tine.connect(env);
		addOsc("sine", f * 14, tine);
		pluck(env.gain, 1, voice.decay);
	} else if (sound === "marimba") {
		voice.decay = Math.max(0.25, Math.min(1.5, 1.1 * Math.pow(260 / f, 0.3)));
		voice.releaseTau = 0.08;
		const over = ctx.createGain();
		pluck(over.gain, 0.3, 0.12);
		over.connect(env);
		addOsc("sine", f, env);
		addOsc("sine", f * 4, over);
		pluck(env.gain, 1, voice.decay);
	} else {
		// piano: slightly inharmonic partials, brightness that fades as the note decays
		voice.decay = Math.max(0.8, Math.min(5, 4 * Math.pow(260 / f, 0.4)));
		const lp = ctx.createBiquadFilter();
		lp.type = "lowpass";
		lp.frequency.setValueAtTime(Math.min(f * 12, 16000), t);
		lp.frequency.exponentialRampToValueAtTime(Math.min(f * 3, 8000), t + voice.decay * 0.5);
		lp.connect(env);
		const partials = [[1, 1], [2, 0.5], [3, 0.25], [4, 0.12], [5, 0.06]];
		for (const [n, level] of partials) addOsc("sine", f * n * (1 + 0.0004 * n * n), lp, level / 2);
		addOsc("sine", f * cents(3), lp, 0.4);
		pluck(env.gain, 1, voice.decay);
	}
	if (voice.percussive) stopVoiceAt(voice, t + voice.decay + 0.2);
	return voice;
}

function stopVoiceAt(voice, when) {
	for (const o of voice.oscs) {
		try {
			o.stop(when);
		} catch (_) {
			// already stopped
		}
	}
	if (voice.oscs[0]) {
		voice.oscs[0].onended = () => {
			try {
				voice.out.disconnect();
			} catch (_) {
				// already disconnected
			}
		};
	}
}

function releaseVoice(voice, sustained) {
	if (!audio || !voice) return;
	if (voice.percussive && sustained) return; // keep ringing until it decays on its own
	const t = audio.ctx.currentTime;
	const tau = voice.percussive ? voice.releaseTau : sustained ? voice.sustainTau : voice.releaseTau;
	voice.out.gain.setTargetAtTime(0, t, tau);
	stopVoiceAt(voice, t + tau * 8);
}

// ---------- notes ----------

function setKeyPressed(midi, pressed) {
	const el = keyEls.get(midi);
	if (!el) return;
	const base = el.className.replace(/\s*\bpressed\b/g, "");
	el.className = pressed ? base + " pressed" : base;
	el.setAttribute("aria-pressed", pressed ? "true" : "false");
}

function pressNote(midi) {
	const entry = held.get(midi);
	if (entry) {
		entry.count++;
		return;
	}
	const a = ensureAudio();
	const voice = a ? buildVoice(state.sound, a.ctx, a.master, midi) : null;
	held.set(midi, { count: 1, voice });
	setKeyPressed(midi, true);
}

function releaseNote(midi) {
	const entry = held.get(midi);
	if (!entry) return;
	if (--entry.count > 0) return;
	held.delete(midi);
	releaseVoice(entry.voice, cfg.sustain);
	setKeyPressed(midi, false);
}

function releaseAll() {
	pointers.clear();
	computerKeys.clear();
	for (const midi of [...held.keys()]) {
		const entry = held.get(midi);
		held.delete(midi);
		releaseVoice(entry.voice, false);
		setKeyPressed(midi, false);
	}
}

// ---------- pointer input ----------

function midiFromElement(el) {
	for (let n = el; n; n = n.parentNode) {
		const v = n.getAttribute ? n.getAttribute("data-midi") : null;
		if (v !== null && v !== undefined) return Number(v);
	}
	return null;
}

function midiAtPoint(e) {
	const el = document.elementFromPoint ? document.elementFromPoint(e.clientX, e.clientY) : null;
	return midiFromElement(el);
}

function onKeyPointerDown(e, midi) {
	if (e.button !== undefined && e.button > 0) return;
	if (e.preventDefault) e.preventDefault();
	ensureAudio();
	const previous = pointers.get(e.pointerId);
	if (previous !== undefined && previous !== null) releaseNote(previous);
	pointers.set(e.pointerId, midi);
	pressNote(midi);
}

function onPointerMove(e) {
	if (!pointers.has(e.pointerId)) return;
	const current = pointers.get(e.pointerId);
	const next = midiAtPoint(e);
	if (next === current) return;
	if (current !== null) releaseNote(current);
	pointers.set(e.pointerId, next);
	if (next !== null) pressNote(next);
}

function onPointerEnd(e) {
	if (!pointers.has(e.pointerId)) return;
	const current = pointers.get(e.pointerId);
	pointers.delete(e.pointerId);
	if (current !== null) releaseNote(current);
}

// ---------- computer keyboard ----------

function onKeyDown(e) {
	if (e.repeat || e.ctrlKey || e.metaKey || e.altKey) return;
	const k = String(e.key).toLowerCase();
	if (computerKeys.has(k)) return;
	const midi = computerKeyToMidi(k, state.startOctave);
	if (midi === null) return;
	ensureAudio();
	computerKeys.set(k, midi);
	pressNote(midi);
}

function onKeyUp(e) {
	const k = String(e.key).toLowerCase();
	if (!computerKeys.has(k)) return;
	const midi = computerKeys.get(k);
	computerKeys.delete(k);
	releaseNote(midi);
}

// ---------- rendering ----------

function labelFor(midi, mode) {
	if (mode === "none") return "";
	if (mode === "all") return noteName(midi);
	return midi % 12 === 0 ? noteName(midi) : "";
}

function buildKeyboard() {
	releaseAll();
	keyEls.clear();
	const { keys } = keyboardLayout(state.octaves, state.startOctave);
	const els = keys.map((k) => {
		const el = document.createElement("div");
		el.className = k.black ? "key black" : "key white";
		el.setAttribute("data-midi", String(k.midi));
		el.setAttribute("aria-label", noteName(k.midi));
		el.setAttribute("aria-pressed", "false");
		el.style.left = `${(k.x * 100).toFixed(4)}%`;
		el.style.width = `${(k.w * 100).toFixed(4)}%`;
		el.addEventListener("pointerdown", (e) => onKeyPointerDown(e, k.midi));
		keyEls.set(k.midi, el);
		return el;
	});
	$("keyboard").replaceChildren(...els);
	layoutKey = `${state.octaves}:${state.startOctave}`;
}

function applyLabels() {
	for (const [midi, el] of keyEls) el.textContent = labelFor(midi, cfg.labels);
}

function renderControls() {
	const top = state.startOctave + state.octaves;
	$("oct-label").textContent = `C${state.startOctave}–C${top}`;
	$("oct-down").disabled = state.startOctave <= MIN_START_OCTAVE;
	$("oct-up").disabled = state.startOctave >= MAX_START_OCTAVE;
	for (const btn of $("sounds").children) btn.setAttribute("aria-pressed", btn.getAttribute("data-sound") === state.sound ? "true" : "false");
}

function applyTheme() {
	const style = document.documentElement.style;
	style.setProperty("--accent", cfg.accentColor);
	style.setProperty("--bg", cfg.backgroundColor);
}

function shiftOctave(delta) {
	const next = clampInt(state.startOctave + delta, MIN_START_OCTAVE, MAX_START_OCTAVE, state.startOctave);
	if (next === state.startOctave) return;
	state.startOctave = next;
	saveState();
	render();
}

function chooseSound(sound) {
	state.sound = sound;
	saveState();
	renderControls();
}

// Rebuilds the keys only when the layout changed.
function render() {
	if (layoutKey !== `${state.octaves}:${state.startOctave}`) buildKeyboard();
	applyLabels();
	applyTheme();
	renderControls();
}

function buildControls() {
	const sounds = $("sounds");
	const buttons = SOUNDS.map((s) => {
		const b = document.createElement("button");
		b.type = "button";
		b.textContent = SOUND_LABELS[s];
		b.setAttribute("data-sound", s);
		b.addEventListener("click", () => chooseSound(s));
		return b;
	});
	sounds.replaceChildren(...buttons);
	$("oct-down").addEventListener("click", () => shiftOctave(-1));
	$("oct-up").addEventListener("click", () => shiftOctave(1));
}

// ---------- lifecycle ----------

function refreshState() {
	cfg = readSettings();
	state = seededState(cfg, loadStored());
	saveState();
}

function init() {
	if (initialized) return;
	initialized = true;
	refreshState();
	buildControls();
	render();
	document.addEventListener("pointermove", onPointerMove);
	document.addEventListener("pointerup", onPointerEnd);
	document.addEventListener("pointercancel", onPointerEnd);
	document.addEventListener("lostpointercapture", onPointerEnd);
	document.addEventListener("pointerdown", () => ensureAudio());
	document.addEventListener("keydown", onKeyDown);
	document.addEventListener("keyup", onKeyUp);
	window.addEventListener("blur", releaseAll);
	console.log(`piano: ${state.octaves} octaves from C${state.startOctave}, sound ${state.sound}`);
}

function onSettingsChanged() {
	if (!initialized) return; // boot calls init() first; late loads call it themselves
	refreshState();
	render();
	applyVolume();
}

if (typeof iCUE_initialized !== "undefined" && iCUE_initialized) init();
