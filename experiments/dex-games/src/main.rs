// SPDX-License-Identifier: Apache-2.0
//
// dex-games audit: who captures the bonding curve's pre-paid rounding surplus,
// and by what ordering rule. Local library PoCs against the pinned production
// crates; no node, mempool server, peer, or network target is started.

use std::error::Error;

use noid_chain::state::native_assets::native_chain_id;
use noid_poseidon2b::primitives::Address;
use noid_tx::asset_backing::TokenDenomination;
use noid_tx::native_assets::launch::sequential::{quote_curve_exact_input, TradeState};
use noid_tx::native_assets::launch::{LaunchDefinition, LaunchSide, LaunchState};
use noid_tx::native_assets::market::sequential::quote_exact_input;
use noid_tx::native_assets::market::MARKET_QUANTITY_MAX;
use noid_tx::native_assets::trade::batch::{TradeBatch, TradeParent};
use noid_tx::native_assets::trade::sequencing::{MarketStart, SequencingPlan, TradeBoundary};
use noid_tx::native_assets::trade::{FundingPriority, TradeIntent, TradePolicy};
use noid_tx::native_assets::transition::NativeInputClaim;
use noid_tx::native_assets::{
    native_creation_id, FungibleDefinition, GenesisAnchor, IssuanceRoute, TOKEN_DECIMALS,
    TOKEN_UNITS,
};
use noid_tx::TxInput;
use serde_json::{json, Value};

const HEIGHT: u64 = 279_371;
const FEE_LIMIT: u64 = 1_000_000;
const MIN_SUPPLY: u64 = 100_000 * TOKEN_UNITS;
const MAX_SUPPLY: u64 = (MARKET_QUANTITY_MAX / 900) / TOKEN_UNITS * TOKEN_UNITS;
const MID_SUPPLY: u64 = (MIN_SUPPLY + MAX_SUPPLY) / 2 / TOKEN_UNITS * TOKEN_UNITS;

type AuditResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
struct MarketFixture {
    token: FungibleDefinition,
    launch: LaunchDefinition,
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
    Ok(MarketFixture { token, launch })
}

fn curve_state(fixture: MarketFixture, sold: u64) -> AuditResult<LaunchState> {
    match fixture.launch.after_initial_purchase(sold)? {
        TradeState::Curve(state) => Ok(state),
        TradeState::Market(_) => Err("sold at cap did not produce a curve".into()),
    }
}

/// A synthetic buy intent funded by exactly one NOID UTXO at
/// (`slot_index`, `creation_id`), so the funding-priority anchor is that UTXO.
fn buy_intent(
    fixture: MarketFixture,
    boundary: TradeBoundary,
    actor: Address,
    slot_index: u32,
    creation_id: u64,
    gross: u64,
    fee_limit: u64,
) -> AuditResult<TradeIntent> {
    let policy = TradePolicy {
        chain: boundary.chain,
        epoch_anchor: boundary.epoch_anchor,
        owner: actor,
        asset: fixture.token.asset,
        recipient: actor,
        side: LaunchSide::Buy,
        gross_input: gross,
        minimum_output: 1,
        fee_limit,
        valid_from: HEIGHT - 1,
        valid_until: HEIGHT + 8,
    };
    let amount = gross + fee_limit + 100_000;
    let intent = TradeIntent {
        policy,
        funding: vec![NativeInputClaim {
            input: TxInput {
                slot_index,
                amount,
                creation_id,
            },
            root: actor,
            opening: None,
        }],
    };
    intent.checked(boundary.chain, boundary.epoch_anchor, boundary.height)?;
    Ok(intent)
}

fn boundary(parent_id: [u8; 32]) -> TradeBoundary {
    TradeBoundary {
        chain: native_chain_id(),
        epoch_anchor: [0x31; 32],
        parent_id,
        height: HEIGHT,
    }
}

fn trade_parent(fixture: MarketFixture, sold: u64) -> AuditResult<TradeParent> {
    Ok(TradeParent {
        slot_index: 1,
        creation_id: native_creation_id(1)?,
        market: MarketStart::Curve {
            definition: fixture.launch,
            state: curve_state(fixture, sold)?,
        },
    })
}

