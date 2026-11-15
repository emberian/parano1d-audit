#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
CARGO_TARGET_DIR="$PWD/target" cargo run --release --locked "$@"
