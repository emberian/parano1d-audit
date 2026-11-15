# Sequencing and ordering audit: the stack-wide map

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. Every experiment calls production code from the pinned crates (`noid_chain`, `noid_tx`): the mempool fee index, the real block-template builder, real state application, the trade `funding_priority` lottery, the NFT `payer_priority` lottery, `ListingBatch`/`ListingSettlement`, `SequencingPlan`, and the deterministic fee model. No node, peer, miner, worker, or RPC server was started.

## 1. The ordering map

| # | Layer | Rule (pinned code) | Who influences it | How |
|---|---|---|---|---|
| 1 | P2P delivery | Payload-key dedup only; first forwarding source kept, no priority, no queue (`noid_p2p/src/tx_delivery.rs:114-124`); mempool-sync serves entries in `HashMap` order (`noid_chain/src/mempool.rs:907-938`) | any peer | arrival timing; order itself is never consensus-relevant — only which tx each node *admits* first |
| 2 | Mempool admission | First-come per node: cheap pre-filter, proof verification outside the lock, final admission under the lock (`noid_mempool/src/pool.rs:380-513`); slot conflicts vs already-admitted set are rejected — there is no replacement path anywhere (`noid_chain/src/mempool.rs:623-674`; absence asserted by `build.rs`) | anyone who submits; node operator (config, fee floor) | win the per-node admission race (verification-throughput order, not arrival order: the lock is released during proof verification, so two conflicting spends can verify concurrently and the first to finish admits); a local dynamic fee floor gates admission only |
| 3 | Template selection (ordinary + native entries) | Descending `fee_rate = fee / (in+out+4·net_new)` with ascending-txid tie-break (`noid_chain/src/mempool.rs:35-55, 730-836`); v2 small/large proof class chosen by claimable revenue (`noid_mempool/src/mining.rs:166-199`) | fee payers; **financial natives cannot pay** — their fee is pinned to the exact parent-derived minimum (`noid_mempool/src/native.rs:162-166`, `noid_chain/src/block/native_assets/financial.rs:159-161` `context.fee != fee`), so equal-shape natives tie-break on txid | fees (ordinary), or free offline txid grinding (natives) |
| 4 | Template selection (trade entries) | Skipped by the fee index; ranked by `funding_priority = H(TRORD_21, parent_id, asset, anchor.creation_id, anchor.slot_index)` ascending, grouped per asset, greedy within financial budgets (`noid_miner/src/template/trades.rs:424-542`) | holders of UTXOs: each owned UTXO is one lottery draw per parent block | pick which pre-positioned UTXO anchors the intent after the parent lands (all inputs public); submit only the best draw |
| 5 | In-block order | Builder discretion: consensus validates but never orders — no fee-order or maximality rule in `validate_block_consensus` (`noid_chain/src/consensus/validation.rs:574-653`; absence asserted by `build.rs`); for trade batches, consensus re-derives exactly the batch the proposer submitted via `prepare_trade_batch_from_claims`, checking only fee/commitment/auxiliary consistency (`noid_chain/src/block/native_assets.rs:718-747`) — the builder's batch choice, not any protocol rule, picks the race winner. Default builder: contract groups first (v2), then descending absolute fee, ascending txid (`noid_chain/src/consensus/template.rs:801-816`); coinbase/payout first, then users (`:1001-1010`); creation ids are assigned in application order, so in-block position determines every output's future priority entropy (`alloc_counter`, `noid_chain/src/state.rs:38-54`) | the block builder (node operator; external miners only return nonces) | include/omit/reorder freely; choose the registration height for round-bound logic |
| 6 | Execution inside native batches | Trade batches: canonical encoding sorted by `authorization_id` (`trade/batch.rs:129-137`, fully grindable but order-neutral), execution re-sorted by the same `funding_priority` with a price-directed side preference (`trade/sequencing.rs:136-163`); NFT listing winner = `min(payer_priority, request root, funded key)` with `payer_priority = H(NMPRIOR1, round, payer)` (`nft_market/settlement.rs:75-83`, `nft_market.rs:361-366`); launch/market requests keyed by `funded_request_key = H(MKREQID1, request_root, slot_index, creation_id)` (`market/queue.rs:44-50`, sorts at `market/queue.rs:70-77` and `launch/queue.rs:70-77`) | traders/NFT buyers via their anchor UTXOs and payer addresses | one draw per pre-positioned UTXO (trades) or per pre-funded payer address (NFT); the round binds the registration *height* (`launch.rs:294-313`), so a grind covers exactly one height |
| 7 | Cross-block | Heaviest cumulative work, lexicographic tip-hash tie-break, finality depth 18 (`noid_chain/src/consensus/fork_choice.rs:43-63, 80-82`); same-anchor replay (finding C6) means one tx can confirm on two same-anchor forks | PoW holders | the work race, not any admission order, decides which fork's spends survive |
| 8 | Time | Block timestamps miner-chosen within (MTP, now+120s] (`consensus/timestamps.rs:26-58`), but **no contract logic reads timestamps** — every gate (trade validity, unlocks, listing expiry, settlement/refund windows, round phases) is height-based | the builder, via inclusion heights | shift a registration to a different height and its round id changes; withhold to push requests toward expiry windows |