/// Width of the free tail: the largest k with C(cap - k) == C(cap).
fn flat_tail_width(launch: LaunchDefinition) -> u64 {
    let cap = launch.sale_units();
    let top = launch.curve_cash(cap).expect("cash at cap");
    let mut k = 0u64;
    while k + 1 <= cap && launch.curve_cash(cap - k - 1).expect("cash") == top {
        k += 1;
    }
    k
}

/// Width of the first flat step at the head: the largest q with C(q) == C(1).
fn flat_head_width(launch: LaunchDefinition) -> u64 {
    let first = launch.curve_cash(1).expect("cash at one unit");
    let mut q = 1u64;
    while q + 1 <= launch.sale_units() && launch.curve_cash(q + 1).expect("cash") == first {
        q += 1;
    }
    q
}

/// Exact marginal price at the cap as (numerator, denominator) over 128 bits:
/// (3S + 10*cap) / (900S).
fn cap_price_fraction(launch: LaunchDefinition) -> (u128, u128) {
    let s = u128::from(launch.initial_supply);
    let q = u128::from(launch.sale_units());
    (3 * s + 10 * q, 900 * s)
}

// ---------------------------------------------------------------------------
// Game 1 (prize): the size of the free tail.
// ---------------------------------------------------------------------------

fn tail_snipe_prize() -> AuditResult<Value> {
    let mut rows = Vec::new();
    for (name, supply) in [
        ("minimum", MIN_SUPPLY),
        ("mid", MID_SUPPLY),
        ("maximum", MAX_SUPPLY),
    ] {
        let fixture = fixture(supply)?;
        let launch = fixture.launch;
        let cap = launch.sale_units();
        let width = flat_tail_width(launch);
        let top = launch.curve_cash(cap)?;

        // A 1-NOID gross buy at sold = cap - width sweeps the whole free tail
        // and graduates the market, paying zero.
        let state = curve_state(fixture, cap - width)?;
        let quote = quote_curve_exact_input(launch, state, LaunchSide::Buy, 1, 1)?;
        assert_eq!(quote.spent_input(), 0, "free-tail sweep must spend zero");
        assert_eq!(quote.output(), width, "sweep must take the whole flat tail");
        assert_eq!(quote.refund_input(), 1);
        assert!(matches!(quote.after(), TradeState::Market(_)));

        // Inside the window the same holds for any starting depth.
        let mid = curve_state(fixture, cap - width / 2)?;
        let mid_quote = quote_curve_exact_input(launch, mid, LaunchSide::Buy, 1, 1)?;
        assert_eq!(mid_quote.spent_input(), 0);
        assert_eq!(mid_quote.output(), width - width / 2);

        // Just below the window the buy pays real cash again (control).
        let below = curve_state(fixture, cap - width - 1)?;
        let below_quote = quote_curve_exact_input(launch, below, LaunchSide::Buy, 1, 1)?;
        assert!(below_quote.spent_input() >= 1);

        // Exact value of the free units at the cap's marginal price.
        let (num, den) = cap_price_fraction(launch);
        let exact_value_noid = (u128::from(width) * num + den - 1) / den; // ceil

        // Closure identity: the exact integral of the marginal price over the
        // whole curve equals the graduation cash exactly, so the curve's
        // cumulative ceiling overcharge at the cap is ZERO — every unit of
        // rounding surplus collected on the way up is returned as the free
        // tail (the pre-paid prize the tail race fights over).
        let s = u128::from(launch.initial_supply);
        let c = u128::from(cap);
        assert_eq!(
            900 * s * u128::from(top),
            3 * s * c + 5 * c * c,
            "exact integral to the cap must equal graduation cash"
        );
        // The excess one window below the cap is the free tail's value.
        let below_width = u128::from(cap - width);
        let exact_below_times = 3 * s * below_width + 5 * below_width * below_width;
        let excess = 900u128 * s * u128::from(top) - exact_below_times;
        let excess_noid = excess.div_ceil(900 * s);
        assert_eq!(
            excess_noid,
            u128::from(exact_value_noid),
            "the pre-paid surplus at the window edge is the tail's value"
        );

        // Monetization: sell the free units into the opening pool. The tail is
        // so small that its own proceeds round to zero and the pool's dust
        // floor rejects the standalone sale; record that, and the smallest
        // bundle that clears.
        let pool = match quote.after() {
            TradeState::Market(pool) => pool,
            _ => unreachable!("graduation verified above"),
        };
        let standalone = quote_exact_input(
            launch.market_definition(),
            pool,
            fixture.token.asset,
            width,
            1,
        );
        let standalone_dust = standalone.is_err();
        let mut min_lot = width;
        if standalone_dust {
            let mut lo = width;
            let mut hi = width * 4;
            while quote_exact_input(launch.market_definition(), pool, fixture.token.asset, hi, 1)
                .is_err()
            {
                lo = hi;
                hi = hi.saturating_mul(2);
            }
            while lo + 1 < hi {
                let middle = lo + (hi - lo) / 2;
                if quote_exact_input(
                    launch.market_definition(),
                    pool,
                    fixture.token.asset,
                    middle,
                    1,
                )
                .is_ok()
                {
                    hi = middle;
                } else {
                    lo = middle;
                }
            }
            min_lot = hi;
        }
        let bundled = quote_exact_input(
            launch.market_definition(),
            pool,
            fixture.token.asset,
            min_lot,
            1,
        )?;

        rows.push(json!({
            "supply": name,
            "supply_units": supply,
            "graduation_cash_micronoid": top,
            "flat_tail_units": width,
            "sweep_spent_micronoid": quote.spent_input(),
            "sweep_output_units": quote.output(),
            "exact_tail_value_micronoid": exact_value_noid as u64,
            "standalone_pool_sale_dust_rejected": standalone_dust,
            "min_sellable_lot_units": min_lot,
            "min_lot_sale_gross_micronoid": bundled.output(),
            "below_window_spent_micronoid": below_quote.spent_input(),
        }));
    }
    println!("tail_snipe_prize: complete");
    Ok(json!({ "supplies": rows }))
}

