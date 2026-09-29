//! Shell side of the overview: camera animation, input, and drawing.
//!
//! The geometry is pure and lives in `browser-layout` (`stack_geometries`,
//! `hit_test`). This module only animates the [`Camera`] toward its target,
//! turns keys / wheel / clicks into requests while the overview is open, and
//! draws the workspace backdrops. Pages are not resized by the overview: the
//! Wayland path scales each page's subsurface (viewport destination), the
//! fallback path scales its `RenderImage` through the frame div.

use crate::{hex, Shell};
use browser_layout::{
    ease_step, hit_test, hit_test_workspace, stack_geometries, workspace_rects, Camera, OverviewSpec,
    StackedPage,
};
use browser_runtime::Request;
use gpui::{div, prelude::*, px, Context, MouseButton, MouseDownEvent, Pixels, Point, ScrollWheelEvent, Window};

/// The camera snaps to its target once this close (workspace index units, or
/// overview progress).
const CAMERA_EPSILON: f32 = 0.002;

/// Wheel distance that moves the overview by one workspace: one notch.
const WHEEL_STEP_PX: f32 = 120.0;

/// What a key does while the overview is open, or None to fall through to the
/// configured bindings. h/j/k/l and the arrows move focus like niri's
/// overview; enter and escape leave it.
pub fn key_request(binding: &str) -> Option<Request> {
    Some(match binding {
        "h" | "left" => Request::FocusLeft,
        "l" | "right" => Request::FocusRight,
        "k" | "up" => Request::FocusUp,
        "j" | "down" => Request::FocusDown,
        "enter" | "escape" => Request::OverviewToggle,
        _ => return None,
    })
}

/// Whole workspaces a wheel of `dy` px (positive = scroll down) moves; the
/// remainder carries so touchpad deltas add up instead of being dropped.
pub fn wheel_steps(carry: &mut f32, dy: f32) -> i32 {
    *carry += dy;
    let steps = (*carry / WHEEL_STEP_PX).trunc();
    *carry -= steps * WHEEL_STEP_PX;
    steps as i32
}

impl Shell {
    pub(crate) fn overview_spec(&self) -> OverviewSpec {
        OverviewSpec {
            scale: self.config.behavior.overview_scale,
            gap: self.config.behavior.overview_gap,
        }
    }

    /// Every page's on-screen geometry at the shell's current scroll and
    /// camera. One pass feeds hit-testing, the element tree and the agent API.
    pub(crate) fn page_geos(&self) -> Vec<StackedPage> {
        stack_geometries(
            &self.state.strip,
            &self.viewport,
            self.state.scroll,
            &self.camera,
            &self.overview_spec(),
        )
    }

    /// Where the camera is heading: the active workspace, zoomed out or not.
    fn camera_target(&self) -> Camera {
        let strip = &self.state.strip;
        Camera {
            ws_pos: strip.workspace_index(strip.active_workspace).unwrap_or(0) as f32,
            overview: if self.state.overview_open { 1.0 } else { 0.0 },
        }
    }

    /// True while the workspace slide or the overview zoom is still moving.
    pub(crate) fn camera_moving(&self) -> bool {
        self.camera != self.camera_target()
    }

    /// One easing step toward the target with the same curve as the strip
    /// scroll. Returns true when the camera moved.
    pub(crate) fn step_camera(&mut self) -> bool {
        let target = self.camera_target();
        let frac = self.config.behavior.smooth_scroll;
        let last = self.state.strip.workspaces.len().saturating_sub(1) as f32;
        let next = Camera {
            ws_pos: ease_step(self.camera.ws_pos.min(last), target.ws_pos, frac, CAMERA_EPSILON),
            overview: ease_step(self.camera.overview, target.overview, frac, CAMERA_EPSILON),
        };
        let moved = next != self.camera;
        self.camera = next;
        moved
    }

    /// Leave the overview if it is open.
    pub(crate) fn close_overview(&mut self, cx: &mut Context<Self>) {
        if self.state.overview_open {
            self.dispatch(Request::OverviewToggle, cx);
        }
    }

    /// Window position to viewport-inner coordinates.
    fn inner_point(&self, pos: Point<Pixels>) -> (f32, f32) {
        (f32::from(pos.x), f32::from(pos.y) - self.chrome_top())
    }

    /// Click in the overview: a page takes focus, an empty workspace becomes
    /// the active one, and either way the overview closes.
    fn overview_click(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let (x, y) = self.inner_point(ev.position);
        if let Some(id) = hit_test(&self.page_geos(), x, y) {
            self.focus_page(id, cx);
            self.close_overview(cx);
            return;
        }
        let rects = workspace_rects(&self.state.strip, &self.viewport, &self.camera, &self.overview_spec());
        if let Some(ws) = hit_test_workspace(&rects, x, y) {
            if let Some(index) = self.state.strip.workspace_index(ws) {
                self.dispatch(Request::WorkspaceFocus(index as u32 + 1), cx);
                self.close_overview(cx);
            }
        }
    }

