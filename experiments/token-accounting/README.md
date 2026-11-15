# Token launch, backing, redemption and inventory accounting audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. Every test drives the real local transition APIs that the sequential consensus interpreter calls (`noid_chain::state::native_assets::{launch, market}`, `apply_native_transition`, `check_native_transition`) against real `ChainState` slot trees — no mock state machine, no reimplementation of the checks under test. No live node, RPC server, peer or network was started.

Scope owned by this lane: fixed-supply issuance, creator purchase, minimum launch funding, holder backing, burn/redemption, launch-to-permanent-market graduation inventory, creation-ID discipline, native opening roots vs raw NOID amounts, and multi-asset conservation with atomic rollback. DEX price/rounding/sequencing belongs to the `dex-economics` lane; the generic UTXO state machine to the UTXO lanes.

## Findings

**No accepted bad transition was found in the audited scope.** Every adversarial input this PoC constructed was rejected by a specific, named guardrail, and every legitimate lifecycle (create → buy round → settle → claim → redeem; sell round; graduation) conserved NOID, token units and inventory exactly. The finding of this lane is therefore a negative result with the guardrails identified, plus the invariant confirmations below. Severity of anything below: informational.

Key structural facts verified against production code (not just prose):

1. **Issuance is launch-only and exactly minimum-backed.** `NativeOperation::CreateToken` and `TopUpReserve` are profile-disabled (`noid_tx/src/native_assets/transition.rs:52-58`, `profile.rs`), `BootstrapPool` and all liquidity operations are disabled (`profile.rs:operation_enabled`), and a `FungibleDefinition` opening cannot even validate with `route != Launchpad` (`noid_tx/src/native_assets.rs:570-574`). The only issuance path (`apply_owned_launch_creation` → `check_launch_creation`, `noid_tx/src/native_assets/launch/owner.rs:70-138`) forces the `TokenReserve` to equal `TokenReserve::from_definition(token, minimum_backing(supply))` **exactly** — underfunding fails the opening's backing floor (`Backing(BelowMinimum)`), overfunding fails the expected-reserve comparison. `minimum_backing(s) = s/1000` microNOID at 6 decimals (`noid_tx/src/asset_backing.rs:34-39`).

2. **Graduation inventory handoff is exact.** For every legal supply (multiple of 10^6 in `[100_000, ~10.26e9]` whole tokens): `sale_units = 3s/5`, `dex_units = 2s/5`, and `curve_cash(3s/5) = s/250 = graduation_cash()` with **no rounding residue** (10^6 = 2^6·5^6 makes every legal supply divisible by 250, so `div_ceil` is exact). The graduated pool is `bootstrap_permanent(market, s/250, 2s/5)` with `total_shares = isqrt(base·quote)` and `locked_shares == total_shares` — permanently locked liquidity (no withdrawal right), and pool conservation `escrow + prior curve cash == pool base + fee + claims` holds exactly (verified in the graduation round control). The DEX-reserved 40% can never be sold on the curve: `LaunchState::check_definition` rejects `inventory_units <= dex_units` (`noid_tx/src/native_assets/launch.rs:283-289`), demonstrated with a state at exactly `dex_units` (rejected) vs `dex_units + 1` with honest cash (accepted).

3. **Redemption is floor-exact and cannot be skimmed.** `TokenReserve::redeem` pays `floor(reserve·burned/supply)`, keeps the residue for remaining holders, and the transition layer requires the successor reserve to equal the quote byte-exactly (`noid_tx/src/native_assets/transition.rs:237-252`). Two conservation-neutral theft attempts were rejected: understating `outstanding_units` by 1 in the successor reserve → `Semantic(Reserve)`; deflating the successor reserve by 1 µNOID while paying +1 raw → the tampered opening itself is under-backed → `Opening(Backing(BelowMinimum))`. A full drain empties reserve and supply together, and a zero-supply reserve with residue is structurally invalid (`EmptySupplyHasReserve`).

