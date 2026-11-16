#!/usr/bin/env bash
# Launch helper: starts the node build detached with a PID file (absolute paths).
LANE=/home/hbox/lanes/forge-bench
rm -f "$LANE/run/build-node.pid"
nohup setsid env SWARM_MEM_MAX=16G swarm-build bash "$LANE/scripts/build_node_hbox.sh" >> "$LANE/logs/build-node.log" 2>&1 < /dev/null &
echo $! > "$LANE/run/build-node.pid"
cat "$LANE/run/build-node.pid"
