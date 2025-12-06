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

