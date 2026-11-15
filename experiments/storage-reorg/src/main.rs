use std::path::Path;

use anyhow::{bail, Context, Result};
use libmdbx::{
    Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, WriteFlags,
};
use noid_chain::consensus::params::{
    BLOCK_TIME, RETAINED_BLOCK_SERVING_DEPTH, UNDO_RETENTION_DEPTH,
};
use noid_chain::storage::{
    export_snapshot_boundary_generation, open_snapshot_generation, CanonicalTipBinding,
    HistoricalStateError,
};
use noid_chain::{
    materialize_accepted_block_state, reconstruct_historical_exact_state, AcceptedBlockBundle,
    Block, MdbxChainContext, MdbxContextError,
};
use noid_poseidon2b::primitives::Address;
use noid_tx::{
    output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

const SUPPLY_CORRUPTION: u128 = 123_456_789;

fn next_bundle(context: &MdbxChainContext, miner_byte: u8) -> Result<AcceptedBlockBundle> {
    next_bundle_with_txs(context, miner_byte, Vec::new())
}

fn next_bundle_with_txs(
    context: &MdbxChainContext,
    miner_byte: u8,
    candidate_txs: Vec<Transaction>,
) -> Result<AcceptedBlockBundle> {
    let parent = *context.tip_header();
    // Ideal ASERT spacing keeps the target at the genesis floor so the PoC
    // does not grind a compounding difficulty curve (BLOCK_TIME=20s,
    // HALFLIFE=120s would otherwise multiply work ~100x over 45 blocks).
    let timestamp = parent.timestamp.saturating_add(BLOCK_TIME);
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
    let mut nonce = 0u128;
    let block = loop {
        let candidate = template.clone().into_block(nonce);
        if noid_chain::consensus::validate_pow(&candidate.header).is_ok() {
            break candidate;
        }
        nonce = nonce.checked_add(1).context("PoW nonce space exhausted")?;
    };
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

fn open_raw(path: &Path) -> Result<Database<NoWriteMap>> {
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

fn update_row(
    path: &Path,
    table: &str,
    key: &[u8],
    update: impl FnOnce(&mut Vec<u8>),
) -> Result<()> {
    let db = open_raw(path)?;
    let txn = db.begin_rw_txn()?;
    let table = txn.open_table(Some(table))?;
    let mut raw: Vec<u8> = txn.get(&table, key)?.context("row to corrupt is missing")?;
    update(&mut raw);
    txn.put(&table, key, raw, WriteFlags::empty())?;
    txn.commit()?;
    Ok(())
}

fn clear_table(path: &Path, table: &str) -> Result<()> {
    let db = open_raw(path)?;
    let txn = db.begin_rw_txn()?;
    let table = txn.open_table(Some(table))?;
    txn.clear_table(&table)?;
    txn.commit()?;
    Ok(())
}

fn compact_restart_trusts_unauthenticated_supply() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let clean_path = temporary.path().join("clean");
    let corrupt_path = temporary.path().join("corrupt");
    let mut clean = MdbxChainContext::open_or_create(&clean_path)?;
    let mut corrupt = MdbxChainContext::open_or_create(&corrupt_path)?;

    let first = next_bundle(&clean, 0x44)?;
    accept(&mut clean, &first)?;
    accept(&mut corrupt, &first)?;
    let clean_first_supply = clean.state.circulating_supply_micronoid;
    assert!(clean_first_supply > 0);
    assert_eq!(clean.tip_hash(), corrupt.tip_hash());
    drop(clean);
    drop(corrupt);

    update_row(&corrupt_path, "state_meta", &[0], |raw| {
        assert_eq!(raw.len(), 36);
        let supply = u128::from_le_bytes(raw[20..36].try_into().unwrap());
        raw[20..36].copy_from_slice(&(supply + SUPPLY_CORRUPTION).to_le_bytes());
    })?;

    let mut clean = MdbxChainContext::open_or_create(&clean_path)?;
    let mut corrupt = MdbxChainContext::open_or_create(&corrupt_path)?;
    assert_eq!(clean.state.circulating_supply_micronoid, clean_first_supply);
    assert_eq!(
        corrupt.state.circulating_supply_micronoid,
        clean_first_supply + SUPPLY_CORRUPTION
    );
    assert_eq!(clean.tip_hash(), corrupt.tip_hash());
    assert_eq!(
        clean.state.cached_state_root(),
        corrupt.state.cached_state_root()
    );
    println!(
        "VULN compact-restart: clean_supply={clean_first_supply} corrupted_supply={} tip={} state_roots_equal=true",
        corrupt.state.circulating_supply_micronoid,
        hex(&corrupt.tip_hash())
    );

    let second = next_bundle(&clean, 0x44)?;
    accept(&mut clean, &second)?;
    accept(&mut corrupt, &second)?;
    assert_eq!(clean.tip_hash(), corrupt.tip_hash());
    assert_eq!(
        clean.state.cached_state_root(),
        corrupt.state.cached_state_root()
    );
    assert_eq!(
        corrupt.state.circulating_supply_micronoid,
        clean.state.circulating_supply_micronoid + SUPPLY_CORRUPTION
    );
    assert_eq!(
        corrupt.store.get_circulating_supply()?,
        Some(clean.state.circulating_supply_micronoid + SUPPLY_CORRUPTION)
    );
    println!(
        "VULN propagated-by-valid-commit: height={} clean_supply={} persisted_corrupt_supply={} tip={}",
        corrupt.tip_height(),
        clean.state.circulating_supply_micronoid,
        corrupt.store.get_circulating_supply()?.unwrap(),
        hex(&corrupt.tip_hash())
    );

    let clean_supply = clean.state.circulating_supply_micronoid;
    drop(clean);
    drop(corrupt);
    clear_table(&corrupt_path, "segment_summaries")?;
    let repaired = MdbxChainContext::open_or_create(&corrupt_path)?;
    assert_eq!(repaired.state.circulating_supply_micronoid, clean_supply);
    println!(
        "CONTROL dense-restart-recomputes: cleared segment_summaries; recovered_supply={} at_height={}",
        repaired.state.circulating_supply_micronoid,
        repaired.tip_height()
    );
    Ok(())
}

fn noncanonical_tip_encoding_is_accepted_then_stalls_successor() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("db");
    let context = MdbxChainContext::open_or_create(&path)?;
    let genesis_hash = context.tip_hash();
    drop(context);
    update_row(&path, "tip", &[0], |raw| {
        assert_eq!(raw.len(), 40);
        raw.push(0xA5);
    })?;
    let mut reopened = MdbxChainContext::open_or_create(&path)?;
    assert_eq!(reopened.tip_hash(), genesis_hash);
    println!("VULN noncanonical-tip: 41-byte chain_tip accepted by open_or_create at height=0");

    let successor = next_bundle(&reopened, 0x44)?;
    let error = accept(&mut reopened, &successor)
        .expect_err("41-byte tip must stall the typed [u8;40] successor read");
    assert_eq!(reopened.tip_hash(), genesis_hash);
    assert_eq!(reopened.store.get_header(1)?, None);
    println!("LIMIT noncanonical-tip: next valid commit rejected with `{error}`; no partial tip mutation");
    Ok(())
}

fn reorg_and_rejected_reorg_controls() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let old_path = temporary.path().join("old");
    let producer_path = temporary.path().join("producer");
    let challenger_path = temporary.path().join("challenger");
    let mut context = MdbxChainContext::open_or_create(&old_path)?;
    let mut producer = MdbxChainContext::open_or_create(&producer_path)?;

    let orphaned = next_bundle(&context, 0x44)?;
    accept(&mut context, &orphaned)?;
    let orphaned_block = decode_block(orphaned.block_bytes())?;
    let orphaned_txid = orphaned_block.transactions[0].txid();

    let first = next_bundle(&producer, 0x55)?;
    accept(&mut producer, &first)?;
    let second = next_bundle(&producer, 0x55)?;
    accept(&mut producer, &second)?;
    let first_block = decode_block(first.block_bytes())?;
    let second_block = decode_block(second.block_bytes())?;
    let first_txid = first_block.transactions[0].txid();
    let second_txid = second_block.transactions[0].txid();

    let result = context.apply_reorg_mdbx_with_applier(
        0,
        &[first.clone(), second.clone()],
        second_block.header.timestamp,
        |context, bundle, _| accept(context, bundle),
    )?;
    assert_eq!(result.reverted_heights, vec![1]);
    assert_eq!(result.applied_heights, vec![1, 2]);
    assert_eq!(context.tip_hash(), second.block_hash());
    assert_eq!(
        context.store.get_header_by_hash(&orphaned.block_hash())?,
        None
    );
    assert_eq!(context.store.get_header(1)?, Some(first_block.header));
    assert_eq!(context.store.get_header(2)?, Some(second_block.header));
    assert_eq!(context.store.get_tx_index(&orphaned_txid.0)?, None);
    assert_eq!(context.store.get_tx_index(&first_txid.0)?, Some((1, 0)));
    assert_eq!(context.store.get_tx_index(&second_txid.0)?, Some((2, 0)));
    assert_eq!(
        context.store.get_recent_canonical_block(1)?.as_deref(),
        Some(first.block_bytes())
    );
    assert!(context
        .store
        .get_verified_utxos_by_owner(&[0x44; 32])?
        .utxos
        .is_empty());
    assert!(!context
        .store
        .get_verified_utxos_by_owner(&[0x55; 32])?
        .utxos
        .is_empty());
    assert_eq!(
        context
            .store
            .get_block_body_object(1, orphaned.block_hash())?,
        Some(orphaned.block_bytes().to_vec())
    );
    println!(
        "CONTROL reorg: old canonical hash/tx/body removed; replacement tip={} old_body_remains_content_addressed_only=true",
        hex(&context.tip_hash())
    );

    let mut challenger = MdbxChainContext::open_or_create(&challenger_path)?;
    accept(&mut challenger, &first)?;
    let rejected_second = next_bundle(&challenger, 0x66)?;
    accept(&mut challenger, &rejected_second)?;
    let rejected_third = next_bundle(&challenger, 0x66)?;
    let rejected_txid = decode_block(rejected_third.block_bytes())?.transactions[0].txid();
    let durable_tip = context.tip_hash();
    let durable_root = context.state.cached_state_root();
    let durable_supply = context.state.circulating_supply_micronoid;
    let error = context
        .apply_reorg_mdbx_with_applier(
            1,
            &[rejected_second.clone(), rejected_third.clone()],
            decode_block(rejected_third.block_bytes())?.header.timestamp,
            |context, bundle, _| {
                accept(context, bundle)?;
                if bundle.height() == 3 {
                    return Err(MdbxContextError::Corrupt(
                        "PoC injected post-apply staged-reorg rejection",
                    ));
                }
                Ok(())
            },
        )
        .expect_err("injected staged-reorg rejection must abort");
    assert!(error.to_string().contains("PoC injected"));
    assert_eq!(context.tip_hash(), durable_tip);
    assert_eq!(context.state.cached_state_root(), durable_root);
    assert_eq!(context.state.circulating_supply_micronoid, durable_supply);
    assert_eq!(context.store.get_chain_tip()?, Some((2, durable_tip)));
    assert_eq!(context.store.get_header(3)?, None);
    assert_eq!(context.store.get_tx_index(&rejected_txid.0)?, None);
    drop(challenger);
    drop(producer);
    drop(context);

    let reopened = MdbxChainContext::open_or_create(&old_path)?;
    assert_eq!(reopened.tip_hash(), durable_tip);
    assert_eq!(reopened.state.cached_state_root(), durable_root);
    assert_eq!(reopened.state.circulating_supply_micronoid, durable_supply);
    for target in [0, 1] {
        let historical = reconstruct_historical_exact_state(
            &reopened.store,
            &reopened.state,
            CanonicalTipBinding {
                height: reopened.tip_height(),
                hash: reopened.tip_hash(),
            },
            target,
            &[],
        )?;
        assert_eq!(
            historical.exact_state().cached_state_root(),
            reopened.store.get_header(target)?.unwrap().state_root
        );
    }
    println!("CONTROL rejected-reorg: post-apply failure restored hot/durable tip, root, supply; reopen and historical rollback to h0/h1 matched");
    Ok(())
}

