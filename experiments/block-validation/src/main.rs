use noid_chain::block::{Block, BlockApplyError};
use noid_chain::block_header::semantic_header_id;
use noid_chain::consensus::difficulty::next_target;
use noid_chain::consensus::fees::fee_breakdown;
use noid_chain::consensus::pow::{block_id, poseidon_pow_digest, search_pow, validate_pow};
use noid_chain::consensus::template::build_block_template;
use noid_chain::consensus::validation::{
    validate_block_checks_template, validate_block_consensus, AnchorInfo,
};
use noid_chain::consensus::{
    validate_block_epoch_anchors, ConsensusError, GENESIS_TARGET, MAX_FUTURE_DRIFT,
};
use noid_chain::{
    apply_tx_at, compute_tx_root, materialize_accepted_block_state, AcceptedBlockBundle,
    AcceptedBlockBundleError, BlockHeader, ChainState, HistoryStepTerminalMetadata,
};
use noid_gkr::wallet_authorization::{
    prove_paged_spend_authorization, verify_paged_spend_authorization, OwnerAuthWitness,
};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::{
    output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, TxPage, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

const AMOUNT: u64 = 1_000_000;

struct Fixture {
    state: ChainState,
    parent: BlockHeader,
    epoch_header: BlockHeader,
    anchor: AnchorInfo,
}

fn header(height: u64, timestamp: u64, log_slots: u32) -> BlockHeader {
    BlockHeader {
        prev_block_hash: [0x11; 32],
        state_root: [0x22; 32],
        tx_root: [0x33; 32],
        timestamp,
        height,
        miner_address: Address([0x44; 32]),
        nonce: 0,
        difficulty_target: GENESIS_TARGET,
        log_slots,
        active_slot_count: 0,
        alloc_counter: 0,
    }
}

fn fixture_at(parent_height: u64) -> Fixture {
    let owner = Address([0xA5; 32]);
    let mut state = ChainState::with_log_slots(8);
    for slot in 1..=4u32 {
        state
            .state
            .set_slot(
                slot,
                noid_chain::SlotValue::with_owner_fields(
                    AMOUNT,
                    u64::from(slot),
                    owner.as_fields(),
                ),
            )
            .unwrap();
    }
    state.active_slot_count = 4;
    state.alloc_counter = 4;
    state.circulating_supply_micronoid = 4 * u128::from(AMOUNT);
    let timestamp = 1_000_000 + parent_height * 20;
    let mut parent = header(parent_height, timestamp, 8);
    parent.state_root = state.state_root();
    parent.active_slot_count = state.active_slot_count;
    parent.alloc_counter = state.alloc_counter;
    let epoch_header = header(0, 900_000, 8);
    let anchor = AnchorInfo {
        anchor_height: parent_height,
        anchor_timestamp: parent.timestamp,
        anchor_target: parent.difficulty_target,
    };
    Fixture {
        state,
        parent,
        epoch_header,
        anchor,
    }
}

fn user(f: &Fixture, input_slot: u32, output_slot: u32, anchor: [u8; 32]) -> Transaction {
    user_with_outputs(f, input_slot, &[output_slot], anchor, 0)
}

fn user_with_outputs(
    f: &Fixture,
    input_slot: u32,
    output_slots: &[u32],
    anchor: [u8; 32],
    fee_delta: i64,
) -> Transaction {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: input_slot,
        amount: AMOUNT,
        creation_id: u64::from(input_slot),
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    for (index, slot) in output_slots.iter().enumerate() {
        outputs[index] = TxOutput {
            slot_index: *slot,
            amount: 0,
            owner: Address([0xB0 + index as u8; 32]),
        };
    }
    let input_owner = Address([0xA5; 32]);
    let required = fee_breakdown(
        1,
        output_slots.len() as u64,
        f.parent.active_slot_count,
        f.parent.log_slots,
    )
    .required_total;
    let fee = (i64::try_from(required).unwrap() + fee_delta) as u64;
    let remaining = AMOUNT - fee;
    outputs[0].amount =
        remaining - (remaining / output_slots.len() as u64) * (output_slots.len() as u64 - 1);
    for output in outputs.iter_mut().take(output_slots.len()).skip(1) {
        output.amount = remaining / output_slots.len() as u64;
    }
    let mut validity_bitmap = 1 | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT;
    for index in 0..output_slots.len() {
        validity_bitmap |= output_bitmap_bit(index);
    }
    Transaction::new(TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner,
        inputs,
        outputs,
        validity_bitmap,
        is_coinbase: false,
    })
}

