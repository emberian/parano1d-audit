# Native Market, Mempool Fee-Floor, Op-80 Sequencer & `noid_pruv` Compositional Seam Audit

> **Replication status (2026-11-15):** Read the [independent claim matrix](../../comp-verification/REPLICATION-MATRIX.md) before treating a described impact as observed. The original market runner did not complete a valid end-to-end trade assembly; source traces and executed local admission controls have different scope.

Target: `parano1d/` at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e` (mirrored read-only at `../pinned/`).
Owned workspace: `/Users/ember/dev/parano1d-break/experiments/comp-system-hunter/native-market-seam/`.

## Execution & Build Status Note

- **Executed directly**: Binary decoding and verification of the pinned mainnet `v21-runtime-metadata.bin` (`NOID_V21_RELEASE_BANK = 6c280ae869afc25e31d46d73b42870512991b78c9498bc7e5ef939bc07e5ed5e`) and `v2-runtime-metadata.bin` (`NOID_V2_RELEASE_BANK = c2a6df736b0d0da22e285b6930b11cf44b520d65b52c7dfe78f44fe0cd48e76e`), establishing exact production class capacities across `Small`, `Large`, and `Native`, plus full line-by-line cross-crate source trace across `noid_tx`, `noid_chain`, `noid_mempool`, `noid_miner`, `noid_rpc`, `noid_block`, `noid_recursive`, and `noid_pruv`.
- **Conformance harness on disk (unbuilt per coordinator time-cutoff `#15`)**: `Cargo.toml`, `src/helpers.rs`, and `src/main.rs` implement end-to-end Rust conformance tests for Seams 1–4 against `../pinned/*` (and `/opt/homebrew/bin/z3` for `noid_pruv::verify`). Per board `#15`, no new `cargo build` / `cargo run` was started after the cutoff; run `CARGO_TARGET_DIR="$PWD/target" cargo run --release` in a follow-up pass to execute the binary suite.

---

## Pinned `v2.1` Proof-Bank Parameters (Decoded from `v21-runtime-metadata.bin`)

Decoding `v21-runtime-metadata.bin` (`O1V21BN1` / `O1V21K01`, `3,396,424` bytes) yields the exact mainnet v2.1 class limits (`noid_recursive/src/acceptance/history_step/runtime_parts_codec/capacity.rs:90-141`):

| Class | `outer_m` | `pages` | `max_live_inputs` | `contracts` | `native_headers` | `financial` `(actions, settle, phase)` | `sequential` `(markets, policies, funding)` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `Small` (`0`) | `23` | `63` | `504` | `63` | `0` | `None` | `None` |
| `Large` (`1`) | `24` | `206` | `504` | `63` | `0` | `None` | `None` |
| `Native` (`2`) | `24` | `63` | `504` | `63` | `5` | `Some((128, 34, Complete))` (`definitions = 9`) | `Some((4, 48, 64))` |

Schedule embedded in all three classes: `v1_1 = 95,125`, `v2 = (210,537, 30s)`, `authorization_activation = 279,370`.

---

## Seam 1: Dark Consensus Operations (`Swap` 33, `LaunchRequest` 49, `RegisterPools` 38–41/43, `RegisterLaunches` 50–53) vs Op-80 `ExecuteTrades` & `TemplateBuilder`

### Finding 1A — Sequential Consensus vs Pinned `Class::Native` Divergence, Permanent `PoolPhase::Frozen` Lock, and Total `TemplateBuilder` Abort

1. **Sequential Consensus (`noid_tx` + `noid_chain`) Enables Round-Based Operations**:
   - `noid_tx/src/native_assets/profile.rs:8-33` (`operation_enabled`) enables `Swap` (`33`), `RegisterPools`/`SettlePool`/`RefundMarket`/`ResetPool`/`ClaimSwap` (`38, 39, 40, 41, 43`), `LaunchRequest` (`49`), and `RegisterLaunches`/`SettleLaunch`/`RefundLaunch`/`ResetLaunch` (`50, 51, 52, 53`).
   - `profile.rs:35-53` (`kind_enabled`) enables `MarketRequest`, `MarketClaim`, and `LaunchRequest` openings.
   - Neither `noid_chain::consensus::validation::validate_block_checks` (`noid_chain/src/consensus/validation.rs:170-287`), `validate_block_consensus` (`validation.rs:574-653`), `prepare_native_block_state` (`noid_chain/src/block/native_assets.rs:597-805`), nor `MdbxChainContext::apply_next_block` (`noid_chain/src/storage/mdbx_context.rs:1325-1458`, `4012-4060`) restricts financial operations to `{48, 64, 65, 66, 80}`. A block containing `LaunchRequest` (`49`) + `RegisterLaunches` (`50`) or `Swap` (`33`) + `RegisterPools` (`38`) passes sequential consensus validation and commits `PoolPhase::Frozen` to `ChainState` and `MdbxStore`.

