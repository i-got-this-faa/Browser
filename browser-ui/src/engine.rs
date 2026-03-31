//! The engine side of the shell: one web-content engine, one WebContents per
//! page, the raw-frame paint path, and event draining.
//!
//! Backends:
//!   * `webview-cef` (default): genuine Chromium content API via the C ABI in
//!     cef-sys. Frames are raw BGRA + damage rects; nothing is ever encoded.
//!   * `webview-cdp` (STRIP_ENGINE=cdp): the frozen screencast test harness.
//!     Kept only for exercising the WebView seam; do not invest here.
//!
//! This module is the only place that knows which backend is live. Everything
//! above works on page ids.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::{Arc, Mutex};
use webview_cdp::{
    InputMods, KeyInput, MouseButton as CdpMouseButton, MouseKind, WebView, WebViewCommand,
    WebViewEvent,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Cef,
    Cdp,
}

const BACKEND_NONE: u8 = 0;
const BACKEND_CEF: u8 = 1;
const BACKEND_CDP: u8 = 2;

enum PageView {
    Cef(Box<webview_cef::CefWebView>),
    #[allow(dead_code)] // reachable via STRIP_ENGINE=cdp
    Cdp(Box<dyn WebView>),
}

impl PageView {
    fn as_webview(&self) -> &dyn WebView {
        match self {
            PageView::Cef(v) => v.as_ref(),
            PageView::Cdp(v) => v.as_ref(),
        }
    }

    /// Consume the view: `close` takes `self: Box<Self>`, so it needs the
    /// owned box, not the reference `as_webview` hands out.
    fn close(self) -> Result<()> {
        match self {
            PageView::Cef(v) => v.close(),
            PageView::Cdp(v) => v.close(),
        }
    }
}

/// Cloneable handle handed to the UI. All engine access funnels through the
/// shared state guarded by one mutex (UI-thread contention is negligible).
#[derive(Clone)]
