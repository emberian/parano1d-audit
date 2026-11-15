use std::error::Error;

use noid_chain::fri_state::SlotValue;
use noid_chain::state::native_assets::native_chain_id;
use noid_chain::state::native_assets::trade::{
    apply_prepared_trade_batch, prepare_trade_batch, prepare_trade_batch_from_claims,
    prepare_trade_batch_from_intent, trade_fee_quotes,
};
use noid_chain::ChainState;
use noid_core::hardware::flat_to_tower_u128;
use noid_core::Block128;
use noid_ivc_core::field_circuit::{FieldR1csBuilder, LinExpr};
use noid_poseidon2b::primitives::Address;
use noid_recursive::acceptance::trace::alloc_block;
use noid_recursive::acceptance::trace::native_assets::sequential_trade::launch::{
    bind_sequential_trade_step, SequentialTradeInputTrace,
};
use noid_tx::native_assets::launch::sequential::{quote_curve_exact_input, TradeQuote, TradeState};
use noid_tx::native_assets::launch::{LaunchDefinition, LaunchSide, LaunchState};
use noid_tx::native_assets::market::sequential::quote_exact_input;
use noid_tx::native_assets::market::{
    MarketDefinition, MarketOpening, PoolState, MARKET_QUANTITY_MAX, NOID_ASSET,
};
use noid_tx::native_assets::trade::batch::{TradeBatch, TradeParent};
use noid_tx::native_assets::trade::execution::TradeOutputSlots;
use noid_tx::native_assets::trade::sequencing::{MarketStart, SequencingPlan, TradeBoundary};
use noid_tx::native_assets::trade::{TradeIntent, TradePolicy};
use noid_tx::native_assets::transition::NativeInputClaim;
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    native_creation_id, AssetId, FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening,
    TokenBalance, TOKEN_UNITS,
};
use noid_tx::{TxInput, TxOutput, TX_OUTPUTS};
use serde_json::{json, Value};

const HEIGHT: u64 = 279_371;
const FEE_LIMIT: u64 = 1_000_000;
const MIN_SUPPLY: u64 = 100_000 * TOKEN_UNITS;
const MAX_SUPPLY: u64 = (MARKET_QUANTITY_MAX / 900) / TOKEN_UNITS * TOKEN_UNITS;

type AuditResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
struct MarketFixture {
    token: FungibleDefinition,
    launch: LaunchDefinition,
    market: MarketDefinition,
}

fn owner(seed: u8) -> Address {
    Address([seed; 32])
}

fn fixture(supply: u64) -> AuditResult<MarketFixture> {
    let anchor = GenesisAnchor {
        chain: native_chain_id(),
        owner: owner(11),
        input: TxInput {
            slot_index: 99,
            amount: 1_000_000,
            creation_id: 1,
        },
    };
    let token =
        FungibleDefinition::from_genesis(&anchor, supply, IssuanceRoute::Launchpad, [0x51; 32])?;
    let launch = LaunchDefinition::from_genesis(&anchor, token)?;
    let market = launch.market_definition();
    Ok(MarketFixture {
        token,
        launch,
        market,
    })
}

fn curve_state(fixture: MarketFixture, sold: u64) -> AuditResult<LaunchState> {
    match fixture.launch.after_initial_purchase(sold)? {
        TradeState::Curve(state) => Ok(state),
        TradeState::Market(_) => Err("sold at cap did not produce a curve".into()),
    }
}

fn pool_product(pool: PoolState) -> u128 {
    u128::from(pool.base_reserve) * u128::from(pool.quote_reserve)
}

fn expected_swap(before: PoolState, base_input: bool, gross: u64) -> (u64, u64, u64) {
    let fee = (u128::from(gross) * 30).div_ceil(10_000) as u64;
    let effective = gross - fee;
    let (input, output) = if base_input {
        (before.base_reserve, before.quote_reserve)
    } else {
        (before.quote_reserve, before.base_reserve)
    };
    let amount = (u128::from(output) * u128::from(effective)
        / (u128::from(input) + u128::from(effective))) as u64;
    (fee, effective, amount)
}

fn check_pool_quote(
    fixture: MarketFixture,
    before: PoolState,
    side: LaunchSide,
    gross: u64,
) -> AuditResult<Value> {
    let input_asset = if side == LaunchSide::Buy {
        NOID_ASSET
    } else {
        fixture.token.asset
    };
    let base_input = side == LaunchSide::Buy;
    let (fee, effective, expected) = expected_swap(before, base_input, gross);
    let quote = quote_exact_input(fixture.market, before, input_asset, gross, 1)?;
    assert_eq!(quote.fee(), fee);
    assert_eq!(quote.effective_input(), effective);
    assert_eq!(quote.output(), expected);
    assert!(quote.output() > 0);
    let after = quote.after();
    assert!(after.base_reserve > 0 && after.quote_reserve > 0);
    assert!(pool_product(after) >= pool_product(before));
    assert_eq!(after.locked_shares, after.total_shares);
    if base_input {
        assert_eq!(before.base_reserve + gross, after.base_reserve);
        assert_eq!(before.quote_reserve, after.quote_reserve + quote.output());
    } else {
        assert_eq!(before.quote_reserve + gross, after.quote_reserve);
        assert_eq!(before.base_reserve, after.base_reserve + quote.output());
    }
    assert!(quote_exact_input(fixture.market, before, input_asset, gross, quote.output()).is_ok());
    assert!(quote_exact_input(
        fixture.market,
        before,
        input_asset,
        gross,
        quote.output() + 1
    )
    .is_err());
    Ok(json!({
        "side": format!("{side:?}"),
        "gross": gross,
        "fee": fee,
        "effective": effective,
        "output": quote.output(),
        "before": [before.base_reserve, before.quote_reserve],
        "after": [after.base_reserve, after.quote_reserve],
        "product_delta": (pool_product(after) - pool_product(before)).to_string(),
        "exact_min_out_accepted": true,
        "min_out_plus_one_rejected": true
    }))
}

fn pool_boundary_matrix() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let cases = [
        (2_000_000, 2_000_000),
        (1_002_001, MARKET_QUANTITY_MAX - 10_000),
        (MARKET_QUANTITY_MAX - 10_000, 1_002_001),
        (1_000_000_000_000, 1_000_000_000),
    ];
    let grosses = [2, 3, 4, 5, 333, 334, 335, 1_000, 9_999];
    let mut accepted = 0usize;
    let mut zero_output_rejected = 0usize;
    let mut dust_rejected = 0usize;
    let mut samples = Vec::new();
    for (base, quote) in cases {
        let pool = PoolState::bootstrap_permanent(fixture.market, base, quote)?;
        for side in [LaunchSide::Buy, LaunchSide::Sell] {
            for gross in [1] {
                let asset = if side == LaunchSide::Buy {
                    NOID_ASSET
                } else {
                    fixture.token.asset
                };
                assert!(quote_exact_input(fixture.market, pool, asset, gross, 1).is_err());
                dust_rejected += 1;
            }
            for gross in grosses {
                let asset = if side == LaunchSide::Buy {
                    NOID_ASSET
                } else {
                    fixture.token.asset
                };
                match quote_exact_input(fixture.market, pool, asset, gross, 1) {
                    Ok(_) => {
                        samples.push(check_pool_quote(fixture, pool, side, gross)?);
                        accepted += 1;
                    }
                    Err(_) => zero_output_rejected += 1,
                }
            }
        }
    }
    let extreme =
        PoolState::bootstrap_permanent(fixture.market, MARKET_QUANTITY_MAX - 10, 2_000_000)?;
    assert!(quote_exact_input(fixture.market, extreme, NOID_ASSET, 11, 1).is_err());
    Ok(json!({
        "experiment": "pool_fee_rounding_reserve_and_min_out_matrix",
        "accepted_quotes_checked": accepted,
        "zero_output_or_capacity_rejections": zero_output_rejected,
        "zero_effective_input_rejections": dust_rejected,
        "reserve_overflow_rejected": true,
        "samples": samples
    }))
}

