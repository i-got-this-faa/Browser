#!/usr/bin/env bash
# Measure idle CPU of the browser main process over ~12s via two ps samples.
# ps 'time' is cumulative CPU, so the delta over the window IS the CPU load.
# Usage: scripts/measure_idle_cpu.sh [binary]
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/debug/browser}"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/strip-browser.sock"

pkill -f "debug/browse[r]" 2>/dev/null || true
sleep 1

"$BIN" >/tmp/cpu.log 2>&1 &
PID=$!
for i in $(seq 1 150); do [ -S "$SOCK" ] && break; sleep 0.2; done
[ -S "$SOCK" ] || { echo "FAIL: no socket"; tail -5 /tmp/cpu.log; exit 1; }
sleep 3   # settle: first page loads, initial frames land

sample() { ps -o time= -p "$1" | awk -F'[:.]' '{print ($1*60)+$2"."$3}'; }

T0=$(sample "$PID")
sleep 12
T1=$(sample "$PID")
kill "$PID" 2>/dev/null || true

python3 -c "t0=$T0; t1=$T1; print(f'idle CPU: {(t1-t0)/12*100:.1f}% of one core (cumulative {t1-t0:.2f}s over 12s)')"
