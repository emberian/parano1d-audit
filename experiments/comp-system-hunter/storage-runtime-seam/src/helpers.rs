use std::collections::{BTreeMap, HashSet};
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
use noid_chain::consensus::{add_work, block_work, tx_epoch_anchor_height_for_child};
use noid_chain::header_anchor::HeaderChainAnchor;
use noid_chain::mempool::{BlockSelectionBudget, NativeFinancialBudget};
use noid_chain::state::native_assets::market::liquidity::MarketFeeContext;
use noid_chain::state::native_assets::trade::quote_trade_charge;
use noid_chain::storage::{
    encode_chain_tip, encode_chain_work, encode_consensus_meta, encode_header,
    encode_header_chain_anchor, encode_state_meta, encode_undo_log, u64_key, ConsensusMeta,
    FinalizedCheckpoint, MdbxChainContext, MdbxContextError, MdbxStore,
};
use noid_chain::{
    materialize_accepted_block_state, AcceptedBlockBundle, Block, BlockHeader, ChainState,
    HistoryStepTerminalMetadata,
};
use noid_mempool::{AsyncMempool, ChainView, MempoolConfig};
use noid_poseidon2b::primitives::Address;
use noid_rpc::wallet_ops::{
    WalletNativeContext, WalletNativeIntent, WalletNativeProofRequest, WalletOps,
};
use noid_tx::native_assets::financial::FinancialOperation;
use noid_tx::native_assets::launch::{
    self, launch_supply_from_whole, LaunchDefinition, LaunchOpening, LaunchSide,
};
use noid_tx::native_assets::trade::{
    batch::TradeBatch,
    execution::{TradeExecution, TradeFeeQuotes, TradeOutputSlots},
    sequencing::TradeBoundary,
    TradeIntent, TradePolicy,
};
use noid_tx::native_assets::transition::{
    NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    market, AssetId, FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening, TokenBalance,
    TokenReserve, TOKEN_DECIMALS,
};
use noid_tx::{Transaction, TxInput, TxOutput};

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

/// Execute the exact P2P reorg verification + indexed suffix application path
/// used by `noid_node/src/main.rs:4560-4585`.
pub fn apply_p2p_reorg(
    ctx: &mut MdbxChainContext,
    ancestor_height: u64,
    replacement_bundles: &[AcceptedBlockBundle],
) -> Result<noid_chain::consensus::reorg::ReorgResult, MdbxContextError> {
    let last_bundle = replacement_bundles
        .last()
        .expect("non-empty replacement bundles");
    let tip_block = Block::from_bytes(last_bundle.block_bytes()).expect("valid tip block");
    let epoch_anchor_height = tx_epoch_anchor_height_for_child(tip_block.header.height);
    let epoch_anchor_header = ctx
        .get_header_from_store(epoch_anchor_height)?
        .expect("epoch anchor header present");
    let authority = ctx.verify_reorg_suffix(
        ancestor_height,
        tip_block.header,
        epoch_anchor_header,
        last_bundle.history_step_terminal_bytes().to_vec(),
        |_| Ok(()),
    )?;
    let bodies: Vec<Vec<u8>> = replacement_bundles
        .iter()
        .map(bundle_body_payload_bytes)
        .collect();
    ctx.apply_verified_reorg_suffix_with_applier(
        authority,
        &bodies,
        tip_block.header.timestamp,
        |block, state| materialize_accepted_block_state(state, block).map_err(|e| format!("{e:?}")),
    )
}

pub fn make_v21_mempool(ctx: &MdbxChainContext) -> AsyncMempool {
    let view = ChainView::from_mdbx(ctx);
    let v2_budgets = [63usize, 206usize].map(|pages| BlockSelectionBudget {
        pages,
        live_inputs: 504,
        contract_calls: 63,
        native_headers: 16,
        native_records: 64,
        owner_authorizations: pages,
        financial: None,
    });
    let v21_budget = BlockSelectionBudget {
        pages: 206,
        live_inputs: 504,
        contract_calls: 63,
        native_headers: 32,
        native_records: 128,
        owner_authorizations: 206,
        financial: Some(NativeFinancialBudget {
            actions: 64,
            definitions: 16,
            markets: 8,
            policies: 32,
            funding: 64,
            owner_operations: true,
        }),
    };
    let config = MempoolConfig::default()
        .with_v2_block_budgets(v2_budgets)
        .with_v21_native_budget(v21_budget);
    AsyncMempool::new(view, config)
}

