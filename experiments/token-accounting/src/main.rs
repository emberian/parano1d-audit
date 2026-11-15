// Adversarial audit PoC: token launch, backing, redemption and cross-asset
// inventory accounting on the parano1d ("noid") L1, pinned commit
// 52938433a30b185ab007afaa15166a632cf2e82e.
//
// Everything below drives the REAL local transition APIs that the sequential
// consensus interpreter calls (noid_chain::state::native_assets::*): no mock
// state machine, no reimplementation of the checks under test.

use noid_chain::state::native_assets::launch::{
    apply_block_launch_registrations, apply_launch_settlement, apply_owned_launch_creation,
    apply_owned_launch_request, ParentLaunch,
};
use noid_chain::state::native_assets::market::liquidity::MarketFeeContext;
use noid_chain::state::native_assets::market::{apply_claim_payout, FundedClaimRef};
use noid_chain::state::native_assets::{apply_native_transition, native_chain_id};
use noid_chain::{ChainState, SlotValue};
use noid_poseidon2b::primitives::Address;
use noid_tx::asset_backing::{BackingState, TokenDenomination};
use noid_tx::native_assets::launch::queue::LaunchRequestRef;
use noid_tx::native_assets::launch::sequential::TradeState;
use noid_tx::native_assets::launch::{
    curve_cash_for_supply, LaunchDefinition, LaunchOpening, LaunchRequest, LaunchSide,
};
use noid_tx::native_assets::market::{
    MarketOpening, PoolState, MARKET_REQUEST_FEE_CREDIT, NOID_ASSET,
};
use noid_tx::native_assets::profile;
use noid_tx::native_assets::transition::{
    check_native_transition, NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
};
use noid_tx::native_assets::{
    native_creation_id, native_sequence, AssetId, FungibleDefinition, GenesisAnchor, IssuanceRoute,
    NativeOpening, TokenBalance, TokenReserve, NATIVE_SEQUENCE_MAX, TOKEN_DECIMALS,
};
use noid_tx::{TxInput, TxOutput};
use std::collections::BTreeSet;

include!(concat!(env!("OUT_DIR"), "/pinned_verification.rs"));

const HEIGHT: u64 = 300_000; // > MAINNET_V2_1_ACTIVATION_HEIGHT (279_370)
const SUPPLY: u64 = 100_000 * 1_000_000; // 100_000 whole tokens, the launch minimum
const CREATOR_PURCHASE: u64 = 10_000_000_000; // 10_000 whole tokens

fn addr(b: u8) -> Address {
    Address([b; 32])
}

fn raw_input(slot: u32, amount: u64, creation_id: u64, owner: Address) -> NativeInputClaim {
    NativeInputClaim {
        input: TxInput {
            slot_index: slot,
            amount,
            creation_id,
        },
        root: owner,
        opening: None,
    }
}

fn native_out(opening: NativeOpening, slot: u32) -> NativeOutputClaim {
    NativeOutputClaim {
        output: TxOutput {
            slot_index: slot,
            amount: opening.amount_micronoid(),
            owner: opening.root(),
        },
        opening: Some(opening),
    }
}

fn raw_out(slot: u32, amount: u64, owner: Address) -> NativeOutputClaim {
    NativeOutputClaim {
        output: TxOutput {
            slot_index: slot,
            amount,
            owner,
        },
        opening: None,
    }
}

fn required_fee(state: &ChainState, inputs: usize, outputs: usize) -> u64 {
    MarketFeeContext::from_state(state)
        .required(inputs, outputs)
        .unwrap()
}

fn context(_state: &ChainState, height: u64, authority: Address, fee: u64) -> NativeContext {
    NativeContext {
        chain: native_chain_id(),
        height,
        authority,
        fee,
    }
}

struct Slots {
    next: u32,
}

impl Slots {
    fn take(&mut self) -> u32 {
        self.next += 1;
        self.next
    }
}

/// One fully applied launch creation, with everything needed to keep auditing
/// the emitted slots.
struct Launch {
    token: FungibleDefinition,
    definition: LaunchDefinition,
    reserve: TokenReserve,
    minimum: u64,
    control_slot: u32,
    control_cid: u64,
    control: TradeState,
    reserve_slot: u32,
    reserve_cid: u64,
    def_slot: u32,
    def_cid: u64,
    balance: Option<(u32, u64, TokenBalance)>,
    paid_micronoid: u64,
    fee: u64,
}

#[allow(clippy::too_many_arguments)]
fn create_launch(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
    purchase: u64,
    metadata: [u8; 32],
) -> Result<Launch, String> {
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner: authority,
        input: TxInput {
            slot_index: funding.0,
            amount: funding.1,
            creation_id: funding.2,
        },
    };
    let token =
        FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, metadata)
            .map_err(|e| format!("token genesis: {e:?}"))?;
    let definition = LaunchDefinition::from_genesis(&anchor, token)
        .map_err(|e| format!("launch genesis: {e:?}"))?;
    let minimum = TokenDenomination::new(TOKEN_DECIMALS)
        .and_then(|d| d.minimum_backing(supply))
        .map_err(|e| format!("minimum: {e:?}"))?;
    let reserve =
        TokenReserve::from_definition(&token, minimum).map_err(|e| format!("reserve: {e:?}"))?;
    let initial = definition
        .after_initial_purchase(purchase)
        .map_err(|e| format!("initial state: {e:?}"))?;
    let cash = definition.curve_cash(purchase).map_err(|e| e.to_string())?;

    let inputs = vec![raw_input(funding.0, funding.1, funding.2, authority)];
    let mut outputs = vec![
        native_out(NativeOpening::FungibleDefinition(token), slots.take()),
        native_out(NativeOpening::TokenReserve(reserve), slots.take()),
    ];
    let control_out = match initial {
        TradeState::Curve(s) => {
            native_out(NativeOpening::Launch(LaunchOpening::State(s)), slots.take())
        }
        TradeState::Market(p) => {
            native_out(NativeOpening::Market(MarketOpening::Pool(p)), slots.take())
        }
    };
    outputs.push(control_out);
    if purchase > 0 {
        outputs.push(native_out(
            NativeOpening::TokenBalance(TokenBalance {
                asset: token.asset,
                owner: authority,
                units: purchase,
                unlock_height: 0,
            }),
            slots.take(),
        ));
    }
    let fee = required_fee(state, inputs.len(), outputs.len() + 1);
    let change = funding
        .1
        .checked_sub(fee + minimum + cash)
        .ok_or_else(|| "funding too small".to_string())?;
    if change > 0 {
        outputs.push(raw_out(slots.take(), change, authority));
    }
    let before = state.circulating_supply_micronoid;
    let effects = apply_owned_launch_creation(
        state,
        &context(state, HEIGHT, authority, fee),
        token,
        definition,
        0,
        &inputs,
        &outputs,
        MarketFeeContext::from_state(state),
    )
    .map_err(|e| format!("apply: {e:?}"))?;
    assert_eq!(
        state.circulating_supply_micronoid,
        before - u128::from(fee),
        "creation must burn exactly the fee"
    );
    let ids: Vec<(u32, u64)> = effects
        .outputs
        .iter()
        .map(|(slot, id, _)| (*slot, *id))
        .collect();
    let mut balance = None;
    for (slot, id, claim) in &effects.outputs {
        if let Some(NativeOpening::TokenBalance(b)) = claim.opening {
            balance = Some((*slot, *id, b));
        }
    }
    Ok(Launch {
        token,
        definition,
        reserve,
        minimum,
        control_slot: ids[2].0,
        control_cid: ids[2].1,
        control: initial,
        reserve_slot: ids[1].0,
        reserve_cid: ids[1].1,
        def_slot: ids[0].0,
        def_cid: ids[0].1,
        balance,
        paid_micronoid: minimum + cash,
        fee,
    })
}

fn parent_of(launch: &Launch) -> ParentLaunch {
    let TradeState::Curve(state) = launch.control else {
        panic!("parent_of needs a curve-phase launch");
    };
    ParentLaunch {
        slot_index: launch.control_slot,
        creation_id: launch.control_cid,
        definition: launch.definition,
        launch: state,
    }
}

