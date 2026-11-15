# DEX permanent-pool and bonding-curve economics audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`, mirrored byte-identically under `pinned/` (verified against `git archive` of the commit before and after the final run).

All created files are confined to this directory. The PoCs are local library validations against the pinned production crates (`noid_tx`, `noid_chain`, `noid_recursive`, `noid_ivc_core`); no node, mempool, RPC server, peer, or network target was started.

## Finding: no accepted value-extracting transition; boundary observation on the cumulative-cash ceiling

**Severity: no value-extraction finding (negative result). Informational boundary observation below.**

Sought and did not obtain: an accepted value-extracting transition, a manipulated canonical fill, free asset creation, or an invalid market successor — each attack was constructed and each was rejected by a named production guardrail (see "Negative and control results").

### Boundary observation (informational): the final curve unit can be free and can trigger graduation

The curve prices sales by cumulative cash `C(q) = ceil((3*S*q + 5*q^2)/(900*S))` (`pinned/noid_tx/src/native_assets/launch.rs:210-218`), and a buy pays exactly `C(next) - C(current)` (`pinned/noid_tx/src/native_assets/launch/sequential.rs:163-168`). Because the ceiling is flat over intervals shorter than ~1/marginal-price, the last unit before the cap costs `C(cap) - C(cap-1) = 0` for the minimum supply shape:

- At `sold = cap-1`, a buy with gross input `1` NOID purchases the final unit (`output = 1`, `1e-6` tokens), **spends 0 NOID** (the whole gross input is refunded), and triggers graduation into the permanent pool seeded with `(S/250, 0.4*S)` (`results.txt`: `zero_spend_graduation`).
- Symmetrically, the first NOID buys the whole flat prefix near `sold = 0` (299 units for exactly 1 NOID).
- In the other direction the same rounding is blocked: a sell whose proceeds would round to zero is always rejected because `minimum_output >= 1` is mandatory (`LaunchError::Quantity`/`TradeError::Minimum`; `results.txt`: `zero_proceeds_sell_rejected`).

Why this is not extractable value:

1. **Aggregate overcharge, never undercharge.** `C` is a ceiling, so cumulative curve intake is always at least the exact integral price; the free tail unit is paid for by earlier buyers' rounding surplus. At the cap the intake is exactly the seeded base reserve (`C(sale_units) = S/250` asserted across minimum, mid, and maximum admissible supplies).
2. **Graduation creates no claimable value.** The pool is seeded with exactly the cumulative cash plus the unsold inventory, `locked_shares == total_shares` (no ownable LP shares), and the transition conserves NOID and tokens (asserted on an accepted graduation batch).
3. **No amplification loop.** Re-selling the free unit crosses the same flat ceiling and yields zero proceeds, which is rejected; buy-then-sell roundtrips on the curve are exact cumulative-cash reversals (500/500 random roundtrips, `assert sell.output() == buy.spent_input()`), and pool roundtrips pay the 0.30% fee in both directions (500/500, never profitable).

Precondition for the observation: an attacker can only reach `sold = cap-1` after the market has already absorbed `C(cap-1)` NOID (99.999998% of the graduation cash) from prior buyers. The 0-NOID trigger moves graduation one rounding unit earlier than a naive reading of "buys pay for tokens" suggests, and nothing more.

## Negative and control results

Guardrails named per rejected attack. All numbers below are from the final run in `results.txt` (9/9 experiments, all assertions passed, exit 0).

Pool (permanent market, `quote_exact_input`):

- 44 accepted quotes across reserve shapes (balanced, extreme `MARKET_QUANTITY_MAX`-adjacent, drifted) match a fee of `ceil(30*gross/10000)` (rounded up, pro-pool) and an output of `floor(out_reserve*effective/(in_reserve+effective))` (rounded down, pro-pool); reserves move by exactly gross in / output out; the constant product never decreases (`pinned/noid_tx/src/native_assets/market/sequential.rs:77-115`, enforced again at `:112-115`).
- Exact-minimum-output accepted, `minimum_output + 1` rejected on every accepted quote (`MarketError::Minimum`).
- Dust rejected: gross 1 (effective input 0) errors `Quantity` in all 8 shape/side combinations; zero-output swaps impossible because the post-swap output reserve must stay nonzero (`:99-102`).
- Reserve overflow rejected: input pushing a reserve past `MARKET_QUANTITY_MAX` errors `Capacity` (`:87-90`).

Curve (bonding phase, `quote_curve_exact_input`):

- 195 accepted quotes satisfy the exact cumulative-cash identity `spent = C(next) - C(current)` with full refund of the unspent remainder, zero curve fee, and `check_definition` re-derives `market_noid` from inventory (`pinned/noid_tx/src/native_assets/launch.rs:279-290`), so a LaunchState UTXO cannot claim cash inconsistent with its inventory.
- 81 graduation-crossing buys: the curve leg spends exactly to the cap, the pool is seeded with the exact cumulative cash and unsold inventory, and a market remainder of 1 NOID (zero effective input) is refunded, not swallowed (`pinned/noid_tx/src/native_assets/launch/sequential.rs:198-219` allows only `Quantity|Minimum` to be swallowed). A two-unit remainder executes and pays the market fee.
- 12 exact buy-sell roundtrips return the chain to the identical curve state.
- The two independent production encodings of the curve (interval `average_price` used for sequencing, cumulative `curve_cash` used for execution) agree within 1 unit of rounding on sampled intervals.

Accepted batches (real `ChainState` + `prepare_trade_batch` / `prepare_trade_batch_from_intent` / `apply_prepared_trade_batch`):

