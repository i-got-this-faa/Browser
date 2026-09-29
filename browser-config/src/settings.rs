//! Settings write-back: the GUI edits `browser.lua` without owning it.
//!
//! The settings page owns exactly one generated block, delimited by
//! [`BEGIN`]/[`END`] marker lines, at the top of the file:
//!
//! ```lua
//! -- BEGIN strip-settings (...)
//! STRIP_SETTINGS = { behavior = { gap = 20 }, keys = { { "ctrl+t", "page.new" } } }
//! -- END strip-settings
//! ```
//!
//! Everything outside the block is the user's and is never touched. The
//! loader ([`crate::Config::parse`]) merges `STRIP_SETTINGS` over the table
//! the file returns: `theme`/`behavior` per key, `keys` as a whole list (it
//! replaces the defaults or the user's list, like a plain `keys` table).
//!
//! The editable fields are not listed anywhere by hand: [`fields`] derives
//! them from the serde shape of the typed [`crate::Theme`] and
//! [`crate::Behavior`], so a new config field shows up here by itself.

use crate::{Config, Keybind};
use anyhow::{anyhow, bail, Context, Result};
use mlua::{IntoLua, Lua, Table, Value};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const BEGIN: &str = "-- BEGIN strip-settings";
pub const END: &str = "-- END strip-settings";
/// Lua global the block assigns; the loader reads it after running the file.
pub const GLOBAL: &str = "STRIP_SETTINGS";

/// The two scalar config sections the settings page edits per key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Theme,
    Behavior,
}

impl Section {
    pub fn as_str(self) -> &'static str {
        match self {
            Section::Theme => "theme",
            Section::Behavior => "behavior",
        }
    }
}

/// One config value as it appears in Lua.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Bool(bool),
    Num(f64),
    Str(String),
}

impl Scalar {
    /// A number at f32 precision: every numeric config field is an f32/u32,
    /// so `0.18` stays `0.18` (not `0.18000000715`) and `0.18 + 0.01` stays
    /// `0.19`. Every constructor path goes through here, which also keeps
    /// the written block and the value read back from it equal.
    pub fn num(n: f64) -> Self {
        Scalar::Num((n as f32).to_string().parse().unwrap_or(n))
    }

    /// Text the GUI shows and edits.
    pub fn display(&self) -> String {
        match self {
            Scalar::Bool(b) => b.to_string(),
            Scalar::Num(n) => n.to_string(),
            Scalar::Str(s) => s.clone(),
        }
    }

    fn lua_literal(&self) -> String {
        match self {
            Scalar::Str(s) => lua_string(s),
            other => other.display(),
        }
    }
}

impl IntoLua for Scalar {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            Scalar::Bool(b) => Ok(Value::Boolean(b)),
            Scalar::Num(n) => Ok(Value::Number(n)),
            Scalar::Str(s) => lua.create_string(s).map(Value::String),
        }
    }
}

/// What a field holds; decides the editor and the validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Color,
    Bool,
    Int,
    Float,
    Text,
}

impl Kind {
    fn of(section: Section, v: &serde_json::Value) -> Option<Kind> {
        match v {
            serde_json::Value::Bool(_) => Some(Kind::Bool),
            serde_json::Value::Number(n) if n.is_f64() => Some(Kind::Float),
            serde_json::Value::Number(_) => Some(Kind::Int),
            serde_json::Value::String(_) if section == Section::Theme => Some(Kind::Color),
            serde_json::Value::String(_) => Some(Kind::Text),
            _ => None,
        }
    }

    /// Turn text typed in the GUI into a value of this kind.
    pub fn parse_input(self, text: &str) -> Result<Scalar> {
        let t = text.trim();
        let v = match self {
            Kind::Bool => match t {
                "true" | "on" | "yes" => Scalar::Bool(true),
                "false" | "off" | "no" => Scalar::Bool(false),
                _ => bail!("expected true or false"),
            },
            Kind::Int | Kind::Float => {
                Scalar::num(t.parse::<f64>().map_err(|_| anyhow!("expected a number"))?)
            }
            Kind::Color | Kind::Text => Scalar::Str(t.to_string()),
        };
        self.check(&v)?;
        Ok(v)
    }

