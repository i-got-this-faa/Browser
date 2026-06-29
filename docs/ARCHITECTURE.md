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

## Reused vs newly implemented

Reused (binary-level, no source fork yet):
- Chromium engine: Blink/V8/Skia/networking/sandbox/site isolation/media/
  WebRTC/WebGL/WebGPU/web compatibility — via the spawned Chrome process.
- Zed's GPUI crate (crates.io `gpui 0.2.2`, Apache-2.0) for the Rust UI:
  windows, elements, focus, actions, text system, rendering.

Newly implemented (this workspace):
- The Niri-inspired strip/workspace model and all its math.
- The Lua configuration/scripting system and its typed boundary.
- The GPUI shell (no traditional browser chrome).
- The CDP driver (websocket framing, screencast, input) in `webview-cdp`.

Deferred Helium work (tracked in decisions.tsv): replacing the spawned
Chrome binary with Helium/Chromium built from source (privacy patches,
uBlock integration). `webview-cdp` already isolates this behind the
`WebView` trait; the Helium backend implements the same trait.

## Frame path

1. Engine pump (16 ms timer in `Shell`) drains `WebViewEvent`s.
2. `Page.startScreencast` PNG frames land in `PageSlot.frame_png`.
3. `render()` positions one `img(ImageSource::Render)` per page at its
   strip geometry; PNG is decoded once to BGRA (`RenderImage` format).

