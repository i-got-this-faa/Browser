//! Pure strip-layout math shared by UI, scripting, and tests.
//!
//! Niri-inspired model: pages are first-class surfaces on an infinite
//! horizontal strip. Opening a page never resizes another page.

use browser_core::{PageId, Strip, WorkspaceId};

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

/// One easing step: move `frac` of the remaining distance toward the target,
/// snapping once within `epsilon`. Every animation in the shell (horizontal
/// scroll, workspace slide, overview zoom) uses this one curve.
pub fn ease_step(current: f32, target: f32, frac: f32, epsilon: f32) -> f32 {
    let delta = target - current;
    if delta.abs() < epsilon {
        target
    } else {
        current + delta * frac.clamp(0.05, 1.0)
    }
}

/// Smooth-scroll step in pixels.
pub fn scroll_step(current: ScrollOffset, target: ScrollOffset, frac: f32) -> ScrollOffset {
    ease_step(current, target, frac, 0.5)
}

/// Smallest page width, as a share of the viewport.
const MIN_WIDTH_FRACTION: f32 = 0.1;
/// Width change of one `page.width_increase` / `page.width_decrease`.
pub const WIDTH_STEP_FRACTION: f32 = 0.1;

/// Pixel width of a page that takes `fraction` of the viewport, counting the
/// gaps between pages so that 1/2 + 1/2 (or 1/3 x 3) fill the viewport exactly.
pub fn width_for_fraction(fraction: f32, viewport_width: f32, gap: f32) -> f32 {
    (fraction * (viewport_width + gap) - gap).max(1.0)
}

/// Inverse of [`width_for_fraction`].
pub fn fraction_of_width(width: f32, viewport_width: f32, gap: f32) -> f32 {
    (width + gap) / (viewport_width + gap)
}

/// The preset after the current width: the first preset wider than the page,
/// wrapping to the first one. `presets` must be ascending.
pub fn next_preset_width(current: f32, presets: &[f32], viewport_width: f32, gap: f32) -> Option<f32> {
    let now = fraction_of_width(current, viewport_width, gap);
    let next = presets.iter().copied().find(|p| *p > now + 0.005).or(presets.first().copied())?;
    Some(width_for_fraction(next, viewport_width, gap))
}

/// `current` plus (or minus) one width step of the viewport, kept between the
/// minimum width and the full viewport.
pub fn stepped_width(current: f32, grow: bool, viewport_width: f32) -> f32 {
    let step = viewport_width * WIDTH_STEP_FRACTION;
    let width = if grow { current + step } else { current - step };
    width.clamp(viewport_width * MIN_WIDTH_FRACTION, viewport_width)
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

    /// True when any part of the page lies inside the viewport.
    pub fn on_screen(&self, vp: &Viewport) -> bool {
        self.rel_x < vp.width
            && self.rel_x + self.width > 0.0
            && self.top < vp.height
            && self.top + self.height > 0.0
    }
}

/// A rectangle in viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// A page rectangle cut down to the area it may draw in, plus the part of the
/// page that survives as 0..1 shares of its width and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clipped {
    pub rect: Rect,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

/// Cut `rect` to `bounds`; None when nothing (at least one pixel) is left.
/// The Wayland path needs this because a page is a subsurface above the whole
/// window: unclipped, the overview's pages would draw over the bars.
pub fn clip(rect: Rect, bounds: Rect) -> Option<Clipped> {
    let (x0, y0) = (rect.x.max(bounds.x), rect.y.max(bounds.y));
    let (x1, y1) = ((rect.x + rect.w).min(bounds.x + bounds.w), (rect.y + rect.h).min(bounds.y + bounds.h));
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    Some(Clipped {
        rect: Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 },
        u0: (x0 - rect.x) / rect.w,
        v0: (y0 - rect.y) / rect.h,
        u1: (x1 - rect.x) / rect.w,
        v1: (y1 - rect.y) / rect.h,
    })
}

impl PageGeometry {
    pub fn rect(&self) -> Rect {
        Rect { x: self.rel_x, y: self.top, w: self.width, h: self.height }
    }
}

/// Where the camera looks at the vertical stack of workspaces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Fractional workspace index at the viewport center (0.0 = top
    /// workspace). Animated toward the active workspace's index.
    pub ws_pos: f32,
    /// 0.0 = normal view, 1.0 = fully zoomed out overview. Animated.
    pub overview: f32,
}

/// Overview look: zoom of the whole desktop and the space between workspaces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverviewSpec {
    /// Zoom at full overview (0.5 = everything at half size).
    pub scale: f32,
    /// Space between stacked workspaces, in unzoomed px.
    pub gap: f32,
}

