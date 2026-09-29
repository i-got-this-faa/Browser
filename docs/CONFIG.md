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
| `width_presets` | `{ 1/3, 1/2, 2/3 }` | viewport shares `page.width_preset` cycles through (0.1-1.0, sorted) |
| `overview_scale` | 0.5 | zoom of the overview (0.2-0.9) |
| `overview_gap` | 48 | px between workspaces in the overview (unzoomed) |

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
focus.left           page.close          workspace.prev
                                         workspace.focus (arg = n)
                                         page.to_workspace (arg = n)
                                         page.to_workspace_up / _down
                                         page.mute_toggle
                                         page.width_preset
                                         page.width_decrease / _increase
                                         page.maximize
                                         overview.toggle
```

Workspaces are a vertical stack that follows niri: there is always exactly
one empty workspace at the bottom, and an empty workspace disappears as soon
as you leave it. `focus.up` / `focus.down` (aliases `workspace.prev` /
`workspace.next`) stop at the ends; `workspace.new` focuses the empty one at
the bottom; `workspace.focus n` and `page.to_workspace n` count from the top
and clamp to the last workspace. `page.to_workspace_up` / `_down` move the
page to the end of the neighbor's strip and the view follows it;
`page.to_workspace n` leaves focus where it was (so an `events` hook can
file pages away silently).

Resizing only ever changes the target page; the pages to its right slide to
close or make room. `page.width_preset` moves to the next wider preset
(wrapping); `page.width_decrease` / `page.width_increase` change the width by
10% of the viewport (kept between 10% and 100%); `page.maximize` toggles the
full viewport width and back. Presets count the gaps: two `0.5` pages, or
three `1/3` pages, fill the viewport exactly.

Default bindings that avoid existing ones: `ctrl+r` stays reload, so presets
are `ctrl+alt+r`; `ctrl+minus` / `ctrl+equal` stay free for page zoom, so
width is `ctrl+alt+-` / `ctrl+alt+=`; `ctrl+pageup` / `ctrl+pagedown` moved
from previous/next page (still `ctrl+shift+n` / `ctrl+n`) to workspaces.

Overview (`overview.toggle`): the whole desktop zooms out to `overview_scale`
with every workspace stacked vertically, each strip at its real column
widths and positions, live page contents, the focused page ringed in
`theme.accent`. Inside it `h` `j` `k` `l` / arrows move focus across pages
and workspaces, `enter` / `escape` leave, a click on a page focuses it and
leaves (a click on an empty workspace switches to it), and the wheel moves
between workspaces. Other keys still run their configured bindings; opening
a new page also leaves the overview.

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
`page_navigated`, `page_title_changed`, `page_audio_changed`
(`{ id, playing }`), `workspace_changed`, `config_reloaded`.

## Reading live state

Before each command/hook call the shell sets:

- `browser.tabs` — list of `{ id, url, title, workspace, active, audio_playing, muted }`
  (pages of the active workspace)
- `browser.active_page` — id or nil
- `browser.active_workspace` — id

Also available: `browser.request { ... }` (emit a request) and
`browser.log("...")` (stderr log).

## Audio

Pages play sound through the system output like any Chromium browser. A page
counts as playing while it has a live audio stream (CEF 154 reports stream
start and stop, not audibility, so a silent stream counts). The page bar
shows a note for a playing page and a struck-through dim note for a muted
one. Pages on other workspaces keep playing. `page.mute_toggle` (`ctrl+m`)
mutes or unmutes the active page.

## Prompt

`ctrl+k` opens the prompt (configurable via `focus.url`):

- `example.com` → navigates
- `two words` → searches via `search_engine_url`
- `:workspace.focus 2` → runs a command
