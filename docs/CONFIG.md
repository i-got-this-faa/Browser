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

