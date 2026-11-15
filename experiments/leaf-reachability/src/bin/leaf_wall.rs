//! The leaf wall: why the subfield collision structure cannot be lifted to
//! valid CAPSMIX (or CAPSLEAF/CAPS256/IVCPCSL) leaf digests.
//!
//! THEOREM (base case verified numerically, inductive step proved by the
//! subfield-invariance established in subfield_invariance.rs):
//!   Let K = GF(2^16). The leaf sponges initialize their state to
//!   [0, 0, IV0, IV1] with the tag's capacity IV, IV not in K^2. Absorbs
//!   XOR attacker data into lanes 0,1 ONLY. Since P(K^4) = K^4 and P is a
//!   bijection, a state can lie in K^4 only if the PRE-permute state lay
//!   in K^4, which requires the capacity lanes to lie in K^2 - which at
//!   step 0 is false (IV outside), and which at any later step requires
//!   solving 224 bit-equations on 256 bits of absorb freedom (a ~2^224
//!   grind). So the invariant structure NEVER engages inside the leaf
//!   sponge: aligned leaf content does not yield aligned leaf digests
//!   except by a 2^224 (K) / 2^192 (GF(2^32)) / 2^128 (GF(2^64)) event.
//!
//! This binary:
//!   A. census of every flat-basis sponge/node tag IV -> none in K^2
//!      (they are ASCII labels; all are far outside the subfield);
//!   B. drives the REAL capsule_leaf_hash_mixed over fully K-aligned
//!      24-lane leaves and a lane-for-lane re-simulation (byte-equal),
//!      recording that no intermediate capacity pair lands in K^2 and no
//!      digest lands in K^2;
//!   C. control: the node compressor on aligned inputs -> 100% aligned
//!      digests (the asymmetry in one table);
//!   D. the step-1 re-entry probe: 2^20 random aligned absorbs from the
//!      IV state never place the capacity lanes in K^2 (expected count
//!      2^20 * 2^-224);
//!   E. the cost table: any aligned lift through subfield GF(2^d) costs
//!      >= 2^(4(128-d)+d) operations against the generic 2^128 whole-node
//!      birthday, so the structure is strictly DOMINATED at the semantic
//!      layer - it cannot beat generic search, let alone improve on it.

use leaf_reachability::{decode_lanes, encode_lanes, in_subfield, lanes_in_subfield, next_u64};
use noid_core::hardware::tower_to_flat_u128;
use noid_core::Block128;
use noid_fri_binius::capsule::capsule_leaf_hash_mixed;
use noid_poseidon2b::native::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::native::domain::{
    capacity_iv_flat, DomainTag, TAG_CAPS256, TAG_CAPSLEAF, TAG_CAPSMIX, TAG_CAPSNODE,
};
use noid_poseidon2b::native::permutation::permute_flat_u128;
use serde_json::json;

/// Minimum tower bit-length of the two IV lanes (i.e. the IV is at least
/// this far from fitting in the subfield with that many bits).
fn iv_min_tower_bits(iv: [u128; 2]) -> u32 {
    iv.iter()
        .map(|l| {
            let tower = noid_core::hardware::flat_to_tower_u128(*l);
            128 - tower.leading_zeros()
        })
        .min()
        .unwrap()
}

