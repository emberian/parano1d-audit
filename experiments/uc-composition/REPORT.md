# UC/composable-security investigation

## Scope and result

Source: `52938433a30b185ab007afaa15166a632cf2e82e`. The complete pinned tree is archived under `pinned/`; all reported results used that snapshot, not the repository's pre-existing working-tree modifications. The manuscript is the supplied `signatureless-authorization-artifact` snapshot.

**Bottom line: no concrete Low-or-higher composable-security vulnerability was demonstrated.** One **Informational assurance gap** remains: the native authorization boundary is statement-scoped and has no UC session abstraction, global-UC oracle model, or online concurrent-extraction theorem. This does not contradict the manuscript, which claims a fresh-intent game and a recursive relation proposition, not UC security.

This investigation did not duplicate version replay, artifact-identity/cache issues, legacy-FRI malleability, or recursive synchronization.

## UC-GAP-01 — Statement-scoped acceptance is not a per-session UC functionality

**Severity: Informational (assurance gap, not an authorization bypass).**

The capsule's entire public statement is `(tx_body_hash, address)`:

- `pinned/noid_gkr/src/zk_authorization.rs:245-266` defines and flattens exactly those four field elements.
- `pinned/noid_gkr/src/zk_authorization.rs:393-407` absorbs the construction/geometry prefix, those four fields, and the source cap.
- `pinned/noid_gkr/src/wallet_authorization.rs:642-658` reconstructs the statement from the canonical body at verification.
- `pinned/noid_gkr/src/zk_auth_qrom.rs:160-176` explicitly records `dedicated_session_salt_bits: 0`.

Consequently, a proof for an already authorized `(intent_id, address)` is valid in another verifier session with the same statement. Independent fresh proofs for that statement are also valid. This is exactly what the local PoC observes.

That behavior does **not** win the manuscript's forgery game. The game authorizes canonical intents and requires the final canonical intent not to have been requested previously (`manuscript/sections/05-security.tex:71-87`). It does not require unique proof bytes or per-session freshness. The requested set is keyed by the intent identifier (`manuscript/sections/f-merkle-simulation.tex:139-154` and `j-fresh-extraction.tex:10-12`). Execution, rather than `CheckAuth`, enforces state and freshness (`manuscript/sections/02-statements.tex:40-65`).

A hypothetical `F_auth(sid, ...)` that authorizes only one session, or an ideal functionality that treats new proof bytes as a new authorization, would diverge from native acceptance. Neither functionality is specified or claimed here. Adding a session salt would also change the intended statement-boundary semantics and should not be presented as a necessary fix without an application requirement.

### Real pinned PoC

`src/main.rs` uses the production wallet prover/verifier and performs the following:

1. Spawns two concurrent proving threads with independent production `OsRng` attempts for the same body and owner.
2. Verifies both distinct proofs in session A.
3. Replays both proofs in an independent session B with the same body.
4. Changes only one byte of `epoch_anchor`, producing a different transaction ID, and confirms that both proofs reject.

Observed in `pinned-run.txt`:

```text
statement=e9be25bf74f589f2cee20634d3ae92c3948a0782cd9425511108bc6f615c0816
changed_statement=ed3f9f91e0358e47b83a5d96a70d52ccf0e65e3ac04c2132818802e8d0c47335
proofs_have_distinct_bytes=true
proofs_have_distinct_source_commitments=true
both_proofs_accept_in_two_sessions=true
both_proofs_reject_changed_context=true
```

**Malleability assessment:** randomized proofs and acceptance of multiple byte strings for one statement do not violate the stated fresh-intent ideal. A proof-malleability finding would need either a changed semantic statement accepted without authority, an execution/freshness bypass, or a separately specified non-malleable/session ideal. None was established here.

## Cross-protocol and statement-context checks

No unintended second meaning was found in the inspected production adapters:

- Ordinary wallet verification validates public transaction logic and derives `TxBodyHash` plus `input_owner` (`pinned/noid_tx/src/owner_auth.rs:35-40`, `pinned/noid_gkr/src/wallet_authorization.rs:642-658`).
- Paged spends use the `PAGEDTX_` domain, an explicit version/page count, and ordered page hashes (`pinned/noid_tx/src/paged_spend.rs:348-374`).
- Trade authorization uses `TRINT_21` over the policy root and ordered funding. The policy root binds chain, epoch, owner, asset, recipient, side, limits, and validity (`pinned/noid_tx/src/native_assets/trade.rs:60-122,206-227`).
- Native operation headers encode chain, authority, operation, mask, epoch, and fee into the pages before the logical ID is derived (`pinned/noid_tx/src/native_assets/wire.rs:821-925`). Native bits 14/15 are outside the ordinary paged-spend validity mask, so native pages do not also validate as an ordinary paged spend (`pinned/noid_tx/src/paged_spend.rs:28-40,516-524`).
- Generic native verification and financial-owner verification are not interchangeable: the generic transition rejects `NativeOperation::Financial` (`pinned/noid_tx/src/native_assets/transition.rs:222`), while the financial-owner path requires it (`pinned/noid_tx/src/native_assets/wire.rs:563-594`).
- Contract calls use the opened authority rather than treating the object root as the wallet address (`pinned/noid_gkr/src/wallet_authorization.rs:340-378,596-632`), and `check_call` separately enforces object policy and transition semantics (`pinned/noid_tx/src/experimental_object.rs:166-222`).

