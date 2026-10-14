//! The Edge surface: one window covering the XENEON EDGE (or a scaled stand-in window when no
//! Edge is connected) with a child web view per widget.
//!
//! Geometry: widgets are laid out in panel pixels ([`edge_layout`]). `k` is the number of window
//! logical points per panel pixel. Each web view's bounds are `slot × k` and its page zoom is `k`,
//! so the widget always sees a CSS viewport equal to its slot size (e.g. 1688×696 for L).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use edge_layout::{Cell, MonitorInfo, Orientation, Slot, find_edge, slot_rect};
use icuew_manifest::WidgetDef;
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{ElementState, Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::keyboard::KeyCode;
use tao::window::{Window, WindowBuilder};
use uuid::Uuid;
use wry::http::{Response, header};
use wry::{Rect, WebView, WebViewBuilder};

use crate::assets::{mime_for, read_asset, resolve};
use crate::shim::{self, ShimConfig, TemperatureUnit};

/// URL scheme widgets are served from.
const SCHEME: &str = "icuew";
/// Namespace for EdgeHost's name-based (v5) UUIDs.
const NAMESPACE: Uuid = Uuid::from_u128(0x5f0e_9c1d_7a2b_4e63_9d84_1c6f_2b7a_e0d3);

/// Options for [`run_one`].
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Unpacked widget directory.
    pub widget_dir: PathBuf,
    /// Slot size to run the widget at.
    pub slot: Slot,
    /// Top-left grid cell of the slot.
    pub origin: Cell,
    /// Orientation for the stand-in window (an attached Edge reports its own).
    pub orientation: Orientation,
    /// Stand-in window size as a fraction of the panel's pixel size.
    pub windowed_scale: f64,
    /// Use the stand-in window even if an Edge is attached.
    pub force_windowed: bool,
    /// Enable the Web Inspector.
    pub devtools: bool,
    /// Shared `common/` folder served at `/common/` when the widget has no copy of its own.
    /// iCUE ships one beside its built-in widgets; defaults to a `common` folder beside the widget.
    pub common_dir: Option<PathBuf>,
}

/// Where the surface is shown.
enum Target {
    /// Borderless over an attached Edge.
    Edge {
        position: PhysicalPosition<i32>,
        scale_factor: f64,
        name: String,
    },
    /// Ordinary window on the current display.
    Windowed,
}

