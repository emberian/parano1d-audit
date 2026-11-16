// SPDX-License-Identifier: Apache-2.0
//! Deterministic specification-conformance and R1CS constraint-completeness
//! audit harness for `noid_recursive` (`self_verify.rs`, Walk-B / `ff_merkle`,
//! `r_pcs_region`, `ShiftDischarge`, and `matrix_fold`) against `noid_ivc_core`
//! at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`.
//!
//! All checks are purely defensive specification, transcript-binding, and
//! constraint-rank unit tests over synthetic local fixtures.

use noid_ivc_core::challenger::{fs_pack_bytes_lanes, Challenger, FsLaneChallenger};
use noid_ivc_core::deep_chain::ff_merkle::{
    build_ff_merkle_path_columns, ff_merkle_booleanity_terms, ff_merkle_chain_terms,
    ff_merkle_fixed_patterns, FfMerkleFamilyRefs, FfMerklePathFamily, FfMerklePathWitness,
};
use noid_ivc_core::deep_chain::relations::c1::{
    prove_shift_discharge, shift_kernel_eval, verify_shift_discharge,
};
use noid_ivc_core::deep_chain::relations::{ColRef, FixedPattern, RelationTerm};
use noid_ivc_core::field::{F128, F256};
use noid_ivc_core::field_circuit::{
    ExtExpr, FieldR1csBuilder, FsChannelOps, FsChannelTrace, FsChannelUnionRecorder, LinExpr,
};
use noid_ivc_core::field_r1cs::synthetic_satisfiable;
use noid_ivc_core::matrix_claim::c1::{
    prove_matrix_claim_fold_c1, stacked_matrix_mle_eval_c1, verify_matrix_claim_fold_c1,
    C1MatrixAccClaim,
};
use noid_ivc_core::ntt::AdditiveNttF128;
use noid_ivc_core::pcs::basefold::{
    self, default_fri_queries, fri_commit_layout, BaseFoldProof, C1BaseFoldProof, C1QueryOpening,
    C1RoundMessage, QueryOpening, RoundCommitment, RoundMessage, QUERY_GRIND_BITS,
};
use noid_ivc_core::pcs::{compute_fri_arities, PcsParams, LOG_PACKING};
use noid_ivc_core::proof::FieldShape;
use noid_ivc_core::public_io::WitnessSlice;
use noid_ivc_prover::field_prover::{prove_field_c1, prove_field_c1_capturing_fresh};
use noid_recursive::acceptance::trace::deep_chain::{
    verify_c1_shift_discharge_trace, C1ShiftDischargeProofTrace,
};
use noid_recursive::acceptance::trace::matrix_fold::{
    verify_matrix_claim_fold_c1_trace, C1MatrixAccClaimTrace, C1MatrixFoldProofTrace,
};
use noid_recursive::acceptance::trace::self_verify::{
    alloc_flat_digest, basefold_verify_c1_trace, basefold_verify_trace,
    verify_field_c1_trace_deferred_region, verify_opening_batch_quirky_direct_c1_trace,
    BaseFoldProofTrace, C1BaseFoldProofTrace, C1FieldR1csProofTrace, C1QuirkyDirectClaimTrace,
};
use noid_recursive::region_sidecar::{
    MerkleRegionFamily, MerkleRegionVk, MERKLE_REGION_COMMITTED_COLUMNS,
};

/// Forwarding challenger wrapper that logs absorbed byte slices (Merkle roots)
/// and sampled challenges during native `basefold::verify` / `verify_c1`.
struct TranscriptAuditChallenger {
    inner: FsLaneChallenger,
    observed_roots: Vec<Vec<u8>>,
    sampled_f128: Vec<F128>,
    sampled_f256: Vec<F256>,
}

impl TranscriptAuditChallenger {
    fn new(domain: &[u8]) -> Self {
        Self {
            inner: FsLaneChallenger::new(domain),
            observed_roots: Vec::new(),
            sampled_f128: Vec::new(),
            sampled_f256: Vec::new(),
        }
    }

    fn new_c1(domain: &[u8]) -> Self {
        Self {
            inner: FsLaneChallenger::new_c1(domain),
            observed_roots: Vec::new(),
            sampled_f128: Vec::new(),
            sampled_f256: Vec::new(),
        }
    }
}

impl Challenger for TranscriptAuditChallenger {
    fn observe_label(&mut self, label: &[u8]) {
        self.inner.observe_label(label);
    }

    fn observe_f128(&mut self, value: F128) {
        self.inner.observe_f128(value);
    }

    fn observe_f128_slice(&mut self, values: &[F128]) {
        self.inner.observe_f128_slice(values);
    }

    fn observe_f256(&mut self, value: F256) {
        self.inner.observe_f256(value);
    }

    fn observe_f256_slice(&mut self, values: &[F256]) {
        self.inner.observe_f256_slice(values);
    }

    fn observe_bytes(&mut self, bytes: &[u8]) {
        self.observed_roots.push(bytes.to_vec());
        self.inner.observe_bytes(bytes);
    }

    fn sample_f128(&mut self) -> F128 {
        let c = self.inner.sample_f128();
        self.sampled_f128.push(c);
        c
    }

    fn sample_f128_vec(&mut self, n: usize) -> Vec<F128> {
        let v = self.inner.sample_f128_vec(n);
        self.sampled_f128.extend_from_slice(&v);
        v
    }

    fn sample_f256(&mut self) -> F256 {
        let c = self.inner.sample_f256();
        self.sampled_f256.push(c);
        c
    }

    fn sample_f256_vec(&mut self, n: usize) -> Vec<F256> {
        let v = self.inner.sample_f256_vec(n);
        self.sampled_f256.extend_from_slice(&v);
        v
    }

    fn verify_pow(&mut self, nonce: u64, bits: u32) -> bool {
        self.inner.verify_pow(nonce, bits)
    }
}

/// Evaluate a slice of `RelationTerm`s weighted by `eq_weights` over concrete columns.
fn eval_terms_on_columns(
    terms: &[RelationTerm],
    committed: &[Vec<F128>],
    fixed: &[FixedPattern],
    eq_weights: &[F128],
) -> F128 {
    let w = eq_weights.len();
    let resolve = |r: ColRef| -> Vec<F128> {
        match r {
            ColRef::Committed(i) => committed[i].clone(),
            ColRef::CommittedShift(i) => {
                let mut out = vec![F128::ZERO; w];
                out[1..].copy_from_slice(&committed[i][..w - 1]);
                out
            }
            ColRef::Fixed(i) => fixed[i].materialize(w),
            other => panic!("unexpected ColRef in Walk-B zero-term audit: {other:?}"),
        }
    };

    let mut total = F128::ZERO;
    for term in terms {
        let cols: Vec<Vec<F128>> = term.factors.iter().map(|&f| resolve(f)).collect();
        for row in 0..w {
            let mut prod = eq_weights[row] * term.coeff;
            for col in &cols {
                prod *= col[row];
            }
            total += prod;
        }
    }
    total
}

