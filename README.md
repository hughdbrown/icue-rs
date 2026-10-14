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

## Development

```bash
cargo check --all-targets
cargo test --workspace
cargo clippy --all-targets -- -D warnings
node --test crates/widget-runtime/shim/
node --test examples/widgets/portfolio/
```

| Path | Contents |
|---|---|
| `crates/icuew-manifest` | Parse `manifest.json`, `<head>` controls and groups, `translation.json` |
| `crates/edge-layout` | Edge grid, S/M/L/XL slots, display selection |
| `crates/widget-runtime` | Injection shim, `icuew://` asset server, Edge surface and web views |
| `apps/edgectl` | Developer CLI |
| `examples/widgets/hello-edge` | Test widget that displays the injected globals |
| `docs/` | Spec, review, and research notes (reverse-engineering findings, API reference) |