fn retention_and_historical_controls() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("db");
    let exports = temporary.path().join("exports");
    let mut context = MdbxChainContext::open_or_create(&path)?;
    let mut bundles = Vec::new();
    for _ in 0..45 {
        let bundle = next_bundle(&context, 0x77)?;
        accept(&mut context, &bundle)?;
        bundles.push(bundle);
    }
    assert_eq!(context.tip_height(), 45);
    let serving_boundary = 45 - RETAINED_BLOCK_SERVING_DEPTH;
    let undo_boundary = 45 - UNDO_RETENTION_DEPTH;
    assert_eq!((serving_boundary, undo_boundary), (3, 9));
    assert_eq!(context.store.get_recent_block(2)?, None);
    assert_eq!(context.store.get_recent_block(3)?, None);
    assert!(context.store.get_recent_block(4)?.is_some());
    assert_eq!(
        context
            .store
            .get_history_step_terminal_at(2, bundles[1].block_hash())?,
        None
    );
    assert!(context
        .store
        .get_history_step_terminal_at(3, bundles[2].block_hash())?
        .is_some());
    assert_eq!(context.store.get_undo_log(9)?, None);
    assert!(context.store.get_undo_log(10)?.is_some());
    let third_txid = decode_block(bundles[2].block_bytes())?.transactions[0].txid();
    let fourth_txid = decode_block(bundles[3].block_bytes())?.transactions[0].txid();
    assert_eq!(context.store.get_tx_index(&third_txid.0)?, None);
    assert_eq!(context.store.get_tx_index(&fourth_txid.0)?, Some((4, 0)));
    println!("CONTROL retention: h2 fully pruned; h3 terminal-only; h4 body/tx retained; undo h9 pruned and h10 retained");
    drop(context);

    let reopened = MdbxChainContext::open_or_create(&path)?;
    let historical = reconstruct_historical_exact_state(
        &reopened.store,
        &reopened.state,
        CanonicalTipBinding {
            height: reopened.tip_height(),
            hash: reopened.tip_hash(),
        },
        9,
        &[],
    )?;
    assert_eq!(
        historical.exact_state().cached_state_root(),
        reopened.store.get_header(9)?.unwrap().state_root
    );
    let outside = reconstruct_historical_exact_state(
        &reopened.store,
        &reopened.state,
        CanonicalTipBinding {
            height: reopened.tip_height(),
            hash: reopened.tip_hash(),
        },
        8,
        &[],
    );
    assert!(matches!(
        outside,
        Err(HistoricalStateError::TargetOutsideUndoWindow { target: 8, tip: 45 })
    ));
    let generation = export_snapshot_boundary_generation(&reopened.store, &exports, 9, None)?;
    assert_eq!(generation.manifest().target_height, 9);
    assert_eq!(
        generation.manifest().target_hash,
        reopened
            .store
            .get_header(9)?
            .map(|h| noid_chain::block_id(&h))
            .unwrap()
    );
    assert_eq!(generation.manifest().bridge_tip_height, 9);
    let reread = open_snapshot_generation(generation.directory())?;
    assert_eq!(reread.manifest(), generation.manifest());
    // The manifest carries only segments that are live at the boundary; the
    // export skips empty segments, so read and authenticate exactly the
    // descriptors the manifest itself lists (length, digest, sparse decode,
    // exact segment root and native bindings are all checked inside).
    let manifest = generation.manifest();
    assert!(
        !manifest.segments.is_empty(),
        "boundary state at h9 must contain at least one live segment"
    );
    for descriptor in &manifest.segments {
        let encoded = reread.read_encoded_segment(descriptor.segment_id)?;
        assert_eq!(encoded.len(), descriptor.encoded_len as usize);
    }
    println!(
        "CONTROL historical/snapshot: exact state at oldest retained h9 matched header; h8 rejected; boundary generation reopened and authenticated {} live segment(s) ids={:?}",
        manifest.segments.len(),
        manifest
            .segments
            .iter()
            .map(|descriptor| descriptor.segment_id)
            .collect::<Vec<_>>()
    );
    Ok(())
}

