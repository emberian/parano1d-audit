//! Independent verification of the runtime half of the agent-25 HIGH finding.
//!
//! Builds two valid same-shape, same-canonical-length relations with different
//! structural statement digests (one coefficient differs). Then:
//!
//!  1. control: the safe loader `open_packed` must REJECT the foreign relation
//!     under the approved digest (this is the check that would catch the
//!     substitution if the runtime performed it);
//!  2. `open_build_authenticated` — the exact decoder `rebuild_from_canonical`
//!     applies to the binary's EMBEDDED legacy matrix bytes
//!     (noid_miner/src/history_step_artifacts.rs) — must be tested with the
//!     foreign canonical bytes under the approved seal;
//!  3. `open_build_authenticated_packed_image` — the decoder applied to the
//!     runtime CACHE image — same test.

use noid_ivc_core::{
    field::F128,
    field_r1cs::{BuildAuthenticatedFieldR1csSeal, CompactFieldR1cs, FieldR1cs, SparseFieldMatrix},
    proof::FieldShape,
};

fn fixture(coefficient: F128) -> (FieldShape, [u8; 32], Vec<u8>, Vec<u8>) {
    let k_log = 4;
    let k = 1usize << k_log;
    let mut rows = vec![Vec::new(); k];
    rows[0] = vec![(0, coefficient)];
    let relation = FieldR1cs {
        m: k_log,
        k_log,
        k_skip: 2,
        useful_rows: 1,
        a_0: SparseFieldMatrix::from_rows(k, rows),
        b_0: SparseFieldMatrix::zero(k),
        const_pin: Some(0),
        digest_cache: std::sync::OnceLock::new(),
        csc_cache: std::sync::OnceLock::new(),
    };
    let shape = FieldShape::of(&relation);
    let digest = relation.structural_statement_digest();
    let mut canonical = Vec::new();
    relation.write_artifact(&mut canonical).unwrap();
    let packed =
        CompactFieldR1cs::open_packed(canonical.clone().into_boxed_slice(), shape, digest).unwrap();
    let image = packed.encode_startup_packed_image().unwrap();
    (shape, digest, canonical, image.into_vec())
}

fn main() {
    let (shape, approved_digest, approved_canonical, approved_image) = fixture(F128::ONE);
    let (foreign_shape, foreign_digest, foreign_canonical, foreign_image) = fixture(F128::new(2, 0));
    assert_eq!(shape, foreign_shape);
    assert_ne!(approved_digest, foreign_digest);
    assert_eq!(approved_canonical.len(), foreign_canonical.len());
    assert_eq!(approved_image.len(), foreign_image.len());

    // 1. Control: the safe loader rejects the foreign relation.
    let safe_rejected = CompactFieldR1cs::open_packed(
        foreign_canonical.clone().into_boxed_slice(),
        shape,
        approved_digest,
    )
    .is_err();

    // 2. Embedded-bytes decoder (used by rebuild_from_canonical in the node).
    let approved_seal = unsafe {
        BuildAuthenticatedFieldR1csSeal::from_release_build(
            shape,
            approved_digest,
            approved_canonical.len(),
        )
    };
    let embedded = unsafe {
        CompactFieldR1cs::open_build_authenticated(
            foreign_canonical.clone().into_boxed_slice(),
            approved_seal,
        )
    };
    let embedded_accepted = matches!(
        embedded.as_ref(),
        Ok(relation) if relation.statement_digest() == approved_digest
            && relation.artifact_bytes().as_ref() == foreign_canonical.as_slice()
    );

    // 3. Packed runtime-cache decoder (used by load_cached_image in the node).
    let cached = unsafe {
        CompactFieldR1cs::open_build_authenticated_packed_image(&foreign_image, approved_seal)
    };
    let cache_accepted = matches!(
        cached.as_ref(),
        Ok(relation) if relation.statement_digest() == approved_digest
    );

    println!(
        "safe_loader_rejected={safe_rejected} embedded_decoder_foreign_accepted={embedded_accepted} cache_decoder_foreign_accepted={cache_accepted} approved_digest={approved_digest:02x?} foreign_digest={foreign_digest:02x?} canonical_len={}",
        approved_canonical.len()
    );
    if let Err(error) = embedded.as_ref() {
        println!("embedded_decoder_error={error:?}");
    }
    if let Err(error) = cached.as_ref() {
        println!("cache_decoder_error={error:?}");
    }
    if !(safe_rejected && embedded_accepted && cache_accepted) {
        std::process::exit(1);
    }
}