// ---------------------------------------------------------------------------
// Game 1 (race): the tail race inside one batch.
// ---------------------------------------------------------------------------

fn tail_snipe_race() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let launch = fixture.launch;
    let cap = launch.sale_units();
    let width = flat_tail_width(launch);
    let boundary = boundary([0x61; 32]);

    // Find two UTXO coordinates with deterministically ordered digests.
    let actor_a = owner(21);
    let actor_b = owner(22);
    let mut anchors = Vec::new();
    let mut slot = 5_000u32;
    let mut seq = 100u64;
    while anchors.len() < 2 {
        let intent = buy_intent(fixture, boundary, actor_a, slot, seq, 1, FEE_LIMIT)?;
        let priority = intent.funding_priority(boundary.parent_id)?;
        anchors.push(((slot, seq), priority));
        slot += 1;
        seq += 1;
    }
    // Order the two anchors so racer W has the lower digest.
    let (w_anchor, l_anchor) = if anchors[0].1 < anchors[1].1 {
        (anchors[0], anchors[1])
    } else {
        (anchors[1], anchors[0])
    };
    assert_ne!(w_anchor.1.digest, l_anchor.1.digest);

    let winner = buy_intent(
        fixture,
        boundary,
        actor_a,
        w_anchor.0 .0,
        w_anchor.0 .1,
        1,
        FEE_LIMIT,
    )?;
    let loser = buy_intent(
        fixture,
        boundary,
        actor_b,
        l_anchor.0 .0,
        l_anchor.0 .1,
        1,
        FEE_LIMIT,
    )?;

    // Control 1: the loser offers a thousand times the fee ceiling. Both the
    // digest and therefore the outcome are unchanged: fee levels are not a
    // priority dimension.
    let rich_loser = buy_intent(
        fixture,
        boundary,
        actor_b,
        l_anchor.0 .0,
        l_anchor.0 .1,
        1,
        1_000 * FEE_LIMIT,
    )?;
    assert_eq!(
        loser.funding_priority(boundary.parent_id)?,
        rich_loser.funding_priority(boundary.parent_id)?
    );

    // Control 2: the authorization key orders the batch ENCODING only. Grind
    // the loser's recipient so its authorization id is smaller than the
    // winner's while its priority digest stays larger.
    let mut loser_low_key = loser.clone();
    let mut key_order_inverted = false;
    for seed in 1u8..=255 {
        let policy = TradePolicy {
            recipient: Address([seed; 32]),
            ..loser.policy
        };
        let candidate = TradeIntent {
            policy,
            funding: loser.funding.clone(),
        };
        candidate.checked(boundary.chain, boundary.epoch_anchor, boundary.height)?;
        if candidate.authorization_id()?.0 < winner.authorization_id()?.0 {
            assert_eq!(
                candidate.funding_priority(boundary.parent_id)?,
                loser.funding_priority(boundary.parent_id)?
            );
            loser_low_key = candidate;
            key_order_inverted = true;
            break;
        }
    }
    assert!(
        key_order_inverted,
        "recipient grinding must reach a lower auth id"
    );

    // The race: real batch, real sequencing plan, market parked at the tail.
    // Prize reference for the net-economics check below.
    let (num, den) = cap_price_fraction(launch);
    let exact_tail_value_micro = ((u128::from(width) * num + den - 1) / den) as u64;

    let parent = trade_parent(fixture, cap - width)?;
    let batch = TradeBatch::new(parent, vec![loser_low_key.clone(), winner.clone()])?;
    let plan = SequencingPlan::execute(
        parent.market,
        boundary,
        &[loser_low_key.clone(), winner.clone()],
    )?;

    assert_eq!(plan.fills().len(), 1, "only one racer can fill");
    assert_eq!(plan.fills()[0].authorization_id, winner.authorization_id()?);
    assert_eq!(plan.fills()[0].outcome.spent_input, 0);
    assert_eq!(plan.fills()[0].outcome.output, width);
    assert!(matches!(plan.after(), Some(TradeState::Market(_))));
    assert_eq!(plan.unfilled(), &[0], "the higher-digest racer is declined");

    // The miner's stable_policies filter (noid_miner template/trades.rs) drops
    // unfilled policies and retries; the winner-only batch then checks.
    let stable = TradeBatch::new(parent, vec![winner.clone()])?;
    let stable_plan = stable.checked(boundary)?;
    assert_eq!(stable_plan.fills().len(), 1);
    assert_eq!(stable_plan.fills()[0].outcome.output, width);
    // The dropped racer is charged nothing: charges exist only for fills
    // (trade/execution.rs prepare maps quote_charge over plan.fills()).
    assert_eq!(stable.policies().len(), 1);

    // Encoding order (authorization key) disagrees with execution order here:
    // the batch stores the lower-auth-id racer first, but the plan executed
    // the lower-digest racer first.
    assert_eq!(
        batch.policies()[0].authorization_id()?,
        loser_low_key.authorization_id()?
    );
    assert_eq!(
        plan.fills()[0].authorization_id,
        winner.authorization_id()?,
        "execution follows the funding-priority digest, not the encoding key"
    );

    // A loser with a large gross instead of dust does fill against the opening
    // pool in the same batch, at pool prices (repriced, not rejected).
    let big_loser = buy_intent(
        fixture,
        boundary,
        actor_b,
        l_anchor.0 .0,
        l_anchor.0 .1,
        400_000_000,
        FEE_LIMIT,
    )?;
    let big_plan = SequencingPlan::execute(parent.market, boundary, &[big_loser, winner.clone()])?;
    assert_eq!(big_plan.fills().len(), 2);
    assert_eq!(
        big_plan.fills()[0].authorization_id,
        winner.authorization_id()?
    );
    assert_eq!(big_plan.fills()[0].outcome.spent_input, 0);
    assert!(big_plan.fills()[1].outcome.spent_input > 0);
    assert!(big_plan.fills()[1].outcome.market_fee > 0);

    // Two intents funding from the same UTXO cannot enter one batch.
    let clash_a = buy_intent(
        fixture,
        boundary,
        actor_a,
        w_anchor.0 .0,
        w_anchor.0 .1,
        1,
        FEE_LIMIT,
    )?;
    let clash_b = buy_intent(
        fixture,
        boundary,
        actor_b,
        w_anchor.0 .0,
        w_anchor.0 .1,
        1,
        FEE_LIMIT,
    )?;
    assert!(
        TradeBatch::new(parent, vec![clash_a, clash_b]).is_err(),
        "anchor reuse must be rejected"
    );

    // Net economics of the winning snipe at the quietest state geometry:
    // the trade group is 2 live inputs (anchor UTXO + market parent) and 3
    // live outputs (two payouts + market successor), so the protocol-minimum
    // charge dwarfs the prize.
    let breakdown = noid_chain::consensus::fee_breakdown(2, 3, 0, 12);
    assert!(breakdown.required_total >= 9_000);
    assert!(
        breakdown.required_total > 1_000 * u64::from(exact_tail_value_micro),
        "the snipe fee must exceed the prize by three orders of magnitude"
    );

    println!("tail_snipe_race: complete");
    Ok(json!({
        "flat_tail_units": width,
        "tail_prize_micronoid": exact_tail_value_micro,
        "winner_fee_floor_micronoid": breakdown.required_total,
        "winner_by": "lowest funding_priority digest (anchor UTXO + parent id)",
        "winner_spent_micronoid": 0,
        "winner_output_units": width,
        "graduated": true,
        "loser_outcome": "declined (unfilled), retried next block, charged nothing",
        "big_loser_outcome": "filled against the opening pool at pool prices in the same batch",
        "fee_inflation_changes_outcome": false,
        "authorization_key_order_decides_execution": false,
    }))
}

