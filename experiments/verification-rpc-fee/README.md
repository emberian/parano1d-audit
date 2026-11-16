# SR-4 / CS-7: external miner claimable fee accounting

Pinned source: `experiments/comp-system-hunter/pinned/`, commit `52938433a30b185ab007afaa15166a632cf2e82e`.

The RPC computes `claimable_fees_micronoid` by summing `claimable_fee_for_tx_body` across each physical page in `noid_rpc/src/server.rs:1318-1329`. The template builder computes coinbase value from logical spend groups in `noid_chain/src/consensus/template.rs:939-951`. The former uses each page's input/output counts; the latter uses group-wide counts. A NativeIntent header page carries the entire fee and has no inputs or outputs. Later pages carry the inputs/outputs and zero fee. Consequently, the RPC expression can report as claimable the state-growth fee that the template burns.

`src/main.rs` constructs a local V2.1 chain under the isolated fork schedule, builds a signed three-input, four-output CreateLaunch using the production NativeIntent encoder and authorization prover, and passes its physical pages and openings through the production block-template builder. It compares the exact expression used by the RPC with `coinbase - miner_subsidy_at_height`, then mines PoW. The native block passes production `validate_block_checks` and native state materialization, including the header root check. A one-page ordinary signed spend is the control case and passes `validate_block_consensus`. Both cases assert that all submitted pages were selected. The local chain is cached under ignored `.local-chain/`, allowing rapid fixture refinement without repeating its PoW.

Observed in `run.log`:

| Case | Pages | Total fee | RPC expression | Coinbase less miner subsidy | Overreport |
|---|---:|---:|---:|---:|---:|
| Ordinary control | 1 | 100,000 | 100,000 | 100,000 | 0 |
| Native CreateLaunch | 3 | 10,600 | 10,600 | 8,100 | **2,500 μNOID** |

The NativeIntent has three live inputs, four live outputs, and a 2,500 μNOID state-growth burn. Its fee resides on the zero-I/O header page; the two later pages carry zero fee. The RPC per-page calculation therefore reports the burn as miner-claimable even though the miner's coinbase cannot include it.

Run:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=experiments/comp-system-hunter/core-seams/target cargo run --release --offline --manifest-path experiments/verification-rpc-fee/Cargo.toml
```

The fixture does not invoke the HTTP RPC endpoint. `getBlockTemplate` requires a running RPC node and full HistoryStep proof preparation. Its arithmetic is copied exactly from the pinned RPC source, using the selected production template. The local checks do not verify the HistoryStep recursive proof or submit the height-24 block to the full node acceptance pipeline.
