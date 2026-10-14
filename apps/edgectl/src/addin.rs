//! `edgectl addin …`: check, install, list, remove and try out Rhai add-ins.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use edge_plugins::addin::{Addin, Registry};
use edge_plugins::rhai_provider::RhaiProvider;
use edge_plugins::{Emitter, Provider};
use edge_store::{InstallError, Store};
use serde_json::Value;

#[derive(Subcommand)]
pub enum AddinCommand {
    /// Check an add-in folder and print what it provides.
    Check {
        /// Folder holding addin.toml and main.rhai.
        dir: PathBuf,
    },
    /// Install an add-in folder, replacing an installed copy with the same id.
    Install {
        /// Folder holding addin.toml and main.rhai.
        dir: PathBuf,
    },
    /// List installed add-ins, and any that cannot be used.
    List,
    /// Remove an installed add-in.
    Remove {
        /// Add-in id, as `addin list` shows it.
        id: String,
    },
    /// Run an add-in outside EdgeHost: optionally call one method, then print its signals.
    Run {
        /// Add-in folder, or the id of an installed add-in.
        addin: String,
        /// Method to call after `init()`.
        #[arg(long)]
        call: Option<String>,
        /// Arguments of --call, as a JSON array.
        #[arg(long, default_value = "[]")]
        args: String,
        /// How long to keep printing signals (default: 0 with --call, else 10).
        #[arg(long)]
        seconds: Option<u64>,
    },
}

pub fn run(store: &dyn Fn() -> Result<Store>, command: &AddinCommand) -> Result<()> {
    match command {
        AddinCommand::Check { dir } => {
            let addin = load(dir)?;
            describe(&addin);
            println!("ok");
            Ok(())
        }
        AddinCommand::Install { dir } => install(&store()?, dir),
        AddinCommand::List => {
            list(&store()?);
            Ok(())
        }
        AddinCommand::Remove { id } => remove(&store()?, id),
        AddinCommand::Run {
            addin,
            call,
            args,
            seconds,
        } => try_out(store, addin, call.as_deref(), args, *seconds),
    }
}

fn load(dir: &Path) -> Result<Addin> {
    Addin::load(dir).with_context(|| format!("{} is not a usable add-in", dir.display()))
}

/// Print what an add-in provides and may do.
pub fn describe(a: &Addin) {
    println!("{} — {}", a.id, a.provides);
    if !a.description.is_empty() {
        println!("  {}", a.description);
    }
    println!("  facade:   window.plugins.{}", a.provides.facade_name());
    println!("  methods:  {}", list_or_none(&a.methods));
    println!("  signals:  {}", list_or_none(&a.signals));
    if let Some(p) = a.poll {
        println!("  poll:     every {} ms", p.as_millis());
    }
    let hosts: Vec<String> = a
        .http
        .iter()
        .map(|g| match g.port {
            Some(port) => format!("{}:{port}", g.host),
            None => g.host.clone(),
        })
        .collect();
    println!("  http:     {}", list_or_none(&hosts));
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_owned()
    } else {
        items.join(", ")
    }
}

fn install(store: &Store, dir: &Path) -> Result<()> {
    match edge_store::install_addin(store, dir) {
        Ok(out) => {
            describe(&out.addin);
            let verb = if out.replaced {
                "reinstalled"
            } else {
                "installed"
            };
            println!("{verb} {} → {}", out.addin.id, out.addin.dir.display());
            println!("widgets pick it up the next time their page is shown");
            Ok(())
        }
        Err(InstallError::InvalidAddin(reason)) => {
            bail!("{} cannot be installed: {reason}", dir.display())
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!("installing {}", dir.display()))),
    }
}

fn list(store: &Store) {
    let (registry, rejected) = Registry::load(&store.addins_dir());
    let mut any = false;
    for a in registry.iter() {
        any = true;
        println!(
            "{:<36} {:<44} {}",
            a.id,
            a.provides.to_string(),
            a.description
        );
    }
    for r in &rejected {
        any = true;
        println!("{:<36} unusable: {}", file_name(&r.dir), r.reason);
    }
    if !any {
        println!("no add-ins installed in {}", store.addins_dir().display());
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn remove(store: &Store, id: &str) -> Result<()> {
    if id.is_empty() || id.starts_with('.') || id.contains(['/', '\\']) {
        bail!("{id:?} is not an add-in id");
    }
    let dir = store.addins_dir().join(id);
    if !dir.is_dir() {
        bail!("no add-in {id} in {}", store.addins_dir().display());
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    println!("removed {id}");
    Ok(())
}

fn try_out(
    store: &dyn Fn() -> Result<Store>,
    which: &str,
    call: Option<&str>,
    args: &str,
    seconds: Option<u64>,
) -> Result<()> {
    let path = Path::new(which);
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        store()?.addins_dir().join(which)
    };
    let addin = Arc::new(load(&dir)?);
    let args: Vec<Value> =
        serde_json::from_str(args).context("--args must be a JSON array, e.g. '[\"BTC-USD\"]'")?;
    let started = Instant::now();
    let emit: Emitter = Arc::new(move |signal: &str, args: Vec<Value>| {
        println!(
            "{:>7.2}s  signal {signal}({})",
            started.elapsed().as_secs_f64(),
            args.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    });
    let provider = RhaiProvider::start(Arc::clone(&addin), emit)?;
    if let Some(method) = call {
        match provider.call(method, &args) {
            Ok(value) => println!("{method} → {value}"),
            Err(e) => bail!("{method} failed: {e}"),
        }
    }
    let wait = seconds.unwrap_or(if call.is_some() { 0 } else { 10 });
    if wait > 0 {
        println!("printing signals for {wait} s…");
        std::thread::sleep(Duration::from_secs(wait));
    }
    Ok(())
}
