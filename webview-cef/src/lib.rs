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
            WebViewCommand::Mouse { x, y, kind, button, mods: _ } => {
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
                unsafe { ffi::cef_view_mouse(self.view, k, b, x, y, 1) };
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