    /// Reject values the loader would silently ignore or misread.
    pub fn check(self, v: &Scalar) -> Result<()> {
        match (self, v) {
            (Kind::Bool, Scalar::Bool(_)) => Ok(()),
            (Kind::Float, Scalar::Num(n)) if n.is_finite() => Ok(()),
            (Kind::Int, Scalar::Num(n)) if n.is_finite() && *n >= 0.0 && n.fract() == 0.0 => {
                Ok(())
            }
            (Kind::Int, Scalar::Num(_)) => bail!("expected a whole number, 0 or more"),
            (Kind::Color, Scalar::Str(s)) => {
                let digits = s.strip_prefix('#').unwrap_or("");
                if matches!(digits.len(), 6 | 8) && digits.chars().all(|c| c.is_ascii_hexdigit())
                {
                    Ok(())
                } else {
                    bail!("expected a color like #rrggbb")
                }
            }
            (Kind::Text, Scalar::Str(s)) if !s.trim().is_empty() => Ok(()),
            (Kind::Text, Scalar::Str(_)) => bail!("must not be empty"),
            _ => bail!("wrong type for this setting"),
        }
    }

    /// Amount one nudge (left/right in the GUI) moves a number.
    pub fn step(self, current: f64) -> f64 {
        match self {
            Kind::Float if current.abs() <= 1.0 => 0.01,
            _ => 1.0,
        }
    }
}

/// One editable scalar setting with its effective value.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub section: Section,
    pub key: String,
    pub kind: Kind,
    pub value: Scalar,
}

/// Every scalar setting of `cfg`, in declaration order, derived from the
/// serde shape of the typed sections. A new field on `Theme`/`Behavior`
/// appears here (and in the settings page) with no other change.
pub fn fields(cfg: &Config) -> Vec<Field> {
    let mut out = Vec::new();
    for (section, value) in [
        (Section::Theme, serde_json::to_value(&cfg.theme)),
        (Section::Behavior, serde_json::to_value(&cfg.behavior)),
    ] {
        let Ok(serde_json::Value::Object(map)) = value else { continue };
        for (key, v) in map {
            let Some(kind) = Kind::of(section, &v) else { continue };
            let value = match &v {
                serde_json::Value::Bool(b) => Scalar::Bool(*b),
                serde_json::Value::Number(n) => Scalar::num(n.as_f64().unwrap_or_default()),
                serde_json::Value::String(s) => Scalar::Str(s.clone()),
                _ => continue,
            };
            out.push(Field { section, key, kind, value });
        }
    }
    out
}

/// What the GUI has overridden. Empty means the file has no managed block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overrides {
    pub theme: BTreeMap<String, Scalar>,
    pub behavior: BTreeMap<String, Scalar>,
    /// The complete key list; keys are all-or-nothing like a `keys` table.
    pub keys: Option<Vec<Keybind>>,
}

