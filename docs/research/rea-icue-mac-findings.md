# REA findings: iCUE 5.52.93 for macOS

Date: 2026-10-13
Target: `/Applications/Corsair iCUE5 Software/iCUE.app` (bundle id `com.corsair.cue.main`, v5.52.93)
Tools: `rea` 6.4.0 (`npm install --global rea-agents`), Ghidra 12.1.4 (`brew install ghidra`, JDK 21),
`nm`/`c++filt`/`lipo`, a UTF-16 string extractor, iCUE's own logs and caches.

Scope: interoperability research on a locally installed copy, aimed at a compatible widget host for
hardware the user owns. Nothing here copies Corsair code or assets. The replica implements the
**public** widget contract (docs.elgato.com) and uses these findings only to confirm architecture and
fill gaps in that contract.

## How to reproduce

```bash
npm install --global rea-agents          # installs `rea`
brew install ghidra                      # pulls openjdk@21
export GHIDRA_INSTALL_DIR=/opt/homebrew/opt/ghidra/libexec
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
rea doctor                               # ghidra + ghidra-java should be "healthy"

APP="/Applications/Corsair iCUE5 Software/iCUE.app"
rea inspect-signature "$APP" --format json
rea inspect-macho "$APP/Contents/Frameworks/modules/libWebSocketApiServer.dylib" --format json

# Ghidra rejects universal binaries; thin first
lipo "$APP/Contents/Frameworks/modules/libWebSocketApiServer.dylib" -thin arm64 -output /tmp/ws.dylib
rea function /tmp/ws.dylib \
  --procedure=__ZN3cue3api4impl24WebSocketApiServerRunner19initializeEndpointsEv \
  --provider ghidra --snapshot /tmp/ws.snapshot --format json
```

Note: `/usr/libexec/java_home` on this machine resolves to an old x86_64 Java 8 plug-in. Set
`JAVA_HOME` explicitly as shown above or `rea doctor` will report `ghidra-java: missing_dependency`.

## 1. Binary and bundle facts (rea inspect-signature / inspect-macho)

| Fact | Value |
|---|---|
| Format | Mach-O universal (x86_64 + arm64) |
| Signing | Developer ID Application: Corsair Memory, Inc. (Y93VXCB8Q5), hardened runtime |
| Entitlements | `automation.apple-events`, `cs.allow-jit`, `cs.disable-library-validation`, `cs.allow-dyld-environment-variables` |
| UI stack | Qt 6 + QML (QtQuick, QtQuickControls2), **QtWebEngine + QtWebChannel**, QtRemoteObjects, QtScxml, QtWebSockets |
| Size | ~1.5 GB. About 150 module dylibs in `Contents/Frameworks/modules`, 53 QML plugins in `Contents/PlugIns/cue_qml_plugins` |
| Architecture | Service-locator modules. Interfaces are named like `Name.area.cue.corsair.com` and resolved at runtime by `cue.core.srvmgr` |
| Device support | Delivered as **downloadable packages** (`dev-<device>`) by "iCUE Package Manager" 2.25.2, keyed by USB VID/PID in `~/Library/Application Support/Corsair/CUE5/cache/devices.json` |

## 2. XENEON EDGE status on macOS

- `devices.json` (the device-to-package catalog) has **no XENEON EDGE entry**. Its only displays are
  `XENEON 32QHD165` (pid 61710), `32QHD240` (pid 3336) and `32UHD144` (pid 3333), all with VID 6940 = 0x1B1C.
  Mac iCUE therefore never downloads an Edge device package.
- Even so, `libiCUE.dylib` contains the Edge ("DashLCD") host classes:
  - `DashLCDScreenManager` (`isDashLCDDevice`, `initializeDashLCDContext`, `onDeviceInserted` and related)
  - `DashLCDSetupController` (`attachToDevice`, `requestRunWizard`)
  - `DashLCDSetupStateMachine`, a compiled SCXML machine with states `touchToStartScreenCalibrationState`,
    `turnOffGestureState` and `screenCalibratedState`. This is the touch-to-display association wizard.
  - `DashLCDDeviceImagesControllerImpl` (`setOrientation`, `subScreenDeviceImagePath`). "Sub-screens" are
    the Edge's layout regions.
  - `cue::dashlcd::ScreensLayoutsListModel` (`screenOrientationChanged`)
  - UI models: `DisplayScreenOrientationFeatureSettingModel`, `DisplayScreenRotationFeatureSettingModel`,
    `DisplayScreenBacklightFeatureSettingModel` and `DisplayScreenConnectionErrorModel`. Orientation,
    rotation and backlight are **device-controlled settings**, which implies a USB control channel
    besides video and touch.
  - The string "%1 is now functioning as a regular monitor" confirms that the release-to-plain-monitor mode exists.
