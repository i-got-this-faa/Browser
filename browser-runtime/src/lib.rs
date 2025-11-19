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

