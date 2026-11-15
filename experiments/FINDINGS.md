# parano1d adversarial audit — consolidated findings

Target: "parano1d" L1 (`noid_*` crates) by @ignotus_nemo, pinned commit
`52938433a30b185ab007afaa15166a632cf2e82e` (checkout at `/Users/ember/dev/parano1d-break/parano1d`,
treated as read-only reference). Audit run: kimi swarm (`session_506c8a03`), 2026-11-15, 01:10-03:03;
29 sub-agents, root session died on quota at 03:03. This document consolidates what the lanes and the
root actually established — no new claims. A separate verifier handles reproduction.

Certainty classes used throughout (never upgraded):

- **[RV] root-verified** — the root agent independently re-ran the PoC / rebuilt the builds and
  recorded confirmation in `experiments/kimi-root-narrative.md`.
- **[RV-T] takeover-verified** — verified on the takeover side (ember ruling, 2026-11-15): either
  independently reproduced/verified by the takeover root or its dedicated verifier (deliverables
  `verify-run.txt`, `VERIFY.md`), or a takeover-completed lane whose run output is tee'd on disk
  (the lane's own `results.txt`). Kimi-era agent-reported rows stay [AR] unless the kimi root
  itself reran them ([RV]) — that distinction reflects dead agents with no report.
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
both root-verified) and, from the takeover's escalation lane, a **cross-tag claw** — one 32-byte
digest simultaneously valid as a `TAG_CAPSNODE` and an `IVCPCSN_` node digest, with two
prehashed trees committing to identical bytes under both domains' production raw verifier
(111,423 evaluations / 0.49 s), refuting the shipped design comment that "distinct trees cannot
share collisions" — a **prehashed Merkle commitment collision at the exact production source-tree
geometry** (depth 13, cap depth 3: two distinct leaf vectors, one eight-hash cap, both openings
accepted by the production prehashed Merkle verifier), **proof malleability at the legacy
`noid_fri` public verifier API** (distinct five- and six-lane final-codeword proofs verify for one
statement, with no collision search), an **exact same-anchor fork replay** (one real 323-byte
transaction and one real owner-auth bundle pass all epoch/logic/auth/state-delta checks on two forks
with different parents and state roots), a **reduction bound that provides only 41.65 bits at
T=2^32 and becomes vacuous near 2^41**, and a **soundness certificate whose decisive
fixed-permutation premise Delta_P2b is assumed (< 0.45067) and never evaluated — and, per the
formal verdict lane, either false or vacuous for the shipped object: under the only reading
that makes their ideal binding term meaningful, the cross-tag claw forces Delta above both
stated thresholds; under the other reading, their definitions cover the shipped multi-tag
commitment layer not at all (C23)**. These are real: primitive collision
resistance objectively fails at the raw node / prehashed-Merkle interfaces, and the ideal-model
justification for the shipped primitive does not hold.

What was **not** demonstrated, despite 16 completed attack lanes trying: **no unauthorized spend, no
invalid accepted block, no owner-key recovery, no fresh-request forgery, and no split-brain/state-
equivocation escalation.** Every route from the primitive collisions into protocol semantics hit a
guardrail: the exact UTXO state tree uses the other (two-permutation) compressor; capsule and
BaseFold verifiers recompute leaf hashes and pin depths and path lengths; fold equations reject
substituted openings (0 altered full PCS openings accepted); the caller sweep found no production
caller that authenticates attacker-supplied child digests without recomputing descendants. The
consistent escalation barrier is **valid leaf/descendant semantics for the colliding children** —
now proven a leaf wall: the leaf sponges' capacity IVs sit outside every invariant subfield, so
no aligned leaf lift exists below the generic 2^128 bound (the earlier 2^224 estimate is
superseded) — not more node-hash grinding; the only live finish line left is an **external**
consumer of the public raw APIs. The
remaining confirmed protocol-level flaws are conditional or bounded: same-anchor replay is a replay
property (not a forgery; severity depends on whether authorization is meant to be fork-specific),
the HeaderDAG capacity stall is a Medium conditional liveness defect (requires ~1,024 valid PoW
headers), the unbounded `getSlotsByOwner` RPC query is local-only under the pinned credential
policy, the P2P profile-ID collision across fork schedules yields incompatible-peer mixing /
DoS, not cross-version proof acceptance — and **two Medium unauthenticated-remote (anonymous
P2P) availability defects (C17, C22)**: four 12-byte mempool pulls can pin the process-wide
outbound byte domain and convert serving for all peers from delay to rejection, and sustained
admission-queue occupancy runs at a 62x CPU discount with zero attacker state cost (plus the
gossip-window variant, U21) — neither with any acceptance impact; the native trading stack's
priority surfaces are unpriced lotteries (UTXO-count draws for trades/NFT, a free txid grind for
fee-pinned financial natives — conditional on v2.1 activation; SEQ-1/SEQ-2); the
circulating-supply counter is unauthenticated on the compact-restart path (C20, local-tamper
precondition: silent divergence upward, permanent spend-admission stall downward). The root's own position statement (02:58:28): *"serious
warning signs, no consequential end-to-end attack shown yet — not 'definitely broken,' but also not
'these results are satisfying.'"* The upstream `main` (`e6d324ff…`) still contains the vulnerable
compressor unchanged, and the upstream test suites pass — this is a design/parameterization failure,
not a broken build.

---

