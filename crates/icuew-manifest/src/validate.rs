//! Package validation: the rules of `icuewidget-cli` 0.4.48 (`validate`), plus EdgeHost checks.
//!
//! Errors block installation; warnings are shown and the widget is installed anyway. Where
//! EdgeHost is more lenient than iCUE (for example a non-self-closing `<meta>` in `<head>`, which
//! iCUE's XML import rejects), the finding is a warning that says so.

use std::fmt;
use std::fs;
use std::path::{Component, Path};

use scraper::{Html, Selector};
use serde_json::Value;

use crate::{PluginRef, WidgetDef, WidgetError};

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Shown, does not block installation.
    Warning,
    /// Blocks installation.
    Error,
}

/// One validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(f, "{label}: {}", self.message)
    }
}

/// Result of [`validate_dir`].
#[derive(Debug, Default)]
pub struct Report {
    /// All findings, in discovery order.
    pub diagnostics: Vec<Diagnostic>,
    /// The parsed widget, if parsing succeeded.
    pub widget: Option<WidgetDef>,
}

impl Report {
    /// Whether any finding blocks installation.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    fn push(&mut self, severity: Severity, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            severity,
            message: message.into(),
        });
    }

    fn error(&mut self, message: impl Into<String>) {
        self.push(Severity::Error, message);
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.push(Severity::Warning, message);
    }
}

/// Validate an unpacked widget directory.
///
/// Never fails: problems, including unreadable files, are reported as diagnostics.
#[must_use]
pub fn validate_dir(dir: &Path) -> Report {
    let mut report = Report::default();
    let manifest_text = fs::read_to_string(dir.join("manifest.json"));
    let index_text = fs::read_to_string(dir.join("index.html"));
    if manifest_text.is_err() {
        report.error("missing required file manifest.json");
    }
    if index_text.is_err() {
        report.error("missing required file index.html");
    }
    if !dir.join("translation.json").is_file() {
        report.warn("translation.json is missing; tr() will return its keys");
    }
    if let Ok(text) = &manifest_text {
        check_manifest(dir, text, &mut report);
    }
    if let Ok(html) = &index_text {
        check_index(dir, html, &mut report);
    }
    if !report.has_errors() {
        match WidgetDef::load(dir) {
            Ok(def) => {
                if !def.manifest.supports_edge() {
                    report.warn("supported_devices does not include dashboard_lcd (XENEON EDGE)");
                }
                report.widget = Some(def);
            }
            Err(e) => report.error(error_chain(&e)),
        }
    }
    report
}

fn check_manifest(dir: &Path, text: &str, report: &mut Report) {
    let manifest: Value = match serde_json::from_str(text) {
        Ok(v @ Value::Object(_)) => v,
        Ok(_) => return report.error("manifest.json must be a JSON object"),
        Err(e) => return report.error(format!("manifest.json is not valid JSON: {e}")),
    };
    let string = |key: &str| manifest.get(key).and_then(Value::as_str).map(str::trim);
    for key in [
        "author",
        "id",
        "name",
        "description",
        "version",
        "preview_icon",
        "min_framework_version",
    ] {
        if string(key).is_none_or(str::is_empty) {
            report.error(format!(
                "manifest.json: \"{key}\" must be a non-empty string"
            ));
        }
    }
    match string("min_app_version") {
        None | Some("") => {
            report.warn("manifest.json: \"min_app_version\" is missing (required by iCUE)");
        }
        Some(v) if !is_version(v, 2..=3) => {
            report.error(format!(
                "manifest.json: \"min_app_version\" {v:?} must look like 5.47 or 5.47.1"
            ));
        }
        Some(_) => {}
    }
    if let Some(id) = string("id").filter(|id| !id.is_empty()) {
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            report.error(format!(
                "manifest.json: \"id\" {id:?} may only contain letters, digits, '.' and '-'"
            ));
        }
    }
    for key in ["version", "min_framework_version"] {
        if let Some(v) = string(key).filter(|v| !v.is_empty() && !is_version(v, 3..=3)) {
            report.error(format!(
                "manifest.json: \"{key}\" {v:?} must be MAJOR.MINOR.PATCH"
            ));
        }
    }
    if let Some(icon) = string("preview_icon").filter(|p| !p.is_empty()) {
        check_local_file(dir, icon, "preview_icon", report);
    }
    check_array(&manifest, "os", report);
    check_array(&manifest, "supported_devices", report);
    if let Some(devices) = manifest.get("supported_devices").and_then(Value::as_array) {
        for d in devices {
            let kind = d.get("type").and_then(Value::as_str).unwrap_or_default();
            if !matches!(kind, "dashboard_lcd" | "pump_lcd" | "keyboard_lcd") {
                report.error(format!(
                    "manifest.json: unknown supported_devices type {kind:?}"
                ));
            }
        }
    }
    match manifest.get("interactive") {
        None | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            let others = manifest
                .get("supported_devices")
                .and_then(Value::as_array)
                .is_some_and(|ds| {
                    ds.iter()
                        .any(|d| d.get("type").and_then(Value::as_str) != Some("dashboard_lcd"))
                });
            if others {
                report.warn("\"interactive\": true only applies to dashboard_lcd; iCUE ignores it for pump and keyboard LCDs");
            }
        }
        Some(_) => report.error("manifest.json: \"interactive\" must be true or false"),
    }
    for plugin in strings(&manifest, "required_plugins", report) {
        if let Err(e) = plugin.parse::<PluginRef>() {
            report.error(format!("manifest.json: {e}"));
        }
    }
    for module in strings(&manifest, "modules", report) {
        check_local_file(dir, &module, "module", report);
    }
}