/// One user action in the settings page.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Set { section: Section, key: String, value: Scalar },
    /// Drop the override so the file (or the default) decides again.
    Reset { section: Section, key: String },
    SetKeys(Vec<Keybind>),
    ResetKeys,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.theme.is_empty() && self.behavior.is_empty() && self.keys.is_none()
    }

    fn section_mut(&mut self, s: Section) -> &mut BTreeMap<String, Scalar> {
        match s {
            Section::Theme => &mut self.theme,
            Section::Behavior => &mut self.behavior,
        }
    }

    pub fn is_set(&self, section: Section, key: &str) -> bool {
        match section {
            Section::Theme => self.theme.contains_key(key),
            Section::Behavior => self.behavior.contains_key(key),
        }
    }

    /// Apply one edit, validating it against the real field list.
    pub fn apply(&mut self, edit: Edit) -> Result<()> {
        match edit {
            Edit::Set { section, key, value } => {
                let value = match value {
                    Scalar::Num(n) => Scalar::num(n),
                    other => other,
                };
                let defaults = fields(&Config::default());
                let field = defaults
                    .iter()
                    .find(|f| f.section == section && f.key == key)
                    .ok_or_else(|| anyhow!("unknown setting {}.{key}", section.as_str()))?;
                field.kind.check(&value).with_context(|| format!("{}.{key}", section.as_str()))?;
                self.section_mut(section).insert(key, value);
            }
            Edit::Reset { section, key } => {
                self.section_mut(section).remove(&key);
            }
            Edit::SetKeys(keys) => {
                if keys.iter().any(|k| k.key.trim().is_empty() || k.command.trim().is_empty()) {
                    bail!("a binding needs a key and a command");
                }
                self.keys = Some(keys);
            }
            Edit::ResetKeys => self.keys = None,
        }
        Ok(())
    }

    /// Read the overrides a script left in the `STRIP_SETTINGS` global.
    pub fn from_lua_global(lua: &Lua) -> Result<Self> {
        let value: Value = lua.globals().get(GLOBAL)?;
        let table = match value {
            Value::Nil => return Ok(Self::default()),
            Value::Table(t) => t,
            other => bail!("{GLOBAL} must be a table, got {}", other.type_name()),
        };
        let mut out = Self::default();
        for pair in table.pairs::<String, Value>() {
            let (name, value) = pair?;
            match (name.as_str(), value) {
                ("theme", Value::Table(t)) => out.theme = scalar_map(&t)?,
                ("behavior", Value::Table(t)) => out.behavior = scalar_map(&t)?,
                ("keys", Value::Table(t)) => out.keys = Some(crate::parse_keys(&t)?),
                (other, _) => bail!("{GLOBAL}.{other} is not a settings section"),
            }
        }
        Ok(out)
    }

    /// Merge over the table the user's file returned (loader side).
    pub fn merge_into(self, lua: &Lua, root: &Table) -> Result<()> {
        for (name, map) in [("theme", self.theme), ("behavior", self.behavior)] {
            if map.is_empty() {
                continue;
            }
            let target = match root.raw_get::<Value>(name)? {
                Value::Table(t) => t,
                Value::Nil => {
                    let t = lua.create_table()?;
                    root.raw_set(name, t.clone())?;
                    t
                }
                other => bail!("field `{name}` should be a table, got {}", other.type_name()),
            };
            for (k, v) in map {
                target.raw_set(k, v)?;
            }
        }
        if let Some(keys) = self.keys {
            let list = lua.create_table()?;
            for k in keys {
                let entry = lua.create_table()?;
                entry.raw_push(k.key)?;
                entry.raw_push(k.command)?;
                if let Some(arg) = k.arg {
                    entry.raw_set("arg", arg)?;
                }
                list.raw_push(entry)?;
            }
            root.raw_set("keys", list)?;
        }
        Ok(())
    }

    /// The generated block, markers included, ending in a newline.
    pub fn to_block(&self) -> String {
        let mut s = String::new();
        s.push_str(BEGIN);
        s.push_str(" (written by the settings page, ctrl+, by default; edit values there.\n");
        s.push_str("-- Only this block is regenerated. Everything outside it is yours.)\n");
        s.push_str(&format!("{GLOBAL} = {{\n"));
        for (name, map) in [("theme", &self.theme), ("behavior", &self.behavior)] {
            if map.is_empty() {
                continue;
            }
            s.push_str(&format!("  {name} = {{\n"));
            for (k, v) in map {
                s.push_str(&format!("    {k} = {},\n", v.lua_literal()));
            }
            s.push_str("  },\n");
        }
        if let Some(keys) = &self.keys {
            s.push_str("  keys = {\n");
            for k in keys {
                s.push_str(&format!("    {{ {}, {}", lua_string(&k.key), lua_string(&k.command)));
                if let Some(arg) = &k.arg {
                    s.push_str(&format!(", arg = {}", lua_string(arg)));
                }
                s.push_str(" },\n");
            }
            s.push_str("  },\n");
        }
        s.push_str("}\n");
        s.push_str(END);
        s.push('\n');
        s
    }
}

