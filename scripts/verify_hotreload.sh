#!/usr/bin/env bash
# One-shot hot-reload verification: launch, query, edit browser.lua, query
# again. Everything in one process so the session survives the tool call.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/strip-browser.sock"
CFG="$HOME/.config/strip-browser/browser.lua"

pkill -f "debug/browse[r]" 2>/dev/null || true
sleep 1

env STRIP_TRACE=/tmp/trace-hr2.jsonl "$ROOT/target/debug/browser" >/tmp/hr2.log 2>&1 &
PID=$!

for i in $(seq 1 100); do [ -S "$SOCK" ] && break; sleep 0.2; done
[ -S "$SOCK" ] || { echo "FAIL: no socket"; tail -5 /tmp/hr2.log; exit 1; }
sleep 3

