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
                                         settings.open
bookmark.toggle      bookmark.open (arg = url, title, or fragment)
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