fn downward_supply_corruption_stalls_spend_admission() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let clean_path = temporary.path().join("clean");
    let corrupt_path = temporary.path().join("corrupt");
    let mut clean = MdbxChainContext::open_or_create(&clean_path)?;
    let mut corrupt = MdbxChainContext::open_or_create(&corrupt_path)?;

    // Three identical coinbase-only blocks on both contexts.
    for _ in 0..3 {
        let bundle = next_bundle(&clean, 0x44)?;
        accept(&mut clean, &bundle)?;
        accept(&mut corrupt, &bundle)?;
    }
    assert_eq!(clean.tip_height(), 3);
    assert_eq!(clean.tip_hash(), corrupt.tip_hash());

    // Build one production-template block that spends two accumulated miner
    // coinbase UTXOs (total > one block reward) into a fresh output slot.
    let utxos = clean.store.get_verified_utxos_by_owner(&[0x44; 32])?.utxos;
    assert!(
        utxos.len() >= 2,
        "expected at least two miner UTXOs, got {}",
        utxos.len()
    );
    let mut picked = utxos;
    picked.sort_unstable_by_key(|utxo| std::cmp::Reverse(utxo.amount));
    let picked: Vec<_> = picked.into_iter().take(2).collect();
    let input_total: u64 = picked.iter().map(|utxo| utxo.amount).sum();

    let parent = *clean.tip_header();
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    for (index, utxo) in picked.iter().enumerate() {
        inputs[index] = TxInput {
            slot_index: utxo.slot_index,
            amount: utxo.amount,
            creation_id: utxo.creation_id,
        };
    }
    // Fresh output slot ABOVE every spent input slot: supply_after_slot_updates
    // folds deltas in slot order, so each input debit hits the corrupted base
    // before the mint credit can mask it.
    let output_slot = picked
        .iter()
        .map(|utxo| utxo.slot_index)
        .max()
        .unwrap_or_default()
        .saturating_add(5_000_000);
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: output_slot,
        amount: input_total,
        owner: Address([0x99; 32]),
    };
    let anchor_height =
        noid_chain::consensus::tx_epoch_anchor_height_for_child(parent.height.saturating_add(1));
    let anchor_header = clean
        .store
        .get_header(anchor_height)?
        .with_context(|| format!("epoch anchor header at height {anchor_height} missing"))?;
    let mut body = TxBody {
        epoch_anchor: noid_chain::block_id(&anchor_header),
        fee: 0,
        input_owner: Address([0x44; 32]),
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
    body.outputs[0].amount = input_total - body.fee;
    let spend = Transaction::new(body);
    let bundle = next_bundle_with_txs(&clean, 0x44, vec![spend])?;

    // CONTROL: the identical bundle is fully valid on the clean context.
    accept(&mut clean, &bundle)?;
    assert_eq!(clean.tip_height(), 4);
    assert_eq!(
        clean.tip_hash(),
        noid_chain::block_id(&decode_block(bundle.block_bytes())?.header)
    );
    let clean_supply = clean.state.circulating_supply_micronoid;
    println!(
        "CONTROL clean context accepted the two-input spend of tagged coinbase UTXOs {}+{} micronoid at height 4 (supply={clean_supply})",
        picked[0].amount, picked[1].amount
    );

    // Zero the persisted supply counter and compact-restart the corrupt copy.
    drop(clean);
    drop(corrupt);
    update_row(&corrupt_path, "state_meta", &[0], |raw| {
        assert_eq!(raw.len(), 36);
        raw[20..36].fill(0);
    })?;
    let mut corrupt = MdbxChainContext::open_or_create(&corrupt_path)?;
    assert_eq!(corrupt.tip_height(), 3);
    assert_eq!(corrupt.state.circulating_supply_micronoid, 0);
    let header3 = corrupt.store.get_header(3)?.context("header 3 missing")?;
    assert_eq!(
        corrupt.state.cached_state_root(),
        header3.state_root,
        "zeroed supply must still authenticate against the header root"
    );
    println!(
        "VULN zeroed-supply restart: supply=0 accepted silently at height 3, state root still header-authenticated"
    );

    // The spend block is now permanently inadmissible on this node. The
    // underflow fires inside materialization; the rollback of the uncommitted
    // candidate then hits the same underflow, so the surfaced error is the
    // rollback failure and the context recovers via a full MDBX reload.
    let error = accept(&mut corrupt, &bundle).expect_err("spend must underflow the zeroed supply");
    assert!(
        error.to_string().contains("CirculatingSupplyInvariant")
            || error.to_string().contains("supply rollback failed"),
        "unexpected error: {error}"
    );
    assert_eq!(corrupt.tip_height(), 3);
    assert_eq!(corrupt.store.get_header(4)?, None);
    let again = accept(&mut corrupt, &bundle).expect_err("rejection must be deterministic");
    assert_eq!(again.to_string(), error.to_string());
    println!(
        "VULN spend-admission stall: valid block rejected twice with `{error}`; tip pinned at height 3, no header 4 written"
    );

    // Coinbase-only blocks still pass: the node looks alive while diverged.
    let coinbase_only = next_bundle(&corrupt, 0x44)?;
    accept(&mut corrupt, &coinbase_only)?;
    assert_eq!(corrupt.tip_height(), 4);
    println!(
        "LIMIT zeroed supply still admits coinbase-only blocks (supply={} at height 4, clean chain had {clean_supply}); the stall binds any block whose spent total exceeds the corrupted base plus its own coinbase",
        corrupt.state.circulating_supply_micronoid
    );
    Ok(())
}