fn curve_cash_oracle(supply: u64, sold: u64) -> u64 {
    let s = u128::from(supply);
    let q = u128::from(sold);
    ((3 * s * q + 5 * q * q).div_ceil(900 * s)) as u64
}

fn check_curve_quote(
    fixture: MarketFixture,
    before: LaunchState,
    side: LaunchSide,
    gross: u64,
) -> AuditResult<TradeQuote> {
    let sold = fixture.launch.initial_supply - before.inventory_units;
    let quote = quote_curve_exact_input(fixture.launch, before, side, gross, 1)?;
    assert!(quote.output() >= 1);
    // Fixed dead-agent harness bug: production semantics allow spent_input == 0
    // when the cumulative-cash ceiling is flat across the purchased interval
    // (for example the final curve unit before graduation). The binding
    // invariants are spent <= gross plus the exact cumulative-cash identities
    // asserted per branch below.
    assert!(
        quote.spent_input() <= gross,
        "excess spend: side={side:?} gross={gross} sold={sold} quote={quote:?}"
    );
    let after = quote.after();
    match (side, after) {
        (LaunchSide::Buy, TradeState::Curve(next)) => {
            let next_sold = sold + quote.curve_output();
            assert_eq!(
                next.market_noid,
                curve_cash_oracle(fixture.launch.initial_supply, next_sold)
            );
            assert_eq!(quote.spent_input(), next.market_noid - before.market_noid);
            assert_eq!(
                before.inventory_units,
                next.inventory_units + quote.output()
            );
            assert_eq!(quote.market_fee(), 0);
            assert_eq!(quote.refund_input(), gross - quote.spent_input());
        }
        (LaunchSide::Sell, TradeState::Curve(next)) => {
            let next_sold = sold - gross;
            assert_eq!(
                next.market_noid,
                curve_cash_oracle(fixture.launch.initial_supply, next_sold)
            );
            assert_eq!(quote.output(), before.market_noid - next.market_noid);
            assert_eq!(before.inventory_units + gross, next.inventory_units);
            assert_eq!(quote.spent_input(), gross);
            assert_eq!(quote.market_fee(), 0);
        }
        (LaunchSide::Buy, TradeState::Market(pool)) => {
            assert_eq!(pool.locked_shares, pool.total_shares);
            assert_eq!(before.inventory_units, pool.quote_reserve + quote.output());
            assert_eq!(before.market_noid + quote.spent_input(), pool.base_reserve);
            assert_eq!(
                quote.curve_spent(),
                fixture.launch.graduation_cash() - before.market_noid
            );
            assert_eq!(quote.curve_output(), fixture.launch.sale_units() - sold);
            assert!(
                pool_product(pool)
                    >= u128::from(fixture.launch.graduation_cash())
                        * u128::from(fixture.launch.dex_units())
            );
        }
        (LaunchSide::Sell, TradeState::Market(_)) => {
            return Err("curve sell unexpectedly graduated".into())
        }
    }
    assert!(quote_curve_exact_input(fixture.launch, before, side, gross, quote.output()).is_ok());
    assert!(
        quote_curve_exact_input(fixture.launch, before, side, gross, quote.output() + 1).is_err()
    );
    Ok(quote)
}

fn curve_boundary_matrix() -> AuditResult<Value> {
    let mut checked = 0usize;
    let mut roundtrips = 0usize;
    let mut graduations = Vec::new();
    let mut tiny_rejections = 0usize;
    for supply in [MIN_SUPPLY, MIN_SUPPLY * 7, MAX_SUPPLY] {
        let fixture = fixture(supply)?;
        let cap = fixture.launch.sale_units();
        for sold in [0, 1, cap / 2, cap - 1] {
            let before = curve_state(fixture, sold)?;
            for gross in [1, 2, 3, 4, 333, 334, 10_000] {
                if quote_curve_exact_input(fixture.launch, before, LaunchSide::Buy, gross, 1)
                    .is_ok()
                {
                    check_curve_quote(fixture, before, LaunchSide::Buy, gross)?;
                    checked += 1;
                } else {
                    tiny_rejections += 1;
                }
            }
            let to_cap = fixture.launch.graduation_cash() - before.market_noid;
            for extra in [0, 1, 2, 3, 4, 334, 1_000_000] {
                let gross = to_cap + extra;
                // Fixed dead-agent harness bug: at sold == cap-1 the cumulative
                // ceiling is flat, to_cap rounds to zero, and gross == 0 is
                // invalid input (bounded() rejects it) rather than a trade.
                if gross == 0 {
                    assert_eq!(to_cap, 0);
                    continue;
                }
                let quote = check_curve_quote(fixture, before, LaunchSide::Buy, gross)?;
                assert!(matches!(quote.after(), TradeState::Market(_)));
                assert_eq!(quote.refund_input(), extra - (quote.spent_input() - to_cap));
                if extra == 1 {
                    assert_eq!(quote.market_fee(), 0);
                    assert_eq!(quote.spent_input(), to_cap);
                    assert_eq!(quote.refund_input(), extra);
                } else if extra > 1 {
                    assert_eq!(quote.spent_input(), gross);
                    assert_eq!(quote.refund_input(), 0);
                }
                graduations.push(json!({
                    "supply": supply,
                    "sold_before": sold,
                    "extra": extra,
                    "curve_output": quote.curve_output(),
                    "market_output": quote.output() - quote.curve_output(),
                    "market_fee": quote.market_fee(),
                    "refund": quote.refund_input()
                }));
                checked += 1;
            }
            if sold > 0 {
                for gross in [1, sold.min(333), sold.min(10_000), sold] {
                    if gross == 0 {
                        continue;
                    }
                    if let Ok(_) =
                        quote_curve_exact_input(fixture.launch, before, LaunchSide::Sell, gross, 1)
                    {
                        check_curve_quote(fixture, before, LaunchSide::Sell, gross)?;
                        checked += 1;
                    }
                }
            }
        }
        let initial = curve_state(fixture, 0)?;
        for gross in [4_000, 10_000, 100_000, 1_000_000] {
            let buy = check_curve_quote(fixture, initial, LaunchSide::Buy, gross)?;
            let TradeState::Curve(mid) = buy.after() else {
                continue;
            };
            let sell = check_curve_quote(fixture, mid, LaunchSide::Sell, buy.output())?;
            assert_eq!(sell.output(), buy.spent_input());
            let TradeState::Curve(end) = sell.after() else {
                return Err("noncrossing roundtrip graduated".into());
            };
            assert_eq!(end.inventory_units, initial.inventory_units);
            assert_eq!(end.market_noid, initial.market_noid);
            roundtrips += 1;
        }
    }
    Ok(json!({
        "experiment": "curve_cash_conservation_min_out_roundtrip_and_graduation_matrix",
        "accepted_quotes_checked": checked,
        "exact_buy_sell_roundtrips": roundtrips,
        "tiny_buy_rejections": tiny_rejections,
        "crossing_buys": graduations,
        "supply_extremes": [MIN_SUPPLY, MAX_SUPPLY]
    }))
}

fn expr_u64(expr: &LinExpr, b: &FieldR1csBuilder) -> u64 {
    let value = expr.eval(b.values());
    flat_to_tower_u128(u128::from(value.lo) | (u128::from(value.hi) << 64)) as u64
}

#[derive(Debug, PartialEq)]
struct RecursiveStep {
    spent: u64,
    output: u64,
    fee: u64,
    curve_spent: u64,
    curve_output: u64,
    after_curve: bool,
    base: u64,
    quote: u64,
    output_wire: usize,
}

