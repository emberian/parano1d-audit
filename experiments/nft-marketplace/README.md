# NFT marketplace: active operations and gated paths

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`. A verbatim, read-only copy in `source/` supplies path dependencies to `poc/`. Tests use synthetic in-memory `ChainState`; no node, wallet, peer, or RPC server was run.

**Result:** Collection creation, mint, transfer, listing, instant purchase, cancellation, and burn passed the tested lifecycle and rejection controls. Funded purchase rounds and offers are disabled by the activation profile at this commit, so results from synthetic round escrow describe dormant code, not reachable marketplace behavior. A suspected native/recursive disagreement for collection genesis was refuted by a production-faithful R1CS witness.

## Funded purchase rounds and offers are disabled

The NFT marketplace in `noid_tx`/`noid_chain` has two layers:

1. **Enabled:** base NFT operations (CreateCollection/Mint/Transfer/Burn, codes 5–8) and the owner listing lifecycle — CreateListing (64), Purchase (65), CancelListing (66) — with opening kinds `NftListingDefinition`/`NftListing`. `Purchase` dispatches to the instant-buy check (`check_financial_owner` → `check_listing_purchase`, `noid_tx/src/native_assets/financial/owner.rs:229-238`): one idle listing input plus buyer cash, with the seller paid `ask + NFT_FEE_CREDIT` exactly.
2. **Implemented but disabled:** the funded round system (request funding → registration → settlement → timeout reset → recovery → refunds, operations 70–75) and offers (67–69). Two independent gates apply:
   - `noid_tx/src/native_assets/profile.rs:8-33` — `operation_enabled` omits codes 67..=75, so `FinancialOperation::operand()` rejects them before any semantic check.
   - `profile.rs:35-53` — `kind_enabled` omits `NftPurchase` and `NftOffer`, so `NativeOpening::validate()` returns `Err(Kind)` for those openings. `apply_owned_purchase_funding` and offer funding/acceptance/cancellation fail inside `claims()` (`check_output` → `validate`) even with otherwise valid claims; the PoC demonstrates this path.

   Additionally, `check_purchase_funding` has no dispatcher in `check_financial_owner` at all (only the chain-side `apply_owned_purchase_funding`, which no production code calls), and `FinancialBlockTracker::owner_outputs` returns early for `Purchase` (`noid_chain/src/block/native_assets/financial.rs:276-278`), making its `Purchase → registration` arm (lines 341-350) dead code.

At this commit, no profile-enabled path reaches round settlement or offers. The round-settlement economics below apply only to synthetically seeded escrow and possible future activation. The listing-settlement recursive circuit (`bind_financial_listing_settlement`, operation code 71) exists but its operation is also gated.

## Collection genesis: no native/recursive divergence found

An earlier PoC modeled the `CreateCollection` funding action's slot, packed value, and root as zero in the recursive trace and predicted rejection of a native-accepted collection. The production trace retains the funding row's real values: `noid_recursive/src/acceptance/trace/native_assets/actions.rs` takes `slot: row[4]`, `packed_value: row[5]`, `root: [row[6], row[7]]`; `financial_base_projection.rs` masks only by family selector (`slot: mul(b, &selected, &action.slot)`). Only the opening-validation inputs to `bind_native_action_slot` are native-masked. The genesis accumulator (`kernel.rs`, `complete_group_summaries`) uses the real funding row.

The `bind_native_kernel` R1CS accepts the production-faithful witness and rejects a witness with a zeroed funding anchor. This checks binding to the funding slot, value, and root. It does not establish native/recursive equivalence outside this collection-genesis path.

## Negative and control results

All of the following ran against the real production functions (no mocks), on synthetic in-memory state, and every rejection names the guardrail that fired:

- **Full live lifecycle, accepted transitions:** create collection → mint 2 items → transfer → list (escrow `NFT_FEE_CREDIT` + item) → instant purchase (seller paid `ask + credit` exactly, buyer change exact, item delivered with metadata/serial preserved, supply burn exactly the fee) → cancellation on a cloned pre-buy state (seller refunded `credit − fee`, item returned) → burn. All creation ids follow the native allocator sequence; `NftItem.id()` is `(collection, serial)` only — owner/metadata/unlock_height changes preserve identity by design, and conservation checks carry metadata separately.
- **Stale/race rejections:** replaying the purchase on the post-buy state fails (listing slot consumed); duplicate listing inputs rejected (`Input`); tampered request budget or a dropped request at settlement fail queue-commitment recomputation (`Queue`); duplicate registration requests rejected (`Input`); settlement replay after settlement fails; double timeout-reset fails (`Input`).
- **Frozen-round interference:** instant purchase of a *frozen* listing and cancellation of a frozen listing both rejected (`Phase`) — the owner path and the round system cannot touch the same escrow.
- **Authorization:** non-owner mint (`Authority`), non-owner offer acceptance (`Owner`), non-payer offer cancellation (`Owner`), non-owner transfer (`Authority`) all rejected.
- **Conservation:** duplicate items in transfer and metadata mutation in transfer rejected (`NftConservation`); the recursive transfer kernel (real `bind_native_kernel` R1CS witness) accepts the honest transfer and rejects the metadata-mutated one; skimmed offer payouts (1 micronoid less to the item owner) rejected (`Conservation`).
- **Timelocks:** items minted with `unlock_height > height` cannot be transferred (`Timelock` at `check_input`) or listed (`Height` at `ListingDefinition::from_item`). Offer acceptance carries the same check in code (`NftOffer::check_item`), though that path is profile-gated (Finding 1). The recursive circuit enforces the same input timelock for item openings (`validation.rs`, `bind_native_action_slot`: `height < secondary` pinned for item/balance inputs).
- **Round economics (conditional on activation, seeded synthetically below the kind gate):** settlement pays the seller `ask + credit`, winner by `min(payer_priority, request root, funded key)` (recomputed independently in the PoC and matched), each request charged `fee/n + (i < fee%n)` in funded-key order, refunds exact to the micronoid, same-owner payouts merge, supply burn exactly the fee; window boundaries hold (settlement only `registered+1..=registered+3` height-wise, refunds only at `>= registered+4`); timeout reset bumps the generation and changes the round id (old round ids unrepeatable); expired recovery returns item + remaining credit to the seller only after both expiry and refund-phase boundaries.
- **Fee caps can never bind:** worst case (90% occupancy, 8× pressure multiplier, 1..=16 requests, log_slots 8/24/32) settlement fee ≤ ~39.3k vs cap `n × 50_000` (≥ 50k even at n=1); refund fee is constant 5,800 and recovery fee ≤ 26.5k, both far below the 100,000 credit — no fee-induced settlement/refund deadlock exists via `MarketFeeContext`.
- **Static native-vs-recursive consistency (read, not executed):** the kernel's NFT inventory routing sort and its strict-order key agree (little-endian significance); the listing-settlement circuit's winner key places `payer_priority` as the most-significant segment, matching the native `min_by_key((priority, root, key))`, and distributes `q + (i < r)` over funded-key-sorted rows, matching the native order (`queue.rs` sorts by the same key bytes). `bind_nft_transition` in `nft_recursive/.../native_assets/nft.rs` has no callers anywhere in the tree (dead code; its own ordering is also self-consistent).

## PoC implementation and reproduction

`poc/` is a library crate (`nft-marketplace-audit`) depending on `noid_tx`, `noid_chain`, `noid_recursive`, `noid_core`, `noid_poseidon2b`, `noid_ivc_core` from `../source` at the pinned commit. The ten tests in `poc/src/lib.rs` are the audit. The R1CS controls build real constraint systems with `FieldR1csBuilder` and call the production `bind_native_kernel` binder; everything else calls the production apply/check functions directly.

From this directory:

```bash
cd poc
cargo fmt --check
CARGO_TARGET_DIR="$PWD/../target" cargo test --release 2>&1 | tee ../results.txt
```

Final output (see `results.txt`):

```text
running 10 tests
test tests::accepted_collection_mint_transfer_list_buy_cancel_burn_and_stale_replay ... ok
test tests::activation_profile_gates_round_and_offer_surface ... ok
test tests::identity_codec_and_external_bytes_are_strict ... ok
test tests::owner_price_expiry_metadata_and_duplicate_controls_reject ... ok
test tests::recursive_genesis_kernel_binds_funding_anchor_and_accepts_native_genesis ... ok
test tests::recursive_transfer_matches_native_and_binds_metadata ... ok
test tests::round_settlement_exact_accounting_and_stale_rejections ... ok
test tests::time_locked_items_reject_transfer_and_listing_paths ... ok
test tests::timeout_refund_reset_and_expired_recovery_paths ... ok
test tests::worst_case_fees_stay_within_settlement_and_refund_bounds ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured
```

Test map: lifecycle/stale-replay and owner/metadata/duplicate/codec controls; `recursive_genesis_kernel...` (Finding 2 both directions) and `recursive_transfer...` (R1CS controls); `activation_profile_gates_round_and_offer_surface` (Finding 1, including the two end-to-end blocked transitions and the runnable expired-offer refund on synthetic escrow); `round_settlement_exact_accounting_and_stale_rejections` and `timeout_refund_reset_and_expired_recovery_paths` (round economics on synthetic escrow); `time_locked_items...`; `worst_case_fees...`.

## Assurance limits

- No vulnerability was demonstrated on the live surface: every attempted unauthorized action, accounting flip, stale consumption, duplication, and race was rejected by a named check. The lane's outcome is negative results plus the two findings above.
- The round-settlement/refund/reset/recovery tests seed the funded-request escrow synthetically (slot values bound to opening roots, native allocator ids, height-boundary-validated state) because the funding transition itself is kind-gated at this commit (Finding 1). They exercise the exact apply functions a block would call if the operations were enabled, but not a profile-enabled admission path, and not the recursive settlement circuit as an executed witness.
- Native-vs-recursive consistency for the listing-settlement circuit (`bind_financial_listing_settlement`) is established by code reading (ordering, winner rule, fee-split arithmetic, timelocks), not by building its witness, which needs the full `FinancialActionTrace`/definitions machinery. The executed R1CS evidence covers the base kernel (collection genesis funding anchor, transfer metadata binding) only.
- The sequential-trade route (operation 80, `ExecuteTrades`, the currently live trading kernel) was not executed; prior lanes covered budget/conservation properties broadly, and this lane read its NFT listing market glue only superficially.
- `NftRoundAnchor` fixtures use synthetic parent headers; round-context binding to real header windows (`FinancialHeaderWindow`) was not exercised beyond `from_parent_header`'s own checks.
- Minor observation, no security impact: `ChainState::from_sparse_utxos` without a height boundary rejects native-tagged slot ids (raw id > alloc counter); height-boundary rebuilds (`from_sparse_utxos_at` at a v2.1-active height) validate them by sequence instead.
- No upstream source was edited and no fix was applied.
