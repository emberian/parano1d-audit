//! Reproducible checks of selected Gemini cryptography and model claims.
//! Synthetic inputs only. This program does not construct a network proof.

use std::{collections::HashMap, time::Instant};

use noid_core::{
    hardware::{clmul_gcm, flat_to_tower_u128, tower_to_flat_u128, Flat, HardwareField},
    sumcheck::prove::RoundPolynomial,
    transcript::FiatShamir,
    Block128, TowerField,
};
use noid_poseidon2b::{
    hasher::CryptographicHasher,
    native::{
        capacity_iv, compress_with_tag, poseidon2b_digest, poseidon2b_hash_bytes, DomainTag,
        Poseidon2bPermutation, Poseidon2bSponge, TAG_EXSTNOD,
    },
    Poseidon2bChannel,
};
use noid_soundness::{parameters::ProductionParameters, qrom::history_query_squeeze_permutations};

const SEARCH_LIMIT: u32 = 500_000;

fn first_block(a: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&((a & 0xffff) as u128).to_le_bytes());
    out[16..].copy_from_slice(&((a >> 16) as u128).to_le_bytes());
    out
}

fn after_first_block(a: u32) -> [Block128; 4] {
    let mut state = [
        Block128((a & 0xffff) as u128),
        Block128((a >> 16) as u128),
        Block128::ZERO,
        Block128::ZERO,
    ];
    Poseidon2bPermutation.permute_mut(&mut state);
    state
}

fn sponge_digest(data: &[u8]) -> [u8; 32] {
    let mut sponge = Poseidon2bSponge::new();
    sponge.update(data);
    sponge.finalize()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}

fn collision() {
    let start = Instant::now();
    let mut seen: HashMap<(u128, u128), (u32, [Block128; 4])> = HashMap::new();
    for i in 0..SEARCH_LIMIT {
        // Odd multiplication is an injective traversal of 32-bit pairs.
        let candidate = i.wrapping_mul(0x9e37_79b1).wrapping_add(0x243f_6a88);
        let state = after_first_block(candidate);
        assert_eq!(state[2].0 >> 16, 0, "capacity left GF(2^16)");
        assert_eq!(state[3].0 >> 16, 0, "capacity left GF(2^16)");
        let key = (state[2].0, state[3].0);
        if let Some((other, other_state)) = seen.insert(key, (candidate, state)) {
            assert_ne!(candidate, other);
            assert_ne!((other_state[0], other_state[1]), (state[0], state[1]));
            let mut joined = state;
            joined[0] += other_state[0] + state[0];
            joined[1] += other_state[1] + state[1];
            assert_eq!(
                joined, other_state,
                "second-block correction must equalize full state"
            );
            let mut left = [0u8; 64];
            let mut right = [0u8; 64];
            left[..32].copy_from_slice(&first_block(other));
            right[..32].copy_from_slice(&first_block(candidate));
            right[32..48].copy_from_slice(&(other_state[0].0 ^ state[0].0).to_le_bytes());
            right[48..64].copy_from_slice(&(other_state[1].0 ^ state[1].0).to_le_bytes());
            assert_ne!(left, right);
            let left_digest = sponge_digest(&left);
            let right_digest = sponge_digest(&right);
            assert_eq!(
                left_digest, right_digest,
                "full padded digest collision failed"
            );
            assert_eq!(poseidon2b_digest(&left), left_digest);
            assert_eq!(poseidon2b_digest(&right), right_digest);
            let tagged_differ = poseidon2b_hash_bytes(b"verification", &left)
                != poseidon2b_hash_bytes(b"verification", &right);
            assert!(tagged_differ, "tagged control unexpectedly collided");
            let hasher = Poseidon2bSponge::new();
            let mut left_a = [0u8; 32];
            let mut left_b = [0u8; 32];
            let mut right_a = [0u8; 32];
            let mut right_b = [0u8; 32];
            left_a.copy_from_slice(&left[..32]);
            left_b.copy_from_slice(&left[32..]);
            right_a.copy_from_slice(&right[..32]);
            right_b.copy_from_slice(&right[32..]);
            assert_eq!(hasher.hash_concatenation(&left_a, &left_b), left_digest);
            assert_eq!(hasher.hash_concatenation(&right_a, &right_b), right_digest);
            let exact_node_differs = compress_with_tag(TAG_EXSTNOD, &left_a, &left_b)
                != compress_with_tag(TAG_EXSTNOD, &right_a, &right_b);
            assert!(
                exact_node_differs,
                "exact-state node control unexpectedly collided"
            );
            println!(
                "collision_status=confirmed iterations={} elapsed_ms={}",
                i + 1,
                start.elapsed().as_millis()
            );
            println!("left_64={}", hex(&left));
            println!("right_64={}", hex(&right));
            println!("digest_32={}", hex(&left_digest));
            println!("tagged_hash_control_differs={}", tagged_differ);
            println!("exact_state_node_control_differs={}", exact_node_differs);
            return;
        }
    }
    println!(
        "collision_status=not_found limit={} elapsed_ms={}",
        SEARCH_LIMIT,
        start.elapsed().as_millis()
    );
}

