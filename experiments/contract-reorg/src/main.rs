// Contract-layer semantic idempotence under MDBX reorgs (parano1d audit lane).
//
// Drives real `MdbxChainContext` chains with real templates, real PoW and real
// `apply_next_block`/`apply_reorg_mdbx_with_applier`, where the reorged blocks
// carry real `ObjectOpening` contract pages (refundable payments, allowance
// wallets, period budgets). Every reorged chain is compared field-by-field
// against a never-reorged twin that received the identical block sequence.
//
// The HistoryStep terminal verifier is stubbed `Ok(())` exactly as in the
// storage-reorg lane; contract program semantics are additionally enforced
// here by running `ObjectOpening::check_call` at each inclusion height, the
// same check the production witness preparation runs per contract page.

use anyhow::{bail, Context as _, Result};
use noid_chain::{
    block_id, materialize_accepted_block_state, AcceptedBlockBundle, Block, MdbxChainContext,
    MdbxContextError,
};
use noid_poseidon2b::primitives::Address;
use noid_tx::experimental_object::{applications, ObjectOpening};
use noid_tx::{
    output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

const MINER: [u8; 32] = [0x44; 32];
const PAYER: [u8; 32] = [0x77; 32];
const PAYEE: [u8; 32] = [0xEE; 32];
const SPENDING_KEY: [u8; 32] = [0x5A; 32];
const RECOVERY_KEY: [u8; 32] = [0x66; 32];
const RECIPIENT_A: [u8; 32] = [0x91; 32];
const RECIPIENT_B: [u8; 32] = [0x92; 32];

const FUND_AMOUNT: u64 = 20_000_000;
const CALL_FEE: u64 = 5;
const RULE_MAX_FEE: u64 = 1_000_000;
const FUND_SLOT: u32 = 5_000_000;
const PAYOUT_SLOT: u32 = 6_000_000;
// Distinct payout/successor slots per scenario leg so owner-index reads are
// unambiguous. Twins replay identical bundles, so slots never collide.
const SLOT_BASE: u32 = 7_000_000;

fn slot(n: u32) -> u32 {
    SLOT_BASE + n
}

// ---------------------------------------------------------------------------
// Harness (adapted from the storage-reorg lane; the timestamp delta is
// parameterized so a fork can compress difficulty below the genesis floor or
// relax it back to the floor).
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn bundle_at(
    context: &MdbxChainContext,
    miner_byte: u8,
    candidate_txs: Vec<Transaction>,
    ts_delta: u64,
) -> Result<AcceptedBlockBundle> {
    bundle_at_beating(context, miner_byte, candidate_txs, ts_delta, None)
}

/// Under v2 ASERT the child target is a function of the PARENT timestamp, so
/// two children of one parent always carry equal work. Fork choice then falls
/// to the documented lexicographic hash tie-break; `beat` grinds it.
fn bundle_at_beating(
    context: &MdbxChainContext,
    miner_byte: u8,
    candidate_txs: Vec<Transaction>,
    ts_delta: u64,
    beat: Option<[u8; 32]>,
) -> Result<AcceptedBlockBundle> {
    let parent = *context.tip_header();
    let timestamp = parent.timestamp.saturating_add(ts_delta.max(1));
    let anchor = context.anchor_info()?;
    let target = noid_chain::consensus::next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height.saturating_add(1),
        parent.timestamp,
        timestamp,
    );
    let finalized_active_counts = context.finalized_active_counts()?;
    let (template, _) = noid_chain::consensus::template::build_node_owned_block_template(
        &parent,
        &context.state,
        &finalized_active_counts,
        candidate_txs,
        Address([miner_byte; 32]),
        timestamp,
        target,
    )
    .map_err(|error| anyhow::anyhow!(format!("block template: {error:?}")))?;
    // Grind by mutating only the header nonce: the sealed body is identical
    // for every candidate, so build it once outside the loop.
    let mut block = template.into_block(0);
    let mut nonce = 0u128;
    loop {
        block.header.nonce = nonce;
        if noid_chain::consensus::validate_pow(&block.header).is_ok()
            && beat.is_none_or(|hash| noid_chain::block_id(&block.header) < hash)
        {
            break;
        }
        nonce = nonce.checked_add(1).context("PoW nonce space exhausted")?;
    }
    let mut terminal = noid_chain::HistoryStepTerminalMetadata::new(
        block.header.height,
        noid_chain::block_header::semantic_header_id(&block.header),
        0,
    )?
    .encode_prefix()
    .to_vec();
    terminal.push(0xA5);
    Ok(AcceptedBlockBundle::try_from_parts(
        block.to_bytes(),
        terminal,
    )?)
}

fn decode_block(bytes: &[u8]) -> Result<Block, MdbxContextError> {
    Block::from_bytes(bytes).map_err(|_| MdbxContextError::Corrupt("PoC generated malformed block"))
}

fn accept(
    context: &mut MdbxChainContext,
    bundle: &AcceptedBlockBundle,
) -> Result<(), MdbxContextError> {
    let block = decode_block(bundle.block_bytes())?;
    context
        .apply_next_block(
            bundle,
            block.header.timestamp,
            |block, state| {
                materialize_accepted_block_state(state, block).map_err(|error| format!("{error:?}"))
            },
            |_| Ok(()),
        )
        .map(|_| ())
}

fn work_of(bundle: &AcceptedBlockBundle) -> [u8; 32] {
    let block = decode_block(bundle.block_bytes()).expect("bundle decodes");
    noid_chain::consensus::block_work(&block.header.difficulty_target)
}

fn add_works(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    noid_chain::consensus::add_work(a, b)
}

fn chain_work(bundles: &[AcceptedBlockBundle]) -> [u8; 32] {
    let mut total = [0u8; 32];
    for bundle in bundles {
        total = add_works(&total, &work_of(bundle));
    }
    total
}

/// Mirror the production fork-choice rule over the suffixes and fail unless
/// the challenger wins under it (work, then the lexicographic hash tie-break).
fn assert_wins_fork_choice(
    challenger: &[AcceptedBlockBundle],
    incumbent: &[AcceptedBlockBundle],
) -> Result<()> {
    let challenger_tip =
        decode_block(challenger.last().context("empty challenger")?.block_bytes())?;
    let incumbent_tip = decode_block(incumbent.last().context("empty incumbent")?.block_bytes())?;
    match noid_chain::consensus::choose_chain_by_work(
        &chain_work(challenger),
        &noid_chain::block_id(&challenger_tip.header),
        &chain_work(incumbent),
        &noid_chain::block_id(&incumbent_tip.header),
    ) {
        noid_chain::consensus::ChainChoice::A => Ok(()),
        other => bail!(
            "challenger does not win fork choice ({other:?}): work {} vs {}, tip {} vs {}",
            hex(&chain_work(challenger)),
            hex(&chain_work(incumbent)),
            hex(&noid_chain::block_id(&challenger_tip.header)),
            hex(&noid_chain::block_id(&incumbent_tip.header))
        ),
    }
}

