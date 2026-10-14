//! The document-start injection script.
//!
//! `shim/icue-shim.js` is a function of one argument, the payload; [`render`] serialises the
//! payload for a widget instance and substitutes it for the `__PAYLOAD__` token.
//!
//! Two modes ([`ShimMode`]):
//! - **Runtime**: the widget page gets concrete property values from Rust and never evaluates
//!   attribute expressions.
//! - **Evaluate**: a hidden page with the same globals imports the manifest modules, evaluates every
//!   `data-default` expression and posts the results back (`{k: "evaluated", values}`).

use std::collections::BTreeMap;

use icuew_manifest::{ControlKind, UrlPermission, WidgetDef};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

const SHIM_SOURCE: &str = include_str!("../shim/icue-shim.js");
const PAYLOAD_TOKEN: &str = "__PAYLOAD__";

/// Unit returned by `iCUE.defaultTemperatureUnit()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TemperatureUnit {
    /// `°C`.
    #[default]
    Celsius,
    /// `°F`.
    Fahrenheit,
}

impl TemperatureUnit {
    const fn symbol(self) -> &'static str {
        match self {
            Self::Celsius => "°C",
            Self::Fahrenheit => "°F",
        }
    }
}

/// Per-instance values injected alongside the widget's own definition.
#[derive(Debug, Clone)]
pub struct ShimConfig {
    /// `iCUE.iCUELanguage`, also selects the `tr()` table.
    pub language: String,
    /// `iCUE.fpsLimit`.
    pub fps_limit: u32,
    /// `iCUE.isPreview`.
    pub is_preview: bool,
    /// Widget instance id: `uniqueId` and `iCUE.widgetId`.
    pub instance_id: Uuid,
    /// `device.deviceId`.
    pub device_id: Uuid,
    /// `iCUE.defaultTemperatureUnit()`.
    pub temperature_unit: TemperatureUnit,
    /// Base URL the widget is served from, ending in `/` (e.g. `icuew://<host>/`).
    pub base_url: String,
    /// Developer runs only: pressing Escape in the widget asks the host to close.
    pub escape_closes: bool,
    /// Log pointer and mouse events to the host (`EDGEHOST_DEBUG_INPUT=1`).
    pub debug_input: bool,
}

/// What the injected script does.
#[derive(Debug, Clone, Copy)]
pub enum ShimMode<'a> {
    /// Show the widget with these property values (missing keys become `undefined`).
    Runtime(&'a BTreeMap<String, Value>),
    /// Evaluate the property defaults and post them to the host.
    Evaluate,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Payload<'a> {
    mode: &'static str,
    values: Option<&'a BTreeMap<String, Value>>,
    lang: &'a str,
    fps_limit: u32,
    preview: bool,
    widget_id: String,
    instance_id: String,
    device_id: String,
    temp_unit: &'static str,
    props: Vec<PropPayload<'a>>,
    tr: BTreeMap<String, String>,
    modules: Vec<ModulePayload>,
    plugins: Vec<PluginPayload>,
    escape_closes: bool,
    proxy: Vec<UrlPermission>,
    debug_input: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PropPayload<'a> {
    name: &'a str,
    default_expr: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<ControlKind>,
}

/// A `required_plugins` entry. Unavailable plugins carry no methods or signals.
#[derive(Serialize)]
struct PluginPayload {
    name: String,
    available: bool,
    methods: &'static [&'static str],
    signals: &'static [&'static str],
}

#[derive(Serialize)]
struct ModulePayload {
    url: String,
    global: String,
}

