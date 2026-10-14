//! `edgectl run-one`: show a single widget on the Edge (or a stand-in window) until closed.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use edge_layout::{Cell, DisplayChoice, Orientation, Slot};
use icuew_manifest::WidgetDef;
use tao::event::{ElementState, Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::keyboard::KeyCode;
use uuid::Uuid;

use crate::surface::{EventSink, NAMESPACE, RuntimeEvent, Surface, SurfaceOptions, WidgetInstance};

/// Options for [`run_one`].
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Unpacked widget directory.
    pub widget_dir: PathBuf,
    /// Slot size to run the widget at.
    pub slot: Slot,
    /// Top-left grid cell of the slot.
    pub origin: Cell,
    /// Which display to use.
    pub display: DisplayChoice,
    /// Orientation for the stand-in window (a real display reports its own).
    pub orientation: Orientation,
    /// Stand-in window size as a fraction of the panel's pixel size.
    pub windowed_scale: f64,
    /// Enable the Web Inspector.
    pub devtools: bool,
    /// Shared `common/` folder served at `/common/` when the widget has no copy of its own.
    /// iCUE ships one beside its built-in widgets; defaults to a `common` folder beside the widget.
    pub common_dir: Option<PathBuf>,
}

/// Run a single widget until its window is closed or Escape is pressed.
///
/// The widget's defaults are evaluated in a hidden view first, then the widget is shown with
/// those concrete values (the same path the EdgeHost app uses).
///
/// # Errors
///
/// Fails if the widget cannot be parsed, the slot does not fit the grid, or the window or web
/// views cannot be created. On success this function does not return: the event loop exits the
/// process when the window closes.
pub fn run_one(opts: &RunOptions) -> Result<()> {
    let (root, def) = load_widget(&opts.widget_dir)?;
    let common_dir = shared_common_dir(opts.common_dir.as_deref(), &root);
    let event_loop = EventLoopBuilder::<RuntimeEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let sink: EventSink = Rc::new(move |event| {
        // Sending only fails once the loop has exited, when nothing is listening anyway.
        let _ = proxy.send_event(event);
    });
    let mut surface = Surface::open(
        &event_loop,
        SurfaceOptions {
            display: opts.display.clone(),
            windowed_orientation: opts.orientation,
            windowed_scale: opts.windowed_scale,
            devtools: opts.devtools,
            common_dir,
            language: "en".to_owned(),
            escape_closes: true,
            web_data_dir: std::env::temp_dir().join("edgehost-run-one"),
        },
        sink,
    )?;
    let instance = WidgetInstance {
        id: Uuid::new_v5(
            &NAMESPACE,
            format!("run-one:{}:{}", def.manifest.id, opts.slot).as_bytes(),
        ),
        root,
        def,
        slot: opts.slot,
        origin: opts.origin,
    };
    surface.evaluate(&instance)?;
    eprintln!(
        "running {} at {} on {} ({} grid)",
        instance.def.manifest.id,
        opts.slot,
        surface.description(),
        surface.orientation()
    );

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(RuntimeEvent::Evaluated {
                instance: id,
                values,
            }) if id == instance.id => {
                surface.end_evaluation(id);
                if let Err(e) = surface.mount(&instance, &values) {
                    eprintln!("error: {e:#}");
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(RuntimeEvent::WantsKeyboard) => surface.take_keyboard(),
            Event::UserEvent(RuntimeEvent::Close)
            | Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } if surface.on_display()
                && event.physical_key == KeyCode::Escape
                && event.state == ElementState::Pressed =>
            {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
}

/// Canonicalise and parse a widget directory, warning if it does not target the Edge.
fn load_widget(dir: &Path) -> Result<(PathBuf, WidgetDef)> {
    let root = dir
        .canonicalize()
        .with_context(|| format!("opening widget directory {}", dir.display()))?;
    let def =
        WidgetDef::load(&root).with_context(|| format!("loading widget {}", root.display()))?;
    if !def.manifest.supports_edge() {
        eprintln!(
            "warning: {} does not list dashboard_lcd (XENEON EDGE) in supported_devices",
            def.manifest.id
        );
    }
    Ok((root, def))
}

/// The `common/` folder to fall back to, if any (canonicalised).
fn shared_common_dir(explicit: Option<&Path>, widget_root: &Path) -> Option<PathBuf> {
    let candidate = match explicit {
        Some(dir) => dir.to_path_buf(),
        None => widget_root.parent()?.join("common"),
    };
    let dir = candidate.canonicalize().ok().filter(|d| d.is_dir());
    if dir.is_none() && explicit.is_some() {
        eprintln!(
            "warning: --common-dir {} is not a directory; ignoring it",
            candidate.display()
        );
    }
    dir
}
