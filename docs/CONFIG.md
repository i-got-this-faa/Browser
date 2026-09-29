# browser.lua — configuration reference

File location: `~/.config/strip-browser/browser.lua`. The binary writes
the default file on first launch. Save the file to hot-reload; a broken
file keeps the previous config and shows the error as a toast.

## Top-level shape

```lua
return {
  theme    = { ... },   -- colors, "#rrggbb"
  behavior = { ... },   -- layout + UX switches
  keys     = { ... },   -- { "ctrl+t", "page.new" } bindings
  commands = { ... },   -- user commands for the palette
  events   = { ... },   -- lifecycle hooks
}
```

Missing keys fall back to defaults. `keys = {}` disables every default
binding ( redefine everything or nothing).

## theme

`bg`, `bar`, `bar_text`, `border`, `border_focus`, `prompt_bg`,
`prompt_text`, `accent` — `#rrggbb` strings.

## behavior

| key | default | meaning |
|---|---|---|
| `page_width_fraction` | 0.78 | viewport share of one page (clamped 0.2–1.0) |
| `gap` | 12 | px between pages on the strip |
| `home_page` | duckduckgo.com | first page / `page.new` fallback |
| `search_engine_url` | ddg `?q={}` | `{}` replaced by the query |
| `smooth_scroll` | 0.18 | easing fraction per frame (0.05 slow … 1 instant) |
| `show_page_bar` | true | top chips of open pages |
| `show_status_bar` | true | bottom workspaces + URL bar |

## keys

One binding per entry: `{ "mod+key", "command" }` or
`{ "ctrl+1", "workspace.focus", arg = "1" }`. Same keystroke syntax as
GPUI (`ctrl+alt+shift+key`).

Commands (arg where noted):

```
focus.url            focus.right         page.move_right     palette.open
page.reload          focus.up            page.next           config.reload
page.reload_bypass_cache  focus.down     page.prev           app.quit
page.back            page.new            workspace.new       layout.scroll_left
page.forward         page.new_beside     workspace.next      layout.scroll_right
focus.left           page.close          workspace.prev (arg = n)
                                         workspace.focus (arg = n)
                                         page.to_workspace (arg = n)
                                         overview.toggle
                                         settings.open
```

## Settings page

`ctrl+,` (or `settings.open` from the palette, the prompt, `:settings.open`,
or `{"cmd":"exec","arg":"settings.open"}` on the agent API) opens
`strip://settings`: a page on the strip, drawn by the shell, with no web
view. Typing `strip://settings` in the prompt opens it too. It lists every
`theme` and `behavior` field, and every keybinding:

| key | action |
|---|---|
| `up` `down` `pageup` `pagedown` `home` `end` | select a row (the wheel scrolls) |
| `enter` | toggle a bool; edit a text/number/color; on a keybinding, capture a new key |
| `left` `right` | nudge a number (0.01 for fractions, 1 otherwise), toggle a bool |
| `r` | reset the selected field (drop its override) |
| `e` | edit a binding's command (`workspace.focus 3`, or a Lua command name) |
| `delete` | remove the selected binding |
| `+ add binding` row | press the key, then type its command |
| `esc` | cancel an edit or a key capture |

Clicking a row does what `enter` does; a filled dot marks a value the page
has overridden, with a `reset` button. Unbound chords (`ctrl+w`, ...) still
reach the normal keybindings, so the page closes like any other page.
Inputs are validated (`#rrggbb` colors, whole numbers for `refresh_rate`,
non-empty text, known commands, no duplicate key) and shown as an error in the
page, never written.

The fields are not listed by hand: the page reads the `Theme` and `Behavior`
structs in `browser-config`, so a new config field appears in it, and in
`settings.get`, with no other change. `commands` and `events` are Lua code
and are edited in the file; the page shows their count.

### The settings block

The page never regenerates your file. It owns one block, marked by two lines,
at the top of `browser.lua`:

```lua
-- BEGIN strip-settings (written by the settings page, ...
STRIP_SETTINGS = {
  behavior = { gap = 30, show_status_bar = false },
  keys = {
    { "ctrl+t", "page.new" },
    { "ctrl+j", "page.new_beside" },
  },
}
-- END strip-settings
```

* Everything outside the two marker lines is yours and is left byte for byte
  as it was: comments, `local` helpers, hooks, commands.
* When the file loads, `STRIP_SETTINGS` is merged over the table your file
  returns: `theme` and `behavior` per key (the block wins), and `keys` as a
  whole list, replacing your `keys` (or the defaults), exactly like a plain
  `keys = { ... }` would. Clamps still apply (`page_width_fraction` 0.2 to 1).
  `commands` and `events` are never touched.
* Only changed values are stored. `reset` removes a key from the block, and
  the block disappears when nothing is overridden. Delete the block by hand to
  drop every GUI change.
* Changing keybindings from the page writes the whole list, so later changes
  to the default keys do not reach you until you use "reset keybindings".
* A write parses the result with the real loader first. If your file (or the
  change) would not load, nothing is written and the page shows why. If you
  break the file by hand later, the old config stays active, a toast appears,
  and the page shows the load error above the list.
* Writes are atomic (temp file in the same directory, then rename, keeping the
  permissions) and go through a symlinked `browser.lua` to its target, so a
  dotfiles setup keeps its link. The normal file watcher, which also watches a
  symlink target's directory, applies the change; there is no other apply path.
* Do not put a second `BEGIN strip-settings` line in the file. A `BEGIN`
  without a matching `END` is refused rather than guessed at.

## commands

```lua
commands = {
  ["my.hackernews"] = {
    desc = "Open Hacker News beside this page",
    run = function()
      browser.request { "page.new_beside" }
      return { "page.navigate", "https://news.ycombinator.com" }
    end,
  },
}
```

`run` may return requests or emit them via `browser.request`. Run from
the palette (`ctrl+p`) or the prompt (`:my.hackernews`).

## events

Hooks receive a payload table and may return requests:

```lua
events = {
  page_created = function(page)        -- page = { id, url }
    if page.url:find("^https://x%.com") then
      return { cmd = "page.to_workspace", arg = 3 }
    end
  end,
}
```

Available hooks: `page_created`, `page_closed`, `page_focused`,
`page_navigated`, `page_title_changed`, `workspace_changed`,
`config_reloaded`.

## Reading live state

Before each command/hook call the shell sets:

- `browser.tabs` — list of `{ id, url, title, workspace, active }`
- `browser.active_page` — id or nil
- `browser.active_workspace` — id

Also available: `browser.request { ... }` (emit a request) and
`browser.log("...")` (stderr log).

## Prompt

`ctrl+k` opens the prompt (configurable via `focus.url`):

- `example.com` → navigates
- `two words` → searches via `search_engine_url`
- `:workspace.focus 2` → runs a command
