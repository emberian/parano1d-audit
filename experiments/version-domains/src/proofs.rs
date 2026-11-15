use noid_ivc_core::challenger::FsLaneChallenger;
use noid_ivc_core::field::{F128, F256};
use noid_ivc_core::field_r1cs::{CompactFieldR1cs, FieldR1cs, SparseFieldMatrix};
use noid_ivc_core::matrix_claim::c1::{
    fresh_claim_value_c1, prove_matrix_claim_fold_c1, stacked_matrix_mle_eval_c1,
    verify_matrix_claim_fold_c1, C1FreshLincheckClaim, C1MatrixAccClaim,
};
use noid_ivc_core::matrix_claim::sparse_c1::{
    SparseEvaluationBudget, SparseMatrixEvaluationKey, SparseMatrixProver,
};
use noid_ivc_core::proof::FieldShape;
use serde_json::json;
use std::sync::OnceLock;

fn wide(seed: u64) -> F256 {
    F256::new(
        F128::new(seed.wrapping_mul(7919), seed.rotate_left(17)),
        F128::new(seed ^ 0xc1, !seed),
    )
}

fn matrix(k: usize, salt: u64) -> FieldR1cs {
    let width = 1usize << k;
    let rows = |side: usize| {
        (0..width)
            .map(|row| {
                if row % 3 == 1 {
                    Vec::new()
                } else {
                    (0..1 + (row + side) % 4)
                        .map(|j| {
                            (
                                ((row * 5 + j * 3 + side) % width) as u32,
                                F128::new(
                                    (row + j + 1) as u64 + salt * (side as u64 + 1),
                                    (side + j) as u64 + salt,
                                ),
                            )
                        })
                        .collect()
                }
            })
            .collect()
    };
    FieldR1cs {
        m: k,
        k_log: k,
        k_skip: k.min(2),
        useful_rows: width,
        a_0: SparseFieldMatrix::from_rows(width, rows(0)),
        b_0: SparseFieldMatrix::from_rows(width, rows(1)),
        const_pin: Some(0),
        digest_cache: OnceLock::new(),
        csc_cache: OnceLock::new(),
    }
}

fn claim(matrix: &FieldR1cs) -> C1MatrixAccClaim {
    let point = (0..2 * matrix.k_log + 1)
        .map(|index| wide(30 + index as u64))
        .collect();
    let mut claim = C1MatrixAccClaim {
        point,
        value: F256::ZERO,
    };
    claim.value = stacked_matrix_mle_eval_c1(matrix, &claim);
    claim
}

fn fresh(matrix: &FieldR1cs) -> C1FreshLincheckClaim {
    let rest = matrix.k_log - matrix.k_skip;
    let mut fresh = C1FreshLincheckClaim {
        alpha: wide(101),
        z_skip: wide(102),
        x_inner_rest: (0..rest).map(|i| wide(110 + i as u64)).collect(),
        r_inner_rest: (0..rest).map(|i| wide(120 + i as u64)).collect(),
        z_partial: (0..1usize << matrix.k_skip)
            .map(|i| wide(130 + i as u64))
            .collect(),
        value: F256::ZERO,
    };
    fresh.value = fresh_claim_value_c1(matrix, &fresh);
    fresh
}

