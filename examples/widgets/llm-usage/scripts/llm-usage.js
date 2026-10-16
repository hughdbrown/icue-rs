// LLM Usage widget: quota left, reset times and pace of each LLM subscription, from EdgeHost's
// built-in LlmUsage plugin (edgehost.llmusage:LlmUsage:1.0). Subscriptions and credentials live
// in llm-usage.toml (`edgectl llm-usage init`), not in widget settings.
(function () {
	"use strict";

	const M = window.LlmModel;
	const $ = (id) => document.getElementById(id);
	const TICK_MS = 30 * 1000;

	let snapshot = null;

	const plugin = () => window.plugins && window.plugins.Llmusage;

	function make(tag, className, text) {
		const e = document.createElement(tag);
		if (className) e.className = className;
		if (text !== undefined) e.textContent = text;
		return e;
	}

	function windowRow(w, i) {
		const row = make("div", `win state-${w.state}${i >= 2 ? " win-extra" : ""}`);
		const bar = make("div", "bar");
		const fill = make("div", "fill");
		fill.style.width = `${(w.used * 100).toFixed(1)}%`;
		bar.append(fill);
		if (w.tick !== null) {
			const tick = make("div", "tick");
			tick.style.left = `${(w.tick * 100).toFixed(1)}%`;
			bar.append(tick);
		}
		row.append(make("span", "wlabel", w.label), bar, make("span", "left", w.left), make("span", "reset", w.reset));
		return row;
	}

	function card(s) {
		const el = make("article", `sub status-${s.status}${s.suggested ? " suggested" : ""}`);
		const head = make("header");
		head.append(make("span", "name", s.label));
		if (s.plan) head.append(make("span", "plan", s.plan));
		if (s.suggested) head.append(make("span", "flag", "Use next"));
		if (s.status !== "ok") head.append(make("span", "status", s.status === "stale" ? "stale" : "error"));
		if (s.detail) head.append(make("span", "detail", s.detail));
		el.append(head);
		if (s.message) el.append(make("div", "msg", s.message));
		s.windows.forEach((w, i) => el.append(windowRow(w, i)));
		if (s.note.text) el.append(make("div", `note note-${s.note.kind}`, s.note.text));
		return el;
	}

	function render() {
		const v = M.view(snapshot, Date.now());
		const isList = v.kind === "list";
		$("usage").hidden = !isList;
		$("message").hidden = isList;
		if (!isList) {
			$("message-title").textContent = v.title;
			$("message-detail").textContent = v.detail;
			return;
		}
		$("suggestion").textContent = v.suggestion;
		$("updated").textContent = v.updated;
		$("subs").replaceChildren(...v.subs.map(card));
		$("subs").className = `count-${Math.min(v.subs.length, 5)}`;
	}

	function onUpdated(snap) {
		snapshot = snap || null;
		render();
	}

	function applyColors() {
		const root = document.documentElement.style;
		if (typeof textColor !== "undefined") root.setProperty("--fg", textColor);
		if (typeof accentColor !== "undefined") root.setProperty("--accent", accentColor);
		if (typeof backgroundColor !== "undefined") root.setProperty("--bg", backgroundColor);
	}

	function connect() {
		const p = plugin();
		if (!p || connect.done) return;
		connect.done = true;
		p.updated.connect(onUpdated);
		// The first poll may already have reported; take what the plugin has.
		Promise.resolve(p.snapshot()).then((snap) => {
			if (snap && !snapshot) onUpdated(snap);
		});
	}

	function onReady() {
		applyColors();
		connect();
		if (!plugin()) {
			$("usage").hidden = true;
			$("message").hidden = false;
			$("message-title").textContent = "LlmUsage plugin missing";
			$("message-detail").textContent = "This widget needs EdgeHost's built-in edgehost.llmusage:LlmUsage:1.0 plugin.";
			return;
		}
		render();
	}

	$("refresh").addEventListener("click", () => {
		const p = plugin();
		if (!p) return;
		$("refresh").classList.add("spinning");
		setTimeout(() => $("refresh").classList.remove("spinning"), 1200);
		p.refresh();
	});

	window.pluginLlmusageEvents = { onInitialized: connect };
	window.icueEvents = { onICUEInitialized: onReady, onDataUpdated: applyColors };
	setInterval(() => {
		if (snapshot) render();
	}, TICK_MS);
})();