Who owns each canonical race by default:

- **Double spend of one UTXO**: the PoW winner's mempool state. Per-node first-come with no replacement (confirmed by ledger-fairness §3/§4); nothing binds a miner to any inclusion policy; finality depth 18 bounds cross-fork acceptance. A same-slot conflict cannot coexist in one block, so ordering decides nothing here — *inclusion* does.
- **Two trades claiming the same price move**: the larger UTXO inventory, by count, at ~9,000 μNOID per persistent ticket (Section 2, SEQ-1).
- **Two claims on one fixed-price NFT**: the larger pre-funded address inventory (SEQ-1); the builder's height choice multiplies grind coverage (D2).
- **Congested financial-native inclusion**: whoever grinds the smallest txid — free, offline (SEQ-2).
- **Inclusion within a block interval**: whoever reached the winning builder's mempool before its template build; mid-interval arrivals cannot jump at any fee (SEQ-3).

## 2. Findings

### SEQ-1 — Priority across the native trading stack is a UTXO-count lottery: dust-weighted, one-time cost, capacity-capturing

**Severity: Medium (conditional on v2.1 native activation, mainnet height 279,370). Preconditions: pre-positioned UTXOs/address funding, and arrival at the block-winning builder before its template build (BLOCK_TIME = 20 s).**

Trade execution order and NFT fixed-price winner selection both reduce to `min` over i.i.d. hash draws whose inputs are entirely public: `H(parent_id, asset, creation_id, slot_index)` for trades (`noid_tx/src/native_assets/trade.rs:231-264`), `H(round, payer)` for NFT listings (`noid_market.rs:361-366`). An attacker who holds K UTXOs (NFT: K funded addresses) computes all K priorities offline the moment the parent (or listing round) is known and submits only the best draw; against M single-draw competitors they win the first position with probability K/(K+M). The PoC measured, with the production functions: 0.672 vs 0.667 at K=8, M=4 over 2,000 random parents (trades), and 0.733 vs 0.750 at K=12, M=4 over 400 rounds (NFT).

Three properties make this a market rather than a lottery tax:

1. **Count-weighted, not value-weighted** — the priority digest hashes `(creation_id, slot_index)` only, never the amount. The PoC asserts the minimum viable anchor (amount = gross+fee) produces the identical priority as a 10^12-μNOID whale at the same slot.
2. **One-time, persistent cost** — a ticket is a dust UTXO: the real `fee_breakdown(1→2 outputs, 0 occupancy)` gives base 5,000 + io 1,500 + state-growth 2,500 = **9,000 μNOID**, paid once, and the UTXO keeps drawing every future block until spent. Marginal cost per block: zero.
3. **Capacity capture** — when demand exceeds the per-market batch cap (MAX_SEQUENTIAL_TRADES = 64; MAX_ROUND_REQUESTS = 16 per listing round) the builder admits in priority order (`trades.rs:424-442`), so K ≥ cap plus small digests excludes victims entirely for that block. The PoC asserts the exact 64/65 boundary (`SequencingError::Bounds`) and that one UTXO cannot buy two draws in a batch (`DuplicateFunding`).

Countermeasure asymmetry: a victim can only respond by holding their own inventory — the protocol offers no fee- or value-based path to priority inside these batches (contrast ordinary spends, layer 3).

### SEQ-2 — Financial natives have no fee lever: pinned fees make their ordering a free offline txid grind

**Severity: Medium for native-asset liveness/fairness under congestion (v2.1-conditional). Preconditions: page/financial budget saturation, i.e. more pending natives than the producer's class budget admits.**

