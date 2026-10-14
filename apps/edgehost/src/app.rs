//! App state: the store, the loaded layout, the surface and the current page.

use std::rc::Rc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use edge_layout::DisplayChoice;
use edge_store::{InstanceId, Layout, Store};
use icuew_manifest::WidgetDef;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::WindowId;
use widget_runtime::{
    EventSink, RuntimeEvent, Surface, SurfaceOptions, WidgetInstance, list_displays,
};

use crate::AppEvent;
use crate::config_api::{self, Outcome, Request};
use crate::configurator::Configurator;
use crate::menus::{Menus, ids};

/// The running dashboard.
pub struct App {
    store: Store,
    display_override: Option<DisplayChoice>,
    devtools: bool,
    proxy: EventLoopProxy<AppEvent>,
    layout: Layout,
    layout_mtime: Option<SystemTime>,
    page: usize,
    surface: Option<Surface>,
    /// Instances on the current page, with their parsed widgets.
    shown: Vec<WidgetInstance>,
    menus: Option<Menus>,
    configurator: Option<Configurator>,
    configurator_connected: bool,
    open_configurator_at_start: bool,
}

impl App {
    /// Create the app; nothing is shown until [`App::start`].
    pub fn new(
        store: Store,
        display_override: Option<DisplayChoice>,
        devtools: bool,
        open_configurator_at_start: bool,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        Self {
            store,
            display_override,
            devtools,
            proxy,
            layout: Layout::default(),
            layout_mtime: None,
            page: 0,
            surface: None,
            shown: Vec::new(),
            menus: None,
            configurator: None,
            configurator_connected: false,
            open_configurator_at_start,
        }
    }

    /// Install menus, load the layout and show the first page.
    ///
    /// # Errors
    ///
    /// If the menus, the layout or the surface cannot be created.
    pub fn start(&mut self, target: &EventLoopWindowTarget<AppEvent>) -> Result<()> {
        self.menus = Some(Menus::install()?);
        self.reload(target)?;
        if self.open_configurator_at_start || self.layout.instances().next().is_none() {
            self.open_configurator(target);
        }
        Ok(())
    }

    /// Re-read `layout.json`, reopen the surface and show the current page.
    fn reload(&mut self, target: &EventLoopWindowTarget<AppEvent>) -> Result<()> {
        self.layout_mtime = self.layout_file_mtime();
        self.layout = self.store.load_layout().context("loading layout.json")?;
        let display = self.display_override.clone().unwrap_or_else(|| {
            self.layout
                .display
                .as_deref()
                .map_or(DisplayChoice::Auto, |d| d.parse().unwrap_or_default())
        });
        self.shown.clear();
        self.surface = None;
        let proxy = self.proxy.clone();
        let sink: EventSink = Rc::new(move |e| {
            // Fails only after the loop has exited.
            let _ = proxy.send_event(AppEvent::Runtime(e));
        });
        let surface = Surface::open(
            target,
            SurfaceOptions {
                display,
                windowed_orientation: self.layout.orientation,
                windowed_scale: 0.5,
                devtools: self.devtools,
                common_dir: self.store.common_dir(),
                language: "en".to_owned(),
                escape_closes: false,
                web_data_dir: self.store.root().join("webdata"),
                addins_dir: Some(self.store.addins_dir()),
            },
            sink,
        )?;
        if surface.orientation() != self.layout.orientation {
            eprintln!(
                "warning: the display is {} but the layout is for {}; widgets that do not fit are skipped",
                surface.orientation(),
                self.layout.orientation
            );
        }
        eprintln!("EdgeHost on {}", surface.description());
        self.surface = Some(surface);
        if self.page >= self.layout.pages.len() {
            self.page = 0;
        }
        self.show_page();
        Ok(())
    }

