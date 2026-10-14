// Tests for the Git Repository widget: the .git file parsers and the plugin wiring.
// Run with: node --test examples/widgets/git/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const MODEL = readFileSync(new URL("./scripts/git-model.js", import.meta.url), "utf8");
const WIDGET = readFileSync(new URL("./scripts/git.js", import.meta.url), "utf8");
const plain = (v) => JSON.parse(JSON.stringify(v));

const model = () => {
	const ctx = {};
	vm.createContext(ctx);
	vm.runInContext(MODEL, ctx);
	return ctx.GitModel;
};

const A = "a".repeat(40);
const B = "b".repeat(40);
const C = "c".repeat(40);
const D = "d".repeat(40);
const who = "Ada Lovelace <ada@example.com>";
const reflog = (...lines) => lines.map(([o, n, t, msg]) => `${o} ${n} ${who} ${t} -0600\t${msg}`).join("\n") + "\n";

const CONFIG = `[core]
	repositoryformatversion = 0
[remote "origin"]
	url = git@github.com:ada/engine.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
	remote = origin
	merge = refs/heads/main
[branch "feature/x"] ; tracking a differently named branch
	remote = origin
	merge = refs/heads/x
`;

test("HEAD, refs and packed-refs", () => {
	const M = model();
	assert.deepEqual(plain(M.parseHead("ref: refs/heads/feature/x\n")), { branch: "feature/x" });
	assert.deepEqual(plain(M.parseHead(`${A}\n`)), { detached: A });
	assert.equal(M.parseHead("garbage"), null);
	assert.equal(M.parseRefFile(`${B}\n`), B);
	assert.equal(M.parseRefFile("ref: refs/heads/main"), null);
	const packed = M.parsePackedRefs(`# pack-refs with: peeled fully-peeled sorted\n${A} refs/heads/main\n${B} refs/remotes/origin/main\n^${C}\n`);
	assert.equal(packed.get("refs/heads/main"), A);
	assert.equal(packed.get("refs/remotes/origin/main"), B);
	assert.equal(packed.size, 2);
});

test("config: branches, remotes, comments and quoted names", () => {
	const M = model();
	const c = M.parseConfig(CONFIG);
	assert.equal(c.remotes.origin.url, "git@github.com:ada/engine.git");
	assert.deepEqual(plain(c.branches["feature/x"]), { remote: "origin", merge: "refs/heads/x" });
	assert.equal(M.upstreamRef(c, "main"), "refs/remotes/origin/main");
	assert.equal(M.upstreamRef(c, "feature/x"), "refs/remotes/origin/x");
	assert.equal(M.upstreamRef(c, "nope"), null);
	assert.equal(M.upstreamRef(M.parseConfig('[branch "l"]\n\tremote = .\n\tmerge = refs/heads/main'), "l"), null);
});

test("reflog lines and messages", () => {
	const M = model();
	const log = M.parseReflog(
		reflog([A, B, 100, "commit: Add parser: with a colon"], [B, C, 200, "checkout: moving from main to dev"], [C, D, 300, "commit (amend): Fix typo"]),
	);
	assert.equal(log.length, 3);
	assert.deepEqual(plain(log[0]), { old: C, new: D, name: "Ada Lovelace", email: "ada@example.com", time: 300, message: "commit (amend): Fix typo" });
	const cases = {
		"commit: Add parser: with a colon": ["commit", "Add parser: with a colon"],
		"commit (initial): First": ["commit", "First"],
		"commit (merge): Merge branch 'x'": ["merge", "Merge branch 'x'"],
		"commit (amend): Fix typo": ["amend", "Fix typo"],
		"checkout: moving from main to dev": ["checkout", "main → dev"],
		"merge feature: Fast-forward": ["merge", "Fast-forward"],
		"pull: Fast-forward": ["pull", "Fast-forward"],
		"pull --rebase origin main (finish): returning to refs/heads/main": ["pull", "returning to refs/heads/main"],
		"rebase (finish): returning to refs/heads/dev": ["rebase", "finish: returning to refs/heads/dev"],
		"reset: moving to HEAD~1": ["reset", "HEAD~1"],
		"clone: from https://github.com/ada/engine.git": ["clone", "from https://github.com/ada/engine.git"],
		"weird": ["other", "weird"],
	};
	for (const [message, [kind, text]] of Object.entries(cases)) {
		assert.deepEqual(plain(M.describe(message)), { kind, text }, message);
	}
});

