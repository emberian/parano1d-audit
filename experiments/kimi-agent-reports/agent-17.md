# agent-17: Probe UC composition failures

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Investigation complete. Full details are in `experiments/uc-composition/REPORT.md`.

## Findings

**No concrete Low-or-higher composable-security vulnerability was demonstrated.** The supported finding is **Informational: UC assurance gap, not a false manuscript theorem or authorization bypass.**

- Native authorization is statement-scoped: exactly `(tx_body_hash, address)`, with `dedicated_session_salt_bits: 0` at `pinned/noid_gkr/src/zk_auth_qrom.rs:160-176`.
- A real pinned PoC generated two concurrent, distinct production proofs for one statement. Both verified in two independent verifier sessions; both rejected after changing only the epoch context.
- This does **not** violate the manuscript’s fresh-intent game: a different proof for an already requested intent is not a fresh-intent forgery. It would diverge only from an additional per-session or non-malleable ideal functionality, which the manuscript neither defines nor claims.
- No unintended second meaning was found across the inspected wallet, paged-spend, native-asset, financial, trade, and contract adapters. Their statement derivation and execution checks prevent the straightforward cross-protocol replays examined.
- The manuscript handles adaptive authorization attempts, but its extractor handles one fresh accepted capsule after the complete experiment. That is not an online UC extraction theorem for arbitrary environment interleavings.
- Oracle programming composes the shared permutation with swaps. A global-UC proof would additionally need to model arbitrary environment protocols sharing that oracle and their ideal functionalities. No repository protocol was found to accept incorrectly because of these swaps.
- Recursive composition is a native-equations/relation proposition, not a UC theorem. No native-versus-recursive acceptance differential was found.

A research-only caveat is documented: `policy_capsule.rs` and `universal_policy_capsule.rs` share `POLICY__` despite different statement encodings and relations. No equal digest or cross-accepting artifact was found, so this is domain-registry hygiene, not an exploit.

## Verification

All reported execution used the complete archived `52938433` tree under `experiments/uc-composition/pinned/`.

- `cargo run --release`: passed; distinct concurrent proofs accepted for the original statement and rejected for the changed statement. Output: `pinned-run.txt`.
- `native_end_to_end_candidate_is_satisfied_with_exact_alias_ledger`: **1 passed**. Log: `recursive-candidate-test.txt`.
- `canonical_full_transcripts_match_compiled_layouts_and_bridge`: **1 passed**. Log: `recursive-transcript-test.txt`.
- Domain audit: **71 distinct labels across 103 uses**. Output: `domain-audit.json`.
- `rustfmt --check src/main.rs`: passed.
- Workspace-wide `cargo fmt --check` additionally inspected archived pinned dependencies and reported a pre-existing module-order formatting difference in `pinned/noid_tx/src/native_assets.rs`; pinned sources were left unchanged.

## Files created

All changes are confined to `experiments/uc-composition`:

- `REPORT.md`
- `Cargo.toml`, `Cargo.lock`
- `src/main.rs`
- `audit_domains.py`, `domain-audit.json`
- `pinned-run.txt`
- `recursive-candidate-test.txt`, `recursive-transcript-test.txt`
- `pinned/` — full source archive at the requested commit

Build artifacts were removed. The original repository’s pre-existing modifications remain untouched. No live targets were used.
