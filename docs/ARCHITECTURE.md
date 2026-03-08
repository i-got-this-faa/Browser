# Strip Browser — Architecture

A programmable, Niri-inspired browser shell. The Chromium engine (Google
Chrome today, Helium/Chromium source later) renders web content headless;
a GPUI Rust application is the entire visible browser. Everything about
the experience — layout, keys, commands, hooks — is defined by one Lua
file: `~/.config/strip-browser/browser.lua`.

## Data flow (strict direction)

```
browser.lua ──load──▶ LuaHost ──Request──▶ ops::apply ──Effects──▶ EngineController
    ▲                                        │                         │
    └── snapshot (tabs/active) ◀─────────────┘                         ▼
                                                        WebView trait ──▶ Chromium (CDP)
                                                                │
                                                PNG frames ◀────┘
```

- Scripts never touch GPUI or Chromium. They return `Request` values; the
  shell executes them.
- The UI pushes a `BrowserSnapshot` into Lua before each command/hook call,
  so scripts read `browser.tabs`, `browser.active_page`,
  `browser.active_workspace` fresh each time.
- `ops::apply` is engine-free and unit-tested without Chrome.

