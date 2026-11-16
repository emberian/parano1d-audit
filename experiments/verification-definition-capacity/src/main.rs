//! Local CD-3 probe against Parano1d commit 52938433.
//! All parent state and blocks are synthetic; no peer or live chain is touched.

use std::{collections::BTreeMap, fs, path::PathBuf};

use noid_block::{
    candidate_history::prepare_candidate_input_with_native_data, HistoryStepPreparationContext,
};
use noid_chain::{
    block::{
        native_assets::{
            financial::FinancialHeaderWindow, materialize_accepted_block_state_with_native_data,
            try_compute_tx_root_at,
        },
        payload::BlockBodyPayload,
        Block,
    },
    consensus::{
        development_allocation::miner_subsidy_at_height,
        fees::fee_breakdown,
        forks::{ForkSchedule, V2Activation},
        genesis::genesis_header,
        params::{coinbase_creation_id, GENESIS_TARGET, TX_EPOCH_BLOCKS},
        validation::{validate_block_checks_with_schedule, AnchorInfo},
    },
    fri_state::SlotValue,
    hash_block_header,
    mempool::{BlockSelectionBudget, Mempool, NativeFinancialBudget},
    state::{native_assets::native_chain_id, ChainState},
    BlockHeader,
};
use noid_gkr::{
    ghost_tx::prove_selected_ghost_authorization_for_version,
    wallet_authorization::{
        prove_native_financial_authorization, verify_native_financial_authorization,
        WalletAuthorizationBundle,
    },
    zk_authorization::ZkAuthorizationProof,
    OwnerAuthWitness,
};
use noid_ivc_core::field_circuit::FieldR1csBuilder;
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_recursive::{
    acceptance::{
        history_step::{prepare_history_step_ghost_authorization_for_version, v2::V2Config},
        trace::native_assets::{
            financial_definitions::bind_financial_definition_table,
            sequential_trade::kernel::TradeKernelConfig,
        },
    },
    ChainAccumulator,
};
use noid_tx::{
    asset_backing::TokenDenomination,
    native_assets::{
        financial::FinancialOperation,
        launch::{launch_supply_from_whole, LaunchDefinition, LaunchOpening},
        market::MarketOpening,
        native_creation_id,
        nft_market::NftRoundContext,
        transition::{NativeContext, NativeInputClaim, NativeOutputClaim},
        wire::NativeIntent,
        FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening, TokenBalance,
        TokenReserve, TOKEN_DECIMALS,
    },
    output_bitmap_bit, OwnerAuthorizationVersion, Transaction, TxBody, TxInput, TxOutput,
    TX_INPUTS, TX_OUTPUTS,
};

