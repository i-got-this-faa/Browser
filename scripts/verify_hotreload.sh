#!/usr/bin/env bash
# One-shot hot-reload verification: launch, query, edit browser.lua, query
# again. Everything in one process so the session survives the tool call.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/strip-browser.sock"
