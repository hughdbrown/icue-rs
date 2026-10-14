# EdgeHost configurator: UI ↔ host contract

The configurator is a normal window on the main display (opened from the menu-bar item, "Configure
Widgets…"). Its UI is static HTML/CSS/JS in `apps/edgehost/ui/` (embedded in the binary) and talks to
EdgeHost over a small JSON request/response channel. Every change is written to `layout.json`; the
dashboard on the Edge reloads by itself.

## Transport

- UI → host: `window.ipc.postMessage(JSON.stringify({ id, method, params }))`
  - `id`: positive integer chosen by the UI, unique per request.
- Host → UI: the host calls `window.__configurator.reply(message)` with
  `{ id, result }` on success or `{ id, error: { message } }` on failure.
- Host → UI events: `window.__configurator.event({ type: "changed" })` after the layout or the
  installed widgets change outside the UI (e.g. `edgectl`); the UI should re-fetch state.
- The UI must define `window.__configurator = { reply, event }` before sending its first request.

## Asset URLs

- UI files are served at `edgecfg://app/<path>` (`index.html`, `app.js`, `app.css`).
- Widget icons/previews: `edgecfg://widget/<widget-id>/<version>/<relative path>` (e.g. the manifest's
  `preview_icon`). Missing files return 404; show a placeholder.

## Types

```ts
type Slot = "S" | "M" | "L" | "XL";
type Orientation = "landscape" | "portrait";
type Cell = { col: number; row: number };

type Property = {
  name: string;                 // global variable name
  kind: "slider" | "switch" | "textfield" | "color" | "combobox" | "search-combobox"
      | "tab-buttons" | "media-selector" | "sensors-combobox" | "sensors-factory"
      | "stream-deck-virtual-device";
  label: string;                // resolved for display (tr('X') → translation; 'X' → X); raw text otherwise
  default_expr: string | null;  // raw JS expression (informational)
  min?: number; max?: number; step?: number; unit?: string;   // slider, when literal numbers
  options?: { key: string; label: string }[];                 // combobox/tab-buttons, when literal
};

type Group = { title: string; properties: string[]; info?: string };   // titles resolved like labels

type Widget = {               // one installed widget version
  id: string; version: string; name: string; description: string; author: string;
  icon: string | null;        // edgecfg://widget/... URL or null
  supports_edge: boolean; interactive: boolean;
  required_plugins: string[]; // "namespace:Name:x.y" — not implemented yet on EdgeHost
  properties: Property[]; groups: Group[];
};

type Instance = {
  id: string;                 // UUID
  widget: string; version: string;
  slot: Slot; origin: Cell;
  values: Record<string, unknown>;   // current concrete values
  user_set: string[];                // keys the user changed (others are defaults)
  defaults_pending: boolean;         // defaults not evaluated yet (shown after the Edge loads it)
};

type Display = { index: number; name: string; pixel_size: [number, number]; edge: boolean; native: boolean };

type State = {
  orientation: Orientation;
  grid: { cols: number; rows: number };          // 3×2 landscape, 1×6 portrait
  slots: Record<Slot, { cols: number; rows: number }>;   // cell span per slot for this orientation
  pages: { instances: Instance[] }[];
  display: string;                               // "auto" | "windowed" | index | name
  displays: Display[];
  widgets: Widget[];                             // newest version of each installed widget first
  data_dir: string;
};
```

## Methods

| Method | Params | Result | Notes |
|---|---|---|---|
| `state.get` | `{}` | `State` | |
| `layout.add` | `{ widget, version?, slot, page, origin? }` | `{ instance: string }` | `page` may equal the page count (adds a page). No `origin` = first free cell. Errors: overlap, does not fit, no room. |
| `layout.move` | `{ instance, page, origin, slot }` | `{}` | Move and/or resize. Same errors as add. |
| `layout.remove` | `{ instance }` | `{}` | Empty trailing pages are dropped. |
| `layout.set` | `{ instance, key, value }` | `{}` | `key` must be one of the widget's properties. |
| `layout.unset` | `{ instance, key }` | `{}` | Back to the widget's default. |
| `layout.display` | `{ choice }` | `{}` | `"auto"`, `"windowed"`, an index or a name. |
| `layout.orientation` | `{ orientation }` | `{}` | Only when every page is empty. |
| `widgets.uninstall` | `{ widget, version? }` | `{}` | Fails while instances use it. |

Errors are human-readable sentences suitable to show as-is.

## UI expectations

- Left: **library** of installed widgets (icon, name, description, a "needs plugins" badge when
  `required_plugins` is non-empty, a warning when `supports_edge` is false). Tapping/clicking a slot
  size button (S/M/L/XL) adds it to the current page at the first free position.
- Centre: **page editor**: page tabs (+ add page), a scaled-down Edge grid (2560×720 aspect in
  landscape) showing instances as tiles at their cells with name and slot; click a tile to select;
  drag a tile to another free position (snap to cells) → `layout.move`; resize via S/M/L/XL buttons
  on the selected tile; delete button.
- Right: **settings** for the selected instance: controls generated from `properties` grouped by
  `groups` (ungrouped ones last). Controls: color → `<input type=color>`; slider → range + number
  with unit; switch → toggle; textfield → text; combobox/tab-buttons → select/segmented buttons from
  `options` (text input if absent); everything else → JSON text area. Changes call `layout.set`
  (debounced 300 ms for text/slider). A "reset" control per property calls `layout.unset` when the
  key is in `user_set`.
- Top bar: display choice (select from `displays` + auto/windowed), orientation (disabled unless
  empty), data folder path.
- Works with mouse and touch, keyboard accessible, light/dark following `prefers-color-scheme`.