/// Check 1 (Finding 1): Audit transcript absorption of `post_row_batch_commit` (T2 root)
/// when `log_batch_size == 0` in C0 (`basefold::verify` & `basefold_verify_trace`)
/// versus C1 (`basefold::verify_c1` & `basefold_verify_c1_trace`).
fn audit_1_c0_vs_c1_basefold_t2_transcript_binding() {
    println!("=== [Check 1] BaseFold T2 (`post_row_batch_commit`) Transcript Binding at `log_batch_size == 0` ===");

    let log_msg_len = 5usize;
    let log_batch_size = 0usize;
    let log_inv_rate = 2usize;
    let log_dim = log_msg_len - log_batch_size;
    let k_code = log_dim + log_inv_rate;
    let params = PcsParams {
        m: log_msg_len + LOG_PACKING,
        log_inv_rate,
        log_batch_size,
        profile: Default::default(),
    };
    let arities = compute_fri_arities(log_dim);
    let (num_fri_commits, tail_layout) = fri_commit_layout(k_code, &arities);
    let n_queries = default_fri_queries(log_dim, log_inv_rate);
    let arity_0 = arities[0];
    let tail_len = tail_layout.map_or(0, |(len, _)| len);

    let initial_root = [0x42u8; 32];
    let t2_root_a = [0xAAu8; 32];
    let t2_root_b = [0xBBu8; 32];

    let make_c0_fixture = |t2_root: [u8; 32]| BaseFoldProof {
        round_messages: vec![
            RoundMessage {
                u_0: F128::new(3, 0),
                u_2: F128::new(5, 0),
            };
            log_msg_len
        ],
        post_row_batch_commit: RoundCommitment { root: t2_root },
        round_commitments: vec![RoundCommitment { root: [0x55; 32] }; num_fri_commits],
        final_a: F128::ONE,
        final_b: F128::ONE,
        final_codeword: vec![F128::ONE; 1 << log_inv_rate],
        plaintext_tail: vec![F128::ONE; tail_len],
        pow_nonce: 0,
        queries: vec![
            QueryOpening {
                position: 0,
                initial_leaf: vec![F128::ONE; 1 << log_batch_size],
                initial_path: vec![[0u8; 32]; k_code],
                post_row_batch_leaf: vec![F128::ONE; 1 << arity_0],
                post_row_batch_path: vec![[0u8; 32]; k_code - arity_0],
                epoch_leaves: Vec::new(),
                epoch_paths: Vec::new(),
            };
            n_queries
        ],
    };

    let make_c1_fixture = |t2_root: [u8; 32]| C1BaseFoldProof {
        round_messages: vec![
            C1RoundMessage {
                u_0: F256::from_base(F128::new(3, 0)),
                u_2: F256::from_base(F128::new(5, 0)),
            };
            log_msg_len
        ],
        post_row_batch_commit: RoundCommitment { root: t2_root },
        round_commitments: vec![RoundCommitment { root: [0x55; 32] }; num_fri_commits],
        final_a: F256::ONE,
        final_b: F256::ONE,
        final_codeword: vec![F256::ONE; 1 << log_inv_rate],
        plaintext_tail: vec![F256::ONE; tail_len],
        pow_nonce: 0,
        queries: vec![
            C1QueryOpening {
                position: 0,
                initial_leaf: vec![F128::ONE; 1 << log_batch_size],
                initial_path: vec![[0u8; 32]; k_code],
                post_row_batch_leaf: vec![F256::ONE; 1 << arity_0],
                post_row_batch_path: vec![[0u8; 32]; k_code - arity_0],
                epoch_leaves: Vec::new(),
                epoch_paths: Vec::new(),
            };
            n_queries
        ],
    };

    // 1A. Native C0 `basefold::verify`: inspect absorbed roots & sampled sumcheck/FRI challenges.
    let ntt = AdditiveNttF128::standard(k_code);
    let mut c0_ch_a = TranscriptAuditChallenger::new(b"audit-c0-t2");
    let mut c0_ch_b = TranscriptAuditChallenger::new(b"audit-c0-t2");
    let _ = basefold::verify(
        F128::ONE,
        &make_c0_fixture(t2_root_a),
        &initial_root,
        &ntt,
        log_batch_size,
        log_inv_rate,
        log_msg_len,
        &mut c0_ch_a,
    );
    let _ = basefold::verify(
        F128::ONE,
        &make_c0_fixture(t2_root_b),
        &initial_root,
        &ntt,
        log_batch_size,
        log_inv_rate,
        log_msg_len,
        &mut c0_ch_b,
    );

    assert!(
        !c0_ch_a.observed_roots.contains(&t2_root_a.to_vec()),
        "C0 native basefold::verify unexpectedly absorbed post_row_batch_commit at log_batch_size=0"
    );
    assert_eq!(
        c0_ch_a.sampled_f128, c0_ch_b.sampled_f128,
        "C0 native challenges must be invariant to post_row_batch_commit when unabsorbed"
    );

    // 1B. Recursive trace C0 `basefold_verify_trace`: inspect recorded data lanes & challenges.
    let run_c0_trace = |t2_root: [u8; 32]| -> (Vec<F128>, Vec<F128>) {
        let mut b = FieldR1csBuilder::new();
        let mut rec = FsChannelUnionRecorder::new(b"audit-c0-t2");
        let root_e = alloc_flat_digest(&mut b, &initial_root);
        let target_e = LinExpr::from_wire(b.alloc_f128(F128::ONE));
        let proof_e = BaseFoldProofTrace::alloc(&mut b, &make_c0_fixture(t2_root), &params);
        let (chals, _) =
            basefold_verify_trace(&mut b, &mut rec, &target_e, &proof_e, &root_e, &params, None);
        let chal_vals = chals.iter().map(|c| c.eval(b.values())).collect();
        let recording = rec.finish();
        (recording.data_flat, chal_vals)
    };
    let (c0_trace_data_a, c0_trace_chals_a) = run_c0_trace(t2_root_a);
    let (_, c0_trace_chals_b) = run_c0_trace(t2_root_b);
    let t2_lanes_a = fs_pack_bytes_lanes(&t2_root_a);
    assert!(
        !c0_trace_data_a
            .windows(2)
            .any(|w| w == t2_lanes_a.as_slice()),
        "C0 recursive trace unexpectedly absorbed post_row_batch_commit lanes"
    );
    assert_eq!(c0_trace_chals_a, c0_trace_chals_b);
    assert_eq!(c0_trace_chals_a, c0_ch_a.sampled_f128[..log_msg_len]);

    // 1C. Contrast with C1 (`basefold::verify_c1` & `basefold_verify_c1_trace`), where
    // `if log_batch_size == 0 && !arities.is_empty()` absorbs `post_row_batch_commit` pre-round-0.
    let mut c1_ch_a = TranscriptAuditChallenger::new_c1(b"audit-c1-t2");
    let mut c1_ch_b = TranscriptAuditChallenger::new_c1(b"audit-c1-t2");
    let _ = basefold::verify_c1(
        F256::ONE,
        &make_c1_fixture(t2_root_a),
        &initial_root,
        &ntt,
        log_batch_size,
        log_inv_rate,
        log_msg_len,
        &mut c1_ch_a,
    );
    let _ = basefold::verify_c1(
        F256::ONE,
        &make_c1_fixture(t2_root_b),
        &initial_root,
        &ntt,
        log_batch_size,
        log_inv_rate,
        log_msg_len,
        &mut c1_ch_b,
    );
    assert!(
        c1_ch_a.observed_roots.contains(&t2_root_a.to_vec()),
        "C1 native basefold::verify_c1 must absorb post_row_batch_commit before round 0"
    );
    assert_ne!(
        c1_ch_a.sampled_f256, c1_ch_b.sampled_f256,
        "C1 native challenges must bind to post_row_batch_commit at log_batch_size=0"
    );

    let run_c1_trace = |t2_root: [u8; 32]| -> (Vec<F128>, Vec<F256>) {
        let mut b = FieldR1csBuilder::new();
        let mut rec = FsChannelUnionRecorder::new_c1(b"audit-c1-t2");
        let root_e = alloc_flat_digest(&mut b, &initial_root);
        let target_e = ExtExpr::from_base(LinExpr::from_wire(b.alloc_f128(F128::ONE)));
        let proof_e = C1BaseFoldProofTrace::alloc(&mut b, &make_c1_fixture(t2_root), &params);
        let (chals, _) = basefold_verify_c1_trace(
            &mut b, &mut rec, &target_e, &proof_e, &root_e, &params, None,
        );
        let chal_vals = chals
            .iter()
            .map(|c| F256 {
                lo: c.lo.eval(b.values()),
                hi: c.hi.eval(b.values()),
            })
            .collect();
        let recording = rec.finish();
        (recording.data_flat, chal_vals)
    };
    let (c1_trace_data_a, c1_trace_chals_a) = run_c1_trace(t2_root_a);
    let (_, c1_trace_chals_b) = run_c1_trace(t2_root_b);
    assert!(
        c1_trace_data_a
            .windows(2)
            .any(|w| w == t2_lanes_a.as_slice()),
        "C1 recursive trace must absorb post_row_batch_commit lanes before round 0"
    );
    assert_ne!(c1_trace_chals_a, c1_trace_chals_b);
    assert_eq!(c1_trace_chals_a, c1_ch_a.sampled_f256[..log_msg_len]);

    println!(
        "  [CONFIRMED FINDING 1] C0 native (`basefold.rs:1281-1283`) & trace (`self_verify.rs:2292-2297`) \
         absorb {} roots at `log_batch_size=0` (T2 omitted; challenges invariant).\n\
         \x20                       C1 native (`basefold.rs:1915-1917`) & trace (`self_verify.rs:2527-2530`) \
         absorb {} root(s) pre-round-0 (T2 bound; all {} challenges diverge).",
        c0_ch_a.observed_roots.len(),
        c1_ch_a.observed_roots.len(),
        log_msg_len
    );
}

