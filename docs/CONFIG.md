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
```

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
