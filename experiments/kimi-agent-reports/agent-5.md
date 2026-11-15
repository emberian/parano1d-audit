# agent-5: Attack BaseFold commitments

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Implemented and ran the PoC crate at:

`/Users/ember/dev/parano1d-break/experiments/basefold-chains`

## Result

The `IVCPCSN_` compressor is cheaply broken, including through the raw Merkle and recursive path-replay APIs. However, I did **not** obtain a valid collision between two real byte/codeword trees or an accepted alternate BaseFold opening. The fixed-IV leaf hash currently blocks lifting the small-subfield node collision into production codewords.

### Confirmed attacks

- Found an `IVCPCSN_` node collision in **111,525 evaluations / 0.205 seconds**, using production `merkle::hash_pair`.
- Effective collision search is about `2^16`: GF(2^16) inputs and all permutation constants remain in that subfield, reducing the two-lane digest to 32 bits.
- Collision:
  ```text
  digest = f1ca994e8a1b9a79e5da33ad7015b44940e5b5d95739c87a3ee5ba1b6b32c8b6

  L1 = 49d9e476d1be3b6cf72a377d0e2891971c476390618849e332e39f5e472ce84c
  R1 = 6dcfb2eb798ef1cedc9820823713aebf81feede11d04c3ab22b754b6b3b71655

  L2 = 70189405afb2605c0ee689152fc6a3bbfe23f9166595bb4fb2cedd133cf174e2
  R2 = 11f2e27b5b382faa046a9df6ba7e6eabae7d63fc8eeb316fc9650faf2bb8b2af
  ```
- Both distinct openings verify through production `verify_merkle_proof`.
- Equivalent Ligerito-style openings verify through `verify_merkle_multi_proof`.
- Both witnesses produce the same root through production recursive `build_ff_merkle_path_columns`.

I also implemented and validated the inverse permutation. It yields a left fixed point `H(a,b)=a`:

```text
a = 63460cf8dec8c05d45cb0769dc52ea105a37f73f4e569ba4495f5f8242e823a3
b = 0097f20e1ec97ac1d17ac29cc846e30de6e065addc123e3b1754209caec691bb
```

The raw API accepts the same root with path lengths 0, 1, and 2; the recursive builder preserves the entry through a depth-3 path.

### Leaf/internal type confusion

For 32-byte fixed leaves, with `R = IV_IVCPCSN XOR IV_IVCPCSF`, the exact production relation is:

```text
hash_pair(A, R) = hash_leaf(A) XOR A
```

Thus at `A = 0`, the leaf and internal-node digests are identical. This is reproduced in the PoC, but it does not provide a real leaf preimage or full-tree collision.

### Real BaseFold tests

Using production commit/prove/verify functions, with 156 transcript-derived queries:

- Honest C0 and C1 proofs verify with transcript lockstep.
- Shortened and fixed-point-extended paths: `InvalidProofShape`.
- Collision bytes substituted as an actual 32-byte initial leaf: `InitialMerkleFailed` in both C0 and C1.
- Reordered queries: `FoldMismatch`.
- Mutated post-row-batch root: rejected.
- Collision inputs used as real leaf bytes produce different tree roots, neither equal to the node-collision digest.

Relevant enforcement is at `noid_ivc_core/src/pcs/basefold.rs:1337`, `noid_ivc_core/src/pcs/basefold.rs:1351`, and C1 equivalents at `noid_ivc_core/src/pcs/basefold.rs:1984`. Ligerito likewise recomputes `hash_leaf` at `noid_ivc_core/src/pcs/ligerito.rs:4633`.

## Severity and blocker

- **Primitive/raw-digest APIs: high severity.** Collision resistance is effectively broken at roughly 16-bit birthday cost wherever attackers can supply child digests directly.
- **Actual BaseFold/Ligerito codeword commitments: no end-to-end break demonstrated.** The required next step is a real leaf preimage for a colliding child digest, or a collision/chosen-prefix attack involving actual leaf hashes.
- The fixed-leaf IV lies outside GF(2^64), so small codeword symbols do **not** keep leaf digests in GF(2^16). Under a random-output model, one digest having both lanes in GF(2^16) has probability `2^-224`; matching a prescribed collision input is harder still.
- Path-length pinning and transcript/query ordering held in all production C0/C1 tests.
- Recursive testing used the production native path-column builder, not a complete recursive proof-generation run.

## Commands

```bash
cd /Users/ember/dev/parano1d-break/experiments/basefold-chains
CARGO_NET_OFFLINE=true cargo check
cargo fmt --check
CARGO_NET_OFFLINE=true cargo run --release -- all
```

All passed; final output ended with:

```text
status=pass mode=all collision_first_at=15641 collision_evaluations=111525
```

Files created:

- `experiments/basefold-chains/src/main.rs`
- `experiments/basefold-chains/Cargo.toml`
- `experiments/basefold-chains/Cargo.lock`
- `experiments/basefold-chains/rust-toolchain.toml`
- `experiments/basefold-chains/.cargo/config.toml`
- `experiments/basefold-chains/.gitignore`

The shared checkout remains at `52938433a30b185ab007afaa15166a632cf2e82e`. Its pre-existing unrelated modifications were left untouched, and no `noid_fri_binius` files were changed.
