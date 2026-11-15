#!/usr/bin/env bash
# Reproduction for the malicious-node composition lane.
# Pins: vendored crates in pinned/ come from git archive 52938433 (see
# vendor.log); src/snapshot_header_staging.rs is verified byte-identical to
# the commit's blob before every run.
set -euo pipefail
cd "$(dirname "$0")"

PIN=52938433a30b185ab007afaa15166a632cf2e82e
SRC=/Users/ember/dev/parano1d-break/parano1d

expected_blob=$(git -C "$SRC" rev-parse "${PIN}:noid_node/src/snapshot_header_staging.rs")
actual_blob=$(git -C "$SRC" hash-object "$PWD/src/snapshot_header_staging.rs")
if [ "$expected_blob" != "$actual_blob" ]; then
  echo "FATAL: snapshot_header_staging.rs is not the pinned blob ($actual_blob != $expected_blob)" >&2
  exit 1
fi
echo "pinned_module_blob=$actual_blob"

CARGO_TARGET_DIR="$PWD/target" cargo run --release --locked 2>&1 | tee results.txt
echo "--- isolated-V2.1 dialect run ---"
CARGO_TARGET_DIR="$PWD/target-isolated" cargo run --release --locked --features isolated-v2-1-fork-testnet 2>&1 | tee results-isolated.txt
