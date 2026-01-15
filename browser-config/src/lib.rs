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
pub struct Behavior {
    /// Fraction of the viewport occupied by one page.
    pub page_width_fraction: f32,
    /// Gap between pages, virtual px.
    pub gap: f32,
    /// URL for `page.new` and first launch.
    pub home_page: String,
    /// Search URL with a `{}` placeholder for the query.
    pub search_engine_url: String,
    /// Smooth-scroll fraction per frame (higher = faster).
    pub smooth_scroll: f32,
    pub show_page_bar: bool,
    pub show_status_bar: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            page_width_fraction: 0.78,
            gap: 12.0,
            home_page: "https://duckduckgo.com".into(),
            search_engine_url: "https://duckduckgo.com/?q={}".into(),
            smooth_scroll: 0.18,
            show_page_bar: true,
            show_status_bar: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keybind {
    pub key: String,
    pub command: String,
    #[serde(default)]
    pub arg: Option<String>,
}

/// A command declared from Lua: name, description, and the Lua function.
#[derive(Debug, Clone)]
pub struct LuaCommand {
    pub name: String,
    pub description: String,
    pub run: Function,
}

/// A lifecycle hook declared from Lua.
#[derive(Debug, Clone)]
pub struct LuaHook {
    pub event: String,
    pub handler: Function,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub theme: Theme,
    pub behavior: Behavior,
    pub keys: Vec<Keybind>,
    pub commands: Vec<LuaCommand>,
    pub hooks: Vec<LuaHook>,
    pub source_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            behavior: Behavior::default(),
            keys: default_keys(),
            commands: Vec::new(),
            hooks: Vec::new(),
            source_path: PathBuf::new(),
        }
    }
}

/// Default bindings. Every one of them can be replaced from browser.lua.