/// One page's on-screen geometry plus the workspace it lives on.
#[derive(Debug, Clone, Copy)]
pub struct StackedPage {
    pub id: PageId,
    pub workspace: WorkspaceId,
    pub geom: PageGeometry,
}

/// Zoom and workspace spacing of the camera right now.
fn zoom_and_gap(cam: &Camera, spec: &OverviewSpec) -> (f32, f32) {
    let t = cam.overview.clamp(0.0, 1.0);
    (1.0 + (spec.scale.clamp(0.1, 1.0) - 1.0) * t, spec.gap.max(0.0) * t)
}

/// Screen top of workspace `index`. The workspace under the camera sits
/// centered vertically; the others stack above and below it.
fn workspace_top(index: usize, vp: &Viewport, cam: &Camera, zoom: f32, gap: f32) -> f32 {
    let stride = vp.height + gap;
    let cy = vp.height / 2.0;
    cy + (index as f32 * stride - cam.ws_pos * stride - cy) * zoom
}

/// Geometry of every page on every workspace: a pure function of the strip
/// and camera. `active_scroll` is the horizontal scroll of the active
/// workspace; the others draw at the scroll they were left at. With the
/// camera on the active workspace and no overview this is the plain strip
/// view: its pages at full size, all other workspaces off-screen.
pub fn stack_geometries(
    strip: &Strip,
    vp: &Viewport,
    active_scroll: ScrollOffset,
    cam: &Camera,
    spec: &OverviewSpec,
) -> Vec<StackedPage> {
    let (zoom, gap) = zoom_and_gap(cam, spec);
    let cx = vp.width / 2.0;
    let mut out = Vec::with_capacity(strip.pages.len());
    for (i, ws) in strip.workspaces.iter().enumerate() {
        let scroll = if ws.id == strip.active_workspace { active_scroll } else { ws.scroll };
        let top = workspace_top(i, vp, cam, zoom, gap);
        for p in strip.workspace_pages(ws.id) {
            let center_dist = ((p.x + p.width / 2.0) - (scroll + vp.width / 2.0)) / vp.width;
            out.push(StackedPage {
                id: p.id,
                workspace: ws.id,
                geom: PageGeometry {
                    rel_x: cx + (p.x - scroll - cx) * zoom,
                    top,
                    width: p.width * zoom,
                    height: vp.height * zoom,
                    center_dist_vp: center_dist,
                },
            });
        }
    }
    out
}

/// The screen area each workspace covers (for drawing backdrops and for
/// clicking an empty workspace).
pub fn workspace_rects(
    strip: &Strip,
    vp: &Viewport,
    cam: &Camera,
    spec: &OverviewSpec,
) -> Vec<(WorkspaceId, Rect)> {
    let (zoom, gap) = zoom_and_gap(cam, spec);
    let w = vp.width * zoom;
    strip
        .workspaces
        .iter()
        .enumerate()
        .map(|(i, ws)| {
            let rect = Rect { x: (vp.width - w) / 2.0, y: workspace_top(i, vp, cam, zoom, gap), w, h: vp.height * zoom };
            (ws.id, rect)
        })
        .collect()
}

/// The page under a point in viewport coordinates (later pages win).
pub fn hit_test(pages: &[StackedPage], x: f32, y: f32) -> Option<PageId> {
    pages
        .iter()
        .rev()
        .find(|p| p.geom.rect().contains(x, y))
        .map(|p| p.id)
}

