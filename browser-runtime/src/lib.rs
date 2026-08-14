//! browser-runtime: Lua -> runtime API -> UI/state -> web engine glue.
//!
//! Strict direction of flow: scripts never touch GPUI or Chromium. They
//! return [`Request`] values (from commands, hooks, or `browser.request`)
//! and the UI executes them against [`BrowserState`]. See `ops::apply`.

pub mod bridge;
pub mod ops;
pub mod state;

use anyhow::Result;
use mlua::{Lua, LuaSerdeExt, MultiValue, Table, Value};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub use bridge::{BrowserSnapshot, HostEvent, TabInfo};
pub use mlua::Value as LuaValue;
pub use state::{BrowserState, PageSlot};

// ---------------------------------------------------------------------------
pub enum Request {
    /// Navigate the active page (or open a first page).
    Navigate(String),
    /// Focus a URL: input goes to the prompt, not straight to the engine.
    FocusUrl,
    Reload,
    ReloadBypassCache,
    Back,
    Forward,
    PageNew,
    PageNewBeside,
    PageClose,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    PageMoveLeft,
    PageMoveRight,
    PageNext,
    PagePrev,
    WorkspaceNew,
    WorkspaceNext,
    WorkspacePrev,
    /// Focus workspace n (1-based).
    WorkspaceFocus(u32),
    /// Send the active page to workspace n (1-based).
    PageToWorkspace(u32),
    OverviewToggle,
    ScrollLeft,
    ScrollRight,
    OpenPalette,
    ConfigReload,
    Quit,
    /// User typed text into the prompt and pressed enter.
    PromptSubmit(String),
    /// Run a registered command by name, with an optional string arg.
    RunCommand { name: String, arg: Option<String> },
    /// Evaluate a Lua chunk (REPL/palette use).
    ExecLua(String),
}

impl Request {
    /// The command name used by keybinds and the palette.
    pub fn name(&self) -> &'static str {
        match self {
            Request::Navigate(_) => "page.navigate",
            Request::FocusUrl => "focus.url",
            Request::Reload => "page.reload",
            Request::ReloadBypassCache => "page.reload_bypass_cache",
            Request::Back => "page.back",
            Request::Forward => "page.forward",
            Request::PageNew => "page.new",
            Request::PageNewBeside => "page.new_beside",
            Request::PageClose => "page.close",
            Request::FocusLeft => "focus.left",
            Request::FocusRight => "focus.right",
            Request::FocusUp => "focus.up",
            Request::FocusDown => "focus.down",
            Request::PageMoveLeft => "page.move_left",
            Request::PageMoveRight => "page.move_right",
            Request::PageNext => "page.next",
            Request::PagePrev => "page.prev",
            Request::WorkspaceNew => "workspace.new",
            Request::WorkspaceNext => "workspace.next",
            Request::WorkspacePrev => "workspace.prev",
            Request::WorkspaceFocus(_) => "workspace.focus",
            Request::PageToWorkspace(_) => "page.to_workspace",
            Request::OverviewToggle => "overview.toggle",
            Request::ScrollLeft => "layout.scroll_left",
            Request::ScrollRight => "layout.scroll_right",
            Request::OpenPalette => "palette.open",
            Request::ConfigReload => "config.reload",
            Request::Quit => "app.quit",
            Request::PromptSubmit(_) => "prompt.submit",
            Request::RunCommand { .. } => "command.run",
            Request::ExecLua(_) => "lua.exec",
        }
    }

    /// Build a request from command name + optional arg (keybind/palette path).
    pub fn from_command(name: &str, arg: Option<&str>) -> Option<Request> {
        let r = match name {
            "focus.url" => Request::FocusUrl,
            "page.reload" => Request::Reload,
            "page.reload_bypass_cache" => Request::ReloadBypassCache,
            "page.back" => Request::Back,
            "page.forward" => Request::Forward,
            "page.new" => Request::PageNew,
            "page.new_beside" => Request::PageNewBeside,
            "page.close" => Request::PageClose,
            "focus.left" => Request::FocusLeft,
            "focus.right" => Request::FocusRight,
            "focus.up" => Request::FocusUp,
            "focus.down" => Request::FocusDown,
            "page.move_left" => Request::PageMoveLeft,
            "page.move_right" => Request::PageMoveRight,
            "page.next" => Request::PageNext,
            "page.prev" => Request::PagePrev,
            "workspace.new" => Request::WorkspaceNew,
            "workspace.next" => Request::WorkspaceNext,
            "workspace.prev" => Request::WorkspacePrev,
            "workspace.focus" => Request::WorkspaceFocus(arg?.parse().ok()?),
            "page.to_workspace" => Request::PageToWorkspace(arg?.parse().ok()?),
            "overview.toggle" => Request::OverviewToggle,
            "layout.scroll_left" => Request::ScrollLeft,
            "layout.scroll_right" => Request::ScrollRight,
            "palette.open" => Request::OpenPalette,
            "config.reload" => Request::ConfigReload,
            "app.quit" => Request::Quit,
            "page.navigate" => Request::Navigate(arg?.to_string()),
            _ => return None,
        };
        Some(r)
    }

    /// All built-in command names, for the palette and docs.
    pub fn all_commands() -> &'static [(&'static str, &'static str)] {
        &[
            ("focus.url", "Focus the address prompt"),
            ("page.reload", "Reload the active page"),
            ("page.reload_bypass_cache", "Reload ignoring cache"),
            ("page.back", "Back in history"),
            ("page.forward", "Forward in history"),
            ("page.new", "New page"),
            ("page.new_beside", "New page beside the active one"),
            ("page.close", "Close the active page"),
            ("focus.left", "Focus the page to the left"),
            ("focus.right", "Focus the page to the right"),
            ("focus.up", "Focus the workspace above"),
            ("focus.down", "Focus the workspace below"),
            ("page.move_left", "Move the page one slot left"),
            ("page.move_right", "Move the page one slot right"),
            ("page.next", "Focus the next page on the strip"),
            ("page.prev", "Focus the previous page on the strip"),
            ("workspace.new", "Create a workspace"),
            ("workspace.next", "Focus the next workspace"),
            ("workspace.prev", "Focus the previous workspace"),
            ("workspace.focus", "Focus workspace n"),
            ("page.to_workspace", "Send the page to workspace n"),
            ("overview.toggle", "Toggle the workspace overview"),
            ("layout.scroll_left", "Scroll the strip left"),
            ("layout.scroll_right", "Scroll the strip right"),
            ("palette.open", "Open the command palette"),
            ("config.reload", "Reload browser.lua"),
            ("app.quit", "Quit the browser"),
        ]
    }
}

