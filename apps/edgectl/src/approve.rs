//! Install-time approval: show what a widget or add-in may access and ask before installing.
//!
//! This is the one-time approval of SPEC §8.1, in its simplest form: installing is still what
//! grants the permissions, but only after the user has seen them and agreed (at the terminal, or
//! with `--yes` in scripts). There is no stored approval state, runtime prompt or revocation yet.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Result, bail};
use edge_plugins::addin::Addin;
use serde_json::Value;

/// Plain-language lines for a widget manifest's `permissions` entries.
pub fn widget_permissions(permissions: &[Value]) -> Vec<String> {
    permissions.iter().map(describe_widget_permission).collect()
}

fn describe_widget_permission(p: &Value) -> String {
    let text = |key: &str| p.get(key).and_then(Value::as_str).unwrap_or("?");
    let port = p
        .get("port")
        .and_then(Value::as_u64)
        .map(|n| format!(", port {n}"));
    match text("type") {
        "url" if text("domain") == "any" => format!(
            "network: any host except this Mac{}",
            port.unwrap_or_default()
        ),
        "url" => format!(
            "network: {}{}",
            host_scope(text("domain")),
            port.unwrap_or_default()
        ),
        "file" => format!("read the file {}", text("path")),
        "folder" => format!("read the folder {} and everything in it", text("path")),
        other => format!("{other}: {p} (recorded; EdgeHost does not provide it yet)"),
    }
}

/// `host`, plus "and its subdomains" where that applies (names, not addresses or `localhost`).
fn host_scope(host: &str) -> String {
    let address = host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok();
    if address || host.eq_ignore_ascii_case("localhost") {
        host.to_owned()
    } else {
        format!("{host} and its subdomains")
    }
}

/// Plain-language lines for an add-in's `permissions.http`.
pub fn addin_permissions(addin: &Addin) -> Vec<String> {
    addin
        .http
        .iter()
        .map(|g| match g.port {
            Some(port) => format!("network: {}, port {port}", host_scope(&g.host)),
            None => format!("network: {}", host_scope(&g.host)),
        })
        .collect()
}

/// Show `lines` (what `name` may access) and get the user's agreement. Nothing to approve passes
/// at once; `yes` approves without asking; otherwise the user is asked at the terminal, and a run
/// without a terminal fails, asking for `--yes`.
///
/// # Errors
///
/// The user declined, or there is no terminal and `yes` is false.
pub fn confirm(name: &str, lines: &[String], yes: bool) -> Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    println!("{name} asks to:");
    for l in lines {
        println!("  - {l}");
    }
    if yes {
        println!("approved with --yes");
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        bail!("{name} declares permissions; review them above and pass --yes to install");
    }
    print!("Grant these and install? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        bail!("not installed")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn describes_each_permission_kind() {
        let lines = widget_permissions(&[
            json!({"type": "url", "domain": "query1.finance.yahoo.com", "port": 443}),
            json!({"type": "url", "domain": "any", "port": 8111}),
            json!({"type": "url", "domain": "localhost", "port": 8111}),
            json!({"type": "url", "domain": "192.168.1.20"}),
            json!({"type": "folder", "path": "<Home>"}),
            json!({"type": "file", "path": "<Documents>/a.txt"}),
            json!({"type": "camera"}),
        ]);
        assert_eq!(
            lines,
            [
                "network: query1.finance.yahoo.com and its subdomains, port 443",
                "network: any host except this Mac, port 8111",
                "network: localhost, port 8111",
                "network: 192.168.1.20",
                "read the folder <Home> and everything in it",
                "read the file <Documents>/a.txt",
                r#"camera: {"type":"camera"} (recorded; EdgeHost does not provide it yet)"#,
            ]
        );
    }

    #[test]
    fn nothing_declared_needs_no_approval_and_yes_approves() {
        assert!(confirm("w", &[], false).is_ok());
        assert!(confirm("w", &["network: x".into()], true).is_ok());
    }
}
