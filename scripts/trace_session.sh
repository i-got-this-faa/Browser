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
case "$RUN" in
  standard)
    ctl '{"cmd":"exec","arg":"page.new_beside"}' >/dev/null || true
    sleep 2
    ctl '{"cmd":"exec","arg":"focus.left"}' >/dev/null || true
    sleep 2
    ctl '{"cmd":"exec","arg":"overview.toggle"}' >/dev/null || true
    sleep 2
    ctl '{"cmd":"exec","arg":"overview.toggle"}' >/dev/null || true
    sleep 2
    ;;
  idle)
    ;;  # pure idle baseline: nothing but the frame pump
  idle25)
    sleep 22
    ;;
  navigate)
    ctl '{"cmd":"prompt","arg":"example.com"}' >/dev/null || true
    sleep 6
    ctl '{"cmd":"exec","arg":"page.new_beside"}' >/dev/null || true
    sleep 2
    ;;
  burst)
    # Multi-page interaction: spawn pages, flip focus, toggle overview.
    ctl '{"cmd":"exec","arg":"page.new_beside"}' >/dev/null || true
    sleep 2
    ctl '{"cmd":"exec","arg":"page.new_beside"}' >/dev/null || true
    sleep 2
    ctl '{"cmd":"exec","arg":"focus.left"}' >/dev/null || true
    ctl '{"cmd":"exec","arg":"focus.right"}' >/dev/null || true
    ctl '{"cmd":"exec","arg":"overview.toggle"}' >/dev/null || true
    sleep 2
    ctl '{"cmd":"exec","arg":"overview.toggle"}' >/dev/null || true
    sleep 2
    ;;
  verify)
    # Functional E2E: navigate, open a second page, report state.
    ctl '{"cmd":"prompt","arg":"example.com"}'
    sleep 5
    ctl '{"cmd":"exec","arg":"page.new_beside"}' >/dev/null || true
    sleep 2
    ctl '{"cmd":"state"}'
    ;;
  *)
    echo "unknown run: $RUN"; exit 1
    ;;
esac

# Give the final frames a moment to land, then stop.
sleep 1
kill "$BROWSER_PID" 2>/dev/null || true
wait "$BROWSER_PID" 2>/dev/null || true
trap - EXIT

python3 "$ROOT/scripts/trace_report.py" "$TRACE" 18