- iCUE's log on this Mac reports these interfaces as **missing an implementation**. They live in the
  Windows-only package(s):
  `DisplayScreenManager.manager.dashlcd`, `DashboardWidgetsStorage.dashboardscreen`,
  `TypeRegistry.types.widgets`, `WidgetImportController`, `WidgetExportController`,
  `WidgetDeleteController`, `WidgetPermissionManager`, `WidgetPermissionNotificationController`,
  `WidgetPluginProviderController.ui.widgets`, `AutoReselector.widgets`, `WidgetCacheCleaner.widgets`,
  `GaugeWidgetsPropertySerializer.widgets` and `ImageInfoPropertySerializer.widgets`.
  - **Implication:** the HTML-widget runtime (script injection, `window.plugins.*` providers) is **not
    present** in the Mac build. A search for `iCUE_initialized`, `icueEvents`, `window.plugins`,
    `widgetbuilder`, `x-icue-property`, `required_plugins` and `qwebchannel` across all non-Qt binaries
    found nothing. The replica's JavaScript contract must come from the public spec plus black-box
    testing against real widgets.

## 3. Widget data model and permissions (present in the Mac build)

- `libWidgetsData`: `cue::widgets::data::{SettingsData, HtmlSettingsData, HtmlPathData(Location)}`. Settings
  are keyed by `(widget_id: QUuid, widget type id: string)`.
- `libWidgetPermissions`: 14 permission kinds, each with `toMap/fromMap/isSame` and state
  `pending | approved | blocked`:

  | Class | Key string |
  |---|---|
  | ApplicationPermissionInfo | `application` (`applicationName`) |
  | CameraPermissionInfo | `camera` |
  | ClipboardReadWritePermissionInfo | `clipboard_read_write` |
  | DownloadPermissionInfo | `download` (`path`) |
  | FilePermissionInfo | `file` (`permissionPath`) |
  | FolderPermissionInfo | `folder` (`permissionPath`) |
  | GeolocationPermissionInfo | `geolocation` |
  | LocalFontsAccessPermissionInfo | `local_fonts_access` |
  | MicrophonePermissionInfo | `microphone` |
  | NotificationPermissionInfo | `notification` |
  | ScreenPermissionInfo | `screen` (`monitorIndex`) |
  | SelfSignedCertificatePermissionInfo | `self_signed_certificate` (`host`, `port`) |
  | SpeakerPermissionInfo | `speaker` |
  | UrlPermissionInfo | `url` (`host`, `port`; `tools::isLocalhost`, `compareHosts`) |

- `PermissionPath` uses symbolic roots: `AppData, Desktop, Documents, Downloads, Home, Movies, Music,
  Pictures, Temp, Runtime`, plus storages `CueLocalWidgetStorage` and `CueUserWidgetStorage`.
- Browser storage is kept per widget under `~/Library/Application Support/Corsair/CUE5/html_widgets_browser_storage`
  (an earlier layout `..._legacy_5_51` is migrated). User data lives in `.../CUE5/dashlcd`.

## 4. Local WebSocket API (`libWebSocketApiServer`, decompiled with rea + Ghidra)

- Logger `cue.api.websocket.server`. It listens on **port 50123** (confirmed in the iCUE log) with a 60 s
  client timeout. Name "iCUE API", `api_version` "1.2.0".
- `WebSocketApiServerRunner::initializeEndpoints()` registers 7 handlers (from Ghidra pseudocode):
  `buildApiVersionReply`, `buildSupportedEndpointsReply`, `buildStatusReply`, `buildPingReply`, and
  `processChangeDeviceControlRequest` twice (flag `IsCueControlled` false, then true). Together with
  the UTF-16 literals these give the endpoints:
  `api_version`, `supported_endpoints`, `status`, `status_extended`, `ping`, `release_device`, `restore_device`.
- Request/response JSON keys: `endpoint`, `direction` (`in/out`), `device_id`, `application`, `status`,
  `error`, `error_details`, `managingDevices`, `version`, `name`. Error codes: `invalid_format`,
  `unsupported_endpoint`, `internal_error`.
