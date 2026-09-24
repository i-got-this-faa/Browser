//! The agent API: drive the whole browser programmatically over the control
//! socket, so AI agents never need screenshots-of-the-chrome or coordinate
//! guessing ("computer use") to operate it.
//!
//! Design rules:
//! * **Same dispatch path as a human.** Every command either performs the
//!   exact engine calls the mouse/keyboard handlers perform (same focus,
//!   hidden, and resize bookkeeping) or reads the same state the renderer
//!   draws from. There is no second browser behind the API.
//! * **Coordinates are page-local.** `get_state` returns each page's
//!   on-screen geometry; the agent clicks inside that box. No window pixels.
//! * **Self-describing.** `{"cmd":"help"}` returns every command with a
//!   one-line description, so an agent can discover the surface at runtime.
//!
//! Wire protocol is unchanged: one JSON request line per connection, one
//! JSON reply line back.

use crate::Shell;
use browser_layout::{frame_geometries, scroll_to_page};
use serde_json::{json, Value};

/// Every command the control socket understands: name, arg format, and what
/// it does. `exec <typed-command>` entries are generated from the runtime's
/// command table so the list never drifts from the real actions.
pub fn help_json(shell: &Shell) -> String {
    let native: &[(&str, &str, &str)] = &[
        ("help", "-", "this list: every command, native and typed"),
        ("state", "-", "compact snapshot: pages, active id, scroll, overlay"),
        (
            "get_state",
            "-",
            "full snapshot: per-page url/title/geometry/webview/loading + workspaces",
        ),
        (
            "page_geometry",
            "[id|active|url-substr]",
            "on-screen rect of one page (page-local click coords come from this)",
        ),
        (
            "click",
            "x y [id|active] [left|middle|right]",
            "click at page-local (x,y); focuses the page first, like a real click",
        ),
        (
            "type",
            "text [id|active]",
            "keyboard-type text into the page (focuses it first)",
        ),
        (
            "key",
            "ctrl+shift+a [id|active]",
            "one keystroke with optional modifiers, straight to the page",
        ),
        (
            "wheel",
            "dx dy [id|active]",
            "scroll wheel at the page center; positive dy scrolls down",
        ),
        (
            "screenshot",
            "[id|active|url-substr]",
            "PNG of the page's latest frame, base64-encoded",
        ),
        (
            "open",
            "url",
            "open a new page on the active workspace and focus it",
        ),
        ("scroll_to", "id|url-substr", "center the strip on a page (instant)"),
        (
            "overlay",
            "get|none|prompt|palette",
            "read or set the shell overlay (agents usually close it: 'none')",
        ),
        (
            "toast",
            "text",
            "show a toast (visible confirmation for humans watching an agent)",
        ),
        (
            "config.get",
            "-",
            "theme + behavior + keybindings as JSON (what browser.lua set)",
        ),
        ("quit", "-", "close the browser"),
    ];
    let mut commands: Vec<Value> = native
        .into_iter()
        .map(|(n, a, d)| json!({ "cmd": n, "arg": a, "desc": d }))
        .collect();
    for (n, d) in shell.typed_command_list() {
        commands.push(json!({ "cmd": format!("exec {n}"), "arg": "-", "desc": d }));
    }
    json!({
        "ok": true,
        "protocol": "one JSON request line per connection; reply is one JSON line",
        "socket": crate::control::socket_path().display().to_string(),
        "commands": commands,
    })
    .to_string()
}

