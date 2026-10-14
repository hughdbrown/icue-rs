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
	const evaluate = (expr) => new Function("tr", `"use strict"; return (${expr});`)(trSync);
	const own = (object, key) => object != null && Object.prototype.hasOwnProperty.call(object, key);

	// Runtime pages receive concrete values; anything missing is undefined, as in iCUE.
	for (const p of P.props) {
		window[p.name] = own(P.values, p.name) ? P.values[p.name] : undefined;
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

	const importModules = async () => {
		for (const m of P.modules) {
			try {
				const ns = await import(m.url);
				if (IDENT.test(m.global)) window[m.global] = ns;
			} catch (e) {
				console.error(`module ${m.url} failed to load:`, e);
			}
		}
	};

	const EVALUATE_TIMEOUT_MS = 10000;
	const withTimeout = (promise, ms) =>
		Promise.race([
			promise,
			new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out after ${ms} ms`)), ms)),
		]);

	// Evaluate mode (hidden page): compute defaults after modules (and, from M3, provider
	// snapshots) are available, then report them. Values that JSON cannot carry are dropped.
	const evaluateDefaults = async () => {
		await importModules();
		const values = {};
		for (const p of P.props) {
			if (p.defaultExpr == null) continue;
			try {
				let value = evaluate(p.defaultExpr);
				// search-combobox defaults are provider functions that may return a promise of
				// [key, displayText]; iCUE calls them and stores the key.
				if (typeof value === "function") value = value();
				if (value && typeof value.then === "function") value = await withTimeout(value, EVALUATE_TIMEOUT_MS);
				if (p.kind === "search-combobox" && Array.isArray(value)) value = value[0];
				if (value !== undefined) {
					values[p.name] = value;
					window[p.name] = value;
				}
			} catch (e) {
				console.warn(`default for property ${p.name} could not be evaluated: ${fmt(e)}`);
			}
		}
		let plain = {};
		try {
			plain = JSON.parse(JSON.stringify(values));
		} catch (e) {
			console.error(`defaults are not serialisable: ${fmt(e)}`);
		}
		post({ k: "evaluated", values: plain });
	};

	let booted = false;
	const boot = async () => {
		if (booted) return;
		booted = true;
		if (P.mode === "evaluate") {
			await evaluateDefaults();
			return;
		}
		await importModules();
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
