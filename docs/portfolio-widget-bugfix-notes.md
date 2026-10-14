# Portfolio Widget Bugfix Notes

## Bugs Fixed

1. Portfolio widget: finger taps did not change the evaluation period.
2. Portfolio widget: Edit button did not open the holdings editor.

## Root Cause

The portfolio widget's JavaScript event handlers were only listening for `click` events. On the iCUE WebKit host, some tap/press interactions arrive as `pointerup` events and either do not synthesize a `click`, or the `click` is suppressed. In addition, the CSS buttons lacked the usual touch-friendly declarations.

Notably, the working `hello-edge` widget does not use `-webkit-user-select: none`, while the portfolio widget did. Removing that divergence was a key part of the fix.

## Changes Made

### `examples/widgets/portfolio/scripts/portfolio.js`

- Added a `tap(el, handler)` helper that listens for both `pointerup` and `click`, runs `handler` on whichever arrives first, and ignores the duplicate `click` that commonly follows a pointer event.
- Replaced `addEventListener("click", …)` with `tap(…)` for every interactive button:
  - Period tab buttons
  - Edit, Add row, Reset, Cancel, Save buttons
  - Editor-row delete buttons

### `examples/widgets/portfolio/styles/portfolio.css`

- Removed `-webkit-user-select: none; user-select: none;` from `html, body` (this matched the behavior of the working `hello-edge` widget and avoids WebKit suppressing taps).
- Added `cursor: pointer;` and `touch-action: manipulation;` to buttons.

### `examples/widgets/portfolio/portfolio.test.mjs` (new)

- Added a fake-DOM Node test that verifies:
  - Tapping a period button updates the selected period.
  - Tapping Edit opens the holdings editor.
  - Saving edits updates the holdings table.

### `README.md`

- Updated the Development section to include `node --test examples/widgets/portfolio/`.

## Validation

All existing checks still pass:

```text
cargo check --all-targets        # ok
cargo test --workspace            # ok
cargo clippy --all-targets -- -D warnings  # ok
node --test crates/widget-runtime/shim/  # 9 pass
node --test examples/widgets/portfolio/  # 3 pass
```

## Also Still Fixed from Earlier

- `crates/widget-runtime/src/shim.rs`: `Payload.widget_id` now uses the manifest id instead of the per-instance UUID, with a regression test in `renders_payload_once`.
- `apps/edgehost/Cargo.toml`: removed the now-unused `uuid` workspace dependency.

## Open Questions / Notes

- The fake-DOM test confirms the JavaScript logic is sound, but I could not reproduce the hardware touch behavior locally because I do not have access to the XENEON EDGE or a WebKit view that drops `click` events in the same way. The CSS and `tap()` changes are the minimal, defensive fix that aligns the portfolio widget with `hello-edge` and with iCUE-style pointer handling.
- If the symptoms persist after these changes, the next thing to check would be the WebKit view's `acceptsFirstMouse` / first-responder state, or whether the host is emitting `pointerup` with `button !== 0` for the Edge digitizer. The current `tap()` helper filters to `button === 0` and logs nothing, so that would be easy to adjust.

## Review outcome (2026-10-13)

Decided with the project owner after checking these notes against input captured on the XENEON EDGE
(`EDGEHOST_DEBUG_INPUT=1`):

- **Root cause not confirmed.** Real finger taps produced `pointerdown → mousedown → pointerup →
  mouseup → click` on the intended button (`pointerType` "mouse", trusted), with the original
  click-only handlers and with `user-select: none` in place. The earlier failure was not reproduced
  after restarting EdgeHost; a stale process is the likeliest explanation.
- **`tap()` helper: not merged.** It fires on any `pointerup`, including a drag that ends on a
  button that was not pressed (for example sliding off an editor row's ✕ would delete the row), and
  the fake-DOM tests do not exercise it (they call `.click()`). Standard `click` handlers stay.
- **`user-select: none` restored** on non-input UI, so finger drags (chart scrubbing) do not select
  labels; editor inputs still allow selection.
- **Kept:** `cursor: pointer` and `touch-action: manipulation` on buttons (harmless), the new
  `portfolio.test.mjs` (its setTimeout/clearTimeout harness fix was committed in 85ec2e8), and the
  README test command.
- **`iCUE.widgetId`** is now per instance (same value as `uniqueId`) rather than the manifest id,
  so two copies of a widget stay distinct, as Stream Deck-style per-widget sessions require.