/// Full state: everything an agent needs to decide its next action without
/// taking a screenshot — including per-page on-screen geometry so click
/// coordinates can be computed directly.
pub fn agent_state_json(shell: &Shell) -> String {
    let active = shell.state.active_id();
    let vp = shell.viewport;
    let geos = overview_geos(shell);
    let geo = |id: u64| -> Option<Value> {
        geos.iter().find(|(gid, _)| *gid == id).map(|(_, g)| {
            json!({
                "x": g.rel_x,
                "y": g.top,
                "w": g.width,
                "h": g.height,
                "center": [g.rel_x + g.width / 2.0, g.top + g.height / 2.0],
            })
        })
    };
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
                "visible": p.workspace == shell.state.strip.active_workspace,
                "webview": shell.engine.has_view(p.id),
                "loading": shell.state.slot(p.id).map(|s| s.loading).unwrap_or(false),
                "geometry": geo(p.id),
            })
        })
        .collect();
    json!({
        "ok": true,
        "active": active,
        "active_workspace": shell.state.strip.active_workspace,
        "workspaces": shell.state.strip.workspaces.len(),
        "scroll": shell.state.scroll as f64,
        "overview": shell.state.overview_open,
        "overlay": shell.overlay_kind(),
        "viewport": { "w": vp.width, "h": vp.height },
        "pages": pages,
    })
    .to_string()
}

/// Overview zoom: the whole strip shrinks around the viewport center.
const OVERVIEW_SCALE: f32 = 0.55;

/// Per-page on-screen geometry at the shell's current scroll/overview state.
fn overview_geos(shell: &Shell) -> Vec<(u64, browser_layout::PageGeometry)> {
    frame_geometries(
        &shell.state.strip,
        &shell.viewport,
        shell.state.scroll,
        if shell.state.overview_open { OVERVIEW_SCALE } else { 1.0 },
    )
}

/// Resolve an agent-supplied page selector: numeric id, "active", or a
/// case-insensitive substring match on url/title (first match in the active
/// workspace wins). Substring matching is what makes one-round-trip scripts
/// possible: `click 10 10 github`.
fn resolve_id(shell: &Shell, sel: &str) -> Option<u64> {
    let sel = sel.trim();
    if sel.is_empty() || sel.eq_ignore_ascii_case("active") {
        return shell.state.active_id();
    }
    if let Ok(id) = sel.parse::<u64>() {
        if shell.state.strip.page(id).is_some() {
            return Some(id);
        }
    }
    let lower = sel.to_lowercase();
    shell
        .state
        .strip
        .pages
        .iter()
        .find(|p| {
            p.workspace == shell.state.strip.active_workspace
                && (p.url.to_lowercase().contains(&lower)
                    || p.title.to_lowercase().contains(&lower))
        })
        .map(|p| p.id)
}

fn click(
    shell: &mut Shell,
    x: i32,
    y: i32,
    sel: &str,
    button: &str,
    cx: &mut gpui::Context<Shell>,
) -> Result<Value, String> {
    let id = resolve_id(shell, sel).ok_or("no page")?;
    let button = match button {
        "left" | "" => webview_cdp::MouseButton::Left,
        "middle" => webview_cdp::MouseButton::Middle,
        "right" => webview_cdp::MouseButton::Right,
        other => return Err(format!("unknown button {other:?}")),
    };
    shell.focus_page(id, cx);
    // Down + up pair at the same point: exactly what on_mouse_down/up emit.
    shell
        .engine
        .mouse(id, x, y, webview_cdp::MouseKind::Down, button, Default::default());
    shell
        .engine
        .mouse(id, x, y, webview_cdp::MouseKind::Up, button, Default::default());
    cx.notify();
    Ok(json!({ "ok": true, "page": id, "at": [x, y] }))
}

fn type_text(
    shell: &mut Shell,
    text: &str,
    sel: &str,
    cx: &mut gpui::Context<Shell>,
) -> Result<Value, String> {
    let id = resolve_id(shell, sel).ok_or("no page")?;
    shell.focus_page(id, cx);
    let keys: Vec<webview_cdp::KeyInput> = text
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| webview_cdp::KeyInput::new(&c.to_string(), Default::default()))
        .collect();
    let n = keys.len();
    shell.engine.keys(id, keys);
    cx.notify();
    Ok(json!({ "ok": true, "page": id, "chars": n }))
}