/// Run a single widget until its window is closed (or Esc is pressed on the Edge).
///
/// # Errors
///
/// Fails if the widget cannot be parsed, the slot does not fit the grid, or the window or web
/// views cannot be created. On success this function does not return: the event loop exits the
/// process when the window closes.
pub fn run_one(opts: &RunOptions) -> Result<()> {
    if !(0.1..=2.0).contains(&opts.windowed_scale) {
        bail!("window scale {} is outside 0.1..=2.0", opts.windowed_scale);
    }
    let root = opts
        .widget_dir
        .canonicalize()
        .with_context(|| format!("opening widget directory {}", opts.widget_dir.display()))?;
    let def =
        WidgetDef::load(&root).with_context(|| format!("loading widget {}", root.display()))?;
    if !def.manifest.supports_edge() {
        eprintln!(
            "warning: {} does not list dashboard_lcd (XENEON EDGE) in supported_devices",
            def.manifest.id
        );
    }

    let common = shared_common_dir(opts, &root);

    let event_loop = EventLoop::new();
    let (orientation, target) = detect_target(&event_loop, opts);
    let (panel_w, panel_h) = orientation.panel_size();
    let slot_px = slot_rect(orientation, opts.slot, opts.origin)?;
    let (k, device_key) = match &target {
        Target::Edge {
            scale_factor, name, ..
        } => (1.0 / scale_factor, format!("edge:{name}")),
        Target::Windowed => (opts.windowed_scale, "windowed".to_owned()),
    };
    let title = format!(
        "EdgeHost — {} ({} {})",
        def.manifest.name, opts.slot, orientation
    );
    let window = build_window(&event_loop, &target, orientation, k, title)?;

    let background = WebViewBuilder::new()
        .with_bounds(logical_rect(0, 0, panel_w, panel_h, k))
        .with_html("<!doctype html><html style=\"background:#000;height:100%\"></html>")
        .build_as_child(&window)
        .context("creating the background view")?;

    let instance_id = Uuid::new_v5(
        &NAMESPACE,
        format!("run-one:{}:{}", def.manifest.id, opts.slot).as_bytes(),
    );
    let device_id = Uuid::new_v5(&NAMESPACE, device_key.as_bytes());
    let host = instance_id.simple().to_string();
    let cfg = ShimConfig {
        language: "en".to_owned(),
        fps_limit: 30,
        is_preview: false,
        instance_id,
        device_id,
        temperature_unit: TemperatureUnit::Celsius,
        base_url: format!("{SCHEME}://{host}/"),
    };
    let script = shim::render(&def, &cfg).context("rendering the injection shim")?;
    let label = def.manifest.id.to_string();
    let widget = build_widget_view(
        &window,
        ViewSpec {
            root: &root,
            common: common.as_deref(),
            script: &script,
            url: &format!("{}index.html", cfg.base_url),
            bounds: logical_rect(slot_px.x, slot_px.y, slot_px.width, slot_px.height, k),
            instance_id,
            label,
            devtools: opts.devtools,
        },
    )?;
    widget.zoom(k).context("setting widget zoom")?;

    eprintln!(
        "running {} at {} ({}×{} px) on {}",
        def.manifest.id,
        opts.slot,
        slot_px.width,
        slot_px.height,
        match &target {
            Target::Edge { name, .. } => name.as_str(),
            Target::Windowed => "a stand-in window (no XENEON EDGE detected)",
        }
    );

    let is_edge = matches!(target, Target::Edge { .. });
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        // The window and views live as long as the loop.
        let _keep_alive = (&window, &background, &widget);
        if let Event::WindowEvent { event, .. } = event {
            match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::KeyboardInput { event, .. }
                    if is_edge
                        && event.physical_key == KeyCode::Escape
                        && event.state == ElementState::Pressed =>
                {
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            }
        }
    })
}

/// Everything needed to create one widget web view.
struct ViewSpec<'a> {
    root: &'a Path,
    common: Option<&'a Path>,
    script: &'a str,
    url: &'a str,
    bounds: Rect,
    instance_id: Uuid,
    label: String,
    devtools: bool,
}

/// Pick the attached Edge, or fall back to a stand-in window in the requested orientation.
fn detect_target(event_loop: &EventLoop<()>, opts: &RunOptions) -> (Orientation, Target) {
    let monitors: Vec<_> = event_loop.available_monitors().collect();
    let infos: Vec<MonitorInfo> = monitors
        .iter()
        .map(|m| MonitorInfo {
            width: m.size().width,
            height: m.size().height,
        })
        .collect();
    let detected = if opts.force_windowed {
        None
    } else {
        find_edge(&infos)
    };
    match detected.and_then(|(i, o)| monitors.get(i).map(|m| (m, o))) {
        Some((m, o)) => (
            o,
            Target::Edge {
                position: m.position(),
                scale_factor: m.scale_factor(),
                name: m.name().unwrap_or_else(|| "XENEON EDGE".to_owned()),
            },
        ),
        None => (opts.orientation, Target::Windowed),
    }
}

/// Borderless window covering the Edge, or a decorated window scaled by `k`.
fn build_window(
    event_loop: &EventLoop<()>,
    target: &Target,
    orientation: Orientation,
    k: f64,
    title: String,
) -> Result<Window> {
    let (panel_w, panel_h) = orientation.panel_size();
    let builder = WindowBuilder::new().with_title(title).with_resizable(false);
    let builder = match target {
        Target::Edge { position, .. } => builder
            .with_decorations(false)
            .with_position(*position)
            .with_inner_size(PhysicalSize::new(panel_w, panel_h))
            .with_visible_on_all_workspaces(true),
        Target::Windowed => builder.with_inner_size(LogicalSize::new(
            f64::from(panel_w) * k,
            f64::from(panel_h) * k,
        )),
    };
    builder
        .build(event_loop)
        .context("creating the surface window")
}

