# Kimi root session - running-findings narrative (session_506c8a03, 2026-11-15, died 03:03 on quota)

Assistant messages over 400 chars, in order. These are the root agent summaries to ember; findings marked confirmed were independently rerun by the root.


## @ 01:10:56

Yes—their `noid_soundness/src/poseidon2b_cryptanalysis.rs` is primarily an **attack-correspondence and complexity calculator**. It pins the production parameters, substitutes them into formulas from several ePrints using `BigUint`, and checks the resulting numbers. It does not search for new trails, test the small-subfield closure, generate collisions, or attempt a forgery. That is useful bookkeeping, but it is not comprehensive cryptanalysis.

What we have so far:

- **Confirmed a real primitive break:** a `TAG_CAPSNODE` collision in **72,274 evaluations and 0.128 seconds**, using the production code. My reproducer is in `poc/src/main.rs`.
- **Confirmed their own bound:** **41.65 proved bits at \(T=2^{32}\)** and a vacuous bound at \(T=2^{41}\). This is only a weak reduction, not a \(2^{41}\)-cost attack.
- **Found an important error in our earlier gist:** production UTXO nodes use the two-permutation `compress_with_tag`, so the claimed cheap transfer into `TAG_EXSTNOD` state-tree collisions is wrong. I also found no production use of the vulnerable compressor for `TAG_SEGMENTTREE`.
- **No fresh-request forgery yet.** Capsule verifiers recompute leaf hashes and pin tree depths; an arbitrary internal-node collision does not automatically supply valid alternative leaves or codewords.
- **Strongest currently supported criticism:** the security theorem uses a uniform random-completion model, while the shipped permutation has invariant subfields and cheap collisions outside the fixed address slice. Therefore that theorem does not establish security for the shipped primitive—even without a fresh-request forgery.
- Relevant upstream suites pass: **112 unit tests plus doc tests**. The vulnerable permutation/compressor is unchanged on current upstream `main`.

I have the full paper and the Poseidon2b reference already. I’m adding four independent lanes: address-preimage cryptanalysis, transaction/intent binding, native-contract authorization, and a critical review of what their “soundness certificate” actually proves.

## @ 01:13:21

Yes—**that program already exists and has run successfully**. `poc/src/main.rs` imports the production `noid_core` and `noid_poseidon2b` crates directly, finds a collision, and checks both distinct inputs against the real compressor. It produced the same digest after **72,274 evaluations in 0.128 seconds**.

A ZKP is also feasible: the public statement can be just “I know two distinct input pairs with equal `TAG_CAPSNODE` outputs”; the witness is the two pairs. The circuit reveals the target primitive but **not the pairs, your search code, or how you found them**. Use an independent proof system/hash, since building the credibility proof on the same broken primitive would be circular. ZK is optional here—the ordinary Rust reproducer is much easier for everyone to verify.

## @ 01:15:55

Done: `poc/src/bin/verify_collision.rs` contains **only hardcoded inputs and two production compressor calls**—no search or grinding. I ran it successfully.

```rust
use noid_poseidon2b::native::compression::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::native::domain::TAG_CAPSNODE;

fn b(s: &str) -> [u8; 32] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn main() {
    let a = (
        b("2d8a16372955da0403d2a69740bd860cb83fa1c69c1e7129c671a616a5951930"),
        b("e681a0b3bee99f85353fbaac20999ef2ec23409e03068aa9e97759648e114d01"),
    );
    let c = (
        b("6f7bb24eb68869bdb4f59a582a6a984c5e40eaa3ade728c02e887b0303825ca6"),
        b("e6312b07507b9f4cc0675c2f9fcd656d4e90a4a753c0a5314779d2d3896cf208"),
    );

    assert_ne!(a, c);
    let x = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &a.0, &a.1);
    let y = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &c.0, &c.1);
    assert_eq!(x, y);
    println!("{}", hex::encode(x));
}
```

Output:

```text
b2cf54a478131d318ebef1af2ec0e315245b8ee3af237a21e3fef04e4a20478b
```

