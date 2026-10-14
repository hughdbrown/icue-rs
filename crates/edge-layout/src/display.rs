//! Choosing the display that shows the Edge surface, and fitting the panel into it.
//!
//! Selection is pure: the caller describes the connected displays as [`DisplayInfo`] (from tao and
//! CoreGraphics) and gets back a [`Selection`].

use std::fmt;
use std::str::FromStr;

use crate::{LayoutError, Orientation};

/// EDID manufacturer id of the XENEON EDGE as reported by `CGDisplayVendorNumber` ("CRX").
pub const EDGE_EDID_VENDOR: u32 = 0x0e58;
/// EDID product id of the XENEON EDGE as reported by `CGDisplayModelNumber`.
pub const EDGE_EDID_MODEL: u32 = 0xed00;

/// A connected display.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    /// Name reported by the windowing system (tao calls the Edge `Monitor #60672`).
    pub name: String,
    /// EDID vendor number, when known.
    pub vendor: Option<u32>,
    /// EDID model number, when known.
    pub model: Option<u32>,
    /// Current mode in device pixels `(width, height)`.
    pub pixel_size: (u32, u32),
    /// Device pixels per logical point.
    pub scale_factor: f64,
    /// Top-left in global logical coordinates.
    pub logical_origin: (f64, f64),
    /// Size in logical points.
    pub logical_size: (f64, f64),
}

impl DisplayInfo {
    /// Whether the EDID identifies this display as a XENEON EDGE.
    #[must_use]
    pub fn is_edge_hardware(&self) -> bool {
        self.vendor == Some(EDGE_EDID_VENDOR) && self.model == Some(EDGE_EDID_MODEL)
    }

    /// The orientation whose native panel size equals the current pixel size, if any.
    #[must_use]
    pub fn native_orientation(&self) -> Option<Orientation> {
        [Orientation::Landscape, Orientation::Portrait]
            .into_iter()
            .find(|o| o.panel_size() == self.pixel_size)
    }

    /// Orientation to lay the panel out in: native if the mode matches, otherwise by aspect.
    #[must_use]
    pub fn orientation(&self) -> Orientation {
        self.native_orientation()
            .unwrap_or(if self.pixel_size.0 >= self.pixel_size.1 {
                Orientation::Landscape
            } else {
                Orientation::Portrait
            })
    }
}

/// Which display to use, as given by the user (`--display`, or `display` in `layout.json`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DisplayChoice {
    /// The XENEON EDGE by EDID, else a display in its native size, else a stand-in window.
    #[default]
    Auto,
    /// Always a stand-in window on the current display.
    Windowed,
    /// A display by index (as listed by `edgectl displays`) or case-insensitive name substring.
    Pick(String),
}

impl FromStr for DisplayChoice {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Self::Auto,
            "windowed" | "window" => Self::Windowed,
            _ => Self::Pick(s.trim().to_owned()),
        })
    }
}

impl fmt::Display for DisplayChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Windowed => f.write_str("windowed"),
            Self::Pick(s) => f.write_str(s),
        }
    }
}

/// Result of [`select_display`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Cover display `index` with the surface.
    Display {
        /// Index into the slice given to [`select_display`].
        index: usize,
        /// Orientation to lay out in.
        orientation: Orientation,
        /// The EDID says this is a XENEON EDGE.
        edge_hardware: bool,
        /// The current mode is the panel's native 2560×720 / 720×2560.
        native: bool,
    },
    /// No suitable display: use a stand-in window.
    Windowed,
}

/// Choose the display for the surface.
///
/// ```
/// use edge_layout::{select_display, DisplayChoice, DisplayInfo, Orientation, Selection};
/// let edge = DisplayInfo {
///     name: "Monitor #60672".into(), vendor: Some(0x0e58), model: Some(0xed00),
///     pixel_size: (2560, 720), scale_factor: 1.0,
///     logical_origin: (1496.0, 0.0), logical_size: (2560.0, 720.0),
/// };
/// let sel = select_display(&[edge], &DisplayChoice::Auto).unwrap();
/// assert_eq!(sel, Selection::Display { index: 0, orientation: Orientation::Landscape,
///                                      edge_hardware: true, native: true });
/// ```
///
/// # Errors
///
/// [`LayoutError::NoSuchDisplay`] if a [`DisplayChoice::Pick`] matches nothing.
pub fn select_display(
    displays: &[DisplayInfo],
    choice: &DisplayChoice,
) -> Result<Selection, LayoutError> {
    let chosen = |index: usize, d: &DisplayInfo| Selection::Display {
        index,
        orientation: d.orientation(),
        edge_hardware: d.is_edge_hardware(),
        native: d.native_orientation().is_some(),
    };
    match choice {
        DisplayChoice::Windowed => Ok(Selection::Windowed),
        DisplayChoice::Auto => Ok(displays
            .iter()
            .position(DisplayInfo::is_edge_hardware)
            .or_else(|| {
                displays
                    .iter()
                    .position(|d| d.native_orientation().is_some())
            })
            .and_then(|i| displays.get(i).map(|d| chosen(i, d)))
            .unwrap_or(Selection::Windowed)),
        DisplayChoice::Pick(query) => {
            let by_index = query
                .parse::<usize>()
                .ok()
                .and_then(|i| displays.get(i).map(|d| (i, d)));
            let needle = query.to_lowercase();
            by_index
                .or_else(|| {
                    displays
                        .iter()
                        .enumerate()
                        .find(|(_, d)| d.name.to_lowercase().contains(&needle))
                })
                .map(|(i, d)| chosen(i, d))
                .ok_or_else(|| LayoutError::NoSuchDisplay {
                    query: query.clone(),
                    available: displays.iter().map(|d| d.name.clone()).collect(),
                })
        }
    }
}

