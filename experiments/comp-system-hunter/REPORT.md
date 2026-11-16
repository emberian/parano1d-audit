# Compositional & Cross-Subsystem Protocol Conformance Audit (`parano1d`)

> **Replication status (2026-11-15):** This handoff contains source hypotheses whose saved core-seams harness had no successful run at review; several predicted outcomes have since been narrowed or contradicted. The [independent verification ledger](../comp-verification/VERIFICATION.md) and [claim matrix](../comp-verification/REPLICATION-MATRIX.md) record executed controls and remaining conditions. No chainwide halt has been shown.

- **Target Repository**: `/Users/ember/dev/parano1d-break/parano1d`
- **Pinned Commit**: `52938433a30b185ab007afaa15166a632cf2e82e` (clean archive verified at `experiments/comp-system-hunter/pinned/`)
- **Lane**: `@system-hunter` (Cross-Crate Compositional Seams: Mempool ↔ Block Template ↔ Chain Reorg ↔ Wallet ↔ Financial Natives ↔ External Mining RPC)
- **Sub-Worker Lanes**:
  - `experiments/comp-system-hunter/core-seams/` (Coordinator core-seams verification harness: `Cargo.toml`, `src/main.rs`)
  - `experiments/comp-system-hunter/native-market-seam/` (Financial natives, sequencer op-80, dark consensus round ops, dynamic fee floor)
  - `experiments/comp-system-hunter/storage-runtime-seam/` (MDBX native indexes, receipt reconciliation, wallet eviction lifecycle, segment resize/eviction)

---

## Executive Summary

Auditing individual gadgets in isolation misses compositional failures where two subsystems enforce slightly different invariants across state transitions. By tracing state-machine invariants end-to-end across `noid_tx` ↔ `noid_mempool` ↔ `noid_miner` ↔ `noid_chain` ↔ `noid_node/src/wallet` ↔ `noid_rpc`, we identified **7 concrete cross-subsystem compositional defects** (plus 1 unwired dead-code fast path):

| ID | Subsystem Seam | Summary | Impact |
|---|---|---|---|
| **CS-1** | `noid_mempool` ↔ `noid_miner` ↔ `noid_chain` | Downward state-pressure or `log_slots` expansion leaves valid-at-admission `CreateLaunch` (op 48) / `CancelListing` (op 66) overpaying the new exact financial fee; `on_new_block` fails to evict, and `consensus/template.rs:787` aborts the entire template with `?` (`FinancialBlockError::Fee`). | Halts local `BlockMiner` (`None` retry loop) and `paranoid_getBlockTemplate` until manual restart or upward occupancy shift. |
| **CS-2** | `noid_miner` ↔ `noid_chain` | `TemplateChainSnapshot::hydrate_coinbase_reuse_segment` checks `empty_slot_hints_in_populated_segments(0, 1, &HashSet::new())`, ignoring selected tx output reservations and 3-slot development payouts. | `TemplateBuilder` fails with `NoCoinbaseSlot` (`None`) when all 256 segments have `>= 1` UTXO and selected txs consume the remaining empty slot(s) in hydrated segment(s), even when >99.6% of chain slots are free. |
| **CS-3** | `noid_mempool` ↔ `noid_rpc` ↔ `noid_chain` | Financial owner ops (`48, 64, 65, 66`) must satisfy both consensus `fee == minimum` and mempool `fee >= floor.max(minimum)`, while `TradeOwnerIntent` admissions skip `floor.record(fee)` (`if !is_trade`) yet increment `pool.len()`. | Whenever `floor > minimum`, `{fee == minimum} ∩ {fee >= floor} = ∅`: 100% mempool/RPC blackout of `CreateLaunch`, `CreateListing`, `Purchase`, and `CancelListing`; zero-fee trade intents hold occupancy above the 50% reset threshold without diluting the rolling median. |
| **CS-4** | `noid_node/src/main.rs` ↔ `noid_node/src/wallet` ↔ `noid_mempool` | Exact-reorg handler snapshots `mempool.reserved_slots()` *before* applying the reorg or running `mempool.on_new_block` / `readmit_after_reorg`, and `install_reorg_snapshot_and_artifacts` omits `native::on_accepted_block` and `remove_pending_inputs`. | Mempool txs evicted during reorg leave wallet UTXOs stranded as `reserved = true`; native/trade intents confirmed on the replacement fork remain stuck in `wallet.native_pending` and cannot even be cleared via `walletDismissNativePending`. |
| **CS-5** | `noid_tx` ↔ `noid_mempool` ↔ `noid_node/src/wallet` ↔ `noid_rpc` | `MempoolEvent::TxEvicted` has no wallet listener, and `walletDismissNativePending` (`forget_native_activity`) removes only `wallet.native_activity` while leaving `wallet.native_pending`, `pending_input_slots`, and `pending_output_slots` untouched. | Missing even 1 block evicts `CreateListing` (op 64, height-bound); clicking Dismiss resurrects a headless `Pending` ghost entry in `walletNativePending` and leaves the seller's NFT and NOID UTXOs locked (`reserved = true`). |
| **CS-6** | `noid_node/src/wallet` ↔ `noid_chain` | Both live `native_receipts::reconcile_canonical` and startup `reconcile_receipts_at_startup` treat `store.get_header(claimed_height) == None` as canonical/retained (`None => true` / `.is_some_and(...)`). | After a shorter, heavier fork reorg lowers `tip_height` below `claimed_height`, orphaned receipts at `claimed_height > new_tip_height` survive both live reorg cleanup and node restart reconciliation. |
| **CS-7** | `noid_rpc` ↔ `noid_chain` | `prepare_external_mining_attempt` (`paranoid_getBlockTemplate`) sums `claimable_fee_for_tx_body` per physical `TxBody` instead of per `ScheduledSpendGroup`. | Over-reports `claimable_fees_micronoid` (disagreeing with `coinbase_value_micronoid`) on every state-growing `NativeIntent` (`pages[0]` has 0 I/O, so 0 burn subtracted) and multi-page `PagedSpend` with `1` input and `>= 3` outputs. |
| **CS-8** | `noid_mempool` (dead fast-path) | `AsyncMempool::verified_authorization_proof_bytes` (`pool.rs:1149-1178`) has zero callers in `noid_node`, `noid_miner`, or `noid_chain`, yet is guarded by a source-string unit test. | Dead maintenance surface (harmless to soundness because V2/V2.1 blocks verify recursive `HistoryStep` terminals rather than raw `WalletAuthorizationBundle`s). |