fn scalar_map(t: &Table) -> Result<BTreeMap<String, Scalar>> {
    let mut out = BTreeMap::new();
    for pair in t.pairs::<String, Value>() {
        let (k, v) = pair?;
        let scalar = match v {
            Value::Boolean(b) => Scalar::Bool(b),
            Value::Integer(i) => Scalar::num(i as f64),
            Value::Number(n) => Scalar::num(n),
            Value::String(s) => Scalar::Str(s.to_str()?.to_string()),
            other => bail!("{GLOBAL}.{k} must be a bool, number or string, got {}", other.type_name()),
        };
        out.insert(k, scalar);
    }
    Ok(out)
}

/// Quote a string as a Lua literal.
fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Source text: find, read, replace the managed block
// ---------------------------------------------------------------------------

/// Byte range of the block (markers through the END line's newline).
fn block_range(src: &str) -> Result<Option<std::ops::Range<usize>>> {
    let mut start = None;
    let mut offset = 0;
    for line in src.split_inclusive('\n') {
        let end_of_line = offset + line.len();
        let text = line.trim_end();
        if start.is_none() && text.starts_with(BEGIN) {
            start = Some(offset);
        } else if let Some(s) = start {
            if text == END {
                return Ok(Some(s..end_of_line));
            }
        }
        offset = end_of_line;
    }
    match start {
        Some(_) => bail!("browser.lua has `{BEGIN}` without a matching `{END}`; fix or delete it"),
        None => Ok(None),
    }
}

/// The overrides currently stored in `src`'s block (empty when absent).
/// Only the block is evaluated, never the user's code.
pub fn read_overrides(src: &str) -> Result<Overrides> {
    let Some(range) = block_range(src)? else { return Ok(Overrides::default()) };
    let lua = Lua::new();
    lua.load(&src[range]).set_name("strip-settings block").exec().context("settings block")?;
    Overrides::from_lua_global(&lua)
}

/// `src` with its block replaced by one for `o`; a new block goes at the top,
/// and an empty `o` removes the block.
pub fn write_overrides(src: &str, o: &Overrides) -> Result<String> {
    let range = block_range(src)?;
    let block = if o.is_empty() { String::new() } else { o.to_block() };
    Ok(match range {
        Some(r) => {
            let mut out = String::with_capacity(src.len() + block.len());
            out.push_str(&src[..r.start]);
            out.push_str(&block);
            let rest = &src[r.end..];
            // Removing the block also removes the blank line that separated it.
            out.push_str(if block.is_empty() { rest.strip_prefix('\n').unwrap_or(rest) } else { rest });
            out
        }
        None if block.is_empty() => src.to_string(),
        None => format!("{block}\n{src}"),
    })
}

/// Apply `edit` to the config file at `path` and write it back atomically.
///
/// The candidate is parsed by the real loader first, so a write that would
/// leave `browser.lua` unloadable is refused and the file stays as it was.
/// The running shell picks the change up through the normal file watcher.
pub fn apply_edit(path: &Path, edit: Edit) -> Result<()> {
    // Write through a symlink (dotfile setups): a rename onto the link would
    // replace the link with a regular file.
    let real = std::fs::canonicalize(path).with_context(|| format!("resolve {}", path.display()))?;
    let src = std::fs::read_to_string(&real).with_context(|| format!("read {}", real.display()))?;
    let mut overrides = read_overrides(&src)?;
    overrides.apply(edit)?;
    let next = write_overrides(&src, &overrides)?;
    if next == src {
        return Ok(());
    }
    Config::parse(&next).context("this change would make browser.lua fail to load; not written")?;
    if read_overrides(&next)? != overrides {
        bail!("internal error: settings block did not round-trip; not written");
    }
    write_atomic(&real, &next)
}

fn write_atomic(real: &Path, contents: &str) -> Result<()> {
    let name = real.file_name().ok_or_else(|| anyhow!("config path has no file name"))?;
    let tmp = real.with_file_name(format!(".{}.strip-settings.tmp", name.to_string_lossy()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        if let Ok(meta) = std::fs::metadata(real) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, real)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("write {}", real.display()))
}

#[cfg(test)]
mod tests;
