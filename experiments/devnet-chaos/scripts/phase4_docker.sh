#!/usr/bin/env bash
# Phase 4 — cross-dialect mesh inside a loopback-only Linux netns (docker
# --network none), because isolated-schedule node builds refuse to start
# unless /proc/net/dev shows only "lo" (noid_node/src/main.rs:1735-1768 —
# the shipped guard for isolated fork tests).
#
# Prerequisite: scripts/run_linux_builds.sh has produced
#   target-linux/default/debug/parano1d      (mainnet-dialect, all packs)
#   target-linux/isolated/debug/parano1d     (isolated-v2-1-fork-testnet)
#
# Everything runs INSIDE the container on its loopback; logs are written to
# the mounted lane directory. Each node is stopped by RPC then PID file.

set -euo pipefail

LANE=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
DEFAULT_BIN=/lane/target-linux/default/debug/parano1d
ISOLATED_BIN=/lane/target-linux/isolated/debug/parano1d
OUT=/lane/logs

docker run --rm -i --network none \
  --name devnet-chaos-phase4 \
  -v "$LANE":/lane \
  -v "$LANE/docker/fake-proc-net-dev":/proc/net/dev:ro \
  -w /tmp \
  devnet-chaos-builder bash -s <<'EOF' 2>&1 | tee -a "$LANE/logs/phase4-docker.log"
set -uo pipefail
mkdir -p /tmp/run /lane/logs
DEFAULT=/lane/target-linux/default/debug/parano1d
ISOLATED=/lane/target-linux/isolated/debug/parano1d

echo "=== PHASE 4 (docker lo-only netns): cross-dialect mesh ==="
date

$DEFAULT --mode miner --genesis \
  --data-dir /tmp/run/n1d --p2p-listen 127.0.0.1:21610 \
  --rpc-listen 127.0.0.1:21611 --disable-dns-seeds --log debug \
  > /lane/logs/phase4-n1d.log 2>&1 &
N1D=$!
echo $N1D > /tmp/run/n1d.pid
echo "[start] n1d (default dialect, mode miner) pid=$N1D"

$ISOLATED --mode node \
  --config /tmp/run/nx.toml \
  --data-dir /tmp/run/nx --p2p-listen 127.0.0.1:21710 \
  --rpc-listen 127.0.0.1:21711 --disable-dns-seeds --log debug \
  --seed 127.0.0.1:21610 \
  > /lane/logs/phase4-nx.log 2>&1 &
NX=$!
echo $NX > /tmp/run/nx.pid
echo "[start] nx (isolated v2.1 dialect, seeds n1d) pid=$NX"

rpc() {
  curl -s -m 5 -X POST -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"paranoid_$1\",\"params\":[]}" \
    "http://127.0.0.1:$2" || echo '{"transport":"down"}'
}

for i in $(seq 1 24); do
  sleep 10
  N1H=$(rpc getChainInfo 21611 | grep -o '"height":[0-9]*' | head -1)
  NXH=$(rpc getChainInfo 21711 | grep -o '"height":[0-9]*' | head -1)
  echo "[t+$((i*10))s] n1d $N1H   nx $NXH"
done

echo "--- nx rejections (last 8) ---"
grep -E "reject|Bad|Mismatch|invalid|refus" /lane/logs/phase4-nx.log | tail -8 || true
echo "--- nx profile handshake ---"
grep -E "profile" /lane/logs/phase4-nx.log | tail -4 || true
echo "--- nx sync activity ---"
grep -E "header|sync|serving" /lane/logs/phase4-nx.log | tail -8 || true
echo "--- n1d peer view ---"
grep -E "profile|peer" /lane/logs/phase4-n1d.log | tail -6 || true

for pidfile in /tmp/run/*.pid; do
  pid=$(cat "$pidfile")
  kill "$pid" 2>/dev/null || true
done
sleep 3
echo "=== PHASE 4 docker run complete ==="
EOF
