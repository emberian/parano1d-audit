use capsule_chains::{
    aligned_digest, build_proof, build_tree, encode_lanes, hash_output_hex, mixed_leaf,
    node_compress, tree_layer, verify_proof,
};
use noid_core::hardware::tower_to_flat_u128;
use noid_fri_binius::capsule::capsule_leaf_hash_mixed;
use noid_poseidon2b::native::domain::{capacity_iv_flat, TAG_CAPSMIX, TAG_CAPSNODE};
use noid_poseidon2b::native::permutation::permute_flat_u128;
use serde_json::json;

fn main() {
    let mut rng = 0xca95_a11a_5000_0001u64;
    let lanes = mixed_leaf(&mut rng, true);

    let mix_iv = capacity_iv_flat(TAG_CAPSMIX);
    let mut penultimate = [0, 0, mix_iv[0], mix_iv[1]];
    for pair in lanes[..22].chunks_exact(2) {
        penultimate[0] ^= tower_to_flat_u128(pair[0].0);
        penultimate[1] ^= tower_to_flat_u128(pair[1].0);
        permute_flat_u128(&mut penultimate);
    }
    assert_eq!(
        [lanes[22].0, lanes[23].0],
        [
            noid_core::hardware::flat_to_tower_u128(penultimate[0]),
            noid_core::hardware::flat_to_tower_u128(penultimate[1]),
        ]
    );
    let leaf_digest = capsule_leaf_hash_mixed(&lanes);
    let final_input = [0, 0, penultimate[2], penultimate[3]];
    let mut final_state = final_input;
    permute_flat_u128(&mut final_state);
    assert_eq!(leaf_digest, encode_lanes([final_state[0], final_state[1]]));

    let zero = [0u8; 32];
    let node_iv = capacity_iv_flat(TAG_CAPSNODE);
    let alias_right = encode_lanes([penultimate[2] ^ node_iv[0], penultimate[3] ^ node_iv[1]]);
    let alias_node_digest = node_compress(&zero, &alias_right);
    let mut direct_node_state = [0, 0, penultimate[2], penultimate[3]];
    permute_flat_u128(&mut direct_node_state);
    assert_eq!(final_state, direct_node_state);
    assert_eq!(
        leaf_digest,
        encode_lanes([direct_node_state[0], direct_node_state[1]])
    );
    assert_eq!(leaf_digest, alias_node_digest);

    let shallow_tree = build_tree(vec![leaf_digest]);
    let deep_tree = build_tree(vec![zero, alias_right]);
    let shallow_cap = tree_layer(&shallow_tree, 0);
    let deep_cap = tree_layer(&deep_tree, 0);
    assert_eq!(shallow_cap, deep_cap);
    let shallow_proof = build_proof(&shallow_tree, &[0], 0, 0);
    let deep_proof = build_proof(&deep_tree, &[0], 1, 0);
    verify_proof(&shallow_cap, 0, &shallow_proof, 0, &[0], &[leaf_digest]).unwrap();
    verify_proof(&deep_cap, 0, &deep_proof, 1, &[0], &[zero]).unwrap();

    let shallow_at_deep_error =
        verify_proof(&shallow_cap, 0, &shallow_proof, 1, &[0], &[leaf_digest]).unwrap_err();
    let deep_at_shallow_error =
        verify_proof(&deep_cap, 0, &deep_proof, 0, &[0], &[zero]).unwrap_err();

    let left_fixed = node_compress(&leaf_digest, &alias_right);
    let right_fixed = node_compress(&alias_right, &leaf_digest);
    let one_extra_level_proof = capsule_chains::SourceBatchedMerkleProof {
        siblings: vec![alias_right],
    };
    let fixed_point_error = verify_proof(
        &shallow_cap,
        0,
        &one_extra_level_proof,
        1,
        &[0],
        &[leaf_digest],
    )
    .unwrap_err();

    let collision = capsule_chains::find_zero_left_collision(1_000_000);
    assert_eq!(collision.first.0, zero);
    assert_eq!(collision.second.0, zero);
    assert_ne!(collision.first.1, collision.second.1);
    let collision_at_alias_cap = verify_proof(
        &shallow_cap,
        0,
        &capsule_chains::SourceBatchedMerkleProof {
            siblings: vec![collision.first.1],
        },
        1,
        &[0],
        &[collision.first.0],
    )
    .unwrap_err();
    let alias_at_collision_cap =
        verify_proof(&[collision.digest], 0, &deep_proof, 1, &[0], &[zero]).unwrap_err();

    let alias_right_matches_collision =
        alias_right == collision.first.1 || alias_right == collision.second.1;
    println!(
        "{}",
        json!({
            "experiment": "CAPSMIX leaf/internal-node alias, depth and collision chaining",
            "zero_left_collision_evaluations": collision.evaluations,
            "zero_left_collision_digest": hash_output_hex(&collision.digest),
            "alias_right_matches_a_collision_right": alias_right_matches_collision,
            "leaf_digest": hash_output_hex(&leaf_digest),
            "alias_node_digest": hash_output_hex(&alias_node_digest),
            "alias_right": hash_output_hex(&alias_right),
            "leaf_equals_internal_node": leaf_digest == alias_node_digest,
            "shallow_depth_0_accepted": true,
            "deep_depth_1_accepted": true,
            "same_root_across_depths": shallow_cap == deep_cap,
            "shallow_proof_at_depth_1_error": shallow_at_deep_error,
            "deep_proof_at_depth_0_error": deep_at_shallow_error,
            "left_fixed_point_C_D_X_equals_D": left_fixed == leaf_digest,
            "right_fixed_point_C_X_D_equals_D": right_fixed == leaf_digest,
            "extra_level_fixed_point_error": fixed_point_error,
            "collision_substituted_at_alias_cap_error": collision_at_alias_cap,
            "alias_substituted_at_collision_cap_error": alias_at_collision_cap,
            "alias_digest_aligned": aligned_digest(&leaf_digest),
            "alias_right_effective_input_aligned": capsule_chains::aligned_node_right(&alias_right),
            "left_fixed_point_required_condition": "P([D, X+IV_CAPSNODE]).rate = 0 (256 bits)",
            "right_fixed_point_required_condition": "P([X, D+IV_CAPSNODE]).rate = D+X (256 bits)",
            "random_alias_right_alignment_expected_trials": "2^224",
            "random_alias_digest_alignment_expected_trials": "2^224",
            "aligned_class_entry_expected_trials": "2^224; digest alignment then follows inside the aligned permutation class",
            "full_capsule_acceptance_claimed": false
        })
    );
}
