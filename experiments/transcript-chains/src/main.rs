use noid_core::transcript::FiatShamir;
use noid_core::{AdditiveNTT, Block128, Block256, TowerField};
use noid_fri::channel::{Channel as FriChannel, NUM_QUERIES, TAU};
use noid_fri::code::{Code, LOG_RATE};
use noid_fri::merkle::{compute_leaf_hashes, MerkleTree, VectorCommitment};
use noid_fri::prover::{commit, prove, EvalProof, FriCommitment};
use noid_fri::verifier::verify;
use noid_ivc_core::challenger::{fs_pad_lane_flat, Challenger, FsChallenger, FsLaneChallenger};
use noid_ivc_core::field::F128;
use noid_poseidon2b::channel::{Poseidon2bChannel, Poseidon2bWideChannel};
use noid_poseidon2b::native::compression::Poseidon2bSponge;

const PAD: Block128 = Block128((1u128 << 120) | 0x80);
const WALLET_QUERY_COUNT: usize = 65;
const WALLET_QUERY_WIDTH: usize = 13;
const WALLET_QUERY_SEEDS: usize = 7;
const WALLET_GRIND_TAG: u128 = 0x5A4B_6A1D_0000_0001;
const WALLET_GRIND_BITS: u32 = 16;

fn b(x: u128) -> Block128 {
    Block128::from(x)
}

