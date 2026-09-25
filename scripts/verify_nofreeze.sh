#!/usr/bin/env bash
# Verify a silent control-socket client cannot freeze the browser: connect,
# send nothing, and confirm the socket still answers a second client.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/strip-browser.sock"

pkill -x browser 2>/dev/null || true
sleep 1

env STRIP_TRACE=/tmp/trace-freeze.jsonl "$ROOT/target/debug/browser" >/tmp/freeze.log 2>&1 &
PID=$!
# Wait until a state query is actually ANSWERED (the socket file can linger
# from a just-killed instance while the new browser starts CEF).
ready=""
for i in $(seq 1 100); do
  r=$(printf '{"cmd":"state"}\n' | timeout 1 nc -U "$SOCK" 2>/dev/null)
  if echo "$r" | grep -q '"ok":true'; then ready=1; break; fi
  sleep 0.2
done
[ -n "$ready" ] || { echo "FAIL: browser never answered"; exit 1; }
sleep 1

# Silent client: holds the connection open without sending a line.
python3 - "$SOCK" <<'PYEOF' &
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
time.sleep(8)   # hold it open silently while the browser must keep serving
s.close()
PYEOF
SILENT=$!

sleep 1
start=$(date +%s.%N)
reply=$(printf '{"cmd":"state"}\n' | timeout 5 nc -U "$SOCK")
end=$(date +%s.%N)
latency=$(python3 -c "print(f'{($end-$start)*1000:.0f}')")
wait $SILENT 2>/dev/null || true

sleep 1
reply2=$(printf '{"cmd":"state"}\n' | timeout 5 nc -U "$SOCK")
kill "$PID" 2>/dev/null || true

if echo "$reply" | grep -q '"ok":true' && echo "$reply2" | grep -q '"ok":true'; then
  echo "NO-FREEZE: PASS (state answered in ${latency}ms during silent client)"
else
  echo "NO-FREEZE: FAIL (reply1=$reply reply2=$reply2)"
fi
