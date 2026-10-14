//! EdgeHost: shows the saved dashboard (`layout.json`) on the XENEON EDGE.
//!
//! A regular Dock app with an application menu and a menu-bar item (page switching, reload, data
//! folder, quit). Widgets are edited with `edgectl layout …`; EdgeHost notices changes to
//! `layout.json` within a couple of seconds and reloads.

mod app;
mod menus;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use edge_layout::DisplayChoice;
use edge_store::Store;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::MenuEvent;
use widget_runtime::RuntimeEvent;

use crate::app::App;

/// How often `layout.json` is checked for outside edits.
const POLL: Duration = Duration::from_secs(2);

#[derive(Parser)]
#[command(
    name = "edgehost",
    version,
    about = "iCUE HTML widget dashboard for the XENEON EDGE"
)]
struct Args {
    /// Data folder (default: `$EDGEHOST_HOME` or `~/Library/Application Support/EdgeHost`).
    #[arg(long, env = "EDGEHOST_HOME")]
    home: Option<PathBuf>,
    /// Override the layout's display: auto, windowed, or an index / name from `edgectl displays`.
    #[arg(long)]
    display: Option<DisplayChoice>,
    /// Enable the Safari Web Inspector for widgets.
    #[arg(long)]
    devtools: bool,
}

/// Everything the event loop reacts to besides window events.
#[derive(Debug)]
pub enum AppEvent {
    /// A widget view reported something.
    Runtime(RuntimeEvent),
    /// A menu item was chosen.
    Menu(String),
}

fn main() -> Result<()> {
    let args = Args::parse();
    let root = match args.home {
        Some(h) => h,
        None => Store::default_root()?,
    };
    let store =
        Store::open(&root).with_context(|| format!("opening data folder {}", root.display()))?;

    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        // Fails only after the loop has exited.
        let _ = menu_proxy.send_event(AppEvent::Menu(event.id.0));
    }));
    let mut app = App::new(
        store,
        args.display,
        args.devtools,
        event_loop.create_proxy(),
    );

    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL);
        match event {
            Event::NewEvents(StartCause::Init) => {
                if let Err(e) = app.start(target) {
                    eprintln!("error: {e:#}");
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => app.poll_layout(target),
            Event::UserEvent(AppEvent::Runtime(e)) => app.on_runtime(e),
            Event::UserEvent(AppEvent::Menu(id)) => {
                if app.on_menu(&id, target) {
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}
