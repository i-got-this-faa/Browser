//! Strip Browser: a programmable, Niri-inspired browser shell.
//!
//! Architecture (docs/ARCHITECTURE.md):
//!   browser.lua -> LuaHost (requests in, snapshot out)
//!   BrowserState + ops -> GPUI shell -> WebView trait -> Chromium engine
//!
//! The Chromium engine runs headless; this GPUI window is the browser.

mod agent;
mod control;
mod engine;
mod trace;

use anyhow::{Context as _, Result};
use browser_config::{config_path, ensure_default_config, watch_config, Config, WatcherHandle};
use browser_core::{perf_event, perf_span};
use std::collections::HashMap;
use browser_layout::{frame_geometries, scroll_step, Viewport};
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
    if let Some(code) = webview_cef::early_process_exit_code() {
        std::process::exit(code);
    }
    trace::init_from_env();

    let config_file = config_path();
    if ensure_default_config(&config_file, DEFAULT_LUA).context("write default browser.lua")? {
        eprintln!("created {}", config_file.display());
    }

    let engine = EngineController::spawn(3)?;

    Application::new().run(move |cx: &mut App| {
        cx.activate(true);

        let bounds = Bounds {
            origin: Point::new(px(0.), px(0.)),
            size: size(px(1280.), px(800.)),
        };
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: true,
                app_id: Some("strip-browser".into()),
                window_decorations: Some(gpui::WindowDecorations::Client),
                ..Default::default()
            },
            |window, cx| {
                use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
                let mut wayland_active = false;
                if let (Ok(disp), Ok(win)) = (
                    HasDisplayHandle::display_handle(window),
                    HasWindowHandle::window_handle(window),
                ) {
                    if let (
                        raw_window_handle::RawDisplayHandle::Wayland(d),
                        raw_window_handle::RawWindowHandle::Wayland(w),
                    ) = (disp.as_raw(), win.as_raw())
                    {
                        wayland_active = engine.wayland_init(
                            d.display.as_ptr() as *mut _,
                            w.surface.as_ptr() as *mut _,
                        );
                        if wayland_active {
                            tracing::info!("initialized native Wayland subsurface presentation");
                        }
                    }
                }
                cx.new(|cx| Shell::new(engine.clone(), wayland_active, cx))
            },
        )
        .unwrap();
    });

    Ok(())
}

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
    Palette { text: String, selected: usize },
    /// Transient message shown near the status bar.
    Toast { text: String },
}

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
    /// which silently killed config hot-reload after startup. Change events
    /// also kick the frame pump, so edits apply even while fully idle.
    _watcher: Option<WatcherHandle>,
    /// Pending smooth-scroll target; None means settled.
    scroll_target: Option<f32>,
    /// Wall-clock time the toast hides at (toast is time-based: the pump no
    /// longer ticks a fixed 60Hz, so frame-counting would freeze mid-display).
    toast_deadline: Option<std::time::Instant>,
    /// Scriptable control socket (automation + headless E2E). None if bind
    /// failed. The acceptor thread hands clients to the pump over a channel,
    /// so the UI thread never calls accept() itself (audit #9 follow-up).
    control: Option<control::ControlListener>,
    /// Stable render surfaces per page: one BGRA buffer + one RenderImage
    /// each, re-uploaded only on damage or resize (no per-frame allocation).
    surfaces: HashMap<u64, Surface>,
    /// Last size sent to each engine view; avoids redundant resize calls.
    view_sizes: HashMap<u64, (u32, u32)>,
    /// Last hidden state pushed per page; avoids redundant SetHidden commands.
    focus_cache: HashMap<u64, bool>,
    wayland_active: bool,
}