2. **Pinned Mainnet `v2.1` Recursive Bank (`Class::Native`) Blocks Ops Outside `{48, 64, 65, 66, 80}` (Correcting `dark-surface/README.md:106`)**:
   - Contrary to the earlier source-only note in `experiments/dark-surface/README.md:106` claiming `submitTxIntent` and the node admit and prove `Swap` (`33`) and `LaunchRequest` (`49`):
     - `noid_mempool/src/native.rs:228-231` rejects `!matches!(operation.code(), 48 | 64 | 65 | 66)` at mempool admission (`submitTxIntent` rejects `33` and `49` with `MalformedIntent("financial operation is outside the Owner admission profile")`).
     - `noid_chain/src/consensus/template/native.rs:94-96` rejects `!matches!(operation.code(), 48 | 64 | 65 | 66 | 80)`.
     - In the pinned `v21-runtime-metadata.bin`, `Small` and `Large` have `native_headers = 0` (`financial = None`), and `Native` has `native_sequential_config() = Some(TradeKernelConfig { markets: 4, policies: 48, funding: 64 })`. Whenever `native_sequential_config().is_some()`, **both** `noid_block/src/candidate_history.rs:303-313` and R1CS `bind_financial_sequential_headers` (`noid_recursive/src/acceptance/trace/native_assets/financial_kernel.rs:121-141`) + `sequential_kind_enabled` (`financial_opening.rs:56`) hard-pin live native headers to base `{2, 3, 5, 6, 7, 8}` and financial `{48, 64, 65, 66, 80}`.
   - Therefore, on the current `v21` proof bank, ops `33, 38..=41, 43, 49..=53` cannot produce a valid `HistoryStep` terminal; however, **`validate_block_consensus` (`noid_chain`) and `HistoryStep` (`noid_recursive` `Class::Native`) diverge** on these operations, and any future deployment of a non-sequential `V2Config::with_native_finance` class (`v2/config.rs:118-151`) alongside `Class::Native` makes them immediately reachable on-chain.

3. **Permanent Freeze + Total `TemplateBuilder` Wedge When `PoolPhase::Frozen` Reaches `MdbxStore`**:
   - **No automatic height-based thaw & zero settlement/reset builders**: `RegisterLaunches` (`noid_tx/src/native_assets/launch/queue.rs:75-82`) and `RegisterPools` (`market/queue.rs:97-106`) set `phase: PoolPhase::Frozen` with `timeout_height = height + 4`. Crossing `timeout_height` only changes `phase_at(height)` to `RoundPhase::Refund` (`launch.rs:326-329`, `market.rs:265-268`); returning to `PoolPhase::Idle` requires an explicit `SettleLaunch`/`ResetLaunch` (`51`/`53`) or `SettlePool`/`ResetPool` (`39`/`41`) transaction (`launch/queue.rs:131-149`). Exhaustive search confirms **zero** construction sites for `SettleLaunch`, `ResetLaunch`, `SettlePool`, or `ResetPool` anywhere in `noid_miner`, `noid_node`, or `noid_rpc`.
   - **Unfiltered `native_trade_parent_at` + fatal `?` in `trades::assemble`**:
     - `MdbxStore::native_trade_parent_at` (`noid_chain/src/storage/mdbx_store.rs:248-329`) resolves the single live `LaunchState` or `MarketPool` controller without checking `state.phase == PoolPhase::Idle`.
     - `AsyncMempool::submit_trade` (`noid_mempool/src/trade.rs:91-198`) admits `TradeOwnerIntent`s without checking market state.
     - During template assembly, `trades::select_candidates` (`noid_miner/src/template/trades.rs:117-125`) runs `assemble` for every candidate budget before class selection. For `Class::Native`, `assemble` (`trades.rs:476`) evaluates:
       ```rust
       let trial = stable_policies(parent, boundary, trial, fees)?;
       ```
       `stable_policies` (`trades.rs:268-269`) calls `SequencingPlan::execute(parent.market, boundary, &policies).map_err(|e| e.to_string())?`, which calls `SequentialLaunch::new` (`launch/sequential.rs:247`) or `SequentialMarket::new` (`market/sequential.rs:141`), returning `Err(...Phase)` (non-declinable in `sequencing.rs:121, 178-191`).
     - That `Err` aborts `select_candidates` at `noid_miner/src/template.rs:394-397` (`.ok()?`), causing **`TemplateBuilder::build_from_snapshot_with_limit` to return `None` across ALL classes (`Small`, `Large`, and `Native`)** — halting block template production for the entire node whenever even one `TradeOwnerIntent` for a `Frozen` market is in `AsyncMempool`.

