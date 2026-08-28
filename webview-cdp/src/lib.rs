//! WebView abstraction: the browser shell never touches Chromium internals.
//!
//! The first backend drives a real Chromium engine (Google Chrome today,
//! Helium/Chromium source later) over CDP (`--remote-debugging-port`), turning
//! `Page.startScreencast` frames into PNG payloads the UI can decode and blit
//! into GPUI, and forwarding input through `Input.dispatch*`. A future
//! in-process Helium backend implements the same [`WebView`] trait.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type WebViewId = u64;

/// Commands the shell can send to a web surface.
#[derive(Debug, Clone)]
pub enum WebViewCommand {
    Navigate(String),
    Reload,
    /// Reload bypassing the HTTP cache (Page.reload ignoreCache).
    HardReload,
    GoBack,
    GoForward,
    Resize { width: u32, height: u32 },
    Mouse { x: i32, y: i32, kind: MouseKind, button: MouseButton, mods: InputMods },
    Scroll { x: i32, y: i32, dx: i32, dy: i32 },
    Keys(Vec<KeyInput>),
    SetFocus(bool),
    /// Background pages must not composite (DoD). No-op on the frozen CDP
    /// harness; CEF maps this to CefBrowserHost::WasHidden.
    SetHidden(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind { Down, Up, Move }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton { Left, Middle, Right }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl InputMods {
    fn cdp_modifiers(&self) -> i32 {
        let mut m = 0;
        if self.alt { m |= 1; }
        if self.ctrl { m |= 2; }
        if self.meta { m |= 4; }
        if self.shift { m |= 8; }
        m
    }
}

/// One key press, resolved to text and a Windows virtual key code for CDP.
#[derive(Debug, Clone)]
pub struct KeyInput {
    pub key: String,
    pub mods: InputMods,
    /// Text Chrome should insert (printable keys only).
    pub text: Option<String>,
    pub vk: u32,
}

impl KeyInput {
    /// Build from a GPUI-style key name.
    pub fn new(key: &str, mods: InputMods) -> Self {
        let (text, vk) = key_translation(key);
        Self { key: key.to_string(), mods, text, vk }
    }
}

fn key_translation(key: &str) -> (Option<String>, u32) {
    match key {
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
        other if other.len() == 1 => (Some(other.to_string()), {
            let c = other.chars().next().unwrap().to_ascii_uppercase() as u32;
            c
        }),
        _ => (None, 0),
    }
}

/// Events that flow from a web surface to the shell.
#[derive(Debug, Clone)]
pub enum WebViewEvent {
    Frame { data: Vec<u8>, width: u32, height: u32 },
    TitleChanged(String),
    UrlChanged(String),
    Closed,
}

/// Contract every web-content backend implements. The shell depends only on
/// this trait, keeping it decoupled from Chromium/CDP specifics.
pub trait WebView: Send {
    fn id(&self) -> WebViewId;
    fn send(&self, cmd: WebViewCommand) -> Result<()>;
    fn events(&self) -> &Receiver<WebViewEvent>;
    fn close(self: Box<Self>) -> Result<()>;
}

static NEXT_WEBVIEW_ID: AtomicU64 = AtomicU64::new(1);

pub fn alloc_webview_id() -> WebViewId {
    NEXT_WEBVIEW_ID.fetch_add(1, Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// WebSocket framing over TCP (minimal client subset for CDP)
// ---------------------------------------------------------------------------

fn write_masked_frame(stream: &mut TcpStream, payload: &[u8]) -> Result<()> {
    let __t0 = std::time::Instant::now();
    let mut frame = vec![0x81u8]; // FIN + text
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask_key: [u8; 4] = rand_mask();
    frame.extend_from_slice(&mask_key);
    frame.extend(payload.iter().zip(mask_key.iter().cycle()).map(|(b, m)| b ^ m));
    stream.write_all(&frame)?;
    stream.flush()?;
    browser_core::perf_event!("cdp.ws_write", "bytes" => frame.len(),
        "us" => __t0.elapsed().as_micros() as u64);
    Ok(())
}

fn rand_mask() -> [u8; 4] {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    [(t & 0xff) as u8, (t >> 8) as u8, (t >> 16) as u8, (t >> 24) as u8 | 0x80]
}

fn read_frame(reader: &mut BufReader<TcpStream>) -> Result<Vec<u8>> {
    let mut header = [0u8; 2];
    reader.read_exact(&mut header)?;
    let len = match header[1] & 0x7F {
        126 => {
            let mut b = [0u8; 2];
            reader.read_exact(&mut b)?;
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            reader.read_exact(&mut b)?;
            u64::from_be_bytes(b) as usize
        }
        n => n as usize,
    };
    let masked = (header[1] & 0x80) != 0;
    let mut payload = vec![0u8; len];
    if masked {
        let mut mask = [0u8; 4];
        reader.read_exact(&mut mask)?;
        reader.read_exact(&mut payload)?;
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    } else {
        reader.read_exact(&mut payload)?;
    }
    Ok(payload)
}

/// Client frames are always masked; mask bytes follow the payload on the wire.
// ---------------------------------------------------------------------------
// CDP session
// ---------------------------------------------------------------------------

/// One CDP websocket session. Writes are serialized behind a mutex; responses
/// are routed back to synchronous callers by the reader thread.
struct CdpSession {
    write_half: Arc<Mutex<TcpStream>>,
pub struct DevtoolsTarget {
    #[serde(rename = "type")]
    pub target_type: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    pub ws_url: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub title: String,
}

impl DevtoolsTarget {
    /// Path part of the ws url, e.g. "/devtools/page/ABC123".
    fn path(&self) -> String {
        // ws://host:port/devtools/page/ID -> everything after host:port
        let without_scheme = self.ws_url.trim_start_matches("ws://");
        match without_scheme.find('/') {
            Some(i) => without_scheme[i..].to_string(),
            None => "/devtools/page".to_string(),
        }
    }
}

impl CdpSession {
    fn connect(port: u16, path: &str) -> Result<(Self, Receiver<WebViewEvent>)> {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .with_context(|| format!("connect to CDP port {port}"))?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
        let mut stream = stream;

        let key = "x3JJHMbDL1EzLkh9GBhXDw==";
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes())?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                return Err(anyhow!("CDP handshake failed: connection closed"));
            }
            if line.trim().is_empty() {
                break;
            }
        }

        let write_half = Arc::new(Mutex::new(stream));

        // Reader thread: routes responses to pending callers, events outward.
        let event_tx = channel::<WebViewEvent>();
        std::thread::Builder::new()
            .name("cdp-reader".into())
            .spawn(move || {
                let mut reader = reader;
                loop {
                    match read_frame(&mut reader) {
                        Ok(bytes) => {
                            let v: Value = match serde_json::from_slice(&bytes) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            if let Some(id) = v["id"].as_i64() {
                                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                    let _ = tx.send(v["result"].clone());
                                }
                                continue;
                            }
                            #[cfg(feature = "debug-spawn")]
                            if v["method"].is_string() {
                                eprintln!("[cdp event] {}", v["method"].as_str().unwrap_or(""));
                            }
                            match v["method"].as_str().unwrap_or("") {
                                "Page.screencastFrame" => {
                                    let data = v["params"]["data"].as_str().unwrap_or("");
                                    let width =
                                        v["params"]["metadata"]["deviceWidth"].as_u64().unwrap_or(0) as u32;
                                    let height =
                                        v["params"]["metadata"]["deviceHeight"].as_u64().unwrap_or(0) as u32;
                                    use base64::Engine as _;
                                    if let Ok(raw) =
                                        base64::engine::general_purpose::STANDARD.decode(data)
                                    {
                                        if event_tx
                                            .0
                                            .send(WebViewEvent::Frame { data: raw, width, height })
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    // Ack so chrome keeps producing frames.
                                    let ack = json!({
                                        "id": next_id.fetch_add(1, Ordering::SeqCst),
                                        "method": "Page.screencastFrameAck",
                                        "params": { "sessionId": v["params"]["sessionId"] }
                                    });
                                    if let Ok(mut w) = write_half.lock() {
                                        let _ = write_masked_frame(&mut w, ack.to_string().as_bytes());
                                    }
                                }
                                "Page.frameNavigated" => {
                                    if let Some(u) = v["params"]["frame"]["url"].as_str() {
                                        if !u.starts_with("about:") {
                                            let _ = event_tx
                                                .0
                                                .send(WebViewEvent::UrlChanged(u.to_string()));
                                        }
                                    }
                                }
                                "Page.titleChanged" => {
                                    if let Some(t) = v["params"]["title"].as_str() {
                                        let _ =
                                            event_tx.0.send(WebViewEvent::TitleChanged(t.to_string()));
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            // Read timeouts are normal on idle pages; only a
                            // truly closed connection ends the reader.
                            match e.downcast_ref::<std::io::Error>().map(|io| io.kind()) {
                                Some(std::io::ErrorKind::WouldBlock)
                                | Some(std::io::ErrorKind::TimedOut)
                                | Some(std::io::ErrorKind::Interrupted) => continue,
                                _ => {
                                    let _ = event_tx.0.send(WebViewEvent::Closed);
                                    break;
                                }
                            }
                        }
                    }
                }
            })
            .ok();

        Ok((session, event_tx.1))
    }

    /// Synchronous call: send, wait for the matching response.
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let __t0 = std::time::Instant::now();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        {
            let mut w = self.write_half.lock().map_err(|_| anyhow!("write lock poisoned"))?;
            let msg = json!({ "id": id, "method": method, "params": params });
            write_masked_frame(&mut w, msg.to_string().as_bytes())?;
        }
        let result = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow!("CDP call {method} timed out"))?;
        self.pending.lock().unwrap().remove(&id);
        browser_core::perf_event!("cdp.call", "method" => method,
            "us" => __t0.elapsed().as_micros() as u64);
        Ok(result)
    }

    /// Fire-and-forget call for high-frequency input.
    fn notify(&self, method: &str, params: Value) -> Result<()> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut w = self.write_half.lock().map_err(|_| anyhow!("write lock poisoned"))?;
        let msg = json!({ "id": id, "method": method, "params": params });
        write_masked_frame(&mut w, msg.to_string().as_bytes())
    }
}

// ---------------------------------------------------------------------------
// Chrome engine
// ---------------------------------------------------------------------------

/// A spawned Chromium engine with its CDP endpoint. One engine serves many
/// WebViews (one CDP target each), like one Helium process serves many pages.
pub struct ChromeEngine {
    pub port: u16,
    child: Mutex<Child>,
    #[allow(dead_code)]
    user_data_dir: PathBuf,
}

impl ChromeEngine {
    /// Spawn a real Chromium engine and wait for its CDP endpoint. When
    /// Helium source is wired in, this is the function that changes.
    pub fn spawn(chrome_path: &str, port: u16, data_dir: &PathBuf) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let mut child = Command::new(chrome_path)
            .arg(format!("--remote-debugging-port={port}"))
            .arg(format!("--user-data-dir={}", data_dir.display()))
            .arg("--headless=new")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-features=TranslateUI")
            .arg("--hide-scrollbars")
            .arg("--force-device-scale-factor=1")
            .arg("--disable-gpu")
            .arg("--window-size=1280,800")
            .arg("--remote-allow-origins=*")
            .arg("about:blank")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {chrome_path}"))?;

        // Chrome announces its endpoint on stderr (also resolves port 0).
        let stderr = child.stderr.take().context("chrome stderr unavailable")?;
        let resolved_port = Self::wait_for_devtools_line(stderr, port)?;

        let engine = Self {
            port: resolved_port,
            child: Mutex::new(child),
            user_data_dir: data_dir.clone(),
        };
        // Endpoint responds, but give /json/list a moment to be consistent.
        for _ in 0..50 {
            if Self::list_targets(engine.port).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(engine)
    }

    fn wait_for_devtools_line(stderr: impl Read, requested: u16) -> Result<u16> {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let line = line?;
            #[cfg(feature = "debug-spawn")]
            eprintln!("[chrome-stderr] {line}");
            if let Some(port) = line
                .split("DevTools listening on ws://127.0.0.1:")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .and_then(|p| p.parse().ok())
            {
                return Ok(port);
            }
        }
        if requested != 0 {
            // Fall back to the requested port; some builds log differently.
            Ok(requested)
        } else {
            Err(anyhow!("chrome never announced a DevTools endpoint"))
        }
    }

    /// Query /json/list. Also used by tests against a fake CDP server.
    pub fn list_targets(port: u16) -> Result<Vec<DevtoolsTarget>> {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .with_context(|| format!("connect to {port} for /json/list"))?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let req = format!(
fn log_step(msg: &str) {
    #[cfg(feature = "debug-spawn")]
    eprintln!("[cdp {msg}]");
    #[cfg(not(feature = "debug-spawn"))]
    let _ = msg;
}

/// Read one full HTTP response. Chrome's DevTools server keeps the socket
/// open even with `Connection: close`, so we must stop exactly at
/// `Content-Length` instead of reading to EOF.
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// CDP WebView
// ---------------------------------------------------------------------------

/// A single web surface backed by one CDP target.
pub struct CdpWebView {
    id: WebViewId,
    session: Arc<CdpSession>,
    events: Receiver<WebViewEvent>,
}

impl CdpWebView {
    fn attach(session: Arc<CdpSession>, events: Receiver<WebViewEvent>) -> Result<Self> {
        session.call("Page.enable", json!({}))?;
        session.call("Emulation.setFocusEmulationEnabled", json!({ "enabled": true }))?;
        // NOTE: format/quality are flat CDP params, not a nested object.
        session.call(
impl WebView for CdpWebView {
    fn id(&self) -> WebViewId {
        self.id
    }

    fn send(&self, cmd: WebViewCommand) -> Result<()> {
        match cmd {
            WebViewCommand::Navigate(url) => {
                self.session.call("Page.navigate", json!({ "url": url }))?;
                Ok(())
            }
            WebViewCommand::Reload => {
                self.session.notify("Page.reload", json!({ "ignoreCache": false }))
            }
            WebViewCommand::HardReload => {
                self.session.notify("Page.reload", json!({ "ignoreCache": true }))
            }
            WebViewCommand::GoBack | WebViewCommand::GoForward => {
                let history = self.session.call("Page.getNavigationHistory", json!({}))?;
                let current = history["currentIndex"].as_i64().unwrap_or(0);
                let entries = history["entries"].as_array().cloned().unwrap_or_default();
                let target_idx = match cmd {
                    WebViewCommand::GoBack => current - 1,
                    _ => current + 1,
                };
                match entries.get(target_idx.max(0) as usize) {
                    Some(entry) if target_idx >= 0 => {
                        let entry_id = entry["id"].as_i64().unwrap_or(0);
                        self.session
                            .call("Page.navigateToHistoryEntry", json!({ "entryId": entry_id }))?;
                        Ok(())
                    }
                    _ => Ok(()), // at the edge of history: no-op
                }
            }
            WebViewCommand::Resize { width, height } => {
                self.session.call(
                    "Emulation.setDeviceMetricsOverride",
                    json!({
                        "width": width, "height": height,
                        "deviceScaleFactor": 1, "mobile": false
                    }),
                )?;
                // Nudge a fresh frame at the new size.
                self.session.notify("Page.captureScreenshot", json!({ "format": "png" }))?;
                Ok(())
            }
            WebViewCommand::Mouse { x, y, kind, button, mods } => {
                let type_ = match kind {
                    MouseKind::Down => "mousePressed",
                    MouseKind::Up => "mouseReleased",
                    MouseKind::Move => "mouseMoved",
                };
                let button = match button {
                    MouseButton::Left => "left",
                    MouseButton::Middle => "middle",
                    MouseButton::Right => "right",
                };
                let pressed = matches!(kind, MouseKind::Down | MouseKind::Up);
                self.session.notify(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": type_, "x": x, "y": y,
                        "button": if kind == MouseKind::Move { "none" } else { button },
                        "buttons": if kind == MouseKind::Down { 1 } else { 0 },
                        "modifiers": mods.cdp_modifiers(),
                        "clickCount": if pressed { 1 } else { 0 },
                    }),
                )
            }
            WebViewCommand::Scroll { x, y, dx, dy } => self.session.notify(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy }),
            ),
            WebViewCommand::Keys(keys) => {
                for k in keys {
                    let modifiers = k.mods.cdp_modifiers();
                    let mut params = json!({
                        "type": "keyDown", "key": k.key,
                        "windowsVirtualKeyCode": k.vk, "nativeVirtualKeyCode": k.vk,
                        "modifiers": modifiers,
                    });
                    if let Some(text) = &k.text {
                        params["text"] = json!(text);
                        params["unmodifiedText"] = json!(text.to_lowercase());
                    }
                    self.session.notify("Input.dispatchKeyEvent", params)?;
                    self.session.notify(
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "keyUp", "key": k.key,
                            "windowsVirtualKeyCode": k.vk, "nativeVirtualKeyCode": k.vk,
                            "modifiers": modifiers,
                        }),
                    )?;
                }
                Ok(())
            }
            WebViewCommand::SetFocus(focused) => self.session.call(
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": focused }),
            ).map(|_| ()),
            // Frozen harness: background compositing suppression is a CEF
            // concern; screencast only produces frames when composited anyway.
            WebViewCommand::SetHidden(_) => Ok(()),
        }
    }

    fn events(&self) -> &Receiver<WebViewEvent> {
        &self.events
    }

    fn close(self: Box<Self>) -> Result<()> {
        let _ = self.session.call("Page.close", json!({}));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// All live web surfaces keyed by shell id. The shell routes geometry/input
/// through this store and polls events per frame.
pub struct WebViewStore {
    views: HashMap<WebViewId, Box<dyn WebView>>,
}

impl Default for WebViewStore {
    fn default() -> Self {
        Self { views: HashMap::new() }
    }
}

impl WebViewStore {
    pub fn add(&mut self, view: Box<dyn WebView>) -> WebViewId {
        let id = view.id();
        self.views.insert(id, view);
        id
    }

    #[allow(dead_code)]
    pub fn get(&self, id: WebViewId) -> Option<&dyn WebView> {
        self.views.get(&id).map(|v| v.as_ref())
    }

    pub fn send(&self, id: WebViewId, cmd: WebViewCommand) -> Result<()> {
        self.views
            .get(&id)
            .ok_or_else(|| anyhow!("unknown webview {id}"))?
            .send(cmd)
    }

    pub fn drain_events(&mut self) -> Vec<(WebViewId, WebViewEvent)> {
        let mut out = Vec::new();
        for (id, view) in self.views.iter_mut() {
            for ev in view.events().try_iter() {
                out.push((*id, ev));
            }
        }
        out
    }

    pub fn remove(&mut self, id: WebViewId) -> Option<Box<dyn WebView>> {
        self.views.remove(&id)
    }
}

// ---------------------------------------------------------------------------
// fake CDP server used by tests (no chrome binary required)
// ---------------------------------------------------------------------------

pub mod testing {
    use super::*;

    /// Minimal fake CDP page target: serves /json/list and /json/new,
