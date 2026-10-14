//! `<head>` metadata of `index.html`: title, icon, group, preview, controls and groups.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::str::FromStr;

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use crate::WidgetError;

/// Control type of an `x-icue-property` (`data-type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ControlKind {
    /// Numeric slider.
    Slider,
    /// Boolean toggle.
    Switch,
    /// Free text.
    Textfield,
    /// Hex colour picker.
    Color,
    /// Drop-down list.
    Combobox,
    /// Drop-down with search provider.
    SearchCombobox,
    /// Segmented buttons.
    TabButtons,
    /// Image/video picker.
    MediaSelector,
    /// Single sensor picker.
    SensorsCombobox,
    /// List of sensors with colours.
    SensorsFactory,
    /// Stream Deck virtual device picker.
    StreamDeckVirtualDevice,
}

impl FromStr for ControlKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "slider" => Self::Slider,
            "switch" => Self::Switch,
            "textfield" => Self::Textfield,
            "color" => Self::Color,
            "combobox" => Self::Combobox,
            "search-combobox" => Self::SearchCombobox,
            "tab-buttons" => Self::TabButtons,
            "media-selector" => Self::MediaSelector,
            "sensors-combobox" => Self::SensorsCombobox,
            "sensors-factory" => Self::SensorsFactory,
            "stream-deck-virtual-device" => Self::StreamDeckVirtualDevice,
            _ => return Err(()),
        })
    }
}

/// A property name that is safe to use as a JavaScript global (`[A-Za-z_][A-Za-z0-9_]*`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PropertyName(String);

impl PropertyName {
    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for PropertyName {
    type Err = WidgetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut chars = s.chars();
        let valid = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(WidgetError::InvalidPropertyName(s.to_owned()))
        }
    }
}

impl fmt::Display for PropertyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One `<meta name="x-icue-property">` control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyDef {
    /// `content`: the JavaScript global the value is assigned to.
    pub name: PropertyName,
    /// `data-type`.
    pub kind: ControlKind,
    /// `data-label` expression, if present.
    pub label: Option<String>,
    /// Remaining `data-*` attributes (`default`, `min`, `max`, `values`, …) without the `data-`
    /// prefix, as raw JavaScript expressions.
    pub attrs: BTreeMap<String, String>,
}

impl PropertyDef {
    /// The `data-default` expression, if any.
    #[must_use]
    pub fn default_expr(&self) -> Option<&str> {
        self.attrs.get("default").map(String::as_str)
    }
}

/// One entry of the `x-icue-groups` JSON array.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PropertyGroup {
    /// Heading expression, e.g. `tr('Settings')`.
    pub title: String,
    /// Property names in this group.
    pub properties: Vec<String>,
    /// Optional help-text expression.
    #[serde(default)]
    pub info: Option<String>,
}

/// Widget metadata from the `<head>` of `index.html`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadInfo {
    /// `<title>` text: a literal or an expression such as `tr('Name')`.
    pub title: Option<String>,
    /// `<link rel="icon">` href.
    pub icon: Option<String>,
    /// `x-icue-widget-group` content.
    pub group: Option<String>,
    /// `x-icue-widget-preview` content.
    pub preview: Option<String>,
    /// Controls, in document order.
    pub properties: Vec<PropertyDef>,
    /// Property groups.
    pub groups: Vec<PropertyGroup>,
}

