//! The Edge surface: one window covering the chosen display, a black background, and one web
//! view per widget instance, plus hidden "evaluate" views that compute property defaults.
//!
//! Geometry: widgets are laid out in panel pixels ([`edge_layout`]) and the panel is fitted into the
//! window ([`edge_layout::fit_panel`]): `scale` logical points per panel pixel, letterboxed when the
//! display is not in its native mode. Each view's bounds are `offset + slot × scale` and its page
//! zoom is `scale`, so the widget always sees a CSS viewport equal to its slot size.
//!
//! Page changes: every visible view (widgets and the background) runs `shim/edge-swipe.js`, which
//! reports horizontal swipes, and a strip of page dots ([`Surface::set_pager`]) sits above the
//! widgets at the bottom centre.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use edge_layout::{
    Cell, DisplayChoice, Fit, Orientation, PxRect, Selection, Slot, fit_panel, select_display,
    slot_rect,
};
use edge_plugins::addin::Registry;
use edge_plugins::paths::Macros;
use edge_plugins::{HostContext, PluginHost};
use icuew_manifest::WidgetDef;
use serde_json::Value;
use tao::dpi::LogicalSize;
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder};
use uuid::Uuid;
use wry::http::{Response, header};
use wry::{Rect, WebView, WebViewBuilder};

use crate::assets::{mime_for, read_asset, resolve};
use crate::display::list_displays;
use crate::platform;
use crate::shim::{self, ShimConfig, ShimMode, TemperatureUnit};
use crate::{plugins, proxy};

/// URL scheme widgets are served from.
const SCHEME: &str = "icuew";
/// Blank page the evaluate views load (reserved path; never a widget file).
const EVAL_PATH: &str = "/__edgehost/eval.html";
/// Swipe detector shared by every visible view (a function expression; see [`swipe_script`]).
const SWIPE_SOURCE: &str = include_str!("../shim/edge-swipe.js");
/// Page-dot strip geometry, in panel pixels: one dot's tap target, the strip height, the inner
/// padding and the gap to the panel's bottom edge.
const DOT_TARGET: u32 = 56;
const PAGER_HEIGHT: u32 = 40;
const PAGER_PADDING: u32 = 8;
const PAGER_BOTTOM: u32 = 2;
/// Namespace for EdgeHost's name-based (v5) UUIDs.
pub(crate) const NAMESPACE: Uuid = Uuid::from_u128(0x5f0e_9c1d_7a2b_4e63_9d84_1c6f_2b7a_e0d3);

/// Options for [`Surface::open`].
#[derive(Debug, Clone)]
pub struct SurfaceOptions {
    /// Which display to cover.
    pub display: DisplayChoice,
    /// Orientation of the stand-in window when no display is selected.
    pub windowed_orientation: Orientation,
    /// Stand-in window size as a fraction of the panel's pixel size.
    pub windowed_scale: f64,
    /// Enable the Web Inspector.
    pub devtools: bool,
    /// Shared `common/` folder served at `/common/` when a widget lacks its own.
    pub common_dir: Option<PathBuf>,
    /// `iCUE.iCUELanguage`.
    pub language: String,
    /// Developer runs: Escape inside a widget asks the host to close.
    pub escape_closes: bool,
    /// Where per-instance browser storage lives on platforms that need a folder (Linux).
    pub web_data_dir: PathBuf,
    /// Installed Rhai add-ins (`<store>/addins`); `None` for none.
    pub addins_dir: Option<PathBuf>,
}

/// Something a web view reported to the host.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeEvent {
    /// An evaluate view finished computing the defaults of `instance`.
    Evaluated {
        /// The instance.
        instance: Uuid,
        /// Property name → concrete value.
        values: BTreeMap<String, Value>,
    },
    /// A widget asked to close the run (Escape with [`SurfaceOptions::escape_closes`]).
    Close,
    /// A text field in a widget gained focus: make EdgeHost the key app so typing reaches it.
    WantsKeyboard,
    /// A horizontal swipe on the surface: show the next page (`forward`) or the previous one.
    Swipe {
        /// `true` for a right-to-left swipe.
        forward: bool,
    },
    /// A page dot was tapped: show this zero-based page.
    ShowPage(usize),
}

