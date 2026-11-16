# Receipt reconciliation after a shorter, heavier reorg

This fixture tests the receipt claim from Gemini SR-3 / CS-6 against the
archived Parano1d source at `52938433a30b185ab007afaa15166a632cf2e82e`.
It compiles the archived daemon wallet module directly from
`../comp-system-hunter/pinned/noid_node/src/wallet/mod.rs` and calls its
startup reconciliation, post-reorg wallet install, accepted-block update, and
`WalletOps::receipts` implementation. No archived source was changed.

Run from the repository root:

```sh
CARGO_BUILD_JOBS=2 \
  CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/core-seams/target \
  cargo run --offline --manifest-path experiments/verification-receipts/Cargo.toml
```

The successful run is saved in [`run.log`](run.log). Its assertions establish:

1. A payment from the local wallet's address was accepted in an old-branch
   block at height 5. The generated `ParanoidReceipt` verifies against both
   its Merkle root and the then-canonical height 5 header.
2. Four fast PoW-valid blocks had more cumulative work than five old-branch
   blocks. MDBX committed a valid shorter, heavier reorg to height 4 and
   returned the payment's logical ID in `reclaimed_tx_hashes`. The canonical
   height 5 header was absent afterward.
3. The **ordinary reorg path** removed the receipt with
   `install_reorg_snapshot_and_artifacts`. It stayed removed after wallet
   restart. The broad claim that normal reorgs always leave orphan payment
   receipts is refuted.
4. In a **crash cut after the durable chain commit but before that wallet
   update**, the persisted wallet receipt survived startup reconciliation:
   `(removed, recovered) == (0, 0)`. `WalletOps::receipts` listed it with
   claimed height 5 while the canonical tip was height 4.
5. When a replacement-branch block later reached height 5, the production
   accepted-block wallet update left the receipt listed even though the
   canonical height 5 header contradicted it. The *next wallet restart*
   removed it: `(removed, recovered) == (1, 0)`.

The relevant startup predicate is in
`pinned/noid_node/src/wallet/mod.rs:307-418`: after validating the receipt's
contents and Merkle path, it treats an absent canonical header at the claimed
height as acceptable. This is intentional for a chain-data purge followed by
snapshot bootstrap, but it also covers the reorg crash cut. Normal cleanup is
in `wallet/mod.rs:425-550`. The daemon performs the chain reorg commit before
the wallet update in `noid_node/src/main.rs:4570-4645`; startup calls receipt
reconciliation at `main.rs:2166-2185`. The RPC receipt list reads the wallet
cache without a canonical-header lookup at `wallet/mod.rs:912-961`.

## Scope

- The branch blocks use the pinned production template, PoW check, native
  state materialization, and MDBX reorg implementation. The fixture supplies
  a structurally valid terminal metadata envelope and explicitly bypasses
  **HistoryStep recursive-proof verification** in its `apply_next_block`
  callback. It does not prove that the synthetic payment's authorization or
  recursive terminal would pass the full daemon admission path.
- The payment receipt is genuine for the accepted local block and is checked
  with the pinned production receipt verifier. The fixture inserts it into the
  wallet's durable cache to model a saved receipt; it does not invoke the
  scanner that would normally produce it.
- This is a process-crash timing case, not a malicious reorg bypass. The
  fixture models the cut by copying the durable wallet files before invoking
  the post-commit wallet step. It did not kill a daemon process or exercise
  P2P sync. The daemon's source ordering makes the cut reachable, but daemon
  occurrence and frequency were not measured.
- The demonstrated effect is a stale local wallet receipt and RPC listing.
  It does not establish a chain halt, spendable orphan outputs, or a way to
  force a counterparty that checks canonical headers to accept the receipt.
- This run covers **ordinary payment receipts**. Native and trade receipt
  reconciliation have a similar missing-header predicate in
  `wallet/native_receipts.rs:177-210`, but no native/trade artifact was created
  here, and their full daemon behavior remains untested.

The source change suggested by this result is to preserve the snapshot
bootstrap policy while distinguishing an unavailable height above the current
tip after a reorg from missing historical data during bootstrap. Another
option is a durable wallet/chain reorg marker that causes reclaimed receipt
IDs to be replayed on startup before RPCs are served.
