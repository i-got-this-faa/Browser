# Strip Browser — Architecture

A programmable, Niri-inspired browser shell. The Chromium engine (Google
Chrome today, Helium/Chromium source later) renders web content headless;
a GPUI Rust application is the entire visible browser. Everything about
the experience — layout, keys, commands, hooks — is defined by one Lua
file: `~/.config/strip-browser/browser.lua`.