### Finding 1B — Verified Boundary: `Purchase` (Op 65) Instant-Settlement vs Disabled `RegisterListings` (Op 70)

- `FinancialBlockTracker::owner_outputs` (`noid_chain/src/block/native_assets/financial.rs:276-278`) explicitly short-circuits `if matches!(operation, FinancialOperation::Purchase { .. }) { return Ok(()); }`, bypassing the legacy round-registration arm at `financial.rs:341-350`.
- `check_listing_purchase` (`noid_tx/src/native_assets/nft_market/owner.rs:170-220`) atomically consumes the `NftListing` input (`ListingState::initial`) and outputs seller NOID (`ask + NFT_FEE_CREDIT`) plus the buyer's `NftItem`.
- All NFT round/offer operations (`67..=75`, including `RegisterListings` `70` and `SettleListing` `71`) are disabled in `profile::operation_enabled` (`profile.rs:8-33`), and `NativeKind::NftPurchase` (`66`) and `NativeKind::NftOffer` (`67`) are disabled in `profile::kind_enabled` (`profile.rs:35-53`).

---

## Seam 2: Unsatisfiable Fee-Floor Contradiction on Fixed-Fee Financial Native Ops (`48, 64, 65, 66`) + Unfillable `TradeOwnerIntent` Median Latch

### Finding 2A — Unsatisfiable Fee Constraint (`fee == minimum && fee >= floor`) Blacks Out `CreateLaunch`, `CreateListing`, `Purchase`, and `CancelListing`

1. **Exact Equality Requirement (`fee == minimum`)**:
   - Consensus (`noid_chain/src/block/native_assets/financial.rs:157-162`), R1CS (`noid_recursive/src/acceptance/trace/native_assets/financial_kernel.rs:191-203`), and mempool (`noid_mempool/src/native.rs:155-166`) require owner financial operations (`CreateLaunch` `48`, `CreateListing` `64`, `Purchase` `65`, `CancelListing` `66`) to pay **exact** parent-derived `minimum`:
     ```rust
     if matches!(authorization, NativeAuthorizationContext::Financial(_)) && facts.fee != minimum {
         return Err(SubmitError::MalformedIntent(
             "financial fee differs from the parent State requirement".into(),
         ));
     }
     ```
2. **Simultaneous Dynamic Fee-Floor Inequality (`fee >= floor.max(minimum)`)**:
   - Immediately below at `noid_mempool/src/native.rs:167-175`:
     ```rust
     let required = state.floor.current().max(minimum);
     if facts.fee < required {
         return Err(SubmitError::Consensus(
             noid_chain::consensus::ConsensusError::BelowMinFee { required, actual: facts.fee },
         ));
     }
     ```
   - Wallet and stateless client planning (`noid_rpc/src/server/native/planner.rs:91-103`, called by `walletPreviewNative`, `walletSubmitNative`, and `prepareNativeIntent` in `client.rs:202-212`) enforce the same contradiction:
     ```rust
     if financial && self.floor > fee {
         return Err("the current relay floor exceeds the native financial fee".into());
     }
     ```