And yes, this is a **genuine collision-resistance failure in a production authorization-PCS primitive**, and it contradicts the random-completion assumption used by the security proof. It is **not yet a demonstrated authorization forgery**: we have not connected these specially aligned child digests to valid fixed-depth leaves and a verifier-accepted capsule. The accurate claim is “production `TAG_CAPSNODE` collision,” not “forged authorization.”

## @ 01:27:41

The two completed lanes were negative in a useful way:

- **Transcript/PCS:** no concrete fresh-request acceptance or other high-severity soundness bug found. Request/address binding, challenge ordering, sumchecks, opening linkage, recursive verification, and canonical decoding all held up.
- **State trees:** no state-tree exploit from this collision. Production `EXSTNOD` uses the other compressor, `TAG_SEGMENTTREE` is unused, leaf transitions and depths are constrained. It did confirm a **narrow same-epoch fork-replay condition** while inputs remain unspent and output slots empty—not arbitrary cross-network replay.

Two earlier lanes are still running. I’ll add coder agents with separate experiment directories so they can build and run attacks rather than merely read code. The targets are chained Merkle/type-confusion constructions, transcript/sponge consequences, BaseFold commitments, and an actual same-anchor replay demonstration.

## @ 01:27:41

The fact-check lane confirmed the core algebra but caught several more corrections we should carry forward:

- The \(2^{16.3}\) cost is a birthday-heuristic estimate, though our **72,274-evaluation experiment independently confirms a practical collision**.
- The quantum sponge argument needs an aligned **no-pad/unpadded-duplex** schedule; padded finalization breaks the claimed confinement.
- The theorem is conditional on its ideal model; the accurate criticism is that the model is unjustified for the shipped permutation, not that the guarded lemmas are logically false within their hypotheses.
- Their current Poseidon2b paper specifies Turbo-SHAKE constants, while Parano1d matches the legacy small-constant Binius reference.
- The earlier asymptotic QROM denominators were wrong; the measured fifth-degree degradation and \(2^{41}\) vacuity point are right.

Five coder lanes are now running. I’m adding one more for address-preimage/weak-key attacks so authorization-key recovery is explored independently of PCS flaws.

## @ 01:27:41

The last initial analysis lane also found no cheap capsule forgery. It precisely identified the barrier: a useful collision must lie on a verifier-recomputed path **and** preserve source→mid→tail fold equations. Arbitrary collisions and same-row leaf/node aliases provide neither automatically. The new coder lane is testing that boundary directly rather than inferring consequences from the primitive break.

I’m also checking whether Parano1d ported constants that the upstream Poseidon2b project later corrected—the ePrint explicitly mentions a fixed 128-bit Binius implementation issue.

## @ 01:27:41

The unchanged certificate does print `PASS`, but its final line is explicitly conditional on the **fixed-Poseidon2b \(\Delta\)** premise. That gives us a cleaner target than overstating Merkle consequences: a random permutation sends \(V_{16}^4\) back to \(V_{16}^4\) with probability \(2^{-448}\), while production does so with probability \(1\). I’m checking exactly how they define \(\Delta\); if it is the distinguishing/quantum-interface advantage, that premise is not merely unproven—it is decisively violated.

## @ 01:27:41

There’s an important caveat in their definition: \(\Delta_{\rm P2b}\) is **not** ordinary qPRP-distinguishing advantage—they explicitly exclude that interpretation. So the one-query subfield distinguisher alone does not break their numerical premise. The remaining question is sharper: whether a `CAPSNODE` collision itself qualifies as their `BadTypedBind`/typed-binding failure event. If it does, their required \(\Delta<0.45\) condition fails immediately; if it requires a semantic alternative opening, we still need the chaining work.

## @ 01:27:41

The upstream Poseidon2b repository’s current `main` still contains the same small constants, so this is **not** a case where Parano1d merely missed a later upstream constants fix. Their research page also explicitly invites “a production-reachable collision”; our result is definitely a collision in the production function, while the coder lanes are determining whether it is reachable as a complete valid commitment rather than only at the node API.

