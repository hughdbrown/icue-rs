//! Runtime for iCUE HTML widgets on the XENEON EDGE.
//!
//! - [`shim`] renders the script injected at document start, which recreates the globals iCUE
//!   provides (`iCUE`, `device`, `uniqueId`, property globals, `tr`, `iCUE_initialized`, …).
//! - [`assets`] serves a widget's files over the `icuew://` scheme without escaping its directory.
//! - [`surface`] opens the Edge surface (or a scaled stand-in window) and hosts widget views.
//! - [`display`] describes connected displays (including EDID ids) for selection.
//! - [`proxy`] performs `fetch()` for domains a widget declares, so APIs without CORS work.
//! - [`plugins`] carries `window.plugins` calls and signals between a page and its
//!   [`edge_plugins::PluginHost`].
//! - [`host`] implements `edgectl run-one`.

pub mod assets;
pub mod display;
pub mod host;
pub mod platform;
pub mod plugins;
pub mod proxy;
pub mod shim;
pub mod surface;

pub use display::list_displays;
pub use host::{RunOptions, run_one};
pub use surface::{EventSink, RuntimeEvent, Surface, SurfaceOptions, WidgetInstance};