/// Where web views deliver [`RuntimeEvent`]s (usually an event-loop proxy).
pub type EventSink = Rc<dyn Fn(RuntimeEvent)>;

/// A widget to show on the surface.
#[derive(Debug)]
pub struct WidgetInstance {
    /// Instance id: `uniqueId`, storage key and data-store identifier.
    pub id: Uuid,
    /// Widget directory.
    pub root: PathBuf,
    /// Parsed widget.
    pub def: WidgetDef,
    /// Slot size.
    pub slot: Slot,
    /// Top-left grid cell.
    pub origin: Cell,
}

/// The window on the Edge (or stand-in) and the widget views in it.
pub struct Surface {
    window: Window,
    parent: platform::ViewParent,
    _background: WebView,
    /// The page dots, when there is more than one page; rebuilt to stay above new widget views.
    pager: Option<WebView>,
    /// `(current, count)` of the last [`Surface::set_pager`].
    pages: (usize, usize),
    orientation: Orientation,
    fit: Fit,
    on_display: bool,
    description: String,
    device_id: Uuid,
    opts: SurfaceOptions,
    sink: EventSink,
    views: HashMap<Uuid, WebView>,
    evaluations: HashMap<Uuid, WebView>,
    storages: HashMap<Uuid, platform::Storage>,
}

