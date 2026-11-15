# parano1d adversarial audit — consolidated findings

Target: "parano1d" L1 (`noid_*` crates) by @ignotus_nemo, pinned commit
`52938433a30b185ab007afaa15166a632cf2e82e` (checkout at `/Users/ember/dev/parano1d-break/parano1d`,
treated as read-only reference). Audit run: kimi swarm (`session_506c8a03`), 2026-11-15, 01:10-03:03;
29 sub-agents, root session died on quota at 03:03. This document consolidates what the lanes and the
root actually established — no new claims. A separate verifier handles reproduction.

Certainty classes used throughout (never upgraded):

- **[RV] root-verified** — the root agent independently re-ran the PoC / rebuilt the builds and
  recorded confirmation in `experiments/kimi-root-narrative.md`.
- **[AR] agent-reported** — PoC output or a full agent report exists on disk, but the root's
  verification was interrupted or never started.
- **[PC] partial/unverified** — dead lane, code on disk, no report, no run evidence.

Corrections the swarm applied to its own earlier gist (from the fact-check lane,
`experiments/kimi-agent-reports/agent-3.md`, endorsed by the root at 01:27:41) are carried forward
here: `TAG_EXSTNOD` and `TAG_SEGMENTTREE` are **not** production users of the vulnerable flat
compressor; the 2^16.3 collision cost is a random-mapping heuristic, not a pigeonhole bound; padded
sponge finalization is *not* subfield-confined; the ideal-model theorems are not "logically false"
— the accurate criticism is that the model is unjustified for the shipped permutation.

---

## 1. Executive summary

Against the pinned commit, the swarm demonstrated **two cheap collisions in production compressor
domains** (`TAG_CAPSNODE`, 72,274 evaluations / 0.128 s; `IVCPCSN_`, 111,525 evaluations / 0.29 s —
both root-verified), a **prehashed Merkle commitment collision at the exact production source-tree
geometry** (depth 13, cap depth 3: two distinct leaf vectors, one eight-hash cap, both openings
accepted by the production prehashed Merkle verifier), **proof malleability at the legacy
`noid_fri` public verifier API** (distinct five- and six-lane final-codeword proofs verify for one
statement, with no collision search), an **exact same-anchor fork replay** (one real 323-byte
transaction and one real owner-auth bundle pass all epoch/logic/auth/state-delta checks on two forks
with different parents and state roots), a **reduction bound that provides only 41.65 bits at
T=2^32 and becomes vacuous near 2^41**, and a **soundness certificate whose decisive fixed-permutation
premise Delta_P2b is assumed (< 0.45067) and never evaluated**. These are real: primitive collision
resistance objectively fails at the raw node / prehashed-Merkle interfaces, and the ideal-model
justification for the shipped primitive does not hold.

What was **not** demonstrated, despite 16 completed attack lanes trying: **no unauthorized spend, no
invalid accepted block, no owner-key recovery, no fresh-request forgery, and no split-brain/state-
equivocation escalation.** Every route from the primitive collisions into protocol semantics hit a
guardrail: the exact UTXO state tree uses the other (two-permutation) compressor; capsule and
BaseFold verifiers recompute leaf hashes and pin depths and path lengths; fold equations reject
substituted openings (0 altered full PCS openings accepted); the caller sweep found no production
caller that authenticates attacker-supplied child digests without recomputing descendants. The
consistent escalation barrier is **valid leaf/descendant semantics for the colliding children**
(estimated 2^224 per random leaf to enter the aligned class), not more node-hash grinding. The
remaining confirmed protocol-level flaws are conditional or bounded: same-anchor replay is a replay
property (not a forgery; severity depends on whether authorization is meant to be fork-specific),
the HeaderDAG capacity stall is a Medium conditional liveness defect (requires ~1,024 valid PoW
headers), the unbounded `getSlotsByOwner` RPC query is local-only under the pinned credential
policy, and the P2P profile-ID collision across fork schedules yields incompatible-peer mixing /
DoS, not cross-version proof acceptance. The root's own position statement (02:58:28): *"serious
warning signs, no consequential end-to-end attack shown yet — not 'definitely broken,' but also not
'these results are satisfying.'"* The upstream `main` (`e6d324ff…`) still contains the vulnerable
compressor unchanged, and the upstream test suites pass — this is a design/parameterization failure,
not a broken build.

---

## 2. Confirmed findings (root-verified)

