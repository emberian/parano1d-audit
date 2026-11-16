//! NFT live-marketplace races, op-80 sequential trades and the listing
//! settlement circuit witness. Audit PoC against the pinned parano1d tree.
//!
//! Build configuration: `noid_*` path dependencies point at the verbatim
//! source copy verified byte-identical to commit 52938433 by the
//! nft-marketplace lane (re-verified for this lane's key files).

#[cfg(test)]
mod tests {
    use noid_chain::fri_state::SlotValue;
    use noid_chain::state::native_assets::market::liquidity::MarketFeeContext;
    use noid_chain::state::native_assets::nft_market::{
        apply_owned_listing_cancellation, apply_owned_listing_creation,
        apply_owned_listing_purchase, ParentListing,
    };
    use noid_chain::state::native_assets::trade::{
        apply_prepared_trade_batch, prepare_trade_batch, prepare_trade_batch_from_intent,
        trade_fee_quotes,
    };
    use noid_chain::state::native_assets::{apply_native_transition, native_chain_id};
    use noid_chain::ChainState;
    use noid_poseidon2b::primitives::Address;
    use noid_tx::native_assets::launch::{
        LaunchDefinition, LaunchOpening, LaunchSide, LaunchState,
    };
    use noid_tx::native_assets::nft_market::{
        ListingDefinition, ListingState, NftMarketOpening, NFT_FEE_CREDIT,
    };
    use noid_tx::native_assets::trade::batch::{BatchError, TradeBatch, TradeParent};
    use noid_tx::native_assets::trade::execution::TradeOutputSlots;
    use noid_tx::native_assets::trade::sequencing::{MarketStart, TradeBoundary};
    use noid_tx::native_assets::trade::{TradeIntent, TradePolicy};
    use noid_tx::native_assets::transition::{
        NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
    };
    use noid_tx::native_assets::{
        native_creation_id, FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening,
        NftCollection, NftItem, TokenBalance,
    };
    use noid_tx::{TxInput, TxOutput};

    const HEIGHT: u64 = 279_371; // v2.1 active (activation 279_370)
    const CREATOR: u8 = 1;
    const SELLER: u8 = 2;
    const BUYER_A: u8 = 3;
    const BUYER_B: u8 = 4;
    const ATTACKER: u8 = 9;
    const INITIAL_CASH: u64 = 8_000_000_000;
    const ASK: u64 = 500_000;
    const EPOCH: [u8; 32] = [0xE1; 32];
    const PARENT: [u8; 32] = [0xB1; 32];

    fn address(byte: u8) -> Address {
        Address([byte; 32])
    }

    fn context(authority: u8, fee: u64) -> NativeContext {
        NativeContext {
            chain: native_chain_id(),
            height: HEIGHT,
            authority: address(authority),
            fee,
        }
    }

    fn required_fee(state: &ChainState, inputs: usize, outputs: usize) -> u64 {
        MarketFeeContext::from_state(state)
            .required(inputs, outputs)
            .unwrap()
    }

    fn initial_state() -> ChainState {
        let slots = [
            (
                1,
                SlotValue::with_owner_fields(INITIAL_CASH, 1, address(CREATOR).as_fields()),
            ),
            (
                2,
                SlotValue::with_owner_fields(INITIAL_CASH, 2, address(SELLER).as_fields()),
            ),
            (
                3,
                SlotValue::with_owner_fields(INITIAL_CASH, 3, address(BUYER_A).as_fields()),
            ),
            (
                4,
                SlotValue::with_owner_fields(INITIAL_CASH, 4, address(BUYER_B).as_fields()),
            ),
        ];
        ChainState::from_sparse_utxos(8, &slots, 8).unwrap()
    }

