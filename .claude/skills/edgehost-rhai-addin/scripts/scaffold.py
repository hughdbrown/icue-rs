#!/usr/bin/env python3
"""Scaffold an EdgeHost Rhai add-in, or a widget that uses one.

  scaffold.py addin  --id dev.you.weather --module dev.weather --name Weather \\
                     --methods current,refresh --signals updated [--poll-ms 60000] \\
                     [--http api.open-meteo.com,localhost:8123] [--dir examples/addins]
  scaffold.py widget --id dev.you.weather-card --title "Weather" \\
                     --plugin dev.weather:Weather:1.0 --methods current --signals updated \\
                     [--dir examples/widgets]

Writes files that already pass `edgectl addin check` / `edgectl validate` and `node --test`;
refuses to overwrite an existing folder. Fill in the TODOs afterwards.
"""

import argparse
import json
import re
import sys
from pathlib import Path

IDENT = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")
ADDIN_ID = re.compile(r"^[a-z0-9_-][a-z0-9._-]*$")


def csv(value):
    return [v.strip() for v in value.split(",") if v.strip()] if value else []


def fail(message):
    sys.exit(f"scaffold.py: {message}")


def facade_name(module):
    last = module.rsplit(".", 1)[-1]
    return last[:1].upper() + last[1:].lower()


def check_idents(kind, names):
    for n in names:
        if not IDENT.match(n):
            fail(f"{kind} {n!r} is not a JavaScript identifier")


def write(folder, files):
    if folder.exists():
        fail(f"{folder} already exists; choose another --id or --dir")
    for rel, text in files.items():
        path = folder / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    for rel in sorted(files):
        print(folder / rel)


def addin(a):
    if not ADDIN_ID.match(a.id):
        fail("--id may use only a-z, 0-9, '.', '_' and '-'")
    if not re.match(r"^[A-Za-z0-9_]+$", a.module.rsplit(".", 1)[-1]) or ":" in a.module:
        fail("the last part of --module must be letters, digits or '_'")
    if not re.match(r"^\d+\.\d+$", a.version):
        fail("--version must look like 1.0")
    methods, signals, hosts = csv(a.methods), csv(a.signals), csv(a.http)
    check_idents("method", methods)
    check_idents("signal", signals)
    if a.poll_ms is not None and not 100 <= a.poll_ms <= 3_600_000:
        fail("--poll-ms must be between 100 and 3600000")

    toml = [
        f'id = "{a.id}"',
        f'module = "{a.module}"',
        f'name = "{a.name}"',
        f'version = "{a.version}"',
        f"description = {json.dumps(a.description or 'TODO: what this add-in provides')}",
        f"methods = {json.dumps(methods)}",
        f"signals = {json.dumps(signals)}",
    ]
    if a.poll_ms is not None:
        toml.append(f"poll_ms = {a.poll_ms}")
    toml += ["", "[permissions]", f"http = {json.dumps(hosts)}", ""]

    first_signal = signals[0] if signals else None
    lines = [
        f"// {a.name}: TODO describe the data. Reference: docs/addin-api.md",
        "//",
        "// State lives in `this` (reset to #{} on every start and page reload). Functions cannot see",
        "// top-level variables. Call helpers as this.helper(…) to keep `this` bound.",
        "",
        "fn init() {",
        "    this.value = ();",
        "}",
    ]
    if methods:
        lines += ["", "fn handle(method, args) {", "    switch method {"]
        for m in methods:
            lines.append(f'        "{m}" => {{ if this.value == () {{ this.refresh(); }} this.value }}')
        lines += ["    }", "}"]
    if a.poll_ms is not None:
        lines += ["", "fn poll() {", "    this.refresh();", "}"]
    lines += ["", "// Fetch fresh data into this.value and tell widgets."]
    lines += ["fn refresh() {"]
    if hosts:
        host = hosts[0]
        scheme = "http" if host.startswith(("localhost", "127.")) else "https"
        lines.append(f'    let body = parse_json(http_get("{scheme}://{host}/TODO"));')
        lines.append("    this.value = body;")
    else:
        lines.append("    this.value = now_ms();  // TODO: compute the real value")
    if first_signal:
        lines.append(f'    emit("{first_signal}", [this.value]);')
    lines += ["}", ""]

    write(Path(a.dir) / a.id, {"addin.toml": "\n".join(toml), "main.rhai": "\n".join(lines)})
    print(f"next: edgectl addin check {Path(a.dir) / a.id}")


WIDGET_JS = """// {title}: shows data from the {plugin} add-in through window.plugins.{facade}.
(function () {{
	"use strict";

	const $ = (id) => document.getElementById(id);
	const api = () => window.plugins && window.plugins.{facade};

	function show(value) {{
		$("value").textContent = value === undefined || value === null ? "—" : String(value);
	}}

	function connect() {{
		const p = api();
		if (!p || connect.done) return;
		connect.done = true;
{connects}	}}

	async function onReady() {{
		const p = api();
		$("message").hidden = Boolean(p);
		if (!p) return;
		connect();
{first_call}	}}

	window.plugin{facade}Events = {{ onInitialized: connect }};
	window.icueEvents = {{ onICUEInitialized: onReady, onDataUpdated: onReady }};
	window.{global_name} = {{ show }};
}})();
"""