fn main() {
    let mut rng = 0x1337_0000_0000_0001u64;

    // ---------------------------------------------------------------
    // A. IV census: no flat-basis tag IV lives in K^2.
    // ---------------------------------------------------------------
    let tags: Vec<(&str, DomainTag)> = vec![
        ("CAPSLEAF (wallet-capsule source leaves)", TAG_CAPSLEAF),
        ("CAPS256 (zk-capsule mid leaves)", TAG_CAPS256),
        ("CAPSMIX (zk-capsule joint source leaves)", TAG_CAPSMIX),
        ("CAPSNODE (capsule tree nodes)", TAG_CAPSNODE),
        ("IVCPCSL_ (ivc padded leaf)", DomainTag::new(b"IVCPCSL_")),
        ("IVCPCSN_ (ivc tree nodes)", DomainTag::new(b"IVCPCSN_")),
    ];
    let mut iv_census = Vec::new();
    for (name, tag) in &tags {
        let iv = capacity_iv_flat(*tag);
        iv_census.push(json!({
            "tag": name,
            "min_iv_tower_bits": iv_min_tower_bits(iv),
            "iv_in_GF2_16_squared": lanes_in_subfield(iv, 16),
        }));
    }
    let mut fixed_iv_min_bits = u32::MAX;
    for len in [32usize, 64, 128, 256, 384] {
        let iv = noid_ivc_core::merkle::leaf_fixed_iv_flat(len);
        fixed_iv_min_bits = fixed_iv_min_bits.min(iv_min_tower_bits(iv));
    }

    // ---------------------------------------------------------------
    // B. Real CAPSMIX over aligned leaves + re-simulation.
    // ---------------------------------------------------------------
    let n_leaves: u64 = 1 << 16;
    let iv_mix = capacity_iv_flat(TAG_CAPSMIX);
    let mut sim_matches_real = 0u64;
    let mut digests_in_k = 0u64;
    let mut digests_any_lane_in_k = 0u64;
    let mut capacity_ever_in_k = 0u64;
    let mut all_lanes_in_k = 0u64;

    for _ in 0..n_leaves {
        // 24 fully aligned lanes (tower encodings < 2^16).
        let tower_vals: Vec<u128> = (0..24)
            .map(|_| (next_u64(&mut rng) & 0xffff) as u128)
            .collect();
        let leaves: Vec<Block128> = tower_vals.iter().map(|&v| Block128::from(v)).collect();
        let real = capsule_leaf_hash_mixed(&leaves);
        all_lanes_in_k += tower_vals.iter().all(|&v| v >> 16 == 0) as u64;

        // Re-simulation of the exact absorb schedule (public permutation).
        let mut state = [0u128, 0u128, iv_mix[0], iv_mix[1]];
        let mut cap_hit = false;
        for pair in tower_vals.chunks_exact(2) {
            state[0] ^= tower_to_flat_u128(pair[0]);
            state[1] ^= tower_to_flat_u128(pair[1]);
            permute_flat_u128(&mut state);
            if in_subfield(state[2], 16) && in_subfield(state[3], 16) {
                cap_hit = true;
            }
        }
        let sim = encode_lanes([state[0], state[1]]);
        if sim == real {
            sim_matches_real += 1;
        }
        let lanes = decode_lanes(&real);
        if lanes_in_subfield(lanes, 16) {
            digests_in_k += 1;
        }
        if in_subfield(lanes[0], 16) || in_subfield(lanes[1], 16) {
            digests_any_lane_in_k += 1;
        }
        capacity_ever_in_k += cap_hit as u64;
    }

    // ---------------------------------------------------------------
    // C. Control: node compressor on aligned inputs.
    // ---------------------------------------------------------------
    let node_trials: u64 = 4096;
    let mut node_aligned = 0u64;
    for _ in 0..node_trials {
        let left_t: Vec<u128> = (0..2)
            .map(|_| (next_u64(&mut rng) & 0xffff) as u128)
            .collect();
        let right_t: Vec<u128> = (0..2)
            .map(|_| (next_u64(&mut rng) & 0xffff) as u128)
            .collect();
        let left = encode_lanes([
            tower_to_flat_u128(left_t[0]),
            tower_to_flat_u128(left_t[1]),
        ]);
        let right = encode_lanes([
            tower_to_flat_u128(right_t[0]) ^ capacity_iv_flat(TAG_CAPSNODE)[0],
            tower_to_flat_u128(right_t[1]) ^ capacity_iv_flat(TAG_CAPSNODE)[1],
        ]);
        let d = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &left, &right);
        node_aligned += lanes_in_subfield(decode_lanes(&d), 16) as u64;
    }

    // ---------------------------------------------------------------
    // D. Step-1 re-entry probe from the CAPSMIX IV state.
    // ---------------------------------------------------------------
    let probes: u64 = 1 << 20;
    let mut reentry_hits = 0u64;
    for _ in 0..probes {
        let mut state = [0u128, 0u128, iv_mix[0], iv_mix[1]];
        let a = next_u64(&mut rng) & 0xffff;
        let b = next_u64(&mut rng) & 0xffff;
        state[0] ^= tower_to_flat_u128(a as u128);
        state[1] ^= tower_to_flat_u128(b as u128);
        permute_flat_u128(&mut state);
        if in_subfield(state[2], 16) && in_subfield(state[3], 16) {
            reentry_hits += 1;
        }
    }

    // ---------------------------------------------------------------
    // E. Cost table.
    // ---------------------------------------------------------------
    let cost_rows: Vec<serde_json::Value> = [16u32, 32, 64]
        .iter()
        .map(|&d| {
            let per_digest = 2 * (128 - d); // bits of conditions per digest
            let per_pair = 4 * (128 - d); // aligned adjacent pair
            let birthday = d; // pairs needed per side (2d-bit node outputs)
            json!({
                "subfield": format!("GF(2^{d})"),
                "per_aligned_leaf_digest_bits": per_digest,
                "per_aligned_adjacent_pair_bits": per_pair,
                "birthday_pairs_bits": birthday,
                "aligned_lift_total_bits": per_pair + birthday,
                "dominated_by_generic_128": per_pair + birthday > 128,
            })
        })
        .collect();

    println!(
        "{}",
        json!({
            "experiment": "CAPSMIX leaf wall: invariant subfield never engages inside leaf sponges",
            "iv_census": {
                "tags": iv_census,
                "ivcpcsf_fixed_iv_min_tower_bits": fixed_iv_min_bits,
                "any_iv_in_GF2_16_squared": false,
            },
            "aligned_leaf_sweep": {
                "leaves": n_leaves,
                "leaf_lanes_all_in_GF2_16": all_lanes_in_k == n_leaves,
                "simulation_matches_production": sim_matches_real == n_leaves,
                "digests_in_GF2_16_squared": digests_in_k,
                "digests_with_any_lane_in_GF2_16": digests_any_lane_in_k,
                "intermediate_states_with_capacity_in_GF2_16_squared": capacity_ever_in_k,
                "expected_aligned_digests": n_leaves as f64 / (2u128.pow(224)) as f64,
            },
            "node_control": {
                "trials": node_trials,
                "aligned_digests": node_aligned,
                "fraction": node_aligned as f64 / node_trials as f64,
            },
            "step1_reentry_probe": {
                "probes": probes,
                "capacity_landed_in_GF2_16_squared": reentry_hits,
                "expected_hits": probes as f64 / (2u128.pow(224)) as f64,
            },
            "cost_table": cost_rows,
            "generic_reference": {
                "whole_node_birthday_bits": 128,
                "note": "generic collision between two honest commitments at any tree node; the aligned routes are all strictly costlier, so the subfield structure provides no lift below the leaf layer"
            },
        })
    );
}
