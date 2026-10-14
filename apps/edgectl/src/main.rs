//! `edgectl`: EdgeHost developer and administration CLI.

mod addin;
mod approve;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use edge_layout::{Cell, DisplayChoice, Orientation, Slot};
use edge_plugins::addin::Registry;
use edge_store::{InstallError, Store};
use icuew_manifest::{Report, Severity, WidgetDef, WidgetId, validate_dir};
use serde_json::Value;
use tao::event_loop::EventLoop;
use widget_runtime::{RunOptions, list_displays, run_one};

#[derive(Parser)]
#[command(
    name = "edgectl",
    version,
    about = "EdgeHost tools for iCUE HTML widgets on the XENEON EDGE"
)]
struct Cli {
    /// Data folder (default: `$EDGEHOST_HOME`, else the platform data folder).
    #[arg(long, global = true, env = "EDGEHOST_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate a widget folder or .icuewidget/.zip package and print what EdgeHost sees.
    Validate {
        /// Widget folder or package.
        path: PathBuf,
    },
    /// Install a widget folder or .icuewidget/.zip package, after approving what it may access.
    Install {
        /// Widget folder or package.
        path: PathBuf,
        /// Approve the widget's declared permissions without asking.
        #[arg(long)]
        yes: bool,
    },
    /// List installed widgets.
    List,
    /// Remove an installed widget (every version unless --version is given).
    Uninstall {
        /// Widget id, e.g. com.corsair.airquality.
        id: WidgetId,
        /// Only this version.
        #[arg(long)]
        version: Option<String>,
        /// Also remove layout instances that use it.
        #[arg(long)]
        force: bool,
    },
    /// List connected displays and which one EdgeHost would use.
    Displays,
    /// Show or edit the saved dashboard layout.
    Layout {
        #[command(subcommand)]
        command: LayoutCommand,
    },
    /// Check, install, list, remove or try out Rhai add-ins (data providers for widgets).
    Addin {
        #[command(subcommand)]
        command: addin::AddinCommand,
    },
    /// Run one widget folder on the XENEON EDGE, or in a scaled window when no Edge is attached.
    RunOne {
        /// Unpacked widget directory.
        dir: PathBuf,
        /// Slot size: S, M, L or XL.
        #[arg(long, default_value = "L")]
        slot: Slot,
        /// Grid column of the slot's top-left cell.
        #[arg(long, default_value_t = 0)]
        col: u32,
        /// Grid row of the slot's top-left cell.
        #[arg(long, default_value_t = 0)]
        row: u32,
        /// Display: auto, windowed, or an index / name from `edgectl displays`.
        #[arg(long, default_value = "auto")]
        display: DisplayChoice,
        /// Shorthand for --display windowed.
        #[arg(long)]
        windowed: bool,
        /// Orientation of the stand-in window: landscape or portrait.
        #[arg(long, default_value = "landscape")]
        orientation: Orientation,
        /// Stand-in window size relative to the panel.
        #[arg(long, default_value_t = 0.5)]
        scale: f64,
        /// Enable the Safari Web Inspector for the widget.
        #[arg(long)]
        devtools: bool,
        /// Shared `common/` folder (iCUE's widgets/common) served at /common/.
        /// Defaults to a `common` folder beside the widget directory.
        #[arg(long)]
        common_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum LayoutCommand {
    /// Print pages and widget instances.
    Show,
    /// Place an installed widget on a page.
    Add {
        /// Installed widget id.
        widget: WidgetId,
        /// Slot size: S, M, L or XL.
        #[arg(long, default_value = "S")]
        slot: Slot,
        /// Page index (use the current page count to add a new page).
        #[arg(long, default_value_t = 0)]
        page: usize,
        /// Grid column (requires --row); default: first free position.
        #[arg(long, requires = "row")]
        col: Option<u32>,
        /// Grid row (requires --col).
        #[arg(long, requires = "col")]
        row: Option<u32>,
        /// Installed version to use (default: newest).
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove an instance (by id prefix, as shown by `layout show`).
    Remove {
        /// Instance id or unique prefix.
        instance: String,
    },
    /// Set a property value. VALUE is parsed as JSON, falling back to a plain string.
    Set {
        /// Instance id or unique prefix.
        instance: String,
        /// Property name (an x-icue-property `content`).
        key: String,
        /// Value, e.g. '#ff0000', 42, true, '{"pathToAsset": "…"}'.
        value: String,
    },
    /// Forget a user-set value so the widget's default applies again.
    Unset {
        /// Instance id or unique prefix.
        instance: String,
        /// Property name.
        key: String,
    },
    /// Choose the display the dashboard uses: auto, windowed, or an index / name.
    Display {
        /// Display choice.
        choice: DisplayChoice,
    },
    /// Set the grid orientation pages are laid out for.
    Orientation {
        /// landscape or portrait.
        orientation: Orientation,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = || -> Result<Store> {
        let root = match &cli.home {
            Some(h) => h.clone(),
            None => Store::default_root()?,
        };
        Store::open(&root).with_context(|| format!("opening data folder {}", root.display()))
    };
    match cli.command {
        Command::Validate { ref path } => validate(path, &addins(store().ok().as_ref())),
        Command::Install { ref path, yes } => install(&store()?, path, yes),
        Command::List => list(&store()?),
        Command::Uninstall {
            ref id,
            ref version,
            force,
        } => uninstall(&store()?, id, version.as_deref(), force),
        Command::Displays => displays(),
        Command::Layout { ref command } => layout(&store()?, command),
        Command::Addin { ref command } => addin::run(&store, command),
        Command::RunOne {
            dir,
            slot,
            col,
            row,
            display,
            windowed,
            orientation,
            scale,
            devtools,
            common_dir,
        } => run_one(&RunOptions {
            widget_dir: dir,
            slot,
            origin: Cell { col, row },
            display: if windowed {
                DisplayChoice::Windowed
            } else {
                display
            },
            orientation,
            windowed_scale: scale,
            devtools,
            common_dir,
            addins_dir: store().ok().map(|s| s.addins_dir()),
        }),
    }
}

/// The add-ins installed in `store` (none without a store), reporting unusable ones.
fn addins(store: Option<&Store>) -> Registry {
    let Some(store) = store else {
        return Registry::default();
    };
    let (registry, rejected) = Registry::load(&store.addins_dir());
    for r in rejected {
        eprintln!("skipping add-in {}: {}", r.dir.display(), r.reason);
    }
    registry
}

fn validate(path: &Path, addins: &Registry) -> Result<()> {
    let staging = tempfile::tempdir().context("creating a temporary folder")?;
    let root = if path.is_dir() {
        path.to_path_buf()
    } else {
        edge_store::unpack(path, staging.path())
            .with_context(|| format!("unpacking {}", path.display()))?
    };
    let report = validate_dir(&root);
    if let Some(def) = &report.widget {
        describe(def, addins);
    }
    print_report(&report);
    if report.has_errors() {
        bail!("{} is not a valid widget", path.display());
    }
    println!("ok");
    Ok(())
}

fn describe(def: &WidgetDef, addins: &Registry) {
    let m = &def.manifest;
    println!("{} {} — {}", m.id, m.version, m.name);
    println!("  author:       {}", m.author);
    println!(
        "  framework:    {} (min iCUE {})",
        m.min_framework_version,
        m.min_app_version.as_deref().unwrap_or("?")
    );
    println!("  interactive:  {}", m.interactive);
    let devices: Vec<String> = m
        .supported_devices
        .iter()
        .map(|d| format!("{:?}", d.kind))
        .collect();
    println!("  devices:      {}", devices.join(", "));
    for p in &m.required_plugins {
        let status = match (edge_plugins::builtin(p), addins.find(p)) {
            (Some(d), _) if d.satisfies(p) => "built in".to_owned(),
            (Some(d), _) => format!("built in at {}.{}, older than required", d.major, d.minor),
            (None, Some(a)) => format!("add-in {}", a.id),
            (None, None) => "not available in EdgeHost".to_owned(),
        };
        println!(
            "  plugin:       {p}  → window.plugins.{} ({status})",
            p.facade_name()
        );
    }
    for module in &m.modules {
        println!("  module:       {module}");
    }
    println!(
        "  title:        {}",
        def.head.title.as_deref().unwrap_or("(missing)")
    );
    for p in &def.head.properties {
        println!(
            "  property:     {:<20} {:<26} default = {}",
            p.name.as_str(),
            format!("{:?}", p.kind),
            p.default_expr().unwrap_or("(none)")
        );
    }
    let languages: Vec<&str> = def.translations.languages().collect();
    println!(
        "  translations: {}",
        if languages.is_empty() {
            "(none)".to_owned()
        } else {
            languages.join(", ")
        }
    );
}

fn print_report(report: &Report) {
    for d in &report.diagnostics {
        let tag = match d.severity {
            Severity::Error => "error:  ",
            Severity::Warning => "warning:",
        };
        println!("  {tag}     {}", d.message);
    }
}

/// The validated widget in `path` (folder or package), if it validates; `install` reports errors.
fn peek_widget(path: &Path) -> Option<WidgetDef> {
    let staging = tempfile::tempdir().ok()?;
    let root = if path.is_dir() {
        path.to_path_buf()
    } else {
        edge_store::unpack(path, staging.path()).ok()?
    };
    let report = validate_dir(&root);
    if report.has_errors() {
        None
    } else {
        report.widget
    }
}

fn install(store: &Store, path: &Path, yes: bool) -> Result<()> {
    if let Some(def) = peek_widget(path) {
        let m = &def.manifest;
        approve::confirm(
            &format!("{} {}", m.id, m.version),
            &approve::widget_permissions(&m.permissions),
            yes,
        )?;
    }
    match edge_store::install(store, path) {
        Ok(out) => {
            print_report(&out.report);
            let verb = if out.replaced {
                "reinstalled"
            } else {
                "installed"
            };
            println!("{verb} {} {} → {}", out.id, out.version, out.path.display());
            Ok(())
        }
        Err(InstallError::Invalid(report)) => {
            print_report(&report);
            bail!(
                "{} failed validation; nothing was installed",
                path.display()
            )
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!("installing {}", path.display()))),
    }
}

fn list(store: &Store) -> Result<()> {
    let installed = store.installed()?;
    if installed.is_empty() {
        println!("no widgets installed in {}", store.widgets_dir().display());
    }
    for w in installed {
        println!("{:<40} {:<10} {}", w.id.as_str(), w.version, w.name);
    }
    Ok(())
}

fn uninstall(store: &Store, id: &WidgetId, version: Option<&str>, force: bool) -> Result<()> {
    let mut layout = store.load_layout()?;
    let users: Vec<_> = layout
        .instances()
        .filter(|(_, i)| &i.widget == id && version.is_none_or(|v| v == i.version))
        .map(|(_, i)| i.id)
        .collect();
    if !users.is_empty() && !force {
        bail!(
            "{} layout instance(s) use {id}; remove them first or pass --force",
            users.len()
        );
    }
    for instance in users {
        layout.remove(instance)?;
    }
    store.save_layout(&layout)?;
    for v in store.uninstall(id, version)? {
        println!("removed {id} {v}");
    }
    Ok(())
}

fn displays() -> Result<()> {
    let event_loop = EventLoop::new();
    let all = list_displays(&event_loop);
    let chosen = edge_layout::select_display(&all, &DisplayChoice::Auto)?;
    for (i, d) in all.iter().enumerate() {
        let ids = match (d.vendor, d.model) {
            (Some(v), Some(m)) => format!("edid {v:04x}:{m:04x}"),
            _ => "edid ?".to_owned(),
        };
        let mut notes = Vec::new();
        if d.is_edge_hardware() {
            notes.push("XENEON EDGE".to_owned());
        }
        if let Some(o) = d.native_orientation() {
            notes.push(format!("native {o}"));
        } else if d.is_edge_hardware() {
            notes.push(
                "NOT in native 2560x720 mode — see docs/research/edge-hardware.md".to_owned(),
            );
        }
        if matches!(chosen, edge_layout::Selection::Display { index, .. } if index == i) {
            notes.push("← auto".to_owned());
        }
        println!(
            "{i}: {:<20} {}x{} px @{}x  at ({}, {})  {ids}  {}",
            d.name,
            d.pixel_size.0,
            d.pixel_size.1,
            d.scale_factor,
            d.logical_origin.0,
            d.logical_origin.1,
            notes.join(", ")
        );
    }
    if chosen == edge_layout::Selection::Windowed {
        println!("auto: no XENEON EDGE found; EdgeHost would use a stand-in window");
    }
    Ok(())
}

fn layout(store: &Store, command: &LayoutCommand) -> Result<()> {
    let mut layout = store.load_layout()?;
    match command {
        LayoutCommand::Show => {
            show_layout(store, &layout);
            return Ok(());
        }
        LayoutCommand::Add {
            widget,
            slot,
            page,
            col,
            row,
            version,
        } => {
            let installed = match version {
                Some(v) => store
                    .installed()?
                    .into_iter()
                    .find(|w| &w.id == widget && &w.version == v)
                    .with_context(|| format!("{widget} {v} is not installed"))?,
                None => store.latest(widget)?,
            };
            let origin = col.zip(*row).map(|(col, row)| Cell { col, row });
            let id = layout.add(
                *page,
                widget.clone(),
                installed.version.clone(),
                *slot,
                origin,
            )?;
            println!(
                "added {} {} as {} on page {page}",
                widget,
                installed.version,
                id.short()
            );
        }
        LayoutCommand::Remove { instance } => {
            let id = layout.resolve(instance)?;
            let removed = layout.remove(id)?;
            println!("removed {} ({})", id.short(), removed.widget);
        }
        LayoutCommand::Set {
            instance,
            key,
            value,
        } => {
            let id = layout.resolve(instance)?;
            check_property(store, &layout, id, key)?;
            let parsed =
                serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone()));
            println!("{} {key} = {parsed}", id.short());
            layout.set_value(id, key, parsed)?;
        }
        LayoutCommand::Unset { instance, key } => {
            let id = layout.resolve(instance)?;
            layout.unset_value(id, key)?;
            println!("{} {key} reset to its default", id.short());
        }
        LayoutCommand::Display { choice } => {
            layout.display = Some(choice.to_string()).filter(|c| c != "auto");
            println!("display: {choice}");
        }
        LayoutCommand::Orientation { orientation } => {
            if layout.pages.iter().any(|p| !p.instances.is_empty())
                && *orientation != layout.orientation
            {
                bail!(
                    "pages already hold widgets laid out for {}; remove them first",
                    layout.orientation
                );
            }
            layout.orientation = *orientation;
            println!("orientation: {orientation}");
        }
    }
    store.save_layout(&layout)?;
    Ok(())
}

fn show_layout(store: &Store, layout: &edge_store::Layout) {
    println!(
        "display: {}   orientation: {}   file: {}",
        layout.display.as_deref().unwrap_or("auto"),
        layout.orientation,
        store.layout_path().display()
    );
    if layout.pages.is_empty() {
        println!("(empty: add widgets with `edgectl layout add <widget-id>`)");
    }
    for (p, page) in layout.pages.iter().enumerate() {
        println!("page {p}:");
        for i in &page.instances {
            let state = if i.needs_evaluation() {
                "defaults pending"
            } else {
                "ready"
            };
            println!(
                "  {}  {:<36} {:<8} {:<2} col {} row {}  {} value(s), {state}",
                i.id.short(),
                i.widget.as_str(),
                i.version,
                i.slot,
                i.origin.col,
                i.origin.row,
                i.values.len()
            );
            for (k, v) in &i.values {
                let mark = if i.user_set.contains(k) { "*" } else { " " };
                println!("      {mark}{k} = {v}");
            }
        }
    }
}

/// Refuse keys that the widget does not declare, to catch typos.
fn check_property(
    store: &Store,
    layout: &edge_store::Layout,
    id: edge_store::InstanceId,
    key: &str,
) -> Result<()> {
    let Some((_, inst)) = layout.instances().find(|(_, i)| i.id == id) else {
        bail!("instance {} disappeared", id.short());
    };
    let def = WidgetDef::load(store.widget_dir(&inst.widget, &inst.version))
        .with_context(|| format!("loading {} {}", inst.widget, inst.version))?;
    if def.head.properties.iter().any(|p| p.name.as_str() == key) {
        Ok(())
    } else {
        let names: Vec<&str> = def
            .head
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        bail!(
            "{} has no property {key:?}; it has: {}",
            inst.widget,
            names.join(", ")
        )
    }
}
