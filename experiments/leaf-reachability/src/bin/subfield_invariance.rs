//! WHY the TAG_CAPSNODE / IVCPCSN_ collisions are cheap: the exact algebraic
//! characterization.
//!
//! Claim established here (all against the pinned production code):
//! 1. Every round constant and every MDS multiplier of the Poseidon2b
//!    permutation has tower encoding < 2^16, i.e. lies in the subfield
//!    K = GF(2^16) of GF(2^128).
//! 2. Therefore K^4 is invariant under the full permutation
//!    (`permute_flat_u128`), and the restriction is exactly "the same
//!    Poseidon2 schedule executed over GF(2^16)". Verified empirically.
//! 3. GF(2^8)^4 and GF(2^4)^4 are NOT invariant (the partial-round MDS
//!    multipliers 0x200/0x2000 leave the small subfield), so K = GF(2^16)
//!    is the smallest invariant subfield - matching "V_16" from the root
//!    narrative.
//! 4. The production node compressor
//!    `compress_flat_feed_forward_with_tag` whitens only the right input
//!    with the tag IV, so an attacker who picks right = k ^ IV with k in
//!    K^2 and left in K^2 places the whole state in K^4: the compressor
//!    restricts to a 64-bit -> 32-bit function and collisions cost a
//!    2^16 birthday. The known production collision pair is exactly of
//!    this form; a fresh birthday collision is regenerated here.
//! 5. The two-permutation sponge compressor `compress_with_tag` (used for
//!    EXSTNOD etc.) loads the tag IV into capacity lanes BEFORE the first
//!    permutation; the IV is not in K^2, so the state can never be placed
//!    in K^4 by choice of data lanes, and its digests on aligned inputs
//!    are NOT aligned. This is the guardrail asymmetry, demonstrated.

use std::collections::HashMap;
use std::time::Instant;

use leaf_reachability::{
    decode_lanes, encode_lanes, in_subfield, known_collision, lanes_in_subfield, next_u64,
    random_subfield_lanes,
};
use noid_poseidon2b::native::compression::compress_with_tag;
use noid_poseidon2b::native::domain::{capacity_iv, capacity_iv_flat, TAG_COMPRESS};
use noid_poseidon2b::native::permutation::{
    permute_flat_u128, MDS_FULL, MDS_PARTIAL, ROUND_CONSTANTS,
};
use noid_poseidon2b::native::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::native::domain::TAG_CAPSNODE;
use serde_json::json;

