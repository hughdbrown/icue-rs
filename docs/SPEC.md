# EdgeHost: an iCUE-compatible widget host and add-in controller for XENEON EDGE on macOS

Status: draft v0.2, 2026-10-13 (v0.2 applies `docs/recommendations.md` and owner decisions; see §0)
Repo: `~/projects/software/icue-rs`
Inputs:
- `~/projects/data/corsair-xeneon-edge-icue/outline.md`: the 2026-10-03 feasibility Q&A
- `docs/research/rea-icue-mac-findings.md`: reverse-engineering run of iCUE 5.52.93 for Mac (rea, Ghidra, Hopper)
- `docs/research/icue-widget-api-reference.md`: condensed public widget API 1.9.0 (docs.elgato.com)
- `~/tools/corsair-labs/iCUE-widget-runner-{RaspberryPi,windows}`: Corsair's own MIT-licensed Electron runners (prior art)

---

## 0. Revision 0.2 decisions

Owner decisions:
- The name is **EdgeHost**.
- The app appears **in the Dock and in the menu bar** (regular activation policy plus an `NSStatusItem`).
- The minimum OS is **macOS 14 Sonoma**.
- **Add-ins are scripts in [Rhai](https://github.com/rhaiscript/rhai)** (§7.1).

From the review (`docs/recommendations.md`):

| Topic | Decision |
|---|---|
| Crate count | Start with 5 crates (`icuew-manifest`, `edge-layout`, `widget-runtime`, an `edgehost` app crate, `edgectl`). Providers, store, permissions and touch begin as modules of the app crate and are extracted only when the boundary is proven (§3.1). |
| M0 | Hard gate for **touch (M4) and Edge display identity** only. Packages, layout, runtime and providers do not depend on the hardware and proceed in parallel. |
| Attribute expressions | Runtime pages receive **concrete property values only**. Expressions are evaluated in a hidden evaluation context **after provider snapshots arrive** (§6.4). M1's in-page evaluation is an interim shortcut. |
| "Block" (synchronous) plugin methods | Kept, answered in JavaScript from host-pushed snapshots (§6.5). The review's claim that they cannot be synchronous applies only to native round trips. |
| Providers in v1 | `link`, `filesystem` and basic `sensors` (CPU, GPU and memory load; temperatures best-effort). `media` moves to v1.5. `notifications`, `fps`, `deviceaction` and `streamdeck` are declared unsupported stubs (§7.2). |
| Permissions | Enforced in phases (§8.3). |
| Engine | WebKit only in v1. CEF is out of scope unless the corpus forces it (§6.2). |
| Display | Size-based auto-detection plus a **manual display picker** (§5.1). |

M1 results (2026-10-13), from `edgectl run-one --windowed` at L size:
- `hello-edge` and all five Corsair Labs runner samples boot: AirQuality, Doodle Pad, Robex Tourbillon,
  SpectrumAnalyzer and VU Meter.
- Compatibility fixes the corpus forced:
  - a shared `common/` folder served at `/common/` (iCUE's built-ins load `../common/tools/...`);
  - flat `translation.json` tables;
  - unknown `os.platform` values such as `linux`.

---

## 1. Problem

The CORSAIR XENEON EDGE is a 14.5" 2560×720 touchscreen. On Windows, iCUE turns it into a widget
dashboard. On macOS it is only a plain monitor:

- iCUE for Mac has no Edge entry in its device catalog (`devices.json`).
- The HTML-widget runtime is not shipped in the Mac build. iCUE's log on this Mac lists
  `DisplayScreenManager.manager.dashlcd`, `WidgetPluginProviderController` and 11 more interfaces as
  "No implementation found".

EdgeHost is a macOS app that:

1. drives the Edge as a dedicated dashboard surface,
2. **runs unmodified iCUE HTML widgets** (`.icuewidget` packages, manifest + `index.html`) by
   reproducing the *public* iCUE widget contract (Widget API 1.9.0), and
3. provides an **add-in controller**: a plugin system that implements the `window.plugins.*` data
   providers with macOS back ends, and accepts new third-party providers.

### Goals

- G1. Widgets built with Corsair's `icuewidget` CLI install and run without edits, whenever their
  plugins have a macOS back end.
- G2. Edge layout matches iCUE: S/M/L/XL slots, landscape and portrait, multiple pages.
- G3. Touch input on the Edge works, including when macOS maps the digitizer to the wrong display.
- G4. Per-widget settings UI is generated from the widget's `x-icue-property` meta tags.
- G5. The permission model matches iCUE's (`file`, `folder`, `url`, …) and is actually enforced.
- G6. Add-in providers are isolated, versioned and negotiated against `required_plugins`.

### Non-goals (v1)

- Controlling Corsair RGB, fans or AIOs (that is iCUE's device side; use iCUE for Mac or liquidctl).
- Corsair Marketplace or account integration.
- iCUE's native QML "screens" (pump and keyboard LCD content); only HTML widgets are in scope.
- Pixel-identical Chromium rendering (see §6.2 on the engine).
- Shipping, or depending on, any Corsair binary, built-in widget or `common/` bundle.

### Naming and legal

- Product name **EdgeHost** (working title). Don't use "iCUE" or "CORSAIR" in the product name or icon;
  they are trademarks. Compatibility can be stated factually ("runs iCUE widgets").
- Implement from the public spec plus black-box behaviour. The reverse-engineering findings are used
  only to confirm architecture (interoperability). Corsair's runner repos are MIT, so code may be reused
  with attribution.

---

## 2. Key findings that shape the design

| # | Finding | Source | Design consequence |
|---|---|---|---|
| F1 | The Edge is HDMI or DP-Alt video plus a separate USB HID touch digitizer. Widgets run on the **host**. | outline.md, Corsair docs | No display protocol to reverse. Build a host window plus a touch driver. |
| F2 | Widgets are HTML/CSS/JS. iCUE injects globals **before** widget scripts: one global per `x-icue-property`, plus `iCUE`, `device`, `uniqueId`, `iCUE_initialized`, `window.plugins.*` and `plugin<Module>_initialized`. | spec 1.9.0 | Inject with `WKUserScript` at document start. |
| F3 | Plugins are QWebChannel-shaped. Async calls are `method(requestId, …)` answered by the signal `asyncResponse(requestId, value)`. Signals use `.connect(fn)`. Some "Block" methods are **synchronous** (`getDefaultSensorIdBlock`). | plugin docs | The JS shim keeps a local cache so sync methods answer without a native round trip. |
| F4 | Landscape slot sizes are S 840×344, M 840×696, L 1688×696, XL 2536×696. Portrait sizes are 696×416, 696×840, 696×1688, 696×2536. | Corsair Labs | Derived grid (§5.3): landscape 3×2 cells of 840×344, portrait 1×6 cells of 696×416, 8 px gutter, 12 px margin. |
| F5 | iCUE keeps browser storage **per widget instance** (`html_widgets_browser_storage`). Widgets key their own `localStorage` by `uniqueId`. | rea findings, local-storage doc | Each instance gets its own WebKit data store. |
| F6 | There are 14 permission kinds with state `pending | approved | blocked`. Path macros are `<Home>`, `<Documents>`, … `<AppData>`, `<Runtime>`. | `libWidgetPermissions`, permissions doc | A permission engine in Rust, enforced through WebKit content rules plus a URL-scheme handler. |
| F7 | iCUE sets up the Edge with a state machine: `touchToStartScreenCalibration`, then `turnOffGesture`, then `screenCalibrated`. It also has orientation, rotation and backlight settings. | `libiCUE` symbols (Ghidra/nm) | Ship a first-run calibration wizard with the same steps. A control channel for brightness and rotation probably exists (to be verified in M0). |
| F8 | iCUE renders device content out of process (`QmlRenderer` over QtRemoteObjects). | `libQmlRenderRpc` | One web process per widget (WKWebView does this already). A widget crash must not kill the host. |
| F9 | iCUE has a local WebSocket API on port 50123 (`api_version` 1.2.0) with endpoints `ping`, `status`, `status_extended`, `supported_endpoints`, `release_device` and `restore_device`. | Ghidra decompile of `initializeEndpoints` | Optional compatibility shim (M5). Unrelated to widgets. |
| F10 | Engine is QtWebEngine 6.9.3 / Chromium 130. `os` is officially `windows` only. `media-selector` video codecs are unsupported on macOS in iCUE. | spec | Treat the `os` field as advisory, flag Chromium-only APIs at import, and use WebKit on macOS. |

---

## 3. Architecture

```text
┌──────────────────────────────── EdgeHost.app (Rust, one process) ────────────────────────────────┐
│                                                                                                   │
│  Menu-bar item ── Configurator window (main display; local web UI in a WKWebView)                  │
│        │                     │                                                                    │
│        ▼                     ▼                                                                    │
│  ┌──────────┐  ┌────────────────────┐  ┌──────────────────┐  ┌────────────────────────────────┐   │
│  │  store   │◄─┤  core / controller │──┤ add-in controller│──┤ providers (in-proc or out-proc)│   │
│  │ (JSON)   │  │ pages, instances,  │  │ registry,        │  │ sensors · media · link · fs ·  │   │
│  └──────────┘  │ settings, perms    │  │ negotiation,     │  │ notifications · fps ·          │   │
│                └─────────┬──────────┘  │ fan-out          │  │ deviceaction · streamdeck(stub)│   │
│                          │             └────────┬─────────┘  │ + 3rd-party add-ins (JSON-RPC) │   │
│                          ▼                      │            └────────────────────────────────┘   │
│  ┌───────────────────────────────────────────────▼──────────────────────────────────────────┐     │
│  │ widget-runtime: per instance → child WKWebView (own data store, own content-rule list)    │     │
│  │   icuew://<instance>/…  scheme handler  ·  WKUserScript shim  ·  IPC bridge  ·  watchdog   │     │
│  └──────────────────────────────────────────┬───────────────────────────────────────────────┘     │
│                                             ▼                                                     │
│  edge-display: borderless NSWindow covering the Edge's CGDisplay bounds; hot-plug; sleep          │
│  edge-touch:   IOHIDManager digitizer → calibration transform → hit-test → inject into webview    │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 3.1 Workspace layout (logical crates)

> **v0.2:** the tree below is the *eventual* shape. v1 starts with five crates:
> `icuew-manifest`, `edge-layout`, `widget-runtime`, `apps/edgehost` and `apps/edgectl`. Everything else
> (`icuew-package`, `edge-perms`, `edge-store`, `plugin-api`, `edge-display`, `edge-touch`, providers,
> `icue-ws-compat`) starts as a module of `apps/edgehost` or `widget-runtime`. A module becomes a
> crate when its interface has been stable for one milestone, or when a second consumer needs it.

```text
icue-rs/
├── Cargo.toml                  # [workspace]
├── crates/
│   ├── icuew-manifest/         # manifest.json, <head> meta parsing, x-icue-groups, translation.json  (pure)
│   ├── icuew-package/          # .icuewidget zip import/export + validator (port of icuewidget-cli rules)  (pure)
│   ├── edge-layout/            # grid model, slots, orientation, pages, placement constraints  (pure)
│   ├── edge-perms/             # permission model, macro resolution, URL/path matching, rule compilation  (pure)
│   ├── edge-store/             # persisted config (versioned serde JSON), atomic writes
│   ├── plugin-api/             # Provider trait, message types, version negotiation  (pure)
│   ├── widget-runtime/         # shim JS generation, bridge protocol, webview lifecycle (wry/objc2)
│   ├── edge-display/           # CoreGraphics display discovery, window placement, hot-plug (macOS)
│   ├── edge-touch/             # IOKit HID digitizer reader, calibration, transforms (macOS)
│   ├── providers/
│   │   ├── sensors-macos/      # SMC, IOHIDEventSystem, IOReport, host stats
│   │   ├── media-macos/        # Now Playing via adapter / AppleScript fallback
│   │   ├── link/               # NSWorkspace open
│   │   ├── filesystem/         # FSEvents (notify crate) + diffing
│   │   ├── notifications-macos/# best-effort count (opt-in)
│   │   ├── fps-macos/          # frontmost process; fps unavailable
│   │   ├── deviceaction/       # no-op on Edge; optional hotkey mapping
│   │   └── streamdeck-stub/    # reports unreachable
│   └── icue-ws-compat/         # optional port-50123 API emulation
├── apps/
│   ├── edgehost/               # the .app binary (tao event loop, menu bar, wiring)
│   └── edgectl/                # CLI: validate/import/list/inspect/run-one/capture-touch
├── ui/configurator/            # static web UI (vanilla TS or Svelte; built to ui/dist, embedded)
├── shim/                       # icue-shim.js source + tests (bundled into widget-runtime via include_str!)
└── tests/corpus/               # widget compatibility corpus (git-submodule or fetched; not vendored)
```

Pure crates (no macOS FFI) carry most of the logic and build and test quickly on any host. The macOS
crates stay thin. Build speed: `lld` (via `-C link-arg=-fuse-ld=lld` in `.cargo/config.toml`),
`sccache` as `RUSTC_WRAPPER`, and a workspace-level `[profile.dev] debug = "line-tables-only"`.

### 3.2 Technology choices

| Concern | Choice | Why / alternative |
|---|---|---|
| Event loop and windows | `tao` | Multi-window and multi-monitor. Same stack as Tauri, without its app model. |
| Webviews | `wry` (WKWebView) as **child webviews** inside the Edge window | Native, low memory, and each WKWebView already runs in its own WebContent process (meets F8). |
| Native APIs | `objc2` + `objc2-app-kit`, `objc2-web-kit`, `objc2-core-graphics`, `io-kit-sys` | Typed bindings for anything wry doesn't expose (data store identifier, content rule lists, display UUIDs). |
| Engine fallback | `RenderEngine` trait. A CEF back end (`cef` crate) is a possible later option. | Use only if the corpus shows WebKit gaps that matter (§6.2). |
| Persistence | serde JSON files, schema-versioned, atomic rename | Small data, easy to read, diff-friendly. SQLite isn't needed. |
| Async | `tokio` (providers, FS watch, add-in IPC); the UI thread stays on tao | Standard. |
| Packaging | `cargo-bundle` or a hand-written `Info.plist`; Developer ID signing and notarization; `SMAppService` login item | Dock app (regular activation policy) plus an `NSStatusItem` menu-bar item (owner decision) |

---

## 4. Widget package and manifest handling (`icuew-manifest`, `icuew-package`)

### 4.1 Import

1. Accept `.icuewidget` (an uncompressed ZIP), `.zip` or a folder. Reject entries that escape the root
   (`..`, absolute paths, symlinks), and enforce size limits (default 64 MB total, 10k entries).
2. Validate with the **same rules as `icuewidget-cli` 0.4.48** (see the API reference §22): required
   files, field regexes, the `supported_devices` set, `interactive` typing, existing `preview_icon` and
   `modules[]`, self-closed void elements in `<head>`, known `data-type` values, and a JSON-valid
   `x-icue-groups`. Report errors and warnings using the CLI's wording, so widget authors see familiar
   messages.
3. Extra EdgeHost checks:
   - `supported_devices` must include `dashboard_lcd`. Otherwise warn "not designed for XENEON EDGE"
     and allow a forced install.
   - Every `required_plugins` entry is checked against the add-in registry (§7.3). Unsupported plugins
     are shown as *Unsupported on macOS: <name>*. The user may still install; the plugin object is then
     present but never initializes.
   - Flag Chromium-only APIs with a static scan of the JS: `queryLocalFonts`, `navigator.hid`,
     `navigator.serial`, `navigator.usb`, `showOpenFilePicker`, `getDisplayMedia` in a non-main frame,
     `CSS.registerProperty`, and others as found. These are warnings, not errors.
   - `os` lists only `windows`: ignore it (advisory).
4. Store the package at `~/Library/Application Support/EdgeHost/widgets/<id>/<version>/`, keeping old
   versions until no instance references them. The install record holds the manifest, a content hash,
   the source and the time.

### 4.2 Parsed model

```rust
pub struct WidgetDef {
    pub manifest: Manifest,               // all manifest keys, unknown keys preserved
    pub title: Expr,                      // <title> text, e.g. tr('Name') or 'Name'
    pub icon: RelPath,
    pub group: Option<Expr>,              // x-icue-widget-group
    pub preview: Option<RelPath>,         // x-icue-widget-preview (128×56)
    pub properties: Vec<PropertyDef>,     // x-icue-property, in document order
    pub groups: Vec<PropertyGroup>,       // x-icue-groups
    pub translations: Translations,       // translation.json (en required as fallback)
}

pub struct PropertyDef {
    pub name: Ident,                      // `content`, also the JS global name
    pub label: Expr,                      // data-label
    pub kind: ControlKind,                // data-type (11 kinds)
    pub attrs: BTreeMap<String, Expr>,    // data-default, data-min, data-values, data-filters, …
}

pub enum ControlKind { Slider, Switch, Textfield, Color, Combobox, SearchCombobox, TabButtons,
                       MediaSelector, SensorsCombobox, SensorsFactory, StreamDeckVirtualDevice }
```

`Expr` is the raw JavaScript expression text. Expressions are **evaluated in a webview, never in Rust**
(§6.4).

### 4.3 Parsing `<head>`

iCUE parses `<head>` as XML (a vendor note says void elements must self-close). Use `html5ever` (via
`scraper`) for leniency, plus a separate **strict-XML check** that reproduces iCUE's import failure
("Missing Title Element") as a warning. That way EdgeHost doesn't reject widgets iCUE accepts, and
authors are told about widgets iCUE would reject.

---

## 5. The Edge surface (`edge-display`, `edge-layout`)

### 5.1 Finding the Edge

- Enumerate displays with `CGGetOnlineDisplayList`. Match on EDID vendor and product from
  `CGDisplayVendorNumber` / `CGDisplayModelNumber` (record the Edge's values in M0) **or** on the
  native mode 2560×720 or 720×2560. Persist the display by `CGDisplayCreateUUIDFromDisplayID` so
  re-plugging reattaches.
- **Manual override (v0.2):** auto-detection is not the only path. `edgectl displays` lists
  displays, and `--display <index|name>` (later a picker in the configurator) pins one, persisted by
  display UUID. This covers hubs that alter the EDID, scaled modes, and rotated panels whose
  `CGDisplayRotation` differs from the physical mounting.
- Hot-plug: use `CGDisplayRegisterReconfigurationCallback`. On removal, hide the surface and suspend
  webviews (`isInspectable` off, timers throttled). On return, restore.
- Orientation comes from `CGDisplayRotation`. A rotation change switches the layout to the portrait
  page set.

### 5.2 Surface window

- A borderless `NSWindow` exactly covering `CGDisplayBounds(edge)`. `collectionBehavior` is
  `canJoinAllSpaces | stationary | ignoresCycle | fullScreenAuxiliary`, and the level is `.normal`
  (configurable to `.floating`).
- Pin the window to the Edge display so Mission Control and Stage Manager don't move it. The app itself
  is a regular Dock app with a menu-bar item. The surface window is excluded from the Window menu
  and Mission Control.
- Optional keep-awake: an `IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep)`
  while the dashboard is active, with schedule-based dimming.
- **Release mode** (iCUE: "is now functioning as a regular monitor"): a menu toggle closes the surface
  and leaves the Edge as a normal monitor.
- Background: solid colour, image or video (a `<video>` in a dedicated background webview at z-index 0).

### 5.3 Grid and slots

Derived from the published slot sizes. Each derivation is exact; confirm against a real Edge in M0.

| Orientation | Panel | Cells | Cell | Gutter | Outer margin |
|---|---|---|---|---|---|
| Landscape | 2560×720 | 3 cols × 2 rows | 840×344 | 8 | 12 |
| Portrait | 720×2560 | 1 col × 6 rows | 696×416 | 8 | 12 |

| Slot | Landscape (cols×rows → px) | Portrait (rows → px) |
|---|---|---|
| S | 1×1 → 840×344 | 1 → 696×416 |
| M | 1×2 → 840×696 | 2 → 696×840 |
| L | 2×2 → 1688×696 | 4 → 696×1688 |
| XL | 3×2 → 2536×696 | 6 → 696×2536 |

Check: 3·840 + 2·8 + 2·12 = 2560 and 2·344 + 8 + 2·12 = 720 (landscape). 6·416 + 5·8 + 2·12 = 2560
(portrait).

`edge-layout` provides:
- a `Page { orientation, placements: Vec<Placement { instance, slot, origin_cell }> }` collision checker,
- an auto-pack helper for drops in the configurator,
- **multiple pages**, switched by a horizontal swipe (landscape) or vertical swipe (portrait) on the
  Edge, a page-dots overlay, an optional auto-rotate timer, and a programmatic `goto(page)`.

The widget viewport equals the slot's pixel size at device-pixel-ratio 1 (the Edge is not Retina).
`@media (aspect-ratio)` rules in widgets then behave as they do in iCUE.

---

## 6. Widget runtime (`widget-runtime`, `shim/`)

### 6.1 One instance, one webview

| Aspect | Implementation |
|---|---|
| View | A child WKWebView in the surface window at the slot rectangle, with a transparent background (`drawsBackground = false`) |
| Origin | A custom scheme `icuew://<instance-uuid>/…` served by a `WKURLSchemeHandler` from the widget's version directory. A unique host per instance gives a unique origin. |
| Storage | `WKWebsiteDataStore(forIdentifier: instance-uuid)` (macOS 14+) gives per-instance `localStorage` and IndexedDB, mirroring iCUE's per-widget browser storage (F5). Deleting the instance deletes the store. |
| Process isolation | A WebContent process per webview. Watch `webViewWebContentProcessDidTerminate` and auto-reload with backoff (max 3 per 5 min), then show a "widget crashed" tile. |
| Frame rate | `iCUE.fpsLimit` (default 30) is advisory. Throttle hidden pages with `setAllMediaPlaybackSuspended` and by pausing rAF (removing the webview from the window while its page isn't visible). |
| Inspector | `isInspectable = true` in developer mode, so Safari Web Inspector can attach. |
| Input | `interactive: false` sets `pointer-events: none` on `<html>` via an injected stylesheet, and taps go to page-swipe handling. `interactive: true` delivers touches to the page. |

### 6.2 Engine compatibility (WebKit vs Chromium 130)

Widgets are tested by their authors on Chromium. Known gaps on WebKit: `queryLocalFonts`, WebHID,
WebSerial, WebUSB, some `getDisplayMedia` cases, the File System Access API, and some CSS features.
Strategy:
1. Static scan at import (§4.1) produces warnings.
2. The compatibility corpus (§11.3) produces a pass/fail matrix per widget.
3. **v0.2: WebKit is the only engine for v1.** Known gaps are documented for widget authors. A second
   engine (CEF) is reconsidered only if the corpus shows it is unavoidable, given its cost in binary
   size, signing, update cadence and licensing.

### 6.3 Injected globals (document start, main frame only)

The Rust side renders `icue-shim.js` with a JSON payload and installs it as a
`WKUserScript(injectionTime: .atDocumentStart, forMainFrameOnly: true)`. Order of operations inside
the shim:

1. `window.iCUE = { iCUELanguage, fpsLimit, isPreview, widgetId, defaultTemperatureUnit() }`
   (`widgetId` is used by the Stream Deck example but isn't on the global-object page; provide it anyway).
2. `window.device = { deviceId }`: a stable UUID **without braces** for the Edge (derived from the
   display UUID).
3. `window.uniqueId = "<instance-uuid>"`.
4. **Property globals.** For each `PropertyDef`, `window[name] = <resolved value>`, taken from the
   instance's stored settings. The values are already concrete JSON (see §6.4), so they are available
   synchronously, before any widget script runs (F2).
5. `window.tr = (key) => Promise.resolve(lookup(key))`, using the instance language and `en` as fallback.
6. `window.plugins = {}`. For each *requested and supported* plugin, the shim creates its JS facade
   (§7.2) and sets `plugin<Module>_initialized = false`.
7. **Modules:** for each `manifest.modules[]`, run `import("icuew://…/<path>")` and assign
   `window[<basename>] = namespace`, so `CitySearch.search` resolves in expressions.
8. When the bridge reports provider readiness, set each `plugin<Module>_initialized = true` and call
   `plugin<Module>Events?.onInitialized?.()`.
9. Set `iCUE_initialized = true`, then call `icueEvents?.onICUEInitialized?.()` and
   `icueEvents?.onDataUpdated?.()`.

Robustness rules (from vendor notes):
- Read `icueEvents` and `plugin<Module>Events` **from `window` at call time**, never cached. Widgets
  assign them later with bare assignment.
- Steps 8 and 9 run after `DOMContentLoaded` **and** after module imports settle, so late-loading
  scripts still see the events. Widgets also check `iCUE_initialized` themselves (late-load guard).
- Settings changed in the configurator reassign the property globals and call `onDataUpdated()`.
  Changes are debounced and coalesced to at most 10 per second.

### 6.4 Evaluating attribute expressions

`data-default`, `data-values`, `data-label`, `data-min`, `data-max` and the other `data-*` attributes
are JavaScript expressions. They may use `tr()`, `iCUE`, plugin **Block** methods and manifest
modules. They are evaluated in an **evaluation context**: a hidden webview for the instance, loaded with
the same shim (steps 1–7) and a blank document. That context:
- computes initial defaults when an instance is created, and stores them as concrete values;
- computes dynamic `data-values` lists and `search-combobox` results while the configurator is open.

Runtime pages therefore never evaluate defaults. They receive concrete values only, which matches F2.

**v0.2 ordering:** the evaluation context waits for every requested provider's **snapshot** before it
evaluates, so `plugins.Sensorsdataprovider.getDefaultSensorIdBlock('temperature')` resolves.
Expressions may use only **blocking** plugin methods and synchronous module exports; async methods are
documented as illegal in attributes.

**Interim (M1):** with no store yet, the shim evaluates defaults in the runtime page at document start
and retries module-dependent ones after module import. This is replaced in M2. The VU Meter sample shows
why: its plugin-based defaults are `undefined` until providers exist.

### 6.5 Bridge protocol

- JS → native: `window.webkit.messageHandlers.edgehost.postMessage({v:1, id, plugin, method, args})`.
- Native → JS: `evaluateJavaScript("__edgehost.dispatch(<json>)")`, batched per frame (16 ms) per webview.
- Message kinds: `call` (async method with `requestId`), `signal` (plugin signal fan-out), `props`
  (property update), `ready` (provider readiness), `perm` (permission state change).
- **Synchronous "Block" methods** cannot round-trip through WKWebView. Each provider publishes a
  **snapshot** (for example the sensor catalogue: ids, types, kinds, names), which the shim injects at
  document start and refreshes with `signal` messages. `getDefaultSensorIdBlock(type, kind)` is then
  answered locally from the snapshot with the documented priority (type+kind, then type, then any, then "").
- Every message is validated against the plugin's declared method table. Unknown methods are dropped
  and logged. Message size is capped at 1 MB, and a call rate limit applies (500/s per instance).

---

## 7. Add-in controller (`plugin-api` and `providers/*`)

### 7.1 Provider contract

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;          // module "widgetbuilder.sensorsdataprovider",
                                                          // name "Sensors", version 1.0, methods, signals,
                                                          // properties, block_methods, required permission kinds
    async fn start(&self, ctx: ProviderCtx) -> Result<()>;           // ctx: emit(signal), set_snapshot(), log
    async fn call(&self, inst: InstanceId, method: &str, args: Vec<Value>) -> Result<Value>;
    async fn subscribe(&self, inst: InstanceId) -> Result<()>;       // per-instance lifecycle
    async fn unsubscribe(&self, inst: InstanceId);
    async fn stop(&self);
}
```

- **Version negotiation.** A manifest string `namespace:Name:X.Y` is satisfied by a provider with the
  same namespace and name and a version ≥ X.Y with the same major. Mismatches are reported at import.
- **Facade generation.** The JS facade in `window.plugins.<Module>` is generated from the descriptor:
  each method becomes `(…args) => bridge.call(...)`, each signal an object with `connect/disconnect`,
  each property a getter over the shim cache with `<prop>Changed` signals. The facade's shape matches
  the documented one exactly; the facade name is the module name minus its namespace, with the first
  letter capitalised (`Sensorsdataprovider`).
- **Fan-out.** One provider instance is shared by every widget instance. Signals are filtered per
  instance where the API is per-widget (Stream Deck `widgetId`) and broadcast otherwise.
- **Third-party add-ins are Rhai scripts (v0.2, owner decision).** *Implemented 2026-10-14; see
  [`addins.md`](addins.md) for what shipped. Differences from this text: the call entry point is
  `fn handle(method, args)` (`call` is reserved in Rhai); each widget instance gets its own engine
  (no fan-out yet); installing an add-in approves its `http` hosts; properties, snapshots,
  `read_file` and block methods are not implemented.* An add-in is a folder
  `~/Library/Application Support/EdgeHost/addins/<id>/` holding `addin.toml` (provider descriptor:
  module, name, version, methods, signals, properties, block methods, required capabilities) and
  `main.rhai`. Each add-in runs in its own `rhai::Engine` on its own thread:
  - **Sandboxed by default.** A script can call only the host functions registered for the
    capabilities it declares and the user approved: for example `http_get` limited to declared hosts,
    `read_file` limited to approved paths, `emit(signal, args…)`, `set_property(name, value)`,
    `set_snapshot(value)`, `log(...)`. Raw process, filesystem and network access are not available.
  - **Bounded.** `Engine::set_max_operations`, `set_max_call_levels`, `set_max_string_size` and
    `set_max_array_size` apply, plus a wall-clock budget per call via `on_progress`. A script that
    exceeds its limits is stopped and restarted with backoff.
  - **Entry points:** `fn init(ctx)`, `fn call(method, args)` and `fn poll()` (on a declared interval,
    for example 1 s for sensor-like data). Results map to the same bridge messages as built-in providers.
  - Custom namespaces such as `edgehost.homelab:Prometheus:1.0` let new widgets use new data; iCUE
    widgets are unaffected.
  - Built-in providers stay in Rust. Rhai is for third-party and experimental providers, and a Rhai
    add-in can **override** a stub (for example a community `fps` add-in).

### 7.2 Built-in providers on macOS

> **v0.2 scope:**
> - **v1:** `link`, `filesystem`, and `sensors` (CPU, GPU and memory load and power; temperatures
>   best-effort via IOHIDEventSystem, which does not depend on the Edge).
> - **v1.5:** `media`.
> - **Unsupported stubs in v1:** `notifications`, `fps`, `deviceaction` and `streamdeck`. Their
>   `_initialized` flags stay `false` and import shows "unsupported on macOS". A Rhai add-in may
>   replace any stub.

| Plugin (manifest string) | macOS back end | Fidelity / notes |
|---|---|---|
| `widgetbuilder.sensorsdataprovider:Sensors:1.0` | **CPU/GPU temperatures** from `IOHIDEventSystemClient` thermal services (Apple Silicon, no root) with an SMC fallback (`AppleSMC` keys `Tp*`, `Tg*`, `TC*`). **Fans** from SMC `F<n>Ac`. **Load** from `host_processor_info` (CPU) and the IOAccelerator `PerformanceStatistics` "Device Utilization %" (GPU). **Memory load** from `host_statistics64`. **Power** from IOReport "Energy Model" (CPU/GPU/ANE W). **Battery** from IOPowerSources. | Sensor ids look like `cpu_temp_0`, `gpu_temp_0`, `fan_0`, `cpu_load_0`, `gpu_load_0`, `memory_load_0`, `cpu_power_0`, `battery_charge_0`. `type`/`kind` use only the documented enums (`temperature`/`cpu-temp`, `load`/`gpu-load`, `power`/`total-power-draw`, …). Polling defaults to 1 Hz (configurable). `sensorValueChanged(id, value)` is emitted with the value as a **string**, as documented. Add-ins can add sensors (for example Corsair AIO data via liquidctl). |
| `widgetbuilder.mediadataprovider:Media:1.1` | Now Playing. Primary: a MediaRemote adapter. Since macOS 15.4, MediaRemote needs an entitlement, so use an out-of-process helper (the `mediaremote-adapter` technique). Fallback: AppleScript or ScriptingBridge for Music and Spotify. | `thumbnail` is a `data:image/png;base64,…` (resized to ≤ 512 px). Play/next/previous go through the same channel. Requires the Automation permission prompt for the fallback. |
| `widgetbuilder.linkprovider:Url:1.0` | `NSWorkspace.open(URL)`, allowing only http, https and mailto | Exact. |
| `widgetbuilder.filesystemprovider:FileSystem:1.0` | `notify` (FSEvents), macro resolution with `{Macro}`, and the **permission check before every watch**. Line diffs come from the `similar` crate. | Signals: `fileChanged`, `fileContentDiff`, `fileAdded`, `fileRemoved`. Paths are absolute. Text files only, capped at 1 MB. Recursive folder watching is **off** until iCUE's behaviour is confirmed (open question). |
| `widgetbuilder.notificationsprovider:Notifications:1.0` | There is no public API for counting other apps' notifications. **Opt-in** reader of the usernoted DB (needs Full Disk Access) or Dock badge counts via the Accessibility API. | Best effort, off by default. `getNotificationCount()` returns 0 when disabled. |
| `widgetbuilder.fpsdataprovider:Fps:1.0` | `currentProcess` is the frontmost app's `localizedName` (NSWorkspace notifications). There is no system-wide FPS source on macOS, so `fpsAvailable = false`. | Partial, as documented: the availability signal tells widgets to hide FPS. |
| `widgetbuilder.deviceactionprovider:DeviceAction:1.0` | The Edge has no dials. `initDevice` succeeds, and `dialTriggered` can optionally be mapped from global hotkeys or a Stream Deck+ dial via an add-in. | Never fires by default (iCUE preview behaves the same). |
| `widgetbuilder.streamdeck:StreamDeck:1.2` | Officially Windows-only. The stub sets `connectionStatus = "Disconnected"` and emits `streamdeckUnreachable(widgetId)` on connect. | It is marked unsupported at import. |

Each provider declares the permission kinds it checks (FileSystem: `file`/`folder`; Stream Deck:
`application`).

### 7.3 Registry and UI

The configurator's **Add-ins** page lists each provider with its status (running, error or disabled),
version, the widgets using it, the macOS permissions it needs (Input Monitoring, Accessibility,
Automation, Full Disk Access) and buttons for those permissions.

---

## 8. Permissions (`edge-perms`)

### 8.1 Model

The model matches iCUE (F6): kinds `file`, `folder`, `url`, `microphone`, `camera`, `speaker`, `screen`,
`clipboard_read_write`, `local_fonts_access`, `self_signed_certificate`, `download` and `application`.
States are `pending | approved | blocked`, stored per *widget id* (not per instance). This follows iCUE,
where grants apply to a widget id.

- Manifest-declared permissions are shown at install for one-time approval.
- Undeclared access triggers a runtime prompt, shown in the configurator and as a toast on the Edge.
  "Allow" persists.
- Users can revoke at any time. Revoked access fails **silently** at the network or filesystem level,
  as in iCUE.

### 8.2 Enforcement

| Kind | Mechanism |
|---|---|
| `url` | A per-widget **`WKContentRuleList`** compiled from the approved domains: block everything, then `ignore-previous-rules` for each approved domain and its subdomains and port. `"any"` allows all non-localhost hosts; localhost needs an explicit port. `data:` and `blob:` network loads are always blocked. Covers fetch, XHR, WebSocket, images, media and scripts. Rules are recompiled on permission change, and the page is reloaded. |
| `file` / `folder` | Widget pages only reach files through `icuew://` (their own package) or providers. The scheme handler also serves approved local paths under `icuew://<inst>/__fs/<macro>/<path>` for `fetch`. `<Macro>` resolution: `<Home>` ~, `<Desktop>`, `<Documents>`, `<Downloads>`, `<Music>`, `<Pictures>`, `<Movies>`, `<Temp>` $TMPDIR, `<AppData>` ~/Library/Application Support/EdgeHost/appdata, `<Runtime>` ~/Library/Application Support. Paths are canonicalized and must stay inside the grant. |
| `microphone`, `camera`, `screen` | `WKUIDelegate` `requestMediaCapturePermissionFor…` checks the state, and the app has `NSMicrophoneUsageDescription` / `NSCameraUsageDescription`. |
| `clipboard_read_write` | Allowed only when approved, enforced with a shim wrapper around `navigator.clipboard`. |
| `self_signed_certificate` | `WKNavigationDelegate` `didReceiveAuthenticationChallenge` accepts server trust only for approved hosts. |
| `download` | `WKDownloadDelegate` writes to `~/Downloads/EdgeHost/<widget>/`. |
| `local_fonts_access`, `speaker` | Recorded but has no effect on WebKit (no API, or always allowed). |
| `application` | Checked by the provider (Stream Deck). |

---

### 8.3 Phasing (v0.2)

| Phase | Kinds | Behaviour |
|---|---|---|
| M3 | `url`, `file`, `folder`, `download` | Fully enforced, with tests. Tests prove `file://`, `data:` and `blob:` loads are blocked, and that the content rule list does not interfere with `icuew://`. |
| M3 | `microphone`, `camera`, `screen` | Denied by default; prompting comes later. |
| v1 | `application`, `local_fonts_access`, `speaker`, `self_signed_certificate`, `clipboard_read_write` | Recorded but not enforced; the user is warned at install. |

## 9. Touch (`edge-touch`)

> **M0 result (2026-10-13):** macOS already delivers Edge taps as clicks on the Edge display (see
> `docs/research/edge-hardware.md`). M4 therefore starts from plain web-view input. Seize mode,
> calibration and synthetic events are needed only for multi-touch gestures.
>
> **v0.2:** this design is **gated on M0**. If macOS already delivers Edge touches as clicks on the
> Edge display, M4 starts with plain `NSEvent` handling and seize mode is not needed. If it does not,
> Input Monitoring becomes a stated requirement for interactive widgets, with honest UI copy. Cursor
> mode (Accessibility) remains a fallback, not the default.

### 9.1 Problem

macOS has no touchscreen support. A USB HID digitizer either acts as an absolute pointer, often mapped
to the **main** display, or is ignored. iCUE's Windows setup exists to fix the same association problem
(F7).

### 9.2 Design

- **Discovery:** `IOHIDManager` matching the Edge's VID/PID (from M0) and usage page `0x0D`
  (Digitizers), usage `0x04` (Touch Screen).
- **Exclusive mode:** open with `kIOHIDOptionsTypeSeizeDevice` so macOS stops moving the cursor. This
  needs the **Input Monitoring** permission (TCC `ListenEvent`).
- **Decoding:** use `IOHIDValue` callbacks per element (Contact Identifier, Tip Switch, X, Y, Contact
  Count, Scan Time) to assemble frames of up to 5 contacts. Avoid raw report parsing, so any
  report-descriptor layout works.
- **Transform:** an affine transform from digitizer logical space to panel pixels, fitted by
  least-squares over calibration points, then composed with the display rotation.
- **Gestures:** tap, long-press, drag, horizontal or vertical swipe (page change when it starts on a
  non-interactive area or uses 2 fingers), and pinch (passed to the page).
- **Delivery, in two modes:**
  1. **Direct (default).** Hit-test the slot, convert to webview coordinates, and dispatch synthetic
     `PointerEvent`s (`pointerType: "touch"`), `TouchEvent`s and a final `click` through the shim
     (`__edgehost.touch(frame)` → `document.elementFromPoint` → `dispatchEvent`). The cursor never moves.
     The caveat is that `event.isTrusted === false`; widgets rarely check it.
  2. **Cursor.** Post `CGEvent` mouse events at the mapped global point (needs Accessibility), then
     restore the cursor. Events are trusted. Use this for widgets that check `isTrusted` or for
     `<select>` and other native controls.
- **Calibration wizard** (mirrors iCUE's states). Step 1, "Touch to start" on the Edge: this confirms
  the digitizer belongs to this display. Step 2, "Turn off gestures": check that no macOS Accessibility
  or pointer-control setting is fighting the device. Step 3, 4-corner plus centre targets: fit and store
  the transform per display UUID and rotation. Step 4, "Calibrated".
- **Fallback with no digitizer access:** if the user declines Input Monitoring and macOS already maps
  touches to the Edge as clicks, use plain `NSEvent`s on the surface window. Detect this automatically
  by checking where the first touch-click lands.

---

## 10. Configurator (`ui/configurator`)

A local web UI in a normal window on the main display, talking to the core over the same bridge
mechanism (different handler name and full privileges).

- **Library:** installed widgets (icon, name, group variants, version, supported/unsupported plugins,
  warnings) and an import button or drop target. Group variants are merged, as iCUE does with
  `x-icue-widget-group`.
- **Layout editor:** a 1:4-scale Edge canvas with page tabs and an orientation toggle. Drag widgets in;
  slot-size buttons S/M/L/XL are limited to sizes that fit.
- **Instance settings:** controls generated from `PropertyDef`s and `x-icue-groups`, covering all 11
  types (`slider`, `switch`, `textfield`, `color`, `combobox`, `search-combobox`, `tab-buttons`,
  `media-selector`, `sensors-combobox`, `sensors-factory`, `stream-deck-virtual-device`).
  - Output encodings follow the API reference §10. For example `media-selector` produces
    `{pathToAsset: "file:///…", scale, positionX, positionY, baseWidth, baseHeight, angle}`. Copy the
    chosen media into the instance's asset directory and expose it through `icuew://<inst>/__assets/…`
    rather than raw `file://`.
  - **Custom Style toggle:** groups containing `textColor`, `accentColor` or `backgroundColor` get a
    toggle. When off, the Edge theme defaults are injected. This is iCUE's Edge-only behaviour.
  - **Live preview:** a preview webview at the slot size, with `iCUE.isPreview = true`.
- **Permissions:** pending prompts, plus per-widget grants with revoke.
- **Add-ins** (§7.3), **Display and Touch** (calibration, orientation, brightness if supported,
  keep-awake, release mode) and **Diagnostics** (per-instance console log capture, provider logs and a
  "copy support bundle" button).

---

## 11. Quality

### 11.1 Unit tests (pure crates, `cargo test`)

- `icuew-manifest`: every manifest rule from icuewidget-cli 0.4.48 (positive and negative fixtures), meta
  parsing including `tr()` in attributes, groups, translations fallback, and the strict-XML head check.
- `icuew-package`: zip-slip, symlinks, size limits, and stored-vs-deflated entries. Round-trip export
  must be byte-compatible with `icuewidget package`: sorted entries, method 0.
- `edge-layout`: the slot table above, collision checks, auto-pack, and the rotation mapping.
- `edge-perms`: macro resolution, path canonicalization escapes, domain/subdomain/port matching, the
  "any" and localhost rules, and the content-rule JSON output (snapshot tests with `insta`).
- `plugin-api`: version negotiation and facade descriptor → JS generation (snapshot tests).
- Providers: logic sits behind small traits (`SmcReader`, `ThermalSource`, `NowPlayingSource`,
  `Clock`, `Fs`) with fake implementations, so diffing, polling and id mapping are tested without
  hardware.

### 11.2 Shim tests (`shim/`)

- `icue-shim.js` is tested with `node --test` + `jsdom`. Cases: injection order, the late-load guard,
  bare-assignment `icueEvents`, module exposure, Block methods answered from the snapshot,
  `asyncResponse` routing by `requestId`, and `onDataUpdated` coalescing.
- A WebKit integration test (macOS CI runner, `#[ignore]` locally) loads fixture widgets into a real
  WKWebView offscreen and asserts on console output and `takeSnapshot` images.

### 11.3 Compatibility corpus

These are fetched, not vendored, because of their licences:
- Corsair Labs runner sample widgets (MIT): AirQuality, Doodle Pad, Robex Tourbillon, SpectrumAnalyzer,
  VU Meter, Flap Clock.
- `inerthel-agi/icue-edge-widgets` (Apache-2.0): 14 widgets.
- `icuewidget init` scaffold output, plus one widget per control type and per plugin, generated from
  the docs.

`edgectl corpus run` installs each widget, opens it at every supported slot size and records three
results: whether `onICUEInitialized` fired, JS errors, and a screenshot. The output is
`tests/corpus/report.md`. The CI gate is no regressions against the last report.

### 11.4 Hardware checks (manual, with an Edge connected)

The touch wizard on HDMI and on USB-C DP-Alt; hot-plug; rotation; sleep and wake; a 24-hour soak (memory
growth under 50 MB, no provider leaks).

---

## 12. Milestones (v0.2)

| M | Deliverable | Exit criteria | Status |
|---|---|---|---|
| **M0 Recon** (½ day with hardware) | `docs/research/edge-hardware.md`: VID/PID, HID report descriptor (`hidutil list`, `ioreg -p IOUSB -l -w0`), EDID ids and `CGDisplay` info, whether macOS maps touch to the Edge, whether `kIOHIDOptionsTypeSeizeDevice` stops the cursor, and whether there is a USB control channel for brightness and rotation. | The file exists. **This gates M4 and Edge-specific display identity only.** | Waiting for hardware |
| **M1 Surface + 1 widget** | `edge-layout`, `icuew-manifest`, `widget-runtime` (shim, `icuew://`, per-instance data store), `edgectl validate` and `run-one` | Corsair Labs samples render at L size | **Done** (6/6 widgets boot in a stand-in window) |
| **M2 Packages, store, layout** | `.icuewidget` import and validation (CLI rules), store, multi-instance pages (config file plus `edgectl` property editing), **evaluation-context webview** producing concrete defaults, display picker, `apps/edgehost` app (Dock and menu bar) running the saved layout | `icuewidget init` scaffold and 5 corpus widgets install, configure through `edgectl`, and persist across restarts | |
| **M3 Add-ins + core providers** | Provider contract, **Rhai add-in host**, `link`, `filesystem`, basic `sensors`, unsupported stubs, permissions phase M3 (§8.3) | Sensor corpus widgets show live data. A denied `url` silently blocks fetch. A sample Rhai add-in provides a custom namespace. | |
| **M4 Touch + configurator** | Touch scoped by M0's findings; configurator UI (library, layout editor, the 11 control types, permissions) | Interactive widgets work by touch. All instance settings are editable in the UI. | |
| **M5 / v1.5** | `media` provider (Now Playing helper), packaging and notarization, login item, port-50123 compatibility (optional), Edge brightness and rotation if M0 found a channel | Corpus pass rate ≥ 90% for widgets whose plugins are supported. 24-hour soak passes. | |

## 13. Open questions and things to verify

1. **Edge USB identity and HID layout.** The PID isn't in Mac iCUE's `devices.json` (M0).
2. **Touch on macOS today.** Does it act as an absolute pointer on the main display, as an absolute
   pointer on the Edge, or nothing?
3. **Control channel.** Are orientation, rotation and backlight set over USB HID or over DDC/CI? If DDC,
   use the IOAVService DDC path (as `m1ddc` does).
4. **`iCUE.widgetId` vs `uniqueId`.** Are they the same value in iCUE? Default: `widgetId` = the
   instance UUID with braces stripped, and `uniqueId` = the same UUID. Confirm on a Windows box if one
   is available.
5. **FileSystem provider recursion and initial snapshot.** This is unspecified in the docs; the default
   is non-recursive with no initial `fileChanged`.
6. **`onUpdateRequested`**, an undocumented handler seen in vendor examples. Call it on visibility
   regain and at most 10 per second.
7. **Engine risk.** If the corpus pass rate on WebKit is below about 80%, schedule the CEF engine
   (§6.2) ahead of M4.
8. **Isolation level for Hopper and Ghidra follow-ups** (decompile of `DashLCDScreenManager::isDashLCDDevice`).
   Optional, as it doesn't block any milestone.
9. ~~Owner preferences~~ **Resolved (v0.2):** EdgeHost; Dock and menu bar; macOS 14 minimum; Rhai add-ins.

---

## 14. Appendix: injected shim (sketch)

```js
// rendered by widget-runtime; PAYLOAD is JSON produced in Rust
(() => {
  const P = /*PAYLOAD*/{};
  const post = (m) => window.webkit.messageHandlers.edgehost.postMessage(m);
  const signal = () => { const hs = new Set();
    return { connect: f => hs.add(f), disconnect: f => hs.delete(f),
             _emit: (...a) => hs.forEach(f => { try { f(...a); } catch (e) { console.error(e); } }) }; };

  window.iCUE = { iCUELanguage: P.lang, fpsLimit: P.fps, isPreview: P.preview, widgetId: P.widgetId,
                  defaultTemperatureUnit: () => P.tempUnit };
  window.device = { deviceId: P.deviceId };
  window.uniqueId = P.instanceId;
  for (const [k, v] of Object.entries(P.props)) window[k] = v;
  window.tr = (k) => Promise.resolve(P.tr[k] ?? k);
  window.iCUE_initialized = false;
  window.plugins = {};

  const facades = {};
  for (const d of P.plugins) {                       // descriptors of supported, requested plugins
    const f = {}; facades[d.facade] = { f, d };
    for (const s of d.signals) f[s] = signal();
    for (const m of d.methods) f[m] = (...args) => post({ v: 1, plugin: d.facade, method: m, args });
    for (const b of d.blockMethods) f[b] = (...args) => d.blockImpl[b](P.snapshots[d.facade], ...args);
    for (const [p, v] of Object.entries(d.props)) { let val = v;
      Object.defineProperty(f, p, { get: () => val, set: x => { val = x; } }); }
    window.plugins[d.facade] = f;
    window[`plugin${d.facade}_initialized`] = false;
  }

  window.__edgehost = {
    dispatch(batch) { for (const m of batch) {
      if (m.k === "signal") facades[m.plugin]?.f[m.name]?._emit(...m.args);
      else if (m.k === "prop") { facades[m.plugin].f[m.name] = m.value;
                                 facades[m.plugin].f[`${m.name}Changed`]?._emit(); }
      else if (m.k === "props") { Object.assign(window, m.values); window.icueEvents?.onDataUpdated?.(); }
      else if (m.k === "ready") { window[`plugin${m.plugin}_initialized`] = true;
                                  window[`plugin${m.plugin}Events`]?.onInitialized?.(); }
    } },
  };

  const boot = async () => {
    for (const p of P.modules) { const ns = await import(p.url); window[p.global] = ns; }
    window.iCUE_initialized = true;
    window.icueEvents?.onICUEInitialized?.();
    window.icueEvents?.onDataUpdated?.();
    post({ v: 1, k: "booted" });
  };
  document.readyState === "loading" ? document.addEventListener("DOMContentLoaded", boot) : boot();
})();
```

(The real `blockImpl` functions are generated from Rust-side descriptors. For the Sensors provider,
`getDefaultSensorIdBlock(snapshot, type, kind)` applies the documented priority order.)