fn padding_collisions() {
    println!("== one-lane padding equals an explicit second lane ==");
    println!("pad tower bits = 0x{PAD:032x}", PAD = PAD.0);
    let x = b(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
    let y = b(0xfedc_ba98_7654_3210_8899_aabb_ccdd_eeff);

    let mut legacy_short = Poseidon2bChannel::new();
    legacy_short.absorb(x);
    let mut legacy_long = Poseidon2bChannel::new();
    legacy_long.absorb(x);
    legacy_long.absorb(PAD);
    let legacy_a = legacy_short.squeeze_n(4);
    let legacy_b = legacy_long.squeeze_n(4);
    assert_eq!(legacy_a, legacy_b);
    println!("Poseidon2bChannel: absorb(x) == absorb(x, PAD); four challenges equal");

    let mut wide_short = Poseidon2bWideChannel::new();
    wide_short.absorb_base(x);
    let mut wide_long = Poseidon2bWideChannel::new();
    wide_long.absorb_wide(Block256::new(x, PAD));
    assert_eq!(wide_short.squeeze_wide(), wide_long.squeeze_wide());
    println!("Poseidon2bWideChannel: absorb_base(x) == absorb_wide((x, PAD))");

    let mut fri_short = FriChannel::new();
    fri_short.observe_field_elems(&[x, y, b(7)]);
    let fri_queries_a = fri_short.gen_queries(7);
    let mut fri_long = FriChannel::new();
    fri_long.observe_field_elems(&[x, y, b(7), PAD]);
    let fri_queries_b = fri_long.gen_queries(7);
    assert_eq!(fri_queries_a, fri_queries_b);
    println!(
        "noid_fri::Channel: 3 lanes == 4 lanes ending in PAD; {} queries equal: {:?}",
        NUM_QUERIES, fri_queries_a
    );

    let mut legacy_domain = Poseidon2bChannel::new();
    legacy_domain.absorb(x);
    legacy_domain.absorb(y);
    let legacy_out = legacy_domain.squeeze();
    let mut wide_domain = Poseidon2bWideChannel::new();
    wide_domain.absorb_wide(Block256::new(x, y));
    let wide_out = wide_domain.squeeze_wide().lo;
    let mut fri_domain = FriChannel::new();
    fri_domain.observe_field_elems(&[x, y]);
    let fri_out = fri_domain.get_random_point();
    assert_ne!(legacy_out, wide_out);
    assert_ne!(legacy_out, fri_out);
    assert_ne!(wide_out, fri_out);
    println!("cross-family control: KSCHANNL, KSCH256, FRICHANL outputs all differ");

    let raw = F128 {
        lo: x.0 as u64,
        hi: (x.0 >> 64) as u64,
    };
    let mut lane_short = FsLaneChallenger::new(b"pad-control");
    lane_short.observe_f128(raw);
    let lane_a = lane_short.sample_f128();
    let mut lane_long = FsLaneChallenger::new(b"pad-control");
    lane_long.observe_f128(raw);
    lane_long.observe_f128(fs_pad_lane_flat());
    let lane_b = lane_long.sample_f128();
    assert_ne!(lane_a, lane_b);
    println!("framed lane control: adding observe_f128(PAD) does NOT collide (extra op header)");

    let mut scalar = FsLaneChallenger::new(b"frame-control");
    scalar.observe_f128(raw);
    let scalar_out = scalar.sample_f128();
    let mut slice = FsLaneChallenger::new(b"frame-control");
    slice.observe_f128_slice(&[raw]);
    let slice_out = slice.sample_f128();
    assert_ne!(scalar_out, slice_out);
    println!("framed lane control: scalar and one-element slice differ (kind header)");

    let byte_raw = F128 { lo: 11, hi: 22 };
    let mut byte_scalar = FsChallenger::new(b"frame-control");
    byte_scalar.observe_f128(byte_raw);
    let byte_a = byte_scalar.sample_f128();
    let mut byte_slice = FsChallenger::new(b"frame-control");
    byte_slice.observe_f128_slice(&[byte_raw]);
    let byte_b = byte_slice.sample_f128();
    assert_ne!(byte_a, byte_b);
    println!("framed byte control: scalar and one-element slice differ (kind header)");

    let lo = b(0xaaaa_bbbb_cccc_dddd_1111_2222_3333_4444);
    let raw_hi = b(0x1234_5678_9abc_def0_0f1e_2d3c_4b5a_6978);
    let c0 = Block256::from_raw_challenge_lanes(lo, raw_hi);
    let c1 = Block256::from_raw_challenge_lanes(lo, raw_hi + Block128::ONE);
    assert_eq!(c0, c1);
    assert!(!c0.is_in_base_subfield());
    println!("wide challenge map: raw_hi and raw_hi+1 give the same challenge; hi != 0");
    println!(
        "exact distribution: uniform over 2^255 outputs under uniform raw lanes (2-to-1 high map)"
    );
}

fn mle_eval(evals: &[Block128], point: &[Block128]) -> Block128 {
    let mut buf = evals.to_vec();
    for &r in point.iter().rev() {
        let half = buf.len() / 2;
        for i in 0..half {
            buf[i] = buf[i] + r * (buf[i + half] + buf[i]);
        }
        buf.truncate(half);
    }
    buf[0]
}

fn eq_table(point: &[Block128]) -> Vec<Block128> {
    noid_fri::compute_eq_table(point)
}

fn transcript_to_final(
    commitment: &FriCommitment,
    eval_point: &[Block128],
    eval: Block128,
    proof: &EvalProof,
) -> (FriChannel, Vec<Block128>) {
    let tau = TAU.min(eval_point.len());
    let mut channel = FriChannel::new();
    channel.observe_fri_commitment(commitment);
    channel.observe_field_elems(eval_point);
    channel.observe_field_elem(eval);
    let tensor = channel.get_random_points(tau);
    let mut challenges = Vec::with_capacity(proof.fri_oracles.len());
    for (oracle, vc) in proof.sum_check_oracles.iter().zip(&proof.fri_oracles) {
        channel.observe_field_elems(&oracle.coeffs);
        channel.observe_vector_commitment(vc);
        challenges.push(channel.get_random_point());
    }
    let mut all_challenges = tensor;
    all_challenges.extend(challenges);
    (channel, all_challenges)
}

fn regenerate_with_final(
    mut proof: EvalProof,
    prefinal: &FriChannel,
    codes: &[Code],
    trees: &[MerkleTree],
    final_codeword: Vec<Block128>,
    rounds: usize,
) -> (EvalProof, Vec<usize>) {
    let mut query_channel = prefinal.clone();
    query_channel.observe_field_elems(&final_codeword);
    let queries = query_channel.gen_queries(rounds + LOG_RATE);
    proof.final_codeword = final_codeword;
    for round in 0..rounds {
        let mut symbols = Vec::with_capacity(queries.len());
        let mut paths = Vec::with_capacity(queries.len());
        for &query in &queries {
            let pair = (query >> round) >> 1;
            symbols.push((codes[round].idx(2 * pair), codes[round].idx(2 * pair + 1)));
            paths.push(trees[round].get_merkle_path(pair));
        }
        proof.fri_queried_symbols[round] = symbols;
        proof.fri_merkle_paths[round] = paths;
    }
    (proof, queries)
}

fn fresh_verifier_channel(commitment: &FriCommitment) -> FriChannel {
    let mut channel = FriChannel::new();
    channel.observe_fri_commitment(commitment);
    channel
}

fn fri_final_codeword_pair() {
    println!("== end-to-end noid_fri final-codeword padding pair ==");
    let log_len = TAU + 5;
    let ntt = AdditiveNTT::<Block128>::new(log_len + LOG_RATE);
    let hasher = Poseidon2bSponge::new();
    let evals: Vec<Block128> = (0..1usize << log_len)
        .map(|i| {
            let x = (i as u128).wrapping_add(1);
            b(x.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (x << 64) ^ 0xa5a5_5a5a)
        })
        .collect();
    let eval_point: Vec<Block128> = (0..log_len)
        .map(|i| b(0x1000_0000_0000_0000u128.wrapping_mul(i as u128 + 3) + 17))
        .collect();
    let eval = mle_eval(&evals, &eval_point);
    let (commitment, _initial_tree, _initial_code) = commit(&evals, &ntt, &hasher);
    let mut prover_channel = fresh_verifier_channel(&commitment);
    let honest = prove(&evals, &eval_point, &ntt, &mut prover_channel, &hasher);
    assert_eq!(honest.final_codeword.len(), 4);
    let honest_result = verify(
        &eval_point,
        eval,
        honest.clone(),
        &ntt,
        &mut fresh_verifier_channel(&commitment),
        &hasher,
    );
    assert!(
        honest_result.is_ok(),
        "honest proof failed: {honest_result:?}"
    );
    println!("honest four-lane proof verifies; NUM_QUERIES={NUM_QUERIES}");

    let (prefinal, all_challenges) = transcript_to_final(&commitment, &eval_point, eval, &honest);
    let tau = TAU.min(eval_point.len());
    let rounds = eval_point.len() - tau;
    let tensor = &all_challenges[..tau];
    let round_challenges = &all_challenges[tau..];
    let right = &eval_point[..rounds];
    let batching_eq = eq_table(tensor);
    let row_len = evals.len() >> tau;
    let mut batched = vec![Block128::ZERO; row_len];
    for (row, &weight) in batching_eq.iter().enumerate() {
        for j in 0..row_len {
            batched[j] += weight * evals[row * row_len + j];
        }
    }
    let eq_right = eq_table(right);
    let sumcheck_evals: Vec<Block128> = batched
        .iter()
        .zip(eq_right)
        .map(|(&value, eq)| value * eq)
        .collect();

    let mut current_code = Code::new_parallel(&sumcheck_evals, &ntt);
    let mut codes = Vec::with_capacity(rounds);
    let mut trees = Vec::with_capacity(rounds);
    for round in 0..rounds {
        let leaves = compute_leaf_hashes(&current_code.encoding, &hasher);
        let tree = MerkleTree::new_parallel(leaves, &hasher);
        let vc = VectorCommitment {
            root: tree.get_root(),
            depth: tree.num_layers() - 1,
        };
        assert_eq!(vc.root, honest.fri_oracles[round].root);
        assert_eq!(vc.depth, honest.fri_oracles[round].depth);
        codes.push(current_code.clone());
        trees.push(tree);
        current_code = current_code.fold_code(round_challenges[round], round, &ntt);
    }
    assert_eq!(current_code.encoding, honest.final_codeword);

    let extra = b(0xdead_beef_0000_0001_2222_3333_4444_5555);
    let mut five = honest.final_codeword.clone();
    five.push(extra);
    let mut six = five.clone();
    six.push(PAD);
    let (proof_five, queries_five) =
        regenerate_with_final(honest.clone(), &prefinal, &codes, &trees, five, rounds);
    let (proof_six, queries_six) =
        regenerate_with_final(honest, &prefinal, &codes, &trees, six, rounds);
    assert_eq!(queries_five, queries_six);
    let result_five = verify(
        &eval_point,
        eval,
        proof_five,
        &ntt,
        &mut fresh_verifier_channel(&commitment),
        &hasher,
    );
    let result_six = verify(
        &eval_point,
        eval,
        proof_six,
        &ntt,
        &mut fresh_verifier_channel(&commitment),
        &hasher,
    );
    assert!(
        result_five.is_ok(),
        "five-lane proof failed: {result_five:?}"
    );
    assert!(result_six.is_ok(), "six-lane proof failed: {result_six:?}");
    let distinct = {
        let mut sorted = queries_five.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.len()
    };
    println!("five-lane verifier: Ok; six-lane verifier: Ok");
    println!(
        "identical query set ({} draws, {} distinct): {:?}",
        queries_five.len(),
        distinct,
        queries_five
    );
    println!(
        "extra lane and PAD are beyond every final index used by this {rounds}-round proof (0..4)"
    );
    println!("cost after one honest proof: rebuild {rounds} round trees + regenerate both query opening lists; no collision search");
}

fn wallet_queries_from_seeds(seeds: &[Block128; WALLET_QUERY_SEEDS]) -> Vec<usize> {
    (0..WALLET_QUERY_COUNT)
        .map(|query| {
            let mut index = 0usize;
            for query_bit in 0..WALLET_QUERY_WIDTH {
                let absolute = query * WALLET_QUERY_WIDTH + query_bit;
                let seed = absolute / 128;
                let bit = absolute % 128;
                index |= (((seeds[seed].0 >> bit) & 1) as usize) << query_bit;
            }
            index
        })
        .collect()
}

fn wallet_query_grinding() {
    println!("== Wallet-style nonce selection changes the packed queries ==");
    let domain = 1usize << WALLET_QUERY_WIDTH;
    let no_duplicate_probability = (0..WALLET_QUERY_COUNT)
        .map(|i| 1.0 - i as f64 / domain as f64)
        .product::<f64>();
    let duplicate_probability = 1.0 - no_duplicate_probability;
    let expected_unique =
        domain as f64 * (1.0 - (1.0 - 1.0 / domain as f64).powi(WALLET_QUERY_COUNT as i32));
    println!(
        "P(at least one duplicate), random 65 draws over 8192 = {duplicate_probability:.9}; expected unique = {expected_unique:.6}"
    );
    println!(
        "random-nonce model: expected candidates to select a duplicate-containing set = 2^{WALLET_GRIND_BITS}/p = {:.1}",
        (1u64 << WALLET_GRIND_BITS) as f64 / duplicate_probability
    );

    let mut base = Poseidon2bWideChannel::new();
    base.absorb_base(b(0x5A4B_7A11_0000_0001));
    base.absorb_wide(Block256::new(b(0x1111), b(0x2222)));
    let _ = base.squeeze_wide();
    base.absorb_base(b(WALLET_GRIND_TAG));

    let mut candidates = 0u64;
    let mut valid_pow = 0u64;
    let mut selected = None;
    for nonce in 0..2_000_000u64 {
        candidates += 1;
        let mut trial = base.clone();
        trial.absorb_base(b(nonce as u128));
        let grind = trial.squeeze_base();
        if grind.0 & ((1u128 << WALLET_GRIND_BITS) - 1) != 0 {
            continue;
        }
        valid_pow += 1;
        let seeds: [Block128; WALLET_QUERY_SEEDS] = std::array::from_fn(|_| trial.squeeze_base());
        let queries = wallet_queries_from_seeds(&seeds);
        let mut sorted = queries.clone();
        sorted.sort_unstable();
        if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
            selected = Some((nonce, grind, queries));
            break;
        }
    }
    let (nonce, grind, queries) =
        selected.expect("deterministic search should find a duplicate query set");
    let mut replay = base.clone();
    replay.absorb_base(b(nonce as u128));
    assert_eq!(replay.squeeze_base(), grind);
    let replay_seeds: [Block128; WALLET_QUERY_SEEDS] =
        std::array::from_fn(|_| replay.squeeze_base());
    assert_eq!(wallet_queries_from_seeds(&replay_seeds), queries);
    println!("selected nonce={nonce}, candidates={candidates}, PoW-valid candidates={valid_pow}");
    println!("selected queries={queries:?}");
    println!(
        "real wide-channel replay matches; nonce is not required to be the first PoW-valid nonce"
    );
    println!("cost model: 2 permutations per nonce + 3 more only for PoW-valid nonces whose 7 seeds are decoded");
}

