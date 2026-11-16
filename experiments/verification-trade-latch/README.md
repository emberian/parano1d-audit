# Trade occupancy and the local fee floor

This isolated runner exercises the NM-2B hypothesis against the copied source in
`../comp-system-hunter/pinned/` (original source commit
`52938433a30b185ab007afaa15166a632cf2e82e`). It uses the isolated v2.1
schedule, where the candidate at height 24 has V4 owner authorization.

The runner mines 23 local PoW blocks into an MDBX chain, creates two ordinary
100,000 µNOID spends and three distinct funded trade policies for an asset absent
from that chain, and submits all five to the real `AsyncMempool`. Every admission
uses its normal V4 proof verifier. At capacity six, the fifth entry crosses
the 80% threshold; the ordinary spends supply the rolling fee samples.
It then mines and applies a block containing those spends, calls the mempool's
normal block update with the accepted chain view, and tests whether the three
trade policies hold the fee floor above the base minimum. A signed exact-fee
`CreateLaunch` is submitted to both the updated pool and a fresh pool at the
same height.

The local block applies full native block consensus checks and PoW. Its
HistoryStep terminal is a fixture prefix with a stubbed proof-verification
callback, as in the earlier local fee-floor runner. Thus an accepted MDBX block
here is **not** evidence of a completed recursive HistoryStep proof or of a
release daemon receiving the block through P2P. The test uses a configured
six-entry pool to keep V4 proving bounded; production pool size is 1024, so
the test establishes the mechanism and not the resource cost of saturating a
production node. It does not send transactions to a live network.

Run with:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=../comp-system-hunter/core-seams/target \
  cargo run --release --offline --manifest-path experiments/verification-trade-latch/Cargo.toml
```

See `RESULT.md` for the actual observed result and exact boundary.