fn press_key(
    shell: &mut Shell,
    spec: &str,
    sel: &str,
    cx: &mut gpui::Context<Shell>,
) -> Result<Value, String> {
    let id = resolve_id(shell, sel).ok_or("no page")?;
    let mut mods = webview_cdp::InputMods::default();
    let mut key = String::new();
    for part in spec.split(['+', '-']) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "meta" | "cmd" | "super" => mods.meta = true,
            "" => {}
            other => key = other.to_string(),
        }
    }
    if key.is_empty() {
        return Err("no key in spec".into());
    }
    shell.focus_page(id, cx);
    let (text, vk) = webview_cdp::key_translation(&key);
    shell.engine.keys(
        id,
        vec![webview_cdp::KeyInput { key, mods, text, vk }],
    );
    cx.notify();
    Ok(json!({ "ok": true, "page": id, "key": spec }))
}

fn wheel(
    shell: &mut Shell,
    dx: i32,
    dy: i32,
    sel: &str,
    cx: &mut gpui::Context<Shell>,
) -> Result<Value, String> {
    let id = resolve_id(shell, sel).ok_or("no page")?;
    shell.focus_page(id, cx);
    // Wheel at the page center, matching where a user's cursor usually is.
    let (w, h) = shell
        .view_sizes
        .get(&id)
        .map(|(w, h)| (*w as i32, *h as i32))
        .unwrap_or((800, 600));
    shell.engine.scroll(id, w / 2, h / 2, dx, dy);
    cx.notify();
    Ok(json!({ "ok": true, "page": id, "dx": dx, "dy": dy }))
}

/// PNG screenshot of the page's latest composited frame, base64-encoded.
/// Zero new engine machinery: this is the exact buffer the renderer shows.
fn screenshot(shell: &Shell, sel: &str) -> Result<Value, String> {
    let id = resolve_id(shell, sel).ok_or("no page")?;
    let (w, h, rgba) = if let Some((w, h, mut bgra)) = shell.engine.get_screenshot(id) {
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        (w, h, bgra)
    } else {
        let surface = shell.surfaces.get(&id).ok_or("no frame yet for page")?;
        if surface.width == 0 || surface.height == 0 || surface.bgra.is_empty() {
            return Err("no frame yet for page".into());
        }
        let mut rgba = surface.bgra.clone();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        (surface.width, surface.height, rgba)
    };

    let img =
        image::RgbaImage::from_raw(w, h, rgba).ok_or("frame size mismatch")?;
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("png encode: {e}"))?;
    use base64::Engine as _;
    Ok(json!({
        "ok": true,
        "page": id,
        "w": w,
        "h": h,
        "png_base64": base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
    }))
}

/// One page's on-screen geometry.
fn page_geometry(shell: &Shell, sel: &str) -> Result<Value, String> {
    let id = resolve_id(shell, sel).ok_or("no page")?;
    let (_, g) = overview_geos(shell)
        .into_iter()
        .find(|(gid, _)| *gid == id)
        .ok_or("page has no geometry (hidden workspace?)")?;
    Ok(json!({
        "ok": true,
        "page": id,
        "x": g.rel_x,
        "y": g.top,
        "w": g.width,
        "h": g.height,
        "center": [g.rel_x + g.width / 2.0, g.top + g.height / 2.0],
    }))
}

/// Open a new page on the active workspace and focus it: `page.new_beside`
/// + prompt submit fused into one round trip.
fn open_url(
    shell: &mut Shell,
    url: &str,
    cx: &mut gpui::Context<Shell>,
) -> Result<Value, String> {
    if url.trim().is_empty() {
        return Err("open needs a url".into());
    }
    let before = shell.state.strip.pages.len();
    shell.run_typed_command("page.new_beside", cx);
    shell.set_prompt_text(url, cx);
    shell.submit_prompt_text(cx);
    let active = shell.state.active_id();
    Ok(json!({
        "ok": true,
        "page": active,
        "spawned": shell.state.strip.pages.len() > before,
    }))
}

fn scroll_to(shell: &mut Shell, sel: &str) -> Result<Value, String> {
    let id = resolve_id(shell, sel).ok_or("no page")?;
    let target = scroll_to_page(&shell.state.strip, &shell.viewport, id);
    shell.state.scroll = target;
    shell.scroll_target = None; // instant, not animated
    Ok(json!({ "ok": true, "page": id, "scroll": target as f64 }))
}

