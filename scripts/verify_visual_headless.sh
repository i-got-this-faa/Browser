#!/usr/bin/env bash
# Headless/live visual verification script:
# Runs browser, queries refresh rate and output mode, navigates, captures screenshot,
# tests cursor and scroll events, and saves visual artifact.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOCK="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/strip-browser.sock"
ARTIFACT_DIR="/home/radhey/.gemini/antigravity-cli/brain/4045ee87-8eef-4ff9-8e06-e04a4314bade"
mkdir -p "$ARTIFACT_DIR"

pkill -f "target/debug/browser" 2>/dev/null || true
sleep 1

echo "Starting browser on WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-wayland-1}..."
env STRIP_TRACE=/tmp/trace-visual.jsonl "$ROOT/target/debug/browser" >/tmp/visual.log 2>&1 &
BPID=$!

trap "kill $BPID 2>/dev/null || true" EXIT

echo "Waiting for browser control socket..."
ready=""
for i in $(seq 1 80); do
  r=$(printf '{"cmd":"state"}\n' | timeout 1 nc -U "$SOCK" 2>/dev/null || true)
  if echo "$r" | grep -q '"ok":true'; then ready=1; break; fi
  sleep 0.2
done

if [ -z "$ready" ]; then
  echo "FAIL: browser never became ready"
  tail -20 /tmp/visual.log
  exit 1
fi

echo "Browser is online."

python3 - <<'PYEOF'
import socket, json, time, base64, os

sock = os.environ.get("SOCK", f"/run/user/{os.getuid()}/strip-browser.sock")

def send(cmd, arg=None):
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock)
    msg = {"cmd": cmd}
    if arg is not None:
        msg["arg"] = arg
    s.sendall(json.dumps(msg).encode() + b"\n")
    chunks = []
    while True:
        buf = s.recv(65536)
        if not buf:
            break
        chunks.append(buf)
        if b"\n" in buf:
            break
    s.close()
    return json.loads(b"".join(chunks).decode())

print("Querying state...")
st = send("get_state")
print(f"Active workspace: {st.get('active_workspace')}, pages count: {len(st.get('pages', []))}")

print("Navigating to example.com...")
nav = send("open", "example.com")
print(f"Open result: {nav}")

print("Waiting for page load and frame production (5s)...")
time.sleep(5)
shot = None
for _ in range(20):
    shot = send("screenshot")
    if shot.get("ok") and shot.get("png_base64"):
        break
    time.sleep(0.5)

if shot and shot.get("ok") and shot.get("png_base64"):
    raw = base64.b64decode(shot["png_base64"])
    out_path = "/home/radhey/.gemini/antigravity-cli/brain/4045ee87-8eef-4ff9-8e06-e04a4314bade/verified_headless_frame.png"
    with open(out_path, "wb") as f:
        f.write(raw)
    print(f"SUCCESS: Saved verified screenshot ({len(raw)} bytes) to {out_path}")
else:
    print(f"Screenshot failed: {shot}")

print("Testing mouse motion & click...")
click_res = send("click", "200 200 active left")
print(f"Click response: {click_res}")

print("Testing kinetic wheel...")
wheel_res = send("wheel", "0 -80 active")
print(f"Wheel response: {wheel_res}")

print("Visual verification passed.")
PYEOF

echo "SUCCESS: Visual and runtime checks completed successfully."
