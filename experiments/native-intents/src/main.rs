use std::error::Error;
use std::time::Instant;

use noid_chain::fri_state::SlotValue;
use noid_chain::state::native_assets::native_chain_id;
use noid_chain::state::native_assets::trade::{
    apply_prepared_trade_batch, prepare_trade_batch, prepare_trade_batch_from_intent,
    trade_fee_quotes,
};
use noid_chain::ChainState;
use noid_core::hardware::flat_to_tower_u128;
use noid_core::Block128;
use noid_gkr::wallet_authorization::{
    prove_native_trade_authorization, verify_native_trade_authorization,
};
use noid_gkr::OwnerAuthWitness;
use noid_ivc_core::field_circuit::FieldR1csBuilder;
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_recursive::acceptance::trace::native_assets::sequential_trade::market::{
    bind_sequential_market_step, SequentialMarketInputTrace,
};
use noid_recursive::acceptance::trace::{alloc_block, flat_of};
use noid_tx::native_assets::launch::LaunchSide;
use noid_tx::native_assets::market::{MarketDefinition, PoolState, NOID_ASSET};
use noid_tx::native_assets::trade::batch::{TradeBatch, TradeParent};
use noid_tx::native_assets::trade::execution::{
    quote_charge, TradeExecution, TradeFeeQuotes, TradeOutputSlots,
};
use noid_tx::native_assets::trade::sequencing::{MarketStart, TradeBoundary};
use noid_tx::native_assets::trade::{TradeIntent, TradePolicy};
use noid_tx::native_assets::transition::NativeInputClaim;
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    native_creation_id, FungibleDefinition, GenesisAnchor, NativeOpening, TokenBalance,
};
use noid_tx::{OwnerAuthorizationVersion, TxInput, TX_OUTPUTS};

const HEIGHT: u64 = 279_371;
const POOL_SLOT: u32 = 1;
const FUNDING_SLOT: u32 = 2;
const GROSS_INPUT: u64 = 10_000;
const FEE_LIMIT: u64 = 100_000;
const MINIMUM_OUTPUT: u64 = 1;