/// How the panel is drawn inside an area of logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// Logical points per panel pixel.
    pub scale: f64,
    /// Offset of the panel's top-left inside the area, in logical points (letterboxing).
    pub offset: (f64, f64),
}

/// Fit the panel for `orientation` into `area` (logical points), preserving aspect ratio.
///
/// ```
/// use edge_layout::{fit_panel, Orientation};
/// let f = fit_panel(Orientation::Landscape, (2560.0, 720.0));
/// assert!((f.scale - 1.0).abs() < 1e-9 && f.offset == (0.0, 0.0));
/// ```
#[must_use]
pub fn fit_panel(orientation: Orientation, area: (f64, f64)) -> Fit {
    let (pw, ph) = orientation.panel_size();
    let (pw, ph) = (f64::from(pw), f64::from(ph));
    let scale = (area.0 / pw).min(area.1 / ph);
    Fit {
        scale,
        offset: ((area.0 - pw * scale) / 2.0, (area.1 - ph * scale) / 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(name: &str, ids: Option<(u32, u32)>, px: (u32, u32), scale: f64) -> DisplayInfo {
        DisplayInfo {
            name: name.into(),
            vendor: ids.map(|i| i.0),
            model: ids.map(|i| i.1),
            pixel_size: px,
            scale_factor: scale,
            logical_origin: (0.0, 0.0),
            logical_size: (f64::from(px.0) / scale, f64::from(px.1) / scale),
        }
    }

    fn builtin() -> DisplayInfo {
        display("Color LCD", Some((0x610, 0xa051)), (2992, 1934), 2.0)
    }

    #[test]
    fn auto_prefers_edid_then_native_size() {
        let edge_wrong_mode = display("Monitor #60672", Some((0x0e58, 0xed00)), (1920, 1080), 1.0);
        let generic_native = display("Other", None, (2560, 720), 1.0);
        let sel = select_display(
            &[builtin(), generic_native.clone(), edge_wrong_mode],
            &DisplayChoice::Auto,
        )
        .unwrap();
        assert_eq!(
            sel,
            Selection::Display {
                index: 2,
                orientation: Orientation::Landscape,
                edge_hardware: true,
                native: false
            }
        );
        let sel = select_display(&[builtin(), generic_native], &DisplayChoice::Auto).unwrap();
        assert!(matches!(
            sel,
            Selection::Display {
                index: 1,
                native: true,
                edge_hardware: false,
                ..
            }
        ));
        assert_eq!(
            select_display(&[builtin()], &DisplayChoice::Auto).unwrap(),
            Selection::Windowed
        );
    }

    #[test]
    fn pick_by_index_or_name() {
        let ds = [builtin(), display("Monitor #60672", None, (720, 2560), 1.0)];
        let pick = |q: &str| select_display(&ds, &q.parse().unwrap());
        assert!(matches!(
            pick("1").unwrap(),
            Selection::Display {
                index: 1,
                orientation: Orientation::Portrait,
                ..
            }
        ));
        assert!(matches!(
            pick("color").unwrap(),
            Selection::Display { index: 0, .. }
        ));
        assert!(matches!(
            pick("nope"),
            Err(LayoutError::NoSuchDisplay { .. })
        ));
        assert_eq!(pick("windowed").unwrap(), Selection::Windowed);
    }

    #[test]
    fn fit_letterboxes_non_native_modes() {
        let f = fit_panel(Orientation::Landscape, (1920.0, 1080.0));
        assert!((f.scale - 0.75).abs() < 1e-9);
        assert!((f.offset.0).abs() < 1e-9 && (f.offset.1 - 270.0).abs() < 1e-9);
        let f = fit_panel(Orientation::Landscape, (1280.0, 360.0));
        assert!((f.scale - 0.5).abs() < 1e-9);
    }

    #[test]
    fn display_choice_round_trips() {
        for s in ["auto", "windowed", "Monitor #60672", "2"] {
            assert_eq!(s.parse::<DisplayChoice>().unwrap().to_string(), s);
        }
    }
}