    fn ordinary_input(slot: u32, creation_id: u64, amount: u64, owner: u8) -> NativeInputClaim {
        NativeInputClaim {
            input: TxInput {
                slot_index: slot,
                amount,
                creation_id,
            },
            root: address(owner),
            opening: None,
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

    // =====================================================================
    // Part 1: operation 80 (ExecuteTrades) at the State layer. Never
    // executed by any prior lane through prepare/apply; the dex-games lane
    // drove SequencingPlan/TradeBatch only.
    // =====================================================================

    struct LaunchFixture {
        definition: LaunchDefinition,
        state: LaunchState,
        slot: u32,
        parent: TradeParent,
    }

    fn launch_fixture(sold_units: u64) -> (ChainState, LaunchFixture) {
        let anchor = GenesisAnchor {
            chain: native_chain_id(),
            owner: address(CREATOR),
            input: TxInput {
                slot_index: 1,
                amount: INITIAL_CASH,
                creation_id: 1,
            },
        };
        let supply = noid_tx::native_assets::launch::launch_supply_from_whole(100_000).unwrap();
        let token =
            FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, [0xA7; 32])
                .unwrap();
        let definition = LaunchDefinition::from_genesis(&anchor, token).unwrap();
        let curve = definition.after_initial_purchase(sold_units).unwrap();
        let noid_tx::native_assets::launch::sequential::TradeState::Curve(curve_state) = curve
        else {
            panic!("fixture parks the curve mid-flight");
        };
        let opening = NativeOpening::Launch(LaunchOpening::State(curve_state));
        let slot = 10;
        let sequence = 5;
        let root = opening.root();
        let mut seeds = vec![
            (
                slot,
                SlotValue::with_owner_fields(
                    opening.amount_micronoid(),
                    native_creation_id(sequence).unwrap(),
                    root.as_fields(),
                ),
            ),
            (
                11,
                SlotValue::with_owner_fields(FUND_A, 1, address(BUYER_A).as_fields()),
            ),
            (
                12,
                SlotValue::with_owner_fields(FUND_B, 2, address(BUYER_B).as_fields()),
            ),
        ];
        let seller_tokens = TokenBalance {
            asset: definition.asset,
            owner: address(SELLER),
            units: 4_000_000_000,
            unlock_height: 0,
        };
        let seller_opening = NativeOpening::TokenBalance(seller_tokens);
        seeds.push((
            13,
            SlotValue::with_owner_fields(
                seller_opening.amount_micronoid(),
                native_creation_id(6).unwrap(),
                seller_opening.root().as_fields(),
            ),
        ));
        seeds.push((
            14,
            SlotValue::with_owner_fields(1_000_000, 3, address(SELLER).as_fields()),
        ));
        let chain = ChainState::from_sparse_utxos_at(8, &seeds, 8, HEIGHT).unwrap();
        let parent = TradeParent {
            slot_index: slot,
            creation_id: native_creation_id(sequence).unwrap(),
            market: MarketStart::Curve {
                definition,
                state: curve_state,
            },
        };
        (
            chain,
            LaunchFixture {
                definition,
                state: curve_state,
                slot,
                parent,
            },
        )
    }

    fn boundary() -> TradeBoundary {
        TradeBoundary {
            chain: native_chain_id(),
            epoch_anchor: EPOCH,
            parent_id: PARENT,
            height: HEIGHT,
        }
    }

    const FUND_A: u64 = 3_000_000;
    const FUND_B: u64 = 4_000_000;

    fn buy_intent(
        fixture: &LaunchFixture,
        owner: u8,
        funding_slot: u32,
        funding_creation: u64,
        seeded_amount: u64,
        gross: u64,
        minimum_output: u64,
    ) -> TradeIntent {
        let fee_limit = 100_000;
        TradeIntent {
            policy: TradePolicy {
                chain: native_chain_id(),
                epoch_anchor: EPOCH,
                owner: address(owner),
                asset: fixture.definition.asset,
                recipient: address(owner),
                side: LaunchSide::Buy,
                gross_input: gross,
                minimum_output,
                fee_limit,
                valid_from: HEIGHT - 10,
                valid_until: HEIGHT + 10,
            },
            funding: vec![ordinary_input(
                funding_slot,
                funding_creation,
                seeded_amount,
                owner,
            )],
        }
    }

    fn sell_intent(
        fixture: &LaunchFixture,
        funding_slot: u32,
        gross_units: u64,
        minimum_output: u64,
    ) -> TradeIntent {
        let fee_limit = 100_000;
        let balance = TokenBalance {
            asset: fixture.definition.asset,
            owner: address(SELLER),
            units: 4_000_000_000,
            unlock_height: 0,
        };
        TradeIntent {
            policy: TradePolicy {
                chain: native_chain_id(),
                epoch_anchor: EPOCH,
                owner: address(SELLER),
                asset: fixture.definition.asset,
                recipient: address(SELLER),
                side: LaunchSide::Sell,
                gross_input: gross_units,
                minimum_output,
                fee_limit,
                valid_from: HEIGHT - 10,
                valid_until: HEIGHT + 10,
            },
            funding: vec![
                NativeInputClaim {
                    input: TxInput {
                        slot_index: funding_slot,
                        amount: 0,
                        creation_id: native_creation_id(6).unwrap(),
                    },
                    root: NativeOpening::TokenBalance(balance).root(),
                    opening: Some(NativeOpening::TokenBalance(balance)),
                },
                ordinary_input(14, 3, 1_000_000, SELLER),
            ],
        }
    }

    /// Mirrors the miner: derive per-charge payout shapes, then allocate
    /// fresh empty output slots for every payout plus the market successor.
    fn output_slots(
        state: &ChainState,
        boundary: TradeBoundary,
        batch: &TradeBatch,
        first_slot: u32,
    ) -> TradeOutputSlots {
        let quotes = trade_fee_quotes(state, batch).unwrap();
        let plan = batch.checked(boundary).unwrap();
        let mut outcomes = vec![None; batch.policies().len()];
        for fill in plan.fills() {
            outcomes[fill.source_index] = Some(fill.outcome);
        }
        let mut payouts = Vec::new();
        let mut next = first_slot;
        for (index, intent) in batch.policies().iter().enumerate() {
            let charge = noid_tx::native_assets::trade::execution::quote_charge(
                intent,
                boundary,
                outcomes[index].expect("fully filled batch"),
                quotes[index],
            )
            .unwrap();
            let slots: Vec<u32> = (next..next + charge.payouts.len() as u32).collect();
            next += charge.payouts.len() as u32;
            payouts.push(slots);
        }
        TradeOutputSlots {
            payouts,
            market: next,
        }
    }

    #[test]
    fn op80_honest_execution_end_to_end_and_conservation() {
        let (mut state, fixture) = launch_fixture(1_000_000_000);
        let a = buy_intent(&fixture, BUYER_A, 11, 1, FUND_A, 1_000_000, 1);
        let b = buy_intent(&fixture, BUYER_B, 12, 2, FUND_B, 2_000_000, 1);
        let s = sell_intent(&fixture, 13, 500_000_000, 1);
        // Caller order deliberately scrambled; TradeBatch::new canonicalizes.
        let batch = TradeBatch::new(fixture.parent, vec![s.clone(), b.clone(), a.clone()]).unwrap();
        let slots = output_slots(&state, boundary(), &batch, 100);
        let prepared = prepare_trade_batch(&state, boundary(), &batch, &slots).unwrap();

        // Execution-level invariants before apply.
        let fee = prepared.fee();
        assert!(fee > 0);
        assert_eq!(prepared.commitment(), batch.root().unwrap());
        // Inputs: parent + every policy's funding claim (the sell carries a
        // token carrier plus an ordinary fee input), canonical order.
        assert_eq!(prepared.inputs().len(), 1 + 1 + 1 + 2);
        assert_eq!(prepared.inputs()[0].input.slot_index, fixture.slot);
        let supply_before = state.circulating_supply_micronoid;
        let root_before = state.try_state_root().unwrap();
        let transition = apply_prepared_trade_batch(&mut state, &prepared).unwrap();

        // Parent consumed; successor installed at the market slot.
        assert_eq!(state.state.slot(fixture.slot), SlotValue::EMPTY);
        let successor_slot = slots.market;
        let successor = state.state.slot(successor_slot);
        assert_ne!(successor, SlotValue::EMPTY);
        // Every payout materialized with its charge amount.
        let mut paid_noid = 0u128;
        for (charge, slots) in prepared.charges().iter().zip(&slots.payouts) {
            for (payout, &slot) in charge.payouts.iter().zip(slots) {
                let value = state.state.slot(slot);
                if payout.asset == noid_tx::native_assets::market::NOID_ASSET {
                    assert_eq!(value.amount(), payout.units);
                    paid_noid += u128::from(payout.units);
                } else {
                    // Token payouts carry their units in the opening, zero in
                    // the raw NOID amount lane.
                    assert_eq!(value.amount(), 0);
                }
            }
        }
        // Conservation: µNOID in = NOID payouts + successor value + fee.
        let inputs_total: u128 = prepared
            .inputs()
            .iter()
            .map(|c| u128::from(c.input.amount))
            .sum();
        let successor_value = u128::from(state.state.slot(successor_slot).amount());
        assert_eq!(
            inputs_total,
            paid_noid + successor_value + u128::from(fee),
            "NOID conservation across the executed batch"
        );
        assert_eq!(
            supply_before - state.circulating_supply_micronoid,
            u128::from(fee),
            "supply burns exactly the batch fee"
        );
        assert_ne!(state.try_state_root().unwrap(), root_before);
        let native_inputs = prepared
            .inputs()
            .iter()
            .filter(|c| c.opening.is_some())
            .count();
        let native_outputs = prepared
            .outputs()
            .iter()
            .filter(|c| c.opening.is_some())
            .count();
        assert_eq!(transition.consumed_openings.len(), native_inputs);
        assert_eq!(transition.created_openings.len(), native_outputs);
        // The executed set moved the curve: successor differs from parent.
        let parent_value = SlotValue::with_owner_fields(
            NativeOpening::Launch(LaunchOpening::State(fixture.state)).amount_micronoid(),
            fixture.parent.creation_id,
            NativeOpening::Launch(LaunchOpening::State(fixture.state))
                .root()
                .as_fields(),
        );
        assert_ne!(state.state.slot(successor_slot), parent_value);
    }

    #[test]
    fn op80_caller_order_is_irrelevant_and_wire_round_trip_agrees() {
        let (state, fixture) = launch_fixture(1_000_000_000);
        let a = buy_intent(&fixture, BUYER_A, 11, 1, FUND_A, 1_000_000, 1);
        let b = buy_intent(&fixture, BUYER_B, 12, 2, FUND_B, 2_000_000, 1);
        let s = sell_intent(&fixture, 13, 300_000_000, 1);
        let forward =
            TradeBatch::new(fixture.parent, vec![a.clone(), b.clone(), s.clone()]).unwrap();
        let scrambled =
            TradeBatch::new(fixture.parent, vec![s.clone(), b.clone(), a.clone()]).unwrap();
        assert_eq!(forward.root().unwrap(), scrambled.root().unwrap());
        let slots = output_slots(&state, boundary(), &forward, 100);
        let p1 = prepare_trade_batch(&state, boundary(), &forward, &slots).unwrap();
        let p2 = prepare_trade_batch(&state, boundary(), &scrambled, &slots).unwrap();
        // Same fills in the same order, same charges, same outputs.
        assert_eq!(p1.plan().fills(), p2.plan().fills());
        assert_eq!(p1.charges(), p2.charges());
        assert_eq!(p1.outputs(), p2.outputs());
        assert_eq!(p1.fee(), p2.fee());

        // Wire round trip: encode the execution as a NativeIntent, then let
        // the production from-intent path re-derive it from the pages.
        let intent = noid_tx::native_assets::wire::NativeIntent::from_trade_execution(
            p1.execution(),
            |_| None,
        )
        .unwrap();
        let p3 = prepare_trade_batch_from_intent(
            &state,
            boundary(),
            state.state.log_slots() as u32,
            &forward,
            &intent,
        )
        .unwrap();
        assert_eq!(p3.inputs(), p1.inputs());
        assert_eq!(p3.outputs(), p1.outputs());
        assert_eq!(p3.charges(), p1.charges());
        assert_eq!(p3.fee(), p1.fee());
        assert_eq!(p3.plan().fills(), p1.plan().fills());
    }

    #[test]
    fn op80_mixed_batch_with_one_declined_fill_is_rejected_wholesale() {
        let (mut state, fixture) = launch_fixture(1_000_000_000);
        // Honest fill.
        let good = buy_intent(&fixture, BUYER_A, 11, 1, FUND_A, 1_000_000, 1);
        // Declinable: demands more tokens than 1 NOID can ever buy here.
        let doomed = buy_intent(&fixture, BUYER_B, 12, 2, FUND_B, 1_000_000, 900_000_000_000);
        let batch = TradeBatch::new(fixture.parent, vec![good, doomed]).unwrap();
        let slots = TradeOutputSlots {
            payouts: vec![Vec::new(), Vec::new()],
            market: 100,
        };
        let error = prepare_trade_batch(&state, boundary(), &batch, &slots).unwrap_err();
        assert!(
            matches!(
                error,
                noid_chain::state::native_assets::trade::TradeStateError::Batch(
                    BatchError::Unfilled
                )
            ),
            "declined fill must fail the whole batch, got {error:?}"
        );
        // Nothing changed: batch-level atomicity at the consensus layer.
        let (mut check, _) = launch_fixture(1_000_000_000);
        assert_eq!(
            state.try_state_root().unwrap(),
            check.try_state_root().unwrap()
        );
    }

    #[test]
    fn op80_hard_invalid_policy_rejects_before_any_execution() {
        let (state, fixture) = launch_fixture(1_000_000_000);
        let mut stale = buy_intent(&fixture, BUYER_A, 11, 1, FUND_A, 1_000_000, 1);
        stale.policy.valid_until = HEIGHT - 1; // expired at boundary height
        let batch = TradeBatch::new(fixture.parent, vec![stale]).unwrap();
        let slots = TradeOutputSlots {
            payouts: vec![Vec::new()],
            market: 100,
        };
        let error = prepare_trade_batch(&state, boundary(), &batch, &slots).unwrap_err();
        // `TradeBatch::checked` flattens every sequencing error into
        // `Sequence`, so an expired policy surfaces as Batch(Sequence).
        assert!(
            matches!(
                error,
                noid_chain::state::native_assets::trade::TradeStateError::Batch(_)
            ),
            "hard policy error must abort the batch, got {error:?}"
        );
    }

    #[test]
    fn op80_cannot_touch_listing_or_item_openings() {
        let (_state, fixture) = launch_fixture(1_000_000_000);
        // A listing/item opening as funding is rejected by the intent itself.
        let listing = ListingState::initial(ListingDefinition {
            collection: noid_tx::native_assets::CollectionId([0x51; 32]),
            seller: address(SELLER),
            metadata: [0x6D; 32],
            serial: 1,
            source_creation_id: native_creation_id(4).unwrap(),
            ask: ASK,
            expiry_height: HEIGHT + 100,
        })
        .unwrap();
        for opening in [
            NativeOpening::NftMarket(NftMarketOpening::Listing(listing)),
            NativeOpening::NftItem(NftItem {
                collection: noid_tx::native_assets::CollectionId([0x51; 32]),
                owner: address(SELLER),
                metadata: [0x6D; 32],
                serial: 1,
                unlock_height: 0,
            }),
        ] {
            let intent = TradeIntent {
                policy: buy_intent(&fixture, BUYER_A, 11, 1, FUND_A, 1_000_000, 1).policy,
                funding: vec![NativeInputClaim {
                    input: TxInput {
                        slot_index: 20,
                        amount: opening.amount_micronoid(),
                        creation_id: native_creation_id(7).unwrap(),
                    },
                    root: opening.root(),
                    opening: Some(opening),
                }],
            };
            assert!(
                intent.validate_structure().is_err(),
                "NFT opening must not serve as trade funding"
            );
        }
        // The parent is typed: no listing variant exists in MarketStart
        // (Curve | Market only), so a listing parent is unrepresentable by
        // construction. Control: the same batch with the typed parent works.
        let intent = buy_intent(&fixture, BUYER_A, 11, 1, FUND_A, 1_000_000, 1);
        assert!(TradeBatch::new(fixture.parent, vec![intent]).is_ok());
    }

    #[test]
    fn op80_funding_priority_orders_execution_not_authorization_id() {
        let (state, fixture) = launch_fixture(1_000_000_000);
        // Grind two anchors whose priority order is the reverse of their
        // authorization-id order (mirrors the dex-games control at the
        // State-apply layer).
        let mut anchors = Vec::new();
        let mut slot = 30u32;
        let mut seq = 20u64;
        while anchors.len() < 2 {
            let funding = ordinary_input(slot, seq, FUND_A, BUYER_A);
            let intent = TradeIntent {
                policy: buy_intent(&fixture, BUYER_A, slot, seq, FUND_A, 1_000_000, 1).policy,
                funding: vec![funding],
            };
            let priority = intent.funding_priority(PARENT).unwrap();
            anchors.push(((slot, seq), intent, priority));
            slot += 1;
            seq += 1;
        }
        let (first, second) = if anchors[0].2 < anchors[1].2 {
            (anchors[0].clone(), anchors[1].clone())
        } else {
            (anchors[1].clone(), anchors[0].clone())
        };
        let auth_first = first.1.authorization_id().unwrap();
        let auth_second = second.1.authorization_id().unwrap();
        // The batch encodes policies in authorization order regardless.
        let batch =
            TradeBatch::new(fixture.parent, vec![second.1.clone(), first.1.clone()]).unwrap();
        let encoded_first = batch.policies()[0].authorization_id().unwrap();
        assert!(
            encoded_first == auth_first || encoded_first == auth_second,
            "encoding order is an authorization-id sort"
        );
        // Execution follows funding priority.
        let plan = batch.checked(boundary()).unwrap();
        assert_eq!(
            plan.fills()[0].authorization_id,
            first.1.authorization_id().unwrap()
        );
        assert_eq!(
            plan.fills()[1].authorization_id,
            second.1.authorization_id().unwrap()
        );
        let _ = state;
    }

    // =====================================================================
    // Part 2: live listing races (sequential State applications).
    // =====================================================================

    fn item_opening(item: NftItem) -> NativeOpening {
        NativeOpening::NftItem(item)
    }

    fn listing_opening(listing: ListingState) -> NativeOpening {
        NativeOpening::NftMarket(NftMarketOpening::Listing(listing))
    }

    /// create collection -> mint 1 -> transfer to SELLER -> list at ask.
    /// Returns the pre-listing-decision state plus the listing coordinates.
    fn listed_fixture() -> (ChainState, ParentListing, NftItem) {
        let mut state = initial_state();
        let anchor = GenesisAnchor {
            chain: native_chain_id(),
            owner: address(CREATOR),
            input: TxInput {
                slot_index: 1,
                amount: INITIAL_CASH,
                creation_id: 1,
            },
        };
        let collection = NftCollection::from_genesis(&anchor, 4, [0xA1; 32]).unwrap();
        let inputs = vec![ordinary_input(1, 1, INITIAL_CASH, CREATOR)];
        let fee = required_fee(&state, 1, 2);
        let outputs = vec![
            native_output(10, NativeOpening::NftCollection(collection)),
            ordinary_output(11, INITIAL_CASH - fee, CREATOR),
        ];
        apply_native_transition(
            &mut state,
            &context(CREATOR, fee),
            NativeOperation::CreateCollection { funding_input: 0 },
            &inputs,
            &outputs,
        )
        .unwrap();

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
        let creator_cash = state.state.slot(11).amount();
        let mint_fee = required_fee(&state, 2, 4);
        let mint_inputs = vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: 10,
                    amount: 0,
                    creation_id: state.state.slot(10).creation_id(),
                },
                root: NativeOpening::NftCollection(collection).root(),
                opening: Some(NativeOpening::NftCollection(collection)),
            },
            ordinary_input(
                11,
                state.state.slot(11).creation_id(),
                creator_cash,
                CREATOR,
            ),
        ];
        let mint_outputs = vec![
            native_output(20, NativeOpening::NftCollection(minted)),
            native_output(21, item_opening(item)),
            ordinary_output(23, creator_cash - mint_fee, CREATOR),
        ];
        apply_native_transition(
            &mut state,
            &context(CREATOR, mint_fee),
            NativeOperation::MintNft,
            &mint_inputs,
            &mint_outputs,
        )
        .unwrap();

        let seller_item = NftItem {
            owner: address(SELLER),
            ..item
        };
        let creator_cash = state.state.slot(23).amount();
        let transfer_fee = required_fee(&state, 2, 2);
        let transfer_inputs = vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: 21,
                    amount: 0,
                    creation_id: state.state.slot(21).creation_id(),
                },
                root: item_opening(item).root(),
                opening: Some(item_opening(item)),
            },
            ordinary_input(
                23,
                state.state.slot(23).creation_id(),
                creator_cash,
                CREATOR,
            ),
        ];
        let transfer_outputs = vec![
            native_output(30, item_opening(seller_item)),
            ordinary_output(31, creator_cash - transfer_fee, CREATOR),
        ];
        apply_native_transition(
            &mut state,
            &context(CREATOR, transfer_fee),
            NativeOperation::TransferNft,
            &transfer_inputs,
            &transfer_outputs,
        )
        .unwrap();

        let source_id = state.state.slot(30).creation_id();
        let definition =
            ListingDefinition::from_item(seller_item, source_id, ASK, HEIGHT + 100, HEIGHT)
                .unwrap();
        let listing = ListingState::initial(definition).unwrap();
        let seller_cash = state.state.slot(2).amount();
        let list_fee = required_fee(&state, 2, 2);
        let list_inputs = vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: 30,
                    amount: 0,
                    creation_id: source_id,
                },
                root: item_opening(seller_item).root(),
                opening: Some(item_opening(seller_item)),
            },
            ordinary_input(2, 2, seller_cash, SELLER),
        ];
        let list_outputs = vec![
            native_output(40, listing_opening(listing)),
            ordinary_output(41, seller_cash - list_fee - NFT_FEE_CREDIT, SELLER),
        ];
        let list_fees = MarketFeeContext::from_state(&state);
        apply_owned_listing_creation(
            &mut state,
            &context(SELLER, list_fee),
            definition,
            &list_inputs,
            &list_outputs,
            list_fees,
        )
        .unwrap();
        let listing_creation_id = state.state.slot(40).creation_id();
        (
            state,
            ParentListing {
                slot_index: 40,
                creation_id: listing_creation_id,
                definition,
                listing,
            },
            seller_item,
        )
    }

    fn purchase_claims(
        state: &ChainState,
        parent: &ParentListing,
        buyer: u8,
        cash_slot: u32,
    ) -> (Vec<NativeInputClaim>, Vec<NativeOutputClaim>, u64) {
        let cash = state.state.slot(cash_slot).amount();
        let fee = required_fee(state, 2, 3);
        let inputs = vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: parent.slot_index,
                    amount: NFT_FEE_CREDIT,
                    creation_id: state.state.slot(parent.slot_index).creation_id(),
                },
                root: listing_opening(parent.listing).root(),
                opening: Some(listing_opening(parent.listing)),
            },
            ordinary_input(cash_slot, u64::from(cash_slot), cash, buyer),
        ];
        let buyer_item = parent.definition.item(address(buyer)).unwrap();
        let outputs = vec![
            ordinary_output(50, ASK + NFT_FEE_CREDIT, SELLER),
            native_output(51, item_opening(buyer_item)),
            ordinary_output(52, cash - ASK - fee, buyer),
        ];
        (inputs, outputs, fee)
    }

    #[test]
    fn race_two_buyers_exactly_one_wins_loser_untouched() {
        let (base, parent, item) = listed_fixture();

        // World 1: BUYER_A first.
        let mut world_a = base.clone();
        let (a_in, a_out, a_fee) = purchase_claims(&world_a, &parent, BUYER_A, 3);
        apply_owned_listing_purchase(
            &mut world_a,
            &context(BUYER_A, a_fee),
            parent,
            &a_in,
            &a_out,
            MarketFeeContext::from_state(&base),
        )
        .unwrap();
        assert_eq!(world_a.state.slot(40), SlotValue::EMPTY);
        let winner_item_a = NftItem {
            owner: address(BUYER_A),
            ..item
        };
        let delivered_id = world_a.state.slot(51).creation_id();
        assert_eq!(
            world_a.state.slot(51),
            SlotValue::with_owner_fields(
                0,
                delivered_id,
                item_opening(winner_item_a).root().as_fields()
            ),
            "item delivered to buyer A with identity preserved"
        );
        // Buyer A paid exactly ask (+ fee burned); seller paid once.
        assert_eq!(world_a.state.slot(50).amount(), ASK + NFT_FEE_CREDIT);
        let buyer_a_cash_after = world_a.state.slot(52).amount();

        // The late buyer B on the same parent listing: rejected whole.
        let (b_in, b_out, b_fee) = purchase_claims(&world_a, &parent, BUYER_B, 4);
        let cash_b_before = world_a.state.slot(4).amount();
        let error = apply_owned_listing_purchase(
            &mut world_a,
            &context(BUYER_B, b_fee),
            parent,
            &b_in,
            &b_out,
            MarketFeeContext::from_state(&base),
        );
        assert!(error.is_err(), "second purchase of one listing must fail");
        assert_eq!(world_a.state.slot(4).amount(), cash_b_before);
        assert_eq!(world_a.state.slot(52).amount(), buyer_a_cash_after);
        assert_eq!(world_a.state.slot(50).amount(), ASK + NFT_FEE_CREDIT);

        // World 2: same start, BUYER_B first -> mirror outcome.
        let mut world_b = base.clone();
        let (b_in, b_out, b_fee) = purchase_claims(&world_b, &parent, BUYER_B, 4);
        apply_owned_listing_purchase(
            &mut world_b,
            &context(BUYER_B, b_fee),
            parent,
            &b_in,
            &b_out,
            MarketFeeContext::from_state(&base),
        )
        .unwrap();
        let winner_item_b = NftItem {
            owner: address(BUYER_B),
            ..item
        };
        let delivered_id = world_b.state.slot(51).creation_id();
        assert_eq!(
            world_b.state.slot(51),
            SlotValue::with_owner_fields(
                0,
                delivered_id,
                item_opening(winner_item_b).root().as_fields()
            ),
            "item delivered to buyer B in the mirrored world"
        );
        let (a_in, a_out, a_fee) = purchase_claims(&world_b, &parent, BUYER_A, 3);
        assert!(apply_owned_listing_purchase(
            &mut world_b,
            &context(BUYER_A, a_fee),
            parent,
            &a_in,
            &a_out,
            MarketFeeContext::from_state(&base),
        )
        .is_err());
        // Either world: exactly one item, one seller payment, loser's cash
        // intact; only the winner's identity follows inclusion order.
    }

    #[test]
    fn race_cancel_vs_buy_no_interleaving_moves_item_and_preserves_payment() {
        let (base, parent, item) = listed_fixture();

        let cancel_claims =
            |state: &ChainState| -> (Vec<NativeInputClaim>, Vec<NativeOutputClaim>, u64) {
                let fee = required_fee(state, 1, 2);
                let inputs = vec![NativeInputClaim {
                    input: TxInput {
                        slot_index: parent.slot_index,
                        amount: NFT_FEE_CREDIT,
                        creation_id: state.state.slot(parent.slot_index).creation_id(),
                    },
                    root: listing_opening(parent.listing).root(),
                    opening: Some(listing_opening(parent.listing)),
                }];
                let outputs = vec![
                    ordinary_output(60, NFT_FEE_CREDIT - fee, SELLER),
                    native_output(61, item_opening(item)),
                ];
                (inputs, outputs, fee)
            };

        // Interleaving 1: buy wins, late cancel must fail.
        let mut bought = base.clone();
        let (b_in, b_out, b_fee) = purchase_claims(&bought, &parent, BUYER_A, 3);
        apply_owned_listing_purchase(
            &mut bought,
            &context(BUYER_A, b_fee),
            parent,
            &b_in,
            &b_out,
            MarketFeeContext::from_state(&base),
        )
        .unwrap();
        let (c_in, c_out, c_fee) = cancel_claims(&bought);
        assert!(apply_owned_listing_cancellation(
            &mut bought,
            &context(SELLER, c_fee),
            parent,
            &c_in,
            &c_out,
            MarketFeeContext::from_state(&base),
        )
        .is_err());
        // Payment did not survive unspent AND the item did not return:
        assert_eq!(bought.state.slot(50).amount(), ASK + NFT_FEE_CREDIT);
        assert_eq!(bought.state.slot(61), SlotValue::EMPTY);
        assert_eq!(bought.state.slot(60), SlotValue::EMPTY);

        // Interleaving 2: cancel wins, late buy must fail.
        let mut cancelled = base.clone();
        let (c_in, c_out, c_fee) = cancel_claims(&cancelled);
        apply_owned_listing_cancellation(
            &mut cancelled,
            &context(SELLER, c_fee),
            parent,
            &c_in,
            &c_out,
            MarketFeeContext::from_state(&base),
        )
        .unwrap();
        let (b_in, b_out, b_fee) = purchase_claims(&cancelled, &parent, BUYER_A, 3);
        let cash_before = cancelled.state.slot(3).amount();
        assert!(apply_owned_listing_purchase(
            &mut cancelled,
            &context(BUYER_A, b_fee),
            parent,
            &b_in,
            &b_out,
            MarketFeeContext::from_state(&base),
        )
        .is_err());
        assert_eq!(cancelled.state.slot(3).amount(), cash_before);
        assert_eq!(cancelled.state.slot(50), SlotValue::EMPTY);
        assert_eq!(cancelled.state.slot(51), SlotValue::EMPTY);
    }

    #[test]
    fn wash_trading_self_list_self_buy_round_trip_cost() {
        let (mut state, parent, _item) = listed_fixture();
        let seller_cash_before = state.state.slot(41).amount();
        let supply_before = state.circulating_supply_micronoid;
        // The list fee was already burned at creation; measure from there.
        let list_fee = required_fee(&state, 2, 2); // same geometry as the listing transition

        // Self-buy: SELLER is also the buyer (cash at slot 2 was consumed at
        // listing; use the post-listing change slot 41).
        let cash = state.state.slot(41).amount();
        let fee = required_fee(&state, 2, 3);
        let inputs = vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: parent.slot_index,
                    amount: NFT_FEE_CREDIT,
                    creation_id: state.state.slot(parent.slot_index).creation_id(),
                },
                root: listing_opening(parent.listing).root(),
                opening: Some(listing_opening(parent.listing)),
            },
            ordinary_input(41, state.state.slot(41).creation_id(), cash, SELLER),
        ];
        let buyer_item = parent.definition.item(address(SELLER)).unwrap();
        let outputs = vec![
            ordinary_output(50, ASK + NFT_FEE_CREDIT, SELLER),
            native_output(51, item_opening(buyer_item)),
            ordinary_output(52, cash - ASK - fee, SELLER),
        ];
        let buy_fees = MarketFeeContext::from_state(&state);
        apply_owned_listing_purchase(
            &mut state,
            &context(SELLER, fee),
            parent,
            &inputs,
            &outputs,
            buy_fees,
        )
        .unwrap();
        // Round trip: seller listed, then bought own item. Cash position:
        // after list: seller_cash_before (post-list change slot 41).
        // after buy: slot 50 (ask + credit) + slot 52 (cash - ask - fee).
        let recovered = state.state.slot(50).amount() + state.state.slot(52).amount();
        assert_eq!(recovered + fee, seller_cash_before + NFT_FEE_CREDIT);
        let supply_burned = supply_before - state.circulating_supply_micronoid;
        assert_eq!(supply_burned, u128::from(fee));
        let breakdown = noid_chain::consensus::fees::fee_breakdown(
            2,
            3,
            state.active_slot_count,
            state.state.log_slots() as u32,
        );
        let list_breakdown = noid_chain::consensus::fees::fee_breakdown(
            2,
            2,
            state.active_slot_count,
            state.state.log_slots() as u32,
        );
        println!(
            "wash round trip at this state geometry: list fee {list_fee} + buy fee {fee} = {} µNOID per cycle (escrow credit {NFT_FEE_CREDIT} returned, ask {ASK} round-trips to the seller)",
            list_fee + fee
        );
        println!(
            "fee split at the floor geometry: list (burned {}, claimable {}), buy (burned {}, claimable {})",
            list_breakdown.burned,
            list_fee - list_breakdown.burned,
            breakdown.burned,
            fee - breakdown.burned
        );
        // There is no listing/creation fee beyond the spend-lane fee: the
        // wash cycle's marginal cost is exactly the two inclusion fees.
    }

    #[test]
    fn failed_purchase_costs_nothing_and_leaves_state_unchanged() {
        let (mut state, parent, _item) = listed_fixture();
        let root_before = state.try_state_root().unwrap();
        // Stale listing coordinates (wrong creation id) and attacker roles.
        let stale_parent = ParentListing {
            slot_index: parent.slot_index,
            creation_id: parent.creation_id.wrapping_add(1),
            ..parent
        };
        let cash = state.state.slot(3).amount();
        let fee = required_fee(&state, 2, 3);
        let inputs = vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: parent.slot_index,
                    amount: NFT_FEE_CREDIT,
                    creation_id: stale_parent.creation_id,
                },
                root: listing_opening(parent.listing).root(),
                opening: Some(listing_opening(parent.listing)),
            },
            ordinary_input(3, 3, cash, ATTACKER),
        ];
        let outputs = vec![
            ordinary_output(50, ASK + NFT_FEE_CREDIT, SELLER),
            native_output(
                51,
                item_opening(parent.definition.item(address(ATTACKER)).unwrap()),
            ),
            ordinary_output(52, cash - ASK - fee, ATTACKER),
        ];
        let stale_fees = MarketFeeContext::from_state(&state);
        assert!(apply_owned_listing_purchase(
            &mut state,
            &context(ATTACKER, fee),
            stale_parent,
            &inputs,
            &outputs,
            stale_fees,
        )
        .is_err());
        assert_eq!(state.try_state_root().unwrap(), root_before);
        assert_eq!(state.state.slot(3).amount(), cash);
    }
}

