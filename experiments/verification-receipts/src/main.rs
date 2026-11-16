#![allow(dead_code)] // The included production wallet module has other public interfaces.

use anyhow::{Context, Result};
use noid_chain::consensus::receipt::{
    generate_receipt, verify_against_header, verify_merkle_inclusion,
};
use noid_chain::consensus::{difficulty::work_gt, params::BLOCK_TIME};
use noid_chain::{
    materialize_accepted_block_state, AcceptedBlockBundle, Block, MdbxChainContext,
    MdbxContextError,
};
use noid_poseidon2b::primitives::Address;
use noid_tx::{
    output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};
use std::{
    collections::HashSet,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

// Compile and call the pinned daemon's wallet code without modifying its source.
#[path = "../../comp-system-hunter/pinned/noid_node/src/wallet/mod.rs"]
mod wallet;

fn next_bundle(
    context: &MdbxChainContext,
    miner: Address,
    spacing: u64,
    candidate_txs: Vec<Transaction>,
) -> Result<AcceptedBlockBundle> {
    let parent = *context.tip_header();
    let timestamp = parent.timestamp + spacing;
    let anchor = context.anchor_info()?;
    let target = noid_chain::consensus::next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        timestamp,
    );
    let counts = context.finalized_active_counts()?;
    let (template, _) = noid_chain::consensus::template::build_node_owned_block_template(
        &parent,
        &context.state,
        &counts,
        candidate_txs,
        miner,
        timestamp,
        target,
    )
    .map_err(|error| anyhow::anyhow!(format!("template: {error:?}")))?;
    let mut nonce = 0u128;
    let block = loop {
        let candidate = template.clone().into_block(nonce);
        if noid_chain::consensus::validate_pow(&candidate.header).is_ok() {
            break candidate;
        }
        nonce = nonce.checked_add(1).context("PoW nonce overflow")?;
    };
    let mut terminal = noid_chain::HistoryStepTerminalMetadata::new(
        block.header.height,
        noid_chain::block_header::semantic_header_id(&block.header),
        0,
    )?
    .encode_prefix()
    .to_vec();
    terminal.push(0xA5); // Valid metadata envelope; this fixture bypasses recursive proof verification.
    Ok(AcceptedBlockBundle::try_from_parts(
        block.to_bytes(),
        terminal,
    )?)
}

fn block(bundle: &AcceptedBlockBundle) -> Result<Block> {
    Block::from_bytes(bundle.block_bytes())
        .map_err(|error| anyhow::anyhow!(format!("decode: {error:?}")))
}

fn accept(
    context: &mut MdbxChainContext,
    bundle: &AcceptedBlockBundle,
) -> Result<(), MdbxContextError> {
    let accepted = Block::from_bytes(bundle.block_bytes())
        .map_err(|_| MdbxContextError::Corrupt("fixture malformed block"))?;
    context
        .apply_next_block(
            bundle,
            accepted.header.timestamp,
            |block, state| {
                materialize_accepted_block_state(state, block).map_err(|error| format!("{error:?}"))
            },
            |_| Ok(()), // Explicit HistoryStep proof boundary; all PoW/native/MDBX paths remain real.
        )
        .map(|_| ())
}

fn payment(context: &MdbxChainContext, sender: Address) -> Result<Transaction> {
    let utxo = context
        .store
        .get_verified_utxos_by_owner(&sender.0)?
        .utxos
        .into_iter()
        .next()
        .context("sender coinbase UTXO absent")?;
    let parent = context.tip_header();
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: utxo.slot_index,
        amount: utxo.amount,
        creation_id: utxo.creation_id,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: utxo.slot_index + 5_000_000,
        amount: utxo.amount,
        owner: Address([0x99; 32]),
    };
    let anchor_height = noid_chain::consensus::tx_epoch_anchor_height_for_child(parent.height + 1);
    let anchor_header = context
        .store
        .get_header(anchor_height)?
        .context("epoch anchor missing")?;
    let mut body = TxBody {
        epoch_anchor: noid_chain::block_id(&anchor_header),
        fee: 0,
        input_owner: sender,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    };
    body.fee = noid_chain::consensus::required_fee_for_tx_body(
        &body,
        parent.active_slot_count,
        parent.log_slots,
    );
    body.outputs[0].amount = utxo.amount - body.fee;
    Ok(Transaction::new(body))
}

