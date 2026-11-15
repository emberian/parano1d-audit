use std::collections::HashMap;
use std::time::Instant;

use noid_core::hardware::{flat_to_tower_u128, tower_to_flat_u128};
use noid_core::{Block128, TowerField};
use noid_ivc_core::challenger::{Challenger, FsLaneChallenger};
use noid_ivc_core::deep_chain::ff_merkle::{
    FfMerklePathFamily, FfMerklePathWitness, build_ff_merkle_path_columns,
};
use noid_ivc_core::field::{F128, F256};
use noid_ivc_core::merkle::{self, Hash};
use noid_ivc_core::ntt::AdditiveNttF128;
use noid_ivc_core::pcs::basefold::{self, BaseFoldProof, C1BaseFoldProof, VerifyError};
use noid_ivc_core::pcs::{LOG_PACKING, PcsParams, ProverData, commit, default_fri_queries};
use noid_poseidon2b::native::permutation::{
    F_ROUNDS, MDS_FULL, MDS_PARTIAL, N_ROUNDS, P_ROUNDS, Poseidon2bPermutation, ROUND_CONSTANTS,
    STATE_SIZE,
};
use noid_poseidon2b::native::{DomainTag, capacity_iv, capacity_iv_flat};

const NODE_TAG: DomainTag = DomainTag::new(b"IVCPCSN_");

#[derive(Clone, Copy)]
struct Collision {
    first_left: Hash,
    first_right: Hash,
    second_left: Hash,
    second_right: Hash,
    digest: Hash,
    first_at: u64,
    evaluations: u64,
}

fn encode(lanes: [u128; 2]) -> Hash {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&lanes[0].to_le_bytes());
    out[16..].copy_from_slice(&lanes[1].to_le_bytes());
    out
}

fn decode(hash: &Hash) -> [u128; 2] {
    [
        u128::from_le_bytes(hash[..16].try_into().unwrap()),
        u128::from_le_bytes(hash[16..].try_into().unwrap()),
    ]
}

fn xor_hash(mut left: Hash, right: &Hash) -> Hash {
    for (a, b) in left.iter_mut().zip(right) {
        *a ^= *b;
    }
    left
}

fn flat_f128(value: u128) -> F128 {
    F128::new(value as u64, (value >> 64) as u64)
}

fn digest_f128s(hash: &Hash) -> [F128; 2] {
    decode(hash).map(flat_f128)
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn f128(&mut self) -> F128 {
        F128::new(self.next_u64(), self.next_u64())
    }
}

fn find_node_collision() -> Collision {
    let started = Instant::now();
    let iv = capacity_iv_flat(NODE_TAG);
    let mut rng = Rng(0x4956_4350_4353_4e5f);
    let mut seen = HashMap::new();

    for evaluations in 1..=1_000_000u64 {
        let subfield = std::array::from_fn::<u128, 4, _>(|_| {
            tower_to_flat_u128((rng.next_u64() & 0xffff) as u128)
        });
        let left = encode([subfield[0], subfield[1]]);
        let right = encode([subfield[2] ^ iv[0], subfield[3] ^ iv[1]]);
        let digest = merkle::hash_pair(&left, &right);

        for lane in decode(&digest) {
            assert!(flat_to_tower_u128(lane) <= 0xffff);
        }
        if let Some((old_left, old_right, first_at)) =
            seen.insert(digest, (left, right, evaluations))
        {
            assert_ne!((old_left, old_right), (left, right));
            assert_eq!(digest, merkle::hash_pair(&old_left, &old_right));
            println!("ivcpcsn_collision_evaluations={evaluations}");
            println!("ivcpcsn_collision_first_at={first_at}");
            println!(
                "ivcpcsn_collision_elapsed_seconds={:.3}",
                started.elapsed().as_secs_f64()
            );
            println!("ivcpcsn_collision_digest={}", hex::encode(digest));
            println!("ivcpcsn_first_left={}", hex::encode(old_left));
            println!("ivcpcsn_first_right={}", hex::encode(old_right));
            println!("ivcpcsn_second_left={}", hex::encode(left));
            println!("ivcpcsn_second_right={}", hex::encode(right));
            return Collision {
                first_left: old_left,
                first_right: old_right,
                second_left: left,
                second_right: right,
                digest,
                first_at,
                evaluations,
            };
        }
    }
    panic!("no IVCPCSN collision within one million evaluations");
}

