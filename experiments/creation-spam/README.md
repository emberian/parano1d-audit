# Asset-creation spam and state-growth audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`. All created files are confined to this directory. The PoC (`poc/`) builds against the pinned checkout by path (read-only) and drives only real production functions: `apply_native_transition`, `NativeIntent::from_claims`, `MarketFeeContext`, `fee_breakdown`, `generate_slot_hints`, `build_node_owned_block_template_with_native_openings`, `MdbxChainContext::commit_locally_proved_next_block` / `apply_next_block`, and the real MDBX store. No live node, peer, wallet, or RPC server was started; loopback/synthetic state only.

Prior lanes established: creation-ID namespaces are distinct and duplicate supply is a pure double-spend (token-accounting); NFT base ops 5–8 and listings 64–66 are the live surface, round/offer families 67–75 profile-gated off (nft-marketplace); admission costs 134 µs attacker construction / 8.34 ms node verify per reject (tx-admission, C22); receipt rescan ~205–225 µs/receipt (rpc-api, C19); mempool byte-domain pin (p2p-resources, C17).

## Creation-cost inventory

Fees (`noid_chain/src/consensus/fees.rs:97-122`, constants `consensus/params.rs:427-446`): `required = 5,000 + 100·live_inputs + 700·live_outputs + 2,500·mult·max(0, outputs − inputs)`, the state-growth term **burned** by consensus. `live` is bitmap-driven (`noid_tx/src/types.rs:156-163`), so native objects at amount 0 are priced as outputs. Enforcement: block admission `consensus/validation.rs:248-287` (`BelowMinFee`), mempool admission `noid_mempool/src/pool.rs:1257-1268` (floor = `max(MIN_FEE_BASE, 0.9·median)`) and selection `pool.rs:790`. Zero-fee codes are maintenance ops only (`noid_tx/src/native_assets/financial.rs:14`).

| Object class (live op) | Attacker cost, minimum | Of which burned | Network perpetual carriage per live object | Creation limits found |
|---|---|---|---|---|
| NFT collection (op 5) | **5,800 µNOID** one-time: funding input amount == fee exactly, 1-in/1-out (`transition.rs:269-286`; measured, CONTROL) | **0** (`max(0, 1−1)`) | 1 state slot, 50 B sparse (`serial.rs:270`, measured 49.8 B/slot) + MDBX DATA 32+167 B + `native_owners` 64 B + `native_assets` 42+32 B + two owner-index rows 36+16 and 36+16 B ≈ **441 B logical** (layouts: `native_payloads.rs:39,166-179,198-203`; `mdbx_store.rs:1671-1700`); cold-sync ~192 B (`native_segment.rs:178-187`: 2-B length + 140-B opening per record rides the snapshot segment) | `maximum_items` up to `u64::MAX` free (`native_assets.rs:467-469`); no per-account, per-block-per-account, or global collection-count limit anywhere; **`active_slot_count` is unchanged** by the 1-in/1-out shape, so occupancy/pressure/expansion never see it |
| NFT item (op 6, batch mint) | **3,227 µNOID/item** incl. amortized base (measured: 800,200 per 248-item tx; 700 output + 2,500 growth per net-new slot) | 2,500/item at mult 1 | same ~441 B MDBX + 50 B slot + 192 B sync + 307 B block-relay wire per item (measured 76,218 B per 248-item intent) | 248 items/tx (128-page intent cap: header + 2 per-owner input pages + 125 output pages, `wire.rs:398,413`, `paged_spend.rs:42-44`); 510 live outputs/block (`params.rs:161`), so ~496 items/block in 2 txs |
| Token launch (op 48) | ≥4 opening outputs (definition, reserve, state/pool, balances) → same ~3,200/object + **locked backing = supply/1000 µNOID** (`launch/owner.rs:93-97`, measured `minimum_backing`: supply 1,000 → 1 µNOID, supply 10⁹ → 1,000 µNOID) | 2,500/object net-new | 4–5 native records + reserve slot; backing parked in `TokenReserve`, redeemable by burning tokens | exact-minimum backing enforced (under/over-funded rejected, token-accounting lane); supply bounds `launch.rs:38` |
| Listing (op 64) | ~2 outputs + escrow `NFT_FEE_CREDIT` = 100,000 µNOID (`nft_market.rs:25`, recoverable minus fee) | 2,500×net-new | definition + listing escrow records | round/offer families gated off at this commit (nft-marketplace lane) |
| Plain UTXO dust | 9,000 µNOID for 1-in/2-out | 2,500 | ~102 B node-local (50 B segment entry + 52 B owner index) — the baseline a native object multiplies ~4.8× | same output caps |
| Burn (op 8, cleanup) | 5,900 single; ~123/item batched (254-in) — **paid by the item owner** | 0 (state-shrinking) | removes carriage | — |