/// Check 2 (Finding 2 & Pillar 2): Audit `ff_merkle` booleanity terms in standalone Walk-B
/// (`MerkleRegionVk` / `merkle_protocol_zero_terms`) vs production outer R1CS boolean slices,
/// plus `root_copy_offset()` handling for `depth < stride` vs `depth == stride`.
fn audit_2_walk_b_ff_merkle_booleanity_and_root_copy_offset() {
    println!("=== [Check 2] Walk-B `ff_merkle` Direction-Bit Booleanity & `root_copy_offset` Audit ===");

    // 2A. Check `root_copy_offset()` across non-power-of-two (`depth < stride`) and
    // power-of-two (`depth == stride`) path depths.
    let iv = [F128::new(0x11, 0x22), F128::new(0x33, 0x44)];
    let fam_non_pow2 = FfMerklePathFamily { depth: 3, n_paths: 1 }; // depth=3 -> stride=4 -> Some(3)
    let fam_pow2 = FfMerklePathFamily { depth: 4, n_paths: 1 }; // depth=4 -> stride=4 -> None
    assert_eq!(fam_non_pow2.stride(), 4);
    assert_eq!(fam_non_pow2.root_copy_offset(), Some(3));
    assert_eq!(fam_pow2.stride(), 4);
    assert_eq!(fam_pow2.root_copy_offset(), None);

    // Verify honest witness generation populates `cr[*][root_copy_offset]` when `depth < stride`.
    let cols_non_pow2 = build_ff_merkle_path_columns(
        &fam_non_pow2,
        iv,
        &[FfMerklePathWitness {
            entry: [F128::new(7, 0), F128::new(9, 0)],
            siblings: vec![[F128::ONE, F128::ZERO]; 3],
            directions: vec![false, true, false],
        }],
        2,
    );
    assert_eq!(
        [cols_non_pow2.cr[0][3], cols_non_pow2.cr[1][3]],
        cols_non_pow2.roots[0],
        "depth < stride must copy root into CR[leg_slot + depth]"
    );

    // 2B. Evaluate `ff_merkle_chain_terms` (included in `merkle_protocol_zero_terms` /
    // `c1_merkle_zero_terms`) vs `ff_merkle_booleanity_terms` (omitted from both) on a
    // 1-level strided path (`w_log = 2`, `depth = 1`, `stride = 2`) with non-boolean `D = 2`.
    let w_log = 2usize;
    let w = 1usize << w_log;
    let l_star = [F128::new(0x1234, 0x5678), F128::new(0x9ABC, 0xDEF0)];
    let r_star = [F128::new(0xCAFE, 0xBABE), F128::new(0xFEED, 0xFACE)];
    let fam_1 = FfMerklePathFamily { depth: 1, n_paths: 1 }; // depth=1, stride=2, root_copy_offset=Some(1)
    let honest_cols = build_ff_merkle_path_columns(
        &fam_1,
        iv,
        &[FfMerklePathWitness {
            entry: l_star,
            siblings: vec![r_star],
            directions: vec![false],
        }],
        w_log,
    );

    // Non-boolean direction witness `D = 2` on the affine line passing through `(L*, R*)`:
    //   CR'  = (1 + D)*L* + D*R*
    //   SIB' = D*L* + (1 + D)*R*
    // In GF(2^128), (1 + D)*CR' + D*SIB' = (1 + D^2 + D^2)*L* = L* and
    // D*CR' + (1 + D)*SIB' = R* identically, preserving the exact Poseidon2b state!
    let d_non_bool = F128::new(2, 0);
    let one_plus_d = F128::ONE + d_non_bool;
    let cr_affine = [
        one_plus_d * l_star[0] + d_non_bool * r_star[0],
        one_plus_d * l_star[1] + d_non_bool * r_star[1],
    ];
    let sib_affine = [
        d_non_bool * l_star[0] + one_plus_d * r_star[0],
        d_non_bool * l_star[1] + one_plus_d * r_star[1],
    ];
    assert_ne!(cr_affine, l_star);
    assert_ne!(cr_affine, r_star);
    for lane in 0..2 {
        assert_eq!(
            one_plus_d * cr_affine[lane] + d_non_bool * sib_affine[lane],
            l_star[lane]
        );
        assert_eq!(
            d_non_bool * cr_affine[lane] + one_plus_d * sib_affine[lane],
            r_star[lane]
        );
    }

    // Build the 9 committed Walk-B columns [C0..C3, CR0, CR1, SIB0, SIB1, D] of width 4.
    let mut committed: Vec<Vec<F128>> = vec![vec![F128::ZERO; w]; MERKLE_REGION_COMMITTED_COLUMNS];
    for lane in 0..4 {
        committed[lane][0] = honest_cols.c[lane][0];
    }
    for lane in 0..2 {
        committed[4 + lane][0] = cr_affine[lane]; // slot 0: non-boolean affine entry
        committed[4 + lane][1] = honest_cols.roots[0][lane]; // slot 1: exact honest root!
        committed[6 + lane][0] = sib_affine[lane];
    }
    committed[8][0] = d_non_bool; // non-boolean D at active node slot 0

    // Validate `MerkleRegionVk::new` accepts `FeedForwardStrided` and evaluate fixed patterns.
    let slices: [WitnessSlice; MERKLE_REGION_COMMITTED_COLUMNS] = std::array::from_fn(|col| {
        WitnessSlice {
            log2_len: w_log,
            index: col,
        }
    });
    let _vk = MerkleRegionVk::new(
        [0x77u8; 32],
        w_log,
        slices,
        w_log,
        vec![MerkleRegionFamily::FeedForwardStrided {
            offset: 0,
            stride: 2,
            depth: 1,
            n_paths: 1,
            iv,
        }],
    )
    .expect("valid FeedForwardStrided Walk-B VK");
    let fixed_patterns = ff_merkle_fixed_patterns(&fam_1, iv);
    let refs = FfMerkleFamilyRefs {
        c: [0, 1, 2, 3],
        cr: [4, 5],
        sib: [6, 7],
        d: 8,
        node: 0,
        nodens: 1,
        start: 2,
        iv: [3, 4],
    };
    let eq_weights = [
        F128::new(11, 1),
        F128::new(23, 2),
        F128::new(37, 3),
        F128::new(41, 4),
    ];
    let lambda = F128::new(0x55AA, 0x1122);

    let chain_eval = eval_terms_on_columns(
        &ff_merkle_chain_terms(&refs, lambda),
        &committed,
        &fixed_patterns,
        &eq_weights,
    );
    let bool_eval = eval_terms_on_columns(
        &ff_merkle_booleanity_terms(&refs),
        &committed,
        &fixed_patterns,
        &eq_weights,
    );
    assert_eq!(
        chain_eval,
        F128::ZERO,
        "Walk-B chain terms vanish identically along the non-boolean affine line"
    );
    assert_ne!(
        bool_eval,
        F128::ZERO,
        "`ff_merkle_booleanity_terms` catches non-boolean D != {{0,1}}"
    );

    // 2C. Verify outer R1CS behavior: unconstrained `alloc_f128` (as in standalone `region_sidecar`
    // callers) accepts `D = 2`, whereas production `HistoryStep` boolean slice allocation rejects it.
    let build_outer = |constrain_d_boolean: bool| -> bool {
        let mut b = FieldR1csBuilder::new();
        for (col_idx, col) in committed.iter().enumerate() {
            for &val in col {
                let w_wire = b.alloc_f128(val);
                if constrain_d_boolean && col_idx == 8 {
                    let expr = LinExpr::from_wire(w_wire);
                    let prod = b.mul(&expr, &expr.add_const(F128::ONE));
                    b.pin_f128(&LinExpr::from_wire(prod), F128::ZERO);
                }
            }
        }
        let (r1cs, z) = b.build();
        r1cs.satisfies(&z)
    };
    assert!(
        build_outer(false),
        "unconstrained `alloc_f128` outer slice permits non-boolean D"
    );
    assert!(
        !build_outer(true),
        "production `HistoryStep` boolean column constraint rejects non-boolean D"
    );

    println!(
        "  [CONFIRMED FINDING 2] `ff_merkle_chain_terms` == 0 on non-boolean D={:?} affine witness, while \
         unused `ff_merkle_booleanity_terms` == {:?} != 0.\n\
         \x20                       Standalone `alloc_f128` outer R1CS satisfies=true; production boolean-pinned \
         outer R1CS satisfies=false. `root_copy_offset` verified (`depth=3 -> Some(3)`, `depth=4 -> None`).",
        d_non_bool, bool_eval
    );
}