fn lane_pow_nonminimal() {
    println!("== framed lane PoW accepts a non-minimal valid nonce ==");
    let bits = 6;
    let mut base = FsLaneChallenger::new(b"nonce-selection");
    base.observe_f128(F128 {
        lo: 0x1234,
        hi: 0x5678,
    });
    let mut honest = base.clone();
    let first = honest.grind_pow(bits);
    let mut alternative = None;
    for nonce in (first + 1)..(first + 100_000) {
        let mut trial = base.clone();
        if trial.verify_pow(nonce, bits) {
            alternative = Some((nonce, trial));
            break;
        }
    }
    let (second, mut alternative) = alternative.expect("another valid nonce in bounded range");
    assert_ne!(first, second);
    let honest_next = honest.sample_f128_vec(4);
    let alternative_next = alternative.sample_f128_vec(4);
    assert_ne!(honest_next, alternative_next);
    println!(
        "first valid nonce={first}, later accepted nonce={second}, following challenges differ"
    );
    println!("this is query/challenge grinding, not a verifier/prover schedule mismatch");
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    match mode.as_str() {
        "collision" => padding_collisions(),
        "fri" => fri_final_codeword_pair(),
        "pow" => {
            wallet_query_grinding();
            lane_pow_nonminimal();
        }
        "all" => {
            padding_collisions();
            fri_final_codeword_pair();
            wallet_query_grinding();
            lane_pow_nonminimal();
        }
        _ => panic!("usage: transcript-chains [all|collision|fri|pow]"),
    }
}