fn main() {
    let mut rng = 0x1eaf_0000_0000_0001u64;

    // ---------------------------------------------------------------
    // 1. All permutation constants live in K = GF(2^16) (tower < 2^16).
    // ---------------------------------------------------------------
    let mut rc_max: u128 = 0;
    let mut mds_full_max: u128 = 0;
    let mut mds_partial_max: u128 = 0;
    for row in ROUND_CONSTANTS.iter() {
        for &c in row.iter() {
            rc_max = rc_max.max(c);
        }
    }
    for row in MDS_FULL.iter() {
        for &c in row.iter() {
            mds_full_max = mds_full_max.max(c);
        }
    }
    for row in MDS_PARTIAL.iter() {
        for &c in row.iter() {
            mds_partial_max = mds_partial_max.max(c);
        }
    }
    let constants_in_k =
        rc_max < (1 << 16) && mds_full_max < (1 << 16) && mds_partial_max < (1 << 16);
    assert!(constants_in_k, "constants left GF(2^16)?!");
    let smallest_covering_subfield_bits = [rc_max, mds_full_max, mds_partial_max]
        .iter()
        .map(|&m| 128 - m.leading_zeros())
        .max()
        .unwrap();

    // ---------------------------------------------------------------
    // 2. K^4 invariant under the production permutation.
    // ---------------------------------------------------------------
    let trials = 4096usize;
    let mut k_invariant = true;
    for _ in 0..trials {
        let mut state = [
            random_subfield_lanes(&mut rng, 16)[0],
            random_subfield_lanes(&mut rng, 16)[1],
            random_subfield_lanes(&mut rng, 16)[0],
            random_subfield_lanes(&mut rng, 16)[1],
        ];
        permute_flat_u128(&mut state);
        if !state.iter().all(|&l| in_subfield(l, 16)) {
            k_invariant = false;
            break;
        }
    }
    assert!(k_invariant, "GF(2^16)^4 invariance FAILED");

    // Bijectivity sanity: distinct aligned states stay distinct (the
    // restriction is injective because the full permutation is).
    let mut distinct_in = 0usize;
    let mut seen = HashMap::new();
    for _ in 0..trials {
        let mut state = [
            random_subfield_lanes(&mut rng, 16)[0],
            random_subfield_lanes(&mut rng, 16)[1],
            random_subfield_lanes(&mut rng, 16)[0],
            random_subfield_lanes(&mut rng, 16)[1],
        ];
        let before = state;
        permute_flat_u128(&mut state);
        seen.insert(state, before);
        distinct_in += 1;
    }
    let restriction_is_injective = seen.len() == distinct_in;

    // ---------------------------------------------------------------
    // 3. Smaller subfields are NOT invariant.
    // ---------------------------------------------------------------
    let mut subfield_closure = |bits: u32| -> (bool, f64) {
        let mut all = true;
        let mut stayed = 0usize;
        let t = 256usize;
        for _ in 0..t {
            let mut state = [
                random_subfield_lanes(&mut rng, bits)[0],
                random_subfield_lanes(&mut rng, bits)[1],
                random_subfield_lanes(&mut rng, bits)[0],
                random_subfield_lanes(&mut rng, bits)[1],
            ];
            permute_flat_u128(&mut state);
            let ok = state.iter().all(|&l| in_subfield(l, bits));
            stayed += ok as usize;
            if !ok {
                all = false;
            }
        }
        (all, stayed as f64 / t as f64)
    };
    let (f4_invariant, f4_frac) = subfield_closure(4);
    let (f8_invariant, f8_frac) = subfield_closure(8);
    let (f16_invariant, f16_frac) = subfield_closure(16);
    let (f32_invariant, f32_frac) = subfield_closure(32);
    let (f64_invariant, f64_frac) = subfield_closure(64);
    assert!(!f4_invariant && !f8_invariant && f16_invariant && f32_invariant && f64_invariant);

    // ---------------------------------------------------------------
    // 4. The production collision pair is exactly subfield-aligned; and a
    //    fresh 2^16 birthday collision on the restricted node function.
    // ---------------------------------------------------------------
    let (a0, b0, a1, b1) = known_collision();
    let iv_c = capacity_iv_flat(TAG_CAPSNODE);
    let left_aligned = lanes_in_subfield(decode_lanes(&a0), 16)
        && lanes_in_subfield(decode_lanes(&a1), 16);
    let right_coset = lanes_in_subfield(
        [
            decode_lanes(&b0)[0] ^ iv_c[0],
            decode_lanes(&b0)[1] ^ iv_c[1],
        ],
        16,
    ) && lanes_in_subfield(
        [
            decode_lanes(&b1)[0] ^ iv_c[0],
            decode_lanes(&b1)[1] ^ iv_c[1],
        ],
        16,
    );
    let digest = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &a0, &b0);
    let digest_aligned = lanes_in_subfield(decode_lanes(&digest), 16);
    assert_eq!(
        digest,
        compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &a1, &b1)
    );

    // Fresh birthday regeneration of a zero-left aligned collision
    // (left = 0 is in K^2; only the right input varies in the coset).
    let started = Instant::now();
    let mut seen_right: HashMap<[u8; 32], [u8; 32]> = HashMap::new();
    let zero = [0u8; 32];
    let mut fresh_evals = 0u64;
    let fresh = loop {
        let k = random_subfield_lanes(&mut rng, 16);
        let right = encode_lanes([k[0] ^ iv_c[0], k[1] ^ iv_c[1]]);
        let d = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &zero, &right);
        fresh_evals += 1;
        if let Some(old) = seen_right.insert(d, right) {
            // A redrawn input (input birthday ~2^16 as well) is not a
            // digest collision; only distinct rights count.
            if old != right {
                break (old, right, d);
            }
        }
    };
    let fresh_seconds = started.elapsed().as_secs_f64();
    assert_eq!(
        compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &zero, &fresh.0),
        compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &zero, &fresh.1)
    );

    // ---------------------------------------------------------------
    // 5. The two-permutation sponge compressor does NOT preserve
    //    alignment: its initial state is [a0, a1, IV_hi, IV_lo] with the
    //    IV in the capacity lanes, IV not in K^2. K^4-invariance of the
    //    permutation (step 2) then implies the state can never be placed
    //    in K^4 by any choice of the data lanes, so aligned inputs do not
    //    produce aligned digests. `compress_with_tag` reads its 32-byte
    //    inputs as TOWER lanes (Block128::from), so the aligned inputs
    //    here are tower encodings < 2^16.
    // ---------------------------------------------------------------
    let iv2 = capacity_iv(TAG_COMPRESS);
    let iv2_in_k = iv2
        .iter()
        .all(|w| in_subfield(noid_core::hardware::tower_to_flat_u128(w.0), 16));
    let tower_bytes = |v0: u128, v1: u128| {
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(&v0.to_le_bytes());
        out[16..].copy_from_slice(&v1.to_le_bytes());
        out
    };
    let sponge_trials = 4096usize;
    let mut sponge_aligned_digests = 0usize;
    for _ in 0..sponge_trials {
        let (v0, v1) = (next_u64(&mut rng) & 0xffff, next_u64(&mut rng) & 0xffff);
        let (w0, w1) = (next_u64(&mut rng) & 0xffff, next_u64(&mut rng) & 0xffff);
        let left = tower_bytes(v0 as u128, v1 as u128);
        let right = tower_bytes(w0 as u128, w1 as u128);
        let d = compress_with_tag(TAG_COMPRESS, &left, &right);
        // decode_lanes returns the digest's tower encodings directly here,
        // since compress_with_tag serializes tower lanes to bytes.
        let t = decode_lanes(&d);
        let tower_small = t[0] >> 16 == 0 && t[1] >> 16 == 0;
        if tower_small {
            sponge_aligned_digests += 1;
        }
    }

    println!(
        "{}",
        json!({
            "experiment": "subfield invariance characterization of the production permutation",
            "constants_max_tower_bits": {
                "round_constants": 128 - rc_max.leading_zeros(),
                "mds_full": 128 - mds_full_max.leading_zeros(),
                "mds_partial": 128 - mds_partial_max.leading_zeros(),
            },
            "constants_all_in_GF2_16": constants_in_k,
            "smallest_covering_subfield_bits": smallest_covering_subfield_bits,
            "invariance_trials": trials,
            "GF2_16_pow4_invariant": k_invariant,
            "restriction_injective_on_samples": restriction_is_injective,
            "subfield_invariance": {
                "GF2_4": {"invariant": f4_invariant, "stayed_fraction": f4_frac},
                "GF2_8": {"invariant": f8_invariant, "stayed_fraction": f8_frac},
                "GF2_16": {"invariant": f16_invariant, "stayed_fraction": f16_frac},
                "GF2_32": {"invariant": f32_invariant, "stayed_fraction": f32_frac},
                "GF2_64": {"invariant": f64_invariant, "stayed_fraction": f64_frac},
            },
            "known_production_collision": {
                "left_children_in_GF2_16": left_aligned,
                "right_children_in_IV_plus_GF2_16": right_coset,
                "digest_in_GF2_16": digest_aligned,
                "digest": hex::encode(digest),
            },
            "fresh_zero_left_birthday": {
                "evaluations": fresh_evals,
                "seconds": fresh_seconds,
                "digest": hex::encode(fresh.2),
            },
            "two_permutation_compressor_control": {
                "tag_iv_in_GF2_16": iv2_in_k,
                "trials": sponge_trials,
                "aligned_digests": sponge_aligned_digests,
                "note": "compress_with_tag permutes the IV into all lanes before data addition; alignment cannot be forced from the data lanes",
            },
        })
    );
}
