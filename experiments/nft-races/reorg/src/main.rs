//! Stale listing across a real MDBX fork/reorg, and same-block listing
//! races, at block level: real templates, real PoW, real acceptance.
//!
//! Chain plan (isolated-v2-1-fork-testnet profile: v2.1 active from
//! height 24, so native financial ops are live):
//!   h1..h23       coinbase to the miner
//!   h24           coinbase split: creator / seller / buyer UTXOs
//!   h25           op 5  CreateCollection   (creator)
//!   h26           op 6  MintNft            (creator)
//!   h27           op 7  TransferNft creator->seller   [common prefix ends]
//!   fork A h28:   op 64 CreateListing      (seller)   -> accepted first
//!   fork B h28:   op 7  TransferNft seller->attacker  + h29 coinbase
//!                 -> reorg onto B (more work)
//!   then:         op 65 Purchase of the stale fork-A listing must fail
//!                 at admission on the B tip; no fork-A state survives.

use anyhow::{bail, Context, Result};
use noid_chain::consensus::params::BLOCK_TIME;
use noid_chain::state::native_assets::market::liquidity::MarketFeeContext;
use noid_chain::state::native_assets::native_chain_id;
use noid_chain::{
    block_id, materialize_accepted_block_state, AcceptedBlockBundle, Block, MdbxChainContext,
    MdbxContextError,
};
use noid_poseidon2b::primitives::Address;
use noid_tx::native_assets::nft_market::{
    ListingDefinition, ListingState, NftMarketOpening, NftRoundContext, NFT_FEE_CREDIT,
};
use noid_tx::native_assets::transition::{
    NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{GenesisAnchor, NativeOpening, NftCollection, NftItem};
use noid_tx::{
    output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT,
};

const MINER: u8 = 0x44;
const CREATOR: u8 = 0x01;
const SELLER: u8 = 0x02;
const BUYER: u8 = 0x03;
const ATTACKER: u8 = 0x09;
const CREATOR_CASH: u64 = 40_000_000;
const SELLER_CASH: u64 = 20_000_000;
const BUYER_CASH: u64 = 20_000_000;
const ASK: u64 = 500_000;

fn address(byte: u8) -> Address {
    Address([byte; 32])
}

fn decode_block(bytes: &[u8]) -> Result<Block> {
    Block::from_bytes(bytes).map_err(|error| anyhow::anyhow!(format!("block decode: {error:?}")))
}

fn accept(
    context: &mut MdbxChainContext,
    bundle: &AcceptedBlockBundle,
) -> Result<(), MdbxContextError> {
    let block = decode_block(bundle.block_bytes())
        .map_err(|_| MdbxContextError::Corrupt("PoC block decode"))?;
    context
        .apply_next_block(
            bundle,
            block.header.timestamp,
            |block, state| {
                materialize_accepted_block_state(state, block).map_err(|error| format!("{error:?}"))
            },
            |_| Ok(()),
        )
        .map(|_| ())
}

fn accept_annotated(context: &mut MdbxChainContext, bundle: &AcceptedBlockBundle) -> Result<()> {
    let height = decode_block(bundle.block_bytes())
        .map(|block| block.header.height)
        .unwrap_or_default();
    accept(context, bundle).with_context(|| format!("accept block at height {height}"))
}

struct ChainDriver {
    context: MdbxChainContext,
    history: Vec<AcceptedBlockBundle>,
}

impl ChainDriver {
    fn coinbase(&mut self) -> Result<()> {
        let bundle = self.bundle(Vec::new(), Vec::new())?;
        accept(&mut self.context, &bundle)?;
        self.history.push(bundle);
        Ok(())
    }

    fn bundle(
        &self,
        candidate_txs: Vec<Transaction>,
        native_openings: Vec<NativeOpening>,
    ) -> Result<AcceptedBlockBundle> {
        let parent = *self.context.tip_header();
        let timestamp = parent.timestamp.saturating_add(BLOCK_TIME);
        let anchor = self.context.anchor_info()?;
        let target = noid_chain::consensus::next_target(
            anchor.anchor_height,
            anchor.anchor_timestamp,
            &anchor.anchor_target,
            parent.height.saturating_add(1),
            parent.timestamp,
            timestamp,
        );
        let finalized_active_counts = self.context.finalized_active_counts()?;
        let (template, _) =
            noid_chain::consensus::template::build_node_owned_block_template_with_native_openings(
                &parent,
                &self.context.state,
                &finalized_active_counts,
                candidate_txs,
                address(MINER),
                timestamp,
                target,
                &std::collections::HashSet::new(),
                &native_openings,
            )
            .map_err(|error| anyhow::anyhow!(format!("block template: {error:?}")))?;
        let mut nonce = 0u128;
        let block = loop {
            let candidate = template.clone().into_block(nonce);
            if noid_chain::consensus::validate_pow(&candidate.header).is_ok() {
                break candidate;
            }
            nonce = nonce.checked_add(1).context("PoW nonce space exhausted")?;
        };
        let mut terminal = noid_chain::HistoryStepTerminalMetadata::new(
            block.header.height,
            noid_chain::block_header::semantic_header_id(&block.header),
            0,
        )?
        .encode_prefix()
        .to_vec();
        terminal.push(0xA5);
        Ok(AcceptedBlockBundle::try_from_parts_with_native_openings(
            block.to_bytes(),
            terminal,
            native_openings,
        )?)
    }

    fn replay_onto(&self, other: &mut MdbxChainContext) -> Result<()> {
        for bundle in &self.history {
            accept(other, bundle)?;
        }
        Ok(())
    }
}

fn epoch_anchor(context: &MdbxChainContext) -> Result<[u8; 32]> {
    let parent = *context.tip_header();
    let anchor_height =
        noid_chain::consensus::tx_epoch_anchor_height_for_child(parent.height.saturating_add(1));
    let header = context
        .store
        .get_header(anchor_height)?
        .with_context(|| format!("anchor header at {anchor_height} missing"))?;
    Ok(block_id(&header))
}

fn slot_of(context: &MdbxChainContext, slot: u32) -> noid_chain::fri_state::SlotValue {
    context.state.state.slot(slot)
}

fn utxo(context: &MdbxChainContext, owner: u8, min_amount: u64) -> Result<(u32, TxInput)> {
    utxo_excluding(context, owner, min_amount, &[])
}

fn utxo_excluding(
    context: &MdbxChainContext,
    owner: u8,
    min_amount: u64,
    exclude: &[u32],
) -> Result<(u32, TxInput)> {
    let utxos = context
        .store
        .get_verified_utxos_by_owner(&[owner; 32])?
        .utxos;
    let found = utxos
        .iter()
        .find(|u| u.amount >= min_amount && !exclude.contains(&u.slot_index))
        .with_context(|| format!("no utxo >= {min_amount} for owner {owner:#x}"))?;
    Ok((
        found.slot_index,
        TxInput {
            slot_index: found.slot_index,
            amount: found.amount,
            creation_id: found.creation_id,
        },
    ))
}

fn ordinary_claim(
    context: &MdbxChainContext,
    owner: u8,
    min_amount: u64,
) -> Result<NativeInputClaim> {
    let (_, input) = utxo(context, owner, min_amount)?;
    Ok(NativeInputClaim {
        input,
        root: address(owner),
        opening: None,
    })
}

fn native_claim(context: &MdbxChainContext, slot: u32, opening: NativeOpening) -> NativeInputClaim {
    let value = slot_of(context, slot);
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

fn ordinary_output(slot: u32, amount: u64, owner: u8) -> NativeOutputClaim {
    NativeOutputClaim {
        output: TxOutput {
            slot_index: slot,
            amount,
            owner: address(owner),
        },
        opening: None,
    }
}

fn fee_for(context: &MdbxChainContext, inputs: usize, outputs: usize) -> u64 {
    MarketFeeContext::from_state(&context.state)
        .required(inputs, outputs)
        .unwrap()
}

fn child_height(context: &MdbxChainContext) -> u64 {
    context.tip_height() + 1
}

fn intent_transactions(intent: &NativeIntent) -> Vec<Transaction> {
    intent
        .pages
        .iter()
        .map(|page| Transaction::new(page.body.clone()))
        .collect()
}

fn base_intent(
    context: &MdbxChainContext,
    authority: u8,
    operation: NativeOperation,
    inputs: Vec<NativeInputClaim>,
    outputs: Vec<NativeOutputClaim>,
) -> Result<NativeIntent> {
    let height = child_height(context);
    let fee = fee_for(context, inputs.len(), outputs.len());
    let context_value = NativeContext {
        chain: native_chain_id(),
        height,
        authority: address(authority),
        fee,
    };
    Ok(NativeIntent::from_claims(
        &context_value,
        epoch_anchor(context)?,
        operation,
        inputs,
        outputs,
    )?)
}

fn financial_intent(
    context: &MdbxChainContext,
    authority: u8,
    operation: noid_tx::native_assets::financial::FinancialOperation,
    inputs: Vec<NativeInputClaim>,
    outputs: Vec<NativeOutputClaim>,
    openings: &[NativeOpening],
) -> Result<NativeIntent> {
    let height = child_height(context);
    let parent = *context.tip_header();
    let fee = fee_for(context, inputs.len(), outputs.len());
    let context_value = NativeContext {
        chain: native_chain_id(),
        height,
        authority: address(authority),
        fee,
    };
    let round = NftRoundContext::new(height, block_id(&parent), parent.alloc_counter)?;
    let keyed: std::collections::BTreeMap<[u8; 32], NativeOpening> =
        openings.iter().map(|o| (o.root().0, *o)).collect();
    Ok(NativeIntent::from_financial_claims(
        &context_value,
        epoch_anchor(context)?,
        operation,
        round,
        inputs,
        outputs,
        |root| keyed.get(&root.0).copied(),
    )?)
}

fn main() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let mut driver = ChainDriver {
        context: MdbxChainContext::open_or_create(&temporary.path().join("chain"))?,
        history: Vec::new(),
    };
    for _ in 0..24 {
        driver.coinbase()?;
    }
    let height_after_coinbases = driver.context.tip_height();
    println!("CONTROL common prefix: {height_after_coinbases} coinbase blocks");

    // h24+1: fund creator/seller/buyer from three miner coinbases.
    let anchor = epoch_anchor(&driver.context)?;
    let mut funding_txs = Vec::new();
    let mut used: Vec<u32> = Vec::new();
    for (slot, owner, amount) in [
        (900_000u32, CREATOR, CREATOR_CASH),
        (900_001, SELLER, SELLER_CASH),
        (900_002, BUYER, BUYER_CASH),
    ] {
        let (_, coin) = utxo_excluding(&driver.context, MINER, amount + 10_000, &used)?;
        used.push(coin.slot_index);
        let mut inputs = [TxInput::dummy(); 8];
        inputs[0] = coin;
        let mut outputs = [TxOutput::dummy(); 2];
        outputs[0] = TxOutput {
            slot_index: slot,
            amount,
            owner: address(owner),
        };
        let bitmap = 0b1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT;
        let fee = noid_chain::consensus::required_fee_for_tx_body(
            &TxBody {
                epoch_anchor: anchor,
                fee: 0,
                input_owner: address(MINER),
                inputs,
                outputs,
                validity_bitmap: bitmap,
                is_coinbase: false,
            },
            driver.context.tip_header().active_slot_count,
            driver.context.tip_header().log_slots,
        );
        outputs[0].amount = coin.amount - fee;
        assert!(
            outputs[0].amount >= amount,
            "coinbase must cover the intended funding"
        );
        funding_txs.push(Transaction::new(TxBody {
            epoch_anchor: anchor,
            fee,
            input_owner: address(MINER),
            inputs,
            outputs,
            validity_bitmap: bitmap,
            is_coinbase: false,
        }));
    }
    let bundle = driver.bundle(funding_txs, Vec::new())?;
    accept_annotated(&mut driver.context, &bundle)?;
    driver.history.push(bundle);

    // CreateCollection (op 5).
    let funding = ordinary_claim(&driver.context, CREATOR, 1_000)?;
    let genesis = GenesisAnchor {
        chain: native_chain_id(),
        owner: address(CREATOR),
        input: funding.input,
    };
    let collection = NftCollection::from_genesis(&genesis, 4, [0xA1; 32])?;
    let collection_out = native_output(910_000, NativeOpening::NftCollection(collection));
    let change = ordinary_output(
        910_001,
        funding.input.amount - collection_out.output.amount - fee_for(&driver.context, 1, 2),
        CREATOR,
    );
    let intent = base_intent(
        &driver.context,
        CREATOR,
        NativeOperation::CreateCollection { funding_input: 0 },
        vec![funding],
        vec![collection_out, change],
    )?;
    let bundle = driver.bundle(
        intent_transactions(&intent),
        vec![NativeOpening::NftCollection(collection)],
    )?;
    accept_annotated(&mut driver.context, &bundle)?;
    driver.history.push(bundle);
    let collection_slot = 910_000;
    let collection_id = slot_of(&driver.context, collection_slot).creation_id();
    println!("CONTROL collection created id={collection_id}");

    // MintNft (op 6): one item to the creator.
    let minted = NftCollection {
        issued_items: 1,
        ..collection
    };
    let item = NftItem {
        collection: collection.collection,
        owner: address(CREATOR),
        metadata: [0xB1; 32],
        serial: 1,
        unlock_height: 0,
    };
    let funding = ordinary_claim(&driver.context, CREATOR, 1_000)?;
    let collection_claim = native_claim(
        &driver.context,
        collection_slot,
        NativeOpening::NftCollection(collection),
    );
    let mint_fee = fee_for(&driver.context, 2, 3);
    let outputs = vec![
        native_output(910_010, NativeOpening::NftCollection(minted)),
        native_output(910_011, NativeOpening::NftItem(item)),
        ordinary_output(910_012, funding.input.amount - mint_fee, CREATOR),
    ];
    let intent = base_intent(
        &driver.context,
        CREATOR,
        NativeOperation::MintNft,
        vec![collection_claim, funding],
        outputs,
    )?;
    let bundle = driver.bundle(
        intent_transactions(&intent),
        vec![
            NativeOpening::NftCollection(collection),
            NativeOpening::NftCollection(minted),
            NativeOpening::NftItem(item),
        ],
    )?;
    accept_annotated(&mut driver.context, &bundle)?;
    driver.history.push(bundle);
    let item_slot = 910_011;
    println!("CONTROL item minted at slot {item_slot}");

    // TransferNft (op 7): creator -> seller. [common prefix ends here]
    let seller_item = NftItem {
        owner: address(SELLER),
        ..item
    };
    let funding = ordinary_claim(&driver.context, CREATOR, 1_000)?;
    let item_claim = native_claim(&driver.context, item_slot, NativeOpening::NftItem(item));
    let transfer_fee = fee_for(&driver.context, 2, 2);
    let outputs = vec![
        native_output(910_020, NativeOpening::NftItem(seller_item)),
        ordinary_output(910_021, funding.input.amount - transfer_fee, CREATOR),
    ];
    let intent = base_intent(
        &driver.context,
        CREATOR,
        NativeOperation::TransferNft,
        vec![item_claim, funding],
        outputs,
    )?;
    let transfer_bundle = driver.bundle(
        intent_transactions(&intent),
        vec![
            NativeOpening::NftItem(item),
            NativeOpening::NftItem(seller_item),
        ],
    )?;
    // The creator->seller transfer closes the common prefix: both forks
    // build on the block that leaves the seller-owned item at slot 910_020.
    accept_annotated(&mut driver.context, &transfer_bundle)?;
    driver.history.push(transfer_bundle);
    println!(
        "CONTROL common prefix ends at height {} with the seller-owned item at slot 910_020",
        driver.context.tip_height()
    );

    // Fork B (same parent): seller transfers the item on to the attacker.
    let attacker_item = NftItem {
        owner: address(ATTACKER),
        ..seller_item
    };
    let mut producer = ChainDriver {
        context: MdbxChainContext::open_or_create(&temporary.path().join("producer"))?,
        history: Vec::new(),
    };
    driver.replay_onto(&mut producer.context)?;
    // Fork A bundle on the main driver: CreateListing by the seller.
    let definition = ListingDefinition::from_item(
        seller_item,
        slot_of(&driver.context, 910_020).creation_id(),
        ASK,
        child_height(&driver.context) + 500,
        child_height(&driver.context),
    )?;
    let listing = ListingState::initial(definition)?;
    let listing_fee = fee_for(&driver.context, 2, 2);
    let inputs = vec![
        native_claim(
            &driver.context,
            910_020,
            NativeOpening::NftItem(seller_item),
        ),
        ordinary_claim(&driver.context, SELLER, listing_fee + NFT_FEE_CREDIT)?,
    ];
    let seller_cash_left = inputs[1].input.amount - listing_fee - NFT_FEE_CREDIT;
    let outputs = vec![
        native_output(
            910_030,
            NativeOpening::NftMarket(NftMarketOpening::Listing(listing)),
        ),
        ordinary_output(910_031, seller_cash_left, SELLER),
    ];
    let listing_intent = financial_intent(
        &driver.context,
        SELLER,
        noid_tx::native_assets::financial::FinancialOperation::CreateListing {
            definition: Address(definition.id().0),
        },
        inputs,
        outputs,
        &[
            NativeOpening::NftItem(seller_item),
            NativeOpening::NftMarket(NftMarketOpening::Listing(listing)),
            NativeOpening::NftMarket(NftMarketOpening::Definition(definition)),
        ],
    )?;
    let listing_bundle = driver.bundle(
        intent_transactions(&listing_intent),
        vec![
            NativeOpening::NftItem(seller_item),
            NativeOpening::NftMarket(NftMarketOpening::Listing(listing)),
            NativeOpening::NftMarket(NftMarketOpening::Definition(definition)),
        ],
    )?;
    let listing_slot = 910_030;
    let listing_creation_id_expected = {
        // The listing input consumed the item slot; the listing output slot
        // receives a fresh allocator id after acceptance.
        listing_slot
    };

    // Fork B: build the attacker transfer on the producer context.
    {
        let height = child_height(&producer.context);
        let fee = fee_for(&producer.context, 2, 2);
        let context_value = NativeContext {
            chain: native_chain_id(),
            height,
            authority: address(SELLER),
            fee,
        };
        let inputs = vec![
            native_claim(
                &producer.context,
                910_020,
                NativeOpening::NftItem(seller_item),
            ),
            ordinary_claim(&producer.context, SELLER, fee + 1_000)?,
        ];
        let outputs = vec![
            native_output(910_040, NativeOpening::NftItem(attacker_item)),
            ordinary_output(910_041, inputs[1].input.amount - fee, SELLER),
        ];
        let intent = NativeIntent::from_claims(
            &context_value,
            epoch_anchor(&producer.context)?,
            NativeOperation::TransferNft,
            inputs,
            outputs,
        )?;
        let bundle = producer.bundle(
            intent_transactions(&intent),
            vec![
                NativeOpening::NftItem(seller_item),
                NativeOpening::NftItem(attacker_item),
            ],
        )?;
        accept_annotated(&mut producer.context, &bundle)?;
        // Extension block for strictly more work.
        producer.coinbase()?;
    }
    let producer_bundles: Vec<AcceptedBlockBundle> = {
        // Re-derive the fork-B chain from the producer's tip (two blocks).
        let mut bundles = Vec::new();
        let tip = producer.context.tip_height();
        for height in (driver.context.tip_height() + 1)..=tip {
            let bytes = producer
                .context
                .store
                .get_recent_canonical_block(height)?
                .with_context(|| format!("producer block {height} missing"))?;
            let block = decode_block(&bytes)?;
            let mut terminal = noid_chain::HistoryStepTerminalMetadata::new(
                block.header.height,
                noid_chain::block_header::semantic_header_id(&block.header),
                0,
            )?
            .encode_prefix()
            .to_vec();
            terminal.push(0xA5);
            // Reconstruct openings from the body payload directly.
            let body_bytes = producer
                .context
                .store
                .get_block_body_object(height, block_id(&block.header))?
                .with_context(|| "producer body missing")?;
            let payload = noid_chain::block::payload::BlockBodyPayload::decode(&body_bytes)?;
            let (block2, openings, batches) = payload.into_parts();
            assert!(batches.is_empty());
            bundles.push(AcceptedBlockBundle::try_from_parts_with_native_openings(
                block2.to_bytes(),
                terminal,
                openings,
            )?);
        }
        bundles
    };

    // Accept fork A (listing) on the main chain.
    accept_annotated(&mut driver.context, &listing_bundle)?;
    let fork_a_height = driver.context.tip_height();
    let listing_value_on_a = slot_of(&driver.context, listing_slot);
    assert_ne!(listing_value_on_a, noid_chain::fri_state::SlotValue::EMPTY);
    println!(
        "CONTROL fork A accepted at height {fork_a_height}: listing live at slot {listing_slot}"
    );

    // Reorg onto fork B (two blocks, strictly more work).
    let common = driver.context.tip_height() - 1;
    let result = driver.context.apply_reorg_mdbx_with_applier(
        common,
        &producer_bundles,
        decode_block(producer_bundles.last().unwrap().block_bytes())?
            .header
            .timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    println!(
        "CONTROL reorg onto fork B: reverted {:?} applied {:?}; new tip height {}",
        result.reverted_heights,
        result.applied_heights,
        driver.context.tip_height()
    );

    // THE RACE: the fork-A listing must be gone; the stale purchase fails.
    let listing_value_after = slot_of(&driver.context, listing_slot);
    assert_eq!(
        listing_value_after,
        noid_chain::fri_state::SlotValue::EMPTY,
        "no fork-A listing state may survive the reorg"
    );
    let attacker_value = slot_of(&driver.context, 910_040);
    assert_ne!(attacker_value, noid_chain::fri_state::SlotValue::EMPTY);
    let seller_utxos = driver
        .context
        .store
        .get_verified_utxos_by_owner(&address(SELLER).0)?
        .utxos;
    let buyer_utxos = driver
        .context
        .store
        .get_verified_utxos_by_owner(&address(BUYER).0)?
        .utxos;
    println!(
        "CONTROL post-reorg: attacker owns the transferred item; seller utxos={} buyer utxos={}",
        seller_utxos.len(),
        buyer_utxos.len()
    );

    // Stale purchase of the fork-A listing coordinates on the B tip.
    let buyer_cash = buyer_utxos
        .iter()
        .find(|u| u.amount > ASK + 100_000)
        .context("buyer cash missing")?;
    let purchase_inputs = vec![
        NativeInputClaim {
            input: TxInput {
                slot_index: listing_slot,
                amount: listing_value_on_a.amount(),
                creation_id: listing_value_on_a.creation_id(),
            },
            root: NativeOpening::NftMarket(NftMarketOpening::Listing(listing)).root(),
            opening: Some(NativeOpening::NftMarket(NftMarketOpening::Listing(listing))),
        },
        NativeInputClaim {
            input: TxInput {
                slot_index: buyer_cash.slot_index,
                amount: buyer_cash.amount,
                creation_id: buyer_cash.creation_id,
            },
            root: address(BUYER),
            opening: None,
        },
    ];
    let purchase_fee = fee_for(&driver.context, 2, 3);
    let buyer_item = definition.item(address(BUYER))?;
    let purchase_outputs = vec![
        ordinary_output(910_050, ASK + NFT_FEE_CREDIT, SELLER),
        native_output(910_051, NativeOpening::NftItem(buyer_item)),
        ordinary_output(910_052, buyer_cash.amount - ASK - purchase_fee, BUYER),
    ];
    let stale_purchase = financial_intent(
        &driver.context,
        BUYER,
        noid_tx::native_assets::financial::FinancialOperation::Purchase {
            parent: noid_tx::native_assets::financial::NativeParentRef {
                slot_index: listing_slot,
                creation_id: listing_value_on_a.creation_id(),
                root: NativeOpening::NftMarket(NftMarketOpening::Listing(listing)).root(),
            },
        },
        purchase_inputs,
        purchase_outputs,
        &[
            NativeOpening::NftMarket(NftMarketOpening::Listing(listing)),
            NativeOpening::NftMarket(NftMarketOpening::Definition(definition)),
            NativeOpening::NftItem(buyer_item),
        ],
    );
    match stale_purchase {
        Ok(intent) => {
            // Even if the intent itself builds, the block template must
            // reject it against the reorged parent State.
            let openings = vec![
                NativeOpening::NftMarket(NftMarketOpening::Listing(listing)),
                NativeOpening::NftMarket(NftMarketOpening::Definition(definition)),
                NativeOpening::NftItem(buyer_item),
            ];
            let error = driver
                .bundle(intent_transactions(&intent), openings)
                .expect_err("stale listing purchase must not produce a block");
            println!("RACE stale-listing-buy rejected at admission: {error:#}");
            println!(
                "VERDICT the reorged State carries no stale listing: slot {listing_slot} empty, buy fails before inclusion"
            );
        }
        Err(error) => {
            println!("RACE stale-listing-buy rejected at intent construction: {error:#}");
        }
    }
    let _ = listing_creation_id_expected;

    // Same-block race: fresh listing on the B tip, then two competing buys.
    // Fund the item owner (the attacker) from a distinct miner coinbase.
    {
        let (_, coin) = utxo(&driver.context, MINER, 2_000_000)?;
        let anchor = epoch_anchor(&driver.context)?;
        let mut inputs = [TxInput::dummy(); 8];
        inputs[0] = coin;
        let mut outputs = [TxOutput::dummy(); 2];
        outputs[0] = TxOutput {
            slot_index: 900_004,
            amount: 1_500_000,
            owner: address(ATTACKER),
        };
        let bitmap = 0b1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT;
        let fee = noid_chain::consensus::required_fee_for_tx_body(
            &TxBody {
                epoch_anchor: anchor,
                fee: 0,
                input_owner: address(MINER),
                inputs,
                outputs,
                validity_bitmap: bitmap,
                is_coinbase: false,
            },
            driver.context.tip_header().active_slot_count,
            driver.context.tip_header().log_slots,
        );
        outputs[0].amount = coin.amount - fee;
        let fund = Transaction::new(TxBody {
            epoch_anchor: anchor,
            fee,
            input_owner: address(MINER),
            inputs,
            outputs,
            validity_bitmap: bitmap,
            is_coinbase: false,
        });
        let bundle = driver.bundle(vec![fund], Vec::new())?;
        accept_annotated(&mut driver.context, &bundle)?;
        driver.history.push(bundle);
    }
    let seller_definition = ListingDefinition::from_item(
        attacker_item,
        slot_of(&driver.context, 910_040).creation_id(),
        ASK,
        child_height(&driver.context) + 500,
        child_height(&driver.context),
    )?;
    let relisting = ListingState::initial(seller_definition)?;
    let relist_fee = fee_for(&driver.context, 2, 2);
    let relist_inputs = vec![
        native_claim(
            &driver.context,
            910_040,
            NativeOpening::NftItem(attacker_item),
        ),
        ordinary_claim(&driver.context, ATTACKER, relist_fee + NFT_FEE_CREDIT)?,
    ];
    let relist_outputs = vec![
        native_output(
            910_060,
            NativeOpening::NftMarket(NftMarketOpening::Listing(relisting)),
        ),
        ordinary_output(
            910_061,
            relist_inputs[1].input.amount - relist_fee - NFT_FEE_CREDIT,
            ATTACKER,
        ),
    ];
    let lister = ATTACKER;
    let relist_intent = financial_intent(
        &driver.context,
        ATTACKER,
        noid_tx::native_assets::financial::FinancialOperation::CreateListing {
            definition: Address(seller_definition.id().0),
        },
        relist_inputs,
        relist_outputs,
        &[
            NativeOpening::NftItem(attacker_item),
            NativeOpening::NftMarket(NftMarketOpening::Listing(relisting)),
            NativeOpening::NftMarket(NftMarketOpening::Definition(seller_definition)),
        ],
    )?;
    let relist_bundle = driver.bundle(
        intent_transactions(&relist_intent),
        vec![
            NativeOpening::NftItem(attacker_item),
            NativeOpening::NftMarket(NftMarketOpening::Listing(relisting)),
            NativeOpening::NftMarket(NftMarketOpening::Definition(seller_definition)),
        ],
    )?;
    accept_annotated(&mut driver.context, &relist_bundle)?;
    println!(
        "CONTROL re-listed by the attacker at slot 910_060, height {}",
        driver.context.tip_height()
    );

    let new_listing_slot = 910_060;
    let new_listing_value = slot_of(&driver.context, new_listing_slot);
    let make_buy = |context: &MdbxChainContext, buyer: u8| -> Result<NativeIntent> {
        let cash = utxo(context, buyer, ASK + 200_000)?.1;
        let inputs = vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: new_listing_slot,
                    amount: new_listing_value.amount(),
                    creation_id: new_listing_value.creation_id(),
                },
                root: NativeOpening::NftMarket(NftMarketOpening::Listing(relisting)).root(),
                opening: Some(NativeOpening::NftMarket(NftMarketOpening::Listing(
                    relisting,
                ))),
            },
            NativeInputClaim {
                input: cash,
                root: address(buyer),
                opening: None,
            },
        ];
        let fee = fee_for(context, 2, 3);
        let item = seller_definition.item(address(buyer))?;
        let outputs = vec![
            ordinary_output(910_070, ASK + NFT_FEE_CREDIT, lister),
            native_output(910_071, NativeOpening::NftItem(item)),
            ordinary_output(910_072, cash.amount - ASK - fee, buyer),
        ];
        financial_intent(
            context,
            buyer,
            noid_tx::native_assets::financial::FinancialOperation::Purchase {
                parent: noid_tx::native_assets::financial::NativeParentRef {
                    slot_index: new_listing_slot,
                    creation_id: new_listing_value.creation_id(),
                    root: NativeOpening::NftMarket(NftMarketOpening::Listing(relisting)).root(),
                },
            },
            inputs,
            outputs,
            &[
                NativeOpening::NftMarket(NftMarketOpening::Listing(relisting)),
                NativeOpening::NftMarket(NftMarketOpening::Definition(seller_definition)),
                NativeOpening::NftItem(item),
            ],
        )
    };

    // Fund a second buyer by splitting from the miner.
    let (_, miner_coin) = utxo_excluding(&driver.context, MINER, 2_000_000, &[900_004])?;
    let anchor = epoch_anchor(&driver.context)?;
    let mut inputs = [TxInput::dummy(); 8];
    inputs[0] = miner_coin;
    let mut outputs = [TxOutput::dummy(); 2];
    outputs[0] = TxOutput {
        slot_index: 900_003,
        amount: 1_500_000,
        owner: address(0x05),
    };
    let bitmap = 0b1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT;
    let fee = noid_chain::consensus::required_fee_for_tx_body(
        &TxBody {
            epoch_anchor: anchor,
            fee: 0,
            input_owner: address(MINER),
            inputs,
            outputs,
            validity_bitmap: bitmap,
            is_coinbase: false,
        },
        driver.context.tip_header().active_slot_count,
        driver.context.tip_header().log_slots,
    );
    outputs[0].amount = miner_coin.amount - fee;
    let fund = Transaction::new(TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner: address(MINER),
        inputs,
        outputs,
        validity_bitmap: bitmap,
        is_coinbase: false,
    });
    let bundle = driver.bundle(vec![fund], Vec::new())?;
    accept_annotated(&mut driver.context, &bundle)?;
    driver.history.push(bundle);

    let buy_a = make_buy(&driver.context, BUYER)?;
    let buy_b = make_buy(&driver.context, 0x05)?;

    // Each buy alone builds a block.
    let listing_and_definition = || {
        vec![
            NativeOpening::NftMarket(NftMarketOpening::Listing(relisting)),
            NativeOpening::NftMarket(NftMarketOpening::Definition(seller_definition)),
        ]
    };
    let buyer_a_item = seller_definition.item(address(BUYER))?;
    let buyer_b_item = seller_definition.item(address(0x05))?;
    let mut openings_a = listing_and_definition();
    openings_a.push(NativeOpening::NftItem(buyer_a_item));
    driver.bundle(intent_transactions(&buy_a), openings_a)?;
    let mut openings_b = listing_and_definition();
    openings_b.push(NativeOpening::NftItem(buyer_b_item));
    driver.bundle(intent_transactions(&buy_b), openings_b)?;
    println!("CONTROL each instant-buy alone produces a valid template");

    // Both in one template: the second application must abort the build.
    let mut both = intent_transactions(&buy_a);
    both.extend(intent_transactions(&buy_b));
    let mut openings_both = listing_and_definition();
    openings_both.push(NativeOpening::NftItem(buyer_a_item));
    openings_both.push(NativeOpening::NftItem(buyer_b_item));
    let outcome = driver.bundle(both, openings_both);
    match outcome {
        Ok(_) => bail!("two purchases of one listing built a block template"),
        Err(error) => println!(
            "RACE same-block double-buy: template build aborted ({:#}); no partial block, no double spend",
            error
        ),
    }
    println!("ALL CONTROLS AND RACES COMPLETE");
    Ok(())
}
