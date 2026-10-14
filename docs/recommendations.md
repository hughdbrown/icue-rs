# SPEC Review: EdgeHost v0.1 Plan

Date: 2026-10-13
Scope: `docs/SPEC.md` and the current starter implementation in `~/projects/software/icue-rs`

## TL;DR

The plan is technically sound and well-researched, but it is over-scoped for a first release and contains several assumptions about macOS hardware and WebKit that will not hold without hard evidence. The biggest risks are the untested touch stack, the impossibility of truly synchronous plugin calls in WKWebView, and an overly broad built-in-provider surface.

**Recommendation:** shrink the crate/workspace surface, make M0 a hard hardware spike, implement only core providers first, and explicitly defer CEF and the full touch calibration wizard until the basics work.

---

## 1. Strong parts to keep

- The public iCUE Widget API 1.9.0 research (`docs/research/icue-widget-api-reference.md`) is accurate and detailed.
- The reverse-engineering findings are used correctly: they confirm architecture without copying Corsair code or assets.
- The grid derivation in `docs/SPEC.md` §5.3 is internally consistent and reproduces the published slot sizes.
- Serving widgets from a custom `icuew://<instance>/` origin with per-instance `WKWebsiteDataStore` is the right way to match iCUE's per-widget browser storage.
- The starter code already implements a sensible M1 vertical slice: a single widget in a surface or stand-in window.

---

## 2. Areas that will not work / are high-risk

### 2.1 Workspace/crate plan is too granular for v0.1

`docs/SPEC.md` §3.1 proposes ~17 crates before the first usable release:

```text
icuew-manifest, icuew-package, edge-layout, edge-perms, edge-store,
plugin-api, widget-runtime, edge-display, edge-touch, 8 providers,
icue-ws-compat, edgehost app, edgectl, shim, configurator UI
```

That many crates will slow iteration, force unstable interfaces, and make integration painful before the boundaries are proven.

**Suggested change:** start with 4–5 crates and split only after a boundary is stable.

| Current/starter crate | Responsibility |
|---|---|
| `icuew-manifest` (pure) | manifest + head parsing + translations |
| `edge-layout` (pure) | grid, slots, orientation, placement |
| `widget-runtime` | webview + shim + asset serving |
| `edgehost` app crate | window, touch, store, providers, configurator UI |
| `edgectl` | CLI tooling |

Extract `edge-perms`, `edge-store`, `plugin-api`, etc. only after the app crate proves the interface.

---

### 2.2 Touch input assumptions are unverified and fragile

`docs/SPEC.md` §9 assumes:

1. The Edge digitizer is a USB HID device with usage page `0x0D`, usage `0x04`.
2. `IOHIDManagerOpen(..., kIOHIDOptionsTypeSeizeDevice)` will stop macOS from moving the cursor.
3. If the user denies **Input Monitoring**, a fallback using plain `NSEvent` clicks will work.

These depend entirely on hardware behaviour. If macOS already treats the digitizer as an absolute mouse mapped to the main display, seizing is the right fix, but:

- **Input Monitoring** is a high-friction TCC permission; many users will deny it.
- Synthetic `PointerEvent`/`TouchEvent` injected from JS are **not trusted** (`event.isTrusted === false`). Native controls and some widgets will misbehave.
- The proposed "cursor mode" fallback using `CGEvent` requires **Accessibility**, another TCC prompt, and restoring the cursor cleanly is non-trivial.

**Suggested change:** make M0 a hard gate for answering:

- Does macOS map the digitizer to the Edge display, the main display, or ignore it?
- What is the HID report descriptor / usage page / usage?
- Does `kIOHIDOptionsTypeSeizeDevice` actually stop the system cursor?

If macOS already maps touch to the Edge as clicks, the whole touch stack can start as NSEvent-based. If not, make Input Monitoring required for interactive widgets and be honest in the UI.

---

### 2.3 Synchronous "Block" methods cannot be synchronous in WKWebView

iCUE's QWebChannel supports synchronous plugin calls in expression contexts (e.g. `data-default="plugins.Sensorsdataprovider.getDefaultSensorIdBlock('temperature')"`). WKWebView does not. The current starter shim evaluates `data-default` at document start with `new Function("tr", ...)`, before providers or module imports are ready, so any default that calls a plugin will fail.

`docs/SPEC.md` §6.4 describes the correct fix (a hidden evaluation webview), but it must also wait for provider snapshots to be ready before evaluating defaults.

**Suggested change:**

- Runtime webviews must receive only **concrete property values** from Rust; never evaluate expressions in the runtime page.
- Implement the hidden evaluation-context webview that loads the same shim, waits for provider snapshots, then evaluates defaults and dynamic `data-values` lists.
- Document that only **blocking** plugin methods and synchronous module calls are legal in expressions; async methods are not.

---

### 2.4 Permission enforcement has gaps

`docs/SPEC.md` §8 proposes `WKContentRuleList` for `url` enforcement. Content-rule lists are designed for `http`/`https` network loads and may not behave predictably for custom schemes or `blob:`/`data:` navigations. Subdomain/port matching also needs careful WebKit testing.

`file`/`folder` access is supposed to go through `icuew://<inst>/__fs/<macro>/<path>`. Ordinary `<img src="file:///...">` or `fetch("file://...")` should be blocked, but the spec should explicitly test this.

**Suggested change:** phase permissions:

