// Git repository model built from the text files under .git/ (no git binary, no object store).
//
// Inputs are the contents of .git/HEAD, config, packed-refs, refs/…, and the reflogs
// logs/HEAD and logs/refs/heads/<branch>. Everything here is pure, so it runs under node --test.
(function (root) {
	"use strict";

	const SHA = /^[0-9a-f]{40}([0-9a-f]{24})?$/;

	/** `ref: refs/heads/main` → { branch: "main" }; a bare sha → { detached: sha }. */
	function parseHead(text) {
		const line = String(text || "").trim();
		const ref = /^ref:\s*refs\/heads\/(.+)$/.exec(line);
		if (ref) return { branch: ref[1] };
		if (SHA.test(line)) return { detached: line };
		return null;
	}

	/** packed-refs → Map(refName → sha). Peeled lines (`^sha`) are skipped. */
	function parsePackedRefs(text) {
		const refs = new Map();
		for (const line of String(text || "").split("\n")) {
			const m = /^([0-9a-f]{40,64}) (refs\/\S+)$/.exec(line.trim());
			if (m) refs.set(m[2], m[1]);
		}
		return refs;
	}

	/** A loose ref file holds one sha (or a symbolic `ref:`); null otherwise. */
	function parseRefFile(text) {
		const line = String(text || "").trim();
		return SHA.test(line) ? line : null;
	}

	/**
	 * The parts of .git/config the widget uses:
	 * { branches: { name: { remote, merge } }, remotes: { name: { url } } }.
	 */
	function parseConfig(text) {
		const out = { branches: {}, remotes: {} };
		let section = null;
		for (const raw of String(text || "").split("\n")) {
			const line = raw.replace(/[#;].*$/, "").trim();
			if (!line) continue;
			const header = /^\[\s*(\w+)(?:\s+"((?:[^"\\]|\\.)*)")?\s*\]$/.exec(line);
			if (header) {
				const kind = header[1].toLowerCase();
				const name = header[2] && header[2].replace(/\\(.)/g, "$1");
				section = null;
				if (kind === "branch" && name) section = out.branches[name] = out.branches[name] || {};
				if (kind === "remote" && name) section = out.remotes[name] = out.remotes[name] || {};
				continue;
			}
			const kv = /^([\w-]+)\s*=\s*(.*)$/.exec(line);
			if (section && kv) section[kv[1].toLowerCase()] = kv[2].replace(/^"(.*)"$/, "$1");
		}
		return out;
	}

	/** Reflog lines, newest first: { old, new, name, email, time (s), message }. */
	function parseReflog(text) {
		const entries = [];
		for (const line of String(text || "").split("\n")) {
			const m = /^([0-9a-f]{40,64}) ([0-9a-f]{40,64}) (.*?) <([^>]*)> (\d+) [+-]\d{4}\t?(.*)$/.exec(line);
			if (m) entries.push({ old: m[1], new: m[2], name: m[3], email: m[4], time: Number(m[5]), message: m[6] });
		}
		return entries.reverse();
	}

	/**
	 * Split a reflog message into a kind and text:
	 * "commit (amend): Fix x" → { kind: "amend", text: "Fix x" };
	 * "checkout: moving from main to dev" → { kind: "checkout", text: "main → dev" }.
	 */
	function describe(message) {
		const m = /^([\w-]+)(?: --?[\w-]+)*(?: \(([^)]*)\))?(?: [^:]*)?: (.*)$/.exec(String(message || ""));
		if (!m) return { kind: "other", text: String(message || "") };
		const [, verb, detail, rest] = m;
		if (verb === "checkout") {
			const move = /^moving from (.+) to (.+)$/.exec(rest);
			return { kind: "checkout", text: move ? `${move[1]} → ${move[2]}` : rest };
		}
		if (verb === "commit") {
			const kind = { amend: "amend", merge: "merge", initial: "commit" }[detail] || "commit";
			return { kind, text: rest };
		}
		if (verb === "rebase") return { kind: "rebase", text: detail ? `${detail}: ${rest}` : rest };
		if (verb === "reset") return { kind: "reset", text: rest.replace(/^moving to /, "") };
		return { kind: verb, text: rest };
	}

	/** Remote URL → "host/owner/repo" for display. */
	function remoteLabel(url) {
		const s = String(url || "").trim();
		const scp = /^[\w.-]+@([^:/]+):(.+?)(?:\.git)?\/?$/.exec(s);
		if (scp) return `${scp[1]}/${scp[2]}`;
		const web = /^\w+:\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\/(.+?)(?:\.git)?\/?$/.exec(s);
		if (web) return `${web[1]}/${web[2]}`;
		return s;
	}

	/** "3m ago" style age of a unix time (seconds) at `now` (ms). */
	function age(time, now) {
		const s = Math.max(0, Math.round(now / 1000 - time));
		if (s < 60) return "just now";
		const units = [
			[60 * 60 * 24 * 365, "y"],
			[60 * 60 * 24 * 30, "mo"],
			[60 * 60 * 24 * 7, "w"],
			[60 * 60 * 24, "d"],
			[60 * 60, "h"],
			[60, "m"],
		];
		for (const [size, unit] of units) {
			if (s >= size) return `${Math.floor(s / size)}${unit} ago`;
		}
		return "just now";
	}

	/** Paths under .git/ to read for the current HEAD and config. */
	function filesToWatch(files) {
		const want = new Set(["HEAD", "config", "packed-refs", "logs/HEAD"]);
		const head = parseHead(files.HEAD);
		if (head && head.branch) {
			want.add(`refs/heads/${head.branch}`);
			want.add(`logs/refs/heads/${head.branch}`);
			const up = upstreamRef(parseConfig(files.config), head.branch);
			if (up) want.add(up);
		}
		return [...want];
	}

	/** refs/remotes/<remote>/<branch> tracked by `branch`, from config; null if none. */
	function upstreamRef(config, branch) {
		const b = config.branches[branch];
		if (!b || !b.remote || !b.merge || b.remote === ".") return null;
		const merged = b.merge.replace(/^refs\/heads\//, "");
		return `refs/remotes/${b.remote}/${merged}`;
	}

	/**
	 * How many commits the branch is ahead of its upstream, read from the branch reflog: the number
	 * of plain commit entries since the entry that equalled the upstream. null when the reflog
	 * cannot tell (history rewritten, or upstream not in it).
	 */
	function aheadFromReflog(branchLog, upstreamSha) {
		let count = 0;
		for (const entry of branchLog) {
			if (entry.new === upstreamSha) return count;
			const { kind } = describe(entry.message);
			if (kind !== "commit" && kind !== "merge") return null;
			count++;
		}
		return null;
	}

	/** Everything the view shows, from the files read so far (keyed by path under .git/). */
	function summarize(files) {
		const head = parseHead(files.HEAD);
		if (!head) return { ok: false };
		const config = parseConfig(files.config);
		const packed = parsePackedRefs(files["packed-refs"]);
		const refSha = (ref) => parseRefFile(files[ref]) || packed.get(ref) || null;
		const branch = head.branch || null;
		const sha = branch ? refSha(`refs/heads/${branch}`) : head.detached;
		const upRef = branch ? upstreamRef(config, branch) : null;
		const upstream = upRef ? { name: upRef.replace(/^refs\/remotes\//, ""), sha: refSha(upRef) } : null;
		let sync = "none";
		let ahead = null;
		if (upstream && upstream.sha && sha) {
			if (upstream.sha === sha) {
				sync = "same";
			} else {
				ahead = aheadFromReflog(parseReflog(files[`logs/refs/heads/${branch}`]), upstream.sha);
				sync = ahead === null ? "differs" : "ahead";
			}
		}
		const remoteName = (branch && config.branches[branch] && config.branches[branch].remote) || "origin";
		const remote = config.remotes[remoteName] ? remoteLabel(config.remotes[remoteName].url) : "";
		const activity = parseReflog(files["logs/HEAD"]).map((e) => ({ ...describe(e.message), sha: e.new, name: e.name, time: e.time }));
		return { ok: true, branch, sha, upstream, sync, ahead, remote, activity };
	}

	root.GitModel = {
		parseHead,
		parsePackedRefs,
		parseRefFile,
		parseConfig,
		parseReflog,
		describe,
		remoteLabel,
		age,
		filesToWatch,
		upstreamRef,
		aheadFromReflog,
		summarize,
	};
})(typeof window !== "undefined" ? window : globalThis);