Financial native intents must pay exactly the parent-derived minimum — `facts.fee != minimum` is rejected at mempool admission (`noid_mempool/src/native.rs:162-166`) and `context.fee != fee` at consensus (`noid_chain/src/block/native_assets/financial.rs`); both lines are asserted verbatim in the pinned sources by `build.rs`. Consequently: (a) any ordinary spend paying 1 μNOID more displaces every financial native from a saturated template, and no native can respond; (b) among equal-shape financial natives the mempool `FeeKey` and the template comparator both reduce to ascending `logical_txid` — and a txid is a body hash the attacker constructs. The PoC shows a one-slot truncation between two pinned-fee entries keeps the smaller txid, and that beating a specific victim's txid costs a **median of 2 body hashes** (mean 7.5, 200 races) by varying one output-owner field. Grinding is free, offline, and unlimited — unlike SEQ-1 there is no capital ticket at all.

### SEQ-3 — Inclusion freezes at template build: arrival timing beats fee within a block interval

**Severity: Low-Medium (fairness/liveness policy of the default miner; not a consensus violation).**

The miner's event loop refreshes a template on `TxAdmitted` or heartbeat **only while `user_pages == 0`** (`noid_miner/src/miner.rs:857-875`; both guards asserted by `build.rs`). Once a template contains any user page, later admissions — any fee, any priority — wait for the next block; only a new tip rebuilds. Two consequences: (i) fee priority is decided at build time, so the practical race for block N+1 is "be in the mempool before the winning builder processes tip N," a latency race; (ii) combined with C15 (invalid PoW consumes the only prepared template), a mining-credential holder can repeatedly burn prepared templates, forcing rebuilds whose inclusion set re-freezes at times the attacker influences — a churn lever adjacent to censorship, at the already-reported cost profile.

### SEQ-4 — Builder discretion is total and reaches future priority entropy (structural, informational)

`validate_block_consensus` checks resources, coinbase value, fee floors, slot conflicts, and state/tx roots — never order or inclusion maximality (absence asserted by `build.rs`). The default sort (contract-first, desc fee, asc txid) is a node policy, not a protocol rule. Additionally, creation ids are `alloc_counter+1` assigned in application order: the builder chooses which colliding outputs receive which creation ids, i.e. directly assigns the future `funding_priority` entropy (SEQ-1) of every output in its blocks — there is no directional bias available (the digest is a hash), but the assignment is discretionary, and the PoC shows reversed application order produces a different, equally constructible state root. Timestamp discretion exists within (MTP, now+120 s] but nothing in the native-asset logic reads timestamps (grep: zero reads); the real time-discretion surface is the inclusion *height*: round ids bind height (`launch.rs:294-313`), so a builder shifts which pre-ground purchase requests are even eligible (PoC: a request bound to round(h) is rejected at h+1), and can push registrations toward expiry windows.

### SEQ-5 — Same-anchor replay × ordering (integration with finding C6; scoped, no new PoC)

C6 established one tx + authorization can be valid on two same-anchor forks. Ordering interaction: within a fork, order is committed (tx_root/state_root) but unconstrained; across same-anchor forks the same spend can confirm on both, and which spend of a conflicted UTXO survives is purely the work race (layer 7), decided up to 18 blocks late. Downstream consumer indexes that treat <finality confirmations as settled observe whichever fork their node currently prefers — a per-fork, non-monotonic history until finality. No additional mechanism was found that lets admission order (layer 2) leak into fork resolution.

## 3. Negative and control results

