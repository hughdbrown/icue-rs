# Add-in patterns

Rhai snippets for common shapes. All of them assume the `addin.toml` declares the methods,
signals, `poll_ms` and hosts they use.

## Poll an API, cache, push changes

```rhai
fn init() { this.last = (); }
fn poll() { this.refresh(); }
fn handle(method, args) {
    switch method {
        "current" => { if this.last == () { this.refresh(); } this.last }
    }
}
fn refresh() {
    let v = parse_json(http_get("https://api.example.com/v1/now")).value;
    if v != this.last {                 // emit only on change
        this.last = v;
        emit("changed", [v]);
    }
}
```

## Watch list driven by the widget

`watch` and `unwatch` add and remove keys, and `poll` refreshes each one. This is what
`examples/addins/dev.edgehost.coin-prices` does.

```rhai
fn init() { this.watched = #{}; }
fn handle(method, args) {
    switch method {
        "watch"   => { let k = args[0]; if !this.watched.contains(k) { this.watched[k] = (); this.refresh(k); } }
        "unwatch" => { this.watched.remove(args[0]); }
    }
}
fn poll() { for k in this.watched.keys() { this.refresh(k); } }
fn refresh(k) {
    try {
        let v = fetch(k);
        if this.watched[k] != v { this.watched[k] = v; emit("changed", [k, v]); }
    } catch (e) {
        emit("failed", [k, `${e}`]);    // widgets cannot see throw; give them a signal
    }
}
fn fetch(k) { parse_json(http_get(`https://api.example.com/v1/items/${k}`)).value }
```

## Validate arguments

```rhai
fn key_arg(args) {
    if args.len() == 0 || type_of(args[0]) != "string" { throw "expected a key string"; }
    let k = args[0];
    for c in k.chars() {
        if !((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-') { throw `bad key "${k}"`; }
    }
    k
}
```

Interpolating unchecked widget input into a URL path or query is how one widget gets data it
should not. Validate before you build URLs.

## Return errors as data

```rhai
"lookup" => {
    let r = #{ ok: false };
    try { r = #{ ok: true, value: fetch(args[0]) }; }
    catch (e) { r.error = `${e}`; }
    r
}
```

`try`/`catch` is a statement in Rhai, not an expression: `try { x } catch { y }` evaluates to
`()`, so assign inside it and return afterwards. The widget checks `r && r.ok`. A plain `throw` only
resolves to `undefined`.

## Local service (Home Assistant, Prometheus, a dev server)

```toml
[permissions]
http = ["localhost:8123", "homeassistant.local:8123"]
```

Loopback hosts need an explicit port. There are no custom headers, so APIs that need
`Authorization: Bearer …` (Home Assistant's REST API among them) cannot be called directly. Use an
endpoint that takes the token in the query string if the service offers one, or say that a
built-in provider is needed.

## Spread heavy work across polls

```rhai
fn poll() {
    let end = (this.cursor + 500).min(this.items.len());
    for i in this.cursor..end { this.process(this.items[i]); }
    this.cursor = if end == this.items.len() { 0 } else { end };
}
```

## Timestamps

`now_ms()` gives Unix milliseconds for the widget to format. Rhai's `timestamp()` is only for
measuring elapsed time inside the script, and a timestamp cannot be returned or emitted.

## Provide an iCUE plugin EdgeHost lacks

Use iCUE's module and name exactly, for example
`module = "widgetbuilder.sensorsdataprovider"`, `name = "Sensors"`, `version = "1.0"`. Then
implement the methods and signals iCUE documents, with the same argument and value shapes (iCUE's
Sensors sends sensor values as **strings**). Existing iCUE widgets then work unchanged. Built-in
plugins (File System) cannot be replaced.

## Debugging

- `print(`…`)` writes to the log as `[addin <id>] …`.
- `edgectl addin run <dir> --call m --args '[…]'` shows the result, or the error the widget would
  log.
- A limit violation logs `restarting after a limit violation` and `this` starts over. Check for
  unbounded loops and for strings that keep growing.
