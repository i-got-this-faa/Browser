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
| `home_page` | duckduckgo.com | first page / `page.new` fallback (resolved like typed text) |
| `search_engine_url` | ddg `?q={}` | `{}` replaced by the percent-encoded query |
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
bookmark.toggle      bookmark.open (arg = url, title, or fragment)
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

## Prompt and addresses

`ctrl+k` opens the prompt (configurable via `focus.url`):

- `example.com` → navigates
- `two words` → searches via `search_engine_url`
- `:workspace.focus 2` → runs a command
- typing also lists up to 5 bookmarks whose title or URL match; `down` / `up`
  highlight one, `enter` opens it. With nothing highlighted, `enter` submits
  the typed text.

### How typed text becomes a URL

The same rules apply to the prompt, the agent `open` command, the
`page.navigate` request (Lua commands, hooks, `:page.navigate …`), and
`home_page`. They live in one function, `browser_runtime::address::resolve`.

1. Full URLs pass through unchanged: `http://`, `https://`, `file://`,
   `chrome://`, `about:`, `data:`, `view-source:`.
2. Text with spaces is a search.
3. Otherwise the text is a URL when the part before the first `/ ? #` is
   - `localhost`, `*.localhost`, an IPv4 address, or a bracketed IPv6
     address (`[::1]`), each with an optional `:port` → `http://` is added;
   - a hostname with an explicit `:port` (`myserver:3000`) → `https://`;
   - a domain: two or more labels whose last label is at least two letters
     (`google.com`, `example.org/path`, `bücher.de`) → `https://`.
4. Everything else is a search: `youtube`, `rust gpui`, `1.5`,
   `me@example.com`, `site:example.com`.

There is no TLD list, since one goes stale as new TLDs ship. The cost is that
file-like names such as `main.rs` or `notes.txt` load as hosts; prefix them
with `https://` or search for them with a second word.

## Bookmarks

Stored in `~/.config/strip-browser/bookmarks.json`, a JSON array of
`{ "url", "title" }`, newest first, one entry per URL. Writes are atomic
(temp file, then rename). A malformed file is moved to
`bookmarks.json.corrupt` and the browser starts with an empty list.

- `ctrl+d` (`bookmark.toggle`) adds the active page or removes its bookmark.
  Blank and `about:blank` pages are skipped. A `★` in the status bar marks a
  bookmarked page.
- `bookmark.open` with an arg opens the bookmark with that exact URL, else
  the newest one whose title or URL contains the arg, in the active page.
- The palette (`ctrl+p`) lists bookmarks once you type: `bookmark` lists all,
  other text matches title or URL. `enter` opens the row.
- From Lua: `browser.request { "bookmark.toggle" }`,
  `browser.request { "bookmark.open", "rust" }`.

`keys` in an existing `browser.lua` replaces the defaults, so add
`{ "ctrl+d", "bookmark.toggle" }` to a config written before this feature.