test("remote labels and ages", () => {
	const M = model();
	assert.equal(M.remoteLabel("git@github.com:ada/engine.git"), "github.com/ada/engine");
	assert.equal(M.remoteLabel("https://user@gitlab.com:8443/g/sub/repo.git/"), "gitlab.com/g/sub/repo");
	assert.equal(M.remoteLabel("/srv/git/repo"), "/srv/git/repo");
	const now = 1_000_000 * 1000;
	assert.equal(M.age(1_000_000 - 30, now), "just now");
	assert.equal(M.age(1_000_000 - 150, now), "2m ago");
	assert.equal(M.age(1_000_000 - 3 * 3600, now), "3h ago");
	assert.equal(M.age(1_000_000 - 9 * 86400, now), "1w ago");
	assert.equal(M.age(1_000_000 + 99, now), "just now");
});

test("summary: in sync, ahead by reflog, diverged, detached", () => {
	const M = model();
	const base = { HEAD: "ref: refs/heads/main\n", config: CONFIG, "logs/HEAD": reflog([A, B, 100, "commit: One"]) };
	const same = M.summarize({ ...base, "refs/heads/main": `${B}\n`, "packed-refs": `${B} refs/remotes/origin/main\n` });
	assert.equal(same.sync, "same");
	assert.equal(same.branch, "main");
	assert.equal(same.remote, "github.com/ada/engine");
	assert.equal(same.upstream.name, "origin/main");
	assert.equal(same.activity[0].text, "One");

	const branchLog = reflog([A, B, 100, "pull: Fast-forward"], [B, C, 200, "commit: Two"], [C, D, 300, "commit: Three"]);
	const ahead = M.summarize({ ...base, "refs/heads/main": D, "refs/remotes/origin/main": B, "logs/refs/heads/main": branchLog });
	assert.equal(ahead.sync, "ahead");
	assert.equal(ahead.ahead, 2);

	const rewritten = reflog([A, B, 100, "pull: Fast-forward"], [B, D, 300, "reset: moving to HEAD~1"]);
	const differs = M.summarize({ ...base, "refs/heads/main": D, "refs/remotes/origin/main": C, "logs/refs/heads/main": rewritten });
	assert.equal(differs.sync, "differs");

	const detached = M.summarize({ ...base, HEAD: `${C}\n` });
	assert.equal(detached.branch, null);
	assert.equal(detached.sha, C);
	assert.equal(detached.sync, "none");

	assert.equal(M.summarize({}).ok, false);
});

test("files to watch follow HEAD and the branch's upstream", () => {
	const M = model();
	assert.deepEqual(plain(M.filesToWatch({})).sort(), ["HEAD", "config", "logs/HEAD", "packed-refs"]);
	const want = M.filesToWatch({ HEAD: "ref: refs/heads/feature/x", config: CONFIG });
	assert.ok(want.includes("refs/heads/feature/x"));
	assert.ok(want.includes("logs/refs/heads/feature/x"));
	assert.ok(want.includes("refs/remotes/origin/x"));
});

// ── Widget wiring with a fake DOM and a fake File System plugin ──

function widgetEnv(repoPath) {
	const elements = new Map();
	const el = (id) => {
		if (!elements.has(id)) {
			const classes = new Set();
			elements.set(id, {
				id,
				hidden: false,
				textContent: "",
				className: "",
				children: [],
				classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)) },
				replaceChildren(...nodes) {
					this.children = nodes;
				},
				append(...nodes) {
					this.children.push(...nodes);
				},
			});
		}
		return elements.get(id);
	};
	const calls = [];
	const handlers = {};
	const signal = (name) => ({ connect: (fn) => (handlers[name] = fn), disconnect() {} });
	const ctx = {
		repoPath,
		document: {
			getElementById: el,
			createElement: () => el(`new-${Math.random()}`),
			documentElement: { style: { setProperty() {} } },
		},
		setTimeout: () => 0,
		clearTimeout() {},
		setInterval() {},
		Date,
		plugins: {
			Filesystemprovider: {
				watchPath: (p) => calls.push(["watch", p]),
				unwatchPath: (p) => calls.push(["unwatch", p]),
				clearPaths: () => calls.push(["clear"]),
				fileChanged: signal("fileChanged"),
				fileRemoved: signal("fileRemoved"),
			},
		},
	};
	ctx.window = ctx;
	vm.createContext(ctx);
	vm.runInContext(MODEL, ctx);
	vm.runInContext(WIDGET, ctx);
	return { ctx, calls, handlers, el };
}