impl Surface {
    /// Select a display and open the surface on it.
    ///
    /// # Errors
    ///
    /// An unknown `--display` pick, or failure to create the window or background view.
    pub fn open<T: 'static>(
        target: &EventLoopWindowTarget<T>,
        opts: SurfaceOptions,
        sink: EventSink,
    ) -> Result<Self> {
        if !(0.1..=2.0).contains(&opts.windowed_scale) {
            bail!("window scale {} is outside 0.1..=2.0", opts.windowed_scale);
        }
        let displays = list_displays(target);
        let selection = select_display(&displays, &opts.display)?;
        let builder = WindowBuilder::new()
            .with_title("EdgeHost")
            .with_resizable(false);
        let (orientation, area, builder, description, device_key) = match selection {
            Selection::Display {
                index,
                orientation,
                edge_hardware,
                native,
            } => {
                let d = displays
                    .get(index)
                    .context("selected display disappeared")?;
                if edge_hardware && !native {
                    eprintln!(
                        "warning: the XENEON EDGE is in {}×{} mode, not its native 2560×720; the image is scaled.\n\
                         Fix: {}.",
                        d.pixel_size.0,
                        d.pixel_size.1,
                        platform::native_mode_hint()
                    );
                }
                let monitor = target
                    .available_monitors()
                    .nth(index)
                    .context("selected display disappeared")?;
                let builder =
                    platform::place_surface(builder, &monitor, d.logical_origin, d.logical_size);
                let kind = if edge_hardware {
                    "XENEON EDGE"
                } else {
                    "display"
                };
                let key = format!("display:{:?}:{:?}:{}", d.vendor, d.model, d.name);
                (
                    orientation,
                    d.logical_size,
                    builder,
                    format!("{kind} \"{}\"", d.name),
                    key,
                )
            }
            Selection::Windowed => {
                let (pw, ph) = opts.windowed_orientation.panel_size();
                let area = (
                    f64::from(pw) * opts.windowed_scale,
                    f64::from(ph) * opts.windowed_scale,
                );
                let builder = builder.with_inner_size(LogicalSize::new(area.0, area.1));
                let text = "a stand-in window (no XENEON EDGE detected; see `edgectl displays`)"
                    .to_owned();
                (
                    opts.windowed_orientation,
                    area,
                    builder,
                    text,
                    "windowed".to_owned(),
                )
            }
        };
        let window = builder
            .build(target)
            .context("creating the surface window")?;
        let parent = platform::view_parent(&window)?;
        let fit = fit_panel(orientation, area);
        let background = background_view(&parent, &window, area, fit.scale, &sink)?;
        window.set_focus();
        Ok(Self {
            window,
            parent,
            _background: background,
            pager: None,
            pages: (0, 0),
            orientation,
            fit,
            on_display: matches!(selection, Selection::Display { .. }),
            description,
            device_id: Uuid::new_v5(&NAMESPACE, device_key.as_bytes()),
            opts,
            sink,
            views: HashMap::new(),
            evaluations: HashMap::new(),
            storages: HashMap::new(),
        })
    }

    /// Grid orientation of the surface.
    #[must_use]
    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Whether the surface covers a real display (as opposed to a stand-in window).
    #[must_use]
    pub fn on_display(&self) -> bool {
        self.on_display
    }

    /// Human-readable description of where the surface is.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The surface window.
    #[must_use]
    pub fn window(&self) -> &Window {
        &self.window
    }

    /// Make EdgeHost the active app with the surface as key window, so keystrokes reach the
    /// focused widget field ([`RuntimeEvent::WantsKeyboard`]).
    pub fn take_keyboard(&self) {
        self.window.set_focus();
    }

    /// Start computing `instance`'s property defaults in a hidden view. The result arrives as
    /// [`RuntimeEvent::Evaluated`]; call [`Surface::end_evaluation`] afterwards.
    ///
    /// # Errors
    ///
    /// The slot does not fit the grid, or the view cannot be created.
    pub fn evaluate(&mut self, instance: &WidgetInstance) -> Result<()> {
        let rect = self.slot_bounds(instance)?;
        // Evaluate views see every plugin as unavailable, so they need no add-ins.
        let script = shim::render(
            &instance.def,
            &self.shim_config(instance, Arc::default()),
            ShimMode::Evaluate,
        )
        .context("rendering the evaluate shim")?;
        let view = self.build_view(instance, &script, EVAL_PATH, rect, None)?;
        self.evaluations.insert(instance.id, view);
        Ok(())
    }

    /// Drop the evaluate view of `id`, if any.
    pub fn end_evaluation(&mut self, id: Uuid) {
        self.evaluations.remove(&id);
    }

    /// Show `instance` with concrete property `values`, replacing any existing view for it.
    ///
    /// # Errors
    ///
    /// The slot does not fit the grid, or the view cannot be created.
    pub fn mount(
        &mut self,
        instance: &WidgetInstance,
        values: &BTreeMap<String, Value>,
    ) -> Result<()> {
        let rect = self.slot_bounds(instance)?;
        let addins = self.addins(instance);
        let script = shim::render(
            &instance.def,
            &self.shim_config(instance, Arc::clone(&addins)),
            ShimMode::Runtime(values),
        )
        .context("rendering the widget shim")?;
        self.views.remove(&instance.id);
        let view = self.build_view(instance, &script, "/index.html", rect, Some(addins))?;
        view.zoom(self.fit.scale).context("setting widget zoom")?;
        if self.opts.escape_closes {
            if let Err(e) = view.focus() {
                eprintln!("warning: could not focus the widget view: {e}");
            }
        }
        self.views.insert(instance.id, view);
        if self.pager.is_some() {
            // The new view was stacked on top; put the dots back above it.
            if let Err(e) = self.build_pager() {
                eprintln!("warning: could not show the page dots: {e:#}");
            }
        }
        Ok(())
    }

    /// Remove every widget and evaluate view, and the page dots.
    pub fn unmount_all(&mut self) {
        self.pager = None;
        self.views.clear();
        self.evaluations.clear();
    }

    /// Show page dots for `count` pages with `current` highlighted (none for fewer than two
    /// pages). Call after mounting the page so the dots sit above its widgets.
    ///
    /// # Errors
    ///
    /// The dot view cannot be created.
    pub fn set_pager(&mut self, current: usize, count: usize) -> Result<()> {
        self.pages = (current, count);
        self.build_pager()
    }

    fn build_pager(&mut self) -> Result<()> {
        self.pager = None;
        let (current, count) = self.pages;
        if count < 2 {
            return Ok(());
        }
        let (panel_w, panel_h) = self.orientation.panel_size();
        let dots = u32::try_from(count).unwrap_or(u32::MAX);
        let width = dots
            .saturating_mul(DOT_TARGET)
            .saturating_add(2 * PAGER_PADDING)
            .min(panel_w);
        let x = (panel_w - width) / 2;
        let y = panel_h - PAGER_HEIGHT - PAGER_BOTTOM;
        let s = self.fit.scale;
        let (ox, oy) = self.fit.offset;
        let bounds = logical_rect(
            (ox + f64::from(x) * s, oy + f64::from(y) * s),
            (f64::from(width) * s, f64::from(PAGER_HEIGHT) * s),
        );
        let view = platform::attach(
            &self.parent,
            &self.window,
            WebViewBuilder::new()
                .with_bounds(bounds)
                .with_transparent(true)
                .with_accept_first_mouse(true)
                .with_initialization_script_for_main_only(swipe_script(), true)
                .with_ipc_handler(host_ipc(Rc::clone(&self.sink)))
                .with_html(pager_html(current, count)),
        )
        .context("creating the page dots")?;
        view.zoom(s).context("setting page-dot zoom")?;
        self.pager = Some(view);
        Ok(())
    }

    fn slot_bounds(&self, instance: &WidgetInstance) -> Result<Rect> {
        let px: PxRect =
            slot_rect(self.orientation, instance.slot, instance.origin).with_context(|| {
                format!(
                    "placing {} on the {} surface",
                    instance.def.manifest.id, self.orientation
                )
            })?;
        let s = self.fit.scale;
        let (ox, oy) = self.fit.offset;
        Ok(logical_rect(
            (ox + f64::from(px.x) * s, oy + f64::from(px.y) * s),
            (f64::from(px.width) * s, f64::from(px.height) * s),
        ))
    }

    /// The installed add-ins, read afresh so newly installed ones apply at the next mount. Only
    /// widgets that require plugins pay for the scan.
    fn addins(&self, instance: &WidgetInstance) -> Arc<Registry> {
        let Some(dir) = &self.opts.addins_dir else {
            return Arc::default();
        };
        if instance.def.manifest.required_plugins.is_empty() {
            return Arc::default();
        }
        let (registry, rejected) = Registry::load(dir);
        for r in rejected {
            eprintln!("skipping add-in {}: {}", r.dir.display(), r.reason);
        }
        Arc::new(registry)
    }

    fn shim_config(&self, instance: &WidgetInstance, addins: Arc<Registry>) -> ShimConfig {
        ShimConfig {
            language: self.opts.language.clone(),
            fps_limit: 30,
            is_preview: false,
            instance_id: instance.id,
            device_id: self.device_id,
            temperature_unit: TemperatureUnit::Celsius,
            base_url: format!("{SCHEME}://{}/", instance.id.simple()),
            escape_closes: self.opts.escape_closes,
            debug_input: std::env::var_os("EDGEHOST_DEBUG_INPUT").is_some(),
            addins,
        }
    }

    fn build_view(
        &mut self,
        instance: &WidgetInstance,
        script: &str,
        path: &str,
        bounds: Rect,
        addins: Option<Arc<Registry>>,
    ) -> Result<WebView> {
        // Only shown widgets get add-ins and providers; evaluate views see every plugin as
        // unavailable.
        let visible = addins.is_some();
        let serve_root = instance.root.clone();
        let serve_common = self.opts.common_dir.clone();
        let permissions = Arc::new(instance.def.manifest.url_permissions());
        let label = instance.def.manifest.id.to_string();
        let id = instance.id;
        let sink = Rc::clone(&self.sink);
        let plugin_host = addins.and_then(|a| plugin_host(instance, a));
        let plugin_label = label.clone();
        let devtools = self.opts.devtools;
        let Self {
            window,
            parent,
            storages,
            opts,
            ..
        } = self;
        let storage = storages
            .entry(id)
            .or_insert_with(|| platform::storage(&opts.web_data_dir, id));
        let builder = platform::builder(storage, id)
            .with_bounds(bounds)
            .with_visible(visible)
            .with_transparent(true)
            // The Edge is a second screen: a tap must reach the widget even when EdgeHost is not
            // the active app, instead of only activating it (the first click would be swallowed).
            .with_accept_first_mouse(true)
            .with_devtools(devtools)
            .with_initialization_script_for_main_only(script, true);
        let builder = if visible {
            builder.with_initialization_script_for_main_only(swipe_script(), true)
        } else {
            builder
        };
        let builder = builder
            .with_asynchronous_custom_protocol(SCHEME.to_owned(), move |_id, request, responder| {
                let uri = request.uri();
                if uri.path() == proxy::ROUTE {
                    proxy::handle(
                        Arc::clone(&permissions),
                        uri.query().map(str::to_owned),
                        responder,
                    );
                } else if let Some(route) = uri.path().strip_prefix(plugins::PREFIX) {
                    plugins::handle(
                        plugin_host.clone(),
                        plugin_label.clone(),
                        route.to_owned(),
                        uri.query().map(str::to_owned),
                        responder,
                    );
                } else {
                    responder.respond(serve(&serve_root, serve_common.as_deref(), uri.path()));
                }
            })
            .with_ipc_handler(move |request| {
                if let Some(event) = report(&label, id, request.body()) {
                    sink(event);
                }
            })
            .with_url(format!("{SCHEME}://{}{path}", id.simple()));
        platform::attach(parent, window, builder).context("creating a widget view")
    }
}

