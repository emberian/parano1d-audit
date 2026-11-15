use std::error::Error;

use noid_chain::block::native_assets::{
    build_native_block_action_surface, materialize_accepted_block_state_with_native_openings,
};
use noid_chain::consensus::difficulty::next_target;
use noid_chain::consensus::epoch_anchor::validate_block_epoch_anchors;
use noid_chain::consensus::fees::fee_breakdown;
use noid_chain::consensus::pow::search_pow;
use noid_chain::consensus::template::build_node_owned_block_template_with_native_openings;
use noid_chain::consensus::validation::{validate_block_checks, AnchorInfo};
use noid_chain::fri_state::SlotValue;
use noid_chain::state::native_assets::native_chain_id;
use noid_chain::{
    block_id, build_exact_action_surface_for_transactions_at_log_slots,
    materialize_accepted_block_state, Block, BlockHeader, ChainState,
};
use noid_poseidon2b::primitives::Address;
use noid_tx::native_assets::transition::{
    NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    native_creation_id, CollectionId, GenesisAnchor, NftCollection, NftItem, NATIVE_SEQUENCE_MAX,
};
use noid_tx::{
    output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

const HEIGHT: u64 = 25;
const USER_ANCHOR: [u8; 32] = [0xe0; 32];
const CREATOR_SLOT: u32 = 1;
const ATTACKER_SLOT: u32 = 2;
const NFT_SLOT: u32 = 3;
const COLLECTION_SLOT: u32 = 4;

#[derive(Clone)]
struct Fixture {
    state: ChainState,
    parent: BlockHeader,
    creator: Address,
    attacker: Address,
    item: NftItem,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let creator = Address([0x11; 32]);
        let attacker = Address([0x22; 32]);
        let anchor = GenesisAnchor {
            chain: native_chain_id(),
            owner: creator,
            input: TxInput {
                slot_index: 200,
                amount: 1_000_000,
                creation_id: 1,
            },
        };
        let mut collection = NftCollection::from_genesis(&anchor, 5, [0x51; 32])?;
        collection.issued_items = 1;
        let item = NftItem {
            collection: collection.collection,
            owner: creator,
            metadata: [0x61; 32],
            serial: 1,
            unlock_height: 0,
        };
        let collection_opening = noid_tx::native_assets::NativeOpening::NftCollection(collection);
        let item_opening = noid_tx::native_assets::NativeOpening::NftItem(item);
        let slots = [
            (
                CREATOR_SLOT,
                SlotValue::with_owner_fields(1_000_000, 1, creator.as_fields()),
            ),
            (
                ATTACKER_SLOT,
                SlotValue::with_owner_fields(1_000_000, 2, attacker.as_fields()),
            ),
            (
                NFT_SLOT,
                SlotValue::with_owner_fields(
                    0,
                    native_creation_id(3)?,
                    item_opening.root().as_fields(),
                ),
            ),
            (
                COLLECTION_SLOT,
                SlotValue::with_owner_fields(
                    0,
                    native_creation_id(4)?,
                    collection_opening.root().as_fields(),
                ),
            ),
        ];
        let state = ChainState::from_sparse_utxos_at(8, &slots, 10, HEIGHT - 1)
            .map_err(|error| format!("fixture state: {error:?}"))?;
        let parent = BlockHeader {
            prev_block_hash: [0x33; 32],
            state_root: state.cached_state_root(),
            tx_root: [0x44; 32],
            timestamp: 0,
            height: HEIGHT - 1,
            miner_address: Address([0x99; 32]),
            nonce: 0,
            difficulty_target: [0xff; 32],
            log_slots: 8,
            active_slot_count: state.active_slot_count,
            alloc_counter: state.alloc_counter,
        };
        Ok(Self {
            state,
            parent,
            creator,
            attacker,
            item,
        })
    }

    fn ordinary_input(&self, slot: u32, owner: Address) -> NativeInputClaim {
        let creation_id = if slot == CREATOR_SLOT { 1 } else { 2 };
        NativeInputClaim {
            input: TxInput {
                slot_index: slot,
                amount: 1_000_000,
                creation_id,
            },
            root: owner,
            opening: None,
        }
    }

    fn nft_input(&self) -> NativeInputClaim {
        NativeInputClaim {
            input: TxInput {
                slot_index: NFT_SLOT,
                amount: 0,
                creation_id: native_creation_id(3).unwrap(),
            },
            root: noid_tx::native_assets::NativeOpening::NftItem(self.item).root(),
            opening: Some(noid_tx::native_assets::NativeOpening::NftItem(self.item)),
        }
    }

    fn output(&self, slot: u32, amount: u64, owner: Address) -> NativeOutputClaim {
        NativeOutputClaim {
            output: TxOutput {
                slot_index: slot,
                amount,
                owner,
            },
            opening: None,
        }
    }

    fn native_output(
        &self,
        slot: u32,
        opening: noid_tx::native_assets::NativeOpening,
    ) -> NativeOutputClaim {
        NativeOutputClaim {
            output: TxOutput {
                slot_index: slot,
                amount: opening.amount_micronoid(),
                owner: opening.root(),
            },
            opening: Some(opening),
        }
    }

    fn intent(
        &self,
        authority: Address,
        operation: NativeOperation,
        inputs: Vec<NativeInputClaim>,
        outputs: Vec<NativeOutputClaim>,
    ) -> Result<NativeIntent, Box<dyn Error>> {
        let fee = fee_breakdown(
            inputs.len() as u64,
            outputs.len() as u64,
            self.parent.active_slot_count,
            self.parent.log_slots,
        )
        .required_total;
        Ok(NativeIntent::from_claims(
            &NativeContext {
                chain: native_chain_id(),
                height: HEIGHT,
                authority,
                fee,
            },
            USER_ANCHOR,
            operation,
            inputs,
            outputs,
        )?)
    }

    fn create_collection_intent(&self) -> Result<NativeIntent, Box<dyn Error>> {
        let new_collection = NftCollection::from_genesis(
            &GenesisAnchor {
                chain: native_chain_id(),
                owner: self.creator,
                input: self.ordinary_input(CREATOR_SLOT, self.creator).input,
            },
            7,
            [0x71; 32],
        )?;
        let fee = fee_breakdown(1, 2, self.parent.active_slot_count, self.parent.log_slots)
            .required_total;
        self.intent(
            self.creator,
            NativeOperation::CreateCollection { funding_input: 0 },
            vec![self.ordinary_input(CREATOR_SLOT, self.creator)],
            vec![
                self.native_output(
                    10,
                    noid_tx::native_assets::NativeOpening::NftCollection(new_collection),
                ),
                self.output(11, 1_000_000 - fee, self.creator),
            ],
        )
    }

    fn build(
        &self,
        intents: &[NativeIntent],
    ) -> Result<(Block, Vec<noid_tx::native_assets::NativeOpening>), Box<dyn Error>> {
        let transactions = intents
            .iter()
            .flat_map(|intent| {
                intent
                    .pages
                    .iter()
                    .map(|page| Transaction::new(page.body.clone()))
            })
            .collect();
        let openings = intents
            .iter()
            .flat_map(|intent| {
                intent
                    .input_openings
                    .iter()
                    .chain(&intent.output_openings)
                    .flatten()
                    .copied()
            })
            .collect::<Vec<_>>();
        for intent in intents {
            NativeIntent::from_pages_with_openings(intent.pages.clone(), |root| {
                openings
                    .iter()
                    .find(|opening| opening.root() == root)
                    .copied()
            })
            .map_err(|error| format!("opening rehydration: {error:?}"))?;
        }
        let target = next_target(
            self.parent.height,
            self.parent.timestamp,
            &self.parent.difficulty_target,
            HEIGHT,
            self.parent.timestamp,
            30,
        );
        let (template, _) = build_node_owned_block_template_with_native_openings(
            &self.parent,
            &self.state,
            &[],
            transactions,
            Address([0x99; 32]),
            30,
            target,
            &std::collections::HashSet::new(),
            &openings,
        )
        .map_err(|error| format!("template: {error:?}"))?;
        Ok((template.into_block(0), openings))
    }

    fn validate(
        &self,
        block: &Block,
        openings: &[noid_tx::native_assets::NativeOpening],
    ) -> Result<ChainState, Box<dyn Error>> {
        let mut sealed = block.clone();
        sealed.header.nonce =
            search_pow(&sealed.header, 0, 2_000_000).ok_or("local PoW search exhausted")?;
        validate_block_checks(
            &sealed,
            &self.parent,
            &[self.parent.timestamp],
            &[],
            sealed.header.timestamp,
            &AnchorInfo {
                anchor_height: self.parent.height,
                anchor_timestamp: self.parent.timestamp,
                anchor_target: self.parent.difficulty_target,
            },
        )
        .map_err(|error| format!("full native block checks: {error:?}"))?;
        validate_block_epoch_anchors(&sealed, USER_ANCHOR, block_id(&self.parent))
            .map_err(|error| format!("epoch anchors: {error:?}"))?;
        let surface = build_native_block_action_surface(&self.state, &sealed, |root| {
            openings
                .iter()
                .find(|opening| opening.root() == root)
                .copied()
        })
        .map_err(|error| format!("native exact action surface: {error:?}"))?;
        let mut post = self.state.clone();
        materialize_accepted_block_state_with_native_openings(&mut post, &sealed, |root| {
            openings
                .iter()
                .find(|opening| opening.root() == root)
                .copied()
        })
        .map_err(|error| format!("native block materialization: {error:?}"))?;
        let expected_active =
            self.state.active_slot_count - u64::from(surface.spends) + u64::from(surface.mints);
        let expected_alloc = self.state.alloc_counter + u64::from(surface.mints);
        assert_eq!(post.active_slot_count, expected_active);
        assert_eq!(post.alloc_counter, expected_alloc);
        assert_eq!(post.cached_state_root(), sealed.header.state_root);
        let decoded = Block::from_bytes(&sealed.to_bytes())
            .map_err(|error| format!("block wire roundtrip: {error:?}"))?;
        assert_eq!(decoded, sealed);
        Ok(post)
    }
}

