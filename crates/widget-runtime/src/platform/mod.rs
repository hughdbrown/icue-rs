//! Platform-specific code, behind one interface.
//!
//! [`Platform`] declares every operation that differs between operating systems. Each supported OS
//! has a module (`macos.rs`, `linux.rs`, selected with `#[cfg(target_os = …)]`) whose type
//! implements the trait, so all implementations share the **same function names, arguments and
//! return types**: a signature that drifts on one platform fails to compile. The rest of the crate
//! calls the free functions below and contains no `cfg` of its own.

use std::io;
use std::path::Path;

use anyhow::Result;
use tao::monitor::MonitorHandle;
use tao::window::{Window, WindowBuilder};
use uuid::Uuid;
use wry::{WebView, WebViewBuilder};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
type Current = linux::Linux;
#[cfg(target_os = "macos")]
type Current = macos::MacOs;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("EdgeHost supports macOS and Linux");

/// Operations whose implementation depends on the operating system.
pub trait Platform {
    /// Where child web views are attached inside the surface window.
    type ViewParent;
    /// Per-widget-instance browser storage that must live as long as the instance's views.
    type Storage;

    /// EDID vendor and product numbers of a display, if the platform can tell.
    fn edid_ids(monitor: &MonitorHandle) -> (Option<u32>, Option<u32>);

    /// Configure the surface window to cover `monitor` (logical `origin` and `size`).
    fn place_surface(
        builder: WindowBuilder,
        monitor: &MonitorHandle,
        origin: (f64, f64),
        size: (f64, f64),
    ) -> WindowBuilder;

    /// How to switch the XENEON EDGE to its native 2560×720 mode on this platform.
    fn native_mode_hint() -> &'static str;

    /// Prepare `window` to host child web views.
    ///
    /// # Errors
    ///
    /// If the window has no usable container.
    fn view_parent(window: &Window) -> Result<Self::ViewParent>;

    /// Attach a configured web view to the surface.
    ///
    /// # Errors
    ///
    /// If the web view cannot be created.
    fn attach(
        parent: &Self::ViewParent,
        window: &Window,
        builder: WebViewBuilder<'_>,
    ) -> wry::Result<WebView>;

    /// A web view filling `window` and following its size (for ordinary app windows).
    ///
    /// # Errors
    ///
    /// If the web view cannot be created.
    fn attach_fill(window: &Window, builder: WebViewBuilder<'_>) -> wry::Result<WebView>;

    /// Storage for one widget instance, under `root`.
    fn storage(root: &Path, instance: Uuid) -> Self::Storage;

    /// A web-view builder that uses `storage` for `instance`.
    fn builder(storage: &mut Self::Storage, instance: Uuid) -> WebViewBuilder<'_>;

    /// Show `path` in the file manager.
    ///
    /// # Errors
    ///
    /// If the file manager cannot be launched.
    fn reveal(path: &Path) -> io::Result<()>;
}

/// Where child web views are attached on this platform.
pub type ViewParent = <Current as Platform>::ViewParent;
/// Per-instance browser storage on this platform.
pub type Storage = <Current as Platform>::Storage;

/// See [`Platform::edid_ids`].
#[must_use]
pub fn edid_ids(monitor: &MonitorHandle) -> (Option<u32>, Option<u32>) {
    Current::edid_ids(monitor)
}

/// See [`Platform::place_surface`].
#[must_use]
pub fn place_surface(
    builder: WindowBuilder,
    monitor: &MonitorHandle,
    origin: (f64, f64),
    size: (f64, f64),
) -> WindowBuilder {
    Current::place_surface(builder, monitor, origin, size)
}

/// See [`Platform::native_mode_hint`].
#[must_use]
pub fn native_mode_hint() -> &'static str {
    Current::native_mode_hint()
}

/// See [`Platform::view_parent`].
///
/// # Errors
///
/// See [`Platform::view_parent`].
pub fn view_parent(window: &Window) -> Result<ViewParent> {
    Current::view_parent(window)
}

/// See [`Platform::attach`].
///
/// # Errors
///
/// See [`Platform::attach`].
pub fn attach(
    parent: &ViewParent,
    window: &Window,
    builder: WebViewBuilder<'_>,
) -> wry::Result<WebView> {
    Current::attach(parent, window, builder)
}

/// See [`Platform::attach_fill`].
///
/// # Errors
///
/// See [`Platform::attach_fill`].
pub fn attach_fill(window: &Window, builder: WebViewBuilder<'_>) -> wry::Result<WebView> {
    Current::attach_fill(window, builder)
}

/// See [`Platform::storage`].
#[must_use]
pub fn storage(root: &Path, instance: Uuid) -> Storage {
    Current::storage(root, instance)
}

/// See [`Platform::builder`].
pub fn builder(storage: &mut Storage, instance: Uuid) -> WebViewBuilder<'_> {
    Current::builder(storage, instance)
}

/// See [`Platform::reveal`].
///
/// # Errors
///
/// See [`Platform::reveal`].
pub fn reveal(path: &Path) -> io::Result<()> {
    Current::reveal(path)
}