| ID | Finding | Demonstrated | Severity (as scoped by the lane/root) | Certainty | Reproducer |
|----|---------|--------------|----------------------------------------|-----------|------------|
| C1 | `TAG_CAPSNODE` flat feed-forward compressor collision | Two distinct aligned input pairs, identical production digest, 72,274 evals / 0.128 s | Primitive break (production authorization-PCS node hash); not a forgery | **[RV]** — root re-ran `poc/src/main.rs` and the hardcoded verifier | `/Users/ember/dev/parano1d-break/poc/src/bin/verify_collision.rs` |
| C2 | `IVCPCSN_` compressor collision (BaseFold/IVC Merkle domain) | 111,525 evals / 0.29 s; distinct raw openings verify through production `verify_merkle_proof`; multiproofs share a root; recursive path replay matches | High at primitive/raw-digest APIs; no end-to-end BaseFold break | **[RV]** — root rerun 01:44:12 | `/Users/ember/dev/parano1d-break/experiments/basefold-chains/src/main.rs` |
| C3 | Prehashed Merkle commitment collision at production geometry | Two distinct depth-13/cap-depth-3 leaf vectors -> identical 8-hash cap; both altered openings accepted by production Merkle verification | Collision-resistance failure at the prehashed API; leaf digests are supplied directly, not as valid `CAPSMIX` leaves | **[RV]** — root rerun 01:48:10 | `/Users/ember/dev/parano1d-break/experiments/capsule-chains/src/bin/merkle_collision.rs` |
| C4 | `CAPSMIX` leaf / `CAPSNODE` internal-node alias | Same digest authenticates as depth-0 leaf and depth-1 node vs one root when depth is caller-selected | Raw-API type confusion; production PCS pins depths | **[RV]** — root rerun 01:48:10 (same lane) | `/Users/ember/dev/parano1d-break/experiments/capsule-chains/src/bin/alias_depth.rs` |
| C5 | Legacy `noid_fri` proof malleability + transcript PAD ambiguity | Distinct 5- and 6-lane final-codeword proofs pass real `noid_fri::verify` for one statement; `absorb(x) == absorb(x, PAD)` on all three unframed channels | Real malleability at the legacy public verifier API; no current Wallet caller; BaseFold pins exact lengths | **[RV]** — root rerun 01:44:12 | `/Users/ember/dev/parano1d-break/experiments/transcript-chains/src/main.rs` (output: `output-release.txt`) |
| C6 | Same-anchor fork replay | Identical 323-byte tx + owner-auth bundle passes epoch, public-logic, owner-auth, and UTXO state-delta checks on two forks with different parents/state roots | Conditional protocol property; replay, not forgery; severity depends on fork-specificity intent | **[RV]** — root reran `run.sh` 01:40:01 | `/Users/ember/dev/parano1d-break/experiments/fork-replay/run.sh` (output: `output.txt`) |
| C7 | Reduction bound vacuity | Certificate's own numbers: 41.65 bits at T=S=2^32; vacuous near 2^41; ~5th-degree degradation in T | Weakness of the *proved bound*, not an attack algorithm | **[RV]** — root confirmed the bound from the artifact's data (01:10:56) | Artifact `data/security-bounds.json`; fact-check `agent-3.md` |
| C8 | Unevaluated Delta_P2b premise behind the certificate's `PASS` | Certificate prints `PASS` conditional on fixed-Poseidon2b Delta < 0.45067; Delta is never evaluated or bounded; a random permutation returns V_16^4 -> V_16^4 with prob 2^-448, production with prob 1 | Security-assurance failure: the hard instantiation problem is assumed away | **[RV]** — root analysis (01:27:41), with agent-3 definitional caveat below | `noid_soundness` certificate + `agent-3.md`, `agent-10.md` |
| C9 | P2P profile-ID collision across different fork schedules | Default, isolated-V2, isolated-V2.1 builds: identical profile ID `600c7311…6f85` and namespace `/noid/mainnet/860e70453390bf81/1`, different consensus rules | Domain-separation gap; demonstrated impact = incompatible-peer mixing / test-isolation / DoS; isolated builds are loopback-guarded | **[RV]** — root rebuilt all three feature variants (02:27:24) | `/Users/ember/dev/parano1d-break/experiments/version-domains/` (`results/network-comparison.json`) |
| C10 | HeaderDAG selected-ancestry capacity stall | At capacity, the next valid canonical child is rejected `Capacity`; reconciliation inserts before advancing finality, so it retries the failure; `header_dag_faulted` blocks suffix application | Medium **conditional liveness** defect; requires ~1,024 valid PoW headers; restart/finality recovers | **[RV]** — root reran the lane successfully (02:54:49) | `/Users/ember/dev/parano1d-break/experiments/ledger-fairness/` (`REPORT.md` §1) |
| C11 | Unbounded owner-query cost amplification | `paranoid_getSlotsByOwner` holds `chain.read()` and returns every UTXO; 4,096-UTXO fixture: root measured 6.16x a 32-record page (382 us vs 62 us median), agent 5.78x (347 us vs 60 us) | Low: **local-only** under the pinned RPC credential policy (`server.rs:3846`) — a local resource concern, not remote DoS | **[RV]** — root reran; root issued the local-only scope correction (02:57:14) | `/Users/ember/dev/parano1d-break/experiments/ledger-fairness/` (`REPORT.md` §2) |
| C12 | Mempool liveness policies (three Low behaviors) | First-seen output-slot pinning (higher-fee rival -> `SlotConflict`); coinbase/output-slot race evicting a pending spend; no automatic re-admission of reorged transactions | Low fairness/liveness policies, documented behavior; no soundness impact | **[RV]** — root reran the lane (same run as C10/C11) | `/Users/ember/dev/parano1d-break/experiments/ledger-fairness/` (`REPORT.md` §3-5) |
| C13 | Rejected proofs suppress same-scan recovery of the same txid | Malformed auth -> `InvalidProof` -> txid marked known; later valid same-txid intent from that peer skipped for that scan | Low, scan-bounded availability flaw; per-peer and per-scan only; new scan clears it; no mempool admission, no state change | **[RV]** — root called it confirmed (02:11:13) and independently scoped it (02:12:09) | `/Users/ember/dev/parano1d-break/experiments/identity-encoding/src/main.rs` |
| C14 | Merkle-layer typed-graph collision witness | The CAPSNODE collision lifted to 4 accepting production-shaped native Merkle paths + 4 matching recursive trace roots, sharing one source-cap node | Positive-but-limited: colliding children lack protocol-valid leaf openings; **not** `BadTypedBind`, **not** a Delta violation | **[RV]** — root rerun, "confirmed but limited" (01:40:01) | `/Users/ember/dev/parano1d-break/experiments/typed-binding/src/main.rs` |