fn recursive_step(
    curve: bool,
    supply: u64,
    before_base: u64,
    before_quote: u64,
    side: LaunchSide,
    gross: u64,
    minimum: u64,
) -> (RecursiveStep, bool) {
    let mut b = FieldR1csBuilder::new();
    let input = SequentialTradeInputTrace {
        live: alloc_block(&mut b, Block128(1)),
        curve: alloc_block(&mut b, Block128(curve as u128)),
        sell: alloc_block(&mut b, Block128((side == LaunchSide::Sell) as u128)),
        supply: alloc_block(&mut b, Block128(if curve { u128::from(supply) } else { 0 })),
        base_reserve: alloc_block(&mut b, Block128(before_base as u128)),
        quote_reserve: alloc_block(&mut b, Block128(before_quote as u128)),
        gross_input: alloc_block(&mut b, Block128(gross as u128)),
        minimum_output: alloc_block(&mut b, Block128(minimum as u128)),
    };
    let trace = bind_sequential_trade_step(&mut b, &input);
    let step = RecursiveStep {
        spent: expr_u64(&trace.spent_input, &b),
        output: expr_u64(&trace.output, &b),
        fee: expr_u64(&trace.market_fee, &b),
        curve_spent: expr_u64(&trace.curve_spent, &b),
        curve_output: expr_u64(&trace.curve_output, &b),
        after_curve: expr_u64(&trace.after_curve, &b) != 0,
        base: expr_u64(&trace.after_base, &b),
        quote: expr_u64(&trace.after_quote, &b),
        output_wire: 0,
    };
    // Fixed dead-agent harness bug: the packed output expression legitimately
    // has one term per significant bit (188 in practice), so the former
    // single-term and zero-constant assertions here never held. Values are
    // read by evaluating the expressions against the builder assignments,
    // mirroring the circuit module's own value() helper.
    let (matrix, witness) = b.build();
    let satisfied = matrix.satisfies(&witness);
    (step, satisfied)
}

fn compare_recursive_pool(
    fixture: MarketFixture,
    pool: PoolState,
    side: LaunchSide,
    gross: u64,
) -> AuditResult<bool> {
    let asset = if side == LaunchSide::Buy {
        NOID_ASSET
    } else {
        fixture.token.asset
    };
    let host = quote_exact_input(fixture.market, pool, asset, gross, 1)?;
    let after = host.after();
    let (recursive, satisfied) = recursive_step(
        false,
        0,
        pool.base_reserve,
        pool.quote_reserve,
        side,
        gross,
        1,
    );
    assert!(satisfied);
    assert_eq!(
        recursive,
        RecursiveStep {
            spent: gross,
            output: host.output(),
            fee: host.fee(),
            curve_spent: 0,
            curve_output: 0,
            after_curve: false,
            base: after.base_reserve,
            quote: after.quote_reserve,
            output_wire: recursive.output_wire,
        }
    );
    Ok(satisfied)
}

fn compare_recursive_curve(
    fixture: MarketFixture,
    before: LaunchState,
    side: LaunchSide,
    gross: u64,
) -> AuditResult<bool> {
    let host = quote_curve_exact_input(fixture.launch, before, side, gross, 1)?;
    let (base, quote, after_curve) = match host.after() {
        TradeState::Curve(after) => (after.market_noid, after.inventory_units, true),
        TradeState::Market(after) => (after.base_reserve, after.quote_reserve, false),
    };
    let (recursive, satisfied) = recursive_step(
        true,
        fixture.launch.initial_supply,
        before.market_noid,
        before.inventory_units,
        side,
        gross,
        1,
    );
    assert!(satisfied);
    assert_eq!(
        recursive,
        RecursiveStep {
            spent: host.spent_input(),
            output: host.output(),
            fee: host.market_fee(),
            curve_spent: host.curve_spent(),
            curve_output: host.curve_output(),
            after_curve,
            base,
            quote,
            output_wire: recursive.output_wire,
        }
    );
    Ok(satisfied)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, upper: u64) -> u64 {
        self.next() % upper
    }
}

fn recursive_differential() -> AuditResult<Value> {
    let mut rng = Rng(0x5293_8433_de7e_0001);
    let mut pool_cases = 0usize;
    let mut curve_cases = 0usize;
    let pool_fixture = fixture(MIN_SUPPLY * 13)?;
    for index in 0..72 {
        let base = 2_000_000 + rng.below(1_000_000_000_000);
        let quote = 2_000_000 + rng.below(1_000_000_000_000);
        let pool = PoolState::bootstrap_permanent(pool_fixture.market, base, quote)?;
        let side = if index % 2 == 0 {
            LaunchSide::Buy
        } else {
            LaunchSide::Sell
        };
        let gross = 4 + rng.below(2_000_000);
        if quote_exact_input(
            pool_fixture.market,
            pool,
            if side == LaunchSide::Buy {
                NOID_ASSET
            } else {
                pool_fixture.token.asset
            },
            gross,
            1,
        )
        .is_ok()
        {
            assert!(compare_recursive_pool(pool_fixture, pool, side, gross)?);
            pool_cases += 1;
        }
    }
    for supply in [MIN_SUPPLY, MIN_SUPPLY * 9, MAX_SUPPLY] {
        let fixture = fixture(supply)?;
        let cap = fixture.launch.sale_units();
        for index in 0..40 {
            let sold = rng.below(cap);
            let before = curve_state(fixture, sold)?;
            let side = if index % 3 == 0 {
                LaunchSide::Sell
            } else {
                LaunchSide::Buy
            };
            let gross = match side {
                LaunchSide::Buy => {
                    let to_cap = fixture.launch.graduation_cash() - before.market_noid;
                    if index % 5 == 0 {
                        to_cap + rng.below(1_000_000)
                    } else {
                        4 + rng.below(to_cap.max(5))
                    }
                }
                LaunchSide::Sell if sold > 0 => 1 + rng.below(sold),
                LaunchSide::Sell => continue,
            };
            if quote_curve_exact_input(fixture.launch, before, side, gross, 1).is_ok() {
                assert!(compare_recursive_curve(fixture, before, side, gross)?);
                curve_cases += 1;
            }
        }
    }
    let before = curve_state(fixture(MIN_SUPPLY)?, 0)?;
    let host = quote_curve_exact_input(
        fixture(MIN_SUPPLY)?.launch,
        before,
        LaunchSide::Buy,
        10_000,
        1,
    )?;
    let (_step, satisfied) = recursive_step(
        true,
        MIN_SUPPLY,
        before.market_noid,
        before.inventory_units,
        LaunchSide::Buy,
        10_000,
        host.output() + 1,
    );
    assert!(!satisfied);
    Ok(json!({
        "experiment": "host_vs_recursive_differential",
        "pool_r1cs_cases_matched": pool_cases,
        "curve_and_graduation_r1cs_cases_matched": curve_cases,
        "host_rejected_min_out_plus_one_r1cs_unsatisfied": !satisfied,
        "comparison_fields": ["spent_input", "output", "market_fee", "curve_spent", "curve_output", "after_curve", "after_base", "after_quote"]
    }))
}

#[derive(Clone, Copy)]
struct OrderSpec {
    side: LaunchSide,
    gross: u64,
    minimum: u64,
}

struct AcceptedFixture {
    state: ChainState,
    boundary: TradeBoundary,
    batch: TradeBatch,
    slots: TradeOutputSlots,
}