struct Fixture {
    state: ChainState,
    boundary: TradeBoundary,
    batch: TradeBatch,
    slots: TradeOutputSlots,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let chain = native_chain_id();
        let epoch_anchor = [0x31; 32];
        let owner = derive_address(&SpendSecret::from_bytes([0x42; 32]));
        let anchor = GenesisAnchor {
            chain,
            owner: Address([0x11; 32]),
            input: TxInput {
                slot_index: 99,
                amount: 1_000_000,
                creation_id: 1,
            },
        };
        let token = FungibleDefinition::from_genesis(
            &anchor,
            100_000_000_000,
            noid_tx::native_assets::IssuanceRoute::Launchpad,
            [0x51; 32],
        )?;
        let definition = MarketDefinition::from_genesis(&anchor, NOID_ASSET, token.asset)?;
        let pool = PoolState::bootstrap_permanent(definition, 1_000_000, 1_000_000)?;
        let funding = NativeInputClaim {
            input: TxInput {
                slot_index: FUNDING_SLOT,
                amount: GROSS_INPUT + FEE_LIMIT + 50_000,
                creation_id: 1,
            },
            root: owner,
            opening: None,
        };
        let intent = TradeIntent {
            policy: TradePolicy {
                chain,
                epoch_anchor,
                owner,
                asset: token.asset,
                recipient: Address([0x22; 32]),
                side: LaunchSide::Buy,
                gross_input: GROSS_INPUT,
                minimum_output: MINIMUM_OUTPUT,
                fee_limit: FEE_LIMIT,
                valid_from: HEIGHT - 1,
                valid_until: HEIGHT + 8,
            },
            funding: vec![funding],
        };
        let parent = TradeParent {
            slot_index: POOL_SLOT,
            creation_id: native_creation_id(1)?,
            market: MarketStart::Market {
                definition,
                state: pool,
            },
        };
        let batch = TradeBatch::new(parent, vec![intent])?;
        let state = ChainState::from_sparse_utxos_at(
            5,
            &[
                (
                    POOL_SLOT,
                    SlotValue::with_owner_fields(
                        pool.noid_value(),
                        parent.creation_id,
                        pool.root().as_fields(),
                    ),
                ),
                (
                    FUNDING_SLOT,
                    SlotValue::with_owner_fields(
                        funding.input.amount,
                        funding.input.creation_id,
                        owner.as_fields(),
                    ),
                ),
            ],
            10,
            HEIGHT - 1,
        )
        .map_err(|error| format!("sparse fixture: {error:?}"))?;
        Ok(Self {
            state,
            boundary: TradeBoundary {
                chain,
                epoch_anchor,
                parent_id: [0x61; 32],
                height: HEIGHT,
            },
            batch,
            slots: TradeOutputSlots {
                payouts: vec![vec![10, 11]],
                market: 12,
            },
        })
    }

    fn intent(&self) -> &TradeIntent {
        &self.batch.policies()[0]
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn output_mut(intent: &mut NativeIntent, index: usize) -> &mut noid_tx::TxOutput {
    &mut intent.pages[1 + index / TX_OUTPUTS].body.outputs[index % TX_OUTPUTS]
}

fn host_execution(f: &Fixture) -> Result<(), Box<dyn Error>> {
    let prepared = prepare_trade_batch(&f.state, f.boundary, &f.batch, &f.slots)?;
    let outcome = prepared.plan().fills()[0].outcome;
    assert_eq!(outcome.spent_input, GROSS_INPUT);
    assert!(outcome.output > MINIMUM_OUTPUT);
    assert!(prepared.fee() < FEE_LIMIT);
    println!(
        "canonical host fill: spent_input={} output={} minimum={} network_fee={} fee_limit={}",
        outcome.spent_input,
        outcome.output,
        MINIMUM_OUTPUT,
        prepared.fee(),
        FEE_LIMIT
    );
    println!(
        "authorization_id={}",
        hex(&f.intent().authorization_id()?.0)
    );

    let canonical = NativeIntent::from_trade_execution(prepared.execution(), |_| None)?;
    let replay = prepare_trade_batch_from_intent(&f.state, f.boundary, 5, &f.batch, &canonical)?;
    assert_eq!(replay.outputs(), prepared.outputs());
    println!("canonical NativeIntent accepted by block/template preparation: yes");

    let mut reduced = canonical.clone();
    let index = reduced
        .output_openings
        .iter()
        .position(|opening| {
            matches!(opening, Some(NativeOpening::TokenBalance(TokenBalance { owner, .. })) if *owner == f.intent().policy.recipient)
        })
        .expect("recipient token payout");
    let replacement = NativeOpening::TokenBalance(TokenBalance {
        asset: f.intent().policy.asset,
        owner: f.intent().policy.recipient,
        units: MINIMUM_OUTPUT,
        unlock_height: 0,
    });
    reduced.output_openings[index] = Some(replacement);
    let output = output_mut(&mut reduced, index);
    output.amount = replacement.amount_micronoid();
    output.owner = replacement.root();
    reduced.validate_structure()?;
    let error = prepare_trade_batch_from_intent(&f.state, f.boundary, 5, &f.batch, &reduced)
        .expect_err("minimum-output substitution must not be accepted");
    println!("minimum-output substitution rejected: {error:?}");

    let hostile_fees = vec![TradeFeeQuotes([FEE_LIMIT; 3])];
    let hostile = TradeExecution::new(&f.batch, f.boundary, &hostile_fees, &f.slots)?;
    assert_eq!(hostile.fee(), FEE_LIMIT);
    assert_eq!(hostile.plan().fills()[0].outcome.output, outcome.output);
    let hostile = NativeIntent::from_trade_execution(&hostile, |_| None)?;
    hostile.validate_structure()?;
    let error = prepare_trade_batch_from_intent(&f.state, f.boundary, 5, &f.batch, &hostile)
        .expect_err("caller-selected fee_limit must not be accepted");
    println!("low-level fee_limit execution constructed, block/template preparation rejected it: {error:?}");

    let mut fabricated = outcome;
    fabricated.output = MINIMUM_OUTPUT;
    let charge = quote_charge(
        f.intent(),
        f.boundary,
        fabricated,
        TradeFeeQuotes([FEE_LIMIT; 3]),
    )?;
    assert_eq!(charge.fee, FEE_LIMIT);
    assert_eq!(charge.payouts.len(), 2);
    println!(
        "isolated quote_charge accepts fabricated outcome=minimum and fee=fee_limit ({} payouts); this witness is not an accepted execution",
        charge.payouts.len()
    );

    let quotes = trade_fee_quotes(&f.state, &f.batch)?;
    let exact = TradeExecution::from_claims(
        &f.batch,
        f.boundary,
        &quotes,
        prepared.inputs(),
        prepared.outputs(),
    )?;
    assert_eq!(exact.fee(), prepared.fee());
    println!("parent-State fee quotes and exact claims independently reconstructed: yes");

    let mut post = f.state.clone();
    apply_prepared_trade_batch(&mut post, &prepared)?;
    assert_eq!(post.state.slot(POOL_SLOT), SlotValue::EMPTY);
    assert_eq!(post.state.slot(FUNDING_SLOT), SlotValue::EMPTY);
    println!("checked canonical transition applied to local State: yes");

    let replay_boundary = TradeBoundary {
        parent_id: [0x62; 32],
        height: HEIGHT + 1,
        ..f.boundary
    };
    f.intent().policy.check_context(
        replay_boundary.chain,
        replay_boundary.epoch_anchor,
        replay_boundary.height,
    )?;
    let replay_slots = TradeOutputSlots {
        payouts: vec![vec![20, 21]],
        market: 22,
    };
    let error = prepare_trade_batch(&post, replay_boundary, &f.batch, &replay_slots)
        .expect_err("consumed sources must prevent same-anchor replay");
    println!("same-anchor, next-height, different-parent replay rejected: {error:?}");
    Ok(())
}

fn owner_capsule(f: &Fixture) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let bundle = prove_native_trade_authorization(
        f.intent(),
        f.boundary.chain,
        f.boundary.epoch_anchor,
        f.boundary.height,
        OwnerAuthWitness::new(SpendSecret::from_bytes([0x42; 32])),
        OwnerAuthorizationVersion::V4,
    )?;
    println!("owner capsule generated in {:?}", started.elapsed());
    verify_native_trade_authorization(
        f.intent(),
        f.boundary.chain,
        f.boundary.epoch_anchor,
        f.boundary.height,
        &bundle,
        OwnerAuthorizationVersion::V4,
    )?;
    verify_native_trade_authorization(
        f.intent(),
        f.boundary.chain,
        f.boundary.epoch_anchor,
        HEIGHT + 1,
        &bundle,
        OwnerAuthorizationVersion::V4,
    )?;
    println!("same capsule verifies at adjacent heights under one epoch anchor: yes");
    assert!(verify_native_trade_authorization(
        f.intent(),
        f.boundary.chain,
        [0x71; 32],
        HEIGHT + 1,
        &bundle,
        OwnerAuthorizationVersion::V4,
    )
    .is_err());
    assert!(verify_native_trade_authorization(
        f.intent(),
        [0x72; 32],
        f.boundary.epoch_anchor,
        HEIGHT + 1,
        &bundle,
        OwnerAuthorizationVersion::V4,
    )
    .is_err());
    println!("same capsule with wrong epoch anchor or chain rejected: yes");

    let alternate_parent = TradeBoundary {
        parent_id: [0x63; 32],
        ..f.boundary
    };
    let slots = TradeOutputSlots {
        payouts: vec![vec![20, 21]],
        market: 22,
    };
    let prepared = prepare_trade_batch(&f.state, alternate_parent, &f.batch, &slots)?;
    verify_native_trade_authorization(
        f.intent(),
        alternate_parent.chain,
        alternate_parent.epoch_anchor,
        alternate_parent.height,
        &bundle,
        OwnerAuthorizationVersion::V4,
    )?;
    println!(
        "parent_id is outside the capsule and changes no single-policy fill: output={} (expected; slot incarnations prevent two spends on one State)",
        prepared.plan().fills()[0].outcome.output
    );
    Ok(())
}

