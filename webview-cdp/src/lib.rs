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