WIDGET_TEST = """// Run with: node --test {test_dir}
import assert from "node:assert/strict";
import {{ readFileSync }} from "node:fs";
import test from "node:test";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("./scripts/widget.js", import.meta.url), "utf8");

function load(plugin) {{
	const byId = new Map();
	const el = () => ({{ textContent: "", hidden: false }});
	const document = {{ getElementById: (id) => byId.get(id) || byId.set(id, el()).get(id) }};
	const ctx = {{ document, plugins: plugin ? {{ {facade}: plugin }} : {{}} }};
	ctx.window = ctx;
	vm.createContext(ctx);
	vm.runInContext(SOURCE, ctx);
	return {{ ctx, byId }};
}}

function fakePlugin() {{
	const handlers = {{}};
	const signal = (name) => ({{ connect: (f) => (handlers[name] = f), disconnect() {{}} }});
	return {{
		handlers,
		api: {{
{fake_methods}{fake_signals}		}},
	}};
}}

test("shows the first value and later signals", async () => {{
	const fake = fakePlugin();
	const env = load(fake.api);
	await env.ctx.icueEvents.onICUEInitialized();
	assert.equal(env.byId.get("value").textContent, {expect_first});
{signal_test}}});

test("explains a missing add-in", async () => {{
	const env = load(null);
	await env.ctx.icueEvents.onICUEInitialized();
	assert.equal(env.byId.get("message").hidden, false);
}});
"""


def widget(a):
    try:
        module, name, version = a.plugin.split(":")
        major, minor = version.split(".")
        int(major), int(minor)
    except ValueError:
        fail("--plugin must look like module:Name:1.0")
    facade = facade_name(module)
    methods, signals = csv(a.methods), csv(a.signals)
    check_idents("method", methods)
    check_idents("signal", signals)
    folder = Path(a.dir) / a.id.rsplit(".", 1)[-1]
    global_name = re.sub(r"\W", "", a.title.title()) or "Widget"

    connects = "".join(f"\t\tp.{s}.connect(show);\n" for s in signals)
    first_call = (
        f"\t\tshow(await p.{methods[0]}()); // undefined if the call failed\n"
        if methods
        else "\t\tshow(undefined); // the first signal fills it in\n"
    )
    fake_methods = "".join(f"\t\t\t{m}: async () => 42,\n" for m in methods)
    fake_signals = "".join(f'\t\t\t{s}: signal("{s}"),\n' for s in signals)
    signal_test = (
        f'\tfake.handlers.{signals[0]}(7);\n\tassert.equal(env.byId.get("value").textContent, "7");\n'
        if signals
        else ""
    )
    manifest = {
        "author": a.author,
        "id": a.id,
        "name": a.title,
        "description": f"TODO. Needs the {name} add-in ({a.plugin}).",
        "version": "0.1.0",
        "preview_icon": "resources/icon.svg",
        "min_app_version": "5.47",
        "min_framework_version": "1.0.0",
        "os": [{"platform": "windows"}, {"platform": "mac"}],
        "supported_devices": [{"type": "dashboard_lcd"}],
        "interactive": False,
        "required_plugins": [a.plugin],
    }
    html = f"""<!doctype html>
<html lang="en">
	<head>
		<meta charset="utf-8" />
		<title>{a.title}</title>
		<link rel="icon" type="image/svg+xml" href="resources/icon.svg" />
		<meta name="x-icue-property" content="textColor" data-label="Text Color" data-type="color" data-default="'#E8ECF2'" />
		<style>
			html, body {{ margin: 0; height: 100%; background: #101318; color: #e8ecf2;
				font: 600 16px/1.2 -apple-system, sans-serif; user-select: none; -webkit-user-select: none; }}
			main {{ height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; }}
			#value {{ font-size: clamp(18px, 30vh, 120px); font-variant-numeric: tabular-nums; }}
			#message {{ opacity: 0.7; }}
		</style>
	</head>
	<body>
		<main>
			<div id="value">—</div>
			<p id="message" hidden>Needs the {name} add-in ({a.plugin}).</p>
		</main>
		<script src="scripts/widget.js"></script>
	</body>
</html>
"""
    icon = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
        '<rect width="64" height="64" rx="12" fill="#101318"/>'
        '<circle cx="32" cy="32" r="14" fill="none" stroke="#e8ecf2" stroke-width="5"/></svg>\n'
    )
    write(
        folder,
        {
            "manifest.json": json.dumps(manifest, indent="\t") + "\n",
            "index.html": html,
            "resources/icon.svg": icon,
            "scripts/widget.js": WIDGET_JS.format(
                title=a.title, plugin=a.plugin, facade=facade, connects=connects,
                first_call=first_call, global_name=global_name,
            ),
            "widget.test.mjs": WIDGET_TEST.format(
                test_dir=f"{folder}/", facade=facade, fake_methods=fake_methods,
                fake_signals=fake_signals, signal_test=signal_test,
                expect_first='"42"' if methods else '"—"',
            ),
        },
    )
    print(f"next: edgectl validate {folder} && node --test {folder}/")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="kind", required=True)
    pa = sub.add_parser("addin", help="a Rhai add-in folder")
    pa.add_argument("--id", required=True)
    pa.add_argument("--module", required=True)
    pa.add_argument("--name", required=True)
    pa.add_argument("--version", default="1.0")
    pa.add_argument("--description")
    pa.add_argument("--methods", default="")
    pa.add_argument("--signals", default="")
    pa.add_argument("--poll-ms", type=int)
    pa.add_argument("--http", default="", help="comma-separated host or host:port grants")
    pa.add_argument("--dir", default="examples/addins")
    pw = sub.add_parser("widget", help="an HTML widget that uses an add-in")
    pw.add_argument("--id", required=True)
    pw.add_argument("--title", required=True)
    pw.add_argument("--plugin", required=True, help="module:Name:Major.Minor")
    pw.add_argument("--methods", default="")
    pw.add_argument("--signals", default="")
    pw.add_argument("--author", default="EdgeHost")
    pw.add_argument("--dir", default="examples/widgets")
    a = p.parse_args()
    (addin if a.kind == "addin" else widget)(a)


if __name__ == "__main__":
    main()
