#!/usr/bin/env bash
# Verify a silent control-socket client cannot freeze the browser: connect,
# send nothing, and confirm the socket still answers a second client.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/strip-browser.sock"

pkill -f "debug/browse[r]" 2>/dev/null || true
sleep 1

env STRIP_TRACE=/tmp/trace-freeze.jsonl "$ROOT/target/debug/browser" >/tmp/freeze.log 2>&1 &
PID=$!
for i in $(seq 1 100); do [ -S "$SOCK" ] && break; sleep 0.2; done
[ -S "$SOCK" ] || { echo "FAIL: no socket"; exit 1; }
sleep 3

# Silent client: holds the connection open without sending a line.
python3 - "$SOCK" <<'PYEOF' &
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
time.sleep(8)   # hold it open silently while the browser must keep serving
s.close()
PYEOF
