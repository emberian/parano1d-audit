use std::sync::OnceLock;
use std::time::Instant;

use noid_core::{Block128, CanonicalSerialize, TowerField};
use noid_poseidon2b::native::domain::{capacity_iv, TAG_ADDRFIX};
use noid_poseidon2b::native::permutation::{
    Poseidon2bPermutation, F_ROUNDS, MDS_FULL, MDS_PARTIAL, N_ROUNDS, P_ROUNDS, ROUND_CONSTANTS,
    STATE_SIZE,
};
use noid_poseidon2b::primitives::{derive_address, SpendSecret};

pub type F = Block128;
pub type State = [F; STATE_SIZE];
pub type Address = [F; 2];

pub fn address(secret: [F; 2]) -> Address {
    let iv = capacity_iv(TAG_ADDRFIX);
    let mut state = [secret[0], secret[1], iv[0], iv[1]];
    Poseidon2bPermutation.permute_mut(&mut state);
    [state[0], state[1]]
}

pub fn reference_permute(state: &mut State, rounds: usize) {
    assert!(rounds <= N_ROUNDS);
    apply_mds(state, &MDS_FULL);
    for r in 0..rounds {
        if !(F_ROUNDS / 2..F_ROUNDS / 2 + P_ROUNDS).contains(&r) {
            for i in 0..STATE_SIZE {
                state[i] += F::from(ROUND_CONSTANTS[i][r]);
                state[i] = sbox(state[i]);
            }
            apply_mds(state, &MDS_FULL);
        } else {
            state[0] += F::from(ROUND_CONSTANTS[0][r]);
            state[0] = sbox(state[0]);
            apply_mds(state, &MDS_PARTIAL);
        }
    }
}

pub fn inverse_permute(state: &mut State) {
    for r in (0..N_ROUNDS).rev() {
        if !(F_ROUNDS / 2..F_ROUNDS / 2 + P_ROUNDS).contains(&r) {
            apply_inverse_mds(state, inverse_full());
            for i in 0..STATE_SIZE {
                state[i] = inverse_sbox(state[i]);
                state[i] += F::from(ROUND_CONSTANTS[i][r]);
            }
        } else {
            apply_inverse_mds(state, inverse_partial());
            state[0] = inverse_sbox(state[0]);
            state[0] += F::from(ROUND_CONSTANTS[0][r]);
        }
    }
    apply_inverse_mds(state, inverse_full());
}

pub fn frobenius(mut x: F, power: u32) -> F {
    for _ in 0..power {
        x = x.square();
    }
    x
}

pub fn minimal_subfield(x: F) -> u32 {
    [1, 2, 4, 8, 16, 32, 64, 128]
        .into_iter()
        .find(|&d| frobenius(x, d) == x)
        .unwrap()
}

pub fn common_subfield(values: impl IntoIterator<Item = F>) -> u32 {
    values.into_iter().map(minimal_subfield).max().unwrap_or(1)
}

pub fn hex(x: F) -> String {
    format!("{:032x}", x.0)
}

pub fn splitmix64(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

pub fn random_field(seed: &mut u64) -> F {
    let lo = splitmix64(seed) as u128;
    let hi = splitmix64(seed) as u128;
    Block128((hi << 64) | lo)
}

fn sbox(x: F) -> F {
    let x2 = x * x;
    let x4 = x2 * x2;
    x4 * (x2 * x)
}

fn inverse_sbox(x: F) -> F {
    pow(x, 0x4924_9249_2492_4924_9249_2492_4924_9249)
}

fn pow(mut base: F, mut exponent: u128) -> F {
    let mut result = F::ONE;
    while exponent != 0 {
        if exponent & 1 == 1 {
            result *= base;
        }
        base *= base;
        exponent >>= 1;
    }
    result
}

fn apply_mds(state: &mut State, matrix: &[[u128; STATE_SIZE]; STATE_SIZE]) {
    let input = *state;
    for i in 0..STATE_SIZE {
        state[i] = (0..STATE_SIZE)
            .map(|j| input[j] * F::from(matrix[i][j]))
            .fold(F::ZERO, |a, b| a + b);
    }
}

fn apply_inverse_mds(state: &mut State, matrix: &[[F; STATE_SIZE]; STATE_SIZE]) {
    let input = *state;
    for i in 0..STATE_SIZE {
        state[i] = (0..STATE_SIZE)
            .map(|j| input[j] * matrix[i][j])
            .fold(F::ZERO, |a, b| a + b);
    }
}

fn inverse_full() -> &'static [[F; STATE_SIZE]; STATE_SIZE] {
    static INVERSE: OnceLock<[[F; STATE_SIZE]; STATE_SIZE]> = OnceLock::new();
    INVERSE.get_or_init(|| invert_matrix(&MDS_FULL.map(|row| row.map(F::from))))
}