fn epoch_anchor_for_child(context: &MdbxChainContext, child_height: u64) -> Result<[u8; 32]> {
    let anchor_height = noid_chain::consensus::tx_epoch_anchor_height_for_child(child_height);
    let header = context
        .store
        .get_header(anchor_height)?
        .with_context(|| format!("epoch anchor header at {anchor_height} missing"))?;
    Ok(block_id(&header))
}

/// Ordinary miner spend that funds a contract root (no contract bit).
/// Must be built while the context sits at the parent height.
fn funding_tx(
    context: &MdbxChainContext,
    root: &Address,
    fund_slot: u32,
    change_slot: u32,
) -> Result<Transaction> {
    let parent = *context.tip_header();
    let utxos = context.store.get_verified_utxos_by_owner(&MINER)?.utxos;
    let source = utxos
        .iter()
        .filter(|utxo| utxo.amount > FUND_AMOUNT + 100_000)
        .max_by_key(|utxo| utxo.amount)
        .context("no funded miner UTXO available")?;
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: source.slot_index,
        amount: source.amount,
        creation_id: source.creation_id,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: fund_slot,
        amount: FUND_AMOUNT,
        owner: *root,
    };
    outputs[1] = TxOutput {
        slot_index: change_slot,
        amount: 0,
        owner: Address(MINER),
    };
    let mut body = TxBody {
        epoch_anchor: epoch_anchor_for_child(context, parent.height.saturating_add(1))?,
        fee: 0,
        input_owner: Address(MINER),
        inputs,
        outputs,
        validity_bitmap: 0b1
            | output_bitmap_bit(0)
            | output_bitmap_bit(1)
            | PAGED_SPEND_START_BIT
            | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    };
    body.fee = noid_chain::consensus::required_fee_for_tx_body(
        &body,
        parent.active_slot_count,
        parent.log_slots,
    );
    body.outputs[1].amount = source
        .amount
        .checked_sub(FUND_AMOUNT + body.fee)
        .context("funding source too small")?;
    Ok(Transaction::new(body))
}

/// The contract's live UTXO for a root: (slot, amount, creation_id).
fn root_utxo(context: &MdbxChainContext, root: &Address) -> Result<(u32, u64, u64)> {
    let utxos = context.store.get_verified_utxos_by_owner(&root.0)?.utxos;
    let utxo = utxos.first().context("root owns no live UTXO")?;
    Ok((utxo.slot_index, utxo.amount, utxo.creation_id))
}

/// The template builder silently drops any group whose fee is below the
/// consensus fee floor for its shape at the parent. Build pages at exactly
/// that floor: probe with fee 0 (shape-identical), read the floor, rebuild.
fn min_fee_at(context: &MdbxChainContext, probe_body: &TxBody) -> Result<u64> {
    let parent = *context.tip_header();
    Ok(noid_chain::consensus::required_fee_for_tx_body(
        probe_body,
        parent.active_slot_count,
        parent.log_slots,
    ))
}

fn call_with_min_fee(
    context: &MdbxChainContext,
    opening: &ObjectOpening,
    input: TxInput,
    output_slot: u32,
    height: u64,
    terminal: bool,
) -> Result<noid_tx::TxPage> {
    let anchor = epoch_anchor_for_child(context, height)?;
    let probe = opening.build_call(input, output_slot, 0, anchor, height, terminal)?;
    let fee = min_fee_at(context, &probe.body)?;
    Ok(opening.build_call(input, output_slot, fee, anchor, height, terminal)?)
}

fn payment_with_min_fee(
    context: &MdbxChainContext,
    opening: &ObjectOpening,
    input: TxInput,
    successor_slot: u32,
    height: u64,
    payout: TxOutput,
) -> Result<noid_tx::TxPage> {
    let anchor = epoch_anchor_for_child(context, height)?;
    let probe = opening.build_payment(input, successor_slot, 0, anchor, height, payout)?;
    let fee = min_fee_at(context, &probe.body)?;
    Ok(opening.build_payment(input, successor_slot, fee, anchor, height, payout)?)
}

fn owner_total(context: &MdbxChainContext, owner: &[u8; 32]) -> Result<u64> {
    Ok(context
        .store
        .get_verified_utxos_by_owner(owner)?
        .utxos
        .iter()
        .map(|utxo| utxo.amount)
        .sum())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    root: [u8; 32],
    supply: u128,
    alloc_counter: u64,
    active_slot_count: u64,
    tip_height: u64,
}

fn fingerprint(context: &MdbxChainContext) -> Fingerprint {
    Fingerprint {
        root: context.state.cached_state_root(),
        supply: context.state.circulating_supply_micronoid,
        alloc_counter: context.state.alloc_counter,
        active_slot_count: context.state.active_slot_count,
        tip_height: context.tip_height(),
    }
}