    /// The wheel moves between workspaces while the overview is open.
    fn overview_scroll(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        // GPUI's y is positive for wheel-up; steps are positive for down.
        let dy = -f32::from(ev.delta.pixel_delta(px(crate::WHEEL_LINE_PX)).y);
        let steps = wheel_steps(&mut self.overview_wheel, dy);
        let req = if steps > 0 { Request::FocusDown } else { Request::FocusUp };
        for _ in 0..steps.abs() {
            self.dispatch(req.clone(), cx);
        }
    }

    /// The overview's drawing: a panel behind every workspace with its number,
    /// and a ring around each page (accent for the focused one). Sits under the
    /// pages; fades in and out with the camera.
    pub(crate) fn render_overview_backdrop(&self, geos: &[StackedPage]) -> Option<gpui::Div> {
        let t = self.camera.overview;
        if t < 0.001 {
            return None;
        }
        let (bar, bar_text) = (hex(&self.config.theme.bar), hex(&self.config.theme.bar_text));
        let (border, accent) = (hex(&self.config.theme.border), hex(&self.config.theme.accent));
        let strip = &self.state.strip;
        let mut layer = div()
            .absolute()
            .left(px(0.0))
            .top(px(self.chrome_top()))
            .w(px(self.viewport.width))
            .h(px(self.viewport.height))
            .overflow_hidden()
            .opacity(t);
        for (ws, r) in workspace_rects(strip, &self.viewport, &self.camera, &self.overview_spec()) {
            let number = strip.workspace_index(ws).map_or(0, |i| i + 1);
            let is_active = ws == strip.active_workspace;
            layer = layer
                .child(
                    div()
                        .absolute()
                        .left(px(r.x))
                        .top(px(r.y))
                        .w(px(r.w))
                        .h(px(r.h))
                        .rounded_md()
                        .bg(bar)
                        .border_1()
                        .border_color(if is_active { accent } else { border }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(r.x))
                        .top(px(r.y - 16.0))
                        .text_size(px(11.0))
                        .text_color(if is_active { accent } else { bar_text })
                        .child(number.to_string()),
                );
        }
        for sp in geos {
            let focused = strip.active_page == Some(sp.id);
            let grow = if focused { 3.0 } else { 1.0 };
            let g = sp.geom;
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(g.rel_x - grow))
                    .top(px(g.top - grow))
                    .w(px(g.width + 2.0 * grow))
                    .h(px(g.height + 2.0 * grow))
                    .rounded_sm()
                    .bg(if focused { accent } else { border }),
            );
        }
        Some(layer)
    }

    /// Full-viewport input catcher while the overview is open. It sits above
    /// the pages so they get no clicks, hover or wheel; the bars stay above it.
    pub(crate) fn render_overview_input(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .absolute()
            .left(px(0.0))
            .top(px(self.chrome_top()))
            .w(px(self.viewport.width))
            .h(px(self.viewport.height))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this: &mut Shell, ev: &MouseDownEvent, _: &mut Window, cx| {
                    this.overview_click(ev, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(
                |this: &mut Shell, ev: &ScrollWheelEvent, _: &mut Window, cx| {
                    this.overview_scroll(ev, cx);
                },
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_keys_map_to_focus_requests() {
        assert_eq!(key_request("h"), Some(Request::FocusLeft));
        assert_eq!(key_request("left"), Some(Request::FocusLeft));
        assert_eq!(key_request("l"), Some(Request::FocusRight));
        assert_eq!(key_request("right"), Some(Request::FocusRight));
        assert_eq!(key_request("j"), Some(Request::FocusDown));
        assert_eq!(key_request("down"), Some(Request::FocusDown));
        assert_eq!(key_request("k"), Some(Request::FocusUp));
        assert_eq!(key_request("up"), Some(Request::FocusUp));
        assert_eq!(key_request("enter"), Some(Request::OverviewToggle));
        assert_eq!(key_request("escape"), Some(Request::OverviewToggle));
        assert_eq!(key_request("ctrl+o"), None, "configured bindings still apply");
        assert_eq!(key_request("a"), None);
    }

    #[test]
    fn wheel_moves_one_workspace_per_notch_and_carries_fractions() {
        let mut carry = 0.0;
        assert_eq!(wheel_steps(&mut carry, 120.0), 1);
        assert_eq!(wheel_steps(&mut carry, -120.0), -1);
        let total: i32 = (0..10).map(|_| wheel_steps(&mut carry, 30.0)).sum();
        assert_eq!(total, 2, "300 px of touchpad is two workspaces, 60 px carried");
        assert_eq!(wheel_steps(&mut carry, 60.0), 1);
    }
}
