//! Standalone CDP verification: spawn chrome, open a page, wait for one
//! real screencast frame. Run: cargo run -p webview-cdp --example cdp_probe

use std::time::{Duration, Instant};
use webview_cdp::{ChromeEngine, WebView, WebViewEvent};

