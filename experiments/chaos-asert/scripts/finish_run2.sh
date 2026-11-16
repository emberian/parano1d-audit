#!/usr/bin/env bash
# Continue the chaos-asert run AFTER the fork stage (fork already built and
# served by S; state.json has the fork record). Runs the remaining stages:
#   wait   (sleeps until the fork tip ages into the honest drift window)
#   adopt  (spontaneous-adoption watch, then relay restarts -> fork choice)
#   produce (honest stock-binary miner on the collapsed chain, sample blocks)
# Completion marker: "RUN COMPLETE" at the end of logs/stage-finish2.log.
set -u
cd /Users/ember/dev/parano1d-break/experiments/chaos-asert

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