### Per-finding detail and exact scope limits

**C1 — `TAG_CAPSNODE` collision.** Hardcoded-inputs verifier (no search, two production
`compress_flat_feed_forward_with_tag(TAG_CAPSNODE, …)` calls, `assert_ne!` on inputs, `assert_eq!`
on digests) at `/Users/ember/dev/parano1d-break/poc/src/bin/verify_collision.rs`. Root's recorded
output: `b2cf54a478131d318ebef1af2ec0e315245b8ee3af237a21e3fef04e4a20478b`. Cost figures: root
72,274 evaluations / 0.128 s (collision search); capsule-chains independently regenerated a
collision after the same 72,274 evaluations (0.192 s for the complete experiment) —
`experiments/capsule-chains` (agent-4). Preconditions: attacker-chosen inputs in the aligned
`V16^2 x V16^2` class; expected birthday work ~2^16.33 under the random-mapping heuristic (agent-3:
pigeonhole alone guarantees a collision only by 2^32+1 evaluations). Scope limits, all root-stated
(01:10:56) or fact-check-confirmed: the exact UTXO state tree uses the two-permutation
`compress_with_tag(TAG_EXSTNOD, …)` and is unaffected; `TAG_SEGMENTTREE` has no production call
site; this is a collision in the production node hash used by the authorization PCS, **not** an
authorization forgery ("The accurate claim is 'production TAG_CAPSNODE collision,' not 'forged
authorization'" — root, 01:15:55).

**C2 — `IVCPCSN_` collision.** Agent-5
(`/Users/ember/dev/parano1d-break/experiments/basefold-chains/src/main.rs`, final line
`status=pass mode=all collision_first_at=15641 collision_evaluations=111525`); digest
`f1ca994e8a1b9a79e5da33ad7015b44940e5b5d95739c87a3ee5ba1b6b32c8b6`. Both distinct openings verify
through production `verify_merkle_proof`; Ligerito-style multiproofs share a root; the recursive
`build_ff_merkle_path_columns` replays both witnesses to the same root; a left fixed point
`H(a,b)=a` exists; the raw API accepts path lengths 0/1/2 for the same root. Root rerun confirms
(111,525 evaluations / 0.29 s; agent timing 0.205 s — same evaluation count, different runs).
Limits (agent-5, root-endorsed 01:44:12): full BaseFold C0/C1 verification rejects everything
tested — collision bytes as an actual 32-byte initial leaf -> `InitialMerkleFailed`; shortened or
fixed-point-extended paths -> `InvalidProofShape`; reordered queries -> `FoldMismatch`. The fixed-IV
leaf hash keeps leaf digests out of GF(2^16), so the node collision does not lift into codeword
trees.

**C3 — prehashed Merkle commitment collision at production geometry.** From
`/Users/ember/dev/parano1d-break/experiments/capsule-chains/src/bin/merkle_collision.rs` (agent-4):
two distinct **depth-13, cap-depth-3** leaf vectors — the exact source-tree geometry of
`zk_capsule_pcs.rs` — produce the identical eight-hash commitment, and production Merkle
verification accepts both distinct openings (`same cap: true … both openings accepted: true`).
Substituting an actual production `CAPSMIX` leaf digest instead fails
(`source batched Merkle cap mismatch`). Root reran and confirmed (01:48:10): "two distinct
prehashed leaf vectors with the exact production source-tree geometry—depth 13, cap depth 3—produce
the same eight-hash commitment, and production verification accepts both altered openings."
Precision statement (root, same entry): *"We have a production Merkle commitment/opening collision
at the correct tree geometry, but not valid CAPSMIX preimages for the colliding children and not a
full capsule attack."* A random 256-bit `CAPSMIX` digest enters the aligned class with probability
2^-224 (agent-4).

**C4 — `CAPSMIX` leaf/internal alias.** `alias_depth.rs`: a real `CAPSMIX` leaf digest
(`8f896f52226dfede76e85ea49a622eedcba1aeae61a6b2cbc2f965adcf2c9241`) equals
`CAPSNODE(0, X)`; the production Merkle verifier accepts a depth-0 leaf proof and a depth-1
internal-node proof against the same root. Cross-depth attempts fail
(`insufficient source siblings at layer 0` / `source batched Merkle cap mismatch`); neither
256-bit fixed-point equation exists; a zero-left aligned collision (4,327 evaluations) does not
chain with the alias. Root reran (01:48:10, same lane reproduction).

**C5 — legacy `noid_fri` malleability.** From
`/Users/ember/dev/parano1d-break/experiments/transcript-chains/output-release.txt`:
`honest four-lane proof verifies; NUM_QUERIES=64` / `five-lane verifier: Ok; six-lane verifier: Ok`
/ `identical query set (64 draws, 53 distinct)`. Mechanism: partial-block padding
`PAD = 0x01000000000000000000000000000080` is itself a valid explicit lane, so
`absorb(x) == absorb(x, PAD)` on `Poseidon2bChannel`, `Poseidon2bWideChannel`, and `noid_fri::Channel`;
the verifier checks the final codeword for nonempty, not exact length
(`noid_fri/src/verifier.rs:114-121`), and every final index stays in 0..4, so the extra lanes never
affect evaluation. No collision search; cost is rebuilding five round trees plus regenerating
openings. Root rerun (01:44:12): "two distinct final-codeword encodings verify for the same
statement. This is genuine proof malleability at the legacy public verifier API, but no current
Wallet caller was found, and newer BaseFold pins exact lengths." It is proof malleability, not a
false-statement proof (the collision occurs after statement absorption). The framed newer
challengers correctly reject the ambiguities (`framed lane control: adding observe_f128(PAD) does
NOT collide`). Same lane also measured ordinary query-grinding (a real wide-channel nonce search
selected a duplicate-containing query set; nonce not required to be minimal) — grinding, not a
bypass.

**C6 — same-anchor fork replay.** Verbatim from
`/Users/ember/dev/parano1d-break/experiments/fork-replay/output.txt`:
`replay_result: same_transaction=true same_proof=true both_fork_context_checks_passed=true`,
`same_fork_second_spend: … state_delta=InputMismatch { tx_index: 0, input_index: 0 }`,
`changed_epoch_control: epoch_check=BadEpochAnchor owner_auth=AuthProof`,
`scope: exact transaction/proof replay across same-anchor forks; no new proof forged`.
Root independently re-ran `run.sh` (01:40:01) and confirmed byte-identical behavior. Scope (root +
agent-7): local composition of real validation APIs — no PoW, chainwork, transaction roots,
recursive `HistoryStep` verification, mempool, nodes, or peers; no transaction modification, no
proof creation without the owner's witness, no cross-chain replay (the epoch anchor blocks
different-genesis networks). Root's severity framing (01:44:12): "a real replay property …
short competing forks often reuse transactions by design, and finality leaves only one canonical
spend. It becomes a clear protocol flaw if authorization is supposed to be fork-specific, or if
independent networks can share epoch anchors." Root also notes (02:01:00) this replay is
**unrelated to the collisions** and visible, not a hidden semantic split.