// ---------------------------------------------------------------------------
// Lua host
// ---------------------------------------------------------------------------

/// Runs browser.lua and translates table-shaped requests into [`Request`]s.
///
/// Scripts describe *what* they want (`{ "page", "new_beside" }` or
/// `browser.request("page.new")`); the shell decides *how*. Before each
/// command/hook call the UI refreshes `browser.tabs` /
/// `browser.active_page` / `browser.active_workspace` from the snapshot, so
/// scripts always read fresh state.
pub struct LuaHost {
    lua: Lua,
    /// Requests queued by `browser.request` during the most recent call.
    pending: Arc<Mutex<Vec<Request>>>,
}

impl Default for LuaHost {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaHost {
    pub fn new() -> Self {
        Self { lua: Lua::new(), pending: Arc::default() }
    }

    fn bind_browser_table(&self) -> Result<()> {
        let lua = &self.lua;
        let pending2 = self.pending.clone();

        let request = lua.create_function(move |_, req: Value| {
            if let Some(r) = lua_value_to_request(req).ok().flatten() {
                pending2.lock().unwrap().push(r);
            }
            Ok(())
        })?;

        let log = lua.create_function(|_, msg: String| {
            eprintln!("[browser.lua] {msg}");
            Ok(())
        })?;

        let browser = lua.create_table()?;
        browser.set("request", request)?;
        browser.set("log", log)?;
        lua.globals().set("browser", browser)?;
        Ok(())
    }

    /// Take requests queued by `browser.request` since the last drain.
    fn drain_pending(&mut self) -> Vec<Request> {
        self.pending.lock().unwrap().drain(..).collect()
    }

    /// Parse `browser.lua`, bind the `browser` API, and run it. The file
    /// returns a config table (theme/behavior/keys/commands/events).
    /// Requests emitted at load time land in `out`.
    pub fn load_config(&mut self, src: &str, out: &mut Vec<Request>) -> Result<()> {
        self.bind_browser_table()?;
        let value: Value = self
            .lua
            .load(src)
            .set_name("browser.lua")
            .eval()
            .map_err(|e| anyhow::anyhow!("browser.lua did not run: {e}"))?;
        out.extend(self.drain_pending());
        // Keep the returned table reachable as CONFIG so call_command and
        // call_hook can find commands/events later.
        if let Value::Table(t) = value {
            self.lua.globals().set("CONFIG", t)?;
        }
        Ok(())
    }

