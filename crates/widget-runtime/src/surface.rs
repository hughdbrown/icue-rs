//! The Edge surface: one window covering the chosen display, a black background, and one web
//! view per widget instance, plus hidden "evaluate" views that compute property defaults.
//!
//! Geometry: widgets are laid out in panel pixels ([`edge_layout`]) and the panel is fitted into the
//! window ([`edge_layout::fit_panel`]): `scale` logical points per panel pixel, letterboxed when the
//! display is not in its native mode. Each view's bounds are `offset + slot × scale` and its page
//! zoom is `scale`, so the widget always sees a CSS viewport equal to its slot size.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use edge_layout::{
    Cell, DisplayChoice, Fit, Orientation, PxRect, Selection, Slot, fit_panel, select_display,
    slot_rect,
};
use icuew_manifest::WidgetDef;
use serde_json::Value;
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder};
use uuid::Uuid;
use wry::http::{Response, header};
use wry::{Rect, WebView, WebViewBuilder};

use crate::assets::{mime_for, read_asset, resolve};
use crate::display::list_displays;
use crate::shim::{self, ShimConfig, ShimMode, TemperatureUnit};

/// URL scheme widgets are served from.
const SCHEME: &str = "icuew";
/// Blank page the evaluate views load (reserved path; never a widget file).
const EVAL_PATH: &str = "/__edgehost/eval.html";
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
    _background: WebView,
    orientation: Orientation,
    fit: Fit,
    on_display: bool,
    description: String,
    device_id: Uuid,
    opts: SurfaceOptions,
    sink: EventSink,
    views: HashMap<Uuid, WebView>,
    evaluations: HashMap<Uuid, WebView>,
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
                         Fix: `brew install displayplacer`, then `displayplacer list` and select its \
                         res:2560x720 mode (see docs/research/edge-hardware.md).",
                        d.pixel_size.0, d.pixel_size.1
                    );
                }
                let builder = builder
                    .with_decorations(false)
                    .with_position(LogicalPosition::new(d.logical_origin.0, d.logical_origin.1))
                    .with_inner_size(LogicalSize::new(d.logical_size.0, d.logical_size.1))
                    .with_visible_on_all_workspaces(true);
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
        let background = WebViewBuilder::new()
            .with_bounds(logical_rect((0.0, 0.0), area))
            .with_html("<!doctype html><html style=\"background:#000;height:100%\"></html>")
            .build_as_child(&window)
            .context("creating the background view")?;
        window.set_focus();
        Ok(Self {
            window,
            _background: background,
            orientation,
            fit: fit_panel(orientation, area),
            on_display: matches!(selection, Selection::Display { .. }),
            description,
            device_id: Uuid::new_v5(&NAMESPACE, device_key.as_bytes()),
            opts,
            sink,
            views: HashMap::new(),
            evaluations: HashMap::new(),
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

    /// Start computing `instance`'s property defaults in a hidden view. The result arrives as
    /// [`RuntimeEvent::Evaluated`]; call [`Surface::end_evaluation`] afterwards.
    ///
    /// # Errors
    ///
    /// The slot does not fit the grid, or the view cannot be created.
    pub fn evaluate(&mut self, instance: &WidgetInstance) -> Result<()> {
        let rect = self.slot_bounds(instance)?;
        let script = shim::render(
            &instance.def,
            &self.shim_config(instance),
            ShimMode::Evaluate,
        )
        .context("rendering the evaluate shim")?;
        let view = self.build_view(instance, &script, EVAL_PATH, rect, false)?;
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
        let script = shim::render(
            &instance.def,
            &self.shim_config(instance),
            ShimMode::Runtime(values),
        )
        .context("rendering the widget shim")?;
        self.views.remove(&instance.id);
        let view = self.build_view(instance, &script, "/index.html", rect, true)?;
        view.zoom(self.fit.scale).context("setting widget zoom")?;
        if self.opts.escape_closes {
            if let Err(e) = view.focus() {
                eprintln!("warning: could not focus the widget view: {e}");
            }
        }
        self.views.insert(instance.id, view);
        Ok(())
    }

    /// Remove every widget and evaluate view.
    pub fn unmount_all(&mut self) {
        self.views.clear();
        self.evaluations.clear();
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

    fn shim_config(&self, instance: &WidgetInstance) -> ShimConfig {
        ShimConfig {
            language: self.opts.language.clone(),
            fps_limit: 30,
            is_preview: false,
            instance_id: instance.id,
            device_id: self.device_id,
            temperature_unit: TemperatureUnit::Celsius,
            base_url: format!("{SCHEME}://{}/", instance.id.simple()),
            escape_closes: self.opts.escape_closes,
        }
    }

    fn build_view(
        &self,
        instance: &WidgetInstance,
        script: &str,
        path: &str,
        bounds: Rect,
        visible: bool,
    ) -> Result<WebView> {
        let serve_root = instance.root.clone();
        let serve_common = self.opts.common_dir.clone();
        let label = instance.def.manifest.id.to_string();
        let id = instance.id;
        let sink = Rc::clone(&self.sink);
        let builder = WebViewBuilder::new()
            .with_bounds(bounds)
            .with_visible(visible)
            .with_transparent(true)
            // The Edge is a second screen: a tap must reach the widget even when EdgeHost is not
            // the active app, instead of only activating it (the first click would be swallowed).
            .with_accept_first_mouse(true)
            .with_devtools(self.opts.devtools)
            .with_initialization_script_for_main_only(script, true)
            .with_custom_protocol(SCHEME.to_owned(), move |_id, request| {
                serve(&serve_root, serve_common.as_deref(), request.uri().path())
            })
            .with_ipc_handler(move |request| {
                if let Some(event) = report(&label, id, request.body()) {
                    sink(event);
                }
            })
            .with_url(format!("{SCHEME}://{}{path}", id.simple()));
        #[cfg(target_os = "macos")]
        let builder = {
            use wry::WebViewBuilderExtDarwin;
            // Per-instance WebKit data store (macOS 14+): isolated localStorage/IndexedDB, as iCUE does.
            builder.with_data_store_identifier(*id.as_bytes())
        };
        builder
            .build_as_child(&self.window)
            .context("creating a widget view")
    }
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
        assert_eq!(report("w", id, r#"{"k":"booted"}"#), None);
        assert_eq!(report("w", id, "not json"), None);
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
