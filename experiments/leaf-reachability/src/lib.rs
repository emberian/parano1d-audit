//! Leaf-reachability escalation lane.
//!
//! Shared helpers: sampling inside the invariant subfield tower K_d =
//! { x in GF(2^128) : tower encoding < 2^d } (for d in {4,8,16,32,64} these
//! are exactly the subfields GF(2^d) of the nested tower basis used by
//! noid_core), and lane/bytes codecs for 32-byte digests.

use noid_core::hardware::{flat_to_tower_u128, tower_to_flat_u128};

/// Crate-root constant the included production module expects
/// (noid_fri_binius/src/lib.rs:65 pins this to 5).
pub const MERKLE_CAP_DEPTH: usize = 5;

/// Include the production Merkle implementation directly (the batched
/// proof builders and the tree are `pub(crate)` in noid_fri_binius, so
/// this is the same include the previous capsule-chains lane used).
#[path = "../../../parano1d/noid_fri_binius/src/interleaved_commit.rs"]
pub mod production_merkle;

pub use production_merkle::{
    CommitmentHashBackend, SourceBatchedMerkleProof, SourceHash, SourceHashMerkleTree,
};

/// Pub wrappers over the `pub(crate)` production builders (same pattern as
/// the capsule-chains lane's lib).
pub fn build_proof_to_cap(
    tree: &SourceHashMerkleTree,
    leaf_indices: &[usize],
    depth: usize,
    cap_depth: usize,
    hasher: &dyn noid_fri::hasher::CryptographicHasher,
) -> SourceBatchedMerkleProof {
    production_merkle::build_source_batched_merkle_proof_to_cap(
        tree,
        leaf_indices,
        depth,
        cap_depth,
        CommitmentHashBackend::Arithmetic,
        hasher,
    )
}

pub fn verify_proof_to_cap(
    cap_nodes: &[SourceHash],
    cap_depth: usize,
    batch: &SourceBatchedMerkleProof,
    depth: usize,
    leaf_indices: &[usize],
    leaf_hashes: &[SourceHash],
    hasher: &dyn noid_fri::hasher::CryptographicHasher,
) -> Result<(), String> {
    production_merkle::verify_source_batched_merkle_proof_to_cap(
        cap_nodes,
        cap_depth,
        batch,
        depth,
        leaf_indices,
        leaf_hashes,
        CommitmentHashBackend::Arithmetic,
        hasher,
    )
}

/// Build a prehashed tree under an arbitrary node hasher.
pub fn build_tree(
    leaves: Vec<SourceHash>,
    hasher: &dyn noid_fri::hasher::CryptographicHasher,
) -> SourceHashMerkleTree {
    SourceHashMerkleTree::new(leaves, CommitmentHashBackend::Arithmetic, hasher)
}

/// Layer at a depth (0 = root).
pub fn tree_layer(tree: &SourceHashMerkleTree, depth: usize) -> Vec<SourceHash> {
    tree.get_layer_at_depth(depth)
}

/// Membership of a FLAT-basis lane in the subfield with `tower_bits` bits.
#[inline]
pub fn in_subfield(flat_lane: u128, tower_bits: u32) -> bool {
    flat_to_tower_u128(flat_lane) >> tower_bits == 0
}

/// Both lanes of a digest inside the subfield (the "aligned digest"
/// predicate of the previous capsule-chains lane, generalized).
#[inline]
pub fn lanes_in_subfield(lanes: [u128; 2], tower_bits: u32) -> bool {
    lanes.iter().all(|&l| in_subfield(l, tower_bits))
}

pub fn next_u64(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

/// Two random flat lanes whose tower encodings are < 2^tower_bits.
pub fn random_subfield_lanes(rng: &mut u64, tower_bits: u32) -> [u128; 2] {
    debug_assert!(tower_bits <= 64);
    let draw = |rng: &mut u64| {
        let mut v = next_u64(rng);
        if tower_bits < 64 {
            v &= (1u64 << tower_bits) - 1;
        }
        tower_to_flat_u128(v as u128)
    };
    [draw(rng), draw(rng)]
}

pub fn encode_lanes(lanes: [u128; 2]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&lanes[0].to_le_bytes());
    out[16..].copy_from_slice(&lanes[1].to_le_bytes());
    out
}

pub fn decode_lanes(digest: &[u8; 32]) -> [u128; 2] {
    [
        u128::from_le_bytes(digest[..16].try_into().unwrap()),
        u128::from_le_bytes(digest[16..].try_into().unwrap()),
    ]
}

/// The hardcoded TAG_CAPSNODE collision inputs from the root-verified PoC
/// (poc/src/bin/verify_collision.rs).
pub fn known_collision() -> ([u8; 32], [u8; 32], [u8; 32], [u8; 32]) {
    let b = |s: &str| hex::decode(s).unwrap().try_into().unwrap();
    (
        b("2d8a16372955da0403d2a69740bd860cb83fa1c69c1e7129c671a616a5951930"),
        b("e681a0b3bee99f85353fbaac20999ef2ec23409e03068aa9e97759648e114d01"),
        b("6f7bb24eb68869bdb4f59a582a6a984c5e40eaa3ade728c02e887b0303825ca6"),
        b("e6312b07507b9f4cc0675c2f9fcd656d4e90a4a753c0a5314779d2d3896cf208"),
    )
}
