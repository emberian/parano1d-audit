# What the typed-binding claim covers

This is a reading of the paper and certificate against three previously
verified objects: a `TAG_CAPSNODE` node collision, a `TAG_CAPSNODE` /
`IVCPCSN_` cross-tag claw, and the `GF(2^16)` invariant subfield that makes
both cheap. No new attack was run for this report. The evidence and commands
for the objects are in `../leaf-reachability/{README.md,results.txt}` and
`../../poc/src/bin/verify_collision.rs`.

The distinction is between **a collision in the raw node hash** and **a
failure of the terminal-state security game**. The former is established.
The latter has not been demonstrated. Whether the former counts as the
certificate's `BadTypedBind` event depends on what “typed semantic graph”
includes.

| Candidate | Extractor-worklist reading | Measured-dictionary reading |
| --- | --- | --- |
| Same-tag node collision | No represented accepting root changes | Two node rows give one anchor more than one pair of children |
| Cross-tag claw | No represented accepting root changes; the second node domain is not modeled explicitly | Same ambiguity; also a failure of cross-domain binding if equal bytes name both objects |
| `GF(2^16)` route to `Delta_P2b` | Does not determine `Delta_P2b` | Gives a large `BadTypedBind` probability, conditional on this event including raw node rows |

## Certificate statements

The security game in `noid_soundness/docs/category-one.md` starts from a
terminal `HistoryStep` State. The adversary wins when the production
verifier accepts its proof although no valid execution from genesis ends
there. None of the three objects above is such a proof.

The same document defines `BadAll(D)` for a *represented accepting* wallet
or History root whose deterministic extractor fails or returns an invalid
local witness. It separately names `MissRep` (a required noncertified child
is absent) and `BadTypedBind` (“a collision, ambiguous encoding or domain
confusion changes the typed semantic graph”). It states
`BadState ⊆ BadAll ∪ MissRep ∪ BadTypedBind`. The v2 correspondence note,
`docs/v2-correspondence-2026-10.md`, describes `BadTypedBind` as
“object/statement/namespace binding fails” and says that equal digests
imply equal named objects only under the certificate's binding game. The
documents do not say unambiguously whether raw node rows outside an
accepting extractor worklist belong to that game.

The paper's Appendix H.1 (`manuscript/sections/k-native-extraction.tex`,
lines 27–78) defines a finite dictionary from a measured database `D`.
Each free permutation record supplies a node anchor and children; the
reconstruction lemma says that a cap and index determine at most one
payload. At exponent zero, its proof invokes “the unique node row”. The
stated guard excludes certain capacity-edge and anchor-orbit conflicts;
it does not explicitly exclude two distinct node rows with the same
anchor. The ideal analysis instead charges a separate global binding
collision term, `6T^3/2^256` (`docs/category-one.md`, eq. 11;
`src/qrom.rs`, `binding_collision_term`). This is the textual basis for
counting node-anchor collisions in the measured dictionary.

The paper's node hash has a single `v_node` (`k-native-extraction.tex`,
eq. 52); its three sponge capacities are `CAPSMIX_`, `CAPS256_`, and
`KSCH256_`. Appendix F's simulator also uses one `v_node`
(`f-merkle-simulation.tex:83`). Production has at least two distinct
one-permutation node domains, `TAG_CAPSNODE` and `IVCPCSN_`, plus the
two-permutation `EXSTNOD` path and additional leaf sponges. The
certificate mentions “typed commitment domains” in its fixed experiment,
but the inspected paper and certificate do not define a multi-node-domain
dictionary corresponding to these production tags. Under the paper's
single-node dictionary, the cross-tag claw is represented, if at all, as
two node rows with one digest; its cross-domain character is lost.

## What the experiments establish

The same-tag witness has distinct child pairs and one production
`TAG_CAPSNODE` digest. The hardcoded witness is checked by
`poc/src/bin/verify_collision.rs`. The earlier 72,274 and 111,525
evaluation counts are in `../FINDINGS.md` (C1–C2). The subfield checks
and a fresh 167,686-evaluation search are in
`../leaf-reachability/results.txt`: 4,096 of 4,096 aligned permutation
states stayed aligned.

The cross-tag witness has one 32-byte root for two different prehashed
two-leaf trees. Production
`verify_source_batched_merkle_proof_to_cap` accepts each tree under its
respective hasher. The recorded claw used 111,423 evaluations and its
JSON contains both accepted results, distinct leaves, and identical root
bytes. This refutes the claim in
`noid_poseidon2b/src/native/compression.rs:319-325` that tag whitening
prevents distinct trees from sharing collisions. The witness uses a raw
Merkle interface; the report does not establish acceptance by a capsule,
PCS proof, wallet, or terminal State. The in-tree semantic consumers
inspected in `../leaf-reachability/README.md` recompute leaf hashes.