fn crash_recovery_replay_control() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("db");
    let executable = std::env::current_exe()?;
    let status = std::process::Command::new(&executable)
        .arg("crashchild")
        .arg(&path)
        .arg("3")
        .status()
        .context("spawn crash child")?;
    assert!(!status.success(), "crash child must not exit cleanly");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let signal = status.signal();
        assert_eq!(signal, Some(6), "expected SIGABRT, got {:?}", status);
    }

    let mut context = MdbxChainContext::open_or_create(&path)?;
    assert_eq!(context.tip_height(), 3);
    let header3 = context
        .store
        .get_header(3)?
        .context("header 3 after crash")?;
    assert_eq!(context.tip_hash(), noid_chain::block_id(&header3));
    assert_eq!(context.state.cached_state_root(), header3.state_root);
    assert_eq!(
        context.store.get_circulating_supply()?,
        Some(context.state.circulating_supply_micronoid)
    );
    assert_eq!(context.state.circulating_supply_micronoid, 135_000_000);
    let fourth = next_bundle(&context, 0x44)?;
    accept(&mut context, &fourth)?;
    assert_eq!(context.tip_height(), 4);
    println!(
        "CONTROL crash-recovery: SIGABRT after 3 durable commits; reopen resumed at height 3 with header-matched root and supply=135000000; block 4 accepted after restart"
    );
    Ok(())
}

