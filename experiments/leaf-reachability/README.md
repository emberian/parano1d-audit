# Leaf reachability: why the CAPSNODE/IVCPCSN_ collision structure stops at the leaves

Escalation lane of the parano1d adversarial audit. Target: pinned read-only
checkout `52938433a30b185ab007afaa15166a632cf2e82e`. Everything below was
executed against the real production functions; raw outputs in `results.txt`,
code in `src/bin/`.

## TL;DR

1. **The cheap collisions are now fully explained algebraically.** Every
   round constant and MDS multiplier of the Poseidon2b permutation lives in
   the subfield `K = GF(2^16)` (max tower encoding 14 bits, from
   `MDS_PARTIAL`'s `0x2000`), so `K^4` is invariant under
   `permute_flat_u128` and the restriction is exactly "the same Poseidon2
   schedule over GF(2^16)". The production 1-perm node compressor whitens
   only the *right* input with the tag IV, so `left in K^2, right in
   IV + K^2` pins the whole state in `K^4`: a 64-bit -> 32-bit function.
   Collisions are a `2^16` birthday — which is what the swarm observed
   (72,274 / 111,525 evaluations). `GF(2^8)^4` and `GF(2^4)^4` are **not**
   invariant; GF(2^16) is the smallest invariant subfield.
2. **New object — a cross-tag claw (mission B).** The tag only selects
   *which right-input coset* maps into `K^4`; the restricted permutation is
   the same for every tag. A single digest value is simultaneously a valid
   `TAG_CAPSNODE` node digest and a valid `IVCPCSN_` node digest
   (production `noid_ivc_core::merkle::hash_pair`), with shared left and
   distinct rights, found in **111,423 evaluations / 0.49 s**. Two 2-leaf
   prehashed trees — one per domain — share the **identical 32-byte
   commitment**, each accepted by the production raw
   `verify_source_batched_merkle_proof_to_cap` under its own node hasher.
   This refutes the design comment at
   `noid_poseidon2b/src/native/compression.rs:319-325`: *"the
   caller-selected tag whitens the right-input lanes so distinct trees
   cannot share collisions."* Distinct trees (two tag domains) share a
   collision at `2^16` cost; the domain separation is worth ~16 bits on the
   aligned coset, not 128.
3. **The escalation is blocked, provably (mission C).** The lift to valid
   CAPSMIX/CAPSLEAF/CAPS256/IVCPCSL leaf digests requires a leaf digest to
   land in the aligned coset, and the leaf sponges' capacity IVs (ASCII
   labels, 63+ tower bits) sit outside every invariant subfield. Theorem:
   absorbs touch lanes 0,1 only, `P(K^4) = K^4`, `P` bijective, IV
   `not in K^2` at step 0 — so the sponge state **never** enters `K^4`;
   verified: 65,536 fully-aligned leaves -> 0 aligned digests, 0
   intermediate states with capacity in `K^2`, simulation byte-identical
   to the real `capsule_leaf_hash_mixed`.
4. **Economics inversion (stronger than the previous "2^-224" estimate).**
   Routing the lift through any invariant subfield `GF(2^d)` costs
   `2^(4(128-d)+d)` operations (per aligned adjacent leaf pair
   `2^(4(128-d))`, birthday `2^d` pairs): `2^464` (d=16), `2^416` (d=32),
   `2^320` (d=64) — **all strictly dominated by the generic `2^128`
   whole-node birthday** between two honest commitments. The subfield
   structure therefore cannot beat generic search below the leaf layer,
   not even in principle along these routes. The semantic layer's
   effective collision resistance is the generic `2^128` — the full digest
   length — even though the node layer offers only `2^16`.

## Finding A — the mechanism (mission A)

**Why the collisions are cheap.**
`noid_poseidon2b/src/native/permutation.rs:185-206` pins the constants:
`ROUND_CONSTANTS` and `MDS_FULL` entries are tower encodings in `[0,7]`
(3 bits); `MDS_PARTIAL` entries are `{1, 0x20, 0x200, 0x800, 0x2000}`
(max 14 bits). In the nested tower basis, tower encodings `< 2^16` are
exactly the subfield `K = GF(2^16)`. Field operations (add, multiply by a
constant in K, and the x^7 S-box, since K is a field) preserve K, hence
`P(K^4) = K^4` — the permutation runs as the *same Poseidon2 over
GF(2^16)* on that slice (verified: 4096/4096 aligned states stay aligned;
restriction injective). A random permutation maps a 64-dim subspace into
itself with probability `2^-448`; production does it with probability 1.