fn build_widget_view(window: &Window, spec: ViewSpec<'_>) -> Result<WebView> {
    let ViewSpec {
        root,
        common,
        script,
        url,
        bounds,
        instance_id,
        label,
        devtools,
    } = spec;
    let serve_root = root.to_path_buf();
    let serve_common = common.map(Path::to_path_buf);
    let builder = WebViewBuilder::new()
        .with_bounds(bounds)
        .with_transparent(true)
        .with_devtools(devtools)
        .with_initialization_script_for_main_only(script, true)
        .with_custom_protocol(SCHEME.to_owned(), move |_id, request| {
            serve(&serve_root, serve_common.as_deref(), request.uri().path())
        })
        .with_ipc_handler(move |request| report(&label, request.body()))
        .with_url(url);
    #[cfg(target_os = "macos")]
    let builder = {
        use wry::WebViewBuilderExtDarwin;
        // Per-instance WebKit data store (macOS 14+): isolated localStorage/IndexedDB, as iCUE does.
        builder.with_data_store_identifier(*instance_id.as_bytes())
    };
    #[cfg(not(target_os = "macos"))]
    let _ = instance_id;
    builder
        .build_as_child(window)
        .context("creating the widget view")
}

fn logical_rect(x: u32, y: u32, width: u32, height: u32, k: f64) -> Rect {
    Rect {
        position: wry::dpi::LogicalPosition::new(f64::from(x) * k, f64::from(y) * k).into(),
        size: wry::dpi::LogicalSize::new(f64::from(width) * k, f64::from(height) * k).into(),
    }
}

/// The `common/` folder to fall back to, if any (canonicalised).
fn shared_common_dir(opts: &RunOptions, widget_root: &Path) -> Option<PathBuf> {
    let candidate = match &opts.common_dir {
        Some(dir) => dir.clone(),
        None => widget_root.parent()?.join("common"),
    };
    let dir = candidate.canonicalize().ok().filter(|d| d.is_dir());
    if dir.is_none() && opts.common_dir.is_some() {
        eprintln!(
            "warning: --common-dir {} is not a directory; ignoring it",
            candidate.display()
        );
    }
    dir
}

/// Serve a widget file; `/common/…` falls back to the shared `common` folder.
fn serve(root: &Path, common: Option<&Path>, url_path: &str) -> Response<Cow<'static, [u8]>> {
    let result = match read_asset(root, url_path) {
        Err(e) if e.status() == 404 => match (common, url_path.strip_prefix("/common/")) {
            (Some(dir), Some(rest)) => read_asset(dir, rest).map_err(|_| e),
            _ => Err(e),
        },
        other => other,
    };
    let built = match result {
        Ok(bytes) => {
            let mime = resolve(root, url_path).map_or("application/octet-stream", |p| mime_for(&p));
            Response::builder()
                .status(200)
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Cow::Owned(bytes))
        }
        Err(e) => {
            eprintln!("icuew: {url_path}: {e:#}");
            Response::builder()
                .status(e.status())
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Cow::Owned(e.to_string().into_bytes()))
        }
    };
    built.unwrap_or_else(|e| {
        eprintln!("icuew: building response for {url_path}: {e}");
        Response::new(Cow::Borrowed(b"internal error".as_slice()))
    })
}

/// Print a message posted by the shim (`{k, level?, msg?}`) to stderr.
fn report(label: &str, body: &str) {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(body) else {
        eprintln!("[{label}] unparseable message: {body}");
        return;
    };
    let field = |name: &str| {
        message
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    };
    match field("k") {
        "console" => eprintln!("[{label}] {}: {}", field("level"), field("msg")),
        "booted" => eprintln!("[{label}] booted"),
        other => eprintln!("[{label}] {other}: {body}"),
    }
}
