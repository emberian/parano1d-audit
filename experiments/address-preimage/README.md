# ADDRFIX address-preimage experiments

Scope: Parano1d commit `52938433a30b185ab007afaa15166a632cf2e82e`, synthetic keys and addresses only. No live targets and no shared-source modifications.

## Construction and generic cost

`derive_address` absorbs exactly one aligned rate block and returns the first two lanes after one Poseidon2b permutation:

```text
A(s0,s1) = Tr_2(P([s0,s1,ADDRFIX_IV0,ADDRFIX_IV1]))
```

Relevant implementation points:

- `noid_poseidon2b/src/primitives.rs:298-305`: public ADDRFIX derivation.
- `noid_poseidon2b/src/native/compression.rs:58-76,114-123,132-145`: aligned absorb and no-pad squeeze.
- `noid_poseidon2b/src/native/permutation.rs:17-21,68-89`: 8 full + 58 partial rounds.
- `noid_poseidon2b/src/native/permutation.rs:184-206`: small constants and matrices.
- `noid_poseidon2b/src/native/domain.rs:33-39,56`: fixed ADDRFIX IV.
- `noid_gkr/src/zk_auth_qrom.rs:92-98,110-133`: 128-bit post-quantum hash-preimage premise in the soundness ledger.

For unrestricted 256-bit secrets and a 256-bit address target, generic classical preimage search is `2^256` evaluations and ideal Grover search is `2^128` oracle queries. The experiments do not establish a cheaper uniform-key attack, so they do not contradict the ledger's 128-bit post-quantum premise. This is not a proof that no such attack exists.

## Full-round results over the real GF(2^128)

### Subfield closure is blocked by the IV

The round constants lie in `GF(2^8)` and all MDS coefficients lie in `GF(2^16)`. In contrast, `ADDRFIX_IV0` has minimal subfield `GF(2^128)` and `ADDRFIX_IV1` has minimal subfield `GF(2^64)`.

For any field `K` over which the permutation is defined, `K^4` is invariant under both the permutation and its inverse. But the initial state can belong to `K^4` only if both IV lanes belong to `K`. Since the IV requires the full field, no proper lane-wise subfield state closure applies to ADDRFIX. Small-subfield secrets alone do not imply small-subfield addresses: 0 of 65,536 addresses in the full-round 8-bit-lane weak-key run had both output lanes in `GF(2^8)`.

This rules out the direct `K^4` invariant, not every conceivable nonlinear or lower-dimensional invariant.

### Frobenius gives no ADDRFIX equivalent-key orbit

The full permutation satisfies

```text
P(sigma^k(x)) = sigma^k(P(x))  when 16 divides k.
```

The experiment verifies 48 full-state relations at `k = 16, 32, 64`, and checks every round prefix as well. For the address construction with secret-only conjugation,

```text
A(sigma^k(s)) = Tr_2(P([sigma^k(s0),sigma^k(s1),IV]))
sigma^k(A(s)) = Tr_2(P([sigma^k(s0),sigma^k(s1),sigma^k(IV)])).
```

A direct orbit relation therefore requires `sigma^k(IV) = IV`. Exact stabilizer tests show that no nontrivial Frobenius automorphism fixes both IV lanes. Thus conjugation does not identify equivalent ADDRFIX keys or reduce the search space by an orbit factor. A synthetic 112-relation sample found neither equivalent conjugate keys nor conjugated-address relations; the stabilizer argument, rather than that sample, is the blocker.

### Inverting the permutation does not beat generic search

The inverse S-box exponent is `0x49249249249249249249249249249249`, the inverse of 7 modulo `2^128-1`. The harness verifies 16 full-round inverse round trips against states whose native and independent forward evaluations match.

For a target `(a0,a1)`, guessing the two hidden output lanes `(h0,h1)` and inverting gives a candidate initial state. It is an ADDRFIX preimage only if its capacity lanes equal the fixed IV. There are `2^256` hidden-lane guesses and a random-model success probability of `2^-256` per guess, the same candidate count as forward secret guessing. One real-field trial inverted successfully to a state with the chosen public output but, as expected, did not match the IV.

### Exact weak-key recovery, but only a low-entropy class

For keys where each lane is known to be a raw 8-bit value, exhaustive native full-round recovery found the unique target key `s0=0xa5, s1=0x5a` in all `65,536 = 2^16` evaluations. The public `derive_address` API is checked against the direct permutation target. All 65,536 class addresses were distinct.

This class has probability `2^-240` under a uniform 256-bit secret. Its worst-case classical search is `2^16`, and ideal Grover search would be `2^8`; these are entropy weaknesses, not a faster attack on uniform keys. More generally, a promised `GF(2^d)` pair has `2^(2d)` candidates, but the fixed IV prevents a corresponding output-subfield shortcut.

No unrestricted full-round key recovery was found or demonstrated.

## Explicitly reduced-field control

The `reduced-field` command replaces `GF(2^128)` with `GF(2^8)` and uses a synthetic IV, while retaining all 66 rounds. It is **not** a full-field Parano1d attack and is not reduced-round.

With 16-bit secrets and addresses, forward and inverse attacks both enumerate exactly `2^16` candidates and return the same two target preimages, `0x5f9e` and `0xc237`. The full class has 41,466 distinct addresses and 32,602 colliding key pairs. This illustrates that equivalent keys can occur from a same-size truncated map without yielding a cheaper target-preimage algorithm. It provides no evidence of full-field key recovery.

## Run

From this directory:

```bash
export CARGO_TARGET_DIR="$PWD/target"
cargo check
cargo run --release -- structure
cargo run --release -- weak-key
cargo run --release -- reduced-field
cargo run --release -- all
```

`results.txt` records the final `all` run on this host. `src/field128.rs` contains the real-field structural, inverse, and weak-key experiments. `src/gf8.rs` contains the explicitly reduced-field control. All targets, secrets, and IVs used for recovery are synthetic constants in the code.
