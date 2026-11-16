// SPDX-License-Identifier: Apache-2.0
//! Protocol-conformance and cross-layer invariant harness for `native-market-seam`.
//!
//! Exercises consistency properties across `noid_tx`, `noid_chain`, `noid_mempool`,
//! `noid_miner`, `noid_rpc`, `noid_block`, `noid_recursive`, and `noid_pruv` against
//! the pinned `parano1d` workspace (`../pinned/`).

mod helpers;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use noid_chain::consensus::fees::fee_breakdown;
use noid_chain::mempool::trade_policy_resources;
use noid_chain::state::native_assets::native_chain_id;
use noid_chain::storage::MdbxChainContext;
use noid_mempool::AsyncMempool;
use noid_miner::template::{project_trade, TemplateBuilder, TemplateChainSnapshot};
use noid_poseidon2b::primitives::Address;
use noid_pruv::native::NativeTemplate;
use noid_pruv::verify::{self, Verdict};
use noid_tx::experimental_object::{integer_program as vm, policy as obj_policy, ObjectOpening};
use noid_tx::native_assets::{
    financial::{
        public::{FinancialPublicContext, FinancialRegistration},
        FinancialOperation, NativeParentRef,
    },
    launch::{
        launch_supply_from_whole,
        queue::{LaunchRegisteredBatch, LaunchRequestRef},
        sequential::SequentialLaunch,
        LaunchDefinition, LaunchOpening, LaunchRequest, LaunchSide, LaunchState, LAUNCH_PROFILE,
    },
    market::{PoolPhase, MARKET_QUANTITY_MAX, MARKET_REQUEST_FEE_CREDIT},
    nft_market::NftRoundContext,
    profile::{kind_enabled, operation_enabled, sequential_kind_enabled},
    trade::{
        sequencing::{MarketStart, SequencingPlan, TradeBoundary},
        TradeIntent, TradePolicy,
    },
    transition::{NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim},
    wire::NativeIntent,
    AssetId, FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeKind, NativeOpening,
    TokenReserve, TOKEN_DECIMALS,
};
use noid_tx::{TxInput, TxOutput};
use serde_json::json;

use helpers::*;

