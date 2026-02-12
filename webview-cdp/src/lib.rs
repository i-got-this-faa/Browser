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