/// Check 3 (Finding 3): Audit input-domain range constraints on `pow_nonce` in
/// `FsChannelTrace::verify_pow_trace` (`field_circuit.rs:1847-1860`).
fn audit_3_pow_nonce_range_constraint_domain_gap() {
    println!("=== [Check 3] `verify_pow_trace` `pow_nonce` Upper-64-Bit Input-Domain Audit ===");

    // 3A. At `bits == 0`, `verify_pow_trace` calls `decompose_bits_le(nonce, 0)`, pinning `nonce == 0`.
    {
        let mut b = FieldR1csBuilder::new();
        let mut ch = FsChannelTrace::new_c1(&mut b, b"audit-pow-zero-bits");
        let nonce_wire = b.alloc_f128(F128::ZERO);
        ch.verify_pow(&mut b, &LinExpr::from_wire(nonce_wire), 0);
        let (r1cs, mut z) = b.build();
        assert!(r1cs.satisfies(&z));
        z[nonce_wire.0 as usize] = F128 { lo: 0, hi: 1 };
        assert!(
            !r1cs.satisfies(&z),
            "bits == 0 must reject any non-zero nonce limb"
        );
    }

    // 3B. At `bits == QUERY_GRIND_BITS` (16), measure exact R1CS rows emitted for `verify_pow`
    // on `FsChannelUnionRecorder` (isolating `verify_pow` from inline Poseidon2b permutation rows):
    // 1 challenge wire + (128 - 16) = 112 boolean bit wires + 1 linear reconstruction pin = 114 wires,
    // with ZERO bit-decomposition wires on `nonce`.
    let noncanonical_nonce = F128 {
        lo: 7,
        hi: 0xDEAD_BEEF_CAFE_BABE,
    };
    {
        let mut b = FieldR1csBuilder::new();
        let mut rec = FsChannelUnionRecorder::new_c1(b"audit-pow-16-bits");
        let nonce_wire = b.alloc_f128(noncanonical_nonce);
        let before = b.num_wires();
        rec.verify_pow(&mut b, &LinExpr::from_wire(nonce_wire), QUERY_GRIND_BITS);
        let added_wires = b.num_wires() - before;
        // Expected wires: 1 squeezed `pt` wire + 112 bit wires + 1 zero-pin wire = 114 wires.
        assert_eq!(
            added_wires,
            1 + (128 - QUERY_GRIND_BITS as usize) + 1,
            "verify_pow adds only output challenge bit-decomposition wires and zero input range checks"
        );

        // Contrast with adding a 64-bit `decompose_bits_le` range check on `nonce` (matching `zk_auth_nonce.rs`).
        let mut b_fixed = FieldR1csBuilder::new();
        let nonce_fixed = b_fixed.alloc_f128(noncanonical_nonce);
        b_fixed.decompose_bits_le(&LinExpr::from_wire(nonce_fixed), 64);
        let (r1cs_fixed, z_fixed) = b_fixed.build();
        assert!(
            !r1cs_fixed.satisfies(&z_fixed),
            "64-bit `decompose_bits_le` remediation rejects non-zero `pow_nonce.hi`"
        );
    }

    println!(
        "  [CONFIRMED FINDING 3] At `bits={QUERY_GRIND_BITS}`, `verify_pow` emits exactly {} wires \
         (1 challenge + 112 output bits + 1 sum pin) and 0 range-check rows on `nonce.hi`; \
         64-bit `decompose_bits_le(&nonce, 64)` remediation rejects `hi != 0`.",
        1 + (128 - QUERY_GRIND_BITS as usize) + 1
    );
}

