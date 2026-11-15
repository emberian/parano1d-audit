use std::time::Instant;

use capsule_chains::{known_collision, mixed_leaf};
use noid_core::{Block128, Block256, TowerField};
use noid_fri_binius::zk_affine_code::{AFFINE_CODE_MESSAGE_LEN, AFFINE_STATE_LEN};
use noid_fri_binius::zk_capsule_algebra::{MID_STANDARD_FOLDS, SOURCE_STANDARD_FOLDS};
use noid_fri_binius::zk_capsule_pcs::{
    zk_capsule_pcs_bind_owner, zk_capsule_pcs_bind_phase_a, zk_capsule_pcs_commit_fresh,
    zk_capsule_pcs_commit_mid, zk_capsule_pcs_link_phase_b, zk_capsule_pcs_open,
    zk_capsule_pcs_prove_phase_a, zk_capsule_pcs_reveal_tail, zk_capsule_pcs_verify,
    ZkCapsulePcsError, ZkCapsulePcsMidCommitment, ZkCapsulePcsOpening,
    ZkCapsulePcsSourceCommitment, ZkCapsulePcsTailReveal, ZK_CAPSULE_PCS_MID_CAP_HASHES,
    ZK_CAPSULE_PCS_QUERY_COUNT, ZK_CAPSULE_PCS_SOURCE_CAP_HASHES, ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH,
};
use noid_fri_binius::zk_phase_a::PHASE_A_VARS;
use rand::{rngs::StdRng, SeedableRng};
use serde_json::json;

fn elem(index: usize, domain: u128, salt: u128) -> Block128 {
    Block128::from(
        domain
            .wrapping_mul(index as u128 + 1)
            .rotate_left(((19 * index + 7) % 127) as u32)
            ^ salt.rotate_left(((13 * index + 3) % 127) as u32)
            ^ (index as u128 + 11).wrapping_mul(0x9E37_79B9_7F4A_7C15),
    )
}

fn wide(index: usize, domain: u128, salt: u128) -> Block256 {
    Block256::new(
        elem(index, domain, salt),
        elem(index + 10_000, domain ^ 0xC1_0256, salt.rotate_left(17)),
    )
}

fn queries() -> Vec<usize> {
    let mut out = Vec::with_capacity(ZK_CAPSULE_PCS_QUERY_COUNT);
    for cap in 0..ZK_CAPSULE_PCS_SOURCE_CAP_HASHES {
        let base = cap << ZK_CAPSULE_PCS_SOURCE_PATH_DEPTH;
        for mid_leaf_within_cap in (0..64).step_by(8) {
            out.push(base | (mid_leaf_within_cap << MID_STANDARD_FOLDS));
        }
    }
    out.push(4 << MID_STANDARD_FOLDS);
    assert_eq!(out.len(), ZK_CAPSULE_PCS_QUERY_COUNT);
    out
}

struct Fixture {
    source: ZkCapsulePcsSourceCommitment,
    mid: ZkCapsulePcsMidCommitment,
    tail: ZkCapsulePcsTailReveal,
    gamma: Block256,
    beta_source: [Block256; SOURCE_STANDARD_FOLDS],
    beta_mid: [Block256; MID_STANDARD_FOLDS],
    queries: Vec<usize>,
    opening: ZkCapsulePcsOpening,
}

impl Fixture {
    fn verify(&self, opening: &ZkCapsulePcsOpening) -> Result<(), ZkCapsulePcsError> {
        zk_capsule_pcs_verify(
            &self.source,
            &self.mid,
            &self.tail,
            self.gamma,
            self.beta_source,
            self.beta_mid,
            &self.queries,
            opening,
        )
        .map(|_| ())
    }

    fn verify_with_source(
        &self,
        source: &ZkCapsulePcsSourceCommitment,
        opening: &ZkCapsulePcsOpening,
    ) -> Result<(), ZkCapsulePcsError> {
        zk_capsule_pcs_verify(
            source,
            &self.mid,
            &self.tail,
            self.gamma,
            self.beta_source,
            self.beta_mid,
            &self.queries,
            opening,
        )
        .map(|_| ())
    }
}