## @ 01:27:41

Exactly—especially their \(\Delta\). It is defined as the residual fixed-permutation compiler failure that their ideal model fails to capture, and the certificate **never evaluates or bounds it**. It prints `PASS` *if you assume* \(\Delta<0.45067\). That includes precisely the hard question of what collisions and domain confusion do to the typed semantic graph.

So the accurate criticism is not merely “your theorem is false within its definitions.” It is:

> Your executable certificate checks the arithmetic after the difficult fixed-permutation instantiation problem has been assumed away. The production collision is evidence that this unevaluated premise cannot be silently treated as a small random-permutation error.

Their definitions can guide the analysis, but they are not a shield. I’m checking both possibilities: exploits inside their stated game, and security-relevant behavior their game excludes or defines away.

## @ 01:35:43

All seven newer coder lanes are still running; none has returned a final result yet.

Current visible progress:

- **Capsule chains:** three experiment binaries exist—`merkle_collision.rs`, `pcs_verify.rs`, and `alias_depth.rs`—so it is actively testing Merkle verification and alias/depth chaining.
- **BaseFold:** crate scaffold exists; still in its earlier source-analysis/setup phase.
- **Transcript algebra:** experiment program exists.
- **Fork replay:** experiment program exists.
- **Native intents:** experiment program exists.
- **Address preimage:** experiment plus separate field-arithmetic modules exist.
- **Typed-binding \(\Delta\):** experiment program exists and release dependencies have been built.

There are **no new completed findings to claim yet**. I’m not interpreting the existence of PoC files as evidence that an attack succeeded; I’ll report each result only after the agent returns and I verify its output.

## @ 01:38:50

It is not all depth-and-exhaustion. **Two lanes have genuinely interesting new behavior, and one has a stronger Merkle-binding demonstration with a remaining semantic gap.** These are code inspections, not yet my independent confirmation of every run.

### Most promising

- **Legacy-FRI proof malleability** — `experiments/transcript-chains/src/main.rs:191`
  - Builds an honest `noid_fri` proof.
  - Appends an arbitrary extra lane to the final codeword.
  - Builds another variant containing that extra lane plus `PAD`.
  - Regenerates query openings for each variant.
  - The program asserts that **both altered proofs pass the real `noid_fri::verify`** for the same statement.
  
  If confirmed through production decoding/recursive verification, this is more than a hash collision: it is **distinct proof encodings accepted for one statement**, with no collision search. It is not an invalid-statement proof, but it could undermine canonical-proof binding or typed extraction assumptions.

  The same lane also shows legacy transcript schedules where `absorb(x)` and `absorb(x, PAD)` produce identical challenges. The framed newer challengers correctly reject those ambiguities.

- **Real same-anchor fork replay** — `experiments/fork-replay/src/main.rs:78`
  - Generates one legitimate wallet authorization.
  - Constructs two forks with **different parent block IDs and different state roots**, but the same epoch anchor.
  - Reuses the identical transaction and proof on both.
  - Checks public logic, epoch rules, owner authorization, and the actual state-transition function on each fork.
  - Also checks that a second spend on the same fork fails.

  This is a real end-to-end protocol experiment, not a primitive-only observation. It demonstrates conditional **cross-fork replay**, not a newly forged authorization.

### Interesting, but still has a reachability gap

- **Fixed-depth Merkle binding failure** — `experiments/capsule-chains/src/bin/merkle_collision.rs:24`
  - Uses the collision to build two different fixed-depth trees with the same cap.
  - Produces different sibling paths, both accepted by the production prehashed Merkle API.
  - Then substitutes a real `CAPSMIX` leaf and shows rejection.

  So it has advanced beyond “two arbitrary compressor inputs”: it demonstrates **two Merkle forests with one cap and accepted paths**. But the colliding child values are not supplied as valid `CAPSMIX` leaf openings. The program estimates random alignment at \(2^{-224}\), so the semantic chain is still missing.