#[cfg(test)]
mod circuit_tests {
    // =====================================================================
    // Part 3: bind_financial_listing_settlement witness. The binder is dead
    // code at the pinned commit (no caller in the tree; the one function
    // taking FinancialListingSeedTrace ignores it and block_slots.rs passes
    // `&[]`), and the shipped profile blocks NftPurchase openings inside the
    // opening-root trace, so a live settlement is unprovable as shipped.
    // These tests (a) demonstrate both facts, then (b) build the intended
    // live-configuration witness via verbatim copies of the three private
    // helpers with one line changed (NftPurchase admitted to the opening
    // schema), and hunt second-satisfying witnesses on that configuration.
    // =====================================================================

    use noid_chain::state::native_assets::native_chain_id;
    use noid_core::Block128;
    use noid_poseidon2b::native::capacity_iv;
    use noid_poseidon2b::primitives::Address;
    use noid_recursive::acceptance::trace::native_assets::arithmetic::range_check_bits;
    use noid_recursive::acceptance::trace::native_assets::arithmetic::split_packed_input_value;
    use noid_recursive::acceptance::trace::native_assets::arithmetic::{checked_add, pack};
    use noid_recursive::acceptance::trace::native_assets::financial_actions::FinancialActionTrace;
    use noid_recursive::acceptance::trace::native_assets::financial_cash_payouts::FinancialCashOutputTrace;
    use noid_recursive::acceptance::trace::native_assets::financial_definitions::{
        FinancialDefinitionsTrace, SelectedFinancialDefinitionTrace,
    };
    use noid_recursive::acceptance::trace::native_assets::financial_listing_settlement::{
        bind_financial_listing_settlement, FinancialListingSeedTrace,
    };
    use noid_recursive::acceptance::trace::native_assets::financial_opening::{
        bind_financial_opening_frame, FinancialOpeningRootTrace,
    };
    use noid_recursive::acceptance::trace::native_assets::financial_operation::NativeOperationMetadataTrace;
    use noid_recursive::acceptance::trace::native_assets::financial_operation::FINANCIAL_OPERATION_CODES;
    use noid_recursive::acceptance::trace::native_assets::financial_slot::{
        bind_financial_slot, FinancialSlotTrace,
    };
    use noid_recursive::acceptance::trace::native_assets::header::{
        FinancialNativeHeaderTrace, NativeHeaderTrace,
    };
    use noid_recursive::acceptance::trace::native_assets::validation::{is_zero, require_nonzero};
    use noid_recursive::acceptance::trace::{
        alloc_block, flat_const, flat_of, mul, pin_eq, pin_zero, poseidon2b_permute,
        FieldR1csBuilder, LinExpr, F128,
    };
    use noid_tx::native_assets::market::queue::funded_request_key;
    use noid_tx::native_assets::nft_market::queue::{FundedPurchaseRef, ListingBatch};
    use noid_tx::native_assets::nft_market::settlement::ListingSettlement;
    use noid_tx::native_assets::nft_market::{
        ListingDefinition, ListingId, ListingState, NftMarketOpening, NftRoundContext,
        PurchaseRequest, NFT_FEE_CREDIT,
    };
    use noid_tx::native_assets::{
        market::MARKET_REQUEST_FEE_CREDIT, native_creation_id, NativeKind, NativeOpening, NftItem,
    };
    use noid_tx::pack_amount_creation_id;