fn recursive_quote(expected: u64) -> Result<(), Box<dyn Error>> {
    let mut b = FieldR1csBuilder::new();
    let input = SequentialMarketInputTrace {
        live: alloc_block(&mut b, Block128(1)),
        sell: alloc_block(&mut b, Block128(0)),
        base_reserve: alloc_block(&mut b, Block128(1_000_000)),
        quote_reserve: alloc_block(&mut b, Block128(1_000_000)),
        gross_input: alloc_block(&mut b, Block128(GROSS_INPUT as u128)),
        minimum_output: alloc_block(&mut b, Block128(MINIMUM_OUTPUT as u128)),
    };
    let trace = bind_sequential_market_step(&mut b, &input);
    let value = trace.output.eval(b.values());
    let output = flat_to_tower_u128(u128::from(value.lo) | (u128::from(value.hi) << 64));
    assert_eq!(output as u64, expected);
    assert_eq!(trace.output.terms.len(), 1);
    assert_eq!(trace.output.constant, noid_ivc_core::field::F128::ZERO);
    let wire = trace.output.terms[0].0 as usize;
    let (matrix, witness) = b.build();
    assert!(matrix.satisfies(&witness));
    let mut forged = witness;
    forged[wire] = flat_of(Block128(MINIMUM_OUTPUT as u128));
    assert!(!matrix.satisfies(&forged));
    println!(
        "recursive market trace: canonical output={} satisfies R1CS; replacing output wire with minimum={} fails R1CS",
        output, MINIMUM_OUTPUT
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    host_execution(&fixture)?;
    owner_capsule(&fixture)?;
    recursive_quote(9_871)?;
    println!("RESULT: arbitrary output/fee execution and same-State replay were not accepted");
    Ok(())
}