- **Native + recursive path chaining** — `experiments/typed-binding/src/main.rs:86`
  - Feeds colliding branches through production native Merkle paths and the recursive `FfMerkle` trace builder.
  - Gets matching roots in both.
  - Supplies valid subtree openings for the common siblings, but not for the colliding children.

  This is useful chaining work, but the program itself correctly says it has **not** instantiated `BadTypedBind` or violated \(\Delta\).

### Mostly negative controls or exhaustion

- **Full capsule PCS:** naive collision substitution, alias substitution, their combination, and shortened caps all reject. The interesting result is the precise acceptance barrier, not an exploit.
- **Alias/depth:** constructs a leaf/internal-node alias with one root at depths zero and one; production’s pinned-depth verification rejects the mismatch.
- **Native intents:** the earlier “builder can force minimum output and maximum fee” claim looks **wrong**. Low-level quotation accepts the fabricated values, but block preparation rejects them; the recursive market constraint also rejects the altered output.
- **Address preimage:** confirms the algebra and a trivial low-entropy weak-key class, but no uniform-key attack. Secret-only Frobenius does not preserve addresses because the `ADDRFIX` IV moves; reduced-field inversion provides no asymptotic gain.
- **BaseFold:** no substantive experiment yet.

The **FRI malleability** is the most surprising new surface. The **fork replay** is the closest complete protocol demonstration. I’ll prioritize independent verification of those two when the agents return.

## @ 01:40:01

The typed-binding lane finished with a precise negative result: it lifted the collision into four production-shaped native/recursive Merkle paths, but without valid leaf openings for the colliding children it still does not instantiate `BadTypedBind` or violate \(\Delta\).

The fork lane reports a positive end-to-end replay: one real transaction and one real owner-auth bundle pass on two different states sharing an epoch anchor. I’m rerunning both experiments myself before treating those as confirmed deliverables.

## @ 01:40:01

I independently reran both completed experiments.

- **Same-anchor fork replay: confirmed.** The identical 323-byte transaction and serialized owner-auth bundle passed the real epoch, public-logic, owner-auth, and UTXO state-delta checks on two forks with different parent IDs and state roots. A second spend on the same fork failed with `InputMismatch`; changing the epoch failed with `BadEpochAnchor`/`AuthProof`.
  - Scope: this is **exact transaction/proof replay across same-anchor forks**, not a newly forged proof or arbitrary cross-chain replay.
  - It does not yet include full node/block acceptance, PoW, transaction roots, or recursive `HistoryStep` verification. Reproducer: `experiments/fork-replay/run.sh`.

- **Typed-binding witness: confirmed but limited.** Four production-shaped Merkle paths verify natively and four matching recursive trace roots replay correctly. However, the colliding children still lack protocol-valid leaf openings, so this does **not** establish `BadTypedBind` or a \(\Delta\) violation.

The fork replay is our first completed protocol-level result beyond the primitive collision.

## @ 01:44:12

It is a real **replay property**, but its severity depends on intent: short competing forks often reuse transactions by design, and finality leaves only one canonical spend. It becomes a clear protocol flaw if authorization is supposed to be fork-specific, or if independent networks can share epoch anchors. I’ll keep that distinction rather than labeling every replay a double-spend.

Other high-value classes we have not covered are state-machine conservation, block/mempool validation gaps, recursive/native mismatches, contract-rights bypasses, upgrade/version replay, and transaction/proof identity malleability. I’m assigning six new coder lanes to those areas.

## @ 01:44:12

Two more completed lanes clarify the picture:

- **Legacy `noid_fri` malleability is real at its public verifier API:** distinct five- and six-lane final-codeword proofs verify for the same statement, with no collision search. But the agent found no current Wallet caller, and newer BaseFold pins exact final lengths—so this is not yet a production authorization break.
- **The earlier worst-fill/native-trade accusation is disproved.** Host block preparation and the recursive relation both reject fabricated minimum-output/maximum-fee executions.