pub fn next_epoch_anchor_id(ctx: &MdbxChainContext) -> Result<[u8; 32]> {
    let anchor_height = tx_epoch_anchor_height_for_child(ctx.tip_height() + 1);
    let hdr = ctx
        .get_header_from_store(anchor_height)?
        .context("missing epoch anchor header")?;
    Ok(block_id(&hdr))
}

pub fn collect_empty_slots(
    ctx: &MdbxChainContext,
    reserved: &HashSet<u32>,
    mut seed: u64,
    count: usize,
) -> Vec<u32> {
    let mask = ((1u64 << ctx.state.state.log_slots()) - 1) as u32;
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let candidate = (noid_chain::consensus::splitmix64(&mut seed) as u32) & mask;
        if candidate == 0 || reserved.contains(&candidate) || out.contains(&candidate) {
            continue;
        }
        if ctx.state.state.slot(candidate) == noid_chain::fri_state::SlotValue::EMPTY {
            out.push(candidate);
        }
    }
    out
}

/// Build a `CreateLaunch` proof request for `WalletOps::prove_native_submission`.
pub fn build_create_launch_proof_request(
    ctx: &MdbxChainContext,
    wallet: &dyn WalletOps,
    whole_tokens: u64,
    initial_buy_units: u64,
    metadata_hash: [u8; 32],
) -> Result<(WalletNativeProofRequest, AssetId, Vec<NativeOpening>)> {
    let parent = *ctx.tip_header();
    let owner = Address::parse(&wallet.get_address(0).context("missing address 0")?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let epoch_anchor = next_epoch_anchor_id(ctx)?;
    let utxos = wallet.list_utxos();
    let funding_utxo = utxos
        .iter()
        .find(|u| !u.reserved)
        .context("no unreserved wallet UTXO for CreateLaunch")?;
    let key_index = funding_utxo.key_index;

    let supply = launch_supply_from_whole(whole_tokens).map_err(|e| anyhow::anyhow!("{e}"))?;
    let deposit = noid_tx::asset_backing::TokenDenomination::new(TOKEN_DECIMALS)
        .and_then(|d| d.minimum_backing(supply))
        .map_err(|e| anyhow::anyhow!("minimum_backing: {e:?}"))?;
    let purchase_cost =
        launch::curve_cash_for_supply(supply, initial_buy_units).map_err(|e| anyhow::anyhow!("{e}"))?;
    let debit = deposit.checked_add(purchase_cost).context("debit overflow")?;

    let native_out_count = 3 + usize::from(initial_buy_units != 0);
    let fee = noid_chain::consensus::fee_breakdown(
        1,
        (native_out_count + 1) as u64,
        parent.active_slot_count,
        parent.log_slots,
    )
    .required_total;
    let change = funding_utxo
        .value_micronoid
        .checked_sub(debit + fee)
        .context("insufficient UTXO value for CreateLaunch")?;

    let funding_input = TxInput {
        slot_index: funding_utxo.slot_index,
        amount: funding_utxo.value_micronoid,
        creation_id: funding_utxo.creation_id,
    };
    let anchor = GenesisAnchor {
        chain: noid_chain::state::native_assets::native_chain_id(),
        owner,
        input: funding_input,
    };
    let token = FungibleDefinition::from_genesis(
        &anchor,
        supply,
        IssuanceRoute::Launchpad,
        metadata_hash,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let definition = LaunchDefinition::from_genesis(&anchor, token).map_err(|e| anyhow::anyhow!("{e}"))?;
    let def_opening = NativeOpening::Launch(LaunchOpening::Definition(definition));
    let definition_root = def_opening.root();
    let trade_state = definition
        .after_initial_purchase(initial_buy_units)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let reserve = TokenReserve::from_definition(&token, deposit).map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut auxiliary = vec![def_opening];
    let control = match trade_state {
        launch::sequential::TradeState::Curve(state) => {
            NativeOpening::Launch(LaunchOpening::State(state))
        }
        launch::sequential::TradeState::Market(pool) => {
            auxiliary.push(NativeOpening::Market(market::MarketOpening::Definition(
                definition.market_definition(),
            )));
            NativeOpening::Market(market::MarketOpening::Pool(pool))
        }
    };

    let mut native_outputs = vec![
        NativeOpening::FungibleDefinition(token),
        NativeOpening::TokenReserve(reserve),
        control,
    ];
    if initial_buy_units != 0 {
        native_outputs.push(NativeOpening::TokenBalance(TokenBalance {
            asset: token.asset,
            owner,
            units: initial_buy_units,
            unlock_height: 0,
        }));
    }

    let mut reserved = HashSet::from([funding_utxo.slot_index]);
    let slots = collect_empty_slots(ctx, &reserved, 0xCAFE_0001, native_outputs.len() + 1);
    for &s in &slots {
        reserved.insert(s);
    }

    let mut out_claims = Vec::new();
    let mut all_openings = auxiliary.clone();
    for (idx, opening) in native_outputs.iter().copied().enumerate() {
        all_openings.push(opening);
        out_claims.push(NativeOutputClaim {
            output: TxOutput {
                slot_index: slots[idx],
                amount: opening.amount_micronoid(),
                owner: opening.root(),
            },
            opening: Some(opening),
        });
    }
    out_claims.push(NativeOutputClaim {
        output: TxOutput {
            slot_index: *slots.last().unwrap(),
            amount: change,
            owner,
        },
        opening: None,
    });

    let wallet_ctx = WalletNativeContext {
        key_index,
        owner,
        parent,
        epoch_anchor,
    };
    let native_ctx = NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height: parent.height + 1,
        authority: owner,
        fee,
    };
    let inputs = vec![NativeInputClaim {
        input: funding_input,
        root: owner,
        opening: None,
    }];
    let map: BTreeMap<_, _> = all_openings.iter().map(|o| (o.root().0, *o)).collect();
    let intent = NativeIntent::from_financial_claims(
        &native_ctx,
        epoch_anchor,
        FinancialOperation::CreateLaunch {
            funding_input: 0,
            definition: definition_root,
        },
        wallet_ctx.round().map_err(anyhow::Error::msg)?,
        inputs,
        out_claims,
        |root| map.get(&root.0).copied(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok((
        WalletNativeProofRequest {
            context: wallet_ctx,
            intent: WalletNativeIntent::Operation(intent),
        },
        token.asset,
        all_openings,
    ))
}

/// Build a `TransferToken` proof request that splits `source_units` into `splits`
/// distinct carrier UTXO slots (e.g., two identical `(recipient, 1_000)` `TokenBalance`
/// openings to exercise identical `opening.root()` multi-slot reference counting).
pub fn build_split_token_transfer_request(
    ctx: &MdbxChainContext,
    wallet: &dyn WalletOps,
    asset: AssetId,
    splits: &[(Address, u64)],
) -> Result<(WalletNativeProofRequest, Vec<NativeOpening>)> {
    let parent = *ctx.tip_header();
    let owner = Address::parse(&wallet.get_address(0).context("missing address 0")?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let epoch_anchor = next_epoch_anchor_id(ctx)?;
    let utxos = wallet.list_utxos();
    let cash_utxo = utxos
        .iter()
        .find(|u| !u.reserved)
        .context("no unreserved cash UTXO")?;
    let key_index = cash_utxo.key_index;

    let page = ctx.store.native_wallet_page(owner, None, 64)?;
    let token_item = page
        .items
        .into_iter()
        .find(|item| {
            matches!(
                item.opening,
                NativeOpening::TokenBalance(tb) if tb.asset == asset
            )
        })
        .context("no token balance carrier found for owner")?;
    let NativeOpening::TokenBalance(src_tb) = token_item.opening else {
        unreachable!();
    };

    let split_sum: u64 = splits.iter().map(|(_, u)| *u).sum();
    let token_change = src_tb
        .units
        .checked_sub(split_sum)
        .context("insufficient token units for split")?;

    let mut native_outputs: Vec<NativeOpening> = splits
        .iter()
        .map(|&(recipient, units)| {
            NativeOpening::TokenBalance(TokenBalance {
                asset,
                owner: recipient,
                units,
                unlock_height: 0,
            })
        })
        .collect();
    if token_change > 0 {
        native_outputs.push(NativeOpening::TokenBalance(TokenBalance {
            asset,
            owner,
            units: token_change,
            unlock_height: 0,
        }));
    }

    let fee = noid_chain::consensus::fee_breakdown(
        2,
        (native_outputs.len() + 1) as u64,
        parent.active_slot_count,
        parent.log_slots,
    )
    .required_total;
    let cash_change = cash_utxo
        .value_micronoid
        .checked_sub(fee)
        .context("insufficient cash for TransferToken fee")?;

    let reserved = HashSet::from([token_item.carrier.slot_index, cash_utxo.slot_index]);
    let slots = collect_empty_slots(ctx, &reserved, 0xCAFE_0002, native_outputs.len() + 1);

    let mut out_claims = Vec::new();
    let mut unique_openings = BTreeMap::new();
    unique_openings.insert(token_item.root, token_item.opening);
    for (idx, opening) in native_outputs.iter().copied().enumerate() {
        unique_openings.insert(opening.root().0, opening);
        out_claims.push(NativeOutputClaim {
            output: TxOutput {
                slot_index: slots[idx],
                amount: opening.amount_micronoid(),
                owner: opening.root(),
            },
            opening: Some(opening),
        });
    }
    out_claims.push(NativeOutputClaim {
        output: TxOutput {
            slot_index: *slots.last().unwrap(),
            amount: cash_change,
            owner,
        },
        opening: None,
    });

    let inputs = vec![
        NativeInputClaim {
            input: TxInput {
                slot_index: token_item.carrier.slot_index,
                amount: token_item.carrier.amount,
                creation_id: token_item.carrier.creation_id,
            },
            root: Address(token_item.root),
            opening: Some(token_item.opening),
        },
        NativeInputClaim {
            input: TxInput {
                slot_index: cash_utxo.slot_index,
                amount: cash_utxo.value_micronoid,
                creation_id: cash_utxo.creation_id,
            },
            root: owner,
            opening: None,
        },
    ];

    let wallet_ctx = WalletNativeContext {
        key_index,
        owner,
        parent,
        epoch_anchor,
    };
    let native_ctx = NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height: parent.height + 1,
        authority: owner,
        fee,
    };
    let intent = NativeIntent::from_claims(
        &native_ctx,
        epoch_anchor,
        NativeOperation::TransferToken { asset },
        inputs,
        out_claims,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok((
        WalletNativeProofRequest {
            context: wallet_ctx,
            intent: WalletNativeIntent::Operation(intent),
        },
        unique_openings.into_values().collect(),
    ))
}

/// Build a `TradeIntent` (`LaunchSide::Buy`) proof request for `WalletOps::prove_native_submission`.
pub fn build_buy_trade_proof_request(
    ctx: &MdbxChainContext,
    wallet: &dyn WalletOps,
    asset: AssetId,
    gross_input: u64,
    valid_until: u64,
) -> Result<(WalletNativeProofRequest, TradeIntent)> {
    let parent = *ctx.tip_header();
    let owner = Address::parse(&wallet.get_address(0).context("missing address 0")?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let epoch_anchor = next_epoch_anchor_id(ctx)?;
    let utxos = wallet.list_utxos();
    let cash_utxo = utxos
        .iter()
        .find(|u| !u.reserved)
        .context("no unreserved cash UTXO for trade")?;
    let key_index = cash_utxo.key_index;

    let fee_limit = noid_chain::consensus::fee_breakdown(
        2,
        4,
        parent.active_slot_count,
        parent.log_slots,
    )
    .required_total;

    let trade = TradeIntent {
        policy: TradePolicy {
            chain: noid_chain::state::native_assets::native_chain_id(),
            epoch_anchor,
            owner,
            asset,
            recipient: owner,
            side: LaunchSide::Buy,
            gross_input,
            minimum_output: 1,
            fee_limit,
            valid_from: parent.height + 1,
            valid_until,
        },
        funding: vec![NativeInputClaim {
            input: TxInput {
                slot_index: cash_utxo.slot_index,
                amount: cash_utxo.value_micronoid,
                creation_id: cash_utxo.creation_id,
            },
            root: owner,
            opening: None,
        }],
    };

    let wallet_ctx = WalletNativeContext {
        key_index,
        owner,
        parent,
        epoch_anchor,
    };
    Ok((
        WalletNativeProofRequest {
            context: wallet_ctx,
            intent: WalletNativeIntent::Trade(trade.clone()),
        },
        trade,
    ))
}

/// Sequence a slice of `TradeIntent`s against `ctx` at `parent.height + 1` into
/// concrete block `Transaction` pages, `NativeOpening`s, and a `TradeBatch`.
pub fn sequence_trade_batch_for_next_block(
    ctx: &MdbxChainContext,
    asset: AssetId,
    policies: Vec<TradeIntent>,
) -> Result<(Vec<Transaction>, Vec<NativeOpening>, TradeBatch)> {
    let parent = *ctx.tip_header();
    let parent_id = block_id(&parent);
    let epoch_anchor = next_epoch_anchor_id(ctx)?;
    let trade_parent = ctx
        .store
        .native_trade_parent_at(asset, parent.height, parent_id)?
        .context("missing trade parent at tip")?;
    let boundary = TradeBoundary {
        chain: noid_chain::state::native_assets::native_chain_id(),
        parent_id,
        epoch_anchor,
        height: parent.height + 1,
    };
    let batch = TradeBatch::new(trade_parent, policies).map_err(|e| anyhow::anyhow!("{e}"))?;
    let plan = batch.checked(boundary).map_err(|e| anyhow::anyhow!("{e}"))?;
    let fees = MarketFeeContext::from_state(&ctx.state);

    let mut payout_counts = vec![0usize; batch.policies().len()];
    for fill in plan.fills() {
        payout_counts[fill.source_index] = quote_trade_charge(
            &batch.policies()[fill.source_index],
            boundary,
            fill.outcome,
            fees,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .payouts
        .len();
    }
    let total_slots = payout_counts.iter().sum::<usize>() + 1;
    let chosen_slots = collect_empty_slots(ctx, &HashSet::new(), 0xCAFE_0003, total_slots);
    let mut offset = 0usize;
    let payouts = payout_counts
        .into_iter()
        .map(|count| {
            let v = chosen_slots[offset..offset + count].to_vec();
            offset += count;
            v
        })
        .collect();
    let output_slots = TradeOutputSlots {
        payouts,
        market: *chosen_slots.last().unwrap(),
    };
    let fee_quotes = batch
        .policies()
        .iter()
        .map(|intent| {
            let mut quotes = [0u64; 3];
            for (idx, q) in quotes.iter_mut().enumerate() {
                *q = fees
                    .required(intent.funding.len() + 1, idx + 2)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            Ok(TradeFeeQuotes(quotes))
        })
        .collect::<Result<Vec<_>>>()?;

    let execution = TradeExecution::new(&batch, boundary, &fee_quotes, &output_slots)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let intent = NativeIntent::from_trade_execution(&execution, |root| {
        ctx.store
            .native_payload(root.0)
            .ok()
            .flatten()
            .map(|r| r.opening)
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut openings = BTreeMap::new();
    for op in intent
        .input_openings
        .iter()
        .chain(&intent.output_openings)
        .filter_map(|o| *o)
        .chain(intent.dependencies.iter().copied())
    {
        openings.insert(op.root().0, op);
    }
    let txs: Vec<Transaction> = intent
        .pages
        .into_iter()
        .map(|page| Transaction::new(page.body))
        .collect();
    Ok((txs, openings.into_values().collect(), batch))
}