fn signed_immediate_launch(
    state: &ChainState,
    parent: &BlockHeader,
    anchor: [u8; 32],
    secret: SpendSecret,
    funding_slot: u32,
    output_base: u32,
) -> (NativeIntent, WalletAuthorizationBundle) {
    let owner = derive_address(&secret);
    let slot = state.state.slot(funding_slot);
    let funding = TxInput {
        slot_index: funding_slot,
        amount: slot.amount(),
        creation_id: slot.creation_id(),
    };
    let genesis = GenesisAnchor {
        chain: native_chain_id(),
        owner,
        input: funding,
    };
    let supply = launch_supply_from_whole(100_000).unwrap();
    let token = FungibleDefinition::from_genesis(
        &genesis,
        supply,
        IssuanceRoute::Launchpad,
        [funding_slot as u8 + 1; 32],
    )
    .unwrap();
    let launch = LaunchDefinition::from_genesis(&genesis, token).unwrap();
    let reserve_amount = TokenDenomination::new(TOKEN_DECIMALS)
        .unwrap()
        .minimum_backing(supply)
        .unwrap();
    let reserve = TokenReserve::from_definition(&token, reserve_amount).unwrap();
    let sale_units = launch.sale_units();
    let pool = match launch.after_initial_purchase(sale_units).unwrap() {
        noid_tx::native_assets::launch::sequential::TradeState::Market(pool) => pool,
        _ => panic!("full initial purchase must graduate immediately"),
    };
    let control = NativeOpening::Market(MarketOpening::Pool(pool));
    let balance = NativeOpening::TokenBalance(TokenBalance {
        asset: token.asset,
        owner,
        units: sale_units,
        unlock_height: 0,
    });
    let mut outputs = vec![
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_base,
                amount: 0,
                owner: NativeOpening::FungibleDefinition(token).root(),
            },
            opening: Some(NativeOpening::FungibleDefinition(token)),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_base + 1,
                amount: reserve_amount,
                owner: NativeOpening::TokenReserve(reserve).root(),
            },
            opening: Some(NativeOpening::TokenReserve(reserve)),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_base + 2,
                amount: control.amount_micronoid(),
                owner: control.root(),
            },
            opening: Some(control),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_base + 3,
                amount: 0,
                owner: balance.root(),
            },
            opening: Some(balance),
        },
    ];
    let fee = fee_breakdown(1, 5, parent.active_slot_count, parent.log_slots).required_total;
    let spent = outputs.iter().map(|o| o.output.amount).sum::<u64>() + fee;
    assert!(funding.amount >= spent, "fixture funding is sufficient");
    outputs.push(NativeOutputClaim {
        output: TxOutput {
            slot_index: output_base + 4,
            amount: funding.amount - spent,
            owner,
        },
        opening: None,
    });
    let height = parent.height + 1;
    let round =
        NftRoundContext::new(height, hash_block_header(parent), state.alloc_counter).unwrap();
    let context = NativeContext {
        chain: native_chain_id(),
        height,
        authority: owner,
        fee,
    };
    let launch_opening = NativeOpening::Launch(LaunchOpening::Definition(launch));
    let market_opening =
        NativeOpening::Market(MarketOpening::Definition(launch.market_definition()));
    let mut intent = NativeIntent::from_financial_claims(
        &context,
        anchor,
        FinancialOperation::CreateLaunch {
            definition: launch_opening.root(),
            funding_input: 0,
        },
        round,
        vec![NativeInputClaim {
            input: funding,
            root: owner,
            opening: None,
        }],
        outputs,
        |root| {
            if root == launch_opening.root() {
                Some(launch_opening)
            } else if root == market_opening.root() {
                Some(market_opening)
            } else {
                None
            }
        },
    )
    .expect("native-valid immediately graduating CreateLaunch");
    let auth = prove_native_financial_authorization(
        &intent,
        context.chain,
        round,
        OwnerAuthWitness::new(secret),
        OwnerAuthorizationVersion::V4,
    )
    .expect("real owner authorization");
    verify_native_financial_authorization(
        &intent,
        context.chain,
        round,
        &auth,
        OwnerAuthorizationVersion::V4,
    )
    .expect("authorization verifies");
    intent.authorization_bytes = auth.to_bytes().unwrap();
    assert_eq!(
        intent
            .dependencies
            .iter()
            .filter(|o| o.is_auxiliary())
            .count(),
        2
    );
    (intent, auth)
}

struct Fixture {
    state: ChainState,
    parent: BlockHeader,
    schedule: ForkSchedule,
    config: V2Config,
    asert: AnchorInfo,
    anchor: [u8; 32],
    miner: Address,
    intents: Vec<NativeIntent>,
    proofs: Vec<ZkAuthorizationProof>,
}

impl Fixture {
    fn new() -> Self {
        let schedule = ForkSchedule::new(Some(5), V2Activation::new(10, 30)).unwrap();
        let config = V2Config::with_limits(24, 63, 256, 8, schedule)
            .unwrap()
            .with_authorization_update(24)
            .unwrap()
            .with_native_sequential_finance(
                4,
                64,
                TradeKernelConfig {
                    markets: 1,
                    policies: 2,
                    funding: 4,
                },
            )
            .unwrap();
        assert_eq!(
            config
                .native_financial_config()
                .unwrap()
                .kernel
                .definition_capacity,
            5
        );
        let secrets = [
            SpendSecret::from_bytes([0x41; 32]),
            SpendSecret::from_bytes([0x42; 32]),
            SpendSecret::from_bytes([0x43; 32]),
        ];
        let slots = secrets
            .iter()
            .enumerate()
            .map(|(i, secret)| {
                (
                    20 + i as u32,
                    SlotValue::with_owner_fields(
                        10_000_000_000,
                        (i + 1) as u64,
                        derive_address(secret).as_fields(),
                    ),
                )
            })
            .collect::<Vec<_>>();
        let state = ChainState::from_sparse_utxos(24, &slots, 3).unwrap();
        let height = TX_EPOCH_BLOCKS;
        let target = GENESIS_TARGET;
        let miner = Address([0x99; 32]);
        let parent = BlockHeader {
            prev_block_hash: [0x22; 32],
            state_root: state.cached_state_root(),
            tx_root: [0; 32],
            timestamp: 1_800_100_000,
            height,
            miner_address: miner,
            nonce: 0,
            difficulty_target: target,
            log_slots: 24,
            active_slot_count: state.active_slot_count,
            alloc_counter: state.alloc_counter,
        };
        let anchor = hash_block_header(&parent);
        let asert = AnchorInfo {
            anchor_height: height,
            anchor_timestamp: parent.timestamp,
            anchor_target: target,
        };
        let cache_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("auth-cache");
        fs::create_dir_all(&cache_dir).unwrap();
        let (intents, auths): (Vec<_>, Vec<_>) = secrets
            .into_iter()
            .enumerate()
            .map(|(i, secret)| {
                let path = cache_dir.join(format!("launch-{}.bin", i + 1));
                let result = if let Ok(bytes) = fs::read(&path) {
                    let intent = NativeIntent::from_bytes(&bytes).expect("cached native intent");
                    let auth = WalletAuthorizationBundle::from_bytes(&intent.authorization_bytes)
                        .expect("cached authorization bundle");
                    let round = NftRoundContext::new(
                        parent.height + 1,
                        hash_block_header(&parent),
                        state.alloc_counter,
                    )
                    .unwrap();
                    verify_native_financial_authorization(
                        &intent,
                        native_chain_id(),
                        round,
                        &auth,
                        OwnerAuthorizationVersion::V4,
                    )
                    .expect("cached proof remains valid for parent");
                    println!(
                        "loaded and verified owner authorization for launch {}",
                        i + 1
                    );
                    (intent, auth)
                } else {
                    let result = signed_immediate_launch(
                        &state,
                        &parent,
                        anchor,
                        secret,
                        20 + i as u32,
                        300 + i as u32 * 10,
                    );
                    fs::write(&path, result.0.to_bytes().unwrap()).unwrap();
                    println!(
                        "generated, verified, and cached owner authorization for launch {}",
                        i + 1
                    );
                    result
                };
                result
            })
            .unzip();
        let proofs = auths.into_iter().map(|a| a.proof).collect();
        Self {
            state,
            parent,
            schedule,
            config,
            asert,
            anchor,
            miner,
            intents,
            proofs,
        }
    }