#[allow(clippy::too_many_arguments)]
fn buy_request(
    state: &mut ChainState,
    parent: &ParentLaunch,
    slots: &mut Slots,
    buyer: Address,
    funding: (u32, u64, u64),
    quantity: u64,
    noid_limit: u64,
    height: u64,
) -> Result<(u32, u64, LaunchRequest), String> {
    let round = parent.launch.round_id(height).map_err(|e| e.to_string())?;
    let request = LaunchRequest {
        round,
        asset: parent.definition.asset,
        owner: buyer,
        recipient: buyer,
        quantity,
        noid_limit,
        registered_height: height,
        side: LaunchSide::Buy,
        profile: 1,
    };
    let inputs = vec![raw_input(funding.0, funding.1, funding.2, buyer)];
    let mut outputs = vec![native_out(
        NativeOpening::Launch(LaunchOpening::Request(request)),
        slots.take(),
    )];
    let fee = required_fee(state, inputs.len(), outputs.len() + 1);
    let escrow = MARKET_REQUEST_FEE_CREDIT + noid_limit;
    let change = funding
        .1
        .checked_sub(fee + escrow)
        .ok_or_else(|| "funding too small".to_string())?;
    if change > 0 {
        outputs.push(raw_out(slots.take(), change, buyer));
    }
    let effects = apply_owned_launch_request(
        state,
        &context(state, height, buyer, fee),
        *parent,
        &inputs,
        &outputs,
        MarketFeeContext::from_state(state),
    )
    .map_err(|e| format!("apply: {e:?}"))?;
    let (slot, id, _) = effects.outputs[0];
    Ok((slot, id, request))
}

fn verify_branch(name: &str, cids: &[u64], state: &ChainState) {
    let mut seen = BTreeSet::new();
    for cid in cids {
        assert!(
            native_sequence(*cid).is_ok(),
            "{name}: created ids must be native-tagged"
        );
        assert!(
            seen.insert(*cid),
            "{name}: created ids must be distinct within one chain branch"
        );
    }
    let max_sequence = native_sequence(*cids.iter().max().unwrap()).unwrap();
    assert!(
        state.alloc_counter >= max_sequence,
        "{name}: every allocated sequence must be <= alloc_counter"
    );
    println!(
        "CONTROL creation ids [{name}]: {} native ids, all distinct, max sequence {} <= alloc_counter {}",
        cids.len(),
        max_sequence,
        state.alloc_counter
    );
}