I’m rerunning both to confirm the outputs and their limits.

## @ 01:44:12

Independent reruns confirm all three results:

- **Legacy `noid_fri`:** two distinct final-codeword encodings verify for the same statement. This is genuine proof malleability at the legacy public verifier API, but no current Wallet caller was found, and newer BaseFold pins exact lengths.
- **Native trade:** no accepted worst-case-fill vulnerability. Fabricated outputs/fees are rejected by both host preparation and the recursive relation.
- **BaseFold/raw Merkle:** a second production node domain, `IVCPCSN_`, fell in **111,525 evaluations / 0.29 seconds**. Distinct raw openings verify, Ligerito-style multiproofs share a root, recursive path replay matches, and raw APIs accept fixed-point depth changes. However, full BaseFold C0/C1 verification rejects the altered openings and pins path lengths. The fixed-IV leaf hash currently prevents lifting this into real codeword trees.

So we now have two independently reproduced compressor collisions and one legacy-FRI malleability result. The remaining escalation barrier is consistently **valid leaf/descendant semantics**, not the node hash calls.

## @ 01:44:51

The address-preimage lane is also complete and independently reproduced: **no unrestricted full-round key recovery found**.

- The `ADDRFIX` IV prevents direct lane-wise subfield confinement and nontrivial Frobenius-equivalent keys.
- Guessing hidden output lanes and inverting still costs \(2^{256}\) candidates.
- A raw-`u8`-per-lane weak-key class is recoverable exhaustively, but that is only a low-entropy key issue with probability \(2^{-240}\) under uniform keys.
- Reduced-field collisions demonstrate equivalent keys but no asymptotic preimage speedup.

So we currently have **no owner-secret recovery path and no fresh-request authorization forgery**. Reproducer and full results are in `experiments/address-preimage/results.txt`.

## @ 01:48:10

The capsule lane is complete and independently reproduced:

- **Positive Merkle-layer result:** two distinct prehashed leaf vectors with the exact production source-tree geometry—**depth 13, cap depth 3**—produce the same eight-hash commitment, and production verification accepts both altered openings.
- **Alias result:** a real `CAPSMIX` leaf digest can equal a `CAPSNODE` digest and authenticate at different depths when depth is caller-selected.
- **Full PCS result:** every altered opening failed—source substitution, mid substitution, alias substitution, their combination, and shortened caps. Accepted altered full PCS openings: **0**.

So the precise statement is now:

> We have a production Merkle commitment/opening collision at the correct tree geometry, but not valid `CAPSMIX` preimages for the colliding children and not a full capsule attack.

That remaining leaf-reachability gap is substantial; more random node collisions alone will not close it.

## @ 01:50:29

That is a reasonable concern, but the accurate description is **compositional fragility**, not a break waiting to be claimed. The system has several sharp edges that each become dangerous only when composed:

- raw/prehashed APIs accept what higher-level semantic verifiers reject;
- leaf and internal-node representations can alias;
- raw paths permit depth changes that production PCS pins;
- transcript encodings can be malleable without immediately changing the statement;
- the security model assumes a unique typed semantic graph, while the primitive permits multiple graphs.

The most plausible finish is **not** more birthday grinding. It is finding one caller that authenticates attacker-supplied child digests without recomputing valid descendants, or one cross-protocol path that reinterprets the same digest under different semantics. I’m checking those call sites directly.

## @ 01:50:29

The caller sweep found no additional unguarded production path: the live BaseFold/Ligerito call sites recompute leaf hashes, and the other recursive hits are tests. So the current internal callers have the necessary guardrails.

That still leaves a genuine architectural concern: **the unsafe raw APIs are public and accept authentication objects that the semantic protocol would reject**. A future caller, SDK consumer, cross-protocol bridge, or recursive/native mismatch could accidentally bypass those guardrails. The strongest next exploit would be one such composition—not another isolated collision.

## @ 01:54:09

