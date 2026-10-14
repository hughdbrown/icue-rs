// Git widget (iCUE Widget API 1.x).
//
// Reads repositories through EdgeHost's same-origin git data source (/__edgehost/git/...; see
// docs/data-sources.md). Those routes do not exist in iCUE, so failures show an explanatory message.
//
// State kept in localStorage under uniqueId: the selected source ("terminal" or a repo path).

icueEvents = {
	onICUEInitialized: init,
	onDataUpdated: onSettingsChanged,
};

const TERMINAL = "terminal";
const IDLE_MS = 60 * 1000;
const SMALL_COMMITS = 4;
const MSG_PERMISSION = "This folder is not permitted — add it to the widget's folder permissions";
const MSG_NO_SOURCE = "Needs EdgeHost's git data source (not available in iCUE)";
const MSG_HOOK = "Terminal not tracked: install the shell hook (zsh: add edgehost_cwd to ~/.zshrc); see docs/data-sources.md";

const $ = (id) => document.getElementById(id);
const setting = (name, fallback) => (typeof window[name] === "undefined" || window[name] === null ? fallback : window[name]);

// ---------- pure helpers ----------

function parseRepoList(text) {
	const out = [];
	for (const part of String(text == null ? "" : text).split(/[,\n\r]+/)) {
		const p = part.trim();
		if (p && !out.includes(p)) out.push(p);
	}
	return out;
}

function basename(path) {
	const parts = String(path == null ? "" : path)
		.replace(/[\\/]+$/, "")
		.split(/[\\/]/);
	return parts[parts.length - 1] || String(path == null ? "" : path);
}

function pad2(n) {
	return String(n).padStart(2, "0");
}

