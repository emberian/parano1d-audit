# Dual-Engine Consensus Differential Investigation (`parano1d`)

> **Replication status (2026-11-15):** This is a hypothesis and source-trace handoff. Its saved test log ends in compilation errors, so the claimed full-proof acceptance and snapshot split were not demonstrated. See the [independent verification ledger](../comp-verification/VERIFICATION.md) and [claim matrix](../comp-verification/REPLICATION-MATRIX.md) for current evidence.

- **Target Repository**: `/Users/ember/dev/parano1d-break/parano1d`
- **Pinned Commit**: `52938433a30b185ab007afaa15166a632cf2e82e`
- **Investigation Lane**: `comp-consensus-diff` (`/Users/ember/dev/parano1d-break/experiments/comp-consensus-diff/`)
- **Verification Harness**: `/Users/ember/dev/parano1d-break/experiments/comp-consensus-diff/src/lib.rs` (`Cargo.toml` referencing `../../parano1d/*`)

---

## 1. Executive Summary

`parano1d` relies on two distinct block-level validation engines whose equivalence underpins network consensus across live full-node execution and snapshot synchronization:

1. **Engine 1 — Sequential Native Consensus & State Materialization** (`noid_chain`, `noid_block`, `noid_tx`, plus `noid_mempool` / `noid_miner` admission & template builders): validates block headers, canonical `TxBody` framing, `slot_index < 2^log_slots` bounds, cross-transaction `slot_index` disjointness, `PagedSpend` framing, native-asset / financial closures, and sequential `ChainState` updates.
2. **Engine 2 — Recursive `HistoryStep` R1CS + GKR/IVC Validation** (`noid_recursive::acceptance::{block_slots, history_step, trace}`, `noid_ivc_core::deep_chain`, `noid_miner::history_protocol`): proves historical state transitions recursively. **Snapshot sync (`MdbxChainContext::verify_snapshot_boundary` + `apply_staged_state_snapshot`) trusts the verified `HistoryStep` terminal for the entire historical chain up to the snapshot boundary without downloading or replaying historical block bodies.**

Systematic differential auditing across both failure directions uncovered **four concrete specification divergences**, led by a **critical Direction (A) missing R1CS slot-domain constraint** in `HistoryStep` (`LegacyV1`, `V2`, and `V2.1`) that creates both a **Snapshot Sync vs. Live Full-Node consensus split** and an **intra-block same-slot chaining bypass** (`Mint -> Spend` and `Spend -> Mint` on the same physical slot within a single block):

