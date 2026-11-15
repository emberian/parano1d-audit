use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use noid_chain::block::Block;
use noid_chain::consensus::pow::{block_id, validate_pow};
use noid_chain::consensus::template::{
    build_block_template, build_node_owned_block_template_avoiding_outputs, BlockTemplate,
};
use noid_chain::consensus::{next_target, ConsensusError};
use noid_chain::fri_state::SlotValue;
use noid_chain::history_step::HistoryStepTerminalMetadata;
use noid_chain::state::ChainState;
use noid_chain::storage::MdbxChainContext;
use noid_chain::{materialize_accepted_block_state, AcceptedBlockBundle, BlockHeader};
use noid_mempool::{
    AsyncMempool, ChainView, EvictReason, MempoolConfig, MempoolEvent, SubmitError,
};
use noid_node::networking::header_dag::{
    HeaderDag, HeaderDagError, HeaderDagUpdate, ValidatedHeader,
};
use noid_node::networking::ChainPoint;
use noid_poseidon2b::primitives::{Address, TxBodyHash};
use noid_tx::{
    output_bitmap_bit, PagedSpendIntent, Transaction, TxBody, TxInput, TxOutput, TxPage,
    PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};
use rayon::prelude::*;
use serde_json::{json, Value};

fn owner(seed: u8) -> Address {
    Address([seed; 32])
}

fn funded_state(log_slots: usize, inputs: &[(u32, u64, Address)]) -> ChainState {
    let mut state = ChainState::with_log_slots(log_slots);
    let mut supply = 0u128;
    for (index, (slot, amount, owner)) in inputs.iter().enumerate() {
        state
            .state
            .set_slot(
                *slot,
                SlotValue::with_owner_fields(*amount, index as u64 + 1, owner.as_fields()),
            )
            .unwrap();
        supply += u128::from(*amount);
    }
    state.active_slot_count = inputs.len() as u64;
    state.alloc_counter = inputs.len() as u64;
    state.circulating_supply_micronoid = supply;
    state
}

fn parent_header(state: &ChainState) -> BlockHeader {
    BlockHeader {
        prev_block_hash: [0; 32],
        state_root: state.cached_state_root(),
        tx_root: [0; 32],
        timestamp: 1_000,
        height: 0,
        miner_address: owner(0),
        nonce: 0,
        difficulty_target: [0xff; 32],
        log_slots: state.state.log_slots() as u32,
        active_slot_count: state.active_slot_count,
        alloc_counter: state.alloc_counter,
    }
}

fn spend_page(
    anchor: [u8; 32],
    input_slot: u32,
    input_amount: u64,
    creation_id: u64,
    input_owner: Address,
    output_slot: u32,
    output_owner: Address,
    fee: u64,
) -> Vec<TxPage> {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: input_slot,
        amount: input_amount,
        creation_id,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: output_slot,
        amount: input_amount.checked_sub(fee).unwrap(),
        owner: output_owner,
    };
    vec![TxPage::new(TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
    .unwrap()]
}

fn intent_for(pages: Vec<TxPage>) -> (PagedSpendIntent, Vec<u8>, TxBodyHash) {
    let id = noid_tx::validate_paged_spend(&pages).unwrap().logical_txid;
    let intent = PagedSpendIntent::new(pages, vec![0xA5; 20]).unwrap();
    let bytes = intent.to_bytes().unwrap();
    (intent, bytes, id)
}

fn accepting_pool(view: ChainView) -> AsyncMempool {
    AsyncMempool::new(view, MempoolConfig::default())
        .with_authorization_verification_executor(Arc::new(|_task| Ok(())))
}

fn view(height: u64, headers: HashMap<u64, BlockHeader>, state: &ChainState) -> ChainView {
    ChainView::new(
        height,
        headers,
        state.active_slot_count,
        state.state.clone(),
    )
}

fn template(
    parent: &BlockHeader,
    state: &ChainState,
    candidates: Vec<Transaction>,
    miner: Address,
) -> BlockTemplate {
    build_block_template(
        parent,
        state,
        &[state.active_slot_count; 18],
        candidates,
        miner,
        parent.timestamp.saturating_add(20),
        [0xff; 32],
    )
    .unwrap()
}