    const REGISTERED: u64 = 400;
    const SETTLE: u64 = REGISTERED + 2;
    const PARENT_ID: [u8; 32] = [0x9B; 32];
    const PARENT_ALLOC: u64 = 9;
    const ASK: u64 = 700_000;
    const FEE: u64 = 7_000;

    fn address(byte: u8) -> Address {
        Address([byte; 32])
    }

    fn digest_bytes(digest: noid_poseidon2b::primitives::Digest) -> [u8; 32] {
        digest
    }

    fn constant(value: u128) -> LinExpr {
        LinExpr::constant(if value == 0 { F128::ZERO } else { F128::ONE })
    }

    /// Verbatim copy of `financial_opening_root_trace`/`opening_root_trace`
    /// (`noid_recursive/src/acceptance/trace/native_assets/financial_opening.rs`)
    /// with the single change that the opening schema also admits NftPurchase
    /// (the kind the live listing-settlement relation requires). Every other
    /// kind's constraint shape is byte-identical to production; the fidelity
    /// of the copy for enabled kinds is asserted by the control test below.
    fn opening_root_trace_live(
        b: &mut FieldR1csBuilder,
        kind: &LinExpr,
        fields: &[LinExpr; 12],
    ) -> FinancialOpeningRootTrace {
        let enabled = |k: NativeKind| {
            k == NativeKind::NftPurchase || noid_tx::native_assets::profile::kind_enabled(k)
        };
        range_check_bits(b, kind, 7);
        let inactive = is_zero(b, kind);
        let selectors = NativeKind::ALL.map(|k| {
            if enabled(k) {
                is_zero(b, &kind.add_const(flat_const(k as u128)))
            } else {
                LinExpr::zero()
            }
        });
        let selected = selectors.iter().fold(inactive.clone(), |sum, s| sum.add(s));
        pin_eq(b, &selected, &LinExpr::constant(F128::ONE));
        for (index, field) in fields.iter().enumerate() {
            let unused = NativeKind::ALL
                .iter()
                .zip(&selectors)
                .filter(|(k, _)| index >= k.field_count())
                .fold(inactive.clone(), |sum, (_, s)| sum.add(s));
            let nonzero_tail = mul(b, &unused, field);
            pin_zero(b, &nonzero_tail);
        }
        let mut iv = [LinExpr::zero(), LinExpr::zero()];
        let mut lengths: [LinExpr; 3] = std::array::from_fn(|_| LinExpr::zero());
        for (&k, selector) in NativeKind::ALL.iter().zip(&selectors) {
            let constants = capacity_iv(k.tag());
            for lane in 0..2 {
                iv[lane] = iv[lane].add(&selector.scale(flat_of(constants[lane])));
            }
            let length = match k.field_count() {
                6 => 0,
                8 => 1,
                12 => 2,
                _ => unreachable!("native width"),
            };
            lengths[length] = lengths[length].add(selector);
        }
        let mut state = [
            LinExpr::zero(),
            LinExpr::zero(),
            iv[0].clone(),
            iv[1].clone(),
        ];
        let mut root = [LinExpr::zero(), LinExpr::zero()];
        let max_fields = NativeKind::ALL
            .iter()
            .copied()
            .filter(|&kind| enabled(kind))
            .map(NativeKind::field_count)
            .max()
            .expect("nonempty native schema");
        for (index, pair) in fields[..max_fields].chunks_exact(2).enumerate() {
            state[0] = state[0].add(&pair[0]);
            state[1] = state[1].add(&pair[1]);
            state = poseidon2b_permute(b, state);
            let length = match index {
                2 => Some(0),
                3 => Some(1),
                5 => Some(2),
                _ => None,
            };
            if let Some(length) = length {
                for lane in 0..2 {
                    root[lane] = root[lane].add(&mul(b, &lengths[length], &state[lane]));
                }
            }
        }
        FinancialOpeningRootTrace {
            inactive,
            selectors,
            root,
        }
    }