Rate/size limits found: block caps (txs 256, user pages 255, user outputs 510, actions 1,531, distinct segments 256: `params.rs:146-171`, `validation.rs:217-226`); block bytes ≤ 82.6 KB + 3,062 openings payload (`wire_limits.rs:27`, `block/payload.rs:19-21`); block time 30 s (v2). **Limits NOT found (searched, cited by absence):** no per-account or global creation rate limit, no object-count ceiling below `NATIVE_SEQUENCE_MAX` = 2^62−1 (`native_assets.rs:26`), no rent/expiry/decay on any slot or native object (`unlock_height` is a spend-timelock only), no variable-length or arbitrary-data field anywhere (metadata is one 32-B `Digest` per object — count-spam is the only spam axis), no state-growth fee escalation with *total* state (only with current occupancy, which expansion resets).

Pressure ladder (measured with the real `fee_breakdown`): multiplier 1 below 50% occupancy, 2 at 50%, 4 at 75%, 8 at 90% — and back to 1 at 37.5% immediately after `log_slots` expansion. Expansion adds one capacity bit when ≥10 of 18 finalized headers are ≥75% occupied (`params.rs:274-311`, `slot_expansion.rs:34-63`), so sustained pressure is 1–2×; capacity doubles up to 2^32 slots. **The per-object price is therefore flat forever regardless of how large total state grows.**

## Findings

### F1. The state-growth burn prices net slot delta, not object carriage — zero-burn permanent creation exists and is invisible to the occupancy feedback loop

**Severity: Low-to-Medium (permanent unpriced node-local growth; bounded one-time cost per object). Preconditions: any funded account; anonymous P2P submission like any tx.**

A `CreateCollection` with funding input amount == fee exactly is 1-in/1-out: `required = 5,800`, **burned = 0**, and `active_slot_count` is unchanged (measured CONTROL). The created collection is permanent (no rent, no expiry; the only removal is a voluntary spend), declares `maximum_items = u64::MAX` for free, and converts one ~102-B ordinary slot into a ~441-B native carriage slot (DATA+OWNER+ASSET+two owner indexes+segment entry) plus ~192 B on every future cold sync. The +339 B/object node-local multiplier over a plain UTXO is never priced: the burn only ever sees `outputs − inputs`. An attacker with an inventory of dust slots (each created once, paying the 2,500 burn) can upgrade every one of them to permanent native objects at 5,800 µNOID, zero burn, zero occupancy signal. Items, the higher-volume class, cannot fully escape the burn (each item is a net-new slot while it exists), but collections, launch definitions and listing definitions can.

### F2. Forced wallet pollution: anyone can mint NFT items owned by arbitrary addresses; removal costs the victim

**Severity: Low-to-Medium nuisance (no value extraction; availability/cleanliness harm to arbitrary addresses). Preconditions: any funded account.**

`mint_nft` (`noid_tx/src/native_assets/transition.rs:411-447`) checks only collection identity and serial continuity; output item owners are unconstrained by the authority check (authority binds *inputs* only, `transition.rs:165-176`). Measured end-to-end through the real block-commit path: a block whose 248 minted items are all owned by a non-signing VICTIM address was accepted, and the victim's row set is provable in the real store (`native_owner_roots(VICTIM)` returns all 248 in one 1.8–3.2 ms page scan; the `(collection, serial)` asset index resolves each item). There is no consent or refuse path; deletion requires the *victim's* signature and fee (5,900 µNOID single, ~123/item batched). Every wallet/GUI inventory surface keyed on native ownership lists attacker junk until the victim pays to burn it. The same applies a fortiori to collection spam against catalog browsing (`NativeCatalogKind::Collections`), though those pages are prefix-bounded (≤256/page, B-tree seek — not a C19-shaped rescan).

