#!/usr/bin/env bash
# halt-reorg node build (production config: early-finance tree, log_slots 24).
# Env recipe copied from experiments/shared-early-finance/build.sh with pack dirs
# repointed at the shared hbox packs. Runs under swarm-build (SWARM_MEM_MAX=8G).
set -euo pipefail
LANE=/othersys/lanes/parano1d/halt-reorg
P=/othersys/lanes/parano1d/packs
export CARGO_TARGET_DIR="$LANE/target"
export NOID_HISTORY_STEP_PACK_DIR="$P/history-step-v1"
export NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST=ad463bd76e27df3c0f414f4fd7640cfb5c45cc7f3a09e44a2f7bc7a8b869485b
export NOID_V2_PACK_DIR="$P/downloads/v2-pack/history-step-pack-v2"
export NOID_V2_RELEASE_BANK=c2a6df736b0d0da22e285b6930b11cf44b520d65b52c7dfe78f44fe0cd48e76e
export NOID_V21_PACK_DIR="$P/downloads/v21-pack/history-step-pack-v21"
export NOID_V21_RELEASE_BANK=6c280ae869afc25e31d46d73b42870512991b78c9498bc7e5ef939bc07e5ed5e
export NOID_RETIREMENT_KEYS_DIR="$P/downloads/v2-pack/history-step-pack-v2/retirement-keys"
export NOID_RETIREMENT_KEY_0_PIN=0651ce507810b61213cbdc0e9f99436aec3a7922cc662bfba6128b466453c8b2
export NOID_RETIREMENT_KEY_1_PIN=c301274d50dc02fd5c383f2e71629075729a954a768f5051388ec2040bce0267
cd "$LANE/source"
cargo build --locked --release -p noid_node --bin parano1d --bin parano1d-cli -p noid-extminer
mkdir -p "$LANE/bin"
cp -f "$CARGO_TARGET_DIR/release/parano1d" "$LANE/bin/parano1d"
cp -f "$CARGO_TARGET_DIR/release/parano1d-cli" "$LANE/bin/parano1d-cli"
cp -f "$CARGO_TARGET_DIR/release/parano1d-miner" "$LANE/bin/parano1d-miner"
echo HALT_REORG_BUILD_DONE