**C7 — bound vacuity.** From the artifact's own `data/security-bounds.json` (agent-3, root-endorsed):
T=2^32 gives 44.96 bits (S=1) and 41.65 bits (S=T); at T=2^41 extraction clips to 1 and the
combined bound is vacuous; degradation is ~fifth degree in T. Fact-checked corrections: T counts
adversary, simulator, verifier, constructor, recheck, inverse, and nonce-search base calls; a
vacuous bound is loss of provable reduction security, **not** a 2^41-query forgery; the earlier
asymptotic denominators quoted in the gist were wrong and should not be used.

**C8 — unevaluated Delta premise.** Root (01:27:41): Delta_P2b is "the residual fixed-permutation
compiler failure that their ideal model fails to capture, and the certificate never evaluates or
bounds it. It prints `PASS` *if you assume* Delta<0.45067." Definitional caveat (root 01:27:41 +
agent-3): Delta_P2b is explicitly **not** ordinary qPRP-distinguishing advantage, so the one-query
subfield distinguisher alone does not break the numeric premise; the open question is whether a
`CAPSNODE` collision itself qualifies as their `BadTypedBind` event (see §5). Root's recommended
wording: "Your executable certificate checks the arithmetic after the difficult fixed-permutation
instantiation problem has been assumed away. The production collision is evidence that this
unevaluated premise cannot be silently treated as a small random-permutation error."

**C9 — P2P profile collision.** From
`/Users/ember/dev/parano1d-break/experiments/version-domains/results/network-comparison.json`:
`"result": "identical P2P preflight identity for three pairwise different consensus schedules"`,
profile ID `600c7311c529b2a8547263851007924e5269f68881c1052bab4456c384fc6f85`, namespace
`/noid/mainnet/860e70453390bf81/1` shared by default (V2.1 activation 279370), isolated-V2 (10),
isolated-V2.1 (24) builds; at height 24 the rules are terminal v4/4 classes/owner V3 vs terminal
v7/2 classes/owner V3 vs terminal v7/3 classes/owner V4. Root rebuilt all three variants and
confirmed (02:27:24). Impact (root + agent-15): mixed binaries pass transport preflight and
exchange incompatible artifacts — test/fork isolation and denial of service; the isolated builds
are loopback-guarded; all tested cryptographic downgrades (V3<->V4 owner proofs, sparse/matrix
proofs, terminal metadata, snapshot boundaries) **reject**. Present on `origin/main` (`e6d324ff…`).

