#[cfg(test)]
mod tests {
    use noid_chain::fri_state::SlotValue;
    use noid_chain::state::native_assets::market::liquidity::MarketFeeContext;
    use noid_chain::state::native_assets::nft_market::{
        apply_block_listing_registrations, apply_expired_listing_recovery,
        apply_expired_offer_refund, apply_expired_purchase_refund, apply_listing_settlement,
        apply_listing_timeout_reset, apply_owned_listing_cancellation,
        apply_owned_listing_creation, apply_owned_listing_purchase, apply_owned_offer_acceptance,
        apply_owned_offer_cancellation, apply_owned_offer_funding, apply_owned_purchase_funding,
        FundedOffer, NftRoundAnchor, ParentListing,
    };
    use noid_chain::state::native_assets::{apply_native_transition, native_chain_id};
    use noid_chain::{block_id, BlockHeader, ChainState};
    use noid_core::Block128;
    use noid_poseidon2b::primitives::Address;
    use noid_recursive::acceptance::trace::native_assets::actions::NativeActionTrace;
    use noid_recursive::acceptance::trace::native_assets::arithmetic::split_packed_input_value;
    use noid_recursive::acceptance::trace::native_assets::header::NativeHeaderTrace;
    use noid_recursive::acceptance::trace::native_assets::kernel::bind_native_kernel;
    use noid_recursive::acceptance::trace::native_assets::validation::{
        bind_native_action_slot, bind_native_context, bind_native_opening,
    };
    use noid_recursive::acceptance::trace::{alloc_block, FieldR1csBuilder, LinExpr, F128};
    use noid_tx::native_assets::market::queue::MAX_ROUND_REQUESTS;
    use noid_tx::native_assets::market::PoolPhase;
    use noid_tx::native_assets::nft_market::queue::FundedPurchaseRef;
    use noid_tx::native_assets::nft_market::settlement::NftPayout;
    use noid_tx::native_assets::nft_market::{
        ListingDefinition, ListingState, NftMarketOpening, NftOffer, PurchaseRequest,
        NFT_FEE_CREDIT, NFT_MAX_SETTLEMENT_FEE_PER_REQUEST,
    };
    use noid_tx::native_assets::transition::{
        NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
    };
    use noid_tx::native_assets::{
        native_creation_id, GenesisAnchor, NativeOpening, NftCollection, NftItem,
    };
    use noid_tx::{pack_amount_creation_id, TxInput, TxOutput};

