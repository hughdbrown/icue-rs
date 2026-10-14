# iCUE Widget API Reference (condensed)

Source: https://docs.elgato.com/icue/widgets/ (scraped 2026-10-13/14 via Firecrawl).
**Widget API Version shown: 1.9.0** (banner on getting-started, specification, icue-global-object, device-object, javascript-expressions, common-tools). Specification page also shows: QtWebEngine 6.9.3, Chromium 130.0.6723.192, **Minimum iCUE 5.52**. Getting-started says iCUE 5.47+ for development. Permissions/translations pages show no version banner.

Pages covered (all under /icue/widgets/): `/` , `specification`, `references/{icue-global-object,device-object,controls (+13 sub-pages),permissions,translations,javascript-expressions,common-tools,local-storage,plugins (+8 sub-pages)}`.
Not found on the site: no standalone pages for lifecycle, per-device resolutions/grid sizes, or known limitations (see Gaps at end).

---

## 1. Getting started / CLI / packaging
Source: https://docs.elgato.com/icue/widgets/

- Requirements: iCUE 5.47+, editor, iCUE Widget CLI, a compatible CORSAIR LCD device.
- CLI: npm package `icuewidget-cli` (https://www.npmjs.com/package/icuewidget-cli); `npm install -g icuewidget-cli`; binary `icuewidget`.
- Commands (only three documented):
  - `icuewidget init <Name>` - interactive scaffold; prompts: widget name, author, widget ID (reverse-DNS), description, version. Produces hello-world widget with headline/subtitle/textColor/backgroundColor properties.
  - `icuewidget validate <dir>` - validates structure and manifest.
  - `icuewidget package <dir>` - validates, then creates `.icuewidget` package, prints summary.
- Detailed validation rules / flags / package internals: NOT documented on the site (mark: not retrieved).
- Install in iCUE: widgets section -> `+` button above widget list -> select `.icuewidget` file.
- Widget ID: reverse-DNS; only lowercase `a-z`, `0-9`, `-`, `.`.
- Minimal layout: `manifest.json`, `index.html`, `resources/icon.svg`, optional `translation.json`.
- Supported device types: `dashboard_lcd` (XENEON EDGE), `keyboard_lcd` (VANGUARD 96, VANGUARD PRO 96), `pump_lcd` (iCUE LINK XC7 ELITE LCD, XD5 ELITE LCD).
- Manifest `os` currently only `windows` (even though permissions page lists macOS path macros).

## 2. Specification
Source: https://docs.elgato.com/icue/widgets/specification

### File structure
Required: `index.html`, `manifest.json`, `icon.svg` or `icon.png`. Typical dirs: `modules/` (*.mjs), `scripts/`, `styles/`, `resources/` (icon.png, thumbnail.png), `translation.json`.
Head: metadata, controls, `<title>` (widget name), `<link rel="icon">` (SVG/PNG/ICO). Body + Script.

### manifest.json
| Key | Type | Req | Notes |
|---|---|---|---|
| `author` | string | Y | |
| `id` | string | Y | reverse-DNS |
| `name` | string | Y | marketplace display name |
| `description` | string | Y | |
| `version` | string | Y | semver |
| `preview_icon` | string | Y | path relative to manifest |
| `min_framework_version` | string | Y | min Widget API version e.g. `1.0.0` |
| `os` | object[] | Y | each `{ "platform": "windows" }`; only windows supported |
| `supported_devices` | object[] | Y | `{ "type": ..., "features"?: [...] }` |
| `interactive` | boolean | N | enables click handling on touch devices; default `false` |
| `required_plugins` | string[] | N | format `namespace:Name:version` |
| `modules` | string[] | N | JS module paths |
| `permissions` | object[] | N | see section 5 |
| `min_app_version` | string | Y | min recommended iCUE e.g. `5.47` |

(Note: getting-started example manifest omits `min_app_version`, though spec marks it required.)

`supported_devices[].type`: `dashboard_lcd`, `keyboard_lcd`, `pump_lcd`. `features` values: only `sensor-screen` ("device supports sensor data display"), e.g. `{"type":"dashboard_lcd","features":["sensor-screen"]}`.

Example plugin strings: `"widgetbuilder.sensorsdataprovider:Sensors:1.0"`, `"widgetbuilder.mediadataprovider:Media:1.1"`.

### HTML meta tags
- Required: `<title>` (supports `tr('Name')`), `<link rel="icon">`.
- `<meta name="x-icue-widget-group" content="tr('Clock Face')">` groups widgets as variants in selector (identical names auto-group).
- `<meta name="x-icue-widget-preview" content="resources/preview.png">` - 128x56 px, PNG preferred, path relative to index.html.
- Controls: `<meta name="x-icue-property" content="<varName>" data-label=... data-type=... data-default=...>`; each becomes a global JS variable named by `content`.
- Property groups: `<script type="application/json" id="x-icue-groups">[{ "title": "tr('Settings')", "properties": ["a","b"], "info": "tr('help')" }]</script>`. `title` (req, JS expr ok), `properties` (req, string[] of `content` names), `info` (opt tooltip, JS expr ok).
- XENEON EDGE only: property groups containing `textColor`, `accentColor`, or `backgroundColor` auto-show a "Custom Style" toggle (second tab only); when off, the device default colour scheme is used.

### Runtime injection and events
iCUE injects before widget scripts: one global per meta property, `iCUE` object, `iCUE_initialized` flag, `device` object, `window.plugins` namespace.
```js
icueEvents = { onICUEInitialized: fn, onDataUpdated: fn };
if (iCUE_initialized) { onIcueInitialized(); onIcueDataUpdated(); } // late-load guard
```
- `onICUEInitialized`: once, when API and data ready.
- `onDataUpdated`: every time any meta parameter changes.
- Plugin events: global `plugin{ModuleName}Events` (module name without namespace, first letter capitalised, rest lowercase as in examples), e.g. `pluginSensorsdataproviderEvents = { onInitialized: fn }`; readiness flag `pluginSensorsdataprovider_initialized`. Plugin objects at `window.plugins.Sensorsdataprovider`.

## 3. iCUE global object
Source: https://docs.elgato.com/icue/widgets/references/icue-global-object (API 1.9.0)
| Member | Type | Notes |
|---|---|---|
| `iCUE.iCUELanguage` | string | `"en"`, `"de"`, ... |
| `iCUE.fpsLimit` | number | render FPS limit, default `30` |
| `iCUE.isPreview` | boolean | true in preview/mimic, false on real device |
| `iCUE.defaultTemperatureUnit()` | -> string | `"°C"` or `"°F"` per iCUE settings |

## 4. Device object
Source: https://docs.elgato.com/icue/widgets/references/device-object (API 1.9.0)
- `device.deviceId` : string, UUID without braces. Injected before widget scripts; safe to read before `onICUEInitialized`.
- Pass to plugin methods needing a device id, e.g. `window.plugins.Deviceactionprovider.initDevice(device.deviceId)`.
- No other properties documented (no resolution/type property on this page).

## 5. Permissions
Source: https://docs.elgato.com/icue/widgets/references/permissions

Behaviour: pre-declared permissions in `manifest.json` are shown at install and approved before first start. Undeclared access triggers a runtime Allow/Deny dialog (allow persists). User may grant/revoke any permission at any time in widget settings; denial/revocation makes access fail silently (blocked at network/file-system level) so widgets must catch errors (`fetch`/`WebSocket` errors, empty file reads, check hardware API availability).

Declaration: `"permissions": [ { "type": "file", "path": "<Documents>/my-widget/data.txt" }, { "type": "folder", "path": "..." }, { "type": "url", "domain": "api.example.com", "port": 443 } ]`

| type | fields | notes |
|---|---|---|
| `file` | `path` (req) | read access to one file (fetch/native JS and File System Provider). Supports macros |
| `folder` | `path` (req) | read access to dir, any depth. Supports macros |
| `url` | `domain` (req: hostname/IP w/o protocol, or `"any"`), `port` (opt number) | http/https/ws/wss. Subdomains of declared domain included. `"any"` = all external non-localhost hosts. localhost requires `port`. `data:` and `blob:` always blocked |
| `microphone` | - | Web Audio API |
| `camera` | - | Media Capture API |
| `speaker` | - | audio output |
| `screen` | - | Screen Capture API |
| `clipboard_read_write` | - | system clipboard |
| `local_fonts_access` | - | Local Font Access API |
| `self_signed_certificate` | `host` (req) | allows self-signed TLS host, e.g. `{ "type":"self_signed_certificate","host":"localhost" }` |
| `download` | - | saved to iCUE-managed path |
| `application` | `application` (req string) | checked by plugin, not browser API; case-insensitive name; separate permission per app. Supported: `"Stream Deck"` -> plugin `widgetbuilder.streamdeck:StreamDeck:1.2`. Plugins must disconnect on denial and retry on approval |

Path macros (angle brackets `<Macro>` in permissions; FS-provider `watchPath` uses curly `{Macro}`, same set):
`<Home>`, `<Desktop>`, `<Documents>`, `<Downloads>`, `<Music>`, `<Pictures>`, `<Movies>` (Win: Videos), `<Temp>`, `<AppData>` (Win `C:/Users/<USER>/AppData/Local/Corsair/CUE5`; mac `~/Library/Preferences/com.corsair/CUE5`), `<Runtime>` (Win `C:/Users/<USER>`; mac `~/Library/Application Support`).

## 6. Translations
Source: https://docs.elgato.com/icue/widgets/references/translations
- File `translation.json` next to `index.html`. Format: `{ "<lang>": { "translation": { "Key": "Value" } } }`.
- Languages: `en` (required, fallback), `de`, `es`, `fr`, `it`, `ja`, `ko`, `pt`, `ru`, `zh_CN`, `zh_TW`, `uk`.
- In meta/HTML attributes: `tr('Key')` in `<title>`, `data-label`, group `title`/`info`, `data-values` (e.g. `[{'key':'utc','value':tr('UTC')}]`).
- In JS: `tr()` returns a **Promise** (`await tr("Hello")`, `Promise.all([...])`).

## 7. JavaScript expressions & modules
Source: https://docs.elgato.com/icue/widgets/references/javascript-expressions (API 1.9.0)
- ECMAScript expressions allowed in any `data-*` attribute **except `data-type`**. Literal strings must be quoted inside: `data-default="'#FFFFFF'"`, `data-label="'Brightness'"`; numbers bare: `data-default="50"`.
- Modules: declare in manifest `"modules": ["modules/SystemUtils.mjs"]`; ES6 `export function`; `.mjs` or `.js`; paths relative to the widget HTML. Exports are called via file name without extension: `SystemUtils.getAvailableTimezones()`.
- Plugin functions callable in expressions, **blocking (synchronous) ones only**: e.g. `data-default="plugins.Sensorsdataprovider.getDefaultSensorIdBlock('temperature')"`.

## 8. Common tools
Source: https://docs.elgato.com/icue/widgets/references/common-tools (API 1.9.0)
Copy `common/` folder (or subfolder) from `<iCUE install dir>/widgets/` into widget root (e.g. `common/tools/media_viewer/MediaViewer.js` + `.css`).
`new MediaViewer({ container, onMediaLoaded?, onMediaError?, debug? })`; methods `loadMedia(params)`, `clear()`, `destroy()`. `loadMedia` params: `path`, `baseWidth`, `baseHeight`, `scale` (default 1), `positionX`, `positionY`, `angle`. Container needs `position: relative`, no `overflow:hidden`, no `visibility:hidden`.
Formats: images jpg/jpeg/png/gif/bmp/webp/svg/ico; video mp4/webm/ogg/mov/avi/mkv.
Typical use with media-selector output: `viewer.loadMedia({ path: backgroundImage.pathToAsset, baseWidth: backgroundImage.baseWidth, baseHeight: backgroundImage.baseHeight, scale: ..., positionX: ..., positionY: ..., angle: ... })`.

## 9. Local storage
Source: https://docs.elgato.com/icue/widgets/references/local-storage
- Standard Web Storage (`localStorage`, ~5-10 MB). Each widget instance has a unique id in global variable **`uniqueId`** (QUuid); it is the localStorage **key**, and the value is a JSON string holding that widget's persisted properties.
- Pattern: `JSON.parse(localStorage.getItem(uniqueId) || '{}')`, mutate, `localStorage.setItem(uniqueId, JSON.stringify(obj))`. Example listens to `window.addEventListener("storage", e => e.key === uniqueId ...)`.
- Intended for widget-render data, not core iCUE functionality.

## 10. Controls (meta `x-icue-property`)
Sources: https://docs.elgato.com/icue/widgets/references/controls/ and `/controls/<type>` (API 1.9.0 on index page)

Common required attributes: `name="x-icue-property"` (constant), `content` (JS variable name; unique; alphanumeric + underscore), `data-label` (JS expr ok), `data-type` (no JS expr). All other `data-*` accept JS expressions (string literals need inner quotes: `data-default="'#FFFFFF'"`; numbers bare). Changes fire `onDataUpdated`. Expression scope: standard JS, `iCUE` object, plugins, `tr()`, declared modules.

Control types (11, each has a page): `slider`, `switch`, `textfield`, `color`, `combobox`, `search-combobox`, `tab-buttons`, `media-selector`, `sensors-combobox`, `sensors-factory`, `stream-deck-virtual-device`.

| data-type | Attributes (all JS-expr capable) | Output value (global var) |
|---|---|---|
| `switch` | `data-default` boolean (`true`/`false`) | boolean |
| `textfield` | `data-default` string (e.g. `"'Hello World'"`) | string |
| `tab-buttons` | `data-default` string key; `data-values` `string[]` \| `array<{key,value}>` (index page suggests 2-4 options) | string (selected key) |
| `color` | `data-default` string hex | string hex e.g. `"#FFFFFF"` |
| `slider` | `data-default` number, `data-min`, `data-max`, `data-step`, `data-unit-label` string (e.g. `"'%'"`) | number |
| `combobox` | `data-default` string = initial **key**; `data-values` = `string[]` \| `object` \| `array<{key,value}>` (e.g. `['apple','banana']` or `[{'key':'left','value':tr('Left')}]`) | string (selected key) |
| `search-combobox` | `data-values` function(query)->results (e.g. `CitySearch.search`, from a manifest module); `data-default` function returning default (e.g. `CitySearch.getDefault`); `data-placeholder` string | string (typically provider ID). Reference impl: `<iCUE install>/widgets/Weather/` |
| `media-selector` | `data-filters` string[] e.g. `['*.png','*.jpg','*.gif']` | object or `undefined` if none: `{ pathToAsset: string (file:///... URL), scale: number (1.0), positionX, positionY, baseWidth, baseHeight, angle (degrees) }`. Windows supports AV1, VP8, VP9 video; macOS: video codecs NOT supported. Guard with `typeof backgroundImage !== "undefined"` |
| `sensors-combobox` | `data-default` string sensor ID, e.g. `plugins.Sensorsdataprovider.getDefaultSensorIdBlock('temperature')` | string sensor ID (e.g. `"cpu_temp_0"`). Needs `widgetbuilder.sensorsdataprovider:Sensors:1.0` |
| `sensors-factory` | `data-default` string default sensor ID for new entries | array of `{ sensorId: string, color: string hex }`. Needs Sensors plugin |
| `stream-deck-virtual-device` | none documented | string device UUID e.g. `"37b6e7f4-77a4-44b6-8aa9-2bb4d6007139"`. Needs Stream Deck app running + `widgetbuilder.streamdeck:StreamDeck:1.2` (example manifest uses `min_framework_version` `1.8.0`) |

Notes: control demo manifests on the site list `os` platforms `windows` and `mac`, but the specification says only `windows` is supported at the moment (inconsistent). `sensors-*` demos load `common/plugins/IcueWidgetApiWrapper.js` and `common/plugins/SimpleSensorApiWrapper.js` (copied from iCUE install `common/`). Widget `<title>` in demos is either `tr('Name')` or a quoted literal `'Name'` (expression form).

## 11. Plugins overview
Source: https://docs.elgato.com/icue/widgets/references/plugins/ (API 1.9.0)

| Plugin | module name | plugin name | ver | `required_plugins` string | `window.plugins.X` | ready flag | events object |
|---|---|---|---|---|---|---|---|
| Sensors | `widgetbuilder.sensorsdataprovider` | Sensors | 1.0 | `widgetbuilder.sensorsdataprovider:Sensors:1.0` | `Sensorsdataprovider` | `pluginSensorsdataprovider_initialized` | `pluginSensorsdataproviderEvents` |
| Media | `widgetbuilder.mediadataprovider` | Media | 1.1 | `...:Media:1.1` | `Mediadataprovider` | `pluginMediadataprovider_initialized` | `pluginMediadataproviderEvents` |
| Link | `widgetbuilder.linkprovider` | Url | 1.0 | `...:Url:1.0` | `Linkprovider` | `pluginLinkprovider_initialized` | `pluginLinkproviderEvents` |
| Stream Deck | `widgetbuilder.streamdeck` | StreamDeck | 1.2 | `...:StreamDeck:1.2` | `Streamdeck` | `pluginStreamdeck_initialized` | `pluginStreamdeckEvents` |
| FPS | `widgetbuilder.fpsdataprovider` | Fps | 1.0 | `...:Fps:1.0` | `Fpsdataprovider` | `pluginFpsdataprovider_initialized` | `pluginFpsdataproviderEvents` |
| Device Action | `widgetbuilder.deviceactionprovider` | DeviceAction | 1.0 | `...:DeviceAction:1.0` | `Deviceactionprovider` | `pluginDeviceactionprovider_initialized` | `pluginDeviceactionproviderEvents` |
| File System | `widgetbuilder.filesystemprovider` | FileSystem | 1.0 | `...:FileSystem:1.0` | `Filesystemprovider` | `pluginFilesystemprovider_initialized` | `pluginFilesystemproviderEvents` |
| Notifications | `widgetbuilder.notificationsprovider` | Notifications | 1.0 | `...:Notifications:1.0` | `Notificationsprovider` | `pluginNotificationsprovider_initialized` | `pluginNotificationsproviderEvents` |

Pattern: `plugin<Module>Events = { onInitialized: fn }` then use `window.plugins.<Module>`; guard late load with `if (plugin<Module>_initialized) fn()`. Sync ("Block") methods are usable in `data-*` expressions; async ones are not.

## 12. Plugin: Sensors Data Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/sensors-data-provider
Module `widgetbuilder.sensorsdataprovider`, name `Sensors`, v`1.0`; `window.plugins.Sensorsdataprovider`.
**Async pattern**: all data methods take a unique `requestId` (int) first arg; result arrives on signal `asyncResponse(requestId, value)`.
```js
const p = window.plugins.Sensorsdataprovider;
p.asyncResponse.connect((id, value) => { if (id === myId) { /* use value */ } });
p.getSensorValue(myId, sensorId);
```
Methods (all `requestId:int` first unless noted):
| Method | Response (`value`) |
|---|---|
| `getSensorValue(requestId, sensorId)` | string |
| `getSensorUnits(requestId, sensorId)` | string (e.g. "C", "%", "RPM"; wrapper doc shows "°C") |
| `getSensorName(requestId, sensorId)` | string |
| `getSensorDeviceName(requestId, sensorId)` | string |
| `getSensorType(requestId, sensorId)` | string (sensor type below) |
| `getSensorKind(requestId, sensorId)` | string (sensor kind below) |
| `getAllSensorIds(requestId)` | string[] |
| `sensorIsConnected(requestId, sensorId)` | bool |
| `getDefaultSensorId(requestId, sensorType, preferredSensorKind)` | string ID, or "" if none. Priority: type+kind match, then type only, then any sensor. `preferredSensorKind=""` ignores kind |
| `getDefaultSensorIdBlock(sensorType, preferredSensorKind)` | **synchronous/blocking**, returns string; usable in `data-default` expressions |

Signals: `asyncResponse(requestId:int, value:var)`, `sensorAdded(sensorId:string)`, `sensorRemoved(sensorId)`, `sensorDataChanged(sensorId)`, `sensorValueChanged(sensorId:string, value:string)`, `sensorUnitsChanged(sensorId:string, units:string)`.

`SimpleSensorApiWrapper` (Promise wrapper; copy `common/plugins/` from the docs bundle/iCUE `common` into widget; do NOT reference iCUE install paths from third-party widgets; include `common/plugins/IcueWidgetApiWrapper.js` + `common/plugins/SimpleSensorApiWrapper.js`): `new SimpleSensorApiWrapper(window.plugins.Sensorsdataprovider, timeoutMs=5000)`; methods returning Promises: `getSensorValue(id)`->string, `getSensorUnits(id)`, `getSensorName(id)`, `getSensorDeviceName(id)`, `getSensorType(id)`, `getSensorKind(id)` (strings), `getAllSensorIds()`->string[], `sensorIsConnected(id)`->boolean. (No wrapper for getDefaultSensorId.)

Sensor types (`sensorType` values): `temperature`, `pump`, `fan`, `voltage`, `load`, `led` (deprecated), `cas-latency`, `command-rate`, `cycle-time`, `dram-frequency`, `ras-precharge`, `ras-to-cas-delay`, `current`, `power`, `battery-charge`, `battery-status`, `efficiency`, `fps`, `pin-protect`.
Sensor kinds: `default`, `core`, `package`, `power-in`, `power-out`, `power-3-3`, `power-5`, `power-12`, `total-power-draw`, `voltage-bat`, `voltage-core`, `voltage-in`, `voltage-3-3`, `voltage-5`, `voltage-12`, `voltage-vdd`, `voltage-vddq`, `voltage-vpp`, `current-3-3`, `current-5`, `current-item-12v`, `current-item-12v-2x6`, `current-12v`, `current-item-atx`, `current-item-sata`, `cpu-temp`, `gpu-temp`, `cpu-pump`, `gpu-pump`, `gpu-load`, `pin-protect-status`, `pin-protect-current`, `pin-protect`, `memory-load`, `frame-buffer-load`, `video-engine-load`, `bus-interface-load`, `invalid`.
Sensor ID example format (from controls docs): `"cpu_temp_0"`, `"gpu_temp_0"` (illustrative).

## 13. Plugin: Media Data Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/media-data-provider
Module `widgetbuilder.mediadataprovider`, name `Media`, v`1.1`; `window.plugins.Mediadataprovider`.
Properties: `songName` string, `artist` string, `album` string, `thumbnail` string (base64 PNG data URL `data:image/png;base64,...` or `""`), `isPlaying` bool.
Async methods (take `requestId:int`; result via `asyncResponse(requestId, value)`): `getSongName`, `getArtist`, `getAlbum`, `getThumbnail` (string), `getIsPlaying` (bool).
Sync control methods (no args): `triggerPlayPause()`, `triggerNextTrack()`, `triggerPreviousTrack()`.
Signals: `asyncResponse(requestId:int, value:var)`; property-change (no args documented): `songNameChanged`, `artistChanged`, `albumChanged`, `thumbnailChanged`, `isPlayingChanged`.
Wrapper `SimpleMediaApiWrapper(window.plugins.Mediadataprovider, timeoutMs=5000)` (`common/plugins/IcueWidgetApiWrapper.js` + `SimpleMediaApiWrapper.js`): `getSongName()`, `getArtist()`, `getAlbum()`, `getThumbnail()` -> Promise<string>; `getIsPlaying()` -> Promise<bool>.
```js
Mediadataprovider.songNameChanged.connect(refresh); // plugin.isPlaying readable directly
```

## 14. Plugin: Link Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/link-provider
Module `widgetbuilder.linkprovider`, name `Url`, v`1.0`; `window.plugins.Linkprovider`.
Standard JS link opening (e.g. `window.open`) opens inside the widget web view; this plugin opens the system default browser.
Method: `open(link: string)` - synchronous, no response, no wrapper. Guard: `if (window.plugins && window.plugins.Linkprovider && pluginLinkprovider_initialized)`. No signals/properties documented.

## 15. Plugin: Stream Deck Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/stream-deck
Module `widgetbuilder.streamdeck`, name `StreamDeck`, v`1.2`; `window.plugins.Streamdeck`. **Windows only.** Requires manifest `permissions` entry `{ "type": "application", "application": "Stream Deck" }` (per permissions page; runtime-requested if missing) and typically the `stream-deck-virtual-device` control.
Revision history: 1.1 added signals `virtualDeviceCreated`, `buttonIconUpdated`, `streamdeckUnreachable`, `authenticationRequired`, `authenticationRejected`, `reconnectStreamDeck`, `disconnectStreamDeck`. 1.2: `connectStreamDeck` no longer creates a virtual device on empty `deviceId` (use the control); deleting a widget no longer deletes its virtual device; added `streamdeckAuthenticated`.
Methods (synchronous calls over async RPC; results via signals):
- `connectStreamDeck(widgetId:string, deviceId:string, columns:int, rows:int)` - connects to an existing virtual device; on success emits `virtualDeviceCreated`.
- `reconnectStreamDeck(widgetId)` - async; reuses existing virtual device if it still exists (then `virtualDeviceCreated` NOT re-emitted, `buttonIconUpdated` re-emitted for known buttons once device opened); otherwise creates new device and re-emits `virtualDeviceCreated` with new `deviceId`. No "reconnect failed" signal; uses regular error signals.
- `disconnectStreamDeck(widgetId)` - closes session when last subscriber for `widgetId` disconnects (dashboard + mimic may share one).
- `updateVirtualDeviceSize(widgetId, columns:int, rows:int)` - resize without reconnect.
- `sendKeyPress(widgetId, buttonIndex:int, pressed:bool)` - `buttonIndex = row * columns + column` (row-major, 0-based, top-left=0); `pressed` true=press, false=release.
Property: `connectionStatus` string, one of `"Disconnected"`, `"Connecting"`, `"AuthRequested"`, `"Authenticated"`, `"DeviceOpened"`, `"AuthenticationRejected"`; notify signal `connectionStatusChanged()` (no args; read the property). Only reflects the widget passed to `connectStreamDeck`.
Signals (all `widgetId:string` first): `virtualDeviceCreated(widgetId, deviceId)`, `buttonIconUpdated(widgetId, buttonIndex:int, iconDataUrl:string data: URL)`, `streamdeckAuthenticated(widgetId)`, `streamdeckUnreachable(widgetId)`, `authenticationRequired(widgetId)`, `authenticationRejected(widgetId)`.
`widgetId` comes from `iCUE.widgetId` in the page's example (NOTE: `iCUE.widgetId` is NOT listed on the iCUE Global Object page - discrepancy; verify at runtime). `deviceId` from the `virtualDevice` control value.
Also documents a QML wrapper (`import widgetbuilder.streamdeck`, `StreamDeck {}` element with same methods/signals) - not relevant to HTML widgets.

## 16. Plugin: FPS Data Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/fps-data-provider
Module `widgetbuilder.fpsdataprovider`, name `Fps`, v`1.0`; `window.plugins.Fpsdataprovider`.
Properties: `currentFps` int, `fpsAvailable` bool, `currentProcess` string (foreground process name = executable's file description).
Async methods (`requestId:int`, result via `asyncResponse(requestId, value)`): `getCurrentFps(requestId)` int, `getFpsAvailable(requestId)` bool, `getCurrentProcess(requestId)` string.
Signals: `asyncResponse(requestId:int, value:var)`, `fpsUpdated(fps:int)`, `fpsAvailabilityChanged(available:bool)`, `processChanged(process:string)`.
Wrapper `SimpleFpsApiWrapper(window.plugins.Fpsdataprovider, timeoutMs=5000)` (`common/plugins/SimpleFpsApiWrapper.js` after `IcueWidgetApiWrapper.js`): `getCurrentFps()` Promise<int>, `getFpsAvailable()` Promise<boolean>, `getCurrentProcess()` Promise<string>.

## 17. Plugin: Device Action Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/device-action-provider
Module `widgetbuilder.deviceactionprovider`, name `DeviceAction`, v`1.0`; `window.plugins.Deviceactionprovider`.
- `initDevice(deviceId: string)` - subscribe to dial/key events; call once at plugin init with `device.deviceId`; later calls ignored.
- Signal `dialTriggered(actionType: string, dialIndex: int)` (dialIndex zero-based dial or key). `actionType`: `"press"` (pressed or released) or `"long-press"` (held). **Emitted only on a real device, not in preview.**
```js
pluginDeviceactionproviderEvents = { onInitialized() {
  const p = window.plugins.Deviceactionprovider;
  p.initDevice(device.deviceId); p.dialTriggered.connect((type, idx) => {});
}};
```

## 18. Plugin: File System Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/file-system-provider
Module `widgetbuilder.filesystemprovider`, name `FileSystem`, v`1.0`; `window.plugins.Filesystemprovider`.
Requires both the plugin AND a matching manifest `file`/`folder` permission; without it the path is silently blocked. Path is macro-resolved first, then permission-checked.
Methods: `watchPath(path:string)` (file -> `fileChanged`+`fileContentDiff`; directory -> `fileAdded`/`fileRemoved`), `unwatchPath(path)`, `clearPaths()` (also clears snapshots).
Signals: `fileChanged(path:string, content:string)` (full new content), `fileContentDiff(path, addedLines:string[], removedLines:string[])` (with fileChanged when diff non-empty; not for identical overwrite), `fileAdded(path, content)`, `fileRemoved(path)`. Paths in signals are absolute.
Macros in `watchPath` use **curly braces** `{Home} {Desktop} {Documents} {Downloads} {Music} {Pictures} {Movies} {Temp} {AppData} {Runtime}` (same locations as permissions `<Macro>`). Example: manifest `{ "type":"file","path":"<Documents>/icue-test-watch/watch.txt" }` with `plugin.watchPath("{Documents}/icue-test-watch/watch.txt")`.
Unclear from docs: whether `fileAdded`/`fileChanged` fire for files within sub-directories of a watched folder; whether `fileChanged` fires for existing files inside a watched folder.

## 19. Plugin: Notifications Provider
Source: https://docs.elgato.com/icue/widgets/references/plugins/notifications-provider
Module `widgetbuilder.notificationsprovider`, name `Notifications`, v`1.0`; `window.plugins.Notificationsprovider`.
Method `getNotificationCount()` -> int (incoming system notification count; page lists it as direct return; wrapper gives Promise). Signal `notificationCountChanged()` (no args). Wrapper `SimpleNotificationsApiWrapper(window.plugins.Notificationsprovider, timeoutMs=5000)`: `getNotificationCount()` Promise<int> (`common/plugins/SimpleNotificationsApiWrapper.js`).

## 20. Screen sizes / resolutions / grid
Source: NOT on docs.elgato.com. From CORSAIR Labs skill repo https://github.com/Corsair-Labs/icue-widget-builder (`skills/icue-widget-builder/SKILL.md` section 2.2, `references/security-and-testing-checklists.md`, `references/responsive-layout-scaling.md`, README of 2026-10-06/12) and the CORSAIR article https://www.corsair.com/us/en/explorer/diy-builder/accessories/how-to-create-a-custom-widget-for-the-xeneon-edge/ ("slots range from Small 840x344 horizontal to Extra Large 2536x696 horizontal"). Secondary/vendor-authored but first-party; treat as authoritative for slot sizes.

**XENEON EDGE (`dashboard_lcd`)** - touch, horizontal + vertical orientation, size slots (px, CSS pixels as widget viewport):
| Size | Horizontal | Vertical |
|---|---|---|
| Small | 840x344 | 696x416 |
| Medium | 840x696 | 696x840 |
| Large | 1688x696 | 696x1688 |
| Extra Large | 2536x696 | 696x2536 |
(Total panel resolution/grid-cell model not documented in retrieved sources; slot sizes only.)

**Pump LCD (`pump_lcd`)**, no touch:
- Round (legacy ELITE LCD e.g. H150i/H170i; iCUE LINK XC7 ELITE LCD; XD5 ELITE LCD): 480x480 (keep content inside inscribed circle ~340 px safe zone).
- Smaller rectangular (NAUTILUS II RS LCD, iCUE LINK TITAN II ULTRA 360 LX LCD): 616x224, 616x456, 456x616, 456x304.
- 5" rectangular (iCUE LINK TITAN II 360 RX LCD = iCUE LINK 5" LCD Screen Module): 696x308, 696x624, 696x1256, 624x344, 624x696, 1256x696.
**Keyboard LCD (`keyboard_lcd`)**, no touch: 320x170.

Related rules (from skill repo, vendor guidance, not API):
- `"interactive": true` => iCUE only recognises the widget for `dashboard_lcd`; silently omitted for pump/keyboard even if declared in `supported_devices` (the CLI warns about this).
- Use CSS `@media (aspect-ratio)`/tokens rather than JS layout switching; recommended readability floors: XENEON EDGE Small horizontal secondary text >= 11 px, interactive labels >= 12 px; keyboard primary >= 24 px, secondary >= 13 px.
- Assignment view (settings preview) vs device: `element.clientHeight` can be 0 in assignment view; use `getBoundingClientRect()`. Avoid `scrollHeight` trim loops.

## 21. Lifecycle notes
Source: docs spec page (section 2) plus vendor guidance https://github.com/Corsair-Labs/icue-widget-builder (`references/lifecycle-and-plugins.md`). No dedicated lifecycle page exists on docs.elgato.com.
- Official: `icueEvents = { onICUEInitialized, onDataUpdated }`; check `iCUE_initialized` for late script load; plugin readiness via `plugin<Module>_initialized` / `plugin<Module>Events.onInitialized`. Properties are globals.
- Vendor-observed (not in official docs): assign `icueEvents` with bare assignment (no `var/let/const`); `icueEvents` must be referenced in scripts even with zero controls or iCUE's import validator rejects with "index.html or scripts do not reference icueEvents"; a one-shot `iCUE_initialized` read can intermittently be false even inside iCUE (injection race) - retry ~100 ms x 15 before falling back to browser/mock mode; properties may be sandbox-local, so use a helper that checks `window[name]` then `Function('return typeof name ...')()`; an `onUpdateRequested` handler is mentioned (commented in the example) - undocumented officially, rate-limit to <= 10 updates/s.
- iCUE parses `<head>` as XML on import: every void element in head must self-close (`<meta ... />`, `<link ... />`), else import fails with "Missing Title Element". `<title>` should use `tr('...')` (plain text yields `{}` in the settings panel per vendor note). Invalid `data-type` => "Invalid meta parameter data type".
- Undocumented `<meta name="x-icue-info" content="appStatus" data-type="app-status" application="stream-deck" data-label=... data-default="">` shows companion-app status row (seen only in first-party StreamDeck widget; unofficial).
- Marketplace packages reportedly need `icon.png` + `icon@2x.png`; widget icon should be a white single-colour SVG with transparent background (vendor checklist).

## 22. WidgetBuilder CLI (`icuewidget`)
Sources: https://docs.elgato.com/icue/widgets/ ; repo https://github.com/Corsair-Labs/icuewidget-cli (package.json, README, src-node/validation.js, constants.js, zip.js read directly via raw.githubusercontent.com); npm `icuewidget-cli` (repo package.json version **0.4.48** at time of reading; the community repo notes testing with CLI 0.2.3).
- Node >= 18, zero runtime dependencies, Apache-2.0, ESM. Run via `npx icuewidget-cli <cmd>` or global install. Binary `icuewidget` -> `node-bin/icuewidget.js`.
- Commands (README): `init [path]`, `validate [path]`, `update [path]` (updates manifest requirements e.g. `--min-app-version 5.45`, bumps widget version, regenerates package), `package [path]` (validate + create `.icuewidget`), `--help`, `--version`. Docs site lists only init/validate/package.
- `init` scaffolds: `manifest.json`, `index.html`, `translation.json`, `resources/icon.svg`, `styles/main.css`, `scripts/main.js`; always platform `windows`, device `dashboard_lcd`; defaults `DEFAULT_APP_VERSION=5.45`, `DEFAULT_FRAMEWORK_VERSION=1.0.0`.
- **Package format**: `.icuewidget` is a plain ZIP (stored/uncompressed entries, method 0, UTF-8 names, CRC32) of the widget folder, entries sorted; excludes the output file, any `*.icuewidget`, and any path segment starting with `.` or named `__pycache__`.
- **Validation rules (as implemented)**:
  - Required files: `manifest.json`, `index.html`. Missing `translation.json` -> warning only (if present it must parse as JSON).
  - manifest non-empty strings: `author, id, name, description, version, preview_icon, min_app_version, min_framework_version`.
  - `id` regex `^[A-Za-z0-9.-]+$` (CLI allows uppercase; docs say lowercase only); `version` and `min_framework_version` `^\d+\.\d+\.\d+$`; `min_app_version` `^\d+\.\d+(\.\d+)?$`.
  - `preview_icon` and each `modules[]` path must exist inside the widget dir (no escaping root).
  - `os` non-empty array, `platform` in {`windows`}. `supported_devices` non-empty, `type` in {`dashboard_lcd`,`pump_lcd`,`keyboard_lcd`}; `features` if present must be string[].
  - `interactive` must be boolean; if true with non-`dashboard_lcd` devices -> warning. `required_plugins`, `modules` must be non-empty string arrays. (`permissions` is NOT validated by the CLI source read.)
  - index.html: warnings if no `<title>`, no icon link, or no `icueEvents` reference (in HTML or linked local .js). Error if a void element in `<head>` is not self-closed. Every `x-icue-property` needs `data-type` in the CLI's allowed set: `slider, switch, color, combobox, search-combobox, tab-buttons, textfield, media-selector, sensors-combobox, sensors-factory` - **`stream-deck-virtual-device` is absent from this set in the 0.4.48 source** (documented control, so the CLI may reject it; verify with a real run).
  - `x-icue-groups` must be valid JSON array of objects each with non-empty `properties`.
  - Local `href`/`src` refs must exist; linked `.css` must have balanced delimiters; linked `.js` must parse (`vm.Script`). Remote/`data:`/`#` refs skipped.

## 23. Known limitations / platform facts (collected)
- Manifest `os` supports only `windows` (spec + CLI); docs examples also show `mac`; permissions page documents mac path macros. Stream Deck provider: Windows only. `media-selector` video codecs: Windows AV1/VP8/VP9; macOS none.
- Engine: QtWebEngine 6.9.3 / Chromium 130 (spec page). No remote `<script>`/fonts per vendor security checklist (not enforced by docs); `data:`/`blob:` URL network permission always blocked.
- `dashboard_lcd` is the only device supporting `interactive` widgets and the "Custom Style" toggle behaviour.
- `dialTriggered` and other real-device events do not fire in preview (`iCUE.isPreview === true`).
- Expression context: only synchronous plugin calls (`...Block`) in `data-*`.
- Marketplace: Elgato Marketplace https://marketplace.elgato.com/icue/widgets ; creators portal "on the way" (CORSAIR article).
- Version inconsistencies across sources: min iCUE 5.44 (CORSAIR article, skill repo), 5.47 (getting-started), 5.52 (specification header); framework version 1.9.0 (docs banner), 1.8.0 used in one demo manifest.

## 24. GitHub repos
- CORSAIR Labs org: https://github.com/Corsair-Labs (7 repos as of 2026-10-13; blog "Introducing CORSAIR Labs GitHub" dated 2026-07-30 says 3 at launch).
  - https://github.com/Corsair-Labs/icue-widget-builder - AI-agent skill (Claude Code/Codex/OpenCode): `skills/icue-widget-builder/{SKILL.md, docs/ (mirror of official docs: controls/, plugins/, specification, translations, local-storage...), references/ (html-template, css-template, lifecycle-and-plugins, responsive-layout-scaling, media-backgrounds, widget-meta-parameters, security-and-testing-checklists, widget-creation, common-tools...)}`. Updated 2026-10-12. Docs mirror in this repo does not list the File System / Notifications plugins in its plugin set (docs predate them); the live docs site does.
  - https://github.com/Corsair-Labs/icuewidget-cli - the CLI (JavaScript, Apache-2.0), see section 22.
  - https://github.com/Corsair-Labs/iCUE-widget-runner-windows and https://github.com/Corsair-Labs/iCUE-widget-runner-RaspberryPi - stand-alone runners for (some) iCUE widgets on XENEON EDGE (HTML, MIT); Windows runner permits widgets that interact with external apps/scripts. (READMEs not read.)
  - https://github.com/Corsair-Labs/XENEON-EDGE-Tuner-Console (MIT), https://github.com/Corsair-Labs/ActivityCapture (Python, feeds an Activity Stats widget), https://github.com/Corsair-Labs/Touchscreen-Calibrator-Raspberry-Pi (Python, MIT).
- https://github.com/CorsairOfficial (org "Corsair Memory Inc.") has only the older iCUE SDK repos: `cue-sdk` (C), `cue-sdk-python`, `cue-sdk-node`. No widget content.
- Community corpus: https://github.com/stealthsrc/icue-edge-widgets (redirects/canonical to https://github.com/inerthel-agi/icue-edge-widgets, Apache-2.0, 47 commits, v1.3.0 packages). 14 packages under `dist/icuewidgets/{xeneon-edge,corsair-watercooling}`; sources `widgets/xeneon-edge/<name>/`, `widgets/pump/<name>/`, shared `shared/{widget-runtime.js,xeneon-widget.css,widget-polish.css}`, `scripts/package-icuewidgets.ps1`, node tests. Widgets: iss-horizon, weather-now, github-repo-monitor, world-clock, focus-timer, daily-brief, countdowns, habit-rings, claude-usage, codex-usage, now-playing, spotify, windows-media-pump, cooling-sensor-pump. Five need the external "iCUE Edge Companion" tray app (https://github.com/inerthel-agi/icue-edge-companion). README notes real-world constraints: CLI 0.2.3 warns when `icueEvents` is declared only in external scripts; state kept under `uniqueId`; example property names (e.g. `weatherCity`, `githubToken`); sizes quoted 840x344, 840x696, 1688x696, 2536x696; pump 480x480.
- Other community (from search results, unvisited): https://github.com/marcimastro98/XeneonEdgeWidget; Reddit post of "17 custom widgets" (r/Corsair).

## 25. Gaps / not retrieved
- No dedicated docs.elgato.com pages exist (per site map) for: lifecycle, device sizes/grid, known limitations; sizes come from CORSAIR Labs skill repo + CORSAIR blog (secondary sources).
- CLI exact flags (e.g. `--min-app-version`, output path flags for `package`) beyond README; contents of `cli.js`, `scaffold.js`, `templates.js` not read; actual `.icuewidget` extension handling at import (signing, size limits) unknown.
- `permissions` array is not checked by CLI source read; no documented schema validation limits.
- `iCUE.widgetId` and `uniqueId` globals appear in examples (Stream Deck page, local-storage page) but are not listed on the iCUE Global Object page.
- File System Provider: behaviour for nested directories and initial snapshot unspecified. Notifications provider: payload shape beyond count unspecified.
- `x-icue-interactive` meta referenced in a vendor checklist but not defined anywhere retrieved; interactivity is documented via manifest `interactive`.
- Runner repos' READMEs and the full skill `docs/` mirror (identical-in-intent to the site) were not read in detail; the `common/` bundle (IcueWidgetApiWrapper.js etc.) shipped inside iCUE not inspected.
- Marketplace/creator portal publishing rules: not available.
