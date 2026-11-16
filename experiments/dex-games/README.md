# DEX ordering, launch priority, and graduation

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`, through the dex-economics experiment's `pinned/` mirror. That experiment verified the mirror against `git archive 52938433`.

The PoC calls the pinned `noid_tx` and `noid_chain` libraries locally. It does not run a node, miner, mempool, peer, or RPC server.

The preceding dex-economics experiment established cumulative-ceiling pricing `C(q) = ceil((3Sq + 5q²)/(900S))`, a zero-spend final-unit graduation trigger, a 299-unit first-μNOID buy, and `C(cap) = S/250` exactly. This experiment tests who receives the last free units and the first trades before and after graduation.

**Result:** The shipped miner orders admitted intents by a parent-salted funding digest, independent of fee bids. Consensus accepts a builder-selected subset, so the builder can choose which admitted trades to include. The final 100 curve units cost no additional cash, but are worth only 1 μNOID at the terminal price; taking them incurs at least a 9 800 μNOID trade-group fee in the tested minimum-fee state. No accepted value-extracting transition or invalid ordering was demonstrated.

## Inclusion and execution rules

1. **The builder controls inclusion.** Block validation re-derives the submitted trade batch (`noid_chain/src/block/native_assets.rs:718-747`) against parent state. It does not require inclusion of any mempool intent. Ordinary block transaction ordering is explicitly miner policy (`noid_chain/src/consensus/ordering.rs:1-15`); output-slot conflicts resolve by smallest txid (`noid_chain/src/consensus/conflict.rs:19-37`). A builder can include its own eligible trade and omit competitors without violating these checks. This is a code-path conclusion; the PoC did not build such an adversarial block.
2. **The shipped miner uses a parent-salted digest.** It ranks mempool trade intents by `funding_priority` (`noid_miner/src/template/trades.rs:424-433`) and includes them greedily per market (`:465-479`). The sequencing engine uses the same digest order, with a price-directed side preference (`noid_tx/src/native_assets/trade/sequencing.rs:136-153`). The digest is `H("TRORD_21", parent_id, asset, anchor.creation_id, anchor.slot_index)` (`noid_tx/src/native_assets/trade.rs:231-264`). Fees, size, recipient, and owner do not enter it. More eligible principal UTXOs give an actor more independent candidate anchors. With n candidates against m, the uniform-hash model predicts n/(n+m); 4 096-trial samples gave 5.5/19.0/49.4/79.0% for n = 1/4/16/64 against m = 16, versus 5.9/20/50/80% predicted.
3. **Authorization-key order encodes the batch; it does not execute it.** `TradeBatch::new` sorts keys for canonical encoding (`noid_tx/src/native_assets/trade/batch.rs:125-140,156-164`). In the PoC, two intents with reverse key and digest orders executed in digest order. Changing authorization keys did not improve priority under the shipped assembler.
4. **The free tail has a small face value.** At minimum, mid, and maximum supply, the last 100 token units require zero additional curve cash and have a terminal marginal value of 1 μNOID. The exact curve integral and graduation cash both equal `S/250`; the ceiling's earlier overcharge is returned in the flat tail. The minimum trade-group fee in the quietest tested state is 9 800 μNOID (5 000 base + 2·100 + 3·700 + 1·2 500; `noid_chain/src/consensus/params.rs:428-446`, `noid_chain/src/consensus/fees.rs:97-121`). Selling those 100 units alone into the opening pool floors to zero NOID and is dust-rejected; 102 units is the smallest sellable lot. Ignoring timing value and any other trades, the sweep costs at least 9 799 μNOID more than the tail's marginal value.

## The ordering stack, as implemented

| Layer | Rule | Where |
|---|---|---|
| Batch encoding | policies sorted by `authorization_id` (strictly increasing) | `noid_tx/.../trade/batch.rs:125-140,156-164` |
| Batch execution | ascending `funding_priority` digest, price-directed side preference each step; declinable fills dropped (whole batch still valid without them at the miner) | `noid_tx/.../trade/sequencing.rs:136-161` |
| Miner inclusion | all mempool intents ranked by the same digest; greedy per-market assembly in digest order; unfilled intents dropped, uncharged | `noid_miner/src/template/trades.rs:261-286,424-479` |
| Block tx order (ordinary txs) | coinbase first, fee-descending, txid tie-break — explicitly miner policy | `noid_chain/src/consensus/ordering.rs:25-43` |
| Consensus constraint | re-derive the submitted batch against parent state; no membership or ordering fairness rule | `noid_chain/src/block/native_assets.rs:718-747` |
| Fee priority | **none**: the charge is the first fitting state-derived schedule minimum (`fee_limit` is a ceiling; `quote_charge` picks the lowest quote that fits); no tip mechanism exists | `noid_tx/.../trade/execution.rs:288-322`, `noid_tx/.../trade.rs:187`, `noid_mempool/src/trade.rs:117-131,179-186` |

## Tail sweep and competing buys

Rules as implemented: at `sold` anywhere in the final flat window (exactly the last 100 units, any supply), a buy's binary search (`noid_tx/.../launch/sequential.rs:147-169`) reaches the cap within `cash_limit = S/250 + gross`, so *any* buy whose gross ≥ 1 sweeps all remaining units for `spent = 0` and fires graduation (`:179-185`); a remainder gross above 1 continues into the opening pool in the same fill (`:198-219`).

Who wins, mechanism: with both racers' intents admitted (distinct anchors — the mempool rejects shared input slots, `noid_mempool/src/trade.rs:187-196`, and the batch rejects duplicate funding slots, `batch.rs:166-169`), the miner walks them in digest order; the first racer's fill sweeps the tail and graduates; the second racer's 1-μNOID buy is now a dust buy against the pool, is *declined* (declinable `Quantity`/`Minimum`, `sequencing.rs:161,178-191`), and the miner's `stable_policies` filter drops it and retries — the winner-only batch checks clean (`trades.rs:261-286`). PoC-verified end to end, including that the dropped racer is charged nothing (charges exist only for fills, `noid_tx/.../trade/execution.rs:265-284`).

Loser's fate: the intent is parent-agnostic (the digest's `parent_id` enters only at assembly), so it stands in the mempool across blocks (eviction only on epoch-anchor change, expiry, consumed inputs, fee floor: `noid_mempool/src/trade.rs:200-230`) — a free retry lottery each block, forever unfillable as dust. A loser who sizes up (400 NOID gross in the PoC) fills against the opening pool in the *same* batch at pool prices: repriced, not rejected.

Prize: 100 units, exact value 1 μNOID, fee floor 9 800 μNOID, standalone monetization dust-rejected (min lot 102 units for 1 μNOID gross). Strategic consequence: the tail race is a negative-EV object except for the graduation-timing option it carries; the rational "attack" is not to snipe the tail but to *choose when graduation happens* (Game 2).

## Graduation timing

The pool opens seeded `(S/250, 2S/5)` (`noid_tx/.../launch.rs:156-172`), and PoC asserts the opening price `base/quote` equals the curve's terminal marginal price `(3S + 10·cap)/(900S) = 1/100` *exactly* (u128 cross-multiplication). There is no price seam at the transition: the surplus is not in the pool (the exact integral equals the seed), a fair-priced pre-graduation buy round-tripped through the opening pool loses 12 μNOID on fees and impact (PoC control), and the pool's 0.30 % both-ways fee plus constant-product impact make same-block or next-block sandwiching around the transition unprofitable by construction (consistent with the dex-economics lane's 500/500 pool-roundtrip controls).

What timing control is actually worth: (a) the *same-fill remainder leg* — the graduator's single buy can take the free tail and then buy the opening liquidity atomically before any other actor can touch the pool (PoC: 10 NOID gross → 100 free units + 972 754 103 pool units for a 30 000 μNOID pool fee); (b) same-batch paired intents (up to 64 policies, `noid_tx/.../market/sequential.rs:12`, `noid_chain/src/block/payload/trades.rs:26`) let the graduator also sell pre-held inventory into the opening pool in the same block — though competitors' pool buys rank in the *same* digest order in that batch, so the opening-liquidity race is not exclusive to the graduator. This is optionality and first-mover flow, not mechanical value: every leg is fairly priced at execution.

## First trade after launch

Creator rules: the creation transition itself may bundle a creator purchase of zero, part, or the *entire* curve (`noid_tx/.../launch/owner.rs:70-137`, the purchased `TokenBalance` output feeding `after_initial_purchase` at `:110`; `noid_tx/.../launch.rs:178-190`), the full purchase graduating the market in the creation block. Creator cost at minimum supply: curve cash (1 μNOID for the 299-unit flat prefix; 400 NOID = 4e8 μNOID for the whole curve) plus 100 NOID minimum token backing (`noid_tx/src/asset_backing.rs`, 1 000 μNOID per whole token).

Why the head is not snipeable value: the flat prefix is a *ceiling overcharge*, not a discount — the first NOID buys 299 units whose exact integral value is 0.996 7 μNOID (PoC); the second NOID buys 300 units. There is no mechanical free lunch at `sold = 0`; being first is only an informational position (earliest entry in a rising market).

Who can be first if the creator declines the bundle: trades require the launch to exist in an ancestor block (`noid_chain/src/storage/mdbx_store.rs:248-257` resolves the market parent from the durable tip snapshot), so the launch-creation block buys one block of exclusivity and nothing more. Bots can pre-position: a trade intent references only the asset id, which rides inside the creation intent's openings, so head-buys for block N+1 can sit in the mempool before the creation confirms. The first-tradable block's order among them is the same funding-priority digest lottery (PoC: two 1-NOID head buys, both fill, digest-ordered). No creator priority, no launch-window reservation, no minimum-launch-funding gate on trading exists.

## Fees and cross-market priority

Fee levels are not a priority dimension anywhere in the trade path: `fee_limit` is a ceiling, the charge is the first fitting state-derived minimum (`execution.rs:288-322`), the mempool only floors it (`noid_mempool/src/trade.rs:179-186`), and the assembler never reads fees for ordering. PoC: a racer offering 1 000× the fee ceiling has an identical digest and an identical outcome. The only purchasable input to ordering is UTXO inventory:

- The digest is `H(parent_id, asset, creation_id, slot_index)`; `creation_id` is chain-assigned (tagged sequence bounded by the monotone allocator, `noid_chain/src/consensus/params.rs:401-418`; `noid_tx/src/native_assets.rs:259-269`) and slots come from the state's allocator, so coordinates cannot be chosen freely — only *held*.
- Each admitted intent's anchor is its largest principal claim (`trade.rs:236-253`, PoC-verified incl. tie-break); a single-claim intent makes the anchor a free choice among one's UTXOs, and the mempool admits one intent per distinct anchor (slot-conflict rule), so the mempool-equivalent strategy is to pre-submit one intent per UTXO and let the miner's digest sort find the best — a standing, zero-cost-per-block lottery ticket set.
- Cost curve: one paged transaction mints up to 256 UTXOs (128 pages × 2 outputs, `noid_tx/src/paged_spend.rs:42-44`, `noid_tx/src/types.rs:14-15`); each ticket needs ≥ gross + fee_limit (dust-scale). Empirical win rates match n/(n+m) (table in `results.txt`); equal inventories re-flip the winner across parents at 51 % (the lottery redraws every block via `parent_id`).
- Because the global digest ranking also orders *markets* for assembly (`trades.rs:424-442`), the same lottery is the cross-market block-space priority. An attacker cannot buy a better digest with fees, size, or grinding key material — only with UTXO count and mempool occupancy (each intent costs one GKR wallet-authorization verification at admission, `noid_mempool/src/trade.rs:41-66`).

## Builder discretion

If the games collapse, the collapse point is the builder: consensus validates any proposer-chosen subset, so *by the pinned rules* the default winner of every race — tail sweep, head, opening liquidity — is whoever assembles the block (and the fee flow supports this posture: `fee − burned` is coinbase-claimable, `noid_chain/src/consensus/fees.rs:12-13,41-51`, but with no tipping mechanism the builder's leverage is inclusion/exclusion, not price). Against nodes running the shipped assembler, the same races devolve to the UTXO-count lottery above, which is Sybil-weighted but fee-flat, re-randomized per block, and decides prizes that are worth less than their own fees. There is no ordering dimension an attacker can *purchase* on the pinned rules.

## Negative and control results

- Authorization-key order ≠ execution order: racers with key order reversed relative to digest order still execute in digest order (and the batch encodes them key-sorted).
- Fee inflation (1 000×) changes neither the digest, nor the race outcome, nor the charge category.
- Two intents sharing an anchor UTXO cannot enter one batch (funding-slot duplicate); the mempool admits neither pair twice.
- The dropped racer is uncharged: charges map only over fills; a winner-only batch checks clean after the miner's stable-policies retry.
- A large-gross loser fills against the opening pool in the same batch (no whole-batch rejection of mixed outcomes).
- The flat-tail sweep is free at *any* depth inside the window and costs real cash (1 μNOID) one unit below it.
- Fair-priced pre-graduation buy → opening-pool sell loses (−12 μNOID on a 2 000 μNOID gross); pool opens exactly at terminal marginal price (exact-fraction equality asserted).
- The free tail cannot be sold standalone (dust floor rejects; min lot 102 units); its value rides only with a larger bundle.
- Launch head: exact prefix value 0.996 7 μNOID for 1 μNOID paid — overcharge, not discount; second buyer gets 300 units for the same NOID.
- Closure identity at min/mid/max supply: `900·S·C(cap) == 3·S·cap + 5·cap²` — the curve's total ceiling overcharge at the cap is zero; the free window *is* the whole surplus.

## PoC implementation and reproduction

`src/main.rs` (single binary) drives only public production APIs of the pinned crates: `LaunchDefinition::from_genesis`/`after_initial_purchase`, `quote_curve_exact_input`, `quote_exact_input`, `TradeIntent`/`funding_priority`/`authorization_id`, `TradeBatch::new`/`checked`, `SequencingPlan::execute`, and `noid_chain::consensus::fee_breakdown`. Synthetic actors are `Address([seed;32])` wallets with single-UTXO funding. No mempool, node, or network is instantiated; the miner's `stable_policies` retry is reproduced from `noid_miner/src/template/trades.rs:261-286` at library level.

From this directory:

```bash
rustfmt --check src/main.rs
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Expected output: five `... complete` lines, a JSON report (units: all NOID amounts are base units, 1 NOID = 1e6), then `ALL ASSERTIONS PASSED`; exit 0. Any assertion failure panics with a section-prefixed error. `Cargo.toml` pins the crates by path into `../dex-economics/pinned/` (read-only reuse of that lane's verified mirror).

## Assurance limits

- Library-level only: the mempool, miner, and block-assembly behaviors are established by reading the pinned `noid_miner`/`noid_mempool` code paths cited above and reproducing their deterministic core (`stable_policies`, digest ranking, sequencing) against the real `noid_tx` implementations; no node or miner binary was executed, and no multi-node race, fork, or reorg was simulated.
- The digest-lottery win-rate table models digests as uniform (4 096 trials per cell, real `funding_priority` hashes over random UTXO coordinates); n/(n+m) is the standard order-statistic result for minima of uniform draws, not a theorem about Poseidon2b.
- Latency games (observing a parent, re-anchoring, submitting before the template snapshot) are characterized from the code (intents are parent-agnostic; templates snapshot under a parent-bound lock, `noid_mempool/src/mining.rs:66-95`), not measured.
- Prize quantification covers the three canonical supply shapes (minimum, mid, maximum admissible); the 100-unit window and 1 μNOID value are supply-independent by the price algebra (window = 1/(marginal price at cap) = 100 units) but were spot-asserted, not proven exhaustively.
- The consensus-side claim ("the builder owns every race") is a property of the pinned validation path's checks (absence of any membership/ordering fairness rule); it was verified by reading `noid_chain/src/block/native_assets.rs:718-747` and `consensus/ordering.rs`, not by building an adversarial block.
- No fee-curve sampling beyond the quietest geometry (the 9 800 μNOID floor is the minimum; occupancy pressure only raises it). Prior-lane fixtures observed 6 600 μNOID quotes at their state geometry.
- No upstream source was edited and no fix was applied. `pinned/` was used read-only through path dependencies; the one pre-existing working-tree modification in the shared checkout (`noid_tx/src/native_assets.rs`, a `pub mod trade`/`pub mod transition` line swap, semantically null) predates this lane and was left untouched, as the dex-economics lane also recorded.
