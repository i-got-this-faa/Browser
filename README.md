# Strip Browser

A programmable, Niri-inspired desktop browser. The web engine is real
Chromium (Helium/Chromium source is the planned backend); everything you
see and touch is a Rust + GPUI shell driven by one Lua file.

- Pages are first-class surfaces on an infinite horizontal strip.
- Opening a page never resizes another page.
- Dynamic workspaces stacked vertically; `ctrl+1..4` to jump.
- Keyboard-first: prompt (`ctrl+k`), palette (`ctrl+p`), no tab bar required.
- The whole experience is `~/.config/strip-browser/browser.lua` — hot reload on save.

