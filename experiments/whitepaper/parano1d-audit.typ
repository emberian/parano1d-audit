// parano1d adversarial audit — whitepaper
// Source of truth: experiments/FINDINGS.md (consolidated record) + PROVENANCE.md.
// Discipline: no claim beyond what a PoC demonstrates; certainty never upgraded;
// numbers verbatim from FINDINGS.md.

#set document(
  title: "An Adversarial Audit of parano1d",
  author: ("ember", "parano1d-break audit swarm"),
)
#set page(paper: "a4", margin: (top: 2.2cm, bottom: 2.2cm, x: 2.05cm), numbering: "1")
#set text(font: "New Computer Modern", size: 10.5pt, lang: "en")
#set par(justify: true, leading: 0.62em)
#set heading(numbering: "1.1.")
#show table: set text(size: 8.3pt)
#show heading.where(level: 1): it => { v(1.0em, weak: false); it; v(0.45em) }

#let todo-box(title, body) = block(
  fill: rgb("#fdf3d8"),
  stroke: (left: 3pt + rgb("#c99700")),
  inset: 9pt,
  radius: 3pt,
  width: 100%,
  above: 0.9em,
  below: 0.9em,
)[#text(weight: "bold")[TODO — #title.] #body]

#align(center)[
  #text(size: 21pt, weight: "bold")[An Adversarial Audit of parano1d]
  #v(0.4em)
  #text(size: 12pt)[Hash collisions, node trust, and the price of malfeasance]
  #v(0.4em)
  #text(size: 10pt)[Target commit `52938433a30b185ab007afaa15166a632cf2e82e` — audit of 2026-11-15]
  #text(size: 10pt)[ember and the parano1d-break audit swarm]
]

#v(0.9em)

= Abstract

parano1d is a proof-of-work Layer-1 blockchain — the `noid_*` Rust crates by `@ignotus_nemo` — that replaces transaction signatures with zero-knowledge authorization proofs, hashes everything through one Poseidon2-style permutation over binary tower fields, checks blocks with recursive proofs, and ships a node, a GUI wallet, and native token/NFT/DEX layers. On 2026-11-15, in two passes (a 29-agent swarm whose coordinator died on quota, and a takeover swarm that completed every remaining lane — 37 experiment lanes, three of them landing as this document was being drafted), we ran an independent adversarial audit against the pinned commit named above; every positive claim is a local, loopback-only proof of concept against real production code paths, and negative results are recorded with the same care as positives. The honest verdict: parano1d's production hash compressor fails collision resistance at a cost of about `2^16` evaluations — found empirically in 72,274 evaluations (0.128 s) — its own shipped source comment promising that "distinct trees cannot share collisions" is refuted by a digest valid in two hash domains at once, and its executable soundness certificate is, for the object it actually ships, either false or vacuous. Yet every route from those primitive breaks into protocol semantics was blocked by named guardrails: no unauthorized spend, no invalid accepted block, no key recovery, no forged fresh request, and a proven "leaf wall" keeps the collisions out of the layer where money is decided. The consequential failures sit elsewhere: two unauthenticated strangers on the network can starve a node's serving and admission paths at a 62x CPU discount; a patient peer can permanently poison a fresh node's first synchronization and turn it into a distributor of the poison; the wallet verifies nothing and renders whatever its node says as AUTHENTICATED; the release builder authenticates none of the bytes it embeds on its default path; and the protocol sells permanent state carriage at a price that never scales. Not "definitely broken" — but not results anyone should find satisfying.

= How we worked