- Mempool selection is descending fee_rate, ascending txid: verified against the real `Mempool` (fee-distinct and fee-tied fixtures).
- The real template builder orders 40,000 before 9,000 and reproduces its own state root under sequential application; reversed user order yields a *different* state root — creation ids are position-derived (order is consensus-tracked, not consensus-constrained; `compute_tx_root` agrees with the template's root on the original order).
- No replacement policy exists: the pinned `noid_chain/src/mempool.rs` contains no replace path (asserted in `build.rs`); conflicting admissions return `InputConflict`/`OutputConflict`. Cross-references ledger-fairness §3 (first-seen output pinning) rather than duplicating it.
- The grindable `authorization_id` batch sort is encoding-canonical only: feeding the same policies in reversed encoding order still executes in `funding_priority` digest order (PoC, this lane; independently PoC'd by the dex-games lane). The "grind the batch key" attack has no target.
- Fees buy nothing inside the trade path or the native financial path: `fee_limit` is a ceiling (`noid_tx/src/native_assets/trade.rs:282-284`), the charge is the first fitting state-derived quotation — i.e. the minimum (`trade/execution.rs:294-321`), financial natives are pinned to the exact parent-derived minimum (SEQ-2), and no tip mechanism exists anywhere in these paths. Ordinary PagedSpend groups are the one exception: their fee field is free and buys template position (layer 3) — a distinction the stack-wide map keeps explicit.
- Consensus re-derives whatever trade batch the proposer submits (`noid_chain/src/block/native_assets.rs:718-747`), so batch membership and its priority order are settled by the builder before consensus sees them; validation is a re-derivation check, not a selection rule. The one explicit doc line for this stance: coinbase slot avoidance is "a local preference, not a consensus reservation" (`noid_chain/src/consensus/template.rs:501`).
- One UTXO cannot buy two draws in one batch (`DuplicateFunding`); the 64-intent batch is accepted and the 65th rejected (`Bounds`), exactly at `MAX_SEQUENTIAL_TRADES`.
- Trade `funding_priority` binds the real parent (`TRORD_21` over `parent_id`), so no offline pre-parent grinding of the *target* block is possible — the lottery is per-parent; tickets drawn before the parent are re-usable only as inventory.
- NFT rounds are height-bound: `round_id(h) != round_id(h+1)` and a request bound to round(h) fails at height h+1 — a payer grind covers exactly one candidate registration height.
- No timestamp reads exist in `noid_tx/src/native_assets` — every time gate is height-based, so timestamp discretion (MTP..+120 s) has no contract-level ordering effect.
- Financial fee pinning holds on both sides of admission (mempool equality and consensus equality lines asserted verbatim in the pinned sources).
- The miner's two `user_pages == 0` refresh guards exist verbatim at the pin (asserted in `build.rs`); no runtime miner was driven.

## 4. PoC implementation and reproduction

`build.rs` first verifies the checkout is at the pin and asserts every ordering fact used above exists verbatim in `git show` output (fee keys, template comparator, absence of ordering rules in `validate_block_consensus`, miner refresh guards, financial fee pinning, `TRORD_21`/`NMPRIOR1` domains and their bound inputs, batch/execution sorts). `src/main.rs` then runs the controls, the two lottery measurements, the capacity/duplicate boundary checks, the NFT ground-payer race, the height-binding check, and the txid grind measurement, all against production code paths.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final output (25 assertions, all passing; full run in `results.txt`):

```text
attacker tickets=8 victims=4 trials=2000: win rate 0.672 (K/(K+M)=0.667)
ok: priority win rate matches the K/(K+M) exchangeable-lottery model
ok: priority is count-weighted, not value-weighted (dust = whale)
ok: 64 intents fill the batch cap exactly (MAX_SEQUENTIAL_TRADES)
ok: 65 intents exceed the cap — capacity capture truncates by priority
ok: one UTXO cannot buy two draws in one batch (duplicate funding rejected)
ok: fills execute in ascending funding-priority order (all-Buy batch)
ok: reversed authorization-key encoding still executes in funding-priority order
ticket price (1->2 outputs at 0 occupancy): base=5000 io=1500 state_growth=2500 total=9000 micronoid
attacker draws=12 (submits 1) victims=4: win rate 0.733 vs K/(K+M)=0.750
ok: ground payer wins at the predicted offline-grind rate
ok: round id changes with the registration height
ok: the same request is rejected at h+1 (height-specific grind)
txid grind vs a fixed victim: median 2 body hashes (mean 7.51) over 200 races
ALL ASSERTIONS PASSED
```

## 5. Assurance limits

- Library-level only: no node, mempool service, P2P peer, miner runtime, or RPC server was run. The miner refresh guards (SEQ-3) and the financial fee-pin lines (SEQ-2) are verified as verbatim pinned-source facts, not by driving the runtime paths.
- The builder's greedy trade-assembly loop (`assemble` in `noid_miner/src/template/trades.rs`) was read and its ranking line pinned, but not executed; in-batch order claims rest on the production `SequencingPlan` the builder itself calls. The B25/B255 class-budget interaction was not exercised.
- B2 used real template construction and real state application, but nonce-0 headers; `validate_block_consensus`'s PoW-bearing header checks were not run here (the external-miner lane covers real-PoW validation of real templates).
- Lottery measurements are empirical (2,000 and 400 trials) within sampling tolerance of K/(K+M); i.i.d. uniformity of the priority hashes is assumed from the Poseidon2b construction and observed statistically, not proven.
- Latency realities of the "submit before the builder's template build" race (network propagation, template-build duration per class) were not measured; the map's timing claims are structural (trigger rules), not bench data.
- Value-at-stake quantification for a specific first-position fill (price impact per position) is the dex-economics lane's domain; occupancy/queue economics is the tx-admission lane's. This lane deliberately quantifies only *ordering power and its price*.