fn copy_wallet(source: &Path, dest: &Path) -> Result<()> {
    fs::copy(source, dest)?;
    fs::copy(
        source.with_extension("receipts"),
        dest.with_extension("receipts"),
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let chain_path = dir.path().join("chain");
    let fork_path = dir.path().join("fork");
    let wallet_path = dir.path().join("wallet.key");
    let crash_path = dir.path().join("crash.key");
    let mut old = MdbxChainContext::open_or_create(&chain_path)?;
    let mut fork = MdbxChainContext::open_or_create(&fork_path)?;
    let mut original_wallet = wallet::WalletState::create_or_load(wallet_path.clone())?;
    let sender = original_wallet.active_address();

    // The old branch gets five ordinary target-floor blocks; the replacement
    // gets four fast blocks, raising its work above the longer branch.
    for _ in 0..4 {
        let bundle = next_bundle(&old, sender, BLOCK_TIME, Vec::new())?;
        accept(&mut old, &bundle)?;
    }
    let pay = payment(&old, sender)?;
    let payment_bundle = next_bundle(&old, sender, BLOCK_TIME, vec![pay])?;
    accept(&mut old, &payment_bundle)?;
    let payment_block = block(&payment_bundle)?;
    let txids = noid_chain::try_compute_logical_txids(&payment_block.transactions)?;
    let txid = txids[1].0;
    let hashes: Vec<_> = txids.iter().map(|id| id.0).collect();
    let receipt = generate_receipt(
        &payment_block.header,
        &payment_block.transactions[1..],
        1,
        &hashes,
    );
    assert_eq!(receipt.summary.logical_txid, txid);
    assert!(verify_merkle_inclusion(&receipt));
    assert!(verify_against_header(
        &receipt,
        &old.store.get_header(5)?.unwrap()
    ));
    original_wallet.receipts.insert(txid, receipt.to_bytes());
    original_wallet
        .save_receipts()
        .map_err(anyhow::Error::msg)?;
    copy_wallet(&wallet_path, &crash_path)?;
    println!("CONTROL accepted payment at height 5: merkle_valid=true canonical_header_valid=true txid={}", hex::encode(txid));

    let mut replacement = Vec::new();
    for _ in 0..4 {
        let bundle = next_bundle(&fork, Address([0x55; 32]), 1, Vec::new())?;
        accept(&mut fork, &bundle)?;
        replacement.push(bundle);
    }
    println!(
        "CONTROL fork work: old_height=5 old_work={} fork_height=4 fork_work={} fork_heavier={}",
        hex::encode(old.tip_chain_work()),
        hex::encode(fork.tip_chain_work()),
        work_gt(fork.tip_chain_work(), old.tip_chain_work())
    );
    assert!(work_gt(fork.tip_chain_work(), old.tip_chain_work()));
    let result = old.apply_reorg_mdbx_with_applier(
        0,
        &replacement,
        block(replacement.last().unwrap())?.header.timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(old.tip_height(), 4);
    assert_eq!(old.store.get_header(5)?, None);
    assert!(result.reclaimed_tx_hashes.iter().any(|id| id.0 == txid));
    println!("CONTROL committed shorter-heavier reorg: tip=4 orphan_header_5=absent reclaimed_payment=true");

    // The normal daemon post-reorg wallet step removes reclaimed receipts.
    let snapshot = old.store.get_verified_utxos_by_owner(&sender.0)?;
    let expected_active = original_wallet.active_index;
    let expected_next = original_wallet.next_index;
    let shared: wallet::SharedWallet = Arc::new(Mutex::new(Some(original_wallet)));
    let blocks = replacement.iter().map(block).collect::<Result<Vec<_>>>()?;
    let refs = blocks.iter().collect::<Vec<_>>();
    wallet::install_reorg_snapshot_and_artifacts(
        &shared,
        expected_active,
        expected_next,
        sender.0,
        snapshot,
        &HashSet::new(),
        &HashSet::new(),
        &result.reclaimed_tx_hashes,
        &refs,
    )
    .map_err(anyhow::Error::msg)?;
    assert!(!shared
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .receipts
        .contains_key(&txid));
    let mut normal_restart = wallet::WalletState::create_or_load(wallet_path)?;
    assert!(!normal_restart.receipts.contains_key(&txid));
    let normal_result = wallet::reconcile_receipts_at_startup(&mut normal_restart, &old)
        .map_err(anyhow::Error::msg)?;
    assert!(!normal_restart.receipts.contains_key(&txid));
    println!("CONTROL normal wallet reorg cleanup: receipt_absent_after_restart=true startup_result={normal_result:?}");

    // Crash cut: the MDBX commit survived but the wallet post-commit update
    // did not execute. The saved, once-canonical receipt is still present.
    let mut crash_restart = wallet::WalletState::create_or_load(crash_path.clone())?;
    assert!(crash_restart.receipts.contains_key(&txid));
    let crash_result = wallet::reconcile_receipts_at_startup(&mut crash_restart, &old)
        .map_err(anyhow::Error::msg)?;
    assert_eq!(crash_result.0, 0);
    assert!(crash_restart.receipts.contains_key(&txid));
    println!("FINDING crash-window startup: orphan_payment_receipt_retained=true startup_result={crash_result:?} claimed_height=5 canonical_tip=4");

    // The wallet RPC list consumes the saved receipt cache without querying
    // the canonical chain. A later h5 block also does not cause live cleanup.
    let crash_shared: wallet::SharedWallet = Arc::new(Mutex::new(Some(crash_restart)));
    let handle = wallet::WalletHandle {
        inner: crash_shared.clone(),
    };
    let listed = noid_rpc::WalletOps::receipts(&handle, 0, 10).map_err(anyhow::Error::msg)?;
    assert!(listed.receipts.iter().any(|record| record.txid == txid));
    println!("FINDING wallet RPC lists orphaned receipt while canonical tip is 4");

    let successor = next_bundle(&old, Address([0x55; 32]), BLOCK_TIME, Vec::new())?;
    accept(&mut old, &successor)?;
    let successor_block = block(&successor)?;
    wallet::update_for_accepted_block(&crash_shared, &successor_block)
        .map_err(anyhow::Error::msg)?;
    assert_eq!(old.tip_height(), 5);
    let canonical_h5 = old.store.get_header(5)?.unwrap();
    assert!(!verify_against_header(&receipt, &canonical_h5));
    assert!(crash_shared
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .receipts
        .contains_key(&txid));
    let listed = noid_rpc::WalletOps::receipts(&handle, 0, 10).map_err(anyhow::Error::msg)?;
    assert!(listed.receipts.iter().any(|record| record.txid == txid));
    println!("FINDING live h5 acceptance: stale_receipt_still_listed=true canonical_header_contradicts=true");

    drop(handle);
    drop(crash_shared);
    let mut later_restart = wallet::WalletState::create_or_load(crash_path)?;
    let later_result = wallet::reconcile_receipts_at_startup(&mut later_restart, &old)
        .map_err(anyhow::Error::msg)?;
    assert_eq!(later_result.0, 1);
    assert!(!later_restart.receipts.contains_key(&txid));
    println!("CONTROL later restart with replacement h5: contradiction_removes_receipt=true startup_result={later_result:?}");
    Ok(())
}