3. **Zero-Solution Blackout Thresholds (at `<50%` State Occupancy, `pressure_multiplier = 1`)**:
   - `CreateListing` (`64`): `2 -> 1` (`5,900 µNOID`) or `2 -> 2` (`6,600 µNOID`). Unsatisfiable once `floor >= 6,601 µNOID` (unless the user artificially fragments cash into dozens of extra UTXO inputs at `+100 µNOID`/input, which `Planner::funding` at `planner.rs:114-133` **never** does — it stops as soon as `available > changed`!).
   - `Purchase` (`65`): `2 -> 2` (`6,600 µNOID`) or `2 -> 3` (`9,800 µNOID`). Unsatisfiable via `Planner` once `floor > 9,800 µNOID`.
   - **`CancelListing` (`66`)**: Strictly fixed `1` input (`ListingState`) and `2` outputs (`NftItem` + seller refund) (`planner.rs:842-860`, `nft_market/owner.rs` / `financial/owner.rs:240-251`), giving **exact immutable `minimum = 9,000 µNOID`**. Even a custom raw client cannot add extra inputs/outputs to raise `minimum`. Whenever `floor >= 9,001 µNOID`, **no valid `CancelListing` transaction can enter `AsyncMempool` at any fee** (`fee == 9,000` fails `BelowMinFee`; `fee > 9,000` fails `MalformedIntent` and consensus `FinancialBlockError::Fee`).
   - `CreateLaunch` (`48`): `1 -> 4` (`15,400 µNOID`) or `1 -> 5` (`18,600 µNOID`). Blacked out via `Planner` once `floor > 18,600 µNOID`.

### Finding 2B — Non-Draining 144-Block (~72-Minute) `FeeFloor` Latch via Unfillable `TradeOwnerIntent`s

Earlier analysis (`experiments/fee-floor/REPORT.md:9-11, 31`) assumed (i) any transaction paying the raised floor still enters the mempool, and (ii) honest miners drain a 770-entry low-fee backlog within a few blocks so the floor resets below 50% (`512/1,024`). Both assumptions break under composition with `TradeOwnerIntent`:

1. **Unbounded, Unindexed-Asset `TradeOwnerIntent` Admission**:
   - `AsyncMempool::submit_trade` (`noid_mempool/src/trade.rs:91-198`) checks `TradeIntent::checked` (`noid_tx/src/native_assets/trade.rs:75-105, 196-204`), `trade_policy_resources`, `fee_limit >= minimum`, `fee_limit >= state.floor.current()`, and funding UTXO slots in `ChainView`.
   - Unlike `planner.rs:893-895` (which caps wallet `valid_until <= height + 30`), `TradePolicy::validate` (`trade.rs:84`) only requires `valid_from <= valid_until` (allowing **`valid_until = u64::MAX`**), and `submit_trade` **never** checks that `policy.asset` has a live market in `MdbxStore` or that `minimum_output` (`<= MARKET_QUANTITY_MAX`) is satisfiable.
2. **Counts Toward Occupancy Pressure, Skips Median Window, Never Drained by Miners**:
   - In `AsyncMempool::retain_admitted_intent` (`noid_mempool/src/pool.rs:548-552`):
     ```rust
     if !is_trade {
         st.floor.record(fee);
     }
     refresh_fee_floor(st, &self.config, projected_bytes);
     ```
   - Admitting **26** ordinary 1->1 self-transfers at `22,223 µNOID` (`0.578 NOID` total fees; `0` net if self-mined) plus **794** unfillable `TradeOwnerIntent`s (`asset = AssetId([0xAA; 32])` or `minimum_output = MARKET_QUANTITY_MAX`, `valid_until = u64::MAX`, `fee_limit = 25,000`) reaches `820 / 1,024` (`80%`), latching `pressure_active = Some(true)` and setting `floor = floor(0.9 * 22,223) = 20,000 µNOID` (`noid_mempool/src/floor.rs:84-88, 104-113`).
   - When the next block confirms the 26 ordinary transfers, **`794` unfillable `TradeOwnerIntent`s remain** (`794 >= 512`, above the 50% reset threshold at `floor.rs:80-82`).
   - Honest miners **never** include those 794 `TradeOwnerIntent`s (`trades::assemble` skips unindexed assets at `noid_miner/src/template/trades.rs:454-460` and drops unfilled `minimum_output` in `stable_policies`), so **0 µNOID in fees is ever paid** on the 794 trade entries and their 794 funding UTXOs remain unspent on-chain, keeping `floor = 20,000 µNOID` latched until the 144-block (~72-minute) transaction epoch anchor rotates (`noid_mempool/src/trade.rs:205-206`).
   - Throughout that window, NFT sellers are completely unable to relay `CancelListing` (`9,000 < 20,000`), while a block producer can still include a direct `Purchase` at `6,600 µNOID` (consensus checks only `MarketFeeContext::required`, not local `FeeFloor`), and `TradeOwnerIntent`s with `fee_limit >= 20,000` continue to admit and settle on-chain at the baseline `6,600`–`9,800 µNOID` consensus fee (`noid_tx/src/native_assets/trade/execution.rs:297-320`).