fn inverse_partial() -> &'static [[F; STATE_SIZE]; STATE_SIZE] {
    static INVERSE: OnceLock<[[F; STATE_SIZE]; STATE_SIZE]> = OnceLock::new();
    INVERSE.get_or_init(|| invert_matrix(&MDS_PARTIAL.map(|row| row.map(F::from))))
}

fn invert_matrix(matrix: &[[F; STATE_SIZE]; STATE_SIZE]) -> [[F; STATE_SIZE]; STATE_SIZE] {
    let mut augmented = [[F::ZERO; STATE_SIZE * 2]; STATE_SIZE];
    for i in 0..STATE_SIZE {
        for j in 0..STATE_SIZE {
            augmented[i][j] = matrix[i][j];
        }
        augmented[i][STATE_SIZE + i] = F::ONE;
    }
    for col in 0..STATE_SIZE {
        let pivot = (col..STATE_SIZE)
            .find(|&row| augmented[row][col] != F::ZERO)
            .expect("MDS matrix must be invertible");
        augmented.swap(col, pivot);
        let scale = augmented[col][col].invert();
        for j in 0..STATE_SIZE * 2 {
            augmented[col][j] *= scale;
        }
        for row in 0..STATE_SIZE {
            if row != col {
                let factor = augmented[row][col];
                for j in 0..STATE_SIZE * 2 {
                    augmented[row][j] += augmented[col][j] * factor;
                }
            }
        }
    }
    std::array::from_fn(|i| std::array::from_fn(|j| augmented[i][STATE_SIZE + j]))
}

pub fn structure_experiment() {
    assert_eq!(N_ROUNDS, 66);
    let iv = capacity_iv(TAG_ADDRFIX);
    let constants = ROUND_CONSTANTS
        .iter()
        .flat_map(|row| row.iter())
        .copied()
        .map(F::from);
    let matrices = MDS_FULL
        .iter()
        .chain(MDS_PARTIAL.iter())
        .flat_map(|row| row.iter())
        .copied()
        .map(F::from);
    println!("implementation_rounds={N_ROUNDS} (8 full + 58 partial)");
    println!("addrfix_iv0={}", hex(iv[0]));
    println!("addrfix_iv1={}", hex(iv[1]));
    println!(
        "round_constants_common_subfield=GF(2^{})",
        common_subfield(constants)
    );
    println!("mds_common_subfield=GF(2^{})", common_subfield(matrices));
    println!("iv0_minimal_subfield=GF(2^{})", minimal_subfield(iv[0]));
    println!("iv1_minimal_subfield=GF(2^{})", minimal_subfield(iv[1]));
    println!("iv_common_subfield=GF(2^{})", common_subfield(iv));
    println!("lane_wise_proper_subfield_state_closure_applicable=false");

    let mut seed = 0x5293_8433_4144_4452;
    let mut native_matches = 0;
    let mut inverse_roundtrips = 0;
    let mut commutation_checks = 0;
    for _ in 0..16 {
        let mut state = std::array::from_fn(|_| random_field(&mut seed));
        let original = state;
        let mut native = state;
        Poseidon2bPermutation.permute_mut(&mut native);
        reference_permute(&mut state, N_ROUNDS);
        assert_eq!(state, native);
        native_matches += 1;
        inverse_permute(&mut state);
        assert_eq!(state, original);
        inverse_roundtrips += 1;

        for k in [16, 32, 64] {
            let conjugated = original.map(|x| frobenius(x, k));
            let left = native.map(|x| frobenius(x, k));

            for rounds in 0..N_ROUNDS {
                let mut left_prefix = original;
                let mut right_prefix = conjugated;
                reference_permute(&mut left_prefix, rounds);
                reference_permute(&mut right_prefix, rounds);
                assert_eq!(
                    left_prefix.map(|x| frobenius(x, k)),
                    right_prefix,
                    "first_divergence rounds={rounds} frobenius_power={k}"
                );
            }
            let mut right = conjugated;
            reference_permute(&mut right, N_ROUNDS);
            assert_eq!(left, right, "frobenius_power={k}");
            commutation_checks += 1;
        }
    }
    for x in [F::ZERO, F::ONE, Block128(2), Block128(0xdead_beef)] {
        assert_eq!(sbox(inverse_sbox(x)), x);
        assert_eq!(inverse_sbox(sbox(x)), x);
    }
    println!("native_reference_matches={native_matches}/16");
    println!("full_round_inverse_roundtrips={inverse_roundtrips}/16");
    println!("full_state_frobenius_commutation_checks={commutation_checks}/48");

    let mut equivalent = 0;
    let mut relations_tested = 0;
    for _ in 0..16 {
        let secret = [random_field(&mut seed), random_field(&mut seed)];
        let original_address = address(secret);
        for k in [1, 2, 4, 8, 16, 32, 64] {
            let conjugate_address = address(secret.map(|x| frobenius(x, k)));
            let conjugated_original = original_address.map(|x| frobenius(x, k));
            equivalent += usize::from(conjugate_address == original_address);
            assert_ne!(conjugate_address, conjugated_original);
            relations_tested += 1;
        }
    }
    println!("secret_only_frobenius_equivalent_keys={equivalent}/{relations_tested}");
    println!("secret_only_frobenius_address_relations=0/{relations_tested}");
    println!("blocker=ADDRFIX_IV_is_not_fixed_by_any_nontrivial_frobenius");

    let secret = [
        Block128(0x0123_4567_89ab_cdef),
        Block128(0xfedc_ba98_7654_3210),
    ];
    let target = address(secret);
    let started = Instant::now();
    let mut candidate = [target[0], target[1], F::ZERO, F::ZERO];
    inverse_permute(&mut candidate);
    let mut forward_check = candidate;
    reference_permute(&mut forward_check, N_ROUNDS);
    assert_eq!([forward_check[0], forward_check[1]], target);
    println!(
        "one_hidden_lane_inverse_trial_micros={}",
        started.elapsed().as_micros()
    );
    println!(
        "one_hidden_lane_inverse_capacity_matches={}",
        [candidate[2], candidate[3]] == iv
    );
    println!("generic_inverse_candidates=2^256; success_probability_per_candidate=2^-256");
}