fn receiver_iv() {
    let iv = capacity_iv(DomainTag::new(b"IVCHECK_"));
    let seeded = Poseidon2bSponge::with_iv(iv);
    let zero = Poseidon2bSponge::new();
    let a = Block128(7);
    let b = Block128(13);
    let h = [3u8; 32];
    let g = [5u8; 32];
    let same_pair = seeded.hash_pair(&a, &b) == zero.hash_pair(&a, &b);
    let same_field = seeded.hash_field(&a) == zero.hash_field(&a);
    let same_concat = seeded.hash_concatenation(&h, &g) == zero.hash_concatenation(&h, &g);
    let mut seeded_direct = Poseidon2bSponge::with_iv(iv);
    seeded_direct.update(&h);
    seeded_direct.update(&g);
    let direct_differs = seeded_direct.finalize() != zero.hash_concatenation(&h, &g);
    assert!(same_pair && same_field && same_concat && direct_differs);
    println!(
        "receiver_iv pair_same={} field_same={} concatenation_same={} seeded_direct_differs={}",
        same_pair, same_field, same_concat, direct_differs
    );
}

fn soundness_profile() {
    let production = ProductionParameters::load().expect("production parameters");
    let current = history_query_squeeze_permutations(&production);
    println!(
        "production_history classes={:?} squeeze_permutations={}",
        production
            .history_classes
            .iter()
            .map(|c| (c.tier, c.codeword_log2))
            .collect::<Vec<_>>(),
        current
    );
    let mut synthetic = production.clone();
    synthetic.history_classes[0].codeword_log2 = 22;
    let mixed = history_query_squeeze_permutations(&synthetic);
    let class_squeezes: Vec<usize> = synthetic
        .history_classes
        .iter()
        .map(|c| {
            (synthetic.history_queries as usize)
                .div_ceil(128 / c.codeword_log2)
                .div_ceil(synthetic.poseidon_rate_lanes)
        })
        .collect();
    assert_eq!(current, 12);
    assert_eq!(mixed as usize, *class_squeezes.iter().max().unwrap());
    println!(
        "synthetic_history classes={:?} class_squeezes={:?} selected={}",
        synthetic
            .history_classes
            .iter()
            .map(|c| (c.tier, c.codeword_log2))
            .collect::<Vec<_>>(),
        class_squeezes,
        mixed
    );
}

