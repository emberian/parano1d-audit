use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use libmdbx::{
    Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, WriteFlags,
};
use noid_chain::block::payload::BlockBodyPayload;
use noid_chain::consensus::da_prune::BlockUndoLog;
use noid_chain::consensus::params::{
    block_time_at_height, GENESIS_TARGET, LOG_SLOTS_GENESIS, MAINNET_V2_1_ACTIVATION_HEIGHT,
};
use noid_chain::consensus::pow::{block_id, validate_pow};
use noid_chain::consensus::{add_work, block_work};
use noid_chain::header_anchor::HeaderChainAnchor;
use noid_chain::storage::{
    encode_chain_tip, encode_chain_work, encode_consensus_meta, encode_header,
    encode_header_chain_anchor, encode_state_meta, encode_undo_log, u64_key, ConsensusMeta,
    FinalizedCheckpoint, MdbxChainContext, MdbxContextError, MdbxStore,
};
use noid_chain::{
    materialize_accepted_block_state, AcceptedBlockBundle, Block, BlockHeader, ChainState,
    HistoryStepTerminalMetadata,
};
use noid_poseidon2b::primitives::Address;
use noid_tx::native_assets::{trade::batch::TradeBatch, NativeOpening};
use noid_tx::Transaction;

pub fn open_raw_mdbx(path: &Path) -> Result<Database<NoWriteMap>> {
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

pub fn make_terminal_bytes(header: &BlockHeader, class_id: u8) -> Result<Vec<u8>> {
    let mut terminal = HistoryStepTerminalMetadata::new(
        header.height,
        noid_chain::block_header::semantic_header_id(header),
        class_id,
    )?
    .encode_prefix()
    .to_vec();
    // Ensure length strictly differs from RECURSIVE_SUFFIX_MARKER_BYTES (96 bytes).
    terminal.extend_from_slice(&[0xA5; 64]);
    Ok(terminal)
}

/// Bootstrap an authentic `MdbxChainContext` at `MAINNET_V2_1_ACTIVATION_HEIGHT` (279,370)
/// on the default mainnet release schedule (no testnet feature flags), with valid linked
/// ancestor headers covering the 144-block tx epoch, 36-block undo window, ASERT anchor,
/// and MTP-11 window, then re-open via `MdbxChainContext::open_or_create` to verify all
/// startup integrity checks pass.
pub fn bootstrap_mainnet_v21_context(path: &Path) -> Result<MdbxChainContext> {
    let tip_height = MAINNET_V2_1_ACTIVATION_HEIGHT.expect("mainnet v2.1 height");
    // Start below the 144-block epoch anchor (279,216 = 1939 * 144 = 46,536 * 6).
    let start_height = (tip_height / 144) * 144 - 144;
    let store = MdbxStore::open(path)?;
    drop(store);

    let state = ChainState::with_log_slots(LOG_SLOTS_GENESIS as usize);
    let empty_root = state.cached_state_root();

    let db = open_raw_mdbx(path)?;
    let txn = db.begin_rw_txn()?;
    let hdr_tbl = txn.open_table(Some("headers"))?;
    let h2h_tbl = txn.open_table(Some("hash_to_height"))?;
    let work_tbl = txn.open_table(Some("chain_work"))?;
    let anchor_tbl = txn.open_table(Some("header_anchors"))?;
    let undo_tbl = txn.open_table(Some("undo_logs"))?;
    let recent_tbl = txn.open_table(Some("recent_blocks"))?;
    let term_tbl = txn.open_table(Some("history_step_terminals"))?;
    let tip_tbl = txn.open_table(Some("tip"))?;
    let cmeta_tbl = txn.open_table(Some("consensus_meta"))?;
    let smeta_tbl = txn.open_table(Some("state_meta"))?;

    let mut prev_hash = [0x11u8; 32];
    let mut cumulative_work = [0u8; 32];
    let mut base_ts = 1_800_000_000u64;
    let mut finalized = FinalizedCheckpoint {
        height: 0,
        hash: [0u8; 32],
    };
    let mut tip_hash = [0u8; 32];

    for h in start_height..=tip_height {
        base_ts += block_time_at_height(h);
        let header = BlockHeader {
            prev_block_hash: prev_hash,
            state_root: empty_root,
            tx_root: [0u8; 32],
            timestamp: base_ts,
            height: h,
            miner_address: Address([0x22; 32]),
            nonce: 0,
            difficulty_target: GENESIS_TARGET,
            log_slots: LOG_SLOTS_GENESIS,
            active_slot_count: 0,
            alloc_counter: 0,
        };
        let hash = block_id(&header);
        cumulative_work = add_work(&cumulative_work, &block_work(&header.difficulty_target));
        let key = u64_key(h);

        txn.put(&hdr_tbl, key, encode_header(&header), WriteFlags::empty())?;
        txn.put(&h2h_tbl, hash.as_slice(), key, WriteFlags::empty())?;
        txn.put(
            &work_tbl,
            key,
            encode_chain_work(&cumulative_work),
            WriteFlags::empty(),
        )?;
        let anchor = HeaderChainAnchor {
            height: h,
            block_id: hash,
            state_root: header.state_root,
            tx_root: header.tx_root,
            miner_address: header.miner_address,
            log_slots: header.log_slots,
            active_slot_count: header.active_slot_count,
            alloc_counter: header.alloc_counter,
            cumulative_chainwork: cumulative_work,
        };
        txn.put(
            &anchor_tbl,
            key,
            encode_header_chain_anchor(&anchor),
            WriteFlags::empty(),
        )?;
        txn.put(
            &undo_tbl,
            key,
            encode_undo_log(&BlockUndoLog::empty(h, LOG_SLOTS_GENESIS)),
            WriteFlags::empty(),
        )?;

        let dummy_block = Block {
            header,
            transactions: Vec::new(),
        };
        if h + 45 >= tip_height {
            let terminal = make_terminal_bytes(&header, 0)?;
            txn.put(&term_tbl, key, &terminal, WriteFlags::empty())?;
            txn.put(&recent_tbl, key, dummy_block.to_bytes(), WriteFlags::empty())?;
        }
        if h == tip_height - 18 {
            finalized = FinalizedCheckpoint { height: h, hash };
        }
        prev_hash = hash;
        tip_hash = hash;
    }

    txn.put(
        &tip_tbl,
        [0u8],
        encode_chain_tip(tip_height, &tip_hash),
        WriteFlags::empty(),
    )?;
    txn.put(
        &cmeta_tbl,
        [0u8],
        encode_consensus_meta(&ConsensusMeta {
            tip_height,
            tip_hash,
            cumulative_chainwork: cumulative_work,
            finalized,
        }),
        WriteFlags::empty(),
    )?;
    txn.put(
        &smeta_tbl,
        [0u8],
        encode_state_meta(LOG_SLOTS_GENESIS, 0, 0, 0),
        WriteFlags::empty(),
    )?;
    txn.commit()?;
    drop(db);

    let ctx = MdbxChainContext::open_or_create(path)?;
    assert_eq!(ctx.tip_height(), tip_height);
    assert!(noid_chain::consensus::params::v2_1_active(ctx.tip_height()));
    Ok(ctx)
}

pub fn mine_next_bundle_with_native(
    ctx: &MdbxChainContext,
    miner_address: Address,
    candidate_txs: Vec<Transaction>,
    native_openings: &[NativeOpening],
    trade_batches: &[TradeBatch],
    timestamp_delta: Option<u64>,
) -> Result<(AcceptedBlockBundle, Block)> {
    let parent = *ctx.tip_header();
    let child_height = parent.height + 1;
    let delta = timestamp_delta.unwrap_or_else(|| block_time_at_height(child_height));
    let timestamp = parent.timestamp.saturating_add(delta);
    let anchor = ctx.anchor_info()?;
    let target = noid_chain::consensus::next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        child_height,
        parent.timestamp,
        timestamp,
    );
    let finalized_active_counts = ctx.finalized_active_counts()?;
    let pending_outputs = HashSet::new();
    let (template, prepared) =
        noid_chain::consensus::template::build_node_owned_block_template_with_native_data(
            &parent,
            &ctx.state,
            &finalized_active_counts,
            candidate_txs,
            miner_address,
            timestamp,
            target,
            &pending_outputs,
            native_openings,
            trade_batches,
        )
        .map_err(|e| anyhow::anyhow!("template build failed: {e:?}"))?;

    let mut nonce = 0u128;
    let block = loop {
        let candidate = template.clone().into_block(nonce);
        if validate_pow(&candidate.header).is_ok() {
            break candidate;
        }
        nonce = nonce.checked_add(1).context("PoW nonce space exhausted")?;
    };

    let has_native = !prepared.native_openings().is_empty() || !prepared.trade_batches().is_empty();
    let class_id = if has_native { 2 } else { 0 };
    let terminal = make_terminal_bytes(&block.header, class_id)?;
    let bundle = AcceptedBlockBundle::try_from_parts_with_native_data(
        block.to_bytes(),
        terminal,
        prepared.native_openings().to_vec(),
        prepared.trade_batches().to_vec(),
    )?;
    Ok((bundle, block))
}

pub fn accept_bundle(
    ctx: &mut MdbxChainContext,
    bundle: &AcceptedBlockBundle,
) -> Result<[u8; 32], MdbxContextError> {
    let block = Block::from_bytes(bundle.block_bytes())
        .map_err(|_| MdbxContextError::Corrupt("malformed test block"))?;
    ctx.apply_next_block(
        bundle,
        block.header.timestamp,
        |block, state| {
            materialize_accepted_block_state(state, block).map_err(|e| format!("{e:?}"))
        },
        |_| Ok(()),
    )
}

pub fn bundle_body_payload_bytes(bundle: &AcceptedBlockBundle) -> Vec<u8> {
    let block = Block::from_bytes(bundle.block_bytes()).expect("valid block");
    BlockBodyPayload::new_with_native_data(
        block,
        bundle.native_openings().to_vec(),
        bundle.trade_batches().to_vec(),
    )
    .expect("valid body payload")
    .encode()
}
