# Writing add-ins for EdgeHost

An **add-in** is a data provider written in [Rhai](https://rhai.rs), a small scripting language
embedded in EdgeHost. It gives widgets a new `window.plugins.<Name>` object, with methods they call
and signals they connect to, without rebuilding EdgeHost. Widgets ask for it by name in
`required_plugins`, the same way iCUE widgets ask for iCUE's own plugins.

This is the walkthrough. The precise contract (every key, function, limit and error) is in the
[add-in API reference](addin-api.md).

Use an add-in when a widget needs data the page cannot get by itself: for example, a source that
needs polling shared logic, or an API you want behind a stable plugin interface. For a plain
`fetch()` to a public API, a widget's own `url` permission is simpler (see the
[widget developer guide](widget-developer-guide.md), §5).

The sample pair is a good starting point:
- [`examples/addins/dev.edgehost.coin-prices`](../examples/addins/dev.edgehost.coin-prices): an add-in that
  polls Coinbase spot prices;
- [`examples/widgets/coin-ticker`](../examples/widgets/coin-ticker): a widget that shows them.

## 1. Quick start

```bash
# Check the add-in, then try it without any widget
edgectl addin check examples/addins/dev.edgehost.coin-prices
edgectl addin run   examples/addins/dev.edgehost.coin-prices --call getPrice --args '["BTC-USD"]'
edgectl addin run   examples/addins/dev.edgehost.coin-prices --call watch --args '["ETH-USD"]' --seconds 40

# Install it, then install and place the widget that uses it
edgectl addin install examples/addins/dev.edgehost.coin-prices
edgectl install       examples/widgets/coin-ticker
edgectl layout add    dev.edgehost.coin-ticker --slot M
```

`edgectl addin list` shows installed add-ins and any that cannot be used, with the reason.
`edgectl addin remove <id>` uninstalls one. Widgets pick up a newly installed or changed add-in the
next time their page is shown (change page, or **Reload Layout** in the menu-bar menu).

## 2. The folder

An add-in is a folder named after its id, holding two files. It is installed to
`~/Library/Application Support/EdgeHost/addins/<id>/`.

### `addin.toml`

```toml
id = "dev.edgehost.coin-prices"     # must match the folder name; a-z 0-9 . _ -
module = "edgehost.coinprices"      # with name and version, what widgets require:
name = "CoinPrices"                 #   "edgehost.coinprices:CoinPrices:1.0"
version = "1.0"                     # major.minor
description = "Spot prices from api.coinbase.com"
methods = ["getPrice", "watch", "unwatch"]
signals = ["priceChanged", "priceError"]
poll_ms = 30000                     # optional: run poll() this often (100 ms … 1 h)

[permissions]
http = ["api.coinbase.com"]         # hosts http_get() may reach
```

| Key | Meaning |
|---|---|
| `module`, `name`, `version` | The plugin a widget's `required_plugins` names. A widget asking for `X.Y` gets an add-in with the same major version and a minor version ≥ `Y`. |
| Facade name | The part of `module` after the last `.`, first letter upper-case, rest lower-case: `edgehost.coinprices` → `window.plugins.Coinprices`. It must be letters, digits or `_`. |
| `methods`, `signals` | JavaScript identifiers. Widgets can call only the listed methods; the script can emit only the listed signals. |
| `poll_ms` | Interval for `fn poll()`. Required if the script has `poll`, and not allowed without it. |
| `permissions.http` | Each entry is a host name or IP address, optionally `:port`. It also covers subdomains. Without a port, only the URL scheme's default port is allowed. Loopback hosts (`localhost`, `127.*`, `::1`) need a port. |

Unknown keys are errors, so typos do not go unnoticed.

An add-in cannot provide a plugin EdgeHost has built in (today, `widgetbuilder.filesystemprovider:FileSystem`), and two installed add-ins cannot provide the same module and name.

### `main.rhai`

```rhai
fn init() {                 // optional: start, and every time the widget's page reloads
    this.watched = #{};
}

fn handle(method, args) {   // required if addin.toml lists methods
    switch method {
        "getPrice" => fetch_price(args[0]),
        "watch"    => { this.watched[args[0]] = (); }
    }
}

fn poll() {                 // required if poll_ms is set; runs right after init(), then every poll_ms
    for pair in this.watched.keys() {
        emit("priceChanged", [pair, fetch_price(pair)]);
    }
}

fn fetch_price(pair) {
    let body = parse_json(http_get(`https://api.coinbase.com/v2/prices/${pair}/spot`));
    parse_float(body.data.amount)
}
```

- **State** lives in `this`, an object map that starts empty. Rhai functions cannot see variables
  outside themselves, so keep everything you need between calls in `this`. To call your own helper
  with `this` bound, use method syntax: `this.refresh(pair)`.
- **`handle(method, args)`**: `args` is an array of the widget's arguments. The return value
  resolves the widget's promise. It can be a number, string, bool, `()` (null), array or map.
  `throw "message"` fails the call: the widget's promise resolves to `undefined` (plugin calls never
  reject, as in iCUE) and the page logs `Facade.method: message`. To give the widget an error it can
  act on, return it as data (`#{ error: "…" }`) or emit an error signal.
