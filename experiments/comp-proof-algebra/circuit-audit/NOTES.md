# In-Circuit Recursive Verifier (`self_verify.rs`) & IVC Fold Soundness Audit

**Target Repository:** `parano1d` at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e` (`experiments/uc-composition/pinned/`)  
**Audit Harness:** `/Users/ember/dev/parano1d-break/experiments/comp-proof-algebra/circuit-audit/`  

---

## 1. Executive Summary

We performed a systematic, line-by-line equivalence and constraint-completeness audit comparing `noid_recursive`'s arithmetic-circuit verifiers and IVC fold/bank plumbing against `noid_ivc_core`'s native verifiers across three pillars:

1. **Pillar 1 — `self_verify.rs` vs. `noid_ivc_core` Native Verifier:** C0 (`F128`) and C1 (`F256`) statement/public-IO/post-commit binding, zerocheck, lincheck (inline and deferred-matrix), `QuirkyPoint` / `K_SKIP` evaluations, BaseFold sumcheck + multi-arity additive-NTT FRI coset folding, `QUERY_GRIND_BITS` PoW, and 128-bit query-lane bit decomposition.
2. **Pillar 2 — Deferred Merkle & Deep-Chain Walk Discharge:** `r_pcs_region.rs`, `ff_merkle.rs`, `leaf_hash.rs`, `region_source_binding.rs`, `region_source_binding_c1.rs` (`+ trace_impl.rs`), `region_sidecar.rs` (`block.rs`, `link.rs`, `joint_c1.rs`, `recording_duplex.rs`, `combined_duplex.rs`, `merkle_trace.rs`), `deep_chain.rs`, `deep_chain/c1.rs`, and `deep_chain/relations{,/c1}.rs`.
3. **Pillar 3 — Accumulator, Matrix Fold, Class Banks & Origin / Retirement Bindings:** `matrix_fold.rs`, `batch_eval.rs`, `accumulator.rs`, `matrix_claim/c1.rs`, `matrix_claim/sparse_c1/*`, `history_step/{relation,gated_recorder,v2}.rs`, `v2/{bank,banked/*,capacity_banked/*}`, `history_step_bank/{mod,retirement/*}.rs`, and `noid_miner/src/history_protocol{,/origin}.rs`.

### Overall Verdict on Production Consensus (`HistoryStep` C1)
**No consensus-breaking soundness discrepancy exists in production `HistoryStep` (legacy, 2-class banked v2, or 3-class capacity-banked v2.1).** Every C1 Fiat-Shamir absorption/squeeze, deferred Merkle walk join, `root_copy_offset` boundary case (`depth < stride` vs. power-of-two `depth == stride`), `CommittedShift` boundary (`slot = 0`), parent-arm selector, matrix-fold gate, bank digest inheritance, and SPARK / origin certificate check is algebraically tight.

### Summary of Findings Identified

| ID | Pillar | Component | Classification & Reachability | Summary |
|---|---|---|---|---|
| **Finding 1** | Pillar 1 | `pcs/basefold.rs` & `trace/self_verify.rs` (C0) | **Latent Library Soundness Bug** (C0 `log_batch_size == 0`; **unreachable in production C1**, where C1's pre-loop fix closes it) | In C0 `basefold::verify` and `basefold_verify_trace`, `post_row_batch_commit` (`T2` Merkle root) is only absorbed when `round + 1 == log_batch_size`. At `log_batch_size == 0` and `!arities.is_empty()`, `T2` is **never absorbed into the Fiat-Shamir transcript** (and `basefold::prove` panics on `0usize - 1` underflow). C1 explicitly fixed this pre-round-0 (`basefold.rs:1915-1917`, `self_verify.rs:2527-2530`), which actively saves `sparse_c1` at `log_len == 3`. |
| **Finding 2** | Pillar 2 | `ff_merkle.rs`, `region_source_binding{,_c1}.rs`, `region_sidecar.rs` | **Standalone Sidecar API Soundness Bug / Footgun** (**mitigated in production `HistoryStep`** by outer R1CS boolean column slices) | `ff_merkle_booleanity_terms` (`NODE * (D^2 + D) = 0`) is defined in `ff_merkle.rs:224-239` with **zero non-test callers** and omitted from `merkle_protocol_zero_terms` and `c1_merkle_zero_terms{,_trace}` (unlike `TwoPermutation` and `PairedUpdate`, which include booleanity inside Walk-B). Standalone `MerkleRegionVk` callers allocating column 8 via `alloc_f128` (as in `region_sidecar.rs:2446`) leave `D` unconstrained over $\mathbb{F}_{2^{128}}$, admitting $2^{128}$ affine-line `(CR', SIB')` pairs per honest node. Production `HistoryStep` mitigates all 3 instances via outer R1CS boolean slice allocators. |
| **Finding 3** | Pillar 1 & 3 | `self_verify.rs`, `field_circuit.rs`, `gated_recorder.rs` | **Production-Reachable Circuit-vs-Native Language / Witness-Extractability Gap** (**soundness-preserving**; preserves 16-bit PoW cost and R1CS soundness) | `pow_nonce` is allocated as an unconstrained 128-bit `F128` wire (`alloc_f128`), and when `bits > 0` (`QUERY_GRIND_BITS = 16`), all three `verify_pow` circuit gadgets constrain only the squeezed challenge's `128 - bits = 112` low bits and emit **zero range constraints** forcing `pow_nonce.hi == 0` (whereas native `C1BaseFoldProof::pow_nonce` is `u64`). Contrasts with `zk_auth_nonce.rs:4-14,40-63`, which explicitly enforces 64-bit range checks on `zk_authorization` nonces for this exact reason. |

---

## 2. Detailed Findings & Defensive Remediations

### Finding 1: Unabsorbed `post_row_batch_commit` (`T2` Root) in C0 BaseFold Verifier & Recursive Trace when `log_batch_size == 0`

#### Exact Code Citations
- **C0 Native Verifier:** `noid_ivc_core/src/pcs/basefold.rs:1271-1284`
  ```rust
  for round in 0..log_msg_len {
      ...
      if round + 1 == log_batch_size && !arities.is_empty() {
          observe_root(challenger, &proof.post_row_batch_commit.root);
      }
  ```
- **C0 Recursive Trace Verifier:** `noid_recursive/src/acceptance/trace/self_verify.rs:2281-2297`
  ```rust
  for round in 0..log_msg_len {
      ...
      if round + 1 == log_batch_size && !arities.is_empty() {
          observe_flat_digest(b, ch, &proof.post_row_batch_commit);
      }
  ```
- **C0 Prover Underflow / OOB Panic:** `noid_ivc_core/src/pcs/basefold.rs:972,1101` (`round_messages[log_batch_size - 1]` underflows when `log_batch_size == 0`, contradicting the comment at `basefold.rs:830-833` stating *"when `log_batch_size == 0` this equals `initial_codeword.len()` (the FRI-from-round-0 path reads `initial_codeword` directly), so that case is unaffected"*).
- **Statement Validation Permitting `log_batch_size == 0`:** `noid_ivc_core/src/verifier.rs:1093,1192` (`commitment.params.log_batch_size + LOG_PACKING <= commitment.params.m`) and `self_verify.rs:3343`.
- **Contrast with C1 Fix:** `noid_ivc_core/src/pcs/basefold.rs:1915-1917` and `noid_recursive/src/acceptance/trace/self_verify.rs:2527-2530`:
  ```rust
  if log_batch_size == 0 && !arities.is_empty() {
      observe_root(challenger, &proof.post_row_batch_commit.root);
  }
  ```

#### Technical Mechanics & Impact
When `log_batch_size == 0` and `log_dim = log_msg_len > 0` (`!arities.is_empty()`), `round` iterates over `0..log_msg_len`, so `round + 1` is in `1..=log_msg_len` and **never equals `0`**. Consequently:
1. Both `basefold::verify` and `self_verify::basefold_verify_trace` skip absorbing `proof.post_row_batch_commit.root` (`T2`) altogether.
2. Every sumcheck/FRI folding challenge $r_0, \dots, r_{L-1}$, the `QUERY_GRIND_BITS` PoW challenge, and all `n_queries` query indices are squeezed **independently** of `post_row_batch_commit`.
3. Because `T1` (`initial_leaf` of length $2^0 = 1$) only checks `post_row_batch_leaf[inner_offset] == initial_leaf[0]` at the single queried slot inside each $2^{\text{arity}_0}$-element `T2` coset (`arity_0 >= 1`), an unbound `T2` tree breaks adaptive Fiat-Shamir soundness for C0 `log_batch_size == 0`: any `T2` chosen after observing $r_0, \dots, r_{L-1}$ and query positions can leave `post_row_batch_leaf[inner_offset]` untouched while adjusting an unqueried sibling slot in each queried coset to fold to an arbitrary constant `final_a`.

#### Production Reachability
- **Unreachable in production consensus (`HistoryStep`):** Production `HistoryStep` exclusively uses C1 with `HISTORY_STEP_PCS_LOG_BATCH_SIZE = 5` (`history_step_bank.rs`), and SPARK matrix retirement (`noid_ivc_core/src/matrix_claim/sparse_c1.rs:116-120`, where `log_batch_size = 5.min(log_len - 3)` reaches `0` at `log_len == 3`) uses C1, where `basefold.rs:1915-1917` properly absorbs `post_row_batch_commit` before round 0.
- **Confirmed by `Check 1` in `src/main.rs`:** Verified deterministically that changing `post_row_batch_commit.root` at `log_batch_size == 0` leaves C0 native and trace challenges invariant (`0` roots absorbed), whereas C1 native and trace absorb `T2` pre-round-0 and diverge on all challenges.

#### Recommended Remediation
Backport C1's pre-loop absorption guard into C0 `basefold::verify` (`basefold.rs:1270`), `basefold_verify_trace` (`self_verify.rs:2280`), and `basefold::prove` (`basefold.rs:865,972,1101`), or reject `log_batch_size == 0` in `PcsParams` / `basefold::verify` if C0 `log_batch_size == 0` is deprecated.

---

### Finding 2: Missing `ff_merkle_booleanity_terms` in `MerkleRegionVk` Walk-B Zero Terms (`FeedForward` / `FeedForwardStrided`)

#### Exact Code Citations
- **Dead Booleanity Term Constructor:** `noid_ivc_core/src/deep_chain/ff_merkle.rs:224-239` (`pub fn ff_merkle_booleanity_terms` — zero non-test callers across the workspace).
- **Omission in C0 Walk-B Zero Terms:** `noid_recursive/src/acceptance/trace/region_source_binding.rs:5481-5504` (`merkle_protocol_zero_terms`) and `noid_recursive/src/region_sidecar/merkle_trace.rs:177-216` (`merkle_zero_terms_trace`).
- **Omission in C1 Walk-B Zero Terms:** `noid_recursive/src/acceptance/trace/region_source_binding_c1.rs:77-118` (`c1_merkle_zero_terms`) and `noid_recursive/src/acceptance/trace/region_source_binding_c1/trace_impl.rs:825-873` (`c1_merkle_zero_terms_trace`).
- **Standalone Public Sidecar API & Test Fixture:** `noid_recursive/src/region_sidecar.rs:799-831` (`pub struct MerkleRegionVk`) and `:2435-2508` (allocates all 9 Walk-B witness slices via unconstrained `builder.alloc_f128(*value)` at `:2446`).
- **Production `HistoryStep` Mitigations:**
  - Link Walk L-B (`r_pcs_region.rs:1194-1196`): allocates column 8 (`d`) via `alloc_boolean_column_slice_values_only`, plus active path direction bits come from `alloc_bool` query-lane bits (`r_pcs_region.rs:1644`, `self_verify.rs:1909`).
  - Block Walk-B (`region_source_binding.rs:1649,1676`): allocates `WalletB` and `MetaB` column 8 via `alloc_selected_boolean_column_slice` (`d * (d + 1) == 0` on all active rows).

#### Technical Mechanics & Impact
Inside Walk-B, `TwoPermutation` (`c1_merkle_zero_terms` lines `80-93`) and `PairedUpdate` (`lines 120-146`) explicitly include direction-bit booleanity `EVEN * (D^2 + D) = 0` inside the sidecar's own `zero` relation sumcheck. By contrast, `FeedForward` / `FeedForwardStrided` emit **only** `ff_merkle_chain_terms`, omitting `ff_merkle_booleanity_terms` (`NODE * (D^2 + D) = 0`).

In `ff_merkle` (`trace_impl.rs:958-987`), the Poseidon2b input lanes at each node slot are:
$$L = (1 + D) \cdot \text{CR} + D \cdot \text{SIB}, \qquad R \oplus \text{IV} = D \cdot \text{CR} + (1 + D) \cdot \text{SIB}$$
and the feed-forward output is $P(L, R) \oplus L$. Over $\mathbb{F}_{2^{128}}$, the matrix $\begin{pmatrix} 1+D & D \\ D & 1+D \end{pmatrix}$ has determinant $(1+D)^2 + D^2 = 1$ for **every** $D \in \mathbb{F}_{2^{128}}$! Therefore, for any valid child pair $(L^*, R^*)$ under an honest Merkle root and **any** non-boolean $D \in \mathbb{F}_{2^{128}} \setminus \{0, 1\}$, setting:
$$\text{CR}' = (1 + D) L^* + D R^*, \qquad \text{SIB}' = D L^* + (1 + D) R^*$$
satisfies $(1 + D)\text{CR}' + D\,\text{SIB}' = (1 + D^2 + D^2)L^* = L^*$ and $D\,\text{CR}' + (1 + D)\text{SIB}' = R^*$ identically across both 128-bit digest lanes. Every Walk-B `zero`, `substitution`, and `shift` sumcheck equation vanishes identically while binding an arbitrary point on the $2^{128}$-element affine line through $(L^*, R^*)$ to the honest Merkle root.

#### Production Reachability
- **Mitigated in production `HistoryStep`:** Every `FeedForwardStrided` Walk-B region in `HistoryStep` (`Link` Walk L-B, Block `wallet_b`, Block `meta_b`) constrains column 8 (`D`) to `{0, 1}` in the enclosing outer `FieldR1cs` matrix (`r_pcs_region.rs:1194-1196`, `region_source_binding.rs:1649,1676`).
- **Standalone API Footgun:** `MerkleRegionVk` (`region_sidecar.rs:799`) is a public verification key type whose doc comments do **not** warn callers that `FeedForward` / `FeedForwardStrided` require external outer-R1CS boolean constraints on slice 8 (unlike `TwoPermutation` and `PairedUpdate`, which are self-contained). Even `region_sidecar.rs`'s own end-to-end test (`:2446`) allocates slice 8 with unconstrained `alloc_f128`.
- **Confirmed by `Check 2` in `src/main.rs`.**

#### Recommended Remediation
Include `ff_merkle_booleanity_terms(refs)` (weighted by independent $\lambda$ power, e.g., $\lambda^3$) in `merkle_protocol_zero_terms`, `merkle_zero_terms_trace`, `c1_merkle_zero_terms`, and `c1_merkle_zero_terms_trace` so `MerkleRegionVk` is unconditionally standalone-sound across all family variants (note: requires regenerating frozen VK/matrix pins if applied to `HistoryStep`).

---

### Finding 3: Unconstrained Upper 64 Bits (`hi`) of `pow_nonce` in `self_verify.rs` and `FsChannelOps::verify_pow`

#### Exact Code Citations
- **Proof Witness Allocation:** `noid_recursive/src/acceptance/trace/self_verify.rs:1595` (`BaseFoldProofTrace::alloc`) and `:1780` (`C1BaseFoldProofTrace::alloc_mode`):
  ```rust
  // The native challenger absorbs the nonce as `F128 { lo, hi: 0 }`.
  let pow_nonce = LinExpr::from_wire(b.alloc_f128(f128_from_u128(native.pow_nonce as u128)));
  ```
- **All Three `verify_pow` Circuit Gadgets:**
  - `noid_ivc_core/src/field_circuit.rs:1847-1860` (`FsChannelTrace::verify_pow_trace`)
  - `noid_ivc_core/src/field_circuit.rs:2561-2580` (`FsChannelUnionRecorder::verify_pow`)
  - `noid_recursive/src/acceptance/history_step/gated_recorder.rs:325-341` (`BaseSelectableParentRecorder::verify_pow`)
- **Native `u64` Type & Wire Deserializer:** `noid_ivc_core/src/challenger.rs:930-941` (`nonce: u64` absorbed as `F128 { lo: nonce, hi: 0 }`) and `noid_ivc_core/src/matrix_claim/sparse_c1/wire.rs:344`.
- **Contrast with `zk_auth_nonce.rs`:** `noid_recursive/src/acceptance/trace/zk_auth_nonce.rs:4-14,40-63`, where the authors explicitly document and fix this exact issue for `zk_authorization`:
  > *"Native capsules store `nonce: u64` and absorb it into the raw transcript sponge as `[nonce as u128, 0]`... Without an explicit width check on that first lane, the circuit relation is slightly wider than the native serialized proof language: a dishonest recursive prover could grind in the unused upper 64 bits of the first lane even though no standalone capsule encoding can represent such a nonce."*

#### Technical Mechanics & Impact
When `bits > 0` (`QUERY_GRIND_BITS = 16`), all three `verify_pow` gadgets absorb `nonce` directly as a 128-bit `F128` sponge lane, squeeze `pt = sample_f128(b)`, and call `b.decompose_bits_le(&pt, 128 - bits as usize)` (112 boolean constraints on the **output** challenge), without calling `b.decompose_bits_le(nonce, 64)` on the **input** `nonce` wire (`nonce` is only constrained when `bits == 0`).

- **Production Reachability:** Reachable in production `HistoryStep` (legacy, v2, and v2.1) for the verified predecessor proof's `pcs_open.pow_nonce`. A recursive prover can satisfy every `HistoryStep` R1CS constraint and Walk L-C duplex check using a predecessor witness whose `pow_nonce` has `hi != 0`, for which **no native serialized `C1FieldR1csProof` exists**.
- **Why Soundness-Preserving for Consensus:** Each candidate 128-bit `F128` nonce still undergoes a full Poseidon2b sponge permutation and must clear the exact `2^{16}` leading-zero filter (`112`-bit `decompose_bits_le` on `pt`), preserving expected grinding work ($2^{16}$ permutations per proof) and binding all subsequent query positions to the absorbed 128-bit lane.
- **Confirmed by `Check 3` in `src/main.rs`.**

#### Recommended Remediation
Add `b.decompose_bits_le(nonce, 64);` (64 rows per BaseFold verification) inside `verify_pow_trace` / `FsChannelUnionRecorder::verify_pow` / `BaseSelectableParentRecorder::verify_pow` when `bits > 0` (matching `zk_auth_nonce.rs:40-63`) so the in-circuit language matches the native `u64` proof language bit-for-bit.

---

## 3. Pillar-by-Pillar Audit Matrix of Verified-Tight Invariants

### Pillar 1: `self_verify.rs` vs. `noid_ivc_core` Native C1 Verifier

| Protocol Step | Native Reference (`noid_ivc_core`) | Circuit Reference (`noid_recursive` / `noid_ivc_core`) | Verdict & Key Invariant Checked |
|---|---|---|---|
| Statement & `PcsParams` Binding | `proof.rs:146-174` (`bind_statement_field_parts_c1`) | `self_verify.rs:335-363` (`bind_statement_field_parts_c1_trace`) | **TIGHT.** Absorbs `"history-field-r1cs-c1"`, `statement_digest`, `[m, log_inv_rate, log_batch_size]` as constant lanes, and `commitment_root`. |
| Public IO Binding | `public_io.rs:286-341` (`bind_public_io_c1`) | `self_verify.rs:3262-3320` (`bind_public_io_c1_trace`) | **TIGHT.** Absorbs `"history-public-io-c1"`, `spec.transcript_lanes()`, `io` wires, samples `y` in $\mathbb{F}_{2^{256}}$, appends `io_slice` + auxiliary `spec.claims` to PCS batch. |
| Post-Commit Class Tag | `public_io.rs:343-349` (`POST_COMMIT_CLASS_BINDING_LABEL`) | `self_verify.rs:3580-3586` | **TIGHT.** Absorbs `"history-post-commit-class-v1"` + 32-byte `post_commit_class_digest` immediately after `bind_public_io_c1_trace` and before sidecar replay. |
| C1 Zerocheck | `zerocheck/field_c1.rs:479-610` | `self_verify.rs:640-775` (`zerocheck_field_verify_c1_trace`) | **TIGHT.** Exact degree-3 round interpolation (`u(0) + u(1) = claim`, `u(X)` at $X \in \{0, 1, 2, \infty\}$), quirky `z_skip` round, and terminal `eq * (a_eval * b_eval + c_eval) == claim` pin. |
| C1 Lincheck (Inline & Deferred) | `lincheck/c1.rs:519-665` | `self_verify.rs:1165-1335` (`lincheck_verify_c1_trace_deferred`) | **TIGHT.** Samples `alpha`, runs `k_log - k_skip` outer sumcheck rounds, absorbs `z_partial` ($2^{k_{\text{skip}}}$ evaluations), peels `const_pin` column-0 `beta * z_partial[0]` contribution (`z[0] = 1`), emits exact `C1FreshLincheckClaimTrace`. |
| C1 Challenge Subfield Exclusion | `field/gf2_256.rs:119-125` (`from_raw_challenge_lanes`) | `field_circuit.rs:1183-1189` (`c1_challenge_from_raw`) | **TIGHT (`Check 5`).** Maps `(lo, y) -> (lo, y^2 + y + EXTENSION_TAU)`. Since $\text{Tr}(\tau) = 1$ and $\text{Tr}(y^2 + y) = 0$ over $\mathbb{F}_{2^{128}}$, $\text{Tr}(\text{hi}) = 1 \implies \text{hi} \neq 0$ unconditionally. Challenges never lie in $\mathbb{F}_{2^{128}}$ (never $0$ or $1$), with 255-bit min-entropy. |
| Query Index Bit Decomposition & High Bits | `pcs/basefold.rs:274-295` (`sample_query_positions`) | `self_verify.rs:1891-1926` (`bind_query_positions_lane_trace`) | **TIGHT (`Check 4`).** Allocates **all 128** flat bits via `alloc_bool` (`0..n_used*k_code` AND unused `(n_used*k_code)..128`) and pins `sum + lane == 0` (arm-gated when inactive in banked `HistoryStep` via `gated_recorder.rs`). Prevents bit aliasing and high-bit witness malleability. |
| Additive-NTT FRI Coset Fold & Tail | `pcs/basefold.rs:1910-2120` | `self_verify.rs:2505-2745` (`basefold_verify_c1_trace`) | **TIGHT.** Enforces `post_row_batch_commit` pre-round-0 when `log_batch_size == 0`, checks `final_codeword` constancy == `final_a`, `final_a * final_b == running`, cross-checks `T1` row-batch fold against `T2[inner_offset]`, folds cosets with exact `AdditiveNttF128` subspace twiddles, and folds `plaintext_tail` to `final_codeword`. |

---

### Pillar 2: Deferred Merkle, `r_pcs_region`, `ff_merkle`, and Deep-Chain Walk Discharge

| Invariant / Edge Case | Citations | Verdict & Technical Verification |
|---|---|---|
| Entry Leaf Hashing (`IVCPCSF_` vs `IVCPCSL_`) | `leaf_hash.rs:58-145`, `r_pcs_region.rs:150-240,1575-1590` | **TIGHT.** Fixed-length even-lane PCS leaves use length-bound `IVCPCSF_` capacity IV (`leaf_fixed_iv_flat(lanes * 16)`); `r_pcs_region.rs:1575-1590` unconditionally pins Walk L-A's squeezed leaf digest `(slices_a[2][digest_slot], slices_a[3][digest_slot])` equal to Walk L-B's path entry `(slices_b[4][leg_slot], slices_b[5][leg_slot])`. |
| Path Direction Bits (`0..depth`) | `r_pcs_region.rs:1640-1645` | **TIGHT.** Under `arm_gate`, every level `lvl in 0..tree.depth` pins `slices_b[8][leg_slot + lvl]` equal to `obs.bits[lvl]` (the exact LSB-first query-index bit wire from `bind_query_positions_lane_trace`). |
| Terminal Root Pinning: `depth < stride` vs. Power-of-Two `depth == stride` | `ff_merkle.rs:118-135`, `r_pcs_region.rs:1647-1660`, `region_source_binding.rs:2815-2855` | **TIGHT (`Check 2`).** When `tree.depth < carrier_depth` (`root_copy_offset() == Some(depth)`), `nodens` is `1` at `leg_slot + tree.depth`, so `ff_merkle_chain_terms` forces `CR[leg_slot + depth] == C[last] + mix(last)`, and `r_pcs_region.rs:1656-1659` pins `CR[leg_slot + depth] == root`. When `tree.depth == carrier_depth` (power-of-two `depth == stride`, so `root_copy_offset() == None` and no trailing ghost slot exists!), `r_pcs_region.rs:1647-1655` computes `C[last] + CR[last] + D[last] * (CR[last] + SIB[last])` inline in R1CS and pins it directly to `obs.root`! |
| Ghost / Inactive Slots & `CommittedShift` Boundary (`slot = 0`) | `relations.rs:287-298`, `relations/c1.rs:661-687`, `deep_chain.rs:1079-1152` | **TIGHT (`Check 6`).** `CommittedShift` resolves `shifted[0] = 0` and `shifted[i] = col[i - 1]` for `i >= 1` (`shift_kernel_eval(0^w, sigma) == 0` identically; `col[width - 1]` never wraps into slot `0`). Moreover, `nodens` is `0` at every path start (`p * stride`, including `slot = 0`) and on all padding/ghost slots `depth + 1 .. stride`, while `region = node` on `FeedForward` (`c1_merkle_substitution_terms` `region_source_binding_c1.rs:164-175` uses `region + node == 0`, canceling `CommittedShift(C)` on `FeedForward`). |
| Post-Commit $\alpha$-Batching & Joint C1 Causality | `joint_c1.rs:205-380`, `region_source_binding_c1.rs:380-465` | **TIGHT.** Both outer witness commitment `T1` and all sidecar internal commitments (`s0`, `s_out`) are absorbed into the Fiat-Shamir transcript **before** the C1 ragged walk (`prove_ragged_c1` / `verify_ragged_c1`) samples evaluation points and **before** `lambda` and `alpha` are sampled via `sample_f256` (`trace_impl.rs` / `region_source_binding_c1.rs`). Because `sample_f256` never lands in $\mathbb{F}_{2^{128}}$, $\alpha \neq 0$ always, and Schwartz-Zippel soundness error is $\le \deg / 2^{255}$. |
| Multi-Arm `RecordingDuplex` Selector | `recording_duplex.rs:926-975`, `r_pcs_region.rs:1558-1567` | **TIGHT.** Proof-carried `selector` is domain-constrained (`selector * (selector + 1) == 0` for 2 arms; `selector * (selector + 1) * (selector + 2) == 0` for 3 arms), absorbed into the sidecar transcript before fixed-table selection, and emitted as a `log2_len = 0` `C1QuirkyDirectClaim` pinned to the outer circuit's authenticated parent-selector wire. |

---

### Pillar 3: Accumulator, Matrix Fold, Class Banks & Origin / Retirement Bindings

| Invariant / Mechanism | Citations | Verdict & Technical Verification |
|---|---|---|
| C1 Matrix Claim Fold (`gate = 0` Base vs. `gate = 1` Step) | `matrix_claim/c1.rs:962-1027`, `matrix_fold.rs:323-407` | **TIGHT (`Check 7`).** `verify_matrix_claim_fold_c1_trace` matches `verify_matrix_claim_fold_c1` line-by-line. `gamma_gate = gamma * gate` and `delta_gate = delta * gate` cleanly zero out `incoming.value`, `proof.g_e`, and `incoming_weight` when `gate == 0`, and bind both `fresh` and `incoming` with error $\le (2 k_{\log} + 3)/2^{255}$ when `gate == 1`. |
| Banked Class Inheritance & Fork Base (`BASE`) Protection | `v2.rs:585-779`, `v2/banked/assembly.rs:88-288`, `v2/capacity_banked/assembly.rs:113-367`, `v2/banked/bank.rs:210-335` | **TIGHT.** `BASE` is boolean-constrained (`BASE * (BASE + 1) == 0`); `gate = BASE + 1`. Across **both** `BASE == 1` and `BASE == 0`, bank digests, matrix digests, post-commit class digests, and `ORIGIN` / `ORIGIN_ID` / `ORIGIN_ACC` are **unconditionally** pinned equal (`cells[BANK..POINT] == prev[BANK..POINT]`), and `ORIGIN_ACC.height` is pinned to `activation_height - 1`. At `BASE == 1`, all outgoing accumulated matrix lanes (`point..=live`) are pinned to `0`, and `bank.parse` checks `base == (acc.height == activation_height)` and rejects non-empty incoming claims on a base terminal (`base != any(claim.is_some())`), preventing mid-chain accumulator resets. |
| Shared Parent Verifier Replay (3-Class v2.1) | `v2/capacity_banked/shared_parent.rs:66-180`, `assembly.rs:240-345` | **TIGHT.** Runs a single shared C1 deferred verifier trace over selector-muxed parent shape/digest/IO wires (`selector in {0, 1, 2}` from 3 disjoint boolean arm flags summing to `1 - BASE`), feeds `fresh` into the selected class's `verify_matrix_claim_fold_c1_trace` with `fold_gate = arm * incoming_live`, and carries non-selected class lanes unchanged via `prev + arm * (folded + prev)`. |
| Terminal Decider & `ClaimCache` | `v2/banked/decision.rs:45-175`, `v2/capacity_banked/decision.rs:40-135`, `history_step_bank.rs:920-1080` | **TIGHT.** Terminal decider verifies the outer C1 proof + joint sidecar, checks the fresh tip `C1R1csClaim` against the tip matrix, and evaluates `stacked_matrix_mle_eval_c1` on **every** live accumulated class lane (`ClaimCache` keys on `(bank_digest, class_id, matrix_digest, shape, exact_claim)`). |
| `FieldR1csBuilder` Optimizations | `field_circuit.rs:555-810,1210-1230` | **TIGHT (`Check 7`).** `pin_boolean_gated_zero` (`w = g * (e + w)` -> `0 = g * e` in char 2), `bind_linear_pin_in_free_row` (substituting linear equality directly into an unused `z_i = z_i * 1` row), and `symbolic_product_aliases` are strictly witness-independent in matrix shape and algebraically exact over $\mathbb{F}_{2^{128}}$. |
| Origin Certificates & SPARK Legacy Matrix Retirement | `matrix_claim/sparse_c1.rs:110-390`, `sparse_c1/preprocess.rs:140-260`, `history_step_bank/retirement.rs:180-420`, `noid_miner/src/history_protocol.rs:320-610` | **TIGHT.** `PinnedRetirementKeys` authenticates static sparse matrix commitments `(row, col, val)` against release-pinned digests and the legacy bank digest; `write_tag_mle` uses `(1 << entry_log) | index != 0` so read counters never wrap to `0` in characteristic 2; `history_protocol.rs` verifies parent PoW, epoch anchor, and all live legacy matrix claims (or SPARK retirement proof) before producing `VerifiedOrigin`. |

---

## 4. Reproducing the Deterministic Constraint-Audit Suite

From `/Users/ember/dev/parano1d-break/experiments/comp-proof-algebra/circuit-audit/`:

```bash
CARGO_TARGET_DIR="$PWD/target" cargo run --release --offline | tee audit_output.txt
```

All 7 deterministic checks run completely offline against the untouched snapshot at `../../uc-composition/pinned/` (`52938433a30b185ab007afaa15166a632cf2e82e`) without modifying any upstream or sibling files.
