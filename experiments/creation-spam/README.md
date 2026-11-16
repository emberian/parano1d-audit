# Native-object creation costs and persistent state

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`. The PoC in `poc/` uses the pinned production transition, fee, block-template, commit, and MDBX code against synthetic state. It starts no live node, peer, wallet, or RPC server. All generated files stay under this directory.

The question is how much lasting state an account can create per fee paid, and which of those records can be imposed on someone else. The separate `token-accounting`, `nft-marketplace`, `tx-admission`, `rpc-api`, and `p2p-resources` reports cover the adjacent mechanisms.

## Creation-cost inventory

Fees (`noid_chain/src/consensus/fees.rs:97-122`, constants `consensus/params.rs:427-446`): `required = 5,000 + 100·live_inputs + 700·live_outputs + 2,500·mult·max(0, outputs − inputs)`, the state-growth term **burned** by consensus. `live` is bitmap-driven (`noid_tx/src/types.rs:156-163`), so native objects at amount 0 are priced as outputs. Enforcement: block admission `consensus/validation.rs:248-287` (`BelowMinFee`), mempool admission `noid_mempool/src/pool.rs:1257-1268` (floor = `max(MIN_FEE_BASE, 0.9·median)`) and selection `pool.rs:790`. Zero-fee codes are maintenance ops only (`noid_tx/src/native_assets/financial.rs:14`).

| Object class (live op) | Attacker cost, minimum | Of which burned | Network perpetual carriage per live object | Creation limits found |
|---|---|---|---|---|
| NFT collection (op 5) | **5,800 µNOID** fee: exact-fee funding input, 1-in/1-out (`transition.rs:269-286`; measured). Each creation consumes one funding UTXO, so sustained creation also requires an input supply. | **0** for this transaction (`max(0, 1−1)`) | 1 state slot, 50 B sparse (`serial.rs:270`, measured 49.8 B/slot) + MDBX DATA 32+167 B + `native_owners` 64 B + `native_assets` 42+32 B + two owner-index rows 36+16 and 36+16 B ≈ **441 B MDBX logical bytes**, or ~491 B including the sparse entry (layouts: `native_payloads.rs:39,166-179,198-203`; `mdbx_store.rs:1671-1700`); cold-sync ~192 B (`native_segment.rs:178-187`: 2-B length + 140-B opening per record rides the snapshot segment) | `maximum_items` up to `u64::MAX` free (`native_assets.rs:467-469`); no per-account, per-block-per-account, or global collection-count limit found; **`active_slot_count` is unchanged** by this 1-in/1-out transaction |
| NFT item (op 6, batch mint) | **3,227 µNOID/item** incl. amortized base (measured: 800,200 per 248-item tx; 700 output + 2,500 growth per net-new slot) | 2,500/item at mult 1 | same ~441 B MDBX + 50 B slot + 192 B sync + 307 B block-relay wire per item (measured 76,218 B per 248-item intent) | 248 items/tx (128-page intent cap: header + 2 per-owner input pages + 125 output pages, `wire.rs:398,413`, `paged_spend.rs:42-44`); 510 live outputs/block (`params.rs:161`), so ~496 items/block in 2 txs |
| Token launch (op 48) | ≥4 opening outputs (definition, reserve, state/pool, balances) → same ~3,200/object + **locked backing = supply/1000 µNOID** (`launch/owner.rs:93-97`, measured `minimum_backing`: supply 1,000 → 1 µNOID, supply 10⁹ → 1,000 µNOID) | 2,500/object net-new | 4–5 native records + reserve slot; backing parked in `TokenReserve`, redeemable by burning tokens | exact-minimum backing enforced (under/over-funded rejected, token-accounting lane); supply bounds `launch.rs:38` |
| Listing (op 64) | ~2 outputs + escrow `NFT_FEE_CREDIT` = 100,000 µNOID (`nft_market.rs:25`, recoverable minus fee) | 2,500×net-new | definition + listing escrow records | round/offer families gated off at this commit (nft-marketplace lane) |
| Plain UTXO dust | 9,000 µNOID for 1-in/2-out | 2,500 | ~102 B node-local (50 B segment entry + 52 B owner index) — the baseline a native object multiplies ~4.8× | same output caps |
| Burn (op 8, cleanup) | 5,900 single; ~123/item batched (254-in) — **paid by the item owner** | 0 (state-shrinking) | removes carriage | — |

Rate and size limits found: 256 transactions, 255 user pages, 510 user outputs, 1,531 actions, and 256 distinct segments per block (`params.rs:146-171`, `validation.rs:217-226`); block bytes ≤82.6 KB plus 3,062 openings payload (`wire_limits.rs:27`, `block/payload.rs:19-21`); target block interval 30 s (v2). The reviewed paths showed no per-account creation rate, rent, or expiry. Native metadata is fixed-size digests, so this is object-count growth rather than arbitrary-byte storage. The highest sequence value is `NATIVE_SEQUENCE_MAX` = 2^62−1 (`native_assets.rs:26`). Absence claims are limited to the reviewed code.

The real `fee_breakdown` gives multipliers 1 below 50% occupancy, 2 at 50%, 4 at 75%, and 8 at 90%. Expansion adds one capacity bit when at least 10 of 18 finalized headers are at least 75% occupied (`params.rs:274-311`, `slot_expansion.rs:34-63`); occupancy then falls to 37.5%, restoring multiplier 1. Capacity can grow to 2^32 slots. Thus the fee responds to occupancy, and expansion can reset that response; it does not continuously rise with total state size. Sustained multiplier and expansion behavior under an adversarial chain were modeled, not measured here.

## Findings

### F1. A collection can replace a funding slot without paying the state-growth burn

**Availability risk: low to medium.** The account needs spendable funding inputs and must pay the transaction fee. The result is persistent storage growth rather than a consensus or balance violation.

A `CreateCollection` funded by an input whose amount exactly equals the fee is 1-in/1-out: `required = 5,800 µNOID`, **burned = 0**, and `active_slot_count` is unchanged (measured control). The new collection persists until its owner spends it. Its logical carriage is ~441 B in MDBX rows plus ~50 B in sparse state, compared with ~52 B in the ordinary owner index plus ~50 B sparse state for the consumed UTXO: an increase of ~389 B per replacement, excluding database page overhead. Its opening adds ~192 B to a future cold-sync segment by encoding layout. The burn prices net slot count, not this change in record type.

This does **not** make unbounded collection creation free. Each transaction consumes a distinct funding UTXO and pays 5,800 µNOID. An existing inventory can be converted without an additional state-growth burn, but replenishing one ordinary UTXO per new collection may itself incur the 2,500 µNOID net-slot burn at multiplier 1, unless the input comes from already available outputs such as coinbase. A miner who includes its own creation may receive the fee in the coinbase, but still needs this input inventory. The PoC did not model a long-running funding/replenishment strategy.

### F2. NFT items can be assigned to an address without its consent

**Availability risk: targeted wallet pollution.** The minter pays the creation fee. No value is extracted from the recipient.

`mint_nft` (`noid_tx/src/native_assets/transition.rs:411-447`) checks collection identity and serial continuity; authorization covers inputs, not the new item owners (`transition.rs:165-176`). A locally mined and committed block containing 248 items owned by an address that did not sign was accepted. `native_owner_roots(VICTIM)` returned all 248 in a 1.8–3.2 ms page scan, and the asset index resolved them. Without a timelock, the recipient can burn the unwanted items but must sign and pay the fee (5,900 µNOID singly, ~123/item batched).

The later [economic-state reproduction](../economic-state/REPORT.md) strengthens this case: the minter can set each unsolicited item's `unlock_height` to `u64::MAX`. The recipient then cannot transfer, list, or burn the items before that height; a real block with 248 such items was accepted and all 248 victim inputs failed with `Timelock`. This does not lock the recipient's ordinary NOID UTXOs. A custom wallet can filter the display; the stock GUI's performance at large item counts was not measured. Collection catalog pages are prefix-bounded (≤256/page, B-tree seek), not an unbounded rescan.

### F3. Fees, rather than validation CPU, bound high-volume item creation

**Risk depends on the cost of funding and sustained inclusion.** The values below combine local per-item measurements with a hypothetical full chain; they are not a measured attack rate.

Measured per item (248-item batches): construction **~106 µs**, fee **3,227 µNOID** (2,500 burned), and sequential apply **~59 µs/item**, with 119 ms for a 248-item block template build and materialization. This particular item shape costs the attacker more construction CPU per item than the measured apply cost; the fee is the principal limiter in this experiment.

At the *modeled* maximum of 496 items per block and 2,880 blocks per day, the arithmetic gives **1.43 million items/day**, costing **4,609 NOID/day**, adding ~71 MB/day to the sparse consensus representation and ~700 MB/day in logical node-local records across every full node. That assumes continuous inclusion at the low-occupancy fee, adequate inputs and funds, and no competition for block space. One year at that rate would add ~26 GB sparse state and ~256 GB logical node-local records for ~1.68 million NOID. MDBX file growth, compaction, and a live network's sustained throughput were not measured. Expansion can restore the low-occupancy multiplier, but cannot be assumed to keep the fee at that level continuously.

### F4. No creation-side extraction route found

Collection and asset IDs are derived from a genesis anchor (`native_assets.rs:298-316`), with no on-chain human-readable name to squat. Metadata is a digest, not a committed display name. Launch backing remains in `TokenReserve` and can be redeemed by burning tokens. Round and offer refund operations are profile-gated off at this commit. This review found no creation-side path to take another holder's value; it does not prove the absence of one elsewhere.

## Composition with prior availability findings

- **Victim inventory (measured at 248 items):** F2 adds attacker-chosen rows to `native_owner_roots` for a chosen address. The measured page took 1.8–3.2 ms; each row re-reads and validates its DATA record (`native_payloads.rs:699-739`). A larger dataset was not timed.
- **Cold sync (code and encoding layout):** every live native object adds ~192 B of opening data to future snapshot segments (`native_segment.rs:212-243`). Snapshot staging and owner-index rebuild include full-table passes (`mdbx_store.rs:3306,4257,4841`). The resulting scale effect is inferred from the code; this lane did not time a large cold sync.
- **Other availability paths:** creation transactions are valid and fee-paying; this experiment found no added multiplier for the C17 mempool byte budget or C22 invalid-proof verification path. Batch minting produces one receipt per 248 items, so C19 receipt rescans grow with transactions rather than individual items.
- **Raw owner index never sees native beneficiaries (measured):** `get_verified_owner_page(VICTIM)` and `get_verified_utxos_by_owner(VICTIM)` return 0 — raw slots are owned by opening roots; only the native indexes map beneficiaries. The ledger-fairness unbounded-owner-query shape therefore does not directly attach to native pollution.

## Negative and control results

- Fee shapes are exactly the formula for all five canonical tx shapes (printout above); collection minimum 5,800 with zero burn accepted by the real `apply_native_transition`; `active_slot_count` unchanged by it (asserted).
- Conservation held in every applied transition (the real checker rejects otherwise — prior lanes' controls carry over; no re-derivation).
- 24 real empty blocks + a real collection block + a real 248-item mint block accepted through `build_block_template`, `apply_next_block`, and `commit_locally_proved_next_block` with PoW, epoch anchors, and state-root binding intact; attacker funded by real coinbase outputs mined to itself (45M µNOID at genesis occupancy).
- `encoded_state_bytes` grew exactly 49.8 B/slot (matches the 50-B sparse-entry layout); MDBX logical writes 441 B/object (row existence proven by index queries; file size is page-granular and absorbed by MDBX preallocation at this scale — stated as measured, not inflated).
- Launch minimum backing measured: 1 µNOID at supply 1,000 (s/1000 rule); launches are not cheaper per object than items.
- No arbitrary-data channel: metadata/identity fields are fixed-size digests throughout (`MAX_NATIVE_OPENING_BYTES` bounds every record; `to_bytes` enforces exact length, `native_assets.rs:872-907`).
- Catalog and owner pages are prefix-seek + bounded (≤256), not full-store rescans (read, `native_payloads.rs:699-813`).
- No rent/expiry/decay mechanism found in `noid_tx`/`noid_chain` native-asset or state code (searched; `unlock_height` is a timelock).

## PoC implementation and reproduction

`poc/` is a binary crate (`creation-spam-audit`) depending on `noid_chain` (with the `isolated-v2-1-fork-testnet` feature so native ops activate at height 24 in a short local chain), `noid_tx`, `noid_core`, `noid_poseidon2b` by path. Phase 1 drives fee shapes and minimum-cost creations through `apply_native_transition`; Phase 2 measures construction (`NativeIntent::from_claims` + `to_bytes`), apply, wire, and state carriage over 6×248 items on a real `ChainState` with real allocator hints; Phase 3 commits real blocks (template → PoW → seal → MDBX commit) and measures store growth and victim-discovery queries.

From this directory:

```bash
cd poc
cargo fmt --check
CARGO_TARGET_DIR="$PWD/../target" cargo run --release 2>&1 | tee ../results.txt
```

Final output lines (see `results.txt`, 47 lines, exit 0):

```text
PER_ITEM construction=105.8us apply=59.4us fee=3226.6uNOID (burned 2500.0) wire=307.3B
SATURATION items_per_block~496 (BLOCK_MAX_USER_OUTPUTS=510, 30s blocks): items/day=1428480 attacker_cost=4609 NOID/day
SATURATION growth/day: consensus-state ~71.4 MB + node-local MDBX ~701.4 MB on EVERY full node; ~3571 NOID/day burned
RATIO per item: attacker ~106us construction + ~3227uNOID (2500 burned) buys ~491B permanent MDBX + 50B consensus state + ~192B cold-sync payload
MEASURE mint block (248 victim-owned items): intent_construction=17202us template_build+materialize=119233us
MEASURE encoded_state_bytes (T_SEGMENTS sparse) 1309 -> 13759 (+12450: 49.8 B/slot)
MEASURE victim native_owner_roots page: 248 roots in 1804us (owner index proves forced carriage)
ALL PHASES COMPLETED
```

## Assurance limits

- The ~441 B MDBX figure sums encoded row layouts (DATA 32-key+27-prefix+140-opening, OWNER 64-B key, ASSET 42+32, two owner-index rows 36+16 each). The ~50 B sparse segment entry is separate. B-tree and page overhead were not measured; MDBX file size is preallocation-dominated at 250-object scale. Row existence and per-row reads were exercised.
- The node-side cost excludes recursive HistoryStep proving (the seal path substitutes an auditor terminal after full template-side native validation and exact state-root binding — the same bridge the production miner uses). Per-item apply cost is the sequential-materialization path; the recursive prover's marginal cost per action was not measured.
- Cold-sync composition is code-cited (snapshot segment record format; `clear_live_refs`/`replace_refs` full-table passes), not executed against a live snapshot staging run; the 192 B/object figure comes from the real encoder's layout, not a network transfer.
- Victim-discovery timings are one page on a 248-row store; extrapolation to millions of rows is linear-by-code-structure (cursor walk with per-row record read + validate), not measured at that scale.
- All timings are from one laptop (Apple silicon, release build, sequential batches); fee/burn/state-byte numbers are exact and machine-independent; µs numbers are indicative only.
- No live P2P, mempool admission under load, or multi-node sync was run; composition claims involving C17/C19/C22 rest on those lanes' measurements plus this lane's code citations.
- This lane did not edit the target source or implement a fix.