/// Check 4 (Pillar 1 Positive & Negative Control): Full C1 `prove_field_c1` -> native
/// `verify_field_c1_deferred_matrix` + in-circuit `verify_field_c1_trace_deferred_region`
/// lockstep, proof-wire flip rejection, and unused high-bit query-lane pin check.
fn audit_4_c1_self_verify_lockstep_and_query_lane_bits() {
    println!("=== [Check 4] C1 `self_verify` Native-vs-Trace Lockstep & Query-Lane High-Bit Pinning ===");

    let m = 8usize;
    let (inner_r1cs, z) = synthetic_satisfiable(m, m, 0xC1A0_D175);
    let shape = FieldShape::of(&inner_r1cs);
    let digest = inner_r1cs.statement_digest();
    let params = PcsParams {
        m: m + LOG_PACKING,
        log_inv_rate: 2,
        log_batch_size: 2,
        profile: Default::default(),
    };

    let mut prover_ch = FsLaneChallenger::new_c1(b"audit-c1-self-verify");
    let (proof, commitment, native_claim) =
        prove_field_c1(&inner_r1cs, &z, &params, &mut prover_ch);

    // Note: `verify_field_c1_trace_deferred_region` verifies without public IO (`prove_field_c1`).
    // Also verify native `verify_field_c1` accepts and matches the trace claim!
    let mut native_ch = FsLaneChallenger::new_c1(b"audit-c1-self-verify");
    let verified_native = noid_ivc_core::verifier::verify_field_c1(
        &inner_r1cs,
        &commitment,
        &proof,
        &mut native_ch,
    )
    .expect("honest C1 proof must verify natively");
    assert_eq!(verified_native.ab.value, native_claim.ab.value);
    assert_eq!(verified_native.c.value, native_claim.c.value);

    // Build recursive trace for `verify_field_c1_trace_deferred_region`.
    let mut b = FieldR1csBuilder::new();
    let mut trace_ch = FsChannelTrace::new_c1(&mut b, b"audit-c1-self-verify");
    let digest_e = alloc_flat_digest(&mut b, &digest);
    let root_e = alloc_flat_digest(&mut b, &commitment.root);
    let proof_wires_start = b.num_wires();
    let proof_e = C1FieldR1csProofTrace::alloc_shape_mode(&mut b, &proof, &shape, &params, true);
    let proof_wires_end = b.num_wires();

    let (trace_claim, trace_fresh) = verify_field_c1_trace_deferred_region(
        &mut b,
        &mut trace_ch,
        &shape,
        &params,
        &digest_e,
        &root_e,
        &proof_e,
        None,
    );

    // Verify post-verification Fiat-Shamir sponge lockstep between native and trace!
    let post_native = native_ch.sample_f256();
    let post_trace = trace_ch.sample_f256(&mut b);
    assert_eq!(post_trace.lo.eval(b.values()), post_native.lo);
    assert_eq!(post_trace.hi.eval(b.values()), post_native.hi);

    // Verify exact agreement on returned `ab` and `c` evaluation claims.
    assert_eq!(trace_claim.ab.value.lo.eval(b.values()), native_claim.ab.value.lo);
    assert_eq!(trace_claim.ab.value.hi.eval(b.values()), native_claim.ab.value.hi);
    assert_eq!(trace_claim.c.value.lo.eval(b.values()), native_claim.c.value.lo);
    assert_eq!(trace_claim.c.value.hi.eval(b.values()), native_claim.c.value.hi);
    assert_eq!(trace_fresh.x_inner_rest.len(), shape.k_log - shape.k_skip);

    let (outer_r1cs, outer_z) = b.build();
    assert!(
        outer_r1cs.satisfies(&outer_z),
        "honest C1 deferred self-verify trace must satisfy all R1CS constraints"
    );

    // Negative mutation checks across proof segments (zerocheck, lincheck, PCS round messages).
    for &wire_idx in &[
        proof_wires_start,
        proof_wires_start + 5,
        (proof_wires_start + proof_wires_end) / 2,
    ] {
        let mut mutated = outer_z.clone();
        mutated[wire_idx] += F128::ONE;
        assert!(
            !outer_r1cs.satisfies(&mutated),
            "mutating proof wire {wire_idx} must violate outer R1CS"
        );
    }

    // Dedicated check on `verify_opening_batch_quirky_direct_c1_trace` returned `query_bit_ranges`:
    // `k_code = (8 - 2) + 2 = 8`, `per_lane = 128 / 8 = 16`, `n_queries = 133` -> last lane uses
    // `133 % 16 = 5` windows (`5 * 8 = 40` used bits) and `88` unused high bits (`40..128`).
    // Mutating an unused high bit wire MUST fail `r1cs.satisfies`!
    {
        let mut b_q = FieldR1csBuilder::new();
        let mut rec_q = FsChannelUnionRecorder::new_c1(b"audit-query-high-bits");
        let root_q = alloc_flat_digest(&mut b_q, &commitment.root);
        let pcs_q = C1BaseFoldProofTrace::alloc(&mut b_q, &proof.pcs_open, &params);
        let dummy_claim = C1QuirkyDirectClaimTrace {
            z_skip: ExtExpr::zero(),
            k_skip: shape.k_skip,
            x_rest: vec![ExtExpr::zero(); shape.m - shape.k_skip],
            value: ExtExpr::zero(),
        };
        let ranges = verify_opening_batch_quirky_direct_c1_trace(
            &mut b_q,
            &mut rec_q,
            &root_q,
            &[dummy_claim],
            &pcs_q,
            &params,
        );
        let last_range = ranges.last().expect("at least one query lane").clone();
        assert_eq!(
            last_range.len(),
            128,
            "every query lane decomposes all 128 flat bits"
        );
        // Check that flipping the highest unused bit (`start + 127`) violates the lane-sum pin
        // or booleanity even when no Merkle path consumes that bit!
        let (q_r1cs, _) = b_q.build();
        // Inspect the boolean row on `last_range.end - 1`: `a_row` and `b_row` enforce `bit * (bit + 1) = 0`
        // or `bit * bit = bit`, and the subsequent pin row references `last_range.end - 1`!
        let high_bit_col = (last_range.end - 1) as u32;
        let referenced_in_matrix = q_r1cs.a_0.col_indices.contains(&high_bit_col)
            || q_r1cs.b_0.col_indices.contains(&high_bit_col);
        assert!(
            referenced_in_matrix,
            "unused high bit of squeezed query lane must be constrained in R1CS matrix"
        );
    }

    println!(
        "  [VERIFIED TIGHT] C1 `verify_field_c1_trace_deferred_region` ({} R1CS wires) matches native \
         transcript & claims bit-for-bit; proof-wire flips rejected; all 128 bits/lane constrained.",
        outer_r1cs.useful_rows
    );
}