/// Seam 1: Check profile/consensus/mempool/template/R1CS operation gates and
/// verify `PoolPhase::Frozen` handling across `native_trade_parent_at` and `TemplateBuilder`.
async fn check_seam1_round_phase_and_profile_gates() {
    assert!(operation_enabled(33) && operation_enabled(38) && operation_enabled(49) && operation_enabled(50));
    assert!(!operation_enabled(70) && !kind_enabled(NativeKind::NftPurchase));
    assert!(kind_enabled(NativeKind::LaunchRequest) && !sequential_kind_enabled(NativeKind::LaunchRequest));

    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = build_real_history_protocol_runtime(&dir.path().join("origins"));
    let mut ctx = MdbxChainContext::open_or_create(&dir.path().join("chain")).expect("open mdbx");
    let (owner_sec, owner) = keypair(1);

    for _ in 0..24 {
        mine_and_commit_block(&mut ctx, owner, Vec::new(), &[], &[]);
    }
    assert_eq!(ctx.tip_height(), 24);

    // Locate coinbase UTXOs belonging to `owner`.
    let page = ctx.store.get_verified_utxos_by_owner(&owner.0).unwrap();
    let funding_utxo = page.utxos[0];
    let funding_claim = NativeInputClaim {
        input: TxInput {
            slot_index: funding_utxo.slot_index,
            amount: funding_utxo.amount,
            creation_id: funding_utxo.creation_id,
        },
        root: owner,
        opening: None,
    };

    // Create a Launchpad curve via CreateLaunch (op 48).
    let parent = *ctx.tip_header();
    let child_h = parent.height + 1;
    let epoch = current_user_epoch_anchor(&ctx);
    let supply = launch_supply_from_whole(1_000).unwrap();
    let deposit = noid_tx::asset_backing::TokenDenomination::new(TOKEN_DECIMALS)
        .unwrap()
        .minimum_backing(supply)
        .unwrap();
    let fee_48 = fee_breakdown(1, 4, parent.active_slot_count, parent.log_slots).required_total;
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner,
        input: funding_claim.input,
    };
    let tok = FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, [7u8; 32]).unwrap();
    let ldef = LaunchDefinition::from_genesis(&anchor, tok).unwrap();
    let lstate = LaunchState::initial(ldef).unwrap();
    let reserve = TokenReserve::from_definition(&tok, deposit).unwrap();
    let round = NftRoundContext::new(child_h, block_id(&parent), parent.alloc_counter).unwrap();
    let def_open = NativeOpening::Launch(LaunchOpening::Definition(ldef));

    let out_tok = NativeOpening::FungibleDefinition(tok);
    let out_res = NativeOpening::TokenReserve(reserve);
    let out_lst = NativeOpening::Launch(LaunchOpening::State(lstate));
    let create_outputs = vec![
        NativeOutputClaim {
            output: TxOutput { slot_index: 10_000, amount: 0, owner: out_tok.root() },
            opening: Some(out_tok),
        },
        NativeOutputClaim {
            output: TxOutput { slot_index: 10_001, amount: deposit, owner: out_res.root() },
            opening: Some(out_res),
        },
        NativeOutputClaim {
            output: TxOutput { slot_index: 10_002, amount: 0, owner: out_lst.root() },
            opening: Some(out_lst),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 10_003,
                amount: funding_claim.input.amount - deposit - fee_48,
                owner,
            },
            opening: None,
        },
    ];
    let create_intent = NativeIntent::from_financial_claims(
        &NativeContext {
            chain: native_chain_id(),
            height: child_h,
            authority: owner,
            fee: fee_48,
        },
        epoch,
        FinancialOperation::CreateLaunch {
            funding_input: 0,
            definition: def_open.root(),
        },
        round,
        vec![funding_claim],
        create_outputs,
        |r| (r == def_open.root()).then_some(def_open),
    )
    .unwrap();
    let create_txs: Vec<_> = create_intent
        .pages
        .iter()
        .map(|p| noid_tx::Transaction::new(p.body.clone()))
        .collect();
    let mut create_openings = vec![def_open, out_tok, out_res, out_lst];
    create_openings.extend(create_intent.dependencies.clone());
    mine_and_commit_block(&mut ctx, owner, create_txs, &create_openings, &[]);

    // Confirm curve starts Idle and SequentialLaunch::new accepts it.
    let parent_h25 = *ctx.tip_header();
    let tp_idle = ctx
        .store
        .native_trade_parent_at(tok.asset, parent_h25.height, block_id(&parent_h25))
        .unwrap()
        .expect("live market parent");
    assert!(tp_idle.claim().is_ok());

    // Build LaunchRequest (op 49) + RegisterLaunches (op 50) for height 26.
    let h26 = parent_h25.height + 1;
    let epoch26 = current_user_epoch_anchor(&ctx);
    let round26 = NftRoundContext::new(h26, block_id(&parent_h25), parent_h25.alloc_counter).unwrap();
    let utxo2 = ctx.store.get_verified_utxos_by_owner(&owner.0).unwrap().utxos[1];
    let in2 = NativeInputClaim {
        input: TxInput {
            slot_index: utxo2.slot_index,
            amount: utxo2.amount,
            creation_id: utxo2.creation_id,
        },
        root: owner,
        opening: None,
    };
    let req = LaunchRequest {
        round: lstate.round_id(h26).unwrap(),
        asset: tok.asset,
        owner,
        recipient: owner,
        quantity: 1_000,
        noid_limit: 50_000,
        registered_height: h26,
        side: LaunchSide::Buy,
        profile: LAUNCH_PROFILE,
    };
    let req_open = NativeOpening::Launch(LaunchOpening::Request(req));
    let fee_49 = fee_breakdown(1, 2, parent_h25.active_slot_count, parent_h25.log_slots).required_total;
    let parent_ref = NativeParentRef {
        slot_index: tp_idle.slot_index,
        creation_id: tp_idle.creation_id,
        root: out_lst.root(),
    };
    let lr_intent = NativeIntent::from_financial_claims(
        &NativeContext {
            chain: native_chain_id(),
            height: h26,
            authority: owner,
            fee: fee_49,
        },
        epoch26,
        FinancialOperation::LaunchRequest { parent: parent_ref },
        round26,
        vec![in2],
        vec![
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 10_100,
                    amount: req.noid_value().unwrap(),
                    owner: req_open.root(),
                },
                opening: Some(req_open),
            },
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 10_101,
                    amount: in2.input.amount - req.noid_value().unwrap() - fee_49,
                    owner,
                },
                opening: None,
            },
        ],
        |r| {
            if r == def_open.root() {
                Some(def_open)
            } else if r == out_lst.root() {
                Some(out_lst)
            } else {
                None
            }
        },
    )
    .unwrap();

    // Check mempool gate rejects LaunchRequest (op 49).
    let mp = AsyncMempool::new(chain_view_from_context(&ctx), mempool_config_from_runtime(&runtime));
    let mut lr_signed = lr_intent.clone();
    lr_signed.authorization_bytes = vec![1; 32];
    let lr_bytes = lr_signed.to_bytes().unwrap();
    let mp_res = mp.submit_encoded(lr_bytes).await;
    assert!(mp_res.is_err(), "AsyncMempool rejects op 49 outside Owner profile");

    // Compute creation_id assigned to output[0] of LaunchRequest after coinbase (1 system output).
    let sys_outputs = 1u64;
    let req_creation_id =
        noid_tx::native_assets::native_creation_id(parent_h25.alloc_counter + sys_outputs + 1)
            .unwrap();
    let req_ref = LaunchRequestRef {
        slot_index: 10_100,
        creation_id: req_creation_id,
        request: req,
    };
    let reg_batch = LaunchRegisteredBatch::register(ldef, lstate, h26, &[req_ref]).unwrap();
    assert_eq!(reg_batch.after().phase, PoolPhase::Frozen);
    let frozen_open = NativeOpening::Launch(LaunchOpening::State(reg_batch.after()));
    let registrations = [FinancialRegistration::Launch {
        parent: parent_ref,
        request: req_ref,
    }];
    let reg_intent = NativeIntent::from_public_financial_claims(
        &NativeContext {
            chain: native_chain_id(),
            height: h26,
            authority: Address([0; 32]),
            fee: 0,
        },
        epoch26,
        FinancialOperation::RegisterLaunches,
        &FinancialPublicContext {
            round: round26,
            required_fee: fee_breakdown(1, 1, parent_h25.active_slot_count, parent_h25.log_slots)
                .required_total,
            listing_round: None,
            registrations: &registrations,
        },
        vec![tp_idle.claim().unwrap()],
        vec![NativeOutputClaim {
            output: TxOutput {
                slot_index: 10_102,
                amount: frozen_open.amount_micronoid(),
                owner: frozen_open.root(),
            },
            opening: Some(frozen_open),
        }],
        |r| (r == def_open.root()).then_some(def_open),
    )
    .unwrap();

    // Sequential consensus (`validate_block_consensus` + `MdbxChainContext`) accepts 49 + 50.
    let mut user_txs = Vec::new();
    user_txs.extend(lr_intent.pages.iter().map(|p| noid_tx::Transaction::new(p.body.clone())));
    user_txs.extend(reg_intent.pages.iter().map(|p| noid_tx::Transaction::new(p.body.clone())));
    seal_and_commit_custom_native_block(
        &mut ctx,
        owner,
        user_txs,
        vec![def_open, out_lst, req_open, frozen_open],
        Vec::new(),
    )
    .expect("sequential consensus accepts LaunchRequest + RegisterLaunches");

    // Advance 6 more blocks past timeout_height; phase remains Frozen without ResetLaunch.
    for _ in 0..6 {
        mine_and_commit_block(&mut ctx, owner, Vec::new(), &[], &[]);
    }
    let tip = *ctx.tip_header();
    let tp_frozen = ctx
        .store
        .native_trade_parent_at(tok.asset, tip.height, block_id(&tip))
        .unwrap()
        .expect("native_trade_parent_at still returns controller in Frozen phase");
    if let MarketStart::Curve { definition, state } = tp_frozen.market {
        assert_eq!(state.phase, PoolPhase::Frozen);
        assert!(SequentialLaunch::new(definition, state).is_err());
    } else {
        panic!("expected Curve state");
    }

    // Admit a valid TradeOwnerIntent (with real V4 ZK proof) and observe TemplateBuilder failure.
    let mp_after = AsyncMempool::new(chain_view_from_context(&ctx), mempool_config_from_runtime(&runtime));
    let utxo3 = ctx.store.get_verified_utxos_by_owner(&owner.0).unwrap().utxos[2];
    let epoch_after = current_user_epoch_anchor(&ctx);
    let trade = TradeIntent {
        policy: TradePolicy {
            chain: native_chain_id(),
            epoch_anchor: epoch_after,
            owner,
            asset: tok.asset,
            recipient: owner,
            side: LaunchSide::Buy,
            gross_input: 10_000,
            minimum_output: 1,
            fee_limit: 20_000,
            valid_from: tip.height + 1,
            valid_until: tip.height + 10,
        },
        funding: vec![NativeInputClaim {
            input: TxInput {
                slot_index: utxo3.slot_index,
                amount: utxo3.amount,
                creation_id: utxo3.creation_id,
            },
            root: owner,
            opening: None,
        }],
    };
    let (_, trade_bytes) = sign_trade_intent(trade.clone(), Some(owner_sec), epoch_after, tip.height + 1);
    mp_after.submit_encoded(trade_bytes).await.expect("AsyncMempool admits trade on Frozen asset");

    let snap = TemplateChainSnapshot::from_context(&mut ctx).unwrap();
    let builder = TemplateBuilder::new(mp_after.clone()).with_history_protocol(&runtime);
    let built = builder
        .build_from_snapshot(snap, owner, next_block_timestamp(&ctx))
        .await;
    assert!(
        built.is_none(),
        "TemplateBuilder returns None across all classes when a pending trade references a Frozen market"
    );
    println!("Seam 1 checks passed.");
}

