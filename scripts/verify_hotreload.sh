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