/// Check 5 (Pillar 1 & 2 Algebraic Invariant): Verify `F256::from_raw_challenge_lanes` and
/// `FieldR1csBuilder::c1_challenge_from_raw` never land in `GF(2^128)` (so `c != 0, 1` always).
fn audit_5_f256_challenge_subfield_exclusion() {
    println!("=== [Check 5] C1 `F256::from_raw_challenge_lanes` Trace-One Subfield Exclusion ===");

    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let mut next_u64 = || -> u64 {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };

    let mut samples = vec![
        (F128::ZERO, F128::ZERO),
        (F128::ONE, F128::ZERO),
        (F128::ZERO, F128::ONE),
        (F128::ONE, F128::ONE),
        (F128::new(u64::MAX, u64::MAX), F128::new(u64::MAX, u64::MAX)),
    ];
    for _ in 0..1024 {
        samples.push((
            F128::new(next_u64(), next_u64()),
            F128::new(next_u64(), next_u64()),
        ));
    }

    let mut b = FieldR1csBuilder::new();
    for &(lo, raw_hi) in &samples {
        let native = F256::from_raw_challenge_lanes(lo, raw_hi);
        assert_ne!(
            native.hi,
            F128::ZERO,
            "Artin-Schreier image y^2 + y + EXTENSION_TAU must never vanish over GF(2^128)"
        );
        assert!(!native.is_zero());
        assert_ne!(native, F256::ONE);

        let lo_e = LinExpr::from_wire(b.alloc_f128(lo));
        let hi_e = LinExpr::from_wire(b.alloc_f128(raw_hi));
        let trace = b.c1_challenge_from_raw(lo_e, hi_e);
        assert_eq!(trace.lo.eval(b.values()), native.lo);
        assert_eq!(trace.hi.eval(b.values()), native.hi);
    }
    let (r1cs, z) = b.build();
    assert!(r1cs.satisfies(&z));

    println!(
        "  [VERIFIED TIGHT] Checked {} raw `(lo, raw_hi)` pairs: `hi = y^2 + y + tau != 0` always; \
         native & R1CS gadget match bit-for-bit.",
        samples.len()
    );
}