    /// Read-only access for callers that need serde conversions themselves.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Convert a serde JSON payload into a Lua value for hook calls.
    pub fn json_to_lua(&self, v: &serde_json::Value) -> Result<LuaValue> {
        use mlua::LuaSerdeExt;
        self.lua.to_value(v).map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// Names + descriptions of user-defined commands from CONFIG.commands.
    pub fn command_names(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let cfg: Option<Table> = self.lua.globals().get("CONFIG").ok();
        let Some(cfg) = cfg else { return out };
        let commands: Option<Table> = cfg.get("commands").ok();
        let Some(commands) = commands else { return out };
        for pair in commands.pairs::<String, Table>() {
            if let Ok((name, entry)) = pair {
                let desc: String = entry.get("desc").unwrap_or_default();
                out.push((name, desc));
            }
        }
        out
    }

    /// Push the current snapshot so scripts can read live state.
    pub fn push_snapshot(&self, snap: &BrowserSnapshot) -> Result<()> {
        let __t0 = std::time::Instant::now();
        let globals = self.lua.globals();
        let browser: Table = globals.get("browser")?;
        let tabs = self.lua.to_value(snap.tabs.as_slice())?;
        browser.set("tabs", tabs)?;
        browser.set(
            "active_page",
            snap.tabs.iter().find(|t| t.active).map(|t| t.id),
        )?;
        browser.set("active_workspace", snap.active_workspace)?;
        browser_core::perf_event!("lua.snapshot_push",
            "tabs" => snap.tabs.len(),
            "us" => __t0.elapsed().as_micros() as u64);
        Ok(())
    }

    /// Call a named config command's `run` function with an optional arg.
    /// Returns requests the function produced.
    pub fn call_command(&mut self, name: &str, arg: Option<String>) -> Result<Vec<Request>> {
        let __t0 = std::time::Instant::now();
        let globals = self.lua.globals();
        let cfg: Option<Table> = globals.get("CONFIG").ok();
        let Some(cfg) = cfg else {
            anyhow::bail!("no config loaded");
        };
        let commands: Table = cfg.get("commands")?;
        let entry: Option<Table> = commands.get(name).ok();
        let Some(entry) = entry else {
            anyhow::bail!("unknown command `{name}`");
        };
        let run: mlua::Function = entry.get("run")?;
        let ret: MultiValue = match arg {
            Some(a) => run.call((a,))?,
            None => run.call(())?,
        };
        let mut reqs = self.drain_pending();
        reqs.extend(collect_requests(ret));
        browser_core::perf_event!("lua.command", "name" => name,
            "us" => __t0.elapsed().as_micros() as u64);
        Ok(reqs)
    }

    /// Call a lifecycle hook registered under `events[event]`.
    pub fn call_hook(&mut self, event: &str, payload: Value) -> Result<Vec<Request>> {
        let __t0 = std::time::Instant::now();
        let globals = self.lua.globals();
        let cfg: Option<Table> = globals.get("CONFIG").ok();
        let Some(cfg) = cfg else {
            return Ok(vec![]);
        };
        let events: Option<Table> = cfg.get("events").ok();
        let Some(events) = events else {
            return Ok(vec![]);
        };
        let handler: Option<mlua::Function> = events.get(event).ok();
        let Some(handler) = handler else {
            return Ok(vec![]);
        };
        let ret: MultiValue = handler.call((payload,))?;
        let mut reqs = self.drain_pending();
        reqs.extend(collect_requests(ret));
        browser_core::perf_event!("lua.hook", "event" => event,
            "us" => __t0.elapsed().as_micros() as u64);
        Ok(reqs)
    }

    /// Evaluate an arbitrary chunk and collect requests it produces.
    pub fn exec(&mut self, chunk: &str) -> Result<Vec<Request>> {
        let ret: MultiValue = self.lua.load(chunk).set_name("=repl").eval()?;
        let mut reqs = self.drain_pending();
        reqs.extend(collect_requests(ret));
        Ok(reqs)
    }
}

/// Convert whatever a Lua function returned into a request list. Accepts one
/// request table, a list of them, or nil.
fn collect_requests(ret: MultiValue) -> Vec<Request> {
    ret.into_iter().filter_map(|v| lua_value_to_request(v).ok().flatten()).collect()
}

/// Table shapes accepted:
///   { "page", "new_beside" }            positional
///   { cmd = "page.new_beside" }         named
///   { "page.navigate", "https://…" }    with string payload
///   { cmd = "workspace.focus", arg = 2 }
fn lua_value_to_request(v: Value) -> Result<Option<Request>> {
    let Value::Table(t) = v else {
        return Ok(None);
    };
    // Named form.
    let named: Option<String> = t.get("cmd").ok();
    if let Some(cmd) = named {
        let arg: Option<String> = t.get("arg").ok().flatten();
        return Ok(request_with_arg(&cmd, arg.as_deref()));
    }
    // Positional form. `{ "page.navigate", "url" }` passes the payload as
    // the arg; `{ "page", "new_beside" }` joins into "page.new_beside".
    let cmd: Option<String> = t.get(1).ok().flatten();
    let Some(cmd) = cmd else {
        return Ok(None);
    };
    let payload: Option<String> = t.get(2).ok().flatten();
    if let Some(r) = request_with_arg(&cmd, payload.as_deref()) {
        return Ok(Some(r));
    }
    if let Some(p) = payload.as_deref() {
        return Ok(request_with_arg(&format!("{cmd}.{p}"), None));
    }
    Ok(None)
}

fn request_with_arg(cmd: &str, arg: Option<&str>) -> Option<Request> {
    match cmd {
        // Free-string commands take the payload as their arg.
        "page.navigate" => Some(Request::Navigate(arg?.to_string())),
        "lua.exec" => Some(Request::ExecLua(arg?.to_string())),
        _ => Request::from_command(cmd, arg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_with(src: &str) -> LuaHost {
        let mut host = LuaHost::new();
        let mut out = Vec::new();
        host.load_config(src, &mut out).expect("config loads");
        host
    }

    #[test]
    fn command_round_trip() {
        for (name, _) in Request::all_commands() {
            let arg = match *name {
                "workspace.focus" | "page.to_workspace" => Some("2"),
                "page.navigate" => Some("https://x.test"),
                _ => None,
            };
            assert!(
                Request::from_command(name, arg).is_some(),
                "built-in `{name}` must parse"
            );
        }
        assert!(Request::from_command("nope", None).is_none());
        assert_eq!(
            Request::from_command("workspace.focus", Some("3")),
            Some(Request::WorkspaceFocus(3))
        );
        // Missing arg is a None, not a panic.
        assert!(Request::from_command("workspace.focus", None).is_none());
    }

    #[test]
    fn lua_request_positional_and_named() {
        let mut host = LuaHost::new();
        let mut out = Vec::new();
        host.load_config(
            r#"
            browser.request { "page", "new_beside" }
            browser.request { cmd = "workspace.focus", arg = 2 }
            browser.request { "page.navigate", "https://example.com" }
            return {}
        "#,
            &mut out,
        )
        .unwrap();
        assert_eq!(
            out,
            vec![
                Request::PageNewBeside,
                Request::WorkspaceFocus(2),
                Request::Navigate("https://example.com".into()),
            ]
        );
    }

    #[test]
    fn config_commands_run_and_return_requests() {
        let mut host = host_with(
            r#"
            return {
                commands = {
                    ["my.open"] = { desc = "open", run = function()
                        browser.request { "page.navigate", "https://open.test" }
                        return { "page", "new_beside" }
                    end },
                },
            }
        "#,
        );
        let reqs = host.call_command("my.open", None).unwrap();
        assert_eq!(
            reqs,
            vec![
                Request::Navigate("https://open.test".into()),
                Request::PageNewBeside
            ]
        );
    }

    #[test]
    fn hooks_read_snapshot_and_return_requests() {
        let mut host = host_with(
            r#"
            return {
                events = {
                    page_created = function(page)
                        if page.url:find("dev") then
                            return { "page", "close" }
                        end
                    end,
                },
            }
        "#,
        );
        host.push_snapshot(&BrowserSnapshot { tabs: vec![], active_workspace: 1 })
            .unwrap();
        let payload = host
            .lua
            .to_value(&serde_json::json!({ "id": 1, "url": "https://dev.test" }))
            .unwrap();
        let reqs = host.call_hook("page_created", payload).unwrap();
        assert_eq!(reqs, vec![Request::PageClose]);
    }

    #[test]
    fn snapshot_is_readable_from_commands() {
        let mut host = host_with(
            r#"
            return {
                commands = {
                    ["my.count"] = { desc = "c", run = function()
                        return { cmd = "lua.exec", arg = "count=" .. #browser.tabs }
                    end },
                },
            }
        "#,
        );
        host.push_snapshot(&BrowserSnapshot {
            tabs: vec![TabInfo {
                id: 1,
                url: "u".into(),
                title: "t".into(),
                workspace: 1,
                active: true,
            }],
            active_workspace: 1,
        })
        .unwrap();
        let reqs = host.call_command("my.count", None).unwrap();
        assert_eq!(
            reqs,
            vec![Request::ExecLua("count=1".into())],
            "script read #browser.tabs from the snapshot"
        );
    }

    #[test]
    fn unknown_command_errors_cleanly() {
        let mut host = host_with("return {}");
        assert!(host.call_command("nope", None).is_err());
    }

    #[test]
    fn exec_evaluates_chunks() {
        let mut host = host_with("return {}");
        let reqs = host.exec(r#"browser.request { cmd = "overview.toggle" }"#).unwrap();
        assert_eq!(reqs, vec![Request::OverviewToggle]);
    }

    #[test]
    fn broken_config_reports_error() {
        let mut host = LuaHost::new();
        let mut out = Vec::new();
        assert!(host.load_config("return {", &mut out).is_err());
    }
}