    fn block(&self, count: usize) -> (Block, Vec<NativeOpening>) {
        let intents = &self.intents[..count];
        let height = self.parent.height + 1;
        let mut opening_map = BTreeMap::new();
        for intent in intents {
            for opening in intent
                .output_openings
                .iter()
                .flatten()
                .chain(&intent.dependencies)
            {
                opening_map.insert(opening.root().0, *opening);
            }
        }
        let claimable = intents
            .iter()
            .map(|intent| {
                let facts = intent.validate_structure().unwrap();
                let burned = fee_breakdown(
                    facts.live_inputs as u64,
                    facts.live_outputs as u64,
                    self.parent.active_slot_count,
                    self.parent.log_slots,
                )
                .burned;
                facts.fee - burned
            })
            .sum::<u64>();
        let coinbase = Transaction::new(TxBody {
            epoch_anchor: self.anchor,
            fee: 0,
            input_owner: Address([0; 32]),
            inputs: [TxInput::dummy(); TX_INPUTS],
            outputs: {
                let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
                outputs[0] = TxOutput {
                    slot_index: 95,
                    amount: miner_subsidy_at_height(height, 24) + claimable,
                    owner: self.miner,
                };
                outputs
            },
            validity_bitmap: output_bitmap_bit(0),
            is_coinbase: true,
        });
        let mut txs = vec![coinbase];
        for intent in intents {
            txs.extend(
                intent
                    .pages
                    .iter()
                    .map(|p| Transaction::new(p.body.clone())),
            );
        }
        let mut slots = BTreeMap::new();
        for slot in 20..23 {
            let value = self.state.state.slot(slot);
            slots.insert(slot, value);
        }
        let mut alloc = self.state.alloc_counter;
        for (_, output) in txs[0].body.live_outputs() {
            alloc += 1;
            slots.insert(
                output.slot_index,
                SlotValue::with_owner_fields(
                    output.amount,
                    coinbase_creation_id(height),
                    output.owner.as_fields(),
                ),
            );
        }
        for intent in intents {
            for page in &intent.pages {
                for (_, input) in page.body.live_inputs() {
                    slots.remove(&input.slot_index);
                }
            }
            let outputs = intent
                .pages
                .iter()
                .flat_map(|p| p.body.live_outputs().map(|(_, o)| *o))
                .collect::<Vec<_>>();
            for (output, opening) in outputs.iter().zip(&intent.output_openings) {
                alloc += 1;
                let creation = if opening.is_some() {
                    native_creation_id(alloc).unwrap()
                } else {
                    alloc
                };
                slots.insert(
                    output.slot_index,
                    SlotValue::with_owner_fields(output.amount, creation, output.owner.as_fields()),
                );
            }
        }
        let next = ChainState::from_sparse_utxos_at(
            24,
            &slots.into_iter().collect::<Vec<_>>(),
            alloc,
            height,
        )
        .unwrap();
        let block = Block {
            header: BlockHeader {
                prev_block_hash: self.anchor,
                state_root: next.cached_state_root(),
                tx_root: try_compute_tx_root_at(&txs, height).unwrap(),
                timestamp: self.parent.timestamp + 30,
                height,
                miner_address: self.miner,
                nonce: 0,
                difficulty_target: noid_chain::consensus::difficulty::next_target_with_schedule(
                    self.asert.anchor_height,
                    self.asert.anchor_timestamp,
                    &self.asert.anchor_target,
                    height,
                    self.parent.timestamp,
                    self.parent.timestamp + 30,
                    self.schedule,
                ),
                log_slots: 24,
                active_slot_count: next.active_slot_count,
                alloc_counter: next.alloc_counter,
            },
            transactions: txs,
        };
        (block, opening_map.into_values().collect())
    }