/// Providers for `instance`'s supported `required_plugins`, if it has any.
fn plugin_host(instance: &WidgetInstance, addins: Arc<Registry>) -> Option<Arc<PluginHost>> {
    let Some(macros) = Macros::from_env() else {
        eprintln!("warning: $HOME is not set; plugins are disabled");
        return None;
    };
    let host = PluginHost::new(
        &instance.def.manifest.required_plugins,
        &HostContext {
            macros,
            path_permissions: instance.def.manifest.path_permissions(),
            addins,
        },
    );
    (!host.is_empty()).then(|| Arc::new(host))
}

/// The black view behind the widgets; swipes that start between or around widgets land here.
fn background_view(
    parent: &platform::ViewParent,
    window: &Window,
    area: (f64, f64),
    scale: f64,
    sink: &EventSink,
) -> Result<WebView> {
    let view = platform::attach(
        parent,
        window,
        WebViewBuilder::new()
            .with_bounds(logical_rect((0.0, 0.0), area))
            .with_accept_first_mouse(true)
            .with_initialization_script_for_main_only(swipe_script(), true)
            .with_ipc_handler(host_ipc(Rc::clone(sink)))
            .with_html(
                "<!doctype html><html style=\"background:#000;height:100%\">\
                 <body style=\"margin:0;height:100%\"></body></html>",
            ),
    )
    .context("creating the background view")?;
    view.zoom(scale).context("setting background zoom")?;
    Ok(view)
}

