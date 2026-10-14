# Writing widgets for EdgeHost

EdgeHost runs **iCUE HTML widgets** on a CORSAIR XENEON EDGE attached to a Mac. If you have written
an iCUE widget for Windows, most of what you know applies; this guide covers the contract EdgeHost
implements, where it differs from iCUE, and how to build, run and debug a widget.

- Public iCUE contract (Widget API 1.9.0), condensed: [`research/icue-widget-api-reference.md`](research/icue-widget-api-reference.md)
- Design and roadmap: [`SPEC.md`](SPEC.md)
- Working examples: [`examples/widgets/hello-edge`](../examples/widgets/hello-edge) (every injected
  global, a tap counter) and [`examples/widgets/portfolio`](../examples/widgets/portfolio) (network
  data, touch-friendly UI, in-widget editor, chart with scrubbing cursor)

Status: EdgeHost milestone M2. Plugins (`window.plugins.*`) are not implemented yet; see
[Plugins](#9-plugins).

---

## 1. Quick start

```text
my-widget/
├── manifest.json          required
├── index.html             required
├── translation.json       optional (tr() falls back to the key)
├── resources/icon.svg     the manifest's preview_icon
├── scripts/main.js
├── styles/main.css
└── modules/Thing.mjs      optional ES modules (manifest "modules")
```

```bash
cargo run -p edgectl -- validate my-widget          # iCUE CLI rules + EdgeHost checks
cargo run -p edgectl -- run-one my-widget --slot M  # one widget on the Edge (or a stand-in window)
cargo run -p edgectl -- install my-widget           # into the EdgeHost data folder
cargo run -p edgectl -- layout add com.you.my-widget --slot S
cargo run -p edgehost                               # the dashboard app (reloads layout.json on change)
```

`edgectl validate` and `install` accept a folder, a `.icuewidget` package (produced by Corsair's
`icuewidget package`) or a `.zip`.

### Minimal `manifest.json`

```json
{
  "author": "You",
  "id": "com.you.my-widget",
  "name": "My Widget",
  "description": "What it shows",
  "version": "1.0.0",
  "preview_icon": "resources/icon.svg",
  "min_app_version": "5.47",
  "min_framework_version": "1.0.0",
  "os": [{ "platform": "windows" }, { "platform": "mac" }],
  "supported_devices": [{ "type": "dashboard_lcd" }],
  "interactive": true
}
```

- `id`: reverse-DNS, letters, digits, `.` and `-` only.
- `version` and `min_framework_version`: `MAJOR.MINOR.PATCH`.
- `supported_devices` must include `dashboard_lcd` (the XENEON EDGE) or EdgeHost warns.
- `os` is advisory: EdgeHost ignores it. iCUE currently documents only `windows`.
- `interactive: true` if the widget handles taps. EdgeHost delivers taps either way today, but iCUE
  only does so for interactive widgets.

### Minimal `index.html`

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>tr('My Widget')</title>
    <link rel="icon" type="image/svg+xml" href="resources/icon.svg" />
    <meta name="x-icue-property" content="textColor" data-label="tr('Text Color')"
          data-type="color" data-default="'#FFFFFF'" />
    <link rel="stylesheet" href="styles/main.css" />
  </head>
  <body>
    <main id="root"></main>
    <script src="scripts/main.js"></script>
  </body>
</html>
```

**Self-close every void element in `<head>`** (`<meta … />`, `<link … />`). iCUE's importer parses
`<head>` as XML and rejects the widget otherwise. EdgeHost accepts it but `validate` warns.

---

## 2. What EdgeHost injects

Before any of your scripts run, EdgeHost injects a script (`crates/widget-runtime/shim/icue-shim.js`)
that defines:

| Global | Value in EdgeHost |
|---|---|
| `iCUE.iCUELanguage` | `"en"` |
| `iCUE.fpsLimit` | `30` (advisory; not enforced) |
| `iCUE.isPreview` | `false` |
| `iCUE.defaultTemperatureUnit()` | `"°C"` |
| `iCUE.widgetId` | Per-instance UUID (same value as `uniqueId`) |
| `device.deviceId` | Stable UUID (no braces) for the display the widget is on |
| `uniqueId` | **Per-instance** UUID. Use this as your `localStorage` key. |
| one global per `x-icue-property` | The property's concrete value (see §3) |
| `tr(key)` | `Promise<string>` from `translation.json` (`en` fallback, then the key itself) |
| `iCUE_initialized` | `false` until boot finishes, then `true` |
| `window.plugins` | `{}` (see [Plugins](#9-plugins)) |
| `plugin<Module>_initialized` | `false` for every entry in `required_plugins` |
| `<ModuleFileStem>` | Each manifest `modules` entry's exports, e.g. `modules/CitySearch.mjs` → `CitySearch` |

Not provided: `iCUE.ipRegistryApiKey` (Corsair's private key; some Corsair widgets use it for
IP-based location and need their location set by hand instead).

### Lifecycle

```js
// Bare assignment (no var/let/const) is the iCUE convention; EdgeHost also finds let/const.
icueEvents = {
  onICUEInitialized: init,   // once, after modules are imported
  onDataUpdated: update,     // once at boot, and whenever properties change
};

// Guard against your script running after boot.
if (typeof iCUE_initialized !== "undefined" && iCUE_initialized) init();
```

Boot order: injected globals, then your scripts run, then `DOMContentLoaded`, then manifest modules
are imported, then `iCUE_initialized = true`, then `onICUEInitialized()`, then `onDataUpdated()`.

Make `init` idempotent: the late-load guard and the event can both call it.

---

## 3. Properties (`x-icue-property`)

```html
<meta name="x-icue-property" content="period" data-label="tr('Default period')"
      data-type="tab-buttons" data-default="'1d'"
      data-values="[{'key':'1d','value':'1D'},{'key':'5d','value':'5D'}]" />
```

- `content` becomes a global variable: letters, digits and `_`, not starting with a digit.
- `data-type` is one of `slider`, `switch`, `textfield`, `color`, `combobox`, `search-combobox`,
  `tab-buttons`, `media-selector`, `sensors-combobox`, `sensors-factory` or
  `stream-deck-virtual-device`.
- Every other `data-*` attribute is a **JavaScript expression**. Quote strings inside it:
  `data-default="'#FFFFFF'"`; numbers are bare: `data-default="50"`.

### How defaults become values (differs from iCUE)

EdgeHost evaluates each `data-default` **once per widget version**, in a hidden view that has the
same globals and your manifest modules. It stores the results in `layout.json`. Your visible page
then receives **concrete values only**; it never evaluates expressions.

- Expressions may call `tr('…')`, which is synchronous inside expressions, and functions exported by
  your manifest modules.
- A `data-default` that evaluates to a **function** is called. A **Promise** is awaited (10 s
  timeout). For `search-combobox`, a `[key, displayText]` result stores `key`. This matches Corsair
  widgets such as AirQuality (`data-default="OpenMeteo.getDefaultLocation"`).
- A default that throws or times out leaves the property `undefined`, with a warning in the EdgeHost
  log. Code defensively: `typeof myProp === "undefined"`.
- Values must survive JSON. Functions and `undefined` are dropped.

There is no settings UI yet (milestone M4). Users change values with:

```bash
cargo run -p edgectl -- layout show                      # instance ids and current values
cargo run -p edgectl -- layout set 4cbb holdings 'AAPL:20, MSFT:5'   # JSON, else a string
cargo run -p edgectl -- layout unset 4cbb holdings       # back to the default
```

A running EdgeHost picks up the change within about 2 seconds. It reloads the page, and
`onDataUpdated` fires during boot.

### Settings versus state the user edits on the widget

If users can also change something on the widget itself, as the portfolio's Edit panel does, keep
that state in `localStorage` and treat the property as a *seed*. Re-seed only when the property
value changes. The portfolio widget's `seededState()` shows the pattern.

---

## 4. Storage

- Each widget **instance** gets its own WebKit data store, so `localStorage` and IndexedDB are
  already isolated between instances, as in iCUE.
- Still key your data by `uniqueId` (`localStorage.setItem(uniqueId, JSON.stringify(state))`), so
  the same widget behaves identically under iCUE.
- Removing an instance from the layout does not yet delete its data store.

---

## 5. Network access

Widgets are served from `icuew://<instance>/`, a custom origin, so normal browser CORS rules apply.
APIs that send `Access-Control-Allow-Origin: *` (for example open-meteo) work with plain `fetch`.

**For APIs that send no CORS headers, declare the domain**:

```json
"permissions": [{ "type": "url", "domain": "query1.finance.yahoo.com", "port": 443 }]
```

EdgeHost then routes `fetch()` calls to that domain through itself, so CORS does not apply. You still
call `fetch(url)` normally.

| Rule | Detail |
|---|---|
| Matching | `domain` and its subdomains. `"any"` matches every non-loopback host. `port` is optional (default 443/80); loopback (`localhost`, `127.*`, `::1`) needs an explicit port. |
| Methods | `GET` via `fetch()` only. XHR, WebSocket and non-GET requests are not routed and remain subject to CORS. |
| Limits | 15 s timeout, 5 MiB response, redirects followed. |
| Response | Status, `Content-Type` and body pass through. `response.url` is the internal proxy URL. |
| Undeclared domains | Not routed (normal browser behaviour). From milestone M3 the user approves declared permissions. |

Request headers you set (for example API keys in headers) are **not** forwarded yet. Put keys in the
query string, or ask for header support.

---

## 6. Files, modules and `common/`

- Reference your files with relative paths (`scripts/main.js`, `resources/x.png`). EdgeHost serves
  only files inside the widget folder; `..` escapes and symlinks pointing out of the folder are
  refused.
- `manifest.modules` entries are imported as ES modules. Their exports become a global named after
  the file stem.
- iCUE ships a shared `common/` folder beside its built-in widgets (for example
  `common/tools/media_viewer/MediaViewer.js`). Widgets that reference `../common/…` get it from
  EdgeHost's `common` folder, which the user must supply. **Prefer copying what you need into your
  widget.**

---

## 7. Sizes and layout

The landscape panel is 2560×720, a 3×2 grid of 840×344 cells with 8 px gutters and 12 px margins.
Portrait is a single column of six 696×416 cells.

| Slot | Landscape | Portrait |
|---|---|---|
| S | 840×344 | 696×416 |
| M | 840×696 | 696×840 |
| L | 1688×696 | 696×1688 |
| XL | 2536×696 | 696×2536 |

Your page's CSS viewport is exactly the slot size (`innerWidth`×`innerHeight`), at device-pixel-ratio 1.
Use media queries on the viewport:

```css
@media (max-height: 420px) { /* S: short and wide */ }
@media (min-width: 1500px) and (min-height: 421px) { /* L and XL */ }
```

Test every slot you support: `edgectl run-one my-widget --slot S` (also `M`, `L`, `XL`). Add
`--windowed --scale 0.5` to test without an Edge.

---

## 8. Touch and keyboard input

Verified on a XENEON EDGE (macOS, 2026-10):

- **A tap arrives as an ordinary mouse click.** The sequence is `pointerdown` → `mousedown` →
  `pointerup` → `mouseup` → `click`, with `pointerType` `"mouse"` and `isTrusted` true. Use normal
  **`click`** handlers on `<button>` elements. No touch-specific code is needed.
- **There are no `touchstart`/`touchmove` events and no multi-touch.** A finger drag is a mouse drag
  (`pointermove` with `buttons === 1`). For drag interactions (scrubbing, drawing), use pointer
  events with `setPointerCapture` and `touch-action: none` on that element.
- Taps reach your widget even when another app is frontmost: EdgeHost accepts the first click.
- **Make targets finger-sized**: at least 44 px tall, and more for frequent actions. Edge touches are
  less precise than a mouse.
- `user-select: none` on non-text UI stops a drag from selecting labels. Leave inputs selectable.
- **Typing**: when an `<input>`, `<textarea>`, `<select>` or contentEditable element gains focus,
  EdgeHost becomes the active app so keystrokes reach the field. There is no on-screen keyboard; the
  user types on the Mac's keyboard.

---

## 9. Plugins

`window.plugins.*` providers are **not implemented yet**: sensors, media, links and the rest arrive
in milestone M3. For every `required_plugins` entry, `plugin<Module>_initialized` is `false` and the
facade is missing. Guard plugin use so your widget still renders:

```js
if (typeof pluginSensorsdataprovider_initialized !== "undefined" && pluginSensorsdataprovider_initialized) {
  window.plugins.Sensorsdataprovider.getSensorValue(id, sensorId);
}
```

Planned macOS support: links, file system, and basic sensors (CPU/GPU/memory load, best-effort
temperatures) in v1; media in v1.5. Notifications, FPS, device actions and Stream Deck will report
as unsupported. EdgeHost add-ins will be scriptable in [Rhai](https://rhai.rs).

---

## 10. Debugging and testing

| Tool | Use |
|---|---|
| `edgectl validate <dir or package>` | Manifest/HTML/file-reference errors and warnings, with the CLI's wording |
| EdgeHost / `run-one` stderr | Your `console.log/warn/error`, uncaught errors and rejected promises, prefixed `[widget-id]` |
| `edgectl run-one … --devtools` | Safari Web Inspector (Develop menu) for the widget page |
| `EDGEHOST_DEBUG_INPUT=1` | Logs every pointer/mouse/click event with coordinates and target |
| `edgectl layout show` | Instance ids, evaluated and user-set values (`*` = set by the user) |

Logic tests without a browser: load your script into a Node `vm` context with a small fake DOM and
call `icueEvents.onICUEInitialized()`. See `examples/widgets/portfolio/portfolio.test.mjs`. Provide
every global your code touches, timers included (`setTimeout`/`clearTimeout`).

```bash
node --test examples/widgets/portfolio/
```

---

## 11. Differences from iCUE on Windows

| Area | iCUE (Windows) | EdgeHost (macOS) |
|---|---|---|
| Engine | QtWebEngine 6.9 (Chromium 130) | WebKit (WKWebView). No WebHID/WebSerial/WebUSB, File System Access or `queryLocalFonts`. |
| Defaults | Evaluated by iCUE | Evaluated once per version in a hidden view; pages get concrete values (§3) |
| Settings UI | iCUE widget settings | `edgectl layout set` until the configurator (M4) |
| Plugins | Available | Not yet (M3) |
| Cross-origin `fetch` | Widget origin rules | Declare `url` permissions to bypass CORS (§5) |
| Touch | Touch events on the Edge | Mouse events (§8) |
| `iCUE.ipRegistryApiKey` | Corsair's key | Absent |
| Media-selector video | AV1/VP8/VP9 | Same as iCUE on macOS: not supported |

## 12. Open questions

- Whether property changes should re-run `onDataUpdated` without reloading the page (iCUE does; EdgeHost
  currently reloads). Write `onDataUpdated` so it also works when called at boot only.