fn leaf_and_tree_controls(collision: &Collision) {
    let node_iv = capacity_iv_flat(NODE_TAG);
    let leaf_iv = merkle::leaf_fixed_iv_flat(32);
    let reconciling_right = encode([node_iv[0] ^ leaf_iv[0], node_iv[1] ^ leaf_iv[1]]);
    let zero = [0u8; 32];

    let leaf_zero = merkle::hash_leaf(&zero);
    let node_zero = merkle::hash_pair(&zero, &reconciling_right);
    assert_eq!(leaf_zero, node_zero);
    println!("leaf_node_type_confusion_zero_left=true");
    println!("leaf_node_shared_digest={}", hex::encode(leaf_zero));

    let arbitrary = encode([0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210]);
    let relation_left = merkle::hash_pair(&arbitrary, &reconciling_right);
    let relation_right = xor_hash(merkle::hash_leaf(&arbitrary), &arbitrary);
    assert_eq!(relation_left, relation_right);
    println!("leaf_node_type_confusion_feed_forward_relation=true");

    let leaf_iv_tower = leaf_iv.map(flat_to_tower_u128);
    assert_ne!(leaf_iv_tower[0] >> 64, 0);
    println!("fixed_leaf_iv_high_tower={:#034x}", leaf_iv_tower[0]);
    println!("fixed_leaf_iv_outside_gf2_64=true");

    let small_message = encode([tower_to_flat_u128(0x1234), tower_to_flat_u128(0xabcd)]);
    let small_leaf = merkle::hash_leaf(&small_message);
    assert!(
        decode(&small_leaf)
            .map(flat_to_tower_u128)
            .iter()
            .any(|&x| x > 0xffff)
    );
    println!("small_input_leaf_hash_stays_in_gf2_16=false");
    println!("small_input_leaf_hash={}", hex::encode(small_leaf));

    let mut first_data = Vec::from(collision.first_left);
    first_data.extend_from_slice(&collision.first_right);
    let mut second_data = Vec::from(collision.second_left);
    second_data.extend_from_slice(&collision.second_right);
    let first_root = merkle::merkle_root(&first_data, 2);
    let second_root = merkle::merkle_root(&second_data, 2);
    assert_ne!(first_root, second_root);
    assert_ne!(first_root, collision.digest);
    assert_ne!(second_root, collision.digest);
    assert_eq!(
        merkle::merkle_tree(&first_data, 2),
        merkle::merkle_tree_sequential(&first_data, 2)
    );
    println!(
        "collision_inputs_as_real_leaf_bytes_root_0={}",
        hex::encode(first_root)
    );
    println!(
        "collision_inputs_as_real_leaf_bytes_root_1={}",
        hex::encode(second_root)
    );
    println!("arbitrary_node_collision_lifts_to_byte_tree=false");
}

fn sbox_inverse(mut value: Block128) -> Block128 {
    let mut exponent = 0x4924_9249_2492_4924_9249_2492_4924_9249u128;
    let mut result = Block128::ONE;
    while exponent != 0 {
        if exponent & 1 == 1 {
            result *= value;
        }
        value = value.square();
        exponent >>= 1;
    }
    result
}

