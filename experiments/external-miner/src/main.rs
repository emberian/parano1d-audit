use std::time::{Duration, Instant};

use noid_chain::block_header::semantic_header_id;
use noid_chain::consensus::difficulty::next_target;
use noid_chain::consensus::header::validate_header_timeless;
use noid_chain::consensus::pow::{block_id, poseidon_pow_digest, validate_pow};
use noid_chain::consensus::template::{build_block_template, BlockTemplate};
use noid_chain::consensus::validation::{
    validate_block_consensus, validate_mandatory_coinbase, AnchorInfo,
};
use noid_chain::consensus::{ConsensusError, GENESIS_TARGET};
use noid_chain::{BlockHeader, ChainState};
use noid_poseidon2b::primitives::Address;

type ExternalMiningTemplateId = [u8; 16];
include!(concat!(env!("OUT_DIR"), "/pinned_external_mining_slot.rs"));

const PIN: &str = "52938433a30b185ab007afaa15166a632cf2e82e";

struct Fixture {
    state: ChainState,
    parent: BlockHeader,
    anchor: AnchorInfo,
    timestamp: u64,
    target: [u8; 32],
}

impl Fixture {
    fn new() -> Self {
        let mut state = ChainState::with_log_slots(8);
        let mut target = GENESIS_TARGET;
        let mut parent = BlockHeader {
            prev_block_hash: [0x11; 32],
            state_root: state.state_root(),
            tx_root: [0x22; 32],
            timestamp: 1_700_000_000,
            height: 1,
            miner_address: Address([0x33; 32]),
            nonce: 7,
            difficulty_target: target,
            log_slots: 8,
            active_slot_count: state.active_slot_count,
            alloc_counter: state.alloc_counter,
        };
        parent.state_root = state.state_root();
        let timestamp = parent.timestamp + 20;
        target = next_target(
            parent.height,
            parent.timestamp,
            &parent.difficulty_target,
            parent.height + 1,
            parent.timestamp,
            timestamp,
        );
        let anchor = AnchorInfo {
            anchor_height: parent.height,
            anchor_timestamp: parent.timestamp,
            anchor_target: parent.difficulty_target,
        };
        Self {
            state,
            parent,
            anchor,
            timestamp,
            target,
        }
    }

    fn template(&self, payout: u8) -> BlockTemplate {
        build_block_template(
            &self.parent,
            &self.state,
            &[0],
            Vec::new(),
            Address([payout; 32]),
            self.timestamp,
            self.target,
        )
        .unwrap()
    }

    fn validate_sequential(&self, template: &BlockTemplate, nonce: u128) -> ConsensusErrorResult {
        let block = template.clone().into_block(nonce);
        let mut state = self.state.clone();
        validate_block_consensus(
            &block,
            &self.parent,
            &[self.parent.timestamp],
            &[0],
            self.timestamp,
            &self.anchor,
            &mut state,
        )
        .map(|_| ())
    }
}

type ConsensusErrorResult = Result<(), ConsensusError>;