fn post_state(state: &ChainState, block: &Block) -> ChainState {
    let mut post = state.clone();
    let root = materialize_accepted_block_state(&mut post, block).unwrap();
    assert_eq!(root, block.header.state_root);
    post
}

fn headers(parent: &BlockHeader, block: &Block) -> HashMap<u64, BlockHeader> {
    HashMap::from([
        (parent.height, *parent),
        (block.header.height, block.header),
    ])
}

fn txid_bytes(id: TxBodyHash) -> String {
    id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn conflict_experiment() -> Value {
    let alice = owner(1);
    let bob = owner(2);
    let state = funded_state(8, &[(1, 1_000_000, alice), (3, 1_000_000, bob)]);
    let parent = parent_header(&state);
    let anchor = block_id(&parent);
    let pool = accepting_pool(view(0, HashMap::from([(0, parent)]), &state));
    let first = spend_page(anchor, 1, 1_000_000, 1, alice, 10, owner(11), 9_000);
    let higher = spend_page(anchor, 3, 1_000_000, 2, bob, 10, owner(12), 100_000);
    let (first_intent, first_bytes, first_id) = intent_for(first);
    let (higher_intent, higher_bytes, higher_id) = intent_for(higher);

    let runtime = tokio::runtime::Handle::current();
    let first_result = runtime.block_on(pool.submit(first_intent, first_bytes));
    assert!(matches!(first_result, Ok(hash) if hash == first_id));
    let higher_result = runtime.block_on(pool.submit(higher_intent, higher_bytes));
    assert!(matches!(
        higher_result,
        Err(SubmitError::Consensus(ConsensusError::SlotConflict))
    ));
    let selected = runtime.block_on(pool.select_for_block_at_anchor(1, anchor));
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].logical_txid, first_id);

    json!({
        "experiment": "first_seen_output_conflict_rejects_higher_fee",
        "first_fee_micronoid": 9_000,
        "racing_fee_micronoid": 100_000,
        "shared_output_slot": 10,
        "distinct_input_owners": true,
        "first_admitted": true,
        "higher_fee_error": "Consensus(SlotConflict)",
        "selected_txid": txid_bytes(first_id),
        "rejected_txid": txid_bytes(higher_id),
        "pool_len_after_race": runtime.block_on(pool.len()),
        "classification": "documented first-seen relay policy; fairness/liveness, not soundness"
    })
}

fn coinbase_race_experiment() -> Value {
    let alice = owner(3);
    let state = funded_state(8, &[(1, 1_000_000, alice)]);
    let parent = parent_header(&state);
    let anchor = block_id(&parent);

    let remote_template = template(&parent, &state, vec![], owner(90));
    let colliding_slot = remote_template.coinbase.body.outputs[0].slot_index;
    let victim = spend_page(
        anchor,
        1,
        1_000_000,
        1,
        alice,
        colliding_slot,
        owner(13),
        9_000,
    );
    let (victim_intent, victim_bytes, victim_id) = intent_for(victim);
    let pool = accepting_pool(view(0, HashMap::from([(0, parent)]), &state));
    let runtime = tokio::runtime::Handle::current();
    runtime
        .block_on(pool.submit(victim_intent, victim_bytes))
        .unwrap();

    let pending = HashSet::from([colliding_slot]);
    let (mitigated, _prepared) = build_node_owned_block_template_avoiding_outputs(
        &parent,
        &state,
        &[state.active_slot_count; 18],
        vec![],
        owner(90),
        parent.timestamp.saturating_add(20),
        [0xff; 32],
        &pending,
    )
    .unwrap();
    let mitigated_slot = mitigated.coinbase.body.outputs[0].slot_index;
    assert_ne!(mitigated_slot, colliding_slot);

    let remote_block = remote_template.into_block(0);
    assert!(!remote_block
        .transactions
        .iter()
        .any(|tx| tx.txid() == victim_id));
    let post = post_state(&state, &remote_block);
    assert_ne!(post.state.slot(colliding_slot), SlotValue::EMPTY);
    let mut events = pool.subscribe();
    runtime.block_on(pool.on_new_block(&[], 1, view(1, headers(&parent, &remote_block), &post)));
    assert_eq!(runtime.block_on(pool.len()), 0);
    let event = events.try_recv().unwrap();
    assert!(matches!(
        event,
        MempoolEvent::TxEvicted {
            hash,
            reason: EvictReason::OutputSlotOccupied,
        } if hash == victim_id
    ));

    json!({
        "experiment": "remote_coinbase_output_collision_evicts_pending_spend",
        "victim_txid": txid_bytes(victim_id),
        "victim_output_slot": colliding_slot,
        "remote_block_contains_victim": false,
        "remote_coinbase_slot": colliding_slot,
        "pending_spend_evicted": true,
        "eviction_reason": "OutputSlotOccupied",
        "local_pending_aware_template_slot": mitigated_slot,
        "local_avoidance_prevents_exact_collision": true,
        "classification": "best-effort local mitigation, not a consensus reservation; fairness/liveness, not soundness"
    })
}