fn inverse_matrix(
    mut matrix: [[Block128; STATE_SIZE]; STATE_SIZE],
) -> [[Block128; STATE_SIZE]; STATE_SIZE] {
    let mut inverse = [[Block128::ZERO; STATE_SIZE]; STATE_SIZE];
    for (row, out) in inverse.iter_mut().enumerate() {
        out[row] = Block128::ONE;
    }
    for column in 0..STATE_SIZE {
        let pivot = (column..STATE_SIZE)
            .find(|&row| matrix[row][column] != Block128::ZERO)
            .expect("singular production matrix");
        matrix.swap(column, pivot);
        inverse.swap(column, pivot);
        let scale = matrix[column][column].invert();
        for j in 0..STATE_SIZE {
            matrix[column][j] *= scale;
            inverse[column][j] *= scale;
        }
        for row in 0..STATE_SIZE {
            if row == column {
                continue;
            }
            let factor = matrix[row][column];
            for j in 0..STATE_SIZE {
                matrix[row][j] += factor * matrix[column][j];
                inverse[row][j] += factor * inverse[column][j];
            }
        }
    }
    inverse
}

fn apply_matrix(state: &mut [Block128; STATE_SIZE], matrix: &[[Block128; STATE_SIZE]; STATE_SIZE]) {
    let input = *state;
    for (i, out) in state.iter_mut().enumerate() {
        *out = (0..STATE_SIZE).fold(Block128::ZERO, |sum, j| sum + matrix[i][j] * input[j]);
    }
}

fn inverse_permutation_tower(state: &mut [Block128; STATE_SIZE]) {
    let full_inverse = inverse_matrix(MDS_FULL.map(|row| row.map(Block128::from)));
    let partial_inverse = inverse_matrix(MDS_PARTIAL.map(|row| row.map(Block128::from)));
    for round in (0..N_ROUNDS).rev() {
        if !(F_ROUNDS / 2..F_ROUNDS / 2 + P_ROUNDS).contains(&round) {
            apply_matrix(state, &full_inverse);
            for (lane, value) in state.iter_mut().enumerate() {
                *value = sbox_inverse(*value);
                *value += Block128::from(ROUND_CONSTANTS[lane][round]);
            }
        } else {
            apply_matrix(state, &partial_inverse);
            state[0] = sbox_inverse(state[0]);
            state[0] += Block128::from(ROUND_CONSTANTS[0][round]);
        }
    }
    apply_matrix(state, &full_inverse);
}

fn check_inverse_permutation() {
    let mut rng = Rng(0x494e_5650_4552_4d);
    for _ in 0..4 {
        let original = std::array::from_fn(|_| {
            Block128::from(((rng.next_u64() as u128) << 64) | rng.next_u64() as u128)
        });
        let mut forward = original;
        Poseidon2bPermutation.permute_mut(&mut forward);
        inverse_permutation_tower(&mut forward);
        assert_eq!(forward, original);
    }
    println!("inverse_permutation_roundtrips=4");
}

fn left_fixed_point() -> (Hash, Hash) {
    let iv = capacity_iv(NODE_TAG);
    for counter in 1..100u128 {
        let mut state = [
            Block128::ZERO,
            Block128::ZERO,
            Block128::from(counter),
            Block128::from(counter + 1),
        ];
        inverse_permutation_tower(&mut state);
        assert!(state.iter().all(|value| value.0 <= 0xffff));
        let left = encode([
            tower_to_flat_u128(state[0].0),
            tower_to_flat_u128(state[1].0),
        ]);
        if left == [0u8; 32] {
            continue;
        }
        let right = encode([
            tower_to_flat_u128(state[2].0 ^ iv[0].0),
            tower_to_flat_u128(state[3].0 ^ iv[1].0),
        ]);
        assert_eq!(merkle::hash_pair(&left, &right), left);
        return (left, right);
    }
    unreachable!()
}