// ---------------------------------------------------------------------------
// Game 4: the priority market itself.
// ---------------------------------------------------------------------------

fn pseudo_random_u64(state: &mut u64) -> u64 {
    // splitmix64
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

fn digest_of(
    fixture: MarketFixture,
    parent_id: [u8; 32],
    slot: u32,
    creation_id: u64,
) -> AuditResult<FundingPriority> {
    let boundary = boundary(parent_id);
    let intent = buy_intent(
        fixture,
        boundary,
        owner(41),
        slot,
        creation_id,
        1,
        FEE_LIMIT,
    )?;
    Ok(intent.funding_priority(boundary.parent_id)?)
}

fn cross_batch_priority() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let parent_one = boundary([0x61; 32]);

    // Invariance: the digest covers only (parent, asset, anchor coordinates).
    let base = buy_intent(fixture, parent_one, owner(31), 7_777, 901, 5, FEE_LIMIT)?;
    let base_priority = base.funding_priority(parent_one.parent_id)?;
    let policy = TradePolicy {
        gross_input: 9_000_000,
        minimum_output: 2,
        fee_limit: 999_000_000,
        recipient: Address([0x99; 32]),
        valid_from: 1,
        valid_until: HEIGHT + 99,
        ..base.policy
    };
    let variant = TradeIntent {
        policy,
        funding: vec![NativeInputClaim {
            input: TxInput {
                slot_index: 7_777,
                amount: 9_000_000 + 999_000_000 + 5,
                creation_id: 901,
            },
            root: base.policy.owner,
            opening: None,
        }],
    };
    variant.checked(parent_one.chain, parent_one.epoch_anchor, parent_one.height)?;
    assert_eq!(
        variant.funding_priority(parent_one.parent_id)?,
        base_priority,
        "intent economics must not move the priority digest"
    );
    assert_ne!(
        variant.authorization_id()?,
        base.authorization_id()?,
        "authorization ids differ while priorities agree"
    );

    // Anchor selection: the principal claim with the largest amount; ties by
    // creation id then slot.
    let actor = owner(32);
    let big = NativeInputClaim {
        input: TxInput {
            slot_index: 8_001,
            amount: 10_000_000,
            creation_id: 77,
        },
        root: actor,
        opening: None,
    };
    let small = NativeInputClaim {
        input: TxInput {
            slot_index: 8_002,
            amount: FEE_LIMIT + 10,
            creation_id: 5,
        },
        root: actor,
        opening: None,
    };
    let two_claim = TradeIntent {
        policy: TradePolicy {
            chain: parent_one.chain,
            epoch_anchor: parent_one.epoch_anchor,
            owner: actor,
            asset: fixture.token.asset,
            recipient: actor,
            side: LaunchSide::Buy,
            gross_input: 1,
            minimum_output: 1,
            fee_limit: FEE_LIMIT,
            valid_from: HEIGHT - 1,
            valid_until: HEIGHT + 8,
        },
        funding: vec![big, small],
    };
    two_claim.checked(parent_one.chain, parent_one.epoch_anchor, parent_one.height)?;
    let two_priority = two_claim.funding_priority(parent_one.parent_id)?;
    let solo_big = buy_intent(fixture, parent_one, actor, 8_001, 77, 1, FEE_LIMIT)?;
    assert_eq!(
        (two_priority.creation_id, two_priority.slot_index),
        (77, 8_001)
    );
    assert_eq!(
        two_priority,
        solo_big.funding_priority(parent_one.parent_id)?,
        "the anchor is the largest principal claim"
    );

    // The lottery: min-of-N uniform digests vs min-of-M. Empirical win rates.
    let mut rng = 0x5eed_1234u64;
    let competitor_utxos = 16u64;
    let mut table = Vec::new();
    for mine in [1u64, 4, 16, 64] {
        let mut wins = 0u64;
        let trials = 4_096u64;
        for _ in 0..trials {
            let mut parent_id = [0u8; 32];
            for chunk in parent_id.chunks_mut(8) {
                chunk.copy_from_slice(&pseudo_random_u64(&mut rng).to_le_bytes());
            }
            let mut best_mine: Option<FundingPriority> = None;
            for _ in 0..mine {
                let slot = (pseudo_random_u64(&mut rng) % 60_000) as u32;
                let seq = 1_000 + pseudo_random_u64(&mut rng) % 1_000_000;
                let p = digest_of(fixture, parent_id, slot, seq)?;
                if best_mine.map_or(true, |b| p < b) {
                    best_mine = Some(p);
                }
            }
            let mut best_other: Option<FundingPriority> = None;
            for _ in 0..competitor_utxos {
                let slot = (pseudo_random_u64(&mut rng) % 60_000) as u32;
                let seq = 2_000_000 + pseudo_random_u64(&mut rng) % 1_000_000;
                let p = digest_of(fixture, parent_id, slot, seq)?;
                if best_other.map_or(true, |b| p < b) {
                    best_other = Some(p);
                }
            }
            if best_mine < best_other {
                wins += 1;
            }
        }
        let empirical = wins as f64 / trials as f64;
        let predicted = mine as f64 / (mine + competitor_utxos) as f64;
        table.push(json!({
            "my_utxos": mine,
            "competitor_utxos": competitor_utxos,
            "empirical_win_rate": empirical,
            "predicted_n_over_n_plus_m": predicted,
        }));
    }

    // Parent re-salt: the same two standing intents swap the winner across
    // parents (the lottery redraws every block).
    let fixed_a = (42_000u32, 555u64);
    let fixed_b = (43_000u32, 556u64);
    let mut first_wins = 0u64;
    let parents = 512u64;
    for _ in 0..parents {
        let mut parent_id = [0u8; 32];
        for chunk in parent_id.chunks_mut(8) {
            chunk.copy_from_slice(&pseudo_random_u64(&mut rng).to_le_bytes());
        }
        let pa = digest_of(fixture, parent_id, fixed_a.0, fixed_a.1)?;
        let pb = digest_of(fixture, parent_id, fixed_b.0, fixed_b.1)?;
        if pa < pb {
            first_wins += 1;
        }
    }
    let first_win_rate = first_wins as f64 / parents as f64;

    println!("cross_batch_priority: complete");
    Ok(json!({
        "digest_inputs": ["parent_id", "asset", "anchor creation_id", "anchor slot_index"],
        "invariant_to": ["gross_input", "minimum_output", "fee_limit", "recipient", "validity window", "owner"],
        "anchor_rule": "largest principal claim; ties by creation id then slot",
        "lottery": table,
        "equal_utxo_first_win_rate_across_parents": first_win_rate,
    }))
}

