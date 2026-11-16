# Object and native-template claim check

Target: Parano1d commit `52938433a30b185ab007afaa15166a632cf2e82e`, read from the byte-matched `../comp-system-hunter/pinned/` source copy. The control in `src/main.rs` uses synthetic addresses and values; it does not start a node or contact a network.

## NM-4B: zero-address object policies

`Address::parse` accepts a canonical bech32m encoding of 32 zero bytes (`noid_poseidon2b/src/primitives.rs:133-148`). The RPC `createObject` parser calls it without a nonzero check (`noid_rpc/src/server.rs:3087-3094`), and `ObjectOpening::validate` checks rule bits and instruction encodings, not authority or recipient values (`noid_tx/src/experimental_object.rs:86-94`). The predefined `refundable_payment` constructor copies payer/payee into both authority and recipient fields (`applications.rs:20-40`). Consequently, an all-zero payer/payee definition can be accepted as an object descriptor; the control exercises the same production parser, constructor, opening validation, and opening wire round trip used by `walletFundObject`. `pruv native`'s `Owner` type rejects zero explicitly (`noid_pruv/src/native.rs:84-93`), and the control verifies that contrast.

This is an input-validation footgun for a person intentionally creating and then funding such an object, not an operator power or a remote way to lock another holder's coins. `createObject` returns a descriptor only (`noid_rpc/src/server.rs:1679-1683`). Funding is a separate `walletFundObject` RPC (`server.rs:1700-1715`) or CLI `contract fund` action (`noid_node/src/bin/noid_cli/contracts.rs:323-326`). A zero authority alone is not necessarily permanent loss if the later recovery authority is nonzero. An all-zero authority pair has no known spend key under the protocol's address-preimage assumption; we did not prove one cannot exist or submit a funded object.

## Native-template expiry and quantity checks

`NativeTemplate::validate` accepts a positive `valid_until` far beyond the current height and token transfer/burn quantities above `LAUNCH_MAX_SUPPLY` (`noid_pruv/src/native.rs:186-255`). The control verifies those static acceptances and positive/zero expiry controls. The node wallet planner supplies current height and rejects trade expiry beyond 30 blocks (`noid_rpc/src/server/native/planner.rs:892-895`). This is a preview-stage rejection in the Pruv workflow, not a valid transaction or consensus disagreement. The underlying trade policy allows a longer expiry if an independently constructed intent satisfies current height, epoch anchor, funding, and fee checks (`noid_tx/src/native_assets/trade.rs:75-109`; `noid_mempool/src/trade.rs:91-147`); the pool has capacity/byte limits and evicts on anchor change (`noid_mempool/src/trade.rs:149-219`). The [native-template instructions](../comp-system-hunter/pinned/docs/contracts/native-templates.md) describe `pruv native check` as static checking followed by node preview and review; they say the node selects current State, inputs, and fees. The launch ceiling constrains launch supply (`noid_tx/src/native_assets/launch.rs:26-47`), while the planner's token path selects actual balances and fails if they cannot cover a requested transfer/burn (`noid_rpc/src/server/native/planner.rs:199-255`). Thus lack of that supply check in the template does not establish a way to transfer or burn extra tokens.

## CD-4: standalone validation partition

Gemini's [CD-4 report](../comp-consensus-diff/REPORT.md) itself labels this informational. `validate_block_consensus` says it is not the live full-node production path (`noid_chain/src/consensus/validation.rs:567-573`); there are no Rust call sites in the pinned source. `materialize_accepted_block_state` explicitly requires prior full `HistoryStep` verification (`noid_chain/src/block.rs:159-166`). The live accepted-bundle path runs `validate_block_checks`, checks that the terminal binds the exact candidate, verifies the terminal, then materializes with native data (`noid_chain/src/storage/mdbx_context.rs:1357-1425`, `1450-1465`; native records at `4030-4056`). The recursive-suffix path checks and materializes each body under a previously verified suffix authority (`mdbx_context.rs:1532-1580`). Mempool candidate admission calls `ObjectOpening::check_call` (`noid_mempool/src/contracts.rs:72-89`). This is a documented API layering boundary, not a demonstrated bypass in the production acceptance path.

## Run

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=../comp-system-hunter/core-seams/target \
  cargo run --offline --manifest-path Cargo.toml
```

`results.txt` records the bounded control's output. This test does not fund an object, execute RPC or a full `HistoryStep`, or establish a chain halt.