fn raw_path_and_recursive_controls(collision: &Collision) {
    check_inverse_permutation();
    let (entry, sibling) = left_fixed_point();
    assert!(merkle::verify_merkle_proof(&entry, &entry, 0, &[]));
    assert!(merkle::verify_merkle_proof(&entry, &entry, 0, &[sibling]));
    assert!(merkle::verify_merkle_proof(
        &entry,
        &entry,
        0,
        &[sibling, sibling]
    ));
    println!("inverse_fixed_point_left={}", hex::encode(entry));
    println!("inverse_fixed_point_right={}", hex::encode(sibling));
    println!("raw_api_same_root_path_lengths=0,1,2");

    assert!(merkle::verify_merkle_proof(
        &collision.digest,
        &collision.first_left,
        0,
        &[collision.first_right],
    ));
    assert!(merkle::verify_merkle_proof(
        &collision.digest,
        &collision.second_left,
        0,
        &[collision.second_right],
    ));
    println!("raw_api_distinct_collision_openings_verify=true");

    let first_tree = vec![
        collision.first_left,
        collision.first_right,
        collision.digest,
    ];
    let second_tree = vec![
        collision.second_left,
        collision.second_right,
        collision.digest,
    ];
    let first_multi = merkle::merkle_multi_proof(&first_tree, 2, &[0]);
    let second_multi = merkle::merkle_multi_proof(&second_tree, 2, &[0]);
    assert!(merkle::verify_merkle_multi_proof(
        &collision.digest,
        2,
        &[0],
        &[collision.first_left],
        &first_multi,
    ));
    assert!(merkle::verify_merkle_multi_proof(
        &collision.digest,
        2,
        &[0],
        &[collision.second_left],
        &second_multi,
    ));
    println!("raw_ligerito_style_multi_proofs_share_root=true");

    let iv = capacity_iv_flat(NODE_TAG).map(flat_f128);
    let family = FfMerklePathFamily {
        depth: 1,
        n_paths: 2,
    };
    let witnesses = [
        FfMerklePathWitness {
            entry: digest_f128s(&collision.first_left),
            siblings: vec![digest_f128s(&collision.first_right)],
            directions: vec![false],
        },
        FfMerklePathWitness {
            entry: digest_f128s(&collision.second_left),
            siblings: vec![digest_f128s(&collision.second_right)],
            directions: vec![false],
        },
    ];
    let columns = build_ff_merkle_path_columns(&family, iv, &witnesses, 1);
    assert_eq!(columns.roots[0], digest_f128s(&collision.digest));
    assert_eq!(columns.roots[1], digest_f128s(&collision.digest));
    println!("recursive_collision_path_replays_share_root=true");

    let fixed_family = FfMerklePathFamily {
        depth: 3,
        n_paths: 1,
    };
    let fixed_witness = FfMerklePathWitness {
        entry: digest_f128s(&entry),
        siblings: vec![digest_f128s(&sibling); 3],
        directions: vec![false; 3],
    };
    let fixed_columns = build_ff_merkle_path_columns(&fixed_family, iv, &[fixed_witness], 2);
    assert_eq!(fixed_columns.roots[0], digest_f128s(&entry));
    println!("recursive_fixed_point_depth3_root_equals_entry=true");
}

struct Fixture {
    params: PcsParams,
    message: Vec<F128>,
    transparent: Vec<F128>,
    transparent_c1: Vec<F256>,
    target: F128,
    target_c1: F256,
    commitment_root: Hash,
    prover_data: ProverData,
    ntt: AdditiveNttF128,
}

fn fixture() -> Fixture {
    let log_message_len = 5;
    let params = PcsParams {
        m: log_message_len + LOG_PACKING,
        log_inv_rate: 2,
        log_batch_size: 1,
        profile: Default::default(),
    };
    let mut rng = Rng(0x4241_5345_464f_4c44);
    let message = (0..1usize << log_message_len)
        .map(|_| rng.f128())
        .collect::<Vec<_>>();
    let transparent = (0..message.len()).map(|_| rng.f128()).collect::<Vec<_>>();
    let transparent_c1 = (0..message.len())
        .map(|_| F256::new(rng.f128(), rng.f128()))
        .collect::<Vec<_>>();
    let target = message
        .iter()
        .zip(&transparent)
        .fold(F128::ZERO, |sum, (&left, &right)| sum + left * right);
    let target_c1 = message
        .iter()
        .zip(&transparent_c1)
        .fold(F256::ZERO, |sum, (&left, &right)| {
            sum + right.scale_base(left)
        });
    let (commitment, prover_data) = commit(&message, &params);
    let ntt = AdditiveNttF128::standard(params.k_code());
    Fixture {
        params,
        message,
        transparent,
        transparent_c1,
        target,
        target_c1,
        commitment_root: commitment.root,
        prover_data,
        ntt,
    }
}