/// The swipe detector, called with a poster that sends to the host.
fn swipe_script() -> String {
    format!(
        "{SWIPE_SOURCE}((m) => {{ try {{ window.ipc.postMessage(JSON.stringify(m)); }} catch (_) {{}} }});"
    )
}

/// IPC handler for host views (the background and the page dots).
fn host_ipc(sink: EventSink) -> impl Fn(wry::http::Request<String>) + 'static {
    move |request| {
        if let Some(event) = report("edgehost", Uuid::nil(), request.body()) {
            sink(event);
        }
    }
}

/// The page-dot strip: one button per page, `current` highlighted; a tap posts `{k: "page"}`.
fn pager_html(current: usize, count: usize) -> String {
    let mut dots = String::new();
    for i in 0..count {
        let on = if i == current { " class=\"on\"" } else { "" };
        // Writing to a String cannot fail.
        let _ = write!(
            dots,
            "<button data-index=\"{i}\"{on} aria-label=\"Page {}\"><i></i></button>",
            i + 1
        );
    }
    format!(
        "<!doctype html><html><head><style>\
         html,body{{margin:0;height:100%;background:transparent;overflow:hidden}}\
         body{{display:flex;justify-content:center;align-items:center;box-sizing:border-box;\
         padding:0 {PAGER_PADDING}px;border-radius:{r}px;background:rgba(0,0,0,.45);\
         user-select:none;-webkit-user-select:none}}\
         button{{all:unset;width:{DOT_TARGET}px;height:100%;display:flex;justify-content:center;\
         align-items:center;cursor:pointer}}\
         i{{width:12px;height:12px;border-radius:50%;background:rgba(255,255,255,.4)}}\
         .on i{{background:#fff}}\
         </style></head><body>{dots}<script>\
         let down=null;const dot=(e)=>e.target.closest&&e.target.closest('[data-index]');\
         addEventListener('pointerdown',(e)=>{{down=dot(e);}},true);\
         addEventListener('pointerup',(e)=>{{const b=dot(e);\
         if(b&&b===down)window.ipc.postMessage(JSON.stringify({{k:'page',index:Number(b.dataset.index)}}));\
         down=null;}},true);\
         </script></body></html>",
        r = PAGER_HEIGHT / 2
    )
}

