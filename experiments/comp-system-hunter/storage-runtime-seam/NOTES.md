# Storage, Native Index, Wallet Runtime & External Miner Seam Audit (`storage-runtime-seam`)

> **Replication status (2026-11-15):** The original seam test had no completed run at review; several cases now have independent component or local-chain controls. See the [verification ledger](../../comp-verification/VERIFICATION.md) and [claim matrix](../../comp-verification/REPLICATION-MATRIX.md) for exact scope.

- **Pinned Commit Audited**: `52938433a30b185ab007afaa15166a632cf2e82e` (`/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/pinned/`)
- **Profile**: Default release mainnet schedule (`ACTIVE_SCHEDULE = MAINNET_SCHEDULE`, `LOG_SLOTS_GENESIS = 24`, `LOG_SEGMENT_SIZE = 16`, `MAINNET_V2_1_ACTIVATION_HEIGHT = Some(279_370)`)
- **Prior Findings Excluded from Duplication**: `C15` (`submitBlock` stale template), `C19` (concurrent wallet scan TOCTOU), `C20` (`utxo_meta` / `recent_blocks` startup gap), `C24` (`submitWork` height check), `U19` (post-fix `MdbxChainContext` verification)
- **Harness Status**:
  - Workspace scaffolded in `/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam/`:
    - [Cargo.toml](file:///Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam/Cargo.toml)
    - [src/lib.rs](file:///Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam/src/lib.rs) (mounts `pinned/noid_node/src/wallet/mod.rs` directly)
    - [src/helpers.rs](file:///Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam/src/helpers.rs) (`CARGO_TARGET_DIR="$PWD/target" cargo check --offline` verified **exit code 0** against pinned workspace)
    - [tests/conformance_seams.rs](file:///Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam/tests/conformance_seams.rs) (ready-to-execute Rust conformance tests left on disk per time-check cutoff before running `cargo test --release`)

---

## Executive Summary of Confirmed Discrepancies

| ID | Severity | Seam | Component & Exact Line Citations | Core Mechanism & Runtime Effect |
|---|---|---|---|---|
| **SR-1** | **HIGH** | Seam 4 (`log_slots` Expansion × Reorg × `ExactSegmentCache`) | `noid_chain/src/storage/mdbx_context.rs:2068`<br>`noid_chain/src/segmented_state.rs:1126-1127, 1331, 1533`<br>`noid_chain/src/exact_segment_cache.rs:17`<br>`noid_node/src/main.rs:4469, 4576` | `apply_reorg_mdbx_with_objects` calls `shrink_to_log_slots` (instead of `shrink_exact_metadata_to_log_slots`), which calls `flush_all_dirty()` across `SegmentedFriState::dirty`. Because production never clears `SegmentedFriState::dirty` and the 64 MiB `ExactSegmentCache` retains at most **9** of the **>= 192** live segments required to reach the 75% `log_slots = 24 -> 25` threshold (and compact startup marks all persisted segments `evicted ∩ dirty`), any reorg across an expansion block hits `assert!(!self.evicted.contains(&seg_id))` in `--release` while holding `chain.blocking_write()`. |
| **SR-2** | **MEDIUM-HIGH** | Seam 1 (Wallet Pending State × Mempool Eviction × Reorg Ordering) | `noid_node/src/main.rs:2359-2370, 4358, 4617, 4677-4686`<br>`noid_node/src/wallet/mod.rs:138-205, 1115, 1596-1630`<br>`noid_node/src/wallet/state.rs:278, 419-484`<br>`noid_rpc/src/server.rs:2593-2608, 2779-2841` | (a) Daemon ignores `MempoolEvent::TxEvicted`; evicted ordinary/contract/native/trade intents retain wallet slot & `native_pending` reservations in read RPCs and fail `walletDiscoverAddresses` while mempool is empty. (b) `apply_exact_suffix_offthread` snapshots `mempool.reserved_slots()` at line `4358` *before* reorg application and `mempool.on_new_block` / `readmit_after_reorg` (`4677-4686`). (c) Evicted ordinary/contract sends leave `TxHistoryEntry { height: 0, direction: Sent }` in `wallet.history` permanently across `walletScan` and daemon restarts. |
| **SR-3** | **MEDIUM** | Seam 3 (Shorter-Heavier-Fork Reorg × Receipt Reconciliation × `walletDiscoverAddresses` Race) | `noid_node/src/wallet/mod.rs:340-355, 513-561`<br>`noid_node/src/wallet/native_receipts.rs:186-194`<br>`noid_node/src/main.rs:4600-4655`<br>`noid_rpc/src/server.rs:2779-2841`<br>`noid_rpc/src/server/objects.rs:675-695` | (a) `reconcile_receipts_at_startup` and `native_receipts::reconcile_canonical` treat `store.get_header(claimed_height) == None` as `keep = true` without checking `claimed_height > tip_height`, retaining orphaned receipts after shorter, higher-work reorgs (`H_new < H_old`) across startup and subsequent canonical catch-up. (b) `walletDiscoverAddresses` bypasses `wallet_operation_gate` (`server.rs:2805`), mutating `next_index` and causing `install_reorg_snapshot_and_artifacts` to fail `commit_verified_activation` before pruning orphaned Fork-A receipts and history. (c) `objects::call` writes `.opening` files before admission and `object_files` never prunes unmined/reorged `.opening`, `.receipt`, or `activity-v1` files. |
| **SR-4** | **LOW-MEDIUM** | External Miner / RPC Fee Accounting | `noid_rpc/src/server.rs:1318-1329`<br>`noid_chain/src/consensus/validation.rs:248-286`<br>`noid_chain/src/consensus/template.rs:940-956` | `prepare_external_mining_attempt` sums `claimable_fee_for_tx_body` per physical `TxBody` instead of per logical `ScheduledBlockPageGroup`. On every v2.1 Native Asset group, page 0 (`NATIVE_HEADER_BIT`) has `0` live inputs/outputs (`burned = 0`) and `fee = total_fee`, while pages `1..k` have `fee = 0`, so `getBlockTemplate` omits all burned state-growth fees (`+2,500 µNOID * net_new_slots` over-reported vs `coinbase_value_micronoid - miner_subsidy`) and miscomputes multi-page `PagedSpend` fees in both directions. |
| **SR-5** | **LOW (Local-Tamper Extension of C20)** | Seam 2 (MDBX Secondary Indexes on Startup) | `noid_chain/src/storage/mdbx_context.rs:400-624, 738-837`<br>`noid_chain/src/storage/native_payloads.rs:514-605` | Under honest consensus and reorgs, `native_payloads`, `native_wallet_page`, `native_trade_parent_at`, and `native_catalog_page` are exact (verified below). However, neither `try_load_compact_chain_state` nor `load_dense_chain_state` verifies or rebuilds `T_OWNER_INDEX`, `T_NATIVE_OWNER_INDEX`, or `native_payloads::TABLES`; local corruption passes startup without triggering `Corrupt` clear-and-resync recovery and fails closed (`StoreError::Decode`) on subsequent block commits touching affected entries. |

---

## Detailed Technical Analysis & Code Citations

### 1. SR-1 (HIGH — Seam 4): Unhydrated Evicted-Segment Assertion in `apply_reorg_mdbx_with_objects` When Reverting `log_slots` Expansion

#### Code Path & Invariant Mismatch
1. **Production Never Flushes `SegmentedFriState::dirty`**:
   - `ChainState::try_state_root()` (`noid_chain/src/state.rs:680-702`) computes exact UTXO roots over `self.exact_dirty` and clears `self.exact_dirty`, leaving `self.state.dirty` (`SegmentedFriState::dirty`, which tracks legacy FRI `seg_roots[seg_id] == None` at `noid_chain/src/segmented_state.rs:630, 1027`) populated.
   - `commit_applied_next_block` (`noid_chain/src/storage/mdbx_context.rs:1180-1185`) calls `self.state.clear_dirty()` (clearing `mdbx_dirty` and `exact_dirty`) and then `self.state.evict_cold_persisted_segments()`.
   - Compact startup (`try_load_compact_chain_state` -> `install_evicted_exact_summary`, `noid_chain/src/storage/mdbx_context.rs:545` -> `noid_chain/src/segmented_state.rs:1126-1127`) and snapshot installation (`noid_chain/src/storage/mdbx_store.rs:3429`) insert every persisted non-empty segment into **both** `self.evicted` and `self.dirty`.
2. **Guaranteed Eviction Before Mainnet `log_slots = 24 -> 25` Expansion (Even With Zero Restarts)**:
   - `ExactSegmentCache::DEFAULT_CACHE_BYTES` is `64 MiB` (`noid_chain/src/exact_segment_cache.rs:17`). At `LOG_SEGMENT_SIZE = 16` (`65,536` slots/segment), each resident segment accounts for `3 MiB` (`SegmentColumns::HEAP_BYTES`) + `4 MiB` (`ExactSegmentTree::HEAP_BYTES`) = **`7 MiB`**, capping residency at **`floor(64 / 7) = 9` segments** (`9 * 7 = 63 MiB`; `10 * 7 = 70 MiB > 64 MiB`).
   - By `expected_child_log_slots` (`noid_chain/src/consensus/slot_expansion.rs:34-62`), expanding from `log_slots = 24` to `25` requires a strict majority (`>= 10` of `18`) of finalized window headers (`H - 35 ..= H - 18`) to have `active_slot_count >= (2^24 * 3) / 4 = 12,582,912`.
   - Across `65,536`-slot segments, `12,582,912` active slots require at least **`12,582,912 / 65,536 = 192` non-empty segments**. Therefore, **at least `192 - 9 = 183` non-empty segments are guaranteed to reside in `self.evicted ∩ self.dirty` on every mainnet node** when the expansion height is reached, even without any daemon restart.
3. **Asymmetric Shrink Call in `apply_reorg_mdbx_with_objects`**:
   - Uncommitted block rollback (`rollback_uncommitted_block`, `noid_chain/src/storage/mdbx_context.rs:1008-1020`) and historical views (`noid_chain/src/historical_state.rs:425`) call `shrink_exact_metadata_to_log_slots(...)` (`noid_chain/src/segmented_state.rs:1357-1404`), which shrinks vector lengths without touching `flush_all_dirty()`.
   - In contrast, `apply_reorg_mdbx_with_objects` (`noid_chain/src/storage/mdbx_context.rs:2066-2071`) executes:
     ```rust
     Self::preload_segments_for_undo_in_state(&self.store, &mut self.state, &undo)?;
     reclaimed_tx_hashes.extend_from_slice(&undo.tx_hashes);
     revert_block(&mut self.state, &undo)...?;
     self.state
         .state
         .shrink_to_log_slots(undo.log_slots_before as usize)...?;
     ```
   - `preload_segments_for_undo_in_state` (`mdbx_context.rs:2061`) hydrates **only** segments referenced in `undo.slot_changes` of the reverted expansion block (at most `BLOCK_MAX_RESIDENT_SEGMENTS` segments).
   - Note that because `expected_child_log_slots` depends solely on hard-finalized headers (`>= 18` blocks deep), **even a routine 1-block competing-tip reorg at the expansion height (`H_A` -> `H_B`, where both `H_A` and `H_B` expand `24 -> 25`)** first reverts `H_A` from `25` back to `undo.log_slots_before = 24` before applying `H_B`!
   - `shrink_to_log_slots` (`noid_chain/src/segmented_state.rs:1331`) unconditionally calls `self.flush_all_dirty()` -> `self.flush_segment(seg_id)` (`noid_chain/src/segmented_state.rs:1533`), hitting the release-enabled `assert!`:
     ```rust
     assert!(
         !self.evicted.contains(&seg_id),
         "segment commitment requested for evicted segment {seg_id}; hydrate it first"
     );
     ```
   - In `apply_exact_suffix_offthread` (`noid_node/src/main.rs:4469, 4576`), this unwinds inside `tokio::task::spawn_blocking` while holding `let mut ctx = chain.blocking_write()`, poisoning the `RwLock` and terminating node state progression.
4. **Secondary `debug_assertions` Invariant Mismatch (`noid_chain/src/segmented_state.rs:1283-1314`)**:
   - After `load_dense_chain_state` (`noid_chain/src/storage/mdbx_context.rs:467`, which calls `flush_all_dirty()` and populates `self.tree[1]`), subsequent block mutations apply via `apply_delta_unrooted` (setting `seg_roots[id] = None` while leaving `self.tree[1]` at its stale root). Calling `SegmentedFriState::expand` (`segmented_state.rs:1283`) snapshots `old_root = self.tree[1]`, rebuilds `new_tree` substituting `zero_leaf` for `None` entries (`line 1303`), and triggers `debug_assert_eq!(self.tree[1], compress(&old_root, &zero_segtree_node(old_depth)))` (`line 1310`) in debug builds.
5. **Recommended Fix**: Replace `shrink_to_log_slots` at `noid_chain/src/storage/mdbx_context.rs:2068` with `shrink_exact_metadata_to_log_slots(undo.log_slots_before as usize)`.

---

### 2. SR-2 (MEDIUM-HIGH — Seam 1): Wallet Pending-Slot Reservation Retention on `TxEvicted`, Reorg Snapshot Ordering, & Unpruned `height: 0` History

#### Three Distinct State-Machine Desynchronizations
1. **`MempoolEvent::TxEvicted` Ignored by Wallet Runtime (`noid_node/src/main.rs:2359-2370`)**:
   - The mempool event loop in `noid_node/src/main.rs:2359-2370` matches `Ok(noid_mempool::MempoolEvent::TxAdmitted { .. })` and discards all other events via `Ok(_) => {}` (never inspecting `MempoolEvent::TxEvicted`).
   - On each accepted block, `wallet::update_for_accepted_block` (`noid_node/src/wallet/mod.rs:138-205` and `noid_node/src/wallet/native.rs:238-267`) clears `pending_input_slots`, `pending_output_slots`, and `native_pending` **only** when the reserved slot indices appear in `block.transactions`.
   - When `AsyncMempool::on_new_block` (`noid_mempool/src/pool.rs:765-895`) evicts an unmined transaction without spending the wallet's input — e.g.:
     - `EvictReason::TradePolicyExpired` (`valid_until < next_height`),
     - `EvictReason::EpochAnchorChanged` (at every 144-block epoch rollover),
     - `EvictReason::OutputSlotOccupied` (when a coinbase or unrelated transaction allocates the hinted empty output slot),
     - `EvictReason::ConsensusFeeIncreased` or `EvictReason::NativeContextChanged`,
     the mempool becomes empty (`getMempoolSize == 0`), while `WalletState` retains `pending_input_slots`, `pending_output_slots`, and `native_pending`.
   - **Exact RPC Impact Boundary**:
     - Read/polling RPCs (`paranoid_walletGetBalance` at `noid_rpc/src/server.rs:2593`, `paranoid_walletListUtxos` at `2604`, `paranoid_walletUtxoSnapshot` at `2608`) read cached `WalletState` directly without calling `reload_active_wallet()`, reporting `spendable_micronoid == 0` (or reduced) and `reserved: true` indefinitely.
     - `paranoid_walletDiscoverAddresses` (`noid_rpc/src/server.rs:2779-2841` -> `noid_node/src/wallet/mod.rs:1115`) checks `w.has_pending_activity()` (`noid_node/src/wallet/state.rs:278`) without reconciling against `mempool.reserved_slots()`, failing with `"cannot discover addresses while a wallet transaction is pending"`.
     - Active write/planning RPCs (`walletSend`, `walletPlanSend`, `walletConsolidate`, `walletScan`, `walletSetActiveAddress`) invoke `reload_active_wallet()` (`commit_verified_activation`, `wallet/state.rs:379-404`), and `walletPreviewNative` / `walletSubmitNative` / `walletNativePending` invoke `reconcile_pending` (`noid_rpc/src/server/native.rs:20-32`), which clear the stale slot reservations once invoked.
2. **Pre-Eviction `mempool.reserved_slots()` Capture on Reorgs (`noid_node/src/main.rs:4358` vs `4617` vs `4677-4686`)**:
   - In `apply_exact_suffix_offthread`:
     - Line `4358`: `let (reserved_input_slots, reserved_output_slots) = mempool.reserved_slots().await;` is captured **before** reorg execution.
     - Line `4576`: `ctx.apply_verified_reorg_suffix_with_applier_indexed(...)` commits the new fork.
     - Line `4617`: `wallet::install_reorg_snapshot_and_artifacts(..., &reserved_input_slots, &reserved_output_slots, ...)` installs wallet reservations from the **stale pre-reorg** mempool snapshot.
     - Lines `4677-4686`: `mempool.on_new_block(&new_tip_block, view).await` and `mempool.readmit_after_reorg(...)` run **after** the wallet snapshot was already installed, evicting entries invalidated by the new branch (`EpochAnchorChanged`, `NativeContextChanged`, `OutputSlotOccupied`, `TradePolicyExpired`) while leaving their slots reserved in `WalletState`.
3. **Permanent `height: 0` History Entries After Mempool Eviction (`noid_node/src/wallet/state.rs:419-484`)**:
   - `reserve_pending_submission` (`noid_node/src/wallet/mod.rs:1596-1630`, called by `walletSend`, `walletConsolidate`, and `noid_rpc/src/server/objects.rs:689` `walletCallObject`) calls `w.record_pending_send(...)` (`wallet/state.rs:419-444`) and `w.save()`, persisting a `TxHistoryEntry { height: 0, direction: TxDirection::Sent, ... }` to `wallet.json`.
   - `rollback_pending_submission` -> `remove_pending_send` (`wallet/state.rs:446-484`) is called **only** if initial `mempool.submit` returns `Err`.
   - Once `mempool.submit` returns `Ok`, if the transaction is later evicted from `AsyncMempool` and never mined, **no code path in `noid_node` or `noid_rpc`** (`commit_verified_activation` / `walletScan`, `update_for_accepted_block`, `install_reorg_snapshot_and_artifacts`, or `reconcile_receipts_at_startup`) ever removes unconfirmed `height == 0` entries from `wallet.history`.

---

### 3. SR-3 (MEDIUM — Seam 3): Shorter-Heavier-Fork Receipt Survival, `walletDiscoverAddresses` Reorg Race, & Unpruned Object Artifacts

#### (A) `get_header(claimed_height) == None` Treated as Canonical Above `tip_height`
- Under cumulative-chainwork fork choice (`choose_chain_by_work`, `noid_chain/src/storage/mdbx_context.rs:1838`), a shorter fork with higher cumulative PoW (`H_new < H_old`, within `CONSENSUS_FINALITY_DEPTH = 18`) replaces a taller canonical branch. `commit_reorg` (`noid_chain/src/storage/mdbx_store.rs:2972-2985`) deletes canonical headers for `(H_new + 1)..=H_old`, so `store.get_header(h)` returns `Ok(None)` for `H_new < h <= H_old`.
- **Payment receipts (`noid_node/src/wallet/mod.rs:340-355`)**:
  ```rust
  w.receipts.retain(|receipt| match chain.store.get_header(receipt.claimed_height) {
      Ok(Some(header)) => receipt.verify_against_header(&header),
      Ok(None) => true,
      Err(err) => { ... true }
  });
  ```
  `reconcile_receipts_at_startup` omits checking `receipt.claimed_height <= chain.tip_height()`.
- **Native & trade receipts (`noid_node/src/wallet/native_receipts.rs:186-194`)**:
  ```rust
  let invalid = get_header(claimed_height)?
      .is_some_and(|header| !receipt.verify_against_header(&header));
  if invalid { ... }
  ```
  Called both at startup and live after every P2P reorg via `retain_object_receipts_at_tip` (`noid_node/src/main.rs:4647`). When `claimed_height > H_new`, `get_header` returns `Ok(None)` -> `invalid == false` -> orphaned Fork-A native/trade receipts at `(H_new + 1)..=H_old` are retained!
- Furthermore, if the daemon crashes between MDBX reorg commit (`main.rs:4576`) and wallet receipt save (`main.rs:4617`), or if `install_reorg_snapshot_and_artifacts` aborts (see 3B), orphaned payment receipts at `claimed_height > H_new` survive `reconcile_receipts_at_startup`. When the canonical chain later extends through `claimed_height`, `wallet::update_for_accepted_block` never re-verifies existing `w.receipts`, so `walletReceipts`, `walletQueryReceipts`, and `walletExportReceipt` continue serving the orphaned Fork-A receipt (while `paranoid_verifyReceipt` reports `confirmed: false`) until a subsequent restart after `tip_height >= claimed_height`.

#### (B) `walletDiscoverAddresses` Bypasses `wallet_operation_gate` & Aborts Reorg Receipt/History Cleanup
- Unlike `walletScan`, `walletSend`, and `walletSetActiveAddress`, `paranoid_walletDiscoverAddresses` (`noid_rpc/src/server.rs:2779-2841`) does **not** acquire `self.wallet_operation_gate.lock().await` before calling `wallet.commit_address_discovery(&preview)` (which increments `wallet.next_index` and saves `wallet.json`).
- During a P2P reorg in `apply_exact_suffix_offthread` (`noid_node/src/main.rs:4600-4636`):
  1. `wallet::active_scan_selection(&wallet)` (`main.rs:4601`) snapshots `(active_index, next_index, owner)` and releases the `SharedWallet` mutex.
  2. `ctx.store.get_verified_utxos_by_owner` runs outside the `SharedWallet` mutex (`main.rs:4605-4615`).
  3. If `walletDiscoverAddresses` commits during step 2, `wallet::install_reorg_snapshot_and_artifacts` (`noid_node/src/wallet/mod.rs:513-549`) calls `w.commit_verified_activation(...)` (`noid_node/src/wallet/state.rs:327`), which fails with `"wallet address state changed during operation; retry"` **before** reaching lines `528-544` (which prune `reclaimed` tx hashes and `reverted_set` heights from `w.receipts` and `w.history`).
  4. The error handler at `main.rs:4631-4635` calls `wallet::invalidate_active_cache(&wallet)` (`wallet/mod.rs:550-561`), which clears `utxos` and pending sets so balance reloads lazily, **but never prunes `w.receipts` or `w.history`**. Moreover, `reconcile_receipts_at_startup` never reconciles `w.history` against `entry.block_hash`, leaving orphaned Fork-A `Sent`/`Received` history entries permanently in `wallet.json`.

#### (C) Contract `objects/` Directory Retains Unmined & Orphaned Files
- `noid_rpc/src/server/objects.rs:675-680` writes successor `.opening` files via `wallet.save_object_opening(&next_opening)` *before* `mempool.submit_encoded` (`line 694`), with no rollback on submission error, mempool eviction, or reorg.
- `noid_node/src/wallet/object_files.rs:213-301` and `object_files/activity.rs` never delete `.opening`, `.receipt`, or `activity-v1` files on reorgs (`retain_object_receipts_at_tip` at `main.rs:4647` reconciles only `native-receipts-v1`, not `objects/*.receipt`). `walletListObjectReceipts` marks orphaned `.receipt` files `canonical: false`, while `walletListObjectStates` continues listing unmined/orphaned `.opening` states with `has_balance: false`.

---

### 4. SR-4 (LOW-MEDIUM — External Miner / RPC): `getBlockTemplate` Per-Body `claimable_fees_micronoid` Discrepancy

- **Consensus Rule (`noid_chain/src/consensus/validation.rs:248-286` & `noid_chain/src/consensus/template.rs:940-956`)**:
  - Parses `ScheduledBlockPageStream` into logical groups (`ScheduledBlockPageGroup`). Each group stores its total fee on `pages[0].body.fee` while `pages[1..].body.fee == 0`, and computes state-growth burn across the **aggregate** `(group.live_inputs(), group.live_outputs())`:
    $$\text{burned}(\text{group}) = \max(0,\, L_{\text{out}} - L_{\text{in}}) \times \text{state\_growth\_fee\_per\_slot}$$
    $$\text{claimable}(\text{group}) = \text{group.fee}() - \text{burned}(\text{group})$$
- **External Miner RPC Calculation (`noid_rpc/src/server.rs:1318-1329`)**:
  ```rust
  let claimable_fees: u64 = tmpl
      .inner
      .txs
      .iter()
      .map(|tx| {
          noid_chain::consensus::claimable_fee_for_tx_body(
              &tx.body,
              tmpl.parent.active_slot_count,
              tmpl.parent.log_slots,
          )
      })
      .fold(0u64, u64::saturating_add);
  ```
- **Deterministic Divergences**:
  1. **Every v2.1 Native Asset Group (`NATIVE_HEADER_BIT` on page 0)**:
     - By `noid_tx/src/native_assets/wire.rs`, page 0 is a native metadata header with `0` live inputs and `0` live outputs (`burned_fee_for_tx_body == 0`) carrying `body.fee = group_fee`, while pages `1..k` carry all live inputs/outputs with `body.fee = 0` (`0u64.saturating_sub(burned) == 0`).
     - Consequently, `claimable_fees_micronoid` in `getBlockTemplate` **never subtracts state-growth burn on any native asset transaction** (e.g., `CreateLaunch` with `1` input and `5` outputs burns `4 * 2,500 = 10,000 µNOID` at low occupancy; RPC reports `claimable_fees_micronoid = consensus_claimable + 10,000`, directly contradicting `coinbase_value_micronoid - miner_subsidy` in the same JSON response).
  2. **Multi-Page Ordinary `PagedSpend` Groups**:
     - Over-reports on multi-page fan-outs (`1` input -> `9` outputs across 5 pages: page 0 sees `1 -> 2` = `1` burned slot instead of `8` net burned slots -> `+17,500 µNOID` over-reported).
     - Under-reports on consolidations whose page 0 has `1` input and `2` outputs while page 1 spends `8` inputs and `0` outputs (`9 -> 2` net shrinks state by 7 slots so consensus `burned == 0`, whereas page 0 alone sees `1 -> 2` and deducts `2,500 µNOID`).

---

### 5. Seam 2 Assessment: Verified-Tight Boundaries vs Local-Tamper Startup Extension (SR-5)

#### Verified-Tight Under Honest Consensus & Reorgs
- **Identical-Root (`NativeOpening`) Multi-Slot Reference Counting (`noid_chain/src/storage/native_payloads.rs:514-780`, `mdbx_store/native_wallet.rs:60-145`)**:
  - When two distinct UTXO slots carry byte-identical `NativeOpening::TokenBalance` values (same `asset`, `owner`, `units`, `unlock_height` -> identical `opening.root()`), `apply_block_tx` increments `PayloadRecord::live_slots` to `2` using `BTreeMap<[u8; 32], PayloadDelta>` aggregation and updates `T_NATIVE_OWNER_INDEX` safely.
  - `native_wallet_page` paginates over `(root, carrier.slot_index)` via `NativeWalletCursor { after_root, from_slot }`, returning all distinct slot carriers sharing one `root` without skipping or duplicating across `limit = 1` page boundaries.
  - `revert_tx` (`replace_refs`) restores exact `live_slots`, `auxiliary_refs`, and `last_seen_height` (`old.last_seen_height.max(height)` monotonic age index) across partial spends, full spends, reorgs, and compact restarts.
- **Launchpad Curve Graduation -> Reorg Rollback (`mdbx_store/native_catalog.rs:140-290`)**:
  - `CreateLaunch` -> curve graduation (`LaunchState` -> `MarketPool`) -> reorg rollback cleanly restores `native_trade_parent_at` from `TradeMarket::Pool` back to `TradeMarket::Launch` and removes the reverted `MarketPool` from `native_catalog_page(NativeCatalogKind::Markets)` because catalog/trade-parent lookups cross-verify live carrier existence via `get_verified_utxos_by_owner(&root)` against `T_SEGMENTS` + `tip_hash`.

#### SR-5 (LOW — Local-Write / Bit-Rot Extension of C20)
- In `MdbxChainContext::open_or_create` (`noid_chain/src/storage/mdbx_context.rs:400-624, 738-837`), neither `try_load_compact_chain_state` nor `load_dense_chain_state` validates `T_OWNER_INDEX`, `T_NATIVE_OWNER_INDEX`, or `native_payloads::TABLES` against `T_SEGMENTS`.
- If a local crash/bit-rot corrupts `native_payloads` (`live_slots` underflow or malformed record) or `T_OWNER_INDEX`, `open_or_create` succeeds (even if `segment_summaries` is cleared to force dense segment verification) instead of returning `MdbxContextError::Corrupt` to trigger `main.rs` clear-and-resync recovery, causing subsequent block commits touching that owner/payload to fail closed with `StoreError::Decode`.

---

## Follow-Up Execution Checklist for Next Audit Pass

Because the time-cutoff directive halted new `cargo test` invocations right after `cargo check --offline` succeeded on `src/lib.rs` + `src/helpers.rs`:
1. Execute the ready-on-disk conformance test suite in [tests/conformance_seams.rs](file:///Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam/tests/conformance_seams.rs):
   ```bash
   cd /Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam
   CARGO_TARGET_DIR="$PWD/target" cargo test --release --offline -- --nocapture
   ```
2. Optional additional tests already supported by `src/helpers.rs` (`apply_p2p_reorg`, `build_split_token_transfer_request`, `sequence_trade_batch_for_next_block`):
   - Full `MdbxChainContext` compact-restart + competing-block `apply_p2p_reorg` test at `log_slots = 16 -> 17` exercising `mdbx_context.rs:2068` directly (`conformance_seam4_lru_eviction_vs_shrink_to_log_slots` already exercises the exact `ChainState` / `SegmentedFriState::shrink_to_log_slots` assertion directly).
   - End-to-end 7-block-vs-8-block shorter-heavier-fork ASERT reorg test exercising `reconcile_receipts_at_startup` (`wallet/mod.rs:340-355`) and `native_receipts::reconcile_canonical` (`native_receipts.rs:186-194`), plus the `commit_address_discovery` generation mismatch during `install_reorg_snapshot_and_artifacts`.
