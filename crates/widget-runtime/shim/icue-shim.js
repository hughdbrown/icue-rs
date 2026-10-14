// EdgeHost iCUE compatibility shim (milestone M1).
//
// Injected at document start, main frame only, before any widget script runs. It recreates the
// globals iCUE documents for HTML widgets (Widget API 1.9.0):
//   iCUE, device, uniqueId, one global per x-icue-property, tr(), window.plugins,
//   iCUE_initialized, plugin<Module>_initialized, and the icueEvents lifecycle calls.
// Plugins are not implemented yet (M3): their *_initialized flags stay false so widgets that
// guard on them degrade instead of throwing ReferenceError.
//
// The payload placeholder on the last line is replaced by Rust (widget_runtime::shim::render)
// with script-safe JSON; it must appear exactly once in this file.
(function (P) {
	"use strict";

	const post = (message) => {
		try {
			window.ipc.postMessage(JSON.stringify(message));
		} catch (_) {
			// Not running inside EdgeHost (e.g. under test): nothing to report to.
		}
	};

	const fmt = (value) => {
		if (typeof value === "string") return value;
		// WebKit stacks omit the message, so lead with it.
		if (value instanceof Error) return `${value.name}: ${value.message}${value.stack ? `\n${value.stack}` : ""}`;
		try {
			return JSON.stringify(value);
		} catch (_) {
			return String(value);
		}
	};

	for (const level of ["log", "info", "warn", "error"]) {
		const original = console[level].bind(console);
		console[level] = (...args) => {
			original(...args);
			post({ k: "console", level, msg: args.map(fmt).join(" ") });
		};
	}
	window.addEventListener("error", (e) =>
		post({ k: "console", level: "error", msg: `${e.message} (${e.filename}:${e.lineno}:${e.colno})` }),
	);
	window.addEventListener("unhandledrejection", (e) =>
		post({ k: "console", level: "error", msg: `unhandled rejection: ${fmt(e.reason)}` }),
	);
	// Key events go to the web view, not the native window, so the dev-run Escape shortcut lives here.
	if (P.escapeCloses) {
		window.addEventListener(
			"keydown",
			(e) => {
				if (e.key === "Escape") post({ k: "close" });
			},
			true,
		);
	}

	const trTable = P.tr;
	const trSync = (key) => (Object.prototype.hasOwnProperty.call(trTable, key) ? trTable[key] : String(key));

	window.iCUE = {
		iCUELanguage: P.lang,
		fpsLimit: P.fpsLimit,
		isPreview: P.preview,
		widgetId: P.widgetId,
		defaultTemperatureUnit: () => P.tempUnit,
	};
	window.device = { deviceId: P.deviceId };
	window.uniqueId = P.instanceId;
	window.tr = (key) => Promise.resolve(trSync(key));
	window.plugins = window.plugins || {};
	window.iCUE_initialized = false;
	for (const facade of P.plugins) {
		window[`plugin${facade}_initialized`] = false;
		console.warn(`plugin ${facade} is not available yet in EdgeHost`);
	}

	// Attribute expressions see a synchronous tr(), as iCUE resolves them to plain strings.
	const evaluate = (expr) =>
		new Function("tr", `"use strict"; return (${expr});`)(trSync);

	// Defaults that reference manifest modules (e.g. CitySearch.getDefault()) cannot be evaluated
	// until those modules are imported; retry them during boot.
	const deferred = [];
	for (const p of P.props) {
		window[p.name] = undefined;
		if (p.defaultExpr == null) continue;
		try {
			window[p.name] = evaluate(p.defaultExpr);
		} catch (_) {
			deferred.push(p);
		}
	}

	const IDENT = /^[A-Za-z_$][\w$]*$/;
	// Widgets may declare their hooks with `var`, bare assignment, or `let`/`const`; the last
	// creates a global lexical binding that is not a window property, so fall back to an
	// indirect (global-scope) eval of the bare name.
	const globalValue = (name) => {
		if (name in window) return window[name];
		try {
			return (0, eval)(`typeof ${name} === "undefined" ? undefined : ${name}`);
		} catch (_) {
			return undefined;
		}
	};
	const callHook = (objectName, hookName) => {
		const target = globalValue(objectName);
		const hook = target && target[hookName];
		if (typeof hook !== "function") return;
		try {
			hook();
		} catch (e) {
			console.error(`${objectName}.${hookName} threw:`, e);
		}
	};

	let booted = false;
	const boot = async () => {
		if (booted) return;
		booted = true;
		for (const m of P.modules) {
			try {
				const ns = await import(m.url);
				if (IDENT.test(m.global)) window[m.global] = ns;
			} catch (e) {
				console.error(`module ${m.url} failed to load:`, e);
			}
		}
		for (const p of deferred) {
			try {
				window[p.name] = evaluate(p.defaultExpr);
			} catch (e) {
				console.warn(`default for property ${p.name} could not be evaluated: ${fmt(e)}`);
			}
		}
		window.iCUE_initialized = true;
		callHook("icueEvents", "onICUEInitialized");
		callHook("icueEvents", "onDataUpdated");
		post({ k: "booted" });
	};

	if (document.readyState === "loading") {
		document.addEventListener("DOMContentLoaded", () => void boot());
	} else {
		void boot();
	}

	window.__edgehost = { boot };
})(__PAYLOAD__);