    /// Mount the current page, evaluating defaults first where needed.
    fn show_page(&mut self) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        surface.unmount_all();
        self.shown.clear();
        if let Some(menus) = &self.menus {
            menus.set_page(self.page, self.layout.pages.len());
        }
        let pages = self.layout.pages.len();
        let Some(page) = self.layout.pages.get(self.page) else {
            eprintln!(
                "the layout is empty: add widgets in Configure Widgets… (menu bar) or with `edgectl layout add`"
            );
            return;
        };
        for inst in &page.instances {
            let root = self.store.widget_dir(&inst.widget, &inst.version);
            let def = match WidgetDef::load(&root) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("skipping {} {}: {e}", inst.widget, inst.version);
                    continue;
                }
            };
            let widget = WidgetInstance {
                id: inst.id.0,
                root,
                def,
                slot: inst.slot,
                origin: inst.origin,
            };
            let result = if inst.needs_evaluation() {
                surface.evaluate(&widget)
            } else {
                surface.mount(&widget, &inst.values)
            };
            match result {
                Ok(()) => self.shown.push(widget),
                Err(e) => eprintln!("skipping {} ({}): {e:#}", inst.widget, inst.id.short()),
            }
        }
        if let Err(e) = surface.set_pager(self.page, pages) {
            eprintln!("warning: {e:#}");
        }
    }

    /// Show the next (`forward`) or previous page, wrapping around.
    fn step_page(&mut self, forward: bool) {
        let pages = self.layout.pages.len();
        if pages < 2 {
            return;
        }
        self.page = if forward {
            (self.page + 1) % pages
        } else {
            (self.page + pages - 1) % pages
        };
        self.show_page();
    }

    /// Show page `page` if it exists and is not already shown.
    fn go_to_page(&mut self, page: usize) {
        if page < self.layout.pages.len() && page != self.page {
            self.page = page;
            self.show_page();
        }
    }

    /// Handle a report from a widget view.
    pub fn on_runtime(&mut self, event: RuntimeEvent) {
        let (instance, values) = match event {
            RuntimeEvent::Evaluated { instance, values } => (instance, values),
            RuntimeEvent::WantsKeyboard => {
                if let Some(surface) = &self.surface {
                    surface.take_keyboard();
                }
                return;
            }
            RuntimeEvent::Swipe { forward } => {
                self.step_page(forward);
                return;
            }
            RuntimeEvent::ShowPage(page) => {
                self.go_to_page(page);
                return;
            }
            RuntimeEvent::Close => return,
        };
        let id = InstanceId(instance);
        // Persist even if the page changed meanwhile; the defaults are still valid.
        if let Err(e) = self.layout.apply_evaluated(id, values) {
            eprintln!("evaluated defaults for an unknown instance: {e}");
            return;
        }
        if let Err(e) = self.store.save_layout(&self.layout) {
            eprintln!("saving layout.json: {e:#}");
        }
        self.layout_mtime = self.layout_file_mtime();
        if let Some(c) = &self.configurator {
            c.notify_changed();
        }
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        surface.end_evaluation(instance);
        let values = self
            .layout
            .instances()
            .find(|(_, i)| i.id == id)
            .map(|(_, i)| i.values.clone());
        if let (Some(widget), Some(values)) = (self.shown.iter().find(|w| w.id == instance), values)
            && let Err(e) = surface.mount(widget, &values)
        {
            eprintln!("showing {}: {e:#}", widget.def.manifest.id);
        }
    }

    /// Handle a menu choice. Returns `true` to quit.
    pub fn on_menu(&mut self, id: &str, target: &EventLoopWindowTarget<AppEvent>) -> bool {
        match id {
            ids::QUIT => return true,
            ids::NEXT => self.step_page(true),
            ids::PREVIOUS => self.step_page(false),
            ids::RELOAD => self.reload_or_report(target),
            ids::CONFIGURE => self.open_configurator(target),
            ids::OPEN_DATA => {
                if let Err(e) = widget_runtime::platform::reveal(self.store.root()) {
                    eprintln!("opening {}: {e}", self.store.root().display());
                }
            }
            _ => {}
        }
        false
    }

    /// Reload if `layout.json` changed on disk (for example via `edgectl layout …`).
    pub fn poll_layout(&mut self, target: &EventLoopWindowTarget<AppEvent>) {
        if self.layout_file_mtime() != self.layout_mtime {
            eprintln!("layout.json changed; reloading");
            self.reload_or_report(target);
            if let Some(c) = &self.configurator {
                c.notify_changed();
            }
        }
    }

    /// Open the configurator, or bring it to the front.
    pub fn open_configurator(&mut self, target: &EventLoopWindowTarget<AppEvent>) {
        if let Some(c) = &self.configurator {
            c.focus();
            return;
        }
        match Configurator::open(
            target,
            self.proxy.clone(),
            self.store.widgets_dir(),
            self.devtools,
        ) {
            Ok(c) => self.configurator = Some(c),
            Err(e) => eprintln!("opening the configurator: {e:#}"),
        }
    }

    /// Handle a configurator request: apply it, persist and show the result, and reply.
    pub fn on_config(&mut self, body: &str, target: &EventLoopWindowTarget<AppEvent>) {
        let request: Request = match serde_json::from_str(body) {
            Ok(r) => r,
            Err(e) => {
                match serde_json::from_str::<serde_json::Value>(body) {
                    Ok(v) if v.get("k").and_then(serde_json::Value::as_str) == Some("console") => {
                        eprintln!(
                            "[configurator] {}",
                            v.get("msg")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                        );
                    }
                    _ => eprintln!("configurator: unparseable request: {e}"),
                }
                return;
            }
        };
        let displays = list_displays(target);
        let outcome = config_api::handle(
            &self.store,
            &mut self.layout,
            &displays,
            &request.method,
            &request.params,
        );
        if request.method == "state.get" && !self.configurator_connected {
            self.configurator_connected = true;
            if let Ok(Outcome::Read(state)) = &outcome {
                let count = |key: &str| {
                    state
                        .get(key)
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len)
                };
                eprintln!(
                    "configurator connected: {} widget(s) installed, {} page(s)",
                    count("widgets"),
                    count("pages")
                );
            }
        }
        if request.method != "state.get" {
            let status = outcome
                .as_ref()
                .map_or_else(|e| format!("error: {e:#}"), |_| "ok".to_owned());
            eprintln!(
                "configurator: {} {} → {status}",
                request.method, request.params
            );
        }
        let reply = match outcome {
            Ok(Outcome::Read(result)) => Ok(result),
            Ok(Outcome::Changed(result)) => self.persist().map(|()| {
                if self.page >= self.layout.pages.len() {
                    self.page = self.layout.pages.len().saturating_sub(1);
                }
                self.show_page();
                result
            }),
            Ok(Outcome::Reopen(result)) => self.persist().map(|()| {
                self.reload_or_report(target);
                result
            }),
            Err(e) => {
                // Discard a partial in-memory change by re-reading the saved layout.
                if let Ok(saved) = self.store.load_layout() {
                    self.layout = saved;
                }
                Err(format!("{e:#}"))
            }
        };
        if let Some(c) = &self.configurator {
            c.reply(request.id, reply);
        }
    }

    /// A window's close button was pressed. Returns `true` to quit (the Edge surface closed).
    pub fn on_close(&mut self, window: WindowId) -> bool {
        if self
            .configurator
            .as_ref()
            .is_some_and(|c| c.window_id() == window)
        {
            self.configurator = None;
            return false;
        }
        true
    }

    fn persist(&mut self) -> Result<(), String> {
        self.store
            .save_layout(&self.layout)
            .map_err(|e| format!("saving layout.json: {e:#}"))?;
        self.layout_mtime = self.layout_file_mtime();
        Ok(())
    }

    fn reload_or_report(&mut self, target: &EventLoopWindowTarget<AppEvent>) {
        if let Err(e) = self.reload(target) {
            eprintln!("reload failed: {e:#}");
        }
    }

    fn layout_file_mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(self.store.layout_path())
            .and_then(|m| m.modified())
            .ok()
    }
}
