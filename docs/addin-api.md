# EdgeHost add-in API reference

Reference for **Rhai add-ins**, the third-party data providers behind `window.plugins`. For a
walkthrough, read [`addins.md`](addins.md) first. This document is the contract: what EdgeHost
accepts, calls, provides and guarantees. Statements here are checked against the implementation
(`crates/edge-plugins/src/addin.rs`, `rhai_provider.rs`, `widget-runtime/shim/icue-shim.js`).

Rhai version: 1.26. Status: implemented 2026-10-14 (SPEC §7.1); §14 lists what is not.

## 1. Terms

| Term | Meaning |
|---|---|
| **Add-in** | A folder with `addin.toml` and `main.rhai` that provides one plugin. |
| **Plugin** | What a widget names in `required_plugins`: `module:Name:Major.Minor`. |
| **Facade** | The object a widget gets: `window.plugins.<Facade>`, with one function per method and one `{connect, disconnect}` per signal. |
| **Widget instance** | One placement of a widget on a page. Each instance that requires the add-in gets its own running copy of the script. |
| **Entry point** | A script function EdgeHost calls: `init`, `handle` or `poll`. |
| **Host function** | A function EdgeHost adds to the script's language: `emit`, `http_get`, `parse_json`, `to_json`, `now_ms`. |

## 2. Package

```
<id>/
├── addin.toml   descriptor (TOML)
└── main.rhai    script (UTF-8, ≤ 1 MiB)
```

Other files are copied on install and ignored. Hidden files and folders are skipped; symlinks are
refused. Installed location: `<data folder>/addins/<id>/`. The data folder is
`~/Library/Application Support/EdgeHost` on macOS, or `$EDGEHOST_HOME` / `--home`.

### 2.1 `addin.toml`

Unknown keys, at any level, are errors.

| Key | Type | Required | Rule |
|---|---|---|---|
| `id` | string | yes | `a-z`, `0-9`, `.`, `_`, `-`; not starting with `.`. Must equal the installed folder name. Reverse-DNS style is recommended (`dev.you.weather`). |
| `module` | string | yes | Non-empty, no `:`. Its last `.`-separated segment names the facade (§3) and must be letters, digits or `_`. |
| `name` | string | yes | Non-empty, no `:`. |
| `version` | string | yes | `Major.Minor`, both non-negative integers (`"1.0"`, `"2.13"`). |
| `description` | string | no | Free text shown by `edgectl addin list`. |
| `methods` | array of strings | no | JavaScript identifiers (`[A-Za-z_$][A-Za-z0-9_$]*`), no duplicates. Non-empty ⇒ the script must define `handle(method, args)`. |
| `signals` | array of strings | no | Same identifier rule, no duplicates. |
| `poll_ms` | integer | no | 100 … 3,600,000. Set ⇔ the script defines `poll()`: either one without the other is an error. |
| `permissions.http` | array of strings | no | Each `host` or `host:port`. Host: letters, digits, `.`, `-`, not starting or ending with `.` or `-`, case-insensitive. Loopback hosts (`localhost`, `127.*`, `::1`) need a port. |

### 2.2 Load errors

`Addin::load` reports the first problem, prefixed with the file name, for example
`addin.toml: methods entry "get-price" is not a JavaScript identifier` or
`main.rhai: Syntax error: … (line 4, position 9)`. The script is compiled at load, so syntax errors
appear at `check` and `install`, not first in a widget.

At registry level (`edgectl addin list`, and every widget mount), a folder is **unusable** when:
- it fails to load;
- its `id` differs from its folder name;
- it provides a built-in plugin's module and name (any version);
- an earlier folder, in name order, already provides the same module and name.

## 3. Plugin identity and matching

- An add-in **provides** `module:name:version`.
- A widget's `required_plugins` entry `M:N:a.b` is satisfied when module and name are equal, the major
  version is equal, and the add-in's minor version ≥ `b`.
- **Built-ins win.** If EdgeHost has a built-in provider with that module and name (today only
  `widgetbuilder.filesystemprovider:FileSystem`), add-ins are never consulted for it.