    const HEIGHT: u64 = 279_370;
    const CREATOR: u8 = 1;
    const SELLER: u8 = 2;
    const BUYER: u8 = 3;
    const ATTACKER: u8 = 9;
    const INITIAL_CASH: u64 = 2_000_000;
    const ASK: u64 = 500_000;

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
                SlotValue::with_owner_fields(INITIAL_CASH, 3, address(BUYER).as_fields()),
            ),
        ];
        ChainState::from_sparse_utxos(8, &slots, 3).unwrap()
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

    fn collection_opening(collection: NftCollection) -> NativeOpening {
        NativeOpening::NftCollection(collection)
    }

    fn item_opening(item: NftItem) -> NativeOpening {
        NativeOpening::NftItem(item)
    }

    fn listing_opening(listing: ListingState) -> NativeOpening {
        NativeOpening::NftMarket(NftMarketOpening::Listing(listing))
    }

    fn create_collection(state: &mut ChainState) -> NftCollection {
        let anchor = GenesisAnchor {
            chain: native_chain_id(),
            owner: address(CREATOR),
            input: TxInput {
                slot_index: 1,
                amount: INITIAL_CASH,
                creation_id: 1,
            },
        };
        let collection = NftCollection::from_genesis(&anchor, 4, [0xa1; 32]).unwrap();
        let inputs = vec![ordinary_input(1, 1, INITIAL_CASH, CREATOR)];
        let fee = required_fee(state, 1, 2);
        let outputs = vec![
            native_output(10, collection_opening(collection)),
            ordinary_output(11, INITIAL_CASH - fee, CREATOR),
        ];
        let transition = apply_native_transition(
            state,
            &context(CREATOR, fee),
            NativeOperation::CreateCollection { funding_input: 0 },
            &inputs,
            &outputs,
        )
        .unwrap();
        assert_eq!(transition.facts.native_outputs, 1);
        assert_eq!(
            state.state.slot(10).creation_id(),
            native_creation_id(4).unwrap()
        );
        collection
    }

    #[test]
    fn accepted_collection_mint_transfer_list_buy_cancel_burn_and_stale_replay() {
        let mut state = initial_state();
        let collection = create_collection(&mut state);
        let old_collection = collection_opening(collection);
        let collection_id = state.state.slot(10).creation_id();
        let minted_collection = NftCollection {
            issued_items: 2,
            ..collection
        };
        let item_one = NftItem {
            collection: collection.collection,
            owner: address(CREATOR),
            metadata: [0xb1; 32],
            serial: 1,
            unlock_height: 0,
        };
        let item_two = NftItem {
            metadata: [0xb2; 32],
            serial: 2,
            ..item_one
        };
        let mint_inputs = vec![
            native_input(&state, 10, old_collection),
            ordinary_input(
                11,
                state.state.slot(11).creation_id(),
                state.state.slot(11).amount(),
                CREATOR,
            ),
        ];
        let mint_fee = required_fee(&state, 2, 4);
        let creator_cash = state.state.slot(11).amount();
        let mint_outputs = vec![
            native_output(20, collection_opening(minted_collection)),
            native_output(21, item_opening(item_one)),
            native_output(22, item_opening(item_two)),
            ordinary_output(23, creator_cash - mint_fee, CREATOR),
        ];
        let mint = apply_native_transition(
            &mut state,
            &context(CREATOR, mint_fee),
            NativeOperation::MintNft,
            &mint_inputs,
            &mint_outputs,
        )
        .unwrap();
        assert_eq!(mint.facts.nft_items, 2);
        assert_eq!(
            state.state.slot(21).creation_id(),
            native_creation_id(7).unwrap()
        );
        assert_eq!(collection_id, native_creation_id(4).unwrap());

        let seller_item = NftItem {
            owner: address(SELLER),
            ..item_one
        };
        let creator_cash = state.state.slot(23).amount();
        let transfer_fee = required_fee(&state, 2, 2);
        let transfer_inputs = vec![
            native_input(&state, 21, item_opening(item_one)),
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
        assert_eq!(seller_item.id(), item_one.id());

        let source_id = state.state.slot(30).creation_id();
        let definition =
            ListingDefinition::from_item(seller_item, source_id, ASK, HEIGHT + 100, HEIGHT)
                .unwrap();
        let listing = ListingState::initial(definition).unwrap();
        let seller_cash = state.state.slot(2).amount();
        let list_fee = required_fee(&state, 2, 2);
        let list_inputs = vec![
            native_input(&state, 30, item_opening(seller_item)),
            ordinary_input(2, 2, seller_cash, SELLER),
        ];
        let list_outputs = vec![
            native_output(40, listing_opening(listing)),
            ordinary_output(41, seller_cash - list_fee - NFT_FEE_CREDIT, SELLER),
        ];
        let list_fees = MarketFeeContext::from_state(&state);
        let listed = apply_owned_listing_creation(
            &mut state,
            &context(SELLER, list_fee),
            definition,
            &list_inputs,
            &list_outputs,
            list_fees,
        )
        .unwrap();
        assert_eq!(listed.outputs[0].1, native_creation_id(12).unwrap());
        let listed_state = state.clone();
        let parent = ParentListing {
            slot_index: 40,
            creation_id: state.state.slot(40).creation_id(),
            definition,
            listing,
        };

        let buyer_cash = state.state.slot(3).amount();
        let buy_fee = required_fee(&state, 2, 3);
        let buy_inputs = vec![
            native_input(&state, 40, listing_opening(listing)),
            ordinary_input(3, 3, buyer_cash, BUYER),
        ];
        let buyer_item = definition.item(address(BUYER)).unwrap();
        let buy_outputs = vec![
            ordinary_output(50, ASK + NFT_FEE_CREDIT, SELLER),
            native_output(51, item_opening(buyer_item)),
            ordinary_output(52, buyer_cash - ASK - buy_fee, BUYER),
        ];
        let supply_before = state.circulating_supply_micronoid;
        apply_owned_listing_purchase(
            &mut state,
            &context(BUYER, buy_fee),
            parent,
            &buy_inputs,
            &buy_outputs,
            MarketFeeContext::from_state(&listed_state),
        )
        .unwrap();
        assert_eq!(state.state.slot(50).amount(), ASK + NFT_FEE_CREDIT);
        assert_eq!(buyer_item.owner, address(BUYER));
        assert_eq!(buyer_item.metadata, seller_item.metadata);
        assert_eq!(buyer_item.id(), seller_item.id());
        assert_eq!(
            supply_before - state.circulating_supply_micronoid,
            u128::from(buy_fee)
        );

        let mut raced = state.clone();
        let stale = apply_owned_listing_purchase(
            &mut raced,
            &context(BUYER, buy_fee),
            parent,
            &buy_inputs,
            &buy_outputs,
            MarketFeeContext::from_state(&listed_state),
        );
        assert!(stale.is_err(), "second purchase consumed the same listing");
        assert_eq!(raced.state.slot(40), SlotValue::EMPTY);

        let mut cancelled = listed_state.clone();
        let cancel_fee = required_fee(&cancelled, 1, 2);
        let cancel_inputs = vec![native_input(&cancelled, 40, listing_opening(listing))];
        let cancel_outputs = vec![
            ordinary_output(60, NFT_FEE_CREDIT - cancel_fee, SELLER),
            native_output(61, item_opening(seller_item)),
        ];
        apply_owned_listing_cancellation(
            &mut cancelled,
            &context(SELLER, cancel_fee),
            parent,
            &cancel_inputs,
            &cancel_outputs,
            MarketFeeContext::from_state(&listed_state),
        )
        .unwrap();
        assert_eq!(cancelled.state.slot(61).amount(), 0);

        let burn_fee = required_fee(&state, 2, 1);
        let buyer_change = state.state.slot(52).amount();
        let burn_inputs = vec![
            native_input(&state, 51, item_opening(buyer_item)),
            ordinary_input(52, state.state.slot(52).creation_id(), buyer_change, BUYER),
        ];
        let burn_outputs = vec![ordinary_output(70, buyer_change - burn_fee, BUYER)];
        let burn = apply_native_transition(
            &mut state,
            &context(BUYER, burn_fee),
            NativeOperation::BurnNft,
            &burn_inputs,
            &burn_outputs,
        )
        .unwrap();
        assert_eq!(burn.facts.nft_items, 1);
        assert_eq!(state.state.slot(51), SlotValue::EMPTY);
        assert_eq!(state.state.slot(20).amount(), 0);
        assert_eq!(minted_collection.issued_items, 2);
    }

    #[test]
    fn owner_price_expiry_metadata_and_duplicate_controls_reject() {
        let mut state = initial_state();
        let collection = create_collection(&mut state);
        let item = NftItem {
            collection: collection.collection,
            owner: address(CREATOR),
            metadata: [0xcc; 32],
            serial: 1,
            unlock_height: 0,
        };
        let next_collection = NftCollection {
            issued_items: 1,
            ..collection
        };
        let outputs = vec![
            native_output(20, collection_opening(next_collection)),
            native_output(21, item_opening(item)),
        ];
        let snapshot = state.clone();
        let error = apply_native_transition(
            &mut state,
            &context(ATTACKER, 0),
            NativeOperation::MintNft,
            &[native_input(&snapshot, 10, collection_opening(collection))],
            &outputs,
        )
        .unwrap_err();
        assert!(format!("{error:?}").contains("Authority"));

        let old = item_opening(item);
        let new = item_opening(NftItem {
            owner: address(BUYER),
            ..item
        });
        let error = noid_tx::native_assets::transition::check_native_transition(
            &context(CREATOR, 0),
            NativeOperation::TransferNft,
            &[
                NativeInputClaim {
                    input: TxInput {
                        slot_index: 80,
                        amount: 0,
                        creation_id: native_creation_id(7).unwrap(),
                    },
                    root: old.root(),
                    opening: Some(old),
                },
                NativeInputClaim {
                    input: TxInput {
                        slot_index: 81,
                        amount: 0,
                        creation_id: native_creation_id(8).unwrap(),
                    },
                    root: old.root(),
                    opening: Some(old),
                },
            ],
            &[native_output(82, new), native_output(83, new)],
        )
        .unwrap_err();
        assert!(format!("{error:?}").contains("NftConservation"));

        let changed_metadata = item_opening(NftItem {
            owner: address(BUYER),
            metadata: [0xdd; 32],
            ..item
        });
        let error = noid_tx::native_assets::transition::check_native_transition(
            &context(CREATOR, 0),
            NativeOperation::TransferNft,
            &[NativeInputClaim {
                input: TxInput {
                    slot_index: 80,
                    amount: 0,
                    creation_id: native_creation_id(7).unwrap(),
                },
                root: old.root(),
                opening: Some(old),
            }],
            &[native_output(82, changed_metadata)],
        )
        .unwrap_err();
        assert!(format!("{error:?}").contains("NftConservation"));
    }

    #[test]
    fn identity_codec_and_external_bytes_are_strict() {
        let item = NftItem {
            collection: noid_tx::native_assets::CollectionId([0x44; 32]),
            owner: address(CREATOR),
            metadata: [0x55; 32],
            serial: 7,
            unlock_height: 9,
        };
        let id = item.id();
        assert_eq!(
            id,
            NftItem {
                owner: address(BUYER),
                metadata: [0x66; 32],
                unlock_height: 0,
                ..item
            }
            .id()
        );
        let opening = item_opening(item);
        let bytes = opening.to_bytes().unwrap();
        assert_eq!(NativeOpening::from_bytes(&bytes).unwrap(), opening);
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(NativeOpening::from_bytes(&trailing).is_err());
        let mut padding = bytes;
        *padding.last_mut().unwrap() = 1;
        assert!(NativeOpening::from_bytes(&padding).is_err());
        assert!(item_opening(NftItem {
            owner: Address([0; 32]),
            ..item
        })
        .validate()
        .is_err());
    }

    fn constant(value: u128) -> LinExpr {
        LinExpr::constant(if value == 0 { F128::ZERO } else { F128::ONE })
    }

    fn allocated_address(builder: &mut FieldR1csBuilder, value: Address) -> [LinExpr; 2] {
        value.as_fields().map(|field| alloc_block(builder, field))
    }

    fn header(
        builder: &mut FieldR1csBuilder,
        operation: usize,
        authority: u8,
    ) -> NativeHeaderTrace {
        let operation_expr = alloc_block(builder, Block128(operation as u128));
        let chain = allocated_address(builder, Address(native_chain_id()));
        let authority = allocated_address(builder, address(authority));
        NativeHeaderTrace {
            live: constant(1),
            header_page: constant(1),
            operation: operation_expr,
            selectors: std::array::from_fn(|i| constant(u128::from(i == operation))),
            chain,
            authority,
            asset: [constant(0), constant(0)],
            funding_input: constant(0),
            input_types: Vec::new(),
            output_types: Vec::new(),
            logical_txid: [constant(0), constant(0)],
            input_count: constant(1),
            output_count: constant(1),
            fee: constant(0),
        }
    }

    struct ActionSpec {
        operation: usize,
        is_output: bool,
        slot: u32,
        creation_id: u64,
        amount: u64,
        root: Address,
        opening: Option<NativeOpening>,
        authority: u8,
    }

    fn action(builder: &mut FieldR1csBuilder, spec: ActionSpec, routed: bool) -> NativeActionTrace {
        let native = spec.opening.is_some();
        let native_flag = constant(u128::from(native));
        let output_flag = constant(u128::from(spec.is_output));
        let operation = alloc_block(builder, Block128(spec.operation as u128));
        let (kind, fields) = if let Some(opening) = spec.opening {
            (
                opening.kind() as u128,
                opening.compact_fields().unwrap_or([Block128(0); 8]),
            )
        } else {
            (0, [Block128(0); 8])
        };
        let kind = alloc_block(builder, Block128(kind));
        let fields = fields.map(|field| alloc_block(builder, field));
        let opening = bind_native_opening(builder, &kind, &fields);
        let packed = pack_amount_creation_id(spec.amount, spec.creation_id);
        let raw_slot = alloc_block(builder, Block128(u128::from(spec.slot)));
        let raw_packed = alloc_block(builder, packed);
        let raw_root = allocated_address(builder, spec.root);
        let retain = !routed || native;
        let action_slot = if retain {
            raw_slot.clone()
        } else {
            constant(0)
        };
        let action_packed = if retain {
            raw_packed.clone()
        } else {
            constant(0)
        };
        let action_root = if retain {
            raw_root.clone()
        } else {
            [constant(0), constant(0)]
        };
        let opening_slot = if native { raw_slot } else { constant(0) };
        let opening_packed = if native { raw_packed } else { constant(0) };
        let opening_root = if native {
            raw_root
        } else {
            [constant(0), constant(0)]
        };
        let full_authority = allocated_address(builder, address(spec.authority));
        let opening_authority = if native {
            full_authority.clone()
        } else {
            [constant(0), constant(0)]
        };
        let height = alloc_block(builder, Block128(u128::from(HEIGHT)));
        let context = bind_native_context(builder, &native_flag, &opening_authority, &height);
        let packed_trace = split_packed_input_value(builder, &opening_packed);
        bind_native_action_slot(
            builder,
            &opening,
            &opening_slot,
            &packed_trace,
            &opening_root,
            &output_flag,
            &context,
        );
        NativeActionTrace {
            live: constant(1),
            native: native_flag,
            funding: constant(u128::from(!native)),
            header_page: constant(1),
            ordinal: constant(0),
            is_output: output_flag,
            slot: action_slot,
            packed_value: action_packed,
            root: action_root,
            authority: full_authority,
            operation,
            opening,
        }
    }

    fn genesis_witness(routed: bool) -> bool {
        let anchor = GenesisAnchor {
            chain: native_chain_id(),
            owner: address(CREATOR),
            input: TxInput {
                slot_index: 1,
                amount: INITIAL_CASH,
                creation_id: 1,
            },
        };
        let collection = NftCollection::from_genesis(&anchor, 4, [0xa1; 32]).unwrap();
        let specs = vec![
            ActionSpec {
                operation: 5,
                is_output: false,
                slot: 1,
                creation_id: 1,
                amount: INITIAL_CASH,
                root: address(CREATOR),
                opening: None,
                authority: CREATOR,
            },
            ActionSpec {
                operation: 5,
                is_output: true,
                slot: 10,
                creation_id: native_creation_id(4).unwrap(),
                amount: 0,
                root: collection_opening(collection).root(),
                opening: Some(collection_opening(collection)),
                authority: CREATOR,
            },
        ];
        let mut builder = FieldR1csBuilder::new();
        let header = header(&mut builder, 5, CREATOR);
        let actions = specs
            .into_iter()
            .map(|spec| action(&mut builder, spec, routed))
            .collect::<Vec<_>>();
        let height = alloc_block(&mut builder, Block128(u128::from(HEIGHT)));
        bind_native_kernel(&mut builder, &[header], &actions, &height);
        let (r1cs, witness) = builder.build();
        r1cs.satisfies(&witness)
    }

    #[test]
    fn recursive_genesis_kernel_binds_funding_anchor_and_accepts_native_genesis() {
        let mut state = initial_state();
        create_collection(&mut state);
        // Production trace construction keeps the ordinary funding row's real
        // slot/packed/root (noid_recursive .../native_assets/actions.rs builds
        // NativeActionTrace from the live-masked rows; financial_base_projection.rs
        // likewise masks only by `selected`), so the satisfiable witness below is
        // the production-faithful one.
        assert!(
            genesis_witness(false),
            "production-faithful route (real funding anchor) should satisfy"
        );
        // Soundness control: if the funding anchor row were zeroed, the kernel
        // could not be satisfied -- the collection's genesis identity is
        // cryptographically bound to the exact funding slot/value/root.
        assert!(
            !genesis_witness(true),
            "zeroed funding anchor must not satisfy the kernel"
        );
    }

    fn transfer_witness(metadata_mutation: bool) -> bool {
        let item = NftItem {
            collection: noid_tx::native_assets::CollectionId([0x44; 32]),
            owner: address(CREATOR),
            metadata: [0x55; 32],
            serial: 1,
            unlock_height: 0,
        };
        let transferred = NftItem {
            owner: address(BUYER),
            metadata: if metadata_mutation {
                [0x66; 32]
            } else {
                item.metadata
            },
            ..item
        };
        let specs = vec![
            ActionSpec {
                operation: 7,
                is_output: false,
                slot: 21,
                creation_id: native_creation_id(7).unwrap(),
                amount: 0,
                root: item_opening(item).root(),
                opening: Some(item_opening(item)),
                authority: CREATOR,
            },
            ActionSpec {
                operation: 7,
                is_output: true,
                slot: 30,
                creation_id: native_creation_id(10).unwrap(),
                amount: 0,
                root: item_opening(transferred).root(),
                opening: Some(item_opening(transferred)),
                authority: CREATOR,
            },
        ];
        let mut builder = FieldR1csBuilder::new();
        let header = header(&mut builder, 7, CREATOR);
        let actions = specs
            .into_iter()
            .map(|spec| action(&mut builder, spec, true))
            .collect::<Vec<_>>();
        let height = alloc_block(&mut builder, Block128(u128::from(HEIGHT)));
        bind_native_kernel(&mut builder, &[header], &actions, &height);
        let (r1cs, witness) = builder.build();
        r1cs.satisfies(&witness)
    }

    #[test]
    fn recursive_transfer_matches_native_and_binds_metadata() {
        assert!(transfer_witness(false));
        assert!(!transfer_witness(true));
    }

    // ------------------------------------------------------------------
    // ------------------------------------------------------------------
    // Round-based escrow settlement, activation profile, offers, timelocks.
    //
    // At the pinned commit the purchase-request FUNDING transition and the
    // whole offer family are unreachable: profile::kind_enabled rejects the
    // NftPurchase/NftOffer opening kinds (validate() -> Err(Kind)) and
    // profile::operation_enabled rejects operation codes 67..=75. The round
    // apply functions below the funding step never validate openings, so we
    // seed their would-be escrow synthetically and exercise every downstream
    // state transition against the real ChainState machinery.
    // ------------------------------------------------------------------

    const ROUND_HEIGHT: u64 = HEIGHT + 1;
    const SETTLE_HEIGHT: u64 = HEIGHT + 2;
    const REFUND_HEIGHT: u64 = HEIGHT + 5;

    fn locked_item(collection: NftCollection, serial: u64) -> NftItem {
        NftItem {
            collection: collection.collection,
            owner: address(CREATOR),
            metadata: [0xb3; 32],
            serial,
            unlock_height: HEIGHT + 500,
        }
    }

    /// create -> mint(3: two free, one time-locked) -> transfer item1 to SELLER.
    /// Slots: 20 collection, 21 item1(moved to 30 for SELLER), 22 item2(CREATOR),
    /// 23 item3(CREATOR, locked), 31 creator cash, 2 seller cash.
    fn item_fixture() -> (ChainState, NftCollection, NftItem, NftItem) {
        let mut state = initial_state();
        let collection = create_collection(&mut state);
        let item_one = NftItem {
            collection: collection.collection,
            owner: address(CREATOR),
            metadata: [0xb1; 32],
            serial: 1,
            unlock_height: 0,
        };
        let item_two = NftItem {
            metadata: [0xb2; 32],
            serial: 2,
            ..item_one
        };
        let item_three = locked_item(collection, 3);
        let minted = NftCollection {
            issued_items: 3,
            ..collection
        };
        let mint_inputs = vec![
            native_input(&state, 10, collection_opening(collection)),
            ordinary_input(
                11,
                state.state.slot(11).creation_id(),
                state.state.slot(11).amount(),
                CREATOR,
            ),
        ];
        let mint_fee = required_fee(&state, 2, 5);
        let creator_cash = state.state.slot(11).amount();
        let mint_outputs = vec![
            native_output(20, collection_opening(minted)),
            native_output(21, item_opening(item_one)),
            native_output(22, item_opening(item_two)),
            native_output(23, item_opening(item_three)),
            ordinary_output(24, creator_cash - mint_fee, CREATOR),
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
            ..item_one
        };
        let creator_cash = state.state.slot(24).amount();
        let transfer_fee = required_fee(&state, 2, 2);
        let transfer_inputs = vec![
            native_input(&state, 21, item_opening(item_one)),
            ordinary_input(
                24,
                state.state.slot(24).creation_id(),
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
        (state, collection, seller_item, item_two)
    }

    fn list_item(
        state: &mut ChainState,
        item: NftItem,
        item_slot: u32,
        ask: u64,
        expiry: u64,
    ) -> (ListingDefinition, ListingState, u64) {
        let source_id = state.state.slot(item_slot).creation_id();
        let definition =
            ListingDefinition::from_item(item, source_id, ask, expiry, HEIGHT).unwrap();
        let listing = ListingState::initial(definition).unwrap();
        let seller_cash = state.state.slot(2).amount();
        let fee = required_fee(state, 2, 2);
        let inputs = vec![
            native_input(state, item_slot, item_opening(item)),
            ordinary_input(2, 2, seller_cash, SELLER),
        ];
        let outputs = vec![
            native_output(40, listing_opening(listing)),
            ordinary_output(41, seller_cash - fee - NFT_FEE_CREDIT, SELLER),
        ];
        let fees = MarketFeeContext::from_state(state);
        let applied = apply_owned_listing_creation(
            state,
            &context(SELLER, fee),
            definition,
            &inputs,
            &outputs,
            fees,
        )
        .unwrap();
        (definition, listing, applied.outputs[0].1)
    }

    fn parent_header(state: &mut ChainState, height: u64) -> BlockHeader {
        BlockHeader {
            prev_block_hash: [0x99; 32],
            state_root: state.try_state_root().unwrap(),
            tx_root: [0x88; 32],
            timestamp: 1_700_000_000,
            height,
            miner_address: address(5),
            nonce: 7,
            difficulty_target: [0x77; 32],
            log_slots: 8,
            active_slot_count: state.active_slot_count,
            alloc_counter: state.alloc_counter,
        }
    }

    /// Synthetic escrow: a real idle listing plus funded purchase requests,
    /// exactly the post-state the (profile-disabled) funding transition
    /// would have produced. Every slot value is bound to its opening root.
    fn round_fixture(
        expiry: u64,
    ) -> (
        ChainState,
        ParentListing,
        Vec<FundedPurchaseRef>,
        NftRoundAnchor,
    ) {
        let item = NftItem {
            collection: noid_tx::native_assets::CollectionId([0x44; 32]),
            owner: address(SELLER),
            metadata: [0x55; 32],
            serial: 1,
            unlock_height: 0,
        };
        let definition =
            ListingDefinition::from_item(item, native_creation_id(2).unwrap(), ASK, expiry, HEIGHT)
                .unwrap();
        let listing = ListingState::initial(definition).unwrap();
        let parent = BlockHeader {
            prev_block_hash: [0x99; 32],
            state_root: [0x66; 32],
            tx_root: [0x88; 32],
            timestamp: 1_700_000_000,
            height: HEIGHT,
            miner_address: address(5),
            nonce: 7,
            difficulty_target: [0x77; 32],
            log_slots: 8,
            active_slot_count: 6,
            alloc_counter: 9,
        };
        let anchor =
            NftRoundAnchor::from_parent_header(ROUND_HEIGHT, block_id(&parent), parent).unwrap();
        let payers = [CREATOR, SELLER, BUYER];
        let budgets = [600_000, 500_000, 700_000];
        let requests: Vec<_> = (0..3)
            .map(|i| FundedPurchaseRef {
                slot_index: 10 + i as u32,
                creation_id: native_creation_id(10 + i as u64).unwrap(),
                request: PurchaseRequest {
                    listing: definition.id(),
                    round: listing.round_id(anchor.context()).unwrap(),
                    payer: address(payers[i]),
                    recipient: address(payers[i] + 0x10),
                    budget: budgets[i],
                    registered_height: ROUND_HEIGHT,
                    profile: 1,
                },
            })
            .collect();
        let mut slots = vec![
            (
                1,
                SlotValue::with_owner_fields(2_000_000, 1, address(SELLER).as_fields()),
            ),
            (
                2,
                SlotValue::with_owner_fields(2_000_000, 2, address(BUYER).as_fields()),
            ),
            (
                9,
                SlotValue::with_owner_fields(
                    NFT_FEE_CREDIT,
                    native_creation_id(9).unwrap(),
                    listing.root().as_fields(),
                ),
            ),
        ];
        for r in &requests {
            slots.push((
                r.slot_index,
                SlotValue::with_owner_fields(
                    r.request.noid_value().unwrap(),
                    r.creation_id,
                    r.request.root().as_fields(),
                ),
            ));
        }
        let state = ChainState::from_sparse_utxos_at(8, &slots, 12, ROUND_HEIGHT).unwrap();
        (
            state,
            ParentListing {
                slot_index: 9,
                creation_id: native_creation_id(9).unwrap(),
                definition,
                listing,
            },
            requests,
            anchor,
        )
    }

    type MarketOutputs = noid_chain::state::native_assets::nft_market::NftMarketStateOutputs;
    type MarketError = noid_chain::state::native_assets::nft_market::NftMarketApplyError;

    fn settle_round(
        state: &mut ChainState,
        height: u64,
        frozen: ParentListing,
        requests: &[FundedPurchaseRef],
        slots: &[u32],
        anchor: &NftRoundAnchor,
    ) -> Result<noid_chain::state::native_assets::nft_market::ListingSettlementEffects, MarketError>
    {
        let fees = MarketFeeContext::from_state(state);
        apply_listing_settlement(state, height, frozen, requests, slots, fees, anchor)
    }

    fn refund_purchase(
        state: &mut ChainState,
        height: u64,
        request: FundedPurchaseRef,
        slot: u32,
    ) -> Result<MarketOutputs, MarketError> {
        let fees = MarketFeeContext::from_state(state);
        apply_expired_purchase_refund(state, height, request, slot, fees)
    }

    fn recover_listing(
        state: &mut ChainState,
        height: u64,
        frozen: ParentListing,
        slots: &[u32],
    ) -> Result<MarketOutputs, MarketError> {
        let fees = MarketFeeContext::from_state(state);
        apply_expired_listing_recovery(state, height, frozen, slots, fees)
    }

    fn independent_charges(requests: &[FundedPurchaseRef], fee: u64) -> Vec<(u64, u64)> {
        // (creation_id, expected charged fee) in native funded-key order.
        let count = requests.len() as u64;
        let mut sorted = requests.to_vec();
        sorted.sort_by_key(|r| r.key());
        sorted
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    r.creation_id,
                    fee / count + u64::from((i as u64) < fee % count),
                )
            })
            .collect()
    }

    #[test]
    fn round_settlement_exact_accounting_and_stale_rejections() {
        let (mut state, idle, requests, anchor) = round_fixture(HEIGHT + 100);
        let effects =
            apply_block_listing_registrations(&mut state, &anchor, &[idle], &requests, &[7])
                .unwrap();
        let (slot, creation, listing) = effects.controls[0];
        let frozen = ParentListing {
            slot_index: slot,
            creation_id: creation,
            definition: idle.definition,
            listing,
        };

        // Registration duplicated or tampered request sets are rejected.
        let error = apply_block_listing_registrations(
            &mut state,
            &anchor,
            &[idle],
            &[requests[0].clone(), requests[0].clone()],
            &[8],
        );
        assert!(
            format!("{error:?}").contains("Input"),
            "duplicate request registered"
        );
        let mut tampered = requests.clone();
        tampered[0].request.budget += 1;
        let error = settle_round(
            &mut state,
            SETTLE_HEIGHT,
            frozen,
            &tampered,
            &[10, 11, 12, 13],
            &anchor,
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Queue"),
            "tampered budget settled"
        );
        let error = settle_round(
            &mut state,
            SETTLE_HEIGHT,
            frozen,
            &requests[1..],
            &[10, 11, 12],
            &anchor,
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Queue"),
            "dropped request settled"
        );

        // Instant owner purchase and cancellation cannot touch a frozen round.
        let snapshot = state.clone();
        let buyer_cash = snapshot.state.slot(2).amount();
        let fee = MarketFeeContext::from_state(&snapshot)
            .required(2, 3)
            .unwrap();
        let error = apply_owned_listing_purchase(
            &mut state,
            &context(BUYER, fee),
            frozen,
            &[
                native_input(&snapshot, slot, listing_opening(frozen.listing)),
                ordinary_input(2, 2, buyer_cash, BUYER),
            ],
            &[
                ordinary_output(20, ASK + NFT_FEE_CREDIT, SELLER),
                native_output(
                    21,
                    item_opening(frozen.definition.item(address(BUYER)).unwrap()),
                ),
                ordinary_output(22, buyer_cash - ASK - fee, BUYER),
            ],
            MarketFeeContext::from_state(&snapshot),
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Phase"),
            "instant buy on frozen round"
        );
        let cancel_fee = MarketFeeContext::from_state(&snapshot)
            .required(1, 2)
            .unwrap();
        let error = apply_owned_listing_cancellation(
            &mut state,
            &context(SELLER, cancel_fee),
            frozen,
            &[native_input(
                &snapshot,
                slot,
                listing_opening(frozen.listing),
            )],
            &[
                ordinary_output(23, NFT_FEE_CREDIT - cancel_fee, SELLER),
                native_output(
                    24,
                    item_opening(frozen.definition.item(address(SELLER)).unwrap()),
                ),
            ],
            MarketFeeContext::from_state(&snapshot),
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Phase"),
            "cancel on frozen round"
        );

        // Clean settlement: winner, refunds, fee shares and supply burn.
        // The SELLER bids on their own listing, so their refund merges with
        // the sale payout: three cash entries plus the item.
        let fee = MarketFeeContext::from_state(&state)
            .required(requests.len() + 1, requests.len() + 1)
            .unwrap();
        assert!(fee <= requests.len() as u64 * NFT_MAX_SETTLEMENT_FEE_PER_REQUEST);
        let supply_before = state.circulating_supply_micronoid;
        let effects = settle_round(
            &mut state,
            SETTLE_HEIGHT,
            frozen,
            &requests,
            &[20, 21, 22, 23],
            &anchor,
        )
        .unwrap();
        assert_eq!(effects.settlement.fee(), fee);
        assert_eq!(
            supply_before - state.circulating_supply_micronoid,
            u128::from(fee)
        );
        // Winner selection must match the native rule: min by
        // (payer_priority, request root, funded request key), recomputed here
        // independently of ListingSettlement::calculate.
        let winner = requests
            .iter()
            .min_by_key(|r| (r.request.payer_priority(), r.request.root().0, r.key()))
            .unwrap();
        assert_eq!(effects.settlement.winner(), winner.key());
        let charges = independent_charges(&requests, fee);
        let charge_of = |creation: u64| charges.iter().find(|(c, _)| *c == creation).unwrap().1;
        // Same-owner payouts merge (the SELLER bids on their own listing).
        let mut expected_cash: std::collections::BTreeMap<[u8; 32], u64> =
            std::collections::BTreeMap::new();
        expected_cash.entry(address(SELLER).0).or_insert(0);
        *expected_cash.get_mut(&address(SELLER).0).unwrap() += ASK + NFT_FEE_CREDIT;
        for r in &requests {
            let spent = if r.creation_id == winner.creation_id {
                ASK
            } else {
                0
            };
            let refund = r.request.budget - spent + NFT_FEE_CREDIT - charge_of(r.creation_id);
            *expected_cash.entry(r.request.payer.0).or_insert(0) += refund;
        }
        let mut expected_cash: Vec<(u64, [u8; 32])> =
            expected_cash.into_iter().map(|(k, v)| (v, k)).collect();
        let mut actual_cash: Vec<(u64, [u8; 32])> = Vec::new();
        let mut delivered = None;
        for p in effects.settlement.payouts() {
            if let Some(item) = p.item {
                assert_eq!(p.noid, 0);
                delivered = Some(item);
            } else {
                actual_cash.push((p.noid, p.owner.0));
            }
        }
        expected_cash.sort();
        actual_cash.sort();
        assert_eq!(expected_cash, actual_cash);
        let item = delivered.expect("settlement delivers the NFT");
        assert_eq!(item.owner, winner.request.recipient);
        assert_eq!(item.metadata, frozen.definition.metadata);
        assert_eq!(item.serial, frozen.definition.serial);
        assert_eq!(item.unlock_height, 0);

        // Replay of the consumed round fails.
        let replay = settle_round(
            &mut state,
            SETTLE_HEIGHT,
            frozen,
            &requests,
            &[24, 25, 26, 27],
            &anchor,
        );
        assert!(
            replay.is_err(),
            "settlement replay consumed the same escrow"
        );

        // Funding-time semantic rejections (pure layer, below the profile
        // gate that currently blocks the funding operation entirely).
        let round = anchor.context();
        let mut wrong_round = requests[0].request;
        wrong_round.round = [0xAA; 32];
        assert!(wrong_round
            .check_round(idle.definition, idle.listing, round)
            .is_err());
        let mut cheap = requests[0].request;
        cheap.budget = ASK - 1;
        assert!(cheap
            .check_round(idle.definition, idle.listing, round)
            .is_err());
        let good = requests[0].request;
        assert!(good
            .check_round(idle.definition, idle.listing, round)
            .is_ok());
    }

    #[test]
    fn timeout_refund_reset_and_expired_recovery_paths() {
        // Long-expiry listing: expires long after the round window.
        let (mut state, idle, requests, anchor) = round_fixture(HEIGHT + 100);
        let effects =
            apply_block_listing_registrations(&mut state, &anchor, &[idle], &requests, &[7])
                .unwrap();
        let (slot, creation, listing) = effects.controls[0];
        let frozen = ParentListing {
            slot_index: slot,
            creation_id: creation,
            definition: idle.definition,
            listing,
        };

        // Inside the window refunds are rejected; past it, the round is stuck
        // in Refund phase: settlement rejected, per-request refunds accepted.
        let refund_fee = MarketFeeContext::from_state(&state).required(1, 1).unwrap();
        assert!(refund_fee < NFT_FEE_CREDIT);
        let error = refund_purchase(&mut state, HEIGHT + 4, requests[0].clone(), 20).unwrap_err();
        assert!(
            format!("{error:?}").contains("Phase"),
            "early refund accepted"
        );
        let error = settle_round(
            &mut state,
            REFUND_HEIGHT,
            frozen,
            &requests,
            &[10, 11, 12, 13],
            &anchor,
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Phase"),
            "settlement past the window"
        );
        let supply_before = state.circulating_supply_micronoid;
        refund_purchase(&mut state, REFUND_HEIGHT, requests[0].clone(), 20).unwrap();
        assert_eq!(
            state.state.slot(20).amount(),
            requests[0].request.budget + NFT_FEE_CREDIT - refund_fee
        );
        assert_eq!(
            supply_before - state.circulating_supply_micronoid,
            u128::from(refund_fee)
        );
        refund_purchase(&mut state, REFUND_HEIGHT, requests[1].clone(), 21).unwrap();

        // Recovery of this listing is blocked: it has not expired.
        let error = recover_listing(&mut state, REFUND_HEIGHT, frozen, &[22, 23]).unwrap_err();
        assert!(
            format!("{error:?}").contains("Phase"),
            "unexpired listing recovered"
        );

        let (root, _creation, reset) =
            apply_listing_timeout_reset(&mut state, REFUND_HEIGHT, frozen, 22).unwrap();
        assert_ne!(root, [0; 32]);
        assert_eq!(reset.phase, PoolPhase::Idle);
        assert_eq!(reset.generation, frozen.listing.generation + 1);
        assert_ne!(reset.root(), frozen.listing.root());
        // Old round identities cannot be replayed after the reset: the round
        // id mixes the listing root, which embeds the generation, and the
        // frozen generation refuses to mint a round id at all.
        assert!(frozen.listing.round_id(anchor.context()).is_err());
        assert_ne!(
            reset.round_id(anchor.context()).unwrap(),
            requests[0].request.round
        );
        // The consumed frozen control cannot be reset twice.
        let error = apply_listing_timeout_reset(&mut state, REFUND_HEIGHT, frozen, 23).unwrap_err();
        assert!(format!("{error:?}").contains("Input"), "double reset");

        // Short-expiry listing: expired before the refund boundary, so
        // recovery returns the item and remaining escrow credit to the seller.
        let (mut state, idle, requests, anchor) = round_fixture(HEIGHT + 3);
        let effects =
            apply_block_listing_registrations(&mut state, &anchor, &[idle], &requests, &[7])
                .unwrap();
        let (slot, creation, listing) = effects.controls[0];
        let frozen = ParentListing {
            slot_index: slot,
            creation_id: creation,
            definition: idle.definition,
            listing,
        };
        let recovery_fee = MarketFeeContext::from_state(&state).required(1, 2).unwrap();
        assert!(recovery_fee < NFT_FEE_CREDIT);
        let supply_before = state.circulating_supply_micronoid;
        let effects = recover_listing(&mut state, REFUND_HEIGHT, frozen, &[20, 21]).unwrap();
        assert_eq!(
            supply_before - state.circulating_supply_micronoid,
            u128::from(recovery_fee)
        );
        let mut payouts: Vec<(u64, [u8; 32])> = effects
            .outputs
            .iter()
            .map(|(_, _, c)| (c.output.amount, c.output.owner.0))
            .collect();
        payouts.sort();
        let expected_item = NftPayout {
            owner: address(SELLER),
            noid: 0,
            item: Some(idle.definition.item(address(SELLER)).unwrap()),
        };
        let mut expected = vec![
            (NFT_FEE_CREDIT - recovery_fee, address(SELLER).0),
            (0, expected_item.root().0),
        ];
        expected.sort();
        assert_eq!(payouts, expected);
        // The still-funded requests refund independently after the boundary.
        refund_purchase(&mut state, REFUND_HEIGHT, requests[0].clone(), 22).unwrap();
        refund_purchase(&mut state, REFUND_HEIGHT, requests[1].clone(), 23).unwrap();
    }

    #[test]
    fn activation_profile_gates_round_and_offer_surface() {
        use noid_tx::native_assets::financial::FinancialOperation;
        use noid_tx::native_assets::{profile, NativeKind};

        // Live kinds and operations at the pinned commit.
        assert!(profile::kind_enabled(NativeKind::NftItem));
        assert!(profile::kind_enabled(NativeKind::NftCollection));
        assert!(profile::kind_enabled(NativeKind::NftListingDefinition));
        assert!(profile::kind_enabled(NativeKind::NftListing));
        for code in [5u16, 6, 7, 8] {
            assert!(profile::operation_enabled(code), "base NFT op {code}");
        }
        for code in [64u16, 65, 66] {
            assert!(profile::operation_enabled(code), "live listing op {code}");
        }
        // The request and offer opening kinds are disabled.
        assert!(!profile::kind_enabled(NativeKind::NftPurchase));
        assert!(!profile::kind_enabled(NativeKind::NftOffer));
        // The whole round and offer operation family is disabled.
        for code in 67u16..=75 {
            assert!(!profile::operation_enabled(code), "op {code} enabled");
        }
        assert!(FinancialOperation::CreateListing {
            definition: address(1)
        }
        .operand()
        .is_ok());
        assert!(FinancialOperation::SettleListing.operand().is_err());
        assert!(FinancialOperation::CreateOffer { funding_input: 0 }
            .operand()
            .is_err());

        // End-to-end: even perfectly formed purchase-request funding and
        // offer funding are rejected at the opening-kind gate.
        let (mut state, _, seller_item, _) = item_fixture();
        let (definition, listing, listing_creation) =
            list_item(&mut state, seller_item, 30, ASK, HEIGHT + 100);
        let idle = ParentListing {
            slot_index: 40,
            creation_id: listing_creation,
            definition,
            listing,
        };
        let mut probed = state.clone();
        probed.try_state_root().unwrap();
        let (parent, anchor) = {
            let parent = parent_header(&mut probed, HEIGHT);
            let anchor =
                NftRoundAnchor::from_parent_header(ROUND_HEIGHT, block_id(&parent), parent)
                    .unwrap();
            (parent, anchor)
        };
        let fee = MarketFeeContext::from_parent(&parent)
            .required(1, 2)
            .unwrap();
        let cash = state.state.slot(3).amount();
        let request = PurchaseRequest {
            listing: definition.id(),
            round: listing.round_id(anchor.context()).unwrap(),
            payer: address(BUYER),
            recipient: address(BUYER + 0x10),
            budget: ASK,
            registered_height: ROUND_HEIGHT,
            profile: 1,
        };
        let error = apply_owned_purchase_funding(
            &mut state,
            &NativeContext {
                height: ROUND_HEIGHT,
                ..context(BUYER, fee)
            },
            idle,
            &[ordinary_input(3, 3, cash, BUYER)],
            &[
                native_output(
                    42,
                    NativeOpening::NftMarket(NftMarketOpening::Purchase(request)),
                ),
                ordinary_output(43, cash - fee - ASK - NFT_FEE_CREDIT, BUYER),
            ],
            &anchor,
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Funding"),
            "purchase-request funding unexpectedly accepted: {error:?}"
        );

        let (mut state, collection, _, _) = item_fixture();
        let offer_fees = MarketFeeContext::from_state(&state);
        let fee = offer_fees.required(1, 2).unwrap();
        let cash = state.state.slot(3).amount();
        let offer = NftOffer {
            collection: collection.collection,
            payer: address(BUYER),
            recipient: address(BUYER + 0x20),
            serial: 0,
            expiry_height: HEIGHT + 50,
            price: 300_000,
            funding_creation_id: 3,
        };
        let error = apply_owned_offer_funding(
            &mut state,
            &context(BUYER, fee),
            0,
            &[ordinary_input(3, 3, cash, BUYER)],
            &[
                native_output(42, NativeOpening::NftMarket(NftMarketOpening::Offer(offer))),
                ordinary_output(43, cash - fee - 300_000 - NFT_FEE_CREDIT, BUYER),
            ],
            offer_fees,
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Funding"),
            "offer funding unexpectedly accepted: {error:?}"
        );

        // The expired-offer refund path never validates openings, so it runs
        // against synthetic escrow even at this commit.
        let mut refund_state = {
            let offer = NftOffer {
                expiry_height: HEIGHT,
                ..offer
            };
            let mut state = initial_state();
            let _ = &mut state;
            let slots = [
                (
                    9u32,
                    SlotValue::with_owner_fields(
                        offer.noid_value().unwrap(),
                        native_creation_id(9).unwrap(),
                        offer.root().as_fields(),
                    ),
                ),
                (
                    1,
                    SlotValue::with_owner_fields(2_000_000, 1, address(CREATOR).as_fields()),
                ),
            ];
            ChainState::from_sparse_utxos_at(8, &slots, 9, HEIGHT + 1).unwrap()
        };
        let refund_fees = MarketFeeContext::from_state(&refund_state);
        let fee = refund_fees.required(1, 1).unwrap();
        assert!(fee < NFT_FEE_CREDIT);
        let offer = NftOffer {
            expiry_height: HEIGHT,
            ..offer
        };
        let supply_before = refund_state.circulating_supply_micronoid;
        apply_expired_offer_refund(
            &mut refund_state,
            HEIGHT + 1,
            FundedOffer {
                slot_index: 9,
                creation_id: native_creation_id(9).unwrap(),
                offer,
            },
            10,
            refund_fees,
        )
        .unwrap();
        assert_eq!(
            refund_state.state.slot(10).amount(),
            300_000 + NFT_FEE_CREDIT - fee
        );
        assert_eq!(
            supply_before - refund_state.circulating_supply_micronoid,
            u128::from(fee)
        );
    }

    #[test]
    fn time_locked_items_reject_transfer_and_listing_paths() {
        let (mut state, collection, _, _) = item_fixture();
        let locked = locked_item(collection, 3);
        // Direct transfer of the locked item by its owner.
        let fee = MarketFeeContext::from_state(&state).required(2, 2).unwrap();
        let snapshot = state.clone();
        let cash = snapshot.state.slot(24).amount();
        let error = apply_native_transition(
            &mut state,
            &context(CREATOR, fee),
            NativeOperation::TransferNft,
            &[
                native_input(&snapshot, 23, item_opening(locked)),
                ordinary_input(24, snapshot.state.slot(24).creation_id(), cash, CREATOR),
            ],
            &[
                native_output(
                    30,
                    item_opening(NftItem {
                        owner: address(BUYER),
                        ..locked
                    }),
                ),
                ordinary_output(31, cash - fee, CREATOR),
            ],
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Timelock"),
            "locked item transferred: {error:?}"
        );
        // Listing a locked item is rejected at definition construction.
        let error = ListingDefinition::from_item(
            locked,
            state.state.slot(23).creation_id(),
            ASK,
            HEIGHT + 100,
            HEIGHT,
        )
        .unwrap_err();
        assert!(
            format!("{error:?}").contains("Height"),
            "locked item listed"
        );
        // The recursive circuit enforces the same input timelock:
        // validation.rs binds height >= unlock_height for item inputs.
    }

    #[test]
    fn worst_case_fees_stay_within_settlement_and_refund_bounds() {
        use noid_chain::consensus::fees::fee_breakdown;
        for log_slots in [8u32, 24, 32] {
            let full = 1u64 << log_slots;
            let pressured = full - full / 10; // >= 90% occupancy: multiplier 8
            for count in 1..=MAX_ROUND_REQUESTS {
                let n = count as u64;
                let payouts = n + 2; // seller, item, one refund per request
                let fee = fee_breakdown(n + 1, payouts, pressured, log_slots).required_total;
                assert!(
                    fee <= n * NFT_MAX_SETTLEMENT_FEE_PER_REQUEST,
                    "settlement fee {fee} exceeds per-request cap at count {count}"
                );
                assert!(
                    fee_breakdown(1, 1, pressured, log_slots).required_total < NFT_FEE_CREDIT,
                    "purchase refund fee exceeds credit"
                );
                assert!(
                    fee_breakdown(1, 2, pressured, log_slots).required_total < NFT_FEE_CREDIT,
                    "recovery fee exceeds credit"
                );
            }
        }
    }
}
