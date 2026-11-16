//! Offline SR-5 fixture. Mutations require direct local MDBX write access.
//! The accepted blocks use production template, PoW, state and MDBX paths;
//! only the HistoryStep terminal cryptographic verifier is stubbed.

use std::path::Path;

use anyhow::{anyhow, ensure, Context, Result};
use libmdbx::{
    Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, WriteFlags,
};
use noid_chain::consensus::params::BLOCK_TIME;
use noid_chain::consensus::template::build_node_owned_block_template;
use noid_chain::storage::MdbxChainContext;
use noid_chain::{AcceptedBlockBundle, Block, HistoryStepTerminalMetadata};
use noid_poseidon2b::primitives::Address;
use noid_tx::{
    output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

const OWNER_INDEX: &str = "owner_idx";

fn mine_with_txs(
    ctx: &MdbxChainContext,
    owner: Address,
    transactions: Vec<Transaction>,
) -> Result<(AcceptedBlockBundle, Block)> {
    let parent = *ctx.tip_header();
    let height = parent.height + 1;
    let timestamp = parent.timestamp + BLOCK_TIME;
    let anchor = ctx.anchor_info()?;
    let target = noid_chain::consensus::next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        height,
        parent.timestamp,
        timestamp,
    );
    let counts = ctx.finalized_active_counts()?;
    let (template, _) = build_node_owned_block_template(
        &parent,
        &ctx.state,
        &counts,
        transactions,
        owner,
        timestamp,
        target,
    )
    .map_err(|error| anyhow!("template: {error:?}"))?;
    let mut nonce = 0u128;
    let block = loop {
        let candidate = template.clone().into_block(nonce);
        if noid_chain::consensus::validate_pow(&candidate.header).is_ok() {
            break candidate;
        }
        nonce = nonce.checked_add(1).context("nonce exhausted")?;
    };
    let mut terminal = HistoryStepTerminalMetadata::new(
        height,
        noid_chain::block_header::semantic_header_id(&block.header),
        0,
    )?
    .encode_prefix()
    .to_vec();
    terminal.push(0xA5);
    Ok((
        AcceptedBlockBundle::try_from_parts(block.to_bytes(), terminal)?,
        block,
    ))
}

fn mine_empty(ctx: &MdbxChainContext, owner: Address) -> Result<(AcceptedBlockBundle, Block)> {
    mine_with_txs(ctx, owner, Vec::new())
}

fn make_two_input_spend(ctx: &MdbxChainContext, owner: Address) -> Result<Transaction> {
    let utxos = ctx.store.get_verified_utxos_by_owner(&owner.0)?.utxos;
    ensure!(utxos.len() == 2);
    let total: u64 = utxos.iter().map(|utxo| utxo.amount).sum();
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    for (index, utxo) in utxos.iter().enumerate() {
        inputs[index] = TxInput {
            slot_index: utxo.slot_index,
            amount: utxo.amount,
            creation_id: utxo.creation_id,
        };
    }
    let output_slot = (0..(1u32 << ctx.tip_header().log_slots))
        .find(|&slot| ctx.state.state.slot(slot).is_empty())
        .context("no empty output slot")?;
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: output_slot,
        amount: total,
        owner: Address([0x99; 32]),
    };
    let parent = *ctx.tip_header();
    let anchor_height = noid_chain::consensus::tx_epoch_anchor_height_for_child(parent.height + 1);
    let anchor = ctx
        .store
        .get_header(anchor_height)?
        .context("missing epoch anchor")?;
    let mut body = TxBody {
        epoch_anchor: noid_chain::block_id(&anchor),
        fee: 0,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 0b11 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    };
    body.fee = noid_chain::consensus::required_fee_for_tx_body(
        &body,
        parent.active_slot_count,
        parent.log_slots,
    );
    ensure!(total > body.fee);
    body.outputs[0].amount = total - body.fee;
    Ok(Transaction::new(body))
}

fn accept(ctx: &mut MdbxChainContext, bundle: &AcceptedBlockBundle, block: &Block) -> Result<()> {
    ctx.apply_next_block(
        bundle,
        block.header.timestamp,
        |block, state| {
            noid_chain::materialize_accepted_block_state(state, block)
                .map_err(|error| format!("{error:?}"))
        },
        |_| Ok(()), // Skip cryptographic HistoryStep verification only.
    )?;
    Ok(())
}

fn raw_db(path: &Path) -> Result<Database<NoWriteMap>> {
    let rw = ReadWriteOptions {
        sync_mode: SyncMode::Durable,
        min_size: Some(4 * 1024 * 1024),
        max_size: Some(1024isize * 1024 * 1024 * 1024),
        growth_step: Some(64 * 1024 * 1024),
        ..Default::default()
    };
    Ok(Database::<NoWriteMap>::open_with_options(
        path,
        DatabaseOptions {
            max_tables: Some(24),
            mode: Mode::ReadWrite(rw),
            liforeclaim: true,
            rp_augment_limit: Some(65_536),
            ..Default::default()
        },
    )?)
}