- **Facade name** = the part of the *widget's* module after its last `.`, first letter upper-cased,
  the rest lower-cased: `edgehost.coinprices` → `Coinprices`, `dev.you.HomeLab` → `Homelab`.
  The globals derived from it are `window.plugins.Coinprices`, `pluginCoinprices_initialized` and
  `pluginCoinpricesEvents`.

## 4. Script contract

All entry points are optional except as §2.1 requires. Each runs with `this` bound to the
instance's state map.

| Entry point | Called | Arguments | Return value |
|---|---|---|---|
| *(top level)* | Each start and restart, before `init` | — | Ignored. Top-level variables are **not** visible to functions. |
| `fn init()` | Each start and restart (§7) | — | Ignored. |
| `fn handle(method, args)` | A widget called `window.plugins.<Facade>.<method>(…args)` with a declared method | `method`: string; `args`: array of the JS arguments, mapped per §6 | Mapped per §6 and resolves the widget's promise. A function that ends without a value returns `()` → `null`. |
| `fn poll()` | Right after `init`, then every `poll_ms` after the previous poll finished (longer after failures, §8) | — | Ignored. |

- **State.** `this` is an object map that starts as `#{}` on every (re)start. Keep all state in it:
  Rhai functions cannot see variables outside themselves.
- **Helpers.** Define ordinary functions freely. Call `helper(x)` for a pure helper, and
  `this.helper(x)` to bind `this` inside it.
- **Errors.** `throw value` or any runtime error ends the entry point. For `handle` it fails the
  widget's call (§9). For `poll` it counts as a failed poll (§8). For `init` it is logged and the
  script carries on.
- `try { … } catch (e) { … }` is a statement and evaluates to `()`. To return a value from it, assign a
  variable inside the blocks and return that variable afterwards.
- The name is `handle`, not `call` (SPEC §7.1), because `call` is a reserved keyword in Rhai.

## 5. Host functions

| Signature | Behaviour | Fails with |
|---|---|---|
| `emit(signal: string)` | Delivers `signal` with no arguments. | `emit: "x" is not in addin.toml's signals` |
| `emit(signal: string, args: array)` | Delivers `signal`; each widget handler is called as `handler(...args)`, with `args` mapped per §6. | The same, or a value that cannot be mapped to JSON (for example a function pointer). |
| `http_get(url: string) -> string` | `GET` an `http://` or `https://` URL whose host is covered by `permissions.http` (§11). Returns the body decoded as text. Gzip is accepted; system root certificates; `User-Agent: EdgeHost/<version>`. | `"…" is not an http(s) URL`, `host:port is not in the add-in's permissions.http`, network or TLS errors, `HTTP <status> from <url>` for status ≥ 400, a redirect message for 3xx (§11), and body-too-large. All messages are prefixed `http_get: `. Timeout 10 s; body ≤ 1 MiB. |
| `parse_json(text: string) -> Dynamic` | JSON text to a Rhai value (§6). | `parse_json: <serde error>` |
| `to_json(value) -> string` | A Rhai value to compact JSON text. | A value that cannot be mapped. |
| `now_ms() -> int` | Milliseconds since the Unix epoch. | — |
| `print(…)`, `debug(…)` | One line in EdgeHost's log: `[addin <id>] …` (`debug` adds the source position). | — |

Rhai's standard packages are available, including strings, arrays, maps, maths, `parse_int`,
`parse_float`, `type_of`, `timestamp()`, `switch`, closures and `try`/`catch`. **Not available:** `import` (modules
resolve to nothing), `eval` (a syntax error), and any file, process, environment or socket access
beyond `http_get`.

## 6. Value mapping

The widget's arguments travel as JSON, and results and signal arguments come back as JSON. Rhai
values map as follows (verified with `edgectl addin run`):

| JS / JSON → | Rhai (`type_of`) | → JSON / JS |
|---|---|---|
| integer within i64 | `i64` | number |
| integer beyond i64 (e.g. 2⁶⁴−1) | `f64` (precision lost) | number |
| fractional number | `f64` | number |
| string | `string` | string |
| `true`/`false` | `bool` | boolean |
| `null`, and `undefined` in an array (`JSON.stringify` makes it `null`) | `()` | `null` |
| array | `array` | array |
| object | `map` | object |
| — | `char` (`'a'`) | string |
| — | timestamp, function pointer, custom types | not mappable: the call fails |