/// Check 6 (Pillar 2 Positive & Boundary Control): Verify `ShiftDischarge` (C1 native & trace)
/// enforces non-wrapping `w = 0` boundary semantics (`shifted[0] = 0`, never `col[w - 1]`).
fn audit_6_c1_shift_discharge_boundary_non_wrapping() {
    println!("=== [Check 6] C1 `ShiftDischarge` Non-Wrapping `slot = 0` Boundary Audit ===");

    let w_log = 4usize;
    let width = 1usize << w_log;
    let mut column = vec![F128::ZERO; width];
    for (i, slot) in column.iter_mut().enumerate() {
        *slot = F128::new((i as u64 + 1) * 17, 0);
    }
    // Put a large sentinel at `column[width - 1]` to detect any cyclic wrap into `shifted[0]`.
    column[width - 1] = F128::new(0xCAFE_BABE_DEAD_BEEF, 0x1337_4242_9999_8888);

    let mut setup_ch = FsLaneChallenger::new_c1(b"audit-shift-sigma");
    let sigma = setup_ch.sample_f256_vec(w_log);

    // Compute `eq(sigma, row)` for `row in 0..width`.
    let eq_at = |point: &[F256], row: usize| -> F256 {
        point
            .iter()
            .enumerate()
            .fold(F256::ONE, |acc, (bit, &coord)| {
                let b_val = if (row >> bit) & 1 == 1 {
                    F256::ONE
                } else {
                    F256::ZERO
                };
                acc * (F256::ONE + coord + b_val)
            })
    };

    // True non-wrapping shifted MLE: `sum_{row=1..width} eq(sigma, row) * column[row - 1]`.
    let non_wrapping_target = (1..width).fold(F256::ZERO, |acc, row| {
        acc + eq_at(&sigma, row).scale_base(column[row - 1])
    });
    let cyclic_wrapped_target =
        non_wrapping_target + eq_at(&sigma, 0).scale_base(column[width - 1]);
    assert_ne!(non_wrapping_target, cyclic_wrapped_target);

    // Also verify kernel identity directly: `shift_kernel_eval(sigma, point_zero) == 0` when `rho = 0`?
    // More precisely, `shift_kernel(rho, sigma) = sum_{j=0..w-2} eq(rho, j+1) * eq(sigma, j)`.
    // At `rho = (0, 0, ..., 0)` (index 0), `shift_kernel_eval(0, sigma)` MUST be identically ZERO!
    let zero_rho = vec![F256::ZERO; w_log];
    assert_eq!(
        shift_kernel_eval(&zero_rho, &sigma),
        F256::ZERO,
        "shift kernel at rho = 0^w_log must be identically zero (no wrap-around into slot 0)"
    );

    let mut prove_ch = FsLaneChallenger::new_c1(b"audit-shift-discharge");
    let (proof, native_point) =
        prove_shift_discharge(&column, &sigma, non_wrapping_target, &mut prove_ch);

    let mut verify_ch = FsLaneChallenger::new_c1(b"audit-shift-discharge");
    let verified_point =
        verify_shift_discharge(w_log, &sigma, non_wrapping_target, &proof, &mut verify_ch)
            .expect("honest non-wrapping shift discharge must verify");
    assert_eq!(native_point, verified_point);

    // Verify cyclic wrap-around target is rejected natively.
    let mut bad_ch = FsLaneChallenger::new_c1(b"audit-shift-discharge");
    assert!(
        verify_shift_discharge(w_log, &sigma, cyclic_wrapped_target, &proof, &mut bad_ch).is_err(),
        "cyclic wrap-around target must be rejected by verify_shift_discharge"
    );

    // Verify in-circuit `verify_c1_shift_discharge_trace` lockstep & rejection of wrapped target.
    let build_shift_trace = |claimed_target: F256| -> (bool, Vec<F256>) {
        let mut b = FieldR1csBuilder::new();
        let mut ch = FsChannelTrace::new_c1(&mut b, b"audit-shift-discharge");
        let sigma_e: Vec<ExtExpr> = sigma
            .iter()
            .map(|&v| {
                ExtExpr::new(
                    LinExpr::from_wire(b.alloc_f128(v.lo)),
                    LinExpr::from_wire(b.alloc_f128(v.hi)),
                )
            })
            .collect();
        let target_e = ExtExpr::new(
            LinExpr::from_wire(b.alloc_f128(claimed_target.lo)),
            LinExpr::from_wire(b.alloc_f128(claimed_target.hi)),
        );
        let proof_e = C1ShiftDischargeProofTrace::alloc(&mut b, &proof, w_log);
        let pt_e = verify_c1_shift_discharge_trace(
            &mut b, &mut ch, w_log, &sigma_e, &target_e, 0, &proof_e,
        );
        let pt_vals = pt_e
            .iter()
            .map(|e| F256 {
                lo: e.lo.eval(b.values()),
                hi: e.hi.eval(b.values()),
            })
            .collect();
        let (r1cs, z) = b.build();
        (r1cs.satisfies(&z), pt_vals)
    };

    let (ok_honest, trace_pt) = build_shift_trace(non_wrapping_target);
    assert!(ok_honest, "non-wrapping shift claim must satisfy R1CS trace");
    assert_eq!(trace_pt, verified_point);

    let (ok_wrapped, _) = build_shift_trace(cyclic_wrapped_target);
    assert!(
        !ok_wrapped,
        "cyclic wrap-around shift claim must fail R1CS trace"
    );

    println!(
        "  [VERIFIED TIGHT] `shift_kernel_eval(0^w, sigma) == 0`; non-wrapping target accepted & \
         locked to native point; cyclic `col[w-1] -> slot 0` target rejected in both native & R1CS."
    );
}

