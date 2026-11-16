# Leaf reachability of Poseidon2b node collisions

The one-permutation node compressor admits collisions after roughly `2^16`
evaluations. This experiment explains that cost, constructs a collision across
two production node tags, and checks whether the same construction reaches
leaves produced by the capsule hash. The code and raw outputs are in
`src/bin/` and `results.txt`. The tested checkout is
`52938433a30b185ab007afaa15166a632cf2e82e`.

## The invariant subfield

Let `K = GF(2^16)` in the field's tower representation. The round constants
and MDS multipliers used by `permute_flat_u128` lie in `K`: the full matrix
and round constants have encodings at most `7`, and the partial matrix uses
`{1, 0x20, 0x200, 0x800, 0x2000}`
(`noid_poseidon2b/src/native/permutation.rs:185-206`). Addition,
multiplication by these constants, and the `x^7` S-box preserve `K`. Thus
the permutation restricts to a permutation of `K^4`, a 64-bit state space.
The harness checked 4,096 aligned inputs; all 4,096 outputs were aligned.
It also checked that the `GF(2^8)` and `GF(2^4)` slices are not invariant.

The production `compress_flat_feed_forward_with_tag` initializes its state
as `[a0, a1, b0 ^ IV0, b1 ^ IV1]`
(`noid_poseidon2b/src/native/compression.rs:327`). An attacker can choose
`a in K^2` and `b in IV_tag + K^2`, cancelling the tag IV before the
permutation. The resulting node hash maps aligned inputs to a 32-bit
digest. A birthday search therefore needs roughly `2^16` evaluations.
Earlier production searches took 72,274 and 111,525 evaluations
(`../FINDINGS.md`, C1–C2); a fresh zero-left search in this experiment
took 167,686. These are measured runs, not a fixed cost
for every search. The restricted compressor remains nonlinear; the
experiment finds collisions by search and does not solve them as a linear
system.

This applies to the one-permutation `TAG_CAPSNODE` and `IVCPCSN_` node
hashers (`noid_fri_binius/src/capsule.rs:180-202`;
`noid_ivc_core/src/merkle.rs:42,125-129`). The two-permutation
`compress_with_tag`, used for `EXSTNOD`, places the IV in capacity before
the first permutation (`compression.rs:244`). Its tag cannot be cancelled
by choosing data lanes. In 4,096 aligned-input trials, none produced an
aligned digest. That observation does not establish collision resistance
for the two-permutation construction.

## Cross-tag claw

The restriction of the permutation to `K^4` is the same under both
one-permutation tags. Only the attacker-selected right-input coset changes.
`cross_tag_claw` found a common digest after 111,423 evaluations (0.37 s
in the recorded run), with different right children for the two tags.
Two prehashed two-leaf trees consequently have identical 32-byte roots.
The production `verify_source_batched_merkle_proof_to_cap` accepted each
tree under its respective hasher. The witness and both acceptance results
are recorded in `results.txt`.

This contradicts the design comment in
`noid_poseidon2b/src/native/compression.rs:319-325`, which says that
tag whitening prevents distinct trees from sharing collisions. It
demonstrates cross-domain digest equality at the raw, prehashed Merkle
interface. It does not demonstrate that a semantic capsule, PCS proof,
or terminal state accepts either tree.

## Leaf boundary

The semantic consumers inspected here recompute their leaf digests
(`noid_fri_binius/src/capsule.rs:805`, `zk_capsule_pcs.rs:1000,1018`,
and recursive counterparts `self_verify.rs:195-207`,
`r_pcs_region.rs:586-593`). Their CAPSMIX/CAPSLEAF/CAPS256/IVCPCSL
sponges start with capacity IVs outside `K^2`. The absorption steps touch
the rate lanes, so the initial state cannot be put in `K^4` by selecting
the leaf data. Bijectivity also puts the first permutation output outside
`K^4`. Later absorption could change the rate lanes; the experiment below
checks whether the capacity lanes ever permit re-entry. Thus the simple
node-input alignment does not carry through the leaf sponge.

`leaf_wall` tested 65,536 aligned leaf inputs. It found no aligned leaf
digest or intermediate state with capacity in `K^2`, and its simulated
leaf outputs matched production `capsule_leaf_hash_mixed` byte for byte.
This is a sampled negative result, not a proof that every later capacity
state stays outside `K^2` or that the leaf hash is collision resistant.

For the particular strategy of forcing an adjacent leaf pair into a
`GF(2^d)`-aligned node input, the estimated conditioning cost is
`2^(4(128-d))` per pair, followed by about `2^d` pairs for a birthday
collision. The total is `2^(4(128-d)+d)`: `2^464`, `2^416`, and `2^320`
for `d = 16, 32, 64`, respectively. Each exceeds the generic `2^128`
birthday estimate for a 256-bit digest. This cost analysis rules out an
advantage for **these aligned-lift strategies**; it is not a lower bound
against unrelated attacks or other ways to reach a semantic object.

The caller sweep found no in-tree comparison of node digests across these
two domains. The `tower_lanes_to_flat_digest` bridge in
`noid_gkr/src/zk_auth_qrom.rs:1999-2004` is test-only. External consumers
of the public raw verifier or cached cap bytes were outside the sweep.

## Reproduce

```sh
cd /Users/ember/dev/parano1d-break/experiments/leaf-reachability
CARGO_TARGET_DIR="$PWD/target" cargo run --release --bin subfield_invariance
CARGO_TARGET_DIR="$PWD/target" cargo run --release --bin cross_tag_claw
CARGO_TARGET_DIR="$PWD/target" cargo run --release --bin leaf_wall
```

Each binary prints one JSON object; `results.txt` contains the recorded
outputs. The claw checks both production node hashers and the raw verifier.
The leaf test checks its simulation against the production leaf hash.