fn reject(label: &str, result: Result<impl Sized, Box<dyn Error>>) {
    match result {
        Ok(_) => println!("UNEXPECTED ACCEPT: {label}"),
        Err(error) => println!("reject {label}: {error}"),
    }
}

fn valid_control(f: &Fixture) -> Result<(), Box<dyn Error>> {
    let intent = f.create_collection_intent()?;
    let (block, openings) = f.build(std::slice::from_ref(&intent))?;
    let post = f.validate(&block, &openings)?;
    let native = post.state.slot(10);
    assert_eq!(native.amount(), 0);
    assert_eq!(native.creation_id(), native_creation_id(12)?);
    assert!(!native.is_empty());
    assert_eq!(post.state.slot(11).creation_id(), 13);
    assert_eq!(post.circulating_supply_micronoid, 16_397_500);
    assert_eq!((post.active_slot_count, post.alloc_counter), (6, 13));
    println!("accept valid CreateCollection control: active=6 alloc=13 supply=16397500 native_slot_10_id={} nonempty=true", native.creation_id());
    Ok(())
}

fn semantic_attacks(f: &Fixture) -> Result<(), Box<dyn Error>> {
    let valid = f.create_collection_intent()?;
    let fee = fee_breakdown(1, 2, f.parent.active_slot_count, f.parent.log_slots).required_total;
    let collection = match valid.output_openings[0].unwrap() {
        noid_tx::native_assets::NativeOpening::NftCollection(collection) => collection,
        _ => unreachable!(),
    };

    reject(
        "NOID output inflation by 1",
        (|| {
            f.intent(
                f.creator,
                NativeOperation::CreateCollection { funding_input: 0 },
                vec![f.ordinary_input(CREATOR_SLOT, f.creator)],
                vec![
                    f.native_output(
                        10,
                        noid_tx::native_assets::NativeOpening::NftCollection(collection),
                    ),
                    f.output(11, 1_000_000 - fee + 1, f.creator),
                ],
            )?;
            Ok(())
        })(),
    );

    reject(
        "unrelated collection identity",
        (|| {
            let wrong = NftCollection {
                collection: CollectionId([0xad; 32]),
                ..collection
            };
            f.intent(
                f.creator,
                NativeOperation::CreateCollection { funding_input: 0 },
                vec![f.ordinary_input(CREATOR_SLOT, f.creator)],
                vec![
                    f.native_output(
                        10,
                        noid_tx::native_assets::NativeOpening::NftCollection(wrong),
                    ),
                    f.output(11, 1_000_000 - fee, f.creator),
                ],
            )?;
            Ok::<&str, Box<dyn Error>>("forged collection unexpectedly passed".into())
        })(),
    );

    reject(
        "duplicate native inputs",
        (|| {
            f.intent(
                f.creator,
                NativeOperation::CreateCollection { funding_input: 0 },
                vec![
                    f.ordinary_input(CREATOR_SLOT, f.creator),
                    f.ordinary_input(CREATOR_SLOT, f.creator),
                ],
                vec![f.native_output(
                    10,
                    noid_tx::native_assets::NativeOpening::NftCollection(collection),
                )],
            )?;
            Ok::<&str, Box<dyn Error>>("duplicate inputs unexpectedly passed".into())
        })(),
    );

    reject(
        "duplicate output slots",
        (|| {
            f.intent(
                f.creator,
                NativeOperation::CreateCollection { funding_input: 0 },
                vec![f.ordinary_input(CREATOR_SLOT, f.creator)],
                vec![
                    f.native_output(
                        10,
                        noid_tx::native_assets::NativeOpening::NftCollection(collection),
                    ),
                    f.output(10, 1_000_000 - fee, f.creator),
                ],
            )?;
            Ok::<&str, Box<dyn Error>>("duplicate outputs unexpectedly passed".into())
        })(),
    );

    reject(
        "unauthorized NFT transfer by non-owner",
        (|| {
            let fee =
                fee_breakdown(2, 2, f.parent.active_slot_count, f.parent.log_slots).required_total;
            f.intent(
                f.attacker,
                NativeOperation::TransferNft,
                vec![f.nft_input(), f.ordinary_input(ATTACKER_SLOT, f.attacker)],
                vec![
                    f.native_output(10, noid_tx::native_assets::NativeOpening::NftItem(f.item)),
                    f.output(11, 1_000_000 - fee, f.attacker),
                ],
            )?;
            Ok::<&str, Box<dyn Error>>(
                "non-owner transfer unexpectedly passed typed authority".into(),
            )
        })(),
    );

    reject(
        "same native input in two groups",
        (|| {
            let fee =
                fee_breakdown(2, 2, f.parent.active_slot_count, f.parent.log_slots).required_total;
            let transfer = f.intent(
                f.creator,
                NativeOperation::TransferNft,
                vec![f.nft_input(), f.ordinary_input(CREATOR_SLOT, f.creator)],
                vec![
                    f.native_output(10, noid_tx::native_assets::NativeOpening::NftItem(f.item)),
                    f.output(11, 1_000_000 - fee, f.creator),
                ],
            )?;
            f.build(&[transfer.clone(), transfer])?;
            Ok::<&str, Box<dyn Error>>(
                "two groups consumed one native slot without conflict".into(),
            )
        })(),
    );

    reject(
        "out-of-range native output",
        (|| {
            let intent = f.intent(
                f.creator,
                NativeOperation::CreateCollection { funding_input: 0 },
                vec![f.ordinary_input(CREATOR_SLOT, f.creator)],
                vec![
                    f.native_output(
                        256,
                        noid_tx::native_assets::NativeOpening::NftCollection(collection),
                    ),
                    f.output(11, 1_000_000 - fee, f.creator),
                ],
            )?;
            f.build(&[intent])?;
            Ok::<&str, Box<dyn Error>>("out-of-range output unexpectedly materialized".into())
        })(),
    );

    reject(
        "occupied native output slot",
        (|| {
            let intent = f.intent(
                f.creator,
                NativeOperation::CreateCollection { funding_input: 0 },
                vec![f.ordinary_input(CREATOR_SLOT, f.creator)],
                vec![
                    f.native_output(
                        ATTACKER_SLOT,
                        noid_tx::native_assets::NativeOpening::NftCollection(collection),
                    ),
                    f.output(11, 1_000_000 - fee, f.creator),
                ],
            )?;
            f.build(&[intent])?;
            Ok::<&str, Box<dyn Error>>("occupied output unexpectedly materialized".into())
        })(),
    );
    Ok(())
}

