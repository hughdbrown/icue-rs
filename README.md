# EdgeHost

Run iCUE HTML widgets on a CORSAIR XENEON EDGE connected to a Mac.

Corsair's iCUE only drives the XENEON EDGE's widget dashboard on Windows; on macOS the Edge is a plain
2560×720 monitor. EdgeHost hosts widgets written against the public
[iCUE Widget API](https://docs.elgato.com/icue/widgets/) (1.9.0) on that display, and adds a
sandboxed add-in system (Rhai scripts) for data providers.

**Status:** milestone M1. One widget runs in a borderless window on the Edge, or in a scaled stand-in
window when no Edge is attached. See [`docs/SPEC.md`](docs/SPEC.md) for the plan and milestones.

## What it is not

- Not affiliated with or endorsed by CORSAIR or Elgato. "iCUE" and "XENEON" are their trademarks, used
  here only to describe compatibility.
- Not a replacement for iCUE's device features: no RGB, fan, pump or firmware control.
- No Marketplace or account integration. EdgeHost ships no Corsair code, widgets or assets.

## Requirements

- macOS 14 Sonoma or later (per-widget WebKit data stores need it)
- Rust (edition 2024) and Node.js (for the shim tests)
- Optional: `brew install lld`. `.cargo/config.toml` links with `ld64.lld`; if linking fails after an
  Xcode update, run `brew upgrade lld`, or build with `RUSTFLAGS=""` to use Apple's linker.

macOS permissions are requested only when a feature needs them. Input Monitoring (Edge touch, M4) and
Automation (media fallback, v1.5) are expected.

## Usage

```bash
# Inspect a widget directory: manifest, controls, plugins, warnings
cargo run -p edgectl -- validate examples/widgets/hello-edge

# Run it at Large size; uses the Edge if attached, otherwise a half-size stand-in window
cargo run -p edgectl -- run-one examples/widgets/hello-edge --slot L

# Force the stand-in window, pick a slot and grid cell, open Safari Web Inspector
cargo run -p edgectl -- run-one path/to/widget --windowed --slot S --col 2 --row 1 --devtools
```

Widgets that load iCUE's shared `../common/...` folder need it beside the widget directory, or passed
with `--common-dir`. Press Esc to close the borderless Edge window.

### Pages

The EdgeHost app (`cargo run -p edgehost`) shows one page of widgets at a time. Add pages in
**Configure Widgets…** (menu-bar icon) or with `edgectl layout add --page N`. To change the page:

- **Swipe** left (next) or right (previous) on the Edge: a quick horizontal drag of about 2 cm,
  anywhere on a widget or the background. Drags that start in a text field or slider, or on a part
  of a widget that tracks drags itself (piano keys, chart scrubbing), stay with the widget.
- **Tap a dot** in the strip at the bottom centre; it appears when there are two or more pages.
- **Menu-bar icon → Next Page / Previous Page.** The menu's first line shows "Page 2 of 3".

Pages wrap around. The configurator's page tabs only choose which page you edit; they do not
change the Edge.

## Extending EdgeHost

There are two kinds of component, and neither needs EdgeHost to be rebuilt:

| You want… | Write a… | In | Lives in |
|---|---|---|---|
| Something on the Edge's screen | **Widget** | HTML, CSS and JavaScript (the iCUE widget format) | `<data folder>/widgets/<id>/<version>/` |
| New data or a service for widgets (an API, a poller, a local server) | **Add-in** (data provider) | **Rhai** script plus a TOML descriptor | `<data folder>/addins/<id>/` |

Widgets are always HTML; Rhai is only for add-ins. A widget asks for an add-in's plugin in its
`manifest.json` (`required_plugins`) and uses it as `window.plugins.<Name>`, with methods it calls
and signals it connects to. The data folder is `~/Library/Application Support/EdgeHost`.

### Add a data provider (Rhai add-in)

1. **Create a folder** named after the add-in id, holding `addin.toml` and `main.rhai`:

   ```toml
   # dev.you.weather/addin.toml
   id = "dev.you.weather"
   module = "dev.weather"            # window.plugins.Weather
   name = "Weather"
   version = "1.0"                   # widgets require "dev.weather:Weather:1.0"
   methods = ["current"]
   signals = ["updated"]
   poll_ms = 600000                  # optional: run poll() every 10 minutes

   [permissions]
   http = ["api.open-meteo.com"]     # the only hosts http_get() may reach
   ```

   ```rhai
   // dev.you.weather/main.rhai
   fn init() { this.last = (); }                       // state lives in `this`

   fn handle(method, args) {                           // widget calls
       switch method {
           "current" => { if this.last == () { this.refresh(); } this.last }
       }
   }

   fn poll() { this.refresh(); }                       // runs now, then every poll_ms

   fn refresh() {                                      // this.refresh() binds `this`
       let url = "https://api.open-meteo.com/v1/forecast?latitude=39.74&longitude=-104.99&current=temperature_2m";
       this.last = parse_json(http_get(url)).current.temperature_2m;
       emit("updated", [this.last]);
   }
   ```

2. **Check and try it** without any widget:
   `edgectl addin check dev.you.weather`, then
   `edgectl addin run dev.you.weather --seconds 5` (prints signals) or
   `edgectl addin run dev.you.weather --call current`.
3. **Install it:** `edgectl addin install dev.you.weather`. Use `edgectl addin list` to confirm, and
   `edgectl addin remove dev.you.weather` to uninstall.
4. **Use it from a widget:** add `"required_plugins": ["dev.weather:Weather:1.0"]` to the widget's
   `manifest.json`, then in its script:

   ```js
   pluginWeatherEvents = { onInitialized: () => {
       plugins.Weather.updated.connect((t) => show(t));
       plugins.Weather.current().then((t) => t !== undefined && show(t));  // failed calls give undefined
   } };
   ```

   `edgectl validate <widget>` shows whether the add-in is found. Widgets pick up a new or changed
   add-in the next time their page is shown.

Scripts are sandboxed: no files, processes, `import` or `eval`, and network access only through
`http_get` to the hosts listed (redirects are not followed). Operation, time and size limits stop
runaway scripts, which then restart with fresh state.

A complete example is the [`coin-prices`](examples/addins/dev.edgehost.coin-prices) add-in with its
[`coin-ticker`](examples/widgets/coin-ticker) widget. Documentation:
[`docs/addins.md`](docs/addins.md) (walkthrough) and
[`docs/addin-api.md`](docs/addin-api.md) (full API reference).

### Add a widget

1. Make a folder with `manifest.json` and `index.html` (start from
   [`examples/widgets/hello-edge`](examples/widgets/hello-edge)).
2. `edgectl validate <folder>`, then `edgectl run-one <folder> --windowed` to see it.
3. `edgectl install <folder>` and `edgectl layout add <widget id> --slot M` (or **Configure Widgets…**).

See [`docs/widget-developer-guide.md`](docs/widget-developer-guide.md).

### With Claude Code

The repository ships a Claude Code skill, [`edgehost-rhai-addin`](.claude/skills/edgehost-rhai-addin/SKILL.md).
Ask Claude for a data provider or a widget backed by one (for example, "make an add-in that shows
my Home Assistant sensors"). It scaffolds the add-in, writes the script against the API reference,
checks and runs it with `edgectl addin`, and can add a matching widget.

## Development

```bash
cargo check --all-targets
cargo test --workspace
cargo clippy --all-targets -- -D warnings
node --test crates/widget-runtime/shim/
node --test examples/widgets/portfolio/
node --test examples/widgets/git/
node --test examples/widgets/coin-ticker/
```

| Path | Contents |
|---|---|
| `crates/icuew-manifest` | Parse `manifest.json`, `<head>` controls and groups, `translation.json` |
| `crates/edge-layout` | Edge grid, S/M/L/XL slots, display selection |
| `crates/edge-plugins` | Plugins behind `window.plugins`: File System built in, Rhai add-ins, path macros and grants |
| `crates/widget-runtime` | Injection shim, `icuew://` asset server, Edge surface and web views |
| `apps/edgectl` | Developer CLI |
| `examples/widgets/hello-edge` | Test widget that displays the injected globals |
| `examples/widgets/git` | Branch, upstream sync and recent activity of a local repo (File System plugin) |
| `examples/widgets/coin-ticker` | Live crypto prices from the sample add-in |
| `examples/addins/dev.edgehost.coin-prices` | Sample Rhai add-in: Coinbase spot prices as `window.plugins.Coinprices` |
| `docs/` | Spec, review, and research notes (reverse-engineering findings, API reference) |

Writing a widget? Start with [`docs/widget-developer-guide.md`](docs/widget-developer-guide.md).
Writing a data provider (add-in) in Rhai? See [`docs/addins.md`](docs/addins.md) and
[`docs/addin-api.md`](docs/addin-api.md).
