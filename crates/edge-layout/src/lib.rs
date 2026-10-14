//! XENEON EDGE panel geometry.
//!
//! The grid is derived from Corsair's published widget slot sizes (see `docs/SPEC.md` §5.3):
//!
//! | Orientation | Panel    | Cells | Cell    | Gutter | Margin |
//! |-------------|----------|-------|---------|--------|--------|
//! | Landscape   | 2560×720 | 3×2   | 840×344 | 8      | 12     |
//! | Portrait    | 720×2560 | 1×6   | 696×416 | 8      | 12     |
//!
//! Every published slot size (S/M/L/XL in both orientations) is reproduced exactly by
//! [`slot_rect`]. [`select_display`] chooses the display the panel is drawn on.

mod display;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub use display::{
    DisplayChoice, DisplayInfo, EDGE_EDID_MODEL, EDGE_EDID_VENDOR, Fit, Selection, fit_panel,
    select_display,
};

/// Space between adjacent cells, in panel pixels.
pub const GUTTER: u32 = 8;
/// Space between the outermost cells and the panel edge, in panel pixels.
pub const MARGIN: u32 = 12;

/// Panel orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    /// 2560×720, three columns by two rows.
    #[default]
    Landscape,
    /// 720×2560, one column by six rows.
    Portrait,
}

impl Orientation {
    /// Native panel size `(width, height)` in pixels.
    #[must_use]
    pub const fn panel_size(self) -> (u32, u32) {
        match self {
            Self::Landscape => (2560, 720),
            Self::Portrait => (720, 2560),
        }
    }

    /// Grid dimensions `(columns, rows)`.
    #[must_use]
    pub const fn grid(self) -> (u32, u32) {
        match self {
            Self::Landscape => (3, 2),
            Self::Portrait => (1, 6),
        }
    }

    /// Size of one grid cell `(width, height)` in pixels.
    #[must_use]
    pub const fn cell_size(self) -> (u32, u32) {
        match self {
            Self::Landscape => (840, 344),
            Self::Portrait => (696, 416),
        }
    }
}

impl FromStr for Orientation {
    type Err = LayoutError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "landscape" | "horizontal" => Ok(Self::Landscape),
            "portrait" | "vertical" => Ok(Self::Portrait),
            _ => Err(LayoutError::UnknownOrientation(s.to_owned())),
        }
    }
}

impl fmt::Display for Orientation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Landscape => "landscape",
            Self::Portrait => "portrait",
        })
    }
}

/// iCUE widget slot size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Slot {
    /// Small: one cell.
    S,
    /// Medium: two cells.
    M,
    /// Large: four cells.
    L,
    /// Extra large: the whole panel.
    XL,
}

impl Slot {
    /// Cells spanned `(columns, rows)` in the given orientation.
    #[must_use]
    pub const fn span(self, orientation: Orientation) -> (u32, u32) {
        match (orientation, self) {
            (Orientation::Landscape | Orientation::Portrait, Self::S) => (1, 1),
            (Orientation::Landscape | Orientation::Portrait, Self::M) => (1, 2),
            (Orientation::Landscape, Self::L) => (2, 2),
            (Orientation::Landscape, Self::XL) => (3, 2),
            (Orientation::Portrait, Self::L) => (1, 4),
            (Orientation::Portrait, Self::XL) => (1, 6),
        }
    }
}

impl FromStr for Slot {
    type Err = LayoutError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "S" | "SMALL" => Ok(Self::S),
            "M" | "MEDIUM" => Ok(Self::M),
            "L" | "LARGE" => Ok(Self::L),
            "XL" | "EXTRA-LARGE" | "EXTRALARGE" => Ok(Self::XL),
            _ => Err(LayoutError::UnknownSlot(s.to_owned())),
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::S => "S",
            Self::M => "M",
            Self::L => "L",
            Self::XL => "XL",
        })
    }
}

/// A grid cell coordinate, zero-based from the top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Cell {
    /// Column index.
    pub col: u32,
    /// Row index.
    pub row: u32,
}

/// A rectangle in panel pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PxRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Errors from layout calculations and parsing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    /// The slot placed at `origin` would extend past the grid.
    #[error("slot {slot} at column {col}, row {row} does not fit the {orientation} grid", col = origin.col, row = origin.row)]
    OutOfBounds {
        /// Slot being placed.
        slot: Slot,
        /// Requested top-left cell.
        origin: Cell,
        /// Grid orientation.
        orientation: Orientation,
    },
    /// Unrecognised slot name.
    #[error("unknown slot size {0:?} (expected S, M, L or XL)")]
    UnknownSlot(String),
    /// Unrecognised orientation name.
    #[error("unknown orientation {0:?} (expected landscape or portrait)")]
    UnknownOrientation(String),
    /// A display pick matched no connected display.
    #[error("no display matches {query:?}; connected: {}", available.join(", "))]
    NoSuchDisplay {
        /// The index or name that was requested.
        query: String,
        /// Names of the connected displays.
        available: Vec<String>,
    },
}

impl Slot {
    /// Every slot, smallest first.
    pub const ALL: [Self; 4] = [Self::S, Self::M, Self::L, Self::XL];

