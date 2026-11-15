use noid_chain::consensus::{
    block_id,
    epoch_anchor::{resolve_user_epoch_anchor_id, validate_block_epoch_anchors},
    params,
};
use noid_chain::{build_state_delta_witness, Block, BlockHeader, SegmentedFriState, SlotValue};
use noid_gkr::{
    prove_wallet_authorization, verify_wallet_authorization, OwnerAuthWitness,
    WalletAuthorizationBundle,
};
use noid_poseidon2b::primitives::{derive_address, Address, Digest, SpendSecret};
use noid_tx::{
    canonical_owner_auth, output_bitmap_bit, validate_public_tx_logic, Transaction, TxBody,
    TxInput, TxOutput, TX_INPUTS, TX_OUTPUTS,
};

const PINNED_COMMIT: &str = "52938433a30b185ab007afaa15166a632cf2e82e";
const EPOCH_HEIGHT: u64 = 279_504;
const PARENT_HEIGHT: u64 = EPOCH_HEIGHT + 1;
const CHILD_HEIGHT: u64 = EPOCH_HEIGHT + 2;
const INPUT_SLOT: u32 = 3;
const OUTPUT_SLOT: u32 = 9;
const INPUT_AMOUNT: u64 = 100;
const INPUT_CREATION_ID: u64 = 7;
const PARENT_ALLOC_COUNTER: u64 = INPUT_CREATION_ID;

struct ForkContext {
    name: &'static str,
    parent: BlockHeader,
    state: SegmentedFriState,
}

struct ForkOutcome {
    txid: Digest,
    pre_root: Digest,
    post_root: Digest,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("pinned_commit={PINNED_COMMIT}");
    assert_eq!(params::TX_EPOCH_BLOCKS, 144);
    assert_eq!(EPOCH_HEIGHT % params::TX_EPOCH_BLOCKS, 0);
    assert_eq!(
        params::owner_authorization_version(CHILD_HEIGHT),
        noid_tx::OwnerAuthorizationVersion::V4
    );
    println!(
        "heights: epoch={EPOCH_HEIGHT} parents={PARENT_HEIGHT} children={CHILD_HEIGHT} owner_auth=V4"
    );

    let secret = SpendSecret::from_bytes([0x31; 32]);
    let owner = derive_address(&secret);
    let recipient = Address([0x42; 32]);
    let epoch_header = header(EPOCH_HEIGHT, 1_793_000_000, 0);
    let epoch_id = block_id(&epoch_header);
    let body = user_body(epoch_id, owner, recipient);
    let original_tx = Transaction::new(body);
    let tx_wire = original_tx.to_bytes();
    let tx_a = Transaction::from_bytes(&tx_wire).map_err(|error| format!("{error:?}"))?;
    let tx_b = Transaction::from_bytes(&tx_wire).map_err(|error| format!("{error:?}"))?;
    assert_eq!(tx_a, tx_b);
    assert_eq!(tx_a.to_bytes(), tx_wire);

    let facts = validate_public_tx_logic(&tx_a.body)?;
    let statement = canonical_owner_auth(&tx_a.body)?;
    println!(
        "transaction: wire_bytes={} txid={}",
        tx_wire.len(),
        hex(&tx_a.txid().0)
    );
    println!(
        "public_logic: inputs={} outputs={} input_sum={} output_sum={} fee={}",
        facts.n_live_inputs, facts.n_live_outputs, facts.input_sum, facts.output_sum, facts.fee_u64
    );
    println!("epoch_anchor={}", hex(&tx_a.body.epoch_anchor));
    println!("input_owner={}", hex(statement.input_owner.as_bytes()));

    println!("proving one owner authorization with the legitimate wallet witness");
    let bundle = prove_wallet_authorization(&tx_a.body, OwnerAuthWitness::new(secret))?;
    let proof_wire = bundle.to_bytes()?;
    let replayed_bundle = WalletAuthorizationBundle::from_bytes(&proof_wire)?;
    assert_eq!(
        replayed_bundle.proof_wire_bytes().as_deref(),
        bundle.proof_wire_bytes().as_deref()
    );
    assert!(!proof_wire.is_empty());
    println!("owner_authorization: wire_roundtrip=ok same_bundle_reused=true");

