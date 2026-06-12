//! Scriptable control socket: one Unix socket, one JSON command per line.
//!
//! This is the automation boundary of the browser: scripts connect to
//! `$XDG_RUNTIME_DIR/strip-browser.sock` (or `/tmp/strip-browser-$UID.sock`)
//! and drive it exactly like a user would — the same dispatch path as keys.
//! It is also how the headless test loop verifies real behavior.
//!
//! Protocol: send a JSON object, read back one JSON line.
//!   {"cmd":"state"}                                -> full browser state
//!   {"cmd":"exec","arg":"page.new_beside"}         -> run any command
//!   {"cmd":"prompt","arg":"example.com"}           -> open+submit the prompt
//!   {"cmd":"key","arg":"ctrl+k"}                   -> synthesize a keystroke
//!   {"cmd":"quit"}                                 -> close the browser
//!
//! Example: `echo '{"cmd":"exec","arg":"page.new"}' | nc -U /tmp/strip-browser.sock`

use crate::Shell;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;

/// Socket path used by scripts and tests. `STRIP_BROWSER_SOCK` overrides,
/// then `$XDG_RUNTIME_DIR/strip-browser.sock`, then `/tmp/strip-browser.sock`.
pub fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("STRIP_BROWSER_SOCK") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(dir).join("strip-browser.sock");
        if p.parent().map(|d| d.exists()).unwrap_or(false) {
            return p;
        }
    }
    PathBuf::from("/tmp/strip-browser.sock")
}

/// Begin listening (nonblocking). Call once at startup. The shell holds the
/// listener in an `Arc` so the per-frame poll can share it without a dup().
pub fn start() -> Option<Arc<UnixListener>> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok()?;
    listener
        .set_nonblocking(true)
        .expect("control socket nonblocking");
    eprintln!("control socket: {}", path.display());
    Some(Arc::new(listener))
}

/// Poll the listener once per frame. Accepts every pending connection and
/// answers it synchronously (clients send one request and wait for the reply).
pub fn poll(shell: &mut Shell, listener: &UnixListener, cx: &mut gpui::Context<Shell>) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = serve(stream, shell, cx);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
