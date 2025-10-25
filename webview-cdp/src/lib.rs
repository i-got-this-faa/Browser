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