fn c0_verify(fixture: &Fixture, proof: &BaseFoldProof) -> Result<Vec<F128>, VerifyError> {
    let mut challenger = FsLaneChallenger::new(b"basefold-chains-c0");
    basefold::verify(
        fixture.target,
        proof,
        &fixture.commitment_root,
        &fixture.ntt,
        fixture.params.log_msg_len(),
        fixture.params.log_inv_rate,
        fixture.params.log_batch_size,
        &mut challenger,
    )
}

fn c1_verify(fixture: &Fixture, proof: &C1BaseFoldProof) -> Result<Vec<F256>, VerifyError> {
    let mut challenger = FsLaneChallenger::new_c1(b"basefold-chains-c1");
    basefold::verify_c1(
        fixture.target_c1,
        proof,
        &fixture.commitment_root,
        &fixture.ntt,
        fixture.params.log_msg_len(),
        fixture.params.log_inv_rate,
        fixture.params.log_batch_size,
        &mut challenger,
    )
}

fn basefold_controls(collision: &Collision, fixed_sibling: Hash) {
    let fixture = fixture();
    let queries = default_fri_queries(fixture.params.log_dim(), fixture.params.log_inv_rate);
    println!(
        "basefold_fixture_log_dim={} k_code={} queries={queries}",
        fixture.params.log_dim(),
        fixture.params.k_code()
    );

    let mut c0_prover = FsLaneChallenger::new(b"basefold-chains-c0");
    let c0 = basefold::prove(
        &fixture.message,
        fixture.transparent.clone(),
        fixture.target,
        &fixture.prover_data.codeword,
        &fixture.prover_data.merkle_tree,
        &fixture.ntt,
        fixture.params.log_inv_rate,
        fixture.params.log_batch_size,
        queries,
        &mut c0_prover,
    );
    c0_verify(&fixture, &c0).expect("honest C0 BaseFold proof");
    let mut c0_verifier = FsLaneChallenger::new(b"basefold-chains-c0");
    basefold::verify(
        fixture.target,
        &c0,
        &fixture.commitment_root,
        &fixture.ntt,
        fixture.params.log_msg_len(),
        fixture.params.log_inv_rate,
        fixture.params.log_batch_size,
        &mut c0_verifier,
    )
    .unwrap();
    assert_eq!(c0_prover.sample_f128(), c0_verifier.sample_f128());
    println!("c0_honest_proof_and_transcript_lockstep=true");

    let mut short = c0.clone();
    short.queries[0].initial_path.pop();
    assert!(matches!(
        c0_verify(&fixture, &short),
        Err(VerifyError::InvalidProofShape)
    ));
    let mut long = c0.clone();
    long.queries[0].initial_path.push(fixed_sibling);
    assert!(matches!(
        c0_verify(&fixture, &long),
        Err(VerifyError::InvalidProofShape)
    ));
    println!("c0_short_and_fixed_point_extended_paths=InvalidProofShape");

    let mut lifted = c0.clone();
    lifted.queries[0].initial_leaf = digest_f128s(&collision.first_left).to_vec();
    lifted.queries[0].initial_path[0] = collision.first_right;
    assert!(matches!(
        c0_verify(&fixture, &lifted),
        Err(VerifyError::InitialMerkleFailed { query_index: 0 })
    ));
    println!("c0_collision_bytes_as_real_opening=InitialMerkleFailed");

    let swap_index = (1..c0.queries.len())
        .find(|&index| c0.queries[index].position != c0.queries[0].position)
        .expect("distinct sampled query positions");
    let mut reordered = c0.clone();
    reordered.queries.swap(0, swap_index);
    assert!(matches!(
        c0_verify(&fixture, &reordered),
        Err(VerifyError::FoldMismatch {
            query_index: 0,
            epoch: 0
        })
    ));
    println!("c0_reordered_queries=FoldMismatch");

    let mut changed_root = c0.clone();
    changed_root.post_row_batch_commit.root[0] ^= 1;
    assert!(c0_verify(&fixture, &changed_root).is_err());
    println!("c0_post_row_root_transcript_mutation_rejected=true");

    let mut c1_prover = FsLaneChallenger::new_c1(b"basefold-chains-c1");
    let c1 = basefold::prove_c1(
        &fixture.message,
        fixture.transparent_c1.clone(),
        fixture.target_c1,
        &fixture.prover_data.codeword,
        &fixture.prover_data.merkle_tree,
        &fixture.ntt,
        fixture.params.log_inv_rate,
        fixture.params.log_batch_size,
        queries,
        &mut c1_prover,
    );
    c1_verify(&fixture, &c1).expect("honest C1 BaseFold proof");
    let mut c1_verifier = FsLaneChallenger::new_c1(b"basefold-chains-c1");
    basefold::verify_c1(
        fixture.target_c1,
        &c1,
        &fixture.commitment_root,
        &fixture.ntt,
        fixture.params.log_msg_len(),
        fixture.params.log_inv_rate,
        fixture.params.log_batch_size,
        &mut c1_verifier,
    )
    .unwrap();
    assert_eq!(c1_prover.sample_f256(), c1_verifier.sample_f256());
    println!("c1_honest_proof_and_transcript_lockstep=true");

    let mut short = c1.clone();
    short.queries[0].initial_path.pop();
    assert!(matches!(
        c1_verify(&fixture, &short),
        Err(VerifyError::InvalidProofShape)
    ));
    let mut long = c1.clone();
    long.queries[0].initial_path.push(fixed_sibling);
    assert!(matches!(
        c1_verify(&fixture, &long),
        Err(VerifyError::InvalidProofShape)
    ));
    println!("c1_short_and_fixed_point_extended_paths=InvalidProofShape");

    let mut lifted = c1.clone();
    lifted.queries[0].initial_leaf = digest_f128s(&collision.first_left).to_vec();
    lifted.queries[0].initial_path[0] = collision.first_right;
    assert!(matches!(
        c1_verify(&fixture, &lifted),
        Err(VerifyError::InitialMerkleFailed { query_index: 0 })
    ));
    println!("c1_collision_bytes_as_real_opening=InitialMerkleFailed");

    let swap_index = (1..c1.queries.len())
        .find(|&index| c1.queries[index].position != c1.queries[0].position)
        .expect("distinct sampled C1 query positions");
    let mut reordered = c1.clone();
    reordered.queries.swap(0, swap_index);
    assert!(matches!(
        c1_verify(&fixture, &reordered),
        Err(VerifyError::FoldMismatch {
            query_index: 0,
            epoch: 0
        })
    ));
    println!("c1_reordered_queries=FoldMismatch");

    let mut changed_root = c1.clone();
    changed_root.post_row_batch_commit.root[0] ^= 1;
    assert!(c1_verify(&fixture, &changed_root).is_err());
    println!("c1_post_row_root_transcript_mutation_rejected=true");
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    let collision = find_node_collision();
    match mode.as_str() {
        "collision" => {}
        "leaf" => leaf_and_tree_controls(&collision),
        "fixed" => raw_path_and_recursive_controls(&collision),
        "basefold" => {
            let (_, fixed_sibling) = left_fixed_point();
            basefold_controls(&collision, fixed_sibling);
        }
        "all" => {
            leaf_and_tree_controls(&collision);
            raw_path_and_recursive_controls(&collision);
            let (_, fixed_sibling) = left_fixed_point();
            basefold_controls(&collision, fixed_sibling);
        }
        _ => panic!("usage: basefold-chains [all|collision|leaf|fixed|basefold]"),
    }
    println!(
        "status=pass mode={mode} collision_first_at={} collision_evaluations={}",
        collision.first_at, collision.evaluations
    );
}
