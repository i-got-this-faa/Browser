#!/usr/bin/env bash
# Launch the browser under STRIP_TRACE, drive it through the control socket,
# then stop it and print the aggregated trace report.
#
# Usage: scripts/trace_session.sh <trace.jsonl> [drive_fn]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TRACE="${1:-/tmp/strip-trace.jsonl}"
RUN="${2:-standard}"
SOCK="${STRIP_BROWSER_SOCK:-${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/}strip-browser.sock}"
SOCK="${SOCK:-/tmp/strip-browser.sock}"
LOG="${TRACE}.log"

rm -f "$TRACE" "$TRACE".log "$SOCK"

STRIP_TRACE="$TRACE" RUST_LOG=warn "$ROOT/target/debug/browser" >"$LOG" 2>&1 &
BROWSER_PID=$!

cleanup() {
  kill "$BROWSER_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Wait for the control socket (CEF spawn can take a few seconds).
for i in $(seq 1 150); do
  [ -S "$SOCK" ] && break
  if ! kill -0 "$BROWSER_PID" 2>/dev/null; then
    echo "browser died during startup:"; tail -20 "$LOG"; exit 1
  fi
  sleep 0.2
done
[ -S "$SOCK" ] || { echo "no control socket; log:"; tail -20 "$LOG"; exit 1; }

ctl() { printf '%s\n' "$1" | timeout 10 nc -U "$SOCK"; }

# Let it settle, then drive the requested scenario.
sleep 3
