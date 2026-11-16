# Wallet reorg ordering and object-opening verification

Source: the clean `experiments/comp-system-hunter/pinned/` archive of `parano1d` commit `52938433a30b185ab007afaa15166a632cf2e82e`. This directory includes the pinned production wallet module directly and links the pinned mempool. It does not alter the source or contact the live network.

The completed run is recorded in `run.log`. Reproduce with:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/core-seams/target \
  cargo run --offline --manifest-path experiments/verification-wallet-race/Cargo.toml
```

## SR-2: reservations captured before reorg mempool reconciliation

`noid_node/src/main.rs` captures `mempool.reserved_slots()` before the blocking reorg worker (`4357`), installs the owner snapshot with those sets (`4617`), and only afterward calls `mempool.on_new_block` and `readmit_after_reorg` (`4677–4688`). `WalletState::commit_verified_activation` uses the passed set to rebuild `pending_input_slots` (`state.rs:385–400`). I found no `TxEvicted` listener in the daemon that releases those wallet reservations.

The executable constructs a valid wallet-proof transaction and admits it to the pinned mempool. It then follows the ordering above using a controlled replacement owner snapshot, changes the ChainView so the transaction's output slot is occupied, and checks the actual `OutputSlotOccupied` eviction event. The control installs the same owner snapshot *after* eviction, with the now-empty reserved sets. This is an integration of the production wallet and mempool components, **not an MDBX/HistoryStep-accepted reorg**. It establishes the stale-reservation behavior if the committed replacement chain evicts such a pending transaction; a full release reorg remains to be demonstrated.

The observed wallet effect is temporary: spendable balance remains zero and address discovery refuses pending activity until a later wallet reload, explicit rollback, or restart. Pending reservations are in memory and are not restored from the keystore on restart. The chain's money is not destroyed or transferred.

## SR-3: address discovery can change the selected index during reorg

`walletDiscoverAddresses` does not take `wallet_operation_gate`. It holds `self.chain.read().await` while scanning owner indexes, but explicitly **drops the chain read lock** before `commit_address_discovery` (`noid_rpc/src/server.rs:2812–2838`). The exact-suffix worker can acquire `apply_chain.blocking_write()` in that gap (`noid_node/src/main.rs:4469`), then select the wallet's `(active_index,next_index)` and query its owner snapshot (`main.rs:4600–4620`). Discovery's commit can then advance `next_index` without the wallet operation gate. The install rejects this mismatch before pruning orphan artifacts. The caller only invalidates the UTXO cache.

The executable forces that interleaving at the production wallet functions using two threads and a barrier. The pinned installer rejects stale `next_index` before pruning orphan receipts and history; the caller's `invalidate_active_cache` only clears the UTXO cache. A no-discovery control prunes those artifacts. The source establishes a schedulable RPC/chain-lock gap, and the fixture establishes the state consequence. For this particular mismatch, discovery must actually find at least one funded next address and advance `next_index`; an empty discovery is harmless. This is **not yet a full MDBX/HistoryStep-accepted reorg plus concurrent RPC**. The direct `WalletState::create_or_load` restart in the fixture deliberately skips full daemon startup reconciliation: `reconcile_receipts_at_startup` checks canonical headers and removes orphan receipts when those headers exist (`wallet/mod.rs:307–361`, called from `main.rs:2182`). The in-process stale artifact is demonstrated; durable receipt persistence across a healthy full daemon restart is not.

## Object opening saved before admission

`noid_rpc/src/server/objects.rs:673–678` calls `remember_object_opening` for the current and successor openings before proof construction (`:683–690`) and mempool submission (`:698–704`). The wallet method calls `object_files::save_opening`, which atomically writes and syncs the `.opening` file (`noid_node/src/wallet/object_files.rs:97–103`). The fixture checks that the file exists and reloads after that call, with no transaction admission. A failed proof or admission after this point has no rollback in the RPC function. This is a local artifact side effect, not a consensus or remote-node corruption claim.
