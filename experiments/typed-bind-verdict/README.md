# Typed-binding verdict: do our attacks instantiate THEIR bad events?

Formal-analysis lane of the parano1d adversarial audit. Question, exactly as
scoped: do the attacks we already hold — (a) the TAG_CAPSNODE node collision,
(b) the C21 cross-tag claw, (c) the GF(2^16) algebra — instantiate the bad
event or violate the assumed bound **as the authors themselves define them**?
No new attacks were run; everything here is derived from their paper, their
certificate code/docs, and the PoC-verified objects of the prior lanes.

 verdict summary up front:

| Candidate | Verdict (walk reading) | Verdict (dictionary reading) |
|---|---|---|
| (a) same-tag node collision | EXCLUDED-BY-CLAUSE | INSTANTIATES (collision cause) |
| (b) cross-tag claw (C21) | EXCLUDED-BY-CLAUSE + UNMODELED-DOMAIN | INSTANTIATES (collision + domain-confusion causes) |
| (c) GF(2^16) structure → Delta | NOT FORCED (premise unevaluated) | **FORCES-DELTA: Delta ≥ ~0.95 > 0.4507 (C1) and ≥ ~0.81 > 0.3125 (T=2^64)** |

The two readings are theirs, not ours — their corpus states the typed-binding
bad event twice, with different scope, and never disambiguates. Both columns
are licensed public claims; see "Verdicts and licensed public claims".

---

## 1. Their definitions, verbatim

Locations: paper = Zenodo 23778742 PDF (text extract
`post-quantum-signatureless-authorization.txt`) and the artifact's LaTeX
sources; certificate = `parano1d/noid_soundness/` (read-only checkout
`52938433a30b185ab007afaa15166a632cf2e82e`).

### 1.1 The certificate's security game (chain level)

`noid_soundness/docs/category-one.md`, "The security game":

> "The public instance is a terminal `HistoryStep` State. The adversary wins
> when the production verifier accepts its terminal proof but no valid
> execution from genesis ends in that State."

`noid_soundness/README.md`: "The remaining rows concern a different game:
acceptance of an invalid terminal State whose ancestry starts at genesis by a
quantum adversary."

### 1.2 The certificate's bad events