“UC-security” is a useful lens here, with one caveat: they do not claim a formal UC proof, so this is an **assurance gap**, not automatically a contradiction.

The promising questions are different from “can I forge one capsule?”:

- **Concurrent-context binding:** can one valid artifact acquire different meanings in two protocols, sessions, forks, or verifier contexts running concurrently?
- **Simulation/extraction under composition:** does the simulator/extractor still work when the environment interleaves sessions, chooses statements adaptively, and reuses the same global permutation?
- **Non-malleability:** can an observer transform an authorization without learning the witness? Malleability is not automatically a UC failure—it depends on the ideal functionality—but it matters if identifiers, caches, or recursive claims assume canonical artifacts.
- **Ledger semantics:** freshness, reorgs, finality, duplicate handling, and concurrent state transitions can fail even when every local proof verifies.
- **Fairness and liveness:** builder discretion, censorship, query grinding, verification-cost asymmetry, and mempool/block disagreement are mostly invisible to a soundness theorem.
- **Recursive/global-setup composition:** verifier keys, matrix identities, nested statements, and global oracle state must remain bound across arbitrary protocol composition.

The existing lanes cover several of these. I’m adding two specifically for cross-protocol concurrent composition and ledger fairness/liveness, rather than duplicating the current work.

## @ 01:56:34

It definitely is not perfect—we already have concrete counterexamples:

- two cheap production compressor collisions;
- same-cap prehashed Merkle openings;
- legacy-FRI proof malleability;
- exact same-anchor fork replay;
- a reduction bound that becomes vacuous near \(2^{41}\) calls;
- an unevaluated fixed-permutation \(\Delta\) assumption doing a lot of work in the “PASS.”

The open question is **which flaws compose into unauthorized state changes**, not whether imperfections exist. And a failed escalation is still useful: it tells us exactly which guardrail prevents the final step and where to probe next.

## @ 01:58:28

That is a fair assessment **if “consequence” means a demonstrated exploit against accepted chain state**. Right now we have no unauthorized spend, invalid accepted block, owner-key recovery, or fresh-request forgery.

The consequences are currently narrower:

- **Security assurance:** the concrete primitive does not justify the ideal-permutation/random-completion assumptions, and the certificate’s \(\Delta\) remains unevaluated.
- **Primitive correctness:** collision resistance objectively fails at raw node/prehashed-Merkle interfaces.
- **Protocol hygiene:** legacy-FRI malleability and same-anchor replay are real behaviors, but no damaging production use has been demonstrated.

So the honest position is **“serious warning signs, no consequential end-to-end attack shown yet”**—not “definitely broken,” but also not “these results are satisfying.” Closing the gap requires a valid-leaf construction, a live caller for the malleability, or a semantic verifier/extractor failure.

## @ 02:01:00

Exactly—**we found no collision-driven split-brain escalation**.

The reason is that we never obtained the object that matters: **two semantically valid artifacts with one authenticated commitment**. Every attempted route hits a semantic check:

- UTXO state uses the other compressor, and native materialization recomputes the exact root.
- Capsule openings require valid leaves plus source→mid→tail consistency.
- BaseFold recomputes leaf hashes and pins paths.
- Recursive/native checks reject the altered openings we constructed.

So there is no demonstrated scenario where two honest nodes accept different semantics under one root and silently diverge. The same-anchor fork replay is unrelated and visible: it reuses one transaction openly on two forks; it does not hide a semantic difference behind a collision.

At this point, “cheap compressor collisions, **zero demonstrated split-brain or state-equivocation impact**” is the accurate assessment.

## @ 02:11:13

The block-validation lane found no production acceptance vulnerability. A standalone public utility accepts a wrong-epoch block, but the real `MdbxChainContext` rejects it before terminal verification; the utility is not a complete production predicate. I’m rerunning its matrix as well.

The confirmed recovery issue remains low severity: `InvalidProof` is followed by `current_scan_known=1`, while `fresh_scan_known=0`, and duplicate/state-changing controls reject correctly.

## @ 02:12:09