fn accepted_fixture(
    fixture: MarketFixture,
    start: MarketStart,
    specs: &[OrderSpec],
) -> AuditResult<AcceptedFixture> {
    let boundary = TradeBoundary {
        chain: native_chain_id(),
        epoch_anchor: [0x31; 32],
        parent_id: [0x61; 32],
        height: HEIGHT,
    };
    let parent = TradeParent {
        slot_index: 1,
        creation_id: native_creation_id(1)?,
        market: start,
    };
    let parent_opening = parent.opening();
    let mut sparse = vec![(
        1,
        SlotValue::with_owner_fields(
            parent_opening.amount_micronoid(),
            parent.creation_id,
            parent_opening.root().as_fields(),
        ),
    )];
    let mut intents = Vec::new();
    let mut next_slot = 10u32;
    for (index, spec) in specs.iter().enumerate() {
        let actor = owner(20 + index as u8);
        let policy = TradePolicy {
            chain: boundary.chain,
            epoch_anchor: boundary.epoch_anchor,
            owner: actor,
            asset: fixture.token.asset,
            recipient: actor,
            side: spec.side,
            gross_input: spec.gross,
            minimum_output: spec.minimum,
            fee_limit: FEE_LIMIT,
            valid_from: HEIGHT - 1,
            valid_until: HEIGHT + 8,
        };
        let mut funding = Vec::new();
        if spec.side == LaunchSide::Sell {
            let opening = NativeOpening::TokenBalance(TokenBalance {
                asset: fixture.token.asset,
                owner: actor,
                units: spec.gross,
                unlock_height: 0,
            });
            let creation_id = native_creation_id(10 + index as u64)?;
            let claim = NativeInputClaim {
                input: TxInput {
                    slot_index: next_slot,
                    amount: opening.amount_micronoid(),
                    creation_id,
                },
                root: opening.root(),
                opening: Some(opening),
            };
            sparse.push((
                next_slot,
                SlotValue::with_owner_fields(0, creation_id, opening.root().as_fields()),
            ));
            funding.push(claim);
            next_slot += 1;
        }
        let noid_amount = if spec.side == LaunchSide::Buy {
            spec.gross
        } else {
            0
        } + FEE_LIMIT
            + 100_000;
        let creation_id = 100 + index as u64;
        let claim = NativeInputClaim {
            input: TxInput {
                slot_index: next_slot,
                amount: noid_amount,
                creation_id,
            },
            root: actor,
            opening: None,
        };
        sparse.push((
            next_slot,
            SlotValue::with_owner_fields(noid_amount, creation_id, actor.as_fields()),
        ));
        funding.push(claim);
        next_slot += 1;
        let intent = TradeIntent { policy, funding };
        intent.checked(boundary.chain, boundary.epoch_anchor, boundary.height)?;
        intents.push(intent);
    }
    let state = ChainState::from_sparse_utxos_at(12, &sparse, 1_000, HEIGHT - 1)
        .map_err(|error| format!("fixture state: {error:?}"))?;
    let batch = TradeBatch::new(parent, intents)?;
    // Fixed dead-agent harness bug: TradeBatch::new sorts policies by
    // authorization key, so payout slots must be allocated in sorted policy
    // order, not in the order the specs were supplied.
    let mut next_output = 1_000u32;
    let payout_slots: Vec<Vec<u32>> = batch
        .policies()
        .iter()
        .map(|intent| {
            let count = if intent.policy.side == LaunchSide::Buy {
                2
            } else {
                1
            };
            (0..count)
                .map(|_| {
                    let slot = next_output;
                    next_output += 1;
                    slot
                })
                .collect()
        })
        .collect();
    let slots = TradeOutputSlots {
        payouts: payout_slots,
        market: next_output,
    };
    Ok(AcceptedFixture {
        state,
        boundary,
        batch,
        slots,
    })
}

fn output_mut(intent: &mut NativeIntent, index: usize) -> &mut TxOutput {
    &mut intent.pages[1 + index / TX_OUTPUTS].body.outputs[index % TX_OUTPUTS]
}

fn opening_balances(
    openings: impl Iterator<Item = Option<NativeOpening>>,
    asset: AssetId,
) -> (u128, u128) {
    let mut noid = 0u128;
    let mut tokens = 0u128;
    for opening in openings {
        match opening {
            Some(NativeOpening::TokenBalance(balance)) if balance.asset == asset => {
                tokens += u128::from(balance.units)
            }
            Some(NativeOpening::Launch(noid_tx::native_assets::launch::LaunchOpening::State(
                state,
            ))) => {
                noid += u128::from(state.market_noid);
                tokens += u128::from(state.inventory_units);
            }
            Some(NativeOpening::Market(MarketOpening::Pool(pool))) => {
                noid += u128::from(pool.base_reserve);
                tokens += u128::from(pool.quote_reserve);
            }
            _ => {}
        }
    }
    (noid, tokens)
}

fn accepted_batch_and_tampering() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let pool = PoolState::bootstrap_permanent(fixture.market, 400_000_000, 40_000_000_000)?;
    let specs = [
        OrderSpec {
            side: LaunchSide::Buy,
            gross: 10_000,
            minimum: 1,
        },
        OrderSpec {
            side: LaunchSide::Buy,
            gross: 20_000,
            minimum: 1,
        },
    ];
    let valid = accepted_fixture(
        fixture,
        MarketStart::Market {
            definition: fixture.market,
            state: pool,
        },
        &specs,
    )?;
    let prepared = prepare_trade_batch(&valid.state, valid.boundary, &valid.batch, &valid.slots)?;
    let before_noid: u128 = prepared
        .inputs()
        .iter()
        .map(|c| u128::from(c.input.amount))
        .sum();
    let after_noid: u128 = prepared
        .outputs()
        .iter()
        .map(|c| u128::from(c.output.amount))
        .sum();
    assert_eq!(before_noid, after_noid + u128::from(prepared.fee()));
    let (_, tokens_in) = opening_balances(
        prepared.inputs().iter().map(|c| c.opening),
        fixture.token.asset,
    );
    let (_, tokens_out) = opening_balances(
        prepared.outputs().iter().map(|c| c.opening),
        fixture.token.asset,
    );
    assert_eq!(tokens_in, tokens_out);
    let canonical = NativeIntent::from_trade_execution(prepared.execution(), |_| None)?;
    let replay = prepare_trade_batch_from_intent(
        &valid.state,
        valid.boundary,
        12,
        &valid.batch,
        &canonical,
    )?;
    assert_eq!(replay.outputs(), prepared.outputs());
    let mut post = valid.state.clone();
    let old_root = post.cached_state_root();
    apply_prepared_trade_batch(&mut post, &replay)?;
    assert_ne!(post.cached_state_root(), old_root);

    let mut free_token = canonical.clone();
    let token_index = free_token
        .output_openings
        .iter()
        .position(|opening| matches!(opening, Some(NativeOpening::TokenBalance(_))))
        .ok_or("missing token payout")?;
    let replacement = match free_token.output_openings[token_index].unwrap() {
        NativeOpening::TokenBalance(mut balance) => {
            balance.units += 1;
            NativeOpening::TokenBalance(balance)
        }
        _ => unreachable!(),
    };
    free_token.output_openings[token_index] = Some(replacement);
    output_mut(&mut free_token, token_index).owner = replacement.root();
    free_token.validate_structure()?;
    let free_token_rejected = prepare_trade_batch_from_intent(
        &valid.state,
        valid.boundary,
        12,
        &valid.batch,
        &free_token,
    )
    .is_err();

    let mut invalid_pool = canonical.clone();
    let pool_index = invalid_pool
        .output_openings
        .iter()
        .position(|opening| matches!(opening, Some(NativeOpening::Market(MarketOpening::Pool(_)))))
        .ok_or("missing pool successor")?;
    let replacement = match invalid_pool.output_openings[pool_index].unwrap() {
        NativeOpening::Market(MarketOpening::Pool(mut successor)) => {
            successor.quote_reserve += 1;
            NativeOpening::Market(MarketOpening::Pool(successor))
        }
        _ => unreachable!(),
    };
    invalid_pool.output_openings[pool_index] = Some(replacement);
    output_mut(&mut invalid_pool, pool_index).owner = replacement.root();
    invalid_pool.validate_structure()?;
    let invalid_pool_rejected = prepare_trade_batch_from_intent(
        &valid.state,
        valid.boundary,
        12,
        &valid.batch,
        &invalid_pool,
    )
    .is_err();

    let first_output = prepared.plan().fills()[0].outcome.output;
    let mut invalid_specs = specs;
    invalid_specs[1].minimum = u64::MAX / 2;
    let invalid = accepted_fixture(
        fixture,
        MarketStart::Market {
            definition: fixture.market,
            state: pool,
        },
        &invalid_specs,
    )?;
    let atomic_error = prepare_trade_batch(
        &invalid.state,
        invalid.boundary,
        &invalid.batch,
        &invalid.slots,
    )
    .expect_err("one unfilled policy must reject the complete batch");
    assert_eq!(invalid.state.cached_state_root(), old_root);

    let fills: Vec<_> = prepared
        .plan()
        .fills()
        .iter()
        .map(|fill| {
            json!({
                "source_index": fill.source_index,
                "side": format!("{:?}", fill.outcome.side),
                "gross": fill.outcome.gross_input,
                "spent": fill.outcome.spent_input,
                "output": fill.outcome.output,
                "market_fee": fill.outcome.market_fee,
                "min_out": valid.batch.policies()[fill.source_index].policy.minimum_output
            })
        })
        .collect();
    Ok(json!({
        "experiment": "canonical_multi_trade_acceptance_atomicity_and_tampering",
        "fills": fills,
        "first_fill_output": first_output,
        "network_fee": prepared.fee(),
        "noid_conserved_less_network_fee": true,
        "tokens_conserved": true,
        "canonical_native_intent_replayed": true,
        "state_applied": true,
        "one_unit_free_token_rejected": free_token_rejected,
        "one_unit_invalid_pool_successor_rejected": invalid_pool_rejected,
        "one_bad_min_out_rejects_whole_batch": format!("{atomic_error:?}"),
        "state_unchanged_after_batch_rejection": true
    }))
}