fn portability_fixture() {
    // The non-x86/non-aarch64 HardwareField fallback treats a tower-encoded
    // word as already flat. Compare that source-branch operation to the
    // supported-host conversion on identical input words.
    let candidates = [
        2u128,
        3,
        4,
        8,
        0x80,
        0x100,
        0x10000,
        1u128 << 32,
        1u128 << 64,
    ];
    let (a, b) = candidates
        .iter()
        .copied()
        .flat_map(|a| {
            candidates
                .iter()
                .copied()
                .map(move |b| (Block128(a), Block128(b)))
        })
        .find(|(a, b)| clmul_gcm(a.0, b.0) != (*a * *b).0)
        .expect("targeted raw-flat vs tower witness");
    let canonical = (Flat(a).to_flat() * Flat(b).to_flat()).to_tower().inner();
    let fallback_raw = Block128(clmul_gcm(a.0, b.0));
    let tower = a * b;
    assert_eq!(canonical, tower);
    assert_ne!(fallback_raw, tower);
    let flat_a = tower_to_flat_u128(a.0);
    assert_eq!(flat_to_tower_u128(flat_a), a.0);
    let flat_inverse = Flat(Block128(flat_a)).invert();
    let fallback_inverse = Block128(flat_a).invert();
    let from_raw_differs =
        Flat::<Block128>::from(0x80u32).inner() != Flat(Block128(0x80)).to_flat().inner();
    assert!(from_raw_differs);
    println!("portability_fixture a={:032x} b={:032x} tower_product={:032x} source_fallback_raw_product={:032x} flat_inverse={:032x} source_fallback_inverse={:032x}", a.0, b.0, tower.0, fallback_raw.0, flat_inverse.0.0, fallback_inverse.0);
    println!(
        "flat_from_u32_differs_from_tower_converted={}",
        from_raw_differs
    );
}

fn channel_edges() {
    let mut empty = Poseidon2bChannel::new();
    let first_empty = empty.squeeze();
    let second_empty = empty.squeeze();
    assert_eq!(first_empty, Block128::ZERO);
    assert_eq!(second_empty, Block128::ZERO);

    // A half-filled rate block is flushed by appending exactly these bytes.
    // An explicit second lane with the same bytes produces the same state.
    let pad_lane = Block128((1u128 << 120) | 0x80);
    let first_lane = Block128(0x1234);
    let mut half = Poseidon2bChannel::new();
    half.absorb(first_lane);
    let mut explicit = Poseidon2bChannel::new();
    explicit.absorb(first_lane);
    explicit.absorb(pad_lane);
    let paired_equal = (0..4).all(|_| half.squeeze() == explicit.squeeze());
    assert!(paired_equal);
    println!("channel_edges empty_first={:032x} empty_second={:032x} half_vs_explicit_padding_four_squeezes_equal={}", first_empty.0, second_empty.0, paired_equal);

    let mut one_squeeze = Poseidon2bChannel::new();
    let mut two_squeezes = Poseidon2bChannel::new();
    one_squeeze.squeeze();
    two_squeezes.squeeze();
    two_squeezes.squeeze();
    one_squeeze.absorb(Block128(91));
    two_squeezes.absorb(Block128(91));
    let squeeze_count_alias = (0..4).all(|_| one_squeeze.squeeze() == two_squeezes.squeeze());
    assert!(squeeze_count_alias);
    let empty_poly = RoundPolynomial::<Block128>::from_coeffs(vec![]);
    assert_eq!(empty_poly.degree(), 0);
    assert_eq!(empty_poly.evaluate(Block128(9)), Block128::ZERO);
    println!(
        "channel_edges one_vs_two_squeezes_then_absorb_alias={} empty_round_polynomial_degree={}",
        squeeze_count_alias,
        empty_poly.degree()
    );

    // This public method's whole-block precondition is only a debug assert.
    // A release binary can discard a buffered half-block here.
    if !cfg!(debug_assertions) {
        let mut pending = Poseidon2bSponge::new();
        pending.absorb(Block128(17));
        let ignored = pending.finalize_no_pad();
        let empty_no_pad = Poseidon2bSponge::new().finalize_no_pad();
        assert_eq!(ignored, empty_no_pad);
        println!("finalize_no_pad_release_half_block_ignored=true");
    }
}

fn main() {
    receiver_iv();
    soundness_profile();
    portability_fixture();
    channel_edges();
    collision();
}
