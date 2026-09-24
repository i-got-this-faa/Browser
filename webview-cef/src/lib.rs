//! CEF-backed WebView: a genuine Chromium WebContents per page, embedded via
//! the C ABI in `cef-sys`. Frames arrive as raw BGRA + damage rects (never
//! encoded); input/focus/metrics travel back over the same boundary.
//!
//! The shell keeps depending only on `webview_cdp::{WebView, WebViewEvent}`
//! (the seam). This crate is the content-API implementation of that seam;
//! the CDP backend stays as the frozen throwaway test harness.

use anyhow::{anyhow, Result};
use cef_sys as ffi;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use webview_cdp::{
    InputMods, KeyInput, MouseButton, MouseKind, WebView, WebViewCommand, WebViewEvent, WebViewId,
};

/// Everything the global sink needs to route an event to its view. Views
/// register on create and unregister on drop/close.
struct Sink {
    tx: Option<Sender<WebViewEvent>>,
    damage: Arc<Mutex<Option<Vec<[i32; 4]>>>>,
}

static REGISTRY: std::sync::OnceLock<Mutex<HashMap<u64, Sink>>> = std::sync::OnceLock::new();

fn registry() -> &'static Mutex<HashMap<u64, Sink>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A single web surface backed by a CEF browser (windowless). Implements the
/// same `WebView` trait as the frozen CDP backend; frame data is exposed via
/// [`CefWebView::with_frame`] (lock -> patch damage -> unlock: no allocation,
/// no decode, no copy outside the lock).
pub struct CefWebView {
    id: WebViewId,
    /// Shim-side view handle.
    view: *mut c_void,
    /// ABI-level id; also the event routing key in the sink registry.
    abi_id: u64,
    events: Receiver<WebViewEvent>,
    sink_tx: Option<Sender<WebViewEvent>>,
    damage: Arc<Mutex<Option<Vec<[i32; 4]>>>>,
    destroyed: AtomicBool,
}

// The shim's view registry keeps the view alive (refcounted) until destroy;
// all shim entry points are thread-safe.
unsafe impl Send for CefWebView {}
unsafe impl Sync for CefWebView {}

impl CefWebView {
    /// Create a view; the engine must already be started.
    pub fn create(url: &str, w: i32, h: i32) -> Result<Box<Self>> {
        let c_url = CString::new(url)?;
        let abi_id = next_abi_id();
        let view = unsafe { ffi::cef_view_create(abi_id, c_url.as_ptr(), w, h) };
        if view.is_null() {
            return Err(anyhow!("cef_view_create failed"));
        }
        let (tx, rx) = channel();
        let damage: Arc<Mutex<Option<Vec<[i32; 4]>>>> = Arc::new(Mutex::new(None));
        registry().lock().unwrap().insert(
            abi_id,
            Sink { tx: Some(tx.clone()), damage: Arc::clone(&damage) },
        );
        Ok(Box::new(Self {
            id: webview_cdp::alloc_webview_id(),
            view,
            abi_id,
            events: rx,
            sink_tx: Some(tx),
            damage,
            destroyed: AtomicBool::new(false),
        }))
    }

    /// Event routing key (ABI id). The shell uses this to map events to pages.
    pub fn view_key(&self) -> u64 {
        self.abi_id
    }

    /// Run `f` with exclusive access to the stable BGRA buffer. `f` receives
    /// (pixels, width, height, damage rects since last call; empty == full).
    /// The only sanctioned read path — zero copy, zero allocation.
    pub fn with_frame(&self, f: impl FnOnce(&[u8], i32, i32, &[[i32; 4]])) {
        self.with_buffer(false, f);
    }

    /// Same as [`with_frame`] for the popup layer (select dropdowns etc.).
    pub fn with_popup(&self, f: impl FnOnce(&[u8], i32, i32, &[[i32; 4]])) {
        self.with_buffer(true, f);
    }

    fn with_buffer(&self, popup: bool, f: impl FnOnce(&[u8], i32, i32, &[[i32; 4]])) {
        let mut w: i32 = 0;
        let mut h: i32 = 0;
        // SAFETY: paired lock/unlock around the read; the pointer is valid
        // while the shim's buffer mutex is held.
        let (ptr, len) = unsafe {
            let ptr = if popup {
                ffi::cef_view_lock_popup(self.view, &mut w, &mut h)
            } else {
                ffi::cef_view_lock_frame(self.view, &mut w, &mut h)
            };
            if ptr.is_null() || w <= 0 || h <= 0 {
                if popup {
                    ffi::cef_view_unlock_popup(self.view);
                } else {
                    ffi::cef_view_unlock_frame(self.view);
                }
                (std::ptr::null(), 0usize)
            } else {
                (ptr, (w as usize) * (h as usize) * 4)
            }
        };
        if ptr.is_null() {
            return;
        }
        let rects = self.damage.lock().unwrap().take().unwrap_or_default();
        // SAFETY: len matches the shim buffer while its mutex is held.
        let pixels = unsafe { std::slice::from_raw_parts(ptr, len) };
        f(pixels, w, h, &rects);
        // SAFETY: releases the lock taken above.
        unsafe {
            if popup {
                ffi::cef_view_unlock_popup(self.view);
            } else {
                ffi::cef_view_unlock_frame(self.view);
            }
        }
    }

