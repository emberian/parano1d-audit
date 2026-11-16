#!/usr/bin/env bash
# Start ForgeMiner against the loopback devnet node, egress-gated.
#   $1 = wallet (the node's own o1 payout address)
# --password carries the node's mining key: IF forge sends an Authorization
# header it is a valid Mining credential (getBlockTemplate/submitBlock); if it
# sends none, loopback Local access applies. Either way the two calls it makes
# are allowed. All pool connections (incl. the embedded 1% dev fee to
# stratum+ssl://eu2.innovlab.cc:19601) go through scripts/gate.py, which
# forwards only to 127.0.0.1:21241.
set -euo pipefail

LANE=/home/hbox/lanes/forge-bench
WALLET=${1:?usage: start_forge.sh <o1-wallet-address>}
mkdir -p "$LANE/run" "$LANE/logs"

if [ -f "$LANE/run/forge.pid" ]; then
  PID=$(cat "$LANE/run/forge.pid")
  if kill -0 "$PID" 2>/dev/null; then
    echo "forge already running, pid=$PID"; exit 0
  fi
fi

nohup setsid "$LANE/bin/forge" \
  --algorithm noid \
  --pool 127.0.0.1:21241 \
  --wallet "$WALLET" \
  --password 'forge-bench-mining-2f8a51c0b9d7e4' \
  --worker forgebench \
  --proxy socks5://127.0.0.1:7778 \
  --temp-limit 85 \
  --temp-resume 75 \
  --api \
  --plain \
  --log "$LANE/logs/forge-tables.log" \
  >> "$LANE/logs/forge.out" 2>&1 &
echo $! > "$LANE/run/forge.pid"
echo "forge pid=$(cat "$LANE/run/forge.pid")"
