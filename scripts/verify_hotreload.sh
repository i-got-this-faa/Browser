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

ctl() { printf '%s\n' "$1" | timeout 5 nc -U "$SOCK"; }

# Wait until a state query is actually ANSWERED: the socket file can linger
# from a just-killed instance (ECONNREFUSED) while the new browser is still
# starting CEF.
ready=""
for i in $(seq 1 100); do
  r=$(ctl '{"cmd":"state"}' 2>/dev/null)
  if echo "$r" | grep -q '"ok":true'; then ready=1; break; fi
  sleep 0.2
done
[ -n "$ready" ] || { echo "FAIL: browser never answered"; tail -5 /tmp/hr2.log; exit 1; }
sleep 1

before=$(ctl '{"cmd":"state"}' | python3 -c "import json,sys; print(json.load(sys.stdin)['page_fraction'])")
echo "before edit: page_fraction=$before"

sed -i 's/page_width_fraction *= *0.78/page_width_fraction = 0.55/' "$CFG"
sleep 4
after=$(ctl '{"cmd":"state"}' | python3 -c "import json,sys; print(json.load(sys.stdin)['page_fraction'])")
echo "after  edit: page_fraction=$after"

sed -i 's/page_width_fraction *= *0.55/page_width_fraction = 0.78/' "$CFG"
sleep 1
kill "$PID" 2>/dev/null || true

ok=$(python3 -c "print('yes' if abs($before-0.78)<0.01 and abs($after-0.55)<0.01 else 'no')")
if [ "$ok" = "yes" ]; then
  echo "HOT RELOAD: PASS"
else
  echo "HOT RELOAD: FAIL (before=$before after=$after)"
fi
