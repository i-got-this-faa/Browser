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
}



fn serve(
    stream: UnixStream,
    shell: &mut Shell,
    cx: &mut gpui::Context<Shell>,
) -> std::io::Result<()> {
    // Read timeout: a client that connects without sending must not block
    // the UI thread (the frame pump answers this socket synchronously).
    stream.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let reply = exec_line(shell, line.trim(), cx);
    let mut stream = stream;
    stream.write_all(reply.as_bytes())?;
    stream.write_all(b"\n")?;
    Ok(())
}

/// Handle one request. Public so tests can drive a Shell without a socket.
pub fn exec_line(shell: &mut Shell, line: &str, cx: &mut gpui::Context<Shell>) -> String {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return err(&format!("bad json: {e}")),
    };
    let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or_default();
    let arg = req.get("arg").and_then(Value::as_str).unwrap_or_default();

    match cmd {
        "state" => state_json(shell),
        "exec" => {
            if arg.is_empty() {
                return err("exec needs arg");
            }
            shell.run_typed_command(arg, cx);
            ok(&format!("exec {}", arg))
        }
        "prompt" => {
            // Open the prompt, replace its address-bar prefill with the
            // argument (typing replaces a selected prefill in a real bar),
            // submit — the real UX path.
            shell.run_typed_command("focus.url", cx);
