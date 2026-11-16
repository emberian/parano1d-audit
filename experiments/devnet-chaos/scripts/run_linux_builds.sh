#!/usr/bin/env bash
# Reproduction helper for phase 4: build the two Linux node binaries inside a
# docker container (native aarch64-linux; no cross toolchain needed).
#
#   target-linux/default/debug/parano1d    mainnet dialect, v1+v2+v21 packs embedded
#   target-linux/isolated/debug/parano1d   isolated-v2-1-fork-testnet schedule
#
# The pinned checkout is mounted read-only; all build output stays in the
# lane directory. The runtime (phase4_docker.sh) runs in a separate
# --network none container whose netns is loopback-only, which is what the
# isolated-schedule startup guard requires (noid_node/src/main.rs:1735-1768).

set -euo pipefail

LANE=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
V21_PACK="$LANE/downloads/v21-pack/history-step-pack-v21"
V21_BANK=6c280ae869afc25e31d46d73b42870512991b78c9498bc7e5ef939bc07e5ed5e
DIGEST=ad463bd76e27df3c0f414f4fd7640cfb5c45cc7f3a09e44a2f7bc7a8b869485b

docker run --rm --name devnet-chaos-linuxbuild \
  -v /Users/ember/dev/parano1d-break/parano1d:/src:ro \
  -v "$LANE":/lane \
  -w /src \
  -e CARGO_HOME=/lane/.cargo-home-linux \
  -e CARGO_TARGET_DIR=/lane/target-linux/default \
  -e NOID_HISTORY_STEP_PACK_DIR=/lane/packs/history-step-v1 \
  -e NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST="$DIGEST" \
  -e NOID_V21_PACK_DIR="$V21_PACK" \
  -e NOID_V21_RELEASE_BANK="$V21_BANK" \
  -e CARGO_PROFILE_DEV_OPT_LEVEL=3 -e CARGO_PROFILE_DEV_DEBUG=0 \
  devnet-chaos-builder bash -c '
    cargo build --locked -p noid_node --bin parano1d &&
    CARGO_TARGET_DIR=/lane/target-linux/isolated cargo build --locked -p noid_node --bin parano1d --features isolated-v2-1-fork-testnet &&
    echo LINUX_BUILDS_DONE'
