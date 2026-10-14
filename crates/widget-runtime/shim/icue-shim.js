// EdgeHost iCUE compatibility shim (milestone M1).
//
// Injected at document start, main frame only, before any widget script runs. It recreates the
// globals iCUE documents for HTML widgets (Widget API 1.9.0):
//   iCUE, device, uniqueId, one global per x-icue-property, tr(), window.plugins,
//   iCUE_initialized, plugin<Module>_initialized, and the icueEvents lifecycle calls.
// Plugins EdgeHost provides get a window.plugins.<Facade> object whose methods call the host and
// whose signals have connect/disconnect; the rest keep plugin<Facade>_initialized false so widgets
// that guard on it degrade instead of throwing ReferenceError.
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

	// fetch() to domains the manifest declares goes through EdgeHost (same-origin, so APIs that send
	// no CORS headers still work). EdgeHost re-checks the permission; this only picks the route.
	const proxied = (url) => {
		if (url.protocol !== "https:" && url.protocol !== "http:") return false;
		const host = url.hostname.toLowerCase().replace(/^\[|\]$/g, "");
		const port = Number(url.port) || (url.protocol === "https:" ? 443 : 80);
		const loopback = host === "localhost" || host.startsWith("127.") || host === "::1";
		return P.proxy.some((p) => {
			const hostOk = p.domain === "any" ? !loopback : host === p.domain || host.endsWith(`.${p.domain}`);
			const portOk = p.port == null ? !loopback : p.port === port;
			return hostOk && portOk;
		});
	};
	if (P.proxy.length > 0 && typeof window.fetch === "function") {
		const nativeFetch = window.fetch.bind(window);
		window.fetch = (input, init) => {
			try {
				const isRequest = typeof Request !== "undefined" && input instanceof Request;
				const url = new URL(isRequest ? input.url : String(input), location.href);
				const method = String((init && init.method) || (isRequest ? input.method : "GET")).toUpperCase();
				if (method === "GET" && proxied(url)) {
					return nativeFetch(`/__edgehost/fetch?url=${encodeURIComponent(url.href)}`, {
						signal: init && init.signal,
					});
				}
			} catch (_) {
				// Unparseable input: let the native fetch report it.
			}
			return nativeFetch(input, init);
		};
	}

	// Input diagnostics (EDGEHOST_DEBUG_INPUT=1): log what each tap actually delivers.
	if (P.debugInput) {
		const describe = (t) =>
			t && t.tagName ? `${t.tagName.toLowerCase()}${t.id ? `#${t.id}` : ""}${t.textContent ? ` "${t.textContent.trim().slice(0, 12)}"` : ""}` : String(t);
		for (const type of ["pointerdown", "pointerup", "pointercancel", "mousedown", "mouseup", "click", "touchstart", "touchend"]) {
			window.addEventListener(
				type,
				(e) =>
					post({
						k: "console",
						level: "input",
						msg: `${type} ${e.pointerType || ""} x=${Math.round(e.clientX ?? -1)} y=${Math.round(e.clientY ?? -1)} buttons=${e.buttons ?? ""} trusted=${e.isTrusted} → ${describe(e.target)}`,
					}),
				true,
			);
		}
	}

	// Typing needs EdgeHost to be the active app; ask for it when a text field gains focus.
	const EDITABLE = /^(INPUT|TEXTAREA|SELECT)$/;
	window.addEventListener(
		"focusin",
		(e) => {
			const t = e.target;
			if (t && (EDITABLE.test(t.tagName) || t.isContentEditable)) post({ k: "keyboard" });
		},
		true,
	);

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
	// ── Plugins ── calls and signals travel over the widget's own origin (widget_runtime::plugins).
	const PLUGIN_ROUTE = "/__edgehost/plugin/";
	const hostFetch = window.fetch ? window.fetch.bind(window) : null;
	const getJson = async (path) => {
		const response = await hostFetch(PLUGIN_ROUTE + path, { cache: "no-store" });
		return response.json();
	};
	const available = P.plugins.filter((p) => p.available);
	const signalHandlers = new Map(); // "Facade.signal" → Set of handlers
	// Forget state left by a previous load of this page before the widget's first call.
	const pluginsReady = available.length > 0 && hostFetch ? getJson("reset").then((r) => Number(r.seq) || 0) : Promise.resolve(0);
	// Calls run one at a time, in order: clearPaths() then watchPath() must not be reordered.
	let callChain = pluginsReady.catch(() => undefined);
	for (const plugin of P.plugins) {
		window[`plugin${plugin.name}_initialized`] = false;
		if (!plugin.available) {
			// Evaluate pages never get providers; only a shown widget needs to hear about it.
			if (P.mode !== "evaluate") console.warn(`plugin ${plugin.name} is not available in EdgeHost`);
			continue;
		}
		const facade = {};
		for (const method of plugin.methods) {
			facade[method] = (...args) => {
				const query = `call?p=${encodeURIComponent(plugin.name)}&m=${encodeURIComponent(method)}&a=${encodeURIComponent(JSON.stringify(args))}`;
				const result = callChain
					.then(() => getJson(query))
					.then((reply) => {
						if (reply.error) throw new Error(`${plugin.name}.${method}: ${reply.error}`);
						return reply.value;
					})
					.catch((e) => {
						console.warn(fmt(e));
						return undefined;
					});
				callChain = result;
				return result;
			};
		}
		for (const signal of plugin.signals) {
			const handlers = new Set();
			signalHandlers.set(`${plugin.name}.${signal}`, handlers);
			facade[signal] = {
				connect: (fn) => void (typeof fn === "function" && handlers.add(fn)),
				disconnect: (fn) => void handlers.delete(fn),
			};
		}
		window.plugins[plugin.name] = facade;
	}
	const dispatch = (event) => {
		for (const fn of signalHandlers.get(`${event.plugin}.${event.signal}`) || []) {
			try {
				fn(...event.args);
			} catch (e) {
				console.error(`${event.plugin}.${event.signal} handler threw:`, e);
			}
		}
	};
	const pumpSignals = async () => {
		let after = await pluginsReady;
		for (;;) {
			try {
				const events = await getJson(`events?after=${after}`);
				for (const event of events) {
					after = Math.max(after, event.seq);
					dispatch(event);
				}
			} catch (_) {
				await new Promise((resolve) => setTimeout(resolve, 1000));
			}
		}
	};
	const startPlugins = async () => {
		if (available.length === 0 || !hostFetch) return;
		try {
			await pluginsReady;
		} catch (e) {
			console.error(`plugins could not start: ${fmt(e)}`);
			return;
		}
		void pumpSignals();
		for (const plugin of available) {
			window[`plugin${plugin.name}_initialized`] = true;
			callHook(`plugin${plugin.name}Events`, "onInitialized");
		}
	};

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
		await startPlugins();
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