fn reorg_resubmission_experiment() -> Value {
    let alice = owner(4);
    let state = funded_state(8, &[(1, 1_000_000, alice)]);
    let parent = parent_header(&state);
    let anchor = block_id(&parent);
    let pages = spend_page(anchor, 1, 1_000_000, 1, alice, 20, owner(14), 9_000);
    let transactions = vec![Transaction::new(pages[0].body.clone())];
    let (intent, bytes, id) = intent_for(pages);

    let pool = accepting_pool(view(0, HashMap::from([(0, parent)]), &state));
    let runtime = tokio::runtime::Handle::current();
    runtime.block_on(pool.submit(intent, bytes)).unwrap();

    let old_template = template(&parent, &state, transactions, owner(91));
    let old_block = old_template.into_block(0);
    let old_post = post_state(&state, &old_block);
    runtime.block_on(pool.on_new_block(&[id], 1, view(1, headers(&parent, &old_block), &old_post)));
    assert_eq!(runtime.block_on(pool.len()), 0);

    let replacement_template = template(&parent, &state, vec![], owner(92));
    let replacement = replacement_template.into_block(0);
    let replacement_post = post_state(&state, &replacement);
    assert_ne!(replacement_post.state.slot(1), SlotValue::EMPTY);
    assert_eq!(replacement_post.state.slot(20), SlotValue::EMPTY);
    runtime.block_on(pool.on_new_block(
        &[],
        1,
        view(1, headers(&parent, &replacement), &replacement_post),
    ));
    runtime.block_on(pool.readmit_after_reorg(vec![id]));
    let len_after_reorg_hook = runtime.block_on(pool.len());
    assert_eq!(len_after_reorg_hook, 0);

    let fresh_pages = spend_page(anchor, 1, 1_000_000, 1, alice, 21, owner(14), 9_000);
    let (fresh_intent, fresh_bytes, fresh_id) = intent_for(fresh_pages);
    assert_ne!(fresh_id, id);
    let resubmitted = runtime.block_on(pool.submit(fresh_intent, fresh_bytes));
    assert!(matches!(resubmitted, Ok(hash) if hash == fresh_id));

    json!({
        "experiment": "reorg_reclaim_requires_wallet_resubmission",
        "txid": txid_bytes(id),
        "old_confirmation_removed_from_pool": true,
        "replacement_restores_input_and_output_availability": true,
        "pool_len_after_readmit_after_reorg": len_after_reorg_hook,
        "fresh_intent_manual_submission_accepted": true,
        "fresh_txid": txid_bytes(fresh_id),
        "identical_intent_replay_tested": false,
        "classification": "explicitly documented reorg policy; liveness depends on online wallet, not soundness"
    })
}

