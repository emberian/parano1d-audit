#!/usr/bin/env bash
# Finish the chaos-asert run WITHOUT an agent in the loop.
#
# Precondition: the fork stage is running (run/stage-fork.pid) or already
# complete. This wrapper waits for it to finish (it stops the poison miner and
# starts serving node S by itself), then runs the remaining stages in order:
#   wait   (~8 min: sleeps until the fork tip ages into the honest drift
#           window, sampling relay tips + rejection counts)
#   adopt  (~5-10 min: spontaneous-adoption watch, then relay restarts so
#           fork choice selects the collapsed fork)
#   produce(~5-10 min: restarts n2 as an honest miner on the collapsed chain
#           and samples the post-collapse blocks)
# Every stage is PID-filed and logs to logs/stage-<name>.log. Completion
# marker: "RUN COMPLETE" at the end of logs/stage-finish.log.
set -u
cd /Users/ember/dev/parano1d-break/experiments/chaos-asert

until grep -q "serving node S up" logs/stage-fork.log 2>/dev/null; do
  if grep -q "ChaosError\|Traceback" logs/stage-fork.log 2>/dev/null; then
    echo "fork stage FAILED; not continuing (see logs/stage-fork.log)" >&2
    exit 1
  fi
  sleep 20
done
echo "$(date -u +%FT%T) fork stage complete; running wait stage"

if ! python3 scripts/collapse.py wait > logs/stage-wait.log 2>&1; then
  echo "wait stage FAILED (logs/stage-wait.log)" >&2; exit 1
fi
echo "$(date -u +%FT%T) wait stage complete; running adopt stage"

if ! python3 scripts/collapse.py adopt > logs/stage-adopt.log 2>&1; then
  echo "adopt stage FAILED (logs/stage-adopt.log)" >&2; exit 1
fi
echo "$(date -u +%FT%T) adopt stage complete; running produce stage"

if ! python3 scripts/collapse.py produce --blocks 15 > logs/stage-produce.log 2>&1; then
  echo "produce stage FAILED (logs/stage-produce.log)" >&2; exit 1
fi
echo "$(date -u +%FT%T) RUN COMPLETE — harvest with scripts/summarize.py, then write REPORT.md"