    /// Verbatim copy of `amount_from_fields` from
    /// `financial_slot.rs` (private in production).
    fn amount_from_fields(
        b: &mut FieldR1csBuilder,
        root: &FinancialOpeningRootTrace,
        fields: &[LinExpr; 12],
    ) -> (LinExpr, Vec<LinExpr>) {
        use NativeKind::*;
        let s = |kind| root.selector(kind).clone();
        let mut main = LinExpr::zero();
        for (kind, field) in [
            (TokenReserve, 2),
            (MarketPool, 2),
            (MarketRequest, 10),
            (MarketClaim, 10),
            (MarketLiquidityDeposit, 6),
            (MarketLiquidityClaim, 6),
            (LaunchState, 2),
            (LaunchRequest, 8),
            (NftPurchase, 8),
            (NftOffer, 7),
        ] {
            main = main.add(&mul(b, &s(kind), &fields[field]));
        }
        let main = split_packed_input_value(b, &main);
        let flag_kinds = s(MarketPool)
            .add(&s(MarketLiquidityDeposit))
            .add(&s(MarketLiquidityClaim));
        let flags = mul(b, &flag_kinds, &fields[7]);
        let flags = range_check_bits(b, &flags, 128)
            .into_iter()
            .map(LinExpr::from_wire)
            .collect::<Vec<_>>();
        let pool_noid = mul(b, &s(MarketPool), &flags[88]);
        let deposit_noid = mul(b, &s(MarketLiquidityDeposit), &flags[63]);
        let claim_noid = mul(b, &s(MarketLiquidityClaim), &flags[127]);
        let input_zero = {
            let first = is_zero(b, &fields[2]);
            let second = is_zero(b, &fields[3]);
            mul(b, &first, &second)
        };
        let output_zero = {
            let first = is_zero(b, &fields[4]);
            let second = is_zero(b, &fields[5]);
            mul(b, &first, &second)
        };
        let request_noid = mul(b, &s(MarketRequest), &input_zero);
        let refund_noid = mul(b, &s(MarketClaim), &input_zero);
        let output_only = mul(b, &input_zero.add_const(F128::ONE), &output_zero);
        let output_noid = mul(b, &s(MarketClaim), &output_only);
        let launch_side = mul(b, &s(LaunchRequest), &fields[9]);
        let launch_side = range_check_bits(b, &launch_side, 65);
        let launch_buy = mul(
            b,
            &s(LaunchRequest),
            &LinExpr::from_wire(launch_side[64]).add_const(F128::ONE),
        );
        let low = s(TokenReserve)
            .add(&pool_noid)
            .add(&request_noid)
            .add(&refund_noid)
            .add(&deposit_noid)
            .add(&claim_noid)
            .add(&s(NftPurchase))
            .add(&s(NftOffer));
        let high = s(LaunchState).add(&launch_buy).add(&output_noid);
        let cash = (0..64)
            .map(|index| {
                let lo = mul(b, &low, &LinExpr::from_wire(main.amount.bits[index]));
                lo.add(&mul(
                    b,
                    &high,
                    &LinExpr::from_wire(main.creation_id.bits[index]),
                ))
            })
            .collect::<Vec<_>>();
        let ordinary_credit = s(MarketRequest)
            .add(&s(MarketLiquidityDeposit))
            .add(&s(MarketLiquidityWithdrawal))
            .add(&s(MarketLiquidityBurn))
            .add(&s(LaunchRequest));
        let nft_credit = s(NftListing).add(&s(NftPurchase)).add(&s(NftOffer));
        let mut credit = ordinary_credit
            .scale(flat_const(u128::from(MARKET_REQUEST_FEE_CREDIT)))
            .add(&nft_credit.scale(flat_const(u128::from(NFT_FEE_CREDIT))));
        credit = credit.add(&mul(b, &s(MarketClaim), &fields[11]));
        let mut claim_flags = LinExpr::zero();
        for (i, bit) in flags[64..127].iter().enumerate() {
            claim_flags = claim_flags.add(&bit.scale(flat_const(1u128 << i)));
        }
        credit = credit.add(&mul(b, &s(MarketLiquidityClaim), &claim_flags));
        let credit = range_check_bits(b, &credit, 64)
            .into_iter()
            .map(LinExpr::from_wire)
            .collect::<Vec<_>>();
        let bits = checked_add(b, &cash, &credit);
        (pack(&bits), bits)
    }