/// The workspace under a point in viewport coordinates.
pub fn hit_test_workspace(rects: &[(WorkspaceId, Rect)], x: f32, y: f32) -> Option<WorkspaceId> {
    rects.iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser_core::Page;

    const VP: Viewport = Viewport { width: 1000.0, height: 800.0 };
    const SPEC: OverviewSpec = OverviewSpec { scale: 0.5, gap: 40.0 };
    const NORMAL: Camera = Camera { ws_pos: 0.0, overview: 0.0 };

    fn strip3() -> Strip {
        let mut s = Strip::new(12.0, 0.78);
        for i in 0..3 {
            let id = s.alloc_id();
            s.pages.push(Page::new(id, 1, "", i as f32 * 800.0 + i as f32 * 12.0, 800.0));
        }
        s.active_page = Some(s.pages[1].id);
        s
    }

    /// Three workspaces: two pages, one page, and the empty one at the bottom.
    fn stacked() -> Strip {
        let mut s = Strip::new(10.0, 0.5);
        s.create_workspace("second");
        for (ws, x) in [(1, 0.0), (1, 510.0), (2, 0.0)] {
            let id = s.alloc_id();
            s.pages.push(Page::new(id, ws, "", x, 500.0));
        }
        s.normalize_workspaces();
        s.active_page = Some(1);
        s
    }

    fn geom(s: &Strip, scroll: f32, cam: &Camera) -> Vec<StackedPage> {
        stack_geometries(s, &VP, scroll, cam, &SPEC)
    }

    fn of(pages: &[StackedPage], id: PageId) -> PageGeometry {
        pages.iter().find(|p| p.id == id).unwrap().geom
    }

    #[test]
    fn scroll_centers_active_page() {
        let s = strip3();
        let scroll = scroll_to_active(&s, &VP);
        let active = s.active().unwrap();
        let center = active.x + active.width / 2.0;
        assert!((scroll + VP.width / 2.0 - center).abs() < f32::EPSILON);
    }

    #[test]
    fn focus_factor_peaks_at_center() {
        let s = strip3();
        let scroll = scroll_to_active(&s, &VP);
        let geos = geom(&s, scroll, &NORMAL);
        let active = s.active().unwrap();
        let g = of(&geos, active.id);
        assert!((g.focus_factor() - 1.0).abs() < 0.01);
        assert!((g.rel_x - (VP.width - active.width) / 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn scroll_step_converges() {
        let mut cur = 0.0;
        for _ in 0..100 {
            cur = scroll_step(cur, 500.0, 0.18);
        }
        assert!((cur - 500.0).abs() < f32::EPSILON);
    }

    #[test]
    fn page_width_respects_fraction() {
        assert!((VP.page_width(0.78) - 780.0).abs() < f32::EPSILON);
        assert!(VP.page_width(0.1) >= 1.0);
    }

    #[test]
    fn normal_view_shows_the_active_workspace_at_full_size() {
        let s = stacked();
        let geos = geom(&s, 0.0, &NORMAL);
        let g = of(&geos, 1);
        assert_eq!((g.rel_x, g.top, g.width, g.height), (0.0, 0.0, 500.0, 800.0));
        assert!(g.on_screen(&VP));
        let below = of(&geos, 3);
        assert_eq!(below.top, VP.height, "next workspace starts right under the viewport");
        assert!(!below.on_screen(&VP));
    }

    #[test]
    fn camera_slides_between_workspaces() {
        let s = stacked();
        let halfway = geom(&s, 0.0, &Camera { ws_pos: 0.5, overview: 0.0 });
        assert_eq!(of(&halfway, 1).top, -400.0);
        assert_eq!(of(&halfway, 3).top, 400.0);
        assert!(of(&halfway, 1).on_screen(&VP) && of(&halfway, 3).on_screen(&VP));
        let arrived = geom(&s, 0.0, &Camera { ws_pos: 1.0, overview: 0.0 });
        assert_eq!(of(&arrived, 3).top, 0.0);
        assert_eq!(of(&arrived, 1).top, -VP.height);
    }

    #[test]
    fn overview_zooms_out_around_the_center_and_keeps_real_relative_layout() {
        let s = stacked();
        let cam = Camera { ws_pos: 0.0, overview: 1.0 };
        let geos = geom(&s, 0.0, &cam);
        let (a, b) = (of(&geos, 1), of(&geos, 2));
        assert_eq!((a.width, a.height), (250.0, 400.0), "half size");
        // Relative widths and spacing are the real ones, scaled.
        assert_eq!(b.rel_x - a.rel_x, 510.0 * 0.5);
        assert_eq!(a.rel_x, 500.0 + (0.0 - 500.0) * 0.5, "zoom is around the viewport center");
        // The active workspace is vertically centered.
        assert_eq!(a.top, (VP.height - a.height) / 2.0);
        // The next workspace is one workspace plus the gap further down.
        assert_eq!(of(&geos, 3).top - a.top, (VP.height + SPEC.gap) * 0.5);
    }

    #[test]
    fn overview_progress_interpolates_zoom() {
        let s = stacked();
        let half = geom(&s, 0.0, &Camera { ws_pos: 0.0, overview: 0.5 });
        assert_eq!(of(&half, 1).width, 375.0);
    }

    #[test]
    fn non_active_workspaces_draw_at_the_scroll_they_were_left_at() {
        let mut s = stacked();
        s.workspaces[1].scroll = 100.0;
        let geos = geom(&s, 0.0, &Camera { ws_pos: 1.0, overview: 0.0 });
        assert_eq!(of(&geos, 3).rel_x, -100.0);
        let geos = geom(&s, 250.0, &NORMAL);
        assert_eq!(of(&geos, 1).rel_x, -250.0, "the active workspace uses the live scroll");
    }

    #[test]
    fn hit_test_finds_the_page_under_a_click() {
        let s = stacked();
        let geos = geom(&s, 0.0, &Camera { ws_pos: 0.0, overview: 1.0 });
        let (a, b, c) = (of(&geos, 1), of(&geos, 2), of(&geos, 3));
        let mid = |g: PageGeometry| (g.rel_x + g.width / 2.0, g.top + g.height / 2.0);
        for (id, g) in [(1, a), (2, b), (3, c)] {
            let (x, y) = mid(g);
            assert_eq!(hit_test(&geos, x, y), Some(id));
        }
        // The gap between two columns and the space left of the strip miss.
        assert_eq!(hit_test(&geos, a.rel_x + a.width + 2.0, mid(a).1), None);
        assert_eq!(hit_test(&geos, 1.0, 1.0), None);
    }

    #[test]
    fn hit_test_workspace_covers_empty_workspaces_too() {
        let s = stacked();
        let cam = Camera { ws_pos: 0.0, overview: 1.0 };
        let rects = workspace_rects(&s, &VP, &cam, &SPEC);
        let empty = rects.iter().find(|(id, _)| *id == 3).unwrap().1;
        assert_eq!(hit_test_workspace(&rects, empty.x + 1.0, empty.y + 1.0), Some(3));
        assert_eq!(hit_test_workspace(&rects, 1.0, 1.0), None);
    }

    #[test]
    fn clip_keeps_the_visible_part_and_reports_which_part() {
        let bounds = Rect { x: 0.0, y: 28.0, w: 1000.0, h: 800.0 };
        let inside = Rect { x: 100.0, y: 100.0, w: 200.0, h: 100.0 };
        let c = clip(inside, bounds).unwrap();
        assert_eq!((c.rect, c.u0, c.v0, c.u1, c.v1), (inside, 0.0, 0.0, 1.0, 1.0));
        // Hanging over the top edge by a quarter of its height.
        let over_top = Rect { x: 0.0, y: -22.0, w: 200.0, h: 200.0 };
        let c = clip(over_top, Rect { y: 28.0, ..bounds }).unwrap();
        assert_eq!(c.rect, Rect { x: 0.0, y: 28.0, w: 200.0, h: 150.0 });
        assert_eq!((c.v0, c.v1), (0.25, 1.0));
        // Cut on both sides horizontally.
        let wide = Rect { x: -100.0, y: 100.0, w: 1200.0, h: 50.0 };
        let c = clip(wide, bounds).unwrap();
        assert_eq!((c.rect.x, c.rect.w), (0.0, 1000.0));
        assert!((c.u0 - 100.0 / 1200.0).abs() < 1e-6 && (c.u1 - 1100.0 / 1200.0).abs() < 1e-6);
        // Fully outside, or a sliver under one pixel, is gone.
        assert_eq!(clip(Rect { x: 0.0, y: 900.0, w: 10.0, h: 10.0 }, bounds), None);
        assert_eq!(clip(Rect { x: 999.5, y: 100.0, w: 100.0, h: 10.0 }, bounds), None);
    }

    #[test]
    fn width_fractions_fill_the_viewport_with_gaps() {
        let (vw, gap) = (1000.0, 12.0);
        let half = width_for_fraction(0.5, vw, gap);
        assert_eq!(half * 2.0 + gap, vw);
        let third = width_for_fraction(1.0 / 3.0, vw, gap);
        assert!((third * 3.0 + gap * 2.0 - vw).abs() < 0.01);
        assert_eq!(width_for_fraction(1.0, vw, gap), vw);
        assert!((fraction_of_width(half, vw, gap) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn preset_cycle_walks_up_and_wraps() {
        let presets = [1.0 / 3.0, 0.5, 2.0 / 3.0];
        let (vw, gap) = (1000.0, 12.0);
        let w = |f: f32| width_for_fraction(f, vw, gap);
        assert_eq!(next_preset_width(w(1.0 / 3.0), &presets, vw, gap), Some(w(0.5)));
        assert_eq!(next_preset_width(w(0.5), &presets, vw, gap), Some(w(2.0 / 3.0)));
        assert_eq!(next_preset_width(w(2.0 / 3.0), &presets, vw, gap), Some(w(1.0 / 3.0)));
        // Off-preset widths go to the next wider preset.
        assert_eq!(next_preset_width(w(0.4), &presets, vw, gap), Some(w(0.5)));
        assert_eq!(next_preset_width(w(0.78), &presets, vw, gap), Some(w(1.0 / 3.0)));
        assert_eq!(next_preset_width(500.0, &[], vw, gap), None);
    }

    #[test]
    fn width_steps_are_ten_percent_and_clamped() {
        assert_eq!(stepped_width(500.0, true, 1000.0), 600.0);
        assert_eq!(stepped_width(500.0, false, 1000.0), 400.0);
        assert_eq!(stepped_width(950.0, true, 1000.0), 1000.0);
        assert_eq!(stepped_width(150.0, false, 1000.0), 100.0);
    }
}