No. The code confirms the exclusion is **per peer and per recovery scan**, not network-wide:

- `observed_txids` is local to one peer response.
- It is merged into that peer’s `state.seen`.
- The malformed transaction is **not admitted** to the actual mempool.
- Only a later valid transaction with the **same txid from that peer during that scan** can be skipped.
- A new scan resets the exclusion; other peers and direct submission are unaffected.

A malicious peer could suppress a txid it relays during that scan, but it could already withhold that transaction. This is a low-severity recovery-bookkeeping bug, not a network-wide transaction-processing shutdown.

## @ 02:12:55

Both lanes came back negative on unauthorized state changes:

- **UTXO/state machine:** 20 adversarial cases rejected, including inflation, forged identities, duplicate slots, wrong owners, allocator manipulation, and invalid expansion. Two valid controls passed.
- **Contract rights:** no unauthorized action or conservation failure; replay, early unlock, excess allowance, and wrong recipients rejected. It also ran 50,000 generated Pruv programs with 428,880 executions and found no native semantic mismatch.

I’m rerunning their main harnesses before adding these to the verified-negative list.

## @ 02:16:23

The full contract audit is now independently confirmed:

- wrong authority, recipient retargeting, excess allowance, early vault calls, and stale replay all reject;
- value conservation and one-shot counter behavior hold;
- budget, recurring-payment, and tranche schedules enforce their documented rules;
- all **50,000 generated Pruv programs / 428,880 executions** agree with native execution;
- all **1,115** legacy/v2.1 arithmetic-relation checks agree.

No unauthorized contract action was found in that coverage. Four research lanes remain active.

## @ 02:22:15

It is not flawless—it has held up well against the **specific adversarial paths we have executed**. That is useful evidence, not a proof of correctness.

We still have concrete primitive collisions, legacy malleability, conditional replay behavior, and major ideal-model assumptions. Meanwhile, the negative audits are bounded by their fixtures, grammar, and activation paths; they do not cover every composition or full mined-block transition.

The fair summary is: **robust semantic guardrails in the paths tested, no high-impact end-to-end exploit found yet**—not “flawless.”

## @ 02:22:42

The UC lane found no concrete composable-security vulnerability. Its supported conclusion is only an **assurance gap**:

- distinct proofs for one statement verify in independent concurrent verifier sessions;
- that does not violate the manuscript’s fresh-intent game or imply a non-malleable/per-session ideal functionality;
- the extractor handles one fresh accepted capsule after the experiment, not arbitrary online UC interleavings;
- no cross-adapter second meaning or native/recursive acceptance differential was found.

I’m rerunning the concurrent-proof experiment to confirm that behavior.

## @ 02:23:54

The version lane found a genuine domain-separation gap: default, isolated-V2, and isolated-V2.1 builds have different fork/authorization rules but derive the **same P2P profile ID and transport namespace**. Mixed binaries can therefore exchange incompatible artifacts.

Its demonstrated impact is test/fork isolation and denial of service, not cross-version proof acceptance—the V3/V4, sparse/matrix, snapshot, and terminal checks rejected the tested downgrades. I’m rebuilding all three feature variants to verify the collision.

## @ 02:26:05

The recursive/sync lane found a validation differential: snapshot header staging checks future drift only on the tip, while ordinary suffix application checks every block. Its PoW-valid four-header sequence is rejected at height 3 natively but staged through height 4 by the snapshot path.

This is currently a **wall-clock admission mismatch**, not an invalid-state proof—an otherwise valid height-4 `HistoryStep` remains an explicit precondition. I’m rerunning that PoC too.

## @ 02:27:24

Confirmed across the isolated builds:

- **V2:** terminal v7, 2 classes at height 24, owner V3.
- **V2.1:** terminal v7, 3 classes at height 24, owner V4.
- Both use profile ID `600c7311…6f85` and protocol namespace `/noid/mainnet/860e70453390bf81/1`.