**C10-C12 — ledger fairness/liveness.** Root re-ran the lane: "Reproduction for the ledger results
is in experiments/ledger-fairness; I independently reran it successfully" (02:54:49). HeaderDAG
(`noid_node/src/networking/header_dag.rs:287`): capacity rejection of the next valid child when the
selected ancestry is full; production reconciliation inserts the missing canonical child *before*
advancing finality (`noid_node/src/main.rs:4779-4805`), so the failure blocks the pruning that
would resolve it; production capacity is 1,024. Owner query: `paranoid_getSlotsByOwner`
(`parano1d/noid_rpc/src/server.rs:2025`) holds the chain read lock; measured amplification root
6.16x (382 vs 62 us median) / agent 5.78x (347 vs 60 us median) on a 4,096-UTXO fixture;
**root scope correction (02:57:14): local-only under the pinned credential policy
(`server.rs:3846`) — remote mining/operator credentials cannot invoke it.** The three Low mempool
behaviors (first-seen slot pinning, coinbase/output-slot race, no reorg re-admission) are in
`REPORT.md` §3-5 with preconditions. The lane's own bottom line: "No invalid state transition was
accepted and no soundness failure was demonstrated … not a deterministic, low-cost, network-wide
consensus DoS."

**C13 — recovery suppression.** Verbatim from agent-16's PoC:
`RECOVERY_BUG … admission=InvalidProof pool_contains=false current_scan_known=1 fresh_scan_known=0`.
Root (02:12:09) independently confirmed the scoping: `observed_txids` is per peer-response, merged
into that peer's `state.seen`, the malformed transaction is not admitted, a new scan resets the
exclusion — "a low-severity recovery-bookkeeping bug, not a network-wide transaction-processing
shutdown."

**C14 — typed-binding witness.** Verbatim (agent-10):
`native_paths_accepted=4 … recursive_trace_roots_matched=4 … common_siblings_have_capsmix_leaf_openings=true
colliding_child_leaf_openings_supplied=false full_wallet_or_history_acceptance_demonstrated=false
bad_typed_bind_instantiated_by_supplied_artifact=false delta_violation_proven=false`.
Root (01:40:01): "confirmed but limited." Agent-10's definitional finding (root-relevant to C8):
terminal/genesis reachability is **not** additionally required for `BadTypedBind`; what is missing
is protocol-valid represented subgraphs — valid leaf/descendant openings for the colliding children
plus canonical represented artifacts.

---

## 3. Reported-but-unverified

PoC output and/or a full agent report exists; the root's independent verification was interrupted
(02:43-03:03 quota death) or never started. None of these may be cited as root-confirmed.

### U1 — build-artifacts "High": legacy matrix bytes substitutable without changing approved metadata identity **[AR]**

Source: `/Users/ember/dev/parano1d-break/experiments/kimi-agent-reports/agent-25.md` (full 7.4 KB
report; the root died mid-verification of this lane). Claim: `scripts/build_release.sh:143-149`
validates layout and reads `pins.env` but only uses the metadata digest — it never calls
`release_authenticate_pack` or `release_verify_sha256_manifest` and discards
`NOID_HISTORY_STEP_PACK_LEAF_DIGESTS`; `noid_node/build.rs:388-470` stages each legacy matrix by
bounds/decode/length only, never hashing the compressed bytes against the reviewed leaf pins.
Probe (`/Users/ember/dev/parano1d-break/experiments/build-artifacts/probe_release_inputs.py`,
results `probes/release-inputs.json`) replaced a legacy matrix with different valid Zstandard bytes
of the same decompressed length:

```text
cargo_reached=true
legacy_leaf_pin_exported_to_cargo=false
explicit_manifest_verifier_exit=1
error: SHA-256 mismatch ... history-step-c00.field-r1cs.zst
```

