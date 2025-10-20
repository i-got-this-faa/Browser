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

// ---------------------------------------------------------------------------
// Typed config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub bg: String,
    pub bar: String,
    pub bar_text: String,
    pub border: String,
    pub border_focus: String,
    pub prompt_bg: String,
    pub prompt_text: String,
    pub accent: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: "#101014".into(),
            bar: "#0c0c10".into(),
            bar_text: "#9aa0b0".into(),
            border: "#2a2c3a".into(),
            border_focus: "#7aa2f7".into(),
            prompt_bg: "#1c1d27".into(),
            prompt_text: "#e6e9f0".into(),
            accent: "#7aa2f7".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