- The entry point is `handle`, not `call` as in SPEC §7.1, because `call` is a reserved word in Rhai.

## 3. What a script can do

| Function | Does |
|---|---|
| `emit(signal)`, `emit(signal, [args…])` | Deliver a signal listed in `signals` to the widget, which receives `(args…)`. |
| `http_get(url)` | GET an `http`/`https` URL on a host in `permissions.http`. It returns the body as a string. It throws on a refused host, a network error, a status ≥ 400 or a redirect (redirects are not followed). Timeout 10 s, body ≤ 1 MiB. |
| `parse_json(text)` | JSON text → Rhai value (maps, arrays, numbers, strings). |
| `to_json(value)` | Rhai value → JSON text. |
| `now_ms()` | Milliseconds since the Unix epoch. |
| `print(…)`, `debug(…)` | Write a line to EdgeHost's log, prefixed `[addin <id>]`. |

Rhai's standard library is available: strings, arrays, maps, maths, `parse_int`/`parse_float`,
`timestamp()`, `switch` and `try`/`catch`. That is all. Scripts have **no** file, process or other
network access, and `import` and `eval` are disabled.

### Limits

| Limit | Value | When exceeded |
|---|---|---|
| Operations per entry point (an empty `loop {}` reaches it in about 0.1 s) | 5,000,000 | The call fails; the script restarts with a fresh `this` and `init()` runs again. |
| Script time per entry point (not counting time spent waiting in `http_get`) | 2 s | Same. |
| Total time per entry point (counting `http_get` waits) | 20 s | Same. |
| Calls waiting for the script | 16 | Further calls fail at once ("busy"). |
| Call depth / expression depth | 64 / 64 | Same. |
| String size, array length, map size | 1 MiB, 10,000, 10,000 | Same. |
| Widget call waiting for an answer | 30 s | The call fails (resolves to `undefined`); if it had not started, it is skipped. |
| `poll()` after failures | doubles each time, up to 60 s (or `poll_ms` if longer) | Back to `poll_ms` after a success. |
| `main.rhai` size | 1 MiB | Not loaded. |

## 4. How it runs

- Each widget instance that requires the add-in gets **its own** engine on its own thread, with
  its own `this`. Two coin-ticker widgets poll independently. The spec's shared provider instance
  (SPEC §7.1, "Fan-out") is not implemented.
- The script starts when the widget is shown, and `init()` runs again whenever the widget's page
  reloads. It stops when the widget's page goes away (changing page, reloading the layout, removing
  the widget).
- Errors are logged with the add-in id and the full trace. The widget's console gets just the
  message, for example `Coinprices.getPrice: bad pair "A/B"`.
- Installing an add-in counts as approving the hosts it lists. There is no separate approval step or
  configurator page for add-ins yet (SPEC §7.3).

## 5. Using an add-in from a widget

```json
"required_plugins": ["edgehost.coinprices:CoinPrices:1.0"]
```

```js
const prices = window.plugins.Coinprices;            // undefined if the add-in is not installed
prices.priceChanged.connect((pair, price) => draw(pair, price));
prices.watch("BTC-USD");
const now = await prices.getPrice("ETH-USD");        // undefined if the call failed
```

Exactly as with built-in plugins:
- methods return promises that resolve to the result, or to `undefined` on failure;
- `plugin<Facade>_initialized` turns `true` once the plugin is ready;
- `window.plugin<Facade>Events.onInitialized` is called at that point;
- if the add-in is missing, the facade is absent and the flag stays `false`.

`edgectl validate <widget>` reports which add-in, if any, provides each required plugin.

## 6. Developing

- `edgectl addin check <dir>` parses `addin.toml` and compiles `main.rhai`, so syntax errors show
  up with line and column.
- `edgectl addin run <dir|id> [--call METHOD --args JSON] [--seconds N]` runs `init()`, makes one
  call, then prints signals as they arrive. This includes the ones `poll()` emits. Use it to work on
  an add-in before any widget exists.
- `print(…)` from the script shows in that output, and in EdgeHost's log when a widget uses it.
