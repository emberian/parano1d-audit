# CS-1: stale exact fee across a pressure drop

This fixture targets pinned source `52938433a30b185ab007afaa15166a632cf2e82e` in `../comp-system-hunter/pinned/`.

## Reachability boundary

The production State starts at `log_slots = 24`. Crossing the first 50% pressure boundary downward requires a valid parent with at least 8,388,608 live slots, followed by one or more spends that reduce occupancy below that number. No such valid threshold-crossing chain is built here. The high-pressure admission `ChainView` and its matching parent-header count/hash are synthetic; its resident UTXO State is the authentic low-occupancy local State. This view uses `ChainView::new` without an MDBX lazy store because `ChainView::from_mdbx` rejects a fabricated tip hash during slot reads. The subsequent block is accepted by the native consensus/PoW/MDBX path, with a stubbed HistoryStep terminal verifier. Therefore, a pass supports a **conditional component seam**, not a demonstrated release daemon halt or valid-chain threshold crossing.

## Test path

1. Mine 23 local empty blocks to activate isolated v2.1 and obtain real funding UTXOs.
2. Construct a signed `CreateLaunch` with 3 inputs and 4 outputs, so one net-new slot is charged at the synthetic 50% parent fee. Submit through real `AsyncMempool::submit_encoded` and proof verification.
3. Accept one genuine low-pressure empty block and call `AsyncMempool::on_new_block` with its authentic view. Check that the overpaying intent survives and is selected for the next candidate.
4. Run the selected pages through `build_node_owned_block_template_with_native_data` and require the financial `Fee` error. This is the same inner builder whose error `TemplateBuilder::build_from_snapshot_with_limit` maps to `None` in `noid_miner/src/template.rs`. Require an empty template at the same parent to succeed.
5. Sign the same `CreateLaunch` inputs/operation at the authentic parent’s exact lower fee, admit it, select it, and require the same inner template builder to succeed.

Run with:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/core-seams/target timeout 300 cargo run --release --offline --manifest-path /Users/ember/dev/parano1d-break/experiments/verification-stale-fee/Cargo.toml
```

The test does not instantiate `noid_miner::TemplateBuilder` or a release daemon. The call-site propagation to `None` and any persistent mining impact are source conclusions pending an end-to-end valid-chain fixture.
