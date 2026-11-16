# Reorg shrink regression

This small crate tests the exact `SegmentedFriState::shrink_to_log_slots` call that the pinned `MdbxChainContext::apply_reorg_mdbx_with_objects` uses after reverting a state-tree expansion. It constructs a valid sparse state at mainnet slot depth 24, seals its exact root, and compares a resident control with a durable metadata clone whose live segment is evicted. The clone retains the raw FRI dirty marker; this is inferred from the release assertion because that marker has no public accessor.

Run `CARGO_BUILD_JOBS=2 cargo test --release --offline -- --nocapture` from this directory. The test catches the expected release-mode assertion so a passing test means the *local component failure* was observed.

The observed output is in `release-test.log`: one test passed after catching `segment commitment requested for evicted segment 0; hydrate it first`; the resident control returned the original exact root.

This fixture does not build a valid 75%-occupied chain, mine an expansion block, or execute a complete MDBX reorg. It therefore does not demonstrate a release daemon halt or network-wide halt. It isolates the state invariant and the exact shrink primitive reached by the production reorg call; those higher-level preconditions still need a separate controlled devnet run.
