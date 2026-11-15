//! Cross-domain typed-binding failure between the two production 1-perm
//! node domains, TAG_CAPSNODE and IVCPCSN_.
//!
//! noid_poseidon2b/src/native/compression.rs:319-325 states the design
//! claim: "the caller-selected tag whitens the right-input lanes so
//! distinct trees cannot share collisions."
//!
//! That claim is false on the invariant subfield. For ANY tag T, an
//! attacker who picks left in K^2 and right = k ^ IV_T with k in K^2 puts
//! the compressor state inside K^4, where the permutation runs as the
//! SAME restricted function regardless of the tag. The tag only selects
//! WHICH right-input coset maps into K^4. Consequently a single 32-byte
//! digest value D can be produced simultaneously as
//!
//!    D = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, a, rC)
//!    D = noid_ivc_core::merkle::hash_pair(a, rI)          [tag IVCPCSN_]
//!
//! with rC != rI, by a 2^16 birthday between the two tag domains - i.e.
//! the domain separation between the capsule trees and the proof-core
//! PCS trees provides only ~16 bits of claw resistance, not 128.
//!
//! We then lift the claw to the two production raw commitment APIs: a
//! 2-leaf prehashed tree built and opened under the CAPSNODE domain and
//! one under the IVCPCSN_ domain share the IDENTICAL 32-byte commitment,
//! each verifying against its own leaves with the production
//! `verify_source_batched_merkle_proof_to_cap`.

use std::collections::HashMap;
use std::time::Instant;

use leaf_reachability::{decode_lanes, encode_lanes, lanes_in_subfield, random_subfield_lanes};
use noid_fri::hasher::{CryptographicHasher, HashOutput};
use leaf_reachability::{build_proof_to_cap, build_tree, tree_layer, verify_proof_to_cap, SourceHash};
use noid_fri_binius::capsule::CapsuleNodeHasher;
use noid_poseidon2b::native::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::native::domain::{capacity_iv_flat, DomainTag, TAG_CAPSNODE};
use serde_json::json;

/// The proof-core PCS node tag, pinned at noid_ivc_core/src/merkle.rs:42
/// (`POSEIDON_MERKLE_NODE_TAG`) and duplicated at
/// noid_recursive/src/acceptance/trace/self_verify.rs:49.
const TAG_IVCPCSN: DomainTag = DomainTag::new(b"IVCPCSN_");

/// Production IVCPCSN_ node function wrapped in the hasher trait so the
/// production batched-verify API can be driven unchanged.
struct IvcPcsnNodeHasher;

impl CryptographicHasher for IvcPcsnNodeHasher {
    fn hash_pair(&self, _a: &noid_core::Block128, _b: &noid_core::Block128) -> HashOutput {
        unreachable!("prehashed tree: leaves arrive as digests")
    }
    fn hash_field(&self, _elem: &noid_core::Block128) -> HashOutput {
        unreachable!("prehashed tree: leaves arrive as digests")
    }
    fn hash_concatenation(&self, _a: &HashOutput, _b: &HashOutput) -> HashOutput {
        unreachable!("nodes use the 1-perm feed-forward compress")
    }
    fn compress(&self, a: &HashOutput, b: &HashOutput) -> HashOutput {
        noid_ivc_core::merkle::hash_pair(a, b)
    }
}