fn build(f: &Fixture, candidates: Vec<Transaction>) -> Block {
    let timestamp = f.parent.timestamp + 20;
    let difficulty_target = next_target(
        f.anchor.anchor_height,
        f.anchor.anchor_timestamp,
        &f.anchor.anchor_target,
        f.parent.height + 1,
        f.parent.timestamp,
        timestamp,
    );
    build_block_template(
        &f.parent,
        &f.state,
        &[],
        candidates,
        Address([9; 32]),
        timestamp,
        difficulty_target,
    )
    .unwrap()
    .into_block(0)
}

fn user_anchor(f: &Fixture) -> [u8; 32] {
    block_id(&f.epoch_header)
}

fn refresh_tx_root(block: &mut Block) {
    block.header.tx_root = compute_tx_root(&block.transactions);
}

fn rebind_state(f: &Fixture, block: &mut Block) {
    let mut post = f.state.clone();
    for tx in &block.transactions {
        apply_tx_at(&mut post, &tx.body, block.header.height).unwrap();
    }
    block.header.state_root = post.state_root();
    block.header.active_slot_count = post.active_slot_count;
    block.header.alloc_counter = post.alloc_counter;
    block.header.log_slots = post.state.log_slots() as u32;
    refresh_tx_root(block);
}

fn check_template(f: &Fixture, block: &Block) -> Result<(), ConsensusError> {
    validate_block_checks_template(
        block,
        &f.parent,
        &[f.parent.timestamp],
        &[],
        f.parent.timestamp + 20,
        &f.anchor,
    )
}

fn accept_template(f: &Fixture, block: &Block) {
    check_template(f, block).unwrap();
    validate_block_epoch_anchors(block, user_anchor(f), block_id(&f.parent)).unwrap();
    let mut state = f.state.clone();
    materialize_accepted_block_state(&mut state, block).unwrap();
}

fn mine(block: &mut Block) -> u128 {
    let nonce = search_pow(&block.header, 0, 10_000_000).expect("local PoW search");
    block.header.nonce = nonce;
    validate_pow(&block.header).unwrap();
    nonce
}

fn accept_sequential(f: &Fixture, block: &Block) {
    let mut state = f.state.clone();
    validate_block_consensus(
        block,
        &f.parent,
        &[f.parent.timestamp],
        &[],
        block.header.timestamp,
        &f.anchor,
        &mut state,
    )
    .unwrap();
}

fn expect_template_error(f: &Fixture, name: &str, block: &Block, expected: ConsensusError) {
    let actual = check_template(f, block);
    assert_eq!(actual, Err(expected), "{name}");
    println!("REJECT {name}: {actual:?}");
}

fn structural_terminal(block: &Block) -> Vec<u8> {
    let mut terminal =
        HistoryStepTerminalMetadata::new(block.header.height, semantic_header_id(&block.header), 0)
            .unwrap()
            .encode_prefix()
            .to_vec();
    terminal.push(0xA5);
    terminal
}

