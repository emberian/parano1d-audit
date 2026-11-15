use noid_core::Block128;
use noid_fri::merkle::{verify_merkle_path, MerkleTree, VectorCommitment};
use noid_fri_binius::capsule::{capsule_leaf_hash_mixed, CapsuleNodeHasher};
use noid_fri_binius::interleaved_commit::{
    expand_source_batched_merkle_proof_to_cap, IndependentSourceMerklePath,
    SourceBatchedMerkleProof, SourceHash,
};
use noid_fri_binius::zk_capsule_pcs::{
    ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH, ZK_CAPSULE_PCS_SOURCE_TREE_DEPTH,
};
use noid_ivc_core::deep_chain::capsule_leaf::raw_flat_lane;
use noid_ivc_core::deep_chain::ff_merkle::{
    build_ff_merkle_path_columns, FfMerklePathFamily, FfMerklePathWitness,
};
use noid_ivc_core::field::F128;
use noid_poseidon2b::native::compression::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::native::domain::{capacity_iv_flat, TAG_CAPSNODE};

fn decode(hex: &str) -> SourceHash {
    hex::decode(hex).unwrap().try_into().unwrap()
}

fn valid_source_leaf(seed: u128) -> SourceHash {
    let lanes: Vec<Block128> = (0..24)
        .map(|i| Block128::from(seed ^ (i as u128 + 1).wrapping_mul(0x9e37_0019)))
        .collect();
    capsule_leaf_hash_mixed(&lanes)
}

fn valid_source_subtree_root(depth: usize, seed: u128) -> SourceHash {
    let leaves = (0..1usize << depth)
        .map(|i| valid_source_leaf(seed ^ (i as u128).wrapping_mul(0x1f83_d9ab)))
        .collect();
    MerkleTree::new(leaves, &CapsuleNodeHasher).get_root()
}

fn expand(
    children: [SourceHash; 2],
    common_siblings: &[SourceHash],
) -> Vec<IndependentSourceMerklePath> {
    let batch = SourceBatchedMerkleProof {
        siblings: common_siblings.to_vec(),
    };
    expand_source_batched_merkle_proof_to_cap(
        &batch,
        ZK_CAPSULE_PCS_SOURCE_TREE_DEPTH,
        ZK_CAPSULE_PCS_SOURCE_TREE_DEPTH - ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH,
        &[0, 1],
        &children,
        &CapsuleNodeHasher,
    )
    .unwrap()
}

fn lanes(hash: &SourceHash) -> [F128; 2] {
    [
        raw_flat_lane(u128::from_le_bytes(hash[..16].try_into().unwrap())),
        raw_flat_lane(u128::from_le_bytes(hash[16..].try_into().unwrap())),
    ]
}

fn recursive_witness(path: &IndependentSourceMerklePath) -> FfMerklePathWitness {
    FfMerklePathWitness {
        entry: lanes(&path.leaf_hash),
        siblings: path.siblings.iter().map(lanes).collect(),
        directions: path.directions.clone(),
    }
}

fn main() {
    let first = [
        decode("2d8a16372955da0403d2a69740bd860cb83fa1c69c1e7129c671a616a5951930"),
        decode("e681a0b3bee99f85353fbaac20999ef2ec23409e03068aa9e97759648e114d01"),
    ];
    let second = [
        decode("6f7bb24eb68869bdb4f59a582a6a984c5e40eaa3ade728c02e887b0303825ca6"),
        decode("e6312b07507b9f4cc0675c2f9fcd656d4e90a4a753c0a5314779d2d3896cf208"),
    ];
    assert_ne!(first, second);

    let first_node = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &first[0], &first[1]);
    let second_node = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &second[0], &second[1]);
    assert_eq!(first_node, second_node);
    println!("capsnode_collision={}", hex::encode(first_node));

    let common_siblings: Vec<SourceHash> = (1..ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH)
        .map(|depth| valid_source_subtree_root(depth, 0x7269_6e64_0000 + depth as u128))
        .collect();
    assert_eq!(common_siblings.len(), ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH - 1);

    let first_paths = expand(first, &common_siblings);
    let second_paths = expand(second, &common_siblings);
    let mut cap = first_node;
    for sibling in &common_siblings {
        cap = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &cap, sibling);
    }

    let commitment = VectorCommitment {
        root: cap,
        depth: ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH,
    };
    let native_paths = first_paths.iter().chain(second_paths.iter());
    for path in native_paths {
        assert!(verify_merkle_path(
            &commitment,
            path.leaf_hash,
            path.leaf_index,
            &path.siblings,
            &CapsuleNodeHasher,
        ));
    }
    println!("native_paths_accepted=4");
    println!("shared_source_cap_node={}", hex::encode(cap));

    let recursive_paths: Vec<FfMerklePathWitness> = first_paths
        .iter()
        .chain(second_paths.iter())
        .map(recursive_witness)
        .collect();
    let family = FfMerklePathFamily {
        depth: ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH,
        n_paths: recursive_paths.len(),
    };
    let columns = build_ff_merkle_path_columns(
        &family,
        capacity_iv_flat(TAG_CAPSNODE).map(raw_flat_lane),
        &recursive_paths,
        6,
    );
    assert_eq!(columns.roots.len(), 4);
    assert!(columns.roots.iter().all(|root| *root == lanes(&cap)));
    println!("recursive_trace_roots_matched=4");
    println!("source_tree_depth={}", ZK_CAPSULE_PCS_SOURCE_TREE_DEPTH);
    println!("source_cap_path_depth={}", ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH);
    println!("common_siblings_have_capsmix_leaf_openings=true");
    println!("colliding_child_leaf_openings_supplied=false");
    println!("full_wallet_or_history_acceptance_demonstrated=false");
    println!("bad_typed_bind_instantiated_by_supplied_artifact=false");
    println!("delta_violation_proven=false");
}
