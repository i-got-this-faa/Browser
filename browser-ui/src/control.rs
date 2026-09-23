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
//!
//! Wiring: an acceptor thread accepts connections and sends the streams to the
//! UI thread over a channel, then kicks the frame pump. The pump drains the
//! channel and answers each client inline. This replaced two failed shapes:
//! a per-frame nonblocking `accept` (a syscall per pump iteration, and the
//! reason a silent client could stall the pump) and a blocking read on the UI
//! thread (audit #9). The read timeout stays: a client that connects without
//! sending must not hold the UI thread either.

use crate::Shell;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

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

/// Accepted client with its request line, en route to the UI thread.
type Pending = (UnixStream, String);

/// Listener handle kept alive by the shell: holds the acceptor thread so
/// dropping it shuts the acceptor down and unlinks the socket path.
pub struct ControlListener {
    _acceptor: std::thread::JoinHandle<()>,
    rx: Receiver<Pending>,
}

impl Drop for ControlListener {
    fn drop(&mut self) {
        // Unlink before the acceptor's listener closes, so a replacement
        // instance can bind the same path without racing our stale socket.
        let _ = std::fs::remove_file(socket_path());
    }
}

/// Spawn the acceptor thread. The shell stores the returned handle; the pump
/// drains [`ControlListener::rx`] and serves clients inline on the UI thread.
pub fn start() -> Option<ControlListener> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok()?;
    eprintln!("control socket: {}", path.display());
    let (tx, rx) = mpsc::channel::<Pending>();
    let acceptor = std::thread::Builder::new()
        .name("control-acceptor".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        // Read happens HERE, not on the UI thread: a client
                        // that connects and stalls (or sends slowly) blocks
                        // this thread for at most the read timeout, never
                        // the frame pump. Nothing enters the channel until
                        // a full request line is available.
                        let req = read_request(&s);
                        let Some(line) = req else { continue };
                        if tx.send((s, line)).is_err() {
                            break; // UI thread gone
                        }
                        // The pump may be parked awaiting wakes; nudge it to
                        // come answer this client. Without this, a reply
                        // waits for the next unrelated event (measured 213ms
                        // latency before the fix).
                        browser_core::wakeslot::kick();
                    }
                    Err(_) => break,
                }
            }
        })
        .ok()?;
    Some(ControlListener { _acceptor: acceptor, rx })
}

impl ControlListener {
    /// Take every already-accepted client (with its already-read request
    /// line) and answer each inline on the UI thread. The acceptor thread
    /// did all the waiting, so this is pure computation + one write.
    pub fn drain(&mut self, shell: &mut Shell, cx: &mut gpui::Context<Shell>) {
        loop {
            match self.rx.try_recv() {
                Ok((stream, line)) => {
                    let _ = answer(stream, &line, shell, cx);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }
}

/// Blocking read of one request line, for the ACCEPTOR thread only. A
/// silent or stalled client simply never produces a line and is dropped
/// after the timeout.
fn read_request(stream: &UnixStream) -> Option<String> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// Execute one already-read request and write the reply. Runs on the UI
/// thread; no network reads happen here.
fn answer(
    mut stream: UnixStream,
    line: &str,
    shell: &mut Shell,
    cx: &mut gpui::Context<Shell>,
) -> std::io::Result<()> {
    let reply = exec_line(shell, line.trim(), cx);
    stream.write_all(reply.as_bytes())?;
    stream.write_all(b"\n")?;
    Ok(())
}

/// Handle one request. Native control commands stay stable; everything the
/// agent surface serves routes through `agent::handle` (which also owns the
/// help text). `exec_line` stays public so tests can drive a Shell.
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
            shell.set_prompt_text(&arg, cx);
            shell.submit_prompt_text(cx);
            ok("prompt submitted")
        }
        "key" => {
            if arg.is_empty() {
                return err("key needs arg");
            }
            shell.synthesize_key(arg, cx);
            ok(&format!("key {arg}"))
        }
        "quit" => {
            shell.engine.shutdown();
            cx.quit();
            ok("bye")
        }
        _ => {
            // Agent surface (help, get_state, click, type, screenshot, ...).
            // Unknown commands fall out of agent::handle as an error reply.
            crate::agent::handle(shell, cmd, arg, cx)
                .unwrap_or_else(|| err(&format!("unknown cmd {cmd:?}")))
        }
    }
}

/// Full state snapshot as JSON: pages, active, workspace, scroll, overlays.
pub fn state_json(shell: &Shell) -> String {
    let active = shell.state.active_id();
    let pages: Vec<Value> = shell
        .state
        .strip
        .pages
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "url": p.url,
                "title": p.title,
                "workspace": p.workspace,
                "active": Some(p.id) == active,
            })
        })
        .collect();
    json!({
        "ok": true,
        "active": active,
        "active_workspace": shell.state.strip.active_workspace,
        "scroll": shell.state.scroll as f64,
        "page_fraction": shell.state.strip.page_fraction,
        "overlay": shell.overlay_kind(),
        "pages": pages,
    })
    .to_string()
}

fn ok(msg: &str) -> String {
    json!({ "ok": true, "msg": msg }).to_string()
}

fn err(msg: &str) -> String {
    json!({ "ok": false, "error": msg }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_json_shape_is_stable() {
        // Scripts parse these; keep the envelope pinned.
        let v: Value = serde_json::from_str(&ok("kicked")).unwrap();
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["msg"], json!("kicked"));
        let e: Value = serde_json::from_str(&err("boom")).unwrap();
        assert_eq!(e["ok"], json!(false));
        assert_eq!(e["error"], json!("boom"));
    }
}
