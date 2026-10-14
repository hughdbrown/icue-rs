//! Linux (X11 and Wayland via GTK): EDID ids from `/sys/class/drm`, a borderless fullscreen
//! window on the Edge's monitor (Wayland does not let apps position windows), child WebKitGTK views
//! in a `gtk::Fixed`, and one `WebContext` data directory per widget instance.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use edge_layout::parse_edid;
use gtk::prelude::*;
use tao::monitor::MonitorHandle;
use tao::platform::unix::WindowExtUnix;
use tao::window::{Fullscreen, Window, WindowBuilder};
use uuid::Uuid;
use wry::{WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix};

use super::Platform;

/// The Linux implementation of [`Platform`].
pub struct Linux;

impl Platform for Linux {
    type ViewParent = gtk::Fixed;
    type Storage = WebContext;

    fn edid_ids(monitor: &MonitorHandle) -> (Option<u32>, Option<u32>) {
        // GDK names monitors by their EDID model string (e.g. "XENEON EDGE"); find that EDID
        // among the connected DRM connectors.
        let Some(name) = monitor.name() else {
            return (None, None);
        };
        connected_edids()
            .into_iter()
            .find(|e| e.name.as_deref() == Some(name.as_str()))
            .map_or((None, None), |e| (Some(e.vendor), Some(e.product)))
    }

    fn place_surface(
        builder: WindowBuilder,
        monitor: &MonitorHandle,
        _origin: (f64, f64),
        _size: (f64, f64),
    ) -> WindowBuilder {
        builder
            .with_decorations(false)
            .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor.clone()))))
    }

    fn native_mode_hint() -> &'static str {
        "select 2560×720 for the Edge in your display settings (X11: `xrandr --output <name> --mode 2560x720`)"
    }

    fn view_parent(window: &Window) -> Result<gtk::Fixed> {
        let vbox = window
            .default_vbox()
            .context("the surface window has no GTK container")?;
        let fixed = gtk::Fixed::new();
        vbox.pack_start(&fixed, true, true, 0);
        fixed.show_all();
        Ok(fixed)
    }

    fn attach(
        parent: &gtk::Fixed,
        _window: &Window,
        builder: WebViewBuilder<'_>,
    ) -> wry::Result<WebView> {
        builder.build_gtk(parent)
    }

    fn attach_fill(window: &Window, builder: WebViewBuilder<'_>) -> wry::Result<WebView> {
        // build() only works on X11; a GTK container works on X11 and Wayland.
        match window.default_vbox() {
            Some(vbox) => builder.build_gtk(vbox),
            None => builder.build(window),
        }
    }

    fn storage(root: &Path, instance: Uuid) -> WebContext {
        WebContext::new(Some(root.join(instance.simple().to_string())))
    }

    fn builder(storage: &mut WebContext, _instance: Uuid) -> WebViewBuilder<'_> {
        WebViewBuilder::new_with_web_context(storage)
    }

    fn runtime_dir(home: &Path) -> PathBuf {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map_or_else(|| home.join(".local/share"), PathBuf::from)
    }

    fn videos_dir(home: &Path) -> PathBuf {
        home.join("Videos")
    }

    fn reveal(path: &Path) -> io::Result<()> {
        Command::new("xdg-open").arg(path).spawn().map(drop)
    }
}

/// EDIDs of every connected DRM connector (`/sys/class/drm/card*-*/{status,edid}`).
fn connected_edids() -> Vec<edge_layout::Edid> {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| fs::read_to_string(p.join("status")).is_ok_and(|s| s.trim() == "connected"))
        .filter_map(|p| fs::read(p.join("edid")).ok())
        .filter_map(|bytes| parse_edid(&bytes))
        .collect()
}
