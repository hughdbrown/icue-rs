---
name: edgehost-rhai-addin
description: Build EdgeHost Rhai add-ins (sandboxed data providers exposed to iCUE HTML widgets as window.plugins.<Name>) and, optionally, the HTML widget that uses one; scaffolds, writes the script, checks it with edgectl and tests it. Use when the user wants a new data source, service, provider or plugin for the XENEON EDGE / EdgeHost (for example "an add-in for my Home Assistant", "a widget that shows X from an API"), mentions Rhai, addin.toml, main.rhai or `edgectl addin`, or wants to provide an iCUE plugin such as Sensors that EdgeHost lacks.
---

# EdgeHost Rhai add-in

An **add-in** is a folder holding `addin.toml` and `main.rhai`. It provides a plugin
(`module:Name:X.Y`) that widgets list in `required_plugins` and use as `window.plugins.<Facade>`.
Widgets are always HTML and JS; Rhai is only for add-ins. Contract: `docs/addin-api.md`. Recipes and
gotchas: [PATTERNS.md](PATTERNS.md). Worked example: `examples/addins/dev.edgehost.coin-prices` and
`examples/widgets/coin-ticker`.

## Workflow

1. **Pin down the interface before code.** Decide:
   - what the data source is, and whether it needs polling (`poll_ms`) or only on-demand calls;
   - the methods widgets call, and the signals the add-in pushes;
   - every host and port the script must reach;
   - id (`dev.<you>.<thing>`), module (its last part becomes the facade name) and name.

   Ask the user only about what you cannot infer, such as an API base URL or credentials.
2. **Scaffold** from the repo root:
   ```bash
   python3 .claude/skills/edgehost-rhai-addin/scripts/scaffold.py addin \
     --id dev.you.weather --module dev.weather --name Weather \
     --methods current --signals updated --poll-ms 600000 --http api.open-meteo.com
   ```
   This writes `examples/addins/<id>/` (`--dir` to change it). The output already passes `check`.
3. **Write `main.rhai`** against `docs/addin-api.md` §4–§6. Keep the scaffold's shape:
   - state in `this`;
   - `init()` is idempotent;
   - `handle()` switches on the method;
   - `poll()` and `handle()` share `this.refresh()`;
   - errors go out as an error signal or as returned data, not only as `throw` (§9).
4. **Verify**, fixing until each step is clean:
   ```bash
   cargo build -q -p edgectl
   ./target/debug/edgectl addin check examples/addins/<id>
   ./target/debug/edgectl addin run examples/addins/<id> --call <method> --args '[…]'
   ./target/debug/edgectl addin run examples/addins/<id> --seconds 5   # watch poll() signals
   ```
   Exercise each method, a bad argument and an unreachable host. If the real service isn't
   reachable from this machine, say so instead of claiming it works.
5. **Widget (if wanted):**
   ```bash
   python3 .claude/skills/edgehost-rhai-addin/scripts/scaffold.py widget \
     --id dev.you.weather-card --title "Weather" --plugin dev.weather:Weather:1.0 \
     --methods current --signals updated
   ./target/debug/edgectl validate examples/widgets/weather-card
   node --test examples/widgets/weather-card/
   ```
   Then build the real UI. Follow `docs/widget-developer-guide.md`: finger-sized targets,
   `user-select: none`, `click` handlers, and colours from `x-icue-property`. Add the widget folder
   to `js_test_dirs` in the `justfile`.
6. **End to end (optional).** Run it in a scratch data folder, so nothing touches the user's real
   EdgeHost:
   ```bash
   export EDGEHOST_HOME=$(mktemp -d)
   ./target/debug/edgectl addin install examples/addins/<id> --yes   # scratch store only
   ./target/debug/edgectl run-one examples/widgets/<widget> --windowed --slot M
   ```
   A `print(...)` in the script shows up as `[addin <id>]` in the output.
7. **Finish:** run `just ci`, and commit the add-in and the widget as separate commits. Install into
   the user's real store (`edgectl addin install …` with no `EDGEHOST_HOME`) only if they ask. Installing
   approves the add-in's hosts: show the user what it declares, and pass `--yes` only once they agree.

## Rules that bite

- The entry point is `fn handle(method, args)`. `call` is a reserved word in Rhai.
- Functions cannot read top-level variables. Use `this`, and call helpers as `this.helper(…)`.
- A widget's call **never rejects**. A failure resolves to `undefined` and logs a warning.
- `init()` runs at start and again on page load (usually twice), always with `this = #{}`.
- Each entry point gets 2 s of script time and 20 s in total, `http_get` waits included. Keep
  network calls few per entry point.
- 5,000,000 operations per entry point, enough for only about 1–2 million loop iterations. Do heavy
  work in chunks across polls.
- `http_get` covers only `permissions.http` hosts and subdomains on the default port, or the
  `host:port` you list. Loopback needs a port. **Redirects fail**, so request the final URL.
- No files, processes, `import`, `eval`, POST or custom headers. If the source needs those, say so:
  it needs a built-in Rust provider, not an add-in.
- `try`/`catch` is a statement, not an expression: assign inside it, then return the variable.
- Every `methods` entry must be handled and every emitted signal declared. `poll_ms` and `fn poll()`
  must come together.
