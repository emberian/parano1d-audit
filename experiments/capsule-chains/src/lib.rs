use noid_core::hardware::flat_to_tower_u128;
use noid_core::Block128;
use noid_fri::hasher::{CryptographicHasher, HashOutput};
use noid_fri_binius::capsule::CapsuleNodeHasher;
use noid_poseidon2b::native::domain::{capacity_iv_flat, TAG_CAPSNODE};

pub const MERKLE_CAP_DEPTH: usize = 5;

#[path = "../../../parano1d/noid_fri_binius/src/interleaved_commit.rs"]
pub mod production_merkle;

pub use production_merkle::{
    CommitmentHashBackend, SourceBatchedMerkleProof, SourceHash, SourceHashMerkleTree,
};

pub fn encode_lanes(lanes: [u128; 2]) -> SourceHash {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&lanes[0].to_le_bytes());
    out[16..].copy_from_slice(&lanes[1].to_le_bytes());
    out
}

pub fn decode_lanes(digest: &SourceHash) -> [u128; 2] {
    [
        u128::from_le_bytes(digest[..16].try_into().unwrap()),
        u128::from_le_bytes(digest[16..].try_into().unwrap()),
    ]
}

pub fn node_compress(left: &SourceHash, right: &SourceHash) -> SourceHash {
    CapsuleNodeHasher.compress(left, right)
}

pub fn aligned_digest(digest: &SourceHash) -> bool {
    decode_lanes(digest)
        .into_iter()
        .all(|lane| flat_to_tower_u128(lane) <= 0xffff)
}

pub fn aligned_node_right(digest: &SourceHash) -> bool {
    let iv = capacity_iv_flat(TAG_CAPSNODE);
    decode_lanes(digest)
        .into_iter()
        .zip(iv)
        .all(|(lane, whitening)| flat_to_tower_u128(lane ^ whitening) <= 0xffff)
}