// ---------------------------------------------------------------------------
// Game 2: what graduation timing control is worth.
// ---------------------------------------------------------------------------

fn graduation_timing() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let launch = fixture.launch;
    let cap = launch.sale_units();
    let width = flat_tail_width(launch);

    // The pool opens at exactly the curve's final marginal price.
    let pool = launch.graduation_pool()?;
    let (num, den) = cap_price_fraction(launch);
    assert_eq!(
        u128::from(pool.base_reserve) * den,
        u128::from(pool.quote_reserve) * num,
        "opening pool price must equal the terminal marginal price"
    );

    // Monetization: the free tail alone cannot be sold into the opening pool
    // (proceeds round below the pool's dust floor); the tail's ~1 NOID of
    // value rides only with a larger bundle.
    let standalone = quote_exact_input(
        launch.market_definition(),
        pool,
        fixture.token.asset,
        width,
        1,
    );
    assert!(
        standalone.is_err(),
        "the free tail alone must be dust-rejected by the pool"
    );

    // Control: a fair-priced pre-graduation buy just below the window, then
    // selling the same units into the opening pool, is a loss (fees+impact).
    let buy_state = curve_state(fixture, cap - width - 1)?;
    let buy = quote_curve_exact_input(launch, buy_state, LaunchSide::Buy, 2_000, 1)?;
    assert!(buy.spent_input() > 0);
    let sell = quote_exact_input(
        launch.market_definition(),
        pool,
        fixture.token.asset,
        buy.output(),
        1,
    )?;
    let roundtrip = i64::from(sell.output() as i64) - i64::from(buy.spent_input() as i64);
    assert!(
        roundtrip < 0,
        "fair-priced pre-graduation roundtrip must lose"
    );

    // The zero-spend winner's same-quote remainder leg: a gross above 1 keeps
    // buying the opening pool in the same fill at pool prices (0.30% fee).
    let sweep_state = curve_state(fixture, cap - width)?;
    let sweep_big = quote_curve_exact_input(launch, sweep_state, LaunchSide::Buy, 10_000_000, 1)?;
    assert_eq!(sweep_big.curve_spent(), 0, "curve leg stays free");
    assert!(sweep_big.market_fee() > 0, "pool leg pays the swap fee");
    assert!(sweep_big.output() > width);

    println!("graduation_timing: complete");
    Ok(json!({
        "opening_pool_base_micronoid": pool.base_reserve,
        "opening_pool_quote_units": pool.quote_reserve,
        "opening_price_equals_terminal_marginal": true,
        "free_tail_standalone_sale_dust_rejected": standalone.is_err(),
        "fair_pregrad_roundtrip_pnl_micronoid": roundtrip,
        "winner_remainder_leg_fee_micronoid": sweep_big.market_fee(),
        "winner_remainder_leg_pool_units": sweep_big.output() - width,
    }))
}

