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

## Crate map

| Crate | Role | Reused vs new |
|---|---|---|
| `browser-core` | `Page`/`Workspace`/`Strip` model: insert-beside, move, close, neighbor-focus. Pages never resize each other. | new |
| `browser-layout` | Pure viewport math: centering, smooth scroll step, per-frame geometries, focus factor. | new |
| `browser-config` | Parse `browser.lua` into typed `Config`; hot-reload watcher; default keybindings. | new |
| `browser-runtime` | `LuaHost` (mlua, `send`+`serialize`), `Request` catalog, `BrowserState`, ops, snapshot/events bridge. | new |
| `webview-cdp` | `WebView` trait + first backend: CDP over raw TCP/websocket to a real Chromium. Screencast frames → PNG; input dispatch; navigation history. | new (backend targets Chromium/Helium) |
| `browser-ui` | GPUI shell: strip renderer, prompt, palette, page bar, status bar, overlays, key router, engine pump. | new (UI framework is Zed's GPUI, Apache-2.0) |