fn logical_rect(origin: (f64, f64), size: (f64, f64)) -> Rect {
    Rect {
        position: wry::dpi::LogicalPosition::new(origin.0, origin.1).into(),
        size: wry::dpi::LogicalSize::new(size.0, size.1).into(),
    }
}

/// Serve a widget file; `/common/…` falls back to the shared folder; [`EVAL_PATH`] is a blank page.
fn serve(root: &Path, common: Option<&Path>, url_path: &str) -> Response<Cow<'static, [u8]>> {
    if url_path == EVAL_PATH {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Cow::Borrowed(
                b"<!doctype html><html><head></head><body></body></html>".as_slice(),
            ))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"".as_slice())));
    }
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

/// Log a message posted by the shim and turn host requests into [`RuntimeEvent`]s.
fn report(label: &str, instance: Uuid, body: &str) -> Option<RuntimeEvent> {
    let Ok(message) = serde_json::from_str::<Value>(body) else {
        eprintln!("[{label}] unparseable message: {body}");
        return None;
    };
    let field = |name: &str| {
        message
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    match field("k") {
        "console" => eprintln!("[{label}] {}: {}", field("level"), field("msg")),
        "booted" => eprintln!("[{label}] booted"),
        "close" => return Some(RuntimeEvent::Close),
        "keyboard" => return Some(RuntimeEvent::WantsKeyboard),
        "swipe" => match message.get("dir").and_then(Value::as_i64) {
            Some(1) => return Some(RuntimeEvent::Swipe { forward: true }),
            Some(-1) => return Some(RuntimeEvent::Swipe { forward: false }),
            _ => eprintln!("[{label}] bad swipe: {body}"),
        },
        "page" => match message
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
        {
            Some(i) => return Some(RuntimeEvent::ShowPage(i)),
            None => eprintln!("[{label}] bad page: {body}"),
        },
        "evaluated" => {
            let values = match message.get("values") {
                Some(Value::Object(map)) => {
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                }
                _ => BTreeMap::new(),
            };
            return Some(RuntimeEvent::Evaluated { instance, values });
        }
        other => eprintln!("[{label}] {other}: {body}"),
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_parses_host_requests() {
        let id = Uuid::from_u128(7);
        assert_eq!(
            report("w", id, r#"{"k":"close"}"#),
            Some(RuntimeEvent::Close)
        );
        assert_eq!(
            report("w", id, r#"{"k":"evaluated","values":{"a":1,"b":"x"}}"#),
            Some(RuntimeEvent::Evaluated {
                instance: id,
                values: BTreeMap::from([
                    ("a".into(), Value::from(1)),
                    ("b".into(), Value::from("x"))
                ]),
            })
        );
        assert_eq!(
            report("w", id, r#"{"k":"swipe","dir":1}"#),
            Some(RuntimeEvent::Swipe { forward: true })
        );
        assert_eq!(
            report("w", id, r#"{"k":"swipe","dir":-1}"#),
            Some(RuntimeEvent::Swipe { forward: false })
        );
        assert_eq!(report("w", id, r#"{"k":"swipe","dir":0}"#), None);
        assert_eq!(
            report("w", id, r#"{"k":"page","index":2}"#),
            Some(RuntimeEvent::ShowPage(2))
        );
        assert_eq!(report("w", id, r#"{"k":"page","index":-1}"#), None);
        assert_eq!(report("w", id, r#"{"k":"booted"}"#), None);
        assert_eq!(report("w", id, "not json"), None);
    }

    #[test]
    fn pager_marks_the_current_page() {
        let html = pager_html(1, 3);
        assert_eq!(html.matches("<button").count(), 3);
        assert!(html.contains(r#"data-index="1" class="on""#));
        assert!(!html.contains(r#"data-index="0" class="on""#));
    }

    #[test]
    fn swipe_script_calls_the_detector() {
        let script = swipe_script();
        assert!(script.starts_with("// EdgeHost page swipes"));
        assert!(script.trim_end().ends_with("});"));
    }

    #[test]
    fn eval_page_is_served_without_touching_the_widget() {
        let resp = serve(Path::new("/nonexistent"), None, EVAL_PATH);
        assert_eq!(resp.status(), 200);
        assert!(resp.body().starts_with(b"<!doctype html>"));
        assert_eq!(
            serve(Path::new("/nonexistent"), None, "/x.js").status(),
            404
        );
    }
}