### F3. The ratio: creation is fee-priced, not CPU-priced — and the fee is flat to chain scale

**Severity: Informational (quantified design property; becomes a real economic-DoS lever iff NOID is cheap). Preconditions: none beyond funds.**

Measured per item (248-item batches, real construction + real apply): attacker **~106 µs** construction and **3,227 µNOID** (2,500 burned); node **~59 µs/item** apply plus 119 ms/block template build + materialization for all validation. The node is *cheaper* than the attacker per item — there is no C22-style CPU asymmetry on the creation axis; the defense is purely the fee. At block saturation (496 items/block, 2,880 blocks/day): **4,609 NOID/day buys 1.43M objects/day = ~71 MB/day consensus state and ~700 MB/day node-local MDBX on every full node, permanently** (MDBX is monotonic; live records are never pruned — only the 42-block retired tail is, `native_payloads.rs:815-849`). Attacker CPU for a full day of saturation: ~151 s. Equivalently **1 NOID ≈ 310 permanent objects ≈ 15.5 kB consensus state + ~153 kB node-local storage + ~59 kB added to every cold sync**, at the flat low-pressure price that expansion preserves forever. For scale: one year of saturation ≈ 26 GB state + ~256 GB MDBX per node for ~1.68M NOID total — less than 35 days of genesis-era emission (50 NOID/block).

### F4. Extraction check: clean no

Collection/asset ids are hash-derived from a genesis anchor (`native_assets.rs:298-316`): no human-readable namespace exists to squat, no lookalike/impersonation surface is committed on-chain (metadata is a bare digest with no identity semantics), launch backing is parked in the `TokenReserve` and redeemable (no confiscation), and the only refund machinery (rounds/offers) is profile-gated off at this commit with no creation-side refund race found. No creation-side value capture exists beyond ordinary fee market effects.

## Composition with prior availability findings

- **State × victim discovery (demonstrated):** F2's forced inventory makes `native_owner_roots` and catalog scans scale linearly in attacker-chosen junk *for any target address* (measured 248 roots/page in 1.8–3.2 ms; each root re-reads and re-validates its DATA record, `native_payloads.rs:699-739`).
- **State × cold sync (code-cited):** every live native object adds ~192 B to every future cold sync (opening preimages ride snapshot segments, `native_segment.rs:212-243`) and one record to the O(all-records) `clear_live_refs`/`replace_refs` full-table passes that snapshot staging and owner-index rebuild perform (`mdbx_store.rs:3306,4257,4841`) — new-node sync cost grows linearly in historical live-object count even after blocks are pruned.
- **State × C17/C22 (weak, honest negative):** creation txs are ordinary fee-paying valid payloads; they neither amplify the mempool byte-domain pin nor exploit the admission CPU discount (they must pay full fees and verify successfully). A creation spam wave competes for admission slots *honestly*. **State × C19 (weak):** batch minting amortizes to one receipt per 248 items, so the receipt-rescan finding grows sublinearly in object count.
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

- The per-object MDBX byte figure is the sum of the real encoding layouts (DATA 32-key+27-prefix+140-opening, OWNER 64-B key, ASSET 42+32, two owner-index rows 36+16 each, segment entry 50) plus B-tree overhead that this lane did not measure at page granularity; MDBX file size is preallocation-dominated at 250-object scale. Row existence and per-row reads were exercised for real.
- The node-side cost excludes recursive HistoryStep proving (the seal path substitutes an auditor terminal after full template-side native validation and exact state-root binding — the same bridge the production miner uses). Per-item apply cost is the sequential-materialization path; the recursive prover's marginal cost per action was not measured.
- Cold-sync composition is code-cited (snapshot segment record format; `clear_live_refs`/`replace_refs` full-table passes), not executed against a live snapshot staging run; the 192 B/object figure comes from the real encoder's layout, not a network transfer.
- Victim-discovery timings are one page on a 248-row store; extrapolation to millions of rows is linear-by-code-structure (cursor walk with per-row record read + validate), not measured at that scale.
- All timings are from one laptop (Apple silicon, release build, sequential batches); fee/burn/state-byte numbers are exact and machine-independent; µs numbers are indicative only.
- No live P2P, mempool admission under load, or multi-node sync was run; composition claims involving C17/C19/C22 rest on those lanes' measurements plus this lane's code citations.
- No upstream source was edited and no fix was applied.
