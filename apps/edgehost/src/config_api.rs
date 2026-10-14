//! Request handlers for the configurator UI (`docs/configurator-api.md`).
//!
//! Window-free: the caller parses the request, passes the store, the layout and the connected
//! displays, and persists/applies the layout when [`Outcome::Changed`] comes back.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};
use edge_layout::{Cell, DisplayInfo, Orientation, Slot};
use edge_store::{Layout, Store, compare_versions};
use icuew_manifest::{PropertyDef, WidgetDef, WidgetId};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// One request from the UI.
#[derive(Debug, Deserialize)]
pub struct Request {
    /// Request id, echoed in the reply.
    pub id: u64,
    /// Method name, e.g. `layout.add`.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Value,
}

/// What a handled request did.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Nothing changed; reply with this result.
    Read(Value),
    /// The layout changed and must be saved and shown; reply with this result.
    Changed(Value),
    /// The display or orientation changed: save and reopen the surface.
    Reopen(Value),
}

/// Language used to resolve `tr()` labels.
const LANG: &str = "en";

/// Handle `method` with `params`.
///
/// # Errors
///
/// A human-readable message for the UI: bad parameters, unknown instances, overlaps, etc.
pub fn handle(
    store: &Store,
    layout: &mut Layout,
    displays: &[DisplayInfo],
    method: &str,
    params: &Value,
) -> Result<Outcome> {
    let p = Params(params);
    Ok(match method {
        "state.get" => Outcome::Read(state(store, layout, displays)?),
        "layout.add" => {
            let widget: WidgetId = p.str("widget")?.parse().map_err(|e| anyhow!("{e}"))?;
            let version = match p.opt_str("version") {
                Some(v) => v.to_owned(),
                None => store.latest(&widget)?.version,
            };
            if !store.widget_dir(&widget, &version).is_dir() {
                bail!("{widget} {version} is not installed");
            }
            let id = layout.add(
                p.usize("page")?,
                widget,
                version,
                p.slot()?,
                p.opt_cell("origin")?,
            )?;
            Outcome::Changed(json!({ "instance": id.to_string() }))
        }
        "layout.move" => {
            let id = layout.resolve(p.str("instance")?)?;
            let origin = p.opt_cell("origin")?.context("origin is required")?;
            layout.move_to(id, p.usize("page")?, p.slot()?, origin)?;
            Outcome::Changed(json!({}))
        }
        "layout.remove" => {
            let id = layout.resolve(p.str("instance")?)?;
            layout.remove(id)?;
            Outcome::Changed(json!({}))
        }
        "layout.set" => {
            let id = layout.resolve(p.str("instance")?)?;
            let key = p.str("key")?;
            check_property(store, layout, id, key)?;
            let value = params.get("value").cloned().context("value is required")?;
            layout.set_value(id, key, value)?;
            Outcome::Changed(json!({}))
        }
        "layout.unset" => {
            let id = layout.resolve(p.str("instance")?)?;
            layout.unset_value(id, p.str("key")?)?;
            Outcome::Changed(json!({}))
        }
        "layout.display" => {
            let choice = p.str("choice")?.trim();
            layout.display = Some(choice.to_owned()).filter(|c| !c.is_empty() && c != "auto");
            Outcome::Reopen(json!({}))
        }
        "layout.orientation" => {
            let orientation: Orientation = p.str("orientation")?.parse()?;
            if orientation != layout.orientation
                && layout.pages.iter().any(|pg| !pg.instances.is_empty())
            {
                bail!("Remove all widgets before changing the orientation.");
            }
            layout.orientation = orientation;
            Outcome::Reopen(json!({}))
        }
        "widgets.uninstall" => {
            let widget: WidgetId = p.str("widget")?.parse().map_err(|e| anyhow!("{e}"))?;
            let version = p.opt_str("version");
            let used = layout
                .instances()
                .filter(|(_, i)| i.widget == widget && version.is_none_or(|v| v == i.version))
                .count();
            if used > 0 {
                bail!("{widget} is used by {used} widget(s) on the Edge; remove them first.");
            }
            store.uninstall(&widget, version)?;
            Outcome::Changed(json!({}))
        }
        other => bail!("unknown method {other:?}"),
    })
}

