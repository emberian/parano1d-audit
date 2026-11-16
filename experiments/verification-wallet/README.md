# Wallet reservation after mempool eviction

Target source: `experiments/comp-system-hunter/pinned/`, copied from Parano1d commit `52938433a30b185ab007afaa15166a632cf2e82e`.

## Run

The strongest result is `full-inbound-run.log` (exit 0). The pinned-source,
release-profile `full_terminal` binary produced both blocks using the release
HistoryStep matrix packs. For each block, a separate local MDBX chain accepted
the resulting bundle through `apply_next_block`, including the daemon's
`install_inbound_verifier_cpu` and `runtime.verify_terminal` path. The
wallet/mempool desync remained after the independently verified second block:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/core-seams/target \
  cargo run --release --offline --manifest-path experiments/verification-wallet/Cargo.toml --bin full_terminal
```

This is a local full-proof, inbound-verified chain integration run. It does not
exercise the release daemon's P2P or RPC handlers, or show a chain halt. The
earlier `full-run.log` records the prover-generated, locally committed path
before the independent inbound verifier was added.

## Native-consensus control

The observed local-chain result is in `accepted-run.log`:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/core-seams/target \
  cargo run --offline --manifest-path experiments/verification-wallet/Cargo.toml --bin accepted_block
```

The `accepted_block` binary uses the production PoW, block template, MDBX block acceptance, wallet proof, mempool admission, wallet block update, and mempool block update code. It builds and commits an empty coinbase block to fund the wallet, proves and admits a wallet send, then commits an independently mined empty block whose coinbase claims one of the pending send's output slots. The mempool emits `TxEvicted(OutputSlotOccupied)` and becomes empty. The wallet's cached input remains reserved, its spendable balance is zero, and address discovery returns an error. Reloading the wallet clears the in-memory reservation; the unconfirmed send remains in durable history.

**Proof boundary:** the fixture passes a success callback for the opaque HistoryStep terminal in `MdbxChainContext::apply_next_block`. The terminal metadata is bound to each block, but its recursive proof is not generated or verified. This is a local native-consensus and wallet/mempool integration reproduction, not a release-daemon end-to-end run or a chain halt.

The `full_terminal` binary resolves that proof boundary. It copies the
HistoryStep runtime and block production helper into this fixture's single
pinned dependency graph, then verifies both terminals on an independent
inbound chain.

`src/main.rs` and `results.log` are a smaller state-machine control: a real wallet proof and real mempool verification, then a constructed occupied `ChainView`. An unchanged-view control retains the pending transaction; manual rollback restores the wallet's spendable balance. This control does not commit a block.

## Observed output

```text
full terminal: proved and independently verifier-accepted funding block 1
full terminal: wallet proof admitted before unseen competing block
full terminal: verifier-accepted block 2 evicted tx; wallet input still reserved
full terminal: restart clears reservation; pending history persists
```

The native-consensus control printed:

```text
local chain: PoW-valid coinbase block 1 accepted to MDBX; wallet funded
wallet send: real proof admitted before competing coinbase block
accepted block 2: mempool=0; wallet spendable=0; input reserved; discovery blocked
restart: lock clears; unconfirmed send history remains
```