| Phase | Kinds |
|---|---|
| M2/M3 | `url`, `file`, `folder`, `download` — fully enforced |
| M3 | `microphone`, `camera`, `screen` — deny by default, prompt later |
| v1 | `application`, `local_fonts_access`, `speaker`, `self_signed_certificate`, `clipboard_read_write` — recorded but not enforced; warn the user |

---

### 2.5 Built-in provider surface is too ambitious for v1

`docs/SPEC.md` §7.2 plans full implementations for eight providers. Several are problematic on macOS:

| Provider | Risk |
|---|---|
| **Sensors** | Doable but fragile: SMC/IOReport/IOHIDEventSystem vary by Apple Silicon generation and OS release. |
| **Media** | Now Playing via MediaRemote needs an entitlement on macOS 15.4+. The out-of-process helper plan is correct but adds significant work. |
| **Notifications** | No public API for counting other apps' notifications. Reading `usernoted` is brittle and may break with OS updates. |
| **FPS** | System-wide FPS is genuinely unavailable on macOS. This should be a stub. |
| **Device Action / Stream Deck** | Edge has no dials; Stream Deck is Windows-only. These should be stubs or marked unsupported. |

**Suggested change:** v1 should ship only:

- `link` (trivial: `NSWorkspace.open`)
- `filesystem` (FSEvents + permission check)
- A basic `sensors` provider (CPU/memory load; temperatures only if M0 proves reliable)

Mark media as v1.5, and notifications/fps/deviceaction/streamdeck as unsupported in v1.

---

### 2.6 CEF fallback should not be a v1 plan

`docs/SPEC.md` §6.2 says: "If a significant share of real widgets fails, add the CEF engine." Shipping CEF on macOS is a massive undertaking: binary size, signing, update cadence, licensing, and distribution.

**Suggested change:** make WebKit the only engine for v1. Document known incompatibilities (WebHID, WebUSB, File System Access API, etc.) and warn widget authors. Revisit a second engine only if the corpus proves it is unavoidable.

---

### 2.7 Display detection and orientation need hardware truth

The starter code in `crates/widget-runtime/src/host.rs` matches the Edge only by monitor size (`2560×720` or `720×2560`). This will fail if:

- The Edge is behind a hub that reports a different EDID or size.
- macOS applies overscan/underscan or HiDPI scaling.
- The panel is rotated but `CGDisplayRotation` does not reflect physical orientation.

`docs/SPEC.md` §5.1 proposes EDID vendor/product matching, but the Edge's IDs are still unknown.

**Suggested change:**

- M0 must capture `ioreg -p IOUSB -l -w0`, `hidutil list`, `system_profiler SPDisplaysDataType`, and the actual `CGDisplay` info into `docs/research/edge-hardware.md`.
- Add a manual "this is my Edge" picker in the configurator; auto-detection cannot be the only path.

---

### 2.8 `docs/design/prompt.md` conflicts with the product spec

`docs/design/prompt.md` says:

> Make a replica of Corsair application `ICue`...

while `docs/SPEC.md` §1 correctly warns not to use "iCUE" or "CORSAIR" in the product name and calls the project **EdgeHost**.

**Suggested change:** rewrite or delete `docs/design/prompt.md` so it aligns with EdgeHost naming and goals.

---

### 2.9 Missing project entry points

The project root has no `README.md` and no `AGENTS.md`. The current `Cargo.toml` has a good workspace structure, but a new contributor or agent has nowhere to start.

**Suggested change:** add a short `README.md` covering:

- What EdgeHost is and is not (no iCUE/CORSAIR trademarks, no RGB control, no Marketplace)
- Build/run instructions (`cargo run --bin edgectl -- validate …`, `cargo run --bin edgectl -- run-one …`)
- Minimum macOS version (Sonoma 14+, needed for `WKWebsiteDataStore(forIdentifier:)`)
- Entitlements needed: Input Monitoring, Accessibility, Automation
- Legal/naming note

---

## 3. Recommended milestone changes

| Current milestone | Suggested change |
|---|---|
| **M0 Recon** | Keep as a hard gate. Do not start implementation until `docs/research/edge-hardware.md` exists with VID/PID, HID report descriptor, EDID IDs, and touch behaviour. |
| **M1 Surface + 1 widget** | Keep. Current starter code is already close. |
| **M2 Packages + layout + configurator** | Split: M2 = package import, layout model, store, and one static widget per slot. Move the full configurator UI to M4. |
| **M3 Add-ins + permissions** | Reduce scope to link, filesystem, and basic sensors only. Permissions limited to `url`/`file`/`folder`/`download`. |
| **M4 Touch** | Add the calibration wizard and touch delivery only after M0 proves the digitizer needs it. If macOS already maps touch, this milestone shrinks dramatically. |
| **M5 Polish** | Move notifications/FPS/deviceaction/streamdeck here as unsupported stubs. Drop CEF unless the corpus forces it. |

---

## 4. Concrete next steps

1. Rewrite `docs/design/prompt.md` to align with EdgeHost naming and scope.
2. Add a `README.md` with build/run instructions and the legal note.
3. Do not add more crates until the four existing/starter crates are integrated.
4. Move expression evaluation out of the runtime shim and into a dedicated evaluation-context webview that waits for providers.
5. Mark notifications, FPS, deviceaction, and streamdeck as unsupported in v1.
6. Defer CEF; make WebKit the only engine.
7. Complete M0 with the Edge connected before designing the touch stack.
