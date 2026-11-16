// SPDX-License-Identifier: Apache-2.0
//! Pinned local verification of the financial opcode partition.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};

    use noid_chain::{
        block::{
            native_assets::{
                financial::FinancialHeaderWindow,
                materialize_accepted_block_state_with_financial_context, try_compute_tx_root_at,
            },
            Block,
        },
        consensus::{
            development_allocation::miner_subsidy_at_height,
            fees::fee_breakdown,
            forks::{ForkSchedule, V2Activation},
            genesis::genesis_header,
            params::{coinbase_creation_id, V2_1_ACTIVATION_HEIGHT, V2_ACTIVATION_HEIGHT},
            validation::AnchorInfo,
            ConsensusError,
        },
        fri_state::SlotValue,
        hash_block_header,
        state::{native_assets::native_chain_id, ChainState},
        BlockHeader,
    };
    use noid_core::Block128;
    use noid_gkr::{
        ghost_tx::prove_selected_ghost_authorization_for_version,
        wallet_authorization::{
            prove_native_financial_authorization, verify_native_financial_authorization,
        },
        OwnerAuthWitness,
    };
    use noid_ivc_core::field_circuit::{FieldR1csBuilder, LinExpr};
    use noid_mempool::{config::MempoolConfig, view::ChainView, AsyncMempool};
    use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
    use noid_recursive::{
        acceptance::{
            history_step::{
                prepare_history_step_ghost_authorization_for_version,
                v2::{capacity_banked, V2Config, V2Error},
            },
            trace::{
                alloc_block,
                native_assets::{
                    financial_kernel::bind_financial_sequential_headers,
                    financial_operation::FINANCIAL_OPERATION_CODES,
                },
            },
        },
        ChainAccumulator,
    };
    use noid_tx::{
        native_assets::{
            financial::{
                public::{FinancialPublicContext, FinancialRegistration},
                FinancialOperation, NativeParentRef,
            },
            launch::{
                launch_supply_from_whole,
                queue::{LaunchRegisteredBatch, LaunchRequestRef},
                LaunchDefinition, LaunchOpening, LaunchRequest, LaunchSide, LaunchState,
                LAUNCH_PROFILE,
            },
            native_creation_id,
            nft_market::NftRoundContext,
            profile::{kind_enabled, operation_enabled, sequential_kind_enabled},
            transition::{NativeContext, NativeInputClaim, NativeOutputClaim},
            wire::NativeIntent,
            FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeKind, NativeOpening,
            TokenBalance, TokenReserve, TOKEN_DECIMALS,
        },
        output_bitmap_bit, OwnerAuthorizationVersion, Transaction, TxBody, TxInput, TxOutput,
        TX_INPUTS, TX_OUTPUTS,
    };
    #[test]
    fn pinned_mainnet_v21_capacity_and_bank() {
        use noid_recursive::acceptance::history_step::v2::capacity_banked::{
            Bank, Class, RuntimeParts,
        };
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../devnet-chaos/downloads/v21-pack/history-step-pack-v21/v21-runtime-metadata.bin"
        );
        let bytes = std::fs::read(path).expect("pinned v2.1 metadata artifact");
        assert_eq!(&bytes[..8], b"O1V21BN1");
        let len = u32::from_le_bytes(bytes[104..108].try_into().unwrap()) as usize;
        assert_eq!(len, bytes.len() - 108);
        let parts = RuntimeParts::decode_compact(&bytes[108..]).unwrap();
        let matrices: [[u8; 32]; 3] =
            std::array::from_fn(|i| bytes[8 + 32 * i..40 + 32 * i].try_into().unwrap());
        let bank = Bank::pin(matrices, &parts);
        assert_eq!(
            hex::encode(bank.digest()),
            "6c280ae869afc25e31d46d73b42870512991b78c9498bc7e5ef939bc07e5ed5e"
        );
        let config = parts.config();
        assert_eq!(config.activation_height(), 279_370);
        for (class, outer_m, pages) in [
            (Class::Small, 23, 63),
            (Class::Large, 24, 206),
            (Class::Native, 24, 63),
        ] {
            let c = config.class(class);
            assert_eq!(
                (
                    c.outer_m(),
                    c.pages(),
                    c.max_live_inputs(),
                    c.contract_slots()
                ),
                (outer_m, pages, 504, 63)
            );
            assert_eq!(c.authorization_activation(), Some(279_370));
        }
        for class in [Class::Small, Class::Large] {
            let c = config.class(class);
            assert_eq!(c.native_header_capacity(), Some(0));
            assert_eq!(c.native_financial_config(), None);
            assert_eq!(c.native_sequential_config(), None);
        }
        let native = config.class(Class::Native);
        assert_eq!(native.native_header_capacity(), Some(5));
        let financial = native.native_financial_config().unwrap();
        assert_eq!(financial.action_capacity, 128);
        assert_eq!(financial.kernel.settlement_action_capacity, 34);
        assert_eq!(financial.kernel.definition_capacity, 9);
        let sequential = native.native_sequential_config().unwrap();
        assert_eq!(
            (sequential.markets, sequential.policies, sequential.funding),
            (4, 48, 64)
        );
    }

    fn create_launch_intent(
        state: &ChainState,
        parent_header: &BlockHeader,
        height: u64,
        epoch_anchor: [u8; 32],
        creator: Address,
        funding_slot: u32,
        out_base_slot: u32,
        buy_all_to_graduate: bool,
    ) -> (
        NativeIntent,
        LaunchDefinition,
        FungibleDefinition,
        TokenReserve,
    ) {
        let chain = native_chain_id();
        let slot_val = state.state.slot(funding_slot);
        let funding_input = TxInput {
            slot_index: funding_slot,
            amount: slot_val.amount(),
            creation_id: slot_val.creation_id(),
        };
        let anchor = GenesisAnchor {
            chain,
            owner: creator,
            input: funding_input,
        };
        let supply = launch_supply_from_whole(100_000).unwrap();
        let token = FungibleDefinition::from_genesis(
            &anchor,
            supply,
            IssuanceRoute::Launchpad,
            [funding_slot as u8 + 1; 32],
        )
        .unwrap();
        let launch_def = LaunchDefinition::from_genesis(&anchor, token).unwrap();
        let min_backing = noid_tx::asset_backing::TokenDenomination::new(TOKEN_DECIMALS)
            .unwrap()
            .minimum_backing(supply)
            .unwrap();
        let reserve = TokenReserve::from_definition(&token, min_backing).unwrap();

        let purchased_units = if buy_all_to_graduate {
            launch_def.sale_units()
        } else {
            0
        };
        let control_opening = match launch_def.after_initial_purchase(purchased_units).unwrap() {
            noid_tx::native_assets::launch::sequential::TradeState::Curve(s) => {
                NativeOpening::Launch(LaunchOpening::State(s))
            }
            noid_tx::native_assets::launch::sequential::TradeState::Market(p) => {
                NativeOpening::Market(noid_tx::native_assets::market::MarketOpening::Pool(p))
            }
        };

        let mut outputs = vec![
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: out_base_slot,
                    amount: 0,
                    owner: NativeOpening::FungibleDefinition(token).root(),
                },
                opening: Some(NativeOpening::FungibleDefinition(token)),
            },
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: out_base_slot + 1,
                    amount: reserve.reserve_micronoid,
                    owner: NativeOpening::TokenReserve(reserve).root(),
                },
                opening: Some(NativeOpening::TokenReserve(reserve)),
            },
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: out_base_slot + 2,
                    amount: control_opening.amount_micronoid(),
                    owner: control_opening.root(),
                },
                opening: Some(control_opening),
            },
        ];
        if purchased_units > 0 {
            let bal = TokenBalance {
                asset: token.asset,
                owner: creator,
                units: purchased_units,
                unlock_height: 0,
            };
            outputs.push(NativeOutputClaim {
                output: TxOutput {
                    slot_index: out_base_slot + 3,
                    amount: 0,
                    owner: NativeOpening::TokenBalance(bal).root(),
                },
                opening: Some(NativeOpening::TokenBalance(bal)),
            });
        }

        // Add change output for remaining NOID after exact required fee.
        let n_outputs = outputs.len() + 1;
        let fee = fee_breakdown(
            1,
            n_outputs as u64,
            parent_header.active_slot_count,
            parent_header.log_slots,
        )
        .required_total;
        let spent_noid: u64 = outputs.iter().map(|o| o.output.amount).sum::<u64>() + fee;
        let change = funding_input.amount.checked_sub(spent_noid).unwrap();
        outputs.push(NativeOutputClaim {
            output: TxOutput {
                slot_index: out_base_slot + 4,
                amount: change,
                owner: creator,
            },
            opening: None,
        });

        let context = NativeContext {
            chain,
            height,
            authority: creator,
            fee,
        };
        let round = NftRoundContext::new(
            height,
            hash_block_header(parent_header),
            state.alloc_counter,
        )
        .unwrap();
        let def_opening = NativeOpening::Launch(LaunchOpening::Definition(launch_def));
        let mkt_opening =
            NativeOpening::Market(noid_tx::native_assets::market::MarketOpening::Definition(
                launch_def.market_definition(),
            ));
        let intent = NativeIntent::from_financial_claims(
            &context,
            epoch_anchor,
            FinancialOperation::CreateLaunch {
                definition: def_opening.root(),
                funding_input: 0,
            },
            round,
            vec![NativeInputClaim {
                input: funding_input,
                root: creator,
                opening: None,
            }],
            outputs,
            |root| {
                if root == def_opening.root() {
                    Some(def_opening)
                } else if root == mkt_opening.root() {
                    Some(mkt_opening)
                } else {
                    None
                }
            },
        )
        .expect("canonical CreateLaunch intent");

        (intent, launch_def, token, reserve)
    }
    #[tokio::test]
    async fn op49_native_materialization_vs_history_candidate() {
        // 1. Audit the static profile discrepancy across all opcodes 0..=80.
        let enabled_ops: Vec<u16> = (0u16..=90).filter(|&op| operation_enabled(op)).collect();
        assert_eq!(
            enabled_ops,
            vec![
                2, 3, 5, 6, 7, 8, // Base token/NFT ops
                33, 38, 39, 40, 41, 43, // Pool Swap, LP withdrawal/claim/burn, swap refund
                48, 49, 50, 51,
                52, // Launch genesis, LaunchRequest, round register/settle/refund
                53, // SettleListing
                64, 65, 66, // CreateListing, Purchase, CancelListing
                80, // ExecuteTrades (sequential batch)
            ]
        );
        let sequential_circuit_ops: HashSet<u16> =
            [2, 3, 5, 6, 7, 8, 48, 64, 65, 66, 80].into_iter().collect();
        let unprovable_consensus_ops: Vec<u16> = enabled_ops
            .iter()
            .copied()
            .filter(|op| !sequential_circuit_ops.contains(op))
            .collect();
        assert_eq!(
            unprovable_consensus_ops,
            vec![33, 38, 39, 40, 41, 43, 49, 50, 51, 52, 53],
            "11 consensus-enabled financial opcodes are excluded from Class::Native"
        );

        // Carrier-kind split: MarketRequest, MarketClaim, LaunchRequest are kind_enabled
        // in consensus, but excluded from sequential_kind_enabled!
        for kind in [
            NativeKind::MarketRequest,
            NativeKind::MarketClaim,
            NativeKind::LaunchRequest,
        ] {
            assert!(kind_enabled(kind));
            assert!(!sequential_kind_enabled(kind));
        }

        // 2. Verify capacity_banked::Config::new enforces `native_sequential_config().is_some()`
        //    on Class::Native, making non-sequential financial banks impossible in v2.1.
        let v2_height = V2_ACTIVATION_HEIGHT.unwrap_or(10);
        let v21_height = V2_1_ACTIVATION_HEIGHT.unwrap_or(24);
        let schedule = ForkSchedule::new(Some(5), V2Activation::new(v2_height, 30)).unwrap();
        let small_cfg = V2Config::with_limits(23, 63, 504, 63, schedule)
            .unwrap()
            .with_authorization_update(v21_height)
            .unwrap()
            .with_native_assets(0)
            .unwrap();
        let large_cfg = V2Config::with_limits(24, 206, 504, 63, schedule)
            .unwrap()
            .with_authorization_update(v21_height)
            .unwrap()
            .with_native_assets(0)
            .unwrap();
        let non_seq_native = V2Config::with_limits(24, 63, 504, 63, schedule)
            .unwrap()
            .with_authorization_update(v21_height)
            .unwrap()
            .with_native_finance(4, 64, 34)
            .unwrap();
        assert!(
            matches!(
                capacity_banked::Config::new(small_cfg, large_cfg, non_seq_native),
                Err(V2Error::Runtime)
            ),
            "v2.1 capacity_banked::Config forbids non-sequential Class::Native"
        );

        let seq_native = V2Config::with_limits(24, 63, 504, 63, schedule)
            .unwrap()
            .with_authorization_update(v21_height)
            .unwrap()
            .with_native_sequential_finance(
                4,
                64,
                noid_recursive::acceptance::trace::native_assets::sequential_trade::kernel::TradeKernelConfig {
                    markets: 1,
                    policies: 2,
                    funding: 4,
                },
            )
            .unwrap();
        assert!(capacity_banked::Config::new(small_cfg, large_cfg, seq_native).is_ok());

        // Build a funded local state, materialize CreateLaunch, then form a
        // complete LaunchRequest plus RegisterLaunches block.
        let creator = Address([0x31; 32]);
        let buyer_secret_bytes = [0x32; 32];
        let buyer = derive_address(&SpendSecret::from_bytes(buyer_secret_bytes));
        let miner = Address([0x77; 32]);
        let initial_slots = [
            (
                10u32,
                SlotValue::with_owner_fields(5_000_000_000, 1, creator.as_fields()),
            ),
            (
                11u32,
                SlotValue::with_owner_fields(5_000_000_000, 2, buyer.as_fields()),
            ),
        ];
        let mut state = ChainState::from_sparse_utxos(24, &initial_slots, 2).unwrap();

        let h0 = v21_height; // parent height before CreateLaunch block
        let epoch_genesis = genesis_header();
        let target = [0xFF; 32];
        let parent0 = BlockHeader {
            prev_block_hash: [0x10; 32],
            state_root: state.cached_state_root(),
            tx_root: [0; 32],
            timestamp: 1_800_000_000,
            height: h0,
            miner_address: miner,
            nonce: 0,
            difficulty_target: target,
            log_slots: 24,
            active_slot_count: state.active_slot_count,
            alloc_counter: state.alloc_counter,
        };

        // Block 1 (height = h0 + 1): CreateLaunch (op 48) with 0 initial purchase -> Curve state.
        let h1 = h0 + 1;
        let anchor1 = hash_block_header(&parent0);
        let (launch_intent, launch_def, _token, _reserve) = create_launch_intent(
            &state,
            &parent0,
            h1,
            hash_block_header(&epoch_genesis),
            creator,
            10,
            100,
            false,
        );
        let cb1_amt = miner_subsidy_at_height(h1, 24)
            + fee_breakdown(1, 4, parent0.active_slot_count, parent0.log_slots)
                .claimable_at_minimum();
        let cb1 = Transaction::new(TxBody {
            epoch_anchor: anchor1,
            fee: 0,
            input_owner: Address([0; 32]),
            inputs: [TxInput::dummy(); TX_INPUTS],
            outputs: {
                let mut o = [TxOutput::dummy(); TX_OUTPUTS];
                o[0] = TxOutput {
                    slot_index: 90,
                    amount: cb1_amt,
                    owner: miner,
                };
                o
            },
            validity_bitmap: output_bitmap_bit(0),
            is_coinbase: true,
        });
        let mut txs1 = vec![cb1];
        txs1.extend(
            launch_intent
                .pages
                .iter()
                .map(|p| Transaction::new(p.body.clone())),
        );
        let mut openings_map: BTreeMap<[u8; 32], NativeOpening> = launch_intent
            .output_openings
            .iter()
            .flatten()
            .chain(&launch_intent.dependencies)
            .map(|o| (o.root().0, *o))
            .collect();

        // Compute canonical post-state via sparse UTXO projection, then verify with
        // `materialize_accepted_block_state_with_financial_context`.
        let apply_intents_sparse = |live: &mut BTreeMap<u32, SlotValue>,
                                    mut alloc: u64,
                                    height: u64,
                                    cb: &TxBody,
                                    intents: &[NativeIntent]|
         -> ChainState {
            for (_, out) in cb.live_outputs() {
                alloc += 1;
                live.insert(
                    out.slot_index,
                    SlotValue::with_owner_fields(
                        out.amount,
                        coinbase_creation_id(height),
                        out.owner.as_fields(),
                    ),
                );
            }
            for intent in intents {
                for page in &intent.pages {
                    for (_, inp) in page.body.live_inputs() {
                        live.remove(&inp.slot_index);
                    }
                }
                let live_outs: Vec<_> = intent
                    .pages
                    .iter()
                    .flat_map(|p| p.body.live_outputs().map(|(_, o)| *o))
                    .collect();
                for (out, maybe_open) in live_outs.iter().zip(&intent.output_openings) {
                    alloc += 1;
                    let cid = if maybe_open.is_some() {
                        native_creation_id(alloc).unwrap()
                    } else {
                        alloc
                    };
                    live.insert(
                        out.slot_index,
                        SlotValue::with_owner_fields(out.amount, cid, out.owner.as_fields()),
                    );
                }
            }
            let entries: Vec<_> = live.iter().map(|(&k, &v)| (k, v)).collect();
            ChainState::from_sparse_utxos_at(24, &entries, alloc, height).unwrap()
        };

        let mut live_slots: BTreeMap<u32, SlotValue> = initial_slots.into_iter().collect();
        let expected_after_b1 = apply_intents_sparse(
            &mut live_slots,
            state.alloc_counter,
            h1,
            &txs1[0].body,
            std::slice::from_ref(&launch_intent),
        );

        let probe_block1 = Block {
            header: BlockHeader {
                prev_block_hash: anchor1,
                state_root: expected_after_b1.cached_state_root(),
                tx_root: try_compute_tx_root_at(&txs1, h1).unwrap(),
                timestamp: parent0.timestamp + 30,
                height: h1,
                miner_address: miner,
                nonce: 0,
                difficulty_target: target,
                log_slots: 24,
                active_slot_count: expected_after_b1.active_slot_count,
                alloc_counter: expected_after_b1.alloc_counter,
            },
            transactions: txs1,
        };
        let fin_win1 = FinancialHeaderWindow::new(parent0, &[]).unwrap();
        let transition1 = materialize_accepted_block_state_with_financial_context(
            &mut state,
            &probe_block1,
            &fin_win1,
            |r| openings_map.get(&r.0).copied(),
        )
        .expect("CreateLaunch block materializes cleanly");
        assert_eq!(transition1.new_state_root, probe_block1.header.state_root);

        // Locate the live LaunchState slot (slot 102) and its native creation_id.
        let launch_state_slot = 102u32;
        let launch_slot_val = state.state.slot(launch_state_slot);
        let launch_state = LaunchState::initial(launch_def).unwrap();
        assert_eq!(
            NativeOpening::Launch(LaunchOpening::State(launch_state))
                .root()
                .as_fields(),
            [launch_slot_val.owner_hi, launch_slot_val.owner_lo]
        );

        // Block 2 (height = h2 = h1 + 1): Consensus-valid `LaunchRequest` (op 49)!
        let parent1 = probe_block1.header;
        let h2 = h1 + 1;
        let epoch_header = epoch_genesis;
        let user_anchor = hash_block_header(&epoch_header);

        let req_opening = LaunchRequest {
            round: launch_state.round_id(h2).unwrap(),
            asset: launch_def.asset,
            owner: buyer,
            recipient: buyer,
            quantity: 1_000_000,
            noid_limit: 50_000_000,
            registered_height: h2,
            side: LaunchSide::Buy,
            profile: LAUNCH_PROFILE,
        };
        req_opening
            .check_round(launch_def, launch_state, h2)
            .unwrap();
        let req_native = NativeOpening::Launch(LaunchOpening::Request(req_opening));
        openings_map.insert(req_native.root().0, req_native);

        let fee2 = fee_breakdown(1, 2, parent1.active_slot_count, parent1.log_slots).required_total;
        let buyer_slot_val = state.state.slot(11);
        let buyer_in = TxInput {
            slot_index: 11,
            amount: buyer_slot_val.amount(),
            creation_id: buyer_slot_val.creation_id(),
        };
        let req_noid = req_native.amount_micronoid();
        let buyer_change = buyer_in.amount - req_noid - fee2;
        let parent_ref = NativeParentRef {
            slot_index: launch_state_slot,
            creation_id: launch_slot_val.creation_id(),
            root: NativeOpening::Launch(LaunchOpening::State(launch_state)).root(),
        };
        let op49 = FinancialOperation::LaunchRequest { parent: parent_ref };
        assert_eq!(op49.code(), 49);

        let round2 =
            NftRoundContext::new(h2, hash_block_header(&parent1), state.alloc_counter).unwrap();
        let ctx2 = NativeContext {
            chain: native_chain_id(),
            height: h2,
            authority: buyer,
            fee: fee2,
        };
        let mut op49_intent = NativeIntent::from_financial_claims(
            &ctx2,
            user_anchor,
            op49,
            round2,
            vec![NativeInputClaim {
                input: buyer_in,
                root: buyer,
                opening: None,
            }],
            vec![
                NativeOutputClaim {
                    output: TxOutput {
                        slot_index: 200,
                        amount: req_noid,
                        owner: req_native.root(),
                    },
                    opening: Some(req_native),
                },
                NativeOutputClaim {
                    output: TxOutput {
                        slot_index: 201,
                        amount: buyer_change,
                        owner: buyer,
                    },
                    opening: None,
                },
            ],
            |r| openings_map.get(&r.0).copied(),
        )
        .expect("LaunchRequest (op 49) builds a valid NativeIntent");
        let authorization = prove_native_financial_authorization(
            &op49_intent,
            native_chain_id(),
            round2,
            OwnerAuthWitness::new(SpendSecret::from_bytes(buyer_secret_bytes)),
            OwnerAuthorizationVersion::V4,
        )
        .expect("actual buyer V4 owner proof");
        verify_native_financial_authorization(
            &op49_intent,
            native_chain_id(),
            round2,
            &authorization,
            OwnerAuthorizationVersion::V4,
        )
        .expect("verify actual buyer V4 owner proof");
        op49_intent.authorization_bytes = authorization.to_bytes().unwrap();

        // A request is never a complete block on its own: the matching
        // registration operation must consume the controller in the suffix.
        let request_ref = LaunchRequestRef {
            slot_index: 200,
            creation_id: native_creation_id(state.alloc_counter + 2).unwrap(),
            request: req_opening,
        };
        let registered =
            LaunchRegisteredBatch::register(launch_def, launch_state, h2, &[request_ref]).unwrap();
        let frozen_opening = NativeOpening::Launch(LaunchOpening::State(registered.after()));
        openings_map.insert(frozen_opening.root().0, frozen_opening);
        let registrations = [FinancialRegistration::Launch {
            parent: parent_ref,
            request: request_ref,
        }];
        let public = FinancialPublicContext {
            round: round2,
            required_fee: fee_breakdown(1, 1, parent1.active_slot_count, parent1.log_slots)
                .required_total,
            listing_round: None,
            registrations: &registrations,
        };
        let register_intent = NativeIntent::from_public_financial_claims(
            &NativeContext {
                chain: native_chain_id(),
                height: h2,
                authority: Address([0; 32]),
                fee: 0,
            },
            user_anchor,
            FinancialOperation::RegisterLaunches,
            &public,
            vec![NativeInputClaim {
                input: TxInput {
                    slot_index: launch_state_slot,
                    amount: launch_slot_val.amount(),
                    creation_id: launch_slot_val.creation_id(),
                },
                root: parent_ref.root,
                opening: Some(NativeOpening::Launch(LaunchOpening::State(launch_state))),
            }],
            vec![NativeOutputClaim {
                output: TxOutput {
                    slot_index: 202,
                    amount: frozen_opening.amount_micronoid(),
                    owner: frozen_opening.root(),
                },
                opening: Some(frozen_opening),
            }],
            |root| openings_map.get(&root.0).copied(),
        )
        .expect("RegisterLaunches (op 50) builds a valid NativeIntent");

        let cb2_amt = miner_subsidy_at_height(h2, 24)
            + fee_breakdown(1, 2, parent1.active_slot_count, parent1.log_slots)
                .claimable_at_minimum();
        let cb2 = Transaction::new(TxBody {
            epoch_anchor: hash_block_header(&parent1),
            fee: 0,
            input_owner: Address([0; 32]),
            inputs: [TxInput::dummy(); TX_INPUTS],
            outputs: {
                let mut o = [TxOutput::dummy(); TX_OUTPUTS];
                o[0] = TxOutput {
                    slot_index: 91,
                    amount: cb2_amt,
                    owner: miner,
                };
                o
            },
            validity_bitmap: output_bitmap_bit(0),
            is_coinbase: true,
        });
        let mut txs2 = vec![cb2];
        txs2.extend(
            op49_intent
                .pages
                .iter()
                .map(|p| Transaction::new(p.body.clone())),
        );
        txs2.extend(
            register_intent
                .pages
                .iter()
                .map(|p| Transaction::new(p.body.clone())),
        );

        let fin_win2 = FinancialHeaderWindow::new(parent1, &[parent0]).unwrap();
        let state_for_b2 = apply_intents_sparse(
            &mut live_slots,
            state.alloc_counter,
            h2,
            &txs2[0].body,
            &[op49_intent.clone(), register_intent.clone()],
        );

        let ts2 = parent1.timestamp + 30;
        let asert2 = AnchorInfo {
            anchor_height: h1,
            anchor_timestamp: parent1.timestamp,
            anchor_target: target,
        };
        let child_target = noid_chain::consensus::difficulty::next_target_with_schedule(
            h1,
            parent1.timestamp,
            &target,
            h2,
            parent1.timestamp,
            ts2,
            schedule,
        );
        let block2 = Block {
            header: BlockHeader {
                prev_block_hash: hash_block_header(&parent1),
                state_root: state_for_b2.cached_state_root(),
                tx_root: try_compute_tx_root_at(&txs2, h2).unwrap(),
                timestamp: ts2,
                height: h2,
                miner_address: miner,
                nonce: 0,
                difficulty_target: child_target,
                log_slots: 24,
                active_slot_count: state_for_b2.active_slot_count,
                alloc_counter: state_for_b2.alloc_counter,
            },
            transactions: txs2,
        };

        let template_result =
            noid_chain::consensus::template::build_node_owned_block_template_with_native_data(
                &parent1,
                &state,
                &[parent1.active_slot_count; 18],
                op49_intent
                    .pages
                    .iter()
                    .map(|page| Transaction::new(page.body.clone()))
                    .collect(),
                miner,
                ts2,
                child_target,
                &HashSet::new(),
                &openings_map.values().copied().collect::<Vec<_>>(),
                &[],
            );
        assert!(
            matches!(template_result.as_ref(), Err(noid_chain::consensus::template::TemplateBuildError::StateApplyError(reason))
                if reason.contains("financial operation outside the template profile")),
            "template must reject op 49 at its profile gate: {:?}",
            template_result.err()
        );

        // The chain checks accept the complete op49+50 block without PoW.
        let native_res = noid_chain::consensus::validation::validate_block_checks_with_schedule(
            &block2,
            &parent1,
            &[parent0.timestamp, parent1.timestamp],
            &[parent1.active_slot_count; 18],
            Some(ts2),
            &asert2,
            false,
            schedule,
        );
        assert_eq!(
            native_res,
            Ok(()),
            "chain block checks accept the complete op49+50 block"
        );
        let mut state_after_b2 = state.clone();
        let mat_res = materialize_accepted_block_state_with_financial_context(
            &mut state_after_b2,
            &block2,
            &fin_win2,
            |r| openings_map.get(&r.0).copied(),
        );
        assert!(
            mat_res.is_ok(),
            "native materializer accepts the complete op49+50 block: {mat_res:?}"
        );

        // The selected Native proof class excludes op49 during candidate preparation.
        let ghost_raw =
            prove_selected_ghost_authorization_for_version(OwnerAuthorizationVersion::V4).unwrap();
        let ghost = prepare_history_step_ghost_authorization_for_version(
            ghost_raw,
            OwnerAuthorizationVersion::V4,
        )
        .unwrap();
        let start_acc = ChainAccumulator {
            height: parent1.height,
            tip_semantic_id: noid_chain::block_header::semantic_header_id(&parent1),
            state_root: parent1.state_root,
            log_slots: parent1.log_slots,
            active_slot_count: parent1.active_slot_count,
            alloc_counter: parent1.alloc_counter,
            epoch_anchor_id: user_anchor,
        };
        let prep_ctx = noid_block::HistoryStepPreparationContext {
            parent_header: &parent1,
            parent_state: &state,
            previous_timestamps: &[parent0.timestamp, parent1.timestamp],
            finalized_active_counts: &[parent1.active_slot_count; 18],
            local_time: ts2,
            asert_anchor: &asert2,
            start_accumulator: &start_acc,
            tx_epoch_anchor_header: &epoch_header,
        };
        let all_openings: Vec<_> = openings_map.values().copied().collect();
        let cand_res = noid_block::candidate_history::prepare_candidate_input_with_native_data::<63>(
            &block2,
            prep_ctx,
            vec![],
            &ghost,
            &[],
            &all_openings,
            &[parent0],
            &[],
            seq_native,
        );
        assert!(
            matches!(
                cand_res,
                Err(noid_block::HistoryStepWitnessError::Consensus(
                    ConsensusError::ShapeMismatch(ref msg)
                )) if msg.contains("native operation is outside the selected class")
            ),
            "HistoryStep candidate preparation rejects op-49 LaunchRequest: {:?}",
            cand_res.as_ref().err()
        );

        // Exercise the actual pinned sequential-header relation with the two
        // operation selectors. This is a gate unit test, not a full proof.
        use noid_recursive::acceptance::trace::native_assets::{
            financial_operation::NativeOperationMetadataTrace,
            header::{FinancialNativeHeaderTrace, NativeHeaderTrace},
        };
        let gate_satisfies = |code: u16| {
            let mut b = FieldR1csBuilder::new();
            let zero = LinExpr::zero();
            let one = alloc_block(&mut b, Block128::from(1u128));
            let mut financial = std::array::from_fn(|_| zero.clone());
            let index = FINANCIAL_OPERATION_CODES
                .iter()
                .position(|&candidate| candidate == code)
                .unwrap();
            financial[index] = one.clone();
            let header = NativeHeaderTrace {
                live: one.clone(),
                header_page: zero.clone(),
                operation: zero.clone(),
                selectors: std::array::from_fn(|_| zero.clone()),
                chain: std::array::from_fn(|_| zero.clone()),
                authority: std::array::from_fn(|_| zero.clone()),
                asset: std::array::from_fn(|_| zero.clone()),
                funding_input: zero.clone(),
                input_types: Vec::new(),
                output_types: Vec::new(),
                logical_txid: std::array::from_fn(|_| zero.clone()),
                input_count: zero.clone(),
                output_count: zero.clone(),
                fee: zero.clone(),
            };
            let metadata = NativeOperationMetadataTrace {
                inactive: zero.clone(),
                base: std::array::from_fn(|_| zero.clone()),
                financial,
                trade_root: std::array::from_fn(|_| zero.clone()),
                owner_required: zero.clone(),
                read_required: zero.clone(),
                registration_required: zero.clone(),
                definition_required: zero.clone(),
                funding_required: zero.clone(),
                read_slot: zero.clone(),
                read_creation_id: zero.clone(),
                read_sequence: zero.clone(),
                read_root: std::array::from_fn(|_| zero.clone()),
                definition_root: std::array::from_fn(|_| zero.clone()),
                asset: std::array::from_fn(|_| zero.clone()),
                funding_input: zero,
            };
            bind_financial_sequential_headers(
                &mut b,
                &[FinancialNativeHeaderTrace { header, metadata }],
            );
            let (r1cs, witness) = b.build();
            r1cs.satisfies(&witness)
        };
        assert!(
            gate_satisfies(48),
            "op 48 is the control accepted by the sequential-header relation"
        );
        assert!(
            !gate_satisfies(49),
            "op 49 is excluded by the sequential-header relation"
        );

        // 4C. Verify AsyncMempool cleanly rejects op 49 at the submission boundary (no prover wedge).
        let mut recent = HashMap::new();
        recent.insert(epoch_header.height, epoch_header);
        recent.insert(parent1.height, parent1);
        let view = ChainView::new(
            parent1.height,
            recent,
            state.active_slot_count,
            state.state.clone(),
        );
        let mempool = AsyncMempool::new(view, MempoolConfig::default());
        let encoded = op49_intent.to_bytes().unwrap();
        let submit_res = mempool.submit_encoded(encoded).await;
        assert!(
            matches!(
                submit_res,
                Err(noid_mempool::SubmitError::MalformedIntent(ref msg))
                    if msg.contains("outside the Owner admission profile")
            ),
            "AsyncMempool must reject op-49 LaunchRequest before proof verification: {submit_res:?}"
        );
    }
}
