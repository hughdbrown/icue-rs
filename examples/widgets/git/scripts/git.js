// Git Repository widget: branch, sync state and recent activity of one local repository, read
// live through the File System plugin (widgetbuilder.filesystemprovider:FileSystem:1.0).
//
// Settings: repoPath (the repository's working folder; "~/…" and "{Home}/…" both work).
(function () {
	"use strict";

	const M = window.GitModel;
	const $ = (id) => document.getElementById(id);
	const AGE_REFRESH_MS = 30 * 1000;
	const MISSING_AFTER_MS = 3000;

	let base = ""; // "{Home}/projects/x/.git/" form, as passed to watchPath
	let files = {}; // path under .git/ → content
	let watching = new Set(); // paths under .git/ currently watched
	let missingTimer = 0;

	const fs = () => window.plugins && window.plugins.Filesystemprovider;

	/** "~/x" and "{Home}/x" → "{Home}/x"; absolute paths unchanged; trailing slashes dropped. */
	function repoBase(raw) {
		let p = String(raw || "").trim().replace(/\/+$/, "");
		if (!p) return "";
		if (p === "~" || p.startsWith("~/")) p = `{Home}${p.slice(1)}`;
		if (p.endsWith("/.git")) p = p.slice(0, -5);
		return `${p}/.git/`;
	}

	/**
	 * Absolute path from a signal → path under this repository's .git/, or null for other files.
	 * Signals carry absolute paths while `base` may start with a macro, so match its tail.
	 */
	function gitPath(path, repo = base) {
		const p = String(path);
		const tail = repo.replace(/^\{\w+\}/, "");
		const at = p.lastIndexOf("/.git/");
		if (at < 0 || !tail || !p.slice(0, at + 6).endsWith(tail)) return null;
		return p.slice(at + 6);
	}

	function setFiles(next) {
		files = next;
		syncWatches();
		render();
	}

	/** Watch exactly the files the current HEAD and config need. */
	function syncWatches() {
		const plugin = fs();
		if (!plugin || !base) return;
		const want = new Set(M.filesToWatch(files));
		for (const rel of watching) {
			if (!want.has(rel)) {
				plugin.unwatchPath(base + rel);
				if (rel in files) {
					const { [rel]: _, ...rest } = files;
					files = rest;
				}
			}
		}
		for (const rel of want) if (!watching.has(rel)) plugin.watchPath(base + rel);
		watching = want;
	}

	function onFileChanged(path, content) {
		const rel = gitPath(path);
		if (rel === null || !watching.has(rel)) return;
		clearTimeout(missingTimer);
		setFiles({ ...files, [rel]: content });
	}

	function onFileRemoved(path) {
		const rel = gitPath(path);
		if (rel === null || !(rel in files)) return;
		const { [rel]: _, ...rest } = files;
		setFiles(rest);
	}

	function start() {
		const plugin = fs();
		const wanted = repoBase(typeof repoPath === "undefined" ? "" : repoPath);
		if (plugin && watching.size > 0) plugin.clearPaths();
		base = wanted;
		files = {};
		watching = new Set();
		clearTimeout(missingTimer);
		if (!plugin) return showMessage("The File System plugin is not available.");
		if (!base) return showMessage("Set repoPath to a git repository folder.", "edgectl layout set <instance> repoPath ~/path/to/repo");
		showMessage("Reading repository…");
		syncWatches();
		missingTimer = setTimeout(() => {
			if (!files.HEAD) showMessage("No git repository here.", `${base.replace(/\/\.git\/$/, "")} (worktrees and submodules are not supported yet)`);
		}, MISSING_AFTER_MS);
	}

	function showMessage(title, detail = "") {
		$("repo").hidden = true;
		$("message").hidden = false;
		$("message-title").textContent = title;
		$("message-detail").textContent = detail;
	}

	function render() {
		const view = M.summarize(files);
		if (!view.ok) return;
		$("message").hidden = true;
		$("repo").hidden = false;
		const folder = base.replace(/\/\.git\/$/, "").split("/").pop() || base;
		$("name").textContent = folder;
		$("remote").textContent = view.remote;
		$("branch").textContent = view.branch || "detached HEAD";
		$("branch").classList.toggle("detached", !view.branch);
		$("sha").textContent = view.sha ? view.sha.slice(0, 7) : "";
		const sync = $("sync");
		sync.className = `badge ${view.sync}`;
		sync.hidden = view.sync === "none";
		const upstream = view.upstream ? view.upstream.name : "";
		sync.textContent = {
			same: `✓ ${upstream}`,
			ahead: `↑${view.ahead} ${upstream}`,
			differs: `≠ ${upstream}`,
			none: "",
		}[view.sync];
		renderActivity(view.activity);
	}

	function renderActivity(activity) {
		const list = $("activity");
		const now = Date.now();
		const latest = activity[0];
		$("last").textContent = latest ? `${latest.text}` : "No activity recorded";
		$("last-age").textContent = latest ? M.age(latest.time, now) : "";
		const rows = activity.slice(0, 40).map((e) => {
			const li = document.createElement("li");
			li.className = `kind-${e.kind}`;
			const kind = document.createElement("span");
			kind.className = "kind";
			kind.textContent = e.kind;
			const text = document.createElement("span");
			text.className = "text";
			text.textContent = e.text;
			const who = document.createElement("span");
			who.className = "who";
			who.textContent = e.name;
			const when = document.createElement("span");
			when.className = "when";
			when.textContent = M.age(e.time, now);
			li.append(kind, text, who, when);
			return li;
		});
		list.replaceChildren(...rows);
	}

	function applyColors() {
		const root = document.documentElement.style;
		if (typeof textColor !== "undefined") root.setProperty("--fg", textColor);
		if (typeof accentColor !== "undefined") root.setProperty("--accent", accentColor);
		if (typeof backgroundColor !== "undefined") root.setProperty("--bg", backgroundColor);
	}

	function connect() {
		const plugin = fs();
		if (!plugin || connect.done) return;
		connect.done = true;
		plugin.fileChanged.connect(onFileChanged);
		plugin.fileRemoved.connect(onFileRemoved);
	}

	// Boot calls onICUEInitialized then onDataUpdated; restart only when the repository changes.
	let started = false;
	function onReady() {
		applyColors();
		connect();
		const wanted = repoBase(typeof repoPath === "undefined" ? "" : repoPath);
		if (started && wanted === base) return;
		started = true;
		start();
	}

	window.pluginFilesystemproviderEvents = { onInitialized: connect };
	window.icueEvents = { onICUEInitialized: onReady, onDataUpdated: onReady };
	setInterval(() => {
		if (!$("repo").hidden) render();
	}, AGE_REFRESH_MS);

	window.GitWidget = { repoBase, gitPath };
})();