**Scope caveats that must travel with this finding** (agent-25's own limitations): the
release-script probe used a **Cargo stub** (exit 91) and synthetic 1,024-byte blobs — not a real
foreign R1CS and not a full production release build; "Consensus impact beyond those directly
demonstrated trust failures is therefore inferred from the verifier paths." Affects builds
retaining legacy matrices, not the default `retired-history` profile; `publish_release.sh` has an
additional `release_authenticate_pack` gate.

### U2 — build-artifacts "Medium": packed runtime cache accepts a foreign relation under the approved digest **[AR]**

Same report; `probes/results.json` records
`foreign_image_accepted=true approved_digest_retained=true safe_loader_rejected=true canonical_lengths=210 image_lengths=391`.
This probe used small valid same-shape relations and the **actual pinned decoder**
(`noid_ivc_core/src/field_r1cs.rs:3986-4005`, which deliberately omits the structural digest
check). Precondition: write access to the node data directory or a previously poisoned build/cache —
which the agent itself says "limits severity."

### U3 — build-artifacts "Medium": inherited Cargo environment can flip release to debug verifier constants **[AR]**

`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` passes through `build_release.sh` (which clears
`RUSTFLAGS` but not profile overrides): probe shows
`normal: debug_assertions=false fri_queries=64 capsule_queries=64 capsule_grind_bits=16` vs
`overridden: debug_assertions=true fri_queries=10 capsule_queries=8 capsule_grind_bits=6`.
"No evidence the checked-in CI currently sets this override."

### U4 — recursive-sync: snapshot staging checks future drift only on the tip **[AR]**

Source: `/Users/ember/dev/parano1d-break/experiments/recursive-sync/poc-output.txt` (agent-13; root
said "I'm rerunning that PoC too" at 02:26:05 — the rerun was never recorded):

```text
native_block height=3 timestamp=1787330120 result=BadTimestamp
snapshot_timeless_header_stage= sealed height=4 tip_future_drift=pass intermediate_height_3_native=BadTimestamp
```

Scope: **wall-clock admission mismatch, not an invalid-state proof** — an otherwise valid height-4
`HistoryStep` terminal is an explicit, assumed precondition (no production-sized terminal was
generated). Negative results from the same PoC (changed terminal root rejected, exact-root and
native-sidecar binding enforced) are agent-reported by the same caveat.

### U5 — external-miner: invalid PoW consumes the only prepared template **[AR, special case]**

Source: `/Users/ember/dev/parano1d-break/experiments/external-miner/README.md` +
`results.txt` (complete on disk, `ALL ASSERTIONS PASSED`). `submitBlock` consumes the node-owned
attempt (`begin_proving` -> slot `Proving`) *before* `validate_pow`; on `InvalidPoW` the slot returns
to `Idle` and the template is gone (`replay=Unavailable; expensive_template_reusable=false`).
Agent-assigned severity: Medium availability, requiring mining-API authorization. **Certainty
caveat:** per `PROVENANCE.md`, "external-miner finished on disk before its report reached the
root" — the agent's final message never landed (its last wire line is a progress note: "All final
assertions pass. I'm doing the final scope check now"), and the root never verified or summarized
this lane before dying.

### U6-U9 — other agent-reported results awaiting root-style confirmation

- **U6** state-consensus 20-case adversarial UTXO/state-machine matrix — all rejected
  (`/Users/ember/dev/parano1d-break/experiments/state-consensus/output.txt`, agent-11). The root
  said at 02:12:55 it was rerunning both harnesses, but only the contract audit's rerun was
  recorded as confirmed (02:16:23); this matrix's rerun was not.
- **U7** block-validation matrix + informational standalone-utility gap
  (`/Users/ember/dev/parano1d-break/experiments/block-validation/results.txt`, agent-12):
  `POC standalone validate_block_consensus ACCEPTED wrong user epoch anchor; production boundary
  rejected it` — the public utility omits epoch-anchor enforcement, but production
  `MdbxChainContext` rejects before terminal verification and no production caller was found.
  Root rerun ("I'm rerunning its matrix as well", 02:11:13) not recorded.
- **U8** identity-encoding auxiliary results (agent-16): proof-byte malleability does not create a
  second txid or replace a cached entry (`second=AlreadyAdmitted … retained=proof-A`); a legacy-FRI
  mutation would keep the block hash but get a distinct `TerminalObjectId`
  (`same_claim=true distinct_object_ids=true`); Bech32 uppercase/whitespace aliases are a textual
  canonicality issue only. Only the recovery-suppression finding (C13) received root confirmation.
- **U9** UC-composition informational assurance gap (agent-17,
  `/Users/ember/dev/parano1d-break/experiments/uc-composition/REPORT.md`): distinct concurrent
  proofs for one statement verify in independent sessions
  (`both_proofs_accept_in_two_sessions=true both_proofs_reject_changed_context=true`) — which does
  **not** violate the manuscript's fresh-intent game; statement-scoped acceptance with
  `dedicated_session_salt_bits: 0` and one-shot post-hoc extraction are an assurance gap, not a
  false theorem or bypass. Root said it was rerunning the concurrent-proof experiment (02:22:42);
  the rerun was never recorded.

---

## 4. Verified negatives — where the system held

These are ammunition too: they localize exactly which guardrail blocks each escalation. Certainty
noted per row.

| Attack attempted | Guardrail that blocked it | Certainty |
|---|---|---|
| Collision-driven state-tree equivocation / split-brain | Exact UTXO tree uses two-permutation `compress_with_tag(TAG_EXSTNOD)`; `TAG_SEGMENTTREE` has no production call site; block application checks a freshly computed exact post-state root; native materialization recomputes the exact root | [RV] (agent-1 analysis, root-endorsed 01:10:56/02:01:00) |
| Altered full capsule PCS openings (source/mid substitution, alias, chained, shortened caps) | Verifier recomputes `Hmix`/`H256` leaf hashes and enforces source->mid->tail fold equations *before* Merkle checks; cap widths pinned (depth 13/cap 3 source, 9/3 mid); **0 altered openings accepted** | [RV] (root rerun 01:48:10) |
| Leaf/internal-node depth games | Production PCS pins depths; verifier iterates exactly the derived depth and rejects missing/extra siblings; multiprover sorts/dedupes indices and recomputes every parent | [RV] (root reruns 01:44:12, 01:48:10) |
| Lifting `IVCPCSN_` collision into BaseFold/Ligerito codewords | `hash_leaf` recomputed at live call sites; exact final lengths pinned; collision bytes as real leaves -> `InitialMerkleFailed`; reordered queries -> `FoldMismatch` | [RV] (root rerun 01:44:12) |
| Caller authenticating attacker-supplied child digests | Root's caller sweep: "the live BaseFold/Ligerito call sites recompute leaf hashes, and the other recursive hits are tests" — no unguarded production path found | [RV] (root's own check, 01:50:29) |
| Address preimage / owner-key recovery | `ADDRFIX` IV lanes require GF(2^128)/GF(2^64), blocking lane-wise subfield closure; no nontrivial Frobenius fixes both IV lanes (0/112 equivalent keys); generic cost stays 2^256 classical / 2^128 Grover; only a 2^-240-probability low-entropy weak-key class is exhaustible | [RV] (root: "complete and independently reproduced", 01:44:51) |
| Builder-controlled worst-fill trade (minimum output / maximum fee) | Host block/template preparation and the recursive relation both reject fabricated executions (`minimum-output substitution rejected: Wire(Trade(Outputs))`) | [RV] (root rerun 01:44:12) |
| Unauthorized contract action / Pruv-vs-native mismatch | 50,000 generated Pruv programs, 428,880 executions, 0 mismatches; 1,115 legacy/v2.1 relation checks agree; replay/allowance/vault/retargeting all rejected | [RV] (root: "full contract audit is now independently confirmed", 02:16:23) |
| Cross-version proof acceptance | V3-as-V4 and V4-as-V3 owner proofs -> `AuthProof`; sparse/matrix proof substitutions -> `Algebra`; terminal downgrades rejected; snapshot terminal binds its staged boundary | [RV] (lane root-verified via C9 rebuilds; negative matrix agent-15) |
| Transcript PAD ambiguity escaping to framed channels / cross-channel replay | Framed op/kind/length headers reject the raw PAD lane; `KSCHANNL`/`KSCH256`/`FRICHANL` have distinct capacity IVs; wallet capsule shapes fixed before replay | [RV] (lane whose malleability half the root reran; negatives agent-6) |
| Cross-protocol digest reinterpretation via adapters | Paged-spend `PAGEDTX_` version/page binding; `TRINT_21` binds full policy; native bits 14/15 outside ordinary validity mask; generic vs financial verification not interchangeable; domain audit found 71 distinct labels / 103 uses, no accidental sharing (one research-only `POLICY__` reuse caveat) | [AR] (agent-17; root rerun not recorded) |
| UTXO/state-machine conservation & authorization (20 adversarial cases, real PoW) | Semantic/conservation/authority/slot/coinbase/header-counter checks — every case rejected; two valid controls passed | [AR] (agent-11; root rerun not recorded) |
| Block/template acceptance mutations (roots, coinbase, fees, header, proofs) | Full rejection matrix; production epoch boundary rejects before terminal verifier | [AR] (agent-12; root rerun not recorded) |
| Cheap network-wide DoS via malformed proofs | Cheap state/fee/conflict checks before proof verification; concurrent verification capped; payload dedup; shared inbound/outbound memory budgets | [RV] (root first-pass design analysis 02:54:49) |
| Same-fork double spend / changed-epoch replay | Second spend -> `InputMismatch`; changed epoch -> `BadEpochAnchor`/`AuthProof` | [RV] (root rerun 01:40:01) |
| Fresh-request forgery (all lanes) | Owner/Phase-A/Phase-B adaptive ordering; queries derived only after all commitments; no accepted unauthorized statement anywhere | [RV] (root, cumulative: "no unauthorized spend, invalid accepted block, owner-key recovery, or fresh-request forgery", 01:58:28) |

