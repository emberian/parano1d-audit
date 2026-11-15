use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use noid_core::{Block8, TowerField};
use noid_poseidon2b::native::permutation::{
    F_ROUNDS, MDS_FULL, MDS_PARTIAL, N_ROUNDS, P_ROUNDS, ROUND_CONSTANTS, STATE_SIZE,
};

type F = Block8;
type State = [F; STATE_SIZE];

const SYNTHETIC_IV: [F; 2] = [Block8(0x5a), Block8(0xa5)];

fn address(secret: [F; 2]) -> [F; 2] {
    let mut state = [secret[0], secret[1], SYNTHETIC_IV[0], SYNTHETIC_IV[1]];
    permute(&mut state);
    [state[0], state[1]]
}

fn permute(state: &mut State) {
    apply_mds(state, &MDS_FULL);
    for r in 0..N_ROUNDS {
        if !(F_ROUNDS / 2..F_ROUNDS / 2 + P_ROUNDS).contains(&r) {
            for i in 0..STATE_SIZE {
                state[i] += Block8(ROUND_CONSTANTS[i][r] as u8);
                state[i] = sbox(state[i]);
            }
            apply_mds(state, &MDS_FULL);
        } else {
            state[0] += Block8(ROUND_CONSTANTS[0][r] as u8);
            state[0] = sbox(state[0]);
            apply_mds(state, &MDS_PARTIAL);
        }
    }
}

fn inverse(state: &mut State) {
    for r in (0..N_ROUNDS).rev() {
        if !(F_ROUNDS / 2..F_ROUNDS / 2 + P_ROUNDS).contains(&r) {
            apply_inverse_mds(state, inverse_full());
            for i in 0..STATE_SIZE {
                state[i] = pow(state[i], 73);
                state[i] += Block8(ROUND_CONSTANTS[i][r] as u8);
            }
        } else {
            apply_inverse_mds(state, inverse_partial());
            state[0] = pow(state[0], 73);
            state[0] += Block8(ROUND_CONSTANTS[0][r] as u8);
        }
    }
    apply_inverse_mds(state, inverse_full());
}

fn inverse_full() -> &'static [[F; STATE_SIZE]; STATE_SIZE] {
    static INVERSE: OnceLock<[[F; STATE_SIZE]; STATE_SIZE]> = OnceLock::new();
    INVERSE.get_or_init(|| invert_matrix(&MDS_FULL.map(|row| row.map(|x| Block8(x as u8)))))
}

fn inverse_partial() -> &'static [[F; STATE_SIZE]; STATE_SIZE] {
    static INVERSE: OnceLock<[[F; STATE_SIZE]; STATE_SIZE]> = OnceLock::new();
    INVERSE.get_or_init(|| invert_matrix(&MDS_PARTIAL.map(|row| row.map(|x| Block8(x as u8)))))
}

fn sbox(x: F) -> F {
    let x2 = x * x;
    let x4 = x2 * x2;
    x4 * (x2 * x)
}

fn pow(mut base: F, mut exponent: u8) -> F {
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
            .map(|j| input[j] * Block8(matrix[i][j] as u8))
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
            .expect("MDS matrix must be invertible over GF(2^8)");
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

pub fn reduced_field_experiment() {
    assert_eq!(N_ROUNDS, 66);
    for x in 0u16..=255 {
        let x = Block8(x as u8);
        assert_eq!(pow(sbox(x), 73), x);
        assert_eq!(sbox(pow(x, 73)), x);
    }

    let target_secret = [Block8(0x37), Block8(0xc2)];
    let target = address(target_secret);
    let mut forward_seen: HashMap<u16, u32> = HashMap::with_capacity(1 << 16);
    let mut forward_preimages = Vec::new();
    let forward_started = Instant::now();
    for candidate in 0u32..=u16::MAX as u32 {
        let secret = [Block8(candidate as u8), Block8((candidate >> 8) as u8)];
        let digest = address(secret);
        let encoded = u16::from_le_bytes([digest[0].0, digest[1].0]);
        *forward_seen.entry(encoded).or_insert(0) += 1;
        if digest == target {
            forward_preimages.push(candidate as u16);
        }
    }
    let forward_elapsed = forward_started.elapsed();

    let mut inverse_preimages = Vec::new();
    let inverse_started = Instant::now();
    for hidden in 0u32..=u16::MAX as u32 {
        let mut state = [
            target[0],
            target[1],
            Block8(hidden as u8),
            Block8((hidden >> 8) as u8),
        ];
        inverse(&mut state);
        if [state[2], state[3]] == SYNTHETIC_IV {
            assert_eq!(address([state[0], state[1]]), target);
            inverse_preimages.push(u16::from_le_bytes([state[0].0, state[1].0]));
        }
    }
    let inverse_elapsed = inverse_started.elapsed();
    forward_preimages.sort_unstable();
    inverse_preimages.sort_unstable();
    assert_eq!(forward_preimages, inverse_preimages);
    assert!(forward_preimages.contains(&0xc237));

    let equivalent_key_pairs: u64 = forward_seen
        .values()
        .map(|&count| (count as u64 * (count as u64 - 1)) / 2)
        .sum();
    println!("scope=REDUCED_FIELD_GF(2^8)_SYNTHETIC_IV_NOT_PARANOID_128_BIT_SECURITY");
    println!("rounds=66_FULL_SCHEDULE_NOT_REDUCED_ROUND");
    println!("secret_and_address_bits=16_each");
    println!("target_secret=37c2");
    println!("target_address={:02x}{:02x}", target[1].0, target[0].0);
    println!("forward_evaluations=65536");
    println!(
        "forward_elapsed_seconds={:.6}",
        forward_elapsed.as_secs_f64()
    );
    println!("inverse_hidden_lane_trials=65536");
    println!(
        "inverse_elapsed_seconds={:.6}",
        inverse_elapsed.as_secs_f64()
    );
    println!("forward_preimages={forward_preimages:?}");
    println!("inverse_preimages={inverse_preimages:?}");
    println!("distinct_addresses={}", forward_seen.len());
    println!("equivalent_key_pairs={equivalent_key_pairs}");
    println!("full_field_analog_forward_candidates=2^256");
    println!("full_field_analog_inverse_hidden_lane_candidates=2^256");
    println!("conclusion=inversion_exchanges_secret_guessing_for_hidden_output_guessing_no_asymptotic_gain");
}
