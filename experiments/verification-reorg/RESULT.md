# Result: release-mode reorg shrink primitive

Target: pinned Parano1d commit `52938433a30b185ab007afaa15166a632cf2e82e` in `experiments/comp-system-hunter/pinned/`.

`CARGO_BUILD_JOBS=2 cargo test --release --offline --manifest-path experiments/verification-reorg/Cargo.toml -- --nocapture` exited 0. Its single test observed a release-enabled assertion from `noid_chain/src/segmented_state.rs:1533`: `segment commitment requested for evicted segment 0; hydrate it first`. The assertion occurred in `shrink_to_log_slots(24)` after expanding a durable metadata clone from depth 24 to 25. The resident-state control executed the same depth change, shrank without panic, and retained its original exact root. The full output is `release-test.log`.

The fixture uses the same `shrink_to_log_slots` primitive called by `noid_chain/src/storage/mdbx_context.rs:2068` while reverting a block in `apply_reorg_mdbx_with_objects`. The compact startup path also marks live evicted summaries raw-dirty in `noid_chain/src/segmented_state.rs:1113`, and the raw dirty marker survives durable metadata cloning. The test's `clear_dirty()` models the MDBX commit boundary; it only clears the MDBX-dirty set. It does not require 12.6 million live slots because it isolates the faulty reorg subroutine.

**Scope:** This is a successful component-level reproduction of a release-mode panic under the relevant state representation. It is not a completed MDBX reorg, mined expansion block, daemon crash, or chainwide halt. A full devnet demonstration would need the valid expansion-chain precondition and an actual competing suffix; that remains open.