/// The full `State` object.
fn state(store: &Store, layout: &Layout, displays: &[DisplayInfo]) -> Result<Value> {
    let orientation = layout.orientation;
    let (cols, rows) = orientation.grid();
    let slots: Map<String, Value> = Slot::ALL
        .iter()
        .map(|s| {
            let (c, r) = s.span(orientation);
            (s.to_string(), json!({ "cols": c, "rows": r }))
        })
        .collect();
    let pages: Vec<Value> = layout
        .pages
        .iter()
        .map(|page| {
            let instances: Vec<Value> = page
                .instances
                .iter()
                .map(|i| {
                    json!({
                        "id": i.id.to_string(),
                        "widget": i.widget.as_str(),
                        "version": i.version,
                        "slot": i.slot.to_string(),
                        "origin": { "col": i.origin.col, "row": i.origin.row },
                        "values": i.values,
                        "user_set": i.user_set,
                        "defaults_pending": i.needs_evaluation(),
                    })
                })
                .collect();
            json!({ "instances": instances })
        })
        .collect();
    let displays: Vec<Value> = displays
        .iter()
        .enumerate()
        .map(|(index, d)| {
            json!({
                "index": index,
                "name": d.name,
                "pixel_size": [d.pixel_size.0, d.pixel_size.1],
                "edge": d.is_edge_hardware(),
                "native": d.native_orientation().is_some(),
            })
        })
        .collect();
    Ok(json!({
        "orientation": orientation.to_string(),
        "grid": { "cols": cols, "rows": rows },
        "slots": slots,
        "pages": pages,
        "display": layout.display.as_deref().unwrap_or("auto"),
        "displays": displays,
        "widgets": widgets(store)?,
        "data_dir": store.root().display().to_string(),
    }))
}

/// Newest version of each installed widget, described for the UI.
fn widgets(store: &Store) -> Result<Vec<Value>> {
    let mut newest: BTreeMap<String, edge_store::Installed> = BTreeMap::new();
    for w in store.installed()? {
        let replace = newest
            .get(w.id.as_str())
            .is_none_or(|cur| compare_versions(&w.version, &cur.version).is_gt());
        if replace {
            newest.insert(w.id.as_str().to_owned(), w);
        }
    }
    let mut out = Vec::new();
    for w in newest.into_values() {
        let Ok(def) = WidgetDef::load(&w.path) else {
            continue;
        };
        out.push(describe(&def, &w.version));
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(out)
}

fn describe(def: &WidgetDef, version: &str) -> Value {
    let m = &def.manifest;
    let tr = def.translations.table(LANG);
    let icon = Some(m.preview_icon.trim_start_matches("./"))
        .filter(|p| !p.is_empty())
        .map(|p| format!("edgecfg://widget/{}/{version}/{p}", m.id));
    let properties: Vec<Value> = def
        .head
        .properties
        .iter()
        .map(|p| property(p, &tr))
        .collect();
    let groups: Vec<Value> = def
        .head
        .groups
        .iter()
        .map(|g| {
            json!({
                "title": display_text(&g.title, &tr),
                "properties": g.properties,
                "info": g.info.as_deref().map(|i| display_text(i, &tr)),
            })
        })
        .collect();
    json!({
        "id": m.id.as_str(),
        "version": version,
        "name": m.name,
        "description": m.description,
        "author": m.author,
        "icon": icon,
        "supports_edge": m.supports_edge(),
        "interactive": m.interactive,
        "required_plugins": m.required_plugins.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "properties": properties,
        "groups": groups,
    })
}

fn property(p: &PropertyDef, tr: &BTreeMap<String, String>) -> Value {
    let mut v = Map::new();
    v.insert("name".into(), json!(p.name.as_str()));
    v.insert("kind".into(), json!(p.kind));
    v.insert(
        "label".into(),
        json!(
            p.label
                .as_deref()
                .map_or_else(|| p.name.to_string(), |l| display_text(l, tr))
        ),
    );
    v.insert("default_expr".into(), json!(p.default_expr()));
    let number = |key: &str| p.attrs.get(key).and_then(|s| s.trim().parse::<f64>().ok());
    for key in ["min", "max", "step"] {
        if let Some(n) = number(key) {
            v.insert(key.into(), json!(n));
        }
    }
    if let Some(unit) = p.attrs.get("unit-label") {
        v.insert("unit".into(), json!(display_text(unit, tr)));
    }
    if let Some(options) = p.attrs.get("values").and_then(|e| literal_options(e, tr)) {
        v.insert(
            "options".into(),
            Value::Array(
                options
                    .into_iter()
                    .map(|(key, label)| json!({ "key": key, "label": label }))
                    .collect(),
            ),
        );
    }
    Value::Object(v)
}

/// Text for a label-like expression: `tr('Key')` → translation, `'text'` → text, else the raw text.
#[must_use]
pub fn display_text(expr: &str, tr: &BTreeMap<String, String>) -> String {
    let e = expr.trim();
    if let Some(key) = e
        .strip_prefix("tr(")
        .and_then(|r| r.strip_suffix(')'))
        .and_then(unquote)
    {
        return tr.get(key).cloned().unwrap_or_else(|| key.to_owned());
    }
    unquote(e).map_or_else(|| e.to_owned(), str::to_owned)
}

fn unquote(s: &str) -> Option<&str> {
    let s = s.trim();
    ['\'', '"']
        .into_iter()
        .find_map(|q| s.strip_prefix(q).and_then(|r| r.strip_suffix(q)))
        .filter(|inner| !inner.contains(['\'', '"']))
}

/// Options of a literal `data-values` expression: `['a','b']`, `[{'key':'k','value':tr('V')}]` or
/// `{'k': 'V'}`. Dynamic expressions (function calls other than `tr`) give `None`.
#[must_use]
pub fn literal_options(expr: &str, tr: &BTreeMap<String, String>) -> Option<Vec<(String, String)>> {
    let resolved = replace_tr_calls(expr, tr)?;
    let json_text = if resolved.contains('"') {
        resolved
    } else {
        resolved.replace('\'', "\"")
    };
    let scalar = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    match serde_json::from_str::<Value>(&json_text).ok()? {
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Object(o) => {
                    let key = scalar(o.get("key")?)?;
                    let label = o
                        .get("value")
                        .and_then(scalar)
                        .unwrap_or_else(|| key.clone());
                    Some((key, label))
                }
                other => scalar(other).map(|s| (s.clone(), s)),
            })
            .collect(),
        Value::Object(o) => o
            .iter()
            .map(|(k, v)| scalar(v).map(|label| (k.clone(), label)))
            .collect(),
        _ => None,
    }
}