*Target and scope.* The audited object is the parano1d checkout at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`, treated as a read-only reference. All demonstrations run locally and on loopback against that tree: real production codecs, real chain store, real verifiers, the real GUI backend, synthetic identities and keys, our own devnet data directories. No live network, mainnet peer, or third-party user file was touched. The upstream `main` (as of the audit) still contained the vulnerable compressor unchanged and its test suites pass — the failures documented here are design and parameterization failures, not a broken build.

*Method.* Attack lanes were briefed narrowly, required to run their PoC against production code (a lane that only re-derives an interface from prose is a failure mode we specifically guarded against), and required to record what they could *not* do. A fact-check lane audited the swarm's own earlier summary and its corrections are binding on this document; the three that matter most for the reader:

- The exact UTXO state tree does *not* use the vulnerable compressor (details in §3.2); the collision does not transfer into state-tree collisions.
- The "`2^16.3` collision cost" is a random-mapping ("birthday") *heuristic*, demonstrated empirically at roughly 72k evaluations — not a proven bound. Pigeonhole counting alone only guarantees that *some* collision exists by `2^32+1` evaluations.
- The padded-sponge finalization is *not* claimed to be subfield-confined, and the project's ideal-model theorems are not "logically false" — the accurate criticism is that the model is *unjustified for the shipped permutation* (§3.5).

*Certainty classes.* Every finding carries one, and it is never upgraded:

- *[RV] root-verified* — the coordinating agent independently re-ran the PoC and recorded the confirmation.
- *[RV-T] takeover-verified* — independently reproduced by the takeover side, or produced by a takeover-completed lane whose full run output is preserved on disk (`results.txt`).
- *[AR] agent-reported* — PoC output exists on disk, but the coordinator's verification was interrupted or never started.
- *[PC] partial/unverified* — code on disk, no report, no run evidence.

*Governance.* The first pass (a 29-sub-agent swarm) lost its coordinator to a quota cutoff mid-verification; the takeover pass re-verified what it could and completed all lanes the first swarm left dead. Nothing in this paper rests on an unverifiable claim: where evidence is [AR] or code-argument-only, we say so inline.

= The hash story, in plain words

== What a collision is, and what broke

A hash function is *collision-resistant* when finding any two distinct inputs that produce the same output is computationally infeasible; for a digest of n bits, the standard expectation is that an attacker needs about `2^(n/2)` tries (the "birthday" cost, from the birthday-party observation that matches are found far sooner than intuition suggests). parano1d hashes everything — transactions, state, proofs — through a single Poseidon2-style permutation, and builds Merkle trees (trees whose interior nodes are hashes of their children) out of small *compressor* functions: functions that fold input values into a digest. A *tag* is a constant label mixed into a hash so that hashes meant for different purposes cannot be confused with one another; that separation of purposes is called *domain separation*, and its value is measured in bits: a good tag separator should make cross-purpose collisions as hard as `2^128`.

Two production compressor domains fail collision resistance outright:

- *C1 — the authorization-PCS node hash (`TAG_CAPSNODE`)* *[RV]*: two distinct aligned input pairs produce the identical production digest `b2cf54a478131d318ebef1af2ec0e315245b8ee3af237a21e3fef04e4a20478a`. The search cost 72,274 hash evaluations / 0.128 s on a laptop. An independent lane regenerated a collision after the same 72,274 evaluations, and a later lane regenerated a fresh one live in 167,686 evaluations / 0.29 s — this is repeatable, not a lucky artifact. Anyone can re-check it without any search at all: a hardcoded verifier (`poc/src/bin/verify_collision.rs`) contains the two inputs, calls the production `compress_flat_feed_forward_with_tag(TAG_CAPSNODE, ...)` on each, asserts the inputs differ, and asserts the digests are equal.
- *C2 — the IVC/BaseFold Merkle domain (`IVCPCSN_`)* *[RV]*: a collision after 111,525 evaluations / 0.29 s (digest `f1ca994e8a1b9a79e5da33ad7015b44940e5b5d95739c87a3eeb1ba1b6b32c8b6`, first collision pair found at evaluation 15,641). Both distinct openings verify through the production `verify_merkle_proof`; "multiproof" batches sharing one root work; a recursive Merkle-path builder replays both witnesses to the same root; a fixed point where `H(a,b) = a` exists; and the raw API accepts path lengths 0, 1, or 2 for the same root.
- *C3 — a prehashed Merkle commitment collision at exact production geometry* *[RV]*: two distinct leaf vectors at the precise source-tree geometry of the production capsule PCS (depth 13, cap depth 3) produce the identical eight-hash commitment, and the production Merkle verification accepts both distinct openings. *Precision:* the colliding "leaves" are supplied digests, not valid `CAPSMIX` leaf hashes — no valid preimages for the colliding children were found (and §3.4 explains why none will be). This is a collision-resistance failure at the prehashed API, not a full capsule attack.
- *C4 — a leaf/internal-node alias* *[RV]*: one real `CAPSMIX` leaf digest also equals `CAPSNODE(0, X)`, so with caller-selected depth the same root authenticates both a depth-0 leaf proof and a depth-1 internal-node proof. Production callers pin depths, so this stays a raw-API type confusion.

What to make of the cost figure: under the random-mapping heuristic the expected collision cost on the aligned input class is about `2^16.33`; the observed 72,274 / 111,525 / 111,423-evaluation searches sit exactly at that scale. This is an empirical demonstration, not a theorem — but a demonstrated 0.128-second collision needs no theorem.

== Why it collides: the constants live in a small subfield

The permutation is built over *binary tower fields* — nested arithmetics where a 128-bit value can also be read as four 32-bit, or sixteen 8-bit, values. A *subfield* is one of those nested views. The fatal observation *[RV-T]*: every round constant and every MDS matrix multiplier of the permutation lives in `K = GF(2^16)` — a 16-bit subfield (the largest tower encoding of any constant is 14 bits). Consequently the four-lane state space `K^4` is left intact by the permutation, and the permutation restricted to `K^4` is exactly a Poseidon2 schedule over `GF(2^16)`. `GF(2^16)` is the smallest invariant subfield (the 8-bit and 4-bit views are not invariant).

The node compressor makes matters worse by design: it runs *one* permutation pass and whitens (pre-mixes) only the right input. Choose inputs aligned to `K^2 x K^2` and the entire state stays inside `K^4` — the compressor is, on that slice, effectively a 64-bit-in / 32-bit-out function. A collision in such a function is a `2^16` birthday, which is what the searches found. It is *not* cheaply solvable by linear algebra — the `x -> x^7` round map is not additive in this arithmetic — so collisions are found by search, not solved. And because the aligned compressor behaves as a 32-bit-output function, its collision cost is what a 32-bit function's always is: `2^16`. The defect is not that the search is clever; it is that production treats the digest as a 256-bit-domain commitment when its aligned slice offers 32 bits.

The saving grace, demonstrated in the same experiment: the *two-permutation* compressor `compress_with_tag` — the one the exact UTXO state tree uses, with tag `TAG_EXSTNOD` — seats its IV in the capacity lanes before the first permutation, and 4,096 of 4,096 tower-aligned inputs produced zero aligned digests. Same constants, same permutation; the *schedule* is the difference. So the break is confined to the one-permutation node domains: the state tree is unaffected, and `TAG_SEGMENTTREE` has no production call site at all. This is the honest boundary of C1/C2.

== The cross-tag claw: one digest, two domains

A *claw* is a pair of inputs from two different hash domains that land on the same output. Their shipped design comment (`noid_poseidon2b/src/native/compression.rs:319-325`) promises: "the caller-selected tag whitens the right-input lanes so distinct trees cannot share collisions." That promise is refuted *[RV-T]*:

- One 32-byte digest `9964c133d849e900a90f95543d0db8657f5fb3b5e7244c350e160f765ec0429a` is simultaneously a valid `TAG_CAPSNODE` node digest and a valid `IVCPCSN_` node digest (shared left child, distinct rights), found in 111,423 evaluations / 0.49 s.
- Two two-leaf prehashed trees — one per domain — commit to *identical bytes*, and each opening is accepted by the production raw verifier `verify_source_batched_merkle_proof_to_cap` under its own node hasher (`capsnode_raw_verify: accepted`, `ivcpcsn_raw_verify: accepted`, `roots_identical: true`).
- The tag domain separation is therefore worth about 16 bits on the aligned coset, not the ~128 bits the comment implies.

Scope, honestly: no in-tree caller compares digests across domains (the only bridge is test-only code), and as of 2026-11-15 a dedicated ecosystem probe found *zero* external consumers of the cross-domain raw verify surface — the verify routine is crate-private, all known out-of-tree consumers (mobile wallets, SDKs, archivers, pools) call single-domain wrappers or plain RPC, and nothing keys storage on these commitment bytes. So this is a latent footgun in public commitment-handling APIs, not a live exploit — with one recorded caveat: a production third-party hard-fork ("Jetsam"), live on its own mainnet since roughly September 2026, regenerated all 31 of its own domain tags but left the `IVCPCSN_` constant byte-identical, inherited this structure verbatim, and has already added its own direct in-circuit consumer of that node hasher. The class of bug is one refactor away from being live outside this codebase.

== Why it does not cascade: the leaf wall

Sixteen completed attack lanes tried to turn these collisions into protocol-level forgeries (an unauthorized spend, an invalid accepted block, a split-brain state). All failed, and the failure has a structure:

*First, verifiers re-derive content.* The capsule and BaseFold verifiers recompute the leaf hashes themselves (`Hmix`/`H256`), pin depths and path lengths, and enforce the fold equations *before* any Merkle check. Across every escalation attempt: zero altered full-PCS openings were accepted — collision bytes used as a real leaf fail (`InitialMerkleFailed`), shortened or extended paths fail (`InvalidProofShape`), reordered queries fail (`FoldMismatch`). A sweep of every production caller found none that authenticates attacker-supplied child digests without recomputing what is underneath them.

*Second, the leaf constructions provably never enter the small subfield.* The leaf sponges load capacity IVs (constant initialization values derived from ASCII labels — at least 63 tower bits) that sit *outside every invariant subfield*, and absorbed message bytes never touch the capacity lanes. Since the permutation maps `K^4` onto `K^4` and is bijective, a state that starts outside can never land inside: the leaf sponge state provably never enters `K^4`. Verified empirically: 65,536 fully-aligned leaves produced zero aligned digests and zero intermediate aligned states, with the simulation byte-identical to the real leaf hash; the control — the node compressor on aligned inputs — produced 4,096 aligned digests out of 4,096, i.e. 100%.

*The economics.* Any attempt to lift an aligned collision through a `GF(2^d)` subfield costs `2^(4(128-d)+d)` hash work: `2^464` for d=16, `2^416` for d=32, `2^320` for d=64 — every aligned-lift route is strictly dominated by the generic `2^128` whole-node birthday between two honest commitments. The semantic layer therefore keeps the full generic `2^128` collision resistance even though the node layer offers only about `2^16`. More grinding on node collisions can never pay. (Caveat carried from the lane: "dominated" covers the aligned-lift routes through the subfield towers; it is not a lower bound against all conceivable attacks.)

And the semantic chains never touch the broken function at all: state, transaction-root, segment and claim nodes use the two-permutation full-sponge construction, and object roots use one-permutation duplex wraps. The known colliding pair does *not* collide under `TAG_EXSTNOD`, `TAG_TXROOT`, `TAG_SEGMENTTREE`, or `TAG_CLAIMS`, and the object-root wraps are single-bit sensitive — flipping any of 32 tested program words moves the digest. The `2^16` world and the money world never meet. That is the leaf wall.

One related malleability is real but shallow: *C5* *[RV]* — at the legacy `noid_fri` public verifier, the padding value `PAD = 0x01000000000000000000000000000080` is itself a valid data lane, so absorbing `x` and absorbing `x` followed by `PAD` leaves the transcript in the same state; the verifier checks the final codeword is nonempty rather than exactly the right length. Distinct five-lane and six-lane proofs therefore verify for one statement, with no collision search at all. No current caller of that legacy API was found, and the newer framed protocols reject the ambiguity — but it is a public API that malleates.

== The soundness certificate: false or vacuous

parano1d's headline security claim is an executable certificate (`noid_soundness`) that prints `PASS`. Three findings bear on it.

*C7 — the proved bound is thin* *[RV]*. From the artifact's own data: at adversary budget `T = 2^32` the proved security is 44.96 bits (single-instance) and 41.65 bits (multi-instance); the bound degrades roughly as the fifth degree in `T` and becomes vacuous near `2^41`. A vacuous bound is a loss of *provable* reduction security, not an attack that needs `2^41` queries — but a proof whose arithmetic yields 41.65 bits at `T = 2^32` is not much of a proof.

*C8 — the hard premise is assumed* *[RV]*. The certificate's `PASS` is conditional on a quantity called `Delta_P2b` (the residual failure of the fixed permutation that the ideal model fails to capture) being below 0.45067. That premise is never evaluated or bounded anywhere in their corpus. The committee charged with checking the difficult instantiation problem has simply assumed its answer. For calibration: a *random* permutation would return the small-subfield slice to itself with probability `2^-448`; the production permutation does it with probability 1.

*C23 — and for the shipped object, the claim is false or vacuous* *[RV-T]*. A definitional analysis across their paper (Zenodo 23778742) and the certificate found:

- The paper never contains `Delta_P2b` or `0.45067`; the machinery lives only in `noid_soundness` docs.
- Their bad event `BadTypedBind` is stated *twice*, with different scope ("a collision, ambiguous encoding or domain confusion changes the typed semantic graph" versus "object/statement/namespace binding fails"), and is never disambiguated.
- The paper's formalization defines a typed dictionary with a *single* node hash `v_node` (eq. 52) and three sponge capacities; production ships a tag-parameterized compressor with at least *two* one-permutation node domains — never defined as a dictionary at all.

Hence a fork their corpus never resolves. Under the *dictionary* reading (the only one under which their ideal binding term means anything): the same-tag collision (C1) instantiates their bad event, and the cross-tag claw (C21) instantiates it doubly — it satisfies "collision" and "domain confusion" simultaneously, matching their second wording verbatim. The claw is deterministically producible inside both of their envelopes (about `2^41` gates, versus `2^130` for a generic collision), which forces `Delta_P2b` above 0.95067 against a required value below 0.45067 — and `Delta_P2b(2^64)` above 0.81247 against a required 0.31247. Both sufficient conditions are unsatisfiable: the `PASS` premise is *false*. Under the *worklist* reading: both events are excluded by clause and `Delta_P2b` stays unevaluated everywhere — their definitions cover the shipped multi-domain commitment layer *not at all*, and the claim is *vacuous*. False or vacuous; there is no third branch.

The strongest licensed claim, verbatim from the formal lane: "Their certificate's fixed-Poseidon2b premise is either false or vacuous for the shipped object: false, because the `2^16` cross-tag claw instantiates their own typed-binding bad event ('a collision or domain confusion') and forces `Delta_P2b` above 0.95 against a required < 0.4507; or vacuous, because their security statement — whose paper formalizes a single node domain — defines no bad event that covers the shipped multi-tag commitment layer at all, leaving the exact property their design comment claims ('distinct trees cannot share collisions') outside their theorem in either case."

Honesty note: the ideal-model theorems themselves are not logically false — the accurate criticism is that the model is unjustified for the shipped permutation. And `Delta_P2b` is explicitly not an ordinary distinguishing advantage, so the mere existence of a subfield distinguisher does not by itself break the numeric premise; the claw is what does the work.

= What a stranger on the network can do

The first two findings need no credentials, no stake, and no identity cost. Both are *availability* attacks — nothing invalid is ever accepted — and both were demonstrated against the real production components (real codecs, real process-global budgets, real mempool with real proof verification). The subsections that follow add two cheaper or adjacent variants, and then one attack that costs more but buys permanence.

== C17: four 12-byte requests can pin the byte domain *[RV-T]*

*Mechanism.* Every served mempool pull — the request is 12 bytes — unconditionally reserves the *maximum possible* response size (16 MiB, `MAX_MEMPOOL_SYNC_BYTES`) from a single process-global 64 MiB outbound byte budget, *before a single byte is cloned*, even for an empty mempool. The reservation rides inside the response object until the last wire write completes. Exactly 64 / 16 = 4 concurrent responses consume the entire domain. A peer that opens four mempool sync streams and then simply stops reading stalls each write after about 256 KiB (the stream flow-control window — and one legal transaction can legitimately be 303,495 bytes, so an honest mempool easily exceeds it).

*Demonstrated* (real codecs, real budget singleton): `aggressors holding the entire domain mid-write: 4/4`; a 1 KiB mempool response from an honest peer is blocked; the victim is admitted 27 ms after releasing one aggressor. The production-path escalation — blocked object/history-step/state-segment serving, and six blocked object requests exhausting the class slots so that *all further object requests get `Busy`* (rejection, not delay, for every peer) — is established by source reading at the pinned commit, because the admission types are crate-private and cannot be driven externally; we flag that split honestly.

*Honest caveats.* The request timeout aborts a stalled write after 30 s, so an attacker rotates pulls every 30 s or less to sustain the pin. The mempool must hold more than ~256 KiB at pull time (ordinary on an active network). Four honest peers on slow links syncing a full mempool produce the same cross-protocol starvation *by accident* — this is a fragility, not just an attack. No acceptance impact: purely serving liveness. *Fix sketch:* reserve the actual response size instead of the unconditional maximum, or give mempool serving its own bounded sub-domain; release data-plane slots when the requesting stream times out.

== C22: the admission discount — 62x CPU, one proof forever *[RV-T]*

*Mechanism, four independent parts, all from public data:* (1) the mempool's cheap admission checks compare only *claimed* input amounts against readable chain state — ownership is established solely by the expensive proof check, and a declared fee is never collected on rejection; (2) input slots are reserved only *after* proof success, so a rejected transaction costs the node a full verification and the attacker zero state; (3) there is no rejection cache anywhere — a byte-identical resubmission is re-verified (3 of 3 in our test), and changing the declared fee by 1 yields a fresh payload for about 134 µs; (4) one real proof (675 ms to generate, once) is a perpetual template: corrupt one byte per attempt and every variant passes the cheap checks and burns a full verification.

*Measured* (real mempool, real AuthGKR verification): the node pays 8.34 ms per rejection while the attacker pays 134 µs per distinct payload — the 62x asymmetry. Under saturation, the 1,024 non-evictable delivery slots fill in ~4.6 s, honest transactions are then dropped at delivery start on both paths, the shared 64 MiB inbound budget is held by ~742 queued 90 KB pushes, and — because the local wallet RPC queues behind the *same* semaphore — the operator's own wallet calls slow from 24.4 ms (uncontended) to a 303 ms median at moderate depth and 2 s+ at saturation.

*Reach.* Anonymous: any TCP peer completing the free cryptographic handshake can offer transactions; gossip admission requires only mesh membership; there is no application-level peer scoring to punish this. Not reachable via RPC (credentials or loopback required there).

*Honest caveats.* Attacker bandwidth is the binding cost on fast hardware (~58 MB/s total, roughly 84 identities, to saturate this laptop's four verification workers; ten identities at 4.6 MB/s each suffice on the code's own 84 ms verification budget) — this is what keeps the finding Medium rather than High. The cheap garbage-bytes variant *is* blocked: decode-before-verify (~190 µs) means undecodable spam mathematically cannot saturate under the rate caps — the check ordering works there. And the transport-level effects are established from the pinned code, the libp2p configuration, the crate's own unit tests, and a rate model over the measured per-item costs — *not* a live multi-peer swarm (see the pending live demonstration, §11).

*Fix sketch:* a short-TTL negative cache keyed on (transaction id, proof hash) removes the 62x discount for the observed attack shape; three complementary measures are recorded in the lane README.

== The gossip-window variant and the unbudgeted header path

- *U21 (gossip window)* *[RV-T, code-argued, not executed]*: the gossip byte window (64 MiB / 10 s) is global and *shared with block announcements*, and it is checked before decode. About five identities shipping 303,495-byte undecodable payloads (50 transactions per 10 s each: 75.8 MB per window against a 64 MB budget; ~6.4 MB/s of garbage, no validity needed) monopolize it, and the node drops *all* honest transaction *and block* gossip for the rest of each window. The drop carries no peer-score penalty, so the exhaustion is sustainable. Cheaper per identity than C22; gossip-layer only.
- *C18 (header serving memory)* *[RV-T]*: header sync deliberately sits outside every process-wide budget. Each stalled max-batch header response retains both the canonical buffer (up to 1,179,648 bytes) and the compressed buffer; 64 stalled streams (16-byte requests) retained 75,497,472 canonical bytes — more than the entire 64 MiB domain the other large protocols respect. Worst case at the 128-connection inbound cap (~1,024 streams) is on the order of 1–2 GB resident (an extrapolation, flagged as such). The exclusion is documented as intentional; it still makes header serving the one large-response protocol whose retained bytes scale with peer count, against the domain's own stated goal.

C17 and C22 compose: both anonymous, both attacking the same shared 64 MiB byte domain and the delivery paths. The combined live demonstration is queued in the devnet-chaos lane (§11).

== What a patient peer can do: first-sync snapshot poisoning (MN-A) *[RV-T]*

Snapshot sync is how a fresh node bootstraps: instead of validating every block from genesis, it downloads a committed snapshot of the history and state. The malicious-node lane demonstrated, end-to-end against the pinned production code (every step the real function, driven in the node's real order; all local assertions passed), that a malicious serving node can make a snapshot-syncing peer *commit and finalize* a header history that every honest validator rejects by rule:

+ *The attacker mines a real-PoW fork* carrying one far-future timestamp in the middle (the demonstration used a 24-header chain whose height-12 header is timestamped ten years in the future). This is cheap on the ramp: snapshot staging drops the wall-clock drift cap for every non-tip header, and a far-future parent drives the child difficulty target straight to the unconditional genesis floor — the exact gaming the drift cap's own comment says it exists to prevent.
+ *The honest admission path rejects this chain* (`BadTimestamp` at the poisoned height, using the local clock). A node syncing header-by-header can never be fed it.
+ *The snapshot path accepts the identical headers.* Staging validates every header "timelessly" — every rule except wall-clock future drift; the post-staging drift check looks only at the tip; and the recursive boundary proof binds only height, tip identity, and epoch anchor. The provable header rules are, by construction, exactly the timeless ones, so a genuine terminal for the poisoned chain (which the attacker, having mined the chain, possesses) verifies.
+ *The atomic install commits and finalizes.* After install, the poisoned header is canonical and the finalized checkpoint is set to the snapshot tip immediately.
+ *The honest network becomes permanently unreachable.* Honest peers are classified as divergent-below-finality and rejected as incompatible; re-staging an honest snapshot is structurally impossible (the staging base must be the first missing canonical height, which no longer exists on the honest fork); the only recovery is an operator wipe-and-resync. The estrangement is one-sided and permanent: no amount of honest chainwork ever makes the victim's finalized prefix acceptable to the network.
+ *The victim relays.* Manifest serving answers from the committed archive with no drift re-check anywhere, so the victim re-serves the poisoned snapshot to the next snapshot-syncing node. The poison is self-propagating.

*Preconditions and guardrails, honestly.* The victim must be fresh (or eclipsed) and select the attacker's snapshot offer, and the attacker's accumulated work must beat every offer the victim has already seen — fork choice protects any node that has ever ingested honest headers, so the precondition is the same as a plain eclipse attack. What elevates the outcome far above an eclipse: an eclipse *ends*; this survives it, permanently, and recruits the victim into distributing itself. The lane rates this Medium; we grade it *MAJOR* because the demonstrated outcome — rule-invalid history installed as finalized at first sync, permanent estrangement, self-propagation — is qualitatively worse than the transient condition that buys it. Assurance limits carried verbatim: the recursive terminal was stubbed (development builds embed no proof pack; the claim that a genuine terminal cannot reject the poison rests on the verified binding surface and the design fact that provable header rules are timeless), no live sockets were used, and offer selection was argued from the install-selection check rather than demonstrated against a live peer set. One enabler worth naming: the undocumented `--genesis` flag permits isolated block production without peer quorum (dark-surface, §5.3) — the attacker's chain generator.

Two honest negatives from the same lane. *Replay publication is by design:* the same-anchor replay transaction (C6) is now confirmed admitted into fully solved, fully validated blocks on *both* forks (byte-identical transaction, same txid, different tx roots) — and every consumer is fork-correct: one canonical history per node, a durable txid index rewritten through undo logs on reorg, the same linear state refusing the respend; no double-apply exists anywhere. *Cross-dialect confusion is negative:* every artifact class dies on schedule-bound checks (the receiver recomputes difficulty under its own schedule; authorizations are version-bound; terminals dispatch by schedule), with one honestly-open conventional surface — the recursive verifier under a mismatched schedule, which is release-pack-gated and could not be executed; a regression surface, not a proven wall.

= What the operator and the developer can do

== C16: the release builder embeds bytes it does not authenticate *[RV-T]*

The standalone release builder (`scripts/build_release.sh`) embeds legacy matrix bytes — precomputed arithmetic tables used by the verifier — into the node binary *without authenticating them*. Verified end-to-end by a dedicated verifier session (verdict: CONFIRMED-WITH-NARROWER-SCOPE):

- *Build leg:* a substituted legacy matrix (same decompressed length) reaches the compiler sealed under the *unchanged*, pins.env-approved metadata identity; the build log contains *zero* hash or manifest verification lines. The script's own usage text claims "The node build authenticates the matrix packs" — false on the default path for legacy matrices.
- *Runtime leg:* the compiled node's embedded-path decoder — the exact code the binary applies to its own embedded bytes — accepts the foreign relation and returns a statement digest *equal to the approved one* (`safe_loader_rejected=true`, `embedded_decoder_foreign_accepted=true`).
- *The fix exists but is unwired:* a hash-verifying manifest check (`release_verify_sha256_manifest`) is present in the shared build library and rejects the tampered pack when invoked explicitly — the default path simply never calls it. The authenticated pattern already exists in the same build script for v2/v2.1 matrices; the authenticated metadata even carries the correct digest per class. The wiring was just never done for legacy.
- *Scope, honestly:* not in `--retired-history` builds (no legacy bytes embedded), not for v2/v2.1 matrices (payload-hash plus semantic authentication), not in bare `cargo` builds, and not in the official publish chain as scripted (two independent compensating controls live there). Severity: High for the standalone builder as an authentication boundary; Medium as a defect of the official release chain. Blast radius if exercised: a node verifying/proving the legacy relation against a foreign matrix under the canonical pinned identity — silent consensus divergence — with `pins.env`/`SHA256SUMS` looking pristine; and because the runtime cache is keyed by metadata digest only, a poisoned artifact survives a corrected binary.

Adjacent and lower: *U2* (the packed runtime cache also accepts a foreign relation under the approved digest; requires local write access — defense-in-depth) and *U3* (an inherited environment override can flip release builds to debug verifier constants; agent-reported, no evidence current CI sets it).

== The supply-chain composition, leg by leg

This is the clearest way to state what the audit actually established. Each leg was individually demonstrated against real code:

+ *A tampered pack is sealed and built without detection* — C16 above: substituted matrix, unchanged approved identity, zero verification lines in the log. [verified]
+ *The node accepts the foreign relation under the approved digest* — the runtime decoder leg: `statement_digest()` equals the approved value for content that is not what was approved. [verified]
+ *The wallet verifies nothing and renders whatever the node says* — §6 below: 256 bytes of `deadbeef` come back "Merkle-valid, canonical, confirmed" and render as AUTHENTICATED PAYMENT / CONFIRMED; a fabricated transaction id plus a 2 NOID fee on a 5 NOID send renders TRANSACTION SENT. [verified]
+ *The wallet cannot tell which node it is talking to* — node identity is a single unauthenticated ping to `127.0.0.1:9601`; no pinning, no version handshake; a pre-bound impostor is adopted with zero challenge. [verified]

The missing element at every layer is the same: a user-visible check that compares what was approved with what is running. We want to be precise about the end state: *closing the loop into fund loss requires the user to act on fabricated confirmations* — the chain is closed leg by leg, and a single live demonstration stitching the legs together is being assembled in the devnet-chaos lane (§11). We are not claiming a demonstrated theft.

One documented mechanism makes the last leg easier than it looks: an undocumented environment variable (`NOID_GUI_NODE_BIN`) makes the GUI spawn an *arbitrary binary* as its node — whoever controls the user's environment supplies the "node" that the wallet then trusts absolutely (§5.3).

== The dark surface: what ships but is not shown *[RV-T]*

A dedicated lane enumerated every capability of the shipped software (all 79 RPC methods, every CLI flag, the configuration schema, every environment variable, the 39-operation native profile) and diffed it against what the GUI calls, what the CLI calls, and what the documentation states — with live loopback demonstrations against a real node binary of the pinned source. The map itself is informational; three clusters carry weight:

- *Key-material flags hidden from help.* `--export-wallet-secret` prints the wallet master secret (64 hex characters) to stdout and `--import-wallet-secret` replaces a data directory's spending authority from stdin in one command — both absent from `--help` and from the docs (which instead tell users to copy the key file). They are local-user-tier and are exactly what the GUI's own Export/Import dialogs invoke, so no new boundary is crossed — but the only secret-printing code in the tree is undocumented.
- *The loopback trust model.* Node identity for authorization purposes is "any process on the same machine, with no credential": a bare `curl` with no token killed the running daemon via `paranoid_stop`, enumerated any address's live UTXOs, read full wallet posture, and injected raw transaction intents (proof-gated beyond decode). This is an accepted same-host trust design — but it means every local process is inside the node's trust boundary, and nothing distinguishes "the GUI" from "malware on the same machine." Eleven RPC methods are called by no shipped client at all.
- *Phantom consensus operations.* The 39-operation native profile contains 17 dead operations (implemented, consensus-disabled — including the entire offer/escrow family of §7.2) and two *phantom* operations (`Swap`, op 33; `LaunchRequest`, op 49): enabled and provable in consensus, yet no shipped GUI, CLI, or miner code constructs them, and no construction site has ever existed in 740 commits of history. The network will admit and prove an operation class no official client can produce — reachable only by hand-crafted intents through the raw intent-injection RPC. The undocumented `--genesis` flag (solo block production without peer quorum) rounds out the set as the snapshot-attacker's chain generator (§4.4).



== Credential-gated burns

For completeness, the operator-credential side (none of these are anonymous; all are real costs to the operator's own machine):

- *C19 — every receipt query rescans everything* *[RV-T]*: each `paranoid_walletQueryReceipts` call — any page, including page 1 — clones, decodes, and Merkle-reverifies the *entire* receipt store, with no cross-call cache: 20,000 receipts means 4.1–5.6 s per call (~205–225 µs per receipt; repeated page-1 calls measured at 4,514,004 / 4,442,464 / 4,118,766 µs). A GUI walking 400 pages of a 20k-receipt wallet burns ~28 minutes of one core. Bounding guardrails: a single-permit gate rejects concurrent queries, output is capped, and the method is on the operator allowlist — a serialized, authenticated CPU burn, not an amplification.
- *U18 — the same scan starves the async workers* *[RV-T, mirror-verified]*: the scan runs inline on async worker threads holding the wallet mutex; 16 concurrent calls serialized, and unrelated chain-info probes stalled up to 310,884 µs against a 101 µs idle median (a 3078x tail). The mirror is line-cited and identical in lock shape; the production wallet type is binary-private, so production behavior is inferred from code identity rather than measured against the linked function — that split is carried honestly.
- *C15 — an invalid proof-of-work burns the mining template* *[RV-T]*: submitting a block with an invalid nonce consumes the node's one prepared mining template *before* validation runs; the expensive preparation is destroyed and replay returns `Unavailable`. Requires mining-API authorization (a worker with the shared bearer token), so not unauthenticated; six controls confirmed nothing invalid is accepted.
- *C11 — the owner query is unbounded* *[RV]*: `paranoid_getSlotsByOwner` returns every UTXO while holding the chain read lock (6.16x a paged query on a 4,096-UTXO fixture). Local-only under the pinned credential policy — a hygiene item.

= What a corrupted node does to its user

== C24: the wallet trust table *[RV-T]*

The structural fact first: the GUI wallet has *no dependency on any verifier*. No header hash, no proof-of-work, no state root, no Merkle check, no signature check — none is ever computed client-side. Every balance, history row, confirmation verdict, and even the displayed state root is the node's word. In a signatureless system the client has nothing independent to check — and it checks nothing. The send path is worse than trusting: the node *builds and signs* transactions while holding `wallet.key`.

The lane drove the real pinned GUI backend (the verbatim crate, constructed exactly as production constructs it) against a scripted impostor node; all 15 checks passed across 7 scenarios (77 requests, 22 methods):

- *W1 (High) — the phantom payment.* The user opens a payment receipt and presses *verify* — the flow built for exactly that doubt — and the verification call delegates to the node *including the Merkle math*. 256 bytes of `deadbeef` come back merkle-valid/canonical/confirmed, all true, with an attacker-authored summary (attacker-chosen fee, height, and outputs including 999 NOID to the attacker), rendered as AUTHENTICATED PAYMENT / CONFIRMED. "The verification button asks the attacker whether the attacker is lying."
- *W2 (High with a key-holding node) — node-side send.* A fabricated transaction id plus a 2 NOID fee on a 5 NOID send is accepted and rendered TRANSACTION SENT. With the node holding `wallet.key` this is a real, spendable substitution; via a mere impostor it deceives but cannot spend (the impostor never sees the key). The node also reads every plaintext request: the wallet has no privacy from its own node.
- *W3 (Medium) — impostor adoption.* "Which node am I talking to?" is answered by one unauthenticated ping to `127.0.0.1:9601`. A pre-bound impostor is adopted with zero spawns; if the GUI's own child node dies, the GUI reports ONLINE against the dead node.
- *W4 (Medium) — suppression.* Real funds zeroed, history attacker-authored: a healthy chain at height 4324 renders as a calm, empty wallet.
- *W5 (Low) — decorative status.* "terminal verified" is hardcoded true in the GUI backend; a fake tip of 999,999 plus an attacker state root render as SYNCED and TERMINAL VERIFIED.
- *W6 (Low) — lookalike peers.* Peer addresses render as first 13 + last 7 characters; lookalikes are indistinguishable, and the send path validates only non-emptiness.

What the client *does* do itself: local amount and fee parsing, the empty-recipient check, conditional-get revision discipline in the UTXO snapshot (an anti-race mechanism, not anti-node), and a native-asset submission binding that catches a *divergent builder* — but it is node-preview versus node-submission, so it catches a buggy node, not a malicious one. The lane's trust-boundary table (eight rows: balances, UTXO table, history/receipts, receipt verification, payment build/sign/submit, explorer inclusion, fee, chain status — each "client-side verification: none") is the citable summary.

The precondition is a malicious or corrupted node (or the W3 impostor). As the next subsection shows, the system itself supplies a way for a node to become corrupted without anyone choosing malice.

== C20: the zombie supply counter *[RV-T]*

Every hot counter in the node's storage is bound to the hash-committed tip header on restart — except one: the circulating-supply counter appended in the v2 metadata. The compact-restart path reads it verbatim and trusts it, even though the dense fallback path proves the value is derivable from authenticated data. Demonstrated on the real storage engine with real templates and real proof-of-work:

- *Upward corruption* (write +123,456,789 into one row): silently accepted with state roots *equal*, then propagated through a subsequent valid commit (45,000,000 clean became 168,456,789; after the next block, 90,000,000 clean became 213,456,789 persisted).
- *Downward corruption* (zero it): silently accepted at restart — and then a *valid* spend, validated end-to-end on a clean twin node, is rejected deterministically and *forever* ("corrupt: uncommitted block supply rollback failed", rejected twice, tip pinned). Meanwhile coinbase-only blocks keep the node looking alive: it reports 45,000,000 supply against a true 180,000,000. The node is a zombie: up, serving, and refusing every real spend.

Preconditions and scope, honestly: this requires a local database write, bit rot, or a tampered backup — there is no remote vector; no invalid block is ever accepted; nothing is minted; UTXO amounts stay root-authenticated. The harm is single-node integrity and liveness — but combined with §6.1, a silently-diverged node that keeps looking alive is a fully sufficient trigger for the entire W1–W6 deception family. A sibling local-tamper defect: *U19* — a 41-byte (noncanonical) chain-tip row passes every restart check and then permanently fails every subsequent commit with no recovery path.

= The user-facing protocol flaws

== C25: mint-into-victim *[RV-T]*

Anyone can mint NFT items *owned by an arbitrary address*. The mint operation checks only collection identity and serial continuity; the authority proof binds the *inputs*, never the output owners. Demonstrated through the real block-commit path (real transition, real template, real proof-of-work, real database commit): a block whose 248 minted items are all owned by a non-signing victim address was accepted, and the victim's forced carriage is provable in the real store — `native_owner_roots(VICTIM)` returns all 248 rows in a 1.8 ms page scan.

There is no consent or refuse path. Deletion requires the *victim's* signature and a fee (5,900 µNOID for a single item, ~123 µNOID per item batched). Every wallet and GUI inventory surface keyed on native ownership lists attacker junk until the victim pays to burn it — and victim discovery scans scale linearly in attacker-chosen junk. Grading it honestly: this is a nuisance, not value extraction, and it costs the attacker real fees (see the price list, §8) — but the asymmetry (attacker pays once, victim pays to undo, junk is permanent) is a genuine protocol flaw.

Unlike the §6 phantoms, this junk is *real on-chain state*: fixing node attestation does not remove it — and §6 guarantees the victim sees it rendered as ordinary wallet content.

== U20: the dead marketplace *[RV-T]*

At this commit, the funded round-escrow system and the *entire offer family* of the NFT marketplace are unreachable dead code. Two independent profile gates omit them: the operation-enabled table excludes codes 67–75 (rejected before any semantic check), and the kind-enabled table omits the purchase and offer kinds — so even a *perfectly formed* purchase or offer dies at the opening-kind gate. Additionally, one purchase-funding checker has no dispatcher at all, and the output scanner returns early for that operation. The settlement *circuit* is doubly dead as shipped: its wiring terminates in an ignored parameter (the only caller never invokes the binder and passes an empty list), and under the shipped profile a purchase opening is *unsatisfiable by construction* — the kind's one-hot selector is the zero constant — so the circuit can prove no settlement at all. (This corrects the earlier lane phrasing that the circuit was "wired for operation 71": the wiring ends in a discarded parameter.) Built in its intended configuration — purchase kind admitted, everything else production code — the binder accepted the honest settlement witness, constructed for the first time in this audit, and rejected all five adversarial variants (§9). The live surface is base NFT operations plus listing create / instant-buy / cancel. The implication we carry forward: *any documentation or paper claiming marketplace offers or escrow at this commit describes code that cannot run.* (The settlement economics were still tested below the gate on synthetic escrow, and they are exact — §9.)

== The ordering reality

We mapped all five layers of transaction ordering (delivery, admission, template selection, in-block, cross-block). The plain summary, conditional on the v2.1 native activation (mainnet height 279,370):

- *Fees buy nothing.* The fee field is a ceiling; the charge is always the state-derived minimum. A 1,000x fee changes no outcome (SEQ-2 / U25).
- *Priority is inventory.* Trade execution order and NFT fixed-price winners are decided by a hash draw over pre-positioned "dust" UTXOs — count-weighted (more UTXOs = more lottery tickets), never value-weighted, redrawn every block. Measured win rates match the n/(n+m) model (0.672 vs 0.667 for trades over 2,000 parents; 0.733 vs 0.750 for NFT over 400 rounds). A permanent ticket costs ~9,000 µNOID; the batch caps (64/16) are capturable (SEQ-1).
- *Fee-pinned natives order by a free grind.* Because financial-native fees are pinned to the exact parent-derived minimum, their tie-break reduces to an offline transaction-id grind costing a median of two hash attempts (SEQ-2).
- *The block builder owns every consensus-level race.* Validation re-derives whatever batch the proposer submits; there is no inclusion or ordering fairness rule; ordinary ordering is documented "miner policy, not consensus" (U26/SEQ-4). One correction we issued to our own ledger: the authorization-key sort governs *encoding* only — execution order is the funding-priority digest (racers with reversed key order still execute in digest order).

The lane's sharpest public claim, verbatim and with its condition: "conditional on v2.1 native activation (height 279,370), every priority surface in parano1d's native trading stack is an unpriced UTXO-count lottery — trade execution order and NFT fixed-price winners are decided by offline hash draws over pre-positioned dust (count-weighted, never value-weighted, ~9,000 micronoid one-time per ticket), while financial natives, whose fees the protocol pins to the exact minimum, are ordered by a txid grind that costs a median of two body hashes — with no fee- or value-based path to priority anywhere in these paths."

Two adjacent facts: same-anchor forks can carry the same spend — the C6 replay property, since confirmed through *full block admission* on both forks by the malicious-node lane, whose consumer survey found every application fork-correct (one canonical history per node; the durable transaction index is rewritten through undo logs on reorg; the same linear state refuses the respend; no double-apply exists) — with the work race deciding which fork's spend survives, up to 18 blocks late; and inclusion freezes at template build, so late arrivals at any fee wait for the next block.

= The price list of malfeasance

What malice costs, and what it buys. Every row is attacker cost (compute, tokens, credentials, or position) against what the node or network bears — permanent (P) or transient (T).

#table(
  columns: (1.15fr, 1.05fr, 1.3fr),
  inset: 5pt,
  table.header([*Vector*], [*Attacker pays*], [*Node / network bears*]),
  [State growth (U29)],
  [~106 µs + 3,227 µNOID per NFT item (one-time, flat); at saturation 4,609 NOID/day (~151 s CPU); a 1-in/1-out collection creation is 5,800 µNOID and *zero burn*],
  [(P) every full node carries each item forever: ~491 B database + 50 B consensus + ~192 B on every future cold sync; saturation compounds ~71 MB/day of consensus state and ~700 MB/day of node-local storage; one year of saturation ≈ 26 GB state + ~256 GB database per node, for ~1.68M NOID total — under 35 days of genesis-era emission],
  [Admission discount (C22)],
  [134 µs CPU per distinct payload + bandwidth (~58 MB/s to saturate 4 verify workers); one 675 ms proof, once, forever],
  [(T) 8.34 ms node CPU per rejection (62x); honest transactions dropped at saturation; operator's own wallet RPC slowed to 2 s+; delivery slots and the shared 64 MiB inbound budget held],
  [Byte-domain pin (C17)],
  [4 open streams, 12-byte requests each; rotate every ≤30 s; stop reading],
  [(T) all large-payload serving for all peers blocked, then rejected (`Busy`); honest slow peers trigger it accidentally],
  [Gossip-window burn (U21)],
  [~5 identities × ~6.4 MB/s of undecodable bytes (code-argued, not executed)],
  [(T) all honest transaction and block gossip dropped for the rest of each 10 s window; no peer-score penalty],
  [Header memory (C18)],
  [16-byte requests × stalled streams (64 shown; ~1,024 at the connection cap)],
  [(T) 75,497,472 canonical bytes retained by 64 streams — more than the entire shared 64 MiB domain; order of 1–2 GB at the cap (extrapolated)],
  [Receipts burn (C19)],
  [operator credential or loopback; ~zero bandwidth],
  [(T) 4.1–5.6 s per call at 20k receipts; a GUI walking 400 pages burns ~28 min of one core],
  [Template burn (C15)],
  [mining credential (or local owner); one invalid nonce per template],
  [(T) each expensive template preparation destroyed; node must redo snapshot selection and nonce-independent recursive preparation],
  [Mint-into-victim (C25)],
  [full creation fees per item (~3,227 µNOID; ~106 µs each)],
  [(P) permanent junk in any victim's wallet and listings; only the victim can remove it, by paying (5,900 µNOID single, ~123 µNOID/item batched)],
  [Wash-trade cycle (nft-races)],
  [16,400 µNOID per self-list + self-buy cycle (6,600 list + 9,800 buy); 2,500 burned, 13,900 miner-claimable — a self-mining washer recaptures the claimable share and nets 2,500 µNOID per cycle at floor geometry; the schedule is pressure-indexed, so sustained washing raises its own cost],
  [(T) nothing permanent on-chain beyond one ordinary listing slot — the product is fabricated volume and price history; the escrow credit round-trips and the ask returns to the seller],
  [Zombie supply (C20)],
  [one local database row write (or bit rot / tampered backup)],
  [(P) node silently diverges upward, or permanently rejects every real spend while coinbase-only blocks keep it looking alive],
  [Snapshot poisoning (MN-A)],
  [dominate a fresh or eclipsed node's peer set at snapshot-offer time; mine a real-PoW fork (one far-future timestamp collapses the difficulty ramp to the genesis floor; an in-drift eclipse reaches the same floor over time — the cost is the ramp, not the floor); terminals come free with mining; `--genesis` makes the chain generation zero-config],
  [(P) the victim commits rule-invalid history as finalized at install; the honest network becomes permanently unreachable (wipe-only recovery) while honest peers are blacklisted as divergent; the victim re-serves the poison to the next syncer],
)

*The tuning observation, honestly.* The protocol defends the creation axis *purely economically* — and the price is flat forever. Fees are exact and enforced at three layers; block caps bound creation to ~496 items per block; a pressure multiplier escalates 2/4/8x under load but is reset *permanently* by the next capacity expansion, so the per-object price never scales with total state. What does not exist anywhere (searched, cited by absence): per-account or global creation-count limits, rent, expiry, or decay on any object, any cap on declared collection size (the maximum is a free 64-bit field), and any pricing of object *kind* — the burn sees only outputs-minus-inputs, so a 1-in/1-out collection creation is occupancy-invisible and zero-burn while upgrading ~102 B of slot into ~441 B of node-local carriage. Every live native object also adds ~192 B to every future cold sync and one record to every full-table pass that snapshot staging and index rebuilds perform, so new-node sync cost grows linearly in historical object count even after blocks are pruned. Against that, two honest negatives from the same lane: there is *no CPU asymmetry* on creation (the node applies an item in ~59 µs against the attacker's ~106 µs to construct one), and creation transactions are ordinary fee-paying valid payloads that compete for admission honestly — they amplify none of the anonymous attacks above. Whether flat-forever pricing against permanent carriage is "cheap" depends entirely on the token's value; the exchange rate is the whole defense.

= What held

This section exists because an audit that only lists breaks is not credible. The following attacks were attempted seriously and failed, each against a named guardrail:

- *Witness uniqueness (the forgery question) held structurally.* 8,870 build-and-check configurations of the production constraint system (both arithmetic variants, 120 random programs at u64 boundaries plus 9 directed cases) produced zero failures; the forgery battery (field-wrap, non-boolean predicates, writes from inactive instructions, all 128 single-bit flips of a next state, every reserved encoding) was rejected entirely. The blocker is structural — wires are defined by their row over earlier wires, booleanity is pinned by sibling constraints, range decompositions are injective by linear independence — not merely tested. (Assurance limit carried: structural argument plus directed battery, not an exhaustive search of the full recursive relation.)
- *Conservation and graduation math are exact.* Token issuance is launch-only and exactly minimum-backed; graduation handoff is exact for every legal supply (the curve's cumulative cash equals the pool base exactly); redemption is floor-exact; a full lifecycle conserves to the micronoid; the one rounding residue routes into the burned fee, never a recipient. The one "capturable prize" in the entire trading stack — the bonding curve's cumulative rounding surplus — is exactly 1 µNOID, against a 9,800 µNOID fee floor, and cannot be sold standalone. Nine of nine value-extraction experiments found no accepted extracting transition.
- *The keystore is solid.* 0600 permissions, 48-byte secrets, symlink rejection, atomic staged import with rollback; the master secret never transits RPC or argv; secrets are zeroized on drop, unclonable, and debug-redacted; remote RPC fails closed; five Rust and seven Python adversarial tests passed. Address preimage attacks failed too: zero of 112 equivalent-key transformations work, and the only weak-key class has probability `2^-240`.
- *No SIMD disagreement.* 8,241 cross-implementation pairs (random plus full edge cross-product) were bit-exact across every production arithmetic kernel dispatched on this machine; 2,689 truncation/mutation loader cases produced zero panics. (Caveat: aarch64 coverage only.)
- *The state machine, contract semantics, and every market surface rejected us.* A 20-case adversarial UTXO/state matrix (real proof-of-work), a 50,000-program contract differential (428,880 executions, zero mismatches), block-validation mutation matrices, DEX manipulation, NFT theft/duplication/race attempts, token-launch inflation — every case was rejected by a named check (conservation, authority, slot binding, phase machine, queue commitment, or exact-round economics).
- *Every NFT marketplace race is decided by one named invariant.* Not a list of passed tests — a single structural fact: *the listing escrow is a single UTXO that exactly one transition can consume*, and a stale cross-fork consumption dies on an authenticated state read against the reorged parent. Sequential double-buys, same-block double inclusion (the whole template build aborts on the slot conflict), cancel-versus-buy in either interleaving, and a stale listing across a *real* reorg (real chain store, real templates, real proof-of-work) all resolve to a consistent world at three independent layers (state application, template build, mempool admission), with zero inconsistent outcomes and no interleaving that both moves the item and preserves the payment.
- *The live trading kernel (`ExecuteTrades`, op 80) — executed for the first time anywhere in this audit — came back clean.* Micro-NOID conservation is exact (`inputs = payouts + successor + fee`), the batch fee burns exactly, batch-internal ordering is fully re-derived rather than caller-supplied (scrambled caller order yields an identical root, fills, charges, and outputs), a batch containing one unreachable fill is rejected wholesale with state untouched, and the kernel structurally cannot touch NFT state: listing and item openings are rejected as trade funding, and a listing parent is unrepresentable by construction. And the gated settlement circuit, though doubly dead as shipped (§7.2), is sound-shaped where it is aimed: in its intended configuration the real binder accepted the honest settlement witness (built here for the first time) and rejected all five adversarial variants — wrong recipient, settle-too-early, settle-too-late, a queue commitment off by one bit, one cash payout off by one µNOID.
- *Replay consumers and dialect walls held.* Pushing the fork-replay transaction all the way through real block admission on both forks surfaced no consumer that double-applies (single canonical history, undo-logged index, manual re-admission only); and every cross-dialect artifact tested — headers under a foreign difficulty schedule, foreign authorization versions, foreign terminals — was rejected at every executable layer, with one honestly-open conventional surface (the pack-gated recursive verifier under a mismatched schedule), flagged as a regression surface rather than a proven wall.
- *And the headline negatives:* no unauthorized spend, no invalid accepted block, no owner-key recovery, no fresh-request forgery, no split-brain escalation — despite sixteen completed attack lanes aimed exactly there.

The pattern across all of it: the *semantic* layer (where money is decided) is guarded by construction — re-derivation, exact conservation, pinned shapes — while the failures cluster in the *presentation* layer (what the user is shown), the *resource* layer (what strangers can make a node do), and the *justification* layer (what the security paperwork actually proves).

= Severity summary

Classes: *MAJOR* — a major problem: ships a false promise, breaks a production primitive, or lets an unauthenticated stranger degrade the network. *SHOULD-FIX* — a real defect with a bounded or conditional path; fix it, but say precisely what it is. *HYGIENE* — noted for cleanliness, hardening, or claim accuracy. C14 is omitted (it is a neutral witness, not a defect). Counts: 10 MAJOR, 20 SHOULD-FIX, 20 HYGIENE.

#table(
  columns: (auto, auto, 1fr),
  inset: 4.5pt,
  align: (col, row) => if col == 1 { center } else { left },
  table.header([*ID*], [*Class*], [*Finding — one-line justification*]),
  [C1], [MAJOR], [Production `TAG_CAPSNODE` compressor collision at ~`2^16` (72,274 evals / 0.128 s) — a production primitive fails collision resistance; not a forgery, and the state tree is unaffected],
  [C2], [MAJOR], [`IVCPCSN_` compressor collision (111,525 evals / 0.29 s) — second production node domain fails; raw openings verify; full BaseFold verification rejects everything],
  [C3], [MAJOR], [Prehashed Merkle commitment collision at exact production geometry (depth 13 / cap 3, both openings accepted) — collision resistance fails at a production commitment interface],
  [C16], [MAJOR], [Release builder embeds legacy matrix bytes unauthenticated under an approved identity; usage text claims authentication; the verifier that would catch it exists but is unwired on the default path],
  [C17], [MAJOR], [Four 12-byte unauthenticated requests pin the process-wide outbound byte domain and convert serving for all peers from delay to rejection; honest slow peers trigger it accidentally],
  [C21], [MAJOR], [Cross-tag claw: one digest valid in two domains with identical commitment bytes — refutes the shipped 128-bit domain-separation design comment (~16 bits actual); raw-API reach only today, inherited by a live hard-fork],
  [C22], [MAJOR], [Anonymous admission occupancy at a 62x CPU discount (8.34 ms vs 134 µs), one real proof as a perpetual template, honest transactions dropped, operator wallet RPC delayed; bandwidth-bound, which is the only reason this is not High],
  [C23 (+C8)], [MAJOR], [The soundness certificate is false or vacuous for the shipped object — the claw forces Delta past both thresholds under the only meaningful reading, or the definitions never cover the shipped multi-domain layer; C8's unevaluated premise assumed away the hard part],
  [C24], [MAJOR], [The wallet is node-attested end-to-end: no client-side verification of anything, node builds and signs sends while holding the key, and the verify button delegates to the node under attack],
  [MN-A], [MAJOR], [First-sync snapshot poisoning: a malicious serving node makes a fresh/eclipsed peer commit and finalize rule-invalid history, permanently estranges it from the honest network, and the victim re-serves the poison; lane rates Medium — we grade MAJOR for permanence plus self-propagation; fork choice protects every non-fresh node],
  [C4], [SHOULD-FIX], [Leaf/internal-node alias at the raw API (same root authenticates depth-0 and depth-1); production pins depths, so it is a public-API type confusion],
  [C5], [SHOULD-FIX], [Legacy `noid_fri` proof malleability (distinct 5- and 6-lane proofs verify for one statement, no search); public verifier API, no current caller, newer protocols reject it],
  [C6], [SHOULD-FIX], [Same-anchor fork replay: one real transaction and auth bundle pass all checks on two forks — confirmed through full block admission on both forks, with every consumer fork-correct (no double-apply); the residual is the fork-specificity question and sub-finality downstream indexes],
  [C7], [SHOULD-FIX], [The proved security bound yields 41.65 bits at T=2^32 and is vacuous near 2^41 — the proof arithmetic itself is thin (not an attack algorithm)],
  [C9], [SHOULD-FIX], [Identical P2P profile ID across three pairwise-incompatible consensus schedules — domain-separation gap giving peer mixing / test-isolation failure / DoS; all cryptographic downgrades tested were rejected],
  [C10], [SHOULD-FIX], [HeaderDAG capacity stall: at capacity the next valid child is rejected and the failure blocks its own remedy; requires ~1,024 valid PoW headers; restart recovers],
  [C15], [SHOULD-FIX], [Invalid PoW consumes the only prepared mining template before validation — an authorized worker cheaply destroys expensive preparation; not unauthenticated, nothing invalid accepted],
  [C18], [SHOULD-FIX], [Header-sync serving memory is outside every process budget and scales with attacker connections (64 stalled streams retained 75.5 MB > the whole shared domain); contradicts the domain's stated no-peer-scaling goal],
  [C19], [SHOULD-FIX], [Every receipt query rescans and Merkle-reverifies the entire store (4.1–5.6 s per call at 20k receipts, ~28 min for a 400-page GUI walk) — credential-gated, serialized, bounded, but O(N) per page with no cache],
  [C20], [SHOULD-FIX], [Circulating-supply counter trusted verbatim on compact restart: silent upward divergence, permanent spend-admission stall downward while the node looks alive; local-write precondition; it is the natural trigger for C24],
  [C25], [SHOULD-FIX], [Mint-into-victim: anyone permanently places junk NFT items in any address's wallet; only the victim can remove them, by paying — real attacker fees cap this at nuisance, but the asymmetry is a genuine flaw],
  [U2], [SHOULD-FIX], [Packed runtime cache accepts a foreign relation under the approved digest — poisoned artifacts survive a corrected binary; defense-in-depth with a local-write precondition],
  [U18], [SHOULD-FIX], [Receipt scans run inline on async workers holding the wallet mutex — unrelated RPC calls stall up to 3078x idle median (mirror-verified; production type is binary-private, split carried)],
  [U19], [SHOULD-FIX], [A noncanonical 41-byte chain-tip row passes every restart check, then every subsequent commit fails forever with no recovery — fail-closed but dead; local-write precondition],
  [U20], [SHOULD-FIX], [Marketplace offers and round escrow are unreachable dead code behind profile gates — any prose claiming them describes code that cannot run; a claims-vs-code problem, not a soundness one],
  [U21], [SHOULD-FIX], [Global gossip byte window shared with block announcements, checked before decode: ~5 garbage identities drop all honest tx+block gossip per window, unpenalized (code-argued, not executed)],
  [SEQ-1], [SHOULD-FIX], [Priority across the native trading stack is a UTXO-count lottery (count-weighted, never value-weighted; measured 0.672 vs 0.667) — conditional on v2.1 activation (height 279,370)],
  [SEQ-2], [SHOULD-FIX], [Fee-pinned financial natives have no fee lever, so their ordering reduces to a free offline txid grind (median 2 hashes) — v2.1-conditional],
  [U29], [SHOULD-FIX], [Creation is defended purely economically at a flat-forever price against permanent node carriage (saturation: ~71 MB/day state + ~700 MB/day node-local, forever); no rent, no decay, no kind pricing, zero-burn collection creation],
  [D-FOOT], [SHOULD-FIX], [Dark-surface footguns: the GUI spawns an arbitrary binary as its node from the undocumented `NOID_GUI_NODE_BIN` env var (composes directly with C24 and the supply-chain chain); hidden key-material flags replace the wallet's spending authority in one command, absent from help and docs; any loopback process can stop the daemon and enumerate any address's UTXOs],
  [C11], [HYGIENE], [Unbounded owner query returns every UTXO under the chain read lock (6.16x a paged query) — local-only under the pinned credential policy],
  [C12], [HYGIENE], [Three mempool liveness policies (first-seen slot pinning, coinbase race eviction, no reorg re-admission) — documented behaviors, no soundness impact],
  [C13], [HYGIENE], [A rejected malformed proof suppresses same-scan recovery of the same txid from that peer — per-peer, per-scan, cleared by a new scan],
  [U3], [HYGIENE], [Inherited cargo environment can flip release builds to debug verifier constants (64→10 FRI queries) — agent-reported; no evidence current CI sets it],
  [U4], [HYGIENE], [Snapshot staging checks future drift only on the tip — the agent-reported precursor whose mechanism the malicious-node lane has since demonstrated end-to-end as MN-A; kept here as the original record],
  [U7], [HYGIENE], [Standalone validation utility omits epoch-anchor enforcement — informational; the production boundary rejects and no production caller was found],
  [U9], [HYGIENE], [UC-composition assurance gap: distinct concurrent proofs verify in independent sessions — an assurance gap, not a false theorem or bypass],
  [U10], [HYGIENE], [Nothing downstream of the wallet builder can detect a recipient/amount substitution — hardening only (no divergent path in pinned code); escalated to real under exactly the C24 threat model],
  [U11], [HYGIENE], [Exported master secret persists in the system clipboard after dialog close (GUI memory itself is zeroized)],
  [U12], [HYGIENE], [A stale in-memory wallet can sign with a replaced-on-disk secret's successor state — verified by test, no production path],
  [U13], [HYGIENE], [Zero-round provers emit never-written memory as proof fields — latent; proofs self-reject; not production-reachable at the pin],
  [U14], [HYGIENE], [Two degenerate raw-API panics behind commitment-bound parameters in every production caller],
  [U15], [HYGIENE], [73-byte crafted bundle drives a ~1 MB allocation before failing cleanly — format has zero in-tree callers],
  [U16], [HYGIENE], [Bonding-curve free final unit / 0-NOID graduation trigger — provably not extractable value (the tail is pre-paid by earlier rounding surplus)],
  [U17], [HYGIENE], [Out-of-range receipt pages retain the full key set (2.9 MB vs 20.8 KB peak) — bounded by receipt count, transient, credential-gated],
  [U22–U26], [HYGIENE], [MEV posture facts (tail snipe priced at 1 µNOID vs 9,800 µNOID floor; graduation timing; launch-head overcharge; fees buy nothing; builder owns consensus races) — informational map, no extraction vector],
  [U28], [HYGIENE], [The production-selected v2.1 constraint path had zero direct in-tree tests until this audit supplied an 8,870-configuration battery — process finding, the algebra is correct],
  [SEQ-3/4], [HYGIENE], [Inclusion freezes at template build (late arrivals at any fee wait a block); builder discretion is total and reaches future priority entropy — structural facts, recorded],
  [D-MAP], [HYGIENE], [The dark-surface map: 11 RPC methods called by no shipped client, 17 dead native operations plus 2 phantom ops (consensus-valid, never constructed in 740 commits), one GUI-live method absent from all docs, undocumented config and env surface, and the undocumented `--genesis` solo-mining flag (the snapshot-attacker's chain generator)],
  [NR], [HYGIENE], [The listing-settlement circuit is doubly dead as shipped — no caller (the wiring ends in an ignored parameter, correcting the earlier "wired for op 71" phrasing) and the purchase kind unsatisfiable by construction — yet sound-shaped in its intended configuration (honest witness satisfies; five adversarial variants unsatisfiable); it will enter service with zero CI-executed witnesses],
)

= Limitations and reproducibility

*What this audit is.* A pinned-commit, locally-reproduced adversarial review: 39 experiment directories under `/Users/ember/dev/parano1d-break/experiments/`, of which 37 landed results (three — malicious-node, dark-surface, and nft-races — as this document was being drafted, and folded in above), every positive claim tied to a runnable PoC against real production code, every run's output preserved (`results.txt` per lane), negative results recorded with the guardrail that produced them. The consolidated record is `experiments/FINDINGS.md`; workspace provenance is `PROVENANCE.md`.

*What this audit is not.* It is not a live-network measurement. Specifically:

- C22's transport-level effects (and U21 entirely) rest on the pinned code, the libp2p configuration, the crate's own unit tests, and a rate model over measured per-item costs — not a live multi-peer swarm.
- C17's escalation from delay to rejection is established by source reading (the admission types are crate-private); the pin itself and the FIFO victim-blocking are demonstrated at the codec/budget layer.
- SIMD kernel agreement was executed on aarch64 only; the x86 kernels were read, not run.
- The P2P codec panic review is static: "no panic found" is not "no panic exists."
- Where a finding is [AR] (U2's cache-persistence scenario, U3, U4, and several negative-result rows), the coordinator's independent rerun was never recorded; the evidence is the lane's own preserved output. (U4's mechanism, tip-only drift checking, has since been positively demonstrated end-to-end by the malicious-node lane as MN-A.)

*What we did not demonstrate.* No unauthorized spend. No invalid accepted block. No owner-key recovery. No fresh-request forgery. No split-brain or state-equivocation escalation. No live end-to-end supply-chain theft (the composition is closed leg-by-leg; the single live demo is pending). No snapshot poisoning against a live socket (the real staging, install, finalization, and estrangement functions were driven directly, in the node's real order). No claim that the 2^16 collision cost is a proven bound. No claim that the state tree, conservation math, keystore, or witness uniqueness failed — they did not.

*Lanes still running.* Two lanes were still running when this document was drafted (malicious-node, dark-surface, and nft-races landed mid-draft and are folded in above); their sections will be filled when they land. Nothing above depends on their results.

#todo-box[devnet-chaos][The live composition lane: the strongest chaos composable from the confirmed findings against a local devnet (own data dirs, synthetic identities, loopback only) — including the combined C17+C22 anonymous starvation demonstration against a live multi-peer swarm, and the single live demo stitching the C16→C24 supply-chain composition end-to-end (the lane now also carries the tampered-build phase itself). Reproduce: `python3 scripts/run_all.py` (phases 0–5, loopback only).]

#todo-box[contract-reorg][Contract state machines under reorgs (semantic idempotence): the real chain store, real templates, real proof-of-work, real reorg application against throwaway databases. Findings pending.]

= Appendix: index of proof-of-concept directories

All paths are under `/Users/ember/dev/parano1d-break/`. Kimi-era lanes have no README; their entry points are the files named in the Reproducer column of `FINDINGS.md`, and the standard invocation is `CARGO_TARGET_DIR="$PWD/target" cargo run --release` inside the lane directory. The two pending lanes are listed in §11.

#table(
  columns: (0.95fr, 1.55fr, 1.15fr),
  inset: 4.5pt,
  table.header([*PoC*], [*Demonstrates*], [*Reproduce*]),
  [`poc/`], [C1: `TAG_CAPSNODE` collision search (72,274 evals / 0.128 s) plus the hardcoded-inputs verifier (no search: production compressor calls, assert inputs differ, assert digests equal)], [`cargo run --release`; verify: `cargo run --release --bin verify_collision`],
  [`experiments/basefold-chains/`], [C2: `IVCPCSN_` collision, raw opening verification, multiproofs, recursive path replay, fixed point], [`cargo run --release` (src/main.rs)],
  [`experiments/capsule-chains/`], [C3, C4: prehashed Merkle collision at production geometry; leaf/node alias; full-PCS rejections], [`cargo run --release --bin merkle_collision`; `--bin alias_depth`],
  [`experiments/leaf-reachability/`], [C21 cross-tag claw; subfield-invariance algebra; the leaf wall (65,536 aligned leaves, 0 aligned digests; economics)], [three binaries: `--bin subfield_invariance`, `--bin cross_tag_claw`, `--bin leaf_wall`],
  [`experiments/transcript-chains/`], [C5: legacy FRI malleability and PAD ambiguity (output in `output-release.txt`)], [`cargo run --release`],
  [`experiments/typed-binding/`], [C14: collision lifted to production-shaped Merkle paths — witness that the typed bad event is *not* instantiated], [`cargo run --release`],
  [`experiments/typed-bind-verdict/`], [C23: the definitional fork (no run by design — quotes PoC-verified objects of prior lanes)], [read `README.md`],
  [`experiments/fork-replay/`], [C6: same-anchor fork replay of a real 323-byte transaction (output `output.txt`)], [`./run.sh`],
  [`experiments/version-domains/`], [C9: identical P2P identity across three incompatible consensus schedules (`results/network-comparison.json`)], [rebuild three feature variants; see FINDINGS.md C9],
  [`experiments/ledger-fairness/`], [C10–C12: HeaderDAG capacity stall, owner-query amplification, mempool liveness policies (`REPORT.md`)], [`cargo run --release` (kimi-era lane)],
  [`experiments/identity-encoding/`], [C13 + U8: recovery suppression and auxiliary identity results], [`cargo run --release`],
  [`experiments/state-consensus/`], [U6: 20-case adversarial UTXO/state matrix — all rejected (`output.txt`)], [`cargo run --release`],
  [`experiments/block-validation/`], [U7: block-validation mutation matrix + standalone-utility epoch gap], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/native-intents/`], [Negative: worst-fill trade accusation disproved at host and recursive boundaries], [`cargo run --release`],
  [`experiments/contract-rights/`], [Negative: 50,000 Pruv programs / 428,880 executions, zero mismatches], [`cargo run --release` (RESULTS.md)],
  [`experiments/recursive-sync/`], [U4: tip-only future-drift admission differential (`poc-output.txt`)], [`cargo run --release`],
  [`experiments/uc-composition/`], [U9: concurrent-proof sessions + the 71-label/103-use domain audit (`domain-audit.json`)], [`python3 audit_domains.py`; see REPORT.md],
  [`experiments/address-preimage/`], [Negative: no key recovery; weak-key class only (`conclusion=low_entropy_weakness_only_not_a_uniform_key_attack`)], [`cargo run --release -- all`],
  [`experiments/build-artifacts/`], [C16 (+U2, U3): unauthenticated legacy matrix bytes — build leg and runtime decoder leg (verdict `VERIFY.md`)], [see `VERIFY.md` and `verify/` (runs 1–2)],
  [`experiments/external-miner/`], [C15: invalid PoW consumes the prepared template (`verify-run.txt`, ALL ASSERTIONS PASSED)], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/p2p-resources/`], [C17, C18: byte-domain pin and unbudgeted header memory — real codecs, real process-global budget], [`cargo run --release` (or `-- header-aggregate` / `-- mempool-pin`)],
  [`experiments/tx-admission/`], [C22, U21: 62x admission asymmetry, perpetual proof template, delivery-slot fill, gossip-window argument — real mempool + real AuthGKR verify], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/rpc-api/`], [C19, U17, U18: receipt-store rescan per call, page retention, worker starvation — real RPC server, 20k Merkle-valid receipts], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/storage-reorg/`], [C20, U19: supply-counter corruption (zombie) and noncanonical-tip commit stall — real MDBX store, real PoW], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/wallet-vs-node/`], [C24/W1–W6: the real GUI backend driven by an impostor node — 15 checks, 7 scenarios (ALL 15 CHECKS PASSED)], [`cargo test --test deceive -- --nocapture`],
  [`experiments/wallet-safety/`], [U10–U12 + secret-surface negatives (5 Rust + 7 Python tests)], [`cargo test --test repro`; then `PARANO1D_BIN=... python3 -I source_repro.py`],
  [`experiments/creation-spam/`], [C25, U29: mint-into-victim through the real block-commit path; the flat-forever creation price line], [`cargo run --release 2>&1 | tee ../results.txt`],
  [`experiments/nft-marketplace/`], [U20 + marketplace negatives: unreachable offer/escrow families; theft/dup/race rejections (10/10 tests)], [`cargo test --release`],
  [`experiments/token-accounting/`], [Negative: issuance, graduation, redemption, multi-asset atomicity — all exact (ALL ASSERTIONS PASSED)], [`cargo build --release`; `./target/release/token-accounting-audit`],
  [`experiments/dex-economics/`], [Negative + U16: no accepted value-extracting transition (9/9); free-tail boundary observation], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/dex-games/`], [U22–U26: MEV posture — 1 µNOID prize vs 9,800 µNOID floor; ordering-rule corrections], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/sequencing/`], [SEQ-1–5: the five-layer ordering map; 25 assertions incl. 19 verbatim source-fact assertions], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/constraint-systems/`], [Witness uniqueness (8,870 configs), field-wrap negatives, collision confinement, U28], [`cargo run --bin ip_battery` / `--bin pruv_adversarial` / `--bin domain_separation`],
  [`experiments/prover-unsafe/`], [U13–U15 + the SIMD/kernel differential negatives (8,241 pairs bit-exact; 2,689 loader cases, zero panics)], [`cargo run --release 2>&1 | tee results.txt`],
  [`experiments/external-consumers/`], [The zero-external-consumer verdict for the cross-domain raw surface, with the Jetsam caveat and auditable search log], [read `REPORT.md` (probe; clones and search logs gitignored)],
  [`experiments/malicious-node/`], [MN-A/B/C: first-sync snapshot poisoning (stage → install → finalize → estrangement → re-serve, on the real production functions); replay publication confirmed by-design with fork-correct consumers; dialect-confusion negative with one open regression surface], [`./run.sh` (default schedule → `results.txt`; isolated-V2.1 → `results-isolated.txt`)],
  [`experiments/dark-surface/`], [D1–D5: the 79-method RPC reachability matrix, hidden CLI flags (incl. key-material export/import), the undocumented env surface (`NOID_GUI_NODE_BIN`), the 39-operation profile table (17 dead, 2 phantom ops), and live loopback demonstrations incl. `paranoid_stop`], [read `README.md`; live demos in `results.txt` and `demo/`],
  [`experiments/nft-races/`], [First execution of op 80 (`ExecuteTrades`) through the real State apply path (conservation exact, order re-derived, atomic batch abort); every live listing race decided by the single-UTXO escrow invariant incl. a stale listing across a real reorg; the settlement circuit doubly dead as shipped, witness built and sound-shaped in intended config; wash-trip economics (16,400 µNOID/cycle, self-miner nets 2,500)], [`cd poc && cargo test --release 2>&1 | tee ../results.txt`; then `cd ../reorg && cargo run --release 2>&1 | tee -a ../results.txt`],
)

#v(1em)
*Source of truth.* Every number in this paper is taken verbatim from `/Users/ember/dev/parano1d-break/experiments/FINDINGS.md` (the consolidated record with certainty classes), cross-referenced against `/Users/ember/dev/parano1d-break/PROVENANCE.md` and the per-lane artifacts cited there. Where this paper compresses, it compresses toward the weaker claim.