function relativeTime(seconds, nowSeconds) {
	const diff = nowSeconds - seconds;
	if (!Number.isFinite(diff) || diff < 60) return "now";
	if (diff < 3600) return `${Math.floor(diff / 60)} m`;
	if (diff < 86400) return `${Math.floor(diff / 3600)} h`;
	if (diff < 30 * 86400) return `${Math.floor(diff / 86400)} d`;
	const d = new Date(seconds * 1000);
	return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function formatAheadBehind(ahead, behind, upstream) {
	if (!upstream) return "";
	const parts = [];
	if (ahead > 0) parts.push(`↑${ahead}`);
	if (behind > 0) parts.push(`↓${behind}`);
	return parts.length ? parts.join(" ") : "in sync";
}

function describeError(status, text) {
	const t = String(text == null ? "" : text).trim();
	if (status === 403) return MSG_PERMISSION;
	if (status === 502) return /^git error/i.test(t) ? t : `git error: ${t || "command failed"}`;
	if (!status) return MSG_NO_SOURCE;
	if (status === 404) return t && !t.startsWith("<") ? t : MSG_NO_SOURCE;
	return t || `Request failed (${status})`;
}

function cleanRefs(refs) {
	const out = [];
	if (!Array.isArray(refs)) return out;
	for (const r of refs) {
		const c = String(r == null ? "" : r)
			.replace(/^HEAD -> /, "")
			.trim();
		if (c && !out.includes(c)) out.push(c);
	}
	return out;
}

// ---------- state ----------

let initialized = false;
let selection = null; // TERMINAL or a repo path; null = default
let terminalInfo = null; // last /terminal response
let terminalErr = null;
const cache = {}; // repo path -> { status, log, err, stale }
let detail = null; // { repo, hash, subject, short, data, err, loading }
let busy = false;
let pending = false;
let timer = null;
let idleTimer = null;

function loadStored() {
	try {
		return JSON.parse(localStorage.getItem(uniqueId) || "null");
	} catch (_) {
		return null;
	}
}

function saveStored() {
	try {
		localStorage.setItem(uniqueId, JSON.stringify({ selection }));
	} catch (_) {}
}

function repoList() {
	return parseRepoList(setting("repos", ""));
}

function followTerminal() {
	return setting("followTerminal", true) !== false;
}

function commitLimit() {
	const n = Math.round(Number(setting("commitCount", 20)));
	return Math.min(50, Math.max(5, Number.isFinite(n) ? n : 20));
}

function effectiveSelection() {
	const repos = repoList();
	const follow = followTerminal();
	if (selection === TERMINAL && follow) return TERMINAL;
	if (selection && selection !== TERMINAL && repos.includes(selection)) return selection;
	// Nothing chosen (or the choice is gone): follow the terminal when it is in a repo, otherwise
	// show the first configured repo rather than an empty "no repo" view.
	if (follow && terminalInfo && terminalInfo.repo) return TERMINAL;
	if (repos.length) return repos[0];
	return follow ? TERMINAL : null;
}

function currentRepo() {
	const sel = effectiveSelection();
	if (sel === TERMINAL) return terminalInfo && terminalInfo.repo ? terminalInfo.repo : null;
	return sel;
}

// ---------- layout / theme ----------

function applyTheme() {
	const s = document.documentElement.style;
	s.setProperty("--fg", setting("textColor", "#EEF0F4"));
	s.setProperty("--accent", setting("accentColor", "#F5A623"));
	s.setProperty("--bg", setting("backgroundColor", "#14161B"));
}

function layoutMode() {
	const w = Number(window.innerWidth) || 840;
	const h = Number(window.innerHeight) || 696;
	return { wide: w >= 1200, xl: w >= 2000, small: h < 500 };
}

function applyLayout() {
	const m = layoutMode();
	const root = $("root");
	root.className = [m.wide ? "wide" : "narrow", m.xl ? "xl" : "", m.small ? "small" : "", detail && !m.wide ? "detail-open" : ""]
		.filter(Boolean)
		.join(" ");
	return m;
}

// ---------- fetching ----------

async function getJSON(url) {
	let r;
	try {
		r = await fetch(url);
	} catch (_) {
		throw { status: 0, text: "" };
	}
	if (!r.ok) {
		let text = "";
		try {
			text = await r.text();
		} catch (_) {}
		throw { status: r.status, text };
	}
	try {
		return await r.json();
	} catch (_) {
		throw { status: 0, text: "" };
	}
}

const errText = (e) => describeError(e && e.status, e && e.text);

async function refreshOnce() {
	if (followTerminal()) {
		try {
			terminalInfo = await getJSON("/__edgehost/git/terminal");
			terminalErr = null;
		} catch (e) {
			terminalErr = errText(e);
		}
	}
	const repo = currentRepo();
	if (!repo) {
		render();
		return;
	}
	const q = encodeURIComponent(repo);
	try {
		const [status, log] = await Promise.all([getJSON(`/__edgehost/git/status?repo=${q}`), getJSON(`/__edgehost/git/log?repo=${q}&n=${commitLimit()}`)]);
		cache[repo] = { status, log, err: null, stale: false };
	} catch (e) {
		const c = cache[repo] || (cache[repo] = { status: null, log: null });
		c.err = errText(e);
		c.stale = !!c.status;
	}
	render();
}

async function refresh() {
	if (busy) {
		pending = true;
		return;
	}
	busy = true;
	try {
		do {
			pending = false;
			await refreshOnce();
		} while (pending);
	} finally {
		busy = false;
	}
}

async function openCommit(commit) {
	const repo = currentRepo();
	if (!repo) return;
	detail = { repo, hash: commit.hash, short: commit.short, subject: commit.subject, author: commit.author, time: commit.time, data: null, err: null, loading: true };
	touch();
	render();
	const mine = detail;
	try {
		mine.data = await getJSON(`/__edgehost/git/show?repo=${encodeURIComponent(repo)}&rev=${encodeURIComponent(commit.hash)}`);
	} catch (e) {
		mine.err = errText(e);
	}
	mine.loading = false;
	if (detail === mine) render();
}

function closeDetail() {
	detail = null;
	clearTimeout(idleTimer);
	idleTimer = null;
	render();
}

// ---------- rendering ----------

function el(tag, className, text) {
	const e = document.createElement(tag);
	if (className) e.className = className;
	if (text != null) e.textContent = text;
	return e;
}

function button(className, text, onTap) {
	const b = el("button", className, text);
	b.type = "button";
	b.addEventListener("click", () => {
		touch();
		onTap();
	});
	return b;
}

function selectSource(sel) {
	selection = sel;
	detail = null;
	saveStored();
	render();
	refresh();
}

function renderChips() {
	const nav = $("chips");
	const sel = effectiveSelection();
	const chips = [];
	if (followTerminal()) {
		const label = terminalInfo && terminalInfo.repo ? `Terminal: ${basename(terminalInfo.repo)}` : "Terminal: no repo";
		chips.push(chip(label, sel === TERMINAL, () => selectSource(TERMINAL)));
	}
	for (const r of repoList()) chips.push(chip(basename(r), sel === r, () => selectSource(r)));
	nav.replaceChildren(...chips);
}

function chip(text, pressed, onTap) {
	const b = button("chip", text, onTap);
	b.setAttribute("aria-pressed", pressed ? "true" : "false");
	return b;
}

function badge(text, cls) {
	return el("span", `badge${cls ? ` ${cls}` : ""}`, text);
}

function renderHeader(repo, c) {
	const h = $("header");
	if (!repo || !c || !c.status) {
		h.replaceChildren(el("span", "repo-name", repo ? basename(repo) : effectiveSelection() === TERMINAL ? "Terminal" : "Git"));
		return;
	}
	const s = c.status;
	const kids = [el("span", "repo-name", s.name || basename(repo))];
	if (s.detached || !s.branch) kids.push(el("span", "branch mono", `detached at ${(s.head && s.head.short) || "?"}`));
	else kids.push(el("span", "branch mono", s.branch));
	const ab = formatAheadBehind(s.ahead, s.behind, s.upstream);
	if (ab) kids.push(el("span", "ab", ab));
	const badges = el("span", "badges");
	const counts = [
		[s.staged, "staged", ""],
		[s.modified, "modified", ""],
		[s.untracked, "untracked", ""],
		[s.conflicted, "conflicted", "conflict"],
	];
	let any = false;
	for (const [n, label, cls] of counts) {
		if (n > 0) {
			any = true;
			badges.append(badge(`${n} ${label}`, cls));
		}
	}
	if (!any) badges.append(badge("clean", "clean"));
	kids.push(badges);
	if (c.stale) kids.push(el("span", "stale", "stale"));
	h.replaceChildren(...kids);
}

function commitRow(commit, now, showAuthor) {
	const row = el("button", "commit");
	row.type = "button";
	row.setAttribute("aria-pressed", detail && detail.hash === commit.hash ? "true" : "false");
	const main = el("span", "main");
	main.append(el("span", "subject", commit.subject || ""));
	const refs = cleanRefs(commit.refs);
	if (refs.length) {
		const box = el("span", "refs");
		for (const r of refs) box.append(el("span", "tag", r));
		main.append(box);
	}
	row.append(el("span", "hash mono", commit.short || String(commit.hash || "").slice(0, 7)), main);
	if (showAuthor) row.append(el("span", "author", commit.author || ""));
	row.append(el("span", "when", relativeTime(commit.time, now)));
	row.addEventListener("click", () => {
		touch();
		openCommit(commit);
	});
	return row;
}

function message(text, cls) {
	return el("div", `message${cls ? ` ${cls}` : ""}`, text);
}

function terminalProblem() {
	if (terminalErr) return terminalErr;
	if (!terminalInfo) return "Waiting for terminal…";
	if (terminalInfo.cwd == null) return MSG_HOOK;
	if (terminalInfo.error === "not permitted") return MSG_PERMISSION;
	return `Not in a git repository: ${terminalInfo.cwd}`;
}

function renderList(repo, c, mode) {
	const list = $("commits");
	const banner = $("banner");
	banner.hidden = true;
	if (!repo) {
		const sel = effectiveSelection();
		let text = "Add a repository folder in the widget settings.";
		if (sel === TERMINAL) text = terminalProblem();
		list.replaceChildren(message(text, sel === TERMINAL && terminalInfo && terminalInfo.cwd == null ? "info" : ""));
		return;
	}
	if (c && c.err) {
		banner.textContent = c.err;
		banner.hidden = false;
	}
	if (!c || !c.log) {
		if (!c || !c.err) list.replaceChildren(message("Loading…", "info"));
		else list.replaceChildren();
		return;
	}
	const now = Math.floor(Date.now() / 1000);
	const showAuthor = setting("showAuthor", true) !== false;
	let commits = Array.isArray(c.log.commits) ? c.log.commits : [];
	commits = commits.slice(0, mode.small ? SMALL_COMMITS : commitLimit());
	if (!commits.length) {
		list.replaceChildren(message("No commits yet.", "info"));
		return;
	}
	list.replaceChildren(...commits.map((cm) => commitRow(cm, now, showAuthor)));
}

function formatDateTime(seconds) {
	const d = new Date(seconds * 1000);
	return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function fileRow(f) {
	const row = el("div", "file");
	row.append(el("span", "path mono", f.path));
	if (f.additions == null || f.deletions == null) {
		row.append(el("span", "bin", "binary"));
	} else {
		row.append(el("span", "add mono", `+${f.additions}`), el("span", "del mono", `−${f.deletions}`));
	}
	return row;
}

function renderDetail(mode) {
	const d = $("detail");
	if (!detail) {
		if (mode.wide) {
			d.hidden = false;
			d.className = "placeholder";
			d.replaceChildren(el("div", "muted", "Select a commit to see its files"));
		} else {
			d.hidden = true;
			d.replaceChildren();
		}
		return;
	}
	d.hidden = false;
	d.className = "";
	const src = detail.data || detail;
	const top = el("div", "detail-top");
	top.append(button("back", "‹ Back", closeDetail), el("span", "hash mono", detail.short || ""));
	const kids = [top, el("div", "detail-subject", src.subject || detail.subject || "")];
	kids.push(el("div", "detail-meta mono", detail.hash));
	const meta = [src.author || detail.author, src.time != null ? formatDateTime(src.time) : ""].filter(Boolean).join(" · ");
	if (meta) kids.push(el("div", "detail-meta", meta));
	if (detail.loading) kids.push(el("div", "muted", "Loading…"));
	if (detail.err) kids.push(message(detail.err));
	if (detail.data) {
		const body = (detail.data.body || "").trim();
		if (body) kids.push(el("div", "detail-body", body));
		const files = Array.isArray(detail.data.files) ? detail.data.files : [];
		const box = el("div", "files");
		if (!files.length) box.append(el("div", "muted", "No file changes"));
		for (const f of files) box.append(fileRow(f));
		kids.push(box);
	}
	d.replaceChildren(...kids);
}

function render() {
	if (!initialized) return;
	const mode = applyLayout();
	const repo = currentRepo();
	const c = repo ? cache[repo] : null;
	renderChips();
	renderHeader(repo, c);
	renderList(repo, c, mode);
	renderDetail(mode);
}

// ---------- lifecycle ----------

function touch() {
	clearTimeout(idleTimer);
	idleTimer = detail ? setTimeout(closeDetail, IDLE_MS) : null;
}

function isVisible() {
	return typeof document.visibilityState === "undefined" || document.visibilityState === "visible";
}

function restartTimer() {
	clearInterval(timer);
	const secs = Math.min(120, Math.max(5, Number(setting("refreshSeconds", 15)) || 15));
	timer = setInterval(() => {
		if (isVisible()) refresh();
	}, secs * 1000);
}

function init() {
	if (initialized) return;
	initialized = true;
	const stored = loadStored();
	if (stored && typeof stored.selection === "string") selection = stored.selection;
	applyTheme();
	if (typeof addEventListener === "function") addEventListener("resize", () => render());
	document.addEventListener("visibilitychange", () => {
		if (isVisible()) refresh();
	});
	// Re-arm the 60 s idle timer after any tap while a commit is open.
	$("root").addEventListener("click", () => {
		if (detail) touch();
	});
	render();
	refresh();
	restartTimer();
}

function onSettingsChanged() {
	if (!initialized) return;
	applyTheme();
	restartTimer();
	render();
	refresh();
}

if (typeof iCUE_initialized !== "undefined" && iCUE_initialized) {
	init();
}
