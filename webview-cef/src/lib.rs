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

