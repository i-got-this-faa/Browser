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

