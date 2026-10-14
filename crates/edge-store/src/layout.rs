//! `layout.json`: pages of widget instances and their property values.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::str::FromStr;

use edge_layout::{Cell, Orientation, Slot};
use icuew_manifest::WidgetId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Current `layout.json` schema version.
pub const SCHEMA: u32 = 1;

/// Identifier of a placed widget instance (its `uniqueId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstanceId(pub Uuid);

impl InstanceId {
    /// A fresh random id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// First 8 hex digits, for display.
    #[must_use]
    pub fn short(&self) -> String {
        self.0.simple().to_string().chars().take(8).collect()
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

/// The saved dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// Schema version of this file.
    pub schema: u32,
    /// Display choice (`auto`, `windowed`, or an index / name); `None` means auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// Grid orientation the pages are laid out for.
    #[serde(default)]
    pub orientation: Orientation,
    /// Pages, shown one at a time.
    #[serde(default)]
    pub pages: Vec<Page>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            display: None,
            orientation: Orientation::Landscape,
            pages: Vec::new(),
        }
    }
}

/// One page of widgets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    /// Widgets on this page.
    #[serde(default)]
    pub instances: Vec<Instance>,
}

/// A widget placed on a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    /// Instance id (the widget's `uniqueId`, and its storage key).
    pub id: InstanceId,
    /// Installed widget.
    pub widget: WidgetId,
    /// Installed version in use.
    pub version: String,
    /// Slot size.
    pub slot: Slot,
    /// Top-left grid cell.
    pub origin: Cell,
    /// Concrete property values injected as globals.
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
    /// Properties set by the user; evaluated defaults never overwrite these.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub user_set: BTreeSet<String>,
    /// Widget version whose defaults have been evaluated into `values`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluated_version: Option<String>,
}

impl Instance {
    /// Whether defaults must be (re-)evaluated before showing this instance.
    #[must_use]
    pub fn needs_evaluation(&self) -> bool {
        self.evaluated_version.as_deref() != Some(self.version.as_str())
    }
}

/// Errors editing a layout.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LayoutError {
    /// Page index beyond the end (pages may only be appended one at a time).
    #[error("page {page} does not exist (the layout has {pages} page(s); use {pages} to add one)")]
    NoSuchPage {
        /// Requested page.
        page: usize,
        /// Number of pages.
        pages: usize,
    },
    /// The slot does not fit at that position.
    #[error(transparent)]
    Grid(#[from] edge_layout::LayoutError),
    /// The slot overlaps another instance.
    #[error("{slot} at column {col}, row {row} overlaps instance {other}", col = origin.col, row = origin.row)]
    Overlap {
        /// Slot being placed.
        slot: Slot,
        /// Requested position.
        origin: Cell,
        /// Short id of the instance in the way.
        other: String,
    },
    /// No free position on the page for the slot.
    #[error("no room for a {slot} widget on page {page}")]
    NoRoom {
        /// Slot being placed.
        slot: Slot,
        /// Page.
        page: usize,
    },
    /// No instance id starts with the given text.
    #[error("no widget instance matches {0:?}")]
    UnknownInstance(String),
    /// Several instance ids start with the given text.
    #[error("{0:?} matches more than one instance; give more characters")]
    AmbiguousInstance(String),
}

impl Layout {
    /// Every instance with its page index.
    pub fn instances(&self) -> impl Iterator<Item = (usize, &Instance)> {
        self.pages
            .iter()
            .enumerate()
            .flat_map(|(p, page)| page.instances.iter().map(move |i| (p, i)))
    }

    /// Resolve a full id or a unique prefix of its hex digits.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownInstance`] or [`LayoutError::AmbiguousInstance`].
    pub fn resolve(&self, query: &str) -> Result<InstanceId, LayoutError> {
        let needle = query.to_ascii_lowercase().replace('-', "");
        let mut hits = self
            .instances()
            .map(|(_, i)| i.id)
            .filter(|id| id.0.simple().to_string().starts_with(&needle));
        match (hits.next(), hits.next()) {
            (Some(id), None) if !needle.is_empty() => Ok(id),
            (Some(_), Some(_)) => Err(LayoutError::AmbiguousInstance(query.to_owned())),
            _ => Err(LayoutError::UnknownInstance(query.to_owned())),
        }
    }

