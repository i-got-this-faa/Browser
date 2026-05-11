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
pub fn default_keys() -> Vec<Keybind> {
    vec![
        kb("ctrl+h", "focus.left"),
        kb("ctrl+l", "focus.right"),
        kb("ctrl+alt+h", "focus.left"),
        kb("ctrl+alt+l", "focus.right"),
        kb("ctrl+shift+h", "page.move_left"),
        kb("ctrl+shift+l", "page.move_right"),
        kb("ctrl+t", "page.new"),
        kb("ctrl+shift+t", "page.new_beside"),
        kb("ctrl+w", "page.close"),
        kb("ctrl+r", "page.reload"),
        kb("ctrl+i", "focus.url"),
        kb("ctrl+shift+r", "page.reload_bypass_cache"),
        kb("alt+left", "page.back"),
        kb("alt+right", "page.forward"),
        kb("ctrl+pagedown", "page.next"),
        kb("ctrl+pageup", "page.prev"),
        kb_arg("ctrl+1", "workspace.focus", "1"),
        kb_arg("ctrl+2", "workspace.focus", "2"),
        kb_arg("ctrl+3", "workspace.focus", "3"),
        kb_arg("ctrl+4", "workspace.focus", "4"),
        kb_arg("ctrl+shift+1", "page.to_workspace", "1"),
        kb_arg("ctrl+shift+2", "page.to_workspace", "2"),
        kb_arg("ctrl+shift+3", "page.to_workspace", "3"),
        kb_arg("ctrl+shift+4", "page.to_workspace", "4"),
        kb("ctrl+n", "workspace.new"),
        kb("ctrl+shift+n", "workspace.next"),
        kb("ctrl+o", "overview.toggle"),
        kb("ctrl+shift+o", "overview.toggle"),
        kb("ctrl+shift+p", "palette.open"),
        kb("ctrl+p", "palette.open"),
        kb("ctrl+shift+e", "config.reload"),
        kb("ctrl+q", "app.quit"),
    ]
}

fn kb(key: &str, command: &str) -> Keybind {
    Keybind { key: key.into(), command: command.into(), arg: None }
}

fn kb_arg(key: &str, command: &str, arg: &str) -> Keybind {
    Keybind { key: key.into(), command: command.into(), arg: Some(arg.into()) }
}

// ---------------------------------------------------------------------------
// Lua parsing
// ---------------------------------------------------------------------------

impl Config {
    /// Parse a config file. Errors name the problem; unknown fields and
    /// missing optional fields fall back to defaults.
    pub fn load(path: &Path) -> Result<Self> {
        let src = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let mut cfg = Self::parse(&src).with_context(|| format!("in {}", path.display()))?;
        cfg.source_path = path.to_path_buf();
        Ok(cfg)
    }

    pub fn parse(src: &str) -> Result<Self> {
        let lua = Lua::new();
        let value: Value = lua
            .load(src)
            .set_name("browser.lua")
            .eval()
            .context("browser.lua did not run; syntax or runtime error")?;
        let Value::Table(t) = value else {
            return Err(anyhow!("browser.lua must return a table"));
        };

        let mut cfg = Config::default();
        if let Some(theme) = nested_table(&t, "theme")? {
            cfg.theme.bg = get_str_or(&theme, "bg", &cfg.theme.bg);
            cfg.theme.bar = get_str_or(&theme, "bar", &cfg.theme.bar);
            cfg.theme.bar_text = get_str_or(&theme, "bar_text", &cfg.theme.bar_text);
            cfg.theme.border = get_str_or(&theme, "border", &cfg.theme.border);
            cfg.theme.border_focus = get_str_or(&theme, "border_focus", &cfg.theme.border_focus);
            cfg.theme.prompt_bg = get_str_or(&theme, "prompt_bg", &cfg.theme.prompt_bg);
            cfg.theme.prompt_text = get_str_or(&theme, "prompt_text", &cfg.theme.prompt_text);
            cfg.theme.accent = get_str_or(&theme, "accent", &cfg.theme.accent);
        }
        if let Some(b) = nested_table(&t, "behavior")? {
            cfg.behavior.page_width_fraction = get_num_or(&b, "page_width_fraction", cfg.behavior.page_width_fraction)
                .clamp(0.2, 1.0);
            cfg.behavior.gap = get_num_or(&b, "gap", cfg.behavior.gap).max(0.0);
            cfg.behavior.home_page = get_str_or(&b, "home_page", &cfg.behavior.home_page);
            cfg.behavior.search_engine_url =
                get_str_or(&b, "search_engine_url", &cfg.behavior.search_engine_url);
            cfg.behavior.smooth_scroll = get_num_or(&b, "smooth_scroll", cfg.behavior.smooth_scroll)
                .clamp(0.05, 1.0);
            cfg.behavior.show_page_bar = get_bool_or(&b, "show_page_bar", cfg.behavior.show_page_bar);
            cfg.behavior.show_status_bar = get_bool_or(&b, "show_status_bar", cfg.behavior.show_status_bar);
        }
        if let Some(keys) = nested_table(&t, "keys")? {
            cfg.keys = parse_keys(&keys)?;
        }
        if let Some(commands) = nested_table(&t, "commands")? {
            cfg.commands = parse_commands(&commands)?;
        }
        if let Some(hooks) = nested_table(&t, "events")? {
            cfg.hooks = parse_hooks(&hooks)?;
        }
        Ok(cfg)
    }