---

## Detailed Findings & Exact Code Citations

### CS-1: Stale-Fee Financial Native Intents Survive Downward Fee-Pressure / `log_slots` Expansion Transitions and Abort `TemplateBuilder`

#### Exact Citations
- `parano1d/noid_chain/src/block/native_assets/financial.rs:157-162` (`FinancialHeaderWindow::check_owner`)
- `parano1d/noid_mempool/src/native.rs:160-165` (`check_native_limits`) vs `210-285` (`check_native_context` / `eviction_required`)
- `parano1d/noid_mempool/src/pool.rs:789-801` (`AsyncMempool::on_new_block`)
- `parano1d/noid_miner/src/template.rs:394, 477-494, 551-565` (`TemplateBuilder::build_from_snapshot_with_limit`)
- `parano1d/noid_chain/src/consensus/template.rs:786-788` (`native::checked_candidate(...)?`)
- `parano1d/noid_miner/src/miner.rs:455-465` (`BlockMiner::run` 1-second retry loop)
- `parano1d/noid_rpc/src/server.rs:1292-1295` (`prepare_external_mining_attempt`)

#### Compositional Mechanism
1. **Exact-fee consensus equality**: Unlike ordinary spends and non-financial native operations (which require `fee >= minimum`), financial native owner operations (`CreateLaunch` op 48, `CreateListing` op 64, `Purchase` op 65, `CancelListing` op 66) enforce **exact equality** against the parent header's `fee_breakdown(...).required_total`:
   - `parano1d/noid_chain/src/block/native_assets/financial.rs:157-162`:
     ```rust
     let fee = crate::consensus::fee_breakdown(
         facts.live_inputs as u64,
         facts.live_outputs as u64,
         self.active_slot_count,
         self.log_slots,
     )
     .required_total;
     if context.fee != fee {
         return Err(FinancialBlockError::Fee);
     }
     ```
2. **Height-independent financial operations with `net_new_slots > 0`**:
   - Note that `CreateListing` (op 64) embeds `context.height` into `ListingDefinition` (`noid_tx/src/native_assets/nft_market/owner.rs:109`), so `CreateListing` is evicted on the next block by `native::eviction_required`.
   - By contrast, **`CreateLaunch` (op 48)** (`noid_tx/src/native_assets/launch/owner.rs:70-138`) and **`CancelListing` (op 66)** (`noid_tx/src/native_assets/nft_market/owner.rs:185-235`) do **not** bind `context.height` or `NftRoundContext` (`check_financial_owner` ignores `_round` for non-`Purchase` ops at `noid_tx/src/native_assets/financial/owner.rs:24-48`).
   - Every `CreateLaunch` creates at least 3 outputs (`FungibleDefinition`, `TokenReserve`, `LaunchState`, plus optional `TokenBalance` and NOID change = 3..5 outputs) from typically 1 NOID input (`net_new_slots >= 2`). `CancelListing` spends 1 `ListingState` into 2 outputs (`NftItem` + NOID refund, `net_new_slots = 1`).
3. **Downward `required_total` transition**:
   - In `parano1d/noid_chain/src/consensus/fees.rs:76-112`, `required_total = BASE_FEE (10_000) + net_new_slots * SLOT_BURN_UNIT (2_500) * pressure_multiplier(active_slot_count, log_slots)`, where `pressure_multiplier` steps across `1x` (<50%), `2x` (>=50%), `4x` (>=75%), and `8x` (>=90%).
   - Suppose an honest user submits `CreateLaunch` (1 input, 4 outputs, `net_new_slots = 3`) at tip `H` when occupancy is at 50% (`multiplier = 2`, `fee = 25,000` µNOID) or at 75% right before scheduled `log_slots` expansion (`m -> m+1`, where occupancy immediately halves to 37.5% and `multiplier` drops `4 -> 1`, lowering `required_total` from `40,000` to `17,500` µNOID; or a UTXO consolidation reduces `active_slot_count` by 1 across 50%/75%/90%).
