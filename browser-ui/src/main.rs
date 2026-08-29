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
    fn texture(&mut self, cx: &mut Context<Shell>) -> Option<Arc<RenderImage>> {
impl Focusable for Shell {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Shell {
    fn new(engine: EngineController, cx: &mut Context<Self>) -> Self {
        let (tx, reload_rx) = std::sync::mpsc::channel();
        // Watcher must outlive this constructor or the watch is unregistered
        // and browser.lua hot-reload dies (was a local: dropped on return).
        let watcher = watch_config(config_path(), tx).ok();

        let lua_source = std::fs::read_to_string(config_path()).unwrap_or_default();

        let mut shell = Self {
            engine,
            state: BrowserState::default(),
            config: Config::default(),
            lua: None,
            lua_source: Arc::new(lua_source),
            overlay: Overlay::None,
            viewport: Viewport { width: 1280.0, height: 800.0 },
            focus: cx.focus_handle(),
            reload_rx,
            _watcher: watcher,
            scroll_target: None,
            control: control::start(),
            surfaces: HashMap::new(),
            view_sizes: HashMap::new(),
            focus_cache: HashMap::new(),
        };
        shell.reload_lua(cx);
        shell.ensure_first_page();

        // Frame pump: a ~60Hz timer drives event draining, smooth scroll,
        // and toast lifetime. Each tick notifies, which re-renders.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(16))
                .await;
            if this.update(cx, |this, cx| this.frame(cx)).is_err() {
                break; // shell released: window closed
            }
        })
        .detach();

        shell
    }

    fn ensure_first_page(&mut self) {
        if self.state.strip.pages.is_empty() {
            let home = self.config.behavior.home_page.clone();
            let id = self.state.add_page(&home, &self.viewport);
            let mut fx = ops::Effects::default();
            fx.spawn.push((id, home));
            self.effects(&mut fx);
        }
    }

    // -- config / lua -------------------------------------------------------

    /// Re-parse Lua source, apply behavior, run load-time requests, and fire
    /// the config_reloaded hook. On error, show it and keep the old config.
    fn reload_lua(&mut self, cx: &mut Context<Self>) {
        let mut host = LuaHost::new();
        let mut load_requests = Vec::new();
        match host.load_config(&self.lua_source, &mut load_requests) {
            Ok(()) => {
                self.lua = Some(host);
                match Config::parse(&self.lua_source) {
                    Ok(cfg) => {
                        self.state
                            .apply_behavior(cfg.behavior.gap, cfg.behavior.page_width_fraction);
                        self.config = cfg;
                    }
                    Err(e) => self.toast(format!("config warning: {e}")),
                }
                for r in load_requests {
                    self.dispatch(r, cx);
                }
                self.fire_hook("config_reloaded", None, cx);
            }
            Err(e) => self.toast(format!("browser.lua error: {e}")),
        }
    }

    fn fire_hook(
        &mut self,
        event: &str,
        payload: Option<serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        if !self.lua.is_some() {
            return;
        }
        let snapshot = self.snapshot();
        let Some(host) = &mut self.lua else { return };
        if let Err(e) = host.push_snapshot(&snapshot) {
            eprintln!("snapshot push failed: {e}");
            return;
        }
        let payload_value = match &payload {
            Some(p) => host.json_to_lua(p),
            None => Ok(browser_runtime::LuaValue::Nil),
        };
        let Ok(payload_value) = payload_value else { return };
        match host.call_hook(event, payload_value) {
            Ok(reqs) => {
                for r in reqs {
                    self.dispatch(r, cx);
                }
            }
            Err(e) => self.toast(format!("{event} hook: {e}")),
        }
    }

    fn run_lua_command(&mut self, name: &str, arg: Option<String>, cx: &mut Context<Self>) {
        if !self.lua.is_some() {
            self.toast("no config loaded");
            return;
        }
        let snapshot = self.snapshot();
        let Some(host) = &mut self.lua else {
            self.toast("no config loaded");
            return;
        };
        if let Err(e) = host.push_snapshot(&snapshot) {
            self.toast(format!("snapshot: {e}"));
            return;
        }
        match host.call_command(name, arg.clone()) {
            Ok(reqs) => {
                for r in reqs {
                    self.dispatch(r, cx);
                }
            }
            Err(_) => {
                if Request::from_command(name, arg.as_deref()).is_none() {
                    self.toast(format!("unknown command: {name}"));
                }
            }
        }
    }

    fn snapshot(&self) -> browser_runtime::BrowserSnapshot {
        browser_runtime::BrowserSnapshot {
            tabs: self
                .state
                .strip
                .visible()
                .iter()
                .map(|p| browser_runtime::TabInfo {
                    id: p.id,
                    url: p.url.clone(),
                    title: p.title.clone(),
                    workspace: p.workspace,
                    active: self.state.strip.active_page == Some(p.id),
                })
                .collect(),
            active_workspace: self.state.strip.active_workspace,
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.overlay = Overlay::Toast { text: text.into(), ttl_frames: 240 };
    }

    // -- dispatch -----------------------------------------------------------

    /// Apply engine/UI side effects.
    fn effects(&mut self, fx: &mut ops::Effects) {
        for (id, url) in fx.spawn.drain(..) {
            if url.is_empty() {
                continue;
            }
            if let Err(e) = self.engine.spawn_page(id, &url) {
                self.toast(format!("engine: {e}"));
            }
        }
        for (id, url) in fx.navigate.drain(..) {
            if url.is_empty() {
                continue;
            }
            // Pages created empty have no webview yet; spawn one on demand.
            if !self.engine.has_view(id) {
                if let Err(e) = self.engine.spawn_page(id, &url) {
                    self.toast(format!("engine: {e}"));
                    continue;
                }
            }
            if let Err(e) = self.engine.navigate(id, &url) {
                self.toast(format!("engine: {e}"));
            }
        }
        for id in fx.hard_reload.drain(..) {
            let _ = self.engine.reload(id, true);
        }
        for id in fx.soft_reload.drain(..) {
            let _ = self.engine.reload(id, false);
        }
        for id in fx.close.drain(..) {
            self.engine.close_page(id);
            self.view_sizes.remove(&id);
            self.surfaces.remove(&id);
            self.focus_cache.remove(&id);
        }
        if let Some(text) = fx.toast.take() {
            self.toast(text);
        }
    }

    /// Central request path: ops mutate state, effects drive the engine.
    fn dispatch(&mut self, req: Request, cx: &mut Context<Self>) {
        let _s = perf_span!("dispatch");
        perf_event!("dispatch.req", "req" => req.name());
        // History moves are engine-side; the url event updates state after.
        if matches!(req, Request::Back | Request::Forward) {
            if let Some(id) = self.state.active_id() {
                let r = if req == Request::Back {
                    self.engine.go_back(id)
                } else {
                    self.engine.go_forward(id)
                };
                if let Err(e) = r {
                    self.toast(format!("history: {e}"));
                }
            }
            cx.notify();
            return;
        }

        let vp = self.viewport;
        let mut fx = ops::Effects::default();
        ops::apply(&mut self.state, &vp, req, &mut fx);

        if let Some(prefill) = fx.prompt_open.take() {
            self.overlay = Overlay::Prompt { text: prefill, fresh: true };
        }
        if fx.palette_open {
    // -- prompt -------------------------------------------------------------

    fn submit_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        self.overlay = Overlay::None;
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            cx.notify();
            return;
        }
        if let Some(cmd) = trimmed.strip_prefix(':') {
            self.run_typed_command(cmd, cx);
            cx.notify();
            return;
        }
        let url = if is_url(&trimmed) {
            if trimmed.contains("://") || trimmed.starts_with("about:") || trimmed.starts_with("data:")
            {
                trimmed.clone()
            } else {
                format!("https://{trimmed}")
            }
        } else {
            self.search_url(&trimmed)
        };
        self.dispatch(Request::Navigate(url), cx);
    }

    fn run_typed_command(&mut self, cmd: &str, cx: &mut Context<Self>) {
        let mut parts = cmd.splitn(2, ' ');
        let name = parts.next().unwrap_or("");
        let arg = parts.next().map(|s| s.to_string()).filter(|s| !s.is_empty());
        if let Some(req) = Request::from_command(name, arg.as_deref()) {
            self.dispatch(req, cx);
        } else {
            self.run_lua_command(name, arg, cx);
        }
    }

    fn search_url(&self, query: &str) -> String {
        self.config.behavior.search_engine_url.replacen("{}", query, 1)
    }

    // -- per-frame pump -----------------------------------------------------

    /// One frame: drain engine events, poll config watcher, smooth-scroll,
    /// then schedule the next frame. The window is only notified when
    /// something actually changed this frame — a 60Hz unconditional notify
    /// kept GPUI re-rendering forever (perf audit: 60 renders/s at idle).
    fn frame(&mut self, cx: &mut Context<Self>) {
        let _s = perf_span!("frame");
        let mut dirty = false;

        if self.drain_engine_events(cx) {
            dirty = true;
        }
        if self.poll_config_reload(cx) {
            dirty = true;
        }
        if let Some(listener) = self.control.clone() {
            control::poll(self, &listener, cx);
        }

        if let Some(target) = self.scroll_target {
            let next = scroll_step(self.state.scroll, target, self.config.behavior.smooth_scroll);
            self.state.scroll = next;
            if (next - target).abs() <= f32::EPSILON {
                self.scroll_target = None;
            }
            dirty = true;
        }

        if let Overlay::Toast { ttl_frames, .. } = &mut self.overlay {
            *ttl_frames = ttl_frames.saturating_sub(1);
            if *ttl_frames == 0 {
                self.overlay = Overlay::None;
                // Only the hide needs a repaint; the countdown itself draws
                // the identical frame 239 times otherwise.
                dirty = true;
            }
        }

        if dirty {
            cx.notify();
        }
    }

    /// Drain engine events; returns true when anything was handled so the
    /// caller knows a repaint is needed.
    fn drain_engine_events(&mut self, cx: &mut Context<Self>) -> bool {
        let _s = perf_span!("drain_engine_events");
        let mut dirty = false;
        for (page_id, ev) in self.engine.drain_events() {
            match ev {
                webview_cdp::WebViewEvent::Frame { data, .. } => {
                    let __t0 = std::time::Instant::now();
                    let surface = self.surfaces.entry(page_id).or_insert_with(Surface::new);
                    let painted_cdp =
                        self.engine.paint(page_id, |px, w, h, rects| {
                            // CEF path: raw BGRA + damage. No decode, no copy
                            // beyond damaged rows, no allocation.
                            surface.patch_raw(px, w, h, rects);
                        });
                    if !painted_cdp {
                        // Frozen CDP harness: PNG bytes -> decode. Do not
                        // invest here; see decisions.tsv (frame/frozen).
                        surface.patch_png(&data);
                    }
                    // The buffer is published to a GPUI texture in render()
                    // (per visible page, once per rendered frame): publishing
                    // here cloned the full 4MB buffer per damage event even
                    // when several landed within one rendered frame.
                    if let Some(slot) = self.state.slot_mut(page_id) {
                        slot.loading = false;
                    }
                    perf_event!("frame.paint",
                        "page" => page_id,
                        "us" => __t0.elapsed().as_micros() as u64);
                    dirty = true;
                }
                webview_cdp::WebViewEvent::TitleChanged(title) => {
                    perf_event!("event.title", "page" => page_id);
                    self.state.set_title(page_id, &title);
                    let payload = serde_json::json!({ "id": page_id, "title": title });
                    self.fire_hook("page_title_changed", Some(payload), cx);
                    dirty = true;
                }
                webview_cdp::WebViewEvent::UrlChanged(url) => {
                    perf_event!("event.url", "page" => page_id);
                    self.state.set_url(page_id, &url);
                    let payload = serde_json::json!({ "id": page_id, "url": url });
                    self.fire_hook("page_navigated", Some(payload), cx);
                    dirty = true;
                }
                webview_cdp::WebViewEvent::Closed => {
                    if self.state.close_page(page_id, &self.viewport).is_some() {
                        self.engine.close_page(page_id);
                    }
                    self.surfaces.remove(&page_id);
                    self.view_sizes.remove(&page_id);
                    self.focus_cache.remove(&page_id);
                    dirty = true;
                }
            }
        }
        dirty
    }

    fn poll_config_reload(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        while self.reload_rx.try_recv().is_ok() {
            changed = true;
        }
        if changed {
            match std::fs::read_to_string(config_path()) {
                Ok(src) => {
                    self.lua_source = Arc::new(src);
                    self.reload_lua(cx);
                }
                Err(e) => self.toast(format!("read browser.lua: {e}")),
            }
        }
        changed
    }

    // -- key handling -------------------------------------------------------

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.route_key(&ev.keystroke, cx);
    }

    /// The single key-routing path: overlays first, then config bindings,
    /// then the page. Used by real keys and by synthesized control-socket keys.
    fn route_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) {
        let binding = keystroke_string(ks);

        // Overlay-first routing.
        if let Overlay::Palette { text, .. } = &self.overlay {
            let filter = text.clone();
            self.handle_palette_key(&binding, &filter, ks, cx);
            cx.notify();
            return;
        }
        match &mut self.overlay {
            Overlay::Prompt { text, fresh, .. } => {
                match binding.as_str() {
                    "escape" => self.overlay = Overlay::None,
                    "enter" => {
                        let t = std::mem::take(text);
                        self.submit_prompt(t, cx);
                    }
                    "backspace" => {
                        text.pop();
                        *fresh = false;
                    }
                    _ => {
                        if ks.modifiers == gpui::Modifiers::none() {
                            if let Some(c) = &ks.key_char {
                                // First edit on a fresh prefill replaces it
                                // (select-all-then-type), instead of appending
                                // after the current URL.
                                if std::mem::take(fresh) {
                                    text.clear();
                                }
                                text.push_str(c);
                            }
                        } else if ks.modifiers.control && ks.key == "v" {
                            if let Some(pasted) = cx.read_from_clipboard().and_then(|i| i.text()) {
                                if std::mem::take(fresh) {
                                    text.clear();
                                }
                                text.push_str(&pasted);
                            }
                        }
                    }
                }
                cx.notify();
                return;
            }
            _ => {}
        }

        // Normal mode: config keybindings.
        let hit = self.config.keys.iter().find(|k| k.key == binding).map(|k| {
            (k.command.clone(), k.arg.clone())
        });
        if let Some((cmd, arg)) = hit {
            if let Some(req) = Request::from_command(&cmd, arg.as_deref()) {
                self.dispatch(req, cx);
            } else {
                self.run_lua_command(&cmd, arg, cx);
            }
            return;
        }

        // Unbound keys go to the page (typing in inputs etc).
        if let Some(active) = self.state.active_id() {
            let (text, vk) = cdp_key(ks);
            self.engine.keys(
                active,
                vec![webview_cdp::KeyInput {
                    key: ks.key.clone(),
                    mods: cdp_mods(&ks.modifiers),
                    text,
                    vk,
                }],
            );
        }
        cx.notify();
    }

    // -- control socket helpers ----------------------------------------------

    /// Insert one character into the active text overlay (prompt/palette).
    /// Submit the prompt with its current text (control-socket path).
    pub fn submit_prompt_text(&mut self, cx: &mut Context<Self>) {
        if let Overlay::Prompt { text, .. } = &mut self.overlay {
            let t = std::mem::take(text);
            self.submit_prompt(t, cx);
        } else {
            cx.notify();
        }
    }

    /// Replace the prompt's contents (control-socket path). Typing into a
    /// fresh address bar replaces its prefill, so the socket does too.
    fn handle_palette_key(
        &mut self,
        binding: &str,
        filter: &str,
        ks: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        let matches = self.palette_matches(filter);
        let sel = match &self.overlay {
    /// Move the palette selection by `delta`, clamped to the rows actually
    /// rendered (the visible list is capped at PALETTE_VISIBLE).
    fn palette_move(&mut self, delta: i32) {
        let filter = match &self.overlay {
            Overlay::Palette { text, .. } => text.clone(),
            _ => return,
        };
        let n = self.palette_matches(&filter).len().min(PALETTE_VISIBLE);
        if n == 0 {
            return;
        }
    /// Window-space y of the inner viewport's top edge: the page bar height
    /// when the bar is shown, else 0. Layout runs in inner coordinates and
    /// drawing/hit-testing translate through this.
    fn focus_page(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.state.strip.active_page != Some(id) {
            self.state.focus_page(id, &self.viewport);
            self.scroll_target =
                Some(browser_layout::scroll_to_page(&self.state.strip, &self.viewport, id));
            let payload = serde_json::json!({ "id": id });
            self.fire_hook("page_focused", Some(payload), cx);
            self.sync_hidden_and_focus();
        }
    }

    /// Background pages must not composite (DoD): pages outside the active
    /// workspace get WasHidden(true); the focused page gets input focus.
    fn sync_hidden_and_focus(&mut self) {
        let active_ws = self.state.strip.active_workspace;
        let active = self.state.strip.active_page;
        for page in &self.state.strip.pages {
            let hidden = page.workspace != active_ws;
            if self.focus_cache.get(&page.id) != Some(&hidden) {
                self.engine.set_hidden(page.id, hidden);
                self.focus_cache.insert(page.id, hidden);
            }
        }
        if let Some(a) = active {
            self.engine.set_focus(a, true);
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, local)) = self.page_under(ev.position) {
            self.focus_page(id, cx);
            let button = match ev.button {
                MouseButton::Left => webview_cdp::MouseButton::Left,
                MouseButton::Middle => webview_cdp::MouseButton::Middle,
                MouseButton::Right => webview_cdp::MouseButton::Right,
                MouseButton::Navigate(_) => return,
            };
            self.engine.mouse(
                id,
                f32::from(local.x) as i32,
                f32::from(local.y) as i32,
                webview_cdp::MouseKind::Down,
                button,
                cdp_mods(&ev.modifiers),
            );
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some((id, local)) = self.page_under(ev.position) {
            let button = match ev.button {
                MouseButton::Left => webview_cdp::MouseButton::Left,
                MouseButton::Middle => webview_cdp::MouseButton::Middle,
                MouseButton::Right => webview_cdp::MouseButton::Right,
                MouseButton::Navigate(_) => return,
            };
            self.engine.mouse(
                id,
                f32::from(local.x) as i32,
                f32::from(local.y) as i32,
                webview_cdp::MouseKind::Up,
                button,
                cdp_mods(&ev.modifiers),
            );
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some((id, local)) = self.page_under(ev.position) {
            self.engine.mouse(
                id,
                f32::from(local.x) as i32,
                f32::from(local.y) as i32,
                webview_cdp::MouseKind::Move,
                webview_cdp::MouseButton::Left,
                cdp_mods(&ev.modifiers),
            );
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some((id, local)) = self.page_under(ev.position) {
            let d = ev.delta.pixel_delta(px(20.0));
            self.engine
                .scroll(id, f32::from(local.x) as i32, f32::from(local.y) as i32, f32::from(d.x) as i32, f32::from(d.y) as i32);
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The shell always holds window focus: overlays route keys first,
        // everything unbound forwards to the page.
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }

        // Keep the layout viewport in sync with the real window size. The bars
        // reserve their heights from it so frames sit *between* the bars.
        let vs = window.viewport_size();
        let new_vp = Viewport {
            width: vs.width.into(),

        // Webview viewports must track their on-screen frame size so CDP
        // screenshots match what is displayed. Checked per render, but each
        // view is only resized when its rounded frame size actually changes
        // (per-frame resize churned the engine's compositor: see perf audit).
        // In overview the frames are shrunken; resize webviews to match.
        let mut resize_count = 0usize;
        for (id, g) in &geos {
            let size = (g.width.round() as u32, g.height.round() as u32);
            if size.0 == 0 || size.1 == 0 {
                continue;
            }
            if self.view_sizes.get(id) == Some(&size) {
                continue;
            }
            // Pages without a webview yet (spawned empty, never navigated)
            // would fail the resize and retry every render.
            if !self.engine.has_view(*id) {
                continue;
            }
            if self.engine.resize(*id, size.0, size.1).is_ok() {
                self.view_sizes.insert(*id, size);
            }
            resize_count += 1;
        }
        if resize_count > 0 {
            perf_event!("render.resize_calls", "pages" => resize_count);
        }

        let bg = hex(&self.config.theme.bg);
        let bar_bg = hex(&self.config.theme.bar);
        let bar_text = hex(&self.config.theme.bar_text);
        let border = hex(&self.config.theme.border);
        let border_focus = hex(&self.config.theme.border_focus);
        let accent = hex(&self.config.theme.accent);

        let mut pages = div().absolute().size_full();
        for (id, g) in geos {
            let Some(slot) = self.state.slot(id) else { continue };
            let is_active = self.state.strip.active_page == Some(id);
            let title: SharedString = if slot.page.title.is_empty() {
                if slot.page.url.is_empty() {
                    "new page".into()
                } else {
                    truncate(&slot.page.url, 40).into()
                }
            } else {
                truncate(&slot.page.title, 40).into()
            };

            let mut frame = div()
                .absolute()
                .left(px(g.rel_x))

            // Engine frames live in `surfaces` (written by the event pump);
            // publish any newer buffer once per rendered frame here. Hidden
            // pages (other workspaces) never enter this loop, so their
            // buffers are never uploaded while invisible.
            let tex = {
                let surface = self.surfaces.get_mut(&id);
                surface.and_then(|s| s.texture(cx))
            };
            if let Some(image) = tex {
                frame = frame.child(
                    img(ImageSource::Render(image)).object_fit(ObjectFit::Fill).size_full(),
                );
            } else if let Some(png) = &slot.frame_png {
                if let Some(bgra) = decode_png_bgra(png) {
                    let image = Arc::new(RenderImage::new(
                        smallvec::smallvec![image::Frame::new(bgra)],
                    ));
                    frame = frame.child(
                        img(ImageSource::Render(image)).object_fit(ObjectFit::Fill).size_full(),
                    );
                }
            } else if slot.loading {
                frame = frame.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(bar_text)
                        .child(format!("loading {title}")),
                );
            }

            // No floating title chip here: it hovered over page content and
            // duplicated what the tab bar already shows (titles are slop).

            pages = pages.child(frame);
        }

        let mut root = div()
            .id("root")
            .size_full()
            .bg(bg)
            .track_focus(&self.focus)
            .key_context("Browser")
            .on_key_down(cx.listener(Self::on_key))
            .child(pages);

        if self.config.behavior.show_page_bar {
            root = root.child(self.render_page_bar(bar_bg, bar_text, accent, border, cx));
        }
        if self.config.behavior.show_status_bar {
            root = root.child(self.render_status_bar(bar_bg, bar_text, accent));
        }

        match &self.overlay {
            Overlay::Prompt { text, .. } => {
                root = root.child(self.render_prompt(text.clone(), bar_bg, bar_text, accent));
            }
// ---------------------------------------------------------------------------
// Sub-renderers
// ---------------------------------------------------------------------------

impl Shell {
    fn render_page_bar(
        &self,
        bar_bg: gpui::Hsla,
        bar_text: gpui::Hsla,
        accent: gpui::Hsla,
        border: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut bar = div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .h(px(PAGE_BAR_H))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .bg(bar_bg)
            .border_b_1()
            .border_color(border)
            // Full-height page frames lie *under* this bar (top = 0); without
            // this, a tab click also fires the frame's handler underneath and
            // the bubble phase wins, reverting the focus.
            } else {
                truncate(&p.title, 24).into()
            };
            // Tabs are interactive: left-click switches to the page,
            // middle-click closes it (browser convention).
            let tab = p.id;
            bar = bar.child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_size(px(11.0))
                    .text_color(if is_active { accent } else { bar_text })
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _win, cx| {
