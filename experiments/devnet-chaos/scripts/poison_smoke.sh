#!/usr/bin/env bash
# Verify the poison node mines far-future-timestamp blocks that its OWN drift
# gate accepts. PID-filed; stop-by-PID.
set -uo pipefail
LANE=/Users/ember/dev/parano1d-break/experiments/devnet-chaos
cd "$LANE"
rm -rf run/poison-smoke; mkdir -p run/poison-smoke logs
NOID_POISON_CLOCK_OFFSET_SECS=315360000 nohup target-poison/release/parano1d \
  --mode miner --genesis --data-dir "$LANE/run/poison-smoke" \
  --p2p-listen 127.0.0.1:19900 --rpc-listen 127.0.0.1:19901 \
  --disable-dns-seeds --log info > logs/poison-smoke.log 2>&1 &
echo $! > run/poison-smoke.pid
echo "poison node pid=$(cat run/poison-smoke.pid)"
for i in $(seq 1 40); do
  sleep 5
  H=$(curl -s -m3 -X POST -H 'content-type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"paranoid_getChainInfo","params":[]}' \
      http://127.0.0.1:19901 2>/dev/null | python3 -c "import sys,json;print(json.load(sys.stdin)['result']['height'])" 2>/dev/null)
  echo "t+$((i*5))s height=$H"
  if [ "${H:-0}" -ge 3 ] 2>/dev/null; then break; fi
done
echo "--- block 2 timestamp (far-future expected ~2036) ---"
bin/parano1d-cli --rpc http://127.0.0.1:19901 block-header 2 2>/dev/null | grep -i timestamp
echo "--- real now unix ---"; date -u +%s
P=$(cat run/poison-smoke.pid)
curl -s -m3 -X POST -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"paranoid_stop","params":[]}' http://127.0.0.1:19901 >/dev/null 2>&1
sleep 3; if ps -p "$P" >/dev/null 2>&1; then kill "$P"; fi
rm -f run/poison-smoke.pid
echo "poison smoke done"
