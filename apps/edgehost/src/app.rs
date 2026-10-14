//! App state: the store, the loaded layout, the surface and the current page.

use std::rc::Rc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use edge_layout::DisplayChoice;
use edge_store::{InstanceId, Layout, Store};
use icuew_manifest::WidgetDef;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use widget_runtime::{EventSink, RuntimeEvent, Surface, SurfaceOptions, WidgetInstance};

use crate::AppEvent;
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
}

impl App {
    /// Create the app; nothing is shown until [`App::start`].
    pub fn new(
        store: Store,
        display_override: Option<DisplayChoice>,
        devtools: bool,
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
        }
    }

    /// Install menus, load the layout and show the first page.
    ///
    /// # Errors
    ///
    /// If the menus, the layout or the surface cannot be created.
    pub fn start(&mut self, target: &EventLoopWindowTarget<AppEvent>) -> Result<()> {
        self.menus = Some(Menus::install()?);
        self.reload(target)
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
        let Some(page) = self.layout.pages.get(self.page) else {
            eprintln!("the layout is empty: add widgets with `edgectl layout add <widget-id>`");
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
        {
            if let Err(e) = surface.mount(widget, &values) {
                eprintln!("showing {}: {e:#}", widget.def.manifest.id);
            }
        }
    }

    /// Handle a menu choice. Returns `true` to quit.
    pub fn on_menu(&mut self, id: &str, target: &EventLoopWindowTarget<AppEvent>) -> bool {
        let pages = self.layout.pages.len();
        match id {
            ids::QUIT => return true,
            ids::NEXT if pages > 0 => {
                self.page = (self.page + 1) % pages;
                self.show_page();
            }
            ids::PREVIOUS if pages > 0 => {
                self.page = (self.page + pages - 1) % pages;
                self.show_page();
            }
            ids::RELOAD => self.reload_or_report(target),
            ids::OPEN_DATA => {
                if let Err(e) = std::process::Command::new("open")
                    .arg(self.store.root())
                    .spawn()
                {
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
        }
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