4. **Creation IDs are collision-free by construction.** Native outputs get `NATIVE_CREATION_TAG | sequence` from the single monotone `ChainState.alloc_counter` (`noid_chain/src/state/native_assets.rs:181-231`, `noid_chain/src/state/native_assets/market.rs:479-527`); every id allocated across the run was distinct within each chain branch with sequence ≤ alloc_counter, coinbase-tagged and raw ids live in disjoint namespaces, and a parent reference from the future (`sequence == alloc_counter + 1`) is rejected. Asset identity is deterministic in the funding input (chain, owner, amount, creation_id, slot_index, supply, route, metadata): a second identical launch from the same funding input produces the **same asset id** and is a pure double-spend of that input — rejected by the exact slot binding (`State(Input)`), so fixed supply cannot be duplicated.

5. **Root-vs-raw-amount binding is tight.** The raw slot stores exactly `(amount_micronoid(), native creation id, opening.root())` (read back from state after issuance); a `TokenBalance` carrier carrying 7 raw µNOID is rejected (`SlotBinding`) because `amount_micronoid() == 0` for token carriers — token value never rides the raw NOID lane, and NOID value never hides inside a token root.

6. **Multi-asset scans are atomic.** A multi-launch registration scan with a cross-round request rejects the **whole** scan (`Semantic(Round)`) with the state root unchanged; a settlement fed a strict subset of a registered round cannot reproduce the committed queue root (`Semantic(Queue)`) and leaves state untouched. All state mutation happens on a clone committed only after every check passes (`apply_updates_with_input_kinds`, `noid_chain/src/state/native_assets/market.rs:529-548`). Cross-asset confusion (`TransferToken`/`RedeemToken` of asset A consuming asset B's balance/reserve) is rejected with `Asset`.

7. **Fees and rounding are burned, not pocketed.** Every accepted op reduces `circulating_supply_micronoid` by exactly its fee (asserted per operation). In the buy-round control the uniform clearing left `rounding = +1` µNOID, which is charged into the settlement fee and burned; negative rounding (≥ −1) is refunded out of the first request's fee credit with total NOID conservation still exact (`noid_tx/src/native_assets/launch/clearing.rs:416-481`). The sell-round control shows curve cash falling by exactly the floor proceeds with zero rounding residue.

## Negative and control results

Rejected attacks, each with the guardrail that blocked it (see `results.txt` for the exact lines):

- A1 reserve underfunded by 1 µNOID → `Backing(BelowMinimum)` at opening construction.
- A2 reserve overfunded by 1 µNOID → `Semantic(Funding)` (reserve must equal the exact minimum quote).
- A3 creator purchase with `market_noid = 0` (tokens without paying curve cash) → `Semantic(Conservation)`.
- A4 purchase split across two `TokenBalance` outputs → `Semantic(Funding)` (single purchase output).
- A5 purchase of `sale_units + 1` → `Quantity` (60% cap).
- A6 purchase balance owned by a non-authority address → `Semantic(Funding)`.
- A7 second identical launch (same asset id) from the same funding input → `State(Input)` (double-spend).
- A8 standalone `CreateToken`/`TopUpReserve`/`BootstrapPool`/liquidity ops → profile-disabled; `route=Standalone` definition → `Supply`.
- R2 redemption burning 0 units → `Backing(ZeroAmount)`.
- R3 successor reserve with `outstanding_units − 1` → `Semantic(Reserve)`.
- R3b successor reserve deflated by 1 µNOID, payout +1 → `Opening(Backing(BelowMinimum))`.
- R4 token carrier with raw NOID amount → `SlotBinding`.
- R6 redemption of a balance owned by someone else → `Semantic(Authority)`.
- G3 curve state at `inventory == dex_units` (selling the reserved 40%) → `Definition`; `dex_units + 1` with honest cash is legal (strict boundary).
- G4 settlement of a request bound to a different round → `State(Input)` (slot/root binding fails first).
- I3 parent ref with `sequence == alloc_counter + 1` → `Parent`.
- M2 settlement of a strict subset of the registered round → `Semantic(Queue)`, state root unchanged.
- M2b cross-round request inside a multi-launch registration scan → `Semantic(Round)`, whole scan rolled back.
- M3/M4 cross-asset transfer/redemption → `Asset`.

Accepted controls (real transitions, exact numbers in `results.txt`):

- Launch creation at the exact minimum (`s/1000`), creator purchase at exact curve cash, NOID delta = fee only.
- Full lifecycle: create → two funded buy requests → block registration (queue root committed, frozen) → settlement (sold 30%, fee 7401, rounding_fee 1) → claim payouts (tokens + NOID refund/credit) → redemption of round-issued tokens at exact floor payout; global token conservation `inventory + creator purchase + creator round + buyer + burned == supply`.
- Curve sell round: inventory grew back by the sold quantity, curve cash fell to the exact new integral, proceeds floor-exact.
- Graduation round: pool `base = s/250`, `quote = 2s/5`, `locked == total == isqrt(base·quote)`; buyer holds exactly the 60% sale allocation; `60% + 40% == supply`.
- Property sweep over 12 supplies spanning the full legal range (100 000 to ~10.26 billion whole tokens): sale/dex/graduation identities exact, two-step redemption drains any minimum-backed reserve to exactly zero.
- Spot-check: the recursive acceptance traces pin the same graduation constant (`sv / 250`, `noid_recursive/src/acceptance/trace/native_assets/financial_launch_effects.rs:240-252`) as the host.

## PoC implementation and reproduction

`build.rs` verifies the checkout HEAD equals the pin and that **all 470 files** under the six dependency crates inside `pinned/` are byte-identical to `git show 52938433:<path>` (SHA-256, dependency-free); the build fails otherwise. `src/main.rs` then drives only public production APIs: `apply_owned_launch_creation`, `apply_owned_launch_request`, `apply_block_launch_registrations`, `apply_launch_settlement`, `apply_claim_payout`, `apply_native_transition`, `check_native_transition`, plus the pure quote engines (`TokenReserve::redeem`, `BackingState`, `curve_cash_for_supply`, `PoolState::bootstrap_permanent`).

Note: do not run bare `cargo fmt` in this directory — it follows path dependencies into `pinned/` and rewrites the read-only reference (build.rs will then correctly fail); use `rustfmt --edition 2021 src/main.rs build.rs`.

From this directory:

```bash
rustfmt --check --edition 2021 src/main.rs build.rs
CARGO_TARGET_DIR="$PWD/target" cargo build --release
./target/release/token-accounting-audit 2>&1 | tee results.txt
```

Final output ends with:

```text
REJECT M2b cross-round request in a multi-group scan: Semantic(Round); state_root_unchanged=true
REJECT M3 TransferToken(asset A) spending a balance of asset B: Asset
REJECT M4 RedeemToken(asset A) consuming the reserve of asset B: Asset
CONTROL property sweep: 12 supplies from 100000000000 to 10248191152000000: sale=3s/5, curve_cash(3s/5)==s/250==graduation cash, dex=2s/5, redemption drains to exactly zero
ALL ASSERTIONS PASSED
```

## Assurance limits

- These are library-level validations that the sequential consensus interpreter invokes. No network-admitted block, no recursive-proof soundness test, and no mempool/RPC admission path was exercised; the binding between these functions and full block validation belongs to the block-validation/fork-replay lanes.
- The recursive (`noid_recursive`) re-implementation of these financial checks (~16k lines of trace builders) was not audited for divergence from the host semantics; only the graduation-constant spot-check above was performed. Host/recursive equivalence is the recursive-sync lane's scope.
- Rounding and clearing-price selection were exercised only as needed to conserve inventory; adversarial price/rounding/sequencing (e.g. systematic positive-rounding extraction, partial-fill proration edges at cap) is the DEX lane's scope. The rounding slack (−1 .. count+1) was confirmed to route into burned fees, not any recipient.
- The fee schedule itself (`MarketFeeContext`) was used, not audited (ledger-fairness lane). Liveness edges (e.g. a registered round whose clearing has no solution stalls until timeout refund) were reasoned about but not fuzzed.
- NFT collection accounting (`MintNft`/serials) was read but not PoC'd — the nft-marketplace lane owns it; `TopUpReserve` being disabled means reserves can never grow post-creation, which was treated as design, not a defect.
- No upstream source was edited and no fix was applied.