    /// Look up the handler for an event, if the config declared one.
    pub fn hook(&self, event: &str) -> Option<&Function> {
        self.hooks.iter().find(|h| h.event == event).map(|h| &h.handler)
    }
}

fn nested_table(root: &Table, key: &str) -> Result<Option<Table>> {
    let v: Value = root.raw_get(key)?;
    match v {
        Value::Table(t) => Ok(Some(t)),
        Value::Nil => Ok(None),
        other => Err(anyhow!("field `{key}` should be a table, got {}", other.type_name())),
    }
}

fn get_str_or(t: &Table, key: &str, default: &str) -> String {
    let s: Option<String> = t.raw_get(key).ok().flatten();
    s.unwrap_or_else(|| default.to_string())
}

fn get_num_or(t: &Table, key: &str, default: f32) -> f32 {
    let v: Option<f64> = t.raw_get(key).ok().flatten();
    v.map(|v| v as f32).unwrap_or(default)
}

fn get_bool_or(t: &Table, key: &str, default: bool) -> bool {
    let v: Option<bool> = t.raw_get(key).ok().flatten();
    v.unwrap_or(default)
}

fn parse_keys(keys: &Table) -> Result<Vec<Keybind>> {
    let mut out = Vec::new();
    for entry in keys.sequence_values::<Value>() {
        let Value::Table(t) = entry.context("keys entries must be tables")? else {
            return Err(anyhow!("keys entries must be tables like {{ \"ctrl+t\", \"page.new\" }}"));
        };
        // Shorthand: { "ctrl+t", "page.new" } or { "ctrl+1", "workspace.focus", arg = "1" }
        let first: Value = t.raw_get(1)?;
        let Value::String(s) = first else {
            return Err(anyhow!("keys entry missing key string at position 1"));
        };
        let key = s.to_str()?.to_string();
        let second: Value = t.raw_get(2)?;
        let Value::String(s) = second else {
            return Err(anyhow!("keys entry {key} missing command string at position 2"));
        };
        let command = s.to_str()?.to_string();
        let arg: Option<String> = t.raw_get("arg").ok().flatten();
        out.push(Keybind { key, command, arg });
    }
    Ok(out)
}

fn parse_commands(commands: &Table) -> Result<Vec<LuaCommand>> {
    let mut out = Vec::new();
    for pair in commands.pairs::<String, Value>() {
        let (name, value) = pair?;
        let Value::Table(t) = value else {
            return Err(anyhow!("command `{name}` must be a table with desc and run fields"));
        };
        let description = get_str_or(&t, "desc", "");
        let run: Function = t
            .raw_get("run")
            .map_err(|_| anyhow!("command `{name}` is missing its run function"))?;
        out.push(LuaCommand { name, description, run });
    }
    Ok(out)
}

fn parse_hooks(hooks: &Table) -> Result<Vec<LuaHook>> {
    let mut out = Vec::new();
    for pair in hooks.pairs::<String, Function>() {
        let (event, handler) = pair?;
        out.push(LuaHook { event, handler });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Paths and hot reload
// ---------------------------------------------------------------------------

pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(DEFAULT_CONFIG_DIR)
}

pub fn config_path() -> PathBuf {
    config_dir().join(DEFAULT_CONFIG_FILE)
}

/// Write the embedded default config to `path` if absent. Returns true when created.
pub fn ensure_default_config(path: &Path, default_src: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, default_src)?;
    Ok(true)
}

/// Watch a config file and send a reload signal after changes settle.
/// The sender receives one message per settled change.
