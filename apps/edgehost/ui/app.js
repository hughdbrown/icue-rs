// EdgeHost configurator UI. Vanilla JS; talks to the host per docs/configurator-api.md.
(function () {
	"use strict";

	const root = typeof window !== "undefined" ? window : globalThis;
	const SLOTS = ["S", "M", "L", "XL"];
	const DEBOUNCE_MS = 300;
	const RPC_TIMEOUT_MS = 10000;
	const TOAST_MS = 7000;
	const DRAG_THRESHOLD_PX = 4;

	// ---------------------------------------------------------------- RPC client

	function createRpc(post, timeoutMs) {
		let nextId = 1;
		const pending = new Map();

		function call(method, params) {
			const id = nextId++;
			return new Promise((resolve, reject) => {
				const timer = setTimeout(() => {
					pending.delete(id);
					reject(new Error(`The host did not answer ${method} in time.`));
				}, timeoutMs);
				pending.set(id, { resolve, reject, timer });
				try {
					post(JSON.stringify({ id, method, params: params || {} }));
				} catch (e) {
					clearTimeout(timer);
					pending.delete(id);
					reject(e);
				}
			});
		}

		function reply(msg) {
			if (!msg || !pending.has(msg.id)) return;
			const p = pending.get(msg.id);
			pending.delete(msg.id);
			clearTimeout(p.timer);
			if (msg.error) {
				p.reject(new Error((msg.error && msg.error.message) || String(msg.error)));
			} else {
				p.resolve(msg.result);
			}
		}

		return { call, reply, pending };
	}

	const rpc = createRpc((text) => root.ipc.postMessage(text), RPC_TIMEOUT_MS);

	// ---------------------------------------------------------------- app state

	const S = {
		state: null,
		page: 0,
		selected: null,
		drag: null,
		pendingRender: false,
		fetchSeq: 0,
		toastTimer: null,
		timers: new Map(), // debounce timers keyed by instance+property
	};

	// ---------------------------------------------------------------- DOM helpers

	const $ = (id) => document.getElementById(id);
	const DIRECT_PROPS = new Set([
		"value", "checked", "disabled", "type", "min", "max", "step", "placeholder",
		"hidden", "src", "alt", "title", "htmlFor", "id", "rows", "textContent",
	]);

	function h(tag, props, ...kids) {
		const el = document.createElement(tag);
		for (const [k, v] of Object.entries(props || {})) {
			if (v === undefined || v === null || v === false && k !== "checked" && k !== "disabled" && k !== "hidden") continue;
			if (k === "class") el.className = v;
			else if (k === "style") {
				for (const [sk, sv] of Object.entries(v)) {
					if (sk.startsWith("--")) el.style.setProperty(sk, sv);
					else el.style[sk] = sv;
				}
			} else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
			else if (DIRECT_PROPS.has(k)) el[k] = v;
			else el.setAttribute(k, String(v));
		}
		for (const kid of kids.flat()) {
			if (kid === null || kid === undefined || kid === false) continue;
			el.append(typeof kid === "string" || typeof kid === "number" ? document.createTextNode(String(kid)) : kid);
		}
		return el;
	}

	// replaceChildren that tolerates nested arrays and null/false placeholders.
	function fill(el, ...kids) {
		el.replaceChildren(...kids.flat(Infinity).filter((k) => k !== null && k !== undefined && k !== false));
	}

	function walk(node, fn) {
		for (const c of node.children || []) {
			if (fn(c) === true) return c;
			const r = walk(c, fn);
			if (r) return r;
		}
		return null;
	}

	// ---------------------------------------------------------------- model helpers

	function widgetOf(inst) {
		const ws = S.state.widgets;
		return ws.find((w) => w.id === inst.widget && w.version === inst.version) || ws.find((w) => w.id === inst.widget) || null;
	}

	function widgetName(inst) {
		const w = widgetOf(inst);
		return w ? w.name : inst.widget;
	}

	function instancesOnPage(page) {
		const p = S.state.pages[page];
		return p ? p.instances : [];
	}

	function findInstance(id) {
		const pages = S.state.pages;
		for (let i = 0; i < pages.length; i++) {
			const inst = pages[i].instances.find((x) => x.id === id);
			if (inst) return { inst, page: i };
		}
		return null;
	}

	function spanOf(slot) {
		return S.state.slots[slot] || { cols: 1, rows: 1 };
	}

	function overlaps(a, b) {
		return a.col < b.col + b.cols && b.col < a.col + a.cols && a.row < b.row + b.rows && b.row < a.row + a.rows;
	}

	// Can `inst` (with a possibly different slot) sit at `origin` on `page` without leaving the grid or hitting another tile?
	function placementValid(inst, page, origin, slot) {
		const { cols, rows } = S.state.grid;
		const span = spanOf(slot);
		if (origin.col < 0 || origin.row < 0 || origin.col + span.cols > cols || origin.row + span.rows > rows) return false;
		const me = { col: origin.col, row: origin.row, cols: span.cols, rows: span.rows };
		return instancesOnPage(page).every((o) => {
			if (o.id === inst.id) return true;
			const s = spanOf(o.slot);
			return !overlaps(me, { col: o.origin.col, row: o.origin.row, cols: s.cols, rows: s.rows });
		});
	}

	function cellAt(point, rect, grid) {
		if (!rect || !(rect.width > 0) || !(rect.height > 0)) return null;
		return {
			col: Math.floor(((point.clientX - rect.left) / rect.width) * grid.cols),
			row: Math.floor(((point.clientY - rect.top) / rect.height) * grid.rows),
		};
	}

	const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));

	// ---------------------------------------------------------------- toast

	function toast(message, kind) {
		const el = $("toast");
		el.textContent = message;
		el.className = "toast " + (kind || "error");
		el.hidden = false;
		el.setAttribute("role", kind === "info" ? "status" : "alert");
		if (S.toastTimer) clearTimeout(S.toastTimer);
		S.toastTimer = setTimeout(hideToast, TOAST_MS);
	}

	function hideToast() {
		const el = $("toast");
		el.hidden = true;
		S.toastTimer = null;
	}

	function showError(e) {
		toast(e && e.message ? e.message : String(e), "error");
	}

	// ---------------------------------------------------------------- confirm dialog

	// An in-page replacement for window.confirm(): WKWebView answers confirm() with Cancel unless
	// the host implements a UI-delegate method, which wry does not, so confirm() always returned
	// false and nothing could be deleted. Resolves true for the confirm button, false for Cancel,
	// Escape or a click outside. While one is open, another request resolves false at once.
	let confirmOpen = false;
	function confirmInPage(message, confirmLabel) {
		if (confirmOpen) return Promise.resolve(false);
		confirmOpen = true;
		const opener = document.activeElement;
		return new Promise((resolve) => {
			const done = (answer) => {
				confirmOpen = false;
				backdrop.remove();
				if (opener && opener.focus) opener.focus();
				resolve(answer);
			};
			const ok = h("button", { class: "danger", "data-action": "confirm", onclick: () => done(true) }, confirmLabel);
			const cancel = h("button", { "data-action": "cancel", onclick: () => done(false) }, "Cancel");
			const box = h("div", { class: "confirm-box" }, h("p", {}, message), h("div", { class: "confirm-actions" }, cancel, ok));
			const backdrop = h("div", {
				class: "confirm-backdrop",
				role: "dialog",
				"aria-modal": "true",
				"aria-label": message,
				onkeydown: (e) => {
					if (e.key === "Escape") {
						if (e.preventDefault) e.preventDefault();
						done(false);
					}
				},
				onclick: (e) => { if (e.target === backdrop) done(false); },
			}, box);
			document.body.append(backdrop);
			ok.focus();
		});
	}

	// ---------------------------------------------------------------- data flow

	async function refresh() {
		const seq = ++S.fetchSeq;
		try {
			const st = await rpc.call("state.get", {});
			if (seq !== S.fetchSeq) return;
			S.state = st;
			normalize();
			render();
		} catch (e) {
			if (seq === S.fetchSeq) showError(e);
		}
	}

	function normalize() {
		const st = S.state;
		S.page = clamp(S.page, 0, st.pages.length);
		if (S.selected && !findInstance(S.selected)) S.selected = null;
	}

	// Run a mutating call, then re-fetch state (also after a failure, so the UI never stays stale).
	async function mutate(method, params) {
		try {
			const result = await rpc.call(method, params);
			await refresh();
			return result;
		} catch (e) {
			showError(e);
			refresh();
			return null;
		}
	}

	function debounced(key, fn) {
		if (S.timers.has(key)) clearTimeout(S.timers.get(key));
		S.timers.set(key, setTimeout(() => {
			S.timers.delete(key);
			fn();
		}, DEBOUNCE_MS));
	}

	function cancelDebounce(key) {
		if (S.timers.has(key)) {
			clearTimeout(S.timers.get(key));
			S.timers.delete(key);
		}
	}

	function setProp(inst, name, value, debounce) {
		const key = inst.id + "\u0000" + name;
		cancelDebounce(key);
		const send = () => mutate("layout.set", { instance: inst.id, key: name, value });
		if (debounce) debounced(key, send);
		else send();
	}

	function unsetProp(inst, name) {
		cancelDebounce(inst.id + "\u0000" + name);
		return mutate("layout.unset", { instance: inst.id, key: name });
	}

	async function addWidget(w, slot) {
		const r = await mutate("layout.add", { widget: w.id, version: w.version, slot, page: S.page });
		if (r && r.instance && findInstance(r.instance)) {
			S.selected = r.instance;
			render();
		}
	}

	function moveInstance(inst, page, origin, slot) {
		return mutate("layout.move", { instance: inst.id, page, origin: { col: origin.col, row: origin.row }, slot });
	}

	async function removeInstance(inst) {
		if (!(await confirmInPage(`Remove "${widgetName(inst)}" from the layout?`, "Remove"))) return null;
		return mutate("layout.remove", { instance: inst.id });
	}

	function select(id) {
		S.selected = id;
		if (S.state) {
			const board = walk($("editor"), (n) => (n.className || "").split(/\s+/).includes("board"));
			if (board) {
				for (const t of board.children) {
					if (t.getAttribute("data-instance")) {
						const on = t.getAttribute("data-instance") === id;
						t.classList_set(on);
					}
				}
			}
			renderSettings();
		}
	}

	// ---------------------------------------------------------------- render

	function render() {
		if (!S.state) return;
		if (S.drag && S.drag.active) {
			S.pendingRender = true;
			return;
		}
		S.pendingRender = false;
		const ae = document.activeElement;
		const focusKey = ae && ae.getAttribute ? ae.getAttribute("data-focus") : null;
		const settingsFocused = !!(focusKey && focusKey.startsWith("prop:"));
		renderTopbar();
		renderLibrary();
		renderEditor();
		// Don't rebuild the settings under the user's cursor while a debounced edit is pending.
		if (!(settingsFocused && S.timers.size > 0)) renderSettings();
		if (focusKey) {
			const target = walk(document.body, (n) => n.getAttribute && n.getAttribute("data-focus") === focusKey);
			if (target && target !== ae) {
				target.focus();
				if (ae && typeof ae.selectionStart === "number" && typeof target.setSelectionRange === "function") {
					try { target.setSelectionRange(ae.selectionStart, ae.selectionEnd); } catch (_) { /* not a text input */ }
				}
			}
		}
	}

	function displayLabel(d) {
		const marks = (d.edge ? " [Edge]" : "") + (d.native ? " [native]" : "");
		return `${d.index}: ${d.name} (${d.pixel_size[0]}x${d.pixel_size[1]})${marks}`;
	}

	function renderTopbar() {
		const st = S.state;
		const choice = String(st.display);
		const opts = [h("option", { value: "auto" }, "Auto (find the Edge)"), h("option", { value: "windowed" }, "Windowed")];
		let matched = choice === "auto" || choice === "windowed";
		for (const d of st.displays) {
			const isMatch = choice === String(d.index) || choice === d.name;
			if (isMatch) matched = true;
			opts.push(h("option", { value: String(d.index) }, displayLabel(d)));
		}
		if (!matched) opts.push(h("option", { value: choice }, choice));
		const select = h("select", { id: "display-select", "aria-label": "Display" }, opts);
		const current = st.displays.find((d) => choice === String(d.index) || choice === d.name);
		select.value = current ? String(current.index) : choice;
		select.addEventListener("change", () => mutate("layout.display", { choice: select.value }));

		const allEmpty = st.pages.every((p) => p.instances.length === 0);
		const other = st.orientation === "landscape" ? "portrait" : "landscape";
		const orient = h("button", {
			id: "orientation-btn",
			disabled: !allEmpty,
			title: allEmpty ? `Switch to ${other}` : "Remove all widgets to change the orientation",
			onclick: () => mutate("layout.orientation", { orientation: other }),
		}, st.orientation === "landscape" ? "Landscape" : "Portrait");
		orient.disabled = !allEmpty;

		fill($("topbar"), 
			h("span", { class: "title" }, "EdgeHost"),
			h("label", {}, "Display", select),
			h("label", {}, "Orientation", orient),
			h("span", { class: "datadir", title: st.data_dir }, `Data folder: ${st.data_dir}`),
		);
	}

	function renderLibrary() {
		const st = S.state;
		const cards = st.widgets.map((w) => {
			const ph = () => h("div", { class: "w-icon-ph", "aria-hidden": "true" }, (w.name || w.id).slice(0, 1).toUpperCase());
			let icon;
			if (w.icon) {
				icon = h("img", { class: "w-icon", src: w.icon, alt: "", loading: "lazy" });
				icon.addEventListener("error", () => icon.replaceWith(ph()));
			} else icon = ph();
			const badges = [];
			if (w.required_plugins && w.required_plugins.length) badges.push(h("span", { class: "badge", title: w.required_plugins.join(", ") }, "needs plugins"));
			if (!w.supports_edge) badges.push(h("span", { class: "badge" }, "not for Edge"));
			const buttons = SLOTS.map((slot) => h("button", {
				class: "slot-btn",
				"data-slot": slot,
				"data-widget": w.id,
				"aria-label": `Add ${w.name} as ${slot}`,
				title: `Add ${w.name} (${slot}) to page ${S.page + 1}`,
				onclick: () => addWidget(w, slot),
			}, slot));
			return h("div", { class: "widget-card", "data-widget-card": w.id },
				icon,
				h("div", {}, h("div", { class: "w-name" }, w.name), h("div", { class: "w-desc", title: w.description }, w.description)),
				badges.length ? h("div", { class: "badges" }, badges) : null,
				h("div", { class: "add-row" }, h("span", {}, "Add:"), buttons),
			);
		});
		fill($("library"), 
			h("h2", {}, `Widgets (${st.widgets.length})`),
			cards.length ? cards : h("p", { class: "empty" }, "No widgets installed."),
		);
	}

	function renderEditor() {
		const st = S.state;
		const tabs = [];
		for (let i = 0; i < st.pages.length; i++) {
			tabs.push(h("button", {
				class: "page-tab" + (i === S.page ? " active" : ""),
				"aria-pressed": i === S.page ? "true" : "false",
				"data-page": i,
				onclick: () => gotoPage(i),
			}, `Page ${i + 1}`));
		}
		const newPage = S.page === st.pages.length;
		if (newPage) tabs.push(h("button", { class: "page-tab active", "aria-pressed": "true", "data-page": S.page }, `Page ${S.page + 1} (new)`));
		else tabs.push(h("button", { class: "page-add", "aria-label": "Add page", title: "Add a page", onclick: () => gotoPage(st.pages.length) }, "+ Page"));

		const { cols, rows } = st.grid;
		const board = h("div", { class: "board " + st.orientation, role: "group", "aria-label": `Page ${S.page + 1} layout` });
		for (let r = 0; r < rows; r++) {
			for (let c = 0; c < cols; c++) {
				board.append(h("div", { class: "cell", style: place({ col: c, row: r }, { cols: 1, rows: 1 }) }, h("div")));
			}
		}
		for (const inst of instancesOnPage(S.page)) board.append(makeTile(inst, board));
		board.addEventListener("keydown", onBoardKey);
		board.addEventListener("click", (e) => {
			if (e.target === board || (e.target && (e.target.className || "").split(/\s+/).includes("cell"))) select(null);
		});

		fill($("editor"), 
			h("div", { class: "page-tabs", role: "toolbar", "aria-label": "Pages" }, tabs),
			h("div", { class: "stage" }, board),
			h("p", { class: "hint" }, "Drag a tile to move it. Arrow keys move the selected tile; Delete removes it."),
		);
	}

	function place(origin, span) {
		const { cols, rows } = S.state.grid;
		return {
			left: (origin.col / cols) * 100 + "%",
			top: (origin.row / rows) * 100 + "%",
			width: (span.cols / cols) * 100 + "%",
			height: (span.rows / rows) * 100 + "%",
		};
	}

	function gotoPage(i) {
		S.page = i;
		const sel = S.selected && findInstance(S.selected);
		if (!sel || sel.page !== i) S.selected = null;
		render();
	}

	function makeTile(inst, board) {
		const span = spanOf(inst.slot);
		const name = widgetName(inst);
		const tile = h("div", {
			class: "tile" + (inst.id === S.selected ? " selected" : ""),
			role: "button",
			tabindex: "0",
			"aria-pressed": inst.id === S.selected ? "true" : "false",
			"aria-label": `${name}, size ${inst.slot}, column ${inst.origin.col + 1}, row ${inst.origin.row + 1}`,
			"data-instance": inst.id,
			"data-focus": "tile:" + inst.id,
			style: place(inst.origin, span),
		}, h("div", { class: "face" }, h("div", { class: "t-name" }, name), h("div", { class: "t-slot" }, inst.slot)));
		// Keep the class list in sync without a full re-render (used by select()).
		tile.classList_set = (on) => {
			const base = tile.className.split(/\s+/).filter((c) => c && c !== "selected");
			if (on) base.push("selected");
			tile.className = base.join(" ");
			tile.setAttribute("aria-pressed", on ? "true" : "false");
		};

		tile.addEventListener("focus", () => { if (S.selected !== inst.id) select(inst.id); });
		tile.addEventListener("keydown", (e) => {
			if ((e.key === "Enter" || e.key === " ") && S.selected !== inst.id) {
				if (e.preventDefault) e.preventDefault();
				select(inst.id);
			}
		});
		tile.addEventListener("pointerdown", (e) => {
			if (e.button) return;
			select(inst.id);
			const pc = cellAt(e, board.getBoundingClientRect(), S.state.grid);
			S.drag = {
				id: inst.id,
				inst,
				tile,
				board,
				startX: e.clientX,
				startY: e.clientY,
				grab: pc
					? { col: clamp(pc.col - inst.origin.col, 0, span.cols - 1), row: clamp(pc.row - inst.origin.row, 0, span.rows - 1) }
					: { col: 0, row: 0 },
				active: false,
				target: null,
				dropEl: null,
			};
			try { tile.setPointerCapture(e.pointerId); } catch (_) { /* synthetic or unsupported */ }
		});
		tile.addEventListener("pointermove", (e) => {
			const d = S.drag;
			if (!d || d.id !== inst.id) return;
			if (!d.active) {
				if (Math.hypot(e.clientX - d.startX, e.clientY - d.startY) < DRAG_THRESHOLD_PX) return;
				d.active = true;
				tile.classList_set(true);
				tile.className += " dragging";
				d.dropEl = h("div", { class: "drop" }, h("div"));
				board.append(d.dropEl);
			}
			updateDrop(d, e);
		});
		const finish = async (e, commit) => {
			const d = S.drag;
			if (!d || d.id !== inst.id) return;
			S.drag = null;
			if (d.dropEl) d.dropEl.remove();
			tile.className = tile.className.split(/\s+/).filter((c) => c && c !== "dragging").join(" ");
			try { tile.releasePointerCapture(e.pointerId); } catch (_) { /* ignore */ }
			const t = d.target;
			if (commit && d.active && t) {
				const same = t.origin.col === inst.origin.col && t.origin.row === inst.origin.row;
				if (!same) {
					if (t.valid) {
						await moveInstance(inst, S.page, t.origin, inst.slot);
						return;
					}
					toast("That position is not free.", "error");
				}
			}
			if (S.pendingRender) render();
		};
		tile.addEventListener("pointerup", (e) => finish(e, true));
		tile.addEventListener("pointercancel", (e) => finish(e, false));
		return tile;
	}

	function updateDrop(d, e) {
		const pc = cellAt(e, d.board.getBoundingClientRect(), S.state.grid);
		if (!pc) return;
		const origin = { col: pc.col - d.grab.col, row: pc.row - d.grab.row };
		const valid = placementValid(d.inst, S.page, origin, d.inst.slot);
		d.target = { origin, valid };
		d.dropEl.className = "drop " + (valid ? "valid" : "invalid");
		const pos = place(origin, spanOf(d.inst.slot));
		for (const k of Object.keys(pos)) d.dropEl.style[k] = pos[k];
	}

	function onBoardKey(e) {
		const found = S.selected && findInstance(S.selected);
		if (!found || found.page !== S.page) return;
		const { inst } = found;
		const deltas = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
		if (e.key === "Delete" || e.key === "Backspace") {
			if (e.preventDefault) e.preventDefault();
			removeInstance(inst);
		} else if (deltas[e.key]) {
			if (e.preventDefault) e.preventDefault();
			const [dc, dr] = deltas[e.key];
			const origin = { col: inst.origin.col + dc, row: inst.origin.row + dr };
			if (!placementValid(inst, S.page, origin, inst.slot)) {
				toast("Can't move the tile there.", "info");
				return;
			}
			moveInstance(inst, S.page, origin, inst.slot);
		}
	}

	// ---------------------------------------------------------------- settings panel

	function formatValue(v, prop, inst) {
		if (v === undefined) return inst.defaults_pending ? "(pending)" : "(unset)";
		let s;
		if (typeof v === "string") s = v;
		else if (typeof v === "boolean") s = v ? "on" : "off";
		else s = JSON.stringify(v);
		if (prop.unit && typeof v === "number") s += " " + prop.unit;
		return s.length > 40 ? s.slice(0, 37) + "..." : s;
	}

	function normalizeColor(v) {
		if (typeof v !== "string") return null;
		const m = /^#([0-9a-f]{3}|[0-9a-f]{6})([0-9a-f]{2})?$/i.exec(v.trim());
		if (!m) return null;
		let hex = m[1];
		if (hex.length === 3) hex = hex.split("").map((c) => c + c).join("");
		return "#" + hex.toLowerCase();
	}

	function buildControl(inst, prop, valueEl) {
		const name = prop.name;
		const cur = inst.values[name];
		const focus = "prop:" + name;
		const showValue = (v) => { valueEl.textContent = formatValue(v, prop, inst); };
		const id = "prop-" + name;
		const send = (v, debounce) => { showValue(v); setProp(inst, name, v, debounce); };
		const textInput = (extra) => {
			const input = h("input", { type: "text", id, "data-focus": focus, value: cur === undefined || cur === null ? "" : typeof cur === "string" ? cur : JSON.stringify(cur), ...extra });
			input.addEventListener("input", () => send(input.value, true));
			return h("div", { class: "ctl" }, input);
		};

		switch (prop.kind) {
			case "color": {
				const hex = normalizeColor(cur);
				const input = h("input", { type: "color", id, "data-focus": focus, value: hex || "#000000" });
				input.addEventListener("change", () => send(input.value, false));
				return h("div", { class: "ctl" }, input);
			}
			case "slider": {
				const min = typeof prop.min === "number" ? prop.min : 0;
				const max = typeof prop.max === "number" ? prop.max : 100;
				const step = typeof prop.step === "number" ? prop.step : 1;
				const n = Number(cur);
				const val = Number.isFinite(n) ? n : min;
				const range = h("input", { type: "range", id, "data-focus": focus, min, max, step, value: String(val) });
				const num = h("input", { type: "number", "aria-label": prop.label + " (number)", "data-focus": focus + ":num", min, max, step, value: String(val) });
				const sync = (src, dst) => () => {
					const v = Number(src.value);
					if (!Number.isFinite(v)) return;
					dst.value = String(v);
					send(v, true);
				};
				range.addEventListener("input", sync(range, num));
				num.addEventListener("input", sync(num, range));
				return h("div", { class: "ctl" }, range, num, prop.unit ? h("span", { class: "unit" }, prop.unit) : null);
			}
			case "switch": {
				const input = h("input", { type: "checkbox", id, "data-focus": focus, checked: cur === true });
				input.addEventListener("change", () => send(!!input.checked, false));
				return h("div", { class: "ctl" }, h("label", { class: "switch", htmlFor: id }, input, "Enabled"));
			}
			case "textfield":
				return textInput({});
			case "combobox":
			case "tab-buttons": {
				const opts = prop.options;
				if (!opts || !opts.length) return textInput({});
				// Keep numeric-valued properties numeric when the option keys are numbers.
				const coerce = (key) => (typeof cur === "number" && key.trim() !== "" && Number.isFinite(Number(key)) ? Number(key) : key);
				if (prop.kind === "tab-buttons") {
					return h("div", { class: "ctl seg", role: "group", "aria-label": prop.label }, opts.map((o) => h("button", {
						"aria-pressed": String(cur) === o.key ? "true" : "false",
						"data-key": o.key,
						"data-focus": focus + ":" + o.key,
						onclick: () => send(coerce(o.key), false),
					}, o.label)));
				}
				const known = opts.some((o) => String(cur) === o.key);
				const options = opts.map((o) => h("option", { value: o.key }, o.label));
				if (!known && cur !== undefined) options.push(h("option", { value: String(cur) }, `${formatValue(cur, prop, inst)} (current)`));
				const sel = h("select", { id, "data-focus": focus }, options);
				if (cur !== undefined) sel.value = String(cur);
				sel.addEventListener("change", () => send(coerce(sel.value), false));
				return h("div", { class: "ctl" }, sel);
			}
			default: {
				const area = h("textarea", { id, "data-focus": focus, rows: 4, "aria-label": prop.label, value: cur === undefined ? "" : JSON.stringify(cur, null, 2) });
				area.addEventListener("change", () => {
					let v;
					try { v = JSON.parse(area.value); } catch (_) {
						toast(`${prop.label}: this value must be valid JSON.`, "error");
						return;
					}
					send(v, false);
				});
				return h("div", { class: "ctl" }, area);
			}
		}
	}

	function renderSettings() {
		const el = $("settings");
		const found = S.selected && S.state ? findInstance(S.selected) : null;
		if (!found) {
			fill(el, h("h2", {}, "Settings"), h("p", { class: "empty" }, "Select a tile on the page to edit its settings."));
			return;
		}
		const { inst, page } = found;
		const w = widgetOf(inst);
		const props = w ? w.properties : [];
		const byName = new Map(props.map((p) => [p.name, p]));
		const seen = new Set();

		const row = (prop) => {
			const valueEl = h("span", { class: "val" }, formatValue(inst.values[prop.name], prop, inst));
			const isUser = inst.user_set.includes(prop.name);
			const head = h("div", { class: "prop-head" },
				h("label", { htmlFor: "prop-" + prop.name, title: prop.default_expr ? `Default: ${prop.default_expr}` : null }, prop.label || prop.name),
				valueEl,
				isUser ? h("button", { class: "reset", "data-key": prop.name, "aria-label": `Reset ${prop.label || prop.name}`, title: "Back to the default", onclick: () => unsetProp(inst, prop.name) }, "Reset") : null,
			);
			return h("div", { class: "prop", "data-prop": prop.name }, head, buildControl(inst, prop, valueEl));
		};

		const sections = [];
		for (const g of w ? w.groups : []) {
			const rows = [];
			for (const pn of g.properties) {
				const p = byName.get(pn);
				if (p && !seen.has(pn)) { seen.add(pn); rows.push(row(p)); }
			}
			if (rows.length) sections.push(h("section", { class: "group" }, h("h3", {}, g.title), g.info ? h("p", { class: "group-info" }, g.info) : null, rows));
		}
		const rest = props.filter((p) => !seen.has(p.name));
		if (rest.length) sections.push(h("section", { class: "group" }, sections.length ? h("h3", {}, "Other") : null, rest.map(row)));

		const slotBtns = SLOTS.map((s) => h("button", {
			class: "slot-set",
			"data-slot": s,
			"aria-pressed": inst.slot === s ? "true" : "false",
			"aria-label": `Resize to ${s}`,
			onclick: () => { if (s !== inst.slot) moveInstance(inst, page, inst.origin, s); },
		}, s));

		fill(el, 
			h("div", { class: "s-head" },
				h("div", { class: "s-name" }, widgetName(inst)),
				h("div", { class: "seg", role: "group", "aria-label": "Size" }, slotBtns),
				h("button", { class: "danger delete-btn", onclick: () => removeInstance(inst) }, "Delete"),
			),
			inst.defaults_pending ? h("p", { class: "note" }, "Defaults pending: they are evaluated once the widget loads on the Edge, so some values are not known yet.") : null,
			sections.length ? sections : h("p", { class: "empty" }, "This widget has no settings."),
		);
	}

	// ---------------------------------------------------------------- start

	function start() {
		root.__configurator = {
			reply: (msg) => rpc.reply(msg),
			event: (ev) => { if (ev && ev.type === "changed") refresh(); },
		};
		$("toast").addEventListener("click", hideToast);
		return refresh();
	}

	root.__configuratorApp = {
		start,
		render,
		refresh,
		rpc: (method, params) => rpc.call(method, params),
		getState: () => ({ state: S.state, page: S.page, selected: S.selected }),
		select: (id) => { select(id); },
	};

	if (typeof document !== "undefined" && document.addEventListener) {
		document.addEventListener("DOMContentLoaded", () => {
			if (root.ipc) start();
		});
	}
})();
