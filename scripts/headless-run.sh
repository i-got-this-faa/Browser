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