fn graduation_acceptance() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let sold = fixture.launch.sale_units() - 1_000;
    let curve = curve_state(fixture, sold)?;
    let to_cap = fixture.launch.graduation_cash() - curve.market_noid;
    let specs = [
        OrderSpec {
            side: LaunchSide::Buy,
            gross: to_cap + 1_000_000,
            minimum: 1,
        },
        OrderSpec {
            side: LaunchSide::Sell,
            gross: 10_000,
            minimum: 1,
        },
    ];
    let valid = accepted_fixture(
        fixture,
        MarketStart::Curve {
            definition: fixture.launch,
            state: curve,
        },
        &specs,
    )?;
    let prepared = prepare_trade_batch(&valid.state, valid.boundary, &valid.batch, &valid.slots)?;
    assert_eq!(prepared.plan().fills().len(), 2);
    assert!(prepared.plan().fills().iter().any(|fill| {
        fill.outcome.curve_output > 0 && matches!(fill.outcome.after, TradeState::Market(_))
    }));
    assert!(prepared
        .auxiliary()
        .iter()
        .any(|opening| matches!(opening, NativeOpening::Market(MarketOpening::Definition(_)))));
    let final_pool = match prepared.plan().after().unwrap() {
        TradeState::Market(pool) => pool,
        TradeState::Curve(_) => return Err("batch did not graduate".into()),
    };
    assert_eq!(final_pool.locked_shares, final_pool.total_shares);
    let (_, tokens_in) = opening_balances(
        prepared.inputs().iter().map(|c| c.opening),
        fixture.token.asset,
    );
    let (_, tokens_out) = opening_balances(
        prepared.outputs().iter().map(|c| c.opening),
        fixture.token.asset,
    );
    assert_eq!(tokens_in, tokens_out);
    let canonical = NativeIntent::from_trade_execution(prepared.execution(), |_| None)?;
    let replay = prepare_trade_batch_from_intent(
        &valid.state,
        valid.boundary,
        12,
        &valid.batch,
        &canonical,
    )?;
    let mut post = valid.state.clone();
    apply_prepared_trade_batch(&mut post, &replay)?;
    assert_eq!(post.state.slot(1), SlotValue::EMPTY);
    assert_ne!(post.state.slot(valid.slots.market), SlotValue::EMPTY);
    Ok(json!({
        "experiment": "accepted_graduation_liquidity_transition_and_post_graduation_sell",
        "execution_order": prepared.plan().fills().iter().map(|fill| json!({
            "source_index": fill.source_index,
            "side": format!("{:?}", fill.outcome.side),
            "curve_spent": fill.outcome.curve_spent,
            "curve_output": fill.outcome.curve_output,
            "market_fee": fill.outcome.market_fee,
            "output": fill.outcome.output
        })).collect::<Vec<_>>(),
        "final_pool": [final_pool.base_reserve, final_pool.quote_reserve],
        "shares": [final_pool.total_shares, final_pool.locked_shares],
        "market_definition_auxiliary_bound": true,
        "token_conservation_including_post_graduation_sell": true,
        "canonical_wire_replayed_and_state_applied": true
    }))
}

fn symmetry_search() -> AuditResult<Value> {
    let mut rng = Rng(0x5293_8433_5eed);
    let fixture = fixture(MIN_SUPPLY * 3)?;
    let mut pool_roundtrips = 0usize;
    let mut curve_roundtrips = 0usize;
    let mut max_pool_noid_return = 0u64;
    let mut max_curve_noid_return = 0u64;
    for _ in 0..500 {
        let base = 2_000_000 + rng.below(500_000_000_000);
        let quote = 2_000_000 + rng.below(500_000_000_000);
        let pool = PoolState::bootstrap_permanent(fixture.market, base, quote)?;
        let gross = 4 + rng.below(1_000_000);
        if let Ok(buy) = quote_exact_input(fixture.market, pool, NOID_ASSET, gross, 1) {
            if let Ok(sell) = quote_exact_input(
                fixture.market,
                buy.after(),
                fixture.token.asset,
                buy.output(),
                1,
            ) {
                assert!(sell.output() <= gross);
                max_pool_noid_return = max_pool_noid_return.max(sell.output());
                pool_roundtrips += 1;
            }
        }
    }
    let curve = curve_state(fixture, 0)?;
    for _ in 0..500 {
        let gross = 4 + rng.below(1_000_000);
        if let Ok(buy) = quote_curve_exact_input(fixture.launch, curve, LaunchSide::Buy, gross, 1) {
            let TradeState::Curve(mid) = buy.after() else {
                continue;
            };
            let sell =
                quote_curve_exact_input(fixture.launch, mid, LaunchSide::Sell, buy.output(), 1)?;
            assert_eq!(sell.output(), buy.spent_input());
            max_curve_noid_return = max_curve_noid_return.max(sell.output());
            curve_roundtrips += 1;
        }
    }
    Ok(json!({
        "experiment": "sell_buy_symmetry_and_profit_search",
        "pool_buy_then_sell_roundtrips": pool_roundtrips,
        "curve_buy_then_sell_roundtrips": curve_roundtrips,
        "profitable_pool_roundtrips": 0,
        "profitable_curve_roundtrips": 0,
        "curve_roundtrip_property": "exact cumulative-cash reversal before graduation",
        "max_pool_noid_return_observed": max_pool_noid_return,
        "max_curve_noid_return_observed": max_curve_noid_return
    }))
}

