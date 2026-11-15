# agent-4: Build capsule exploit chains

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Implemented and ran all experiments under `experiments/capsule-chains`. **No altered full C1 PCS opening was accepted, and no authorization forgery is claimed.** One positive result is limited to the production **pre-hashed Merkle API**.

## Built

- `experiments/capsule-chains/Cargo.toml` and `Cargo.lock` — path dependencies into the requested checkout.
- `experiments/capsule-chains/src/lib.rs`
  - Compiles the unchanged production `interleaved_commit.rs` through `src/lib.rs:9`, exposing its crate-private production tree/proof functions to the harness.
  - Implements aligned collision generation at `src/lib.rs:154-208`.
  - Constructs the real zero-final-input-rate CAPSMIX leaf at `src/lib.rs:55-70`.
- `experiments/capsule-chains/src/bin/merkle_collision.rs`
  - Two fixed-depth vectors, one commitment, and altered openings through production Merkle verification.
- `experiments/capsule-chains/src/bin/alias_depth.rs`
  - CAPSMIX leaf/internal-node alias, cross-depth proofs, fixed-point attempts, and zero-left collision chaining.
- `experiments/capsule-chains/src/bin/pcs_verify.rs`
  - Honest production typestate fixture and attacks through the actual `zk_capsule_pcs_verify`.

## Results

### 1. Production Merkle accepted two altered openings

`merkle_collision.rs:24-49` regenerated an aligned CAPSNODE collision and built two distinct **depth-13, cap-depth-3** leaf vectors—the exact source-tree geometry. Their eight-hash commitments were identical, and production Merkle verification accepted both distinct openings.

- Collision found after **72,274 evaluations** (`~2^16.14`), in 0.192 seconds for the complete experiment.
- Digest: `b2cf54a478131d318ebef1af2ec0e315245b8ee3af237a21e3fef04e4a20478b`
- Same cap: `true`
- Distinct pre-hashed leaf vectors: `true`
- Distinct opening siblings: `true`
- Both openings accepted: `true`

This is **not full capsule acceptance**: these leaf digests are supplied directly to the Merkle API. Substituting an actual production CAPSMIX digest instead failed with `source batched Merkle cap mismatch` (`merkle_collision.rs:51-55`).

### 2. CAPSMIX alias confirmed; depth/fixed-point chains failed

`alias_depth.rs:15-46` constructs the alias precisely. If `S` is the state after eleven CAPSMIX permutations, the last absorbed pair is chosen to cancel `S.rate`, giving:

```text
D = P([0, 0, S2, S3]).rate
X = [S2 + IV_CAPSNODE_0, S3 + IV_CAPSNODE_1]
CAPSMIX(leaf) = D = CAPSNODE(0, X)
```

Observed digest for both representations:

```text
8f896f52226dfede76e85ea49a622eedcba1aeae61a6b2cbc2f965adcf2c9241
```

The production Merkle verifier accepted a depth-0 leaf proof and a depth-1 internal-node proof against the same root (`alias_depth.rs:48-56`). Cross-depth attempts failed:

- Shallow proof at depth 1: `insufficient source siblings at layer 0`
- Deep proof at depth 0: `source batched Merkle cap mismatch`

Neither fixed point existed (`alias_depth.rs:63-76`):

- Left-carried extension needs `P([D, X+IV]).rate = 0`.
- Right-carried extension needs `P([X, D+IV]).rate = D+X`.

Each is a 256-bit output condition; the extra-level proof failed with `source batched Merkle cap mismatch`.

A separate zero-left aligned collision was found after 4,327 evaluations:

```text
2a77d25c989decd9ddea39a1303906d2d69da7ea854b44a7ce37b31e21cf392d
```

The alias’s `X` matched neither collision right input and was not in the aligned effective-input class. Both substitutions failed with `source batched Merkle cap mismatch` (`alias_depth.rs:78-97`).

### 3. Full production PCS rejected every altered opening

`pcs_verify.rs:149-180` created an honest opening through the complete public production typestate—fresh source commitment, Owner binding, Phase A, Phase B link, mid commitment, tail reveal, and opening. The honest opening verified.

All altered openings failed:

- Source CAPSNODE path substitution:
  `SourceMerkle("source batched Merkle cap mismatch")`
- Mid CAPSNODE path substitution:
  `MidMerkle("source batched Merkle cap mismatch")`
- Real CAPSMIX alias-leaf substitution:
  `SourceToMidMismatch { query_index: 0 }`
- Chained alias leaf plus CAPSNODE path substitution:
  `SourceToMidMismatch { query_index: 0 }`
- One-hash cap/depth abuse:
  `SourceCapCount { expected: 8, actual: 1 }`

Accepted altered full PCS openings: **0**.

This matches production ordering: source/mid leaf hashes and fold relations are recomputed at `noid_fri_binius/src/zk_capsule_pcs.rs:986-1023`, before Merkle verification at `:1025-1046`. Cap widths are pinned at `:935-946`.

## Exact exploit preconditions and costs

- To turn the accepted pre-hashed Merkle collision into a full capsule attack, each colliding child must be reachable as a real CAPSMIX leaf digest or as the root of a valid capsule subtree. Existing arbitrary aligned child digests do not satisfy that precondition.
- A random 256-bit CAPSMIX digest enters either 16-bit-per-lane aligned class with probability `2^-224`. Thus aligned-class entry costs about `2^224` leaf evaluations generically. Hitting one specified collision child costs `2^256`; hitting either of two specified children costs `2^255`. These are expected random-output search costs, not proven lower bounds.
- For alias+collision chaining, the alias right value `X` must either equal an existing collision right input or enter the aligned effective-right class so it can participate in a new collision search. Exact targeting is `2^256` generically; aligned-class entry is `2^224`. Even after that, fixed depth and PCS fold checks remain independent blockers.
- Depth-preserving alias extension requires one of the 256-bit fixed-point equations above; merely having a cross-depth alias is insufficient.
- A same-leaf/alternate-path full PCS opening would already pass the fold checks, but it still requires a collision whose children are reachable from the actual authenticated subtree. An altered source leaf must additionally satisfy source→mid→tail consistency.

The useful follow-up is therefore **reachability**: construct valid CAPSMIX/subtree preimages in the aligned class, then inject the collision on an actual depth-13 authenticated path. More arbitrary CAPSNODE collisions alone do not bridge that gap.

## Commands and verification

```bash
cd /Users/ember/dev/parano1d-break/experiments/capsule-chains

cargo fmt --all -- --check
CARGO_TARGET_DIR=$PWD/target cargo test --offline --release --all-targets
CARGO_TARGET_DIR=$PWD/target cargo build --offline --release --bins

./target/release/merkle_collision --regenerate
./target/release/alias_depth
./target/release/pcs_verify
```

Formatting and release builds passed. Tests passed: **1 passed, 0 failed**; the three binaries contain no additional unit tests. All executions were local and offline.

The checkout remained at `52938433a30b185ab007afaa15166a632cf2e82e`, with no diff in the four dependencies used. Five unrelated shared files showed concurrent modifications by the final status check; I did not edit or revert them.
