#!/bin/zsh
# D5: loopback any-process access — dark methods + stop, against the real node.
# Usage: run-demo.sh   (expects the lane-built binary; kills only by PID file)
set -u
LANE=/Users/ember/dev/parano1d-break/experiments/dark-surface
BIN=${BIN:-$LANE/target/debug/parano1d}
RPC=http://127.0.0.1:39601
D=$LANE/demo

rpc() { curl -s --max-time 20 $RPC -d "$1"; echo }

echo "=== D5: uncredentialed loopback process == full owner (no GUI/CLI involved) ==="
echo "-- node: $BIN (scratch dir $D/ds6, loopback RPC 39601, no credentials)"
rm -rf $D/ds6
nohup $BIN --data-dir $D/ds6 --config $D/ds6.toml \
  --rpc-listen 127.0.0.1:39601 --p2p-listen 127.0.0.1:39606 \
  --disable-dns-seeds --log info > $D/ds6-node.log 2>&1 &
echo $! > $D/ds6.pid

# wait for RPC readiness (blockCount answers)
ready=0
for i in {1..120}; do
  if curl -s --max-time 2 $RPC -d '{"jsonrpc":"2.0","id":0,"method":"paranoid_blockCount","params":[]}' | grep -q result; then ready=1; break; fi
  sleep 1
done
if [ $ready -ne 1 ]; then echo "NODE FAILED TO START"; tail -5 $D/ds6-node.log; exit 1; fi
echo "node ready (pid $(cat $D/ds6.pid))"

ADDR=$(rpc '{"jsonrpc":"2.0","id":1,"method":"paranoid_walletActiveAddress","params":[]}' | sed -E 's/.*"address":"([^"]+)".*/\1/')
echo "-- wallet active address: $ADDR"

echo "-- walletStatus (GUI+CLI-dark):"
rpc '{"jsonrpc":"2.0","id":2,"method":"paranoid_walletStatus","params":[]}'
echo "-- getActiveSlotCount (GUI+CLI-dark):"
rpc '{"jsonrpc":"2.0","id":3,"method":"paranoid_getActiveSlotCount","params":[]}'
echo "-- getNativeMarket synthetic asset (GUI+CLI-dark):"
rpc '{"jsonrpc":"2.0","id":4,"method":"paranoid_getNativeMarket","params":["0000000000000000000000000000000000000000000000000000000000000000"]}'
echo "-- getSlotHints (GUI+CLI-dark):"
rpc '{"jsonrpc":"2.0","id":5,"method":"paranoid_getSlotHints","params":[3]}'
echo "-- getSlotsByOwner arbitrary address enumeration (loopback-only tier):"
rpc "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"paranoid_getSlotsByOwner\",\"params\":[\"$ADDR\"]}"
echo "-- prepareNativeIntent external-client plan (GUI+CLI-dark):"
rpc "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"paranoid_prepareNativeIntent\",\"params\":[\"$ADDR\",{\"TransferToken\":{\"asset_id\":\"0000000000000000000000000000000000000000000000000000000000000000\",\"units\":\"1\",\"recipient\":\"$ADDR\"}},null]}"
echo "-- walletDismissNativePending unknown txid (only fully-undocumented method):"
rpc '{"jsonrpc":"2.0","id":8,"method":"paranoid_walletDismissNativePending","params":["0000000000000000000000000000000000000000000000000000000000000000"]}'
echo "-- submitTxIntent raw garbage (bounded decode error proves reachability):"
rpc '{"jsonrpc":"2.0","id":9,"method":"paranoid_submitTxIntent","params":["00"]}'
echo "-- getNodeStatus queue-depth dump:"
rpc '{"jsonrpc":"2.0","id":10,"method":"paranoid_getNodeStatus","params":[]}'

echo "-- browser-origin request must be rejected pre-dispatch:"
curl -s -o /dev/null -w "HTTP %{http_code}\n" -H "Origin: https://example.org" $RPC -d '{"jsonrpc":"2.0","id":11,"method":"paranoid_blockCount","params":[]}'

echo "-- PARANOID_STOP from this unrelated process:"
rpc '{"jsonrpc":"2.0","id":12,"method":"paranoid_stop","params":[]}'
sleep 3
if ps -p $(cat $D/ds6.pid) > /dev/null 2>&1; then
  echo "node still alive — stop did not kill it"
  kill $(cat $D/ds6.pid)
else
  echo "RESULT: node process $(cat $D/ds6.pid) EXITED after paranoid_stop from a foreign loopback curl"
fi
grep -E "stop command|shutdown" $D/ds6-node.log | tail -2