/// Check 7 (Pillar 3 Positive & Negative Controls): Audit `verify_matrix_claim_fold_c1_trace`
/// for both `gate = 0` (base case: ignores bogus incoming claim) and `gate = 1` (step case:
/// binds both fresh and incoming claims), plus `FieldR1csBuilder::pin_boolean_gated_zero`.
fn audit_7_c1_matrix_fold_and_boolean_gated_pin() {
    println!("=== [Check 7] C1 `matrix_fold` (`gate=0` Base / `gate=1` Step) & `pin_boolean_gated_zero` ===");

    let m = 7usize;
    let (inner_r1cs, z) = synthetic_satisfiable(m, m, 0xF01D_C177);
    let shape = FieldShape::of(&inner_r1cs);
    let params = PcsParams {
        m: m + LOG_PACKING,
        log_inv_rate: 2,
        log_batch_size: 2,
        profile: Default::default(),
    };

    let mut prove_ch = FsLaneChallenger::new_c1(b"audit-fold-source");
    let (_proof, _commitment, _claim, capture) =
        prove_field_c1_capturing_fresh(&inner_r1cs, &z, &params, &mut prove_ch);
    let fresh_native = capture.into_fresh_claim();

    // 7A. Base step (`gate = false` / `0`): even if `incoming` has bogus value, `gate = 0`
    // zeroes `gamma_gate` and `delta_gate`, producing a valid folded claim on `inner_r1cs`.
    let mut bogus_incoming = C1MatrixAccClaim::zero(shape.k_log);
    bogus_incoming.value = F256::from_base(F128::new(0xBAD0_BAD0, 0xDEAD_1234));

    let mut fold_p0 = FsLaneChallenger::new_c1(b"audit-fold-base");
    let (fold_proof_0, acc_0) = prove_matrix_claim_fold_c1(
        &inner_r1cs,
        &fresh_native,
        &bogus_incoming,
        false,
        &mut fold_p0,
    );
    let mut fold_v0 = FsLaneChallenger::new_c1(b"audit-fold-base");
    let verified_0 = verify_matrix_claim_fold_c1(
        shape.k_log,
        shape.k_skip,
        &fresh_native,
        &bogus_incoming,
        F256::ZERO,
        &fold_proof_0,
        &mut fold_v0,
    )
    .expect("native base fold verify");
    assert_eq!(acc_0, verified_0);
    assert_eq!(
        stacked_matrix_mle_eval_c1(&inner_r1cs, &acc_0),
        acc_0.value,
        "base-step folded accumulator must equal true stacked matrix MLE"
    );

    // 7B. Recursive step (`gate = true` / `1`): fold `fresh_native` with valid `acc_0`!
    let mut fold_p1 = FsLaneChallenger::new_c1(b"audit-fold-step");
    let (fold_proof_1, acc_1) = prove_matrix_claim_fold_c1(
        &inner_r1cs,
        &fresh_native,
        &acc_0,
        true,
        &mut fold_p1,
    );
    assert_eq!(
        stacked_matrix_mle_eval_c1(&inner_r1cs, &acc_1),
        acc_1.value,
        "step-folded accumulator must equal true stacked matrix MLE"
    );

    // Replay step fold in R1CS via `verify_matrix_claim_fold_c1_trace` and test witness mutations!
    let mut b = FieldR1csBuilder::new();
    let mut ch = FsChannelTrace::new_c1(&mut b, b"audit-fold-step");
    let alloc_ext = |b: &mut FieldR1csBuilder, v: F256| {
        ExtExpr::new(
            LinExpr::from_wire(b.alloc_f128(v.lo)),
            LinExpr::from_wire(b.alloc_f128(v.hi)),
        )
    };
    let fresh_trace = noid_recursive::acceptance::trace::matrix_fold::C1FreshLincheckClaimTrace {
        alpha: alloc_ext(&mut b, fresh_native.alpha),
        z_skip: alloc_ext(&mut b, fresh_native.z_skip),
        x_inner_rest: fresh_native
            .x_inner_rest
            .iter()
            .map(|&v| alloc_ext(&mut b, v))
            .collect(),
        r_inner_rest: fresh_native
            .r_inner_rest
            .iter()
            .map(|&v| alloc_ext(&mut b, v))
            .collect(),
        z_partial: fresh_native
            .z_partial
            .iter()
            .map(|&v| alloc_ext(&mut b, v))
            .collect(),
        value: alloc_ext(&mut b, fresh_native.value),
    };
    let incoming_val_lo_wire_idx = b.num_wires() + 2 * (2 * shape.k_log + 1);
    let incoming_trace = C1MatrixAccClaimTrace::alloc(&mut b, &acc_0);
    let gate_wire = b.alloc_bool(true);
    let fold_trace = C1MatrixFoldProofTrace::alloc(&mut b, &fold_proof_1, shape.k_log);

    let out_acc = verify_matrix_claim_fold_c1_trace(
        &mut b,
        &mut ch,
        shape.k_log,
        shape.k_skip,
        &fresh_trace,
        &incoming_trace,
        &LinExpr::from_wire(gate_wire),
        &fold_trace,
    );
    assert_eq!(out_acc.value.lo.eval(b.values()), acc_1.value.lo);
    assert_eq!(out_acc.value.hi.eval(b.values()), acc_1.value.hi);

    let (fold_r1cs, fold_z) = b.build();
    assert!(fold_r1cs.satisfies(&fold_z));

    // Mutating `incoming.value.lo` when `gate = 1` MUST violate the fold R1CS!
    let mut bad_incoming_z = fold_z.clone();
    bad_incoming_z[incoming_val_lo_wire_idx] += F128::ONE;
    assert!(
        !fold_r1cs.satisfies(&bad_incoming_z),
        "mutating incoming accumulator value at gate=1 must fail R1CS"
    );

    // 7C. Exhaustively check `pin_boolean_gated_zero` truth table: `w = g * (e + w)`.
    for &(gate_bool, expr_val, expect_sat) in &[
        (false, F128::ZERO, true),
        (false, F128::new(99, 77), true),
        (true, F128::ZERO, true),
        (true, F128::new(99, 77), false),
    ] {
        let mut bg = FieldR1csBuilder::new();
        let g = LinExpr::from_wire(bg.alloc_bool(gate_bool));
        let e = LinExpr::from_wire(bg.alloc_f128(expr_val));
        bg.pin_boolean_gated_zero(&g, &e);
        let pin_wire_idx = bg.num_wires() - 1;
        let (gr1cs, mut gz) = bg.build();
        assert_eq!(gr1cs.satisfies(&gz), expect_sat);
        if !expect_sat {
            // Even if a dishonest prover alters the auxiliary wire `w`, `w = 1 * (e + w)` => `e = 0` has NO solution!
            for trial in [F128::ZERO, F128::ONE, expr_val, F128::new(12345, 67890)] {
                gz[pin_wire_idx] = trial;
                assert!(
                    !gr1cs.satisfies(&gz),
                    "pin_boolean_gated_zero at gate=1, expr!=0 must have no satisfying witness `w`"
                );
            }
        }
    }

    println!(
        "  [VERIFIED TIGHT] C1 `matrix_fold` verified for `gate=0` (base) and `gate=1` (step, {} wires); \
         incoming claim mutation rejected; `pin_boolean_gated_zero` algebraically sound.",
        fold_r1cs.useful_rows
    );
}

fn main() {
    println!("================================================================================");
    println!(" parano1d Recursive Verifier & IVC Fold Constraint Audit (`52938433a30b...`)");
    println!("================================================================================");
    audit_1_c0_vs_c1_basefold_t2_transcript_binding();
    audit_2_walk_b_ff_merkle_booleanity_and_root_copy_offset();
    audit_3_pow_nonce_range_constraint_domain_gap();
    audit_4_c1_self_verify_lockstep_and_query_lane_bits();
    audit_5_f256_challenge_subfield_exclusion();
    audit_6_c1_shift_discharge_boundary_non_wrapping();
    audit_7_c1_matrix_fold_and_boolean_gated_pin();
    println!("================================================================================");
    println!(" ALL 7 AUDIT SUITES COMPLETED DETERMINISTICALLY (EXIT 0)");
    println!("================================================================================");
}