fn overlay(
    shell: &mut Shell,
    what: &str,
    cx: &mut gpui::Context<Shell>,
) -> Result<Value, String> {
    match what {
        "" | "get" => Ok(json!({ "ok": true, "overlay": shell.overlay_kind() })),
        "none" | "close" => {
            shell.overlay = crate::Overlay::None;
            cx.notify();
            Ok(json!({ "ok": true, "overlay": "none" }))
        }
        "prompt" => {
            shell.run_typed_command("focus.url", cx);
            Ok(json!({ "ok": true, "overlay": "prompt" }))
        }
        "palette" => {
            shell.run_typed_command("palette.open", cx);
            Ok(json!({ "ok": true, "overlay": "palette" }))
        }
        other => Err(format!("overlay: get|none|prompt|palette, got {other:?}")),
    }
}

fn config_json(shell: &Shell) -> String {
    json!({
        "ok": true,
        "theme": serde_json::to_value(&shell.config.theme).unwrap_or_default(),
        "behavior": serde_json::to_value(&shell.config.behavior).unwrap_or_default(),
        "keys": shell
            .config
            .keys
            .iter()
            .map(|k| json!({ "key": k.key, "command": k.command }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

/// Dispatch one agent request. Returns the reply line (already valid JSON),
/// or None when `cmd` is not an agent command (the caller falls through to
/// the legacy native commands).
pub fn handle(
    shell: &mut Shell,
    cmd: &str,
    arg: &str,
    cx: &mut gpui::Context<Shell>,
) -> Option<String> {
    let reply = match cmd {
        "help" | "get_state" | "page_geometry" | "click" | "type" | "key" | "wheel"
        | "screenshot" | "open" | "scroll_to" | "overlay" | "toast" | "config.get" => {
            dispatch(shell, cmd, arg, cx)
        }
        _ => return None,
    };
    Some(match reply {
        Ok(v) => v.to_string(),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    })
}

fn dispatch(
    shell: &mut Shell,
    cmd: &str,
    arg: &str,
    cx: &mut gpui::Context<Shell>,
) -> Result<Value, String> {
    match cmd {
        "help" => Ok(serde_json::from_str(&help_json(shell)).unwrap_or_default()),
        "get_state" => Ok(serde_json::from_str(&agent_state_json(shell)).unwrap_or_default()),
        "config.get" => Ok(serde_json::from_str(&config_json(shell)).unwrap_or_default()),
        "page_geometry" => page_geometry(shell, arg),
        "click" => {
            let mut it = arg.split_whitespace();
            let x = it
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| "click needs x y".to_string())?;
            let y = it
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| "click needs x y".to_string())?;
            let sel = it.next().unwrap_or("active");
            let button = it.next().unwrap_or("left");
            click(shell, x, y, sel, button, cx)
        }
        "type" => {
            // arg = "text..." with an optional trailing " @sel" segment (the
            // explicit '@' separator keeps free text with spaces unambiguous).
            let (text, sel) = split_at_sel(arg);
            type_text(shell, text, sel, cx)
        }
        "key" => {
            let (spec, sel) = split_at_sel(arg);
            press_key(shell, spec.trim(), sel, cx)
        }
        "wheel" => {
            let mut it = arg.split_whitespace();
            let dx = it
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| "wheel needs dx dy".to_string())?;
            let dy = it
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| "wheel needs dx dy".to_string())?;
            let sel = it.next().unwrap_or("active");
            wheel(shell, dx, dy, sel, cx)
        }
        "screenshot" => screenshot(shell, arg),
        "open" => open_url(shell, arg, cx),
        "scroll_to" => scroll_to(shell, arg),
        "overlay" => overlay(shell, arg, cx),
        "toast" => {
            shell.toast(arg.to_string());
            Ok(json!({ "ok": true }))
        }
        _ => unreachable!("is_agent_command guarantees a known command"),
    }
}

/// Split "text @sel" -> (text, sel). Absent separator -> (whole, "active").
/// The '@' marker keeps free text with spaces unambiguous.
fn split_at_sel(arg: &str) -> (&str, &str) {
    match arg.split_once(" @") {
        Some((text, sel)) => (text, sel),
        None => (arg, "active"),
    }
}
