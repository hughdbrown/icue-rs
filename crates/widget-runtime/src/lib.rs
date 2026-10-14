//! Runtime for iCUE HTML widgets on the XENEON EDGE.
//!
//! - [`shim`] renders the script injected at document start, which recreates the globals iCUE
//!   provides (`iCUE`, `device`, `uniqueId`, property globals, `tr`, `iCUE_initialized`, …).
//! - [`assets`] serves a widget's files over the `icuew://` scheme without escaping its directory.
//! - [`host`] opens the Edge surface (or a scaled stand-in window) and runs widgets in it.

pub mod assets;
pub mod host;
pub mod shim;

pub use host::{RunOptions, run_one};
