# ABI3 programmable-rights audit

## Outcome

No accepted unauthorized action, caveat bypass, replay, counter manipulation, conservation bug, or Pruv/native semantic disagreement was confirmed at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`.

This is a bounded local audit, not a proof that all custom programs or the full chain are secure. No live target was used. The `pristine/` directory is a `git archive` of the pinned commit, so the final PoCs do not depend on unrelated edits in the shared checkout.

## Runnable artifacts

- `src/main.rs native-rights`: real `ObjectOpening` calls, wallet authorization proof verification, exact `ChainState::apply_tx` transitions, stale replay, conservation, recipients, allowance limits, vault lock, and a one-shot custom counter.
- `src/bin/schedules.rs`: period budgets, recurring payments, and tranche vesting through real build/check transition APIs.
- `src/main.rs fuzz-pruv`: independently generated Pruv statements and expressions, interpreted at source level and compared with compiled ABI3 execution. Arithmetic and branch failures must agree.
- `src/main.rs fuzz-relation`: all ten opcodes against both the legacy and v2.1 arithmetic R1CS relations, including wrong successor states and native arithmetic failures.

Build and run from this directory:

```sh
CARGO_TARGET_DIR=target-pristine cargo build --locked --offline --bins
./target-pristine/debug/contract-rights-audit native-rights
./target-pristine/debug/schedules
./target-pristine/debug/contract-rights-audit fuzz-pruv 50000
./target-pristine/debug/contract-rights-audit fuzz-relation 32
```

Final results:

```text
native_rights_ok wrong_authority_rejected recipient_retarget_rejected allowance_cap_enforced vault_lock_enforced
state_transition_ok conservation=input-fee stale_replay=UnknownOrSpentInput replay_atomic=true successor_creation_id=8
counter_control_ok first_call_accepted successor_repeat_rejected stateless_preflight_replay_requires_state=true

period_budget_ok early_rejected fee_included reset_at_110 exhausted_window_rejected recovery_recipient_bound
recurring_payment_ok due_height_and_exact_amount_enforced inclusion_relative_schedule missed_periods_not_accumulated payer_recovery_bound
tranche_vesting_ok early_rejected missed_tranches_one_per_call fourth_premature_rejected maturity_recipient_bound

pruv_differential_ok iterations=50000 compiled=35740 comparisons=428880
relation_differential_ok pairs_per_opcode=32 comparisons=1115 legacy_and_v2_1=true
```

`cargo fmt -- --check` and `cargo build --locked --offline --bins` passed. Upstream warnings are unchanged.

## Negative findings and outer invariants

- A wrong spending secret could not produce the valid call's authorization. Retargeting a fixed recipient was rejected. Deadline, per-call payout, and early-vault negative controls failed closed.
- For two accepted allowance transitions, exact State supply was `initial_supply - 20`, equal to the two fees. Each body satisfied `input = retained + payout + fee`.
- Reapplying an already applied contract page returned `UnknownOrSpentInput` and left the State root and supply unchanged. Exact slot, amount, creation ID, and owner binding prevents treating stateless `check_call` success as replay acceptance.
- The custom one-shot counter accepted its first transition and rejected the correctly rooted successor's second payment. Its old page still passes stateless preflight, which is why State must also be checked.
- Program and policy mutation is not a template escape: the proof spine recomputes old/new roots from code, policy, and state and pins them to input/output owners. Upstream's every-opening-byte binding test passed.
- Period budgets include fees and reject early/exhausted-window spends. Recurring payments enforce due height and exact amount. Vesting releases missed tranches one at a time and rejects the next premature tranche. Closing recipients remained bound.
- No source-versus-compiled mismatch appeared in 428,880 Pruv executions across 35,740 generated compilations. No interpreter-versus-relation mismatch appeared in 1,115 legacy/v2.1 checks.

The following are documented behavior, not vulnerabilities: allowance caps are per call rather than lifetime caps; a new budget window discards unused budget; recurring next-due height is inclusion-relative; missed tranches remain claimable; closing uses physical output 0 (`retained` in the integer context); and Pruv `ensures` is a per-call integer postcondition, not a lifecycle, authorization, recipient, or initial-state proof. Independently funded outputs also have independent counters.

## Upstream verification

Against the pristine archive:

```sh
PRUV_TEST_Z3=/opt/homebrew/bin/z3 CARGO_TARGET_DIR=target-pristine-upstream \
  cargo test --locked --offline -p noid_pruv -- --include-ignored
CARGO_TARGET_DIR=target-pristine-upstream cargo test --locked --offline -p noid_tx
```

Both commands exited successfully. The Pruv run passed 18 unit tests, including real Z3 queries, and 3 CLI tests. The transaction run passed 36 unit tests, 6 ABI3 tests, and 8 application tests.

## Limits and real-chain preconditions

The PoCs exercise native transition construction/preflight, actual owner-authorization prove/verify, exact State application, and arithmetic relation satisfaction. They do not construct or accept a complete mined HistoryStep block and do not test live networking. Accordingly, no claim of full-block acceptance is made.

On a real chain, a call additionally needs the applicable contract fork to be active, a funded exact UTXO incarnation, valid epoch anchor and output slots, the height-selected authority's valid capsule, admission at the actual inclusion height, and canonical State/HistoryStep acceptance. Deadline equality selects recovery (`height >= deadline`). Reorganizations and conflicting spends remain outer-chain concerns.