JavaScript drops trailing `undefined` arguments only if the widget omits them. `f(undefined)` arrives
as `[()]`.

## 7. Lifecycle

```
widget shown ──► thread + engine created ──► top level, init() ──► poll() now, then every poll_ms
                                                   ▲
widget page loads or reloads (reset) ──────────────┤  fresh this = #{}
limit violation in any entry point ────────────────┘  (logged "restarting after a limit violation")
widget page goes away ──► provider dropped ──► thread ends after its current step
```

- A script starts when its widget is mounted: on the shown page, at app start, on page change, on
  layout reload, or in `edgectl run-one`. The page's first plugin request (`reset`) restarts it once
  more, so `init` normally runs twice in quick succession. `init` must therefore be idempotent and
  cheap.
- Signals emitted before the page has connected handlers are queued (§10) and delivered once the
  page collects them. Those emitted before the page's `reset` are discarded.
- The add-in registry is re-read at every mount of a widget that has `required_plugins`, so
  installing, replacing or removing an add-in takes effect on that widget's next mount.
- Teardown happens when the page goes away. Because an open signal request may hold the provider
  until it times out, a script can run on for up to about 25 s after its widget is gone.

## 8. Concurrency and timing

- **One engine and thread per widget instance.** Instances share nothing, and there is no fan-out
  between widgets (SPEC §7.1's shared provider is not implemented).
- Within an instance, entry points run **one at a time** on the script's thread, so there are no data
  races on `this`.
- From a single page, calls are also **serialized in call order**: the shim chains them, so
  `clear()` then `add(x)` cannot be reordered.
- A slow `http_get` blocks that instance's other calls and polls until it finishes: up to 10 s per
  request, and 20 s for the whole entry point.
- **Poll schedule.** The next poll runs `poll_ms` after the previous one *finished*. After `n`
  failures in a row the wait is `poll_ms × 2ⁿ`, capped at `max(60 s, poll_ms)`. A success resets it.

## 9. Limits and failures

| Limit | Value | On violation |
|---|---|---|
| Operations per entry point | 5,000,000 | `Too many operations`; the script restarts (§7). |
| Script time per entry point, excluding `http_get` waits | 2 s | `script time budget exceeded`; restart. |
| Total time per entry point, including `http_get` waits | 20 s | `time limit for this call reached` (from the running `http_get`, which gets only the time left, or from the script); restart. |
| Function call depth | 64 | Fails with a stack-overflow error; restart. |
| Expression depth | 64 (32 inside functions) | Compile error at load. |
| String length / array length / map size | 1 MiB / 10,000 / 10,000 | `Length of string too large` (and the array and map equivalents); restart. |
| Wait for a `handle` answer | 30 s | `add-in <id> did not answer in time`. A call still queued at that point is skipped, not run late (logged `skipped <method>(): its caller stopped waiting`). |
| Calls waiting for the script | 16 per widget instance | Further calls fail at once: `add-in <id> is busy (16 calls waiting); try again later`. |
| `main.rhai` size | 1 MiB | Not loaded. |
| Signals queued for a page | 1,000 per widget instance | Oldest dropped. |

The operation limit is usually the one hit: a loop iteration costs a few operations, so an empty
`loop {}` stops after about 0.1 s, and even `for i in 0..3000000` is too long. Split large jobs
across polls instead. The time budget catches slow built-ins, such as huge string operations.

What the **widget** sees when a call fails (any of the above, a `throw`, an undeclared method, or a
stopped add-in):
- The promise **resolves to `undefined`**. It does not reject: iCUE plugin methods return nothing,
  and the shim keeps that contract.
- The page logs a warning `<Facade>.<method>: <message>`, which also appears in EdgeHost's log as
  `[<widget id>] warn: …`.
- The message is the innermost error without Rhai's call-stack wrapping: `throw "bad pair"` gives
  `bad pair`, and an `http_get` failure gives `http_get: HTTP 404 from …`. EdgeHost's log keeps the
  full trace under `[addin <id>]`.
- Calling a method missing from `methods` fails with `unknown method <name>`, without reaching the
  script.

To report errors a widget can act on, return them as data (`#{ error: "…" }`) or emit an error
signal, as the sample does with `priceError`.

## 10. Widget-side API

```json
"required_plugins": ["edgehost.coinprices:CoinPrices:1.0"]
```

| Global | When the add-in is installed and matches | Otherwise |
|---|---|---|
| `window.plugins.<Facade>` | Facade object | absent |
| `plugin<Facade>_initialized` | `false`, then `true` once the bridge is ready | `false` |
| `plugin<Facade>Events.onInitialized()` | Called once ready (define it before ready) | never called |
| Console | — | `plugin <Facade> is not available in EdgeHost` (warning) |

- **Methods:** `facade.method(...args) → Promise<value | undefined>`. Arguments must be
  JSON-serializable.
- **Signals:** `facade.signal.connect(fn)` and `facade.signal.disconnect(fn)`. A handler that
  throws is logged and does not affect other handlers.
- **Ordering:** plugin flags are set, and `onInitialized` runs, before
  `icueEvents.onICUEInitialized`. Calls made earlier are queued until the bridge is ready.
- `edgectl validate <widget>` prints, for each required plugin, `built in`,
  `add-in <id>` or `not available in EdgeHost`.

## 11. Security model

- An add-in can reach only what §5 lists. The network is limited to `GET` on granted hosts. A grant
  `api.example.com` covers that host and its subdomains (`eu.api.example.com`) on the URL scheme's
  default port. `host:port` covers exactly that port. Loopback needs an explicit port.
- **This machine is reachable only by name.** Only a URL whose host is written `localhost`, `127.*` or
  `::1`, granted with its port, may connect to this Mac. Any other host that resolves to a loopback
  or unspecified address (`0.0.0.0`, `2130706433`, a DNS name pointing at `127.0.0.1`) is refused when
  connecting, with `… resolves to this machine; …`. Hosts on the local network are not affected.
- **Redirects are not followed.** A 3xx response fails with
  `HTTP 302 from <url> redirects to <location>; redirects are not followed, …`, so a granted host
  cannot send the script to an undeclared one. Request the final URL instead.
- **Approval** is the act of installing. `edgectl addin install` and `check` print the hosts. There
  is no per-host prompt yet (SPEC §7.3).
- Each widget instance's script is isolated from other instances. Widget pages cannot reach
  add-ins they do not require.

## 12. Bridge protocol (internal)

For debugging only. Widgets use the facade, not these routes. All are `GET` on the widget's own
origin (`icuew://<instance>/`) and answer JSON.

| Route | Answer |
|---|---|
| `/__edgehost/plugin/reset` | `{"seq": n}`: restart the page's providers, drop queued signals, then collect after `n`. |
| `/__edgehost/plugin/call?p=<Facade>&m=<method>&a=<JSON array>` | `{"value": …}` or `{"error": "…"}` |
| `/__edgehost/plugin/events?after=<n>` | `[{"seq", "plugin", "signal", "args"}, …]`; held open up to 25 s until something arrives. |

## 13. `edgectl addin`

| Command | Does |
|---|---|
| `check <dir>` | Load and compile; print module, facade, methods, signals, poll and hosts. Exits non-zero on error. |
| `install <dir>` | Validate, then copy to `addins/<id>/`, replacing a copy with the same id. Refuses built-in plugins and a second provider of the same plugin. |
| `list` | Installed add-ins, and unusable folders with the reason. |
| `remove <id>` | Delete `addins/<id>/`. |
| `run <dir \| id> [--call M] [--args JSON] [--seconds N]` | Start the script outside EdgeHost: top level and `init`, optionally one call (prints `M → value`; on failure prints the error and exits non-zero), then print signals with timestamps for `N` s (default 0 with `--call`, else 10). Polls run on schedule. |

All take `--home <dir>` (or `$EDGEHOST_HOME`) to choose the data folder.

## 14. Not implemented

Compared with SPEC §7.1 and §7.3:
- plugin properties and `set_property`;
- `set_snapshot` and block methods;
- `read_file`;
- a shared provider instance across widgets (fan-out);
- an approval UI;
- the configurator's Add-ins page;
- add-ins overriding built-in plugins.

A Rhai add-in can provide any plugin EdgeHost lacks, including iCUE ones such as
`widgetbuilder.sensorsdataprovider:Sensors:1.0`, as long as it implements the methods and signals
iCUE widgets expect.