/// Seam 2: Check dynamic fee-floor interaction with fixed-fee financial native operations
/// and non-diluting `TradeOwnerIntent` admissions.
async fn check_seam2_fee_floor_and_financial_native_constraints() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = build_real_history_protocol_runtime(&dir.path().join("origins"));
    let mut ctx = MdbxChainContext::open_or_create(&dir.path().join("chain")).expect("open mdbx");
    let (owner_sec, owner) = keypair(2);

    // Produce enough coinbase UTXOs for mempool occupancy tests.
    for _ in 0..30 {
        mine_and_commit_block(&mut ctx, owner, Vec::new(), &[], &[]);
    }
    let tip = *ctx.tip_header();
    let epoch = current_user_epoch_anchor(&ctx);
    let utxos = ctx.store.get_verified_utxos_by_owner(&owner.0).unwrap().utxos;

    // 1. Verify on unhooked AsyncMempool that a real V4-proved TradeOwnerIntent for an
    // unindexed asset with `valid_until = u64::MAX` passes full admission and leaves floor at 5,000.
    let unhooked_mp = AsyncMempool::new(
        chain_view_from_context(&ctx),
        mempool_config_from_runtime(&runtime).with_capacity(20),
    );
    let unindexed_trade = TradeIntent {
        policy: TradePolicy {
            chain: native_chain_id(),
            epoch_anchor: epoch,
            owner,
            asset: AssetId([0xAA; 32]),
            recipient: owner,
            side: LaunchSide::Buy,
            gross_input: 1_000,
            minimum_output: MARKET_QUANTITY_MAX,
            fee_limit: 25_000,
            valid_from: tip.height + 1,
            valid_until: u64::MAX,
        },
        funding: vec![NativeInputClaim {
            input: TxInput {
                slot_index: utxos[0].slot_index,
                amount: utxos[0].amount,
                creation_id: utxos[0].creation_id,
            },
            root: owner,
            opening: None,
        }],
    };
    let (_, real_trade_bytes) =
        sign_trade_intent(unindexed_trade, Some(owner_sec), epoch, tip.height + 1);
    unhooked_mp
        .submit_encoded(real_trade_bytes)
        .await
        .expect("real V4 ZK proof passes unhooked AsyncMempool for unindexed-asset trade");

    // 2. Verify unsatisfiability on a capacity-20 pool (80% = 16, 50% = 10):
    // 4 ordinary txs at 22,223 uNOID + 12 unindexed TradeOwnerIntents => 16 entries (80%),
    // floor = 20,000 uNOID; after confirming the 4 ordinary txs, 12 >= 10 keeps floor = 20,000.
    let mp = AsyncMempool::new(
        chain_view_from_context(&ctx),
        mempool_config_from_runtime(&runtime).with_capacity(20),
    )
    .with_authorization_verification_executor(Arc::new(|_task| Ok(())));

    let mut confirmed_ids = Vec::new();
    for i in 0..4usize {
        let u = utxos[i];
        let (intent, bytes) = build_ordinary_self_transfer(
            None,
            owner,
            TxInput {
                slot_index: u.slot_index,
                amount: u.amount,
                creation_id: u.creation_id,
            },
            30_000 + i as u32,
            22_223,
            epoch,
            tip.height + 1,
        );
        let txid = mp.submit_encoded(bytes).await.unwrap();
        assert_eq!(txid, noid_tx::validate_paged_spend(&intent.pages).unwrap().logical_txid);
        confirmed_ids.push(txid);
    }
    for i in 4..16usize {
        let u = utxos[i];
        let tr = TradeIntent {
            policy: TradePolicy {
                chain: native_chain_id(),
                epoch_anchor: epoch,
                owner,
                asset: AssetId([0xBB; 32]),
                recipient: owner,
                side: LaunchSide::Buy,
                gross_input: 1_000,
                minimum_output: MARKET_QUANTITY_MAX,
                fee_limit: 25_000,
                valid_from: tip.height + 1,
                valid_until: u64::MAX,
            },
            funding: vec![NativeInputClaim {
                input: TxInput {
                    slot_index: u.slot_index,
                    amount: u.amount,
                    creation_id: u.creation_id,
                },
                root: owner,
                opening: None,
            }],
        };
        let (_, bytes) = sign_trade_intent(tr, None, epoch, tip.height + 1);
        mp.submit_encoded(bytes).await.unwrap();
    }
    assert_eq!(mp.fee_floor().await, 20_000);

    // Simulate confirming the 4 ordinary transactions; 12 unfillable trades remain (>= 50% of 20).
    mp.on_new_block(&confirmed_ids, tip.height, chain_view_from_context(&ctx))
        .await;
    assert_eq!(mp.fee_floor().await, 20_000);

    // Verify CreateLaunch (op 48) fails BOTH at fee == minimum (BelowMinFee) AND at fee == 20,000 (MalformedIntent).
    let u_fin = utxos[20];
    let supply = launch_supply_from_whole(1_000).unwrap();
    let deposit = 1_000_000u64;
    let min_fee = fee_breakdown(1, 4, tip.active_slot_count, tip.log_slots).required_total;
    assert!(min_fee < 20_000);
    for candidate_fee in [min_fee, 20_000u64] {
        let in_claim = NativeInputClaim {
            input: TxInput {
                slot_index: u_fin.slot_index,
                amount: u_fin.amount,
                creation_id: u_fin.creation_id,
            },
            root: owner,
            opening: None,
        };
        let anchor = GenesisAnchor {
            chain: native_chain_id(),
            owner,
            input: in_claim.input,
        };
        let tok = FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, [9u8; 32]).unwrap();
        let ldef = LaunchDefinition::from_genesis(&anchor, tok).unwrap();
        let lstate = LaunchState::initial(ldef).unwrap();
        let reserve = TokenReserve::from_definition(&tok, deposit).unwrap();
        let def_open = NativeOpening::Launch(LaunchOpening::Definition(ldef));
        let round = NftRoundContext::new(tip.height + 1, block_id(&tip), tip.alloc_counter).unwrap();
        let mut ni = NativeIntent::from_financial_claims(
            &NativeContext {
                chain: native_chain_id(),
                height: tip.height + 1,
                authority: owner,
                fee: candidate_fee,
            },
            epoch,
            FinancialOperation::CreateLaunch {
                funding_input: 0,
                definition: def_open.root(),
            },
            round,
            vec![in_claim],
            vec![
                NativeOutputClaim {
                    output: TxOutput {
                        slot_index: 31_000,
                        amount: 0,
                        owner: NativeOpening::FungibleDefinition(tok).root(),
                    },
                    opening: Some(NativeOpening::FungibleDefinition(tok)),
                },
                NativeOutputClaim {
                    output: TxOutput {
                        slot_index: 31_001,
                        amount: deposit,
                        owner: NativeOpening::TokenReserve(reserve).root(),
                    },
                    opening: Some(NativeOpening::TokenReserve(reserve)),
                },
                NativeOutputClaim {
                    output: TxOutput {
                        slot_index: 31_002,
                        amount: 0,
                        owner: NativeOpening::Launch(LaunchOpening::State(lstate)).root(),
                    },
                    opening: Some(NativeOpening::Launch(LaunchOpening::State(lstate))),
                },
                NativeOutputClaim {
                    output: TxOutput {
                        slot_index: 31_003,
                        amount: u_fin.amount - deposit - candidate_fee,
                        owner,
                    },
                    opening: None,
                },
            ],
            |r| (r == def_open.root()).then_some(def_open),
        )
        .unwrap();
        ni.authorization_bytes = vec![0xCC; 32];
        let res = mp.submit_encoded(ni.to_bytes().unwrap()).await;
        assert!(
            res.is_err(),
            "CreateLaunch rejected at candidate_fee={candidate_fee} while floor=20_000"
        );
    }
    println!("Seam 2 checks passed.");
}