impl HeadInfo {
    /// Parse the widget's `index.html`.
    ///
    /// Parsing is lenient HTML5 (iCUE's stricter XML parsing of `<head>` is a validator concern).
    ///
    /// # Errors
    ///
    /// Invalid or duplicate property names, unknown `data-type` values, missing `content` or
    /// `data-type` attributes, and malformed `x-icue-groups` JSON.
    pub fn parse(html: &str) -> Result<Self, WidgetError> {
        let doc = Html::parse_document(html);
        let mut info = Self {
            title: select_first(&doc, "head > title")
                .map(|e| e.text().collect::<String>().trim().to_owned())
                .filter(|t| !t.is_empty()),
            icon: select_first(&doc, r#"link[rel~="icon"]"#)
                .and_then(|e| e.attr("href"))
                .map(str::to_owned),
            group: meta_content(&doc, "x-icue-widget-group"),
            preview: meta_content(&doc, "x-icue-widget-preview"),
            ..Self::default()
        };

        let mut seen = HashSet::new();
        for el in doc.select(&selector(r#"meta[name="x-icue-property"]"#)) {
            let e = el.value();
            let content = e.attr("content").ok_or(WidgetError::MissingAttribute {
                property: String::new(),
                attribute: "content",
            })?;
            let name: PropertyName = content.parse()?;
            let data_type = e
                .attr("data-type")
                .ok_or_else(|| WidgetError::MissingAttribute {
                    property: content.to_owned(),
                    attribute: "data-type",
                })?;
            let kind = data_type
                .parse()
                .map_err(|()| WidgetError::InvalidDataType {
                    property: content.to_owned(),
                    data_type: data_type.to_owned(),
                })?;
            if !seen.insert(name.clone()) {
                return Err(WidgetError::DuplicateProperty(content.to_owned()));
            }
            let attrs = e
                .attrs()
                .filter_map(|(k, v)| k.strip_prefix("data-").map(|k| (k, v)))
                .filter(|(k, _)| *k != "type" && *k != "label")
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            info.properties.push(PropertyDef {
                name,
                kind,
                label: e.attr("data-label").map(str::to_owned),
                attrs,
            });
        }

        if let Some(script) = select_first(&doc, "script#x-icue-groups") {
            let text: String = script.text().collect();
            info.groups = serde_json::from_str(&text).map_err(WidgetError::Groups)?;
        }
        Ok(info)
    }
}

fn selector(css: &str) -> Selector {
    // Only called with the literal selectors in this module, which are valid CSS.
    Selector::parse(css).unwrap_or_else(|e| unreachable!("invalid built-in selector {css:?}: {e}"))
}

fn select_first<'a>(doc: &'a Html, css: &str) -> Option<scraper::ElementRef<'a>> {
    doc.select(&selector(css)).next()
}

fn meta_content(doc: &Html, name: &str) -> Option<String> {
    doc.select(&selector("meta[name]"))
        .find(|e| e.value().attr("name") == Some(name))
        .and_then(|e| e.value().attr("content"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"<!doctype html>
<html><head>
  <title>tr('Hello')</title>
  <link rel="icon" type="image/svg+xml" href="resources/icon.svg" />
  <meta name="x-icue-widget-group" content="tr('Greeting')" />
  <meta name="x-icue-property" content="textColor" data-label="tr('Text Color')" data-type="color" data-default="'#FFFFFF'" />
  <meta name="x-icue-property" content="size" data-label="'Size'" data-type="slider" data-default="50" data-min="10" data-max="100" />
  <script type="application/json" id="x-icue-groups">[{"title": "tr('Look')", "properties": ["textColor", "size"]}]</script>
</head><body></body></html>"#;

    #[test]
    fn parses_controls_and_groups() {
        let h = HeadInfo::parse(HTML).unwrap();
        assert_eq!(h.title.as_deref(), Some("tr('Hello')"));
        assert_eq!(h.icon.as_deref(), Some("resources/icon.svg"));
        assert_eq!(h.group.as_deref(), Some("tr('Greeting')"));
        assert_eq!(h.properties.len(), 2);
        let size = &h.properties[1];
        assert_eq!(size.name.as_str(), "size");
        assert_eq!(size.kind, ControlKind::Slider);
        assert_eq!(size.default_expr(), Some("50"));
        assert_eq!(size.attrs.get("max").map(String::as_str), Some("100"));
        assert!(!size.attrs.contains_key("type"));
        assert_eq!(h.groups[0].properties, ["textColor", "size"]);
    }

    #[test]
    fn rejects_unknown_type_duplicates_and_bad_names() {
        let bad_type = HTML.replace(r#"data-type="color""#, r#"data-type="colour""#);
        assert!(matches!(
            HeadInfo::parse(&bad_type),
            Err(WidgetError::InvalidDataType { .. })
        ));
        let dup = HTML.replace(r#"content="size""#, r#"content="textColor""#);
        assert!(matches!(
            HeadInfo::parse(&dup),
            Err(WidgetError::DuplicateProperty(_))
        ));
        let bad_name = HTML.replace(r#"content="size""#, r#"content="1size""#);
        assert!(matches!(
            HeadInfo::parse(&bad_name),
            Err(WidgetError::InvalidPropertyName(_))
        ));
        let bad_groups = HTML.replace(r#"[{"title""#, r#"{{"title""#);
        assert!(matches!(
            HeadInfo::parse(&bad_groups),
            Err(WidgetError::Groups(_))
        ));
    }

    #[test]
    fn all_documented_control_types_parse() {
        for t in [
            "slider",
            "switch",
            "textfield",
            "color",
            "combobox",
            "search-combobox",
            "tab-buttons",
            "media-selector",
            "sensors-combobox",
            "sensors-factory",
            "stream-deck-virtual-device",
        ] {
            assert!(t.parse::<ControlKind>().is_ok(), "{t}");
        }
    }
}
