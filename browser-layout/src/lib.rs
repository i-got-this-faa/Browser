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

impl PageGeometry {
    /// 1.0 when centered, falling off as the page leaves the viewport.
    pub fn focus_factor(&self) -> f32 {
        (-self.center_dist_vp.abs()).clamp(-2.0, 0.0).exp()
    }

    /// Visible portion of this page in the viewport.
    pub fn visible(&self) -> bool {
        self.rel_x + self.width > 0.0 && self.rel_x < 100000.0
    }
}

/// Compute per-page geometry for a frame. Pure function of state.
///
/// With `overview = Some((0.0, 1.0))`-style scale factors the whole strip is
/// zoomed out around the viewport center: niri's overview mode.
pub fn frame_geometries(
    strip: &Strip,
    vp: &Viewport,
    scroll: ScrollOffset,
    _fraction: f32,
) -> Vec<(PageId, PageGeometry)> {
    frame_geometries_scaled(strip, vp, scroll, _fraction, 1.0)
}

/// Like [`frame_geometries`] but with an explicit zoom scale (0 < scale <= 1).
/// Pages keep their relative strip positions; everything shrinks toward the
/// viewport center. `scale` 1.0 is the normal mode.
pub fn frame_geometries_scaled(
    strip: &Strip,
    vp: &Viewport,
    scroll: ScrollOffset,
    _fraction: f32,
    scale: f32,
) -> Vec<(PageId, PageGeometry)> {
    let scale = scale.clamp(0.05, 1.0);
    let cx = vp.width / 2.0;
    let cy = vp.height / 2.0;
    strip
        .visible()
        .into_iter()
        .map(|p: &Page| {
            let rel_x = (p.x - scroll - cx) * scale + cx;
            let width = p.width * scale;
            let height = vp.height * scale;
            let top = cy - height / 2.0;
            let center_dist = ((p.x + p.width / 2.0) - (scroll + vp.width / 2.0)) / vp.width;
            (
                p.id,
                PageGeometry {
                    rel_x,
                    width,
                    height,
                    center_dist_vp: center_dist,
                    top,
                },
            )
        })
        .collect()
}

/// Create the first page if the strip is empty. Returns the new page.
pub fn ensure_first_page(strip: &mut Strip, vp: &Viewport) -> Page {
    let width = vp.page_width(strip.page_fraction);
    let id = strip.alloc_id();
    let page = Page::new(id, strip.active_workspace, "", 0.0, width);
    strip.pages.push(page.clone());
    strip.active_page = Some(page.id);
    page
}

/// Default strip, factored for reuse.
pub fn default_strip() -> Strip {
    Strip::new(DEFAULT_GAP, DEFAULT_PAGE_FRACTION)
}

#[cfg(test)]