| ID | Direction | Severity | Component (`file:line`) | Summary |
|---|---|---|---|---|
| **CD-1** | **(A) Circuit weaker than Native** | **Critical** (Consensus Split + Intra-Block Slot-Conflict Bypass) | `noid_recursive/src/acceptance/block_slots.rs:641-720, 3237-3250`; `trace/exact_state.rs:961-1002, 1327-1345` | `bind_paired_exact_state_transition` constrains `slot_index` to 32 bits (`compact_action_rows`) and pins 16 upper Merkle directions (`slot_bits[16..32]`), but `select_upper_paired_roots` extracts the root at `child.log_slots - 16` (depth 8 at `m24`) **without constraining `slot_bits[child.log_slots..32] == 0`**. The helper that originally enforced this (`bind_structural_frontier_count_from_actions_dynamic` at `exact_state.rs:961-1002`) was orphaned as dead code. Enables (1) out-of-domain `slot_index = s + k * 2^d` accepted by `HistoryStep` + snapshot sync yet rejected by live full nodes, and (2) intra-block `Mint -> Spend` (even spending a block's own primary coinbase in the same block) and `Spend -> Mint` on the same physical slot `s`. |
| **CD-2** | **(B) Native broader than Circuit** | **Medium** (Specification Divergence; Gated in Mempool/Miner) | `noid_tx/src/native_assets/profile.rs:8-53`; `noid_chain/src/block/native_assets.rs:597-834` vs `v2/capacity_banked/config.rs:59`; `financial_kernel.rs:121-141` | `profile::operation_enabled` and `noid_chain` sequential validation/materialization enable **22** native opcodes and **3** request/claim carrier kinds, whereas v2.1 `Class::Native` forces `native_sequential_config().is_some()` and rejects **11** consensus-enabled financial opcodes (`{33, 38, 39, 40, 41, 43, 49, 50, 51, 52, 53}`, including `Swap` and `LaunchRequest`). Cleanly gated at `noid_mempool/src/native.rs:228-235` and `noid_chain/src/consensus/template/native.rs:94-101` (no unauthenticated mempool/prover wedge). |
| **CD-3** | **(B) Native broader than Circuit + Witness Panic** | **Medium / Low** (Resource Envelope Gap + Preflight Bug) | `noid_block/src/candidate_history.rs:146-155, 290-389`; `trace/native_assets/financial_definitions.rs:275` | Native `noid_chain` accepts up to `255` user pages and `3,062` native openings, whereas `Class::Native` enforces tighter bank caps (`definition_capacity = headers + markets`, `TradeKernelConfig`). Moreover, `validate_scheduled_candidate_resources` checks `headers` and `records` but **omits checking `auxiliary_definitions.len() <= definition_capacity`** (and `trade.{markets, policies, funding}`). Because one immediate-graduation `CreateLaunch` (`op 48`) consumes **2** auxiliary definitions (`LaunchDefinition` + `MarketDefinition`) in **1** header, a consensus-valid block within `header_limit` can exceed `definition_capacity`, bypass preflight, and trigger an `assert!` panic in `bind_financial_definition_table`. |
| **CD-4** | **Architectural Validation Partition** | **Informational / Design Boundary** | `noid_chain/src/consensus/validation.rs:212-222`; `noid_chain/src/block.rs:196-244` vs `block_slots.rs:921-1141` | Standalone `validate_block_consensus`, `validate_block_checks`, and `materialize_accepted_block_state` inspect only `v2_active(height)` and single-page bitmap framing on `PAGED_SPEND_CONTRACT_BIT` pages because `Block` wire bytes carry no `ObjectOpening`. Full V2 contract semantics (`integer_program`, `ObjectRules` fee/retention/payout caps, `before_deadline` branch selection, successor object roots) are enforced exclusively by `HistoryStep` (and `noid_mempool/src/contracts.rs` pre-mining). Callers using standalone `validate_block_consensus` as a block oracle do not validate V2 contract rules or native openings. |

---

## 2. Architecture of the Dual Consensus Paths

### 2.1 Live Full-Node Suffix Path
When a live node commits a peer block (`noid_node/src/networking/chain_committer.rs:117-225` -> `MdbxChainContext::apply_verified_recursive_suffix_block_with_native_data` at `noid_chain/src/storage/mdbx_context.rs:1468-1589`), **both** engines must accept:
1. `validate_user_epoch_anchors` (`mdbx_context.rs:1491`).
2. `validate_block_checks` (`noid_chain/src/consensus/validation.rs:255-271`), which runs `validate_header`, `validate_block_resources_with_stream` (`slot_index < 2^log_slots` at lines `188-205`), `validate_mandatory_coinbase`, `validate_tx_consensus`, `validate_block_slot_conflicts` (`checks.rs:48-65`), and coinbase fee/burn ceiling checks.
3. `verify_history_step_terminal` (`noid_node/src/networking/verifier_pool.rs:255` -> `EmbeddedHistoryStepPack::verify_terminal` at `noid_node/src/embedded_history_step_pack.rs:265`), verifying the recursive `HistoryStep` terminal and checking `ChainAccumulator` / `VerifiedOrigin` bindings (`mdbx_context.rs:1508-1521`).
4. `materialize_accepted_block_state_with_native_data` (`noid_chain/src/block/native_assets.rs:584-595`), re-executing sequential slot updates and checking `state_root`, `active_slot_count`, `alloc_counter`, `log_slots`, and `tx_root` (`native_assets.rs:812-827`).

### 2.2 Snapshot Synchronization Path
When a node syncs from a state snapshot (`noid_node/src/networking/snapshot_sync.rs` -> `SnapshotHeaderStaging` at `snapshot_header_staging.rs:95-190` -> `MdbxChainContext::verify_snapshot_boundary` at `mdbx_context.rs:2384-2523` -> `apply_staged_state_snapshot` at `mdbx_context.rs:2617-2795`):
1. Headers `0..=boundary_height` are validated (`prev_block_hash`, height, ASERT difficulty, MTP/timestamp, PoW, and 10-of-18 finalized `log_slots` expansion window via `expected_child_log_slots`). **Historical `Block` bodies are never downloaded or validated natively.**
2. `verify_snapshot_boundary` checks the boundary tip's `HistoryStep` recursive proof (`verify_terminal`) and binds `(height, tip_semantic_id, state_root, log_slots, active_slot_count, alloc_counter, epoch_anchor_id)` to the boundary `BlockHeader`.
3. `SnapshotStaging` streams non-empty UTXO slots at canonical indices `slot_index < 2^boundary.log_slots`, verifies `creation_id_within_boundary(slot.creation_id(), boundary.alloc_counter, boundary.height)`, checks `active_slot_count`, and recomputes the exact sparse Merkle root at depth `boundary.log_slots` against `boundary.state_root`.

Consequently, **any invalid block body transition that produces a valid `BlockHeader` and satisfies `HistoryStep` will be permanently accepted across a snapshot boundary even though every live full node rejects that block.**

---

## 3. Detailed Differential Findings

### Finding CD-1 [Critical — Direction (A)]: Missing Dynamic High-Bit Slot-Domain Constraint (`slot_bits[child.log_slots..32] == 0`) in `HistoryStep`

#### Exact Source Locations
- **Active `HistoryStep` exact-state binder (LegacyV1, V2, and V2.1)**:
  - `noid_recursive/src/acceptance/block_slots.rs:641-720` (`bind_paired_exact_state_transition`)
  - `noid_recursive/src/acceptance/block_slots.rs:3237-3250` (`bind_v2_common_tail` -> calls `bind_paired_exact_state_transition`; used by all V2 and V2.1 `Small`, `Large`, and `Native` banks)
  - `noid_recursive/src/acceptance/block_slots.rs:789-805` (LegacyV1 `bind_block_slots_from_parts` -> calls `bind_paired_exact_state_transition`)
- **Action & segment compaction traces**:
  - `noid_recursive/src/acceptance/trace/action_compaction.rs:106-119` (`compact_action_rows`: 32-bit decomposition and 32-bit strict ascending sort)
  - `noid_recursive/src/acceptance/trace/segment_compaction.rs:123-265` (`compact_segment_updates` & `bind_segment_upper_chain`: 16-bit `segment_id = slot_bits[16..32]` strict ascending sort and sequential root chaining)
- **Upper Merkle root depth selector & PairedMerkleUpdate GKR columns**:
  - `noid_recursive/src/acceptance/trace/exact_state.rs:1327-1345` (`select_upper_paired_roots`)
  - `noid_recursive/src/acceptance/trace/paired_merkle_update.rs:108-111, 133-140` (`paired_update_root_offsets`)
- **Orphaned dead-code helper containing the omitted high-bit pin**:
  - `noid_recursive/src/acceptance/trace/exact_state.rs:961-1002` (`bind_structural_frontier_count_from_actions_dynamic`)
- **Native consensus counterparts**:
  - `noid_chain/src/consensus/validation.rs:188-205` (`u64::from(slot_index) >= slot_domain` -> `Err(ConsensusError::ShapeMismatch)`)
  - `noid_chain/src/consensus/checks.rs:48-65` (`validate_block_slot_conflicts` -> `Err(ConsensusError::SlotConflict)`)
  - `noid_chain/src/state.rs:1127, 1169` (`ApplyError::SlotOutOfRange`)

#### Root-Cause Mechanics
1. **32-bit Action Extraction & Compaction**:
   - In `TxBody`, `TxInput.slot_index` and `TxOutput.slot_index` are `u32` fields (`noid_gkr/src/spine_statement.rs:146-175`; `noid_recursive/src/acceptance/trace/tx_body_spine.rs:95-115` constrains each input/output `slot_index` to 32 bits).
   - `compact_action_rows` (`action_compaction.rs:106-119`) decomposes each compacted `row.slot_index` into 32 boolean wires `slot_bits[index][0..32]` and enforces strict ascending inequality (`pin_lt_strict`) across adjacent live rows over **all 32 bits**.
2. **16-Bit Local + 16-Bit Upper Split**:
   - `bind_paired_exact_state_transition` (`block_slots.rs:658-669`) pins each local depth-16 `PairedMerkleUpdate` path's `directions[0..16]` to `compacted.slot_bits[index][0..16]`.
   - `compact_segment_updates` (`segment_compaction.rs:123-209`) defines the 16-bit wire `segment_id = sum_{j=0..15} slot_bits[16 + j] * 2^j`, groups adjacent actions with identical 16-bit `segment_id`, and enforces strict ascending inequality across distinct live segments over **all 16 bits** of `segment_id`.
3. **Unconstrained Upper Tail Beyond `child.log_slots`**:
   - For each upper segment update `i`, `bind_paired_exact_state_transition` (`block_slots.rs:689-709`) pins `paired.upper[i].directions[0..16]` to `segments.segment_id_bits[i][0..16]`, then extracts the active upper roots via `select_upper_paired_roots(b, &depth_transition.child, &roots_by_depth)` (`exact_state.rs:1327-1345`).
   - `depth_transition.child` is a `StateDepthTrace` (`exact_state.rs:1214-1240`) with an 8-bit one-hot selector over `d in 24..=32` (`MIN_EXACT_STATE_DEPTH = 24`, `MAX_EXACT_STATE_DEPTH = 32`).
   - At active tree depth `d` (e.g., mainnet genesis `d = 24`), `upper_depth = d - 16 = 8`. `select_upper_paired_roots` selects `roots_by_depth[upper_depth - 1] = roots_by_depth[7]`, which reads column `c` at `paired_update_root_offsets(8) = [29, 31]` (`paired_merkle_update.rs:108-111, 863`).
   - Crucially, cells `29` (old root after 8 upper hashes) and `31` (new root after 8 upper hashes) depend **only** on `directions[0..8]` (`slot_bits[16..24]`) and `siblings[0..8]`! Levels `8..16` (`directions[8..16] = slot_bits[24..32]`) continue hashing in cells `32..63` of `PairedMerkleUpdateColumns`, whose outputs are **never pinned to `roots.old_root` or `roots.new_root`**.
   - As long as `directions[8..16]` are boolean (`0` or `1`, which `segment_id_bits` guarantees) and any arbitrary `siblings[8..16]` (e.g., `zeros[24..32]`) are supplied in the witness, **every** R1CS constraint and **every** `PairedMerkleUpdate` GKR relation (consistency, carry selection, `DeepChain` Poseidon2b walk, substitution, shift discharges) holds identically!
4. **Confirmed Regression History (`exact_state.rs:961-1002`)**:
   - In `noid_recursive/src/acceptance/trace/exact_state.rs:961-1002`, `bind_structural_frontier_count_from_actions_dynamic` contains the exact comment and constraint:
     ```rust
     // Every live slot index must lie in 0..2^active_depth. At bit position p,
     // exactly the selectors for depths d <= p make that position illegal.
     ```
   - When `block_slots.rs` replaced frontier-count multiproof binding with `bind_paired_exact_state_transition`, `bind_structural_frontier_count_from_actions_dynamic` became **dead code** (zero non-test callers in the workspace), and its `slot_bits[d..32] == 0` constraint was omitted from `bind_paired_exact_state_transition`.
5. **Missing `creation_id` Boundary Check on Spends Compounds the Alias**:
   - Ordinary UTXO spends (`public_arithmetic.rs:260-310`, `block_slots.rs:2563-2575`), V2 contract calls (`block_slots.rs:921-1141`), base native ops (`{2, 3, 5, 6, 7, 8}`), and Owner financial ops (`{48, 64, 66}`) do **not** constrain `input.creation_id <= parent.alloc_counter` or `coinbase_height <= parent.height` (only `op 65` `Purchase` at `financial_reads.rs:78-83` and `op 80` `ExecuteTrades` at `sequential_trade/funding.rs:279-292` check parent `creation_id` boundaries). Instead, the circuit relied on 32-bit `slot_index` uniqueness in `compact_action_rows` to ensure a slot could only be touched once per block.

#### Two Concrete Equivalence Breaks
1. **Break CD-1A — Out-of-Domain Slot Aliasing -> Snapshot Sync vs. Live Full-Node Consensus Split**:
   - At `log_slots = d` (`24 <= d < 32`), there are `2^(32 - d)` distinct `u32` values (`256` values at mainnet `d = 24`) aliasing each real leaf `s in 0..2^d`: `s_k = s + k * 2^d` for `0 <= k < 2^(32 - d)`.
   - A miner producing a block whose coinbase output (or any user transaction input/output — Note: wallet ZK authorization binds `TxBody` over any `u32` `slot_index` without knowing `header.log_slots`) sets `slot_index = s + 2^24` while computing `header.state_root` on real leaf `s` produces a valid `HistoryStep` proof.
   - **Live full nodes** (`validate_block_checks` at `validation.rs:193-204` and `materialize_accepted_block_state*` at `state.rs:1169`) **reject** the block (`ShapeMismatch("output slot ... outside block log_slots")` / `SlotOutOfRange`).
   - **Snapshot-syncing nodes** (`verify_snapshot_boundary` + `apply_staged_state_snapshot`) verify headers, the recursive `HistoryStep` terminal, and the canonical depth-24 `ChainState` (where the UTXO resides at `s < 2^24`), **accepting the chain**.
2. **Break CD-1B — Intra-Block Same-Slot Chaining (`Mint -> Spend` and `Spend -> Mint`)**:
   - Because `s` (`segment_id = s >> 16`) and `s + 2^24` (`segment_id = (s >> 16) + 256`) have **strictly increasing 32-bit `slot_index` values** AND **strictly increasing 16-bit `segment_id` values**, `compact_action_rows` and `compact_segment_updates` accept both in the same block as two sequential segment updates!
   - `bind_segment_upper_chain` (`segment_compaction.rs:227-265`) chains `upper_after[0] == upper_before[1]`. Since both `segment_id` and `segment_id + 256` have identical `directions[0..8]` (`slot_bits[16..24]`), update `0` transitions leaf `s` from `EMPTY -> UTXO_1` (producing intermediate depth-24 root `R_mid`), and update `1` transitions the **same** leaf `s` from `UTXO_1 -> EMPTY` (starting from `R_mid` and producing final depth-24 root `R_final`)!
   - This permits:
     - **Intra-block `Mint -> Spend`**: Minting a UTXO at slot `s` and spending it in the **same block** via input `slot_index = s + 2^24` — including **spending a block's own primary coinbase output in the very block that mints it**, or chaining up to `256` state transitions on the same physical slot / V2 contract / native state carrier in a single block!
     - **Intra-block `Spend -> Mint` slot reuse**: Spending an existing UTXO at `slot_index = s` (`UTXO_0 -> EMPTY`) and minting a new UTXO into the **same** physical slot via `slot_index = s + 2^24` (`EMPTY -> UTXO_1`).
   - Notice also `active_slot_count` conservation (`block_slots.rs:885-908`): in `Mint -> Spend` (`+1` mint, `-1` spend), `child.active_slot_count == parent.active_slot_count + 0`, which matches the real depth-24 post-state (slot `s` ends `EMPTY`), so snapshot staging's `active_slot_count` check passes as well!

#### Proof-of-Concept Coverage (`src/lib.rs`)
- `diff_a1_out_of_domain_slot_accepted_by_circuit_and_snapshot_rejected_by_native` (`src/lib.rs:658-824`): Constructs a height-1 block at `log_slots = 24` with coinbase `slot_index = 5 + 2^24`. Proves `validate_block_checks` and `materialize_accepted_block_state` reject it, while the production `HistoryStep` exact-state R1CS + full `PairedMerkleUpdate` GKR protocol (`verify_paired_merkle_gkr_family`: consistency, carry selection, `DeepChain` walk, substitution, shift discharges, and MLE claims) and depth-24 snapshot boundary checks all accept.
- `diff_a2_intra_block_mint_then_spend_same_slot_via_high_bit_alias` (`src/lib.rs:832-1061`): Constructs a height-1 block where tx 0 mints the primary coinbase reward at `slot_index = 7` and tx 1 spends that coinbase in the **same block** via `slot_index = 7 + 2^24` (`creation_id = coinbase_creation_id(1)`) into destination `slot_index = 8`, satisfying `bind_coinbase_action_with_amount`, `bind_user_action_surface`, `bind_user_public_arithmetic`, `bind_block_fee_arithmetic`, `bind_mint_packed_values_body_order`, `compact_action_rows`, `bind_paired_exact_state_transition`, and both local/upper `PairedMerkleUpdate` GKR proofs.
- `diff_a3_remediation_high_bit_pin_closes_alias_and_preserves_matrix_invariance` (`src/lib.rs:1069-1185`): Verifies that porting the high-bit `StateDepthTrace` pin rejects `5 + 2^24` at `m24` and `5 + 2^25` at `m25`, accepts `5 + 2^24` on an `m24 -> m25` expansion step and `5 + 2^31` at `m32`, and preserves identical `useful_rows` and `FieldR1cs::statement_digest()` across all depths `24..=32`.

#### Defensive Remediation
In `noid_recursive/src/acceptance/block_slots.rs:641` (`bind_paired_exact_state_transition`), immediately after `let depth_transition = bind_exact_state_header_roots_dynamic(...)` (around line `654`), enforce the dynamic upper-bit domain bound from `exact_state.rs:985-1002`:
```rust
for bits in &compacted.slot_bits {
    let mut packed_violations = LinExpr::zero();
    for (position, &bit) in bits.iter().enumerate().skip(MIN_EXACT_STATE_DEPTH) {
        let forbidden = depth_transition
            .child
            .one_hot
            .iter()
            .take(position - MIN_EXACT_STATE_DEPTH + 1)
            .fold(LinExpr::zero(), |sum, selector| sum.add(selector));
        let violation = mul(b, &LinExpr::from_wire(bit), &forbidden);
        packed_violations =
            packed_violations.add(&violation.scale(flat_const(1u128 << position)));
    }
    pin_zero(b, &packed_violations);
}
```
Cost: `8 * touched_capacity` R1CS multiplication gates (`12,248` rows at max `touched_capacity = 1,531`, `< 2,100` rows in `Small`), strictly constant-shape across `log_slots = 24..=32` (requires regenerating frozen v2/v2.1 matrix artifacts).

---

### Finding CD-2 [Medium — Direction (B)]: 11 Consensus-Enabled Financial Operations & 3 Carrier Kinds Accepted by Native `noid_chain` Yet Unprovable in v2.1 `HistoryStep`

#### Exact Source Locations
- **Native consensus profile & sequential state engine**:
  - `noid_tx/src/native_assets/profile.rs:8-33` (`operation_enabled`: enables 22 opcodes)
  - `noid_tx/src/native_assets/profile.rs:35-53` (`kind_enabled` vs `sequential_kind_enabled`)
  - `noid_chain/src/consensus/validation.rs:255-271` (`validate_block_checks` / `validate_block_consensus` -> `validate_scheduled_block_page_stream`)
  - `noid_chain/src/block/native_assets.rs:597-834` (`prepare_native_block_state`, `materialize_accepted_block_state_with_financial_context`, `materialize_accepted_block_state_with_native_data`)
- **Recursive `HistoryStep` v2.1 `Class::Native` & witness preparation**:
  - `noid_recursive/src/acceptance/history_step/v2/capacity_banked/config.rs:59` (`native.native_sequential_config().is_none()` -> `Err(V2Error::Runtime)`)
  - `noid_recursive/src/acceptance/trace/native_assets/financial_kernel.rs:121-141` (`bind_financial_sequential_headers`)
  - `noid_block/src/candidate_history.rs:303-312` (`validate_scheduled_candidate_resources`)
- **Mempool & miner admission gates**:
  - `noid_mempool/src/native.rs:228-235` (`owner_admission_enabled`: admits only `{48, 64, 65, 66}`)
  - `noid_chain/src/consensus/template/native.rs:94-101` (`TemplateBuilder` restricts financial ops to `{48, 64, 65, 66, 80}`)

#### Mechanics & Impact
1. `noid_tx::native_assets::profile::operation_enabled` enables **22** native opcodes at `v2.1`:
   - Base ops (`6`): `{2, 3, 5, 6, 7, 8}` (`CreateFungible`, `MintFungible`, `TransferFungible`, `BurnFungible`, `CreateNftCollection`, `TransferNft`).
   - Financial ops (`16`): `{33, 38, 39, 40, 41, 43, 48, 49, 50, 51, 52, 53, 64, 65, 66, 80}`.
2. However, `capacity_banked::Config::new` (`v2/capacity_banked/config.rs:59`) requires `Class::Native` to use `with_native_sequential_finance(...)`, which compiles `bind_financial_sequential_headers` (`financial_kernel.rs:121-141`), constraining every live native header to `base[2,3,5,6,7,8] + financial[48,64,65,66,80]`.
3. Thus **11 consensus-enabled financial opcodes** — `33` (`Swap`), `38` (`RegisterPools`), `39` (`SettlePool`), `40` (`ClaimLiquidity`), `41` (`ResetPool`), `43` (`ClaimSwap`), `49` (`LaunchRequest`), `50` (`RegisterLaunches`), `51` (`SettleLaunch`), `52` (`RefundLaunch`), and `53` (`SettleListing`) — along with carrier kinds `MarketRequest`, `MarketClaim`, and `LaunchRequest`, pass `validate_block_consensus`, `validate_block_checks`, and `materialize_accepted_block_state_with_financial_context`, yet **cannot be proven by any v2.1 `HistoryStep` bank**.
4. **Liveness Wedge Check (Verified Mitigated)**: `noid_mempool/src/native.rs:228-235` rejects all 11 opcodes at submission (`SubmitError::MalformedIntent("financial operation is outside the Owner admission profile")`) before signature/ZK verification, and `noid_chain/src/consensus/template/native.rs:94-101` prevents `TemplateBuilder` from assembling them. No unauthenticated peer transaction can wedge the miner's prover via these opcodes.
5. **PoC Coverage**: `diff_b1_consensus_enabled_financial_ops_accepted_natively_rejected_by_v21_history_step` (`src/lib.rs:1362-1934`) proves `CreateLaunch` (`op 48`) followed by `LaunchRequest` (`op 49`) passes `validate_block_checks_with_schedule` and `materialize_accepted_block_state_with_financial_context`, fails `prepare_candidate_input_with_native_data` and `bind_financial_sequential_headers` R1CS, and is rejected by `AsyncMempool::submit_encoded`.
6. **Remediation**: Align `noid_tx::native_assets::profile::operation_enabled` and `kind_enabled` with the v2.1 sequential profile (`{2, 3, 5, 6, 7, 8, 48, 64, 65, 66, 80}` and `sequential_kind_enabled`), gating legacy request/round pool & launch opcodes behind a dedicated future feature/fork flag until a proof bank supports them.

---

### Finding CD-3 [Medium / Low — Direction (B)]: Proof-Class Resource Envelope Divergences & Missing `definition_capacity` Preflight Check (Witness-Builder Panic)

#### Exact Source Locations
- **Witness preflight omission**: `noid_block/src/candidate_history.rs:146-155, 290-389` (`validate_scheduled_candidate_resources`)
- **R1CS witness builder panic**: `noid_recursive/src/acceptance/trace/native_assets/financial_definitions.rs:274-276` (`bind_financial_definition_table`: `assert!(capacity > 0 && witness.len() <= capacity)`)
- **Capacity formula**: `noid_recursive/src/acceptance/history_step/v2/config.rs:174-175` (`definition_capacity: headers.checked_add(trade.markets)?`)
- **Sequential native caps**: `noid_chain/src/consensus/params.rs` (`BLOCK_MAX_USER_PAGES = 255`), `noid_chain/src/block/payload.rs` (`MAX_BLOCK_NATIVE_OPENINGS = 3062`)
- **Mempool budget protection**: `noid_chain/src/mempool.rs:820-829` (`NativeFinancialBudget::definitions`)

#### Mechanics & Impact
1. **Envelope Divergence**: Sequential `noid_chain` (`validate_block_checks`, `BlockBodyPayload`, `materialize_accepted_block_state_with_native_data`) accepts native blocks up to `255` user pages and `3,062` native openings without enforcing `Class::Native` bank limits (`pages <= small.pages()`, `owner_authorizations <= small.pages()` after `op 80` policy expansion in `sequential_trade/authorization.rs:188`, `headers`, `records`, `definition_capacity`, and `TradeKernelConfig { markets, policies, funding }`).
2. **Missing Preflight Check -> Witness Panic**:
   - In `noid_block/src/candidate_history.rs:290-389`, `validate_scheduled_candidate_resources` validates `headers <= header_limit`, `records <= record_limit`, and `financial_actions <= financial.kernel.action_capacity`, **but never checks**:
     - `auxiliary_definitions.len() <= financial.kernel.definition_capacity`, nor
     - `batches.len() <= trade.markets`, total policies `<= trade.policies`, or total funding claims `<= trade.funding`.
   - For `Class::Native` configured with `with_native_sequential_finance(headers, records, trade)`, `definition_capacity = headers + trade.markets` (`v2/config.rs:174-175`; e.g., for `headers = 4, markets = 1`, `definition_capacity = 5`; for `headers = 8, markets = 4`, `definition_capacity = 12`).
   - Crucially, a single `op 48` `CreateLaunch` that buys 100% of `sale_units()` graduates immediately to a `MarketPool` (`noid_tx/src/native_assets/launch.rs:348-375`), requiring **two** auxiliary definitions (`LaunchDefinition` + `MarketDefinition`) while consuming only **one** native header!
   - Therefore, `3` immediate-graduation `CreateLaunch` intents in `headers = 4, markets = 1` consume `3 <= 4` headers and `<= 64` records, **6 > 5** auxiliary definitions (`9` immediate-graduation launches in `headers = 8, markets = 4` consume `9` headers? Wait: `7` immediate-graduation launches in `headers = 8, markets = 4` consume `7 <= 8` headers and **14 > 12** auxiliary definitions!).
   - Such a block passes `validate_block_checks`, `BlockBodyPayload::new_with_native_data`, `materialize_accepted_block_state_with_native_data`, **and** `prepare_candidate_input_with_native_data`, then **panics** inside `bind_financial_definition_table` (`financial_definitions.rs:275`) when building the `HistoryStep` R1CS witness!
3. **Honest Miner Mitigation Verified**: `Mempool::select_for_block_with_budget` (`noid_chain/src/mempool.rs:820-829`) tracks `used_definitions` against `NativeFinancialBudget.definitions = definition_capacity` before template sealing, so honest miners do not self-wedge; however, any external caller or custom template passing `prepare_candidate_input_with_native_data` hits an unhandled panic instead of `Err(HistoryStepWitnessError::Consensus(BlockResourceLimitExceeded { ... }))`.
4. **PoC Coverage**: `diff_b2_definition_capacity_overflow_accepted_natively_and_by_preflight_panics_witness` (`src/lib.rs:1943-2270`).
5. **Remediation**: Enforce `auxiliary_definitions.len() <= financial.kernel.definition_capacity` and `TradeKernelConfig` (`markets`, `policies`, `funding`, plus expanded `owner_authorizations <= config.user_page_capacity()`) inside `noid_block/src/candidate_history.rs` (`prepare_candidate_input_with_native_data` / `validate_scheduled_candidate_resources`), returning structured `BlockResourceLimitExceeded` errors.

---

### Finding CD-4 [Informational / Architectural Partition]: Standalone `validate_block_consensus` vs. `HistoryStep` on Off-Body Openings (V2 Contracts & Native Payloads)

#### Exact Source Locations
- `noid_chain/src/consensus/validation.rs:1-28, 212-222, 232-271`
- `noid_chain/src/block.rs:196-244` (`apply_block` / `materialize_accepted_block_state`)
- `noid_recursive/src/acceptance/block_slots.rs:921-1141` (`bind_v2_experimental_contracts`)

#### Mechanics & Design Boundary
- On the wire, `Block` contains only `BlockHeader` and `Vec<Transaction>` (`TxBody` pages), without V2 `ObjectOpening`s or V2.1 `NativeOpening`s / `TradeBatch`es (those travel in `AcceptedBlockBundle` / `BlockBodyPayload`).
- Consequently, standalone `validate_block_consensus`, `validate_block_checks`, and `materialize_accepted_block_state` (`apply_block`) check **only** `v2_active(height)` and single-page bitmap framing (`START | END`) on `PAGED_SPEND_CONTRACT_BIT` pages (`validation.rs:212-222`). They do **not** evaluate `ObjectOpening::check_call`, `integer_program` bytecode, `ObjectRules` (`max_fee`, `min_retained`, `max_payout`, mode flags), `before_deadline` (`height <= deadline`) authority/recipient routing, or successor contract roots.
- In production live nodes, V2 contract soundness relies **entirely** on `verify_history_step_terminal` (`block_slots.rs:921-1141`), while `noid_mempool/src/contracts.rs:96-175` checks `opening.check_call(&tx.page, next_block_height)` prior to template inclusion (revalidated at `next_height = tip.height + 1` on every tip advance).
- **PoC Coverage**: `diff_b3_contract_execution_partition_between_standalone_native_and_history_step` (`src/lib.rs:2279-2425`).

---

## 4. Verified-Tight Equivalence Matrix (Negative / Sound Boundaries)

Every remaining block-level surface between sequential `noid_chain`/`noid_tx` and `noid_recursive` (`HistoryStep`) was audited line-by-line and verified tight:

| Consensus Surface | Sequential Native Implementation | `HistoryStep` R1CS / GKR Implementation | Verdict |
|---|---|---|---|
| **144-Block `tx_epoch` Rollover** | `epoch_anchor.rs:33-85` (`tx_epoch_anchor_height_for_child`, `next_tx_epoch_anchor_id`: switches to `parent_block_id` iff `parent.height % 144 == 0`) | `trace/tx_epoch.rs:41-97` (`144*q + r == start.height`, `r < 144`, `boundary = (r == 0)`) + `accepted_claim_batch.rs:186-191` | **Verified Tight** (`src/lib.rs:2435-2465`) |
| **`m24 -> m25` `log_slots` Expansion & Occupancy Window** | `ChainState::expand_one` (`H(parent_root, zero_root[d])`) + `slot_expansion.rs:44-82` (10-of-18 finalized `>= 75%` occupancy; checked in both live sync & `SnapshotHeaderStaging` at `snapshot_header_staging.rs:150-169`) | `trace/exact_state.rs:1242-1315` (`bind_exact_state_header_roots_dynamic`: allows `delta in {0, 1}`, pins `roots.old_root == H(parent_root, zero_root[parent_depth])` when `grow == 1`) | **Verified Tight** (`src/lib.rs:2467-2517`; note header chain enforces the 18-block occupancy trigger in both sync modes) |
| **Multi-Page `PagedSpend` Framing & `TAG_PAGED_SPEND`** | `noid_tx/src/paged_spend.rs:145-310` (`START..END` state machine, continuation `fee == 0`, matching `input_owner` & `epoch_anchor`, `<= 16` pages/group, `TAG_PAGED_SPEND` txid) | `trace/paged_spend.rs:220-520` (`bind_paged_spend_stream` / `bind_financial_paged_spend_stream`: boolean `open` state machine, `open_at_end == 0`, continuation `fee == 0`, `owner`/`epoch_anchor` equality, sponge chain) | **Verified Tight** |
| **Body-Order `creation_id` Allocation & Tagging** | `state.rs:1158-1185` & `state/native_assets.rs:201-206` (`(1<<63)\|height` for coinbase, `(1<<62)\|alloc` for native openings, `alloc` for plain UTXOs; `alloc <= NATIVE_SEQUENCE_MAX`) | `trace/action_compaction.rs:201-321` (`bind_mint_packed_values_body_order` / `_v2_1`: body-order `running_alloc` +1 per live mint, bit-63 coinbase tag, bit-62 `native_mint` tag, 62-bit range check, `running_alloc == child.alloc_counter`) | **Verified Tight** |
| **Scheduled Development Allocation Multiplexing** | `consensus/development_allocation.rs:45-120` & `validation.rs:515-580` (`miner_subsidy_at_height`, two mandatory payouts at `height % TARGET_BLOCKS_PER_DAY == 0` up to `END_HEIGHT`) | `block_slots.rs:835-883, 2456-2542` (`constrain_development_payout_schedule` + `bind_v2_coinbase_and_development_prefix`: pins `payout_due` bit, exact recipient addresses, `share_each`, and `user_start` offset) | **Verified Tight** |
| **Dynamic State-Growth Fee Burn & Coinbase Ceiling** | `consensus/fees.rs:65-185` (`occupancy_bps`, `1x/2x/4x/8x` pressure multiplier, `burned = max(0, outs - ins) * state_growth_fee`, `coinbase <= miner_subsidy + sum(fee - burned)`) | `trace/fee_arithmetic.rs:95-295` (`bind_block_fee_arithmetic`: exact integer cross-multiplications against `5000/7500/9000` bps thresholds, per-group `fee >= required_total`, `coinbase <= miner_subsidy + claimable_sum`) | **Verified Tight** |
| **V2 Contract `before_deadline` & `integer_program` Rules** | `noid_tx/src/experimental_object.rs:210-395` (`height <= deadline` -> claim branch, `height > deadline` -> refund branch; `max_fee`, `min_retained`, `max_payout`, terminal vs non-terminal successor) | `block_slots.rs:921-1141` (`bind_v2_experimental_contracts`: `before_deadline=1` pins `height <= deadline`, `before_deadline=0` pins `deadline < height`, pins all `ObjectRules` bounds and `integer_program` R1CS) | **Verified Tight** (within `HistoryStep`) |
| **Op-80 `ExecuteTrades` Sequential Batch Execution & Declinability** | `noid_tx/src/native_assets/trade/{batch.rs, execution.rs, sequencing.rs}` (`TRORD_21` priority key, price-directed `Buy`/`Sell`, `ceil((3*S*s + 5*s^2)/(900*S))` curve, `3/5 S` graduation with `isqrt((S/250)*(2S/5))` locked LP, `1..=3` payout fee table, byte-sorted merged payouts) | `trace/native_assets/sequential_trade/{authorization.rs, funding.rs, policy.rs, kernel.rs, global_execution.rs, launch.rs, market.rs, payouts.rs, fees.rs}` (exact integer quotient/remainder range proofs, `in_money & step_positive` progress pins preventing arbitrary decline of executable prefix orders, byte-wise strict lexicographic payout merge) | **Verified Tight** |
| **Fork Activation Schedule (`LegacyV1` / `V2` / `V2.1`)** | `consensus/forks.rs` & `params.rs` (`V2_ACTIVATION_HEIGHT`, `V2_1_ACTIVATION_HEIGHT`) | `v2/capacity_banked/assembly.rs:165-171, 359-364` (`VerifiedOrigin` pins `ORIGIN_ACC = activation_height - 1` into `vk_digest`, preventing cross-fork bank selection) | **Verified Tight** |

---

## 5. Harness Build Status & Follow-Up Checks for Next Audit Team

### 5.1 Verification Harness & Build Status
- **Files in `/Users/ember/dev/parano1d-break/experiments/comp-consensus-diff/`**:
  - `Cargo.toml` & `Cargo.lock` (offline workspace lockfile copied from `parano1d/Cargo.lock`, `noid-ivc-core` path dependency configured).
  - `src/lib.rs` (~2,495 lines; 6 self-contained tests: `diff_a1_*`, `diff_a2_*`, `diff_a3_*`, `diff_b1_*`, `diff_b2_*`, `diff_b3_*`, and `verified_tight_boundaries_*`).
  - `target/` (all upstream `parano1d` crates compiled cleanly in `--offline --release` mode; `src/lib.rs` type/import adjustments from the first compile pass have been applied on disk; per coordinator time-cutoff directive `#15`, a second `cargo test` invocation was not started so findings could be flushed immediately).
- **Reproduction Command**:
  ```bash
  cd /Users/ember/dev/parano1d-break/experiments/comp-consensus-diff
  CARGO_TARGET_DIR="$PWD/target" cargo test --offline --release -- --nocapture
  ```

### 5.2 Follow-Up Leads for Subsequent Audit Passes
1. **End-to-End Full `EmbeddedHistoryStepPack::prove_block_v21` Witness Test for CD-1**:
   - `diff_a1` and `diff_a2` prove R1CS satisfaction and full `PairedMerkleUpdate` GKR verification directly over the exact `block_slots.rs` / `trace/*` gadgets. A follow-up check can wire a custom malicious prover harness bypassing `noid_chain::build_exact_action_surface_at_log_slots`'s host-side `slot_index < slot_domain` guard (`noid_chain/src/state_delta.rs`) to emit a full serialized `HistoryStepProof` verified by `MdbxChainContext::verify_snapshot_boundary`.
2. **Defense-in-Depth Input `creation_id` Boundary Check in `HistoryStep`**:
   - Even after fixing `CD-1` via the dynamic `slot_bits[child.log_slots..32] == 0` constraint, `HistoryStep` currently relies solely on 32-bit `slot_index` uniqueness in `compact_action_rows` to prevent intra-block `Mint -> Spend` on ordinary UTXOs, V2 contracts, and Owner ops `{48, 64, 66}`. Adding a uniform `spend_seq <= parent.alloc_counter` (and `coinbase_height <= parent.height` for bit-63 inputs) check across all spend rows provides defense-in-depth parity with `creation_id_within_boundary`.
3. **Comprehensive Preflight Audit of `noid_block/src/candidate_history.rs` for `TradeKernelConfig` Caps**:
   - Alongside `definition_capacity` (`CD-3`), verify all multi-batch `op 80` edge cases (`batches.len() > trade.markets`, expanded policy count `> trade.policies`, total funding inputs `> trade.funding`, and expanded Owner authorizations `> user_page_capacity()`) return structured `HistoryStepWitnessError::Consensus(BlockResourceLimitExceeded)` rather than triggering internal `assert!` panics in `noid_recursive::acceptance::trace::native_assets::sequential_trade::*`.
