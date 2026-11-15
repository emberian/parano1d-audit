use std::time::Instant;

use capsule_chains::{
    aligned_digest, build_proof, build_tree, find_aligned_collision, hash_output_hex, mixed_leaf,
    tree_layer, verify_proof,
};
use noid_fri_binius::capsule::capsule_leaf_hash_mixed;
use serde_json::json;

fn main() {
    let regenerate = std::env::args().any(|arg| arg == "--regenerate");
    let started = Instant::now();
    let collision = if regenerate {
        find_aligned_collision(1_000_000)
    } else {
        capsule_chains::known_collision()
    };
    let (a0, b0) = collision.first;
    let (a1, b1) = collision.second;
    assert_ne!((a0, b0), (a1, b1));
    assert_eq!(collision.digest, capsule_chains::node_compress(&a0, &b0));
    assert_eq!(collision.digest, capsule_chains::node_compress(&a1, &b1));

    const DEPTH: usize = noid_fri_binius::zk_capsule_pcs::ZK_CAPSULE_PCS_SOURCE_TREE_DEPTH;
    const CAP_DEPTH: usize = noid_fri_binius::zk_capsule_pcs::ZK_CAPSULE_PCS_SOURCE_CAP_DEPTH;
    let filler = |index: usize| {
        let mut digest = [0u8; 32];
        digest[..8].copy_from_slice(&(index as u64 + 0xca95).to_le_bytes());
        digest[16..24].copy_from_slice(&(index as u64 + 0xca950001).to_le_bytes());
        digest
    };
    let mut leaves0 = (0..1usize << DEPTH).map(filler).collect::<Vec<_>>();
    let mut leaves1 = leaves0.clone();
    leaves0[0] = a0;
    leaves0[1] = b0;
    leaves1[0] = a1;
    leaves1[1] = b1;
    let tree0 = build_tree(leaves0.clone());
    let tree1 = build_tree(leaves1.clone());
    let cap0 = tree_layer(&tree0, CAP_DEPTH);
    let cap1 = tree_layer(&tree1, CAP_DEPTH);
    assert_eq!(cap0, cap1);
    assert_ne!(leaves0, leaves1);

    let proof0 = build_proof(&tree0, &[0], DEPTH, CAP_DEPTH);
    let proof1 = build_proof(&tree1, &[0], DEPTH, CAP_DEPTH);
    assert_ne!(proof0.siblings, proof1.siblings);
    verify_proof(&cap0, CAP_DEPTH, &proof0, DEPTH, &[0], &[a0]).unwrap();
    verify_proof(&cap0, CAP_DEPTH, &proof1, DEPTH, &[0], &[a1]).unwrap();

    let mut rng = 0xca95_4d49_0000_0001u64;
    let real_leaf = mixed_leaf(&mut rng, true);
    let real_digest = capsule_leaf_hash_mixed(&real_leaf);
    let reachability_error =
        verify_proof(&cap0, CAP_DEPTH, &proof0, DEPTH, &[0], &[real_digest]).unwrap_err();

    let all_collision_inputs_aligned = [a0, a1].iter().all(aligned_digest)
        && [b0, b1].iter().all(capsule_chains::aligned_node_right);
    println!(
        "{}",
        json!({
            "experiment": "aligned CAPSNODE collision in production Merkle",
            "regenerated": regenerate,
            "collision_evaluations": collision.evaluations,
            "elapsed_seconds": started.elapsed().as_secs_f64(),
            "collision_digest": hash_output_hex(&collision.digest),
            "all_collision_effective_inputs_aligned": all_collision_inputs_aligned,
            "fixed_depth": DEPTH,
            "fixed_cap_depth": CAP_DEPTH,
            "same_cap": cap0 == cap1,
            "distinct_prehashed_leaf_vectors": leaves0 != leaves1,
            "distinct_opening_siblings": proof0.siblings != proof1.siblings,
            "first_prehashed_opening_accepted": true,
            "second_prehashed_opening_accepted": true,
            "real_capsmix_digest_aligned": aligned_digest(&real_digest),
            "real_capsmix_substitution_error": reachability_error,
            "full_capsule_acceptance_claimed": false,
            "alignment_probability_per_random_capsmix_digest": "2^-224",
            "note": "Acceptance is for the production pre-hashed Merkle API, not for a recomputed CAPSMIX/PCS opening."
        })
    );
}