// ---------------------------------------------------------------------------
// Game 3: the launch head.
// ---------------------------------------------------------------------------

fn launch_head() -> AuditResult<Value> {
    let fixture = fixture(MIN_SUPPLY)?;
    let launch = fixture.launch;
    let prefix = flat_head_width(launch);

    // The creator can bundle a purchase into the creation transition itself
    // (launch/owner.rs check_launch_creation): zero, part, or the entire
    // curve — the last graduates instantly, in the creation block.
    let no_purchase = launch.after_initial_purchase(0)?;
    assert!(matches!(no_purchase, TradeState::Curve(_)));
    let prefix_cash = match launch.after_initial_purchase(prefix)? {
        TradeState::Curve(state) => state.market_noid,
        TradeState::Market(_) => unreachable!(),
    };
    let whole_pool = match launch.after_initial_purchase(launch.sale_units())? {
        TradeState::Market(pool) => pool,
        TradeState::Curve(_) => unreachable!("full purchase must graduate"),
    };
    assert_eq!(whole_pool.base_reserve, launch.graduation_cash());

    // Creator cost shape: curve cash + minimum token backing.
    let minimum_backing = TokenDenomination::new(TOKEN_DECIMALS)
        .and_then(|d| d.minimum_backing(launch.initial_supply))
        .map_err(|e| format!("backing: {e:?}"))?;

    // Head pricing: the first NOID buys the flat prefix, and the prefix is a
    // ceiling (slightly overpriced), not a discount.
    let fresh = curve_state(fixture, 0)?;
    let head = quote_curve_exact_input(launch, fresh, LaunchSide::Buy, 1, 1)?;
    assert_eq!(head.output(), prefix);
    assert_eq!(head.spent_input(), 1);
    let s = u128::from(launch.initial_supply);
    let q = u128::from(prefix);
    let exact_prefix = (3 * s * q + 5 * q * q) as f64 / (900.0 * s as f64);
    assert!(exact_prefix < 1.0, "flat prefix is a ceiling overcharge");

    // First-block race on a creator-less launch: two 1-NOID buys, both fill,
    // execution ordered by the same funding-priority digest.
    let race_boundary = boundary([0x71; 32]);
    let mut pair = Vec::new();
    let mut slot = 6_000u32;
    let mut seq = 5_000u64;
    while pair.len() < 2 {
        let intent = buy_intent(
            fixture,
            race_boundary,
            owner(51 + pair.len() as u8),
            slot,
            seq,
            1,
            FEE_LIMIT,
        )?;
        let priority = intent.funding_priority(race_boundary.parent_id)?;
        pair.push((intent, priority));
        slot += 1;
        seq += 1;
    }
    let (first, second) = if pair[0].1 < pair[1].1 {
        (pair[0].clone(), pair[1].clone())
    } else {
        (pair[1].clone(), pair[0].clone())
    };
    let parent = trade_parent(fixture, 0)?;
    let plan = SequencingPlan::execute(
        parent.market,
        race_boundary,
        &[second.0.clone(), first.0.clone()],
    )?;
    assert_eq!(plan.fills().len(), 2, "both head buys fill");
    assert_eq!(
        plan.fills()[0].authorization_id,
        first.0.authorization_id()?
    );
    assert_eq!(plan.fills()[0].outcome.output, prefix);
    assert_eq!(plan.fills()[0].outcome.spent_input, 1);
    assert!(plan.fills()[1].outcome.output >= prefix);
    assert_eq!(plan.fills()[1].outcome.spent_input, 1);

    println!("launch_head: complete");
    Ok(json!({
        "creator_bundle_options": {
            "skip": "state at sold=0",
            "flat_prefix_units": prefix,
            "flat_prefix_cost_micronoid": prefix_cash,
            "entire_curve_cost_micronoid": launch.graduation_cash(),
            "entire_curve_graduates_at_creation": true,
        },
        "creator_minimum_backing_micronoid": minimum_backing,
        "first_buy_units_for_one_noid": head.output(),
        "exact_prefix_value_micronoid": exact_prefix,
        "prefix_is_overcharge_not_discount": true,
        "second_buy_units_for_one_noid": plan.fills()[1].outcome.output,
        "first_block_order_decided_by": "funding-priority digest (identical lottery)",
    }))
}

fn section(name: &str, run: fn() -> AuditResult<Value>) -> AuditResult<Value> {
    run().map_err(|e| format!("section {name}: {e}").into())
}

fn main() -> AuditResult<()> {
    let report = json!({
        "units_note": "all NOID amounts are base units; 1 NOID = 1e6 base units (consensus/params.rs MIN_FEE_BASE)",
        "source_commit": "52938433a30b185ab007afaa15166a632cf2e82e",
        "tail_snipe_prize": section("tail_snipe_prize", tail_snipe_prize)?,
        "tail_snipe_race": section("tail_snipe_race", tail_snipe_race)?,
        "cross_batch_priority": section("cross_batch_priority", cross_batch_priority)?,
        "graduation_timing": section("graduation_timing", graduation_timing)?,
        "launch_head": section("launch_head", launch_head)?,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("ALL ASSERTIONS PASSED");
    Ok(())
}
