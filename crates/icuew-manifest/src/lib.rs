//! Parsing for iCUE HTML widgets (Widget API 1.9.0, see `docs/research/icue-widget-api-reference.md`).
//!
//! A widget directory holds `manifest.json`, `index.html` and optionally `translation.json`.
//! [`WidgetDef::load`] reads all three; the pure parsers ([`Manifest::from_json`],
//! [`HeadInfo::parse`], [`Translations::from_json`]) are usable on in-memory text.
//!
//! Attribute values such as `data-default` are JavaScript expressions. They are kept as raw
//! text ([`PropertyDef::attrs`]) and evaluated inside the widget's web view, never in Rust.

mod head;
mod manifest;
mod translations;
mod validate;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use head::{ControlKind, HeadInfo, PropertyDef, PropertyGroup, PropertyName};
pub use manifest::{
    DeviceType, Manifest, OsEntry, Platform, PluginRef, SupportedDevice, UrlPermission, WidgetId,
};
pub use translations::Translations;
pub use validate::{Diagnostic, Report, Severity, validate_dir};

/// Errors produced while loading or parsing a widget.
#[derive(Debug, thiserror::Error)]
pub enum WidgetError {
    /// A required file could not be read.
    #[error("reading {path}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// `manifest.json` or `translation.json` is not valid for its schema.
    #[error("parsing {file}")]
    Json {
        /// Which file (`manifest.json` / `translation.json`).
        file: &'static str,
        /// Underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// The widget id is not reverse-DNS characters only.
    #[error("invalid widget id {0:?}: only letters, digits, '.' and '-' are allowed")]
    InvalidWidgetId(String),
    /// A `required_plugins` entry is not `namespace:Name:version`.
    #[error("invalid plugin reference {0:?}: expected namespace:Name:major.minor")]
    InvalidPluginRef(String),
    /// An `x-icue-property` has an unknown `data-type` (iCUE: "Invalid meta parameter data type").
    #[error("invalid meta parameter data type {data_type:?} on property {property:?}")]
    InvalidDataType {
        /// Property `content` name.
        property: String,
        /// The offending `data-type` value.
        data_type: String,
    },
    /// An `x-icue-property` is missing a required attribute.
    #[error("x-icue-property {property:?} is missing {attribute}")]
    MissingAttribute {
        /// Property `content` name (empty if `content` itself is missing).
        property: String,
        /// Missing attribute name.
        attribute: &'static str,
    },
    /// A property name is not usable as a JavaScript global.
    #[error(
        "invalid property name {0:?}: use letters, digits and '_' and do not start with a digit"
    )]
    InvalidPropertyName(String),
    /// The same property name is declared twice.
    #[error("duplicate x-icue-property {0:?}")]
    DuplicateProperty(String),
    /// The `x-icue-groups` script is not a valid JSON array of groups.
    #[error("parsing x-icue-groups")]
    Groups(#[source] serde_json::Error),
}

/// A fully parsed widget directory.
#[derive(Debug)]
pub struct WidgetDef {
    root: PathBuf,
    /// Parsed `manifest.json`.
    pub manifest: Manifest,
    /// Parsed `<head>` of `index.html`.
    pub head: HeadInfo,
    /// Parsed `translation.json` (empty if the file is absent).
    pub translations: Translations,
}

impl WidgetDef {
    /// Load and parse a widget directory.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::Io`] if `manifest.json` or `index.html` cannot be read, and the
    /// parse errors of [`Manifest::from_json`], [`HeadInfo::parse`] and
    /// [`Translations::from_json`].
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, WidgetError> {
        let root = dir.as_ref().to_path_buf();
        let manifest = Manifest::from_json(&read(&root.join("manifest.json"))?)?;
        let head = HeadInfo::parse(&read(&root.join("index.html"))?)?;
        let tr_path = root.join("translation.json");
        let translations = match fs::read_to_string(&tr_path) {
            Ok(text) => Translations::from_json(&text)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Translations::default(),
            Err(source) => {
                return Err(WidgetError::Io {
                    path: tr_path,
                    source,
                });
            }
        };
        Ok(Self {
            root,
            manifest,
            head,
            translations,
        })
    }

    /// Directory the widget was loaded from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn read(path: &Path) -> Result<String, WidgetError> {
    fs::read_to_string(path).map_err(|source| WidgetError::Io {
        path: path.to_path_buf(),
        source,
    })
}