---

## Seam 3: Op-80 `ExecuteTrades` Compositional Edge Cases

### Finding 3A — Single-Intent Global `financial.funding = 64` Exhaustion Starves All 4 Markets

- In the pinned `v21-runtime-metadata.bin`, `Class::Native` sets `TradeKernelConfig { markets: 4, policies: 48, funding: 64 }` (`noid_miner/src/template.rs:655-666`).
- `AsyncMempool::submit_trade` (`noid_mempool/src/trade.rs:115-116`) checks individual trade admission against the full `Class::Native` budget via `trade_policy_resources` (`noid_chain/src/mempool.rs:357-386`).
- A **single** `TradeOwnerIntent` carrying **64 funding inputs** (`1` principal input with `amount = gross + fee` to control `funding_priority` via `Reverse(*units)` at `noid_tx/src/native_assets/trade.rs:236-253`, plus `63` `1-µNOID` dust inputs) uses:
  - `pages = 2 + ceil(64 / 8) = 10 <= 63`,
  - `actions = 64 + 5 = 69 <= 128`,
  - `policies = 1 <= 48`, `markets = 1 <= 4`, **`funding = 64 <= 64`**,
  - Consensus fee (`65` inputs -> `3` outputs, `net_new = 0`, **`0` burned state-growth fee**): `5,000 + 65 * 100 + 3 * 700 =` **`13,600 µNOID` (`0.0136 NOID`)** (`12,900 µNOID` with `1` user payout).
- When this single policy wins the first `funding_priority` slot in `trades::assemble` (`noid_miner/src/template/trades.rs:424-444, 497-511`), `available.try_consume(...)` reduces `remaining.financial.funding` from `64` to **`0`**.
- Every subsequent `TradeIntent` across **all 4 markets and all 47 remaining policy slots** fails `if funding > financial.funding { continue; }` (`trades.rs:471`). Compared to `SEQ-1` (`experiments/sequencing/README.md:60`, which modeled 64 separate intents = 64 ZK proofs + `576,000 µNOID`), global `ExecuteTrades` capacity across all markets is exhausted by **1 ZK proof and `13,600 µNOID` (`48x` fewer proofs, `42x` lower fee, `0` burn)**.

### Finding 3B — Non-Monotonic Priority Inversion & Batch Shrinkage in `trades::assemble` + `stable_policies`

In `noid_miner/src/template/trades.rs:465-506`, `assemble` visits policies for an asset in ascending `funding_priority` order and updates `accepted`:
```rust
let mut trial = accepted.clone();
trial.push((**intent).clone());
let trial = stable_policies(parent, boundary, trial, fees)?;
if trial.is_empty() || trial == accepted {
    continue;
}
...
if available.try_consume(...) {
    accepted = trial;
    selected = Some((group, available));
}
```
Because `SequencingPlan::execute` (`noid_tx/src/native_assets/trade/sequencing.rs:142-153`) reorders `remaining` at every step by **price-directed side preference** (`preferred = LaunchSide::Buy` while `current_price <= initial_price`, `preferred = LaunchSide::Sell` while `current_price > initial_price`), appending a **lower-priority** candidate `B` to `accepted = [A_1, ..., A_k]` can execute `B` **ahead** of higher-priority `A_1..A_k`:

1. **Zero-NOID Graduation Displacement (`k` High-Priority Sells Replaced by 1 Worst-Priority Buy)**:
   - On a Launchpad curve at `sold = sale_units - 1`, suppose `accepted` holds `k` (up to `47`) higher-priority `Sell` intents `A_1..A_k` priced against the `0%`-fee curve (`< 30 bps` slippage tolerance).
   - Appending a **worst-priority** `Buy` intent `B` (`gross_input = 1`, `minimum_output = 1`, spending **`0 µNOID`** on the flat curve ceiling `C(cap) - C(cap - 1) = 0`, `launch/sequential.rs:164-185`) causes `SequencingPlan::execute` at step 0 (`current_price == initial_price => preferred = Buy`) to execute `B` **first**, graduating the curve into the `0.30%`-fee AMM pool (`TradeState::Market`).
   - All `A_1..A_k` now execute against the `0.30%`-fee pool, fall below `minimum_output`, and are marked `unfilled`.
   - `stable_policies` (`trades.rs:261-286`) filters out all `k` higher-priority `Sell`s and returns `trial = [B]`. Because `trial != accepted` and `!trial.is_empty()`, **`assemble` overwrites `accepted = [A_1..A_k]` with `[B]`**, evicting `k` higher-priority trades and shrinking miner `claimable_fees` on that market by `k : 1`.
