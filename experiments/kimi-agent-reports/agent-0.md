# agent-0: Escalate Merkle collision

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Bottom line

I find **no cheap escalation from the stated CAPSNODE collisions or CAPSMIX alias to a semantic altered PCS opening or fresh-request authorization forgery**. The collisions break unique Merkle-tree binding, but exploitation requires a substantially stronger object:

- a collision at a node actually recomputed by the verifier, and
- alternative queried payloads satisfying the source→mid→tail fold equations.

A targeted construction meeting both conditions would work; an arbitrary birthday collision or leaf/node digest equality does not.

## Exact acceptance boundary

Production uses the selected PCS in `zk_capsule_pcs.rs`, not the older `capsule.rs` protocol:

- Source: 8,192 leaves, depth 13, cap depth 3, hence 8 cap hashes and 10 path levels.
- Mid: 512 leaves, depth 9, cap depth 3, hence 8 cap hashes and 6 path levels.
- Sixty-five 13-bit source-leaf queries; query `q` maps to mid leaf `q >> 4`, member `q & 15`, with both cap indices `q >> 10`:  
  `source/noid_fri_binius/src/zk_capsule_pcs.rs:63-96`  
  `source/noid_fri_binius/src/zk_capsule_algebra.rs:477-503`

For each query, acceptance requires all of:

1. `Hmix(S_q)` authenticates to the source cap.
2. The affine bank/companion blend of `S_q`, folded by three challenges, equals the selected raw member of the mid leaf.
3. `H256(M_q)` authenticates to the mid cap.
4. The four-fold of `M_q` equals `Code(tail)[q >> 4]`.

These checks are explicit at `source/noid_fri_binius/src/zk_capsule_pcs.rs:977-1023`; Merkle verification follows at `:1025-1046`.

The multiprover has no exploitable type/level dispatch. It sorts and deduplicates indices, recomputes every parent with CAPSNODE, compares exact cap values, and rejects missing or extra siblings:  
`noid_fri_binius/src/interleaved_commit.rs:315-400` in the same-commit full checkout. Repeated indices must have identical hashes: `:565-588`.

Therefore, for a fixed transcript prefix, an altered opening is accepted **iff** its alternate forests reach the same caps **and** preserve these fold equations. Changing source symbols alone hits `SourceToMidMismatch`; compensating in the mid leaf still leaves `MidToTailMismatch`.

## What a cheap collision can and cannot do

CAPSNODE is used uniformly at internal nodes:  
`source/noid_fri_binius/src/capsule.rs:176-201`.

- **Collision off a verified path:** no effect.
- **Collision on a verified path, preserving the parent:** can produce alternate Merkle branches or path-only proof malleability.
- **Collision changing a queried leaf/subtree:** still needs the fold equalities above. It does not imply a changed accepted evaluation or source payload.

For an existing honest commitment, the required operation is a **targeted second preimage at a particular computed node**, not an arbitrary collision. A generic collision found in approximately `2^16.3` work does not include the existing child labels or parent output. Even within the 32-bit `V16^2` image, a targeted second preimage is generically about `2^32` trials, with additional child-compatibility constraints. A general honest 256-bit parent is outside that small image altogether, so the stated collision family cannot reproduce it.

There is also a useful algebraic blocker for small-subfield alternatives. For base-field leaf differences,

\[
\Delta O=\Delta B+\gamma(\Delta B+\Delta C).
\]

Because production explicitly excludes every γ in GF(2^128), `ΔO = 0` implies `ΔB = ΔC = 0`:  
`source/noid_gkr/src/zk_authorization.rs:239-243`.  
Thus distinct base-field bank/companion leaves cannot be invisible after the affine blend. They require mid compensation, followed by acceptance under the later mid fold and fixed tail. For one pre-fixed nonzero difference, the three- and four-variable fold groups have per-instance root bounds of roughly `3/2^255` and `4/2^255`, respectively; this is not a whole-protocol soundness estimate.

## Fresh-cap and fresh-request case

Allowing the forger to choose the source cap removes the fixed-cap second-preimage obstacle, but not the algebraic ones.

The verifier order is:

1. Fresh statement and source cap enter Owner before `rho`:  
   `source/noid_gkr/src/zk_authorization.rs:393-408`, `:621-681`.
2. `sigma` is absorbed before γ; Phase A must connect Owner’s bank claim and `sigma` to the terminal virtual-oracle value:  
   `source/noid_gkr/src/zk_authorization.rs:1320-1343`  
   `source/noid_fri_binius/src/zk_phase_a.rs:230-237`, `:411-445`.
3. `upper` precedes the three source folds.
4. Mid cap precedes the four mid folds.
5. Tail precedes the final local fold.
6. Only after grinding and query derivation are openings supplied:  
   `source/noid_gkr/src/zk_authorization.rs:1345-1378`.

The upper/tail link additionally checks both the Phase-A terminal point and the random Phase-B point:  
`source/noid_gkr/src/zk_authorization.rs:1021-1045`.

Consequently, a forger may select among precommitted branches while constructing later messages, but after queries any alternate branch must remain compatible with the already fixed upper, mid/tail commitments, and folds. Selecting one branch earlier and honestly proving it works, but the unused collisions add no unauthorized capability.

A remaining route is query avoidance: if a malicious cap already has fold-valid openings on a fraction `ρ` of query positions, arbitrary satisfying nonces can search for a favorable query set at roughly `2^16 / ρ^65` expected nonce trials, assuming independent query sets. The verifier does not require nonce minimality: `source/noid_gkr/src/zk_auth_qrom.rs:1011-1017`. This does not create the required fold-valid forest, and small `ρ` is not cheap. Fresh authorization still requires satisfying Owner/Phase A or independently breaking their soundness.

## CAPSMIX leaf/internal alias

The alias is real but supplies only an alternate interpretation of the **same permutation row**:

- CAPSMIX absorbs 24 lanes in 12 blocks:  
  `source/noid_fri_binius/src/capsule.rs:416-423`.
- If the final pre-permutation rate is zero, leaf digest `L` equals  
  `CAPSNODE(0, final_capacity XOR IV_CAPSNODE)`.

The artifact’s executable fixture constructs exactly this at  
`benchmarks/src/bin/native-source-extraction.rs:51-80`; the manuscript describes the same-row identification at `manuscript/sections/k-native-extraction.tex:42-45`.

It gives no second CAPSMIX payload, no different fold projection, and no alternate cap preimage. Native verification does not use the typed extraction dictionary, so it cannot be “confused” into accepting an internal node as a leaf. Position is enforced by the ordered path.

## Best next local experiment

Extend the existing direct PCS fixture at `source/noid_fri_binius/src/zk_capsule_pcs.rs:1152-1194`:

1. Generate a real V16 collision with one fixed child, and place it exactly at a parent recomputed for `worst_queries()`.
2. Keep all leaf payloads, mid/tail values, γ, β values, and queries fixed; substitute only the colliding branch. Verification should accept, demonstrating path-level malleability.
3. Change one source-leaf lane while retaining the cap. Expect `SourceToMidMismatch` at `:1008-1015`.
4. Compensate the corresponding raw mid member through inverse normalization while retaining mid cap and tail. Expect `MidToTailMismatch` at `:1019-1021`.

That experiment directly tests the escalation boundary. If step 2 cannot be made to verify, the collision was not on the authenticated forest and is irrelevant to PCS acceptance. No files were modified during this analysis.