    pub fn popup_visible(&self) -> bool {
        unsafe { ffi::cef_view_popup_visible(self.view) != 0 }
    }

    pub fn popup_rect(&self) -> [i32; 4] {
        let mut out = [0i32; 4];
        unsafe { ffi::cef_view_popup_rect(self.view, out.as_mut_ptr()) };
        out
    }

    fn destroy_once(&self) {
        if !self.destroyed.swap(true, Ordering::SeqCst) {
            registry().lock().unwrap().remove(&self.abi_id);
            // SAFETY: exactly one destroy per live view.
            unsafe { ffi::cef_view_destroy(self.view) };
        }
    }

    pub fn set_geometry(&self, x: i32, y: i32, w: i32, h: i32, visible: bool, has_overlay: bool) {
        unsafe {
            ffi::cef_view_set_geometry(
                self.view,
                x,
                y,
                w,
                h,
                if visible { 1 } else { 0 },
                if has_overlay { 1 } else { 0 },
            );
        }
    }

    pub fn get_screenshot(&self) -> Option<(u32, u32, Vec<u8>)> {
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut w: i32 = 0;
        let mut h: i32 = 0;
        let mut size: usize = 0;
        unsafe {
            if ffi::cef_view_get_screenshot(self.view, &mut buf, &mut w, &mut h, &mut size) == 0 && !buf.is_null() {
                let slice = std::slice::from_raw_parts(buf, size);
                let vec = slice.to_vec();
                libc::free(buf as *mut libc::c_void);
                Some((w as u32, h as u32, vec))
            } else {
                None
            }
        }
    }

    /// Fast-path mouse dispatch without intermediate enum allocation.
    pub fn mouse_fast(&self, kind: i32, button: i32, x: i32, y: i32, count: i32, mods: u32) {
        unsafe { ffi::cef_view_mouse(self.view, kind, button, x, y, count, mods as i32) };
    }

    /// Fast-path wheel dispatch without intermediate enum allocation.
    pub fn wheel_fast(&self, x: i32, y: i32, dx: i32, dy: i32) {
        unsafe { ffi::cef_view_wheel(self.view, x, y, dx, dy) };
    }

    /// Fast-path key dispatch without intermediate enum allocation.
    pub fn key_fast(&self, key_type: i32, code: i32, native_code: i32, mods: u32, ch: u16) {
        unsafe { ffi::cef_view_key(self.view, key_type, code, native_code, mods, ch) };
    }

    /// Dynamically override view frame rate (e.g. 144 FPS).
    pub fn set_frame_rate(&self, fps: i32) {
        unsafe { ffi::cef_view_set_frame_rate(self.view, fps) };
    }
}

pub fn wayland_init(display: *mut std::ffi::c_void, parent_surface: *mut std::ffi::c_void) -> bool {
    unsafe { ffi::cef_wayland_init(display, parent_surface) == 0 }
}

pub fn set_target_frame_rate(fps: i32) {
    unsafe { ffi::cef_set_target_frame_rate(fps) };
}

pub fn get_target_frame_rate() -> i32 {
    unsafe { ffi::cef_get_target_frame_rate() }
}

impl WebView for CefWebView {
    fn id(&self) -> WebViewId {
        self.id
    }

