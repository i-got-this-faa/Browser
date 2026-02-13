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