fn crash_child(path: &str, blocks: usize) -> Result<()> {
    let mut context = MdbxChainContext::open_or_create(Path::new(path))?;
    for _ in 0..blocks {
        let bundle = next_bundle(&context, 0x44)?;
        accept(&mut context, &bundle)?;
    }
    // Hard abort without running destructors: simulates power loss between
    // two durable block commits.
    std::process::abort();
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<()> {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "all".to_owned());
    match scenario.as_str() {
        "supply" => compact_restart_trusts_unauthenticated_supply()?,
        "stall" => downward_supply_corruption_stalls_spend_admission()?,
        "metadata" => noncanonical_tip_encoding_is_accepted_then_stalls_successor()?,
        "reorg" => reorg_and_rejected_reorg_controls()?,
        "retention" => retention_and_historical_controls()?,
        "crash" => crash_recovery_replay_control()?,
        "crashchild" => {
            let path = std::env::args()
                .nth(2)
                .context("crashchild requires a database path")?;
            let blocks: usize = std::env::args()
                .nth(3)
                .context("crashchild requires a block count")?
                .parse()
                .context("crashchild block count must be a usize")?;
            crash_child(&path, blocks)?;
        }
        "all" => {
            compact_restart_trusts_unauthenticated_supply()?;
            downward_supply_corruption_stalls_spend_admission()?;
            noncanonical_tip_encoding_is_accepted_then_stalls_successor()?;
            reorg_and_rejected_reorg_controls()?;
            crash_recovery_replay_control()?;
            retention_and_historical_controls()?;
        }
        _ => bail!("usage: storage-reorg-audit [all|supply|stall|metadata|reorg|retention|crash]"),
    }
    Ok(())
}