fn main() {
    let mut rng = 0xc1a7_0000_0000_0001u64;
    let iv_c = capacity_iv_flat(TAG_CAPSNODE);
    let iv_i = capacity_iv_flat(TAG_IVCPCSN);

    // The two whitening cosets are disjoint: IV_C ^ IV_I has lanes outside K.
    let cosets_disjoint = !lanes_in_subfield(
        [iv_c[0] ^ iv_i[0], iv_c[1] ^ iv_i[1]],
        16,
    );

    // Fixed shared left child, aligned in K^2 (the left lanes are NOT
    // whitened by any tag, so the same left serves both domains).
    let left_lanes = random_subfield_lanes(&mut rng, 16);
    let left = encode_lanes(left_lanes);

    // ---------------------------------------------------------------
    // Birthday between the two tag domains on the aligned coset.
    // ---------------------------------------------------------------
    let started = Instant::now();
    let mut capsnode_side: HashMap<[u8; 32], [u8; 32]> = HashMap::new();
    let mut evals_c = 0u64;
    let mut evals_i = 0u64;
    let (right_c, right_i, shared) = loop {
        let k = random_subfield_lanes(&mut rng, 16);
        let right_c = encode_lanes([k[0] ^ iv_c[0], k[1] ^ iv_c[1]]);
        let d_c = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &left, &right_c);
        evals_c += 1;

        let k2 = random_subfield_lanes(&mut rng, 16);
        let right_i = encode_lanes([k2[0] ^ iv_i[0], k2[1] ^ iv_i[1]]);
        let d_i = noid_ivc_core::merkle::hash_pair(&left, &right_i);
        evals_i += 1;

        if let Some(rc) = capsnode_side.get(&d_i) {
            break (*rc, right_i, d_i);
        }
        capsnode_side.insert(d_c, right_c);
    };
    let claw_seconds = started.elapsed().as_secs_f64();

    // Verify against the real production functions under BOTH tags.
    assert_ne!(right_c, right_i);
    let d_c = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &left, &right_c);
    let d_i = noid_ivc_core::merkle::hash_pair(&left, &right_i);
    assert_eq!(d_c, d_i);
    assert_eq!(d_c, shared);
    let digest_in_k = lanes_in_subfield(decode_lanes(&shared), 16);
    let right_c_coset = lanes_in_subfield(
        [
            decode_lanes(&right_c)[0] ^ iv_c[0],
            decode_lanes(&right_c)[1] ^ iv_c[1],
        ],
        16,
    );
    let right_i_coset = lanes_in_subfield(
        [
            decode_lanes(&right_i)[0] ^ iv_i[0],
            decode_lanes(&right_i)[1] ^ iv_i[1],
        ],
        16,
    );

    // ---------------------------------------------------------------
    // Two production raw-API commitments with identical bytes.
    // ---------------------------------------------------------------
    let build = |leaves: Vec<SourceHash>, hasher: &dyn CryptographicHasher| {
        let tree = build_tree(leaves.clone(), hasher);
        let root = tree_layer(&tree, 0)[0];
        let proof = build_proof_to_cap(&tree, &[0], 1, 0, hasher);
        (root, proof, leaves)
    };
    let (root_c, proof_c, leaves_c) = build(vec![left, right_c], &CapsuleNodeHasher);
    let (root_i, proof_i, leaves_i) = build(vec![left, right_i], &IvcPcsnNodeHasher);
    assert_eq!(root_c, root_i);
    assert_eq!(root_c, shared);
    assert_ne!(leaves_c, leaves_i);

    verify_proof_to_cap(&[root_c], 0, &proof_c, 1, &[0], &[leaves_c[0]], &CapsuleNodeHasher)
        .expect("CAPSNODE-domain raw verify accepts its opening");
    verify_proof_to_cap(
        &[root_i],
        0,
        &proof_i,
        1,
        &[0],
        &[leaves_i[0]],
        &IvcPcsnNodeHasher,
    )
    .expect("IVCPCSN_-domain raw verify accepts its opening");

    println!(
        "{}",
        json!({
            "experiment": "cross-tag claw: CAPSNODE digest == IVCPCSN_ digest, shared left, distinct rights",
            "design_claim_tested": "compression.rs:319-325 'the caller-selected tag whitens the right-input lanes so distinct trees cannot share collisions'",
            "cosets_disjoint_outside_GF2_16": cosets_disjoint,
            "shared_left_hex": hex::encode(left),
            "right_under_CAPSNODE_hex": hex::encode(right_c),
            "right_under_IVCPCSN_hex": hex::encode(right_i),
            "shared_digest_hex": hex::encode(shared),
            "digest_in_GF2_16": digest_in_k,
            "right_CAPSNODE_in_IV_plus_GF2_16": right_c_coset,
            "right_IVCPCSN_in_IV_plus_GF2_16": right_i_coset,
            "capsnode_evaluations": evals_c,
            "ivcpcsn_evaluations": evals_i,
            "claw_seconds": claw_seconds,
            "two_domain_commitment": {
                "capsule_domain_root": hex::encode(root_c),
                "ivc_pcs_domain_root": hex::encode(root_i),
                "roots_identical": root_c == root_i,
                "leaves_distinct": leaves_c != leaves_i,
                "capsnode_raw_verify": "accepted",
                "ivcpcsn_raw_verify": "accepted",
            },
            "note": "Both raw verifications use the production verify_source_batched_merkle_proof_to_cap; the IVC side's node function is the production noid_ivc_core::merkle::hash_pair. Semantic capsule/PCS verifiers recompute leaf hashes, so this object is confined to prehashed/raw consumers - same reachability boundary as the same-tag collision, now ACROSS the two tag domains."
        })
    );
}
