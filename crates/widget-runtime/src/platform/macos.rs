//! macOS: CoreGraphics display ids, a borderless window positioned over the Edge, child
//! `WKWebView`s, and WebKit data stores identified per instance.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::monitor::MonitorHandle;
use tao::platform::macos::MonitorHandleExtMacOS;
use tao::window::{Window, WindowBuilder};
use uuid::Uuid;
use wry::{WebView, WebViewBuilder, WebViewBuilderExtDarwin};

use super::Platform;

/// The macOS implementation of [`Platform`].
pub struct MacOs;

/// Child web views attach directly to the surface window.
pub struct WindowRoot;

/// A WebKit data-store identifier (macOS 14+): one isolated store per widget instance.
pub struct DataStore {
    identifier: [u8; 16],
}

impl Platform for MacOs {
    type ViewParent = WindowRoot;
    type Storage = DataStore;

    fn edid_ids(monitor: &MonitorHandle) -> (Option<u32>, Option<u32>) {
        let display = core_graphics::display::CGDisplay::new(monitor.native_id());
        (Some(display.vendor_number()), Some(display.model_number()))
    }

    fn place_surface(
        builder: WindowBuilder,
        _monitor: &MonitorHandle,
        origin: (f64, f64),
        size: (f64, f64),
    ) -> WindowBuilder {
        // A borderless window exactly over the display. (macOS fullscreen would create a Space.)
        builder
            .with_decorations(false)
            .with_position(LogicalPosition::new(origin.0, origin.1))
            .with_inner_size(LogicalSize::new(size.0, size.1))
            .with_visible_on_all_workspaces(true)
    }

    fn native_mode_hint() -> &'static str {
        "`brew install displayplacer`, then `displayplacer list` and select its res:2560x720 mode \
         (see docs/research/edge-hardware.md)"
    }

    fn view_parent(_window: &Window) -> Result<WindowRoot> {
        Ok(WindowRoot)
    }

    fn attach(
        _parent: &WindowRoot,
        window: &Window,
        builder: WebViewBuilder<'_>,
    ) -> wry::Result<WebView> {
        builder.build_as_child(window)
    }

    fn attach_fill(window: &Window, builder: WebViewBuilder<'_>) -> wry::Result<WebView> {
        builder.build(window)
    }

    fn storage(_root: &Path, instance: Uuid) -> DataStore {
        DataStore {
            identifier: *instance.as_bytes(),
        }
    }

    fn builder(storage: &mut DataStore, _instance: Uuid) -> WebViewBuilder<'_> {
        // Isolated localStorage/IndexedDB per instance, as iCUE does; WebKit keeps the data.
        WebViewBuilder::new().with_data_store_identifier(storage.identifier)
    }

    fn runtime_dir(home: &Path) -> PathBuf {
        home.join("Library/Application Support")
    }

    fn videos_dir(home: &Path) -> PathBuf {
        home.join("Movies")
    }

    fn reveal(path: &Path) -> io::Result<()> {
        Command::new("open").arg(path).spawn().map(drop)
    }
}