2. **Payout-Count Expansion (`1 -> 2` Payouts) Breaking `quote_trade_charge`**:
   - If high-priority `A_0` (`Buy`) raises `current_price > initial_price`, step 1 sets `preferred = Sell`, jumping a worst-priority `Sell` `B` ahead of high-priority `Buy` `A_1`.
   - If `A_1` had `fee_limit = fees.0[0]` (`6,600 µNOID`, exact curve spend `refund_noid == 0 => 1` payout alone), `B` shifting `sold` on the curve can leave `A_1` with a nonzero unspent NOID remainder (`refund_noid >= 1 => 2` payouts, requiring `fees.0[1] = 9,800 µNOID > fee_limit` in `quote_charge`, `execution.rs:289-322`). `stable_policies` drops higher-priority `A_1` and retains `[A_0, B]`.

### Finding 3C — `RedeemToken` (Op 3) Before Graduation + Verified Multi-Batch & `log_slots` Boundaries

- **Pre-Graduation `RedeemToken` Floor Lock**: `CreateLaunch` (`noid_tx/src/native_assets/launch/owner.rs:93-135`) initializes `TokenReserve` with `outstanding_units = initial_supply` (`S`) while all `S` units reside inside `LaunchState`. When a buyer acquires `q` `TokenBalance` units on the curve (`LaunchState.inventory_units = S - q`, `market_noid = C(q)`) and executes `RedeemToken` (`transition.rs:237-252`, GUI `BurnToken`), `TokenReserve` burns `q` units and pays pro-rata `1,000 µNOID/token` backing, while `LaunchState` retains `sold = q` and `market_noid = C(q)`. Because those `q` units no longer exist in any `TokenBalance` UTXO, `sold` can never return below `q`, permanently locking `C(q)` NOID into the curve/pool reserve floor; buying the remaining `sale_units - q` units still graduates cleanly into `MarketPool` with exact conservation.
- **Verified Tight Boundaries**:
  - `noid_chain/src/block/payload/trades.rs:229-234` (`total > MAX_PAGED_SPEND_INPUTS`): `MAX_PAGED_SPEND_INPUTS == BLOCK_MAX_LIVE_INPUTS == 1,020` (`noid_tx/src/paged_spend.rs:43`, `noid_chain/src/consensus/params.rs:158`), while `Class::Native` caps `max_live_inputs = 504` and `markets + funding <= 4 + 64 = 68 <= 1,020`; `trades::assemble` cannot exceed `MAX_PAGED_SPEND_INPUTS`.
  - `definitions` deduplication (`trades.rs:361-366, 377, 535-539` & `noid_chain/src/mempool.rs:820-829`) matches `resolve_native_closure` and `bind_financial_definitions` (`definition_capacity = 5 + 4 = 9`).
  - `log_slots` expansion (`m24 -> m25`): `trades::placement` (`trades.rs:150-166`), `MarketFeeContext::from_state(&snapshot.state)` (`trades.rs:299, 443`), `checked_candidate` (`template/native.rs:134-146`), and `prepare_trade_batch_from_claims` (`state/native_assets/trade.rs:144-156, 236-251`) all price fees on the immutable parent `log_slots` while validating output slot indices against `child_log_slots`.

---

## Seam 4: `noid_pruv` Compiler/Verifier & `experimental_object` / `NativeTemplate` Seam

### Finding 4A — Unsound `noid_pruv::verify::smt` Model (`Verdict::Verified` on Unspendable Contracts + Spurious `Counterexample`s)