    /// Mutable access to an instance.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownInstance`].
    pub fn get_mut(&mut self, id: InstanceId) -> Result<&mut Instance, LayoutError> {
        self.pages
            .iter_mut()
            .flat_map(|p| p.instances.iter_mut())
            .find(|i| i.id == id)
            .ok_or_else(|| LayoutError::UnknownInstance(id.to_string()))
    }

    /// Place a widget on `page` (which may be one past the last page to add a page). With no
    /// `origin`, the first free position in reading order is used.
    ///
    /// # Errors
    ///
    /// [`LayoutError::NoSuchPage`], [`LayoutError::Grid`], [`LayoutError::Overlap`] or
    /// [`LayoutError::NoRoom`].
    pub fn add(
        &mut self,
        page: usize,
        widget: WidgetId,
        version: String,
        slot: Slot,
        origin: Option<Cell>,
    ) -> Result<InstanceId, LayoutError> {
        if page > self.pages.len() {
            return Err(LayoutError::NoSuchPage {
                page,
                pages: self.pages.len(),
            });
        }
        let orientation = self.orientation;
        let occupied = self
            .pages
            .get(page)
            .map(|p| occupied_cells(p, orientation))
            .unwrap_or_default();
        let origin = match origin {
            Some(cell) => {
                let cells =
                    slot.cells(orientation, cell)
                        .ok_or(edge_layout::LayoutError::OutOfBounds {
                            slot,
                            origin: cell,
                            orientation,
                        })?;
                if let Some(other) = cells.iter().find_map(|c| occupied.get(c)) {
                    return Err(LayoutError::Overlap {
                        slot,
                        origin: cell,
                        other: other.short(),
                    });
                }
                cell
            }
            None => orientation
                .all_cells()
                .find(|&c| {
                    slot.cells(orientation, c)
                        .is_some_and(|cells| cells.iter().all(|x| !occupied.contains_key(x)))
                })
                .ok_or(LayoutError::NoRoom { slot, page })?,
        };
        if page == self.pages.len() {
            self.pages.push(Page::default());
        }
        let id = InstanceId::new();
        let instance = Instance {
            id,
            widget,
            version,
            slot,
            origin,
            values: BTreeMap::new(),
            user_set: BTreeSet::new(),
            evaluated_version: None,
        };
        if let Some(p) = self.pages.get_mut(page) {
            p.instances.push(instance);
        }
        Ok(id)
    }

    /// Remove an instance; empty trailing pages are dropped.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownInstance`].
    pub fn remove(&mut self, id: InstanceId) -> Result<Instance, LayoutError> {
        for page in &mut self.pages {
            if let Some(pos) = page.instances.iter().position(|i| i.id == id) {
                let removed = page.instances.remove(pos);
                while self.pages.last().is_some_and(|p| p.instances.is_empty()) {
                    self.pages.pop();
                }
                return Ok(removed);
            }
        }
        Err(LayoutError::UnknownInstance(id.to_string()))
    }

    /// Set a property value chosen by the user.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownInstance`].
    pub fn set_value(
        &mut self,
        id: InstanceId,
        key: &str,
        value: Value,
    ) -> Result<(), LayoutError> {
        let inst = self.get_mut(id)?;
        inst.values.insert(key.to_owned(), value);
        inst.user_set.insert(key.to_owned());
        Ok(())
    }

    /// Forget a user-set value; the default is restored on the next evaluation.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownInstance`].
    pub fn unset_value(&mut self, id: InstanceId, key: &str) -> Result<(), LayoutError> {
        let inst = self.get_mut(id)?;
        inst.values.remove(key);
        inst.user_set.remove(key);
        inst.evaluated_version = None;
        Ok(())
    }

    /// Merge evaluated defaults, keeping user-set values, and mark the version as evaluated.
    ///
    /// # Errors
    ///
    /// [`LayoutError::UnknownInstance`].
    pub fn apply_evaluated(
        &mut self,
        id: InstanceId,
        values: BTreeMap<String, Value>,
    ) -> Result<(), LayoutError> {
        let inst = self.get_mut(id)?;
        for (k, v) in values {
            if !inst.user_set.contains(&k) {
                inst.values.insert(k, v);
            }
        }
        inst.evaluated_version = Some(inst.version.clone());
        Ok(())
    }
}