fn assert_twin_match(
    reorged: &MdbxChainContext,
    twin: &MdbxChainContext,
    label: &str,
) -> Result<()> {
    let a = fingerprint(reorged);
    let b = fingerprint(twin);
    if a != b {
        bail!("twin mismatch at {label}: reorged={a:?} twin={b:?}");
    }
    println!(
        "CONTROL twin-equality[{label}]: root={} supply={} alloc={} active={} height={}",
        hex(&a.root),
        a.supply,
        a.alloc_counter,
        a.active_slot_count,
        a.tip_height
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Nine compressed coinbase-only blocks, then a height-10 funding block.
/// Returns the full shared prefix as bundles so every context replays the
/// identical chain (PoW nonces included).
fn funded_prefix(
    producer: &mut MdbxChainContext,
    opening: &ObjectOpening,
) -> Result<Vec<AcceptedBlockBundle>> {
    let mut prefix = Vec::new();
    for _ in 0..9 {
        let bundle = bundle_at(producer, 0x44, Vec::new(), 1)?;
        accept(producer, &bundle)?;
        prefix.push(bundle);
    }
    assert_eq!(producer.tip_height(), 9);
    let fund_tx = funding_tx(producer, &opening.root(), FUND_SLOT, slot(1))?;
    let bundle = bundle_at(producer, 0x44, vec![fund_tx], 1)?;
    accept(producer, &bundle)?;
    assert_eq!(producer.tip_height(), 10);
    prefix.push(bundle);
    Ok(prefix)
}

fn replay(context: &mut MdbxChainContext, bundles: &[AcceptedBlockBundle]) -> Result<()> {
    for bundle in bundles {
        accept(context, bundle)?;
    }
    Ok(())
}

/// Try to mine+apply one more block with `txs`; print where the spend dies.
fn expect_spend_rejected(
    producer: &mut MdbxChainContext,
    victim: &mut MdbxChainContext,
    txs: Vec<Transaction>,
    label: &str,
) -> Result<()> {
    match bundle_at(producer, 0x55, txs, 1) {
        Err(error) => println!("CONTROL {label}: rejected at template: {error}"),
        Ok(bundle) => match accept(victim, &bundle) {
            Err(error) => println!("CONTROL {label}: rejected at apply: {error}"),
            Ok(()) => bail!("{label}: spend unexpectedly accepted"),
        },
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 1: refund paid on the losing fork, reorg, replay on the winner.
// ---------------------------------------------------------------------------
fn s1_refund_on_losing_fork() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let mut producer = MdbxChainContext::open_or_create(&temporary.path().join("producer"))?;

    let opening = applications::refundable_payment(
        Address(PAYER),
        Address(PAYEE),
        40, // deadline: claim phase through height 39
        RULE_MAX_FEE,
    );
    let prefix = funded_prefix(&mut producer, &opening)?;
    let root1 = opening.root();
    let creation = root_utxo(&producer, &root1)?.2;

    // Fork A (height 11): the payee's claim close pays the payee.
    let claim_page = call_with_min_fee(
        &producer,
        &opening,
        TxInput {
            slot_index: FUND_SLOT,
            amount: FUND_AMOUNT,
            creation_id: creation,
        },
        PAYOUT_SLOT,
        11,
        true,
    )?;
    let close_fee = claim_page.body.fee;
    let bundle_a = bundle_at(
        &mut producer,
        0x44,
        vec![Transaction::new(claim_page.body.clone())],
        20,
    )?;
    let hash_a = noid_chain::block_id(&decode_block(bundle_a.block_bytes())?.header);
    accept(&mut producer, &bundle_a)?;
    assert_eq!(owner_total(&producer, &PAYEE)?, FUND_AMOUNT - close_fee);
    println!(
        "CONTROL s1 losing fork paid the payee: payee_total={} at height 11 (close fee {})",
        FUND_AMOUNT - close_fee,
        close_fee
    );

    // Fork B (height 11): empty competitor. Under v2 ASERT both children of
    // one parent carry EQUAL work (the child target is parent-determined), so
    // fork choice falls to the lexicographic hash tie-break, which we grind.
    let mut producer_b = MdbxChainContext::open_or_create(&temporary.path().join("producer_b"))?;
    replay(&mut producer_b, &prefix)?;
    let bundle_b = bundle_at_beating(&mut producer_b, 0x55, Vec::new(), 1, Some(hash_a))?;
    assert_wins_fork_choice(&[bundle_b.clone()], &[bundle_a.clone()])?;
    accept(&mut producer_b, &bundle_b)?;

    // Victim followed fork A, then reorgs to B. Twin never saw fork A.
    let mut victim = MdbxChainContext::open_or_create(&temporary.path().join("victim"))?;
    let mut twin = MdbxChainContext::open_or_create(&temporary.path().join("twin"))?;
    replay(&mut victim, &prefix)?;
    replay(&mut twin, &prefix)?;
    accept(&mut victim, &bundle_a)?;
    assert_eq!(owner_total(&victim, &PAYEE)?, FUND_AMOUNT - close_fee);
    let result = victim.apply_reorg_mdbx_with_applier(
        10,
        &[bundle_b.clone()],
        decode_block(bundle_b.block_bytes())?.header.timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(result.reverted_heights, vec![11]);
    assert_eq!(result.applied_heights, vec![11]);
    assert_eq!(victim.tip_height(), 11);
    // The losing-fork refund evaporated and the obligation UTXO resurrected.
    assert_eq!(owner_total(&victim, &PAYEE)?, 0);
    assert_eq!(
        root_utxo(&victim, &root1)?,
        (FUND_SLOT, FUND_AMOUNT, creation)
    );
    accept(&mut twin, &bundle_b)?;
    assert_twin_match(&victim, &twin, "s1-after-reorg")?;
    println!(
        "CONTROL s1 refund on losing fork rolled back: payee_total=0 root_utxo_resurrected=true"
    );

    // The same page is still executable on the winner: replay it at height 12.
    // KEEP_STATE programs have no height-dependent successor, so the page
    // stays valid throughout the claim phase.
    let replay_page = claim_page.clone();
    let checked = opening.check_call(&replay_page, 12)?;
    assert_eq!(checked.authority, Address(PAYEE));
    let bundle_replay = bundle_at(
        &mut producer_b,
        0x55,
        vec![Transaction::new(replay_page.body.clone())],
        1,
    )?;
    accept(&mut victim, &bundle_replay)?;
    accept(&mut twin, &bundle_replay)?;
    accept(&mut producer_b, &bundle_replay)?;
    assert_eq!(owner_total(&victim, &PAYEE)?, FUND_AMOUNT - close_fee);
    assert_twin_match(&victim, &twin, "s1-after-replay")?;
    println!(
        "CONTROL s1 replay on winner accepted once: payee_total={} exactly-one-satisfaction-on-surviving-chain=true",
        FUND_AMOUNT - close_fee
    );

    // Replaying again must fail: the input is spent on the surviving chain.
    expect_spend_rejected(
        &mut producer_b,
        &mut victim,
        vec![Transaction::new(replay_page.body.clone())],
        "s1 double-satisfaction",
    )?;
    assert_eq!(owner_total(&victim, &PAYEE)?, FUND_AMOUNT - close_fee);

    // Phase selection is verifier-side and fail-closed at the boundary: below
    // the deadline the page demands the claim authority; at or after it the
    // terminal recipient binding flips to the recovery recipient, so the very
    // same page can never be re-verified in the other phase.
    let checked_claim = opening.check_call(&replay_page, 39)?;
    assert_eq!(checked_claim.authority, Address(PAYEE));
    assert!(matches!(
        opening.check_call(&replay_page, 40),
        Err(noid_tx::experimental_object::ObjectError::Recipient)
    ));
    println!(
        "CONTROL s1 phase selection: authority_at(39)=payee; the same terminal page fails Recipient at 40 (recipient binding flips with the phase)"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 2: a heavier SHORTER fork lowers the tip height across a contract
// deadline; the recovery executed on the long fork must fully evaporate.
// A shorter chain can only outweigh a longer one by accumulating difficulty
// compression over several stalled blocks (each successor re-anchors low).
// ---------------------------------------------------------------------------

fn s2_phase_flip_under_height_decrease() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let mut producer = MdbxChainContext::open_or_create(&temporary.path().join("producer"))?;

    let deadline = 16u64;
    let opening =
        applications::refundable_payment(Address(PAYER), Address(PAYEE), deadline, RULE_MAX_FEE);
    let prefix = funded_prefix(&mut producer, &opening)?;
    let root1 = opening.root();
    let creation = root_utxo(&producer, &root1)?.2;

    // Fork A: six floor-relaxed blocks (slow stamps). Height 16 carries the
    // payer's recovery close, valid because 16 >= deadline.
    let mut fork_a = Vec::new();
    for height in 11..=16 {
        let txs = if height == 16 {
            let page = call_with_min_fee(
                &producer,
                &opening,
                TxInput {
                    slot_index: FUND_SLOT,
                    amount: FUND_AMOUNT,
                    creation_id: creation,
                },
                slot(20),
                16,
                true,
            )?;
            vec![Transaction::new(page.body.clone())]
        } else {
            Vec::new()
        };
        let bundle = bundle_at(&mut producer, 0x44, txs, 3_600)?;
        accept(&mut producer, &bundle)?;
        fork_a.push(bundle);
    }
    let recovery_fee = decode_block(fork_a.last().unwrap().block_bytes())?.transactions[1]
        .body
        .fee;
    assert_eq!(owner_total(&producer, &PAYER)?, FUND_AMOUNT - recovery_fee);
    println!(
        "CONTROL s2 long fork executed payer recovery at height 16: payer_total={}",
        FUND_AMOUNT - recovery_fee
    );

    // Fork B: FIVE compressed blocks (stall stamps). Height 15 carries the
    // payee's claim close (15 < deadline). B must outweigh A's six blocks
    // with five: the compression has to accumulate faster than A accrues
    // floor blocks.
    let mut producer_b = MdbxChainContext::open_or_create(&temporary.path().join("producer_b"))?;
    replay(&mut producer_b, &prefix)?;
    let mut fork_b = Vec::new();
    for height in 11..=15 {
        let txs = if height == 15 {
            let page = call_with_min_fee(
                &producer_b,
                &opening,
                TxInput {
                    slot_index: FUND_SLOT,
                    amount: FUND_AMOUNT,
                    creation_id: creation,
                },
                slot(21),
                15,
                true,
            )?;
            vec![Transaction::new(page.body.clone())]
        } else {
            Vec::new()
        };
        let bundle = bundle_at(&mut producer_b, 0x55, txs, 1)?;
        accept(&mut producer_b, &bundle)?;
        fork_b.push(bundle);
    }
    let claim_fee = decode_block(fork_b.last().unwrap().block_bytes())?.transactions[1]
        .body
        .fee;
    assert_eq!(
        opening
            .check_call(
                &tx_page_from(&decode_block(fork_b.last().unwrap().block_bytes())?.transactions[1]),
                15
            )?
            .authority,
        Address(PAYEE)
    );
    assert_wins_fork_choice(&fork_b, &fork_a)?;
    println!(
        "CONTROL s2 shorter fork is heavier: 5 stalled blocks outweigh 6 floor blocks (v2 ASERT re-anchoring compounds compression)"
    );

    // Victim followed A (tip 16, payer paid), then reorgs to the shorter
    // heavier B (tip 15): the tip height DECREASES across the deadline.
    let mut victim = MdbxChainContext::open_or_create(&temporary.path().join("victim"))?;
    let mut twin = MdbxChainContext::open_or_create(&temporary.path().join("twin"))?;
    replay(&mut victim, &prefix)?;
    replay(&mut twin, &prefix)?;
    for bundle in &fork_a {
        accept(&mut victim, bundle)?;
    }
    assert_eq!(victim.tip_height(), 16);
    assert_eq!(owner_total(&victim, &PAYER)?, FUND_AMOUNT - recovery_fee);
    let result = victim.apply_reorg_mdbx_with_applier(
        10,
        &fork_b,
        decode_block(fork_b.last().unwrap().block_bytes())?
            .header
            .timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(result.reverted_heights, vec![16, 15, 14, 13, 12, 11]);
    assert_eq!(result.applied_heights, vec![11, 12, 13, 14, 15]);
    assert_eq!(victim.tip_height(), 15, "tip height must decrease 16 -> 15");
    // The payer's recovery evaporates; the payee's claim is the surviving
    // satisfaction; the contract root is consumed on the winner.
    assert_eq!(owner_total(&victim, &PAYER)?, 0);
    assert_eq!(owner_total(&victim, &PAYEE)?, FUND_AMOUNT - claim_fee);
    assert!(root_utxo(&victim, &root1).is_err());
    for bundle in &fork_b {
        accept(&mut twin, bundle)?;
    }
    assert_twin_match(&victim, &twin, "s2-after-reorg")?;
    println!(
        "CONTROL s2 height-decreasing reorg across deadline: tip 16->15 payer_recovery_evaporated=true payee_claim_survived=true twin_equal=true"
    );

    // The phase machine re-derives from the surviving chain's height: at the
    // new candidate height 15 the payer's already-mined recovery page fails
    // the terminal recipient binding (below the deadline the close belongs
    // to the claim recipient), so it cannot be re-verified on the winner.
    let recovery_page =
        tx_page_from(&decode_block(fork_a.last().unwrap().block_bytes())?.transactions[1]);
    assert!(matches!(
        opening.check_call(&recovery_page, 15),
        Err(noid_tx::experimental_object::ObjectError::Recipient)
    ));
    println!(
        "CONTROL s2 phase re-derived: recovery page fails Recipient at new candidate height 15 (terminal close below the deadline belongs to the claim recipient)"
    );
    Ok(())
}

fn tx_page_from(transaction: &Transaction) -> noid_tx::TxPage {
    noid_tx::TxPage {
        body: transaction.body.clone(),
    }
}

// ---------------------------------------------------------------------------
// Scenario 3: budget counters drawn on the losing fork, reorged away, drawn
// again on the winner; every counter compared with the never-reorged twin.
// ---------------------------------------------------------------------------

fn s3_consumption_counter_divergence() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let mut producer = MdbxChainContext::open_or_create(&temporary.path().join("producer"))?;

    let terms = applications::PeriodBudget {
        start_height: 11,
        period_blocks: 5,
        budget: 2_000_000,
        recover_at: 200,
        max_fee: RULE_MAX_FEE,
        max_payout: 1_500_000,
        min_retained: RULE_MAX_FEE,
    };
    let opening = applications::period_budget_wallet(
        Address(SPENDING_KEY),
        Address(RECOVERY_KEY),
        None,
        terms,
    )?;
    let prefix = funded_prefix(&mut producer, &opening)?;
    let root1 = opening.root();
    let creation = root_utxo(&producer, &root1)?.2;

    // Draw on fork A at height 11: 1,000,000 payout + fee inside the window.
    let draw_a = payment_with_min_fee(
        &producer,
        &opening,
        TxInput {
            slot_index: FUND_SLOT,
            amount: FUND_AMOUNT,
            creation_id: creation,
        },
        slot(30),
        11,
        TxOutput {
            slot_index: slot(31),
            amount: 1_000_000,
            owner: Address(RECIPIENT_A),
        },
    )?;
    let draw_fee = draw_a.body.fee;
    let bundle_a11 = bundle_at(
        &mut producer,
        0x44,
        vec![Transaction::new(draw_a.body.clone())],
        20,
    )?;
    let hash_a = noid_chain::block_id(&decode_block(bundle_a11.block_bytes())?.header);
    accept(&mut producer, &bundle_a11)?;
    assert_eq!(owner_total(&producer, &RECIPIENT_A)?, 1_000_000);
    // Successor state after the draw: budget 2,000,000 - 1,000,000 - fee,
    // window end unchanged at 11 + 5 (no reset: 11 < 16).
    let opened = opening.successor(opening.execute(&draw_a.body, 11)?);
    let successor = root_utxo(&producer, &opened.root())?;
    println!(
        "CONTROL s3 losing-fork draw applied: remaining_budget={} window_end=16 successor_slot={}",
        2_000_000 - 1_000_000 - draw_fee,
        successor.0
    );

    // Fork B: empty heavier block at 11 (tie-break), then a fresh draw at 12
    // on the resurrected full-budget root.
    let mut producer_b = MdbxChainContext::open_or_create(&temporary.path().join("producer_b"))?;
    replay(&mut producer_b, &prefix)?;
    let bundle_b11 = bundle_at_beating(&mut producer_b, 0x55, Vec::new(), 1, Some(hash_a))?;
    assert_wins_fork_choice(&[bundle_b11.clone()], &[bundle_a11.clone()])?;
    accept(&mut producer_b, &bundle_b11)?;
    let draw_b = payment_with_min_fee(
        &producer_b,
        &opening,
        TxInput {
            slot_index: FUND_SLOT,
            amount: FUND_AMOUNT,
            creation_id: creation,
        },
        slot(32),
        12,
        TxOutput {
            slot_index: slot(33),
            amount: 1_000_000,
            owner: Address(RECIPIENT_B),
        },
    )?;
    let bundle_b12 = bundle_at(
        &mut producer_b,
        0x55,
        vec![Transaction::new(draw_b.body.clone())],
        1,
    )?;
    accept(&mut producer_b, &bundle_b12)?;

    // Victim: prefix + draw on A, reorg to B, draw on B.
    let mut victim = MdbxChainContext::open_or_create(&temporary.path().join("victim"))?;
    let mut twin = MdbxChainContext::open_or_create(&temporary.path().join("twin"))?;
    replay(&mut victim, &prefix)?;
    replay(&mut twin, &prefix)?;
    accept(&mut victim, &bundle_a11)?;
    assert_eq!(owner_total(&victim, &RECIPIENT_A)?, 1_000_000);
    victim.apply_reorg_mdbx_with_applier(
        10,
        &[bundle_b11.clone()],
        decode_block(bundle_b11.block_bytes())?.header.timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(
        owner_total(&victim, &RECIPIENT_A)?,
        0,
        "losing-fork draw must vanish"
    );
    accept(&mut victim, &bundle_b12)?;
    accept(&mut twin, &bundle_b11)?;
    accept(&mut twin, &bundle_b12)?;
    assert_twin_match(&victim, &twin, "s3-after-reorg-and-redraw")?;
    // On the surviving chain exactly one draw executed.
    assert_eq!(owner_total(&victim, &RECIPIENT_B)?, 1_000_000);
    assert_eq!(owner_total(&victim, &RECIPIENT_A)?, 0);
    println!(
        "CONTROL s3 counters reverted with the fork: draw-on-A vanished, draw-on-B stands, twin_equal=true (no double-draw, no resurrected budget)"
    );

    // Replaying A's draw now must fail: its input root was consumed by B's draw.
    expect_spend_rejected(
        &mut producer_b,
        &mut victim,
        vec![Transaction::new(draw_a.body.clone())],
        "s3 cross-fork draw replay",
    )?;
    assert_twin_match(&victim, &twin, "s3-after-replay-attempt")?;

    // Budget exhaustion is enforced on the surviving chain's own counter: the
    // remaining budget cannot cover another full payout.
    let winner_opening = opening.successor(opening.execute(&draw_b.body, 12)?);
    let winner_root = winner_opening.root();
    let winner_utxo = root_utxo(&producer_b, &winner_root)?;
    let exhausted = payment_with_min_fee(
        &producer_b,
        &winner_opening,
        TxInput {
            slot_index: winner_utxo.0,
            amount: winner_utxo.1,
            creation_id: winner_utxo.2,
        },
        slot(34),
        13,
        TxOutput {
            slot_index: slot(35),
            amount: 1_000_000,
            owner: Address(RECIPIENT_A),
        },
    );
    match exhausted {
        Err(error) => println!("CONTROL s3 budget exhaustion holds on winner: {error:?}"),
        Ok(page) => bail!("exhausted budget draw built: {:?}", page.body),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 4: two live authorizations racing one allowance across a reorg.
// ---------------------------------------------------------------------------

fn s4_allowance_race_across_forks() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let mut producer = MdbxChainContext::open_or_create(&temporary.path().join("producer"))?;

    let opening = applications::allowance_wallet(
        Address(SPENDING_KEY),
        Address(RECOVERY_KEY),
        None,
        200,
        RULE_MAX_FEE,
        15_000_000,
        1_000_000,
    );
    let prefix = funded_prefix(&mut producer, &opening)?;
    let root1 = opening.root();
    let creation = root_utxo(&producer, &root1)?.2;
    let input = TxInput {
        slot_index: FUND_SLOT,
        amount: FUND_AMOUNT,
        creation_id: creation,
    };

    // Two independent authorized payments, each alone within the per-call
    // limits, both spending the same funded root.
    let page_one = payment_with_min_fee(
        &producer,
        &opening,
        input,
        slot(40),
        11,
        TxOutput {
            slot_index: slot(41),
            amount: 15_000_000,
            owner: Address(RECIPIENT_A),
        },
    )?;
    let page_two = payment_with_min_fee(
        &producer,
        &opening,
        input,
        slot(42),
        11,
        TxOutput {
            slot_index: slot(43),
            amount: 15_000_000,
            owner: Address(RECIPIENT_B),
        },
    )?;

    // Sequential control (lane A's result re-verified locally): on one chain
    // the second spend of the same root cannot apply.
    let bundle_one = bundle_at(
        &mut producer,
        0x44,
        vec![Transaction::new(page_one.body.clone())],
        20,
    )?;
    let hash_one = noid_chain::block_id(&decode_block(bundle_one.block_bytes())?.header);
    accept(&mut producer, &bundle_one)?;
    assert_eq!(owner_total(&producer, &RECIPIENT_A)?, 15_000_000);
    match bundle_at(
        &mut producer,
        0x44,
        vec![Transaction::new(page_two.body.clone())],
        20,
    ) {
        Err(error) => println!("CONTROL s4 sequential double-spend rejected at template: {error}"),
        Ok(bundle) => match accept(&mut producer, &bundle) {
            Err(error) => println!("CONTROL s4 sequential double-spend rejected at apply: {error}"),
            Ok(()) => bail!("sequential double-spend accepted"),
        },
    }

    // Fork B admits the OTHER authorization at the same height, winning the
    // equal-work tie-break by hash.
    let mut producer_b = MdbxChainContext::open_or_create(&temporary.path().join("producer_b"))?;
    replay(&mut producer_b, &prefix)?;
    let bundle_b11 = bundle_at_beating(
        &mut producer_b,
        0x55,
        vec![Transaction::new(page_two.body.clone())],
        1,
        Some(hash_one),
    )?;
    assert_wins_fork_choice(&[bundle_b11.clone()], &[bundle_one.clone()])?;
    accept(&mut producer_b, &bundle_b11)?;

    let mut victim = MdbxChainContext::open_or_create(&temporary.path().join("victim"))?;
    let mut twin = MdbxChainContext::open_or_create(&temporary.path().join("twin"))?;
    replay(&mut victim, &prefix)?;
    replay(&mut twin, &prefix)?;
    accept(&mut victim, &bundle_one)?;
    assert_eq!(owner_total(&victim, &RECIPIENT_A)?, 15_000_000);
    victim.apply_reorg_mdbx_with_applier(
        10,
        &[bundle_b11.clone()],
        decode_block(bundle_b11.block_bytes())?.header.timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(
        owner_total(&victim, &RECIPIENT_A)?,
        0,
        "rejected-on-sequencing spend must not survive"
    );
    assert_eq!(owner_total(&victim, &RECIPIENT_B)?, 15_000_000);
    accept(&mut twin, &bundle_b11)?;
    assert_twin_match(&victim, &twin, "s4-after-reorg")?;
    println!(
        "CONTROL s4 reorg resolved the allowance race to exactly one authorization: recipient_a=0 recipient_b=15000000 twin_equal=true"
    );

    // After resolution the losing authorization stays dead.
    let page_one_later = payment_with_min_fee(
        &producer_b,
        &opening,
        input,
        slot(44),
        12,
        TxOutput {
            slot_index: slot(45),
            amount: 15_000_000,
            owner: Address(RECIPIENT_A),
        },
    )?;
    expect_spend_rejected(
        &mut producer_b,
        &mut victim,
        vec![Transaction::new(page_one_later.body.clone())],
        "s4 losing authorization retry",
    )?;
    assert_twin_match(&victim, &twin, "s4-after-loser-retry")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Scenario 5: same-block chained contract transitions (mint+spend of the
// successor inside one block) rolled back and re-applied in both directions.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Scenario 5: multiple contract state machines in ONE block, rolled back and
// re-applied in both directions, plus the structural no-chaining control.
// ---------------------------------------------------------------------------

fn s5_same_block_chained_transitions() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let mut producer = MdbxChainContext::open_or_create(&temporary.path().join("producer"))?;

    let terms = applications::PeriodBudget {
        start_height: 11,
        period_blocks: 5,
        budget: 2_100_000,
        recover_at: 200,
        max_fee: RULE_MAX_FEE,
        max_payout: 1_500_000,
        min_retained: RULE_MAX_FEE,
    };
    let wallet_a = applications::period_budget_wallet(
        Address(SPENDING_KEY),
        Address(RECOVERY_KEY),
        None,
        terms,
    )?;
    let wallet_b = applications::period_budget_wallet(Address(PAYEE), Address(PAYER), None, terms)?;
    // Fund BOTH roots at height 10 with two separate miner spends in one
    // block (a body carries at most two live outputs).
    let prefix = {
        let mut chain = Vec::new();
        for _ in 0..9 {
            let bundle = bundle_at(&mut producer, 0x44, Vec::new(), 1)?;
            accept(&mut producer, &bundle)?;
            chain.push(bundle);
        }
        let fund_a = funding_tx(&producer, &wallet_a.root(), slot(60), slot(62))?;
        // The second spend consumes a different miner UTXO; rebuild after the
        // first funding body exists by picking the next-largest source.
        let parent = *producer.tip_header();
        let utxos = producer.store.get_verified_utxos_by_owner(&MINER)?.utxos;
        let source = utxos
            .iter()
            .filter(|utxo| utxo.slot_index != fund_a.body.inputs[0].slot_index)
            .filter(|utxo| utxo.amount > FUND_AMOUNT + 100_000)
            .max_by_key(|utxo| utxo.amount)
            .context("no second funded miner UTXO available")?;
        let mut inputs = [TxInput::dummy(); TX_INPUTS];
        inputs[0] = TxInput {
            slot_index: source.slot_index,
            amount: source.amount,
            creation_id: source.creation_id,
        };
        let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
        outputs[0] = TxOutput {
            slot_index: slot(61),
            amount: FUND_AMOUNT,
            owner: wallet_b.root(),
        };
        outputs[1] = TxOutput {
            slot_index: slot(69),
            amount: 0,
            owner: Address(MINER),
        };
        let mut body_b = TxBody {
            epoch_anchor: epoch_anchor_for_child(&producer, 11)?,
            fee: 0,
            input_owner: Address(MINER),
            inputs,
            outputs,
            validity_bitmap: 0b1
                | output_bitmap_bit(0)
                | output_bitmap_bit(1)
                | PAGED_SPEND_START_BIT
                | PAGED_SPEND_END_BIT,
            is_coinbase: false,
        };
        body_b.fee = noid_chain::consensus::required_fee_for_tx_body(
            &body_b,
            parent.active_slot_count,
            parent.log_slots,
        );
        body_b.outputs[1].amount = source
            .amount
            .checked_sub(FUND_AMOUNT + body_b.fee)
            .context("funding source too small")?;
        let bundle = bundle_at(
            &mut producer,
            0x44,
            vec![fund_a, Transaction::new(body_b)],
            1,
        )?;
        accept(&mut producer, &bundle)?;
        chain.push(bundle);
        chain
    };
    let root_a = wallet_a.root();
    let root_b = wallet_b.root();
    let utxo_a = root_utxo(&producer, &root_a)?;
    let utxo_b = root_utxo(&producer, &root_b)?;
    assert_eq!((utxo_a.0, utxo_a.1), (slot(60), FUND_AMOUNT));
    assert_eq!((utxo_b.0, utxo_b.1), (slot(61), FUND_AMOUNT));

    // Incumbent A: two floor-relaxed blocks at heights 11 and 12.
    let bundle_a11 = bundle_at(&mut producer, 0x44, Vec::new(), 3_600)?;
    accept(&mut producer, &bundle_a11)?;
    let bundle_a12 = bundle_at(&mut producer, 0x44, Vec::new(), 3_600)?;
    accept(&mut producer, &bundle_a12)?;

    // Challenger B: empty block at 11, then a block at 12 carrying BOTH
    // wallets' draws: two independent contract state machines advancing in
    // one block.
    let mut producer_b = MdbxChainContext::open_or_create(&temporary.path().join("producer_b"))?;
    replay(&mut producer_b, &prefix)?;
    let bundle_b11 = bundle_at(&mut producer_b, 0x55, Vec::new(), 1)?;
    accept(&mut producer_b, &bundle_b11)?;
    let draw_a = payment_with_min_fee(
        &producer_b,
        &wallet_a,
        TxInput {
            slot_index: utxo_a.0,
            amount: utxo_a.1,
            creation_id: utxo_a.2,
        },
        slot(63),
        12,
        TxOutput {
            slot_index: slot(64),
            amount: 1_000_000,
            owner: Address(RECIPIENT_A),
        },
    )?;
    let draw_b = payment_with_min_fee(
        &producer_b,
        &wallet_b,
        TxInput {
            slot_index: utxo_b.0,
            amount: utxo_b.1,
            creation_id: utxo_b.2,
        },
        slot(65),
        12,
        TxOutput {
            slot_index: slot(66),
            amount: 1_000_000,
            owner: Address(RECIPIENT_B),
        },
    )?;
    // Structural control first: mint-then-spend of the SAME slot inside one
    // block is rejected at the page-stream level before any state is
    // touched - a contract successor minted by an earlier call in the block
    // cannot be spent by a later call in that block.
    let successor_a = wallet_a.successor(wallet_a.execute(&draw_a.body, 12)?);
    let chained_probe = successor_a.build_payment(
        TxInput {
            slot_index: slot(63),
            amount: FUND_AMOUNT - 1_000_000 - draw_a.body.fee,
            creation_id: 12345,
        },
        slot(67),
        draw_a.body.fee,
        epoch_anchor_for_child(&producer_b, 12)?,
        12,
        TxOutput {
            slot_index: slot(68),
            amount: 1_000_000,
            owner: Address(RECIPIENT_A),
        },
    );
    assert!(chained_probe.is_ok(), "chained page builds standalone");
    let chained = chained_probe?;
    match bundle_at(
        &mut producer_b,
        0x55,
        vec![
            Transaction::new(draw_a.body.clone()),
            Transaction::new(chained.body.clone()),
        ],
        1,
    ) {
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("InputOutputSlotOverlap"),
                "unexpected chaining rejection: {text}"
            );
            println!(
                "CONTROL s5 same-block mint+spend chaining rejected at the page-stream level (input/output slot intersection must be empty)"
            );
        }
        Ok(_) => bail!("same-block mint+spend chaining was accepted"),
    }

    let bundle_b12 = bundle_at(
        &mut producer_b,
        0x55,
        vec![
            Transaction::new(draw_a.body.clone()),
            Transaction::new(draw_b.body.clone()),
        ],
        1,
    )?;
    accept(&mut producer_b, &bundle_b12)?;
    assert_eq!(owner_total(&producer_b, &RECIPIENT_A)?, 1_000_000);
    assert_eq!(owner_total(&producer_b, &RECIPIENT_B)?, 1_000_000);
    println!("CONTROL s5 two contract machines advancing in one block accepted: both payouts live on fork B");
    assert_wins_fork_choice(
        &[bundle_b11.clone(), bundle_b12.clone()],
        &[bundle_a11.clone(), bundle_a12.clone()],
    )?;

    // Victim on A (tip 12) reorgs to the heavier B (equal height 12): the
    // two-contract block enters via reorg.
    let mut victim = MdbxChainContext::open_or_create(&temporary.path().join("victim"))?;
    let mut twin = MdbxChainContext::open_or_create(&temporary.path().join("twin"))?;
    replay(&mut victim, &prefix)?;
    replay(&mut twin, &prefix)?;
    accept(&mut victim, &bundle_a11)?;
    accept(&mut victim, &bundle_a12)?;
    victim.apply_reorg_mdbx_with_applier(
        10,
        &[bundle_b11.clone(), bundle_b12.clone()],
        decode_block(bundle_b12.block_bytes())?.header.timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(victim.tip_height(), 12);
    assert_eq!(owner_total(&victim, &RECIPIENT_A)?, 1_000_000);
    assert_eq!(owner_total(&victim, &RECIPIENT_B)?, 1_000_000);
    accept(&mut twin, &bundle_b11)?;
    accept(&mut twin, &bundle_b12)?;
    assert_twin_match(&victim, &twin, "s5-onto-two-contract-block")?;
    println!(
        "CONTROL s5 reorg onto the two-contract block restored counters exactly (alloc/active/root equal to twin)"
    );

    // Roll the two-contract block BACK with a taller, still-heavier chain.
    let mut producer_c = MdbxChainContext::open_or_create(&temporary.path().join("producer_c"))?;
    replay(&mut producer_c, &prefix)?;
    let mut fork_c = Vec::new();
    for _ in 0..3 {
        let bundle = bundle_at(&mut producer_c, 0x66, Vec::new(), 1)?;
        accept(&mut producer_c, &bundle)?;
        fork_c.push(bundle);
    }
    assert_wins_fork_choice(&fork_c, &[bundle_b11.clone(), bundle_b12.clone()])?;

    let mut victim2 = MdbxChainContext::open_or_create(&temporary.path().join("victim2"))?;
    let mut twin2 = MdbxChainContext::open_or_create(&temporary.path().join("twin2"))?;
    replay(&mut victim2, &prefix)?;
    replay(&mut twin2, &prefix)?;
    accept(&mut victim2, &bundle_b11)?;
    accept(&mut victim2, &bundle_b12)?;
    assert_eq!(owner_total(&victim2, &RECIPIENT_A)?, 1_000_000);
    assert_eq!(owner_total(&victim2, &RECIPIENT_B)?, 1_000_000);
    victim2.apply_reorg_mdbx_with_applier(
        10,
        &fork_c,
        decode_block(fork_c.last().unwrap().block_bytes())?
            .header
            .timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(owner_total(&victim2, &RECIPIENT_A)?, 0);
    assert_eq!(owner_total(&victim2, &RECIPIENT_B)?, 0);
    // Both roots must resurrect exactly as funded.
    assert_eq!(root_utxo(&victim2, &root_a)?, utxo_a);
    assert_eq!(root_utxo(&victim2, &root_b)?, utxo_b);
    for bundle in &fork_c {
        accept(&mut twin2, bundle)?;
    }
    assert_twin_match(&victim2, &twin2, "s5-off-two-contract-block")?;
    println!(
        "CONTROL s5 rolling back the two-contract block restored alloc_counter/active_slot_count/root and both roots to the twin exactly"
    );

    // Staged-reorg atomicity with contract pages inside: inject a rejection
    // after the replacement validated and staged; hot and durable state must
    // return to the pre-reorg view.
    let tip = victim2.tip_hash();
    let root = victim2.state.cached_state_root();
    let supply = victim2.state.circulating_supply_micronoid;
    let error = victim2
        .apply_reorg_mdbx_with_applier(
            10,
            &[bundle_b11.clone(), bundle_b12.clone()],
            decode_block(bundle_b12.block_bytes())?.header.timestamp,
            |context, bundle, _| {
                accept(context, bundle)?;
                Err(MdbxContextError::Corrupt(
                    "PoC injected post-apply staged-reorg rejection",
                ))
            },
        )
        .expect_err("injected staged rejection must abort");
    assert!(error.to_string().contains("PoC injected"));
    assert_eq!(victim2.tip_hash(), tip);
    assert_eq!(victim2.state.cached_state_root(), root);
    assert_eq!(victim2.state.circulating_supply_micronoid, supply);
    assert_eq!(owner_total(&victim2, &RECIPIENT_A)?, 0);
    assert!(victim2.store.get_header(11)?.is_some());
    println!(
        "CONTROL s5 staged-reorg abort with contract pages restored hot+durable state (tip/root/supply unchanged)"
    );
    Ok(())
}

fn s6_schedule_pinning_taxonomy() -> Result<()> {
    // Recurring payment: next due = inclusion height + period, so the baked
    // successor root is valid at exactly one height.
    let recurring = applications::recurring_payment(
        Address(PAYER),
        Address(PAYEE),
        applications::RecurringPayment {
            first_due_height: 11,
            period_blocks: 3,
            payment: 1_000_000,
            recover_at: 200,
            max_fee: RULE_MAX_FEE,
        },
    )?;
    let anchor = [9u8; 32];
    let claim12 = recurring.build_payment(
        TxInput {
            slot_index: 1,
            amount: 20_000_000,
            creation_id: 7,
        },
        2,
        CALL_FEE,
        anchor,
        12,
        TxOutput {
            slot_index: 3,
            amount: 1_000_000,
            owner: Address(PAYEE),
        },
    )?;
    assert!(recurring.check_call(&claim12, 12).is_ok());
    let off_height = recurring.check_call(&claim12, 13);
    assert!(matches!(
        off_height,
        Err(noid_tx::experimental_object::ObjectError::Successor)
    ));
    println!(
        "CONTROL pinning: recurring claim valid at its built height 12 only; at 13 the successor root mismatches (a miner cannot shift the schedule by delaying inclusion)"
    );

    // The successor claim is due at 15 but pinned to whatever height it is
    // built for; every inclusion height re-anchors the following due height.
    let next = recurring.successor(recurring.execute(&claim12.body, 12)?);
    let claim20 = next.build_payment(
        TxInput {
            slot_index: 2,
            amount: 20_000_000 - 1_000_000 - CALL_FEE,
            creation_id: 8,
        },
        4,
        CALL_FEE,
        anchor,
        20,
        TxOutput {
            slot_index: 5,
            amount: 1_000_000,
            owner: Address(PAYEE),
        },
    )?;
    assert!(next.check_call(&claim20, 20).is_ok());
    assert!(next.check_call(&claim20, 19).is_err());
    println!(
        "CONTROL pinning: successor claim due at 15 stays claimable later, but is height-pinned to its built inclusion height"
    );

    // Tranche vesting advances by a constant: unpinned across heights.
    let vesting = applications::tranche_vesting(
        Address(PAYEE),
        applications::TrancheVesting {
            first_unlock_height: 11,
            period_blocks: 3,
            tranche_amount: 1_000_000,
            mature_at: 200,
            max_fee: RULE_MAX_FEE,
        },
    )?;
    let tranche12 = vesting.build_payment(
        TxInput {
            slot_index: 1,
            amount: 20_000_000,
            creation_id: 7,
        },
        2,
        CALL_FEE,
        anchor,
        12,
        TxOutput {
            slot_index: 3,
            amount: 1_000_000,
            owner: Address(PAYEE),
        },
    )?;
    assert!(vesting.check_call(&tranche12, 12).is_ok());
    assert!(
        vesting.check_call(&tranche12, 15).is_ok(),
        "constant-advance schedule stays valid at later heights"
    );
    assert!(vesting.check_call(&tranche12, 10).is_err());
    println!(
        "CONTROL pinning: tranche claim built at 12 also valid at 11 and 15 (due is State1 <= Height, next unlock = previous + 3: inclusion-height independent)"
    );

    // Budget window reset is pinned; in-window draws are not.
    let terms = applications::PeriodBudget {
        start_height: 11,
        period_blocks: 5,
        budget: 2_000_000,
        recover_at: 200,
        max_fee: RULE_MAX_FEE,
        max_payout: 1_500_000,
        min_retained: RULE_MAX_FEE,
    };
    let budget = applications::period_budget_wallet(
        Address(SPENDING_KEY),
        Address(RECOVERY_KEY),
        None,
        terms,
    )?;
    let draw15 = budget.build_payment(
        TxInput {
            slot_index: 1,
            amount: 20_000_000,
            creation_id: 7,
        },
        2,
        CALL_FEE,
        anchor,
        15,
        TxOutput {
            slot_index: 3,
            amount: 1_000_000,
            owner: Address(RECIPIENT_A),
        },
    )?;
    assert!(budget.check_call(&draw15, 15).is_ok());
    assert!(
        budget.check_call(&draw15, 14).is_ok(),
        "in-window draw unpinned"
    );
    // The first call at/after the window end resets the window to
    // height + period: pinned to the exact height.
    let opened = budget.successor(budget.execute(&draw15.body, 14)?);
    let draw19 = opened.build_payment(
        TxInput {
            slot_index: 2,
            amount: 20_000_000 - 1_000_000 - CALL_FEE,
            creation_id: 8,
        },
        4,
        CALL_FEE,
        anchor,
        19,
        TxOutput {
            slot_index: 5,
            amount: 1_000_000,
            owner: Address(RECIPIENT_A),
        },
    )?;
    assert!(opened.check_call(&draw19, 19).is_ok());
    let shifted = opened.check_call(&draw19, 20);
    assert!(shifted.is_err());
    println!(
        "CONTROL pinning: in-window budget draw valid across window heights; window-opening draw (reset) valid only at its built height"
    );
    Ok(())
}

fn main() -> Result<()> {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "all".to_owned());
    match scenario.as_str() {
        "s1" => s1_refund_on_losing_fork()?,
        "s2" => s2_phase_flip_under_height_decrease()?,
        "s3" => s3_consumption_counter_divergence()?,
        "s4" => s4_allowance_race_across_forks()?,
        "s5" => s5_same_block_chained_transitions()?,
        "s6" => s6_schedule_pinning_taxonomy()?,
        "all" => {
            s1_refund_on_losing_fork()?;
            s2_phase_flip_under_height_decrease()?;
            s3_consumption_counter_divergence()?;
            s4_allowance_race_across_forks()?;
            s5_same_block_chained_transitions()?;
            s6_schedule_pinning_taxonomy()?;
        }
        _ => bail!("usage: contract-reorg-audit [all|s1|s2|s3|s4|s5|s6]"),
    }
    println!("contract-reorg-audit: all assertions passed for {scenario}");
    Ok(())
}