/// Render the injection script for one widget instance.
///
/// # Errors
///
/// Only if payload serialisation fails, which would indicate a bug.
pub fn render(
    def: &WidgetDef,
    cfg: &ShimConfig,
    mode: ShimMode<'_>,
) -> Result<String, serde_json::Error> {
    let (mode_name, values, evaluating) = match mode {
        ShimMode::Runtime(values) => ("runtime", Some(values), false),
        ShimMode::Evaluate => ("evaluate", None, true),
    };
    let modules = def
        .manifest
        .modules
        .iter()
        .map(|path| {
            let rel = path.trim_start_matches("./").trim_start_matches('/');
            let file = rel.rsplit('/').next().unwrap_or(rel);
            let global = file
                .split_once('.')
                .map_or(file, |(stem, _)| stem)
                .to_owned();
            ModulePayload {
                url: format!("{}{rel}", cfg.base_url),
                global,
            }
        })
        .collect();
    let payload = Payload {
        mode: mode_name,
        values,
        lang: &cfg.language,
        fps_limit: cfg.fps_limit,
        escape_closes: cfg.escape_closes,
        preview: cfg.is_preview,
        // Per instance, like uniqueId: Stream Deck sessions are keyed by it, so two copies of a
        // widget must differ.
        widget_id: cfg.instance_id.hyphenated().to_string(),
        instance_id: cfg.instance_id.hyphenated().to_string(),
        device_id: cfg.device_id.hyphenated().to_string(),
        temp_unit: cfg.temperature_unit.symbol(),
        props: def
            .head
            .properties
            .iter()
            .map(|p| PropPayload {
                name: p.name.as_str(),
                default_expr: p.default_expr().filter(|_| evaluating),
                kind: evaluating.then_some(p.kind),
            })
            .collect(),
        tr: def.translations.table(&cfg.language),
        modules,
        // Evaluate pages get no providers (their snapshots are a later milestone).
        plugins: def
            .manifest
            .required_plugins
            .iter()
            .map(
                |wanted| match edge_plugins::facade(wanted).filter(|_| !evaluating) {
                    Some(f) => PluginPayload {
                        name: f.name,
                        available: true,
                        methods: f.methods,
                        signals: f.signals,
                    },
                    None => PluginPayload {
                        name: wanted.facade_name(),
                        available: false,
                        methods: &[],
                        signals: &[],
                    },
                },
            )
            .collect(),
        proxy: def.manifest.url_permissions(),
        debug_input: cfg.debug_input,
    };
    let json = script_safe(&serde_json::to_string(&payload)?);
    Ok(SHIM_SOURCE.replacen(PAYLOAD_TOKEN, &json, 1))
}

/// Escape JSON so it cannot terminate an enclosing `<script>` or break a JS string literal.
fn script_safe(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> WidgetDef {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/widgets/hello-edge"
        );
        WidgetDef::load(dir).unwrap()
    }

    fn cfg() -> ShimConfig {
        ShimConfig {
            language: "en".into(),
            fps_limit: 30,
            is_preview: false,
            instance_id: Uuid::from_u128(1),
            device_id: Uuid::from_u128(2),
            temperature_unit: TemperatureUnit::Celsius,
            base_url: "icuew://abc/".into(),
            escape_closes: false,
            debug_input: false,
        }
    }

    #[test]
    fn shim_source_has_exactly_one_placeholder() {
        assert_eq!(SHIM_SOURCE.matches(PAYLOAD_TOKEN).count(), 1);
    }

    #[test]
    fn renders_payload_once() {
        let values = BTreeMap::from([("textColor".to_owned(), Value::from("#123456"))]);
        let js = render(&def(), &cfg(), ShimMode::Runtime(&values)).unwrap();
        assert!(!js.contains(PAYLOAD_TOKEN));
        assert!(js.contains(r##""mode":"runtime","values":{"textColor":"#123456"}"##));
        assert!(
            !js.contains("Greeting.defaultHeadline()"),
            "runtime pages carry no expressions"
        );
        assert!(js.contains(r#""widgetId":"00000000-0000-0000-0000-000000000001""#));
        assert!(js.contains(r#""instanceId":"00000000-0000-0000-0000-000000000001""#));
        assert!(js.contains(r#""url":"icuew://abc/modules/Greeting.mjs","global":"Greeting""#));
        assert!(js.contains(r#""name":"textColor""#));
    }

    #[test]
    fn evaluate_mode_carries_expressions() {
        let js = render(&def(), &cfg(), ShimMode::Evaluate).unwrap();
        assert!(js.contains(r#""mode":"evaluate","values":null"#));
        assert!(js.contains(r#""defaultExpr":"Greeting.defaultHeadline()""#));
    }

    #[test]
    fn plugins_are_available_only_at_runtime() {
        let mut d = def();
        d.manifest.required_plugins = vec![
            "widgetbuilder.filesystemprovider:FileSystem:1.0"
                .parse()
                .unwrap(),
            "widgetbuilder.sensorsdataprovider:Sensors:1.0"
                .parse()
                .unwrap(),
        ];
        let values = BTreeMap::new();
        let js = render(&d, &cfg(), ShimMode::Runtime(&values)).unwrap();
        assert!(
            js.contains(r#"{"name":"Filesystemprovider","available":true,"methods":["watchPath""#)
        );
        assert!(js.contains(
            r#"{"name":"Sensorsdataprovider","available":false,"methods":[],"signals":[]}"#
        ));
        let js = render(&d, &cfg(), ShimMode::Evaluate).unwrap();
        assert!(js.contains(r#"{"name":"Filesystemprovider","available":false"#));
    }

    #[test]
    fn payload_cannot_close_script_tag() {
        let json = serde_json::to_string(&["</script><script>alert(1)</script>\u{2028}"]).unwrap();
        let safe = script_safe(&json);
        assert!(!safe.contains("</script>"));
        assert!(safe.contains("\\u003c/script>") && safe.contains("\\u2028"));
        let back: Vec<String> = serde_json::from_str(&safe).unwrap();
        assert_eq!(back[0], "</script><script>alert(1)</script>\u{2028}");
    }
}