fn fixture() -> Fixture {
    let salt = 0xC4A9_5A11u128;
    let state: Vec<Block128> = (0..AFFINE_STATE_LEN)
        .map(|index| elem(index, 0x57A7E, salt))
        .collect();
    let relation: [Block256; AFFINE_CODE_MESSAGE_LEN] =
        std::array::from_fn(|index| wide(index, 0x7E1A_710, salt ^ 0x44));
    let mut rng = StdRng::seed_from_u64(0xA11C_E104);
    let (source_commitment, source) =
        zk_capsule_pcs_commit_fresh(&state, &mut rng).expect("fresh source");
    let (owner_bank_claim, owner_bound) = zk_capsule_pcs_bind_owner(source, |bank| {
        Ok::<_, ()>(
            bank.iter()
                .zip(relation.iter())
                .fold(Block256::ZERO, |sum, (&bank, &weight)| {
                    sum + Block256::from(bank) * weight
                }),
        )
    })
    .expect("Owner handoff");
    let (_, bound) = zk_capsule_pcs_bind_phase_a(owner_bound, &relation, owner_bank_claim)
        .expect("bind Phase A");

    let mut gamma = wide(17, 0x6A77A, salt ^ 0x33);
    if gamma == Block256::ZERO || gamma == Block256::ONE {
        gamma += Block256::from(2u128);
    }
    let challenges: [Block256; PHASE_A_VARS] =
        std::array::from_fn(|round| wide(round + 31, 0xC4A1_1E6E, salt ^ 0x55));
    let (phase_a, complete) =
        zk_capsule_pcs_prove_phase_a(bound, gamma, |round, _| challenges[round])
            .expect("adaptive Phase A");
    let (_, ready) =
        zk_capsule_pcs_link_phase_b(complete, phase_a.terminal_oracle_value).expect("link Phase B");

    let beta_source = std::array::from_fn(|round| wide(round + 41, 0xBE7A_03, salt ^ 0x66));
    let beta_mid = std::array::from_fn(|round| wide(round + 61, 0xBE7A_47, salt ^ 0x77));
    let (mid_commitment, mid) =
        zk_capsule_pcs_commit_mid(ready, beta_source).expect("mid commitment");
    let tail_state = zk_capsule_pcs_reveal_tail(mid, beta_mid).expect("tail reveal");
    let tail = tail_state.tail.clone();
    let queries = queries();
    let opening = zk_capsule_pcs_open(tail_state, &queries).expect("opening");
    Fixture {
        source: source_commitment,
        mid: mid_commitment,
        tail,
        gamma,
        beta_source,
        beta_mid,
        queries,
        opening,
    }
}

fn main() {
    let started = Instant::now();
    let fixture = fixture();
    fixture
        .verify(&fixture.opening)
        .expect("honest PCS opening");
    let honest_seconds = started.elapsed().as_secs_f64();

    let collision = known_collision();
    let mut source_collision = fixture.opening.clone();
    source_collision.source_batch.siblings[0] = collision.second.1;
    let source_collision_result = fixture.verify(&source_collision).unwrap_err();

    let mut mid_collision = fixture.opening.clone();
    mid_collision.mid_batch.siblings[0] = collision.second.1;
    let mid_collision_result = fixture.verify(&mid_collision).unwrap_err();

    let mut rng = 0xca95_9c55_0000_0001u64;
    let alias_leaf = mixed_leaf(&mut rng, true);
    let mut alias_substitution = fixture.opening.clone();
    alias_substitution.source_joint_symbols[..24].copy_from_slice(&alias_leaf);
    let alias_result = fixture.verify(&alias_substitution).unwrap_err();

    let mut chained = alias_substitution.clone();
    chained.source_batch.siblings[0] = collision.second.1;
    let chained_result = fixture.verify(&chained).unwrap_err();

    let mut wrong_depth_source = fixture.source.clone();
    wrong_depth_source.cap.hashes.truncate(1);
    let depth_result = fixture
        .verify_with_source(&wrong_depth_source, &fixture.opening)
        .unwrap_err();

    assert_ne!(
        source_collision.source_batch.siblings,
        fixture.opening.source_batch.siblings
    );
    assert_ne!(
        mid_collision.mid_batch.siblings,
        fixture.opening.mid_batch.siblings
    );
    assert_ne!(
        alias_substitution.source_joint_symbols,
        fixture.opening.source_joint_symbols
    );

    println!(
        "{}",
        json!({
            "experiment": "full production zk_capsule_pcs_verify chained attacks",
            "honest_opening_accepted": true,
            "honest_fixture_and_verification_seconds": honest_seconds,
            "source_tree_depth": 13,
            "source_cap_hashes": ZK_CAPSULE_PCS_SOURCE_CAP_HASHES,
            "mid_tree_depth": 9,
            "mid_cap_hashes": ZK_CAPSULE_PCS_MID_CAP_HASHES,
            "source_capsnode_collision_error": format!("{source_collision_result:?}"),
            "mid_capsnode_collision_error": format!("{mid_collision_result:?}"),
            "capsmix_alias_leaf_substitution_error": format!("{alias_result:?}"),
            "chained_alias_plus_collision_error": format!("{chained_result:?}"),
            "one_hash_cap_depth_abuse_error": format!("{depth_result:?}"),
            "accepted_altered_openings": 0,
            "authorization_forgery_claimed": false,
            "elapsed_seconds": started.elapsed().as_secs_f64()
        })
    );
}