`noid_pruv::verify::smt` (`noid_pruv/src/verify.rs:153-258`) encodes **only** the 16-instruction `integer_program::execute` transition (`noid_tx/src/experimental_object/integer_program.rs:361-410`) over unconstrained 64-bit bitvectors `fee`, `payout`, `retained`, `input_amount`, `height`, `payout_owner0..3`, `input_creation_id`, `u32`-bounded `input_slot`/`retained_slot`/`payout_slot`, and unconstrained `Bool`s `terminal`, `has_payout`. It omits **all** enclosing contract and page-validity constraints enforced on every on-chain call by `ObjectOpening::check_call` (`noid_tx/src/experimental_object.rs:166-223`) and `validate_paged_spend` (`noid_tx/src/paged_spend.rs:406-496`):

1. `ObjectRules::permits(height < deadline, terminal)` (`experimental_object.rs:189-191`, `policy.rs:50-58`: `CLAIM_CONTINUE`, `CLAIM_CLOSE`, `RECOVERY_CONTINUE`, `RECOVERY_CLOSE`).
2. `fee <= rules.max_fee`, `!terminal => retained >= rules.min_retained`, and `!terminal => payout <= rules.max_payout` (`experimental_object.rs:192-201`).
3. `terminal => !has_payout` (`experimental_object.rs:177-185`) and canonical dead-output zeroing `!has_payout => (payout == 0 && payout_slot == 0 && payout_owner0..3 == 0)`.
4. Exact NOID conservation `input_amount == retained + (if has_payout { payout } else { 0 }) + fee` without `u64` overflow (`paged_spend.rs:485-495`) and non-overlapping live slots (`paged_spend.rs:430-475`).
5. Recipient binding `!terminal && has_payout && (modes & ANY_PAYOUT_RECIPIENT == 0) => payout_owner == recipient_at(height)` (`experimental_object.rs:202-207`).

**Impact**:
- **False `Verdict::Verified` on Permanently Locked Contracts**: Query 1 (`verify.rs:246, 352`) checks `(check-sat)` over the unconstrained SMT state space rather than `ObjectOpening::check_call`. Any contract whose `policy` and `call` blocks have an empty intersection — e.g., all four mode permissions set to `false` (`modes == 0`), `max_payout_micronoid = 0` paired with `require payout >= 100;`, close-only modes paired with `require !terminal;`, or `require terminal; require has_payout;` — compiles via `pruv check`/`pruv build`, is accepted by `paranoid_createObject` (`noid_rpc/src/server/objects.rs:211-249`), and **receives `Verdict::Verified` from `pruv verify`**, despite **100% of on-chain calls failing `ObjectOpening::check_call`** (permanently stranding any NOID funded to `opening.root()`).
- **Spurious `Verdict::Counterexample` on True Protocol/Policy Postconditions**: Valid contracts declaring `ensures fee <= max_fee;`, `ensures retained <= input_amount;`, or `ensures !terminal == true;` (under `claim_can_close = false; recovery_can_close = false;`) fail `pruv verify` with `Verdict::Counterexample` because Z3 selects impossible assignments such as `fee = u64::MAX` or `retained > input_amount`.
- Additionally, `verify.rs:129-136, 248-256` hoists arithmetic overflow/underflow checks (`expr.checks`) unconditionally to the top-level conjunction in `conditions`, so guarded postconditions such as `(old(x) == 0) != (x == old(x) - 1)` fail with `Counterexample` at `old(x) == 0`.

### Finding 4B — Zero-Address Policy Acceptance & `NativeTemplate` Planner Mismatches (Plus Verified `Lowering` Soundness)

- **Zero-Address (`Address([0; 32])`) Acceptance in `noid_pruv::compile` & `ObjectOpening::validate`**:
  - Whereas `noid_pruv::native::Owner` (`noid_pruv/src/native.rs:84-93`) explicitly rejects `address == Address([0; 32])` and non-canonical Bech32 strings, `noid_pruv::compile` (`noid_pruv/src/compile.rs:217-219, 600-605, 652-685`), `ObjectOpening::validate` (`noid_tx/src/experimental_object.rs:86-94`), and `noid_rpc/src/server/objects.rs:235-238` (`parse_address_param` at `noid_rpc/src/server.rs:3087-3093`) all accept `Address([0; 32])` for `claim_authority`, `recovery_authority`, `claim_recipient`, and `recovery_recipient` (an address with no spending preimage under `TAG_ADDRFIX`).