    /// The cells this slot covers when placed at `origin`, or `None` if it does not fit.
    #[must_use]
    pub fn cells(self, orientation: Orientation, origin: Cell) -> Option<Vec<Cell>> {
        slot_rect(orientation, self, origin).ok()?;
        let (span_c, span_r) = self.span(orientation);
        Some(
            (origin.row..origin.row + span_r)
                .flat_map(|row| (origin.col..origin.col + span_c).map(move |col| Cell { col, row }))
                .collect(),
        )
    }
}

impl Orientation {
    /// Every cell of the grid in reading order.
    pub fn all_cells(self) -> impl Iterator<Item = Cell> {
        let (cols, rows) = self.grid();
        (0..rows).flat_map(move |row| (0..cols).map(move |col| Cell { col, row }))
    }
}

/// Pixel rectangle of `slot` placed with its top-left corner at `origin`.
///
/// ```
/// use edge_layout::{slot_rect, Cell, Orientation, PxRect, Slot};
/// let r = slot_rect(Orientation::Landscape, Slot::L, Cell::default()).unwrap();
/// assert_eq!(r, PxRect { x: 12, y: 12, width: 1688, height: 696 });
/// ```
///
/// # Errors
///
/// [`LayoutError::OutOfBounds`] if the slot does not fit inside the grid at `origin`.
pub fn slot_rect(
    orientation: Orientation,
    slot: Slot,
    origin: Cell,
) -> Result<PxRect, LayoutError> {
    let (cols, rows) = orientation.grid();
    let (span_c, span_r) = slot.span(orientation);
    let fits = origin
        .col
        .checked_add(span_c)
        .is_some_and(|end| end <= cols)
        && origin
            .row
            .checked_add(span_r)
            .is_some_and(|end| end <= rows);
    if !fits {
        return Err(LayoutError::OutOfBounds {
            slot,
            origin,
            orientation,
        });
    }
    // All operands are bounded by the grid constants above, so this arithmetic cannot overflow.
    let (cell_w, cell_h) = orientation.cell_size();
    Ok(PxRect {
        x: MARGIN + origin.col * (cell_w + GUTTER),
        y: MARGIN + origin.row * (cell_h + GUTTER),
        width: span_c * cell_w + (span_c - 1) * GUTTER,
        height: span_r * cell_h + (span_r - 1) * GUTTER,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published slot sizes (Corsair Labs icue-widget-builder, CORSAIR blog).
    const PUBLISHED: [(Orientation, Slot, u32, u32); 8] = [
        (Orientation::Landscape, Slot::S, 840, 344),
        (Orientation::Landscape, Slot::M, 840, 696),
        (Orientation::Landscape, Slot::L, 1688, 696),
        (Orientation::Landscape, Slot::XL, 2536, 696),
        (Orientation::Portrait, Slot::S, 696, 416),
        (Orientation::Portrait, Slot::M, 696, 840),
        (Orientation::Portrait, Slot::L, 696, 1688),
        (Orientation::Portrait, Slot::XL, 696, 2536),
    ];

    #[test]
    fn slot_sizes_match_published_values() {
        for (o, s, w, h) in PUBLISHED {
            let r = slot_rect(o, s, Cell::default()).unwrap();
            assert_eq!((r.width, r.height), (w, h), "{o} {s}");
        }
    }

    #[test]
    fn grid_fills_panel_exactly() {
        for o in [Orientation::Landscape, Orientation::Portrait] {
            let (cols, rows) = o.grid();
            let (cw, ch) = o.cell_size();
            let (pw, ph) = o.panel_size();
            assert_eq!(
                2 * MARGIN + cols * cw + (cols - 1) * GUTTER,
                pw,
                "{o} width"
            );
            assert_eq!(
                2 * MARGIN + rows * ch + (rows - 1) * GUTTER,
                ph,
                "{o} height"
            );
        }
    }

    #[test]
    fn offsets_and_bounds() {
        let r = slot_rect(Orientation::Landscape, Slot::S, Cell { col: 2, row: 1 }).unwrap();
        assert_eq!(
            r,
            PxRect {
                x: 12 + 2 * 848,
                y: 12 + 352,
                width: 840,
                height: 344
            }
        );
        let err = slot_rect(Orientation::Landscape, Slot::L, Cell { col: 2, row: 0 }).unwrap_err();
        assert!(matches!(err, LayoutError::OutOfBounds { .. }));
        assert!(slot_rect(Orientation::Portrait, Slot::XL, Cell { col: 0, row: 1 }).is_err());
        assert!(
            slot_rect(
                Orientation::Landscape,
                Slot::S,
                Cell {
                    col: u32::MAX,
                    row: 0
                }
            )
            .is_err()
        );
    }

    #[test]
    fn parse_slot_and_orientation() {
        assert_eq!("xl".parse::<Slot>().unwrap(), Slot::XL);
        assert_eq!("Large".parse::<Slot>().unwrap(), Slot::L);
        assert!("XXL".parse::<Slot>().is_err());
        assert_eq!(
            "vertical".parse::<Orientation>().unwrap(),
            Orientation::Portrait
        );
    }
}