fn main() {
    println!("pinned_commit={PIN}");
    assert!(PINNED_CONSUME_BEFORE_POW_VERIFIED);
    println!("pinned_server_order=begin_proving -> finalize/validate_pow (source verified)");

    let fixture = Fixture::new();
    let template_a = fixture.template(0x44);
    let template_b = fixture.template(0x55);
    assert_ne!(
        semantic_header_id(&template_a.to_pow_header(0)),
        semantic_header_id(&template_b.to_pow_header(0))
    );

    let nonce_a = (0..16_777_216u128)
        .find(|nonce| {
            validate_pow(&template_a.to_pow_header(*nonce)).is_ok()
                && validate_pow(&template_b.to_pow_header(*nonce)).is_err()
        })
        .expect("nonce valid for A but invalid for B");
    let invalid_nonce = (0..16_384u128)
        .find(|nonce| validate_pow(&template_a.to_pow_header(*nonce)).is_err())
        .expect("an invalid nonce for the prepared template");
    let nonce_b = (0..16_777_216u128)
        .find(|nonce| validate_pow(&template_b.to_pow_header(*nonce)).is_ok())
        .expect("valid nonce for the incompatible template");

    fixture.validate_sequential(&template_a, nonce_a).unwrap();
    fixture.validate_sequential(&template_b, nonce_b).unwrap();
    println!(
        "CONTROL real templates accepted: A nonce={nonce_a}, B nonce={nonce_b}, target={}",
        hex::encode(fixture.target)
    );

    assert!(validate_pow(&template_a.to_pow_header(nonce_a)).is_ok());
    assert_eq!(
        validate_pow(&template_b.to_pow_header(nonce_a)),
        Err(ConsensusError::InvalidPoW)
    );
    println!("REJECT nonce A reused on different-payout template B: InvalidPoW");

    let nonce_free = template_a.to_pow_header(0);
    let sealed = template_a.to_pow_header(nonce_a);
    assert_eq!(semantic_header_id(&nonce_free), semantic_header_id(&sealed));
    assert_ne!(block_id(&nonce_free), block_id(&sealed));
    assert_ne!(
        poseidon_pow_digest(&nonce_free),
        poseidon_pow_digest(&sealed)
    );
    println!("CONTROL nonce-only change preserves SEMHDR, changes BLOCKHDR and POWHDR");

    let mut substituted = template_a.clone().into_block(nonce_a);
    substituted.header.miner_address = Address([0x66; 32]);
    assert_eq!(
        validate_mandatory_coinbase(&substituted, &fixture.parent),
        Err(ConsensusError::BadCoinbaseOwner)
    );
    println!("REJECT payout substitution at the coinbase boundary: BadCoinbaseOwner");

    let replacement_parent = template_b.clone().into_block(nonce_b).header;
    let stale = template_a.clone().into_block(nonce_a);
    assert_eq!(
        validate_header_timeless(
            &stale.header,
            &replacement_parent,
            &[fixture.parent.timestamp, replacement_parent.timestamp],
            &[0],
            replacement_parent.height,
            replacement_parent.timestamp,
            &replacement_parent.difficulty_target,
        ),
        Err(ConsensusError::BadParentHash)
    );
    println!("REJECT solved template after competing parent: BadParentHash");

    let now = Instant::now();
    let parent_height = fixture.parent.height;
    let parent_id = block_id(&fixture.parent);
    let mut slot = ExternalMiningAttemptSlot::new(7);
    let template_id = slot.reserve_preparation(now).unwrap();
    slot.install_ready(
        template_id,
        template_a.clone(),
        parent_height,
        parent_id,
        now + Duration::from_secs(30),
    )
    .unwrap();
    let consumed = slot
        .begin_proving(template_id, now, parent_height, parent_id)
        .unwrap();
    assert!(matches!(
        &slot.state,
        ExternalMiningAttemptState::Proving { .. }
    ));
    assert_eq!(
        validate_pow(&consumed.to_pow_header(invalid_nonce)),
        Err(ConsensusError::InvalidPoW)
    );
    slot.finish_proving(template_id);
    assert!(matches!(&slot.state, ExternalMiningAttemptState::Idle));
    assert!(matches!(
        slot.begin_proving(template_id, now, parent_height, parent_id),
        Err(ExternalMiningConsumeError::Unavailable)
    ));
    println!(
        "POC invalid nonce={invalid_nonce} consumed prepared template {}; replay=Unavailable; expensive_template_reusable=false",
        hex::encode(template_id)
    );

    let control_id = slot.reserve_preparation(now).unwrap();
    slot.install_ready(
        control_id,
        template_a.clone(),
        parent_height,
        parent_id,
        now + Duration::from_secs(30),
    )
    .unwrap();
    let valid = slot
        .begin_proving(control_id, now, parent_height, parent_id)
        .unwrap();
    assert!(validate_pow(&valid.to_pow_header(nonce_a)).is_ok());
    slot.finish_proving(control_id);
    assert!(matches!(
        slot.begin_proving(control_id, now, parent_height, parent_id),
        Err(ExternalMiningConsumeError::Unavailable)
    ));
    println!("CONTROL valid nonce is also single-use: replay=Unavailable (intentional)");

    let stale_id = slot.reserve_preparation(now).unwrap();
    slot.install_ready(
        stale_id,
        template_a.clone(),
        parent_height,
        parent_id,
        now + Duration::from_secs(30),
    )
    .unwrap();
    slot.invalidate_except(parent_height + 1, [0x99; 32]);
    assert!(matches!(
        slot.begin_proving(stale_id, now, parent_height, parent_id),
        Err(ExternalMiningConsumeError::Unavailable)
    ));
    println!("CONTROL canonical-tip invalidation drops the ready attempt");

    let expired_id = slot.reserve_preparation(now).unwrap();
    slot.install_ready(expired_id, template_a, parent_height, parent_id, now)
        .unwrap();
    slot.expire_ready(expired_id, now);
    assert!(matches!(&slot.state, ExternalMiningAttemptState::Idle));
    assert!(matches!(
        slot.begin_proving(expired_id, now, parent_height, parent_id),
        Err(ExternalMiningConsumeError::Unavailable)
    ));
    println!("CONTROL expired ready attempt is unavailable");

    println!("ALL ASSERTIONS PASSED");
}
