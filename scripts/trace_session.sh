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