The mechanism is the public permutation's restriction to `K^4` for
`K = GF(2^16)`. The production one-permutation compressor XORs the tag
IV into attacker-controlled right input lanes. Choosing the right input
in `IV_tag + K^2` places the whole state in `K^4`, reducing the digest
space on that slice to 32 bits. Both tags select cosets of the same
restricted function. This is a collision event, not merely a
distinguishing test against a random permutation. The certificate itself
says that `Delta_P2b` is not a qPRP distinguishing advantage
(`docs/category-one.md`, “Fixed Poseidon2b boundary”).

## Consequences for `BadTypedBind`

On a **worklist reading**, “changes the typed semantic graph” refers to
the graph traversed from a represented accepting root. Neither witness
is on such a path. `BadAll` and `MissRep` also do not occur. On that
reading, the experiments say nothing quantitative about `Delta_P2b`.
This interpretation follows the extractor's stated starting point; it
is an interpretation of the underspecified `BadTypedBind` clause, not an
explicit exclusion written into that clause.

On a **dictionary reading**, the graph is the finite dictionary over the
measured database `D` in Appendix H.1. Two records with the same node
anchor and different children make “the unique node row” unavailable.
The same-tag witness is a direct example. The cross-tag witness gives
the same ambiguity when represented by the paper's single `v_node`; in
production it also supplies equal commitment bytes for two different
tag domains. This reading connects `BadTypedBind` to the ideal analysis's
global binding-collision term. It remains conditional on a formal account
of how the fixed game's typed domains and database records are assembled.

The certificate defines

```text
Delta_P2b(R) >= max(0, Pr[BadCompiler]_fixed - epsilon_ideal(R))
BadCompiler = BadAll union MissRep union BadTypedBind
```

(`docs/category-one.md`, eq. 37 and “Fixed Poseidon2b boundary”). Its
published sufficient conditions require
`Delta_P2b(2^64) < 0.312471062061564258` and
`Delta_P2b^C1 < 0.450669651771636316`. The corresponding ideal terms
are `0.187528937938435742` and `0.049330348228363684`
(`noid_soundness/README.md:35-41`; `docs/category-one.md`, eqs. 1–2;
`src/{qrom,resource}.rs`).

**If** both witnessed raw node rows can be recorded in `D`, and that
recording is itself `BadTypedBind`, the fixed event is certain for this
witness. The definition then requires

```text
Delta_P2b(2^64) >= 1 - 0.187528937938435742 = 0.812471062061564258
Delta_P2b^C1    >= 1 - 0.049330348228363684 = 0.950669651771636316
```

Both exceed the sufficient-condition thresholds. Even a success
probability above `0.5` suffices to exceed either threshold. The recorded
search is classical, about `2^17` permutation evaluations. The
certificate's own scalar gate estimate is 17,648,280 per permutation,
yielding roughly `2^41.4` gates for one recorded search, within the
cited C1 envelope
`G = 2^130`, `MAXDEPTH = 2^40`. This calculation is a conditional
consequence of the dictionary reading, not a measured `Delta_P2b` or a
terminal-state forgery. On the worklist reading, it does not apply.

The certificate prints “PASS under the typed parallel-QROM, coherent
response-cost, and fixed-Poseidon2b delta premises”
(`noid_soundness/src/main.rs:517`). Its cryptanalysis module says the
production-QROM delta remains an end-to-end premise
(`src/poseidon2b_cryptanalysis.rs`). No value of `Delta_P2b` is computed
in the inspected artifact. The module's published-attack models do not
instantiate the `GF(2^16)` restriction; that is a gap in the evidence for
the premise, regardless of which `BadTypedBind` scope is intended.

## Scope of the result

The established production issue is cheap equality of node commitments,
including across two tags, at a raw verifier that accepts prehashed leaves.
The formal issue is that the paper models one node domain while the
certificate names multiple typed domains without specifying the
corresponding dictionary or whether raw node collisions count as
`BadTypedBind`. Under the broad dictionary reading, the cheap collision
conflicts with the published `Delta_P2b` thresholds. Under the narrow
worklist reading, these witnesses do not evaluate that premise. Neither
reading turns the witnesses into an accepted invalid terminal State.

Sources: Zenodo 23778742 paper and LaTeX under
`/Users/ember/paperbin/zenodo_23778742_signatureless_auth/`;
certificate checkout `parano1d/noid_soundness/` at
`52938433a30b185ab007afaa15166a632cf2e82e`; production witness
`poc/src/bin/verify_collision.rs`; experimental evidence
`experiments/leaf-reachability/{README.md,results.txt}`.
