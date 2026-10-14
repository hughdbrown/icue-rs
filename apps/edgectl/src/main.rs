//! `edgectl`: EdgeHost developer CLI.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use edge_layout::{Cell, Orientation, Slot};
use icuew_manifest::{Platform, WidgetDef};
use widget_runtime::{RunOptions, run_one};

#[derive(Parser)]
#[command(
    name = "edgectl",
    version,
    about = "EdgeHost developer tools for iCUE HTML widgets"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a widget directory and print what EdgeHost sees.
    Validate {
        /// Unpacked widget directory (contains manifest.json and index.html).
        dir: PathBuf,
    },
    /// Run one widget on the XENEON EDGE, or in a scaled window when no Edge is attached.
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
        /// Orientation of the stand-in window: landscape or portrait.
        #[arg(long, default_value = "landscape")]
        orientation: Orientation,
        /// Stand-in window size relative to the 2560×720 panel.
        #[arg(long, default_value_t = 0.5)]
        scale: f64,
        /// Use the stand-in window even when an Edge is attached.
        #[arg(long)]
        windowed: bool,
        /// Enable the Safari Web Inspector for the widget.
        #[arg(long)]
        devtools: bool,
        /// Shared `common/` folder (iCUE's widgets/common) served at /common/.
        /// Defaults to a `common` folder beside the widget directory.
        #[arg(long)]
        common_dir: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Validate { dir } => validate(&dir),
        Command::RunOne {
            dir,
            slot,
            col,
            row,
            orientation,
            scale,
            windowed,
            devtools,
            common_dir,
        } => run_one(&RunOptions {
            widget_dir: dir,
            slot,
            origin: Cell { col, row },
            orientation,
            windowed_scale: scale,
            force_windowed: windowed,
            devtools,
            common_dir,
        }),
    }
}

fn validate(dir: &Path) -> Result<()> {
    let def = WidgetDef::load(dir).with_context(|| format!("loading widget {}", dir.display()))?;
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
        println!(
            "  plugin:       {p}  → window.plugins.{} (not yet implemented)",
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
    for g in &def.head.groups {
        println!("  group:        {} → {}", g.title, g.properties.join(", "));
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

    let mut warnings = Vec::new();
    if !m.supports_edge() {
        warnings.push("supported_devices does not include dashboard_lcd (XENEON EDGE)".to_owned());
    }
    if def.head.title.is_none() {
        warnings.push("index.html has no <title>".to_owned());
    }
    if def.head.icon.is_none() {
        warnings.push("index.html has no <link rel=\"icon\">".to_owned());
    }
    if !m.os.iter().any(|o| o.platform == Platform::Mac) {
        warnings.push("os lists only windows (advisory; ignored by EdgeHost)".to_owned());
    }
    for w in &warnings {
        println!("  warning:      {w}");
    }
    println!("ok");
    Ok(())
}