/// Seam 3: Check `ExecuteTrades` global `funding = 64` budget consumption and
/// non-monotonic `SequencingPlan` side-preference interaction.
fn check_seam3_trade_sequencing_and_budgets() {
    let (_, owner) = keypair(3);
    let funding_64: Vec<NativeInputClaim> = (0..64u32)
        .map(|i| NativeInputClaim {
            input: TxInput {
                slot_index: 1_000 + i,
                amount: if i == 0 { 100_000 } else { 1 },
                creation_id: 1 + u64::from(i),
            },
            root: owner,
            opening: None,
        })
        .collect();
    let single_64_input_trade = TradeIntent {
        policy: TradePolicy {
            chain: [1; 32],
            epoch_anchor: [2; 32],
            owner,
            asset: AssetId([3; 32]),
            recipient: owner,
            side: LaunchSide::Buy,
            gross_input: 10_000,
            minimum_output: 1,
            fee_limit: 20_000,
            valid_from: 25,
            valid_until: 50,
        },
        funding: funding_64,
    };
    let (pages, usage) = trade_policy_resources(&single_64_input_trade).unwrap();
    assert_eq!(pages, 10);
    assert_eq!(usage.policies, 1);
    assert_eq!(usage.funding, 64);
    assert_eq!(usage.actions, 69);

    // Check side-preference graduation displacement on a Launchpad curve at `sold = sale_units - 1`.
    let anchor = GenesisAnchor {
        chain: [1; 32],
        owner,
        input: TxInput {
            slot_index: 1,
            amount: 10_000_000_000,
            creation_id: 1,
        },
    };
    let supply = launch_supply_from_whole(1_000).unwrap();
    let tok = FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, [4; 32]).unwrap();
    let ldef = LaunchDefinition::from_genesis(&anchor, tok).unwrap();
    let near_grad = match ldef.after_initial_purchase(ldef.sale_units() - 1).unwrap() {
        noid_tx::native_assets::launch::sequential::TradeState::Curve(s) => s,
        _ => unreachable!(),
    };
    let boundary = TradeBoundary {
        chain: [1; 32],
        epoch_anchor: [2; 32],
        parent_id: [5; 32],
        height: 30,
    };

    // Seller A alone receives the 0%-fee curve proceeds.
    let sell_units = 100_000u64;
    let sell_tok = NativeOpening::TokenBalance(noid_tx::native_assets::TokenBalance {
        asset: tok.asset,
        owner,
        units: sell_units,
        unlock_height: 0,
    });
    let mut seller_a = TradeIntent {
        policy: TradePolicy {
            chain: [1; 32],
            epoch_anchor: [2; 32],
            owner,
            asset: tok.asset,
            recipient: owner,
            side: LaunchSide::Sell,
            gross_input: sell_units,
            minimum_output: 1,
            fee_limit: 20_000,
            valid_from: 25,
            valid_until: 50,
        },
        funding: vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: 10,
                    amount: 0,
                    creation_id: noid_tx::native_assets::native_creation_id(10).unwrap(),
                },
                root: sell_tok.root(),
                opening: Some(sell_tok),
            },
            NativeInputClaim {
                input: TxInput {
                    slot_index: 11,
                    amount: 50_000,
                    creation_id: 11,
                },
                root: owner,
                opening: None,
            },
        ],
    };
    let solo = SequencingPlan::execute(
        MarketStart::Curve {
            definition: ldef,
            state: near_grad,
        },
        boundary,
        std::slice::from_ref(&seller_a),
    )
    .unwrap();
    let curve_out = solo.fills()[0].outcome.output;
    seller_a.policy.minimum_output = curve_out; // Tight exact curve quote (or < 30 bps tolerance).

    // Add 0-NOID graduation Buy B (gross_input = 1 buys the final curve unit for 0 NOID).
    let buyer_b = TradeIntent {
        policy: TradePolicy {
            chain: [1; 32],
            epoch_anchor: [2; 32],
            owner,
            asset: tok.asset,
            recipient: owner,
            side: LaunchSide::Buy,
            gross_input: 1,
            minimum_output: 1,
            fee_limit: 20_000,
            valid_from: 25,
            valid_until: 50,
        },
        funding: vec![NativeInputClaim {
            input: TxInput {
                slot_index: 99,
                amount: 50_000,
                creation_id: 99,
            },
            root: owner,
            opening: None,
        }],
    };
    let combined = SequencingPlan::execute(
        MarketStart::Curve {
            definition: ldef,
            state: near_grad,
        },
        boundary,
        &[seller_a, buyer_b],
    )
    .unwrap();
    // Buy B executes at step 0 (`preferred = Buy` at `current_price == initial_price`),
    // spent_input == 0, graduates the curve into the 0.30%-fee AMM pool, and Seller A is declined!
    assert_eq!(combined.fills().len(), 1);
    assert_eq!(combined.fills()[0].source_index, 1);
    assert_eq!(combined.fills()[0].outcome.spent_input, 0);
    assert_eq!(combined.unfilled(), &[0]);
    println!("Seam 3 checks passed.");
}