These are adapter and execution checks, not a claim that arbitrary third-party protocols can safely assign their own meanings to the low-level four-field verifier. Such an external adapter must supply the canonical-intent binding assumed by the manuscript (`manuscript/sections/08-conditions.tex:23-28`).

## Global-oracle/context collision audit

`audit_domains.py` scanned every Rust source in the full pinned tree for `DomainTag::new(b"...")`. `domain-audit.json` records **71 distinct labels across 103 uses**. Repeated production labels are matching native/recursive implementations of the same named hash object, such as `PAGEDTX_`, `TRINT_21`, `IVCPCSL_`, or the contract roots. No accidental shared label between two different production meanings was found.

There is one research-only domain-registry caveat: `policy_capsule.rs:67` and `universal_policy_capsule.rs:69` both use `POLICY__`, but their program encodings and statement absorb schedules differ (`policy_capsule.rs:224-240` versus `universal_policy_capsule.rs:219-237`). Their Owner relations differ as well. No equal statement digest or cross-accepting artifact was found, so this is not an exploit; it is a reason to assign separate domains if these experiments evolve into composable protocols.

This is only a static domain-label check, not a proof that full permutation states cannot collide. The manuscript explicitly uses one common permutation and charges collision/programming error (`manuscript/sections/05-security.tex:108-109,130-137`; `f-merkle-simulation.tex:156-168`).

### Precise UC-model obstruction

The native simulator installs programmed pairs by composing the visible permutation with swaps (`P_* = P \circ \rho`) and protects previously observed pairs (`manuscript/sections/f-merkle-simulation.tex:68-103,139-154`). For an arbitrary environment protocol `Q` sharing this oracle, a global-UC proof must also account for `Q`'s correctness and ideal simulator when those pairs change. The supplied theorem bounds the accessible view for the charged common oracle interface, but it does not define a global UC random-oracle functionality or prove preservation of arbitrary `Q` ideal functionalities.

That is a missing composition proof, not evidence that any repository protocol actually breaks. No concrete repository `Q` with incorrect acceptance under these programmed swaps was found.

## Adaptive concurrency and extraction

The manuscript does cover **adaptive authorization attempts**: `S` adaptive classical attempts, a common query bound `T`, and independent fresh masks (`manuscript/sections/f-merkle-simulation.tex:156-168`; `05-security.tex:154-158`). Thus “the proof only handles one static request” would be inaccurate.

The extraction theorem, however, extracts from **one fresh accepted capsule after the complete native simulated experiment** (`manuscript/sections/j-fresh-extraction.tex:330-352`). It is not an online UC extraction theorem for an environment that can interleave arbitrary sessions, request multiple final forgeries, and continue interacting after each acceptance. A UC realization would need a defined functionality and an online environment simulator handling those events, or a separate argument reducing that execution to this game.

No concrete adaptive/concurrent forgery was produced. The two-thread PoC exercises honest concurrent proving only and is not represented as an attack on the extractor. The production prover's independent-entropy and rollback assumptions are explicit at `pinned/noid_gkr/src/zk_authorization.rs:1160-1165`.

## Recursive composition semantics

The manuscript's recursive claim is deliberately a relation proposition: exact arithmetic, complete tile coverage, canonical bindings, and outer-proof soundness imply the selected capsules' native equations and execution predicates (`manuscript/sections/06-recursive.tex:20-39`). It is not a UC composition theorem and defines no separate recursive ideal functionality.

The code follows that boundary:

- Every live proof is natively verified before preparation (`pinned/noid_recursive/src/acceptance/trace/zk_authorization_candidate.rs:545-578`).
- Recursive binding asserts that every prepared statement equals the canonical slot statement (`.../zk_authorization_candidate.rs:704-718`).
- Statement aliases are checked against native statements before tile construction (`.../zk_authorization_candidate.rs:905-943`).

Two pinned release tests passed:

```text
acceptance::trace::zk_authorization_candidate::tests::native_end_to_end_candidate_is_satisfied_with_exact_alias_ledger ... ok
acceptance::zk_auth_capsule_schedule::tests::canonical_full_transcripts_match_compiled_layouts_and_bridge ... ok
```

The first builds a native algebraic candidate/witness and checks the recursive candidate relation; the second constructs a real native proof and checks native/compiled transcript challenges and bridge equality. Logs are in `recursive-candidate-test.txt` and `recursive-transcript-test.txt`. These tests provide no evidence of an ideal-versus-native semantic gap, and they do not prove the outer recursive proof system or UC composition.

## Reproduction

From `experiments/uc-composition`:

```bash
CARGO_TARGET_DIR="$PWD/target" cargo run --release
python3 audit_domains.py pinned

cd pinned
CARGO_TARGET_DIR="$PWD/../target" cargo test --release -p noid_recursive \
  native_end_to_end_candidate_is_satisfied_with_exact_alias_ledger -- --nocapture
CARGO_TARGET_DIR="$PWD/../target" cargo test --release -p noid_recursive \
  canonical_full_transcripts_match_compiled_layouts_and_bridge -- --nocapture
```

All commands completed successfully. Proof byte lengths vary because fresh randomness changes multiproof path sharing; semantic results are stable.

## Recommended assurance work, not an emergency fix

If UC assurance is desired, first specify the intended authorization functionality: whether records are keyed by `(m,a)` or also by protocol/session, whether same-statement replay is allowed, and what execution freshness consumes. Then model the shared global oracle and its programming interface, prove online extraction for the desired environment scheduling, and state recursive ideal acceptance separately. The current evidence does not justify changing the capsule transcript or claiming a false theorem.