## 2. Confirmed findings

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
| C15 | External-miner: invalid PoW consumes the only prepared template | `submitBlock` consumes the node-owned attempt (`begin_proving`) *before* `validate_pow`; an invalid nonce permanently destroys the prepared template (`replay=Unavailable; expensive_template_reusable=false`) | Medium availability, requires mining-API authorization (or local owner access); not unauthenticated, no invalid block accepted | **[RV-T]** — takeover-root rerun 2026-11-15, `verify-run.txt`, `ALL ASSERTIONS PASSED`; kimi root never verified this lane | `/Users/ember/dev/parano1d-break/experiments/external-miner/` (`README.md`, `results.txt`, `verify-run.txt`) |
| C16 | Standalone release builder embeds legacy matrix bytes without authenticating them | Substituted legacy matrix (same decompressed length) reaches Cargo sealed under the unchanged pins.env-approved metadata identity, with zero hash/manifest lines in the build log; the runtime embedded-path decoder accepts the foreign relation and returns `statement_digest() == approved` | High for the standalone builder as an authentication boundary; Medium as a defect of the official release chain (two independent compensating controls) | **[RV-T]** — independently verified by takeover verifier, `VERIFY.md`, verdict CONFIRMED-WITH-NARROWER-SCOPE | `/Users/ember/dev/parano1d-break/experiments/build-artifacts/` (`VERIFY.md`, `verify-output.txt`, `verify/`) |
| C17 | **Unauthenticated-remote (audit's first): four cheap mempool pulls pin the entire process-wide outbound byte domain** | Every served mempool pull unconditionally reserves the full 16 MiB `MAX_MEMPOOL_SYNC_BYTES` from the single 64 MiB domain — even for an empty mempool; 4 stalled writes (peer stops reading; one 303,495 B legal intent exceeds the ~256 KiB yamux window) pin it all, blocking object/history-step/segment/fork-origin serving; 6 blocked object requests exhaust the class slots and all further object requests get `Busy` — rejection, not delay — for every peer; rotation every ≤30 s sustains ~100% duty cycle | Medium availability, unauthenticated remote peer, **no acceptance impact**; four honest slow-link mempool syncs can cause the same starvation accidentally | **[RV-T]** — takeover lane, tee'd `results.txt` (real codecs + real process-global budget): `aggressors holding the entire domain mid-write: 4/4`; production-path slot exhaustion established by source reading (`pub(crate)` unreachable externally) | `/Users/ember/dev/parano1d-break/experiments/p2p-resources/` (`README.md` Finding 1, `results.txt`) |
| C18 | Header-sync serving memory is outside every process-wide budget; retention scales with attacker connections | `GetHeadersResponse` carries no permit; the codec holds canonical (1,179,648 B cap) plus compressed buffers during stalled writes; 64 stalled streams (8/conn x 8 conns, 16-byte requests) retained 75,497,472 canonical bytes — more than the entire 64 MiB domain other protocols respect; ~1-2 GB order at the 128-connection cap (~1024 streams) | Low-to-Medium availability/memory, unauthenticated remote, by-design exclusion with quantified cost — contradicts the domain's stated "without scaling with peer count" goal | **[RV-T]** — takeover lane, tee'd `results.txt` (canonical figure is a content-independent lower bound; the ~1-2 GB extrapolation and 8-slot-semaphore scope are source-read) | `/Users/ember/dev/parano1d-break/experiments/p2p-resources/` (`README.md` Finding 2, `results.txt`) |
| C19 | Every `paranoid_walletQueryReceipts` call rescans and Merkle-reverifies the entire receipt store | Any page including page 1: fetch+clone every stored receipt, decode, re-run Merkle-inclusion verification, then top-k — no cross-call cache; 20k receipts = 4.1-5.6 s wall per call (~205-225 µs/receipt); a GUI walking 400 pages burns ~28 min of one core | Medium availability, **credential-gated** (operator bearer or loopback owner, `server.rs:3861` allowlist; NOT anonymously reachable); serialized, authenticated CPU burn, not an unauthenticated amplification | **[RV-T]** — takeover lane, tee'd `results.txt` (real `start_rpc_server` + real handlers, 20k Merkle-valid receipts): `A1 … wall 5616038 us`; `A6 repeated page-1 walls: [4514004, 4442464, 4118766] us` | `/Users/ember/dev/parano1d-break/experiments/rpc-api/` (`README.md` Finding 1, `results.txt`) |
| C20 | Unauthenticated circulating-supply counter on the compact-restart path | The v2 supply appended to `state_meta` is the only hot counter NOT bound to the hash-committed tip header; compact restart trusts it verbatim while the dense fallback recomputes from authenticated slots. Upward corruption (+123,456,789): silently accepted with state roots EQUAL, propagates through a subsequent valid commit. Downward (zeroed): silently accepted, then a production-template spend of two coinbase UTXOs (validated end-to-end on a clean twin) is rejected deterministically and permanently; tip pinned at height 3 while coinbase-only blocks keep the node looking alive (45M reported vs 180M true) | Medium — integrity/liveness on a single node; **precondition: local DB write / bit rot / tampered backup** (no remote vector); no invalid block accepted, nothing minted, UTXO amounts stay root-authenticated | **[RV-T]** — takeover lane, tee'd `results.txt` (real `MdbxChainContext`, real templates + PoW): `VULN compact-restart: clean_supply=45000000 corrupted_supply=168456789 … state_roots_equal=true`; spend-stall line quoted in detail | `/Users/ember/dev/parano1d-break/experiments/storage-reorg/` (`README.md` Finding 1, `results.txt`) |
| C21 | Cross-tag claw: one digest simultaneously valid as a `TAG_CAPSNODE` and an `IVCPCSN_` node digest, with two identically-committing prehashed trees | One 32-byte digest (`9964c133…c0429a`), shared left, distinct rights, found in 111,423 evals / 0.49 s; two 2-leaf prehashed trees — one per domain — with IDENTICAL commitment bytes, each accepted by the production raw `verify_source_batched_merkle_proof_to_cap` under its own node hasher (`capsnode_raw_verify: accepted`, `ivcpcsn_raw_verify: accepted`, `roots_identical: true`) | Refutes the shipped design comment at `compression.rs:319-325` ("distinct trees cannot share collisions") — the tag domain separation is worth ~16 bits on the aligned coset, not 128; reach is raw-API-only (no in-tree caller compares digests across domains; the tower-digest bridge is `#[cfg(test)]`) | **[RV-T]** — takeover escalation lane, tee'd `results.txt` (all three binaries exited 0; claw self-verified through the real compressors, real `noid_ivc_core::merkle::hash_pair`, and the production raw verifier) | `/Users/ember/dev/parano1d-break/experiments/leaf-reachability/` (`README.md` Finding B, `results.txt`) |
| C22 | **Unauthenticated-remote (second): sustained admission-queue occupancy at a 62x CPU discount** | Node pays 8.34 ms per reject vs attacker 134 µs per distinct state-valid payload; attacker marginal STATE cost is zero — failed proofs reserve nothing (input slots reserved only after successful verify, `pool.rs:495-542`), NO rejection cache anywhere (byte-identical resubmission re-verifies 3/3), state-validity requires only public data, and one locally-generated real proof (675 ms) is a perpetual template (corrupt a byte -> fresh full-cost verify). Transport caps do not close it: 128 anonymous inbound x 50 tx/10 s = 640 tx/s offered vs 415.5 tx/s measured 4-permit drain (1.5x; ~84 identities / ~58 MB/s on this laptop; 10 identities at 4.6 MB/s on the code's own 84 ms verify budget). Under saturation: 1024 non-evictable delivery slots fill in ~4.6 s, honest txs dropped at `TxDeliveries::start` on both paths, the 64 MiB process-global inbound budget (shared with block/HistoryStep/state-segment responses) is held by ~742 queued 90 KB pushes, and the local wallet RPC queues on the SAME semaphore (p50 303 ms at 256-deep, 2 s+ at saturation) | Medium availability, **anonymous P2P** — `MempoolRequest::Push` is the one request-response arm with no `is_dispatchable` profile gate; TCP + noise handshake with free libp2p keys; gossip needs only mesh grafting, no app scoring (IP-colocation penalty only, >8 identities/IP). NOT reachable via RPC (operator/loopback per the rpc-api lane) | **[RV-T]** — takeover lane, tee'd `results.txt` (real `AsyncMempool`, real AuthGKR verify): `asymmetry ratio (node CPU s per reject / attacker construction s per tx): 62x`; transport-level effects established from code + crate unit tests + a rate model over measured per-item costs, not a live swarm | `/Users/ember/dev/parano1d-break/experiments/tx-admission/` (`README.md` Finding 1, `results.txt`) |
| C23 | The shipped security claim's fixed-Poseidon2b premise is **either false or vacuous** — the typed-binding machinery does not cover the shipped object | The paper (Zenodo 23778742) never contains `Delta_P2b` or `0.45067` — the machinery lives in `noid_soundness` docs; `BadTypedBind` is stated TWICE with different scope ("a collision, ambiguous encoding or domain confusion changes the typed semantic graph" vs "object/statement/namespace binding fails") and never disambiguated; the paper's H.1 typed dictionary has a SINGLE `v_node` (eq. 52), `c = 3` — production ships a tag-parameterized compressor with >= 2 one-perm node domains, never defined as a dictionary. Verdict fork: (a) same-tag collision — EXCLUDED-BY-CLAUSE under the worklist reading / INSTANTIATES under the dictionary reading (it is precisely the event priced at their ideal 6T^3/2^256 term); (b) cross-tag claw — EXCLUDED + UNMODELED-DOMAIN / INSTANTIATES (satisfies two listed causes simultaneously — collision AND domain confusion — matches the v2 wording verbatim; the reconstruction lemma's "unique node row" step is unguarded against same-anchor rows); (c) Delta — FORCES-DELTA under the dictionary reading: the claw is deterministically producible inside both envelopes (~2^41 gates vs C1 2^130), so `Delta_P2b^C1 >= 0.95067 > 0.45067` and `Delta_P2b(2^64) >= 0.81247 > 0.31247` — BOTH sufficient conditions unsatisfiable, PASS premise FALSE; under the worklist reading Delta stays unevaluated everywhere in their corpus | Kills the shipped security claim as stated (sharpens C8 from "unevaluated" to the fork); definitional analysis over the paper + certificate, quoting PoC-verified objects of prior lanes (C1, C21) — no new attacks run, by design | **[RV-T]** — takeover formal lane (`README.md`, no `results.txt` by design); strongest licensed claim quoted verbatim in the detail block | `/Users/ember/dev/parano1d-break/experiments/typed-bind-verdict/README.md` |

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
2^-224 (agent-4; superseded by the §5 leaf-wall economics — the aligned-lift routes are all
dominated by the generic 2^128 bound).

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

**C15 — external-miner template consumption.** Mechanism (agent-20's lane
`/Users/ember/dev/parano1d-break/experiments/external-miner/README.md`):
`submitBlock` decodes and calls `ExternalMiningAttemptSlot::begin_proving`
(`noid_rpc/src/server.rs:529-555`), removing the `Ready` value and setting the sole slot to
`Proving`, *before* `finalize_node_owned_template_after_nonce` runs `validate_pow`
(`noid_rpc/src/server.rs:1470-1491`); on `InvalidPoW` the slot drops to `Idle` and the prepared
value is not restored, so replaying that template ID returns `Unavailable` and the node must repeat
snapshot selection and nonce-independent `HistoryStep` preparation. Key output, identical in the
lane's `results.txt` and in the takeover root's independent rerun
(`verify-run.txt`, 2026-11-15): `POC invalid nonce=0 consumed prepared template
07000000000000000100000000000000; replay=Unavailable; expensive_template_reusable=false` /
`ALL ASSERTIONS PASSED`. The six controls match: real templates accepted with real PoW, cross-
template nonce reuse rejected (`InvalidPoW`), payout substitution rejected (`BadCoinbaseOwner`),
competing parent rejected (`BadParentHash`), and tip/expiry invalidation behaving as documented.
Severity and preconditions as the lane README states them: **Medium availability impact,
requiring mining API authorization (or local owner access)** — a worker with the shared mining
bearer token can cheaply destroy each expensive preparation; it is not an unauthenticated remote
attack and does not accept an invalid block. Certainty: **[RV-T]** — reproduced by the takeover
root; the kimi root never verified or summarized this lane before dying (agent's final message
never landed).

**C16 — standalone release builder embeds unauthenticated legacy matrix bytes.** Originally
agent-25's "High" (former U1); independently verified by a fresh takeover-verifier session at the
pinned commit (deliverables `/Users/ember/dev/parano1d-break/experiments/build-artifacts/VERIFY.md`,
`verify-output.txt`, `verify/`; agent-25's originals untouched; fixture scripts verified
byte-identical to `parano1d/scripts/`). Verdict: **CONFIRMED-WITH-NARROWER-SCOPE** — technically
confirmed in full; severity depends on the pack-acquisition channel.

Scope sentence (from `VERIFY.md` §5): *Legacy matrix bytes sealed under the pins.env-approved
metadata identity can be substituted without detection whenever a node is built via
`scripts/build_release.sh` without `--retired-history` (that script's default — the
transition/current-generation profile) from a pack whose integrity is not independently verified
out-of-band; but NOT in `--retired-history` builds (no matrix bytes embedded), NOT for v2/v2.1
matrices (payload-hash + semantic authentication against external bank pins), NOT in bare `cargo`
builds, and NOT in the official publish chain as scripted, where the publisher-side
`release_authenticate_pack` and the dispatch-input archive digest compensate — controls that live
entirely outside the standalone builder whose documented posture claims matrix authentication.*

Two empirical legs (exact lines from `verify-output.txt`):

- Release script (run 1, Cargo stub exit 91 = "Cargo was reached"): the pristine control and the
  substituted run both reach Cargo with the identical exported metadata pin
  (`metadata_pin_seen_by_cargo: a1a1…a1a1`); `"leaf_pin_exported_to_cargo": false`,
  `"manifest_stale_for_c00": true`, and `"log_lines_mentioning_hash_or_manifest": []` — the build
  log contains zero hash/manifest verification lines. The same pack is rejected by the
  existing-but-unwired `release_verify_sha256_manifest` when invoked explicitly
  (`error: SHA-256 mismatch … history-step-c00.field-r1cs.zst`).
- Runtime decoder (run 2, compiled from pinned `noid_ivc_core`): `safe_loader_rejected=true
  embedded_decoder_foreign_accepted=true cache_decoder_foreign_accepted=true` —
  `open_build_authenticated`, the exact decoder `rebuild_from_canonical` applies to the binary's
  EMBEDDED matrix bytes, accepts the foreign same-shape/same-canonical-length relation and returns
  `statement_digest()` equal to the APPROVED digest. **This leg agent-25 had only inferred from
  code; the takeover verifier demonstrated it.**

Severity (per `VERIFY.md` §5): **High for the standalone builder as an authentication boundary;
Medium as a defect of the official release chain** (which retains two independent compensating
controls). Blast radius if exercised: a node verifying/proving v1 HistoryStep against a foreign
relation under the canonical pinned identity — invalid local acceptance or rejection of canonical
proofs (consensus divergence), silently, with `pins.env`/`SHA256SUMS` looking pristine, and the
runtime cache keyed by metadata digest only (`noid_node/src/main.rs:1002-1008`) so poisoned
artifacts survive a corrected binary.

Corrections carried forward (verifier's precision note): agent-25's "not the default
`retired-history` profile" is correct for `publish_release.sh` (its default is retired), but
`build_release.sh`'s OWN default is the affected legacy-embedding profile — `--retired-history` is
opt-in there — and that script's usage text claims "The node build authenticates the matrix packs,"
which is false on the default path for legacy matrices. The repo's hash-verifying loader is present
but unwired on this path (`release_verify_sha256_manifest` exists in `release_common.sh` and
rejects the fixture when invoked; `build_release.sh` never calls it), and the fix pattern already
exists in the same `build.rs` for v2/v2.1 (`matrix_checks::authenticate` + `preflight_build_matrix`);
the authenticated metadata bank even carries the correct per-class `matrix_digest` — `stage_leaf`
just never compares. Remaining caveat: run 1 still uses a Cargo stub; run 2 is a real compiled
probe from pinned source.

**C17 — mempool-pull byte-domain pin (the audit's first unauthenticated-remote finding).**
Takeover lane `/Users/ember/dev/parano1d-break/experiments/p2p-resources/` (README Finding 1;
run output tee'd in `results.txt`). Mechanism: `prepare_mempool_response_after_admission`
(`noid_p2p/src/network.rs:319-344`) unconditionally calls
`budget.acquire(MAX_MEMPOOL_SYNC_BYTES)` = 16 MiB from the process-global 64 MiB
`OUTBOUND_RESPONSE_BUDGET_BYTES` domain (`outbound_budget.rs:20,43-50`) for **every** served
mempool pull — before cloning any bytes, regardless of actual response size, even for an **empty
mempool**; the permit rides inside `GetMempoolResponse` until the codec's last wire write
(`network.rs:8845`, `mempool_sync_codec.rs:286,298-301`). Exactly 64/16 = 4 concurrent responses
consume the entire domain. A peer that opens 4 mempool sync streams (8 allowed per connection,
2 connections per peer identity) and stops reading stalls each write after ~256 KiB (yamux initial
window; a single legal intent may be 303,495 bytes, `noid_tx/src/paged_spend.rs:55`). Key output,
verbatim from `results.txt`: `aggressors holding the entire domain mid-write: 4/4` / `1 KiB
mempool response (budget-participating) blocked: true` / `exact-object codec write completed
during the pin: true` / `victim admitted after releasing one aggressor: true (27 ms)` /
`ALL ASSERTIONS PASSED` — demonstrated with the real production codecs and the real
process-global budget singleton. Verified-vs-inferred split (README assurance limits): the pin and
FIFO victim-blocking are demonstrated at codec/budget layer; the production-path consequences —
blocked object/history-step/segment/fork-origin acquires (`network.rs:7229`, `:7871`,
`state_segment_codec.rs:454`, `fork_origin.rs:240-242`) and the escalation from delay to rejection
(blocked object tasks hold active data-plane slots taken at `network.rs:7226`; with 6 Live-class
slots, further object requests get `Busy` at `network.rs:7192-7211`, for **all** peers) — are
established by source reading at the pinned commit, because the admission types are `pub(crate)`
and cannot be driven externally. Preconditions and scope, honestly stated: request is 12 bytes per
stream; sustaining past the 30 s request timeout requires rotating pulls every ≤30 s
(`InboundTimeout` aborts the stalled write and drops the permit); the mempool must hold > ~256 KiB
of intents at pull time for the write to actually stall (ordinary on an active network); four
honest peers on slow links syncing a full mempool produce the same cross-protocol starvation by
accident; no invalid data is accepted — purely serving liveness. Recommended correction: reserve
the actual response size instead of the unconditional maximum, or give mempool serving its own
bounded sub-domain; release blocked data-plane slots when the requesting stream times out.

**C18 — header-sync serving memory outside every process-wide budget.** Same lane (README
Finding 2). `GetHeadersResponse` carries no permit and `HeaderSyncCodec` never consults any budget
— header serving "deliberately bypasses" data-plane admission (`network.rs:93-95`,
`protocol.rs:55-63`); during `write_response` the codec holds **both** the canonical buffer (up to
4096 × 288 = 1,179,648 bytes) and the compressed buffer while the final payload write is stalled
(`header_sync_codec.rs:288-335`). The 8-slot `header_response_prepare_semaphore` covers storage
preparation only, not the wire write. Verbatim from `results.txt`: `stalled concurrent max-batch
header payload writers: 64` / `canonical bytes retained (content-independent lower bound):
75497472` / `compressed bytes retained (this synthetic content): 35195688` / `header-sync bytes
admitted by that domain: 0` / `streams needed to exceed the shared 64 MiB domain at this
per-stream cost: ~39`. Scope: the canonical figure is a content-independent lower bound already
larger than the entire 64 MiB domain other large protocols respect; worst case at the
128-connection inbound cap (~1024 streams, reached with 16-byte requests) is on the order of 1-2
GB resident — an extrapolation, as is the 8-slot semaphore's storage-only scope (source-read).
The exclusion is documented as intentional (keeping bulk clients off the control plane), but it
makes header serving the one large-response protocol whose retained bytes scale with peer count,
against the domain's stated goal ("without scaling with peer count", `outbound_budget.rs:18-19`).
Each rotation re-runs storage preparation (MDBX read of up to 4096 headers), so the attack also
costs repeated node I/O.

**C19 — receipt-query full-store rescan per call.** Takeover lane
`/Users/ember/dev/parano1d-break/experiments/rpc-api/` (README Finding 1; run tee'd in
`results.txt`; real `start_rpc_server` HTTP/JSON-RPC stack in-process over loopback, synthetic
wallet with 20,000 real cryptographically valid `ParanoidReceipt`s). Mechanism: for every call —
any page, including page 1 — `scan_receipts` (`noid_rpc/src/server/receipt_query.rs:97-171`)
clones every stored receipt under the wallet mutex, decodes it, and **re-runs Merkle-inclusion
verification and bech32 metadata construction** for every receipt before keeping a top-k heap of
size `required = offset + page_size`; nothing is cached between calls. Verbatim from
`results.txt`: `A1 page 1 … 50 rows, wall 5616038 us, handler alloc peak 20805 B` / `A6 repeated
page-1 walls: [4514004, 4442464, 4118766] us (each a complete rescan; no cross-call cache)` /
`A7 query-path per-receipt cost: ~225 us/receipt`. Cost is identical for every page; a wallet GUI
walking 400 pages of a 20k-receipt wallet burns ~28 minutes of one core, and a credentialed or
local client can keep the blocking-pool thread hot indefinitely at ~zero bandwidth cost. Scope
and bounding guardrails (README): the method is on the operator allowlist (`server.rs:3861`) —
same trust class as the external-miner finding; a **single-permit gate** rejects concurrent
queries immediately (`receipt_query.rs:11,85-87`), the scan runs on `spawn_blocking`, output is
bounded (`page_size <= 50`), and stored receipts are size-capped (`MAX_RPC_RECEIPT_BYTES` =
128 KiB). "A serialized, authenticated, self-inflicted-or-malicious CPU burn that scales linearly
with receipt count — not an unauthenticated amplification." Recommended correction: persist the
authenticated sort key alongside each receipt and paginate the index, or cache the verified key
vector keyed by the existing `query_revision()`.

**C20 — unauthenticated circulating-supply counter.** Takeover lane
`/Users/ember/dev/parano1d-break/experiments/storage-reorg/` (README Finding 1; run tee'd in
`results.txt`; real `MdbxChainContext::open_or_create`/`apply_next_block`, real templates, real
PoW; corruption applied by a second process writing single rows with production-identical MDBX
options). Mechanism: `state_meta` v2 (`noid_chain/src/storage/serial.rs:520-559`) appends a
16-byte supply to the v1 counters; on restart the v1 counters are cross-checked against the
hash-committed tip header (`mdbx_context.rs:781-788`) but the supply is read verbatim
(`:770`) and trusted by the compact restart path (`try_load_compact_chain_state`,
`mdbx_context.rs:503-567`) — while the dense fallback (`load_dense_chain_state`) proves the
value is derivable from authenticated data. Verbatim from `results.txt`: `VULN compact-restart:
clean_supply=45000000 corrupted_supply=168456789 tip=… state_roots_equal=true` /
`VULN propagated-by-valid-commit: height=2 clean_supply=90000000
persisted_corrupt_supply=213456789` / `CONTROL dense-restart-recomputes: cleared
segment_summaries; recovered_supply=90000000` / `VULN zeroed-supply restart: supply=0 accepted
silently at height 3, state root still header-authenticated` / `VULN spend-admission stall:
valid block rejected twice with 'corrupt: uncommitted block supply rollback failed'; tip pinned
at height 3, no header 4 written` / `LIMIT zeroed supply still admits coinbase-only blocks
(supply=45000000 at height 4, clean chain had 180000000)`. The downward-stall mechanism:
`supply_after_slot_updates` (`state.rs:585-612`) folds slot deltas with checked arithmetic from
the corrupted base and underflows; the rollback of the uncommitted candidate hits the same
underflow (`mdbx_context.rs:983-988`). Scope: the project's own posture is to authenticate or
clear-on-corrupt (`mdbx_context.rs:674-701`) — the supply is the one hot counter exempt from
both, and its failure never triggers the clear-and-resync recovery. Precondition: local write
access, bit rot, or tampered backup. Stated limits: upward corruption never stalls admission
(bookkeeping divergence only); the spend-stall demo requires the spent total to exceed the
corrupted base plus the block's own coinbase; the HistoryStep terminal verifier was stubbed
(nothing tested depends on proof soundness).

**C21 — cross-tag claw between the two 1-perm node domains.** Takeover escalation lane
`/Users/ember/dev/parano1d-break/experiments/leaf-reachability/` (README Finding B; run tee'd
in `results.txt`). Object: a single digest that is simultaneously a valid `TAG_CAPSNODE` node
digest and a valid `IVCPCSN_` node digest (production `noid_ivc_core::merkle::hash_pair`),
shared left `298ed143…3d6645`, distinct rights, `shared_digest_hex` =
`9964c133d849e900a90f95543d0db8657f5fb3b5e7244c350e160f765ec0429a`, found in **111,423
evaluations / 0.49 s**. Two 2-leaf prehashed trees, one per domain, share the identical
32-byte commitment, and each opening is accepted by the production raw
`verify_source_batched_merkle_proof_to_cap` under its own node hasher
(`"capsnode_raw_verify":"accepted"`, `"ivcpcsn_raw_verify":"accepted"`,
`"roots_identical":true`, `"leaves_distinct":true`). This empirically refutes the shipped
design comment at `noid_poseidon2b/src/native/compression.rs:319-325` — *"the caller-selected
tag whitens the right-input lanes so distinct trees cannot share collisions"* — the tag domain
separation is worth ~16 bits on the aligned coset, not 128. Scope: no in-tree caller compares
digests across domains (swept; semantic capsule/PCS consumers recompute leaf hashes; the
`tower_lanes_to_flat_digest` bridge is `#[cfg(test)]`, `zk_auth_qrom.rs:1999-2004`), so reach
is the same raw/prehashed-API boundary as the same-tag collisions, now ACROSS domains. The lane
notes: if `BadTypedBind`/`Delta_P2b` is ever evaluated concretely, this is an instantiable
input — two domain-typed authentication objects with equal commitment bytes and disjoint
preimage semantics. **Not claimed anywhere:** no fresh-request forgery, no full-capsule
collision, no semantic verifier accepts these objects.

**C22 — anonymous admission-queue occupancy at a 62x CPU discount.** Takeover lane
`/Users/ember/dev/parano1d-break/experiments/tx-admission/` (README Finding 1; run tee'd in
`results.txt`, `ALL ASSERTIONS PASSED`; real `noid_mempool::AsyncMempool` — real bounded decoder,
real state/fee/anchor/slot admission checks, real 4-permit verification semaphore, real AuthGKR
verify — against synthetic chain state). This is the answer to the root's 02:54:49
cost-asymmetry question. Mechanism, every piece public and nothing consumed: (1)
`check_input_slots` (`pool.rs:1362-1408`) compares only *claimed* `(amount, creation_id, owner)`
against readable on-chain slot values — ownership is established solely by the 8 ms proof check;
the fee is declared but never collectible on rejection; (2) `pool.admit` and
`admitted_input_slots` insertion happen only after proof success (`pool.rs:495-542`) — measured
300+ rejections on one slot, all reaching verification, pool empty after; (3) no rejection cache
— byte-identical resubmission re-verifies 3/3, and varying the fee by 1 yields a new payload and
txid for ~134 µs; (4) one real proof (675-683 ms, public prover) is a perpetual template: flip a
byte, every variant decodes, passes cheap checks, burns a full verify. Verbatim from
`results.txt`: `A5 real-proof, tail corrupted n=20 mean= 8.340ms` / `burst 256 txs: wall=616.1ms
throughput=415.5 tx/s … max_concurrent_verify=4 (semaphore=4)` / `construction: 1000 distinct
state-valid payloads in 134.26125ms (134.261µs/tx)` / `pool.len=0 … reserved_in=0 reserved_out=0`
/ `asymmetry ratio (node CPU s per reject / attacker construction s per tx): 62x` /
`delivery slots fill in 4.6s at max inflow; honest txs are then dropped at TxDeliveries::start
(both paths)`. Wallet-path coupling: the RPC wallet path is `mempool.clone()` with a different
executor but the **same `Arc<Semaphore>`** (`server.rs:3529-3538`); uncontended admission is
24.4 ms. Honest caveats, verbatim from the lane: attacker bandwidth (~58 MB/s to saturate this
laptop's 4 workers) is the binding cost on fast hardware — this is what keeps the finding
Medium rather than High; the cheap garbage-bytes variant IS blocked (decode-before-verify
~190 µs, mathematically cannot saturate under the rate caps — check ordering works there); and
the transport-level effects are established from the pinned code, libp2p configuration, the
crate's own unit tests, and a rate model over measured per-item costs — not a live multi-peer
swarm. Recommended fix to record: a short-TTL negative cache keyed `(txid, proof hash)` (removes
the 62x discount for the observed attack shape; the README adds three complementary measures).
Composes with C17: both anonymous, both attack the shared 64 MiB byte domain and the delivery
paths — cross-referenced to devnet-chaos for the combined live demonstration.

**C23 — the typed-binding verdict (kills the shipped security claim as stated).** Takeover
formal lane `/Users/ember/dev/parano1d-break/experiments/typed-bind-verdict/` (definitional
analysis; no `results.txt` by design — it quotes the PoC-verified objects of prior lanes). It
sharpens C8 from "the Delta premise is unevaluated" to a two-reading fork their corpus never
resolves. Their definitions, verbatim-tracked: the paper's H.1 typed dictionary has a single
`v_node` (eq. 52: `N(a,b)=rate P(a,b XOR v_node) XOR a`) and `c = 3` sponge capacities, while
production ships a tag-parameterized compressor with at least two one-perm node domains
(`TAG_CAPSNODE`, `IVCPCSN_`), never defined as a dictionary; the certificate gestures at
"typed commitment domains" without defining them; `BadTypedBind` is stated twice with
different scope and never disambiguated; the H.1 guard does not exclude node collisions (two
distinct records whose node anchors coincide violate no guard clause — the reconstruction
lemma's "the unique node row" step is unguarded). The verdict fork, per candidate, is in the
table row above. **Strongest licensed claim, verbatim:** *"Their certificate's
fixed-Poseidon2b premise is either false or vacuous for the shipped object: false, because the
2^16 cross-tag claw instantiates their own typed-binding bad event ('a collision or domain
confusion') and forces Delta_P2b^C1 >= 0.95 against a required < 0.4507; or vacuous, because
their security statement — whose paper formalizes a single node domain — defines no bad event
that covers the shipped multi-tag commitment layer at all, leaving the exact property their
design comment claims ('distinct trees cannot share collisions') outside their theorem in
either case."* The lane also records the exclusion list — the five surfaces their definitions
thereby fail to cover (raw-verifier acceptance, cross-domain commitment-byte identity,
subfield-aligned node collisions, the multi-tag node layer, the Delta premise itself).

---

## 3. Reported-but-unverified

Two populations live in this section. (a) Kimi-era items (U1-U9) whose root verification was
interrupted (02:43-03:03 quota death) or never started — **none of these may be cited as
root-confirmed**; they keep [AR]/[PC]. (b) U10-U27 (wallet-safety, prover-unsafe, dex-economics,
rpc-api, storage-reorg, nft-marketplace, tx-admission, dex-games, sequencing):
takeover-completed lanes reclassified **[RV-T]** by ember's 2026-11-15 ruling that a
takeover-completed lane with a tee'd `results.txt` on disk counts as verified — the evidence is
the lane's own run output ("the subagents just ran it"). They keep their U-IDs here for continuity.

### U1 — build-artifacts "High": legacy matrix bytes substitutable without changing approved metadata identity — **promoted to §2 as C16**

Originally agent-25's report (`/Users/ember/dev/parano1d-break/experiments/kimi-agent-reports/agent-25.md`;
the kimi root died mid-verification). On 2026-11-15 a fresh takeover-verifier session independently
confirmed it with a narrower scope: see **C16** and
`/Users/ember/dev/parano1d-break/experiments/build-artifacts/VERIFY.md` (verdict
CONFIRMED-WITH-NARROWER-SCOPE). Two of agent-25's caveats were sharpened by the verifier: the
release-script leg still uses a Cargo stub (run 1), but the runtime-decoder leg is now
empirically demonstrated from pinned source (run 2); and the affected profile is
`build_release.sh`'s OWN default (not merely "builds retaining legacy matrices" — see C16's
corrections).

### U2 — build-artifacts "Medium": packed runtime cache accepts a foreign relation under the approved digest **[AR]**

Same report; `probes/results.json` records
`foreign_image_accepted=true approved_digest_retained=true safe_loader_rejected=true canonical_lengths=210 image_lengths=391`.
This probe used small valid same-shape relations and the **actual pinned decoder**
(`noid_ivc_core/src/field_r1cs.rs:3986-4005`, which deliberately omits the structural digest
check). Precondition: write access to the node data directory or a previously poisoned build/cache —
which the agent itself says "limits severity." The takeover verifier's run 2 empirically
reproduced the decoder-level acceptance (`cache_decoder_foreign_accepted=true` in
`verify-output.txt`); the cache-poisoning *persistence/impact* scenario remains code-level only, so
U2 stays in this section rather than being promoted.

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

### U5 — external-miner: invalid PoW consumes the only prepared template — **promoted to §2 as C15**

Originally quarantined here because the kimi root died before verifying the lane (the agent's final
message never landed). On 2026-11-15 the takeover root independently re-ran the PoC
(`CARGO_TARGET_DIR="$PWD/target" cargo run --release`, output tee'd to
`/Users/ember/dev/parano1d-break/experiments/external-miner/verify-run.txt`): `ALL ASSERTIONS
PASSED`, identical template ID `07000000000000000100000000000000`, same invalid-nonce-consumes-
template behavior and all six controls. Now recorded as **C15** with certainty **[RV-T]**
(takeover-root-verified — distinct from the kimi root's own verification, which never happened for
this lane). Severity/preconditions unchanged from the lane README: Medium, requires mining
credential or local owner.

### U6-U27 — further reported results (U6-U9 kimi-era unverified; U10-U27 [RV-T] by ruling)

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
- **U10** wallet-safety, **Low (hardening/defense-in-depth)** **[RV-T]**: nothing downstream of the
  wallet builder can detect a recipient/amount substitution (takeover lane, run output tee'd in
  `/Users/ember/dev/parano1d-break/experiments/wallet-safety/results.txt`; see `README.md`
  Finding 1). The post-build
  divergence gate (`noid_rpc/src/server.rs:1164-1167`) checks only fee + input/output counts;
  `WalletSendResult` has no recipient field and echoes the *request* amount; the GUI success screen
  renders the request string; history confirms by txid only. PoC: two intents through the real
  `build_send` pipeline, same fee/counts, different recipient and amount — both pass real GKR
  verification and the exact gate condition. **Precondition (verbatim from the lane):** no
  divergent path exists in the pinned code (the control test proves request pass-through into the
  signed page); the gap matters only against builder regression or a compromised binary. The
  stronger binding pattern exists in-tree for native-assets flows but is not applied to ordinary
  sends.
- **U11** wallet-safety, **Low** **[RV-T]**: exported master secret persists in the system clipboard after
  dialog close — GUI-side memory is zeroized, but the three `iced::clipboard::write` sites never
  clear (README Finding 2). Precondition: the user opens Export and presses Copy.
- **U12** wallet-safety, **Informational** **[RV-T]**: a stale in-memory `WalletState` survives an on-disk
  master-secret replacement and still signs with the old secret, with later writes landing in the
  new wallet's history (README Finding 3) — verified by test but **no production path**: import
  runs only in the maintenance CLI before the daemon starts; the GUI stops owned nodes and refuses
  external ones.
- **U13** prover-unsafe, **Low-to-Medium (latent)** **[RV-T]**: zero-round BaseFold provers emit
  never-written memory as proof fields (takeover lane, run output tee'd in
  `/Users/ember/dev/parano1d-break/experiments/prover-unsafe/results.txt`; see `README.md`
  Finding 1). At
  `a_init.len()==1` (`log_msg_len==0`) — accepted by the entry assertions — the round loop never
  executes, yet the epilogue reads `final_a = a_active[0]`
  (`noid_ivc_core/src/pcs/basefold.rs:1074`) and `final_codeword` (`:1076`) from buffers no code
  path wrote, violating the scratch pool's documented write-before-read contract; the C1 twin
  (`:1602-1811`) reads genuine uninitialized `alloc_uninit_vec` memory. Demonstrated
  deterministically: F128 `final_a` = poisoned pool marker `0x1EAD_ED00_0000_0004` (`results.txt`:
  `zero-round final_a=1eaded0000000004:d15c000000000004`); C1 `final_a` = zeros where the honest
  lift is `0x11,0x22` (malloc-recycle marker not handed back, so the C1 leg rests on the
  never-written path plus value mismatch, not a captured marker). **No acceptance impact**: both
  proofs FAIL verification (`verify_c1 -> Err(SumcheckFinalMismatch)`); an honest prover hitting
  this path always self-rejects; the F128 case leaks stale pool bytes into proof bytes.
  **Not production-reachable at the pinned commit:** in-tree callers use `log_msg_len` 22/24
  (`noid_recursive/src/acceptance/history_step_bank.rs:110,48`); reaching it requires
  `m == LOG_PACKING`, which nothing constructs. Limits: reachability established by caller
  enumeration at this commit, not future-proof; aarch64 only (x86 kernels read, not executed).
- **U14** prover-unsafe, **Low** **[RV-T]**: degenerate raw-API panics — `AdditiveNttF128::standard(0)`
  index-out-of-bounds (`noid_ivc_core/src/ntt/additive_ntt_f128.rs:72`);
  `default_fri_queries(0, 1)` panics via `unwrap_or_else` (`basefold.rs:229`). Both sit behind
  commitment-bound parameters in every production caller (README Finding 2).
- **U15** prover-unsafe, **Low (unreachable)** **[RV-T]**: a 73-byte crafted `R1csProofBundle` drives a
  1,048,576-byte allocation (~14,000x input) through bincode's length-prefixed `Vec`
  deserialization before failing cleanly with EOF — `results.txt`:
  `parser hostile: input=73 declared=10000000 max_single_allocation=1048576`; no panic, no OOB,
  and the format has zero in-tree callers (grep at the pinned commit; README Finding 3).
  Limits: single-shape fuzz elsewhere in the lane, not coverage-guided.
- **U16** dex-economics, **Informational boundary observation** **[RV-T]** (takeover lane, run
  output tee'd in `/Users/ember/dev/parano1d-break/experiments/dex-economics/results.txt`):
  the bonding curve prices by the cumulative ceiling `C(q) = ceil((3*S*q + 5*q^2)/(900*S))`
  (`pinned/noid_tx/src/native_assets/launch.rs:210-218`); at `sold = cap-1` the ceiling is flat,
  so a 1-NOID gross buy purchases the final unit **spending 0 NOID** (whole gross input refunded)
  and triggers graduation into the pool seeded `(S/250, 0.4*S)` (`results.txt`:
  `zero_spend_graduation`); symmetrically the first NOID buys 299 units. **NOT extractable
  value**, three reasons (all verified): (1) `ceil` means aggregate intake is never below the
  exact integral price — the free tail is pre-paid by earlier buyers' rounding surplus, and
  `C(cap) = S/250` exactly across min/mid/max admissible supplies; (2) graduation creates no
  claimable value (all LP shares locked, reserves = exact cumulative cash + unsold inventory,
  conservation asserted on an accepted batch); (3) zero-proceeds sells always rejected
  (`minimum_output >= 1`), curve roundtrips are exact reversals (500/500), pool roundtrips pay
  the 0.30% fee both ways (500/500, never profitable). Precondition: an attacker reaches
  `sold = cap-1` only after the market has already absorbed `C(cap-1)` — 99.999998% of the
  graduation cash — from prior buyers; the 0-NOID trigger moves graduation one rounding unit
  earlier than a naive reading suggests, and nothing more. The lane's overall verdict is
  negative (see §4): `"accepted_value_extraction_found": false`, 9/9 experiments, exit 0.
- **U17** rpc-api, **Low (allocation amplification only)** **[RV-T]** (takeover lane, run output
  tee'd in `/Users/ember/dev/parano1d-break/experiments/rpc-api/results.txt`; README Finding 2):
  out-of-range receipt pages retain the complete key set — `required = offset + page_size` is
  caller-controlled (`page` up to `u32::MAX` makes `required` ~2.1e11), so past the end of the
  result set the top-k bound never engages and the `BinaryHeap` retains all N keys. Verbatim:
  `A4 page u32::MAX (offset 214748364700, required 214748364750): 0 rows, wall 4162763 us,
  handler alloc peak 2898757 B` vs `20805 B` for page 1 (`A4-A1 out-of-range peak amplification:
  2877952 B`). Retention starts at the first page past the end (page 401), in-range pages stay
  bounded; it is bounded by receipt count and transient — an allocation multiple of C19's own
  O(N) working set, not an independent unbounded sink. Same credential preconditions as C19.
- **U18** rpc-api, **Low-Medium availability** **[RV-T]** (takeover lane, same run; README
  Finding 3): `paranoid_walletReceipts` scans **inline on async worker threads** with the wallet
  `std::sync::Mutex` held across the entire O(N) pass and **no `wallet_operation_gate` taken**
  (`server.rs:2643-2646` -> `noid_node/src/wallet/mod.rs:910-967`; the gate that serializes
  send/scan operations is taken by other paths but not this one). Demonstrated with a line-cited
  faithful mirror of the production scan behind the real handler and real HTTP stack: 16
  concurrent calls on a 4-worker runtime serialized on the mutex (walls ~0.31 s each) and
  unrelated `paranoid_getChainInfo` probes stalled up to 310,884 µs vs 101 µs idle p50 —
  `B6 worker starvation factor: max_during/idle_p50 = 3078x`. **Verified-vs-inferred split
  (carry it):** the mirror is line-cited and identical in lock shape and receipt crypto, but the
  production `WalletState` is binary-private (`noid_node` exports no wallet module), so
  production behavior is inferred from code identity, not measured against the linked function;
  the stall window scales linearly with receipt count and per-receipt decode cost. Same
  credential preconditions (operator-allowlisted at `server.rs:3860`). Recommended correction:
  route through `spawn_blocking` plus the single-permit gate, or acquire
  `wallet_operation_gate` before the scan.
- **U19** storage-reorg, **Low-to-Medium liveness** **[RV-T]** (takeover lane, run output tee'd
  in `/Users/ember/dev/parano1d-break/experiments/storage-reorg/results.txt`; README Finding 2):
  a noncanonical `chain_tip` encoding is accepted at restart but permanently stalls every
  successor commit. `decode_chain_tip` (`serial.rs:511-518`) accepts any value of at least 40
  bytes, silently ignoring trailing bytes (unlike the exact-length segment-summary and
  consensus-meta decoders), while the commit paths re-read the tip typed as `Option<[u8; 40]>`
  inside the write transaction (`mdbx_store.rs:3829-3838`). A one-byte append therefore passes
  every restart consistency check, then **every** subsequent block commit — including reorgs —
  fails with `store: mdbx: 41`, with no recovery: because restart validation succeeds, the
  documented clear-all recovery never triggers. Verbatim: `VULN noncanonical-tip: 41-byte
  chain_tip accepted by open_or_create at height=0` / `LIMIT noncanonical-tip: next valid commit
  rejected with 'store: mdbx: 41'; no partial tip mutation`. Fail-closed but dead; same
  local-write/bit-rot precondition as C20, no invalid state accepted. Recommended correction:
  make `decode_chain_tip` exact-length so the corrupt row is rejected during
  `restore_from_mdbx` and the clear-and-reinitialize recovery runs.
- **U20** nft-marketplace, **Informational — publicly relevant reachability fact** **[RV-T]**
  (takeover lane, run tee'd in `/Users/ember/dev/parano1d-break/experiments/nft-marketplace/
  results.txt`, `test result: ok. 10 passed; 0 failed`): at this commit the funded round-escrow
  system (request funding, registration op 70, settlement 71, reset/recover/refund 72-75) and
  the **entire offer family (ops 67-69) are unreachable**. Two independent gates:
  `noid_tx/src/native_assets/profile.rs:8-33` omits codes 67..=75 from `operation_enabled`
  (rejected at `operand()` before any semantic check), and `profile.rs:35-53` omits
  `NftPurchase`/`NftOffer` from `kind_enabled`, so `NativeOpening::validate()` returns
  `Err(Kind)` and even **perfectly-formed purchase-request funding and offer funding die at the
  opening-kind gate** (`Err(Kind)` -> `Funding`), demonstrated end-to-end. Additionally
  `check_purchase_funding` has no dispatcher in `check_financial_owner` at all, and
  `FinancialBlockTracker::owner_outputs` returns early for `Purchase`, making its
  purchase->registration arm dead code. Live surface at this pin: base NFT ops (5-8) plus
  listing create/instant-buy/cancel (64-66) — **no royalty/fee-split settlement or offers can
  occur on-chain at this commit**. Implication to carry: any docs/paper claim of marketplace
  offers/escrow at `52938433` is dead code behind a profile gate. (The round-settlement
  economics were still tested below the gate on synthetic escrow — see §4.)
- **U21** tx-admission, **Low-Medium availability, anonymous P2P** **[RV-T]** (takeover lane,
  same run; README Finding 2 — **a code-path argument with the pinned constants, not an
  executed attack**): the gossip byte window (64 MiB / 10 s, `network.rs:493`) is **global and
  shared with block announcements** (`network.rs:6491-6495`), and it is checked **before
  decode**. ~5 identities shipping 303,495 B undecodable payloads (50 tx/10 s each = 75.8 MB
  per window vs the 64 MB budget; ~6.4 MB/s of garbage, no validity needed) monopolize it, and
  the node `Ignore`-drops **all honest tx AND block gossip** for the rest of each window.
  `Ignore` carries no gossipsub peer-score penalty (no app scoring; IP-colocation penalty only),
  so the exhaustion is sustainable and unpenalized. Cheaper per-identity cost than C22, but
  gossip-layer only. Recommended correction: make the byte window per-peer or deduct bytes only
  after the cheap-validity boundary, or keep a small per-peer share of the global window.

### dex-games — MEV posture of the native trading stack (all Informational; includes one ledger correction) **[RV-T]**

Takeover lane `/Users/ember/dev/parano1d-break/experiments/dex-games/` (5/5 sections,
`ALL ASSERTIONS PASSED`, run tee'd in `results.txt`; library-level against the pinned crates).
Overall verdict: **no ordering bug, no extraction vector; every race resolves by a named
deterministic rule; the one capturable prize is priced at 1 µNOID against a 9,800 µNOID fee
floor.**

- **CORRECTION to the established-facts ledger:** "TradeBatch sorts by authorization key"
  (`trade/batch.rs:125-164`) governs **canonical encoding only** — **execution order is the
  funding-priority digest** (PoC: racers with key order reversed relative to digest order still
  execute in digest order; independently confirmed by the sequencing lane). Authorization-key
  grinding is worthless as a priority strategy. The dex-economics lane row's process note is
  annotated accordingly.
- **U22 — tail snipe.** Winner is the racer with the lowest
  `funding_priority = Poseidon("TRORD_21", parent_id, asset, anchor-UTXO creation_id, slot_index)`
  (`noid_tx trade.rs:231-264`); the shipped miner ranks and greedily includes by that digest
  (`noid_miner trades.rs:424-479`); the sequencing engine executes in the same order
  (`sequencing.rs:136-139`). The prize is **exactly 1 µNOID** — provably the curve's entire
  cumulative rounding surplus (exact integral to cap == graduation cash, asserted at 3 supplies)
  — against a 9,800 µNOID fee floor, and the free tail cannot be sold standalone (pool dust
  floor rejects 100 units; minimum lot 102). Net **-9,799 µNOID plus a graduation-timing
  option**. The loser's 1-µNOID buy is declined as pool dust, dropped **uncharged** by
  `stable_policies`, and stands in the mempool as a free retried lottery ticket.
- **U23 — graduation timing.** The pool opens at exactly the curve's terminal marginal price
  (surplus NOT in the pool); no sandwich seam (fair pre-graduation buy -> pool sell loses
  12 µNOID); the graduator controls only the same-fill remainder leg and same-batch paired
  intents — first-mover optionality; competitors can enter the pool leg in the same batch.
- **U24 — launch head is a ceiling overcharge, not a prize** (0.9967 µNOID value for 1 paid;
  the second buyer gets 300 units for the same NOID); the creator can bundle 0/partial/
  entire-curve purchase into the creation transition itself.
- **U25 — fees buy nothing anywhere** (fee_limit is a ceiling; the charge is the state-derived
  minimum; no tip mechanism; PoC: 1,000x fee changes nothing). The priority market is **UTXO
  inventory** — chain-assigned coordinates, win probability n/(n+m), redrawn every block via
  parent_id.
- **U26 — the default race winner at consensus is the block builder**: validation re-derives
  whatever batch the proposer submits (`block/native_assets.rs:718-747`); no inclusion or
  ordering fairness rule exists; ordinary block ordering is documented "miner policy, not
  consensus."

### sequencing — the five-layer ordering map **[RV-T]**

Takeover lane `/Users/ember/dev/parano1d-break/experiments/sequencing/` (25 assertions green,
including 19 verbatim pinned-source fact assertions via `build.rs`; run tee'd in `results.txt`).
The map — delivery (payload-dedup gossip, no order/priority) -> admission (first-come,
slot-conflict reject, **no RBF anywhere**, fee floor is an admission gate not priority,
admission race is verification-throughput order with the lock released during proof check) ->
template selection (ordinary txs desc fee-rate / asc-txid; trades by funding_priority;
fee-pinned financial natives tie-break on txid) -> in-block (builder discretion, consensus
never orders, creation ids assigned by in-block position) -> cross-block (heaviest work +
lexicographic tie-break, finality 18; same-anchor replay means the work race decides which
fork's spend survives). Race ownership: double-spend -> the PoW winner's mempool state;
same-price trade-batch position and NFT fixed-price races -> largest pre-positioned
dust-UTXO inventory (K/(K+M), ~9,000 µNOID per permanent ticket, capacity capture at the
64/16 caps); congested financial-native inclusion -> smallest-txid grind (free, median 2
hashes); inclusion within an interval -> reach the builder before template freeze
(`user_pages == 0` refresh guard); every batch race ultimately -> the builder.

- **SEQ-1 — Medium (conditional on v2.1 native activation, mainnet height 279,370):** priority
  across the native trading stack is a UTXO-count lottery — count-weighted (dust UTXO = whale),
  one-time persistent cost (~9,000 µNOID per ticket), capacity-capturing at the 64/16 batch
  caps; measured 0.672 vs K/(K+M)=0.667 (trades, K=8/M=4, 2,000 parents) and 0.733 vs 0.750
  (NFT, K=12/M=4, 400 rounds).
- **SEQ-2 — Medium (v2.1-conditional):** financial natives have no fee lever — fees pinned to
  the exact parent-derived minimum (both admission and consensus equality lines asserted
  verbatim), so their ordering reduces to a **free offline txid grind** (median 2 body hashes
  to beat a victim); any ordinary spend paying 1 µNOID more displaces every native from a
  saturated template.
- **SEQ-3 — Low-Medium:** inclusion freezes at template build (`user_pages == 0` refresh
  guards, asserted verbatim) — later arrivals at any fee wait for the next block; composes
  with C15 as a template-churn lever adjacent to censorship.
- **SEQ-4 — structural/informational:** builder discretion is total and reaches future
  priority entropy (creation ids assigned by in-block application order; registration-height
  choice shifts round eligibility); no timestamp reads exist in the native logic.
- **SEQ-5 — integration with C6:** across same-anchor forks the same spend can confirm on
  both; which survives is purely the work race, decided up to 18 blocks late; downstream
  indexes treating <finality confirmations as settled see a per-fork, non-monotonic history.
  No mechanism lets admission order leak into fork resolution.

**Sharpest public claim (verbatim, with its condition):** *"conditional on v2.1 native
activation (height 279,370), every priority surface in parano1d's native trading stack is an
unpriced UTXO-count lottery — trade execution order and NFT fixed-price winners are decided by
offline hash draws over pre-positioned dust (count-weighted, never value-weighted, ~9,000
micronoid one-time per ticket), while financial natives, whose fees the protocol pins to the
exact minimum, are ordered by a txid grind that costs a median of two body hashes — with no
fee- or value-based path to priority anywhere in these paths."*

Cross-references kept clean: first-seen pinning -> ledger-fairness §3; invalid-PoW template
burn -> C15 (churn lever); same-anchor replay -> C6.

### constraint-systems — one Low process finding **[RV-T]**

- **U28** constraint-systems, **Low (process)**: the v2.1 compact-arithmetic constraint path —
  `bind_program_v2_1`/`integer_results_v2_1` (`trace/integer_program.rs:178-243`), the
  production-selected variant for `ScheduledV2` profiles and the one with the more delicate
  gadgets (inverse-gadget equality, compact carry) — had **zero direct in-tree tests** (every
  in-tree test exercises the v1 path). Not a bug — the algebra is correct — but until this lane
  the production-selected arithmetic for v2.1 contracts had never been differentially tested
  against native execution; the 8,870-configuration battery now covers it under all 10 builder
  configurations (`results-ip.txt`). Architectural note that scopes all Pruv results
  (including the earlier 50k-program differential, which ran against the reference native, not a
  second consensus path): **consensus never natively executes Pruv** — no `integer_program`
  caller exists in `noid_chain`/`noid_block`; the circuit is the sole consensus executor, so
  circuit soundness is the whole game, and contract witnesses are never serialized in the block
  body (proof-internal wires bound through the object-root commitments).

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
| Wallet secret-surface attacks (exfiltration via RPC/argv/wire/keystore) | Keystore 0600/48-byte/symlink-rejecting; atomic staged import with 0700 rollback; master secret never transits RPC (CLI stdin, never argv); remote RPC fails closed; mining token cannot reach `walletSend`; `SpendSecret`/`OwnerAuthWitness`/`MasterSecret`/`SensitiveString` zeroize-on-drop, un-Clone-able, Debug-redacted; wire bytes contain no secret window | [RV-T] (takeover wallet-safety lane; 5 Rust + 7 Python tests pass, tee'd `results.txt`) |
| Native-vs-SIMD disagreement in prover kernels (the consensus-split probe) | 8,241 F128 pairs (random + full edge cross-product) bit-exact across production NEON dispatch, software, schoolbook, karatsuba, karatsuba-barrett, vec2-neon, and both unreduced-reduce pipelines; F256 lift consistent; NTT dispatched == scalar (log_d 1-10); Poseidon batch == native sponge (1-17 leaves); matrix loader: 2,689 truncation/mutation cases, zero panics; verifier shape-gates pinned to commitment-derived constants before indexing | [RV-T] (takeover prover-unsafe lane, tee'd `results.txt`; aarch64 only — x86 kernels read, not executed; single-shape fuzz, not coverage-guided) |
| Malformed-frame panics in P2P sync codecs; missing backpressure; ban-system bypass | Attacker-controlled lengths checked against count-relative caps before allocation; checked arithmetic; all `unwrap()`s on compile-time-fixed sizes (static review, non-exhaustive — "no panic found" is not "no panic exists"); header `Busy` backpressure works as designed; per-stream/conn/diversity caps, 30-60 s write-abort timeouts, and a separate inbound 64 MiB domain bound both findings; no app-level ban/score system exists to bypass (gossipsub defaults + IP-colocation factor; immediate disconnect at protocol violations) | [RV-T] (takeover p2p-resources lane, tee'd `results.txt`) |
| DEX value extraction (manipulated fill, free asset creation, invalid successor state) | No accepted value-extracting transition in 9/9 experiments (`"accepted_value_extraction_found": false`, exit 0); cash-cut successor + pool bump -> page amount binding (`native_assets.rs:1024-1031`); +1 inventory / +1 token / +1 NOID / permuted claims -> re-derived-execution equality (`wire.rs:317-343`); duplicate funding slot, output-slot collision, one bad `min_out` -> whole-batch atomic rejection with state root unchanged; host-vs-recursive: 72 pool + 120 curve/graduation R1CS cases match the host on all 8 economic fields incl. boundary cases, `min_out+1` makes the R1CS unsatisfiable | [RV-T] (takeover dex-economics lane, tee'd `results.txt`/`results.json`; **assurance limit**: differential testing proves honest-witness agreement and min-out enforcement, NOT absence of alternative witnesses — see §5) |
| RPC receipt forgery / concurrent-query amplification / anonymous remote access / browser drive-by | Every receipt re-verified (Merkle inclusion + key identity), totals correct, no forgery path; single-permit gate rejects concurrent receipt queries immediately; non-loopback listener refuses startup without credentials (`server.rs:3520`); any `Origin` header -> HTTP 403 before dispatch; `Authorization` stripped before tracing; bodies capped `2*128 KiB + 16 KiB`, `deny_unknown_fields`, bounds enforced pre-scan; no panics observed | [RV-T] (takeover rpc-api lane, tee'd `results.txt`) |
| Reorg/crash/retention inconsistency in MDBX storage | Reorg rollback completeness (depth-1 reorg removes old canonical entries, installs replacement, moves owner-index exactly); staged-reorg atomic abort restores durable state byte-identical and survives reopen; SIGABRT crash recovery resumes at h3 root-matched and accepts block 4; retention boundaries exact at documented depths; historical-window (`TargetOutsideUndoWindow`) and snapshot-boundary checks hold; supply corruption does NOT let an invalid block validate — roots/counters/PoW still bind | [RV-T] (takeover storage-reorg lane, tee'd `results.txt`) |
| NFT marketplace theft/duplication/race/accounting flip (live surface) | Stale purchase replay, duplicate listings/registration, settlement replay, double reset all rejected; tampered/dropped requests fail queue-commitment recomputation (`Queue`); frozen-round instant-buy/cancel rejected (`Phase`); duplicate items + metadata mutation rejected (`NftConservation`), and the real `bind_native_kernel` R1CS accepts the honest transfer while rejecting the metadata-mutated one; non-owner mint/transfer/offer rejected; timelocks enforced natively and in-circuit; round economics (synthetic escrow below the profile gate) exact to the micronoid — winner = `min(payer_priority, root, key)` recomputed independently, fee shares `q + (i < r)` in funded-key order, refunds exact, merged payouts, supply burn == fee; window/refund/reset/recovery phase machine holds; worst-case fees (90% occupancy, 8x pressure, 1..=16 requests) stay far under the 50k/request cap and 100k credit | [RV-T] (takeover nft-marketplace lane, tee'd `results.txt`, 10/10 tests; round-economics rows are below the profile gate — see U20) |
| Token-launch theft/inflation/duplicate-supply/slot confusion | Clean negative on real transition APIs + real `ChainState`: issuance launch-only (`CreateToken`/`TopUpReserve`/`BootstrapPool`/liquidity all profile-disabled; `route=Standalone` cannot validate); reserve must equal exact minimum `s/1000` (underfund `Backing(BelowMinimum)`, overfund `Semantic(Funding)`); graduation handoff exact for every legal supply (sale=3s/5, dex=2s/5, `curve_cash(3s/5) == s/250 ==` pool base, liquidity permanently locked, `escrow + curve_cash == pool + fee + claims`, reserved 40% unreachable on curve); redemption floor-exact with two conservation-neutral thefts rejected (`outstanding-1` -> `Semantic(Reserve)`; `reserve-1/payout+1` -> `Opening(BelowMinimum)`); creation IDs distinct/tagged/namespaces disjoint and duplicate-supply via identical asset id is a pure double-spend (`State(Input)`); slots store exactly `(amount, cid, root)`, raw-NOID carrier -> `SlotBinding`; strict-subset settlement -> `Semantic(Queue)`, cross-round scan -> `Semantic(Round)`, state root unchanged; full lifecycle conserved (`7e10+1e10+1e10+5e9+5e9 == 1e11`); +1 micronoid rounding routes into the burned fee, never a recipient | [RV-T] (takeover token-accounting lane, tee'd `results.txt`, `ALL ASSERTIONS PASSED`) |
| DEX ordering/extraction games (tail snipe, sandwich, head snipe, fee priority) | No ordering bug or extraction vector: every race resolves by a named deterministic rule; the one capturable prize (the free tail) is exactly 1 µNOID — the entire cumulative rounding surplus — against a 9,800 µNOID fee floor, unsellable standalone (dust floor, min lot 102); pool opens exactly at the curve's terminal marginal price (no transition seam; fair pre-grad roundtrip loses 12 µNOID); launch head is a ceiling overcharge (0.9967 µNOID value for 1 paid); fees buy nothing (1,000x fee changes no digest, outcome, or charge); authorization-key grinding order-neutral (reversed key order still executes in digest order) | [RV-T] (takeover dex-games lane, tee'd `results.txt`; library-level — miner/mempool behaviors read + deterministic core reproduced, no node executed) |
| Mempool replacement/double-admission games; batch-key grinding as priority attack | No replacement path anywhere (absence asserted verbatim in the pinned source); conflicting admissions -> `InputConflict`/`OutputConflict`; the grindable `authorization_id` sort is encoding-canonical only (PoC both lanes); one UTXO cannot buy two draws (`DuplicateFunding`); batch cap exact at `MAX_SEQUENTIAL_TRADES` (64 accepted, 65th `Bounds`); trade `funding_priority` binds the real parent (`TRORD_21` over `parent_id`) — no offline pre-parent grinding of the target block; NFT rounds height-bound (round id changes with registration height; request bound to round(h) fails at h+1); zero timestamp reads in the native logic | [RV-T] (takeover sequencing lane, tee'd `results.txt`, 25 assertions incl. 19 verbatim source-fact assertions) |
| Second satisfying witness / forged Pruv execution (the circuit is the sole consensus executor) | **Witness uniqueness held**: 8,870 build-and-check configurations (10 builder configs x both arithmetic variants of the real production R1CS, `bind_program` + `bind_program_v2_1`, over 120 random programs at u64 boundaries + 9 directed cases) with 0 failures; forgery battery (wrap-add/wrap-sub, non-boolean predicates as-true and as-false, writes from inactive instructions, skips of active ones, stale-next under tampered context, all 128 single-bit flips of a nontrivial next state, every reserved encoding, immediates >= 2^64) all rejected. Blocker is STRUCTURAL: rows appended in wire order with C = I (each wire defined by its row over earlier wires); the only two-valued rows (booleanity) sit in gadgets whose sibling pins force uniqueness; range decompositions injective via tower-basis linear independence of phi(2^i); selector families telescope to Sum = 1; both carry chains are exact char-2 majority algebra | [RV-T] (takeover constraint-systems lane, tee'd `results-ip.txt`/`results-puv.txt`/`results.txt` + in-tree controls 5/5 and 4/4 re-run; assurance limit: structural + directed battery, NOT an exhaustive witness-space search of the full HistoryStep relation, no end-to-end recursive prove/verify cycle) |
| Field-wrap / negative-value attacks; public-input and gate substitution in contract execution | No range/field wrap anywhere — all lanes range-checked (u64/u32/20/64-bit), arithmetic carry-checked in native AND both circuit variants, native fails closed identically (`p-1` attack structurally unavailable; Z3 checker encoding faithful); public-input binding end-to-end (program/current/next -> single-bit-sensitive object-root sponge wraps -> body owner leaves -> TX8X2 -> count-bound TXROOT -> header); `live`/`terminal` gates are authenticated bitmap bits (live=0 ghost behavior gated on committed data; component context-advice design pinned one level up by in-tree full-relation tests) | [RV-T] (takeover constraint-systems lane; tee'd results) |
| Fresh-request forgery (all lanes) | Owner/Phase-A/Phase-B adaptive ordering; queries derived only after all commitments; no accepted unauthorized statement anywhere | [RV] (root, cumulative: "no unauthorized spend, invalid accepted block, owner-key recovery, or fresh-request forgery", 01:58:28) |

---

## 5. Open escalation paths (the root's stated finish lines, narrowed by the takeover)

The root's original finish lines (01:50:29, 01:56:34, 01:58:28) were: (a) valid `CAPSMIX`
preimages for the colliding `TAG_CAPSNODE` children, (b) a caller that authenticates
attacker-supplied child digests without recomputing descendants, (c) cross-protocol digest
reinterpretation. The takeover's leaf-reachability lane has since **closed (a) against the
attacker** and **narrowed (b)/(c) to external consumers of the public raw APIs** — the only live
route left.

1. **[LEAD, open] External consumers of the public raw APIs.** The only remaining finish: audit
   every *external* consumer of `verify_source_batched_merkle_proof_to_cap`,
   `noid_ivc_core::merkle::hash_pair`, and the serialized `MerkleCap` type — SDKs, bridges,
   indexes/caches keyed by cap bytes — where the C21 cross-domain commitment (identical bytes,
   two semantics, one cache entry) becomes an exploit. In-tree the route is exhausted: the root's
   caller sweep found no unguarded production path, and the leaf-reachability sweep confirmed no
   in-tree caller compares digests across domains (the tower-digest bridge is `#[cfg(test)]`).
   The raw APIs remain public and accept authentication objects the semantic protocol would
   reject (root, 01:50:29); cross-protocol reinterpretation (root 01:54:09) now reduces to this
   same question. If no external consumer exists, the rebuttal's final position is the lane's
   blocker statement: a principled leaf wall, an accidental node breach, and a shipped comment
   that promises the wrong thing.
2. **[CLOSED against the attacker] Valid `CAPSMIX` preimages for the colliding children — the
   leaf wall.** Leaf sponges (`CAPSMIX`/`CAPSLEAF`/`CAPS256`/`IVCPCSL_`/`IVCPCSF`) load capacity
   IVs (ASCII labels, min 63 tower bits — IV census in `results.txt`) **outside every invariant
   subfield**, and absorbs never touch the capacity lanes; with `P(K^4) = K^4` and `P`
   bijective, the sponge state provably never enters `K^4`. Verified: 65,536 fully-aligned
   leaves -> 0 aligned digests, 0 intermediate states with aligned capacity, simulation
   byte-identical to the real `capsule_leaf_hash_mixed`; control: the node compressor on aligned
   inputs -> 100% aligned (4096/4096). **Economics replaces the swarm's 2^224 estimate:** any
   aligned lift through GF(2^d) costs `2^(4(128-d)+d)` — `2^464` / `2^416` / `2^320` for
   d = 16/32/64 — ALL strictly dominated by the generic `2^128` whole-node birthday between two
   honest commitments; the semantic layer keeps the full generic `2^128` collision resistance
   even though the node layer offers only `2^16`. More computing on aligned node collisions can
   never pay. Scope caveat (lane assurance limits): "dominated" covers the aligned-lift routes
   through the subfield towers, not a lower bound against all conceivable attacks. The
   **BadTypedBind formal question is now ANSWERED — as a fork their corpus never resolves**
   (C23): under the dictionary reading the same-tag collision INSTANTIATES and the C21 claw
   INSTANTIATES (collision + domain confusion simultaneously) and FORCES Delta above both
   sufficient-condition thresholds; under the worklist reading both are excluded by clause and
   Delta stays unevaluated everywhere in their corpus. Either way the shipped claim does not
   survive: false or vacuous.

**Collision confinement (constraint-systems lane).** The precise answer to whether the
fold/constraint bindings consume attacker-chosen digests through the colliding domains:
**YES at the fold/PCS layer** — the prover of a block chooses the capsule/PCS commitments, and
the child proof's in-circuit replay (`ff_merkle`) verifies openings against the
transcript-observed root (confirming the basefold-chains exposure; exploitability there
depends on FS query unpredictability). **NO at the semantic layer** — no wire of the
Pruv/object path (program, state, context, amounts, recipients, owners) is ever hashed under
`compress_flat_feed_forward_with_tag` or any tree built with it; the semantic chains all use
full-sponge constructions (`compress_with_tag` for state/tx-root/segment/claim nodes,
one-permutation duplex wraps for object roots). Empirically: the known colliding pair does NOT
collide under `TAG_EXSTNOD`, `TAG_TXROOT`, `TAG_SEGMENTTREE`, or `TAG_CLAIMS`; the object-root
wraps are single-bit sensitive in every committed cell (32/32 program words move the code
digest and both roots). Composing the 2^16 collision into contract-semantics forgery would
require a collision in a full-sponge two-permutation wrap — a different cryptanalytic problem;
the two never meet.

**Algebra note (leaf-reachability lane, characterizes C1/C2/C21).** All permutation round
constants and MDS multipliers live in `K = GF(2^16)` (max tower encoding 14 bits, from
`MDS_PARTIAL`'s `0x2000`); `K^4` is invariant under `permute_flat_u128` and the restriction is
exactly the same Poseidon2 schedule over GF(2^16) — GF(2^16) is the smallest invariant subfield
(GF(2^8)^4 and GF(2^4)^4 are not invariant). The 1-perm node compressor whitens only the right
input, so `left in K^2, right in IV + K^2` pins the whole state in `K^4`: a 64-to-32-bit
function whose collisions are a `2^16` birthday — which is what the swarm observed (72,274 /
111,525 / 111,423 evaluations; a fresh zero-left collision regenerated live in 167,686
evaluations / 0.29 s). It is **not** affine-solvable: `x -> x^7` is not K-additive in
characteristic 2, so collisions are found, not solved. Guardrail asymmetry, demonstrated: the
two-permutation `compress_with_tag` (EXSTNOD state trees) seats the IV in the capacity lanes
before the first permutation, and 4096/4096 tower-aligned inputs yield 0 aligned digests — same
constants, same permutation; the *schedule* is the difference (corroborating C1's scope note
that the exact state tree is unaffected).

Secondary unfinished threads: a live production caller for the legacy-FRI malleability (C5);
a valid height-4 `HistoryStep` terminal to close the snapshot drift differential (U4); root-style
verification of the build-artifacts remainder — U1 was closed by the takeover verifier (now C16,
`VERIFY.md`), U2's decoder leg is empirically covered by that run while its cache-persistence
scenario stays code-level, and U3 (profile override) remains unverified — the external-miner
finding (formerly U5) was closed by the takeover root's rerun and is now C15; the
**alternative-witness / constraint-completeness residual is ANSWERED for the layer it was aimed
at** — the constraint-systems lane holds witness uniqueness on the Pruv integer-program circuit
(the sole consensus executor of contract semantics): structural blocker (C=I wire-order rows +
gadget algebra) plus the 8,870-configuration directed battery, with assurance limits
(structural + directed battery, NOT an exhaustive witness-space search of the full HistoryStep
relation; no end-to-end recursive prove/verify cycle was run). The sequential-trade and
listing-settlement witness legs keep their earlier cross-references (dex-economics
honest-witness agreement + the ordering map; listing-settlement circuit still code-read only);
the collision-composition leg is closed by the confinement paragraph above; the **listing-settlement circuit's native-vs-recursive
consistency is code-read only** (winner rule, ordering, fee-split arithmetic, timelocks match by
reading; the `bind_financial_listing_settlement` witness was not built) — same cross-reference,
constraint-systems lane; the **sequential-trade route (operation 80, `ExecuteTrades`, the live
trading kernel)** — now **ANSWERED** by the dex-games and sequencing lanes: the ordering/priority
map is complete (U22-U27; execution order is the funding-priority digest, the builder owns every
consensus-level race, and no fee- or value-based priority path exists in the native paths); the
**tx-admission cost-asymmetry
question is now ANSWERED** — C22 (62x CPU discount, anonymous occupancy of all bounded
admission resources) plus U21 (gossip-window exhaustion), both from the tx-admission lane;
its own follow-up is the live multi-peer swarm demonstration, which **composes with C17**
(both anonymous, both attacking the shared 64 MiB domain and the delivery paths) and is
cross-referenced to devnet-chaos; and the root's own framing
question — whether authorization is *supposed* to be fork-specific — which decides C6's severity.
Token-accounting residuals (lane assurance limits): block-level admission binding, recursive-circuit
equivalence beyond the graduation-constant spot-check (constraint-systems lane), DEX price
adversarials (dex-games), fee-schedule fairness, and NFT accounting (nft-marketplace lane).

---

## 6. Lane status table

All 31 experiment directories under `/Users/ember/dev/parano1d-break/experiments/` — the kimi
swarm's 25 plus takeover-opened lanes (`leaf-reachability`, `tx-admission`, `dex-games`,
`sequencing`, `constraint-systems`, `typed-bind-verdict`, and later additions). "Completed" = lane returned a final result the root processed
(verification status per §2/§3). All 8 lanes the kimi swarm left dead-with-partial-code have now
been completed by the takeover swarm (wallet-safety, prover-unsafe, p2p-resources, dex-economics,
rpc-api, storage-reorg, nft-marketplace, token-accounting — [RV-T] per the 2026-11-15 ruling:
takeover-completed lanes with tee'd `results.txt` count as verified); zero kimi-era lanes
pending.

| Lane dir | Agent report | Status | Result summary |
|---|---|---|---|
| `address-preimage` | agent-9 | **completed [RV]** | Negative: no unrestricted key recovery; weak-key class only (`results.txt`: `conclusion=low_entropy_weakness_only_not_a_uniform_key_attack`) |
| `basefold-chains` | agent-5 | **completed [RV]** | C2: `IVCPCSN_` collision; no end-to-end BaseFold break |
| `block-validation` | agent-12 | **completed [AR]** | Negative matrix + informational standalone-utility epoch gap (U7); root rerun not recorded |
| `build-artifacts` | agent-25 | **report + takeover-verified [RV-T/AR]** | U1 confirmed as C16 (CONFIRMED-WITH-NARROWER-SCOPE, `VERIFY.md`); U2 decoder leg empirically covered by verifier run 2, cache-persistence scenario still [AR]; U3 still [AR] |
| `capsule-chains` | agent-4 | **completed [RV]** | C3, C4; full PCS rejects everything (`Accepted altered full PCS openings: 0`) |
| `contract-rights` | agent-14 | **completed [RV]** | Negative: 50k Pruv programs / 428,880 executions, 0 mismatches |
| `dex-economics` | agent-26 (takeover) | **completed [RV-T]** | Negative: no accepted value-extracting transition (9/9, `"accepted_value_extraction_found": false`); U16 informational boundary observation (free final curve unit / 0-NOID graduation trigger, not extractable); host-vs-recursive 72+120 cases match; §4 guardrails + §5 alternative-witness cross-ref. Process note: the dead agent's harness had never completed a run — 4 bugs fixed, incl. a false "panic" that was its own assertion bug against legitimate flat-ceiling semantics; a `cargo fmt` incident on `pinned/` was reverted and re-verified byte-identical to `git archive 52938433` |
| `dex-games` | (takeover, new) | **completed [RV-T]** | MEV posture (all Informational, no ordering bug or extraction vector): U22 tail snipe (prize = 1 µNOID surplus vs 9,800 µNOID fee floor), U23 graduation timing, U24 launch head overcharge, U25 fees-buy-nothing / UTXO-inventory priority, U26 builder owns every race at consensus; ledger CORRECTION (authorization-key sort = encoding only; execution = funding-priority digest); §4 negative row | 
| `sequencing` | (takeover, new) | **completed [RV-T]** | Five-layer ordering map + SEQ-1..SEQ-5 (SEQ-1/SEQ-2 Medium, v2.1-conditional: UTXO-count lottery + free txid grind for fee-pinned natives); verbatim public claim recorded; §4 no-RBF/no-key-grind row; cross-refs ledger-fairness, C15, C6 kept clean |
| `constraint-systems` | (takeover, new) | **completed [RV-T]** | Witness uniqueness held (8,870 configs, 0 failures; structural blocker); no range/field wrap; public-input binding end-to-end; §5 collision-confinement answer (YES fold layer / NO semantic layer); U28 Low process finding (v2.1 path untested in-tree); architectural note (circuit = sole consensus executor); assurance limits carried |
| `typed-bind-verdict` | (takeover, new) | **completed [RV-T]** | C23 (kills the shipped security claim as stated: paper never contains `Delta_P2b`/`0.45067`; `BadTypedBind` stated twice, never disambiguated; single-`v_node` dictionary vs shipped multi-tag layer; verdict fork with FORCES-DELTA numbers); strongest licensed claim recorded verbatim; §5 BadTypedBind thread closed (fork); no `results.txt` by design (definitional analysis quoting prior PoC evidence) |
| `external-miner` | agent-20 | **completed [RV-T, C15]** | Template-consumption finding (Medium); reproduced by the takeover root 2026-11-15 (`verify-run.txt`); kimi root never verified this lane |
| `fork-replay` | agent-7 | **completed [RV]** | C6: same-anchor fork replay, byte-identical reruns |
| `identity-encoding` | agent-16 | **completed [RV/AR]** | C13 root-verified; proof-identity/terminal-id/Bech32 results agent-reported (U8) |
| `ledger-fairness` | agent-18 | **completed [RV]** | C10-C12; root reran successfully |
| `leaf-reachability` | (takeover, new) | **completed [RV-T]** | C21 (cross-tag claw, refutes the `compression.rs:319-325` design claim); leaf-wall theorem closing §5 finish line (a) against the attacker; algebra note + economics replacing the 2^224 estimate; evidence `README.md`/`results.txt` (3 binaries, all exited 0) |
| `native-intents` | agent-8 | **completed [RV]** | Negative: worst-fill accusation disproved at both host and recursive boundaries |
| `nft-marketplace` | agent-27 (takeover) | **completed [RV-T]** | U20 (Informational: round-escrow + offer families unreachable at this pin — profile gates `operation_enabled`/`kind_enabled`); §4 lifecycle/race/conservation/round-economics negatives; §5 residuals (settlement circuit code-read only; op 80 not executed). Process note: the dead agent's "native accepts / recursive rejects `CreateCollection`" PoC **mis-modeled production** (both live trace constructors keep the funding row's real slot/packed/root — `actions.rs` `slot: row[4]`; `financial_base_projection.rs` `slot: mul(selected, action.slot)`) — the inherited claim is REFUTED; the finisher substituted a two-direction soundness control on the real `bind_native_kernel` R1CS (production-faithful witness satisfies; zeroed funding anchor cannot — genesis identity is cryptographically bound to the funding anchor) |
| `p2p-resources` | agent-21 (takeover) | **completed [RV-T]** | C17 (Medium, the audit's first unauthenticated-remote finding: mempool-pull domain pin), C18 (Low-to-Medium: header-sync memory unbudgeted); §4 codec negatives; evidence `README.md`/`results.txt`. Note: the dead agent's inherited PoC had a false-positive headline — its "object write blocked" assert was a cooperative-yield scheduling artifact in code never actually run; the finisher caught it and converted it to an explicit control (real-time wall-clock shows the object codec write completes during the pin) |
| `prover-unsafe` | agent-24 (takeover) | **completed [RV-T]** | U13 (Low-to-Medium latent: zero-round provers emit never-written memory; proofs self-reject; not production-reachable), U14 (Low: degenerate raw-API panics), U15 (Low unreachable: parser allocation amplification); §4 kernel-differential negatives; evidence `README.md`/`results.txt` |
| `recursive-sync` | agent-13 | **completed [AR]** | U4: tip-only future-drift admission differential; root rerun not recorded |
| `rpc-api` | agent-19 (takeover) | **completed [RV-T]** | C19 (Medium, credential-gated: full receipt-store rescan + Merkle re-verification per call, 4.1-5.6 s at 20k receipts), U17 (Low: out-of-range page key-set retention), U18 (Low-Medium: inline scan starves async workers, 3078x tail — mirror-verified, split carried); §4 RPC negatives. Process note: the dead agent's draft never compiled (three wrong imports, never run); the finisher fixed it, verified the core hypothesis, and reframed — the draft treated far-page heap retention as the finding, the real story is the per-page rescan cost |
| `state-consensus` | agent-11 | **completed [AR]** | Negative 20-case matrix (U6); root rerun not recorded |
| `storage-reorg` | agent-22 (takeover) | **completed [RV-T]** | C20 (Medium: unauthenticated supply counter on compact restart — silent upward corruption + propagation, permanent spend-admission stall on downward), U19 (Low-Medium: noncanonical 41-byte chain_tip accepted at restart, every later commit fails `store: mdbx: 41`); §4 reorg/crash/retention negatives. Process note: the dead agent's PoC had two defects fixed before recording (hardcoded segment id 0 vs the actual 148; +1 s block spacing would compound ASERT ~100x — switched to 20 s ideal) |
| `token-accounting` | agent-28 (takeover) | **completed [RV-T]** | Clean negative (informational): issuance launch-only and exactly minimum-backed; graduation handoff exact for every legal supply; redemption floor-exact; creation-ID discipline and slot binding tight; multi-asset scans atomic; fees burned not pocketed; §4 row with named guardrails. Process note: `cargo fmt` follows path deps into `pinned/` and rewrote 3 reference files mid-session — restored byte-exact, all 470 re-verified, README warns future runners; the lane's `build.rs` refuses to build unless HEAD == pin and all 470 files byte-match (worth adopting as reproducibility practice) |
| `transcript-chains` | agent-6 | **completed [RV]** | C5: legacy-FRI malleability + PAD ambiguity; framed channels hold |
| `tx-admission` | (takeover, new) | **completed [RV-T]** | C22 (Medium, anonymous P2P: 62x CPU-discount admission occupancy — second unauthenticated-remote finding), U21 (Low-Medium: global gossip byte window shared with block announcements, exhaustible by ~5 garbage identities, code-argued not executed); §5 thread closed + devnet-chaos composition note; evidence `README.md`/`results.txt` |
| `typed-binding` | agent-10 | **completed [RV]** | C14: 4-path witness; `BadTypedBind` not instantiated |
| `uc-composition` | agent-17 | **completed [AR]** | U9: informational assurance gap only |
| `version-domains` | agent-15 | **completed [RV]** | C9: profile-ID collision across schedules; downgrades reject |
| `wallet-safety` | agent-23 (takeover) | **completed [RV-T]** | U10 (Low hardening: substitution undetectable downstream), U11 (Low: clipboard persistence), U12 (Informational: stale in-memory wallet, no production path); §4 secret-surface negatives; evidence `README.md`/`results.txt` (5 Rust + 7 Python tests pass) |

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

---

## 7. Questions not asked — a coverage map

Ember's observation: the kimi-era briefs all asked "is an invalid transition accepted?" in
different disguises, so each negative result is bounded by that question. This section extracts,
from every lane's own assurance-limits section (README where present, agent-report limitations
otherwise), (1) the question the lane did NOT ask, stated as a question; (2) the currently-running
lane that owns it (malicious-node, constraint-systems, dex-games, sequencing, devnet-chaos,
contract-reorg, wallet-vs-node — tx-admission has completed and its question is answered); and
(3) for unowned questions, whether they are orientation-proof (malice/races cannot change the
answer — deterministic math, hygiene, or proven theorems) or a live re-ask candidate. This is a
coverage map, not a critique of lane quality.

| Lane | Unasked question | Owner | Unowned: orientation-proof or live |
|---|---|---|---|
| address-preimage | Is there a nonlinear/algebraic (CICO-style or invariant) attack on the full 66-round ADDRFIX permutation cheaper than 2^256 classical / 2^128 Grover? (subfield, Frobenius, inversion, weak-key directions exhausted) | — | **LIVE** (pure cryptanalysis) |
| capsule-chains / agent-0 | Can a malicious cap carry fold-valid openings on a fraction of query positions so nonce-grinding selects weak query sets (2^16/rho^65)? | — | orientation-proof — premise (fold-valid forests) blocked at generic cost by the leaf wall |
| state trees / agent-1 | Does snapshot verification hold under concurrent reorg at production scale? | devnet-chaos | — |
| transcript PCS / agent-2 | Do the Fiat-Shamir/binding properties hold under executed adversarial inputs, not only source reading? | constraint-systems | — |
| fact-check / agent-3 | Is there a practical collision attack on the PADDED `finalize()` construction (`TAG_BYTEHASH`) exploiting the small constants (the 2^16.3 sponge argument covers only no-pad/aligned schedules)? | — | **LIVE** (cryptanalysis) |
| basefold-chains | Does a complete recursive proof run accept altered paths (only the native path-column builder was executed)? | constraint-systems | — |
| basefold-chains | Do chosen-prefix collisions involving actual leaf hashes exist below generic cost? | — | orientation-proof — leaf-wall IV census covers `IVCPCSL_`/`IVCPCSF` |
| block-validation | Do the acceptance results hold with full recursive HistoryStep terminal substitution and the native financial families? | constraint-systems | — |
| build-artifacts | Does a substituted legacy matrix actually diverge consensus between two running nodes (beyond the demonstrated trust-boundary failure)? | devnet-chaos + malicious-node | — |
| contract-rights | Do contract rights hold under reorg, concurrency, and real inclusion heights? | contract-reorg | — |
| dex-economics | Does the fee curve behave identically at other state geometries (one geometry sampled)? | — | **LIVE** (small, measurement) |
| dex-economics | Adversarial priority/sequencing collisions beyond repeated-execution determinism? | sequencing | — |
| external-miner | Does the full recursive `PreparedBlockAttempt` path have the same consume-before-validate cost profile, and does live JSON-RPC authorization behave as reviewed (no live server was started)? | — | **LIVE** (needs a running node; partially malicious-node adjacent) |
| fork-replay | Does the same-anchor replay survive full node/block acceptance, PoW, transaction roots, and multi-peer propagation? | devnet-chaos | — |
| identity-encoding | Can two same-txid intents race mempool admission? | answered (tx-admission): first-seen-wins, no replacement path | — |
| ledger-fairness | Do the liveness behaviors hold with real authorization execution and production-size state (accept-all executor + small fixtures used)? | devnet-chaos | — |
| leaf-reachability | Is there an out-of-tree consumer of the public raw APIs (`verify_source_batched_merkle_proof_to_cap`, `hash_pair`, serialized `MerkleCap`)? | — | **LIVE** (the §5 lead thread) |
| leaf-reachability | Can an unrelated flaw in the affine LCH code or fold algebra beat the leaf wall (outside the dominated aligned-lift routes)? | constraint-systems | — |
| nft-marketplace | Does the listing-settlement circuit hold as an executed witness (code-read only), and does op 80 `ExecuteTrades` hold under adversarial sequencing? | constraint-systems; dex-games + sequencing | — |
| nft-marketplace | Is round-context binding to real header windows (`FinancialHeaderWindow`) sound beyond synthetic anchors? | devnet-chaos | — |
| native-intents | Do market-state games beyond the tested fixtures exist (adverse parent-state construction)? | dex-games | — |
| p2p-resources | Do C17/C18 hold against a live libp2p swarm (negotiated yamux windows, measured timeouts), and do eclipse/Sybil topologies beyond the diversity caps change reachability? | devnet-chaos | — |
| prover-unsafe | Do the x86 kernels (`ghash_mul_clmul`, AVX-512, `mul_f256_avx2`) agree with the portable implementations (aarch64-only execution here)? | — | orientation-proof (deterministic differential; run on x86) |
| prover-unsafe | Does a maximum-shape (256 KiB) decodable-but-invalid bundle cost more than 8.34 ms to reject? | — | orientation-proof (fixed 65 queries; measure once) |
| recursive-sync | Does the tip-only drift admission hold with a real height-4 HistoryStep terminal and the full daemon (retirement keys prevented the full build)? | constraint-systems (witness) + devnet-chaos | — |
| rpc-api | Does the production (binary-private) wallet scan exhibit the mirror's lock-shape stall, and at other worker counts / during reorg windows? | devnet-chaos (reorg-window half) | production-lock-shape half: orientation-proof (code identity, no adversary dependence) |
| state-consensus | Do financial ops, reorg/snapshot paths, and resource exhaustion hold beyond the synthetic matrix? | dex-games + devnet-chaos; resource exhaustion answered (tx-admission) | — |
| storage-reorg | Do mid-transaction crashes (inside the MDBX commit window) and concurrent online corruption behave like the offline/inter-commit cases tested? | devnet-chaos | — |
| token-accounting | Is fee-schedule fairness across the full state-geometry range and block-level admission binding sound? | — (fairness half) | **LIVE** (small); admission binding half: devnet-chaos |
| transcript-chains | Is there any external/downstream caller of the legacy `noid_fri` verifier (none found in-tree)? | — | **LIVE** (same external-consumer thread as leaf-reachability) |
| tx-admission | Do C22/U21 hold against a live multi-peer swarm, composed with C17? | devnet-chaos | — |
| typed-binding | Does a CAPSNODE collision or the C21 claw instantiate `BadTypedBind` under their own definitions (their evaluator never run)? | — | **LIVE** (formal lane; inputs already in hand) |
| uc-composition | Is there an online-UC extraction theorem (or a counterexample) for a specified `F_auth`, and can permutation-state collisions occur beyond domain labels? | — | **LIVE** (research) |
| version-domains | Does a version change race verification asynchronously (eviction path reviewed, no PoC)? | answered-in-part (tx-admission: first-seen-wins, block-driven eviction); residual: devnet-chaos | — |
| wallet-safety | What does the wallet see when the node binary itself is malicious or compromised (the substitution-blindness gap is only reachable then)? | wallet-vs-node | — |
| dex-games | Do the latency games (observe parent, re-anchor, submit before template snapshot) behave as characterized from code, and is digest uniformity a theorem rather than a statistical observation? | devnet-chaos (latency half) | uniformity half: orientation-proof (statistical property, no adversary dependence) |
| sequencing | What are the real latency/bench numbers for the "reach the builder before template build" race, and how does the B25/B255 class-budget interaction behave under load? | devnet-chaos (latency half) | B25/B255 budget interaction: unowned, **LIVE** (small, load test) |
| constraint-systems | Does witness uniqueness survive an exhaustive witness-space search of the full HistoryStep relation and a complete end-to-end recursive prove/verify cycle for a Pruv-carrying block? | — | **LIVE** (prover-scale run; structural argument + directed battery only) |
| constraint-systems | Is there a collision in the full-sponge two-permutation wraps (`compress_with_tag`, object-root duplexes) — the only cryptanalytic bridge from the 2^16 node collision to semantics? | — | **LIVE** (cryptanalysis; `noid_soundness` territory; same family as the fact-check lane's padded-sponge item) |
| typed-bind-verdict | (No runtime question — definitional lane; the BadTypedBind fork IS the answer.) | — | — |

Unowned + orientation-proof (no adversary orientation needed, only unfinished deterministic
coverage): x86 SIMD kernel differentials, 256 KiB max-shape verify cost, production-linked
wallet-scan lock-shape confirmation, fold-valid-forest premise (leaf wall).
