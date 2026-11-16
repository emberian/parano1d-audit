#!/usr/bin/env bash
# Canonical early-finance node build — ONE shared binary for all financial-op
# lanes (halt-stalefee, halt-binder, chaos-latch). Root-owned (campaign
# session 2026-11-15) after two lanes duplicated ~40-min builds on the laptop.
#
# Tree: copied from experiments/chaos-latch/pinned-early-finance (the
# chaos-latch patch: MAINNET_V2_ACTIVATION_HEIGHT 210_537 -> 10,
# MAINNET_V2_1_ACTIVATION_HEIGHT 279_370 -> Some(24); owner-auth V4 follows
# V2.1 per noid_chain/src/consensus/params.rs:34). Relation itself untouched.
#
# Pack/retirement env recipe: the exact set recorded in devnet-chaos's poison
# build fingerprint (experiments/devnet-chaos/target-poison/release/
# .fingerprint/noid_node-*/run-build-script-build-script-build.json) — the
# same packs, banks and retirement pins the stock devnet-chaos/bin/parano1d
# was built with. noid_node/build.rs:78 asserts the three retirement vars on
# every release build with a scheduled V2.
set -euo pipefail
LANE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
D=/Users/ember/dev/parano1d-break/experiments/devnet-chaos
export CARGO_TARGET_DIR="$LANE/target"
export NOID_HISTORY_STEP_PACK_DIR="$D/packs/history-step-v1"
export NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST=ad463bd76e27df3c0f414f4fd7640cfb5c45cc7f3a09e44a2f7bc7a8b869485b
export NOID_V2_PACK_DIR="$D/downloads/v2-pack/history-step-pack-v2"
export NOID_V2_RELEASE_BANK=c2a6df736b0d0da22e285b6930b11cf44b520d65b52c7dfe78f44fe0cd48e76e
export NOID_V21_PACK_DIR="$D/downloads/v21-pack/history-step-pack-v21"
export NOID_V21_RELEASE_BANK=6c280ae869afc25e31d46d73b42870512991b78c9498bc7e5ef939bc07e5ed5e
export NOID_RETIREMENT_KEYS_DIR="$D/downloads/v2-pack/history-step-pack-v2/retirement-keys"
export NOID_RETIREMENT_KEY_0_PIN=0651ce507810b61213cbdc0e9f99436aec3a7922cc662bfba6128b466453c8b2
export NOID_RETIREMENT_KEY_1_PIN=c301274d50dc02fd5c383f2e71629075729a954a768f5051388ec2040bce0267
cd "$LANE/tree"
cargo build --locked --release -p noid_node --bin parano1d --bin parano1d-cli
cp -f "$CARGO_TARGET_DIR/release/parano1d" "$LANE/bin/parano1d"
cp -f "$CARGO_TARGET_DIR/release/parano1d-cli" "$LANE/bin/parano1d-cli"
echo "SHARED_EARLY_FINANCE_BUILD_DONE"