pub fn next_u64(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

pub fn mixed_leaf(seed: &mut u64, zero_final_rate: bool) -> Vec<Block128> {
    let mut lanes = (0..24)
        .map(|_| Block128::from(((next_u64(seed) as u128) << 64) | next_u64(seed) as u128))
        .collect::<Vec<_>>();
    if zero_final_rate {
        let iv = capacity_iv_flat(noid_poseidon2b::native::domain::TAG_CAPSMIX);
        let mut state = [0, 0, iv[0], iv[1]];
        for pair in lanes[..22].chunks_exact(2) {
            state[0] ^= noid_core::hardware::tower_to_flat_u128(pair[0].0);
            state[1] ^= noid_core::hardware::tower_to_flat_u128(pair[1].0);
            noid_poseidon2b::native::permutation::permute_flat_u128(&mut state);
        }
        lanes[22] = Block128::from(flat_to_tower_u128(state[0]));
        lanes[23] = Block128::from(flat_to_tower_u128(state[1]));
    }
    lanes
}

pub fn build_tree(leaves: Vec<SourceHash>) -> SourceHashMerkleTree {
    SourceHashMerkleTree::new(
        leaves,
        CommitmentHashBackend::Arithmetic,
        &CapsuleNodeHasher,
    )
}

pub fn tree_layer(tree: &SourceHashMerkleTree, depth: usize) -> Vec<SourceHash> {
    tree.get_layer_at_depth(depth)
}

pub fn build_proof(
    tree: &SourceHashMerkleTree,
    leaf_indices: &[usize],
    depth: usize,
    cap_depth: usize,
) -> SourceBatchedMerkleProof {
    production_merkle::build_source_batched_merkle_proof_to_cap(
        tree,
        leaf_indices,
        depth,
        cap_depth,
        CommitmentHashBackend::Arithmetic,
        &CapsuleNodeHasher,
    )
}

pub fn verify_proof(
    cap_nodes: &[SourceHash],
    cap_depth: usize,
    batch: &SourceBatchedMerkleProof,
    depth: usize,
    leaf_indices: &[usize],
    leaf_hashes: &[SourceHash],
) -> Result<(), String> {
    production_merkle::verify_source_batched_merkle_proof_to_cap(
        cap_nodes,
        cap_depth,
        batch,
        depth,
        leaf_indices,
        leaf_hashes,
        CommitmentHashBackend::Arithmetic,
        &CapsuleNodeHasher,
    )
}

#[derive(Clone, Debug)]
pub struct AlignedCollision {
    pub first: (SourceHash, SourceHash),
    pub second: (SourceHash, SourceHash),
    pub digest: SourceHash,
    pub evaluations: u64,
}

pub fn known_collision() -> AlignedCollision {
    let input = |left: &str, right: &str| {
        (
            hex::decode(left).unwrap().try_into().unwrap(),
            hex::decode(right).unwrap().try_into().unwrap(),
        )
    };
    let first = input(
        "2d8a16372955da0403d2a69740bd860cb83fa1c69c1e7129c671a616a5951930",
        "e681a0b3bee99f85353fbaac20999ef2ec23409e03068aa9e97759648e114d01",
    );
    let second = input(
        "6f7bb24eb68869bdb4f59a582a6a984c5e40eaa3ade728c02e887b0303825ca6",
        "e6312b07507b9f4cc0675c2f9fcd656d4e90a4a753c0a5314779d2d3896cf208",
    );
    let digest = node_compress(&first.0, &first.1);
    assert_eq!(digest, node_compress(&second.0, &second.1));
    AlignedCollision {
        first,
        second,
        digest,
        evaluations: 0,
    }
}

pub fn find_aligned_collision(limit: u64) -> AlignedCollision {
    use std::collections::HashMap;

    let iv = capacity_iv_flat(TAG_CAPSNODE);
    let mut rng = 0x4e4f_4944_434f_4c4cu64;
    let mut seen = HashMap::new();
    for evaluations in 1..=limit {
        let subfield = std::array::from_fn::<u128, 4, _>(|_| {
            noid_core::hardware::tower_to_flat_u128((next_u64(&mut rng) & 0xffff) as u128)
        });
        let left = encode_lanes([subfield[0], subfield[1]]);
        let right = encode_lanes([subfield[2] ^ iv[0], subfield[3] ^ iv[1]]);
        let digest = node_compress(&left, &right);
        assert!(aligned_digest(&left));
        assert!(aligned_node_right(&right));
        assert!(aligned_digest(&digest));
        if let Some((old_left, old_right)) = seen.insert(digest, (left, right)) {
            assert_ne!((old_left, old_right), (left, right));
            return AlignedCollision {
                first: (old_left, old_right),
                second: (left, right),
                digest,
                evaluations,
            };
        }
    }
    panic!("no aligned collision in {limit} evaluations");
}

pub fn find_zero_left_collision(limit: u64) -> AlignedCollision {
    use std::collections::HashMap;

    let iv = capacity_iv_flat(TAG_CAPSNODE);
    let zero = [0u8; 32];
    let mut rng = 0x4e4f_4944_5a45_524fu64;
    let mut seen = HashMap::new();
    for evaluations in 1..=limit {
        let subfield = std::array::from_fn::<u128, 2, _>(|_| {
            noid_core::hardware::tower_to_flat_u128((next_u64(&mut rng) & 0xffff) as u128)
        });
        let right = encode_lanes([subfield[0] ^ iv[0], subfield[1] ^ iv[1]]);
        let digest = node_compress(&zero, &right);
        assert!(aligned_node_right(&right));
        assert!(aligned_digest(&digest));
        if let Some(old_right) = seen.insert(digest, right) {
            assert_ne!(old_right, right);
            return AlignedCollision {
                first: (zero, old_right),
                second: (zero, right),
                digest,
                evaluations,
            };
        }
    }
    panic!("no zero-left aligned collision in {limit} evaluations");
}

pub fn hash_output_hex(hash: &HashOutput) -> String {
    hex::encode(hash)
}