---

## 5. Open escalation paths (the root's stated finish lines)

From the root (01:50:29, 01:56:34, 01:58:28), in its own priority order — the plausible finish is
**not** more birthday grinding:

1. **Valid `CAPSMIX` preimages for the colliding `TAG_CAPSNODE` children.** The "leaf-reachability
   gap": construct protocol-valid leaf/descendant openings in the aligned class (generic random
   entry ~2^224 per leaf), then inject the collision on an actual depth-13 authenticated path.
   This is also the missing object for a `BadTypedBind` witness (C14) and therefore the direct
   route to testing whether the CAPSNODE collision fails their Delta < 0.45067 condition (C8).
2. **A caller that authenticates attacker-supplied child digests without recomputing descendants.**
   The root's caller sweep found none in current live callers — but the unsafe raw/prehashed APIs
   remain public and accept authentication objects the semantic protocol would reject; a future
   caller, SDK consumer, cross-protocol bridge, or recursive/native mismatch could bypass the
   guardrails (root, 01:50:29).
3. **Cross-protocol digest reinterpretation** — one valid artifact acquiring different meanings in
   two protocols/sessions/forks sharing the global permutation (root 01:54:09; the UC lane's
   adapter checks are the current negative baseline).

Secondary unfinished threads: a live production caller for the legacy-FRI malleability (C5);
a valid height-4 `HistoryStep` terminal to close the snapshot drift differential (U4); root-style
verification of the build-artifacts triple (U1-U3) and the external-miner finding (U5); and the
root's own framing question — whether authorization is *supposed* to be fork-specific — which
decides C6's severity.

---

## 6. Lane status table

