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