fn check_index(dir: &Path, html: &str, report: &mut Report) {
    let doc = Html::parse_document(html);
    if doc.select(&selector("head > title")).next().is_none() {
        report.warn("index.html has no <title>");
    }
    if doc
        .select(&selector(r#"link[rel~="icon"]"#))
        .next()
        .is_none()
    {
        report.warn("index.html has no <link rel=\"icon\">");
    }
    for tag in unclosed_void_elements_in_head(html) {
        report.warn(format!(
            "<{tag}> in <head> is not self-closed (\"/>\"); iCUE's importer rejects this, EdgeHost accepts it"
        ));
    }

    let mut mentions_icue_events = html.contains("icueEvents");
    for el in doc.select(&selector("[src], link[href]")) {
        let attr = if el.value().name() == "link" {
            "href"
        } else {
            "src"
        };
        let Some(reference) = el.value().attr(attr).and_then(local_reference) else {
            continue;
        };
        if let Some(common) = reference.strip_prefix("../common/") {
            report.warn(format!("{reference} needs iCUE's shared common/ folder (common/{common}); install it in EdgeHost's common folder"));
            continue;
        }
        if check_local_file(dir, reference, attr, report) && el.value().name() == "script" {
            mentions_icue_events |=
                fs::read_to_string(dir.join(reference)).is_ok_and(|js| js.contains("icueEvents"));
        }
    }
    if !mentions_icue_events {
        report.warn(
            "index.html and its scripts never reference icueEvents; iCUE rejects such widgets",
        );
    }
}

/// Warn about `../` and absolute references and report missing files. Returns whether the file exists.
fn check_local_file(dir: &Path, reference: &str, what: &str, report: &mut Report) -> bool {
    let rel = Path::new(reference.trim_start_matches("./"));
    if rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        report.error(format!(
            "{what} {reference:?} points outside the widget directory"
        ));
        return false;
    }
    if dir.join(rel).is_file() {
        true
    } else {
        report.error(format!("{what} {reference:?} does not exist"));
        false
    }
}

fn check_array(manifest: &Value, key: &str, report: &mut Report) {
    if manifest
        .get(key)
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        report.error(format!(
            "manifest.json: \"{key}\" must be a non-empty array"
        ));
    }
}

/// String items of an optional array key; reports non-string items.
fn strings(manifest: &Value, key: &str, report: &mut Report) -> Vec<String> {
    let Some(value) = manifest.get(key) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        report.error(format!(
            "manifest.json: \"{key}\" must be an array of strings"
        ));
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| match v.as_str() {
            Some(s) if !s.trim().is_empty() => Some(s.to_owned()),
            _ => {
                report.error(format!(
                    "manifest.json: \"{key}\" entries must be non-empty strings"
                ));
                None
            }
        })
        .collect()
}