`compress_flat_feed_forward_with_tag`
(`noid_poseidon2b/src/native/compression.rs:327`) forms state
`[a0, a1, b0^IV0, b1^IV1]` — the tag IV is XORed into attacker-supplied
lanes, so choosing `right = k ^ IV` with `k in K^2` cancels the whitening
and puts the state in `K^4`. Both production node domains are affected:
`TAG_CAPSNODE` (capsule trees, `noid_fri_binius/src/capsule.rs:180-202`)
and `IVCPCSN_` (proof-core PCS trees, `noid_ivc_core/src/merkle.rs:42,125-129`;
recursive twin `noid_recursive/src/acceptance/trace/self_verify.rs:49`).

The root-verified collision pair is exactly of this form (checked here):
left lanes in `K^2`, right lanes in `IV + K^2`, digest in `K^2`. A fresh
zero-left collision regenerated live: 167,686 evaluations / 0.29 s (two
birthdays race at this size: digest space and input space are both 32
bits).

**Is the structure linear/affine — can collisions be *solved* for?** No.
Over K the S-box `x -> x^7` is not K-additive (`(x+y)^7 != x^7 + y^7` in
characteristic 2), so the restricted compressor is not affine and its
collision set is not a coset of a kernel; collisions are *found* by the
`2^16` birthday, not solved. (Their own nonlinear-subspace audit,
`noid_soundness/src/poseidon2b_cryptanalysis.rs:290+`, models ePrint
2026/1792 trails and Macaulay projections at `>= 2^1022` bits, and its
`even_balancing_core = 0xbe32` (line 622) is itself an element of K — but
no model in that file instantiates the subfield *restriction*, which is
the structure that actually fires.)