fn header_and_allocator_attacks(f: &Fixture) -> Result<(), Box<dyn Error>> {
    let intent = f.create_collection_intent()?;
    let (block, openings) = f.build(&[intent])?;

    let mut inflated = block.clone();
    inflated.transactions[0].body.outputs[0].amount += 1;
    reject(
        "coinbase subsidy plus 1",
        (|| {
            f.validate(&inflated, &openings)?;
            Ok(())
        })(),
    );

    let mut burn_claim = block.clone();
    burn_claim.transactions[0].body.outputs[0].amount += 2_500;
    reject(
        "claim state-growth burn in coinbase",
        (|| {
            f.validate(&burn_claim, &openings)?;
            Ok::<&str, Box<dyn Error>>("burned fee claimed successfully".into())
        })(),
    );

    let mut bad_active = block.clone();
    bad_active.header.active_slot_count += 1;
    reject(
        "header active_slot_count plus 1",
        (|| {
            f.validate(&bad_active, &openings)?;
            Ok::<&str, Box<dyn Error>>("forged active count accepted".into())
        })(),
    );

    let mut bad_alloc = block.clone();
    bad_alloc.header.alloc_counter += 1;
    reject(
        "header alloc_counter plus 1",
        (|| {
            f.validate(&bad_alloc, &openings)?;
            Ok::<&str, Box<dyn Error>>("forged allocator accepted".into())
        })(),
    );

    let mut expanded = block.clone();
    expanded.header.log_slots += 1;
    reject(
        "unauthorized one-level slot expansion",
        (|| {
            f.validate(&expanded, &openings)?;
            Ok::<&str, Box<dyn Error>>("untriggered expansion accepted".into())
        })(),
    );

    let mut exhausted = f.clone();
    exhausted.state.alloc_counter = NATIVE_SEQUENCE_MAX;
    exhausted.parent.alloc_counter = NATIVE_SEQUENCE_MAX;
    reject(
        "allocator at native namespace maximum",
        (|| {
            exhausted.build(&[f.create_collection_intent()?])?;
            Ok::<&str, Box<dyn Error>>("mint beyond allocator namespace accepted".into())
        })(),
    );
    Ok(())
}