fn production_epoch_control() {
    let directory = tempfile::tempdir().unwrap();
    let mut context = noid_chain::MdbxChainContext::open_or_create(directory.path()).unwrap();
    let parent = *context.tip_header();
    let timestamp = parent.timestamp + 20;
    let anchor = context.anchor_info().unwrap();
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        timestamp,
    );
    let template = build_block_template(
        &parent,
        &context.state,
        &context.finalized_active_counts().unwrap(),
        Vec::new(),
        Address([9; 32]),
        timestamp,
        target,
    )
    .unwrap();
    let mut first = template.into_block(0);
    mine(&mut first);
    let first_bundle =
        AcceptedBlockBundle::try_from_parts(first.to_bytes(), structural_terminal(&first)).unwrap();
    context
        .apply_next_block(
            &first_bundle,
            timestamp,
            |block, state| {
                materialize_accepted_block_state(state, block).map_err(|error| format!("{error:?}"))
            },
            |_| Ok(()),
        )
        .unwrap();

    let parent = *context.tip_header();
    let coinbase = &first.transactions[0].body;
    let input = coinbase.live_outputs().next().unwrap().1;
    let stored = context.state.state.slot(input.slot_index);
    let fee = fee_breakdown(1, 1, parent.active_slot_count, parent.log_slots).required_total;
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: input.slot_index,
        amount: input.amount,
        creation_id: stored.creation_id(),
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: input.slot_index + 1,
        amount: input.amount - fee,
        owner: Address([0xB0; 32]),
    };
    let spend = Transaction::new(TxBody {
        epoch_anchor: [0xA5; 32],
        fee,
        input_owner: input.owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    });
    let timestamp = parent.timestamp + 20;
    let anchor = context.anchor_info().unwrap();
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        timestamp,
    );
    let template = build_block_template(
        &parent,
        &context.state,
        &context.finalized_active_counts().unwrap(),
        vec![spend],
        Address([9; 32]),
        timestamp,
        target,
    )
    .unwrap();
    let mut wrong = template.into_block(0);
    mine(&mut wrong);
    let bundle =
        AcceptedBlockBundle::try_from_parts(wrong.to_bytes(), structural_terminal(&wrong)).unwrap();
    let mut verifier_called = false;
    let result = context.apply_next_block(
        &bundle,
        timestamp,
        |block, state| {
            materialize_accepted_block_state(state, block).map_err(|error| format!("{error:?}"))
        },
        |_| {
            verifier_called = true;
            Ok(())
        },
    );
    assert!(matches!(
        result,
        Err(noid_chain::MdbxContextError::Consensus(
            ConsensusError::BadEpochAnchor
        ))
    ));
    assert!(!verifier_called);
    println!(
        "REJECT production MdbxChainContext::apply_next_block wrong epoch before terminal verifier"
    );
}

fn authorization_substitution(anchor: [u8; 32]) {
    let secret = SpendSecret::from_bytes([0x42; 32]);
    let owner = derive_address(&secret);
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: 17,
        amount: 100_000,
        creation_id: 7,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 29,
        amount: 90_000,
        owner: Address([0xB0; 32]),
    };
    let body = TxBody {
        epoch_anchor: anchor,
        fee: 10_000,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    };
    let pages = vec![TxPage::new(body).unwrap()];
    let proof = prove_paged_spend_authorization(&pages, OwnerAuthWitness::new(secret)).unwrap();
    verify_paged_spend_authorization(&pages, &proof).unwrap();
    let mut substituted = pages.clone();
    substituted[0].body.outputs[0].owner = Address([0xB1; 32]);
    assert!(verify_paged_spend_authorization(&substituted, &proof).is_err());
    let mut replayed = pages.clone();
    replayed[0].body.epoch_anchor[0] ^= 1;
    assert!(verify_paged_spend_authorization(&replayed, &proof).is_err());
    println!("REJECT real wallet proof substituted across output-owner and epoch-anchor mutations");
}

