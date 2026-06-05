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