4. **Asymmetric mempool eviction check**:
   - When block `H+1` commits without including that `CreateLaunch`, `AsyncMempool::on_new_block` (`parano1d/noid_mempool/src/pool.rs:789-800`) runs:
     - Line `789`: `entry.spend.fee < fee_breakdown(...).required_total` — checks **strictly `<`**, so `25_000 < 17_500` is `false`.
     - Line `799`: `native::eviction_required(entry, &st.view, &self.config)` calls `check_native_context` (`noid_mempool/src/native.rs:210-275`), which checks `checked_financial_owner` (stateless, no fee check), whereas the `facts.fee != minimum` check resides **only** in admission-time `check_native_limits` (`noid_mempool/src/native.rs:162`), which is **not** called during `on_new_block`.
   - Consequently, the stale `25,000` µNOID `CreateLaunch` remains resident in `AsyncMempool`. Worse, its higher fee (`12,500` µNOID/page) sorts **ahead** of any fresh `17,500` µNOID (`8,750` µNOID/page) `CreateLaunch` in `Mempool::fee_index`.
5. **Hard abort in template assembly**:
   - On block `H+2` template assembly, `TemplateBuilder::build_from_snapshot_with_limit` (`noid_miner/src/template.rs:394`) selects the stale `CreateLaunch` into `Class::Native` (which wins whenever ordinary fees do not exceed `Class::Native` claimable revenue), re-runs stateless `checked_financial_owner` (`lines 479-488`, which passes), and calls `build_node_owned_block_template_with_native_data` (`line 551`).
   - `parano1d/noid_chain/src/consensus/template.rs:787` executes:
     ```rust
     let native =
         native::checked_candidate(parent, state, new_log_slots, &intent, &batches)?;
     ```
     Using `?` instead of `continue` aborts the entire template build with `Err(TemplateBuildError::Native(NativeBlockError::Financial(FinancialBlockError::Fee)))`.
   - `TemplateBuilder::build_from_snapshot_with_limit` returns `None`; `BlockMiner::run` (`noid_miner/src/miner.rs:455-465`) logs `"template build failed, retrying in 1s"` indefinitely without falling back to `Class::Small`/empty block or evicting the offending entry, and `paranoid_getBlockTemplate` (`noid_rpc/src/server.rs:1295`) returns `"template build failed"` to all external miners until either the 144-block epoch rolls over or state occupancy climbs back into the old multiplier tier.

#### Recommended Remediation
1. In `parano1d/noid_mempool/src/native.rs:210-275` (`check_native_context`), enforce exact fee equality (`facts.fee != minimum`) for `NativeOperation::Financial(_)` so `native::eviction_required` evicts overpaying financial intents on downward pressure/expansion transitions.
2. In `parano1d/noid_chain/src/consensus/template.rs:775-793`, skip (`continue`) individual candidate groups failing `from_pages_with_openings` or `native::checked_candidate` instead of aborting the entire block template with `?`.

---

### CS-2: `hydrate_coinbase_reuse_segment` False-Positive Short-Circuit Causes `NoCoinbaseSlot` Template Failure Despite >99% Empty Slots

#### Exact Citations
- `parano1d/noid_miner/src/template.rs:187-192, 204-254, 540-565` (`TemplateChainSnapshot::from_context`, `hydrate_transaction_segments`, `hydrate_coinbase_reuse_segment`)
- `parano1d/noid_chain/src/segmented_state.rs:1553-1607` (`empty_slot_hints_in_populated_segments`)
- `parano1d/noid_chain/src/consensus/template.rs:416-469, 885-937` (`select_system_mint_slot_avoiding_outputs`)

#### Compositional Mechanism
1. `TemplateChainSnapshot::from_context` (`parano1d/noid_miner/src/template.rs:188`) clones `ctx.state.durable_metadata_clone()`, which marks **every populated segment (`live_count > 0`) as evicted** (`segments[seg] = None`, `is_evicted(seg) == true`).
2. In `TemplateBuilder::build_from_snapshot_with_limit` (`lines 541-548`), `snapshot.hydrate_transaction_segments(&mut state, &txs)` runs **first**, loading from MDBX only the segment(s) touched by the selected mempool transactions.
3. Immediately afterward, `snapshot.hydrate_coinbase_reuse_segment(&mut state)` (`lines 230-254`) executes:
   ```rust
   fn hydrate_coinbase_reuse_segment(
       &self,
       state: &mut ChainState,
   ) -> Result<(), MdbxContextError> {
       if !state
           .state
           .empty_slot_hints_in_populated_segments(0, 1, &HashSet::new())
           .is_empty()
       {
           return Ok(());
       }
   ```
   Two compositional mismatches occur here:
   - **Mismatch A (Reserved transaction outputs ignored)**: It passes `&HashSet::new()` instead of the selected transactions' output slots (`reserved`) and `pending_outputs`. Remember that in `parano1d/noid_chain/src/consensus/template.rs:885-914`, coinbase slot allocation runs on `scratch = state.clone()` (**pre-user-transaction parent state**, because block execution order is `system mints -> users`), with all selected transaction inputs and outputs placed in `reserved`!
   - **Mismatch B (Development payout 3-slot requirement ignored)**: It checks `count = 1` even on scheduled `development_payout` heights (`development_allocation_at_height(child_height, new_log_slots).payout_due == true`), which require **3** distinct empty slots (`coinbase` + 2 development payout outputs at `consensus/template.rs:912-935`).