**The guardrail asymmetry, demonstrated.** The two-permutation sponge
compressor `compress_with_tag` (`compression.rs:244`, used for `EXSTNOD`
state trees) loads the tag IV into the capacity lanes *before* the first
permutation. Since the IV is not in `K^2` and `P(K^4) = K^4`, the state can
never be placed in `K^4` from the data lanes: 4096/4096 trials with both
inputs tower-aligned give **0** aligned digests (and on the aligned slice
the attack set is heuristically empty, `2^-192`). Same constants, same
permutation — the *schedule* is the difference: one permutation with
data-XORed whitening exposes the subfield structure; two permutations with
a capacity-seated IV do not. The capsule node design chose the 1-perm
schedule explicitly for cost ("One permutation per node — half the
sponge-mode compress cost", `compression.rs:319-325`).

## Finding B — cross-domain reinterpretation (mission B)

Sweep result: exactly **two** production tags use the 1-perm compressor
(`CAPSNODE`, `IVCPCSN_`); every semantic consumer recomputes leaf hashes
(`noid_fri_binius/src/capsule.rs:805`, `zk_capsule_pcs.rs:1000,1018`,
recursive twins `self_verify.rs:195-207`, `r_pcs_region.rs:586-593`), and
no in-tree caller compares digests across domains (the
`tower_lanes_to_flat_digest` bridge is `#[cfg(test)]`,
`noid_gkr/src/zk_auth_qrom.rs:1999-2004`).

The one observable cross-domain object is the claw above: identical
commitment bytes in the two tag domains with different preimages, `2^16`
work, both sides opened through the production raw verifier. Its reach is
the same as the same-tag collision — raw/prehashed consumers only — but it
adds the *typed-binding* dimension: the domain separation itself (not just
collision resistance within a domain) degrades to 16 bits on the aligned
coset. If their `BadTypedBind`/`Delta_P2b` formalization is ever evaluated
concretely, this is an instantiable input: two domain-typed authentication
objects with equal commitment bytes and disjoint preimage semantics.

## Finding C — the blocker statement (mission C)

> **The property that prevents the lift:** every path from the `2^16` node
> collisions to a full capsule/PCS collision must pass through a *leaf*
> digest that lies in an invariant-subfield coset, and the leaf sponges'
> capacity-IV initialization (`Poseidon2bFlatSponge::with_tag`,
> `capacity_iv_flat` of ASCII labels) places the sponge state provably
> outside every invariant subfield coset for the entire absorption
> schedule. Conditioning on alignment costs `2^(4(128-d))` per adjacent
> leaf pair — strictly more than the generic `2^128` node birthday for all
> `d in {16,32,64}` — so the subfield structure is *dominated*, not merely
> unlifted. The semantic layer keeps the full `2^128` generic collision
> resistance of a 256-bit digest.

**Accidental or principled?** The mechanism saving the leaves —
capacity-IV initialization that randomizes the state out of the constants'
invariant subfield — is the textbook-principled defense. Its *placement*,
however, is accidental: the node layer uses the same family of tags but
wires the IV into attacker-cancellable data lanes for the 1-perm schedule,
and the design comment claims a protection ("distinct trees cannot share
collisions") that the cross-tag claw disproves. Nothing in the shipped
design acknowledges that (a) all constants live in GF(2^16), (b) the leaf
layers owe their security to the IV ejection effect, or (c) the node
layers lack it. The fix the Poseidon2b paper itself adopted (large
Turbo-SHAKE-derived round constants; the pinned constants here match the
*legacy* Binius reference, pinned by hash in
`poseidon2b_cryptanalysis.rs:554-572`) would remove the invariant subfield
entirely and make the asymmetry moot.

## Severity

- **Confirmed, new:** cross-tag claw + two-domain shared commitment
  through the production raw verify API (`2^16`, refutes a shipped design
  claim; typed-binding degradation). Primitive-level, raw-API-reach only.
- **Confirmed, characterized:** the `2^16` node-collision mechanism
  (subfield restriction; smallest invariant subfield GF(2^16)); fresh
  collisions regenerated in < 0.3 s.
- **Negative result (upgraded to a proof):** no algebraic lift to valid
  leaf preimages exists along the subfield structure — it is dominated by
  generic search. This closes finish line (a) of the previous swarm
  ("valid CAPSMIX preimages for the colliding children") *against the
  attacker*: more computing on aligned node collisions can never pay.
- **Not claimed:** no fresh-request forgery, no full-capsule collision, no
  semantic verifier accepts any object here. Finish lines (b) (a
  caller/SDK/bridge that authenticates attacker child digests without
  recomputation) and (c) remain open by construction, not by algebra.

## Reproduction

```
cd /Users/ember/dev/parano1d-break/experiments/leaf-reachability
CARGO_TARGET_DIR="$PWD/target" cargo run --release --bin subfield_invariance
CARGO_TARGET_DIR="$PWD/target" cargo run --release --bin cross_tag_claw
CARGO_TARGET_DIR="$PWD/target" cargo run --release --bin leaf_wall
```

All three print a single JSON object (see `results.txt`). The claw asserts
its own verification through the real `compress_flat_feed_forward_with_tag`,
the real `noid_ivc_core::merkle::hash_pair`, and the production
`verify_source_batched_merkle_proof_to_cap` under both domains. The leaf
wall asserts its re-simulation is byte-identical to the real
`capsule_leaf_hash_mixed` on all 65,536 aligned leaves. Runtime: ~5 s total
on this machine. The hardcoded cross-tag witness is printed in the JSON
(`shared_digest_hex`, both rights, shared left).

## Assurance limits

- The invariance of `K^4` is verified empirically (4096 states) plus
  argued from the constants (all in K; closure under field operations) —
  the argument is exact, the code checks corroborate it.
- "Dominated by generic `2^128`" covers the aligned-lift routes through
  subfield towers (d = 16/32/64) with pair-conditioning accounting; it is
  an argument about these structures, not a lower bound against all
  conceivable attacks (e.g. an unrelated flaw in the affine LCH code or
  the fold algebra is out of scope here — other lanes covered those).
- The claw's reach was swept over in-tree callers only; out-of-tree SDKs,
  bridges, or future callers consuming the public raw
  `verify_source_batched_merkle_proof_to_cap`/`hash_pair` APIs are outside
  what this checkout can see.
- `2^16` figures are expectation-scale birthdays; individual runs vary by
  small constants (observed: 111k-168k evaluations).

## Single most valuable follow-up

The algebra is now closed in both directions; the only live finish line is
**(b): a consumer that authenticates attacker-supplied child digests
without recomputing descendants.** Concretely: audit every *external*
consumer of the public `verify_source_batched_merkle_proof_to_cap`,
`noid_ivc_core::merkle::hash_pair`, and the serialized `MerkleCap` type
(SDKs, bridges, indexes/caches keyed by cap bytes — where the identical
cross-domain commitment bytes become one cache entry for two semantics).
If none exists, the rebuttal's final position is exactly this lane's
blocker statement: a principled leaf wall, an accidental node breach, and
a shipped comment that promises the wrong thing.