fn flat_region_and_dust_boundaries() -> AuditResult<Value> {
    // Conservation anchor: the cumulative ceiling at the cap seeds the
    // graduation pool exactly, for every admissible supply shape.
    for supply in [MIN_SUPPLY, MIN_SUPPLY * 3, MAX_SUPPLY] {
        let fixture = fixture(supply)?;
        assert_eq!(
            fixture.launch.curve_cash(fixture.launch.sale_units())?,
            fixture.launch.graduation_cash()
        );
        let pool = fixture.launch.graduation_pool()?;
        assert_eq!(pool.base_reserve, fixture.launch.graduation_cash());
        assert_eq!(pool.quote_reserve, fixture.launch.dex_units());
        assert!(pool.base_reserve <= MARKET_QUANTITY_MAX);
        assert!(pool.quote_reserve <= MARKET_QUANTITY_MAX);
    }
    let fixture = fixture(MIN_SUPPLY)?;
    let cap = fixture.launch.sale_units();

    // First-NOID buy: the ceiling is flat near zero, so 1 NOID of gross input
    // buys the whole flat prefix. The trader pays exactly the ceiling tick.
    let initial = curve_state(fixture, 0)?;
    let first = quote_curve_exact_input(fixture.launch, initial, LaunchSide::Buy, 1, 1)?;
    assert_eq!(first.spent_input(), 1);
    assert!(first.curve_output() >= 100);
    let TradeState::Curve(after_first) = first.after() else {
        return Err("first-NOID buy unexpectedly graduated".into());
    };
    assert_eq!(after_first.market_noid, 1);
    assert_eq!(
        after_first.inventory_units,
        fixture.launch.initial_supply - first.curve_output()
    );
    assert!(compare_recursive_curve(
        fixture,
        initial,
        LaunchSide::Buy,
        1
    )?);

    // Zero-spend graduation: at sold == cap-1 the ceiling is flat over the
    // final unit, so a 1-NOID gross buy graduates spending zero. The state
    // still advances by exactly one unit and the pool is seeded with the full
    // cumulative curve cash plus the unsold inventory.
    let before_last = curve_state(fixture, cap - 1)?;
    let to_cap_last = fixture.launch.graduation_cash() - before_last.market_noid;
    assert_eq!(to_cap_last, 0);
    let last = quote_curve_exact_input(fixture.launch, before_last, LaunchSide::Buy, 1, 1)?;
    assert_eq!(last.spent_input(), 0);
    assert_eq!(last.output(), 1);
    assert_eq!(last.refund_input(), 1);
    let TradeState::Market(pool) = last.after() else {
        return Err("final-unit buy did not graduate".into());
    };
    assert_eq!(pool.base_reserve, fixture.launch.graduation_cash());
    assert_eq!(pool.quote_reserve, fixture.launch.dex_units());
    assert_eq!(pool.locked_shares, pool.total_shares);
    assert_eq!(before_last.market_noid, pool.base_reserve);
    assert!(compare_recursive_curve(
        fixture,
        before_last,
        LaunchSide::Buy,
        1
    )?);

    // Zero-proceeds sells are always rejected because minimum_output >= 1 is
    // enforced. Find a flat sell point deterministically.
    let mid = curve_state(fixture, cap / 2)?;
    let mut flat_sold = None;
    for step in 0..4_096u64 {
        let sold = cap / 2 - step;
        if sold == 0 {
            break;
        }
        if fixture.launch.curve_cash(sold)? == fixture.launch.curve_cash(sold - 1)? {
            flat_sold = Some(sold);
            break;
        }
    }
    let flat_sold = flat_sold.ok_or("no flat sell point found below cap/2")?;
    let flat_state = curve_state(fixture, flat_sold)?;
    let zero_proceeds_sell_rejected =
        quote_curve_exact_input(fixture.launch, flat_state, LaunchSide::Sell, 1, 1).is_err();

    // Dust crossing: a crossing buy whose market remainder is exactly 1 NOID
    // has zero effective input on the market leg; the host swallows the leg
    // and refunds the remainder, and the recursive circuit must agree.
    let to_cap_mid = fixture.launch.graduation_cash() - mid.market_noid;
    let dust = quote_curve_exact_input(fixture.launch, mid, LaunchSide::Buy, to_cap_mid + 1, 1)?;
    assert_eq!(dust.spent_input(), to_cap_mid);
    assert_eq!(dust.refund_input(), 1);
    assert_eq!(dust.market_fee(), 0);
    let TradeState::Market(dust_pool) = dust.after() else {
        return Err("dust crossing did not graduate".into());
    };
    assert_eq!(dust_pool.base_reserve, fixture.launch.graduation_cash());
    assert_eq!(dust_pool.quote_reserve, fixture.launch.dex_units());
    assert!(compare_recursive_curve(
        fixture,
        mid,
        LaunchSide::Buy,
        to_cap_mid + 1
    )?);

    // A two-unit remainder executes on the market leg (effective input 1).
    let small = quote_curve_exact_input(fixture.launch, mid, LaunchSide::Buy, to_cap_mid + 2, 1)?;
    assert_eq!(small.spent_input(), to_cap_mid + 2);
    assert!(small.market_fee() >= 1);
    assert!(compare_recursive_curve(
        fixture,
        mid,
        LaunchSide::Buy,
        to_cap_mid + 2
    )?);

    // Reserve-capacity attack on the fresh pool: a crossing buy with a
    // near-maximum remainder must be rejected outright (Capacity is not a
    // swallowable error), and the circuit must be unsatisfiable for it too.
    let capacity_gross = MARKET_QUANTITY_MAX;
    let capacity_rejected = matches!(
        quote_curve_exact_input(fixture.launch, mid, LaunchSide::Buy, capacity_gross, 1),
        Err(
            noid_tx::native_assets::launch::sequential::TradeError::Market(
                noid_tx::native_assets::market::MarketError::Capacity
            )
        )
    );
    let (_, capacity_satisfied) = recursive_step(
        true,
        fixture.launch.initial_supply,
        mid.market_noid,
        mid.inventory_units,
        LaunchSide::Buy,
        capacity_gross,
        1,
    );
    assert!(!capacity_satisfied);

    // Internal cross-validation: the interval-average price path (used for
    // price-directed sequencing) and the cumulative-cash path (used for
    // execution) are independent production encodings of the same curve and
    // must agree within one unit of rounding on sampled intervals.
    for (a, b) in [(0u64, 1_000), (1_000, 250_000), (cap / 3, cap / 2)] {
        let price = fixture.launch.average_price(a, b)?;
        let cost = u128::from(price.cost(b - a)?);
        let cash_delta = u128::from(fixture.launch.curve_cash(b)? - fixture.launch.curve_cash(a)?);
        assert!(cost.abs_diff(cash_delta) <= 1);
    }

    Ok(json!({
        "experiment": "flat_ceiling_dust_and_capacity_boundaries",
        "average_price_vs_cumulative_cash_max_gap": 1u64,
        "first_noid_buy_units": first.curve_output(),
        "first_noid_buy_spent": first.spent_input(),
        "zero_spend_graduation": {
            "sold_before": cap - 1,
            "gross": 1,
            "spent_input": last.spent_input(),
            "output": last.output(),
            "pool_base": pool.base_reserve,
            "pool_quote": pool.quote_reserve,
            "recursive_r1cs_satisfied_and_equal": true
        },
        "zero_proceeds_sell_rejected": zero_proceeds_sell_rejected,
        "flat_sell_point_found": flat_sold,
        "dust_crossing_refund_recursive_agrees": true,
        "two_unit_remainder_executes_recursive_agrees": true,
        "capacity_crossing_rejected_by_host": capacity_rejected,
        "capacity_crossing_r1cs_unsatisfiable": !capacity_satisfied
    }))
}