fn owner_key(owner: &[u8; 32], slot: u32) -> [u8; 36] {
    let mut key = [0u8; 36];
    key[..32].copy_from_slice(owner);
    key[32..].copy_from_slice(&slot.to_be_bytes());
    key
}

fn replace_index_value(path: &Path, key: &[u8; 36], replacement: Option<&[u8]>) -> Result<Vec<u8>> {
    let db = raw_db(path)?;
    let txn = db.begin_rw_txn()?;
    let table = txn.open_table(Some(OWNER_INDEX))?;
    let previous: Vec<u8> = txn
        .get(&table, key)?
        .context("expected owner row missing")?;
    match replacement {
        Some(value) => txn.put(&table, key, value, WriteFlags::empty())?,
        None => {
            txn.del(&table, key, None)?;
        }
    }
    txn.commit()?;
    Ok(previous)
}

fn main() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("chain");
    let clean_path = temp.path().join("clean_chain");
    let owner = Address([0x44; 32]);

    let mut ctx = MdbxChainContext::open_or_create(&path)?;
    let (b1, block1) = mine_empty(&ctx, owner)?;
    accept(&mut ctx, &b1, &block1)?;
    let first = ctx.store.get_verified_utxos_by_owner(&owner.0)?;
    ensure!(ctx.tip_height() == 1 && first.utxos.len() == 1);
    let first_slot = first.utxos[0].slot_index;
    drop(ctx);
    println!("control: h1 persisted; owner has one verified UTXO at slot {first_slot}");

    let ctx = MdbxChainContext::open_or_create(&path)?;
    let clean = ctx.store.get_verified_utxos_by_owner(&owner.0)?;
    ensure!(ctx.tip_height() == 1 && clean.utxos == first.utxos);
    drop(ctx);
    println!("control restart: h1 and identical owner view restored");

    let key = owner_key(&owner.0, first_slot);
    let original = replace_index_value(&path, &key, Some(&[0xFF]))?;
    ensure!(original.len() == 16);
    let ctx = MdbxChainContext::open_or_create(&path)?;
    ensure!(ctx.tip_height() == 1);
    let error = ctx.store.get_verified_utxos_by_owner(&owner.0).unwrap_err();
    ensure!(format!("{error:?}").contains("invalid owner-index value length"));
    drop(ctx);
    println!("malformed row: startup accepted h1; owner read failed closed: {error:?}");

    let removed_bad = replace_index_value(&path, &key, Some(&original))?;
    ensure!(removed_bad == [0xFF]);
    let removed_good = replace_index_value(&path, &key, None)?;
    ensure!(removed_good == original);
    let mut ctx = MdbxChainContext::open_or_create(&path)?;
    ensure!(ctx.tip_height() == 1);
    let missing = ctx.store.get_verified_utxos_by_owner(&owner.0)?;
    ensure!(missing.utxos.is_empty());
    println!("deleted row: startup accepted h1; owner query silently returned zero UTXOs");

    let (b2, block2) = mine_empty(&ctx, owner)?;
    accept(&mut ctx, &b2, &block2)?;
    let after = ctx.store.get_verified_utxos_by_owner(&owner.0)?;
    ensure!(ctx.tip_height() == 2 && after.utxos.len() == 1);
    ensure!(after.utxos[0].slot_index != first_slot);
    println!("later commit: h2 accepted; owner query sees only new coinbase, omits h1 slot");

    drop(ctx);
    let ctx = MdbxChainContext::open_or_create(&path)?;
    let after_restart = ctx.store.get_verified_utxos_by_owner(&owner.0)?;
    ensure!(ctx.tip_height() == 2 && after_restart.utxos == after.utxos);
    println!("second restart: omission persisted at h2");

    let mut clean = MdbxChainContext::open_or_create(&clean_path)?;
    accept(&mut clean, &b1, &block1)?;
    accept(&mut clean, &b2, &block2)?;
    ensure!(clean.tip_hash() == ctx.tip_hash());
    let spend = make_two_input_spend(&clean, owner)?;
    let (b3, block3) = mine_with_txs(&clean, owner, vec![spend])?;
    accept(&mut clean, &b3, &block3)?;
    ensure!(clean.tip_height() == 3);
    println!("spend control: identical h3 block accepted on clean chain");

    drop(ctx);
    let mut corrupt = MdbxChainContext::open_or_create(&path)?;
    let error = accept(&mut corrupt, &b3, &block3).unwrap_err();
    ensure!(format!("{error:?}").contains("missing pre-block owner-index record"));
    ensure!(corrupt.tip_height() == 2 && corrupt.store.get_header(3)?.is_none());
    let retry = accept(&mut corrupt, &b3, &block3).unwrap_err();
    ensure!(format!("{retry:?}").contains("missing pre-block owner-index record"));
    println!("spend commit: valid h3 rejected twice by corrupted node: {error:?}");
    let (alternative, alternative_block) = mine_empty(&corrupt, owner)?;
    accept(&mut corrupt, &alternative, &alternative_block)?;
    ensure!(corrupt.tip_height() == 3);
    println!("alternative h3: coinbase-only block accepted; node is selectively divergent, not globally halted");
    Ok(())
}
