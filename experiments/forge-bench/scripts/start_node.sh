#!/usr/bin/env bash
# Start the forge-bench devnet node on hbox (loopback-only, genesis mode).
# Official v2.1.0 linux-x86_64 release binary, sha256-verified against the
# release SHA256SUMS. PID file discipline: run/node.pid; stop via
# scripts/stop.sh (RPC paranoid_stop + PID).
set -euo pipefail

LANE=/home/hbox/lanes/forge-bench
mkdir -p "$LANE/run" "$LANE/logs"

if [ -f "$LANE/run/node.pid" ]; then
  PID=$(cat "$LANE/run/node.pid")
  if kill -0 "$PID" 2>/dev/null; then
    echo "node already running, pid=$PID"; exit 0
  fi
fi

nohup setsid "$LANE/bin/parano1d" \
  --mode extminer \
  --mining-key 'forge-bench-mining-2f8a51c0b9d7e4' \
  --allow-custom-coinbase \
  --genesis \
  --data-dir "$LANE/run/node" \
  --p2p-listen 127.0.0.1:21240 \
  --rpc-listen 127.0.0.1:21241 \
  --disable-dns-seeds \
  --log debug \
  >> "$LANE/logs/node.log" 2>&1 &
echo $! > "$LANE/run/node.pid"
echo "node pid=$(cat "$LANE/run/node.pid")"