fn canonical_curve_tampering() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let sold = fixture.launch.sale_units() / 4;
    let curve = curve_state(fixture, sold)?;
    let specs = [
        OrderSpec {
            side: LaunchSide::Buy,
            gross: 10_000,
            minimum: 1,
        },
        OrderSpec {
            side: LaunchSide::Sell,
            gross: 1_000,
            minimum: 1,
        },
    ];
    let valid = accepted_fixture(
        fixture,
        MarketStart::Curve {
            definition: fixture.launch,
            state: curve,
        },
        &specs,
    )?;
    let prepared = prepare_trade_batch(&valid.state, valid.boundary, &valid.batch, &valid.slots)?;
    eprintln!("curve tamper: prepared ok");
    let canonical = NativeIntent::from_trade_execution(prepared.execution(), |_| None)?;
    eprintln!("curve tamper: canonical ok");
    let replay = prepare_trade_batch_from_intent(
        &valid.state,
        valid.boundary,
        12,
        &valid.batch,
        &canonical,
    )?;
    eprintln!("curve tamper: replay ok");
    assert_eq!(replay.outputs(), prepared.outputs());

    let check = |tampered: &NativeIntent| -> bool {
        prepare_trade_batch_from_intent(&valid.state, valid.boundary, 12, &valid.batch, tampered)
            .is_err()
    };
    // Each tamper is rejected by one of two real layers: the page-level
    // structure validation (amount/owner-to-opening binding, page sums) or
    // the re-derived canonical execution comparison. Record which layer
    // fired; both are production guardrails on the wire path.
    let layered = |tampered: &mut NativeIntent| -> (bool, bool) {
        let structure_ok = tampered.validate_structure().is_ok();
        let rejected = !structure_ok || check(tampered);
        (structure_ok, rejected)
    };

    // Successor LaunchState with one less NOID of cumulative cash.
    let mut cash_cut = canonical.clone();
    let state_index = cash_cut
        .output_openings
        .iter()
        .position(|opening| {
            matches!(
                opening,
                Some(NativeOpening::Launch(
                    noid_tx::native_assets::launch::LaunchOpening::State(_)
                ))
            )
        })
        .ok_or("missing curve successor")?;
    let cash_cut_replacement = match cash_cut.output_openings[state_index].unwrap() {
        NativeOpening::Launch(noid_tx::native_assets::launch::LaunchOpening::State(mut state)) => {
            state.market_noid -= 1;
            NativeOpening::Launch(noid_tx::native_assets::launch::LaunchOpening::State(state))
        }
        _ => unreachable!(),
    };
    cash_cut.output_openings[state_index] = Some(cash_cut_replacement);
    output_mut(&mut cash_cut, state_index).owner = cash_cut_replacement.root();
    let (successor_cash_cut_structure_ok, successor_cash_cut_rejected) = layered(&mut cash_cut);

    // Successor LaunchState with one extra inventory unit (free token
    // attempt). The opening root changes with it, and the re-derived
    // canonical execution must disagree with the tampered claims.
    let mut free_tokens = canonical.clone();
    let free_tokens_replacement = match free_tokens.output_openings[state_index].unwrap() {
        NativeOpening::Launch(noid_tx::native_assets::launch::LaunchOpening::State(mut state)) => {
            state.inventory_units += 1;
            NativeOpening::Launch(noid_tx::native_assets::launch::LaunchOpening::State(state))
        }
        _ => unreachable!(),
    };
    free_tokens.output_openings[state_index] = Some(free_tokens_replacement.clone());
    output_mut(&mut free_tokens, state_index).owner = free_tokens_replacement.root();
    let (free_tokens_structure_ok, free_tokens_rejected) = layered(&mut free_tokens);

    // A token payout inflated by one unit.
    let mut token_bump = canonical.clone();
    let token_index = token_bump
        .output_openings
        .iter()
        .position(|opening| matches!(opening, Some(NativeOpening::TokenBalance(_))))
        .ok_or("missing token payout")?;
    let token_replacement = match token_bump.output_openings[token_index].unwrap() {
        NativeOpening::TokenBalance(mut balance) => {
            balance.units += 1;
            NativeOpening::TokenBalance(balance)
        }
        _ => unreachable!(),
    };
    token_bump.output_openings[token_index] = Some(token_replacement);
    output_mut(&mut token_bump, token_index).owner = token_replacement.root();
    let (token_bump_structure_ok, token_payout_bump_rejected) = layered(&mut token_bump);

    // A NOID payout amount inflated by one unit inside the page itself.
    let mut noid_bump = canonical.clone();
    let noid_index = noid_bump
        .output_openings
        .iter()
        .position(|opening| opening.is_none())
        .ok_or("missing plain NOID payout")?;
    output_mut(&mut noid_bump, noid_index).amount += 1;
    let (noid_bump_structure_ok, noid_payout_bump_rejected) = layered(&mut noid_bump);

    // Permuted input claims (canonical order enforced).
    let mut inputs = prepared.inputs().to_vec();
    if inputs.len() >= 2 {
        inputs.swap(0, 1);
    }
    let permuted_inputs_rejected = prepare_trade_batch_from_claims(
        &valid.state,
        valid.boundary,
        12,
        &valid.batch,
        &inputs,
        prepared.outputs(),
    )
    .is_err();

    // Graduation batch: an invalid market successor (pool reserves inflated).
    let grad_sold = fixture.launch.sale_units() - 1_000;
    let grad_curve = curve_state(fixture, grad_sold)?;
    let grad_specs = [OrderSpec {
        side: LaunchSide::Buy,
        gross: fixture.launch.graduation_cash() - grad_curve.market_noid + 10_000,
        minimum: 1,
    }];
    let grad_valid = accepted_fixture(
        fixture,
        MarketStart::Curve {
            definition: fixture.launch,
            state: grad_curve,
        },
        &grad_specs,
    )?;
    let grad_prepared = prepare_trade_batch(
        &grad_valid.state,
        grad_valid.boundary,
        &grad_valid.batch,
        &grad_valid.slots,
    )?;
    let grad_canonical = NativeIntent::from_trade_execution(grad_prepared.execution(), |_| None)?;
    let mut pool_bump = grad_canonical.clone();
    let pool_index = pool_bump
        .output_openings
        .iter()
        .position(|opening| matches!(opening, Some(NativeOpening::Market(MarketOpening::Pool(_)))))
        .ok_or("missing pool successor")?;
    let pool_replacement = match pool_bump.output_openings[pool_index].unwrap() {
        NativeOpening::Market(MarketOpening::Pool(mut successor)) => {
            successor.base_reserve += 1;
            NativeOpening::Market(MarketOpening::Pool(successor))
        }
        _ => unreachable!(),
    };
    pool_bump.output_openings[pool_index] = Some(pool_replacement);
    output_mut(&mut pool_bump, pool_index).owner = pool_replacement.root();
    let (pool_bump_structure_ok, graduation_pool_bump_rejected) = (
        pool_bump.validate_structure().is_ok(),
        pool_bump.validate_structure().is_err()
            || prepare_trade_batch_from_intent(
                &grad_valid.state,
                grad_valid.boundary,
                12,
                &grad_valid.batch,
                &pool_bump,
            )
            .is_err(),
    );

    // Fee floor: a fee limit below every quoted fee can never be charged.
    // TradePolicy::validate rejects fee_limit == 0 outright, so probe the
    // boundary with fee_limit == 1 against the real parent-state quotes.
    let mut cheap: Vec<_> = valid.batch.policies().to_vec();
    for policy in &mut cheap {
        policy.policy.fee_limit = 1;
    }
    let cheap_batch = TradeBatch::new(valid.batch.parent(), cheap)?;
    let quotes = trade_fee_quotes(&valid.state, &cheap_batch)?;
    let min_quoted_fee = quotes.iter().map(|quote| quote.0[0]).min().unwrap_or(0);
    assert!(
        min_quoted_fee > 1,
        "fixture fee quotes unexpectedly small: {min_quoted_fee}"
    );
    // Fresh payout slots in the cheap batch's own sorted policy order; the
    // reduced fee limit changes policy roots and can change the sort.
    let mut cheap_next = 2_000u32;
    let cheap_slots = TradeOutputSlots {
        payouts: cheap_batch
            .policies()
            .iter()
            .map(|intent| {
                let count = if intent.policy.side == LaunchSide::Buy {
                    2
                } else {
                    1
                };
                (0..count)
                    .map(|_| {
                        let slot = cheap_next;
                        cheap_next += 1;
                        slot
                    })
                    .collect()
            })
            .collect(),
        market: cheap_next,
    };
    let fee_limit_one_rejected =
        prepare_trade_batch(&valid.state, valid.boundary, &cheap_batch, &cheap_slots).is_err();

    // Duplicate funding slot across two intents. Build a [Sell, Buy] fixture
    // so the buy's single NOID claim can be redirected onto the sell's NOID
    // claim slot without breaking the intra-intent ascending-slot rule.
    let dup_specs = [
        OrderSpec {
            side: LaunchSide::Sell,
            gross: 1_000,
            minimum: 1,
        },
        OrderSpec {
            side: LaunchSide::Buy,
            gross: 10_000,
            minimum: 1,
        },
    ];
    let dup_valid = accepted_fixture(
        fixture,
        MarketStart::Curve {
            definition: fixture.launch,
            state: curve,
        },
        &dup_specs,
    )?;
    let mut duplicate: Vec<_> = dup_valid.batch.policies().to_vec();
    let sell_noid_slot = duplicate[0]
        .funding
        .iter()
        .find(|claim| claim.opening.is_none())
        .ok_or("sell intent missing NOID claim")?
        .input
        .slot_index;
    let buy_index = duplicate
        .iter()
        .position(|intent| intent.policy.side == LaunchSide::Buy)
        .ok_or("missing buy intent")?;
    for claim in &mut duplicate[buy_index].funding {
        if claim.opening.is_none() {
            claim.input.slot_index = sell_noid_slot;
        }
    }
    let duplicate_funding_rejected = TradeBatch::new(dup_valid.batch.parent(), duplicate).is_err();

    // Output slot colliding with the occupied parent slot.
    let mut collision = valid.slots.clone();
    if let Some(first) = collision.payouts.first_mut() {
        if let Some(slot) = first.first_mut() {
            *slot = valid.batch.parent().slot_index;
        }
    }
    let slot_collision_rejected =
        prepare_trade_batch(&valid.state, valid.boundary, &valid.batch, &collision).is_err();

    Ok(json!({
        "experiment": "canonical_curve_batch_tampering_and_hygiene",
        "tamper_rejection_layers": {
            "successor_cash_cut": if successor_cash_cut_structure_ok { "execution_rederivation" } else { "page_structure_amount_binding" },
            "successor_free_tokens": if free_tokens_structure_ok { "execution_rederivation" } else { "page_structure_amount_binding" },
            "token_payout_bump": if token_bump_structure_ok { "execution_rederivation" } else { "page_structure_amount_binding" },
            "noid_payout_bump": if noid_bump_structure_ok { "execution_rederivation" } else { "page_structure" },
            "graduation_pool_bump": if pool_bump_structure_ok { "execution_rederivation" } else { "page_structure_amount_binding" }
        },
        "successor_cash_cut_rejected": successor_cash_cut_rejected,
        "successor_free_tokens_rejected": free_tokens_rejected,
        "token_payout_bump_rejected": token_payout_bump_rejected,
        "noid_payout_bump_rejected": noid_payout_bump_rejected,
        "permuted_input_claims_rejected": permuted_inputs_rejected,
        "graduation_pool_successor_bump_rejected": graduation_pool_bump_rejected,
        "fee_limit_one_rejected": fee_limit_one_rejected,
        "min_quoted_fee_in_fixture": min_quoted_fee,
        "duplicate_funding_slot_rejected": duplicate_funding_rejected,
        "output_slot_collision_rejected": slot_collision_rejected
    }))
}

