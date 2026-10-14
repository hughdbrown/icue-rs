//! Minimal EDID parsing: the identity fields EdgeHost needs to recognise a display.
//!
//! macOS reports these via CoreGraphics; on Linux they come from `/sys/class/drm/*/edid`. Both
//! paths produce the same numbers: `vendor` is the big-endian manufacturer id (`CGDisplayVendorNumber`,
//! 0x0e58 = "CRX" for the XENEON EDGE) and `product` the little-endian product code
//! (`CGDisplayModelNumber`, 0xed00).

/// Identity of a display, decoded from its EDID base block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edid {
    /// Manufacturer id as a 16-bit number (big-endian in the EDID).
    pub vendor: u32,
    /// Product code (little-endian in the EDID).
    pub product: u32,
    /// Three-letter manufacturer code, e.g. `CRX`.
    pub manufacturer: String,
    /// Monitor name descriptor (tag 0xFC), e.g. `XENEON EDGE`.
    pub name: Option<String>,
    /// Preferred timing `(width, height)` from the first detailed timing descriptor.
    pub preferred: Option<(u32, u32)>,
}

const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

/// Parse the 128-byte EDID base block (extension blocks are ignored).
///
/// ```
/// use edge_layout::parse_edid;
/// assert!(parse_edid(&[0u8; 16]).is_none());
/// ```
#[must_use]
pub fn parse_edid(bytes: &[u8]) -> Option<Edid> {
    let base = bytes.get(..128)?;
    if base.get(..8)? != HEADER {
        return None;
    }
    let id = u16::from_be_bytes([*base.get(8)?, *base.get(9)?]);
    let product = u16::from_le_bytes([*base.get(10)?, *base.get(11)?]);
    let manufacturer = [10, 5, 0]
        .iter()
        .map(|shift| char::from(b'@' + u8::try_from((id >> shift) & 0x1f).unwrap_or(0)))
        .collect();
    let mut name = None;
    let mut preferred = None;
    for (i, d) in base.get(54..126)?.as_chunks::<18>().0.iter().enumerate() {
        let pixel_clock = u16::from_le_bytes([*d.first()?, *d.get(1)?]);
        if pixel_clock != 0 {
            if i == 0 {
                let w = u32::from(*d.get(2)?) | (u32::from(*d.get(4)? & 0xf0) << 4);
                let h = u32::from(*d.get(5)?) | (u32::from(*d.get(7)? & 0xf0) << 4);
                preferred = Some((w, h));
            }
        } else if *d.get(3)? == 0xfc {
            let text: String = d
                .get(5..18)?
                .iter()
                .take_while(|&&b| b != b'\n')
                .map(|&b| char::from(b))
                .collect();
            name = Some(text.trim().to_owned()).filter(|t| !t.is_empty());
        }
    }
    Some(Edid {
        vendor: u32::from(id),
        product: u32::from(product),
        manufacturer,
        name,
        preferred,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EDID read from a real XENEON EDGE (docs/research/edge-hardware.md), base block.
    const EDGE: &str = concat!(
        "00ffffffffffff000e5800ed010101011715010380240b78ea1ec5ae4f34b126",
        "0e505420090001010101010101010101010101010101e83500a6a0d078203020",
        "85006c711000001a000000fc0058454e454f4e20454447450a20000000ff0031",
        "313038323633343536393800000000fd00384c1e5122000a2020202020200178",
    );

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn parses_the_xeneon_edge() {
        let e = parse_edid(&bytes(EDGE)).unwrap();
        assert_eq!(e.vendor, crate::EDGE_EDID_VENDOR);
        assert_eq!(e.product, crate::EDGE_EDID_MODEL);
        assert_eq!(e.manufacturer, "CRX");
        assert_eq!(e.name.as_deref(), Some("XENEON EDGE"));
        assert_eq!(e.preferred, Some((2560, 720)));
    }

    #[test]
    fn rejects_bad_header_and_short_input() {
        let mut b = bytes(EDGE);
        assert!(parse_edid(b.get(..100).unwrap()).is_none());
        b[0] = 1;
        assert!(parse_edid(&b).is_none());
    }
}