4. **Trigger condition**: Once all 256 segments at `m=24` contain at least 1 live UTXO (`live_count >= 1`, so zero virtual-zero segments exist and all unhydrated segments have `is_evicted == true`), if the selected mempool transaction(s) touch only a resident segment `S` whose last `k` empty slot(s) (`k >= 1`) are claimed by those transactions' own outputs (or `< 3` unreserved empty slots remain on a `development_payout` height):
   - `empty_slot_hints_in_populated_segments(0, 1, &HashSet::new())` sees those `k` slots are `SlotValue::EMPTY` in `state` (since `txs` have not been applied yet and `reserved` is empty!) and returns non-empty, causing `hydrate_coinbase_reuse_segment` to **short-circuit `Ok(())`** without hydrating any of the other 255 segments (even if 99.6% of the 16.7M chain slots are empty!).
   - `select_system_mint_slot` (`consensus/template.rs:416-469`) then finds 0 unreserved empty slots in segment `S`, skips segments `!= S` because they are `is_evicted == true` and `live_count > 0`, and returns `None` -> `Err(TemplateBuildError::NoCoinbaseSlot)` -> `TemplateBuilder` returns `None`!

#### Recommended Remediation
Pass the union of selected `txs` input/output `slot_index`es and `pending_outputs` into `hydrate_coinbase_reuse_segment`, and request `required_slots = 1 + 2 * usize::from(development_payout_due_at_height(child_height))` instead of `1` with `&HashSet::new()`.

---

### CS-3: Unsatisfiable Fee Inequality Blacks Out Financial Native Owner Ops (`48, 64, 65, 66`) Under Any Elevated Dynamic Fee Floor, Compounded by Zero-Fee `TradeOwnerIntent` Median Exclusion

#### Exact Citations
- `parano1d/noid_mempool/src/native.rs:160-175` (`check_native_limits`)
- `parano1d/noid_chain/src/block/native_assets/financial.rs:157-162` (`FinancialHeaderWindow::check_owner`)
- `parano1d/noid_rpc/src/server/native/planner.rs:91-103` (`Planner::fee`)
- `parano1d/noid_mempool/src/pool.rs:548-555, 1344-1365` (`submit_trade` / `refresh_fee_floor`)
- `parano1d/noid_mempool/src/fee_floor.rs:55-120` (`DynamicFeeFloor`)

#### Compositional Mechanism
1. Prior single-module analysis (`experiments/fee-floor/REPORT.md`) noted the dynamic fee floor latch on `AsyncMempool`, assuming *"A new transaction paying the raised floor is still admitted."*
2. That assumption **fails completely** for all four owner-submitted financial native operations (`CreateLaunch` op 48, `CreateListing` op 64, `Purchase` op 65, `CancelListing` op 66):
   - Consensus (`financial.rs:160`) and mempool (`noid_mempool/src/native.rs:162`) enforce `facts.fee == minimum` (`if facts.fee != minimum { return Err(SubmitError::MalformedIntent("native financial fee differs from the deterministic minimum".into())); }`).
   - Five lines later (`noid_mempool/src/native.rs:167-174`), `check_native_limits` enforces:
     ```rust
     let floor = state.floor.current().max(minimum);
     if facts.fee < floor {
         return Err(SubmitError::FeeBelowMinimum {
             got: facts.fee,
             min: floor,
         });
     }
     ```
   - Similarly, `Planner::fee` (`parano1d/noid_rpc/src/server/native/planner.rs:99-101`) rejects planning with `"the current relay floor exceeds the native financial fee"`.
3. The moment `state.floor.current() > minimum` (even by 1 µNOID, e.g. `20,000 > 12,500`), the conjunction `facts.fee == minimum && facts.fee >= state.floor.current()` is **mathematically unsatisfiable (`∅`)**. An NFT seller cannot even submit `CancelListing` (op 66) at *any* fee to delist their asset before someone executes `Purchase` once the floor drops.
4. **Interaction with `TradeOwnerIntent` (op-80 user trade intents)**:
   - In `parano1d/noid_mempool/src/pool.rs:548-555`:
     ```rust
     if !is_trade {
         st.floor.record(fee);
     }
     ```
     `TradeOwnerIntent`s carry `spend.fee == 0` (their fee is bounded via `policy.fee_limit >= floor` in `noid_mempool/src/trade.rs:155`), so `pool.rs` skips `st.floor.record(fee)` for trades while **still adding them to `st.pool` (`st.pool.len()`)**!
   - Because `refresh_fee_floor` (`pool.rs:1344-1365`) only resets the latched floor when `st.pool.len() < config.max_txs / 2` (50% count low-water mark) and `st.floor` computes a rolling 50-sample median exclusively over `!is_trade` admissions, once 26+ elevated-fee ordinary transactions establish a high median, ordinary/trade occupancy >= 50% holds `floor` elevated without `TradeOwnerIntent` admissions ever diluting the 50-sample median window.

#### Recommended Remediation
1. Exempt consensus-fixed-fee financial operations (`NativeAuthorizationContext::Financial(_)`) from the `state.floor.current()` surcharge in `noid_mempool/src/native.rs:167` and `noid_rpc/src/server/native/planner.rs:99` (or relax consensus `financial.rs:160` to `context.fee < fee` if priority tipping is desired, updating circuit/consensus rules in lockstep).
2. Decay or recompute `DynamicFeeFloor` from currently resident pool entries rather than a sticky historical 50-sample median gated solely on raw entry count.

---

### CS-4: Inverted Exact-Reorg Wallet-vs-Mempool Update Ordering & Missing `native::on_accepted_block` Strands Wallet Reservations

#### Exact Citations
- `parano1d/noid_node/src/main.rs:4358, 4575-4636, 4676-4687` (`apply_exact_suffix_plan`, `SyncPlanKind::Reorg`)
- Contrast with `parano1d/noid_node/src/main.rs:15749-15759` (`install_snapshot` — correct order!)
- `parano1d/noid_node/src/wallet/mod.rs:425-546` (`install_reorg_snapshot_and_artifacts`)
- `parano1d/noid_node/src/wallet/state.rs:379-403` (`commit_verified_activation`)
- `parano1d/noid_rpc/src/server/native.rs:396-412` (`dismiss_pending`)