All 25 experiment directories under `/Users/ember/dev/parano1d-break/experiments/`. "Completed"
= lane returned a final result the root processed (verification status per §2/§3). The 8
dead-with-partial-code lanes are being re-run by the takeover swarm.

| Lane dir | Agent report | Status | Result summary |
|---|---|---|---|
| `address-preimage` | agent-9 | **completed [RV]** | Negative: no unrestricted key recovery; weak-key class only (`results.txt`: `conclusion=low_entropy_weakness_only_not_a_uniform_key_attack`) |
| `basefold-chains` | agent-5 | **completed [RV]** | C2: `IVCPCSN_` collision; no end-to-end BaseFold break |
| `block-validation` | agent-12 | **completed [AR]** | Negative matrix + informational standalone-utility epoch gap (U7); root rerun not recorded |
| `build-artifacts` | agent-25 | **report-only [AR]** | U1-U3 (High + 2 Medium); root died mid-verification |
| `capsule-chains` | agent-4 | **completed [RV]** | C3, C4; full PCS rejects everything (`Accepted altered full PCS openings: 0`) |
| `contract-rights` | agent-14 | **completed [RV]** | Negative: 50k Pruv programs / 428,880 executions, 0 mismatches |
| `dex-economics` | agent-26 | **dead-with-partial-code [PC]** | Mid-flight progress note only (fill-validation correction); `results.json` empty; re-run pending |
| `external-miner` | agent-20 | **completed-on-disk [AR, U5]** | Template-consumption finding complete in `README.md`/`results.txt`; agent's final report never reached the root; unverified |
| `fork-replay` | agent-7 | **completed [RV]** | C6: same-anchor fork replay, byte-identical reruns |
| `identity-encoding` | agent-16 | **completed [RV/AR]** | C13 root-verified; proof-identity/terminal-id/Bech32 results agent-reported (U8) |
| `ledger-fairness` | agent-18 | **completed [RV]** | C10-C12; root reran successfully |
| `native-intents` | agent-8 | **completed [RV]** | Negative: worst-fill accusation disproved at both host and recursive boundaries |
| `nft-marketplace` | agent-27 | **dead-with-partial-code [PC]** | Two borrow-checker errors fixed, scenarios starting; no results; re-run pending |
| `p2p-resources` | agent-21 | **dead-with-partial-code [PC]** | One partial PoC result in the progress note (64 gated writers retained ~110 MB outside the shared 64 MB permit domain) — unverified; re-run pending |
| `prover-unsafe` | agent-24 | **dead-with-partial-code [PC]** | Progress note claims Miri/release/loader checks complete; no report, no artifacts; re-run pending |
| `recursive-sync` | agent-13 | **completed [AR]** | U4: tip-only future-drift admission differential; root rerun not recorded |
| `rpc-api` | agent-19 | **dead-with-partial-code [PC]** | Design note only (pagination heap isolation plan); no results; re-run pending |
| `state-consensus` | agent-11 | **completed [AR]** | Negative 20-case matrix (U6); root rerun not recorded |
| `storage-reorg` | agent-22 | **dead-with-partial-code [PC]** | Partial PoC (41-byte tip accepted at reopen; successor failed at MDBX decode); no report; re-run pending |
| `token-accounting` | agent-28 | **dead-with-partial-code [PC]** | Setup only (`pinned/` tree, no own code); re-run pending |
| `transcript-chains` | agent-6 | **completed [RV]** | C5: legacy-FRI malleability + PAD ambiguity; framed channels hold |
| `typed-binding` | agent-10 | **completed [RV]** | C14: 4-path witness; `BadTypedBind` not instantiated |
| `uc-composition` | agent-17 | **completed [AR]** | U9: informational assurance gap only |
| `version-domains` | agent-15 | **completed [RV]** | C9: profile-ID collision across schedules; downgrades reject |
| `wallet-safety` | agent-23 | **dead-with-partial-code [PC]** | Progress note describes a passing stale-wallet repro (key B replaces A on disk; loaded A wallet still authorizes, writes A's address into B's history) — unverified; re-run pending |

Not in the table (no experiment dir — read-only analysis lanes whose conclusions the root carried
forward): agent-0 (capsule PCS escalation boundary), agent-1 (state-tree equivocation), agent-2
(transcript PCS audit, negative), agent-3 (fact-check of the earlier gist — source of the
corrections in the header of this document). The root's own artifacts: `poc/src/main.rs`
(collision search) and `poc/src/bin/verify_collision.rs` (hardcoded verifier).

Provenance note: `PROVENANCE.md` counts "20 lanes completed, 10 died" across the 29 agents, counting
external-miner among the dead agents (its artifacts finished on disk before its report reached the
root) and agent-25 among the completed reports. The table above classifies by lane directory and
verification status instead; no finding's certainty differs between the two accountings.

Key source files: `/Users/ember/dev/parano1d-break/experiments/kimi-root-narrative.md` (certainty
authority), `/Users/ember/dev/parano1d-break/experiments/kimi-agent-reports/agent-{0..28}.md`,
per-lane artifacts cited inline, `/Users/ember/dev/parano1d-break/poc/src/bin/verify_collision.rs`,
`/Users/ember/dev/parano1d-break/PROVENANCE.md`.
