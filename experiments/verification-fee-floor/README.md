# Local fee-floor admission check

This fixture targets Parano1d source commit `52938433a30b185ab007afaa15166a632cf2e82e`, using the pinned source copy in `../comp-system-hunter/pinned/`.

It builds 23 locally accepted blocks under the source's `isolated-v2-1-fork-testnet` activation schedule, funds ordinary spenders and a launch creator with coinbase outputs, and submits real wallet-authorized ordinary transactions to `AsyncMempool`. A capacity-5 pool and fee-window-1 configuration let four high-fee ordinary transactions activate its normal 80% pressure latch while leaving one admission slot. It then submits one signed, exact-fee `CreateLaunch` intent to a fresh pool as a control and to the pressured pool as the test.

The block setup uses `MdbxChainContext::apply_next_block` and native consensus validation, but supplies a terminal-verification callback that returns success. It therefore establishes the chain-state and mempool behavior under the isolated schedule, not a complete release HistoryStep proof chain. The smaller pool and fee window are local configuration; the exact-fee/raised-floor checks are the production admission code.

Run:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/core-seams/target \
  cargo run --release --offline --manifest-path experiments/verification-fee-floor/Cargo.toml
```

Observed twice in the optimized local run (see `result.log`):

- 23 blocks were accepted by the local native-consensus chain fixture.
- Four independently authorized ordinary spends were admitted; the pool was 4/5 full and its local floor was 90,000 μNOID.
- The same cryptographically authorized CreateLaunch bytes were accepted in a fresh pool at the base floor, then rejected in the pressured pool with `BelowMinFee { required: 90000, actual: 8600 }`. One capacity slot remained free.

This demonstrates the local admission incompatibility under a reachable pressure configuration. It does not show every node using that floor, a chain-level prohibition on including the launch, a market-wide freeze, or a complete release terminal chain. The isolated schedule activates v2.1 at height 24; normal mainnet activation is later.
