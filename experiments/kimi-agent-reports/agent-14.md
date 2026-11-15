# agent-14: Attack contract rights

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Outcome

No accepted unauthorized action, caveat bypass, replay, counter manipulation, conservation bug, or Pruv/native semantic disagreement was confirmed at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`.

All work is confined to `experiments/contract-rights`:

- `Cargo.toml`, `Cargo.lock` — local PoC package.
- `src/main.rs` — authorization, State, counter, Pruv differential, and arithmetic-relation PoCs.
- `src/bin/schedules.rs` — budget, recurring-payment, and vesting transition PoCs.
- `pristine/` — archived pinned sources, isolating results from unrelated shared-checkout edits.
- `RESULTS.md` — commands, complete results, negative findings, and limitations.

Generated build directories were removed after verification.

## Verified results

- Actual owner-authorization prove/verify succeeded for the correct key and rejected the wrong key.
- Fixed-recipient retargeting, excess allowance payout, early close, and pre-unlock vault calls were rejected.
- Two accepted allowance transitions conserved value: supply decreased exactly by the two fees, `initial_supply - 20`.
- Replaying an applied page through `ChainState::apply_tx` returned `UnknownOrSpentInput`, leaving State root and supply unchanged.
- A one-shot custom counter accepted its first call and rejected the correctly rooted successor’s repeat.
- Budget, recurring-payment, and tranche schedule PoCs enforced their documented height, amount, reset, and recipient rules.
- Pruv differential: **50,000 generated programs, 35,740 compilations, 428,880 executions**, no mismatch.
- Legacy and v2.1 arithmetic relations: **1,115 checks**, including wrong successors and arithmetic failures, no mismatch.
- Pristine upstream suites passed:
  - Pruv: 18 unit tests, including real Z3, plus 3 CLI tests.
  - `noid_tx`: 36 unit, 6 ABI3, and 8 application tests.
- `cargo fmt -- --check`, locked offline build, and manifest validation passed.

Documented behaviors were not misreported as exploits: per-call allowance caps, budget reset discarding unused allowance, inclusion-relative recurring schedules, missed-tranche claims, physical output-0 closing semantics, independent funded counters, and Pruv’s per-call rather than lifecycle postconditions.

## Preconditions and limits

The PoCs exercise real transition construction/preflight, owner-authorization proofs, exact State application, and arithmetic relation satisfaction. They do **not** construct or accept a complete mined HistoryStep block, so no full-block or live-network acceptance claim is made.

Real-chain execution additionally requires contract-fork activation, the exact funded UTXO incarnation, valid epoch anchor and slots, the height-selected authority’s capsule, admission at actual inclusion height, and canonical State/HistoryStep acceptance. At `height >= deadline`, the recovery branch applies.

The shared repository was not edited. Its unrelated pre-existing changes and newly appearing `domain-audit.json` were left untouched.
