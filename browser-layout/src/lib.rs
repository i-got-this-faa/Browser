//! Pure strip-layout math shared by UI, scripting, and tests.
//!
//! Niri-inspired model: pages are first-class surfaces on an infinite
//! horizontal strip. Opening a page never resizes another page.

use browser_core::{Page, PageId, Strip, DEFAULT_GAP, DEFAULT_PAGE_FRACTION};

/// Viewport geometry for one frame of layout.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    /// Width of one page slot given the configured fraction.
    pub fn page_width(&self, fraction: f32) -> f32 {
        (self.width * fraction).max(1.0)
    }
}

/// Where the strip is scrolled to: the x offset of the viewport's left edge.
pub type ScrollOffset = f32;

/// Center the viewport on a page's slot, clamped to the strip edges.
pub fn scroll_to_page(strip: &Strip, vp: &Viewport, id: PageId) -> ScrollOffset {
    let Some(p) = strip.page(id) else { return 0.0 };
    let center = p.x + p.width / 2.0;
    (center - vp.width / 2.0).max(0.0)
}

/// Center the viewport on the active page.
pub fn scroll_to_active(strip: &Strip, vp: &Viewport) -> ScrollOffset {
    match strip.active_page {
        Some(id) => scroll_to_page(strip, vp, id),
        None => 0.0,
    }
}

/// Smooth-scroll step: move `frac` of the remaining distance toward the target.
pub fn scroll_step(current: ScrollOffset, target: ScrollOffset, frac: f32) -> ScrollOffset {
    let delta = target - current;
    if delta.abs() < 0.5 {
        target
    } else {
        current + delta * frac.clamp(0.05, 1.0)
    }
}

/// Geometry of one page for a frame: on-screen rect plus edge distance.
#[derive(Debug, Clone, Copy)]
pub struct PageGeometry {
    /// Left edge relative to the viewport's left edge.
    pub rel_x: f32,
    /// Top edge relative to the viewport's top edge.
    pub top: f32,
    pub width: f32,
    pub height: f32,
    /// Signed distance from the viewport's horizontal center to the page's
    /// center, in viewports. 0.0 means centered.
    pub center_dist_vp: f32,
}