#### Compositional Mechanism
1. Compare snapshot installation (`main.rs:15749-15759`) with exact reorg application (`main.rs:4358-4687`):
   - In `install_snapshot`, the daemon calls `mempool.replace_chain_view(new_view).await` **first**, and **then** queries `mempool.reserved_slots().await` to pass into `wallet.commit_activation_snapshot(...)`.
   - In `apply_exact_suffix_plan` (`SyncPlanKind::Reorg`), the order is **inverted**:
     1. `main.rs:4358`: `let (reserved_input_slots, reserved_output_slots) = mempool.reserved_slots().await;` is captured **before** the reorg is applied to MDBX and **before** `mempool` is updated!
     2. `main.rs:4576`: `ctx.apply_verified_reorg_suffix_with_applier_indexed(...)` applies the replacement fork.
     3. `main.rs:4617`: `wallet::install_reorg_snapshot_and_artifacts(..., &reserved_input_slots, &reserved_output_slots, ...)` installs those **stale pre-reorg** reservation sets into `wallet.pending_input_slots`, `wallet.pending_output_slots`, and `wallet.native_pending` (`wallet/state.rs:379-403`).
     4. `main.rs:4678-4686`: **Only after** the wallet snapshot is already committed does `mempool.on_new_block(...)` and `mempool.readmit_after_reorg(...)` run!
2. Simultaneously, inside `install_reorg_snapshot_and_artifacts` (`parano1d/noid_node/src/wallet/mod.rs:512-538`), the replay loop over `replacement_blocks` calls `wallet.confirm_pending_tx` and `wallet.remove_pending_outputs(&output_slots)`, **but omits both**:
   - `wallet.remove_pending_inputs(&spent_slots)` for ordinary transactions without a `pending_tx` entry or native spends, **and**
   - `native::on_accepted_block(wallet, block)` (which normal block acceptance calls at `wallet/mod.rs:164`)!
3. **Two concrete failure modes**:
   - **Mode A (Reorg-evicted mempool tx locks live UTXOs)**: Any pending ordinary or native transaction in `mempool` pre-reorg that is evicted at `main.rs:4678` (e.g., `OutputSlotOccupied` because the replacement fork placed an output or coinbase on its chosen output slot, `EpochAnchorChanged`, `ConsensusFeeIncreased`, or `NativeContextChanged`) or removed by `readmit_after_reorg` at `main.rs:4685` leaves `mempool` empty (`reserved_slots == ∅`), while `wallet.pending_input_slots` permanently retains its input UTXO(s) as `reserved = true` (unspendable in `plan_send` / `planner`).
   - **Mode B (Replacement-fork-confirmed native/trade intent becomes an undismissable zombie)**: If a `TradeOwnerIntent` or `NativeIntent` was in `mempool` pre-reorg and was **confirmed** inside `replacement_blocks`:
     - `commit_verified_activation` (`wallet/state.rs:384-391`) retains it in `wallet.native_pending` because `reserved_input_slots` was captured pre-reorg.
     - `install_reorg_snapshot_and_artifacts` omits `native::on_accepted_block`, so it is **never retired** from `wallet.native_pending` or `wallet.pending_input_slots`.
     - `retain_object_receipts_at_tip` (`main.rs:4647`) writes its canonical receipt into `wallet.receipts`.
     - If the user subsequently calls `walletDismissNativePending(txid)` (`noid_rpc/src/server/native.rs:396-412`), `canonical_submission_height` returns `Some(height)`, causing `dismiss_pending` to **reject** dismissal with `"only unconfirmed submissions outside the local queue can be dismissed"`!

#### Recommended Remediation
1. Update `mempool` (`on_new_block` + `readmit_after_reorg`) **before** capturing `mempool.reserved_slots().await` for `install_reorg_snapshot_and_artifacts` (matching `install_snapshot`), or re-sync wallet reservations after `mempool` updates while holding `wallet_operation_gate`.
2. Call `native::on_accepted_block(wallet, block)` for every `block` in `replacement_blocks` inside `install_reorg_snapshot_and_artifacts` (`wallet/mod.rs:512-538`).

---

### CS-5: Single-Missed-Block `CreateListing` Eviction + Incomplete `forget_native_activity` Locks NFT & NOID UTXOs and Resurrects Ghost Pending Entry

#### Exact Citations
- `parano1d/noid_tx/src/native_assets/nft_market/owner.rs:105-115` (`CreateListing` binds `context.height`)
- `parano1d/noid_node/src/wallet/native.rs:190-217` (`native::reserve`)
- `parano1d/noid_node/src/wallet/mod.rs:656-666` (`WalletHandle::forget_native_activity`)
- `parano1d/noid_rpc/src/server/native.rs:352-367, 396-412` (`pending`, `dismiss_pending`)