    fn send(&self, cmd: WebViewCommand) -> Result<()> {
        match cmd {
            WebViewCommand::Navigate(url) => {
                let c = CString::new(url)?;
                unsafe { ffi::cef_view_navigate(self.view, c.as_ptr()) };
            }
            WebViewCommand::Reload => unsafe { ffi::cef_view_reload(self.view, 0) },
            WebViewCommand::HardReload => unsafe { ffi::cef_view_reload(self.view, 1) },
            WebViewCommand::GoBack => unsafe { ffi::cef_view_back(self.view) },
            WebViewCommand::GoForward => unsafe { ffi::cef_view_forward(self.view) },
            WebViewCommand::Resize { width, height } => unsafe {
                ffi::cef_view_resize(self.view, width as i32, height as i32);
            },
            WebViewCommand::SetFocus(f) => unsafe {
                ffi::cef_view_focus(self.view, f as c_int);
            },
            // Background pages must not composite (DoD) -> WasHidden.
            WebViewCommand::SetHidden(h) => unsafe {
                ffi::cef_view_hidden(self.view, h as c_int);
            },
            WebViewCommand::Mouse { x, y, kind, button, mods } => {
                let k = match kind {
                    MouseKind::Move => 0,
                    MouseKind::Down => 1,
                    MouseKind::Up => 2,
                };
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                };
                let mut m = 0i32;
                if mods.shift { m |= 1 << 1; }
                if mods.ctrl { m |= 1 << 2; }
                if mods.alt { m |= 1 << 3; }
                if mods.meta { m |= 1 << 7; }
                unsafe { ffi::cef_view_mouse(self.view, k, b, x, y, 1, m) };
            }
            WebViewCommand::Scroll { x, y, dx, dy } => unsafe {
                ffi::cef_view_wheel(self.view, x, y, dx, dy);
            },
            WebViewCommand::Keys(keys) => {
                for k in &keys {
                    dispatch_key(self.view, k);
                }
            }
        }
        Ok(())
    }

    fn events(&self) -> &Receiver<WebViewEvent> {
        &self.events
    }

    fn close(self: Box<Self>) -> Result<()> {
        if let Some(tx) = self.sink_tx.as_ref() {
            let _ = tx.send(WebViewEvent::Closed);
        }
        self.destroy_once();
        Ok(())
    }
}

impl Drop for CefWebView {
    fn drop(&mut self) {
        self.destroy_once();
    }
}

fn dispatch_key(view: *mut c_void, k: &KeyInput) {
    let mods = cef_mods(&k.mods);
    let vk = k.vk as i32;
    if let Some(text) = &k.text {
        for c in text.encode_utf16() {
            // KEYEVENT_CHAR delivers the character; Chromium synthesizes text.
            unsafe { ffi::cef_view_key(view, 3, vk, vk, mods, c) };
        }
    }
    // RAWKEYDOWN + KEYUP bracket the press for shortcuts and JS handlers.
    unsafe {
        ffi::cef_view_key(view, 0, vk, vk, mods, 0);
        ffi::cef_view_key(view, 2, vk, vk, mods, 0);
    }
}

fn cef_mods(m: &InputMods) -> u32 {
    let mut f = 0u32;
    if m.shift {
        f |= 2; // EVENTFLAG_SHIFT_DOWN
    }
    if m.ctrl {
        f |= 4; // EVENTFLAG_CONTROL_DOWN
    }
    if m.alt {
        f |= 8; // EVENTFLAG_ALT_DOWN
    }
    if m.meta {
        f |= 128; // EVENTFLAG_COMMAND_DOWN
    }
    f
}

// ---------------------------------------------------------------------------
// Global sink: shim events -> per-view queues
// ---------------------------------------------------------------------------

extern "C" fn sink(ev: *const ffi::CefEvent, _ud: *mut c_void) {
    // SAFETY: the shim guarantees `ev` is valid for the duration of the call.
    let ev = unsafe { &*ev };
    let entry = registry().lock().unwrap().get(&ev.view_id).map(|s| Sink {
        tx: s.tx.clone(),
        damage: Arc::clone(&s.damage),
    });
    let Some(entry) = entry else { return };
    match ev.kind {
        ffi::CEF_EV_FRAME | ffi::CEF_EV_POPUP_FRAME => {
            let n = ev.nrects.clamp(0, 16) as usize;
            let rects: Vec<[i32; 4]> = ev.rects[..n].to_vec();
            // MERGE into what Rust has not drained yet: the shim sends the
            // union of everything painted since the last drain, but several
            // shim events can land between two drains, and OVERWRITING here
            // dropped the earlier frames' rects — the shell then patched only
            // the newest rect and stale pixels stayed on screen. An empty rect
            // list is the documented "full frame" signal and resets the merge.
            // (Cap bounds the list; overflow degrades to a full repaint.)
            const MAX_DAMAGE_RECTS: usize = 64;
            {
                let mut d = entry.damage.lock().unwrap();
                if rects.is_empty() {
                    *d = Some(Vec::new());
                } else if !matches!(d.as_ref(), Some(v) if v.is_empty()) {
                    let full = d
                        .as_mut()
                        .map(|v| {
                            v.extend_from_slice(&rects);
                            v.len() > MAX_DAMAGE_RECTS
                        })
                        .unwrap_or(true);
                    if full {
                        *d = Some(Vec::new());
                    }
                }
            }
            // Payload-free nudge: the shell reads pixels via with_frame();
            // nothing encoded ever crosses this boundary.
            if let Some(tx) = entry.tx.as_ref() {
                let _ = tx.send(WebViewEvent::Frame {
                    data: Vec::new(),
                    width: ev.w as u32,
                    height: ev.h as u32,
                });
            }
            // Event-driven pump: wake the UI exactly once per wake cycle
            // (bounded channel coalesces an animated page's damage storm).
            browser_core::wakeslot::kick();
        }
        ffi::CEF_EV_TITLE => {
            if let Some(tx) = entry.tx.as_ref() {
                let _ = tx.send(WebViewEvent::TitleChanged(cstr(ev.str_)));
            }
        }
        ffi::CEF_EV_URL => {
            if let Some(tx) = entry.tx.as_ref() {
                let _ = tx.send(WebViewEvent::UrlChanged(cstr(ev.str_)));
            }
        }
        ffi::CEF_EV_DMABUF => {
            if let Some(tx) = entry.tx.as_ref() {
                let _ = tx.send(WebViewEvent::Dmabuf {
                    fd: ev.dmabuf_fd,
                    width: ev.w as u32,
                    height: ev.h as u32,
                    stride: ev.stride,
                    offset: ev.offset,
                    modifier: ev.modifier,
                    format: ev.drm_format,
                });
            } else {
                unsafe { libc::close(ev.dmabuf_fd); }
            }
            browser_core::wakeslot::kick();
        }
        ffi::CEF_EV_CURSOR => {
            if let Some(tx) = entry.tx.as_ref() {
                let _ = tx.send(WebViewEvent::CursorChanged(
                    webview_cdp::WebCursor::from_cef_type(ev.cursor_type),
                ));
            }
            browser_core::wakeslot::kick();
        }
        _ => {}
    }
}

