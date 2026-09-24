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
| `webview-cdp` | `WebView` trait + frozen CDP test harness: raw TCP/websocket to a real Chromium. Screencast frames, input dispatch, navigation history. | new (backend targets Chromium/Helium) |
| `webview-cef` + `cef-sys` | The real content backend: CEF via a small C shim. Raw BGRA frames + damage rects; no encoded images. | new |
| `browser-ui` | GPUI shell: strip renderer, prompt, palette, page bar, status bar, overlays, key router, engine pump, control socket, agent API. | new (UI framework is Zed's GPUI, Apache-2.0) |

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

1. **Native Wayland presentation (zero-copy dmabuf, default on Wayland)**:
   - CEF renders with `shared_texture_enabled = 1`.
   - On Linux, CEF delivers native dmabuf file descriptors directly in `OnAcceleratedPaint`.
   - The CEF shim binds the window's parent `wl_surface` from GPUI and creates a `wl_subsurface` for each page.
   - Dmabufs are imported directly into Wayland via `zwp_linux_dmabuf_v1` and scaled to viewport geometry with `wp_viewporter`.
   - Hardware composited directly by the Wayland compositor (e.g. `niri`).
   - Zero GPU→CPU readback, zero CPU memcpys, and zero GPUI texture atlas uploads at 60 fps.
   - Screen capture for agent APIs (`screenshot`) is performed on-demand via dmabuf mapping without per-frame overhead.
2. **Fallback OSR path (CDP harness / non-Wayland)**:
   - The event-driven pump wakes on engine damage and drains `WebViewEvent`s.
   - CEF frames arrive as raw BGRA plus damage rects and are patched into a per-page stable buffer (`Surface`); the frozen CDP harness decodes PNGs instead (`STRIP_ENGINE=cdp`).
   - `render()` publishes each visible page's buffer to a GPUI `RenderImage` once per rendered frame, positioned at its strip geometry.

## Input path

1. GPUI key events → `keystroke_string` (`ctrl+shift+t` form).
2. Overlay first (prompt/palette capture keys), then config keybindings,
   then leftovers forwarded to the page via `Input.dispatchKeyEvent`.
3. Mouse: page hit-test from strip geometry → page-local coordinates →
   `Input.dispatchMouseEvent`; wheel → `mouseWheel`.

## Hot reload

`notify` watches the config directory; the pump debounces by draining all
pending events each frame, re-reads the file, re-parses, re-binds Lua, and
fires `config_reloaded`. A broken file shows a toast and keeps the old
config.

## Workspaces

`Strip` holds workspaces in a vertical stack (dynamic; `ctrl+1..4`,
`workspace.new/next/prev`). Pages live on exactly one workspace's
horizontal strip. `overview.toggle` is wired to state and shows all pages
with their geometry unchanged (see browser.lua keys).