/// Replace every `tr('Key')` with a quoted translation.
fn replace_tr_calls(expr: &str, tr: &BTreeMap<String, String>) -> Option<String> {
    let mut out = String::with_capacity(expr.len());
    let mut rest = expr;
    while let Some(start) = rest.find("tr(") {
        let (before, after) = rest.split_at(start);
        out.push_str(before);
        let after = after.get(3..)?;
        let end = after.find(')')?;
        let key = unquote(after.get(..end)?)?;
        let text = tr.get(key).map_or(key, String::as_str);
        out.push('\'');
        out.push_str(&text.replace(['\'', '"'], "’"));
        out.push('\'');
        rest = after.get(end + 1..)?;
    }
    out.push_str(rest);
    Some(out)
}

fn check_property(
    store: &Store,
    layout: &Layout,
    id: edge_store::InstanceId,
    key: &str,
) -> Result<()> {
    let (_, inst) = layout
        .instances()
        .find(|(_, i)| i.id == id)
        .context("that widget is no longer on the layout")?;
    let def = WidgetDef::load(store.widget_dir(&inst.widget, &inst.version))
        .with_context(|| format!("loading {} {}", inst.widget, inst.version))?;
    if def.head.properties.iter().any(|p| p.name.as_str() == key) {
        Ok(())
    } else {
        bail!("{} has no setting named {key:?}", def.manifest.name)
    }
}

/// Typed access to request parameters with UI-friendly errors.
struct Params<'a>(&'a Value);