fn dag_child(parent: &ValidatedHeader) -> ValidatedHeader {
    let mut header = parent.header;
    header.prev_block_hash = parent.hash;
    header.height = parent.header.height + 1;
    header.timestamp += 20;
    header.nonce = header.nonce.wrapping_add(1);
    let work = noid_chain::add_work(
        &parent.cumulative_work,
        &noid_chain::block_work(&header.difficulty_target),
    );
    ValidatedHeader::new_after_consensus_checks(header, work)
}

fn header_dag_experiment() -> Value {
    let genesis = noid_chain::consensus::genesis::genesis_header();
    let genesis_hash = block_id(&genesis);
    let genesis_work = noid_chain::block_work(&genesis.difficulty_target);
    let root = ValidatedHeader::new_after_consensus_checks(genesis, genesis_work);
    let first = dag_child(&root);
    let second = dag_child(&first);
    let third = dag_child(&second);
    let mut dag = HeaderDag::new(ChainPoint::new(0, genesis_hash), genesis_work, 2);
    assert!(matches!(
        dag.insert(first),
        Ok(HeaderDagUpdate::NewBest { .. })
    ));
    assert!(matches!(
        dag.insert(second),
        Ok(HeaderDagUpdate::NewBest { .. })
    ));
    let before = dag.best_tip();
    let error = dag.insert(third);
    assert_eq!(error, Err(HeaderDagError::Capacity));
    assert_eq!(dag.best_tip(), before);
    assert_eq!(dag.len(), 2);

    dag.advance_finalized(first.point(), first.cumulative_work)
        .unwrap();
    let recovered = dag.insert(third);
    assert!(matches!(recovered, Ok(HeaderDagUpdate::NewBest { .. })));

    json!({
        "experiment": "selected_ancestry_at_capacity_rejects_next_child",
        "configured_test_capacity": 2,
        "production_capacity": 1024,
        "next_child_error": "Capacity",
        "best_tip_unchanged_after_rejection": true,
        "manual_finality_advance_restores_insertion": true,
        "production_reconcile_order": "insert missing canonical child, then advance finalized checkpoint",
        "classification": "bounded-HeaderDAG liveness bug; valid PoW headers are a precondition, not a free attack"
    })
}

fn bundle_for_next_block(context: &MdbxChainContext) -> AcceptedBlockBundle {
    let parent = *context.tip_header();
    let timestamp = parent.timestamp.saturating_add(10_000_000);
    let anchor = context.anchor_info().unwrap();
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        timestamp,
    );
    eprintln!("owner-query: build template");
    let template = build_block_template(
        &parent,
        &context.state,
        &context.finalized_active_counts().unwrap(),
        vec![],
        owner(99),
        timestamp,
        target,
    )
    .unwrap();
    eprintln!("owner-query: search PoW");
    let block = (0u128..1_000_000)
        .into_par_iter()
        .map(|nonce| template.clone().into_block(nonce))
        .find_any(|candidate| validate_pow(&candidate.header).is_ok())
        .expect("one million nonces must contain a genesis-target solution");
    eprintln!("owner-query: PoW found");
    let mut terminal = HistoryStepTerminalMetadata::new(
        block.header.height,
        noid_chain::block_header::semantic_header_id(&block.header),
        0,
    )
    .unwrap()
    .encode_prefix()
    .to_vec();
    terminal.push(0xA5);
    AcceptedBlockBundle::try_from_parts(block.to_bytes(), terminal).unwrap()
}

fn accept_with_test_terminal_verifier(
    context: &mut MdbxChainContext,
    bundle: &AcceptedBlockBundle,
) {
    let block = Block::from_bytes(bundle.block_bytes()).unwrap();
    context
        .apply_next_block(
            bundle,
            block.header.timestamp,
            |block, state| {
                materialize_accepted_block_state(state, block).map_err(|error| format!("{error:?}"))
            },
            |_| Ok(()),
        )
        .unwrap();
}