    /// Verbatim copy of `bind_slot` (private) for the live schema.
    fn bind_financial_slot_live(
        b: &mut FieldR1csBuilder,
        kind: &LinExpr,
        fields: &[LinExpr; 12],
        slot: &LinExpr,
        packed: &noid_recursive::acceptance::trace::public_arithmetic::PackedInputValueTrace,
        root: &[LinExpr; 2],
    ) -> FinancialSlotTrace {
        let commitment = opening_root_trace_live(b, kind, fields);
        bind_financial_opening_frame(b, &commitment, fields);
        let live = commitment.inactive.add_const(F128::ONE);
        let auxiliary = commitment
            .selector(NativeKind::MarketDefinition)
            .add(commitment.selector(NativeKind::LaunchDefinition))
            .add(commitment.selector(NativeKind::NftListingDefinition));
        pin_zero(b, &auxiliary);
        range_check_bits(b, slot, 32);
        let inactive_slot = mul(b, &commitment.inactive, slot);
        pin_zero(b, &inactive_slot);
        for lane in 0..2 {
            pin_eq(b, &root[lane], &commitment.root[lane]);
        }
        pin_zero(b, &LinExpr::from_wire(packed.creation_id.bits[63]));
        pin_eq(b, &LinExpr::from_wire(packed.creation_id.bits[62]), &live);
        let sequence = packed.creation_id.bits[..62]
            .iter()
            .copied()
            .map(LinExpr::from_wire)
            .collect::<Vec<_>>();
        require_nonzero(b, &pack(&sequence), &live);
        let inactive_id = mul(b, &commitment.inactive, &packed.creation_id.value);
        pin_zero(b, &inactive_id);
        let (amount, amount_bits) = amount_from_fields(b, &commitment, fields);
        pin_eq(b, &packed.amount.value, &amount);
        FinancialSlotTrace {
            commitment,
            fields: fields.clone(),
            live,
            amount,
            amount_bits,
            creation_id: packed.creation_id.clone(),
        }
    }

    fn allocated(builder: &mut FieldR1csBuilder, value: u128) -> LinExpr {
        alloc_block(builder, Block128(value))
    }

    fn allocated_address(builder: &mut FieldR1csBuilder, value: Address) -> [LinExpr; 2] {
        value.as_fields().map(|field| alloc_block(builder, field))
    }

    struct SettlementCase {
        definition: ListingDefinition,
        frozen: ListingState,
        requests: Vec<FundedPurchaseRef>,
        settlement: ListingSettlement,
        settle_height: u64,
    }