fn occupied_cells(page: &Page, orientation: Orientation) -> HashMap<Cell, InstanceId> {
    page.instances
        .iter()
        .flat_map(|i| {
            i.slot
                .cells(orientation, i.origin)
                .unwrap_or_default()
                .into_iter()
                .map(move |c| (c, i.id))
        })
        .collect()
}

impl FromStr for InstanceId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> WidgetId {
        "dev.edgehost.hello-edge".parse().unwrap()
    }

    #[test]
    fn auto_placement_fills_reading_order() {
        let mut l = Layout::default();
        let a = l.add(0, id(), "1.0.0".into(), Slot::L, None).unwrap();
        let b = l.add(0, id(), "1.0.0".into(), Slot::S, None).unwrap();
        let c = l.add(0, id(), "1.0.0".into(), Slot::S, None).unwrap();
        let origin = |x| {
            l.instances()
                .find(|(_, i)| i.id == x)
                .map(|(_, i)| i.origin)
                .unwrap()
        };
        assert_eq!(origin(a), Cell { col: 0, row: 0 });
        assert_eq!(origin(b), Cell { col: 2, row: 0 });
        assert_eq!(origin(c), Cell { col: 2, row: 1 });
        assert_eq!(
            l.add(0, id(), "1.0.0".into(), Slot::S, None),
            Err(LayoutError::NoRoom {
                slot: Slot::S,
                page: 0
            })
        );
        assert!(l.add(1, id(), "1.0.0".into(), Slot::XL, None).is_ok());
        assert!(matches!(
            l.add(3, id(), "1.0.0".into(), Slot::S, None),
            Err(LayoutError::NoSuchPage { .. })
        ));
    }

    #[test]
    fn explicit_placement_checks_overlap_and_bounds() {
        let mut l = Layout::default();
        l.add(0, id(), "1".into(), Slot::M, Some(Cell { col: 1, row: 0 }))
            .unwrap();
        assert!(matches!(
            l.add(0, id(), "1".into(), Slot::S, Some(Cell { col: 1, row: 1 })),
            Err(LayoutError::Overlap { .. })
        ));
        assert!(matches!(
            l.add(0, id(), "1".into(), Slot::L, Some(Cell { col: 2, row: 0 })),
            Err(LayoutError::Grid(_))
        ));
    }

    #[test]
    fn values_and_evaluation() {
        let mut l = Layout::default();
        let x = l.add(0, id(), "1.0.0".into(), Slot::S, None).unwrap();
        assert!(l.get_mut(x).unwrap().needs_evaluation());
        l.set_value(x, "textColor", Value::from("#ff0000")).unwrap();
        l.apply_evaluated(
            x,
            BTreeMap::from([
                ("textColor".into(), Value::from("#ffffff")),
                ("size".into(), Value::from(50)),
            ]),
        )
        .unwrap();
        let inst = l.get_mut(x).unwrap();
        assert_eq!(inst.values["textColor"], "#ff0000");
        assert_eq!(inst.values["size"], 50);
        assert!(!inst.needs_evaluation());
        l.unset_value(x, "textColor").unwrap();
        assert!(l.get_mut(x).unwrap().needs_evaluation());
    }

    #[test]
    fn resolve_by_prefix_and_remove() {
        let mut l = Layout::default();
        let x = l.add(0, id(), "1".into(), Slot::S, None).unwrap();
        assert_eq!(l.resolve(&x.short()).unwrap(), x);
        assert_eq!(l.resolve(&x.to_string()).unwrap(), x);
        assert!(l.resolve("zzzz").is_err());
        assert!(l.resolve("").is_err());
        l.remove(x).unwrap();
        assert_eq!(l.pages, Vec::new());
    }

    #[test]
    fn json_shape() {
        let mut l = Layout::default();
        l.add(0, id(), "1.0.0".into(), Slot::XL, None).unwrap();
        let json = serde_json::to_value(&l).unwrap();
        assert_eq!(json["orientation"], "landscape");
        assert_eq!(json["pages"][0]["instances"][0]["slot"], "XL");
        assert_eq!(
            json["pages"][0]["instances"][0]["origin"],
            serde_json::json!({"col": 0, "row": 0})
        );
        let back: Layout = serde_json::from_value(json).unwrap();
        assert_eq!(back, l);
    }
}
