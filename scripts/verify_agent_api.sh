#!/usr/bin/env bash
# E2E verification of the agent API surface: every command, live browser.
# Usage: scripts/verify_agent_api.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/strip-browser.sock"

pkill -x browser 2>/dev/null || true
sleep 1

env STRIP_TRACE=/tmp/trace-agent.jsonl "$ROOT/target/debug/browser" >/tmp/agent-api.log 2>&1 &
BPID=$!

# Wait until a state query is actually ANSWERED (socket file may be stale).
ready=""
for i in $(seq 1 100); do
  r=$(printf '{"cmd":"state"}\n' | timeout 1 nc -U "$SOCK" 2>/dev/null)
  if echo "$r" | grep -q '"ok":true'; then ready=1; break; fi
  sleep 0.2
done
[ -n "$ready" ] || { echo "FAIL: browser never answered"; tail -5 /tmp/agent-api.log; exit 1; }
sleep 2

ctl() { printf '{"cmd":"%s"}\n' "$1" | timeout 8 nc -U "$SOCK"; }
ctl_arg() { printf '{"cmd":"%s","arg":%s}\n' "$1" "$2" | timeout 15 nc -U "$SOCK"; }

pass=0; fail=0
check() { # name, condition-exit-code
  if [ "$2" = "0" ]; then pass=$((pass+1)); echo "  ok: $1";
  else fail=$((fail+1)); echo "  FAIL: $1"; fi
}
has() { echo "$1" | grep -q "$2"; }

echo "== agent api =="
help_out=$(ctl help)
check "help lists get_state"        "$(has "$help_out" get_state; echo $?)"
check "help lists screenshot"       "$(has "$help_out" screenshot; echo $?)"
check "help lists exec commands"    "$(has "$help_out" 'exec page.new_beside'; echo $?)"

st=$(ctl get_state)
check "get_state has viewport"      "$(has "$st" '"viewport"'; echo $?)"
check "get_state has geometry"      "$(has "$st" '"geometry"'; echo $?)"

check "config.get has behavior"     "$(has "$(ctl config.get)" '"behavior"'; echo $?)"
check "overlay get = none"          "$(has "$(ctl_arg overlay '"get"')" '"overlay":"none"'; echo $?)"

open_out=$(ctl_arg open '"example.com"')
check "open returns ok"             "$(has "$open_out" '"ok":true'; echo $?)"
sleep 5   # let the page load and produce frames

st2=$(ctl get_state)
check "open navigated the page"     "$(has "$st2" 'example.com'; echo $?)"
check "page has a webview"          "$(has "$st2" '"webview":true'; echo $?)"

geo=$(ctl page_geometry)
check "page_geometry has rect"      "$(has "$geo" '"w":'; echo $?)"

shot=$(ctl screenshot)
check "screenshot is PNG base64"    "$(has "$shot" 'iVBORw0KGgo'; echo $?)"

check "click ok"                    "$(has "$(ctl_arg click '"400 300"')" '"ok":true'; echo $?)"
check "type ok"                     "$(has "$(ctl_arg type '"hello world"')" '"chars":11'; echo $?)"
check "key with modifiers ok"       "$(has "$(ctl_arg key '"ctrl+a"')" '"ok":true'; echo $?)"
check "wheel ok"                    "$(has "$(ctl_arg wheel '"0 300"')" '"ok":true'; echo $?)"

check "overlay prompt opens"        "$(has "$(ctl_arg overlay '"prompt"')" '"overlay":"prompt"'; echo $?)"
check "overlay none closes"         "$(has "$(ctl_arg overlay '"none"')" '"overlay":"none"'; echo $?)"

scr=$(ctl_arg scroll_to '"example"')
check "scroll_to returns scroll"    "$(has "$scr" '"scroll":'; echo $?)"

ctl_arg toast '"agent-passing-through"' >/dev/null
sleep 0.3
check "toast shows as overlay"      "$(has "$(ctl_arg overlay '"get"')" '"overlay":"toast"'; echo $?)"

check "legacy state still works"    "$(has "$(ctl state)" '"pages"'; echo $?)"
check "legacy exec still works"     "$(has "$(ctl_arg exec '"page.next"')" '"ok":true'; echo $?)"
bad=$(ctl_arg click 'not-a-number')
check "bad args error cleanly"      "$(has "$bad" '"ok":false'; echo $?)"

kill "$BPID" 2>/dev/null || true

echo "AGENT API: pass=$pass fail=$fail"
[ "$fail" = "0" ]
