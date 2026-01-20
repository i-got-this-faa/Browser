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

