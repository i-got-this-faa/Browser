//! Strip Browser: a programmable, Niri-inspired browser shell.
//!
//! Architecture (docs/ARCHITECTURE.md):
//!   browser.lua -> LuaHost (requests in, snapshot out)
//!   BrowserState + ops -> GPUI shell -> WebView trait -> Chromium engine
//!
//! The Chromium engine runs headless; this GPUI window is the browser.

mod control;
mod engine;
mod trace;

use anyhow::{Context as _, Result};
use browser_config::{config_path, ensure_default_config, watch_config, Config, WatcherHandle};
use browser_core::{perf_event, perf_span};
use std::collections::HashMap;
use browser_layout::{frame_geometries_scaled, scroll_step, Viewport};
use browser_runtime::{ops, BrowserState, LuaHost, Request};
use engine::EngineController;
use gpui::{
    div, img, prelude::*, px, relative, size, App, Application, Bounds, Context, FocusHandle,
    Focusable, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Pixels, Point, Render, RenderImage, ScrollWheelEvent, SharedString,
    Window, WindowBounds, WindowOptions,
};
use std::sync::Arc;

pub const DEFAULT_LUA: &str = include_str!("../assets/browser.lua");

fn main() -> Result<()> {
    // CEF re-executes this binary for renderer/GPU/utility subprocesses.
    // They must run CefExecuteProcess and exit here, before the shell, config
    // writer, or GPUI ever start (rc >= 0 => we are a child process).
// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

enum Overlay {
    None,
    /// Address/search prompt with the current editable text. `fresh` marks a
    /// just-prefilled address bar: the first edit replaces it, matching the
    /// select-all behavior of a real one.
    Prompt { text: String, fresh: bool },
    /// Command palette with filter text and a highlighted row (moved by
    /// arrows / scroll wheel).
// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// Page-bar height. The bars are *not* overlays: the layout viewport is the
/// window minus these reservations, so web content never renders underneath.
const PAGE_BAR_H: f32 = 28.0;
/// Status-bar height (bottom).
const STATUS_BAR_H: f32 = 24.0;
/// Rows the command palette renders; selection clamps to this window.
const PALETTE_VISIBLE: usize = 12;

struct Shell {
    engine: EngineController,
    state: BrowserState,
    config: Config,
    lua: Option<LuaHost>,
    lua_source: Arc<String>,
    overlay: Overlay,
    viewport: Viewport,
    focus: FocusHandle,
    reload_rx: std::sync::mpsc::Receiver<()>,
    /// Kept alive here: dropping the watcher unregisters the notify watch,
    /// which silently killed config hot-reload after startup.
    _watcher: Option<WatcherHandle>,
    /// Pending smooth-scroll target; None means settled.
    scroll_target: Option<f32>,
    /// Scriptable control socket (automation + headless E2E). None if bind
    /// failed. Arc-shared so the per-frame poll can clone the handle cheaply
    /// (a per-frame try_clone was a dup() syscall at 60Hz — see perf audit).
    control: Option<Arc<std::os::unix::net::UnixListener>>,
    /// Stable render surfaces per page: one BGRA buffer + one RenderImage
    /// each, re-uploaded only on damage or resize (no per-frame allocation).
    surfaces: HashMap<u64, Surface>,
    /// Last size sent to each engine view; avoids redundant resize calls.
    view_sizes: HashMap<u64, (u32, u32)>,
    /// Last hidden state pushed per page; avoids redundant SetHidden commands.
    focus_cache: HashMap<u64, bool>,
}

/// One page's render surface. `bgra` is the stable CPU-side frame (CEF writes
/// it through the shim buffer patch); `painted` is the GPUI texture derived
/// from it. The texture is only recreated when `version` advances (damage or
/// resize), and the atlas tile recycles via drop_image -> free_list.
impl Surface {
    fn new() -> Self {
        Self {
            bgra: Vec::new(),
            width: 0,
            height: 0,
            version: 0,
            painted: None,
            painted_version: 0,
        }
    }

    /// CEF raw-frame path: patch damage rows from the shim buffer (BGRA,
    /// zero per-pixel work). Empty rects == full frame. Size change is the
    /// only reallocation.
    fn patch_raw(&mut self, px: &[u8], w: i32, h: i32, rects: &[[i32; 4]]) {
        let (w, h) = (w as usize, h as usize);
        if w == 0 || h == 0 {
            return;
        }
        if self.width as usize != w || self.height as usize != h {
            self.width = w as u32;
            self.height = h as u32;
            self.bgra = vec![0u8; w * h * 4];
            self.version += 1;
        }
        let src_stride = w * 4;
        let full = rects.is_empty();
        let rects: &[[i32; 4]] = if full {
            &[[0, 0, w as i32, h as i32]]
        } else {
            rects
        };
        for r in rects {
            let x0 = r[0].clamp(0, w as i32) as usize;
            let y0 = r[1].clamp(0, h as i32) as usize;
            let x1 = (r[0] + r[2]).clamp(0, w as i32) as usize;
            let y1 = (r[1] + r[3]).clamp(0, h as i32) as usize;
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let rowbytes = (x1 - x0) * 4;
            for row in y0..y1 {
                let s = &px[row * src_stride + x0 * 4..row * src_stride + x0 * 4 + rowbytes];
                let d = &mut self.bgra[row * src_stride + x0 * 4..row * src_stride + x0 * 4 + rowbytes];
                d.copy_from_slice(s);
            }
        }
        self.version += 1;
    }

    /// Frozen CDP-harness path: decode a PNG frame into the stable buffer.
    /// Only reachable with STRIP_ENGINE=cdp; never on the CEF frame path.
