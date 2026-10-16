//! `edgectl llm-usage …`: create the subscriptions file and check what the LLM Usage widget will
//! show.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use jiff::{SignedDuration, Timestamp, tz::TimeZone};
use llm_usage::config::{self, EXAMPLE, FILE_NAME};
use llm_usage::http::UreqHttp;
use llm_usage::{Service, Snapshot, Status, Subscription, Window};

#[derive(Args)]
pub struct LlmUsageArgs {
    #[command(subcommand)]
    command: Option<LlmUsageCommand>,
    /// Subscriptions file (default: `$EDGEHOST_LLM_USAGE_CONFIG`, else llm-usage.toml in the data
    /// folder).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Print the snapshot the widget receives, as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum LlmUsageCommand {
    /// Write a commented example subscriptions file (mode 0600) to edit.
    Init {
        /// Replace an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Print where the subscriptions file is.
    Path,
}

/// The file the widget reads: `--config`, else `$EDGEHOST_LLM_USAGE_CONFIG`, else `--home`'s
/// folder, else the default data folder.
fn config_path(args: &LlmUsageArgs, home: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = &args.config {
        return Ok(p.clone());
    }
    if std::env::var_os("EDGEHOST_LLM_USAGE_CONFIG").is_none_or(|v| v.is_empty())
        && let Some(home) = home
    {
        return Ok(home.join(FILE_NAME));
    }
    config::default_path().context("cannot locate the data folder: $HOME is not set")
}

pub fn run(args: &LlmUsageArgs, home: Option<&Path>) -> Result<()> {
    let path = config_path(args, home)?;
    match args.command {
        Some(LlmUsageCommand::Path) => {
            println!("{}", path.display());
            Ok(())
        }
        Some(LlmUsageCommand::Init { force }) => init(&path, force),
        None => {
            let snap = Service::new(Some(path), Box::new(UreqHttp::default())).refresh();
            if args.json {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            } else {
                print!("{}", render(&snap, Timestamp::now()));
            }
            if let Some(e) = &snap.config_error {
                bail!("{e}");
            }
            Ok(())
        }
    }
}

fn init(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists; edit it, or pass --force to replace it",
            path.display()
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode(0o600);
        // `mode` only applies when the file is created; an existing one is tightened too.
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("setting the mode of {}", path.display()))?;
        }
    }
    opts.open(path)
        .and_then(|mut f| f.write_all(EXAMPLE.as_bytes()))
        .with_context(|| format!("writing {}", path.display()))?;
    println!("Wrote {} (mode 600).", path.display());
    println!(
        "Enable the subscriptions you have, paste their keys, then check it with `edgectl llm-usage`."
    );
    Ok(())
}

/// `2 h 13 m`, `4 d 6 h`, `45 s`.
fn span(d: SignedDuration) -> String {
    let s = d.as_secs().max(0);
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} m", s / 60),
        3600..86400 => format!("{} h {} m", s / 3600, s % 3600 / 60),
        _ => format!("{} d {} h", s / 86400, s % 86400 / 3600),
    }
}

/// Local wall-clock time, `Fri 14:30`.
fn clock(t: Timestamp) -> String {
    t.to_zoned(TimeZone::system())
        .strftime("%a %H:%M")
        .to_string()
}

fn bar(used: f64) -> String {
    const WIDTH: usize = 20;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "used is clamped to 0–1 and WIDTH is 20"
    )]
    let full = (used.clamp(0.0, 1.0) * WIDTH as f64).round() as usize;
    format!("{}{}", "█".repeat(full), "░".repeat(WIDTH - full))
}

fn window_line(w: &Window, now: Timestamp) -> String {
    let mut line = format!(
        "    {:<6} {} {:>3.0}% left",
        w.label,
        bar(w.used),
        w.remaining() * 100.0
    );
    if let Some(a) = &w.amount {
        let _ = write!(
            line,
            " (${:.2} of ${:.2} {})",
            a.limit - a.used,
            a.limit,
            a.unit
        );
    }
    match w.resets_at {
        Some(r) => {
            let _ = write!(
                line,
                "  resets in {} ({})",
                span(r.duration_since(now)),
                clock(r)
            );
        }
        None => line.push_str("  not started"),
    }
    if let Some(p) = &w.pace {
        let _ = write!(line, "  pace {:+.0}%", p.ahead * 100.0);
        if let Some(out) = p.runs_out_at {
            let _ = write!(line, ", runs out {}", clock(out));
        }
    }
    line
}

fn sub_block(s: &Subscription, now: Timestamp, out: &mut String) {
    let plan = s
        .plan
        .as_deref()
        .map(|p| format!(" ({p})"))
        .unwrap_or_default();
    let status = match s.status {
        Status::Ok => "ok",
        Status::Stale => "stale",
        Status::Error => "error",
    };
    let _ = writeln!(out, "{}{plan} [{}] {status}", s.label, s.id);
    if let Some(m) = &s.message {
        let _ = writeln!(out, "    ! {m}");
    }
    if let Some(d) = &s.detail {
        let _ = writeln!(out, "    {d}");
    }
    for w in &s.windows {
        let _ = writeln!(out, "{}", window_line(w, now));
    }
    if let Some(b) = s.blocked_until {
        let _ = writeln!(
            out,
            "    used up until {} ({})",
            clock(b),
            span(b.duration_since(now))
        );
    }
}

fn render(snap: &Snapshot, now: Timestamp) -> String {
    let mut out = format!("Subscriptions file: {}\n", snap.config_path);
    if snap.config_error.is_some() {
        return out;
    }
    if snap.subscriptions.is_empty() {
        out.push_str("No subscriptions are enabled.\n");
    }
    for s in &snap.subscriptions {
        out.push('\n');
        sub_block(s, now, &mut out);
    }
    if let Some(sug) = &snap.suggestion
        && let Some(s) = snap.subscriptions.iter().find(|s| s.id == sug.id)
    {
        let _ = writeln!(out, "\nUse next: {} (urgency {:.2})", s.label, sug.urgency);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_and_bars() {
        assert_eq!(span(SignedDuration::from_secs(45)), "45 s");
        assert_eq!(
            span(SignedDuration::from_secs(2 * 3600 + 13 * 60)),
            "2 h 13 m"
        );
        assert_eq!(
            span(SignedDuration::from_secs(4 * 86400 + 6 * 3600)),
            "4 d 6 h"
        );
        assert_eq!(span(SignedDuration::from_secs(-5)), "0 s");
        assert_eq!(bar(0.5).chars().filter(|&c| c == '█').count(), 10);
        assert_eq!(bar(2.0).chars().filter(|&c| c == '░').count(), 0);
    }

    #[test]
    fn init_writes_a_private_example_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/llm-usage.toml");
        init(&path, false).unwrap();
        assert!(init(&path, false).is_err(), "never overwrites by default");
        init(&path, true).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(llm_usage::Config::load(&path).is_ok());
    }
}