    let mut fork_a = ForkContext {
        name: "fork_A",
        parent: header(PARENT_HEIGHT, 1_793_000_060, 0xA),
        state: fork_state(owner, Some((12, 5, 2))),
    };
    let mut fork_b = ForkContext {
        name: "fork_B",
        parent: header(PARENT_HEIGHT, 1_793_000_061, 0xB),
        state: fork_state(owner, Some((13, 6, 3))),
    };
    fork_a.parent.prev_block_hash = epoch_id;
    fork_b.parent.prev_block_hash = epoch_id;
    fork_a.parent.state_root = fork_a.state.root();
    fork_b.parent.state_root = fork_b.state.root();
    let parent_a_id = block_id(&fork_a.parent);
    let parent_b_id = block_id(&fork_b.parent);
    assert_ne!(parent_a_id, parent_b_id);
    assert_ne!(fork_a.state.root(), fork_b.state.root());
    println!("parent_A={parent_a_id}", parent_a_id = hex(&parent_a_id));
    println!("parent_B={parent_b_id}", parent_b_id = hex(&parent_b_id));

    let block_a = candidate_block(&fork_a.parent, tx_a.clone());
    let block_b = candidate_block(&fork_b.parent, tx_b.clone());
    let outcome_a = verify_in_fork(&mut fork_a, &block_a, &epoch_header, &replayed_bundle)?;
    let outcome_b = verify_in_fork(&mut fork_b, &block_b, &epoch_header, &replayed_bundle)?;
    assert_eq!(outcome_a.txid, outcome_b.txid);
    assert_ne!(outcome_a.pre_root, outcome_b.pre_root);
    assert_ne!(outcome_a.post_root, outcome_b.post_root);
    println!(
        "replay_result: same_transaction=true same_proof=true both_fork_context_checks_passed=true"
    );

    verify_wallet_authorization(&tx_a.body, &replayed_bundle)?;
    let same_fork_replay = build_state_delta_witness(
        &mut fork_a.state,
        std::slice::from_ref(&tx_a.body),
        &[tx_a.body.claims_commitment()],
        PARENT_ALLOC_COUNTER + 1,
        CHILD_HEIGHT + 1,
    )
    .expect_err("the already-spent input must reject on fork A");
    println!("same_fork_second_spend: owner_auth=ok state_delta={same_fork_replay:?}");

    let mut wrong_epoch_tx = tx_a.clone();
    wrong_epoch_tx.body.epoch_anchor = [0xEE; 32];
    let wrong_epoch_block = candidate_block(&fork_a.parent, wrong_epoch_tx.clone());
    let epoch_error = validate_block_epoch_anchors(&wrong_epoch_block, epoch_id, parent_a_id)
        .expect_err("a different epoch anchor must reject");
    let auth_error = verify_wallet_authorization(&wrong_epoch_tx.body, &replayed_bundle)
        .expect_err("the original proof must not authorize a changed body");
    println!("changed_epoch_control: epoch_check={epoch_error:?} owner_auth={auth_error:?}");
    println!("scope: exact transaction/proof replay across same-anchor forks; no new proof forged");
    Ok(())
}