- Multi-trade market batch: NOID conserved (inputs = outputs + network fee), tokens conserved, canonical `NativeIntent` replay reproduces identical outputs, state root changes on apply.
- Atomicity: one policy with an unreachable `minimum_output` rejects the entire batch (`Batch(Unfilled)`) with the state root unchanged; in-batch sequencing is price-directed and deterministic across repeated runs and rebuilt batch objects, and the prepared batch's fill order equals the plan's.
- Tamper rejections, with the firing layer named: successor curve state with cash cut by 1 NOID (page structure amount binding, `pinned/noid_tx/src/native_assets.rs:1024-1031`); successor state with +1 inventory unit, token payout +1 unit, and NOID payout +1 unit (re-derived execution equality, `pinned/noid_tx/src/native_assets/wire.rs:317-343` and `trade/execution.rs:153-159`); graduation pool successor with base reserve +1 (amount binding); permuted input claims (canonical order enforced); duplicate funding slot across intents (`TradeBatch` slot set, `trade/batch.rs:154-170`); output slot colliding with the occupied parent slot (`noid_chain/src/state/native_assets/trade.rs:236-244`); fee limit of 1 against real parent-state fee quotes of 6600 (charge selection, `trade/execution.rs:288-322`).
- Host admission authenticates every input claim against parent state slot-for-slot (`noid_chain/src/state/native_assets/trade.rs:297-315`) and requires the batch fee to meet the state-derived floor (`:246-251`).

Host-vs-recursive (circuit `bind_sequential_trade_step` over `FieldR1csBuilder`):

- 72 random pool steps and 120 random curve/graduation steps: the R1CS witness derivation satisfies the constraint matrix and matches the host on all eight economic fields (`spent_input, output, market_fee, curve_spent, curve_output, after_curve, after_base, after_quote`).
- Deterministic boundary cases: first-NOID multi-unit buy, zero-spend graduation, dust-refund crossing (remaining = 1), and two-unit-remainder crossing all agree between host and circuit; a `minimum_output` the host rejects makes the R1CS unsatisfiable; a capacity-overflowing crossing buy is rejected by the host and unsatisfiable in the circuit. The circuit's fee (`gross - floor(9970*gross/10000)`) and the host's (`ceil(30*gross/10000)`) are the same integer by the identity `g - ceil(x) = floor(g - x)`.

## PoC implementation and reproduction

`src/main.rs` (single binary, 1,656 lines) drives only public production APIs of the pinned crates; the "oracle" formulas re-implement the documented fee/cash arithmetic for differential comparison (see Assurance limits). `pinned/` is a `git archive` extraction of the commit (plus the crates' own `Cargo.lock`), pinned by path dependencies in `Cargo.toml`.

From this directory:

```bash
rustfmt --check src/main.rs
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Do not run bare `cargo fmt` here: it also formats the `pinned/` path dependencies (one such formatting was reverted; `pinned/` re-verified byte-identical to `git archive 52938433` afterwards).

Expected output: nine `... complete` progress lines, then a JSON report whose top level is `"accepted_value_extraction_found": false`, ending with the `host_vs_recursive_differential` object (`pool_r1cs_cases_matched: 72`, `curve_and_graduation_r1cs_cases_matched: 120`). Any assertion failure panics with a non-zero exit; a clean run exits 0. `results.json` is the extracted JSON object of the final run.

### Repairs to the inherited (dead-agent) harness

This lane inherited a partial harness that had never completed a run (its `results.json` was empty and it panicked on its own invariants). Repairs made, all to lane-local code:

- `spent_input >= 1` assertion replaced: production semantics legitimately allow a zero-spend buy on a flat ceiling interval (see Finding).
- graduation probe skipped the degenerate `gross == 0` case (invalid input, not a trade).
- payout-slot allocation moved into `TradeBatch::new`'s sorted policy order (`TradeBatch` sorts policies by authorization key; slots were previously allocated in spec order, which breaks any asymmetric spec list).
- removed an R1CS single-term/zero-constant assertion on the packed output expression that never held (values are read by expression evaluation, mirroring the circuit module's own `value()` helper).

## Assurance limits

- The pool and curve arithmetic oracles are re-implementations of the same formulas the production code uses (differential/regression value, not independent derivation). The curve formula is independently corroborated inside the production tree by the `average_price` encoding (checked in code to 1 unit of rounding) and by the calculus identity between them (reviewed, not executed).
- The host-vs-recursive differential proves agreement on honest witness derivations, enforcement of the minimum-output relation, and consistent rejection on the probed boundary cases. It does not attempt an alternative-witness or satisfiability search, so constraint completeness of the per-step predicate (e.g. against a malicious prover crafting non-canonical witnesses) is not established here; nor was the full recursive predicate assembled (authorization, funding, payout, and group-composition circuits were not bound together, and no proof was generated or verified).
- Chain-state experiments use `ChainState::from_sparse_utxos_at` fixtures (4096 slots, synthetic UTXO set) and the real preparation/application functions. No mempool admission, block assembly, consensus validation, networking, or multi-node race was exercised; no live target was contacted.
- Fee behavior was sampled at one state geometry (minimum quoted fee 6600 in the fixture); the fee curve at other state sizes was not explored.
- Priority/sequencing determinism was checked by repeated execution and rebuilt batch objects, not by enumerating adversarial priority collisions.
- Rounding-boundary coverage is exhaustive on the sampled shapes, not exhaustive over the input space; the flat-ceiling observation was verified at minimum supply and the conservation anchors at minimum, mid, and maximum admissible supplies.
- No upstream source was edited and no fix was applied. `pinned/` was verified byte-identical to `git archive 52938433` after the final run; pre-existing unrelated working-tree modifications in the shared checkout were left untouched.