fn budget() -> SparseEvaluationBudget {
    SparseEvaluationBudget {
        max_padded_entries: 1 << 16,
        max_planned_bytes: 64 << 20,
    }
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let first = matrix(3, 0);
    let second = matrix(3, 1);
    assert_eq!(FieldShape::of(&first), FieldShape::of(&second));
    assert_ne!(
        first.structural_statement_digest(),
        second.structural_statement_digest()
    );

    let claim = claim(&first);
    let first_prover =
        SparseMatrixProver::from_resident(&first, first.structural_statement_digest(), budget())?;
    let second_prover =
        SparseMatrixProver::from_resident(&second, second.structural_statement_digest(), budget())?;
    let proof = first_prover.prove([0x21; 32], &claim, budget())?;
    let sparse_original = first_prover.key().verify([0x21; 32], &claim, &proof);
    let sparse_other_context = first_prover.key().verify([0x22; 32], &claim, &proof);
    let sparse_other_key = second_prover.key().verify([0x21; 32], &claim, &proof);
    let key_bytes = first_prover.key().to_bytes();
    let stale_key_pin =
        SparseMatrixEvaluationKey::from_bytes_pinned(&key_bytes, second_prover.key().digest());
    let correct_key_pin =
        SparseMatrixEvaluationKey::from_bytes_pinned(&key_bytes, first_prover.key().digest());
    assert!(sparse_original.is_ok());
    assert!(sparse_other_context.is_err());
    assert!(sparse_other_key.is_err());
    assert!(stale_key_pin.is_err());
    assert!(correct_key_pin.is_ok());

    let first_fresh = fresh(&first);
    let incoming = C1MatrixAccClaim::zero(first.k_log);
    let (fold, _) = prove_matrix_claim_fold_c1(
        &first,
        &first_fresh,
        &incoming,
        false,
        &mut FsLaneChallenger::new_c1(b"VERSION-DOMAINS/MATRIX-FOLD"),
    );
    let outgoing = verify_matrix_claim_fold_c1(
        first.k_log,
        first.k_skip,
        &first_fresh,
        &incoming,
        F256::ZERO,
        &fold,
        &mut FsLaneChallenger::new_c1(b"VERSION-DOMAINS/MATRIX-FOLD"),
    )
    .expect("honest matrix fold");
    let fold_matches_first = stacked_matrix_mle_eval_c1(&first, &outgoing) == outgoing.value;
    let fold_matches_second = stacked_matrix_mle_eval_c1(&second, &outgoing) == outgoing.value;
    let mut second_fresh = fresh(&second);
    second_fresh.x_inner_rest = first_fresh.x_inner_rest.clone();
    second_fresh.r_inner_rest = first_fresh.r_inner_rest.clone();
    second_fresh.z_partial = first_fresh.z_partial.clone();
    second_fresh.alpha = first_fresh.alpha;
    second_fresh.z_skip = first_fresh.z_skip;
    let fold_relabelled_to_second = verify_matrix_claim_fold_c1(
        second.k_log,
        second.k_skip,
        &second_fresh,
        &incoming,
        F256::ZERO,
        &fold,
        &mut FsLaneChallenger::new_c1(b"VERSION-DOMAINS/MATRIX-FOLD"),
    );
    assert!(fold_matches_first);
    assert!(!fold_matches_second);
    assert!(fold_relabelled_to_second.is_err());

    let mut canonical = Vec::new();
    first.write_artifact(&mut canonical)?;
    let safe_stale_artifact = CompactFieldR1cs::open(
        canonical.clone().into_boxed_slice(),
        FieldShape::of(&first),
        second.structural_statement_digest(),
    );
    let safe_correct_artifact = CompactFieldR1cs::open(
        canonical.into_boxed_slice(),
        FieldShape::of(&first),
        first.structural_statement_digest(),
    );
    assert!(safe_stale_artifact.is_err());
    assert!(safe_correct_artifact.is_ok());

    let output = json!({
        "check": "sparse-matrix-and-artifact-binding",
        "first_matrix_digest": crate::hex(&first.structural_statement_digest()),
        "second_matrix_digest": crate::hex(&second.structural_statement_digest()),
        "sparse_original": sparse_original.is_ok(),
        "sparse_other_context": sparse_other_context.is_ok(),
        "sparse_other_context_error": format!("{:?}", sparse_other_context.unwrap_err()),
        "sparse_other_key": sparse_other_key.is_ok(),
        "sparse_other_key_error": format!("{:?}", sparse_other_key.unwrap_err()),
        "stale_sparse_key_pin_accepted": stale_key_pin.is_ok(),
        "correct_sparse_key_pin_accepted": correct_key_pin.is_ok(),
        "matrix_fold_reduction_valid": true,
        "matrix_fold_matches_first": fold_matches_first,
        "matrix_fold_matches_second": fold_matches_second,
        "matrix_fold_relabelled_to_second": fold_relabelled_to_second.is_ok(),
        "safe_artifact_with_stale_digest_accepted": safe_stale_artifact.is_ok(),
        "safe_artifact_with_stale_digest_error": format!("{:?}", safe_stale_artifact.unwrap_err()),
        "safe_artifact_with_correct_digest_accepted": safe_correct_artifact.is_ok(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