fn main() {
    let f = fixture_at(100);
    let correct = user(&f, 1, 11, user_anchor(&f));
    let valid = build(&f, vec![correct]);
    accept_template(&f, &valid);
    println!("ACCEPT validated template with one ordinary PagedSpend");

    let mut sealed = valid.clone();
    let nonce = mine(&mut sealed);
    accept_sequential(&f, &sealed);
    println!("ACCEPT fully sealed sequential-consensus block: nonce={nonce}");

    let mut wrong_epoch = sealed.clone();
    wrong_epoch.transactions[1].body.epoch_anchor[0] ^= 1;
    refresh_tx_root(&mut wrong_epoch);
    let wrong_nonce = mine(&mut wrong_epoch);
    accept_sequential(&f, &wrong_epoch);
    assert_eq!(
        validate_block_epoch_anchors(&wrong_epoch, user_anchor(&f), block_id(&f.parent)),
        Err(ConsensusError::BadEpochAnchor)
    );
    println!("POC standalone validate_block_consensus ACCEPTED wrong user epoch anchor; production boundary rejected it: nonce={wrong_nonce}");

    let mut stale_root = valid.clone();
    stale_root.transactions[1].body.epoch_anchor[0] ^= 1;
    let mut stale_state = f.state.clone();
    assert_eq!(
        materialize_accepted_block_state(&mut stale_state, &stale_root),
        Err(BlockApplyError::HeaderTxRootMismatch)
    );
    println!("REJECT body mutation with stale tx_root: HeaderTxRootMismatch");

    let mut rebound = stale_root.clone();
    refresh_tx_root(&mut rebound);
    let mut rebound_state = f.state.clone();
    materialize_accepted_block_state(&mut rebound_state, &rebound).unwrap();
    assert_eq!(
        validate_block_epoch_anchors(&rebound, user_anchor(&f), block_id(&f.parent)),
        Err(ConsensusError::BadEpochAnchor)
    );
    println!("POC recomputing tx_root does not enforce epoch anchor at materialization boundary");

    let mut bad_state_root = valid.clone();
    bad_state_root.transactions[1].body.outputs[0].owner = Address([0xCC; 32]);
    refresh_tx_root(&mut bad_state_root);
    let mut bad_state = f.state.clone();
    assert_eq!(
        materialize_accepted_block_state(&mut bad_state, &bad_state_root),
        Err(BlockApplyError::HeaderStateRootMismatch)
    );
    println!("REJECT output substitution with rebound tx_root but stale state_root");

    let second = user(&f, 2, 12, user_anchor(&f));
    let ordered = build(&f, vec![valid.transactions[1].clone(), second]);
    let mut reordered = ordered.clone();
    reordered.transactions.swap(1, 2);
    rebind_state(&f, &mut reordered);
    assert_ne!(ordered.header.tx_root, reordered.header.tx_root);
    accept_template(&f, &ordered);
    accept_template(&f, &reordered);
    println!("ACCEPT both independent-transaction orders with distinct rebound tx/state roots");

    let mut duplicate = ordered.clone();
    duplicate
        .transactions
        .push(duplicate.transactions[1].clone());
    expect_template_error(
        &f,
        "duplicate user transaction",
        &duplicate,
        ConsensusError::SlotConflict,
    );

    let mut duplicate_coinbase = valid.clone();
    duplicate_coinbase
        .transactions
        .push(duplicate_coinbase.transactions[0].clone());
    expect_template_error(
        &f,
        "duplicate primary coinbase",
        &duplicate_coinbase,
        ConsensusError::MultipleCoinbase,
    );

    let mut inflated = valid.clone();
    inflated.transactions[0].body.outputs[0].amount += 1;
    refresh_tx_root(&mut inflated);
    expect_template_error(
        &f,
        "coinbase subsidy/fee ceiling +1",
        &inflated,
        ConsensusError::InflatedCoinbase,
    );

    let mut wrong_cb_owner = valid.clone();
    wrong_cb_owner.transactions[0].body.outputs[0].owner = Address([0xEE; 32]);
    refresh_tx_root(&mut wrong_cb_owner);
    expect_template_error(
        &f,
        "coinbase owner/header mismatch",
        &wrong_cb_owner,
        ConsensusError::BadCoinbaseOwner,
    );

    let mut wrong_cb_anchor = valid.clone();
    wrong_cb_anchor.transactions[0].body.epoch_anchor[0] ^= 1;
    refresh_tx_root(&mut wrong_cb_anchor);
    expect_template_error(
        &f,
        "coinbase parent-anchor mismatch",
        &wrong_cb_anchor,
        ConsensusError::BadCoinbaseAnchor,
    );

    let underfee_tx = user_with_outputs(&f, 1, &[11], user_anchor(&f), -1);
    let mut underfee = valid.clone();
    underfee.transactions[1] = underfee_tx;
    refresh_tx_root(&mut underfee);
    let required =
        fee_breakdown(1, 1, f.parent.active_slot_count, f.parent.log_slots).required_total;
    expect_template_error(
        &f,
        "one-below-minimum fee",
        &underfee,
        ConsensusError::BelowMinFee {
            required,
            actual: required - 1,
        },
    );

    let growth_tx = user_with_outputs(&f, 1, &[11, 12], user_anchor(&f), 0);
    let growth = build(&f, vec![growth_tx]);
    accept_template(&f, &growth);
    let growth_fee = fee_breakdown(1, 2, f.parent.active_slot_count, f.parent.log_slots);
    assert!(growth_fee.burned > 0);
    let mut claim_burn = growth.clone();
    claim_burn.transactions[0].body.outputs[0].amount += 1;
    refresh_tx_root(&mut claim_burn);
    expect_template_error(
        &f,
        "claiming state-growth burn",
        &claim_burn,
        ConsensusError::InflatedCoinbase,
    );
    println!(
        "ACCEPT growth-fee template; burned={} claimable={}",
        growth_fee.burned,
        growth_fee.required_total - growth_fee.burned
    );

    let mut bad_parent = valid.clone();
    bad_parent.header.prev_block_hash[0] ^= 1;
    expect_template_error(
        &f,
        "parent mutation",
        &bad_parent,
        ConsensusError::BadParentHash,
    );
    let mut bad_height = valid.clone();
    bad_height.header.height += 1;
    expect_template_error(
        &f,
        "height mutation",
        &bad_height,
        ConsensusError::BadHeight,
    );
    let mut bad_target = valid.clone();
    bad_target.header.difficulty_target[0] ^= 1;
    expect_template_error(
        &f,
        "difficulty mutation",
        &bad_target,
        ConsensusError::BadDifficultyTarget,
    );
    let mut bad_mtp = valid.clone();
    bad_mtp.header.timestamp = f.parent.timestamp;
    bad_mtp.header.difficulty_target = next_target(
        f.anchor.anchor_height,
        f.anchor.anchor_timestamp,
        &f.anchor.anchor_target,
        bad_mtp.header.height,
        f.parent.timestamp,
        bad_mtp.header.timestamp,
    );
    expect_template_error(&f, "MTP mutation", &bad_mtp, ConsensusError::BadTimestamp);
    let mut future = valid.clone();
    future.header.timestamp = valid.header.timestamp + MAX_FUTURE_DRIFT + 1;
    future.header.difficulty_target = next_target(
        f.anchor.anchor_height,
        f.anchor.anchor_timestamp,
        &f.anchor.anchor_target,
        future.header.height,
        f.parent.timestamp,
        future.header.timestamp,
    );
    expect_template_error(
        &f,
        "future-drift mutation",
        &future,
        ConsensusError::BadTimestamp,
    );
    let mut bad_depth = valid.clone();
    bad_depth.header.log_slots += 1;
    expect_template_error(
        &f,
        "unexpected state-depth expansion",
        &bad_depth,
        ConsensusError::BadLogSlotsExpansion,
    );

    let semantic = semantic_header_id(&sealed.header);
    let pow = poseidon_pow_digest(&sealed.header);
    let mut renonced = sealed.clone();
    renonced.header.nonce = renonced.header.nonce.wrapping_add(1);
    assert_eq!(semantic_header_id(&renonced.header), semantic);
    assert_ne!(poseidon_pow_digest(&renonced.header), pow);
    let mut mutated_miner = sealed.clone();
    mutated_miner.header.miner_address = Address([0x99; 32]);
    assert_ne!(semantic_header_id(&mutated_miner.header), semantic);
    assert_ne!(poseidon_pow_digest(&mutated_miner.header), pow);
    println!(
        "BIND nonce excluded only from semantic id; nonce and miner mutation change PoW digest"
    );

    let terminal = structural_terminal(&sealed);
    AcceptedBlockBundle::try_from_parts(sealed.to_bytes(), terminal.clone()).unwrap();
    assert_eq!(
        AcceptedBlockBundle::try_from_parts(wrong_epoch.to_bytes(), terminal.clone()),
        Err(AcceptedBlockBundleError::TerminalHashMismatch)
    );
    AcceptedBlockBundle::try_from_parts(renonced.to_bytes(), terminal).unwrap();
    println!("REJECT structural terminal substitution across semantic headers; ACCEPT same terminal across nonce-only change (intended)");

    let due = fixture_at(4319);
    let due_user = user(&due, 1, 11, user_anchor(&due));
    let payout = build(&due, vec![due_user]);
    assert_eq!(payout.transactions.len(), 3);
    accept_template(&due, &payout);
    println!("ACCEPT mandatory development-payout template at height 4320");
    let mut missing_payout = payout.clone();
    missing_payout.transactions.remove(1);
    refresh_tx_root(&mut missing_payout);
    expect_template_error(
        &due,
        "missing development payout",
        &missing_payout,
        ConsensusError::MissingDevelopmentPayout,
    );
    let mut wrong_payout = payout.clone();
    wrong_payout.transactions[1].body.outputs[0].amount += 1;
    refresh_tx_root(&mut wrong_payout);
    expect_template_error(
        &due,
        "wrong development amount",
        &wrong_payout,
        ConsensusError::BadDevelopmentPayout,
    );
    let mut moved_payout = payout.clone();
    let system = moved_payout.transactions.remove(1);
    moved_payout.transactions.push(system);
    expect_template_error(
        &due,
        "development payout after user page",
        &moved_payout,
        ConsensusError::MultipleCoinbase,
    );

    authorization_substitution(user_anchor(&f));
    production_epoch_control();
    println!("ALL ASSERTIONS PASSED");
}