fn verify_in_fork(
    fork: &mut ForkContext,
    block: &Block,
    epoch_header: &BlockHeader,
    bundle: &WalletAuthorizationBundle,
) -> Result<ForkOutcome, Box<dyn std::error::Error>> {
    let parent_id = block_id(&fork.parent);
    let expected_anchor = resolve_user_epoch_anchor_id(CHILD_HEIGHT, |height| {
        (height == EPOCH_HEIGHT).then_some(*epoch_header)
    })
    .expect("the shared canonical epoch header is available");
    assert_eq!(expected_anchor, block_id(epoch_header));
    validate_block_epoch_anchors(block, expected_anchor, parent_id)?;

    let user = &block.transactions[1];
    validate_public_tx_logic(&user.body)?;
    canonical_owner_auth(&user.body)?;
    verify_wallet_authorization(&user.body, bundle)?;

    let pre_root = fork.state.root();
    let witness = build_state_delta_witness(
        &mut fork.state,
        std::slice::from_ref(&user.body),
        &[user.body.claims_commitment()],
        PARENT_ALLOC_COUNTER,
        CHILD_HEIGHT,
    )
    .expect("the input is live and the output slot is empty on this fork");
    assert_eq!(witness.spends, 1);
    assert_eq!(witness.mints, 1);
    assert_eq!(witness.actions.len(), 2);
    assert!(fork.state.slot(INPUT_SLOT).is_empty());
    assert!(!fork.state.slot(OUTPUT_SLOT).is_empty());
    let post_root = fork.state.root();
    assert_eq!(post_root, witness.post_state_root);
    println!(
        "{}: epoch_anchor=ok public_logic=ok owner_auth=ok state_delta=ok spends={} mints={}",
        fork.name, witness.spends, witness.mints
    );
    println!("{}: pre_root={}", fork.name, hex(&pre_root));
    println!("{}: post_root={}", fork.name, hex(&post_root));
    Ok(ForkOutcome {
        txid: user.txid().0,
        pre_root,
        post_root,
    })
}

fn user_body(epoch_anchor: Digest, owner: Address, recipient: Address) -> TxBody {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: INPUT_SLOT,
        amount: INPUT_AMOUNT,
        creation_id: INPUT_CREATION_ID,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: OUTPUT_SLOT,
        amount: 99,
        owner: recipient,
    };
    TxBody {
        epoch_anchor,
        fee: 1,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0),
        is_coinbase: false,
    }
}

fn coinbase(parent_id: Digest, slot: u32, miner: Address) -> Transaction {
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: slot,
        amount: 50,
        owner: miner,
    };
    Transaction::new(TxBody {
        epoch_anchor: parent_id,
        fee: 0,
        input_owner: Address([0; 32]),
        inputs: [TxInput::dummy(); TX_INPUTS],
        outputs,
        validity_bitmap: output_bitmap_bit(0),
        is_coinbase: true,
    })
}

fn candidate_block(parent: &BlockHeader, user: Transaction) -> Block {
    let parent_id = block_id(parent);
    let header = BlockHeader {
        prev_block_hash: parent_id,
        height: CHILD_HEIGHT,
        timestamp: parent.timestamp + 30,
        ..*parent
    };
    Block {
        header,
        transactions: vec![coinbase(parent_id, 14, parent.miner_address), user],
    }
}

fn fork_state(owner: Address, unrelated: Option<(u32, u64, u64)>) -> SegmentedFriState {
    let mut state = SegmentedFriState::new_empty(4);
    state
        .set_slot(
            INPUT_SLOT,
            SlotValue::with_owner_fields(INPUT_AMOUNT, INPUT_CREATION_ID, owner.as_fields()),
        )
        .unwrap();
    if let Some((slot, amount, creation_id)) = unrelated {
        state
            .set_slot(
                slot,
                SlotValue::with_owner_fields(amount, creation_id, Address([0x77; 32]).as_fields()),
            )
            .unwrap();
    }
    assert!(state.slot(OUTPUT_SLOT).is_empty());
    state
}

fn header(height: u64, timestamp: u64, nonce: u128) -> BlockHeader {
    BlockHeader {
        prev_block_hash: [0x11; 32],
        state_root: [0x22; 32],
        tx_root: [0x33; 32],
        timestamp,
        height,
        miner_address: Address([0x44; 32]),
        nonce,
        difficulty_target: [0x77; 32],
        log_slots: 4,
        active_slot_count: 2,
        alloc_counter: PARENT_ALLOC_COUNTER,
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}
