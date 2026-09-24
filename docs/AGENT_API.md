# Agent API — driving the browser programmatically

The Strip Browser exposes its **entire surface** over a local control socket so
AI agents can operate it directly — the same dispatch path a human's keys and
mouse take, but without screenshots-of-the-chrome, coordinate guessing, or
"computer use" vision loops.

## Connecting

```bash
SOCK="${XDG_RUNTIME_DIR:-/tmp}/strip-browser.sock"

# one JSON request line per connection, one JSON reply line back
printf '{"cmd":"get_state"}\n' | nc -U "$SOCK"
```

`{"cmd":"help"}` returns every command — native and typed — with a one-line
description, so an agent can discover the full surface at runtime instead of
reading docs. The reply includes the socket path, so `help` alone is enough
to bootstrap.

## Commands

### Discover & observe

| cmd | arg | returns |
|---|---|---|
| `help` | – | every command + descriptions + socket path |
| `state` | – | compact snapshot (legacy shape, stable) |
| `get_state` | – | **full snapshot**: per-page url/title/workspace/webview/loading + on-screen geometry + viewport + overlay |
| `page_geometry` | `[id\|active\|url-substr]` | one page's on-screen rect + center |
| `config.get` | – | theme + behavior + keybindings as JSON |
| `screenshot` | `[id\|active\|url-substr]` | PNG of the page's latest frame, base64 |

### Act

| cmd | arg | notes |
|---|---|---|
| `open` | `url` | new page on the active workspace, focused, navigated — one round trip |
| `click` | `x y [sel] [left\|middle\|right]` | page-local coords (from `get_state` geometry); focuses the page first, exactly like a real click |
| `type` | `text [@sel]` | keyboard-types printable text |
| `key` | `ctrl+shift+a [@sel]` | one keystroke with modifiers |
| `wheel` | `dx dy [sel]` | scroll wheel at page center; positive dy scrolls down |
| `scroll_to` | `sel` | center the strip on a page (instant) |
| `overlay` | `get\|none\|prompt\|palette` | read/close the shell overlays |
| `toast` | `text` | visible confirmation for humans watching |
| `exec` | any typed command | `page.new_beside`, `focus.left`, `workspace.focus 3`, `overview.toggle`, `config.reload`, `app.quit`, … (`help` lists all) |
| `key` (shell) | `ctrl+k` | synthesize a real keystroke through the shell's key routing |
| `quit` | – | close the browser |

**Selector** `[sel]`: a numeric page id, `active`, or a case-insensitive
substring of url/title within the active workspace — so `click 10 10 github`
just works in one round trip. `type`/`key` use a trailing ` @sel` separator
(`"hello world @github"`) to stay unambiguous with free text.

## Canonical agent loop

```bash
SOCK="${XDG_RUNTIME_DIR:-/tmp}/strip-browser.sock"
ctl() { printf '{"cmd":"%s","arg":%s}\n' "$1" "$2" | timeout 8 nc -U "$SOCK"; }

# 1. open a page
ctl open '"example.com"'

# 2. observe — geometry arrives with state; no screenshot needed
state=$(ctl get_state -)           # or: '{"cmd":"get_state"}'
echo "$state" | python3 -m json.tool

# 3. act inside the page, using page-local coords from geometry
ctl click '"400 300"'
ctl type '"hello world"'
ctl key '"Return"'
```

## Design notes

* **No second browser.** Every command performs the same engine calls the
  human input path performs (same focus/hidden/resize bookkeeping) or reads
  the same state the renderer draws from.
* **Coordinates are page-local.** `get_state` returns each page's geometry;
  there is nothing to eyeball. The DOM-inside-the-page story depends on the
  backend: the CDP harness can evaluate JS, the CEF backend currently
  cannot — so the API is coordinate-based and works identically on both.
* **The pump is event-driven.** Commands kick the frame pump directly, so a
  reply is computed immediately even on a fully idle shell (measured 3ms).
* **UI-thread safety.** All network reads happen on the acceptor thread; a
  silent client can never stall rendering (verified by
  `scripts/verify_nofreeze.sh`).

## Verification

`scripts/verify_agent_api.sh` drives a live browser through every command
above (23 checks) and exits non-zero on the first regression.