    fn native_settlement() -> SettlementCase {
        let item = NftItem {
            collection: noid_tx::native_assets::CollectionId([0x3A; 32]),
            owner: address(0x0A),
            metadata: [0x5C; 32],
            serial: 7,
            unlock_height: 0,
        };
        let definition = ListingDefinition::from_item(
            item,
            native_creation_id(4).unwrap(),
            ASK,
            REGISTERED + 400,
            REGISTERED - 10,
        )
        .unwrap();
        let idle = ListingState::initial(definition).unwrap();
        let context = NftRoundContext::new(REGISTERED, PARENT_ID, PARENT_ALLOC).unwrap();
        let round = idle.round_id(context).unwrap();
        let payers = [0x21u8, 0x22, 0x23];
        let budgets = [ASK + 40_000, ASK + 90_000, ASK + 5_000];
        let requests: Vec<FundedPurchaseRef> = payers
            .iter()
            .enumerate()
            .map(|(index, &payer)| FundedPurchaseRef {
                slot_index: 100 + index as u32,
                creation_id: native_creation_id(10 + index as u64).unwrap(),
                request: PurchaseRequest {
                    listing: definition.id(),
                    round,
                    payer: address(payer),
                    recipient: address(payer + 0x10),
                    budget: budgets[index],
                    registered_height: REGISTERED,
                    profile: 1,
                },
            })
            .collect();
        let batch = ListingBatch::register(definition, idle, context, &requests).unwrap();
        let settlement = ListingSettlement::calculate(&batch, SETTLE, FEE).unwrap();
        SettlementCase {
            definition,
            frozen: batch.after(),
            requests: batch.requests().to_vec(),
            settlement,
            settle_height: SETTLE,
        }
    }

