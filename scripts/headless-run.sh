#!/usr/bin/env bash
# Headless verification harness: cage (headless wlroots) -> nested niri ->
# Strip Browser. Screenshots via grim against the headless cage output.
# Drives the browser through its control socket (same dispatch path as keys).
#
# Usage: scripts/headless-run.sh
set -euo pipefail

CAGE="$HOME/cage-root/root/usr/bin/cage"
export LD_LIBRARY_PATH="$HOME/cage-root/root/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export WLR_BACKENDS=headless
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER_ALLOW_SOFTWARE=1
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/xdg-harness}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# Unique per-run paths (no collisions with real sessions).
RUN_DIR="$(mktemp -d /tmp/strip-harness-XXXXXX)"
export NIRI_CONFIG="$RUN_DIR/niri.kdl"
export STRIP_BROWSER_SOCK="$XDG_RUNTIME_DIR/strip-browser.sock"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BROWSER_BIN="$ROOT/target/debug/browser"
SHOTS="$RUN_DIR/shots"
mkdir -p "$SHOTS"

# Minimal niri config: zero animations (deterministic shots), no gaps.
cat > "$NIRI_CONFIG" <<'KDL'
animations {
    off
}
layout {
    gaps 0
}
KDL

cleanup() {
  jobs -p | xargs -r kill 2>/dev/null || true
  pkill -f "target/debug/browser" 2>/dev/null || true
  pkill -f "niri -c" 2>/dev/null || true
}
trap cleanup EXIT

echo "== starting cage (headless) =="
"$CAGE" -s "bash -c 'sleep infinity'" >/dev/null 2>&1 &
CAGE_PID=$!
sleep 2

# Cage's wayland socket: newest socket in XDG_RUNTIME_DIR (no .lock files).
CAGE_DISPLAY="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' -printf '%f\n' | sort -V | tail -1)"
export WAYLAND_DISPLAY="$CAGE_DISPLAY"
echo "== cage socket: $CAGE_DISPLAY =="
CAGE_STAMP=$(stat -c %Y "$XDG_RUNTIME_DIR/$CAGE_DISPLAY")

echo "== starting nested niri =="
niri -c "$NIRI_CONFIG" >"$RUN_DIR/niri.log" 2>&1 &
NIRI_PID=$!

# Nested niri creates its own wayland socket: wait for one newer than cage's.
NESTED_DISPLAY=""
for i in $(seq 1 50); do
  NESTED_DISPLAY="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' -newer "$XDG_RUNTIME_DIR/$CAGE_DISPLAY" -printf '%f\n' | sort -V | tail -1)"
  [ -n "$NESTED_DISPLAY" ] && break
  sleep 0.2
done
if [ -z "$NESTED_DISPLAY" ]; then
  echo "nested niri never created a socket; log:"
  cat "$RUN_DIR/niri.log" || true
  exit 1
fi
echo "== niri socket: $NESTED_DISPLAY =="

echo "== launching browser inside nested niri =="
(
  export WAYLAND_DISPLAY="$NESTED_DISPLAY"
  export STRIP_BROWSER_SOCK
  exec env -u LD_LIBRARY_PATH \
    RUST_LOG=info \
    "$BROWSER_BIN"
) >"$RUN_DIR/browser.log" 2>&1 &
BROWSER_PID=$!

# Wait for the control socket.
