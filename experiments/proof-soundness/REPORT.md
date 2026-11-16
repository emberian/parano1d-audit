# Proof authority and version routing: fresh source review

Target: `parano1d/` pinned at `52938433a30b185ab007afaa15166a632cf2e82e`.
This is an independent code trace. I did not run a prover, change the target,
or touch the live network. Earlier experiment results were treated as leads,
not evidence for a new conclusion.

## Verdict

No new unauthorized-spend, proof-bypass, or consensus split was established.
The direct attack ideas below met explicit gates in the pinned production path.
This is a scoped negative result, not a proof that the large recursive circuit
is sound. The most important unresolved attack question remains **whether the
actual release matrix and recursive verifier enforce the same transition as
native block validation for every adversarial body at the v2.1 boundary**.

## Authority path checked

1. A public wallet authorization is for the exact transaction body hash and
   expected address (`noid_gkr/src/wallet_authorization.rs:642-658,739-763`).
   Paged spends first derive the canonical logical txid and owner from the
   complete page group (`:685-708`). `verify_zk_authorization_for_version`
   preflights the proof shape, verifies the owner statement, then replays the
   phase A/B transcript and PCS openings (`noid_gkr/src/zk_authorization.rs:
   1313-1390`). The public version is selected by candidate height
   (`noid_chain/src/consensus/params.rs:47-61`), and the production wallet
   paths call that selection (`noid_node/src/wallet/mod.rs:692,1456,1590`).

2. The producer's selected ZK authorizations are native-verified against the
   slot statements before the recursive witness is built. It checks exact
   proof and source-commitment reuse and keeps the results in a private
   capability (`noid_recursive/src/acceptance/trace/
   zk_authorization_candidate.rs:458-499,545-581`). The later region builder
   asserts the statements match the canonical slot and the version matches
   the class geometry (`:699-729`). This is producer discipline; it cannot
   alone establish verifier soundness for a malicious producer.

3. An inbound terminal's metadata is capped and bound to its claimed tip
   before its verifier callback can mint `VerifiedHistoryStepTerminal`
   (`noid_chain/src/storage/mdbx_context.rs:201-255`). The node's callback
   calls `HistoryProtocolRuntime::verify_terminal` and fails when the
   embedded runtime is absent (`noid_node/src/main.rs:4237-4254`). The
   runtime selects legacy, v2 or v2.1 by **candidate height** and each v2
   branch decodes, resolves a verified origin certificate, and invokes the
   schedule's verifier (`noid_miner/src/history_protocol.rs:727-760`).
   The v2 verifier checks origin against the bank, validates the header/epoch
   boundary, replays the recursive proof and checks its matrix claims
   (`noid_recursive/src/acceptance/history_step/v2.rs:988-1007`).

4. The preverified terminal capability does not by itself commit a suffix.
   The mutable context rebinds it to the current canonical base and persists
   a recovery record under its sole-writer boundary
   (`noid_chain/src/storage/mdbx_context.rs:2508-2546`). Reorg authorization
   checks depth and finalized ancestry again before using it (`:2567-2601`).

## Attack ideas tried by source trace

| Idea | Observed gate | Confidence |
| --- | --- | --- |
| Reuse a proof for a different body or recipient | Authorization statement carries body hash and address; complete page group derives logical txid; verifier transcript uses that statement. | Code path traced; no forged-proof experiment. |
| Send a v3/V4 proof through the wrong scheduled bank | Candidate height selects owner-auth version and terminal runtime; recursive geometry asserts the chosen version. | Code path traced; a cross-version byte-level rejection test would strengthen this. |
| Substitute an origin certificate in a terminal | `verified_origin` retrieves and authenticates the certificate, then compares the resulting origin to the requested one (`noid_miner/src/history_protocol.rs:637-656`). | Code path traced; no forged-certificate experiment. |
| Exploit a metadata-only terminal or absent verifier | Metadata only reaches the callback; production callback requires an embedded runtime and verifies the full terminal. | Code path traced. |
| Have a stale verified terminal authorize a new reorg | The context rechecks ancestor depth/finality and derives authority against its current base. | Code path traced; race/rollback test remains valuable. |
| Allocate arbitrarily from a malformed owner proof | Wire size caps at 92,696 bytes; a hand-written preflight checks vector lengths before bincode (`noid_gkr/src/zk_authorization_wire.rs:55,286-450`). | Code path traced; not a fuzz or peak-memory measurement. |

## Higher-value next experiments

1. **Cross-implementation differential:** generate fully valid v2.1 blocks
   with adversarial but structurally valid transaction/body variants. Compare
   native state transition, released `HistoryStep` terminal, and an independent
   receiving node. Mutate every public field around the v2.1 activation
   height. This directly targets user funds and consensus divergence.
2. **Release artifact substitution:** C16 already found a standalone build
   authentication gap. The next useful test is to compare behavior of an
   official-release-style build and a single-byte altered matrix under the
   same valid/invalid block corpus, with the byte identity of each runtime
   recorded. This separates build provenance from verifier semantics.
3. **Concurrent verification/reorg:** pause after a terminal verifies off
   thread, change the selected base, and resume. Assert it cannot authorize
   the new base or leave a stale crash-recovery record. Code has explicit
   generation checks; a forced interleaving is the refutation test.

These tests are substantial builds. I did not start one while the shared host
was running another lane's phase-8 build. No new severity is claimed here.