    fn check(&self, count: usize) {
        let (block, openings) = self.block(count);
        let definitions = openings
            .iter()
            .copied()
            .filter(NativeOpening::is_auxiliary)
            .collect::<Vec<_>>();
        let cap = self
            .config
            .native_financial_config()
            .unwrap()
            .kernel
            .definition_capacity;
        assert_eq!(definitions.len(), 2 * count);
        let at = block.header.timestamp;
        validate_block_checks_with_schedule(
            &block,
            &self.parent,
            &[self.parent.timestamp],
            &[self.parent.active_slot_count; 18],
            Some(at),
            &self.asert,
            false,
            self.schedule,
        )
        .expect("native block checks");
        BlockBodyPayload::new_with_native_data(block.clone(), openings.clone(), vec![])
            .expect("canonical native payload");
        let window = FinancialHeaderWindow::new(self.parent, &[]).unwrap();
        let mut materialized = self.state.clone();
        let map = openings
            .iter()
            .map(|o| (o.root().0, *o))
            .collect::<BTreeMap<_, _>>();
        materialize_accepted_block_state_with_native_data(
            &mut materialized,
            &block,
            &window,
            &[],
            |root| map.get(&root.0).copied(),
        )
        .expect("native financial state transition");
        assert_eq!(materialized.cached_state_root(), block.header.state_root);
        println!(
            "native block checks, payload, and state materialization accepted {count} launches"
        );

        // The production financial witness binder is the first observed capacity guard.
        let binder = std::panic::catch_unwind(|| {
            let mut builder = FieldR1csBuilder::new();
            bind_financial_definition_table(&mut builder, cap, &definitions);
        });
        assert_eq!(binder.is_err(), definitions.len() > cap);
        println!(
            "financial definition binder for {count} launches: {}",
            if binder.is_err() { "PANIC" } else { "OK" }
        );

        let ghost_proof =
            prove_selected_ghost_authorization_for_version(OwnerAuthorizationVersion::V4).unwrap();
        let ghost = prepare_history_step_ghost_authorization_for_version(
            ghost_proof,
            OwnerAuthorizationVersion::V4,
        )
        .unwrap();
        let genesis = genesis_header();
        let start = ChainAccumulator {
            height: self.parent.height,
            tip_semantic_id: noid_chain::block_header::semantic_header_id(&self.parent),
            state_root: self.parent.state_root,
            log_slots: self.parent.log_slots,
            active_slot_count: self.parent.active_slot_count,
            alloc_counter: self.parent.alloc_counter,
            epoch_anchor_id: hash_block_header(&genesis),
        };
        let context = HistoryStepPreparationContext {
            parent_header: &self.parent,
            tx_epoch_anchor_header: &genesis,
            parent_state: &self.state,
            start_accumulator: &start,
            previous_timestamps: &[self.parent.timestamp],
            finalized_active_counts: &[self.parent.active_slot_count; 18],
            asert_anchor: &self.asert,
            local_time: at,
        };
        let prepared = prepare_candidate_input_with_native_data::<63>(
            &block,
            context,
            self.proofs[..count].to_vec(),
            &ghost,
            &[],
            &openings,
            &[],
            &[],
            self.config,
        )
        .expect("real-authorized candidate preflight");
        assert_eq!(prepared.sealed_header(), &block.header);
        println!("candidate_history accepted {count} authorized launches, {} auxiliary definitions, capacity {cap}", definitions.len());
    }
}

fn main() {
    let fixture = Fixture::new();
    fixture.check(2);
    fixture.check(3);

    let mut pool = Mempool::new(16);
    for intent in &fixture.intents {
        pool.admit_native(intent.clone(), fixture.parent.height)
            .unwrap();
    }
    let cap = fixture
        .config
        .native_financial_config()
        .unwrap()
        .kernel
        .definition_capacity;
    let selected = pool.select_for_block_with_budget(
        BlockSelectionBudget {
            pages: 63,
            live_inputs: 256,
            contract_calls: 8,
            native_headers: 4,
            native_records: 64,
            owner_authorizations: 32,
            financial: Some(NativeFinancialBudget {
                actions: 64,
                definitions: cap,
                markets: 1,
                policies: 2,
                funding: 4,
                owner_operations: true,
            }),
        },
        &fixture.anchor,
    );
    assert_eq!(selected.len(), 2);
    println!(
        "mempool-selected launches under definition budget: {}",
        selected.len()
    );
}