test("repoPath forms map to watchPath arguments", () => {
	const { ctx } = widgetEnv("");
	const W = ctx.GitWidget;
	assert.equal(W.repoBase("~/projects/engine/"), "{Home}/projects/engine/.git/");
	assert.equal(W.repoBase("{Home}/engine/.git"), "{Home}/engine/.git/");
	assert.equal(W.repoBase("/srv/repo"), "/srv/repo/.git/");
	assert.equal(W.repoBase("  "), "");
	const base = "{Home}/engine/.git/";
	assert.equal(W.gitPath("/Users/a/engine/.git/refs/heads/main", base), "refs/heads/main");
	assert.equal(W.gitPath("/Users/a/other/.git/HEAD", base), null);
	assert.equal(W.gitPath("/Users/a/elsewhere", base), null);
	assert.equal(W.gitPath("/srv/repo/.git/HEAD", "/srv/repo/.git/"), "HEAD");
	assert.equal(W.gitPath("/srv/repo/.git/HEAD", ""), null);
});

test("widget watches the repository, follows branch changes and renders", () => {
	const { ctx, calls, handlers, el } = widgetEnv("~/engine");
	ctx.icueEvents.onICUEInitialized();
	ctx.icueEvents.onDataUpdated(); // same repo: no restart
	assert.deepEqual(
		plain(calls.map((c) => c[1])).sort(),
		["{Home}/engine/.git/HEAD", "{Home}/engine/.git/config", "{Home}/engine/.git/logs/HEAD", "{Home}/engine/.git/packed-refs"],
	);
	calls.length = 0;
	const root = "/Users/ada/engine/.git/";
	handlers.fileChanged(`${root}config`, CONFIG);
	handlers.fileChanged(`${root}HEAD`, "ref: refs/heads/main\n");
	assert.ok(calls.some(([k, p]) => k === "watch" && p === "{Home}/engine/.git/refs/heads/main"));
	assert.ok(calls.some(([k, p]) => k === "watch" && p === "{Home}/engine/.git/refs/remotes/origin/main"));
	handlers.fileChanged(`${root}refs/heads/main`, `${B}\n`);
	handlers.fileChanged(`${root}refs/remotes/origin/main`, `${B}\n`);
	handlers.fileChanged(`${root}logs/HEAD`, reflog([A, B, Math.floor(Date.now() / 1000) - 120, "commit: Ship it"]));
	assert.equal(el("repo").hidden, false);
	assert.equal(el("name").textContent, "engine");
	assert.equal(el("branch").textContent, "main");
	assert.equal(el("sha").textContent, B.slice(0, 7));
	assert.equal(el("sync").textContent, "✓ origin/main");
	assert.equal(el("last").textContent, "Ship it");
	assert.equal(el("last-age").textContent, "2m ago");

	calls.length = 0;
	handlers.fileChanged(`${root}HEAD`, "ref: refs/heads/feature/x\n");
	assert.ok(calls.some(([k, p]) => k === "unwatch" && p === "{Home}/engine/.git/refs/heads/main"));
	assert.ok(calls.some(([k, p]) => k === "watch" && p === "{Home}/engine/.git/refs/heads/feature/x"));
	assert.equal(el("branch").textContent, "feature/x");

	handlers.fileChanged("/Users/ada/other/.git/HEAD", "ref: refs/heads/zzz\n"); // not ours: ignored
	assert.equal(el("branch").textContent, "feature/x");
});

test("an empty repoPath asks for one instead of watching", () => {
	const { ctx, calls, el } = widgetEnv("");
	ctx.icueEvents.onICUEInitialized();
	assert.deepEqual(calls, []);
	assert.equal(el("repo").hidden, true);
	assert.match(el("message-title").textContent, /Set repoPath/);
});