`docs/category-one.md`, "One all-root event" (bad events of the ideal
compiler; the fixed game's analogues are folded into Delta, section 1.4):

> "Every transcript family and statement is placed in a typed,
> statement-keyed oracle namespace. Let `D` be the single measured
> compressed-oracle database. Define `BadAll(D)` to mean that there exists any
> represented accepting wallet or History root for which the deterministic
> extractor fails or returns an invalid local witness."
>
> "Two boundary events are kept explicit:
> ```text
> MissRep       a required noncertified child is absent from D
> BadTypedBind  a collision, ambiguous encoding or domain confusion changes
>               the typed semantic graph
> ```"
>
> "BadState ⊆ BadAll ∪ MissRep ∪ BadTypedBind."

Restated in `docs/v2-correspondence-2026-10.md`, "Deterministic v2 all-root
extension" (second, broader wording):

> "Define `MissRep` when a required noncertified child is absent and
> `BadTypedBind` when **object/statement/namespace binding fails**."

And in "Model and authority" of the same document:

> "When a digest is used as authority, the implication from equal digests to
> equal named objects is conditional on the certificate's binding game."

### 1.3 The typed semantic graph (the object BadTypedBind protects)

The only place either document gives the graph structure is the paper's
Appendix H.1, "A finite typed dictionary"
(`manuscript/sections/k-native-extraction.tex`, lines 27-58; PDF lines
1652-1676):

> "A free permutation record $(x,y)$ gives the capacity edge $x_C\to y_C$ and
> node anchor $y_R\oplus x_R$, with children $(x_R, x_C\oplus v_{\rm node})$.
> Reachability starts at the three sponge capacities. Exclude a reachable
> address capacity, a reachable edge ending at a root, and two distinct
> reachable edges with the same endpoint. Every reachable vertex then has a
> unique rooted path."
>
> "Each row supplies a node anchor and at most one typed leaf anchor. …
> Let $\mathcal A(D)$ be the resulting at most $2|D|$ anchor classes. The
> finite guard additionally requires the map $(a,j)\mapsto g^j(a)$, … to be
> injective, and its image to avoid $Z_D=\{0,b_*\}\cup\{r\oplus v_{\rm node}:
> r\text{ is a reachable capacity}\}$."

Lemma H.1 (Typed native reconstruction), same file:

> "On a guarded database, a cap and index determine at most one source
> payload and at most one middle payload. Every accepted opening whose free
> pairs occur in that database has the reconstructed payload."

with the load-bearing proof step (line 78): "At exponent zero **the unique
node row** supplies both children."

Two structural facts about this graph, both decisive later:

* **Single node domain.** Equation (52) (PDF line 1620): "The native node is
  $N(a,b)=\operatorname{rate}P(a,b\oplus v_{\rm node})\oplus a$" — **one**
  $v_{\rm node}$; and "The three sponge capacities for `CAPSMIX_`,
  `CAPS256_` and `KSCH256_` are distinct and outside the address slice.
  Their number is $c=3$." The paper's dictionary has exactly one node tag
  and three leaf-sponge tags. The simulator of Appendix F uses the same
  single-domain model (`f-merkle-simulation.tex` line 83: "At a node with
  child labels $a,b$ and parent label $z$, propose $x=(a,b\oplus v_{\rm node})$…").
* **The guard does not exclude node collisions.** The guard's clauses
  (reachable address capacity / edge at a root / two reachable edges with the
  same endpoint / anchor-orbit injectivity on classes / $Z_D$ avoidance) are
  all about *capacity edges and anchor orbits*. Two distinct records whose
  **node anchors coincide** (i.e. two children-pairs with the same feed-forward
  digest) violate none of them: they form one anchor class with two rows.
  The reconstruction lemma's "the unique node row" is not delivered by any
  guard clause. Their certificate implicitly concedes this: it is exactly why
  the ideal bound carries a *separate* global commitment-collision term
  (section 1.5), priced at the generic rate. In the ideal model a node
  collision is a tiny-probability event charged by that term; the lemma
  operates conditioned on its absence.

### 1.4 Delta_P2b and the certificate's PASS condition

`docs/category-one.md`, "Fixed Poseidon2b boundary":

> "`Delta_P2b(R)` is any explicit upper bound satisfying
> \[\Delta_{\rm P2b}(R)\ge\max\left\{0,\Pr[\mathsf{BadCompiler}]_{\rm fixed\
> P2b}-\varepsilon_{\rm ideal}(R)\right\}.\]"
>
> "The fixed experiment uses the exact `FsLaneChallenger` framing, public
> Poseidon2b constants, **typed commitment domains**, canonical
> nested-artifact recording, native verifier and recursive verifier gates."
>
> "It is not defined as a qPRP advantage between a public fixed permutation
> and a secret random permutation. Direct evaluation would distinguish those
> games. … It does not evaluate `Delta_P2b`, because the ideal-degree attack
> model neither bounds every fixed-permutation failure event nor supplies the
> required coherent quantum statement."

with `BadCompiler = BadAll ∪ MissRep ∪ BadTypedBind` and
`Pr[BadState]_production ≤ ε_ideal + Δ_P2b` (eq. 37).

The shipped numbers (`README.md` lines 35-41, `docs/category-one.md` eqs.
(1),(2); reproduced by `src/qrom.rs` / `src/resource.rs` as
`half_success_headroom_at_two_to_64` / `fixed_poseidon2b_delta_headroom`):

```text
at T = 2^64:      Delta_P2b   < 0.312471062061564258   (= 1/2 − 0.187528937938435742)
at C1 envelope:   Delta_P2b^C1 < 0.450669651771636316  (= 1/2 − 0.049330348228363684)
```

`src/main.rs:517` prints the PASS as conditional:
`"assessment: PASS under the typed parallel-QROM, coherent response-cost, and
fixed-Poseidon2b delta premises"`. Confirmed: nothing in `noid_soundness`
computes, estimates or bounds Delta; `poseidon2b_cryptanalysis.rs` states in
its module header, "The production QROM delta remains a separate premise of
the end-to-end theorem." (This lane endorses the prior fact-check finding:
Delta is not a qPRP advantage, and the subfield *distinguisher* by itself
attacks no premise. The route below is the collision *event*, not a
distinguisher.)

### 1.5 The ideal bound whose fixed-game counterpart Delta must absorb

`docs/category-one.md` eq. (11) and its term table:

> `ε_ideal(T) = min{1, 6T²(κ* + (2T+1)/2^255) + 6T³/2^256}`
>
> | `6T^3/2^256` | one global commitment and binding collision event |

(qrom.rs `binding_collision_term`; parameters.rs pins `digest_bits = 256`.)
On the simulator side the same generic pricing appears as the Appendix F
fences: proposed node pairs must have "distinct inputs and outputs", with
collision fences costing `C̄ = (L̄+1)C(m,2) + L̄m² + (L̄+1)mn` over `s = 2^256`,
`C̄/s < 2^-214` — i.e. in their ideal model a node-digest collision is an
explicit bad event at the 2^256 generic birthday rate.

### 1.6 What instantiating each event formally requires (derivation table)

| Their event / bound | Formal requirement to instantiate | Where our objects stand |
|---|---|---|
| `BadState` (the game win) | Production *terminal* verifier accepts an invalid State | No lane has this; leaf wall blocks the lift from node collisions (leaf-reachability Finding C) |
| `BadAll` | A **represented accepting** wallet/History root whose extractor fails or yields an invalid witness | Claw/collision objects are prehashed; never represented accepting roots |
| `MissRep` | A *required* noncertified child absent from D | Nothing requires our objects as children |
| `BadTypedBind` (cause side) | "a collision, ambiguous encoding or domain confusion" — (a) and (b) each are literally this | satisfied by (a); (b) satisfies two causes at once |
| `BadTypedBind` (object side) | "…changes the typed semantic graph" — **which graph is the entire dispute**: the H.1 dictionary over the measured database D, or only the extractor worklist over represented accepting roots | (a),(b) change the dictionary graph; they never touch the worklist graph |
| Delta_P2b premise | Pr[BadCompiler]_fixed − ε_ideal < 0.3125 / 0.4507 | forced false under the dictionary reading of BadTypedBind; unevaluated (not false) under the worklist reading |

---

## 2. The model-vs-object gap (coordinator's question, answered)

The paper formalizes **one** node domain (`v_node`, eq. 52) and three leaf
sponge capacities (`c = 3`). Production ships a *tag-parameterized* node
compressor with at least two 1-perm node domains (`TAG_CAPSNODE`,
`IVCPCSN_`), one 2-perm node domain (`EXSTNOD`), and four leaf sponges. Their
own October review states the production function
(`docs/october-2026-attack-review.md`):

> "The actual tagged compression is
> ```text
> W = (a0, a1, b0 + IV_hi(tag), b1 + IV_lo(tag))
> H_tag(a,b) = (P(W)_0 + a0, P(W)_1 + a1).
> ```"

and the certificate's fixed experiment is defined to include "typed
commitment domains" (plural) — but **no document in the corpus defines a typed
dictionary over multiple node domains**. Consequences:

1. Under the paper's own H.1 dictionary, a `CAPSNODE` row and an `IVCPCSN_`
   row are *the same kind of row* (one `v_node`): the cross-tag claw is, to
   their formal model, a plain node collision — two rows, one anchor class —
   and the "domain confusion" cause has no model object to attach to. Their
   definitions do not describe the shipped domain structure at all.
2. Under the certificate's plural-domains language, the inter-domain binding
   property (equal commitment bytes must not authenticate objects in two
   domains) is exactly the property the shipped design comment claims
   (`noid_poseidon2b/src/native/compression.rs:319-325`: "the caller-selected
   tag whitens the right-input lanes so distinct trees cannot share
   collisions") and exactly the property the claw refutes at 2^16 — and it
   appears in their security statement only inside the never-evaluated Delta
   premise, via BadTypedBind's one under-specified line.
3. No clause anywhere catches the cross-tag case *as a cross-tag case*. The
   only clauses that can catch it at all are the two general ones — "a
   collision … changes the typed semantic graph" and (v2) "object/statement/
   namespace binding fails" — and their scope is the reading dispute of
   section 3.

---

## 3. Per-candidate derivations

### Candidate (a) — TAG_CAPSNODE node collision (same domain)

Object: two distinct (left,right) input pairs, one digest, under the single
tag `TAG_CAPSNODE`, through the production
`compress_flat_feed_forward_with_tag`. Evidence:
`poc/src/bin/verify_collision.rs` hardcodes both pairs, asserts
`assert_ne!(first, second)` and `assert_eq!(first_digest, second_digest)`;
mechanism and fresh regenerations in leaf-reachability
(`results.txt`, bin 1: `"known_production_collision": digest … b2cf54a4…78b,
"digest_in_GF2_16": true, "left_children_in_GF2_16": true,
"right_children_in_IV_plus_GF2_16": true`; fresh zero-left birthday 167,686
evals / 0.29 s; `P(K^4)=K^4` invariant 4096/4096).

Derivation against their events:

* `BadState`: no. Nothing submits a terminal State; the semantic layer
  recomputes leaf hashes.
* `BadAll`: no. No represented accepting root exists whose extraction fails.
* `MissRep`: no.
* `BadTypedBind`, cause side: **yes, literally** — this is "a collision".
* `BadTypedBind`, object side: depends on the graph.
  * *Worklist reading* (the graph the certificate's proof walks: "a
    deterministic worklist starts at the accepted terminal … reconstructs its
    canonical nested artifacts", and `BadAll`'s quantifier "any represented
    accepting … root"): the collision inputs are prehashed digests; every
    semantic consumer recomputes leaf hashes (leaf-reachability Finding B:
    `noid_fri_binius/src/capsule.rs:805`, `zk_capsule_pcs.rs:1000,1018`,
    recursive twins), so no represented artifact's reconstruction is ever
    ambiguous. **EXCLUDED-BY-CLAUSE**: the clause is "changes the typed
    semantic graph", scoped by their own quantifiers to represented
    accepting artifacts.
  * *Dictionary reading* (the graph H.1 actually defines, over the measured
    database D): the two colliding evaluations are records of D; they form
    one anchor class with two node rows and two child-pairs; "the unique
    node row supplies both children" fails at that anchor; a cap (plus index
    at exponent zero) no longer "determine[s] at most one source payload".
    This is precisely the event their ideal bound prices at `6T³/2^256`.
    **INSTANTIATES** (cause: collision).
* Note what their definitions do *not* say: no clause requires semantic
  verifier acceptance for BadTypedBind. That requirement is read in from
  `BadAll`'s quantifier and the worklist — the strongest textual basis their
  side has, and it is a scope inference, not a stated condition.

### Candidate (b) — C21 cross-tag claw (strongest candidate)

Object: one 32-byte digest valid as a node root in **both** the `CAPSNODE`
and the `IVCPCSN_` domains; two prehashed 2-leaf trees with identical
commitment bytes, each accepted by the production raw verifier
`verify_source_batched_merkle_proof_to_cap` under its own node hasher.
Evidence (leaf-reachability `results.txt`, bin 2):
`"capsnode_raw_verify": "accepted"`, `"ivcpcsn_raw_verify": "accepted"`,
`"roots_identical": true`, `"leaves_distinct": true`,
`shared_digest_hex 9964c133…429a`, `capsnode_evaluations: 111423`,
`claw_seconds: 0.37`. This is exactly "one authenticated object, two
semantics", obtained for ~2^16 work.

Derivation:

* `BadState`, `BadAll`, `MissRep`: no, as in (a) (raw/prehashed surface only;
  leaf-reachability Finding B sweep: no in-tree caller compares digests
  across domains; the `tower_lanes_to_flat_digest` bridge is `#[cfg(test)]`).
* `BadTypedBind`, cause side: **yes, twice over** — "a collision" *and*
  "domain confusion". The v2 wording matches verbatim: "namespace binding
  fails" — one commitment value (the authority) authenticates two different
  named objects in two namespaces, with both acceptances issued by
  production code.
* `BadTypedBind`, object side, worklist reading: **EXCLUDED-BY-CLAUSE**, same
  clause and same scope inference as (a): the claw never changes the graph
  the extractor walks, because that graph is anchored at leaf sponges whose
  capacity IVs (63+ tower bits) eject the state from every invariant
  subfield — their own design, accidentally, is what keeps our object out of
  their event.
* `BadTypedBind`, object side, dictionary reading: **INSTANTIATES** — and
  *a fortiori* under their paper's single-`v_node` dictionary, where the two
  claw rows are same-domain node rows sharing one anchor class (section 2):
  the reconstruction ambiguity is immediate, no cross-domain notion is even
  needed to break it.
* The additional, reading-independent finding: their formal model **cannot
  see the cross-tag structure at all** (one `v_node`; `c = 3` sponge
  capacities; no multi-domain dictionary defined anywhere), so the property
  their design comment claims ("distinct trees cannot share collisions") is
  a property about an object their security statement never models. The
  shipped domain separation is real code carrying a formalization that has
  no corresponding symbol.

### Candidate (c) — the GF(2^16) structure and Delta

Facts (leaf-reachability README + `results.txt`, bins 1 and 3): every round
constant and MDS multiplier lies in `K = GF(2^16)` (their own pinned
`AUDITED_INTERNAL_MATRIX` in `poseidon2b_cryptanalysis.rs` is
`[[0x20,1,1,1],[1,0x2000,1,1],[1,1,0x200,1],[1,1,1,0x800]]` — all in K);
`P(K^4) = K^4` exactly (4096/4096; restriction injective; a random
permutation preserves a 64-dim subspace with probability 2^-448); the 1-perm
tagged compressor whitens only the right input, so `left ∈ K², right ∈
IV_tag + K²` pins the state in K^4 — a 64→32-bit function with a 2^16
birthday; the restriction is the *same* function for every tag (the tag only
selects the right-input coset), so inter-domain separation on the aligned
coset is ~16 bits, not 128. Their cryptanalysis audit instantiates ePrint
2026/306 (Appendix A feed-forward, `d_I ≤ 7^73`, 409.87-bit projection),
2026/1792 (nonlinear subspaces, ≥ 1022.83 bits) and 2026/1905 (screening
only); **none of these models instantiates the subfield restriction**, which
is the structure that actually fires — so their published-attack
correspondence provides no Delta value along this channel either.

Delta derivation. By their definition (1.4),

```
Delta_P2b(R) ≥ max{0, Pr[BadCompiler]_fixed − ε_ideal(R)},
BadCompiler ⊇ BadTypedBind.
```

Dictionary reading of BadTypedBind: the adversary (classically, the
permutation is public — these are gate charges, not oracle queries) runs the
aligned-coset birthday until a colliding pair of node rows exists in its
query history; measured cost 72k-168k evaluations (~2^17.1 permutations ≈
2^41.4 gates in their own scalar price of 17,648,280 gates — inside the C1
envelope G = 2^130 at MAXDEPTH 2^40 with ~89 bits of margin, and inside
T = 2^64 sequential with 2^47 margin); repeat a handful of times so
Pr[collision recorded] → 1. Then Pr[BadTypedBind]_fixed = 1 (candidate (a)
suffices; (b) additionally instantiates the domain-confusion cause), so

```
Delta_P2b(2^64)  ≥ 1 − 0.187528937938435742 = 0.812471062061564258  > 0.312471062061564258  (eq. 1 refuted)
Delta_P2b^C1     ≥ 1 − 0.049330348228363684 = 0.950669651771636316  > 0.450669651771636316  (eq. 2 refuted)
```

i.e. **no valid bound below their own thresholds exists: the sufficient
conditions are unsatisfiable for the shipped permutation, and the
certificate's PASS premise ("fixed-Poseidon2b delta premises") is false
under this reading of their own definition.** The forcing route is the
collision *event* inside BadCompiler — it is not, and does not use, a qPRP
distinguishing advantage (their explicit disclaimer, section 1.4, is
respected: the distinguisher P(K^4)=K^4 w.p. 1 is what makes the event
cheap, but the Delta-relevant quantity is Pr[BadTypedBind]).

Worklist reading of BadTypedBind: Pr[BadCompiler]_fixed for this adversary
stays ~0 — the claw changes no represented accepting root, and every
BadState-relevant route through the subfield structure is dominated
(leaf-reachability cost table: aligned lifts 2^464 / 2^416 / 2^320 vs generic
2^128). Delta is then **not forced** by the measured structure; it remains
what it already was: a premise the certificate states, needs, and never
evaluates. The definition evades the structure by scope, not by bound.

---

## 4. Verdicts and licensed public claims

**Candidate (a) — same-tag node collision.**
EXCLUDED-BY-CLAUSE (worklist reading) / INSTANTIATES (dictionary reading).
Licensed claim (safe under both readings):

> "A node-hash collision in the shipped Poseidon2b compressor does not by
> itself win their game or fire their extractor-level bad events; whether it
> fires their typed-binding event depends on a scope their two documents
> state differently and never resolve — as a collision among database
> records it is exactly the event their ideal bound prices at 6T³/2^256,
> while as a change to the extractor's semantic graph it is excluded because
> every semantic consumer recomputes leaf hashes."

**Candidate (b) — cross-tag claw.**
EXCLUDED-BY-CLAUSE + UNMODELED-DOMAIN (worklist reading) / INSTANTIATES
(dictionary reading).
Licensed claim (safe under both readings, and the publishable core):

> "The certificate's typed-binding bad event does not cover the shipped
> object's actual commitment-domain structure: the paper's typed dictionary
> formalizes a single node domain (one v_node), the certificate says 'typed
> commitment domains' without defining a dictionary over them, and the
> cross-tag claw — one 32-byte commitment accepted by the production raw
> verifier in both the CAPSNODE and IVCPCSN_ domains for ~2^16 work — is
> either outside every bad event they define or, under the reading that makes
> their own ideal binding-collision term meaningful, a direct instantiation
> of 'a collision or domain confusion'."

**Candidate (c) — Delta.**
FORCES-DELTA under the dictionary reading; NOT-FORCED-but-UNEVALUATED under
the worklist reading.
Licensed claim (conditional form):

> "Under the reading of BadTypedBind as the fixed-game counterpart of the
> ideal model's global binding-collision event — the only reading under
> which that 6T³/2^256 term corresponds to anything — the measured GF(2^16)
> invariant-subfield structure forces Delta_P2b above both of the
> certificate's stated thresholds (≥ 0.95 vs < 0.4507 at Category 1; ≥ 0.81
> vs < 0.3125 at T = 2^64), so the certificate's PASS condition cannot hold
> for the shipped permutation; under the narrower extractor-worklist reading
> Delta is merely never evaluated — no text in the paper, certificate, or
> audit docs bounds it."

**Single strongest licensed claim** (for the final message):

> Their certificate's fixed-Poseidon2b premise is either false or vacuous
> for the shipped object: false, because the 2^16 cross-tag claw instantiates
> their own typed-binding bad event ("a collision or domain confusion") and
> forces Delta_P2b^C1 ≥ 0.95 against a required < 0.4507; or vacuous, because
> their security statement — whose paper formalizes a single node domain —
> defines no bad event that covers the shipped multi-tag commitment layer at
> all, leaving the exact property their design comment claims ("distinct
> trees cannot share collisions") outside their theorem in either case.

### What their theorem is therefore not about (the exclusion list)

If they adopt the worklist reading to keep the claw out, these are the
surfaces their definitions thereby exclude — each now documented with the
excluding clause:

1. **Raw-verifier acceptance.** `verify_source_batched_merkle_proof_to_cap`
   (and `hash_pair`) accept prehashed trees; no bad event looks at them
   (excluding clause: `BadAll`'s "represented accepting … root" + the
   worklist's leaf recomputation).
2. **Cross-domain commitment-byte identity.** One digest authenticating
   objects in two typed domains is not an event under the single-`v_node`
   dictionary; under plural domains no dictionary is defined (excluding
   clause: the absence itself, section 2).
3. **Subfield-aligned node collisions.** Two node rows, one anchor class —
   the H.1 guard does not exclude them; the ideal model prices them at
   6T³/2^256; the fixed model's price is 2^16 and no Delta bound exists
   (excluding clause: the walk-scope of "changes the typed semantic graph").
4. **The multi-tag node layer as such.** Symbols `v_node` (one) and `c = 3`
   (sponge capacities) do not describe production's tag set; the fixed
   experiment's "typed commitment domains" are carried only by the Delta
   premise.
5. **The Delta premise itself.** Stated, required by the PASS line, never
   evaluated anywhere in the corpus; their three instantiated
   attack-correspondence models (2026/306, 2026/1792, 2026/1905) do not
   model the subfield restriction that actually fires.

---

## 5. Sources

* Paper: `/Users/ember/paperbin/zenodo_23778742_signatureless_auth/`
  (PDF + text extract; LaTeX in
  `artifact/signatureless-authorization-artifact/manuscript/sections/`,
  notably `k-native-extraction.tex` [H.1], `f-merkle-simulation.tex` [F],
  `j-fresh-extraction.tex` [G]).
* Certificate: `/Users/ember/dev/parano1d-break/parano1d/noid_soundness/` —
  `README.md`, `docs/category-one.md`, `docs/v2-correspondence-2026-10.md`,
  `docs/october-2026-attack-review.md`, `src/{main,qrom,resource,local,
  parameters,poseidon2b_cryptanalysis}.rs`.
* PoC evidence: `/Users/ember/dev/parano1d-break/poc/src/bin/verify_collision.rs`
  (hardcoded witness); `/Users/ember/dev/parano1d-break/experiments/
  leaf-reachability/{README.md,results.txt}` (quoted lines in sections 3a-3c).
* No runs were executed in this lane; all quantitative statements trace to
  the quoted evidence lines of the prior lanes and to the certificate's own
  pinned arithmetic.