#### Compositional Mechanism
1. **No `MempoolEvent::TxEvicted` listener for `WalletState`**: Across the entire daemon (`noid_node/src/main.rs`), `mempool.subscribe_events()` is never wired to release `wallet.pending_input_slots`, `wallet.pending_output_slots`, or `wallet.native_pending` when a transaction is evicted from `AsyncMempool` (`TradePolicyExpired`, `EpochAnchorChanged`, `ConsensusFeeIncreased`, `NativeContextChanged`, `OutputSlotOccupied`).
2. **Immediate 1-block trigger on `CreateListing` (op 64)**: Because `ListingDefinition::from_item` (`noid_rpc/src/server/native/planner.rs:745-752`, `nft_market/owner.rs:109`) commits to `created_height = parent.height + 1`, if a seller submits `ListNft` (`CreateListing`) at tip `H` and **even a single block `H+1` is mined without including it** (e.g., an empty block or race with PoW solve), `AsyncMempool::on_new_block` immediately evicts `CreateListing` with `EvictReason::NativeContextChanged`. (The same applies to any `TradeOwnerIntent` expiring at `valid_until`.)
3. **Broken `walletDismissNativePending` (`forget_native_activity`)**:
   - After eviction, `walletNativePending` (`noid_rpc/src/server/native.rs:325-350`) shows the submission in `native_activity` with `status: Dropped`, and the GUI (`noid_gui/src/view/proofs.rs:135`) offers a **Dismiss** button (`walletDismissNativePending`).
   - `dismiss_pending` (`noid_rpc/src/server/native.rs:411`) calls `handler.wallet.forget_native_activity(id)`, implemented at `parano1d/noid_node/src/wallet/mod.rs:656-666`:
     ```rust
     fn forget_native_activity(&self, txid: [u8; 32]) -> Result<(), String> {
         let mut guard = self.inner.lock().map_err(|_| "wallet lock poisoned")?;
         let wallet = guard.as_mut().ok_or("wallet not initialized")?;
         if wallet.receipts_dirty || wallet.receipts.has_changes() {
             wallet.save_receipts()?;
         }
         wallet.native_activity.remove(txid);
         wallet
             .native_activity
             .save(&wallet.keystore_path.with_extension("native-pending"))
     }
     ```
   - `forget_native_activity` deletes **only** `wallet.native_activity`! It **never removes `txid` from `wallet.native_pending`**, and **never calls `wallet.remove_pending_inputs(&pending.input_slots)` or `wallet.remove_pending_outputs(&pending.output_slots)`**!
4. **Ghost resurrection & locked assets**:
   - On the very next call to `walletNativePending` (`noid_rpc/src/server/native.rs:352-367`), the fallback loop over `handler.wallet.native_pending_submissions()` sees `!seen.contains(&pending.txid)` and **re-emits the dismissed entry** with `status: NativeWalletPendingStatus::Pending, preview: None, submitted_unix: None`!
   - Both the seller's NFT (`item` slot) and NOID funding UTXO remain marked `reserved = true` in `wallet.pending_input_slots` (blocking re-listing, NFT transfer, or spending that NOID UTXO) and continue consuming a slot toward the 256-entry `native_pending` cap (`wallet/native.rs:198`) until a full wallet reload/restart.

#### Recommended Remediation
1. In `WalletHandle::forget_native_activity` (`parano1d/noid_node/src/wallet/mod.rs:656-666`), also `if let Some(pending) = wallet.native_pending.remove(&txid) { wallet.remove_pending_inputs(&pending.input_slots); wallet.remove_pending_outputs(&pending.output_slots); }`.
2. Subscribe to `MempoolEvent::TxEvicted` in `noid_node` (or reconcile `wallet.pending_*` against `mempool.reserved_slots()` on `on_new_block`) so evicted ordinary and native transactions automatically release their slot reservations.

---

### CS-6: Shorter Heavier Fork Reorg Retains Orphaned Receipts (`claimed_height > new_tip_height`) Across Live & Startup Reconciliation

#### Exact Citations
- `parano1d/noid_node/src/wallet/mod.rs:341-351` (`reconcile_receipts_at_startup`)
- `parano1d/noid_node/src/wallet/native_receipts.rs:182-194` (`reconcile_canonical`, called live on reorgs via `retain_object_receipts_at_tip` at `main.rs:4647`)
- `parano1d/noid_node/src/wallet/mod.rs:460-479` (`install_reorg_snapshot_and_artifacts` — checks `receipt.is_trade()` for `native_reclaimed`, omitting `receipt.is_native()` if not in `reclaimed_tx_hashes`!)
- `parano1d/noid_rpc/src/server.rs:2470-2475` (`paranoid_verifyReceipt`: `Ok(None) => None` for `canonical`)

#### Compositional Mechanism
1. Under ASERT PoW fork-choice, a **shorter fork with higher cumulative chain work** can legitimately replace a taller fork within the 18-block finality window (`new_tip_height < old_tip_height`). During atomic reorg commit, MDBX deletes canonical headers above `new_tip_height`, so `store.get_header(h)` returns `Ok(None)` for `h > new_tip_height`.
2. Both receipt reconciliation functions treat `Ok(None)` as **keep/valid** instead of **orphaned**:
   - `parano1d/noid_node/src/wallet/mod.rs:343-351` (`reconcile_receipts_at_startup`):
     ```rust
     match chain.get_header_from_store(receipt.claimed_height) {
         Ok(Some(header)) => verify_against_header(&receipt, &header),
         Ok(None) => true,
         Err(e) => return Err(format!("header lookup failed during receipt sync: {e}")),
     }
     ```
   - `parano1d/noid_node/src/wallet/native_receipts.rs:186-191` (`reconcile_canonical`):
     ```rust
     if (receipt.is_trade() || receipt.is_native())
         && store
             .get_header(receipt.claimed_height)
             .map_err(|e| e.to_string())?
             .is_some_and(|header| !verify_against_header(&receipt, &header))
     {
         removed.push(*id);
     }
     ```