The default build also derives that same identity despite its different schedule. This is a real P2P domain-separation flaw enabling incompatible peers to exchange artifacts. The tested cryptographic downgrades still reject, so the demonstrated concern is incompatible-peer mixing and denial of service—not proof acceptance across versions.

## @ 02:43:56

Yes. So far we have concentrated on cryptography, transaction semantics, and a few networking paths. The largely untouched attack surface includes RPC admission, external-miner templates, P2P resource handling, storage/reorg/crash behavior, GUI/wallet secret handling, unsafe/prover code, and build/release artifact handling.

I’m opening seven additional coder lanes, again requiring concrete local evidence rather than style complaints.

## @ 02:54:49

The first-pass design is more defensive than the earlier “malformed proofs halt the network” hypothesis suggested: admission does cheap state and fee checks first, caps concurrent proof verification, bounds pending delivery bookkeeping, and shares inbound/outbound memory budgets. The remaining question is whether an attacker can cheaply keep those bounded queues occupied with distinct, state-valid transactions and bad proofs.

## @ 02:54:49

The ledger lane has produced a concrete liveness candidate: with its selected HeaderDAG ancestry at capacity, inserting the next valid canonical child fails before finality is advanced; manually advancing finality restores insertion. It also measured an owner-wide RPC query doing about 5.8× the work of the bounded-page alternative, though the absolute fixture cost was sub-millisecond. I’m checking the production constants and call order before assigning severity.

## @ 02:54:49

The HeaderDAG case is real but bounded, not a free network halt: it requires roughly 1,024 valid PoW headers on the selected ancestry without canonical reconciliation catching up. Ordinary progress advances finality and prunes that ancestry. I would presently rate it a backlog/liveness defect, not a practical unauthenticated DoS; the unbounded owner query is the more direct cost-amplification surface.

## @ 02:54:49

So far, there are **availability concerns, but no demonstrated cheap network-wide DoS**.

- **Unbounded owner-query amplification:** `paranoid_getSlotsByOwner` retrieves every UTXO for an address while holding the chain read lock (`parano1d/noid_rpc/src/server.rs:2025`), despite a bounded paginated API already existing. With 4,096 UTXOs, I reproduced **6.16×** the work of a 32-record page—382 µs versus 62 µs median. The absolute cost was small in this fixture, so this is a scaling risk, not yet a demonstrated node outage.
- **HeaderDAG capacity deadlock under backlog:** a full selected ancestry rejects its own next child with `Capacity`. Production reconciliation inserts missing canonical children *before* advancing finality (`parano1d/noid_node/src/main.rs:4779-4805`), so the capacity error can prevent the pruning that would resolve it. Production capacity is 1,024, and exploitation requires valid PoW headers plus canonical reconciliation falling far behind. Real liveness defect, but not a free malformed-packet attack.
- **Scan-bounded recovery suppression:** a malformed proof causes its txid to be marked known before admission succeeds. A later valid authorization for that body is skipped during the same peer-recovery scan. A fresh scan clears it; no persistent poisoning or consensus impact.

Conversely, the earlier malformed-proof hypothesis looks substantially weaker: the node performs cheap state/fee/conflict checks before proof verification, bounds concurrent verification, deduplicates payloads, and applies shared network memory budgets.

The dedicated P2P and RPC experiments are still running; those should determine whether the bounded queues themselves can be occupied cheaply enough to create meaningful amplification. Reproduction for the ledger results is in `experiments/ledger-fairness`; I independently reran it successfully.

## @ 02:57:14

One important scope correction from the completed ledger audit: the unbounded `getSlotsByOwner` method is **local-only under the pinned RPC credential policy** (`parano1d/noid_rpc/src/server.rs:3846`). Remote mining/operator credentials cannot invoke it, so its measured cost amplification is a local resource concern—not a remote DoS primitive.

The strongest ledger availability result remains the **conditional HeaderDAG capacity stall**. The detailed evidence and reproduction commands are in `experiments/ledger-fairness/REPORT.md`. The P2P and RPC lanes are still examining remotely reachable surfaces.