impl Params<'_> {
    fn opt_str(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Value::as_str)
    }

    fn str(&self, key: &str) -> Result<&str> {
        self.opt_str(key)
            .with_context(|| format!("{key} is required"))
    }

    fn usize(&self, key: &str) -> Result<usize> {
        let n = self
            .0
            .get(key)
            .and_then(Value::as_u64)
            .with_context(|| format!("{key} must be a number"))?;
        usize::try_from(n).with_context(|| format!("{key} is too large"))
    }

    fn slot(&self) -> Result<Slot> {
        Ok(self.str("slot")?.parse()?)
    }

    fn opt_cell(&self, key: &str) -> Result<Option<Cell>> {
        let Some(v) = self.0.get(key).filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let coord = |name: &str| -> Result<u32> {
            let n = v
                .get(name)
                .and_then(Value::as_u64)
                .with_context(|| format!("{key}.{name} must be a number"))?;
            u32::try_from(n).with_context(|| format!("{key}.{name} is too large"))
        };
        Ok(Some(Cell {
            col: coord("col")?,
            row: coord("row")?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store_with_hello() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let hello = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/widgets/hello-edge"
        ));
        edge_store::install(&store, hello).unwrap();
        (dir, store)
    }

    fn call(store: &Store, layout: &mut Layout, method: &str, params: &Value) -> Result<Outcome> {
        handle(store, layout, &[], method, params)
    }

    #[test]
    fn state_describes_widgets_and_layout() {
        let (_d, store) = store_with_hello();
        let mut layout = Layout::default();
        let Outcome::Read(s) = call(&store, &mut layout, "state.get", &json!({})).unwrap() else {
            panic!()
        };
        assert_eq!(s["grid"], json!({ "cols": 3, "rows": 2 }));
        assert_eq!(s["slots"]["L"], json!({ "cols": 2, "rows": 2 }));
        let w = &s["widgets"][0];
        assert_eq!(w["id"], "dev.edgehost.hello-edge");
        assert_eq!(
            w["icon"],
            "edgecfg://widget/dev.edgehost.hello-edge/0.1.0/resources/icon.svg"
        );
        let font = w["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "fontScale")
            .unwrap();
        assert_eq!(font["kind"], "slider");
        assert_eq!(font["label"], "Text Size");
        assert_eq!(
            (
                font["min"].as_f64(),
                font["max"].as_f64(),
                font["unit"].as_str()
            ),
            (Some(50.0), Some(200.0), Some("%"))
        );
        assert_eq!(w["groups"][0]["title"], "Content");
    }

    #[test]
    fn add_move_set_unset_remove() {
        let (_d, store) = store_with_hello();
        let mut layout = Layout::default();
        let Outcome::Changed(r) = call(
            &store,
            &mut layout,
            "layout.add",
            &json!({ "widget": "dev.edgehost.hello-edge", "slot": "S", "page": 0 }),
        )
        .unwrap() else {
            panic!()
        };
        let id = r["instance"].as_str().unwrap().to_owned();
        call(
            &store,
            &mut layout,
            "layout.move",
            &json!({ "instance": id, "page": 0, "slot": "M", "origin": { "col": 2, "row": 0 } }),
        )
        .unwrap();
        assert_eq!(layout.pages[0].instances[0].origin, Cell { col: 2, row: 0 });
        call(
            &store,
            &mut layout,
            "layout.set",
            &json!({ "instance": id, "key": "textColor", "value": "#ff0000" }),
        )
        .unwrap();
        assert!(
            call(
                &store,
                &mut layout,
                "layout.set",
                &json!({ "instance": id, "key": "nope", "value": 1 })
            )
            .is_err()
        );
        call(
            &store,
            &mut layout,
            "layout.unset",
            &json!({ "instance": id, "key": "textColor" }),
        )
        .unwrap();
        assert!(
            !layout.pages[0].instances[0]
                .values
                .contains_key("textColor")
        );
        let err = call(
            &store,
            &mut layout,
            "widgets.uninstall",
            &json!({ "widget": "dev.edgehost.hello-edge" }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("remove them first"));
        call(
            &store,
            &mut layout,
            "layout.remove",
            &json!({ "instance": id }),
        )
        .unwrap();
        assert_eq!(layout.pages, Vec::new());
        assert!(call(&store, &mut layout, "layout.bogus", &json!({})).is_err());
    }

    #[test]
    fn display_and_orientation_reopen_the_surface() {
        let (_d, store) = store_with_hello();
        let mut layout = Layout::default();
        assert_eq!(
            call(
                &store,
                &mut layout,
                "layout.display",
                &json!({ "choice": "windowed" })
            )
            .unwrap(),
            Outcome::Reopen(json!({}))
        );
        assert_eq!(layout.display.as_deref(), Some("windowed"));
        call(
            &store,
            &mut layout,
            "layout.display",
            &json!({ "choice": "auto" }),
        )
        .unwrap();
        assert_eq!(layout.display, None);
        call(
            &store,
            &mut layout,
            "layout.orientation",
            &json!({ "orientation": "portrait" }),
        )
        .unwrap();
        assert_eq!(layout.orientation, Orientation::Portrait);
    }

    #[test]
    fn label_and_option_resolution() {
        let tr = BTreeMap::from([
            ("Left".to_owned(), "Links".to_owned()),
            ("Size".to_owned(), "Größe".to_owned()),
        ]);
        assert_eq!(display_text("tr('Size')", &tr), "Größe");
        assert_eq!(display_text("tr(\"Other\")", &tr), "Other");
        assert_eq!(display_text("'Plain'", &tr), "Plain");
        assert_eq!(display_text("`template ${x}`", &tr), "`template ${x}`");
        assert_eq!(
            literal_options(
                "[{'key':'left','value':tr('Left')},{'key':'right','value':'Right'}]",
                &tr
            ),
            Some(vec![
                ("left".into(), "Links".into()),
                ("right".into(), "Right".into())
            ])
        );
        assert_eq!(
            literal_options("['2','3']", &tr),
            Some(vec![("2".into(), "2".into()), ("3".into(), "3".into())])
        );
        assert_eq!(literal_options("CitySearch.search", &tr), None);
    }
}
