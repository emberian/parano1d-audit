#!/bin/bash
# split-cd1 followup runner: rebuild the fixture against the corrected tree
# (pinned 52938433 schedule), then run it if the build is green.
# Launched detached: `nohup setsid bash runner.sh > ../run/fixture-run.log 2>&1
#                    < /dev/null & echo $! > ../run/runner.pid` from fixture/.
set -euo pipefail
cd /othersys/lanes/parano1d/split-cd1/fixture

# Phase 1: rebuild (release, offline, memory-capped). Output to build.log.
export SWARM_MEM_MAX=8G CARGO_BUILD_JOBS=8
echo "=== rebuild start $(date -Is) ===" >> build.log
if ! swarm-build cargo build --release --offline >> build.log 2>&1; then
    echo "=== BUILD FAILED $(date -Is) ===" >> build.log
    exit 1
fi
echo "=== BUILD OK $(date -Is) ===" >> build.log

# Phase 2: the decisive fixture run. Stdout/stderr land in run/fixture-run.log
# via the launcher's redirect; this exec keeps the runner PID live for the run.
cd /othersys/lanes/parano1d/split-cd1
export NOID_SPLIT_CD1_PACKS_ROOT=/othersys/lanes/parano1d/packs
export SWARM_MEM_MAX=8G
exec swarm-build fixture/target/release/split-cd1-fixture