fn main() {
    println!("pinned_commit={PIN} pinned_files_verified={PINNED_FILES_VERIFIED}");
    println!("chain_id={:02x?}", native_chain_id());
    println!("v2_1_active_at_height={}", HEIGHT);

    let creator = addr(0xA1);
    let buyer = addr(0xB2);
    let thief = addr(0xC3);

    // Genesis fixture: five independently owned raw NOID UTXOs.
    let sparse = [
        (
            10,
            SlotValue::with_owner_fields(10_000_000_000, 1, creator.as_fields()),
        ),
        (
            11,
            SlotValue::with_owner_fields(2_000_000_000, 2, buyer.as_fields()),
        ),
        (
            12,
            SlotValue::with_owner_fields(1_000_000, 3, buyer.as_fields()),
        ),
        (
            13,
            SlotValue::with_owner_fields(2_000_000_000, 4, buyer.as_fields()),
        ),
        (
            14,
            SlotValue::with_owner_fields(10_000_000_000, 5, creator.as_fields()),
        ),
        (
            15,
            SlotValue::with_owner_fields(1_000_000, 6, creator.as_fields()),
        ),
        (
            16,
            SlotValue::with_owner_fields(2_000_000_000, 7, creator.as_fields()),
        ),
    ];
    let mut state = ChainState::from_sparse_utxos(12, &sparse, 7).unwrap();
    let mut slots = Slots { next: 20 };
    let mut all_native_cids: Vec<u64> = Vec::new();

    // ------------------------------------------------------------------
    // S1 CONTROL: fixed-supply launch issuance at the exact minimum backing
    // ------------------------------------------------------------------
    let launch = create_launch(
        &mut state,
        &mut slots,
        creator,
        (10, 10_000_000_000, 1),
        SUPPLY,
        CREATOR_PURCHASE,
        [0x77; 32],
    )
    .expect("control: launch creation is accepted");
    println!(
        "CONTROL launch created: supply={SUPPLY} minimum_backing={} curve_cash(purchase)={} creator_paid={} fee={}",
        launch.minimum,
        launch.definition.curve_cash(CREATOR_PURCHASE).unwrap(),
        launch.paid_micronoid,
        launch.fee
    );
    assert_eq!(launch.minimum, SUPPLY / 1_000, "minimum backing is s/1000");
    assert_eq!(
        launch.reserve.outstanding_units, SUPPLY,
        "reserve starts with the full fixed supply outstanding"
    );
    assert_eq!(
        launch.reserve.initial_supply, SUPPLY,
        "initial supply is fixed in the reserve opening"
    );

    // Root-vs-raw-amount binding actually written into State (N4).
    let reserve_opening = NativeOpening::TokenReserve(launch.reserve);
    let written = state.state.slot(launch.reserve_slot);
    assert_eq!(
        written,
        SlotValue::with_owner_fields(
            reserve_opening.amount_micronoid(),
            launch.reserve_cid,
            reserve_opening.root().as_fields()
        ),
        "the raw slot must store (amount, native creation id, opening root) exactly"
    );
    println!(
        "CONTROL slot binding: reserve slot stores amount={} cid={} root==opening.root()",
        reserve_opening.amount_micronoid(),
        launch.reserve_cid
    );

    let TradeState::Curve(curve0) = launch.control else {
        panic!("control launch must start on the curve");
    };
    assert_eq!(curve0.inventory_units, SUPPLY - CREATOR_PURCHASE);
    assert_eq!(
        curve0.market_noid,
        launch.definition.curve_cash(CREATOR_PURCHASE).unwrap()
    );
    let balance0 = launch.balance.expect("creator purchased tokens");
    assert_eq!(balance0.2.units, CREATOR_PURCHASE);
    println!(
        "CONTROL creator purchase: {} units for {} microNOID of curve cash",
        CREATOR_PURCHASE,
        launch.definition.curve_cash(CREATOR_PURCHASE).unwrap()
    );

    // ------------------------------------------------------------------
    // S2 REJECTS: issuance attacks (each on a pristine clone)
    // ------------------------------------------------------------------
    let pristine = ChainState::from_sparse_utxos(12, &sparse, 7).unwrap();
    let reject = |name: &str, err: String| {
        println!("REJECT {name}: {err}");
    };

    // A1: underfund the reserve by one microNOID.
    let mut s = pristine.clone();
    match underfunded_creation(
        &mut s,
        &mut Slots { next: 200 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
    ) {
        Ok(_) => panic!("A1 underfunded reserve must be rejected"),
        Err(e) => reject("A1 underfunded reserve (minimum-1)", e),
    }

    // A2: overfund the reserve by one microNOID.
    let mut s = pristine.clone();
    match overfunded_creation(
        &mut s,
        &mut Slots { next: 200 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
    ) {
        Ok(_) => panic!("A2 overfunded reserve must be rejected"),
        Err(e) => reject("A2 overfunded reserve (minimum+1)", e),
    }

    // A3: creator purchase without paying the curve cash.
    let mut s = pristine.clone();
    match purchase_without_cash(
        &mut s,
        &mut Slots { next: 200 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
    ) {
        Ok(_) => panic!("A3 unpaid purchase must be rejected"),
        Err(e) => reject("A3 creator purchase with market_noid=0 (cash not paid)", e),
    }

    // A4: two purchase balance outputs (split purchase to double count).
    let mut s = pristine.clone();
    match split_purchase(
        &mut s,
        &mut Slots { next: 200 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
    ) {
        Ok(_) => panic!("A4 split purchase must be rejected"),
        Err(e) => reject("A4 two TokenBalance purchase outputs", e),
    }

    // A5: purchase more than the 60% sale allocation.
    let mut s = pristine.clone();
    let sale = SUPPLY / 5 * 3;
    match create_launch(
        &mut s,
        &mut Slots { next: 200 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
        sale + 1,
        [0x77; 32],
    ) {
        Ok(_) => panic!("A5 oversize purchase must be rejected"),
        Err(e) => reject("A5 purchase of sale_units+1", e),
    }

    // A6: purchase output owned by a different address than the funding authority.
    let mut s = pristine.clone();
    match stolen_purchase(
        &mut s,
        &mut Slots { next: 200 },
        creator,
        thief,
        (14, 10_000_000_000, 5),
        SUPPLY,
    ) {
        Ok(_) => panic!("A6 stolen purchase must be rejected"),
        Err(e) => reject("A6 purchase balance owned by non-authority", e),
    }

    // A7: identical second launch from the SAME funding input (same asset id).
    let mut s = pristine.clone();
    let first = create_launch(
        &mut s,
        &mut Slots { next: 200 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
        0,
        [0x88; 32],
    )
    .expect("first launch from funding input 14 is accepted");
    let second = create_launch(
        &mut s,
        &mut Slots { next: 300 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
        0,
        [0x88; 32],
    );
    match second {
        Ok(_) => panic!("A7 double-spend of the launch funding input must be rejected"),
        Err(e) => {
            assert_eq!(
                first.token.asset,
                relaunch_asset_id(creator, (14, 10_000_000_000, 5), SUPPLY, [0x88; 32])
            );
            println!(
                "REJECT A7 second identical launch (same asset id {:02x?}) from the same funding input: {e}",
                first.token.asset.0[..4].to_vec()
            );
        }
    }

    // A8: standalone issuance is unreachable in two independent ways.
    assert!(!NativeOperation::CreateToken { funding_input: 0 }.enabled());
    assert!(!NativeOperation::TopUpReserve {
        asset: AssetId([9; 32])
    }
    .enabled());
    assert!(
        !profile::operation_enabled(32),
        "BootstrapPool is profile-disabled"
    );
    assert!(
        !profile::operation_enabled(34),
        "DepositLiquidity is profile-disabled"
    );
    println!("REJECT A8 standalone CreateToken/TopUpReserve/BootstrapPool/liquidity: operation_enabled=false (profile gate)");
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner: creator,
        input: TxInput {
            slot_index: 99,
            amount: 1_000,
            creation_id: 1,
        },
    };
    match FungibleDefinition::from_genesis(&anchor, SUPPLY, IssuanceRoute::Standalone, [0x77; 32]) {
        Ok(_) => panic!("standalone route must not validate"),
        Err(e) => println!("REJECT A8b FungibleDefinition route=Standalone: {e:?} (opening validation forces Launchpad)"),
    }

    // ------------------------------------------------------------------
    // S3: backed redemption over the real transition API
    // ------------------------------------------------------------------
    // Move creator purchase tokens to the buyer via TransferToken for a
    // multi-holder fixture, then redeem from the buyer side.
    let mut s = pristine.clone();
    let launch_b = create_launch(
        &mut s,
        &mut Slots { next: 400 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
        CREATOR_PURCHASE,
        [0x99; 32],
    )
    .unwrap();
    let mut s2 = s.clone();
    match redeem(
        &mut s2,
        &mut Slots { next: 500 },
        buyer,
        &launch_b,
        &[(
            launch_b.balance.unwrap().0,
            launch_b.balance.unwrap().1,
            launch_b.balance.unwrap().2,
        )],
        1_000_000_000,
        (12, 1_000_000, 3),
    ) {
        Ok(r) => panic!("R6 redemption of a balance owned by someone else must fail, got {r:?}"),
        Err(e) => reject("R6 redeem with balance owner != authority", e),
    }

    // R1 control: the creator redeems part of its own purchase.
    let expected_payout = launch_b.reserve.reserve_micronoid * 1_000_000_000 / SUPPLY;
    let redemption = redeem(
        &mut s,
        &mut Slots { next: 500 },
        creator,
        &launch_b,
        &[(
            launch_b.balance.unwrap().0,
            launch_b.balance.unwrap().1,
            launch_b.balance.unwrap().2,
        )],
        1_000_000_000,
        (15, 1_000_000, 6),
    )
    .expect("control: redemption of own balance is accepted");
    println!(
        "CONTROL redemption: burned=1000000000 payout={}(exact floor) new_reserve={} new_outstanding={}",
        redemption.payout, redemption.next.reserve_micronoid, redemption.next.outstanding_units
    );
    assert_eq!(redemption.payout, expected_payout);
    assert_eq!(
        launch_b.reserve.reserve_micronoid,
        redemption.next.reserve_micronoid + redemption.payout
    );
    assert_eq!(redemption.next.outstanding_units, SUPPLY - 1_000_000_000);

    // R2: zero-burn redemption (units in == units out).
    let mut s3 = pristine.clone();
    let launch_c = create_launch(
        &mut s3,
        &mut Slots { next: 400 },
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
        CREATOR_PURCHASE,
        [0x99; 32],
    )
    .unwrap();
    let mut s4 = s3.clone();
    match redeem_zero_burn(
        &mut s4,
        &mut Slots { next: 500 },
        creator,
        &launch_c,
        (15, 1_000_000, 6),
    ) {
        Ok(_) => panic!("R2 zero burn must be rejected"),
        Err(e) => reject("R2 redemption burning 0 units", e),
    }

    // R3: keep NOID conservation but understate the successor outstanding
    // supply by one unit (deflated backing bookkeeping).
    let mut s5 = s3.clone();
    match redeem_reserve_skim(
        &mut s5,
        &mut Slots { next: 500 },
        creator,
        &launch_c,
        (15, 1_000_000, 6),
    ) {
        Ok(_) => panic!("R3 supply understatement must be rejected"),
        Err(e) => reject(
            "R3 successor reserve with outstanding_units-1 (conservation-neutral)",
            e,
        ),
    }

    // R3b: pocket one microNOID by deflating the successor reserve and paying
    // it out raw (payout+1, reserve_new-1).
    let mut s6 = s3.clone();
    match redeem_payout_skim(
        &mut s6,
        &mut Slots { next: 500 },
        creator,
        &launch_c,
        (15, 1_000_000, 6),
    ) {
        Ok(_) => panic!("R3b payout skim must be rejected"),
        Err(e) => reject(
            "R3b successor reserve deflated by -1, raw payout +1 (conservation-neutral)",
            e,
        ),
    }

    // R4: a TokenBalance carrier that carries raw NOID (root/amount confusion).
    let bad_balance = TokenBalance {
        asset: launch_c.token.asset,
        owner: creator,
        units: 1_000,
        unlock_height: 0,
    };
    match NativeOpening::TokenBalance(bad_balance).check_output(&TxOutput {
        slot_index: 1,
        amount: 7,
        owner: NativeOpening::TokenBalance(bad_balance).root(),
    }) {
        Ok(()) => panic!("R4 must fail"),
        Err(e) => println!("REJECT R4 TokenBalance carrier with raw amount 7: {e:?} (amount_micronoid()==0 for token carriers)"),
    }

    // R5: full drain empties reserve and supply exactly; residue guard.
    let denomination = TokenDenomination::new(TOKEN_DECIMALS).unwrap();
    let full = BackingState::new(denomination, 1_000_000, 1_000_000_000).unwrap();
    let drain = full.quote_redemption(1_000_000_000).unwrap();
    assert_eq!(drain.payout_micronoid, 1_000_000);
    assert_eq!(drain.next.reserve_micronoid(), 0);
    assert_eq!(drain.next.supply_units(), 0);
    assert_eq!(
        BackingState::new(denomination, 1, 0),
        Err(noid_tx::asset_backing::BackingError::EmptySupplyHasReserve)
    );
    println!("CONTROL R5 full drain pays the whole reserve; supply-0-with-reserve rejected: EmptySupplyHasReserve");

    // ------------------------------------------------------------------
    // S4: full round lifecycle -> settlement -> payout -> redemption
    // ------------------------------------------------------------------
    let mut s = pristine.clone();
    let mut sl = Slots { next: 600 };
    let launch_g = create_launch(
        &mut s,
        &mut sl,
        creator,
        (10, 10_000_000_000, 1),
        SUPPLY,
        CREATOR_PURCHASE,
        [0xAA; 32],
    )
    .expect("lifecycle launch");
    all_native_cids.extend([launch_g.def_cid, launch_g.reserve_cid, launch_g.control_cid]);

    let parent = parent_of(&launch_g);
    let quantity = 10_000_000_000u64;
    // Two buys of `quantity` each; the uniform clearing price after
    // CREATOR_PURCHASE sold for total delta 2*quantity is
    // numerator = 3s + 10*sold + 5*2*quantity, denominator = 900s.
    let price_num = 3 * SUPPLY + 10 * CREATOR_PURCHASE + 5 * 2 * quantity;
    let price_den = 900 * SUPPLY;
    let cost =
        (u128::from(quantity) * u128::from(price_num)).div_ceil(u128::from(price_den)) as u64;
    let limit = cost + 400;
    let (req_slot, req_cid, request) = buy_request(
        &mut s,
        &parent,
        &mut sl,
        buyer,
        (11, 2_000_000_000, 2),
        quantity,
        limit,
        HEIGHT + 1,
    )
    .expect("buyer request accepted");
    let (creq_slot, creq_cid, creq) = buy_request(
        &mut s,
        &parent,
        &mut sl,
        creator,
        (16, 2_000_000_000, 7),
        quantity,
        limit,
        HEIGHT + 1,
    )
    .expect("creator request accepted");
    all_native_cids.push(req_cid);
    all_native_cids.push(creq_cid);
    println!("CONTROL two buyer requests funded: escrow={} each (fee_credit {MARKET_REQUEST_FEE_CREDIT} + limit {limit}) cost_at_clearing={cost} per request", MARKET_REQUEST_FEE_CREDIT + limit);

    let request_ref = LaunchRequestRef {
        slot_index: req_slot,
        creation_id: req_cid,
        request,
    };
    let creator_ref = LaunchRequestRef {
        slot_index: creq_slot,
        creation_id: creq_cid,
        request: creq,
    };
    let registration = apply_block_launch_registrations(
        &mut s,
        HEIGHT + 1,
        &[parent],
        &[request_ref, creator_ref],
        &[sl.take()],
    )
    .expect("registration accepted");
    let (frozen_slot, frozen_cid, frozen) = registration.controls[0];
    all_native_cids.push(frozen_cid);
    assert_eq!(
        frozen.phase,
        noid_tx::native_assets::market::PoolPhase::Frozen
    );
    println!(
        "CONTROL round registered at height {}: queue_root committed, timeout={}",
        HEIGHT + 1,
        frozen.timeout_height
    );

    // Rollback probe: settle with a strict subset of the registered set.
    let root_before = s.cached_state_root();
    let mut s_bad = s.clone();
    let bad_parent = ParentLaunch {
        slot_index: frozen_slot,
        creation_id: frozen_cid,
        definition: launch_g.definition,
        launch: frozen,
    };
    let fees_bad = MarketFeeContext::from_state(&s_bad);
    match apply_launch_settlement(
        &mut s_bad,
        HEIGHT + 2,
        bad_parent,
        &[request_ref],
        &[sl.take(), sl.take()],
        fees_bad,
    ) {
        Ok(_) => panic!("settlement with a strict subset must be rejected"),
        Err(e) => println!("REJECT M2 settlement of a strict subset of the registered round: {e:?} (committed queue root cannot be reproduced); state_root_unchanged={}",
            s_bad.cached_state_root() == root_before),
    }
    assert_eq!(
        s_bad.cached_state_root(),
        root_before,
        "failed settlement must not mutate state"
    );

    let fees_good = MarketFeeContext::from_state(&s);
    let settlement = apply_launch_settlement(
        &mut s,
        HEIGHT + 2,
        bad_parent,
        &[request_ref, creator_ref],
        &[sl.take(), sl.take(), sl.take()],
        fees_good,
    )
    .expect("settlement accepted");
    all_native_cids.push(settlement.control.1);
    for (_, id, _) in &settlement.claims {
        all_native_cids.push(*id);
    }
    let sold_now = SUPPLY
        - match settlement.settlement.outcome() {
            noid_tx::native_assets::launch::clearing::LaunchOutcome::Continue(st) => {
                st.inventory_units
            }
            _ => unreachable!(),
        };
    println!(
        "CONTROL settlement: sold={sold_now} fee={} rounding_fee={} claims={}",
        settlement.settlement.fee(),
        settlement.settlement.rounding_fee(),
        settlement.claims.len()
    );
    assert_eq!(sold_now, CREATOR_PURCHASE + 2 * quantity);

    // Claim payouts materialize the SwapClaims into real slots. Claims come
    // back in committed queue-key order, so identify them by refund owner.
    let mut buyer_tokens = 0u64;
    let mut buyer_noid = 0u64;
    let mut creator_claim_tokens = 0u64;
    let mut buyer_balance_slot = (0u32, 0u64);
    for &(claim_slot, claim_cid, claim) in settlement.claims.iter() {
        let is_buyer = claim.refund_owner == buyer;
        let payout = apply_claim_payout(
            &mut s,
            HEIGHT + 3,
            FundedClaimRef {
                slot_index: claim_slot,
                creation_id: claim_cid,
                claim,
            },
            &[sl.take(), sl.take()],
        )
        .expect("claim payout accepted");
        for &(slot, id, p) in &payout.payouts {
            if p.asset == launch_g.token.asset {
                if is_buyer {
                    buyer_tokens += p.units;
                    buyer_balance_slot = (slot, id);
                } else {
                    creator_claim_tokens += p.units;
                }
            } else if is_buyer {
                buyer_noid += p.units;
            }
        }
    }
    assert_eq!(buyer_tokens, quantity);
    assert_eq!(creator_claim_tokens, quantity);
    println!("CONTROL claim payouts: buyer received {buyer_tokens} token units and {buyer_noid} microNOID; creator received {creator_claim_tokens} token units");

    // Redeem a slice of the market-issued tokens against the reserve.
    // The reserve is untouched by the round; redeem half the buyer tokens.
    let redeem_balances = [(
        buyer_balance_slot.0,
        buyer_balance_slot.1,
        TokenBalance {
            asset: launch_g.token.asset,
            owner: buyer,
            units: buyer_tokens,
            unlock_height: 0,
        },
    )];
    let expected = launch_g.reserve.reserve_micronoid * (buyer_tokens / 2) / SUPPLY;
    let redemption = redeem(
        &mut s,
        &mut sl,
        buyer,
        &launch_g,
        &redeem_balances,
        buyer_tokens / 2,
        (12, 1_000_000, 3),
    )
    .expect("redemption of market-issued tokens");
    assert_eq!(redemption.payout, expected);
    println!(
        "CONTROL post-graduation-path redemption of round-issued tokens: burned={} payout={} (exact floor)",
        buyer_tokens / 2, redemption.payout
    );

    // Global token conservation across the whole lifecycle so far:
    // inventory + creator balances + buyer balance + burned == SUPPLY.
    let TradeState::Curve(after_round) = settlement_control(&settlement) else {
        unreachable!()
    };
    let burned = buyer_tokens / 2;
    let total = after_round.inventory_units
        + CREATOR_PURCHASE
        + creator_claim_tokens
        + (buyer_tokens - burned)
        + burned;
    assert_eq!(total, SUPPLY);
    println!("CONTROL global token conservation: inventory {} + creator purchase {} + creator round {} + buyer {} + burned {} == fixed supply {SUPPLY}",
        after_round.inventory_units, CREATOR_PURCHASE, creator_claim_tokens, buyer_tokens - burned, burned);
    verify_branch("lifecycle chain", &all_native_cids, &s);

    // S4c: curve SELL round against the post-settlement parent. The creator
    // sells part of its purchase back; inventory must grow and curve cash
    // must fall by exactly the uniform-clearing proceeds.
    let sell_state = match settlement.control.2 {
        NativeOpening::Launch(LaunchOpening::State(st)) => st,
        _ => unreachable!("post-settlement control must be a curve state"),
    };
    let sell_parent = ParentLaunch {
        slot_index: settlement.control.0,
        creation_id: settlement.control.1,
        definition: launch_g.definition,
        launch: sell_state,
    };
    let sell_quantity = 2_000_000_000u64;
    let sell_request = LaunchRequest {
        round: sell_parent.launch.round_id(HEIGHT + 4).unwrap(),
        asset: launch_g.token.asset,
        owner: creator,
        recipient: creator,
        quantity: sell_quantity,
        noid_limit: 0,
        registered_height: HEIGHT + 4,
        side: LaunchSide::Sell,
        profile: 1,
    };
    let sell_balance = launch_g.balance.unwrap();
    let sell_balance_opening = NativeOpening::TokenBalance(sell_balance.2);
    let sell_inputs = vec![
        NativeInputClaim {
            input: TxInput {
                slot_index: sell_balance.0,
                amount: 0,
                creation_id: sell_balance.1,
            },
            root: sell_balance_opening.root(),
            opening: Some(sell_balance_opening),
        },
        raw_input(15, 1_000_000, 6, creator),
    ];
    let mut sell_outputs = vec![
        native_out(
            NativeOpening::Launch(LaunchOpening::Request(sell_request)),
            sl.take(),
        ),
        native_out(
            NativeOpening::TokenBalance(TokenBalance {
                asset: launch_g.token.asset,
                owner: creator,
                units: CREATOR_PURCHASE - sell_quantity,
                unlock_height: 0,
            }),
            sl.take(),
        ),
    ];
    let sell_fee = required_fee(&s, sell_inputs.len(), sell_outputs.len() + 1);
    let sell_change = 1_000_000 - sell_fee - MARKET_REQUEST_FEE_CREDIT;
    sell_outputs.push(raw_out(sl.take(), sell_change, creator));
    let sell_ctx = context(&s, HEIGHT + 4, creator, sell_fee);
    let sell_fees_ctx = MarketFeeContext::from_state(&s);
    let sell_effects = apply_owned_launch_request(
        &mut s,
        &sell_ctx,
        sell_parent,
        &sell_inputs,
        &sell_outputs,
        sell_fees_ctx,
    )
    .expect("sell request accepted");
    let (sell_req_slot, sell_req_cid) = (sell_effects.outputs[0].0, sell_effects.outputs[0].1);
    let sell_ref = LaunchRequestRef {
        slot_index: sell_req_slot,
        creation_id: sell_req_cid,
        request: sell_request,
    };
    let sell_registration = apply_block_launch_registrations(
        &mut s,
        HEIGHT + 4,
        &[sell_parent],
        &[sell_ref],
        &[sl.take()],
    )
    .expect("sell round registered");
    let (sfrozen_slot, sfrozen_cid, sfrozen) = sell_registration.controls[0];
    let sell_frozen_parent = ParentLaunch {
        slot_index: sfrozen_slot,
        creation_id: sfrozen_cid,
        definition: launch_g.definition,
        launch: sfrozen,
    };
    let sell_fees = MarketFeeContext::from_state(&s);
    let sell_settlement = apply_launch_settlement(
        &mut s,
        HEIGHT + 5,
        sell_frozen_parent,
        &[sell_ref],
        &[sl.take(), sl.take()],
        sell_fees,
    )
    .expect("sell settlement accepted");
    let noid_tx::native_assets::launch::clearing::LaunchOutcome::Continue(sell_after) =
        sell_settlement.settlement.outcome()
    else {
        unreachable!("a partial sell cannot graduate")
    };
    let expected_sold = CREATOR_PURCHASE + 2 * quantity - sell_quantity;
    assert_eq!(
        sell_after.inventory_units,
        SUPPLY - expected_sold,
        "inventory must grow back by the sold quantity"
    );
    assert_eq!(
        sell_after.market_noid,
        launch_g.definition.curve_cash(expected_sold).unwrap(),
        "curve cash must equal the curve integral after the sell"
    );
    let proceeds = sell_settlement.settlement.execution()[0].noid_proceeds;
    println!(
        "CONTROL curve sell round: sold back {sell_quantity} units, inventory grew to {}, market_noid fell to {}, seller proceeds={proceeds} (conserved: prior cash - proceeds == new cash + rounding)",
        sell_after.inventory_units, sell_after.market_noid
    );
    let prior_cash = after_round.market_noid;
    let rounding_sell = prior_cash as i128 - proceeds as i128 - sell_after.market_noid as i128;
    assert!(
        rounding_sell >= -1 && rounding_sell <= 2,
        "sell rounding within slack, got {rounding_sell}"
    );

    // ------------------------------------------------------------------
    // S4b: graduation round (sells out the 60% allocation)
    // ------------------------------------------------------------------
    let mut s = pristine.clone();
    let mut sl = Slots { next: 900 };
    let launch2 = create_launch(
        &mut s,
        &mut sl,
        creator,
        (10, 10_000_000_000, 1),
        SUPPLY,
        0,
        [0xBB; 32],
    )
    .unwrap();
    let mut grad_cids: Vec<u64> = Vec::new();
    grad_cids.extend([launch2.def_cid, launch2.reserve_cid, launch2.control_cid]);
    let parent2 = parent_of(&launch2);
    let sale = SUPPLY / 5 * 3;
    let cap_price_num = 3 * SUPPLY + 5 * sale; // average_price(0, sale)
    let cap_cost =
        (u128::from(sale) * u128::from(cap_price_num)).div_ceil(u128::from(900 * SUPPLY)) as u64;
    assert_eq!(
        cap_cost,
        SUPPLY / 250,
        "cap cost must equal graduation cash exactly"
    );
    let limit2 = cap_cost + 5_000;
    let (req2_slot, req2_cid, request2) = buy_request(
        &mut s,
        &parent2,
        &mut sl,
        buyer,
        (13, 2_000_000_000, 4),
        sale,
        limit2,
        HEIGHT + 1,
    )
    .expect("graduation buy request");
    let request2_ref = LaunchRequestRef {
        slot_index: req2_slot,
        creation_id: req2_cid,
        request: request2,
    };
    let registration2 = apply_block_launch_registrations(
        &mut s,
        HEIGHT + 1,
        &[parent2],
        &[request2_ref],
        &[sl.take()],
    )
    .unwrap();
    let (frozen2_slot, frozen2_cid, frozen2) = registration2.controls[0];
    grad_cids.push(frozen2_cid);
    let parent2f = ParentLaunch {
        slot_index: frozen2_slot,
        creation_id: frozen2_cid,
        definition: launch2.definition,
        launch: frozen2,
    };
    let fees_grad = MarketFeeContext::from_state(&s);
    let settlement2 = apply_launch_settlement(
        &mut s,
        HEIGHT + 2,
        parent2f,
        &[request2_ref],
        &[sl.take(), sl.take()],
        fees_grad,
    )
    .expect("graduation settlement accepted");
    let noid_tx::native_assets::launch::clearing::LaunchOutcome::Graduate(pool) =
        settlement2.settlement.outcome()
    else {
        panic!("expected graduation");
    };
    grad_cids.push(settlement2.control.1);
    println!(
        "CONTROL graduation: pool base_reserve={} quote_reserve={} locked_shares==total_shares={} total_shares=isqrt(base*quote)",
        pool.base_reserve, pool.quote_reserve, pool.locked_shares == pool.total_shares
    );
    assert_eq!(
        pool.base_reserve,
        SUPPLY / 250,
        "pool NOID == curve cash at cap == s/250"
    );
    assert_eq!(
        pool.quote_reserve,
        SUPPLY - sale,
        "pool tokens == the reserved 40% (dex units)"
    );
    assert_eq!(
        pool.total_shares,
        integer_sqrt(u128::from(pool.base_reserve) * u128::from(pool.quote_reserve))
    );
    assert_eq!(
        pool.locked_shares, pool.total_shares,
        "graduation liquidity is permanently locked"
    );
    // NOID conservation across the graduation settlement:
    // escrow + prior curve cash == pool base + fee + claims.
    let (g_slot, g_cid, g_claim) = settlement2.claims[0];
    let escrow = MARKET_REQUEST_FEE_CREDIT + limit2;
    let claim_value = g_claim.noid_value().unwrap();
    assert_eq!(
        0 + escrow,
        pool.base_reserve + settlement2.settlement.fee() + claim_value,
        "graduation NOID conservation"
    );
    let payout2 = apply_claim_payout(
        &mut s,
        HEIGHT + 3,
        FundedClaimRef {
            slot_index: g_slot,
            creation_id: g_cid,
            claim: g_claim,
        },
        &[sl.take(), sl.take()],
    )
    .unwrap();
    let mut grad_tokens = 0u64;
    for (_, _, p) in &payout2.payouts {
        if p.asset == launch2.token.asset {
            grad_tokens += p.units;
        }
    }
    assert_eq!(grad_tokens, sale);
    println!("CONTROL graduation handoff: buyer holds {grad_tokens} (60%), pool holds {} (40%), reserve redeemable {} microNOID; total token units == supply",
        pool.quote_reserve, launch2.reserve.reserve_micronoid);
    assert_eq!(grad_tokens + pool.quote_reserve, SUPPLY);

    // G3: a curve state that already dipped into the DEX-reserved 40%.
    let sale2 = launch2.definition.sale_units();
    let mut dipped = parent2.launch;
    dipped.inventory_units = launch2.definition.dex_units();
    dipped.market_noid = launch2.definition.curve_cash(sale2).unwrap();
    match dipped.check_definition(launch2.definition) {
        Ok(()) => panic!("G3 must fail"),
        Err(e) => println!("REJECT G3 curve state with inventory == dex_units and honest cash (selling the reserved 40%): {e:?}"),
    }
    let mut dipped2 = parent2.launch;
    dipped2.inventory_units = launch2.definition.dex_units() + 1;
    dipped2.market_noid = launch2.definition.curve_cash(sale2 - 1).unwrap();
    assert!(
        dipped2.check_definition(launch2.definition).is_ok(),
        "one unit above dex reserve with honest cash is a legal curve state"
    );
    println!("CONTROL G3b inventory == dex_units+1 with cash==curve_cash(sale-1): accepted (boundary is strict)");

    // G4: settlement with a request bound to a stale round.
    let mut s_bad = s.clone();
    let mut stale = request2;
    stale.round = [7; 32];
    let fees_stale = MarketFeeContext::from_state(&s_bad);
    match apply_launch_settlement(
        &mut s_bad,
        HEIGHT + 2,
        parent2f,
        &[LaunchRequestRef {
            slot_index: req2_slot,
            creation_id: req2_cid,
            request: stale,
        }],
        &[sl.take(), sl.take()],
        fees_stale,
    ) {
        Ok(_) => panic!("stale round settlement must fail"),
        Err(e) => println!("REJECT G4 settlement with a request from a different round: {e:?}"),
    }

    // ------------------------------------------------------------------
    // S5: creation-id discipline
    // ------------------------------------------------------------------
    verify_branch("graduation chain", &grad_cids, &s);
    assert_eq!(
        native_creation_id(0),
        Err(noid_tx::native_assets::NativeAssetError::Provenance)
    );
    assert_eq!(
        native_creation_id(NATIVE_SEQUENCE_MAX + 1),
        Err(noid_tx::native_assets::NativeAssetError::Provenance)
    );
    assert!(
        native_sequence(1 << 63).is_err(),
        "coinbase-tagged ids are not native-sequence ids"
    );
    assert!(
        native_sequence(7).is_err(),
        "raw ids are not native-sequence ids"
    );
    println!("CONTROL creation-id namespaces: sequence 0, overflow, coinbase tag and raw ids all rejected");
    let future = noid_tx::native_assets::financial::NativeParentRef {
        slot_index: 1,
        creation_id: native_creation_id(s.alloc_counter + 1).unwrap(),
        root: Address([1; 32]),
    };
    assert!(future.check_boundary(s.alloc_counter).is_err());
    println!("REJECT I3 parent ref from the future (sequence == alloc_counter+1): Parent");

    // ------------------------------------------------------------------
    // S6: multi-group atomic rollback (registration scan)
    // ------------------------------------------------------------------
    let mut s = pristine.clone();
    let mut sl = Slots { next: 1200 };
    let l1 = create_launch(
        &mut s,
        &mut sl,
        creator,
        (10, 10_000_000_000, 1),
        SUPPLY,
        0,
        [0xD1; 32],
    )
    .unwrap();
    let l2 = create_launch(
        &mut s,
        &mut sl,
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
        0,
        [0xD2; 32],
    )
    .unwrap();
    let p1 = parent_of(&l1);
    let p2 = parent_of(&l2);
    let (r1_slot, r1_cid, r1) = buy_request(
        &mut s,
        &p1,
        &mut sl,
        buyer,
        (11, 2_000_000_000, 2),
        1_000_000_000,
        60_000_000,
        HEIGHT + 1,
    )
    .unwrap();
    let (r2_slot, r2_cid, r2) = buy_request(
        &mut s,
        &p2,
        &mut sl,
        buyer,
        (13, 2_000_000_000, 4),
        1_000_000_000,
        60_000_000,
        HEIGHT + 1,
    )
    .unwrap();
    let ok = apply_block_launch_registrations(
        &mut s,
        HEIGHT + 1,
        &[p1, p2],
        &[
            LaunchRequestRef {
                slot_index: r1_slot,
                creation_id: r1_cid,
                request: r1,
            },
            LaunchRequestRef {
                slot_index: r2_slot,
                creation_id: r2_cid,
                request: r2,
            },
        ],
        &[sl.take(), sl.take()],
    );
    assert!(
        ok.is_ok(),
        "two-group registration is accepted: {:?}",
        ok.err()
    );
    println!("CONTROL M1 two-launch registration scan in one atomic update: accepted");

    let mut s = pristine.clone();
    let mut sl = Slots { next: 1200 };
    let l1 = create_launch(
        &mut s,
        &mut sl,
        creator,
        (10, 10_000_000_000, 1),
        SUPPLY,
        0,
        [0xD1; 32],
    )
    .unwrap();
    let l2 = create_launch(
        &mut s,
        &mut sl,
        creator,
        (14, 10_000_000_000, 5),
        SUPPLY,
        0,
        [0xD2; 32],
    )
    .unwrap();
    let p1 = parent_of(&l1);
    let p2 = parent_of(&l2);
    let (r1_slot, r1_cid, r1) = buy_request(
        &mut s,
        &p1,
        &mut sl,
        buyer,
        (11, 2_000_000_000, 2),
        1_000_000_000,
        60_000_000,
        HEIGHT + 1,
    )
    .unwrap();
    let (r2_slot, r2_cid, r2) = buy_request(
        &mut s,
        &p2,
        &mut sl,
        buyer,
        (13, 2_000_000_000, 4),
        1_000_000_000,
        60_000_000,
        HEIGHT + 1,
    )
    .unwrap();
    let root_before = s.cached_state_root();
    // Same request slot claimed by both rounds' requests -> duplicate slot.
    let mut r2_dup = r2;
    r2_dup.round = r1.round; // wrong round for launch 2
    match apply_block_launch_registrations(
        &mut s,
        HEIGHT + 1,
        &[p1, p2],
        &[
            LaunchRequestRef {
                slot_index: r1_slot,
                creation_id: r1_cid,
                request: r1,
            },
            LaunchRequestRef {
                slot_index: r2_slot,
                creation_id: r2_cid,
                request: r2_dup,
            },
        ],
        &[sl.take(), sl.take()],
    ) {
        Ok(_) => panic!("cross-round registration must be rejected"),
        Err(e) => println!(
            "REJECT M2b cross-round request in a multi-group scan: {e:?}; state_root_unchanged={}",
            s.cached_state_root() == root_before
        ),
    }
    assert_eq!(
        s.cached_state_root(),
        root_before,
        "the whole multi-group scan must roll back"
    );

    // ------------------------------------------------------------------
    // S7: pure transition-level multi-asset confusion
    // ------------------------------------------------------------------
    let asset_a = AssetId([1; 32]);
    let asset_b = AssetId([2; 32]);
    let bal_b = TokenBalance {
        asset: asset_b,
        owner: buyer,
        units: 5,
        unlock_height: 0,
    };
    let opening_b = NativeOpening::TokenBalance(bal_b);
    let claim_b = NativeInputClaim {
        input: TxInput {
            slot_index: 9,
            amount: 0,
            creation_id: native_creation_id(9).unwrap(),
        },
        root: opening_b.root(),
        opening: Some(opening_b),
    };
    match check_native_transition(
        &NativeContext {
            chain: native_chain_id(),
            height: HEIGHT,
            authority: buyer,
            fee: 0,
        },
        NativeOperation::TransferToken { asset: asset_a },
        &[claim_b],
        &[native_out(opening_b, 10)],
    ) {
        Ok(_) => panic!("cross-asset transfer must fail"),
        Err(e) => println!("REJECT M3 TransferToken(asset A) spending a balance of asset B: {e:?}"),
    }
    // RedeemToken with an input reserve of a different asset.
    let reserve_b = TokenReserve {
        asset: asset_b,
        reserve_micronoid: 10,
        outstanding_units: 100,
        initial_supply: 100,
        definition_root: Address([3; 32]),
    };
    let opening_rb = NativeOpening::TokenReserve(reserve_b);
    match check_native_transition(
        &NativeContext {
            chain: native_chain_id(),
            height: HEIGHT,
            authority: buyer,
            fee: 0,
        },
        NativeOperation::RedeemToken { asset: asset_a },
        &[NativeInputClaim {
            input: TxInput {
                slot_index: 9,
                amount: 10,
                creation_id: native_creation_id(9).unwrap(),
            },
            root: opening_rb.root(),
            opening: Some(opening_rb),
        }],
        &[native_out(opening_rb, 10)],
    ) {
        Ok(_) => panic!("cross-asset redemption must fail"),
        Err(e) => {
            println!("REJECT M4 RedeemToken(asset A) consuming the reserve of asset B: {e:?}")
        }
    }

    // ------------------------------------------------------------------
    // S8: property sweep over the whole legal supply range
    // ------------------------------------------------------------------
    let min_supply = 100_000 * 1_000_000u64;
    let max_supply = noid_tx::native_assets::launch::LAUNCH_MAX_SUPPLY;
    let mut checked = 0u32;
    let mut multipliers = Vec::new();
    let mut m = 1u64;
    while min_supply.saturating_mul(m) <= max_supply && multipliers.len() < 24 {
        multipliers.push(min_supply * m);
        m *= 3;
    }
    let max_multiple = max_supply / 1_000_000 * 1_000_000;
    for supply in multipliers.iter().copied().chain([max_multiple]) {
        let sale = supply / 5 * 3;
        let dex = supply - sale;
        let grad_cash = supply / 250;
        assert_eq!(
            supply % 5,
            0,
            "every legal supply is a multiple of 10^6, hence of 5"
        );
        assert_eq!(
            curve_cash_for_supply(supply, sale).unwrap(),
            grad_cash,
            "curve cash at the cap must equal graduation cash exactly"
        );
        assert_eq!(dex, supply - sale);
        let market = noid_tx::native_assets::market::MarketDefinition {
            genesis: [9; 32],
            base: NOID_ASSET,
            quote: AssetId([7; 32]),
            profile: 1,
        };
        let pool = PoolState::bootstrap_permanent(market, grad_cash, dex).unwrap();
        assert_eq!(pool.base_reserve, grad_cash);
        assert_eq!(pool.quote_reserve, dex);
        assert!(pool.total_shares > 1_000);
        // Redemption floor identity across the reserve's life.
        let denom = TokenDenomination::new(TOKEN_DECIMALS).unwrap();
        let reserve0 =
            BackingState::new(denom, denom.minimum_backing(supply).unwrap(), supply).unwrap();
        let half = supply / 2;
        let q = reserve0.quote_redemption(half).unwrap();
        assert_eq!(
            q.payout_micronoid + q.next.reserve_micronoid(),
            denom.minimum_backing(supply).unwrap()
        );
        assert_eq!(q.next.supply_units(), supply - half);
        let q2 = q.next.quote_redemption(supply - half).unwrap();
        assert_eq!(q2.next.reserve_micronoid(), 0);
        assert_eq!(q2.next.supply_units(), 0);
        checked += 1;
    }
    println!("CONTROL property sweep: {checked} supplies from {min_supply} to {max_multiple}: sale=3s/5, curve_cash(3s/5)==s/250==graduation cash, dex=2s/5, redemption drains to exactly zero");

    println!("ALL ASSERTIONS PASSED");
}

fn integer_sqrt(n: u128) -> u64 {
    let mut lo = 0u128;
    let mut hi = 1u128 << 64;
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if mid * mid <= n {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo as u64
}

fn settlement_control(
    settlement: &noid_chain::state::native_assets::launch::LaunchSettlementEffects,
) -> TradeState {
    match settlement.settlement.outcome() {
        noid_tx::native_assets::launch::clearing::LaunchOutcome::Continue(s) => {
            TradeState::Curve(s)
        }
        noid_tx::native_assets::launch::clearing::LaunchOutcome::Graduate(p) => {
            TradeState::Market(p)
        }
    }
}

#[derive(Debug)]
struct Redemption {
    payout: u64,
    next: TokenReserve,
    balance_slot: (u32, u64),
}

#[allow(clippy::too_many_arguments)]
fn redeem(
    state: &mut ChainState,
    slots: &mut Slots,
    holder: Address,
    launch: &Launch,
    balances: &[(u32, u64, TokenBalance)],
    burn: u64,
    fee_funding: (u32, u64, u64),
) -> Result<Redemption, String> {
    let reserve_opening = NativeOpening::TokenReserve(launch.reserve);
    let mut inputs = vec![NativeInputClaim {
        input: TxInput {
            slot_index: launch.reserve_slot,
            amount: launch.reserve.reserve_micronoid,
            creation_id: launch.reserve_cid,
        },
        root: reserve_opening.root(),
        opening: Some(reserve_opening),
    }];
    let mut total_in = 0u64;
    for (slot, cid, balance) in balances {
        let opening = NativeOpening::TokenBalance(*balance);
        total_in = total_in.checked_add(balance.units).ok_or("overflow")?;
        inputs.push(NativeInputClaim {
            input: TxInput {
                slot_index: *slot,
                amount: 0,
                creation_id: *cid,
            },
            root: opening.root(),
            opening: Some(opening),
        });
    }
    inputs.push(raw_input(
        fee_funding.0,
        fee_funding.1,
        fee_funding.2,
        holder,
    ));
    let (next, payout) = launch
        .reserve
        .redeem(burn)
        .map_err(|e| format!("quote: {e:?}"))?;
    let remaining = total_in - burn;
    let mut outputs = vec![native_out(NativeOpening::TokenReserve(next), slots.take())];
    if remaining > 0 {
        outputs.push(native_out(
            NativeOpening::TokenBalance(TokenBalance {
                asset: launch.token.asset,
                owner: balances[0].2.owner,
                units: remaining,
                unlock_height: 0,
            }),
            slots.take(),
        ));
    }
    outputs.push(raw_out(slots.take(), payout, holder));
    let fee = required_fee(state, inputs.len(), outputs.len() + 1);
    let change = fee_funding
        .1
        .checked_sub(fee)
        .ok_or("fee funding too small")?;
    if change > 0 {
        outputs.push(raw_out(slots.take(), change, holder));
    }
    let before = state.circulating_supply_micronoid;
    if std::env::var_os("TA_DEBUG").is_some() {
        for c in &inputs {
            let actual = state.state.slot(c.input.slot_index);
            let expected = SlotValue::with_owner_fields(
                c.input.amount,
                c.input.creation_id,
                c.root.as_fields(),
            );
            eprintln!(
                "DEBUG input slot={} matches={} want(amount={},cid={}) have(amount={},cid={},empty={})",
                c.input.slot_index,
                actual == expected,
                c.input.amount,
                c.input.creation_id,
                actual.amount(),
                actual.creation_id(),
                actual.is_empty(),
            );
        }
    }
    let effects = apply_native_transition(
        state,
        &context(state, HEIGHT, holder, fee),
        NativeOperation::RedeemToken {
            asset: launch.token.asset,
        },
        &inputs,
        &outputs,
    )
    .map_err(|e| format!("apply: {e:?}"))?;
    assert_eq!(state.circulating_supply_micronoid, before - u128::from(fee));
    let mut balance_slot = (0, 0);
    for created in &effects.created_openings {
        if let NativeOpening::TokenBalance(_) = created.opening {
            balance_slot = (created.slot_index, created.creation_id);
        }
    }
    Ok(Redemption {
        payout,
        next,
        balance_slot,
    })
}

fn redeem_zero_burn(
    state: &mut ChainState,
    slots: &mut Slots,
    holder: Address,
    launch: &Launch,
    fee_funding: (u32, u64, u64),
) -> Result<(), String> {
    let balance = launch.balance.unwrap();
    redeem(
        state,
        slots,
        holder,
        launch,
        &[(balance.0, balance.1, balance.2)],
        0,
        fee_funding,
    )
    .map(|_| ())
}

fn redeem_reserve_skim(
    state: &mut ChainState,
    slots: &mut Slots,
    holder: Address,
    launch: &Launch,
    fee_funding: (u32, u64, u64),
) -> Result<(), String> {
    let burn = 1_000_000_000u64;
    let (next, payout) = launch.reserve.redeem(burn).map_err(|e| e.to_string())?;
    // Understate the outstanding supply by one unit in the successor reserve:
    // raw NOID amounts are untouched, so NOID conservation still holds, but
    // the bookkeeping no longer matches the redemption quote.
    let skimmed = TokenReserve {
        outstanding_units: next.outstanding_units - 1,
        ..next
    };
    let reserve_opening = NativeOpening::TokenReserve(launch.reserve);
    let balance = launch.balance.unwrap();
    let balance_opening = NativeOpening::TokenBalance(balance.2);
    let inputs = vec![
        NativeInputClaim {
            input: TxInput {
                slot_index: launch.reserve_slot,
                amount: launch.reserve.reserve_micronoid,
                creation_id: launch.reserve_cid,
            },
            root: reserve_opening.root(),
            opening: Some(reserve_opening),
        },
        NativeInputClaim {
            input: TxInput {
                slot_index: balance.0,
                amount: 0,
                creation_id: balance.1,
            },
            root: balance_opening.root(),
            opening: Some(balance_opening),
        },
        raw_input(fee_funding.0, fee_funding.1, fee_funding.2, holder),
    ];
    let mut outputs = vec![
        native_out(NativeOpening::TokenReserve(skimmed), slots.take()),
        native_out(
            NativeOpening::TokenBalance(TokenBalance {
                asset: launch.token.asset,
                owner: holder,
                units: balance.2.units - burn,
                unlock_height: 0,
            }),
            slots.take(),
        ),
        raw_out(slots.take(), payout, holder),
    ];
    let fee = required_fee(state, inputs.len(), outputs.len() + 1);
    let change = fee_funding.1 - fee;
    outputs.push(raw_out(slots.take(), change, holder));
    apply_native_transition(
        state,
        &context(state, HEIGHT, holder, fee),
        NativeOperation::RedeemToken {
            asset: launch.token.asset,
        },
        &inputs,
        &outputs,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// R3b: successor reserve deflated by one microNOID with the raw payout
/// inflated by one, keeping NOID conservation. A minimum-funded reserve makes
/// the tampered successor under-backed, so the opening-level backing guard
/// fires before the semantic reserve comparison.
fn redeem_payout_skim(
    state: &mut ChainState,
    slots: &mut Slots,
    holder: Address,
    launch: &Launch,
    fee_funding: (u32, u64, u64),
) -> Result<(), String> {
    let burn = 1_000_000_000u64;
    let (next, payout) = launch.reserve.redeem(burn).map_err(|e| e.to_string())?;
    let skimmed = TokenReserve {
        reserve_micronoid: next.reserve_micronoid - 1,
        ..next
    };
    let reserve_opening = NativeOpening::TokenReserve(launch.reserve);
    let balance = launch.balance.unwrap();
    let balance_opening = NativeOpening::TokenBalance(balance.2);
    let inputs = vec![
        NativeInputClaim {
            input: TxInput {
                slot_index: launch.reserve_slot,
                amount: launch.reserve.reserve_micronoid,
                creation_id: launch.reserve_cid,
            },
            root: reserve_opening.root(),
            opening: Some(reserve_opening),
        },
        NativeInputClaim {
            input: TxInput {
                slot_index: balance.0,
                amount: 0,
                creation_id: balance.1,
            },
            root: balance_opening.root(),
            opening: Some(balance_opening),
        },
        raw_input(fee_funding.0, fee_funding.1, fee_funding.2, holder),
    ];
    let mut outputs = vec![
        native_out(NativeOpening::TokenReserve(skimmed), slots.take()),
        native_out(
            NativeOpening::TokenBalance(TokenBalance {
                asset: launch.token.asset,
                owner: holder,
                units: balance.2.units - burn,
                unlock_height: 0,
            }),
            slots.take(),
        ),
        raw_out(slots.take(), payout + 1, holder),
    ];
    let fee = required_fee(state, inputs.len(), outputs.len() + 1);
    let change = fee_funding.1 - fee;
    outputs.push(raw_out(slots.take(), change, holder));
    apply_native_transition(
        state,
        &context(state, HEIGHT, holder, fee),
        NativeOperation::RedeemToken {
            asset: launch.token.asset,
        },
        &inputs,
        &outputs,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// A1: reserve funded with minimum-1.
fn underfunded_creation(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
) -> Result<(), String> {
    creation_with_reserve(state, slots, authority, funding, supply, 0, |minimum| {
        minimum - 1
    })
    .map(|_| ())
}

/// A2: reserve funded with minimum+1.
fn overfunded_creation(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
) -> Result<(), String> {
    creation_with_reserve(state, slots, authority, funding, supply, 0, |minimum| {
        minimum + 1
    })
    .map(|_| ())
}

/// A3: purchase tokens but emit an initial launch state with market_noid = 0.
fn purchase_without_cash(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
) -> Result<(), String> {
    let purchase = 1_000_000_000u64;
    let base = creation_skeleton(state, authority, funding, supply, purchase)?;
    // initial state claims zero curve cash was paid.
    let mut state0 = match base.initial {
        TradeState::Curve(s) => s,
        _ => unreachable!(),
    };
    state0.market_noid = 0;
    state0.inventory_units = supply - purchase;
    let outputs = base.outputs_with_control(
        NativeOpening::Launch(LaunchOpening::State(state0)),
        slots,
        base.cash, // still fund the honest cash into the change output
    );
    finish_creation(state, slots, authority, base, outputs, 0)
}

/// A4: split the purchase across two balance outputs.
fn split_purchase(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
) -> Result<(), String> {
    let purchase = 1_000_000_000u64;
    let base = creation_skeleton(state, authority, funding, supply, purchase)?;
    let outputs = base.outputs_with_control(
        match base.initial {
            TradeState::Curve(s) => NativeOpening::Launch(LaunchOpening::State(s)),
            TradeState::Market(p) => NativeOpening::Market(MarketOpening::Pool(p)),
        },
        slots,
        base.cash,
    );
    // Duplicate the balance output with half the units each.
    let mut outputs = outputs;
    let last = outputs.pop().expect("balance output");
    if let NativeOutputClaim {
        opening: Some(NativeOpening::TokenBalance(b)),
        ..
    } = last
    {
        for half in [b.units / 2, b.units - b.units / 2] {
            outputs.push(native_out(
                NativeOpening::TokenBalance(TokenBalance { units: half, ..b }),
                slots.take(),
            ));
        }
    }
    finish_creation(state, slots, authority, base, outputs, 0)
}

/// A6: the purchase balance is owned by the thief, not the funding authority.
fn stolen_purchase(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    thief: Address,
    funding: (u32, u64, u64),
    supply: u64,
) -> Result<(), String> {
    let purchase = 1_000_000_000u64;
    let mut base = creation_skeleton(state, authority, funding, supply, purchase)?;
    base.purchase_owner = thief;
    let outputs = base.outputs_with_control(
        match base.initial {
            TradeState::Curve(s) => NativeOpening::Launch(LaunchOpening::State(s)),
            TradeState::Market(p) => NativeOpening::Market(MarketOpening::Pool(p)),
        },
        slots,
        base.cash,
    );
    finish_creation(state, slots, authority, base, outputs, 0)
}

struct CreationSkeleton {
    token: FungibleDefinition,
    definition: LaunchDefinition,
    reserve: TokenReserve,
    initial: TradeState,
    cash: u64,
    funding: (u32, u64, u64),
    fee: u64,
    purchase: u64,
    purchase_owner: Address,
    authority: Address,
}

fn creation_skeleton(
    _state: &ChainState,
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
    purchase: u64,
) -> Result<CreationSkeleton, String> {
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner: authority,
        input: TxInput {
            slot_index: funding.0,
            amount: funding.1,
            creation_id: funding.2,
        },
    };
    let token =
        FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, [0x77; 32])
            .map_err(|e| e.to_string())?;
    let definition = LaunchDefinition::from_genesis(&anchor, token).map_err(|e| e.to_string())?;
    let initial = definition
        .after_initial_purchase(purchase)
        .map_err(|e| e.to_string())?;
    let cash = definition.curve_cash(purchase).map_err(|e| e.to_string())?;
    Ok(CreationSkeleton {
        token,
        definition,
        reserve: TokenReserve::from_definition(
            &token,
            TokenDenomination::new(TOKEN_DECIMALS)
                .unwrap()
                .minimum_backing(supply)
                .unwrap(),
        )
        .map_err(|e| e.to_string())?,
        initial,
        cash,
        funding,
        fee: 0,
        purchase,
        purchase_owner: authority,
        authority,
    })
}

impl CreationSkeleton {
    fn outputs_with_control(
        &self,
        control: NativeOpening,
        slots: &mut Slots,
        _cash: u64,
    ) -> Vec<NativeOutputClaim> {
        let mut outputs = vec![
            native_out(NativeOpening::FungibleDefinition(self.token), slots.take()),
            native_out(NativeOpening::TokenReserve(self.reserve), slots.take()),
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: slots.take(),
                    amount: control.amount_micronoid(),
                    owner: control.root(),
                },
                opening: Some(control),
            },
        ];
        if self.purchase > 0 {
            outputs.push(native_out(
                NativeOpening::TokenBalance(TokenBalance {
                    asset: self.token.asset,
                    owner: self.purchase_owner,
                    units: self.purchase,
                    unlock_height: 0,
                }),
                slots.take(),
            ));
        }
        outputs
    }
}

#[allow(clippy::too_many_arguments)]
fn creation_with_reserve(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
    purchase: u64,
    reserve_amount: impl FnOnce(u64) -> u64,
) -> Result<(), String> {
    let mut base = creation_skeleton(state, authority, funding, supply, purchase)?;
    let minimum = TokenDenomination::new(TOKEN_DECIMALS)
        .unwrap()
        .minimum_backing(supply)
        .unwrap();
    base.reserve = TokenReserve::from_definition(&base.token, reserve_amount(minimum))
        .map_err(|e| format!("reserve construction: {e:?}"))?;
    let outputs = base.outputs_with_control(
        match base.initial {
            TradeState::Curve(s) => NativeOpening::Launch(LaunchOpening::State(s)),
            TradeState::Market(p) => NativeOpening::Market(MarketOpening::Pool(p)),
        },
        slots,
        base.cash,
    );
    finish_creation(state, slots, authority, base, outputs, 0)
}

fn finish_creation(
    state: &mut ChainState,
    slots: &mut Slots,
    authority: Address,
    base: CreationSkeleton,
    mut outputs: Vec<NativeOutputClaim>,
    extra_burn: u64,
) -> Result<(), String> {
    let inputs = vec![raw_input(
        base.funding.0,
        base.funding.1,
        base.funding.2,
        authority,
    )];
    let fee = required_fee(state, inputs.len(), outputs.len() + 1);
    let minimum = base.reserve.reserve_micronoid;
    let change = base
        .funding
        .1
        .checked_sub(fee + minimum + base.cash + extra_burn)
        .ok_or("funding too small")?;
    if change > 0 {
        outputs.push(raw_out(slots.take(), change, authority));
    }
    apply_owned_launch_creation(
        state,
        &context(state, HEIGHT, authority, fee),
        base.token,
        base.definition,
        0,
        &inputs,
        &outputs,
        MarketFeeContext::from_state(state),
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

fn relaunch_asset_id(
    authority: Address,
    funding: (u32, u64, u64),
    supply: u64,
    metadata: [u8; 32],
) -> AssetId {
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner: authority,
        input: TxInput {
            slot_index: funding.0,
            amount: funding.1,
            creation_id: funding.2,
        },
    };
    FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, metadata)
        .unwrap()
        .asset
}
