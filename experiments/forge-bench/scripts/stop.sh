#!/usr/bin/env bash
# Stop the forge-bench lane on hbox — PID files only, never by pattern.
# Order: forge (stop hashing first), then the node via RPC paranoid_stop
# (fallback SIGTERM by PID), then the sampler and the gate by PID.
set -uo pipefail

LANE=/home/hbox/lanes/forge-bench

stop_by_pid() {
  local name=$1 pidfile="$LANE/run/$2" sig=$3
  if [ -f "$pidfile" ]; then
    local pid
    pid=$(cat "$pidfile")
    if kill -0 "$pid" 2>/dev/null; then
      # Re-read the command line right before stopping (PIDs get reused).
      echo "[stop] $name pid=$pid cmd=$(tr '\0' ' ' < /proc/$pid/cmdline 2>/dev/null | cut -c1-120)"
      kill "$sig" "$pid" 2>/dev/null || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
      done
      kill -KILL "$pid" 2>/dev/null || true
    else
      echo "[stop] $name pid=$pid not alive"
    fi
    rm -f "$pidfile"
  else
    echo "[stop] $name: no pid file"
  fi
}

# 1) forge — SIGTERM lets it restore any card state it changed.
stop_by_pid forge forge.pid TERM

# 2) node — graceful RPC stop first, then PID fallback.
if [ -f "$LANE/run/node.pid" ]; then
  PID=$(cat "$LANE/run/node.pid")
  if kill -0 "$PID" 2>/dev/null; then
    echo "[stop] node pid=$PID cmd=$(tr '\0' ' ' < /proc/$PID/cmdline 2>/dev/null | cut -c1-120)"
    python3 - <<'EOF' || true
import json, urllib.request
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "paranoid_stop", "params": []}).encode()
req = urllib.request.Request("http://127.0.0.1:21241", data=body, headers={"content-type": "application/json"})
try:
    with urllib.request.urlopen(req, timeout=8) as resp:
        print("rpc stop:", resp.read().decode()[:120])
except Exception as exc:
    print("rpc stop failed:", exc)
EOF
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
      kill -0 "$PID" 2>/dev/null || break
      sleep 1
    done
    kill -TERM "$PID" 2>/dev/null || true
    sleep 2
    kill -KILL "$PID" 2>/dev/null || true
  else
    echo "[stop] node pid=$PID not alive"
  fi
  rm -f "$LANE/run/node.pid"
fi

# 3) sampler and gate — plain PIDs.
stop_by_pid sampler sampler.pid TERM
stop_by_pid gate gate.pid TERM
echo "[stop] done; see $LANE/logs"
