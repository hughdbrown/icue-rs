//! `manifest.json` schema.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer};

use crate::WidgetError;

/// Reverse-DNS widget identifier, e.g. `com.corsair.weatherwidget`.
///
/// Accepts the same characters as `icuewidget-cli` 0.4.48 (`^[A-Za-z0-9.-]+$`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WidgetId(String);

impl WidgetId {
    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for WidgetId {
    type Err = WidgetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(WidgetError::InvalidWidgetId(s.to_owned()))
        }
    }
}

impl fmt::Display for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WidgetId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A `required_plugins` entry: `namespace:Name:major.minor`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginRef {
    /// Module name, e.g. `widgetbuilder.sensorsdataprovider`.
    pub module: String,
    /// Plugin name, e.g. `Sensors`.
    pub name: String,
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
}

impl PluginRef {
    /// Name of the `window.plugins` facade: the module name without its namespace, first letter
    /// upper-cased and the rest lower-cased (`widgetbuilder.sensorsdataprovider` → `Sensorsdataprovider`).
    #[must_use]
    pub fn facade_name(&self) -> String {
        let last = self.module.rsplit('.').next().unwrap_or(&self.module);
        let mut chars = last.chars();
        chars.next().map_or_else(String::new, |first| {
            first
                .to_uppercase()
                .chain(chars.flat_map(char::to_lowercase))
                .collect()
        })
    }
}

impl FromStr for PluginRef {
    type Err = WidgetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || WidgetError::InvalidPluginRef(s.to_owned());
        let mut parts = s.split(':');
        let (Some(module), Some(name), Some(version), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let (major, minor) = version.split_once('.').ok_or_else(bad)?;
        if module.is_empty() || name.is_empty() {
            return Err(bad());
        }
        Ok(Self {
            module: module.to_owned(),
            name: name.to_owned(),
            major: major.parse().map_err(|_| bad())?,
            minor: minor.parse().map_err(|_| bad())?,
        })
    }
}

impl fmt::Display for PluginRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}.{}",
            self.module, self.name, self.major, self.minor
        )
    }
}

impl<'de> Deserialize<'de> for PluginRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Operating system a widget declares support for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// The only platform the official spec allows today.
    Windows,
    /// Seen in some official demo manifests.
    Mac,
    /// Any other value (e.g. `linux` in Corsair's Raspberry Pi runner samples). `os` is advisory.
    #[serde(other)]
    Other,
}

/// One `os` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OsEntry {
    /// Declared platform.
    pub platform: Platform,
}

/// Corsair LCD device class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    /// XENEON EDGE.
    DashboardLcd,
    /// Keyboards with an LCD (VANGUARD 96 family).
    KeyboardLcd,
    /// AIO pump LCDs.
    PumpLcd,
}

/// One `supported_devices` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SupportedDevice {
    /// Device class.
    #[serde(rename = "type")]
    pub kind: DeviceType,
    /// Optional required features, e.g. `sensor-screen`.
    #[serde(default)]
    pub features: Vec<String>,
}

/// Parsed `manifest.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// Author name.
    pub author: String,
    /// Reverse-DNS id.
    pub id: WidgetId,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Semver widget version.
    pub version: String,
    /// Icon path relative to the manifest.
    pub preview_icon: String,
    /// Minimum Widget API version.
    pub min_framework_version: String,
    /// Minimum iCUE version. Required by the spec but omitted by some official examples.
    #[serde(default)]
    pub min_app_version: Option<String>,
    /// Declared platforms (advisory on macOS).
    pub os: Vec<OsEntry>,
    /// Device classes the widget targets.
    pub supported_devices: Vec<SupportedDevice>,
    /// Whether the widget handles touch input.
    #[serde(default)]
    pub interactive: bool,
    /// Plugins the widget needs.
    #[serde(default)]
    pub required_plugins: Vec<PluginRef>,
    /// ES module paths whose exports become globals named after the file stem.
    #[serde(default)]
    pub modules: Vec<String>,
    /// Permission declarations, enforced from milestone M3.
    #[serde(default)]
    pub permissions: Vec<serde_json::Value>,
}

impl Manifest {
    /// Parse `manifest.json` text.
    ///
    /// # Errors
    ///
    /// [`WidgetError::Json`] for missing required keys, wrong types, an invalid `id`, unknown
    /// device types or malformed `required_plugins` entries.
    pub fn from_json(text: &str) -> Result<Self, WidgetError> {
        serde_json::from_str(text).map_err(|source| WidgetError::Json {
            file: "manifest.json",
            source,
        })
    }

    /// Whether the widget declares support for the XENEON EDGE.
    #[must_use]
    pub fn supports_edge(&self) -> bool {
        self.supported_devices
            .iter()
            .any(|d| d.kind == DeviceType::DashboardLcd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
        "author": "A", "id": "com.example.w", "name": "W", "description": "D",
        "version": "1.0.0", "preview_icon": "resources/icon.svg",
        "min_app_version": "5.47", "min_framework_version": "1.0.0",
        "os": [{"platform": "windows"}],
        "supported_devices": [{"type": "dashboard_lcd", "features": ["sensor-screen"]}, {"type": "pump_lcd"}],
        "required_plugins": ["widgetbuilder.sensorsdataprovider:Sensors:1.0"]
    }"#;

    #[test]
    fn parses_minimal_manifest() {
        let m = Manifest::from_json(MINIMAL).unwrap();
        assert_eq!(m.id.as_str(), "com.example.w");
        assert!(m.supports_edge());
        assert!(!m.interactive);
        assert_eq!(m.required_plugins[0].facade_name(), "Sensorsdataprovider");
        assert_eq!(
            m.required_plugins[0].to_string(),
            "widgetbuilder.sensorsdataprovider:Sensors:1.0"
        );
    }

    #[test]
    fn rejects_bad_id_and_unknown_device() {
        let bad_id = MINIMAL.replace("com.example.w", "com example");
        assert!(Manifest::from_json(&bad_id).is_err());
        let bad_dev = MINIMAL.replace("pump_lcd", "mouse_lcd");
        assert!(Manifest::from_json(&bad_dev).is_err());
        let linux = MINIMAL.replace(r#""windows""#, r#""linux""#);
        assert_eq!(
            Manifest::from_json(&linux).unwrap().os[0].platform,
            Platform::Other
        );
        let missing = MINIMAL.replace(r#""author": "A", "#, "");
        assert!(Manifest::from_json(&missing).is_err());
    }

    #[test]
    fn plugin_ref_parsing() {
        let p: PluginRef = "widgetbuilder.mediadataprovider:Media:1.1".parse().unwrap();
        assert_eq!((p.major, p.minor), (1, 1));
        assert_eq!(p.facade_name(), "Mediadataprovider");
        for bad in ["", "a:b", "a:b:1", "a:b:1.x", ":b:1.0", "a:b:1.0:extra"] {
            assert!(bad.parse::<PluginRef>().is_err(), "{bad:?}");
        }
    }
}