- **`NativeTemplate` vs `Planner` Bounds Gaps (`noid_pruv/src/native.rs:197-253`)**:
  - `NativeTemplate::Trade` (`native.rs:230-237`) only checks `*valid_until != 0`, so `pruv native check` approves `valid_until = u64::MAX`, which `paranoid_walletPreviewNative` / `prepareNativeIntent` (`noid_rpc/src/server/native/planner.rs:893-895`) rejects (`"trade expiry must be within30 blocks"`).
  - `NativeTemplate::TransferToken` and `BurnToken` (`native.rs:128-136, 250`) omit the `LAUNCH_MAX_SUPPLY` (`10^15` units) ceiling enforced on all fungible launches (`launch.rs`).
- **Verified Soundness of `Lowering` (`noid_pruv/src/compile.rs:301-575`)**:
  - Audited register allocation (`Scratch0`/`Scratch1`), unconditional `Move register <- 0 (when ALWAYS)` reset before nested `when` guards (`compile.rs:551-559`), boolean inversion (`!=`, `<=`, `>=` at `compile.rs:417-448`), two-immediate rejection (`compile.rs:336-341`), and 16-step `Instruction::to_fields` packing (`compile.rs:765-770`) against `integer_program::execute`: all lowered instructions preserve exact ABI 3 semantics.

---

## Recommended Defensive Remediations

1. **Seam 1 (`noid_tx`, `noid_chain`, `noid_miner`)**:
   - Align `noid_tx/src/native_assets/profile.rs:8-53` (`operation_enabled` and `kind_enabled`) with `sequential_kind_enabled` (`{2, 3, 5, 6, 7, 8, 48, 64, 65, 66, 80}`) until automatic sequencer settlement/reset support exists in `noid_miner`.
   - Filter `state.phase == PoolPhase::Idle` in `MdbxStore::native_trade_parent_at` (`mdbx_store.rs:292-302`) and treat non-idle `TradeParent` errors in `trades::assemble` (`noid_miner/src/template/trades.rs:476`) via `continue` rather than `?` so one unusable market never aborts block template construction for unrelated transactions.
2. **Seam 2 (`noid_mempool`, `noid_rpc`)**:
   - Exempt fixed-fee consensus-pinned financial owner operations (`48, 64, 65, 66`) from the `state.floor.current()` inequality in `noid_mempool/src/native.rs:167-168` and `noid_rpc/src/server/native/planner.rs:99-101` (or allow consensus/R1CS `>= fee` tips on financial owner operations).
   - In `AsyncMempool::submit_trade` (`noid_mempool/src/trade.rs:91-198`), enforce `valid_until <= height + MAX_TRADE_VALIDITY_WINDOW` (`30` blocks), require an active idle market for `intent.policy.asset`, and track `TradeOwnerIntent` occupancy separately from ordinary fee-bearing pool pressure in `FeeFloor` (`pool.rs:548-552`).
3. **Seam 3 (`noid_mempool`, `noid_miner`)**:
   - Cap per-policy `intent.funding.len()` at mempool admission (e.g. `<= 8` or `<= 16` inputs) so a single `TradeOwnerIntent` cannot consume all `64` global `financial.funding` slots across all 4 markets.
   - In `trades::assemble` (`noid_miner/src/template/trades.rs:476-505`), guard against batch regression by rejecting `trial` when `trial.len() < accepted.len()` (or comparing `group.claimable_fees >= previous_group.claimable_fees`) so a lower-priority policy cannot evict already-accepted higher-priority fills.
4. **Seam 4 (`noid_pruv`, `noid_tx`, `noid_rpc`)**:
   - Assert the full `ObjectOpening::check_call` and `validate_paged_spend` preconditions (`ObjectRules::permits`, `fee <= max_fee`, `!terminal => retained >= min_retained && payout <= max_payout`, `terminal => !has_payout`, dead-output zeroing, value conservation, recipient binding, and distinct slots) in `noid_pruv::verify::smt` (`verify.rs:180-186`), and scope subexpression overflow/underflow checks to the boolean guard context rather than hoisting unconditionally.
   - Reject `Address([0; 32])` in `noid_pruv/src/compile.rs:600-605`, `ObjectOpening::validate` (`experimental_object.rs:86-94`), and `noid_rpc/src/server/objects.rs:235-238`, and enforce `LAUNCH_MAX_SUPPLY` and relative `valid_until` bounds in `noid_pruv/src/native.rs`.
