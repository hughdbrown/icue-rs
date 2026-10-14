//! Describing connected displays for [`edge_layout::select_display`].

use edge_layout::DisplayInfo;
use tao::event_loop::EventLoopWindowTarget;

use crate::platform;

/// Every connected display, in the order the windowing system reports them (the index used by
/// `edgectl displays` and `--display <index>`).
#[must_use]
pub fn list_displays<T: 'static>(target: &EventLoopWindowTarget<T>) -> Vec<DisplayInfo> {
    target
        .available_monitors()
        .map(|m| {
            let scale = m.scale_factor();
            let size = m.size();
            let origin = m.position().to_logical::<f64>(scale);
            let logical = size.to_logical::<f64>(scale);
            let (vendor, model) = platform::edid_ids(&m);
            DisplayInfo {
                name: m.name().unwrap_or_else(|| "(unnamed)".to_owned()),
                vendor,
                model,
                pixel_size: (size.width, size.height),
                scale_factor: scale,
                logical_origin: (origin.x, origin.y),
                logical_size: (logical.width, logical.height),
            }
        })
        .collect()
}