fn sequencing_determinism() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let pool = PoolState::bootstrap_permanent(fixture.market, 400_000_000, 40_000_000_000)?;
    let specs = [
        OrderSpec {
            side: LaunchSide::Buy,
            gross: 500_000_000,
            minimum: 1,
        },
        OrderSpec {
            side: LaunchSide::Sell,
            gross: 30_000_000,
            minimum: 1,
        },
        OrderSpec {
            side: LaunchSide::Buy,
            gross: 40_000,
            minimum: 1,
        },
    ];
    let valid = accepted_fixture(
        fixture,
        MarketStart::Market {
            definition: fixture.market,
            state: pool,
        },
        &specs,
    )?;
    let first = SequencingPlan::execute(
        valid.batch.parent().market,
        valid.boundary,
        valid.batch.policies(),
    )?;
    let second = SequencingPlan::execute(
        valid.batch.parent().market,
        valid.boundary,
        valid.batch.policies(),
    )?;
    assert_eq!(first, second);
    assert_eq!(first.fills().len(), specs.len());
    let order: Vec<_> = first
        .fills()
        .iter()
        .map(|fill| {
            json!({
                "source_index": fill.source_index,
                "side": format!("{:?}", fill.outcome.side),
                "gross": fill.outcome.gross_input,
                "output": fill.outcome.output
            })
        })
        .collect();
    // A fresh equivalent batch object must produce the identical plan.
    let rebuilt = TradeBatch::new(valid.batch.parent(), valid.batch.policies().to_vec())?;
    let third =
        SequencingPlan::execute(rebuilt.parent().market, valid.boundary, rebuilt.policies())?;
    assert_eq!(third, first);
    let prepared = prepare_trade_batch(&valid.state, valid.boundary, &valid.batch, &valid.slots)?;
    let prepared_order: Vec<_> = prepared
        .plan()
        .fills()
        .iter()
        .map(|fill| fill.source_index)
        .collect();
    let plan_order: Vec<_> = first.fills().iter().map(|fill| fill.source_index).collect();
    assert_eq!(prepared_order, plan_order);
    Ok(json!({
        "experiment": "sequencing_determinism_and_price_direction",
        "execution_order": order,
        "plan_deterministic_across_runs_and_batch_objects": true,
        "prepared_batch_matches_plan_order": true
    }))
}

fn main() -> AuditResult<()> {
    let mut results = Vec::new();
    results.push(pool_boundary_matrix()?);
    eprintln!("pool boundary matrix complete");
    results.push(curve_boundary_matrix()?);
    eprintln!("curve boundary matrix complete");
    results.push(flat_region_and_dust_boundaries()?);
    eprintln!("flat-region and dust boundaries complete");
    results.push(symmetry_search()?);
    eprintln!("symmetry search complete");
    results.push(accepted_batch_and_tampering()?);
    eprintln!("canonical batch and tamper checks complete");
    results.push(graduation_acceptance()?);
    eprintln!("graduation acceptance complete");
    results.push(canonical_curve_tampering()?);
    eprintln!("canonical curve tampering complete");
    results.push(sequencing_determinism()?);
    eprintln!("sequencing determinism complete");
    results.push(recursive_differential()?);
    eprintln!("recursive differential complete");
    let report = json!({
        "pinned_commit": "52938433a30b185ab007afaa15166a632cf2e82e",
        "source": "git archive of pinned commit under experiments/dex-economics/pinned",
        "live_targets": false,
        "accepted_value_extraction_found": false,
        "results": results
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