/// A reference into the widget's own files, without query or fragment; `None` for remote URLs.
fn local_reference(raw: &str) -> Option<&str> {
    let r = raw.trim();
    let is_remote = r.is_empty()
        || r.starts_with('#')
        || r.starts_with("//")
        || r.contains("://")
        || ["data:", "blob:", "mailto:", "javascript:"]
            .iter()
            .any(|p| r.starts_with(p));
    if is_remote {
        return None;
    }
    let end = r.find(['?', '#']).unwrap_or(r.len());
    r.get(..end).filter(|s| !s.is_empty())
}

fn is_version(v: &str, parts: std::ops::RangeInclusive<usize>) -> bool {
    let pieces: Vec<&str> = v.split('.').collect();
    parts.contains(&pieces.len())
        && pieces
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Void elements inside `<head>` written without a closing `/>`, by tag name.
fn unclosed_void_elements_in_head(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let (Some(start), Some(end)) = (lower.find("<head"), lower.find("</head>")) else {
        return Vec::new();
    };
    let Some(head) = html.get(start..end) else {
        return Vec::new();
    };
    head.split('<')
        .skip(1)
        .filter_map(|chunk| {
            let tag_end = chunk.find('>')?;
            let tag = chunk.get(..tag_end)?;
            let name: String = tag
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_ascii_lowercase();
            let void = matches!(
                name.as_str(),
                "meta" | "link" | "base" | "br" | "hr" | "img" | "input"
            );
            (void && !tag.trim_end().ends_with('/')).then_some(name)
        })
        .collect()
}

fn selector(css: &str) -> Selector {
    // Only called with the literal selectors in this module, which are valid CSS.
    Selector::parse(css).unwrap_or_else(|e| unreachable!("invalid built-in selector {css:?}: {e}"))
}

fn error_chain(e: &WidgetError) -> String {
    let mut out = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn hello() -> &'static Path {
        Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/widgets/hello-edge"
        ))
    }

    fn copy_hello() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(hello(), dir.path());
        dir
    }

    fn copy_dir(from: &Path, to: &Path) {
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                fs::create_dir_all(&target).unwrap();
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn sample_widget_is_clean() {
        let r = validate_dir(hello());
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert!(r.widget.is_some());
    }

    #[test]
    fn reports_cli_rule_violations() {
        let dir = copy_hello();
        let m = dir.path().join("manifest.json");
        let text = fs::read_to_string(&m)
            .unwrap()
            .replace("\"0.1.0\"", "\"0.1\"")
            .replace("\"modules/Greeting.mjs\"", "\"modules/Missing.mjs\"")
            .replace("\"dashboard_lcd\"", "\"mouse_lcd\"");
        fs::write(&m, text).unwrap();
        fs::remove_file(dir.path().join("styles/main.css")).unwrap();
        let r = validate_dir(dir.path());
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message.as_str())
            .collect();
        assert!(
            errors.iter().any(|e| e.contains("\"version\"")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("Missing.mjs")),
            "{errors:?}"
        );
        assert!(errors.iter().any(|e| e.contains("mouse_lcd")), "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("main.css")), "{errors:?}");
        assert!(r.widget.is_none());
    }

    #[test]
    fn warns_about_unclosed_head_elements_and_missing_hooks() {
        let dir = copy_hello();
        let index = dir.path().join("index.html");
        let html = fs::read_to_string(&index)
            .unwrap()
            .replace("<meta charset=\"utf-8\" />", "<meta charset=\"utf-8\">");
        fs::write(&index, html).unwrap();
        let js = dir.path().join("scripts/main.js");
        fs::write(
            &js,
            fs::read_to_string(&js)
                .unwrap()
                .replace("icueEvents", "otherEvents"),
        )
        .unwrap();
        let r = validate_dir(dir.path());
        assert!(!r.has_errors(), "{:?}", r.diagnostics);
        let warnings: Vec<_> = r.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(
            warnings.iter().any(|w| w.starts_with("<meta>")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("icueEvents")),
            "{warnings:?}"
        );
    }

    #[test]
    fn missing_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let r = validate_dir(dir.path());
        assert!(r.has_errors());
        assert_eq!(
            r.diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count(),
            2
        );
    }

    #[test]
    fn local_reference_filtering() {
        assert_eq!(local_reference("scripts/a.js?v=2"), Some("scripts/a.js"));
        assert_eq!(local_reference("https://x/y.js"), None);
        assert_eq!(local_reference("data:image/png;base64,AA"), None);
        assert_eq!(local_reference("#top"), None);
    }
}
