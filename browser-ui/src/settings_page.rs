//! The settings page: a native page on the strip at `strip://settings`.
//!
//! It is drawn by the shell (GPUI) in the page's frame, like the palette is,
//! so it needs no web view, no message bridge and no second config store.
//! Every change becomes one [`Edit`] applied to the active `browser.lua` by
//! `browser_config::settings::apply_edit`; the normal file watcher then
//! reloads the config and the page re-renders from the reloaded `Config`.
//!
//! The rows are derived from the typed config (`settings::fields`), so a new
//! `Theme`/`Behavior` field appears here with no change to this file.
//!
//! [`SettingsPage::on_key`] is the whole interaction model and is unit
//! tested without GPUI; the `impl Shell` block at the bottom adapts it.

use crate::Shell;
use browser_config::settings::{self, Edit, Field, Kind, Overrides, Scalar};
use browser_config::{config_path, Config, Keybind};
use browser_runtime::{Request, SETTINGS_URL};
use gpui::{div, prelude::*, px, Context, MouseButton, SharedString};

const ROW_H: f32 = 28.0;
/// Title + status area above the list, hint line below it.
const TOP_H: f32 = 64.0;
const FOOT_H: f32 = 30.0;

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Row {
    Header(&'static str),
    Field(Field),
    /// Index into `Config::keys`.
    Key(usize),
    AddKey,
    /// Only listed while the block overrides `keys`.
    ResetKeys,
    Note(String),
}

impl Row {
    fn selectable(&self) -> bool {
        !matches!(self, Row::Header(_) | Row::Note(_))
    }
}

fn rows(cfg: &Config, ov: &Overrides) -> Vec<Row> {
    let all = settings::fields(cfg);
    let mut out = vec![Row::Header("Theme")];
    out.extend(all.iter().filter(|f| f.section == settings::Section::Theme).cloned().map(Row::Field));
    out.push(Row::Header("Behavior"));
    out.extend(all.iter().filter(|f| f.section == settings::Section::Behavior).cloned().map(Row::Field));
    out.push(Row::Header("Keybindings"));
    out.extend((0..cfg.keys.len()).map(Row::Key));
    out.push(Row::AddKey);
    if ov.keys.is_some() {
        out.push(Row::ResetKeys);
    }
    out.push(Row::Header("Scripting"));
    out.push(Row::Note(format!(
        "{} commands and {} event hooks are Lua code: edit them in browser.lua",
        cfg.commands.len(),
        cfg.hooks.len()
    )));
    out
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum EditTarget {
    Field(settings::Section, String, Kind),
    /// Change the command of `cfg.keys[i]`.
    KeyCommand(usize),
    /// Command for a key just captured with "add binding".
    NewKeyCommand(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CaptureTarget {
    Rebind(usize),
    New,
}

#[derive(Debug, Clone, Default, PartialEq)]
enum Mode {
    #[default]
    Browse,
    Edit(EditTarget, String),
    Capture(CaptureTarget),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub error: bool,
    pub text: String,
}

/// What the shell must do after a key: swallow it, and maybe write an edit.
#[derive(Debug, Default, PartialEq)]
pub struct KeyOutcome {
    pub consumed: bool,
    pub edit: Option<Edit>,
}

impl KeyOutcome {
    fn eaten() -> Self {
        Self { consumed: true, edit: None }
    }
    fn edit(e: Edit) -> Self {
        Self { consumed: true, edit: Some(e) }
    }
}

#[derive(Default)]
pub struct SettingsPage {
    /// The block's current contents, re-read on every config reload.
    overrides: Overrides,
    selected: usize,
    mode: Mode,
    scroll: f32,
    view_h: f32,
    content_h: f32,
    status: Option<Status>,
    /// Why the last reload of browser.lua failed, if it did.
    load_error: Option<String>,
}

impl SettingsPage {
    /// A reload finished: refresh what the block holds and the error state.
    pub fn on_reload(&mut self, src: &str, error: Option<String>) {
        self.overrides = settings::read_overrides(src).unwrap_or_default();
        if error.is_none() && self.status.as_ref().is_some_and(|s| !s.error) {
            self.status = Some(Status { error: false, text: "applied".into() });
        }
        self.load_error = error;
    }

    pub fn set_status(&mut self, error: bool, text: impl Into<String>) {
        self.status = Some(Status { error, text: text.into() });
    }

    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Browse => "browse",
            Mode::Edit(..) => "edit",
            Mode::Capture(_) => "capture",
        }
    }

    pub fn scroll_by(&mut self, dy: f32) {
        self.scroll = (self.scroll + dy).clamp(0.0, self.max_scroll());
    }

    fn max_scroll(&self) -> f32 {
        (self.content_h - (self.view_h - TOP_H - FOOT_H)).max(0.0)
    }

    /// Called from render with the page frame height: keeps the selection on
    /// a selectable row and the scroll inside the content.
    fn layout(&mut self, view_h: f32, cfg: &Config) {
        let rows = rows(cfg, &self.overrides);
        self.view_h = view_h;
        self.content_h = rows.len() as f32 * ROW_H;
        if !rows.get(self.selected).is_some_and(Row::selectable) {
            self.selected = rows.iter().position(Row::selectable).unwrap_or(0);
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn ensure_visible(&mut self) {
        let list_h = (self.view_h - TOP_H - FOOT_H).max(ROW_H);
        let top = self.selected as f32 * ROW_H;
        if top < self.scroll {
            self.scroll = top;
        } else if top + ROW_H > self.scroll + list_h {
            self.scroll = top + ROW_H - list_h;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn step_selection(&mut self, rows: &[Row], delta: i32) {
        let mut i = self.selected as i32;
        let mut remaining = delta.abs();
        let dir = delta.signum();
        while remaining > 0 {
            let mut j = i + dir;
            while j >= 0 && (j as usize) < rows.len() && !rows[j as usize].selectable() {
                j += dir;
            }
            if j < 0 || j as usize >= rows.len() {
                break;
            }
            i = j;
            remaining -= 1;
        }
        self.selected = i as usize;
    }

    /// Handle one keystroke. `binding` is the config-style key string
    /// (`ctrl+shift+t`); `typed` is the text the key produces, if any.
    pub fn on_key(&mut self, binding: &str, typed: Option<&str>, cfg: &Config) -> KeyOutcome {
        self.status = None;
        let out = match std::mem::take(&mut self.mode) {
            Mode::Browse => self.browse_key(binding, cfg),
            Mode::Edit(target, buf) => self.edit_key(target, buf, binding, typed, cfg),
            Mode::Capture(target) => self.capture_key(target, binding, cfg),
        };
        self.ensure_visible();
        out
    }

    /// Insert pasted text into the active edit buffer.
    pub fn paste(&mut self, text: &str) {
        if let Mode::Edit(_, buf) = &mut self.mode {
            buf.push_str(text.lines().next().unwrap_or(""));
        }
    }

    /// A click on row `i`: select it and do what Enter does. Any open edit
    /// or capture is cancelled first.
    pub fn click(&mut self, i: usize, cfg: &Config) -> KeyOutcome {
        self.mode = Mode::Browse;
        self.status = None;
        let rows = rows(cfg, &self.overrides);
        if !rows.get(i).is_some_and(Row::selectable) {
            return KeyOutcome::eaten();
        }
        self.selected = i;
        self.activate(&rows, cfg)
    }

    /// Remove key row `i` (the "x" button).
    pub fn remove_key(&mut self, i: usize, cfg: &Config) -> KeyOutcome {
        self.mode = Mode::Browse;
        self.status = None;
        KeyOutcome::edit(Edit::SetKeys(without(&cfg.keys, i)))
    }

    /// Reset field `i` to the file/default value (the "reset" button).
    pub fn reset_field(&mut self, i: usize, cfg: &Config) -> KeyOutcome {
        self.mode = Mode::Browse;
        self.status = None;
        match rows(cfg, &self.overrides).get(i) {
            Some(Row::Field(f)) => {
                KeyOutcome::edit(Edit::Reset { section: f.section, key: f.key.clone() })
            }
            _ => KeyOutcome::eaten(),
        }
    }

    fn browse_key(&mut self, binding: &str, cfg: &Config) -> KeyOutcome {
        let rows = rows(cfg, &self.overrides);
        match binding {
            "up" => self.step_selection(&rows, -1),
            "down" => self.step_selection(&rows, 1),
            "pageup" => self.step_selection(&rows, -10),
            "pagedown" => self.step_selection(&rows, 10),
            "home" => self.step_selection(&rows, -(rows.len() as i32)),
            "end" => self.step_selection(&rows, rows.len() as i32),
            "enter" => return self.activate(&rows, cfg),
            "left" | "right" => {
                let dir = if binding == "right" { 1.0 } else { -1.0 };
                if let Some(Row::Field(f)) = rows.get(self.selected) {
                    return match nudged(f, dir) {
                        Some(value) => KeyOutcome::edit(Edit::Set {
                            section: f.section,
                            key: f.key.clone(),
                            value,
                        }),
                        None => KeyOutcome::eaten(),
                    };
                }
            }
            "r" => {
                if let Some(Row::Field(f)) = rows.get(self.selected) {
                    if self.overrides.is_set(f.section, &f.key) {
                        return KeyOutcome::edit(Edit::Reset {
                            section: f.section,
                            key: f.key.clone(),
                        });
                    }
                }
            }
            "delete" | "backspace" => {
                if let Some(Row::Key(i)) = rows.get(self.selected) {
                    return KeyOutcome::edit(Edit::SetKeys(without(&cfg.keys, *i)));
                }
            }
            "e" => {
                if let Some(Row::Key(i)) = rows.get(self.selected) {
                    let k = &cfg.keys[*i];
                    self.mode = Mode::Edit(EditTarget::KeyCommand(*i), command_text(k));
                }
            }
            _ => return KeyOutcome::default(),
        }
        KeyOutcome::eaten()
    }

    fn activate(&mut self, rows: &[Row], cfg: &Config) -> KeyOutcome {
        match rows.get(self.selected) {
            Some(Row::Field(f)) if f.kind == Kind::Bool => match nudged(f, 1.0) {
                Some(value) => {
                    KeyOutcome::edit(Edit::Set { section: f.section, key: f.key.clone(), value })
                }
                None => KeyOutcome::eaten(),
            },
            Some(Row::Field(f)) => {
                self.mode = Mode::Edit(
                    EditTarget::Field(f.section, f.key.clone(), f.kind),
                    f.value.display(),
                );
                KeyOutcome::eaten()
            }
            Some(Row::Key(i)) => {
                self.mode = Mode::Capture(CaptureTarget::Rebind(*i));
                KeyOutcome::eaten()
            }
            Some(Row::AddKey) => {
                self.mode = Mode::Capture(CaptureTarget::New);
                KeyOutcome::eaten()
            }
            Some(Row::ResetKeys) => {
                let _ = cfg;
                KeyOutcome::edit(Edit::ResetKeys)
            }
            _ => KeyOutcome::eaten(),
        }
    }

    fn edit_key(
        &mut self,
        target: EditTarget,
        mut buf: String,
        binding: &str,
        typed: Option<&str>,
        cfg: &Config,
    ) -> KeyOutcome {
        match binding {
            "escape" => return KeyOutcome::eaten(),
            "enter" => match self.commit(&target, &buf, cfg) {
                Ok(edit) => return KeyOutcome::edit(edit),
                Err(e) => self.set_status(true, e),
            },
            "backspace" => {
                buf.pop();
            }
            "space" => buf.push(' '),
            _ => {
                if let Some(t) = typed {
                    buf.push_str(t);
                }
            }
        }
        self.mode = Mode::Edit(target, buf);
        KeyOutcome::eaten()
    }

    fn commit(&self, target: &EditTarget, buf: &str, cfg: &Config) -> Result<Edit, String> {
        match target {
            EditTarget::Field(section, key, kind) => {
                let value = kind.parse_input(buf).map_err(|e| format!("{key}: {e}"))?;
                Ok(Edit::Set { section: *section, key: key.clone(), value })
            }
            EditTarget::KeyCommand(i) => {
                let (command, arg) = parse_command(buf, cfg)?;
                let mut keys = cfg.keys.clone();
                keys[*i].command = command;
                keys[*i].arg = arg;
                Ok(Edit::SetKeys(keys))
            }
            EditTarget::NewKeyCommand(key) => {
                let (command, arg) = parse_command(buf, cfg)?;
                let mut keys = cfg.keys.clone();
                keys.push(Keybind { key: key.clone(), command, arg });
                Ok(Edit::SetKeys(keys))
            }
        }
    }

    fn capture_key(&mut self, target: CaptureTarget, binding: &str, cfg: &Config) -> KeyOutcome {
        if binding == "escape" {
            return KeyOutcome::eaten();
        }
        if let Some(existing) = cfg.keys.iter().find(|k| k.key == binding) {
            self.set_status(true, format!("{binding} is already bound to {}", existing.command));
            self.mode = Mode::Capture(target);
            return KeyOutcome::eaten();
        }
        match target {
            CaptureTarget::Rebind(i) => {
                let mut keys = cfg.keys.clone();
                keys[i].key = binding.to_string();
                KeyOutcome::edit(Edit::SetKeys(keys))
            }
            CaptureTarget::New => {
                self.mode = Mode::Edit(EditTarget::NewKeyCommand(binding.to_string()), String::new());
                KeyOutcome::eaten()
            }
        }
    }
}

fn without(keys: &[Keybind], i: usize) -> Vec<Keybind> {
    let mut keys = keys.to_vec();
    if i < keys.len() {
        keys.remove(i);
    }
    keys
}

fn command_text(k: &Keybind) -> String {
    match &k.arg {
        Some(a) => format!("{} {a}", k.command),
        None => k.command.clone(),
    }
}

/// `name [arg]` -> a command the shell can run, or why it cannot.
fn parse_command(text: &str, cfg: &Config) -> Result<(String, Option<String>), String> {
    let text = text.trim();
    let (name, arg) = match text.split_once(' ') {
        Some((n, a)) => (n, Some(a.trim().to_string()).filter(|a| !a.is_empty())),
        None => (text, None),
    };
    let builtin = Request::from_command(name, arg.as_deref()).is_some();
    let lua = cfg.commands.iter().any(|c| c.name == name);
    if builtin || lua {
        Ok((name.to_string(), arg))
    } else if Request::all_commands().iter().any(|(n, _)| *n == name) {
        Err(format!("{name} needs a valid argument"))
    } else {
        Err(format!("unknown command {name:?} (press ctrl+p for the list)"))
    }
}

/// The value one left/right step away, or None for text/color fields.
fn nudged(f: &Field, dir: f64) -> Option<Scalar> {
    match (&f.kind, &f.value) {
        (Kind::Bool, Scalar::Bool(b)) => Some(Scalar::Bool(!b)),
        (Kind::Int | Kind::Float, Scalar::Num(n)) => {
            let next = n + dir * f.kind.step(*n);
            Some(Scalar::num(if f.kind == Kind::Int { next.max(0.0) } else { next }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shell integration
// ---------------------------------------------------------------------------

impl Shell {
    pub(crate) fn is_settings_page(&self, id: u64) -> bool {
        self.state.strip.page(id).is_some_and(|p| p.url == SETTINGS_URL)
    }

    pub(crate) fn settings_active(&self) -> bool {
        self.state.active_id().is_some_and(|id| self.is_settings_page(id))
    }

    /// Route a keystroke to the settings page when it is the active page.
    /// Returns true when the page consumed it.
    pub(crate) fn settings_route_key(
        &mut self,
        binding: &str,
        ks: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.settings_active() {
            return false;
        }
        if binding == "ctrl+v" && self.settings.mode_name() == "edit" {
            if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                self.settings.paste(&text);
            }
            cx.notify();
            return true;
        }
        let m = &ks.modifiers;
        let typed = if m.control || m.alt || m.platform { None } else { ks.key_char.as_deref() };
        let out = self.settings.on_key(binding, typed, &self.config);
        self.settings_finish(out, cx)
    }

    pub(crate) fn settings_click(&mut self, i: usize, cx: &mut Context<Self>) {
        let out = self.settings.click(i, &self.config);
        self.settings_finish(out, cx);
    }

    pub(crate) fn settings_remove_key(&mut self, i: usize, cx: &mut Context<Self>) {
        let out = self.settings.remove_key(i, &self.config);
        self.settings_finish(out, cx);
    }

    pub(crate) fn settings_reset_field(&mut self, i: usize, cx: &mut Context<Self>) {
        let out = self.settings.reset_field(i, &self.config);
        self.settings_finish(out, cx);
    }

    fn settings_finish(&mut self, out: KeyOutcome, cx: &mut Context<Self>) -> bool {
        if let Some(edit) = out.edit {
            // The outcome is already in the page's status line.
            let _ = self.settings_apply(edit);
        }
        cx.notify();
        out.consumed
    }

    /// Write one edit to the active config file. The file watcher applies it.
    pub(crate) fn settings_apply(&mut self, edit: Edit) -> Result<(), String> {
        match settings::apply_edit(&config_path(), edit) {
            Ok(()) => {
                self.settings.set_status(false, "saved, applying");
                Ok(())
            }
            Err(e) => {
                let msg = format!("{e:#}");
                self.settings.set_status(true, msg.clone());
                Err(msg)
            }
        }
    }

    /// Agent API `settings.get`: every editable field with its effective
    /// value, whether the settings block overrides it, and the last error.
    pub(crate) fn settings_json(&self) -> serde_json::Value {
        let fields: Vec<_> = settings::fields(&self.config)
            .into_iter()
            .map(|f| {
                serde_json::json!({
                    "section": f.section.as_str(),
                    "key": f.key,
                    "kind": format!("{:?}", f.kind).to_lowercase(),
                    "value": f.value,
                    "overridden": self.settings.overrides.is_set(f.section, &f.key),
                })
            })
            .collect();
        serde_json::json!({
            "ok": true,
            "path": config_path().display().to_string(),
            "open": self.state.strip.pages.iter().any(|p| p.url == SETTINGS_URL),
            "mode": self.settings.mode_name(),
            "fields": fields,
            "keys_overridden": self.settings.overrides.keys.is_some(),
            "load_error": self.settings.load_error,
            "status": self.settings.status.as_ref().map(|s| s.text.clone()),
        })
    }

    /// Agent API `settings.set section.key value`: the same edit path the
    /// page uses, so validation and the atomic write are shared.
    pub(crate) fn settings_set(&mut self, arg: &str) -> Result<serde_json::Value, String> {
        const USAGE: &str = "settings.set needs: section.key value";
        let (path, text) = arg.split_once(' ').ok_or(USAGE)?;
        let (section, key) = path.split_once('.').ok_or(USAGE)?;
        let section = match section {
            "theme" => settings::Section::Theme,
            "behavior" => settings::Section::Behavior,
            other => return Err(format!("unknown section {other:?}: theme or behavior")),
        };
        let field = settings::fields(&Config::default())
            .into_iter()
            .find(|f| f.section == section && f.key == key)
            .ok_or_else(|| format!("unknown setting {path}"))?;
        let value = field.kind.parse_input(text).map_err(|e| format!("{path}: {e}"))?;
        self.settings_apply(Edit::Set { section, key: key.into(), value })?;
        Ok(serde_json::json!({ "ok": true, "set": path }))
    }

    /// Called from `reload_lua` after every load attempt.
    pub(crate) fn settings_reloaded(&mut self, error: Option<String>) {
        self.settings.on_reload(&self.lua_source, error);
    }

    /// The page body, drawn inside the page's frame.
    pub(crate) fn render_settings(
        &mut self,
        height: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.settings.layout(height, &self.config);
        let t = &self.config.theme;
        let (bg, text, dim, accent, border) = (
            crate::hex(&t.bar),
            crate::hex(&t.prompt_text),
            crate::hex(&t.bar_text),
            crate::hex(&t.accent),
            crate::hex(&t.border),
        );
        let page = &self.settings;
        let rows = rows(&self.config, &page.overrides);

        let mut list = div().absolute().top(px(-page.scroll)).left(px(0.)).right(px(0.));
        for (i, row) in rows.iter().enumerate() {
            let selected = i == page.selected && row.selectable();
            let base = div().h(px(ROW_H)).flex().flex_row().items_center().gap_3().px_4().text_size(px(13.));
            let base = if selected { base.bg(accent.opacity(0.14)) } else { base };
            let el = match row {
                Row::Header(name) => base.text_color(accent).text_size(px(11.)).child(name.to_uppercase()),
                Row::Note(n) => base.text_color(dim).child(n.clone()),
                Row::Field(f) => {
                    let editing = match (&page.mode, selected) {
                        (Mode::Edit(_, buf), true) => Some(buf.clone()),
                        _ => None,
                    };
                    let overridden = page.overrides.is_set(f.section, &f.key);
                    let shown = f.value.display();
                    let swatch = (f.kind == Kind::Color).then(|| {
                        div().w(px(14.)).h(px(14.)).rounded_sm().border_1().border_color(border).bg(crate::hex(&shown))
                    });
                    let value: SharedString = match editing {
                        Some(buf) => format!("{buf}\u{258f}").into(),
                        None => shown.into(),
                    };
                    let mut r = base
                        .text_color(text)
                        .child(div().w(px(200.)).text_color(dim).child(f.key.clone()))
                        .children(swatch)
                        .child(div().flex_1().child(value))
                        .child(div().w(px(14.)).text_color(accent).child(if overridden { "\u{25cf}" } else { "" }));
                    if overridden {
                        r = r.child(
                            div()
                                .text_color(dim)
                                .text_size(px(11.))
                                .child("reset")
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                                    cx.stop_propagation();
                                    this.settings_reset_field(i, cx);
                                })),
                        );
                    }
                    r
                }
                Row::Key(k) => {
                    let bind = &self.config.keys[*k];
                    let k = *k;
                    let key_text: SharedString = if matches!(page.mode, Mode::Capture(CaptureTarget::Rebind(c)) if c == k) {
                        "press a key\u{2026}".into()
                    } else {
                        bind.key.clone().into()
                    };
                    let command: SharedString = match (&page.mode, selected) {
                        (Mode::Edit(EditTarget::KeyCommand(_), buf), true) => format!("{buf}\u{258f}").into(),
                        _ => command_text(bind).into(),
                    };
                    base.text_color(text)
                        .child(div().w(px(200.)).text_color(dim).child(key_text))
                        .child(div().flex_1().child(command))
                        .child(
                            div()
                                .text_color(dim)
                                .text_size(px(11.))
                                .child("remove")
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                                    cx.stop_propagation();
                                    this.settings_remove_key(k, cx);
                                })),
                        )
                }
                Row::AddKey => {
                    let label: SharedString = match (&page.mode, selected) {
                        (Mode::Capture(CaptureTarget::New), true) => "press the new key\u{2026}".into(),
                        (Mode::Edit(EditTarget::NewKeyCommand(key), buf), true) => {
                            format!("{key}  \u{2192}  {buf}\u{258f}").into()
                        }
                        _ => "+ add binding".into(),
                    };
                    base.text_color(accent).child(label)
                }
                Row::ResetKeys => base.text_color(dim).child("reset keybindings to the file/defaults"),
            };
            let el = if row.selectable() {
                el.on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    this.settings_click(i, cx);
                }))
            } else {
                el
            };
            list = list.child(el);
        }

        let hint = match &page.mode {
            Mode::Browse => "up/down select   enter edit/toggle   left/right adjust   r reset   e edit command   del remove binding",
            Mode::Edit(..) => "type a value   enter save   esc cancel",
            Mode::Capture(_) => "press the key combination to bind   esc cancel",
        };
        let status = page
            .load_error
            .as_ref()
            .map(|e| (true, format!("browser.lua failed to load, keeping the previous config: {e}")))
            .or_else(|| page.status.as_ref().map(|s| (s.error, s.text.clone())));
        let path = config_path().display().to_string();

        div()
            .size_full()
            .bg(bg)
            .text_color(text)
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top(px(0.))
                    .left(px(0.))
                    .right(px(0.))
                    .h(px(TOP_H))
                    .px_4()
                    .pt_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(16.)).child(format!("Settings  \u{00b7}  {path}")))
                    .child(match status {
                        Some((true, s)) => div().text_size(px(12.)).text_color(gpui::red()).child(s),
                        Some((false, s)) => div().text_size(px(12.)).text_color(accent).child(s),
                        None => div().text_size(px(12.)).text_color(dim).child(
                            "changes are written to the marked block at the top of browser.lua",
                        ),
                    }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(TOP_H))
                    .bottom(px(FOOT_H))
                    .left(px(0.))
                    .right(px(0.))
                    .overflow_hidden()
                    .child(list),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(0.))
                    .left(px(0.))
                    .right(px(0.))
                    .h(px(FOOT_H))
                    .px_4()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(border)
                    .text_size(px(11.))
                    .text_color(dim)
                    .child(hint),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_at(cfg: &Config, key: &str) -> SettingsPage {
        let mut p = SettingsPage::default();
        p.layout(600.0, cfg);
        let i = rows(cfg, &p.overrides)
            .iter()
            .position(|r| matches!(r, Row::Field(f) if f.key == key))
            .unwrap();
        p.selected = i;
        p
    }

    fn press(p: &mut SettingsPage, cfg: &Config, keys: &[&str]) -> KeyOutcome {
        let mut out = KeyOutcome::default();
        for k in keys {
            let typed = (k.chars().count() == 1).then_some(*k);
            out = p.on_key(k, typed, cfg);
        }
        out
    }

    #[test]
    fn first_selection_skips_the_header() {
        let cfg = Config::default();
        let mut p = SettingsPage::default();
        p.layout(600.0, &cfg);
        assert!(matches!(rows(&cfg, &p.overrides)[p.selected], Row::Field(_)));
    }

    #[test]
    fn every_config_field_and_key_has_a_row() {
        let cfg = Config::default();
        let r = rows(&cfg, &Overrides::default());
        let fields = r.iter().filter(|r| matches!(r, Row::Field(_))).count();
        assert_eq!(fields, settings::fields(&cfg).len());
        let keys = r.iter().filter(|r| matches!(r, Row::Key(_))).count();
        assert_eq!(keys, cfg.keys.len());
        assert!(!r.contains(&Row::ResetKeys), "no reset row until keys are overridden");
    }

    #[test]
    fn enter_toggles_bools_and_edits_numbers() {
        let cfg = Config::default();
        let mut p = page_at(&cfg, "show_status_bar");
        let out = press(&mut p, &cfg, &["enter"]);
        assert_eq!(
            out.edit,
            Some(Edit::Set {
                section: settings::Section::Behavior,
                key: "show_status_bar".into(),
                value: Scalar::Bool(false)
            })
        );

        let mut p = page_at(&cfg, "gap");
        assert_eq!(press(&mut p, &cfg, &["enter"]).edit, None);
        assert_eq!(p.mode_name(), "edit");
        // The buffer starts as the current value; replace it.
        press(&mut p, &cfg, &["backspace", "backspace", "3", "0"]);
        let out = press(&mut p, &cfg, &["enter"]);
        assert_eq!(
            out.edit,
            Some(Edit::Set {
                section: settings::Section::Behavior,
                key: "gap".into(),
                value: Scalar::Num(30.0)
            })
        );
        assert_eq!(p.mode_name(), "browse");
    }

    #[test]
    fn invalid_input_stays_in_edit_with_an_error() {
        let cfg = Config::default();
        let mut p = page_at(&cfg, "accent");
        press(&mut p, &cfg, &["enter"]);
        for _ in 0..7 {
            press(&mut p, &cfg, &["backspace"]);
        }
        press(&mut p, &cfg, &["r", "e", "d"]);
        let out = press(&mut p, &cfg, &["enter"]);
        assert_eq!(out.edit, None);
        assert_eq!(p.mode_name(), "edit");
        assert!(p.status.as_ref().unwrap().error);
        assert_eq!(press(&mut p, &cfg, &["escape"]).edit, None);
        assert_eq!(p.mode_name(), "browse");
    }

    #[test]
    fn arrows_nudge_numbers_at_a_sensible_step() {
        let cfg = Config::default();
        let mut p = page_at(&cfg, "smooth_scroll");
        let Some(Edit::Set { value, .. }) = press(&mut p, &cfg, &["right"]).edit else { panic!() };
        assert_eq!(value, Scalar::Num(0.19));
        let mut p = page_at(&cfg, "gap");
        let Some(Edit::Set { value, .. }) = press(&mut p, &cfg, &["left"]).edit else { panic!() };
        assert_eq!(value, Scalar::Num(11.0));
        let mut p = page_at(&cfg, "refresh_rate");
        let Some(Edit::Set { value, .. }) = press(&mut p, &cfg, &["left"]).edit else { panic!() };
        assert_eq!(value, Scalar::Num(0.0), "ints stop at zero");
    }

    #[test]
    fn rebind_add_edit_and_remove_keys() {
        let cfg = Config::default();
        let idx = cfg.keys.iter().position(|k| k.key == "ctrl+t").unwrap();
        let select_key = |p: &mut SettingsPage, i: usize| {
            p.selected = rows(&cfg, &p.overrides).iter().position(|r| *r == Row::Key(i)).unwrap();
        };

        // Rebind: capture takes the next keystroke, even ones with bindings elsewhere.
        let mut p = SettingsPage::default();
        p.layout(600.0, &cfg);
        select_key(&mut p, idx);
        press(&mut p, &cfg, &["enter"]);
        assert_eq!(p.mode_name(), "capture");
        let out = p.on_key("ctrl+j", None, &cfg);
        let Some(Edit::SetKeys(keys)) = out.edit else { panic!() };
        assert_eq!(keys[idx].key, "ctrl+j");
        assert_eq!(keys[idx].command, "page.new");
        assert_eq!(keys.len(), cfg.keys.len());

        // A key that is already bound is refused, capture stays open.
        select_key(&mut p, idx);
        press(&mut p, &cfg, &["enter"]);
        assert_eq!(p.on_key("ctrl+w", None, &cfg).edit, None);
        assert_eq!(p.mode_name(), "capture");
        assert!(p.status.as_ref().unwrap().text.contains("already bound"));
        p.on_key("escape", None, &cfg);

        // Add: capture a key, then type a command.
        p.selected = rows(&cfg, &p.overrides).iter().position(|r| *r == Row::AddKey).unwrap();
        press(&mut p, &cfg, &["enter"]);
        p.on_key("ctrl+9", None, &cfg);
        assert_eq!(p.mode_name(), "edit");
        for c in "no.such".chars() {
            p.on_key(&c.to_string(), Some(&c.to_string()), &cfg);
        }
        assert_eq!(press(&mut p, &cfg, &["enter"]).edit, None, "unknown command refused");
        for _ in 0..7 {
            press(&mut p, &cfg, &["backspace"]);
        }
        for c in "workspace.focus".chars() {
            p.on_key(&c.to_string(), Some(&c.to_string()), &cfg);
        }
        assert_eq!(press(&mut p, &cfg, &["enter"]).edit, None, "workspace.focus needs an arg");
        p.on_key("space", None, &cfg);
        p.on_key("9", Some("9"), &cfg);
        let Some(Edit::SetKeys(keys)) = press(&mut p, &cfg, &["enter"]).edit else { panic!() };
        let added = keys.last().unwrap();
        assert_eq!((added.key.as_str(), added.command.as_str(), added.arg.as_deref()),
            ("ctrl+9", "workspace.focus", Some("9")));

        // Remove.
        select_key(&mut p, idx);
        let Some(Edit::SetKeys(keys)) = press(&mut p, &cfg, &["delete"]).edit else { panic!() };
        assert_eq!(keys.len(), cfg.keys.len() - 1);
        assert!(keys.iter().all(|k| k.key != "ctrl+t"));
    }

    #[test]
    fn unhandled_keys_fall_through_to_config_bindings_in_browse_mode() {
        let cfg = Config::default();
        let mut p = SettingsPage::default();
        p.layout(600.0, &cfg);
        assert!(!p.on_key("ctrl+w", None, &cfg).consumed);
        assert!(p.on_key("down", None, &cfg).consumed);
    }

    #[test]
    fn selection_scrolls_into_view() {
        let cfg = Config::default();
        let mut p = SettingsPage::default();
        p.layout(300.0, &cfg);
        press(&mut p, &cfg, &["end"]);
        assert!(p.scroll > 0.0);
        let bottom = p.selected as f32 * ROW_H + ROW_H;
        assert!(bottom <= p.scroll + (300.0 - TOP_H - FOOT_H) + 0.01);
    }
}
