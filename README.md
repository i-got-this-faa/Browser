# Strip Browser

A programmable, Niri-inspired desktop browser. The web engine is real
Chromium (Helium/Chromium source is the planned backend); everything you
see and touch is a Rust + GPUI shell driven by one Lua file.

- Pages are first-class surfaces on an infinite horizontal strip.
- Opening a page never resizes another page.
- Dynamic workspaces stacked vertically; `ctrl+1..4` to jump.
- Keyboard-first: prompt (`ctrl+k`), palette (`ctrl+p`), no tab bar required.
- The whole experience is `~/.config/strip-browser/browser.lua` — hot reload on save.

## Run

```sh
cargo run -p browser-ui --release
```

Requires a Chromium-family browser on PATH (`google-chrome`, `chromium`,
`chromium-browser`). The engine spawns headless; the GPUI window is the
browser. Linux/Wayland first (niri-tested), X11 works via gpui's x11
backend.

## Default keys

| key | action |
|---|---|
| `ctrl+t` / `ctrl+shift+t` | new page / new page beside |
| `ctrl+w` | close page |
| `ctrl+h` / `ctrl+l` | focus left / right |
| `ctrl+shift+h` / `ctrl+shift+l` | move page left / right |
| `ctrl+n` / `ctrl+shift+n` | next / previous page |
| `alt+left` / `alt+right` | back / forward |
| `ctrl+r` / `ctrl+shift+r` | reload / reload bypassing cache |
| `ctrl+k` | prompt: URL, search, or `:command` |
| `ctrl+p` | command palette |
| `ctrl+1..4` | focus workspace |
| `ctrl+shift+1..2` | send page to workspace |
| `ctrl+o` | overview |
| `ctrl+shift+e` | reload browser.lua |
| `ctrl+q` | quit |

All redefinable in `browser.lua` (`keys = {}` disables every default).