fn median(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn owner_query_experiment() -> Value {
    const UTXOS: usize = 4_096;
    const SEGMENTS: u32 = 1;
    eprintln!("owner-query: create fixture");
    const PAGE: usize = 32;
    std::fs::create_dir_all("target").unwrap();
    let directory = tempfile::Builder::new()
        .prefix("owner-query-")
        .tempdir_in("target")
        .unwrap();
    let mut context = MdbxChainContext::open_or_create(directory.path()).unwrap();
    let owner = owner(77);
    let per_segment = UTXOS as u32 / SEGMENTS;
    let occupied: Vec<_> = (0..UTXOS as u32)
        .map(|index| {
            let segment = index / per_segment;
            let local = index % per_segment;
            (
                segment * 65_536 + local,
                SlotValue::with_owner_fields(10_000, u64::from(index) + 1, owner.as_fields()),
            )
        })
        .collect();
    context.state = ChainState::from_sparse_utxos(24, &occupied, UTXOS as u64).unwrap();
    context.state.circulating_supply_micronoid = (UTXOS as u128) * 10_000;
    eprintln!("owner-query: sparse state constructed");

    let bundle = bundle_for_next_block(&context);
    accept_with_test_terminal_verifier(&mut context, &bundle);
    let store = context.store.clone();

    let started = Instant::now();
    let cold_page = store.get_verified_owner_page(&owner.0, 0, PAGE).unwrap();
    let cold_page_us = started.elapsed().as_micros();
    assert_eq!(cold_page.utxos.len(), PAGE);
    let started = Instant::now();
    let cold_full = store.get_verified_utxos_by_owner(&owner.0).unwrap();
    let cold_full_us = started.elapsed().as_micros();
    assert_eq!(cold_full.utxos.len(), UTXOS);

    let mut full_us = Vec::new();
    let mut page_us = Vec::new();
    for _ in 0..7 {
        let started = Instant::now();
        let full = store.get_verified_utxos_by_owner(&owner.0).unwrap();
        black_box(full);
        full_us.push(started.elapsed().as_micros());
        let started = Instant::now();
        let page = store.get_verified_owner_page(&owner.0, 0, PAGE).unwrap();
        black_box(page);
        page_us.push(started.elapsed().as_micros());
    }
    let full_median = median(full_us);
    let page_median = median(page_us);

    json!({
        "experiment": "unbounded_owner_query_vs_bounded_page",
        "owner_utxos": UTXOS,
        "segments_touched_by_full_set": SEGMENTS,
        "page_limit": PAGE,
        "full_records_returned": cold_full.utxos.len(),
        "page_records_returned": cold_page.utxos.len(),
        "cold_full_us": cold_full_us,
        "cold_page_us": cold_page_us,
        "median_full_us": full_median,
        "median_page_us": page_median,
        "median_cost_ratio_full_over_page": full_median as f64 / page_median.max(1) as f64,
        "record_count_ratio": UTXOS / PAGE,
        "rpc_callsite": "paranoid_getSlotsByOwner holds chain.read() and synchronously calls get_verified_utxos_by_owner",
        "classification": "RPC cost-externalization/liveness risk, not consensus soundness"
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let report = json!({
        "pinned_commit": "52938433a30b185ab007afaa15166a632cf2e82e",
        "live_nodes_or_peers": false,
        "fixture_notes": [
            "Mempool submissions use the production AsyncMempool API with an accept-all authorization executor test hook; decoding, state, fee, conflict, selection, and eviction logic are real.",
            "The MDBX fixture commits a real PoW-valid public block transition with a test terminal verifier; owner queries and durable indexes are real.",
            "Fixture-only adjustment: the archived context commit uses its existing full owner-index rebuild flag so batch-seeded pre-parent state is indexed; query code is unchanged.",
            "No native-trade worst-fill or same-anchor replay experiment is included."
        ],
        "results": [
            header_dag_experiment(),
            tokio::task::spawn_blocking(conflict_experiment).await.unwrap(),
            tokio::task::spawn_blocking(coinbase_race_experiment).await.unwrap(),
            tokio::task::spawn_blocking(reorg_resubmission_experiment).await.unwrap(),
            owner_query_experiment(),
        ]
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
