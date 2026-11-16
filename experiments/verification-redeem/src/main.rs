//! Bounded transition-level control for the pre-graduation redemption claim.
use noid_poseidon2b::primitives::Address;
use noid_tx::native_assets::{
    launch::{
        launch_supply_from_whole,
        owner::check_launch_creation,
        sequential::{quote_curve_exact_input, TradeState},
        LaunchDefinition, LaunchOpening, LaunchSide,
    },
    native_creation_id,
    transition::{
        check_native_transition, NativeContext, NativeInputClaim, NativeOperation,
        NativeOutputClaim, NativeTransitionError,
    },
    FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening, TokenBalance, TokenReserve,
    TOKEN_DECIMALS, TOKEN_UNITS,
};
use noid_tx::{TxInput, TxOutput};

fn input(slot: u32, sequence: u64, opening: NativeOpening) -> NativeInputClaim {
    NativeInputClaim {
        input: TxInput {
            slot_index: slot,
            amount: opening.amount_micronoid(),
            creation_id: native_creation_id(sequence).unwrap(),
        },
        root: opening.root(),
        opening: Some(opening),
    }
}

fn output(slot: u32, opening: NativeOpening) -> NativeOutputClaim {
    NativeOutputClaim {
        output: TxOutput {
            slot_index: slot,
            amount: opening.amount_micronoid(),
            owner: opening.root(),
        },
        opening: Some(opening),
    }
}

fn main() {
    let owner = Address([7; 32]);
    let other = Address([8; 32]);
    let chain = [1; 32];
    let funding_amount = 1_000_000_000u64;
    let funding = NativeInputClaim {
        input: TxInput {
            slot_index: 10,
            amount: funding_amount,
            creation_id: 1,
        },
        root: owner,
        opening: None,
    };
    let context = NativeContext {
        chain,
        height: 25,
        authority: owner,
        fee: 0,
    };
    let anchor = GenesisAnchor {
        chain,
        owner,
        input: funding.input,
    };
    let supply = launch_supply_from_whole(100_000).unwrap();
    assert!(launch_supply_from_whole(1_000).is_err());
    let purchased = 10_000 * TOKEN_UNITS;
    let token =
        FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, [9; 32])
            .unwrap();
    let launch = LaunchDefinition::from_genesis(&anchor, token).unwrap();
    let TradeState::Curve(curve) = launch.after_initial_purchase(purchased).unwrap() else {
        panic!("10,000 tokens unexpectedly graduated");
    };
    let reserve_amount = noid_tx::asset_backing::TokenDenomination::new(TOKEN_DECIMALS)
        .unwrap()
        .minimum_backing(supply)
        .unwrap();
    let reserve = TokenReserve::from_definition(&token, reserve_amount).unwrap();
    let balance = TokenBalance {
        asset: token.asset,
        owner,
        units: purchased,
        unlock_height: 0,
    };
    let outputs = [
        output(100, NativeOpening::FungibleDefinition(token)),
        output(101, NativeOpening::TokenReserve(reserve)),
        output(102, NativeOpening::Launch(LaunchOpening::State(curve))),
        output(103, NativeOpening::TokenBalance(balance)),
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 104,
                amount: funding_amount - reserve_amount - curve.market_noid,
                owner,
            },
            opening: None,
        },
    ];
    check_launch_creation(&context, token, launch, 0, &[funding], &outputs).unwrap();

    let (after_reserve, payout) = reserve.redeem(purchased).unwrap();
    let redeem_outputs = [
        output(110, NativeOpening::TokenReserve(after_reserve)),
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 111,
                amount: payout,
                owner,
            },
            opening: None,
        },
    ];
    let redeem_inputs = [
        input(101, 1, NativeOpening::TokenReserve(reserve)),
        input(103, 2, NativeOpening::TokenBalance(balance)),
    ];
    let facts = check_native_transition(
        &context,
        NativeOperation::RedeemToken { asset: token.asset },
        &redeem_inputs,
        &redeem_outputs,
    )
    .unwrap();
    assert_eq!(facts.redeemed_units, purchased);
    assert_eq!(facts.redemption_micronoid, payout);
    assert!(curve.market_noid > payout);
    assert_eq!(after_reserve.outstanding_units, curve.inventory_units);
    assert_eq!(
        after_reserve.reserve_micronoid + payout,
        reserve.reserve_micronoid
    );

    // A controller with a different Owner statement cannot burn this balance.
    let denied = check_native_transition(
        &NativeContext {
            authority: other,
            ..context
        },
        NativeOperation::RedeemToken { asset: token.asset },
        &redeem_inputs,
        &redeem_outputs,
    );
    assert_eq!(denied, Err(NativeTransitionError::Authority));

    // The launch state's inventory cannot be supplied as a redemption input.
    let launch_claim = input(102, 3, NativeOpening::Launch(LaunchOpening::State(curve)));
    let mut launch_redeem_outputs = redeem_outputs.to_vec();
    launch_redeem_outputs.push(NativeOutputClaim {
        output: TxOutput {
            slot_index: 112,
            amount: curve.market_noid,
            owner,
        },
        opening: None,
    });
    let denied = check_native_transition(
        &context,
        NativeOperation::RedeemToken { asset: token.asset },
        &[redeem_inputs[0], launch_claim],
        &launch_redeem_outputs,
    );
    assert_eq!(denied, Err(NativeTransitionError::Shape));

    // Buying the unallocated sale units still graduates at the fixed reserve.
    let remaining_sale = launch.sale_units() - purchased;
    let remaining_cash = launch.graduation_cash() - curve.market_noid;
    let graduation = quote_curve_exact_input(
        launch,
        curve,
        LaunchSide::Buy,
        remaining_cash,
        remaining_sale,
    )
    .unwrap();
    assert_eq!(graduation.output(), remaining_sale);
    let TradeState::Market(pool) = graduation.after() else {
        panic!("remaining sale did not graduate");
    };
    assert_eq!(pool.base_reserve, launch.graduation_cash());
    assert_eq!(
        after_reserve.outstanding_units,
        remaining_sale + launch.dex_units()
    );

    println!(
        "launch creation transition accepted: supply={supply} purchased={purchased} curve_cash={}",
        curve.market_noid
    );
    println!(
        "redemption transition accepted: burned={} payout={} reserve_after={} outstanding_after={}",
        facts.redeemed_units,
        payout,
        after_reserve.reserve_micronoid,
        after_reserve.outstanding_units
    );
    println!(
        "purchase cost minus redemption payout, before fees and launch backing deposit: {} micronoid",
        curve.market_noid - payout
    );
    println!("negative controls: foreign_owner=Authority launch_as_balance=Shape");
    println!("graduation quote: remaining_sale={remaining_sale} additional_cash={remaining_cash} pool_cash={} pool_tokens={}",
        pool.base_reserve, pool.quote_reserve);
}
