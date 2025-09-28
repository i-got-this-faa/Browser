//! browser-config: `browser.lua` defines the browser experience.
//!
//! The config file returns one table: theme, behavior, keybinds, custom
//! commands, and event hooks. Rust parses it into strongly typed structs;
//! anything unknown falls back to defaults so a minimal file stays valid.

use anyhow::{anyhow, Context, Result};
use mlua::{Function, Lua, Table, Value};
use notify::{RecommendedWatcher, Watcher};
pub use notify::RecommendedWatcher as WatcherHandle;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

pub const DEFAULT_CONFIG_DIR: &str = ".config/strip-browser";
pub const DEFAULT_CONFIG_FILE: &str = "browser.lua";