fn cstr(p: *const std::os::raw::c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: shim passes a valid NUL-terminated UTF-8 string.
    unsafe { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// The one CEF engine per process.
pub struct CefEngine {
    _private: (),
}

impl CefEngine {
    /// Start CEF (windowless, CEF-owned UI thread). `exe` is this binary (the
    /// standard CEF single-binary subprocess pattern); resource paths resolve
    /// to the vendored CEF layout.
    pub fn start(exe: &str, resources: &str, locales: &str, cache: &str) -> Result<Self> {
        let c_exe = CString::new(exe)?;
        let c_res = CString::new(resources)?;
        let c_loc = CString::new(locales)?;
        let c_cache = CString::new(cache)?;
        // SAFETY: one-time init before any view exists; strings outlive call.
        let rc = unsafe {
            ffi::cef_engine_start(
                c_exe.as_ptr(),
                c_res.as_ptr(),
                c_loc.as_ptr(),
                c_cache.as_ptr(),
            )
        };
        if rc != 0 {
            return Err(anyhow!("cef_engine_start rc={rc}"));
        }
        unsafe { ffi::cef_set_sink(sink, std::ptr::null_mut()) };
        Ok(Self { _private: () })
    }
}

/// Early process check: if this invocation is a CEF child process
/// (renderer/gpu/utility), the caller must exit with the returned code
/// immediately instead of running the shell.
pub fn early_process_exit_code() -> Option<i32> {
    let argv: Vec<CString> = std::env::args_os()
        .map(|a| {
            CString::new(a.into_string().unwrap_or_default()).unwrap_or_default()
        })
        .collect();
    let mut ptrs: Vec<*mut std::os::raw::c_char> =
        argv.iter().map(|a| a.as_ptr() as *mut _).collect();
    ptrs.push(std::ptr::null_mut());
    let argc = ptrs.len() as c_int - 1;
    // SAFETY: argv is a valid C argv-shaped array for the duration of the call.
    let rc = unsafe { ffi::cef_early_process(argc, ptrs.as_mut_ptr()) };
    (rc >= 0).then_some(rc)
}

static NEXT_ABI_ID: AtomicU64 = AtomicU64::new(1);
fn next_abi_id() -> u64 {
    NEXT_ABI_ID.fetch_add(1, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_routes_by_abi_id() {
        // Registry mechanics without a live engine (no CEF init in tests).
        let (tx, rx) = channel();
        let damage: Arc<Mutex<Option<Vec<[i32; 4]>>>> = Arc::new(Mutex::new(None));
        registry().lock().unwrap().insert(
            42,
            Sink { tx: Some(tx.clone()), damage: Arc::clone(&damage) },
        );
        *damage.lock().unwrap() = Some(vec![[1, 2, 3, 4]]);
        {
            let entry = registry().lock().unwrap().get(&42).map(|s| Sink {
                tx: s.tx.clone(),
                damage: Arc::clone(&s.damage),
            });
            let e = entry.unwrap();
            assert_eq!(e.damage.lock().unwrap().take().unwrap(), vec![[1, 2, 3, 4]]);
            let _ = e.tx.as_ref().unwrap().send(WebViewEvent::TitleChanged("t".into()));
        }
        registry().lock().unwrap().remove(&42);
        assert!(matches!(rx.try_recv(), Ok(WebViewEvent::TitleChanged(t)) if t == "t"));
    }
}
