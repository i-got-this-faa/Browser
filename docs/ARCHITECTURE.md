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
   - The shell places each page with `cef_view_set_geometry`: position
     (`wl_subsurface`), destination size and a source crop
     (`wp_viewport`). Pages are subsurfaces above the window, so the shell clips
     each one to the inner viewport (`browser_layout::clip`) and crops the
     buffer to match; the bars stay visible over the overview's oversize
     workspaces. Position is parent state and size is child state, so while a
     page's geometry changes its subsurface runs synchronized (both land with
     the parent's commit) and returns to desync once it is stable. A page that
     is clipped away or hidden only records frames; it re-attaches its last
     buffer when it shows again.
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
   the engine's mouse event (CEF `SendMouseEvent`, CDP `Input.dispatchMouseEvent`).
4. Wheel: each GPUI wheel event goes to the page at once, with no shell-side
   smoothing. Chromium runs the only scroll animation, as in Chrome. A notch is
   3 lines × 40 px = 120 px. `WebViewCommand::Scroll` uses the DOM sign
   (positive `dy` scrolls down), and the CEF adapter flips it for CEF.

## Hot reload

`notify` watches the config directory; the pump debounces by draining all
pending events each frame, re-reads the file, re-parses, re-binds Lua, and
fires `config_reloaded`. A broken file shows a toast and keeps the old
config.

## Settings page

`strip://settings` is a page on the strip with no web view: `ops::open_settings`
creates or refocuses it, the shell draws it in the page's frame
(`browser-ui/src/settings_page.rs`), and the key router hands it the keyboard
while it is active. Each change is one `browser_config::settings::Edit`
applied to the active `browser.lua` (a marked, generated block; atomic write);
the file watcher reloads it like any other edit and the page re-renders from
the reloaded `Config`. There is no second store and no bridge. See
docs/CONFIG.md for the block format.

## Workspaces

`Strip` holds workspaces in a vertical stack and keeps niri's invariant:
`normalize_workspaces` drops an empty workspace as soon as it is not the
active one and guarantees exactly one empty workspace at the bottom. Pages
live on exactly one workspace's horizontal strip; a workspace remembers its
focused page and horizontal scroll while you are away. Width changes
(`resize_page`, `toggle_maximize`) touch only the target page and slide the
pages to its right, so opening or resizing a page never resizes another.

Geometry is one pure function, `browser_layout::stack_geometries`: every page
of every workspace, given the strip, the active scroll and a `Camera`
(`ws_pos`: fractional workspace index at the center, `overview`: 0..1 zoom
progress). At rest the active workspace fills the viewport and the rest sit
below it; the same function with `overview = 1` gives the overview (zoom
around the center, workspaces stacked with a gap, each strip at its real
relative widths). The shell eases the camera toward its target with the same
`ease_step` curve as the horizontal scroll, so the workspace slide and the
overview zoom animate for free. `hit_test` maps a click to a page.

The overview never resizes a page: webviews keep their real size and only
the picture is scaled (viewport destination on Wayland, the frame div on the
fallback path). Pages of other workspaces are hidden to the engine
(`WasHidden`) unless the slide or the overview brings them on screen;
hidden pages keep running, so background audio keeps playing.
