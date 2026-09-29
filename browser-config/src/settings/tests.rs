use super::*;

const USER_FILE: &str = r##"-- my notes: keep this comment
local function shout(s) return s:upper() end

return {
  theme = { accent = "#111111" }, -- user accent
  behavior = { gap = 8 },
  commands = {
    ["my.cmd"] = { desc = "hook stays", run = function() return shout("x") end },
  },
  events = { page_created = function(page) end },
}
"##;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "strip-settings-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn set(section: Section, key: &str, value: Scalar) -> Edit {
    Edit::Set { section, key: key.into(), value }
}

#[test]
fn edit_round_trips_through_the_real_loader_and_keeps_user_code() {
    let dir = temp_dir("roundtrip");
    let path = dir.join("browser.lua");
    std::fs::write(&path, USER_FILE).unwrap();

    apply_edit(&path, set(Section::Behavior, "gap", Scalar::Num(24.0))).unwrap();
    apply_edit(&path, set(Section::Behavior, "show_status_bar", Scalar::Bool(false))).unwrap();
    apply_edit(&path, set(Section::Theme, "bg", Scalar::Str("#010203".into()))).unwrap();
    apply_edit(&path, set(Section::Behavior, "smooth_scroll", Scalar::Num(0.25))).unwrap();

    let out = std::fs::read_to_string(&path).unwrap();
    // Everything outside the block is byte-identical.
    let range = block_range(&out).unwrap().unwrap();
    let outside = format!("{}{}", &out[..range.start], &out[range.end..]);
    assert_eq!(outside.trim_start_matches('\n'), USER_FILE);

    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.behavior.gap, 24.0, "override wins over the user's gap = 8");
    assert!(!cfg.behavior.show_status_bar);
    assert_eq!(cfg.behavior.smooth_scroll, 0.25);
    assert_eq!(cfg.theme.bg, "#010203");
    assert_eq!(cfg.theme.accent, "#111111", "untouched user value survives");
    assert_eq!(cfg.commands[0].name, "my.cmd", "user commands survive");
    assert!(cfg.hook("page_created").is_some(), "user hooks survive");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn reset_removes_the_override_and_then_the_block() {
    let dir = temp_dir("reset");
    let path = dir.join("browser.lua");
    std::fs::write(&path, USER_FILE).unwrap();
    apply_edit(&path, set(Section::Behavior, "gap", Scalar::Num(24.0))).unwrap();
    apply_edit(&path, Edit::Reset { section: Section::Behavior, key: "gap".into() }).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), USER_FILE, "empty block is removed");
    assert_eq!(Config::load(&path).unwrap().behavior.gap, 8.0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn keys_are_replaced_as_a_whole_list_and_round_trip() {
    let dir = temp_dir("keys");
    let path = dir.join("browser.lua");
    std::fs::write(&path, "return {}").unwrap();
    let keys = vec![
        Keybind { key: "ctrl+j".into(), command: "page.new".into(), arg: None },
        Keybind { key: "ctrl+9".into(), command: "workspace.focus".into(), arg: Some("9".into()) },
    ];
    apply_edit(&path, Edit::SetKeys(keys.clone())).unwrap();
    assert_eq!(Config::load(&path).unwrap().keys, keys);
    apply_edit(&path, Edit::ResetKeys).unwrap();
    assert_eq!(Config::load(&path).unwrap().keys, crate::default_keys());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn awkward_strings_survive() {
    let dir = temp_dir("strings");
    let path = dir.join("browser.lua");
    std::fs::write(&path, "return {}").unwrap();
    let url = "https://x.test/?q={}&a=\"b\"\\c";
    apply_edit(&path, set(Section::Behavior, "search_engine_url", Scalar::Str(url.into()))).unwrap();
    assert_eq!(Config::load(&path).unwrap().behavior.search_engine_url, url);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn invalid_values_are_refused_and_file_is_untouched() {
    let dir = temp_dir("invalid");
    let path = dir.join("browser.lua");
    std::fs::write(&path, USER_FILE).unwrap();
    for bad in [
        set(Section::Theme, "bg", Scalar::Str("red".into())),
        set(Section::Behavior, "gap", Scalar::Str("wide".into())),
        set(Section::Behavior, "refresh_rate", Scalar::Num(-1.0)),
        set(Section::Behavior, "nope", Scalar::Bool(true)),
        Edit::SetKeys(vec![Keybind { key: "".into(), command: "x".into(), arg: None }]),
    ] {
        assert!(apply_edit(&path, bad).is_err());
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), USER_FILE);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn broken_user_file_is_not_written() {
    let dir = temp_dir("broken");
    let path = dir.join("browser.lua");
    std::fs::write(&path, "return {").unwrap();
    let err = apply_edit(&path, set(Section::Behavior, "gap", Scalar::Num(1.0))).unwrap_err();
    assert!(format!("{err:#}").contains("not written"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "return {");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn unterminated_block_is_refused() {
    let dir = temp_dir("unterminated");
    let path = dir.join("browser.lua");
    let src = format!("{BEGIN}\nreturn {{}}\n");
    std::fs::write(&path, &src).unwrap();
    assert!(apply_edit(&path, Edit::ResetKeys).is_err());
    assert!(apply_edit(&path, set(Section::Behavior, "gap", Scalar::Num(1.0))).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), src);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn symlinked_config_is_edited_in_place() {
    let dir = temp_dir("symlink");
    let real = dir.join("dotfiles-browser.lua");
    let link = dir.join("browser.lua");
    std::fs::write(&real, USER_FILE).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    apply_edit(&link, set(Section::Behavior, "gap", Scalar::Num(30.0))).unwrap();
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "link kept");
    assert!(std::fs::read_to_string(&real).unwrap().contains("gap = 30"));
    assert!(!dir.join(".dotfiles-browser.lua.strip-settings.tmp").exists(), "temp file renamed away");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn every_config_value_is_an_editable_field() {
    // Adding a top-level Config field breaks this destructuring on purpose:
    // decide whether the settings page edits it.
    let Config { theme, behavior, keys: _, commands: _, hooks: _, source_path: _ } =
        Config::default();
    // Keys have their own editor; commands/hooks are Lua code (see docs).
    let count = |v: serde_json::Value| v.as_object().unwrap().len();
    let expected = count(serde_json::to_value(&theme).unwrap())
        + count(serde_json::to_value(&behavior).unwrap());
    let listed = fields(&Config::default());
    assert_eq!(listed.len(), expected, "a Theme/Behavior field has a type the editor cannot show");
    for name in [
        "gap",
        "smooth_scroll",
        "refresh_rate",
        "home_page",
        "search_engine_url",
        "show_status_bar",
        "show_page_bar",
        "page_width_fraction",
    ] {
        assert!(listed.iter().any(|f| f.section == Section::Behavior && f.key == name), "{name}");
    }
    assert!(listed.iter().filter(|f| f.section == Section::Theme).all(|f| f.kind == Kind::Color));
}

#[test]
fn every_default_value_passes_its_own_validation() {
    for f in fields(&Config::default()) {
        f.kind.check(&f.value).unwrap_or_else(|e| panic!("{}: {e}", f.key));
        let text = f.value.display();
        assert_eq!(f.kind.parse_input(&text).unwrap().display(), text, "{}", f.key);
    }
}
