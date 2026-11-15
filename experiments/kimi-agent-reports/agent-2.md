# agent-2: Audit transcript PCS

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Result: no concrete bug found

I found no substantiated fresh-request acceptance without the owner secret, and no other concrete high-severity soundness defect beyond the excluded Poseidon2b Merkle-subfield issue. Accordingly, there is no exploit precondition/cost or minimal reproduction to report.

The audit covered:

- **Request/address binding and Fiat–Shamir ordering.** Transaction hash and owner address are reconstructed from the request in `noid_gkr/src/wallet_authorization.rs:642-658` and `noid_gkr/src/wallet_authorization.rs:685-701`. The statement and source commitment are absorbed before Owner challenges in `noid_gkr/src/zk_authorization.rs:393-407` and `noid_gkr/src/zk_authorization.rs:621-666`. Sigma, Phase-A rounds, upper table, mid commitment, tail, nonce, and query seeds follow the required adaptive order in `noid_gkr/src/zk_authorization.rs:1320-1378`.
- **Owner MLE-check, Phase A, and Phase B.** Constant recovery and terminal unbatching are enforced in `noid_gkr/src/zk_mlecheck.rs:488-539`. Phase A recomputes its initial claim and terminal product in `noid_fri_binius/src/zk_phase_a.rs:411-445`. The Phase-A/Phase-B upper/tail linkage is checked in `noid_gkr/src/zk_authorization.rs:1021-1045`.
- **PCS openings and queries.** Every derived query is range-checked and mapped into both trees; source-to-mid and mid-to-tail relations are checked before both Merkle multiproofs in `noid_fri_binius/src/zk_capsule_pcs.rs:923-1048`. Duplicate queries retain query-major openings but deduplicate Merkle leaves consistently.
- **Recursive verifier.** Native proofs are reverified before preparation in `noid_recursive/src/acceptance/trace/zk_authorization_candidate.rs:545-583`; prepared statements are checked against canonical slots at `noid_recursive/src/acceptance/trace/zk_authorization_candidate.rs:704-718`. Body statements are pinned to transcript cells at `noid_recursive/src/acceptance/trace/zk_authorization_candidate.rs:2006-2017`, and the Owner/Main bridge is pinned at `noid_recursive/src/acceptance/trace/zk_split_bridge.rs:231-273`. The recursive schedule matches native commit/challenge ordering in `noid_recursive/src/acceptance/zk_auth_capsule_schedule.rs:225-318`.
- **Canonical decoding.** The proof decoder prescans all six variable-length fields, rejects trailing bytes, and rechecks decoded shape in `noid_gkr/src/zk_authorization_wire.rs:286-403` and `noid_gkr/src/zk_authorization_wire.rs:443-452`. Outer bundle decoding is exact-length in `noid_gkr/src/wallet_authorization.rs:117-155`; paged and native intent decoders likewise enforce complete canonical consumption in `noid_tx/src/paged_spend.rs:199-237` and `noid_tx/src/native_assets/wire.rs:724-775`.

This was a read-only source audit; no files were modified.