3. Note why `Ok(None) => true` was likely written assuming pruned *ancient* headers (`claimed_height < earliest_header`): in `parano1d`, headers are never pruned below `tip_height` (`headers` table retains `0..=tip_height`), so `get_header(claimed_height) == Ok(None)` **only** occurs when `claimed_height > tip_height` (an orphaned taller fork or corrupt future receipt)! Consequently, any receipt at `claimed_height > new_tip_height` that survives into `wallet.receipts` is **never purged** by `reconcile_canonical` or `reconcile_receipts_at_startup`.

#### Recommended Remediation
Compare `receipt.claimed_height > tip_height` explicitly (or treat `Ok(None)` when `claimed_height > tip_height` as non-canonical/remove) in both `reconcile_receipts_at_startup` (`wallet/mod.rs:346`) and `native_receipts::reconcile_canonical` (`wallet/native_receipts.rs:188`).

---

### CS-7: `paranoid_getBlockTemplate` Computes Burn Per Physical `TxBody` Instead of Per `ScheduledSpendGroup`

#### Exact Citations
- `parano1d/noid_rpc/src/server.rs:1318-1329` (`prepare_external_mining_attempt`)
- `parano1d/noid_chain/src/consensus/fees.rs:115-128` (`claimable_fee_for_tx_body`)
- Contrast with `parano1d/noid_chain/src/consensus/template.rs:940-952` (true consensus per-group burn)

#### Compositional Mechanism
1. Consensus (`consensus/template.rs:940-952` and `consensus/validation.rs`) computes burned state-growth fees per **logical spend group** (`fee_breakdown(group.live_inputs, group.live_outputs, active_slot_count, log_slots).burned`), subtracting that burn from `group.fee` to determine `coinbase_value_micronoid`.
2. By contrast, `prepare_external_mining_attempt` (`parano1d/noid_rpc/src/server.rs:1318-1329`) computes `BlockTemplateResponse.claimable_fees_micronoid` by summing `claimable_fee_for_tx_body` over each physical `TxBody` in `tmpl.inner.txs`:
   ```rust
   let claimable_fees_micronoid = tmpl
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
       .fold(0u64, |acc, fee| acc.saturating_add(fee));
   ```