/// Seam 4: Check `noid_pruv` SMT verification model against `ObjectOpening::check_call`
/// and `NativeTemplate` bounds against `noid_rpc` planner rules.
fn check_seam4_pruv_smt_and_template_consistency() {
    let z3 = Path::new("/opt/homebrew/bin/z3");
    let (_, auth) = keypair(10);
    let (_, rec) = keypair(11);

    // 1. Contract with all four mode permissions disabled (`modes == 0`):
    // `ObjectOpening::check_call` rejects every call, yet `pruv::verify` reports `Verified`.
    let disabled_modes_src = r#"
contract UncallableModes {
    param auth: address;
    param rec: address;
    state counter: u64 = 0;
    policy {
        claim_authority = auth;
        recovery_authority = rec;
        claim_recipient = auth;
        recovery_recipient = rec;
        deadline_height = 100;
        max_fee_micronoid = 50000;
        max_payout_micronoid = 0;
        min_retained_micronoid = 1000;
        claim_can_continue = false;
        claim_can_close = false;
        recovery_can_continue = false;
        recovery_can_close = false;
        unrestricted_payout_recipient = false;
    }
    call {
        counter += 1;
    }
    ensures counter == old(counter) + 1;
}
"#;
    let params = json!({
        "auth": auth.to_bech32(),
        "rec": rec.to_bech32(),
    });
    let compiled = noid_pruv::compile(disabled_modes_src, &params).unwrap();
    assert_eq!(compiled.opening().rules.modes, 0);
    for (before_dl, term) in [(true, false), (true, true), (false, false), (false, true)] {
        assert!(!compiled.opening().rules.permits(before_dl, term));
    }
    if z3.exists() {
        let verdict = verify::verify(&compiled, z3).unwrap();
        assert_eq!(
            verdict,
            Verdict::Verified,
            "SMT model omits ObjectRules::permits and certifies an uncallable contract"
        );
    }

    // 2. Valid contract asserting `ensures fee <= 50000;` (guaranteed by `max_fee_micronoid = 50000`):
    // `pruv::verify` reports `Counterexample` because `fee` is unconstrained in SMT.
    let policy_inv_src = disabled_modes_src
        .replace("claim_can_continue = false;", "claim_can_continue = true;")
        .replace(
            "ensures counter == old(counter) + 1;",
            "ensures fee <= 50000;",
        );
    let compiled_inv = noid_pruv::compile(&policy_inv_src, &params).unwrap();
    if z3.exists() {
        let verdict = verify::verify(&compiled_inv, z3).unwrap();
        assert_eq!(
            verdict,
            Verdict::Counterexample,
            "SMT model omits max_fee_micronoid and reports a spurious counterexample"
        );
    }

    // 3. Zero-address policy acceptance in `noid_pruv::compile` and `ObjectOpening::validate`.
    let zero_addr = Address([0; 32]).to_bech32();
    let zero_params = json!({ "auth": zero_addr, "rec": zero_addr });
    let compiled_zero = noid_pruv::compile(&policy_inv_src, &zero_params).unwrap();
    assert!(compiled_zero.opening().validate().is_ok());

    // 4. `NativeTemplate::Trade` accepts `valid_until = u64::MAX`, exceeding planner's +30 block window.
    let tmpl_json = json!({
        "action": "trade",
        "asset_id": "11".repeat(32),
        "buy": true,
        "gross_input": "10000",
        "minimum_output": "1",
        "valid_until": u64::MAX
    })
    .to_string();
    let parsed = NativeTemplate::parse(&tmpl_json).expect("NativeTemplate accepts u64::MAX expiry");
    assert_eq!(parsed.operation_code(), 80);
    println!("Seam 4 checks passed.");
}

#[tokio::main]
async fn main() {
    let _ = &project_trade;
    let _ = obj_policy::CLAIM_CONTINUE;
    let _ = vm::PROGRAM_STEPS;
    let _ = ObjectOpening::from_bytes;
    let _ = HashSet::<u32>::new();

    check_seam1_round_phase_and_profile_gates().await;
    check_seam2_fee_floor_and_financial_native_constraints().await;
    check_seam3_trade_sequencing_and_budgets();
    check_seam4_pruv_smt_and_template_consistency();
    println!("ALL NATIVE-MARKET-SEAM CHECKS COMPLETED");
}