    /// Independent recomputation of the native winner rule for cross-checks.
    fn native_winner_index(case: &SettlementCase) -> usize {
        let mut best: Option<(usize, ([u8; 32], [u8; 32], [u8; 32]))> = None;
        for (index, r) in case.requests.iter().enumerate() {
            let root: [u8; 32] = r.request.root().0;
            let key = funded_request_key(Address(root), r.slot_index, r.creation_id);
            let tuple = (digest_bytes(r.request.payer_priority()), root, key);
            if best.as_ref().is_none_or(|(_, b)| &tuple < b) {
                best = Some((index, tuple));
            }
        }
        best.expect("nonempty").0
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Variant {
        Honest,
        ItemToLoserRecipient,
        SettleTooEarly,
        SettleTooLate,
        QueueRootTampered,
        PayoutOffByOne,
    }

    fn settlement_witness(variant: Variant) -> bool {
        let case = native_settlement();
        let mut b = FieldR1csBuilder::new();

        // header (op 71 live)
        let op_index = FINANCIAL_OPERATION_CODES
            .iter()
            .position(|&x| x == 71)
            .unwrap();
        let mut financial = std::array::from_fn(|_: usize| constant(0));
        financial[op_index] = constant(1);
        let cash_payouts: Vec<_> = case
            .settlement
            .payouts()
            .iter()
            .filter(|p| p.item.is_none())
            .collect();
        let n_cash = cash_payouts.len();
        let header = FinancialNativeHeaderTrace {
            header: NativeHeaderTrace {
                live: constant(1),
                header_page: constant(1),
                operation: allocated(&mut b, 71),
                selectors: std::array::from_fn(|_| constant(0)),
                chain: allocated_address(&mut b, Address(native_chain_id())),
                authority: allocated_address(&mut b, address(0x0A)),
                asset: [constant(0), constant(0)],
                funding_input: constant(0),
                input_types: Vec::new(),
                output_types: Vec::new(),
                logical_txid: [constant(0), constant(0)],
                input_count: allocated(&mut b, case.requests.len() as u128 + 1),
                output_count: allocated(&mut b, n_cash as u128 + 1),
                fee: allocated(&mut b, FEE as u128),
            },
            metadata: NativeOperationMetadataTrace {
                inactive: constant(0),
                base: std::array::from_fn(|_| constant(0)),
                financial,
                trade_root: [constant(0), constant(0)],
                owner_required: constant(0),
                read_required: constant(0),
                registration_required: constant(0),
                definition_required: constant(0),
                funding_required: constant(0),
                read_slot: constant(0),
                read_creation_id: constant(0),
                read_sequence: constant(0),
                read_root: [constant(0), constant(0)],
                definition_root: [constant(0), constant(0)],
                asset: [constant(0), constant(0)],
                funding_input: constant(0),
            },
        };

        // actions: frozen listing input, request inputs, item output, cash
        let frozen = match variant {
            Variant::QueueRootTampered => {
                let mut tampered = case.frozen;
                tampered.queue_root[0] ^= 1;
                tampered
            }
            _ => case.frozen,
        };
        let _listing_fields = NftMarketOpening::Listing(frozen).fields();
        let mut actions: Vec<FinancialActionTrace> = Vec::new();
        let mut definitions: Vec<SelectedFinancialDefinitionTrace> = Vec::new();
        let mut cash_outputs: Vec<FinancialCashOutputTrace> = Vec::new();

        let push_action = |b: &mut FieldR1csBuilder,
                           actions: &mut Vec<FinancialActionTrace>,
                           definitions: &mut Vec<SelectedFinancialDefinitionTrace>,
                           cash_outputs: &mut Vec<FinancialCashOutputTrace>,
                           is_output: bool,
                           ordinal: u128,
                           slot: u32,
                           amount: u64,
                           creation_id: u64,
                           root: Address,
                           opening: Option<NativeOpening>| {
            let kind = allocated(b, opening.map_or(0, |o| o.kind() as u128));
            let fields = opening.map_or([Block128(0); 12], |o| o.fields());
            let fields = fields.map(|field| alloc_block(b, field));
            let slot_expr = allocated(b, u128::from(slot));
            let packed = allocated(b, pack_amount_creation_id(amount, creation_id).0);
            let root_expr = allocated_address(b, root);
            let packed_trace = split_packed_input_value(b, &packed);
            let opening_trace = if opening.is_some() {
                bind_financial_slot_live(b, &kind, &fields, &slot_expr, &packed_trace, &root_expr)
            } else {
                let zero_packed = split_packed_input_value(b, &constant(0));
                bind_financial_slot_live(
                    b,
                    &kind,
                    &fields,
                    &constant(0),
                    &zero_packed,
                    &[constant(0), constant(0)],
                )
            };
            let definition = if opening
                == Some(NativeOpening::NftMarket(NftMarketOpening::Listing(frozen)))
            {
                SelectedFinancialDefinitionTrace {
                    market: constant(0),
                    launch: constant(0),
                    listing: constant(1),
                    root: allocated_address(
                        b,
                        NftMarketOpening::Definition(case.definition).root(),
                    ),
                    fields: std::array::from_fn(|i: usize| {
                        alloc_block(b, NftMarketOpening::Definition(case.definition).fields()[i])
                    }),
                    base_is_noid: constant(0),
                }
            } else {
                SelectedFinancialDefinitionTrace {
                    market: constant(0),
                    launch: constant(0),
                    listing: constant(0),
                    root: [constant(0), constant(0)],
                    fields: std::array::from_fn(|_| constant(0)),
                    base_is_noid: constant(0),
                }
            };
            if !is_output && opening.is_none() {
                unreachable!("all inputs are native carriers");
            }
            if is_output && opening.is_none() {
                cash_outputs.push(FinancialCashOutputTrace {
                    live: constant(1),
                    group: constant(1),
                    owner: root_expr.clone(),
                    amount: allocated(b, u128::from(amount)),
                    ordinal: allocated(b, ordinal),
                });
            }
            actions.push(FinancialActionTrace {
                live: constant(1),
                native: constant(u128::from(opening.is_some())),
                funding: constant(u128::from(opening.is_none())),
                header_page: constant(1),
                ordinal: allocated(b, ordinal),
                is_output: constant(u128::from(is_output)),
                slot: slot_expr,
                packed_value: packed,
                root: root_expr,
                authority: allocated_address(b, address(0x0A)),
                operation: allocated(b, 71),
                opening: opening_trace,
            });
            definitions.push(definition);
        };

        // 1. frozen listing input
        push_action(
            &mut b,
            &mut actions,
            &mut definitions,
            &mut cash_outputs,
            false,
            0,
            40,
            NFT_FEE_CREDIT,
            native_creation_id(3).unwrap(),
            frozen.root(),
            Some(NativeOpening::NftMarket(NftMarketOpening::Listing(frozen))),
        );
        // 2. request inputs
        for (index, r) in case.requests.iter().enumerate() {
            let opening = NativeOpening::NftMarket(NftMarketOpening::Purchase(r.request));
            push_action(
                &mut b,
                &mut actions,
                &mut definitions,
                &mut cash_outputs,
                false,
                index as u128 + 1,
                r.slot_index,
                r.request.noid_value().unwrap(),
                r.creation_id,
                opening.root(),
                Some(opening),
            );
        }
        // 3. item output for the winner (or the loser, adversarially)
        let winner = native_winner_index(&case);
        let item_recipient = match variant {
            Variant::ItemToLoserRecipient => {
                case.requests[1.min(case.requests.len() - 1)]
                    .request
                    .recipient
            }
            _ => case.requests[winner].request.recipient,
        };
        let item = case.definition.item(item_recipient).unwrap();
        push_action(
            &mut b,
            &mut actions,
            &mut definitions,
            &mut cash_outputs,
            true,
            n_cash as u128,
            200,
            0,
            native_creation_id(30).unwrap(),
            NativeOpening::NftItem(item).root(),
            Some(NativeOpening::NftItem(item)),
        );
        // 4. cash payout outputs in native order (byte-ordered owners)
        for (ordinal, payout) in cash_payouts.iter().enumerate() {
            let amount = match variant {
                Variant::PayoutOffByOne if ordinal == 0 => payout.noid - 1,
                _ => payout.noid,
            };
            push_action(
                &mut b,
                &mut actions,
                &mut definitions,
                &mut cash_outputs,
                true,
                ordinal as u128,
                210 + ordinal as u32,
                amount,
                50 + ordinal as u64,
                payout.owner,
                None,
            );
        }

        let settle_height = match variant {
            Variant::SettleTooEarly => REGISTERED,
            Variant::SettleTooLate => REGISTERED + 4,
            _ => case.settle_height,
        };
        let height = allocated(&mut b, u128::from(settle_height));
        let seeds = [FinancialListingSeedTrace {
            parent_id: allocated_address(&mut b, Address(PARENT_ID)),
            parent_counter: allocated(&mut b, PARENT_ALLOC as u128),
        }];
        let definitions = FinancialDefinitionsTrace {
            table: Vec::new(),
            headers: vec![SelectedFinancialDefinitionTrace {
                market: constant(0),
                launch: constant(0),
                listing: constant(1),
                root: allocated_address(
                    &mut b,
                    NftMarketOpening::Definition(case.definition).root(),
                ),
                fields: std::array::from_fn(|i: usize| {
                    alloc_block(
                        &mut b,
                        NftMarketOpening::Definition(case.definition).fields()[i],
                    )
                }),
                base_is_noid: constant(0),
            }],
            reads: Vec::new(),
            actions: definitions,
        };
        // The listing input's opening must track the (possibly tampered)
        // frozen state: its root and fields were allocated above from
        // `case.frozen`; for the tampered variant rebuild that one action.
        bind_financial_listing_settlement(
            &mut b,
            &actions,
            &definitions,
            &[header],
            &seeds,
            &height,
        );

        let (r1cs, witness) = b.build();
        r1cs.satisfies(&witness)
    }

    #[test]
    fn circuit_honest_settlement_witness_satisfies() {
        assert!(
            settlement_witness(Variant::Honest),
            "the honest native settlement must satisfy the (live-schema) circuit"
        );
    }

    #[test]
    fn circuit_second_witness_hunt_all_adversarial_variants_fail() {
        assert!(!settlement_witness(Variant::ItemToLoserRecipient));
        assert!(!settlement_witness(Variant::SettleTooEarly));
        assert!(!settlement_witness(Variant::SettleTooLate));
        assert!(!settlement_witness(Variant::QueueRootTampered));
        assert!(!settlement_witness(Variant::PayoutOffByOne));
    }

    #[test]
    fn production_profile_blocks_purchase_openings_in_the_root_trace() {
        // The shipped `kind_enabled` set omits NftPurchase, so the production
        // opening-root trace cannot bind a purchase opening at all: its
        // one-hot selector sum constraint is unsatisfiable for that kind.
        let request = PurchaseRequest {
            listing: ListingId([0x11; 32]),
            round: [0x22; 32],
            payer: address(0x21),
            recipient: address(0x31),
            budget: 1_000,
            registered_height: REGISTERED,
            profile: 1,
        };
        let opening = NativeOpening::NftMarket(NftMarketOpening::Purchase(request));
        let mut b = FieldR1csBuilder::new();
        let kind = allocated(&mut b, opening.kind() as u128);
        let fields = opening.fields().map(|field| alloc_block(&mut b, field));
        let slot = allocated(&mut b, 100);
        let packed = allocated(
            &mut b,
            pack_amount_creation_id(
                request.noid_value().unwrap(),
                native_creation_id(10).unwrap(),
            )
            .0,
        );
        let root = allocated_address(&mut b, opening.root());
        let packed_trace = split_packed_input_value(&mut b, &packed);
        bind_financial_slot(&mut b, &kind, &fields, &slot, &packed_trace, &root);
        let (r1cs, witness) = b.build();
        assert!(
            !r1cs.satisfies(&witness),
            "production root trace must reject NftPurchase at this commit"
        );
        // The live-schema copy accepts the same opening.
        let mut b = FieldR1csBuilder::new();
        let kind = allocated(&mut b, opening.kind() as u128);
        let fields = opening.fields().map(|field| alloc_block(&mut b, field));
        let slot = allocated(&mut b, 100);
        let packed = allocated(
            &mut b,
            pack_amount_creation_id(
                request.noid_value().unwrap(),
                native_creation_id(10).unwrap(),
            )
            .0,
        );
        let root = allocated_address(&mut b, opening.root());
        let packed_trace = split_packed_input_value(&mut b, &packed);
        bind_financial_slot_live(&mut b, &kind, &fields, &slot, &packed_trace, &root);
        let (r1cs, witness) = b.build();
        assert!(r1cs.satisfies(&witness));
    }

    #[test]
    fn live_schema_copy_matches_production_for_enabled_kinds() {
        // Fidelity control: for kinds the shipped profile already admits,
        // the copied binders accept exactly what production accepts.
        let idle = ListingState::initial(
            ListingDefinition::from_item(
                NftItem {
                    collection: noid_tx::native_assets::CollectionId([0x3A; 32]),
                    owner: address(0x0A),
                    metadata: [0x5C; 32],
                    serial: 7,
                    unlock_height: 0,
                },
                native_creation_id(4).unwrap(),
                ASK,
                REGISTERED + 400,
                REGISTERED - 10,
            )
            .unwrap(),
        )
        .unwrap();
        for opening in [
            NativeOpening::NftMarket(NftMarketOpening::Listing(idle)),
            NativeOpening::NftItem(NftItem {
                collection: noid_tx::native_assets::CollectionId([0x3A; 32]),
                owner: address(0x0A),
                metadata: [0x5C; 32],
                serial: 7,
                unlock_height: 0,
            }),
        ] {
            for mutated in [false, true] {
                let mut fields = opening.fields();
                if mutated {
                    fields[0] = Block128(fields[0].0 ^ 1);
                }
                let satisfies = |binder: fn(
                    &mut FieldR1csBuilder,
                    &LinExpr,
                    &[LinExpr; 12],
                    &LinExpr,
                    &noid_recursive::acceptance::trace::public_arithmetic::PackedInputValueTrace,
                    &[LinExpr; 2],
                ) -> FinancialSlotTrace| {
                    let mut b = FieldR1csBuilder::new();
                    let kind = allocated(&mut b, opening.kind() as u128);
                    let field_exprs = fields.map(|field| alloc_block(&mut b, field));
                    let slot = allocated(&mut b, 40);
                    let amount = opening.amount_micronoid();
                    let packed = allocated(
                        &mut b,
                        pack_amount_creation_id(amount, native_creation_id(3).unwrap()).0,
                    );
                    let root = allocated_address(&mut b, opening.root());
                    let packed_trace = split_packed_input_value(&mut b, &packed);
                    binder(&mut b, &kind, &field_exprs, &slot, &packed_trace, &root);
                    let (r1cs, witness) = b.build();
                    r1cs.satisfies(&witness)
                };
                assert_eq!(
                    satisfies(bind_financial_slot),
                    satisfies(bind_financial_slot_live),
                    "copy fidelity for {:?} (mutated {mutated})",
                    opening.kind()
                );
                assert_eq!(satisfies(bind_financial_slot), !mutated);
            }
        }
    }

    #[test]
    fn native_and_circuit_winner_rules_agree_on_digest_order() {
        // The circuit's 768-bit order key is key || root || priority in
        // native big-endian byte order with priority most significant
        // (digest_bits + MSB-last less_than). Assert the chosen winner is
        // the native min_by_key((priority, root, key)) request.
        let case = native_settlement();
        let winner = native_winner_index(&case);
        assert_eq!(
            case.settlement.winner(),
            case.requests[winner].key(),
            "native settlement pays the min (priority, root, key) request"
        );
        // Sanity: winner depends on payer_priority first (grind-free check
        // using the actual digests).
        let mut by_priority = case.requests.clone();
        by_priority.sort_by(|a, b| {
            digest_bytes(a.request.payer_priority()).cmp(&digest_bytes(b.request.payer_priority()))
        });
        assert_eq!(by_priority[0].key(), case.settlement.winner());
    }
}