3. This per-body calculation diverges from true consensus claimable fees in two common cases:
   - **Every state-growing `NativeIntent` / `ExecuteTrades` group**: In canonical native wire format, `pages[0]` is the metadata header page (`NATIVE_HEADER_BIT`, `0` live inputs, `0` live outputs) and carries `body.fee = group_fee`, while `pages[1..]` carry `body.fee = 0` and all UTXO inputs/outputs. On `pages[0]`, `claimable_fee_for_tx_body` sees `0` inputs and `0` outputs -> `burned = 0`, returning the **entire `group_fee` as claimable**; on `pages[1..]`, `0.saturating_sub(burned) == 0`. Thus 100% of native state-growth burn is omitted from `claimable_fees_micronoid` (e.g., for `CreateLaunch` with 1 input and 4 outputs at 1x pressure, `claimable_fees_micronoid` reports `17,500` µNOID instead of `10,000` µNOID, contradicting `coinbase_value_micronoid - miner_subsidy`).
   - **Multi-page `PagedSpend`s with `1` input and `>= 3` outputs**: With 1 input and 4 outputs across 2 pages (`page 0`: 1 input, 2 outputs, `fee = 17,500`; `page 1`: 0 inputs, 2 outputs, `fee = 0`), per-body accounting burns only `1 * 2,500 = 2,500` on page 0 and `0.saturating_sub(5,000) = 0` on page 1, reporting `15,000` µNOID instead of `10,000` µNOID (`+5,000` µNOID error). (Conversely, if a multi-page spend has `1` input and `2` outputs? That's 1 page. What about `9` inputs and `4` outputs across 2 pages? `page 0` has `8` inputs, `2` outputs (`burned = 0`), `page 1` has `1` input, `2` outputs (`fee = 0`, `0 - 2,500 = 0`), matching `0` group burn; what about `2` inputs and `5` outputs across 3 pages? `page 0` has `2` inputs, `2` outputs -> `burned = 0`, missing **all** `3 * 2,500 = 7,500` µNOID of burn!).

#### Recommended Remediation
Compute `claimable_fees_micronoid` on `BlockTemplate` directly from `ordered_winners` in `consensus/template.rs:940-952` (or `coinbase_value_micronoid.saturating_sub(allocation.miner_subsidy)`), and delete or restrict single-page `claimable_fee_for_tx_body`.

---

### CS-8: Unwired Fast-Path `AsyncMempool::verified_authorization_proof_bytes`

- **Location**: `parano1d/noid_mempool/src/pool.rs:1144-1178` and test at `pool.rs:2511-2532`.
- **Observation**: `verified_authorization_proof_bytes` serializes cached `WalletAuthorizationBundle` proof bytes with a doc comment stating *"Block acceptance uses them as a re-verification fast path"*, and `template_and_preverified_paths_never_clone_raw_intent_payloads` (`pool.rs:2523-2532`) asserts properties of its source text. Workspace-wide search confirms **zero callers** exist outside that string-inspection test because V2/V2.1 block acceptance verifies `HistoryStep` recursive terminals (`begin_preverified_recursive_suffix` / `authorize_preverified_reorg_suffix`) rather than per-tx owner bundles.

---

## Verified Tight / Negative Controls

To avoid false positives, we explicitly verified and ruled out the following candidate cross-crate hypotheses:

1. **`noid_mempool/src/native.rs:186` `intent.pages.iter().skip(1)` output-slot scan**:
   - *Hypothesis*: Skipping `pages[0]` might allow a `NativeIntent` to smuggle an unindexed output on `pages[0]`.
   - *Result (Tight)*: `validate_native_pages` (`noid_tx/src/native_assets/wire.rs:820-865`) enforces that `pages[0]` carries `NATIVE_HEADER_BIT` with zero live inputs/outputs (`validity_bitmap` live I/O bits all zero); all UTXO inputs and outputs reside strictly in `pages[1..]`.
2. **Untagged ordinary UTXO spoofing a `NativeOpening::root()` address**:
   - *Hypothesis*: Sending an ordinary NOID output with `owner = opening.root()` and `amount = opening.amount_micronoid()` might allow spending it as a forged `FungibleDefinition`, `NftCollection`, or `TokenBalance`.
   - *Result (Tight)*: Prevented at three independent layers via `NATIVE_CREATION_TAG` (`1 << 63` namespace bit on `creation_id`): `NativeOpening::check_input_binding` (`noid_tx/src/native_assets.rs:1017`), `validate_scheduled_block_page_stream` (`noid_chain/src/block/native_assets.rs:247-255`), and `Planner::opening` (`noid_rpc/src/server/native/planner.rs:176`) all require `native_sequence(creation_id).is_some()`, which ordinary `PagedSpend` outputs never receive.
3. **`v2_selection_budget` (`noid_miner/src/template.rs:678-688`) keeping `owner_authorizations = config.pages()` (63) on `development_payout` heights (`pages = 62`)**:
   - *Hypothesis*: Allowing 63 `TradeOwnerIntent` authorizations inside one op-80 `ExecuteTrades` group on a `development_payout` block (where `pages` is reduced to `62`) might panic `prepare_candidate_authorizations`.
   - *Result (Tight)*: `TradeKernelConfig` caps `policies <= 16`, and `prepare_candidate_authorizations` (`noid_recursive/src/acceptance/history_step.rs:285-287`) explicitly bounds `inputs.len() <= config.pages()` (`63`) when `native_sequential_config().is_some()`.
4. **`CreateListing` (op 64) across downward pressure transitions (CS-1 scope check)**:
   - *Result*: `ListingDefinition::from_item` (`noid_tx/src/native_assets/nft_market/owner.rs:109`) binds `created_height = context.height`, so `CreateListing` is evicted on the next block by `native::eviction_required` (`NativeContextChanged`) — which instead triggers **CS-5**! `CreateLaunch` (op 48) and `CancelListing` (op 66) do *not* bind `context.height`, so they survive across blocks and trigger **CS-1**.

---

## Harness Artifacts, Build Status & Sub-Worker Integration Notes

1. **`/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/core-seams/`**:
   - Contains `Cargo.toml` and `src/main.rs` (`~1,050` lines) implementing 5 end-to-end conformance tests covering **CS-1**, **CS-2** (using genuine genesis-to-V2 multi-page blocks), **CS-4**, **CS-5**, and **CS-7** against unmodified `pinned/` crates (including mounting `pinned/noid_node/src/wallet/mod.rs` directly).
   - **Build status at time cutoff**: All 420 external/workspace crate dependencies compiled cleanly into `core-seams/target/`. The first compile pass of `core-seams/src/main.rs` surfaced mechanical method-name adjustments needed before execution (specifically: `noid_poseidon2b::primitives::derive_address(&secret)` instead of `secret.derive_address()`, `TxInput::dummy()` / `TxOutput::dummy()` and `is_coinbase: false` on `TxBody`, `AsyncMempool::new(view, config)` argument order, `state.slot(idx)` instead of `get_slot`, `WalletOps::list_utxos()` and `remember_native_submission`, `noid_gkr::wallet_authorization::prove_paged_spend_authorization_for_version`, and `RuntimeParts::decode_compact` for `HistoryProtocolRuntime`). Per time-cutoff instruction `#15`, no new `cargo build` was started; applying those mechanical signature mappings will execute all 5 tests immediately against the pre-compiled `core-seams/target/` cache.
2. **Sub-Worker Directories (`native-market-seam/` & `storage-runtime-seam/`)**:
   - `/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/native-market-seam/`: Investigates dark consensus operations (`Swap` op 33, `LaunchRequest` op 49, and sequencer round ops `38-41, 43, 50-53` enabled in `profile.rs` while `noid_miner` only builds op-80 `ExecuteTrades` and never synthesizes `Settle*`/`Reset*`), **CS-3** dynamic fee-floor vs fixed-fee financial native blackout + `TradeOwnerIntent` median latch, multi-market `ExecuteTrades` graduation/input bounds, and `noid_pruv` vs `integer_program` conformance. See `native-market-seam/NOTES.md` and `native-market-seam/src/`.
   - `/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/storage-runtime-seam/`: Investigates **CS-5** wallet eviction reservation leaks across `TradePolicyExpired` / `EpochAnchorChanged`, MDBX native storage `live_slots` reference counting on identical-`opening.root()` UTXOs & graduation reorgs & compact-restart table authentication, **CS-6** shorter-heavier-fork receipt retention (`reconcile_receipts_at_startup` & `native_receipts::reconcile_canonical`), and `mdbx_context.rs` rollback / `shrink` across `log_slots` expansion with evicted segments. See `storage-runtime-seam/NOTES.md` and `storage-runtime-seam/src/`.