pub fn weak_key_experiment() {
    use std::collections::HashMap;

    let target_secret = [Block128(0xa5), Block128(0x5a)];
    let target = address(target_secret);
    let mut serialized = [0u8; 32];
    serialized[..16].copy_from_slice(&target_secret[0].to_bytes());
    serialized[16..].copy_from_slice(&target_secret[1].to_bytes());
    let public_api_address = derive_address(&SpendSecret::from_bytes(serialized));
    assert_eq!(&public_api_address.0[..16], &target[0].to_bytes());
    assert_eq!(&public_api_address.0[16..], &target[1].to_bytes());

    let mut seen: HashMap<[u128; 2], u32> = HashMap::with_capacity(1 << 16);
    let mut target_preimages = Vec::new();
    let mut collision_events = 0usize;
    let mut max_preimages = 0u32;
    let mut small_output_addresses = 0usize;
    let started = Instant::now();
    for candidate in 0u32..=u16::MAX as u32 {
        let secret = [
            Block128((candidate & 0xff) as u128),
            Block128((candidate >> 8) as u128),
        ];
        let digest = address(secret);
        let count = seen.entry([digest[0].0, digest[1].0]).or_insert(0);
        *count += 1;
        if *count == 2 {
            collision_events += 1;
        }
        max_preimages = max_preimages.max(*count);
        if digest == target {
            target_preimages.push(candidate);
        }
        if common_subfield(digest) <= 8 {
            small_output_addresses += 1;
        }
    }
    let elapsed = started.elapsed();
    assert_eq!(target_preimages, vec![0x5aa5]);
    assert_eq!(seen.len(), 1 << 16);
    assert_eq!(collision_events, 0);
    println!("scope=EXACT_FULL_66_ROUNDS_GF(2^128)_ADDRFIX");
    println!("weak_class=each_secret_lane_is_a_raw_u8_value");
    println!("weak_class_size=65536=2^16");
    println!("uniform_256_bit_secret_probability=2^-240");
    println!("target_secret0=000000000000000000000000000000a5");
    println!("target_secret1=0000000000000000000000000000005a");
    println!("target_address0={}", hex(target[0]));
    println!("target_address1={}", hex(target[1]));
    println!("evaluations=65536");
    println!("elapsed_seconds={:.6}", elapsed.as_secs_f64());
    println!(
        "evaluations_per_second={:.0}",
        65536.0 / elapsed.as_secs_f64()
    );
    println!("recovered_candidates={}", target_preimages.len());
    println!("recovered_combined_hex={:04x}", target_preimages[0]);
    println!("distinct_addresses={}", seen.len());
    println!("collision_events={collision_events}");
    println!("max_preimages_per_address={max_preimages}");
    println!("both_output_lanes_in_GF(2^8)={small_output_addresses}/65536");
    println!("unrestricted_generic_classical_evaluations=2^256");
    println!("known_weak_class_classical_worst_case=2^16");
    println!("unrestricted_generic_grover_queries=2^128");
    println!("known_weak_class_grover_queries=2^8");
    println!("conclusion=low_entropy_weakness_only_not_a_uniform_key_attack");
}