fn ordinary_tx(
    f: &Fixture,
    input_slot: u32,
    claimed_owner: Address,
    outputs: [TxOutput; TX_OUTPUTS],
    output_count: usize,
    fee: u64,
) -> Transaction {
    let slot = f.state.state.slot(input_slot);
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: input_slot,
        amount: slot.amount(),
        creation_id: slot.creation_id(),
    };
    let output_bits = (0..output_count).fold(0u16, |bits, index| bits | output_bitmap_bit(index));
    Transaction::new(TxBody {
        epoch_anchor: USER_ANCHOR,
        fee,
        input_owner: claimed_owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bits | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
}

fn build_ordinary(f: &Fixture, transactions: Vec<Transaction>) -> Result<Block, Box<dyn Error>> {
    let target = next_target(
        f.parent.height,
        f.parent.timestamp,
        &f.parent.difficulty_target,
        HEIGHT,
        f.parent.timestamp,
        30,
    );
    let (template, _) = build_node_owned_block_template_with_native_openings(
        &f.parent,
        &f.state,
        &[],
        transactions,
        Address([0x99; 32]),
        30,
        target,
        &std::collections::HashSet::new(),
        &[],
    )
    .map_err(|error| format!("ordinary template: {error:?}"))?;
    Ok(template.into_block(0))
}

fn build_and_validate_ordinary(
    f: &Fixture,
    transactions: Vec<Transaction>,
) -> Result<ChainState, Box<dyn Error>> {
    let block = build_ordinary(f, transactions)?;
    let post = f.validate(&block, &[])?;
    let surface = build_exact_action_surface_for_transactions_at_log_slots(
        &f.state.state,
        block.header.log_slots,
        &block.transactions,
        f.state.alloc_counter,
        block.header.height,
    )
    .map_err(|error| format!("ordinary exact action surface: {error:?}"))?;
    let mut ordinary_post = f.state.clone();
    materialize_accepted_block_state(&mut ordinary_post, &block)
        .map_err(|error| format!("ordinary materialization: {error:?}"))?;
    assert_eq!(ordinary_post.cached_state_root(), post.cached_state_root());
    assert_eq!(ordinary_post.active_slot_count, post.active_slot_count);
    assert_eq!(ordinary_post.alloc_counter, post.alloc_counter);
    assert_eq!(
        ordinary_post.circulating_supply_micronoid,
        post.circulating_supply_micronoid
    );
    assert_eq!(
        u64::from(surface.mints) + f.state.alloc_counter,
        post.alloc_counter
    );
    Ok(post)
}

fn ordinary_matrix(f: &Fixture) -> Result<(), Box<dyn Error>> {
    let fee = fee_breakdown(1, 2, f.parent.active_slot_count, f.parent.log_slots).required_total;
    let zero = TxOutput {
        slot_index: 10,
        ..TxOutput::dummy()
    };
    let change = TxOutput {
        slot_index: 11,
        amount: 1_000_000 - fee,
        owner: f.creator,
    };
    let valid = ordinary_tx(f, CREATOR_SLOT, f.creator, [zero, change], 2, fee);
    let post = build_and_validate_ordinary(f, vec![valid.clone()])?;
    assert_eq!(post.state.slot(10).amount(), 0);
    assert_eq!(post.state.slot(10).creation_id(), 12);
    assert!(!post.state.slot(10).is_empty());
    assert_eq!(post.circulating_supply_micronoid, 16_397_500);
    println!("accept ordinary zero/zero output control: slot_10_id=12 nonempty=true; ordinary/native materializers and exact surfaces agree");

    reject(
        "ordinary output inflation by 1",
        (|| {
            let mut inflated = change;
            inflated.amount += 1;
            build_and_validate_ordinary(
                f,
                vec![ordinary_tx(
                    f,
                    CREATOR_SLOT,
                    f.creator,
                    [zero, inflated],
                    2,
                    fee,
                )],
            )?;
            Ok::<&str, Box<dyn Error>>("inflated ordinary output accepted".into())
        })(),
    );

    reject(
        "ordinary wrong input owner",
        (|| {
            build_and_validate_ordinary(
                f,
                vec![ordinary_tx(
                    f,
                    CREATOR_SLOT,
                    f.attacker,
                    [zero, change],
                    2,
                    fee,
                )],
            )?;
            Ok::<&str, Box<dyn Error>>("wrong owner spent creator UTXO".into())
        })(),
    );

    reject(
        "ordinary wrong creation_id",
        (|| {
            let mut wrong = valid.clone();
            wrong.body.inputs[0].creation_id += 1;
            build_and_validate_ordinary(f, vec![wrong])?;
            Ok::<&str, Box<dyn Error>>("wrong incarnation spent UTXO".into())
        })(),
    );

    reject(
        "ordinary duplicate input across groups",
        (|| {
            let mut second = valid.clone();
            second.body.outputs[0].slot_index = 12;
            second.body.outputs[1].slot_index = 13;
            build_and_validate_ordinary(f, vec![valid.clone(), second])?;
            Ok::<&str, Box<dyn Error>>("same ordinary input accepted twice".into())
        })(),
    );

    reject(
        "ordinary duplicate outputs in one body",
        (|| {
            let first = TxOutput {
                slot_index: 10,
                amount: 400_000,
                owner: f.creator,
            };
            let second = TxOutput {
                slot_index: 10,
                amount: 1_000_000 - fee - 400_000,
                owner: f.attacker,
            };
            build_and_validate_ordinary(
                f,
                vec![ordinary_tx(
                    f,
                    CREATOR_SLOT,
                    f.creator,
                    [first, second],
                    2,
                    fee,
                )],
            )?;
            Ok::<&str, Box<dyn Error>>("duplicate output slots accepted".into())
        })(),
    );

    reject(
        "ordinary input/output slot overlap",
        (|| {
            let spent_again = TxOutput {
                slot_index: CREATOR_SLOT,
                amount: 0,
                owner: Address([0; 32]),
            };
            build_and_validate_ordinary(
                f,
                vec![ordinary_tx(
                    f,
                    CREATOR_SLOT,
                    f.creator,
                    [spent_again, change],
                    2,
                    fee,
                )],
            )?;
            Ok::<&str, Box<dyn Error>>("same-body slot reuse accepted".into())
        })(),
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!(
        "state-consensus local PoC; height={HEIGHT}, isolated activation only; no live target"
    );
    let fixture = Fixture::new()?;
    valid_control(&fixture)?;
    semantic_attacks(&fixture)?;
    header_and_allocator_attacks(&fixture)?;
    ordinary_matrix(&fixture)?;
    println!("RESULT: no accepted-invalid transition in this matrix; any UNEXPECTED ACCEPT above is a finding");
    Ok(())
}
