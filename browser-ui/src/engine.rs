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
pub struct EngineController {
    shared: Arc<Mutex<EngineShared>>,
    backend: Arc<AtomicU8>,
    dead: Arc<AtomicBool>,
    /// CDP-only: chrome process handle for shutdown.
    cdp_engine: Option<Arc<webview_cdp::ChromeEngine>>,
    cdp_data_dir: Option<PathBuf>,
}

#[derive(Default)]
struct EngineShared {
    /// page id -> live view
    views: HashMap<u64, PageView>,
}

impl EngineController {
    /// Spawn the engine: CEF (default) or the frozen CDP harness
    /// (STRIP_ENGINE=cdp). Falls back to CDP when CEF cannot start.
    pub fn spawn(max_retries: u32) -> Result<Self> {
        if std::env::var("STRIP_ENGINE").as_deref() != Ok("cdp") {
            match Self::spawn_cef() {
                Ok(s) => return Ok(s),
                Err(e) => eprintln!("cef engine unavailable ({e}); falling back to cdp harness"),
            }
        }
        Self::spawn_cdp(max_retries)
    }

    fn spawn_cef() -> Result<Self> {
        // Vendored CEF layout; STRIP_CEF_ROOT overrides for relocatable builds.
        let root = std::env::var("STRIP_CEF_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vendor/cef"));
        let resources = root.join("Resources");
    fn spawn_cdp(max_retries: u32) -> Result<Self> {
        let chrome = webview_cdp::which_chrome().ok_or_else(|| {
            anyhow!("no Chromium engine found (tried google-chrome, chromium, chromium-browser)")
        })?;
        let mut last_err = None;
        for attempt in 0..max_retries {
            let data_dir = std::env::temp_dir().join(format!(
                "strip-browser-engine-{}-{attempt}",
                std::process::id()
            ));
            match webview_cdp::ChromeEngine::spawn(&chrome, 0, &data_dir) {
                Ok(engine) => {
                    return Ok(Self {
                        shared: Arc::new(Mutex::new(EngineShared::default())),
                        backend: Arc::new(AtomicU8::new(BACKEND_CDP)),
                        dead: Arc::new(AtomicBool::new(false)),
                        cdp_engine: Some(Arc::new(engine)),
                        cdp_data_dir: Some(data_dir),
                    });
                }
                Err(e) => {
                    eprintln!("engine spawn attempt {attempt} failed: {e}");
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("engine spawn failed")))
    }

    pub fn backend(&self) -> Backend {
        match self.backend.load(std::sync::atomic::Ordering::SeqCst) {
            BACKEND_CEF => Backend::Cef,
            _ => Backend::Cdp,
        }
    }

    /// Create a webview for a page and remember it.
    pub fn spawn_page(&self, page_id: u64, url: &str) -> Result<()> {
        let view = match self.backend() {
            Backend::Cef => PageView::Cef(webview_cef::CefWebView::create(url, 800, 600)?),
            Backend::Cdp => {
                let engine = self
                    .cdp_engine
                    .as_ref()
                    .ok_or_else(|| anyhow!("cdp engine not running"))?;
                PageView::Cdp(Box::new(engine.new_webview(url)?))
            }
        };
        self.shared.lock().unwrap().views.insert(page_id, view);
        Ok(())
    }

    /// True when a page has a live webview behind it.
    pub fn has_view(&self, page_id: u64) -> bool {
        self.shared.lock().unwrap().views.contains_key(&page_id)
    }

    fn send(&self, page_id: u64, cmd: WebViewCommand) -> Result<()> {
        let shared = self.shared.lock().unwrap();
        let view = shared
            .views
            .get(&page_id)
            .ok_or_else(|| anyhow!("no view for page {page_id}"))?;
        view.as_webview().send(cmd)
    }

    pub fn navigate(&self, page_id: u64, url: &str) -> Result<()> {
        self.send(page_id, WebViewCommand::Navigate(url.to_string()))
    }

    pub fn reload(&self, page_id: u64, bypass_cache: bool) -> Result<()> {
        self.send(
            page_id,
            if bypass_cache {
                WebViewCommand::HardReload
            } else {
                WebViewCommand::Reload
            },
        )
    }

    pub fn go_back(&self, page_id: u64) -> Result<()> {
        self.send(page_id, WebViewCommand::GoBack)
    }

    pub fn go_forward(&self, page_id: u64) -> Result<()> {
        self.send(page_id, WebViewCommand::GoForward)
    }

    pub fn resize(&self, page_id: u64, width: u32, height: u32) -> Result<()> {
        self.send(page_id, WebViewCommand::Resize { width, height })
    }

    /// Background pages must not composite (DoD): WasHidden(true) stops CEF
    /// from producing frames for off-screen pages.
    pub fn set_hidden(&self, page_id: u64, hidden: bool) {
        let _ = self.send(page_id, WebViewCommand::SetHidden(hidden));
    }

    pub fn set_focus(&self, page_id: u64, focused: bool) {
        let _ = self.send(page_id, WebViewCommand::SetFocus(focused));
    }

    pub fn close_page(&self, page_id: u64) {
        let mut shared = self.shared.lock().unwrap();
        if let Some(view) = shared.views.remove(&page_id) {
            let _ = view.close();
        }
    }

    /// Forward a mouse event at page-local coordinates.
    pub fn mouse(
        &self,
        page_id: u64,
        x: i32,
        y: i32,
        kind: MouseKind,
        button: CdpMouseButton,
        mods: InputMods,
    ) {
        let _ = self.send(
            page_id,
            WebViewCommand::Mouse { x, y, kind, button, mods },
        );
    }

    /// Forward a scroll event at page-local coordinates.