/// One page's render surface. `bgra` is the LIVE CPU-side frame: the pump
/// patches engine damage into it in place, and it accumulates the full page
/// across publishes. `painted` is the GPUI texture snapshot derived from it;
/// it is only rebuilt when `version` advances (damage or resize), and the
/// superseded atlas tile is retired via drop_image on every publish (each
/// skipped retirement leaks a full-page texture).
struct Surface {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    version: u64,
    painted: Option<Arc<RenderImage>>,
    painted_version: u64,
}

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
    fn patch_png(&mut self, png: &[u8]) {
        if let Some(img) = decode_png_bgra(png) {
            let (w, h) = img.dimensions();
            self.width = w;
            self.height = h;
            self.bgra = img.into_raw();
            self.version += 1;
        }
    }

    /// Publish `bgra` as the GPUI texture `render` draws, if it changed since
    /// the last upload. Returns None until the first frame arrives.
    ///
    /// gpui's sprite atlas keys tiles by `ImageId` and reads bytes only on
    /// first insert (`get_or_insert_with`), so updated pixels require a NEW
    /// RenderImage — reusing the old one would keep drawing stale tiles.
    ///
    /// The buffer is NOT handed to gpui by value: `bgra` is the live
    /// cumulative frame the pump keeps patching, so publishing snapshots it
    /// with one bounded clone and `bgra` stays intact for the next damage
    /// round. Taking ownership used to zero the live buffer between
    /// publishes — every later patch repainted only its damage rect into a
    /// zeroed page, leaving black holes everywhere except whichever region
    /// happened to be damaged last (the floating-content-on-black bug).
    ///
    /// The superseded image is retired through THIS window's atlas: during
    /// paint gpui holds the current window taken out of `App.windows`, so
    /// `drop_image(old, None)` iterates only the OTHER windows and skips the
    /// one that actually inserted the tile — leaking a full-page texture per
    /// damage event. Same `Some(window)` pattern as gpui's image cache.
    fn texture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) -> Option<Arc<RenderImage>> {
        if self.painted.is_some() && self.painted_version == self.version {
            return self.painted.clone();
        }
        let bytes = self.width as usize * self.height as usize * 4;
        if bytes == 0 || self.bgra.len() != bytes {
            return None;
        }
        // BGRA (CEF's format; patch_png swaps too) — gpui wants BGRA.
        let __t0 = std::time::Instant::now();
        let frame = image::RgbaImage::from_raw(self.width, self.height, self.bgra.clone())?;
        perf_event!("surface.texture_upload",
            "bytes" => bytes,
            "us" => __t0.elapsed().as_micros() as u64);
        let next = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            frame,
        )]));
        let old = self.painted.replace(Arc::clone(&next));
        self.painted_version = self.version;
        if let Some(old) = old {
            cx.drop_image(old, Some(window));
        }
        Some(next)
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Shell {
    fn new(
        engine: EngineController,
        wayland_active: bool,
        cx: &mut Context<Self>,
    ) -> Self {
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
            toast_deadline: None,
            control: control::start(),
            surfaces: HashMap::new(),
            view_sizes: HashMap::new(),
            focus_cache: HashMap::new(),
            wayland_active,
        };
        shell.reload_lua(cx);
        shell.ensure_first_page(cx);

        // Event-driven frame pump: awaits browser_core::wakeslot::frame_wake()
        // instead of ticking a ~60Hz timer, so an idle shell runs zero pump
        // iterations and burns no CPU. Producers (CEF sink, CDP reader,
        // config watcher, control acceptor) kick the pump; GPUI notifies the
        // window on state changes and repaints itself. A timer backs the
        // pump only while animation is in flight (smooth scroll, toast).
        cx.spawn(async move |this, cx| loop {
            let animate = this.update(cx, |this, _| this.animation_deadline().is_some()).unwrap_or(false);
            // 60Hz only while animation is in flight; otherwise a 1h timeout
            // that exists purely so the race below can also be won by the
            // wake channel (and so a lost kick can never wedge the pump).
            let timer = cx.background_executor().timer(match animate {
                true => std::time::Duration::from_millis(16),
                false => std::time::Duration::from_secs(3600),
            });
            let wake = async { let _ = browser_core::wakeslot::frame_wake().recv().await; };
            futures_lite::future::or(timer, wake).await;
            if this.update(cx, |this, cx| this.frame(cx)).is_err() {
                break; // shell released: window closed
            }
        })
        .detach();

        shell
    }

    fn ensure_first_page(&mut self, cx: &mut Context<Self>) {
        if self.state.strip.pages.is_empty() {
            let home = self.config.behavior.home_page.clone();
            let id = self.state.add_page(&home, &self.viewport);
            let mut fx = ops::Effects::default();
            fx.spawn.push((id, home));
            self.effects(&mut fx, cx);
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
        // 240 frames at 60Hz was ~4s; keep the same wall-clock duration.
        self.overlay = Overlay::Toast { text: text.into() };
        self.toast_deadline = Some(std::time::Instant::now() + std::time::Duration::from_millis(4000));
        // The countdown advances on pump ticks; kick so idle shells animate
        // the toast away on time.
        browser_core::wakeslot::kick();
    }

    /// The built-in (and Lua) command table for the agent `help` surface.
    pub fn typed_command_list(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = browser_runtime::Request::all_commands()
            .iter()
            .map(|(n, d)| (n.to_string(), d.to_string()))
            .collect();
        if let Some(host) = &self.lua {
            out.extend(host.command_names());
        }
        out
    }

    // -- dispatch -----------------------------------------------------------

    /// Apply engine/UI side effects.
    fn effects(&mut self, fx: &mut ops::Effects, cx: &mut Context<Self>) {
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
            self.retire_surface(id, cx);
            self.focus_cache.remove(&id);
        }
        if let Some(text) = fx.toast.take() {
            self.toast(text);
        }
    }

    /// Remove a page's render surface, retiring its atlas tile. Runs on the
    /// pump, where the current window IS inside `App.windows`, so
    /// `drop_image(_, None)` reaches every window that holds the texture.
    fn retire_surface(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(surface) = self.surfaces.remove(&id) {
            if let Some(image) = surface.painted {
                cx.drop_image(image, None);
            }
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
            self.overlay = Overlay::Palette { text: String::new(), selected: 0 };
        }
        self.effects(&mut fx, cx);
        if fx.quit {
            // Palette/Lua `app.quit`: ops set Effects::quit; nothing read it
            // before, so the command silently did nothing.
            self.engine.shutdown();
            cx.quit();
            return;
        }
        // Retarget the smooth scroll only for commands that actually move
        // focus/workspaces; per-frame pokes (toast, prompt, palette) must not
        // re-center the strip (perf audit: every dispatch reset scroll).
        if fx.scroll_recenter {
            self.scroll_target = Some(browser_layout::scroll_to_active(&self.state.strip, &vp));
        }
        cx.notify();
    }

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
        // The acceptor thread queues clients; answer them here on the UI
        // thread. take() sidesteps the double borrow of self.
        let mut listener = self.control.take();
        if let Some(l) = listener.as_mut() {
            l.drain(self, cx);
        }
        self.control = listener;

        if let Some(target) = self.scroll_target {
            let next = scroll_step(self.state.scroll, target, self.config.behavior.smooth_scroll);
            self.state.scroll = next;
            if (next - target).abs() <= f32::EPSILON {
                self.scroll_target = None;
            }
            dirty = true;
        }

        // Toast lifetime is wall-clock now: the pump only runs when kicked,
        // so frame-counting would freeze the countdown while idle.
        if let Overlay::Toast { .. } = &self.overlay {
            if self.toast_deadline.map(|d| std::time::Instant::now() >= d).unwrap_or(true) {
                self.overlay = Overlay::None;
                self.toast_deadline = None;
                dirty = true;
            }
        }

        if dirty {
            cx.notify();
        }
    }

    /// When the next pump iteration must run for animation to look smooth:
    /// while a smooth scroll is in flight, or while a toast is counting down.
    /// None means fully idle — the pump then waits for the next kick.
    fn animation_deadline(&self) -> Option<std::time::Instant> {
        if self.scroll_target.is_some() || self.toast_deadline.is_some() {
            Some(std::time::Instant::now())
        } else {
            None
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
                webview_cdp::WebViewEvent::Dmabuf {
                    fd,
                    ..
                } => {
                    let __t0 = std::time::Instant::now();
                    unsafe { libc::close(fd); }
                    if let Some(slot) = self.state.slot_mut(page_id) {
                        slot.loading = false;
                    }
                    perf_event!("frame.dmabuf",
                        "page" => page_id,
                        "us" => __t0.elapsed().as_micros() as u64);
                    dirty = true;
                }
                webview_cdp::WebViewEvent::Closed => {
                    if self.state.close_page(page_id, &self.viewport).is_some() {
                        self.engine.close_page(page_id);
                    }
                    self.retire_surface(page_id, cx);
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
    pub fn set_prompt_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Overlay::Prompt { text: slot, fresh, .. } = &mut self.overlay {
            *slot = text.to_string();
            *fresh = false;
        }
        cx.notify();
    }

    /// Synthesize a keystroke through the same routing as real keys.
    pub fn synthesize_key(&mut self, binding: &str, cx: &mut Context<Self>) {
        let ks = parse_binding(binding);
        self.route_key(&ks, cx);
    }

    /// Overlay kind, for control-socket state reporting.
    pub fn overlay_kind(&self) -> &'static str {
        match &self.overlay {
            Overlay::None => "none",
            Overlay::Prompt { .. } => "prompt",
            Overlay::Palette { .. } => "palette",
            Overlay::Toast { .. } => "toast",
        }
    }

    // -- palette ------------------------------------------------------------

    /// Palette key handling, split out so the overlay borrow ends first.
    fn handle_palette_key(
        &mut self,
        binding: &str,
        filter: &str,
        ks: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        let matches = self.palette_matches(filter);
        let sel = match &self.overlay {
            Overlay::Palette { selected, .. } => *selected,
            _ => 0,
        };
        let pick = matches
            .get(sel.min(matches.len().saturating_sub(1)))
            .map(|(name, _)| name.clone());
        match binding {
            "escape" => self.overlay = Overlay::None,
            "enter" => {
                self.overlay = Overlay::None;
                if let Some(name) = pick {
                    self.run_typed_command(&name, cx);
                }
            }
            // Selection moves with arrows (and the scroll wheel, wired in
            // render_palette) instead of falling through to `_` = do nothing.
            "up" | "pageup" => self.palette_move(-1),
            "down" | "pagedown" => self.palette_move(1),
            "backspace" => {
                if let Overlay::Palette { text, selected, .. } = &mut self.overlay {
                    text.pop();
                    *selected = 0;
                }
            }
            "tab" => {
                if let Some(name) = pick {
                    if let Overlay::Palette { text, selected, .. } = &mut self.overlay {
                        *text = name;
                        *selected = 0;
                    }
                }
            }
            _ => {
                if ks.modifiers == gpui::Modifiers::none() {
                    if let Some(c) = &ks.key_char {
                        if let Overlay::Palette { text, selected, .. } = &mut self.overlay {
                            text.push_str(c);
                            *selected = 0;
                        }
                    }
                }
            }
        }
    }

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
        if let Overlay::Palette { selected, .. } = &mut self.overlay {
            *selected = (*selected as i32 + delta).clamp(0, n as i32 - 1) as usize;
        }
    }

    fn palette_matches(&self, filter: &str) -> Vec<(String, String)> {
        self.palette_matches_for(filter)
    }

    fn palette_matches_for(&self, filter: &str) -> Vec<(String, String)> {
        let f = filter.to_lowercase();
        let mut out: Vec<(String, String)> = Request::all_commands()
            .iter()
            .map(|(n, d)| (n.to_string(), d.to_string()))
            .filter(|(n, _)| f.is_empty() || n.contains(&f))
            .collect();
        if let Some(host) = &self.lua {
            for (name, desc) in host.command_names() {
                if f.is_empty() || name.contains(&f) {
                    out.push((name.clone(), desc.clone()));
                }
            }
        }
        out
    }

    // -- chrome / viewport inset --------------------------------------------

    /// Window-space y of the inner viewport's top edge: the page bar height
    /// when the bar is shown, else 0. Layout runs in inner coordinates and
    /// drawing/hit-testing translate through this.
    fn chrome_top(&self) -> f32 {
        if self.config.behavior.show_page_bar { PAGE_BAR_H } else { 0.0 }
    }

    /// Space reserved at the bottom by the status bar (inner coords end here).
    fn chrome_bottom(&self) -> f32 {
        if self.config.behavior.show_status_bar { STATUS_BAR_H } else { 0.0 }
    }

    /// Overview zoom: the whole strip shrinks around the viewport center.
    /// Same constant the agent API reports geometry with, so agent clicks
    /// land where the renderer draws.
    const OVERVIEW_SCALE: f32 = 0.55;

    /// Per-page on-screen geometry at the current scroll/overview state.
    /// One pass feeds hit-testing, webview resize, and the element tree.
    fn page_geos(&self) -> Vec<(u64, browser_layout::PageGeometry)> {
        frame_geometries(
            &self.state.strip,
            &self.viewport,
            self.state.scroll,
            if self.state.overview_open { Self::OVERVIEW_SCALE } else { 1.0 },
        )
    }

    // -- mouse --------------------------------------------------------------

    fn page_under(&self, pos: Point<Pixels>) -> Option<(u64, Point<Pixels>)> {
        let geos = self.page_geos();
        for (id, g) in geos {
            let x0 = g.rel_x;
            let x1 = g.rel_x + g.width;
            let y0 = g.top;
            let y1 = g.top + g.height;
            let px_x = f32::from(pos.x);
            // Window y -> inner-viewport y (bars live above/below the inner box).
            let px_y = f32::from(pos.y) - self.chrome_top();
            if px_x >= x0 && px_x <= x1 && px_y >= y0 && px_y <= y1 {
                let local = Point::new(px(px_x - x0), px(px_y - y0));
                return Some((id, local));
            }
        }
        None
    }

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
            height: (f32::from(vs.height) - self.chrome_top() - self.chrome_bottom()).max(1.0),
        };
        if (new_vp.width - self.viewport.width).abs() > f32::EPSILON
            || (new_vp.height - self.viewport.height).abs() > f32::EPSILON
        {
            self.viewport = new_vp;
            // Re-center the active page when the window resizes.
            if let Some(active) = self.state.active_id() {
                self.scroll_target =
                    Some(browser_layout::scroll_to_page(&self.state.strip, &self.viewport, active));
            }
        }

        // One geometry pass feeds both the webview resize check and the
        // element tree (the two-pass version duplicated the math per frame).
        let __t_geo = std::time::Instant::now();
        let geos = self.page_geos();
        perf_event!("render.geos",
            "pages" => geos.len(),
            "us" => __t_geo.elapsed().as_micros() as u64);

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

        let chrome_top = self.chrome_top();
        let has_overlay = !matches!(self.overlay, Overlay::None);
        if self.wayland_active {
            let visible_ids: std::collections::HashSet<u64> = geos.iter().map(|(id, _)| *id).collect();
            for (id, g) in &geos {
                let x = g.rel_x.round() as i32;
                let y = (g.top + chrome_top).round() as i32;
                let w = g.width.round() as i32;
                let h = g.height.round() as i32;
                self.engine.set_geometry(*id, x, y, w, h, true, has_overlay);
            }
            for p in &self.state.strip.pages {
                if !visible_ids.contains(&p.id) {
                    self.engine.set_geometry(p.id, 0, 0, 0, 0, false, has_overlay);
                }
            }
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
                .top(px(g.top + self.chrome_top()))
                .w(px(g.width))
                .h(px(g.height))
                .border_1()
                .border_color(if is_active { border_focus } else { border })
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
                .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
                .on_mouse_move(cx.listener(Self::on_mouse_move))
                .on_scroll_wheel(cx.listener(Self::on_scroll));

            if !self.wayland_active {
                frame = frame.bg(bar_bg);
                let tex = {
                    let surface = self.surfaces.get_mut(&id);
                    surface.and_then(|s| s.texture(window, cx))
                };
                if let Some(image) = tex {
                    frame = frame.child(
                        img(ImageSource::Render(image)).object_fit(ObjectFit::Fill).size_full(),
                    );
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
            Overlay::Palette { text, selected } => {
                let matches = self.palette_matches(text);
                root = root.child(self.render_palette(
                    text.clone(),
                    matches,
                    *selected,
                    bar_bg,
                    bar_text,
                    accent,
                    cx,
                ));
            }
            Overlay::Toast { text, .. } => {
                root = root.child(
                    div()
                        .absolute()
                        .bottom(px(40.0))
                        .left(px(16.0))
                        .px_3()
                        .py_1()
                        .rounded_sm()
                        .bg(bar_bg)
                        .block_mouse_except_scroll()
                        .text_size(px(12.0))
                        .text_color(bar_text)
                        .child(text.clone()),
                );
            }
            Overlay::None => {}
        }

        root
    }
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
            .block_mouse_except_scroll();
        for p in self.state.strip.visible() {
            let is_active = self.state.strip.active_page == Some(p.id);
            let label: SharedString = if p.title.is_empty() {
                if p.url.is_empty() { "new".into() } else { truncate(&p.url, 24).into() }
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
                            this.focus_page(tab, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _ev, _win, cx| {
                            this.focus_page(tab, cx);
                            this.dispatch(Request::PageClose, cx);
                        }),
                    ),
            );
        }
        bar
    }

    fn render_status_bar(
        &self,
        bar_bg: gpui::Hsla,
        bar_text: gpui::Hsla,
        accent: gpui::Hsla,
    ) -> impl IntoElement {
        let ws = self
            .state
            .strip
            .workspaces
            .iter()
            .map(|w| {
                if w.id == self.state.strip.active_workspace {
                    format!("[{}]", w.name)
                } else {
                    format!(" {}", w.name)
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let active_url: SharedString = self
            .state
            .active_id()
            .and_then(|id| self.state.strip.page(id))
            .map(|p| truncate(&p.url, 80))
            .unwrap_or_default()
            .into();
        div()
            .absolute()
            .bottom(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .h(px(STATUS_BAR_H))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_2()
            .bg(bar_bg)
            .text_size(px(11.0))
            .text_color(bar_text)
            .block_mouse_except_scroll()
            .child(div().text_color(accent).child(ws))
            .child(active_url)
    }

    fn render_prompt(
        &self,
        text: String,
        bar_bg: gpui::Hsla,
        bar_text: gpui::Hsla,
        accent: gpui::Hsla,
    ) -> impl IntoElement {
        let shown: SharedString = if text.is_empty() {
            "search or enter address  (enter = go, esc = cancel)".into()
        } else {
            text.clone().into()
        };
        div().absolute().top(px(36.0)).left(px(0.0)).right(px(0.0)).flex().justify_center().child(
            div()
                .w(relative(0.6))
                .px_3()
                .py_2()
                .rounded_md()
                .bg(bar_bg)
                .border_1()
                .border_color(accent)
                .block_mouse_except_scroll()
                .text_size(px(14.0))
                .text_color(if text.is_empty() { bar_text } else { accent })
                .child(shown),
        )
    }

    fn render_palette(
        &self,
        text: String,
        matches: Vec<(String, String)>,
        selected: usize,
        bar_bg: gpui::Hsla,
        bar_text: gpui::Hsla,
        accent: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sel = selected.min(matches.len().min(PALETTE_VISIBLE).saturating_sub(1));
        let mut list = div().flex().flex_col();
        for (i, (name, desc)) in matches.iter().take(PALETTE_VISIBLE).enumerate() {
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .text_size(px(12.0))
                    .text_color(if i == sel { accent } else { bar_text })
                    .child(name.clone())
                    .child(desc.clone()),
            );
        }
        div().absolute().top(px(36.0)).left(px(0.0)).right(px(0.0)).flex().justify_center().child(
            div()
                .w(relative(0.6))
                .rounded_md()
                .bg(bar_bg)
                .border_1()
                .border_color(accent)
                // The open menu owns the wheel: selection moves and the page
                // behind must not scroll (occlude blocks scroll behind too).
                .occlude()
                .overflow_hidden()
                .on_scroll_wheel(cx.listener(
                    |this: &mut Self, ev: &gpui::ScrollWheelEvent, _window: &mut Window, cx| {
                    // Wayland axis convention: positive y = wheel down.
                    let dy = f32::from(ev.delta.pixel_delta(px(20.0)).y);
                    if dy > 0.5 {
                        this.palette_move(1);
                    } else if dy < -0.5 {
                        this.palette_move(-1);
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(14.0))
                        .text_color(bar_text)
                        .child(if text.is_empty() {
                            SharedString::from(
                                "type a command  (up/down or scroll = select, enter = run, tab = complete)",
                            )
                        } else {
                            text.into()
                        }),
                )
                .child(list),
        )
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn is_url(text: &str) -> bool {
    text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with("about:")
        || text.starts_with("file://")
        || text.starts_with("data:")
        || (text.contains('.') && !text.contains(' '))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

/// GPUI keystroke -> our config's `ctrl+shift+t` style string.
fn keystroke_string(ks: &gpui::Keystroke) -> String {
    let mut out = String::new();
    let m = &ks.modifiers;
    if m.control { out.push_str("ctrl+"); }
    if m.alt { out.push_str("alt+"); }
    if m.shift { out.push_str("shift+"); }
    if m.platform { out.push_str("cmd+"); }
    out.push_str(&ks.key);
    out
}

/// Our `ctrl+shift+t` style string -> a synthetic GPUI keystroke (control
/// socket). key_char set for printable singles so overlay typing works.
fn parse_binding(binding: &str) -> gpui::Keystroke {
    let mut mods = gpui::Modifiers::none();
    let mut key = binding.to_string();
    loop {
        let Some(idx) = key.find('+') else { break };
        let (m, rest) = key.split_at(idx);
        let rest = &rest[1..];
        match m {
            "ctrl" | "control" => mods.control = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "cmd" | "super" => mods.platform = true,
            _ => break,
        }
        key = rest.to_string();
    }
    let key_char = if mods == gpui::Modifiers::none()
        && key.chars().count() == 1
        && key.chars().next().unwrap().is_ascii_graphic()
    {
        Some(key.clone())
    }
    else {
        None
    };
    gpui::Keystroke { modifiers: mods, key, key_char }
}

/// GPUI keystroke -> (cdp text, windows vk).
fn cdp_key(ks: &gpui::Keystroke) -> (Option<String>, u32) {
    if let Some(c) = &ks.key_char {
        if c.chars().count() == 1 && ks.modifiers == gpui::Modifiers::none() {
            let vk = c.chars().next().unwrap().to_ascii_uppercase() as u32;
            return (Some(c.clone()), vk);
        }
    }
    match ks.key.as_str() {
        "enter" => (Some("\r".into()), 13),
        "tab" => (Some("\t".into()), 9),
        "backspace" => (None, 8),
        "escape" => (None, 27),
        "delete" => (None, 46),
        "left" => (None, 37),
        "up" => (None, 38),
        "right" => (None, 39),
        "down" => (None, 40),
        "home" => (None, 36),
        "end" => (None, 35),
        "pageup" => (None, 33),
        "pagedown" => (None, 34),
        "space" => (Some(" ".into()), 32),
        other if other.chars().count() == 1 => {
            let c = other.chars().next().unwrap();
            (Some(c.to_string()), c.to_ascii_uppercase() as u32)
        }
        _ => (None, 0),
    }
}

fn cdp_mods(m: &gpui::Modifiers) -> webview_cdp::InputMods {
    webview_cdp::InputMods {
        ctrl: m.control,
        alt: m.alt,
        shift: m.shift,
        meta: m.platform,
    }
}

/// Decode PNG bytes into a BGRA frame. GPUI `RenderImage` frames are BGRA
/// (see gpui assets.rs), so the R/B swap happens here, once per frame.
fn decode_png_bgra(png: &[u8]) -> Option<image::RgbaImage> {
    let mut img = image::load_from_memory(png).ok()?.into_rgba8();
    for px in img.pixels_mut() {
        px.0.swap(0, 2);
    }
    Some(img)
}

/// Parse `#rrggbb`/`#rrggbbaa` theme colors into GPUI Hsla.
fn hex(s: &str) -> gpui::Hsla {
    match gpui::Rgba::try_from(s) {
        Ok(rgba) => gpui::Hsla::from(rgba),
        Err(_) => gpui::black(),
    }
}