- Purpose: lets third-party apps ask iCUE to **release** control of a device and later **restore** it.
  It has nothing to do with widgets, but a replica can emulate it so tools that negotiate with iCUE
  behave correctly.

## 5. Rendering and process model

- `libQmlRenderRpc`: `qmlrender::Process` launches a separate `/QmlRenderer` process over
  QtRemoteObjects (`local:` socket) with `QmlRendererRpcReplica` and `pushComponentName`, `pushModuleName`,
  `pushRenderSize`, `pushFps` and `pushEnabled`. iCUE renders device-screen content (QML widgets for pump
  and keyboard LCDs) **out of process** at a fixed FPS and size. A replica should isolate widget rendering
  the same way: one web process per widget, and a crash must not take down the host.
- Touch: `TouchScreensManager`, `TouchScreenWindow`, `TouchScreenSleepController` and
  `TouchScreenLayoutsProvider` handle iCUE-native (non-HTML) button screens. `TouchScreenUtils` has
  `buttonWidth/Height/Spacing` and `maxButtonsCountOnScreen`.

## 6. Hardware path (from the 2026-10-03 analysis plus this run)

- Video: HDMI 2.0 or USB-C DP Alt Mode, 2560×720 @ 60 Hz. No proprietary display protocol.
- Touch: a separate USB HID digitizer (5-point) that the host OS must associate with the right display.
- Control: orientation, rotation and backlight settings exist in the Edge UI models, so a USB control
  interface probably exists. **Unverified.** Capture it with `ioreg -p IOUSB -l -w0` and
  `hidutil list` while an Edge is connected. No Edge was connected during this run.

## 7. Prior art: Corsair's own stand-alone runners

`github.com/Corsair-Labs/iCUE-widget-runner-{RaspberryPi,windows}` (MIT, cloned to `~/tools/corsair-labs/`)
are Electron apps that load widgets into an `<iframe>`. They inject a `<script>` right after `<head>`
that sets property globals, `iCUE`, `iCUE_initialized`, `icueEvents`, `tr()` (Promise) and a fake
`window.plugins.Sensorsdataprovider` with `asyncResponse` and `sensorValueChanged` objects that use
`connect`/`emit`. This confirms the injection model. The shim is minimal: it hard-codes property
defaults, does not parse `x-icue-property` tags, fakes sensor values and implements no permissions.

## 8. How iCUE recognises an Edge (`DashLCDScreenManager::isDashLCDDevice`)

Hopper via rea was impractical on the 52 MB arm64 `libiCUE`. The first run timed out waiting for the
first-run dialog. After the licence was added, the full auto-analysis ran its bridge script for more
than an hour and was stopped. The function is tiny, so it was disassembled directly instead
(`otool -tV -p <sym>`, then `llvm-objdump -d --start-address`):

```asm
DashLCDScreenManager::isDashLCDDevice(QSharedPointer<CorsairDevice> const&):
  ldr  x0, [x1]
  bl   CorsairDevice::manifest() const
  mov  w1, #0x34            ; key 52
  bl   0x5af60              ; inlined QHash<int,…> lookup on the device manifest
  cmp  w0, #0x1a            ; == 26 ?
  cset w0, eq
```

**Conclusion:** a device is a DashLCD (XENEON EDGE family) when **device-manifest property 52 equals
26**, which is probably a device-category enum. Detection is data-driven: the manifest comes from the
downloaded `dev-*` package, and VID/PID is not hard-coded in `libiCUE`. The Mac catalog has no Edge
package, so the Edge's USB IDs must be captured from hardware (M0).

Lesson: for single small functions in huge binaries, use `otool` or `llvm-objdump` on the symbol. Use
Ghidra or Hopper through rea only when you need pseudocode for larger functions, and thin
(`lipo -thin arm64`) and preferably split the target first.

## 9. Gaps and next probes

1. Edge USB VID/PID and HID interface list (connect the device, then run `ioreg` and `hidutil list`).
2. Whether macOS already maps the Edge digitizer to the Edge display, or to the main display.
3. The exact JS bridge semantics, from the public spec and black-box runs of community widgets:
   QWebChannel-style async methods with callbacks, and `signal.connect(fn)`.
4. Optional, if a Windows machine is available: run `rea inspect-pe-resources` and Ghidra `rea function`
   on the Windows iCUE HTML-widget module, to confirm the injected script and plugin method signatures.
