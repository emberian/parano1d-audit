// SPDX-License-Identifier: Apache-2.0
//! Shared defensive invariant-testing helpers driving the real pinned crates
//! (`noid_tx`, `noid_chain`, `noid_mempool`, `noid_miner`, `noid_rpc`, `noid_gkr`).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use noid_chain::block::native_assets::{
    financial::FinancialHeaderWindow, materialize_accepted_block_state_with_native_data,
};
use noid_chain::consensus::{
    forks::ACTIVE_SCHEDULE, next_target, pow::block_id, tx_epoch_anchor_height_for_child,
    validate_block_consensus,
};
use noid_chain::history_step::HistoryStepTerminalMetadata;
use noid_chain::storage::{MdbxChainContext, VerifiedOwnerSnapshot};
use noid_chain::{AcceptedBlockBundle, Block, BlockHeader, ChainState};
use noid_gkr::{
    wallet_authorization::{
        prove_native_asset_authorization, prove_native_financial_authorization,
        prove_native_trade_authorization, prove_paged_spend_authorization_for_version,
    },
    OwnerAuthWitness,
};
use noid_mempool::{AsyncMempool, ChainView, MempoolConfig};
use noid_miner::{
    v21_artifacts::decode_v21_runtime_metadata_pinned,
    v2_artifacts::decode_v2_runtime_metadata_pinned, HistoryProtocolRuntime,
};
use noid_p2p::{
    object_protocol::ChainPoint, BackgroundCapacity, NetworkTopics, P2PNetwork,
};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_recursive::acceptance::history_step::v2::{banked as v2, capacity_banked as v21};
use noid_rpc::{
    server::start_rpc_server,
    types::{
        WalletAddressInfo, WalletBalance, WalletConsolidationPlan, WalletHistoryEntry,
        WalletScanResult, WalletSendPlan, WalletStatus, WalletUtxoInfo,
    },
    wallet_ops::{
        WalletActivationPreview, WalletAddressDiscoveryPreview, WalletMinedBlockSlice,
        WalletOps, WalletReceiptSlice, WalletSendPlanError,
    },
    ExternalMiningAttemptInvalidator, NodeSyncStage,
};
use noid_tx::{
    native_assets::{
        nft_market::NftRoundContext,
        trade::{batch::TradeBatch, wire::TradeOwnerIntent, TradeIntent},
        wire::NativeIntent,
        NativeOpening,
    },
    output_bitmap_bit, OwnerAuthorizationVersion, PagedSpendIntent, Transaction, TxBody, TxInput,
    TxOutput, TxPage, PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};
use tokio::sync::RwLock;

pub const V2_METADATA_PATH: &str =
    "/Users/ember/dev/parano1d-break/experiments/devnet-chaos/downloads/v2-pack/history-step-pack-v2/v2-runtime-metadata.bin";
pub const V21_METADATA_PATH: &str =
    "/Users/ember/dev/parano1d-break/experiments/devnet-chaos/downloads/v21-pack/history-step-pack-v21/v21-runtime-metadata.bin";

pub fn keypair(seed: u8) -> ([u8; 32], Address) {
    let mut raw = [0u8; 32];
    raw[0] = seed;
    raw[1] = seed.wrapping_mul(37).wrapping_add(11);
    raw[31] = 0x5A ^ seed;
    let secret = SpendSecret::from_bytes(raw);
    let addr = derive_address(&secret);
    (raw, addr)
}

pub fn witness(raw: [u8; 32]) -> OwnerAuthWitness {
    OwnerAuthWitness::new(SpendSecret::from_bytes(raw))
}

/// Verify the untouched mainnet v21 runtime metadata parameters directly from disk,
/// then construct an authenticated `HistoryProtocolRuntime` with the exact same
/// class geometry adapted to `ACTIVE_SCHEDULE` (`isolated-v2-1-fork-testnet`: 5 / 10 / 24).
pub fn build_real_history_protocol_runtime(origin_dir: &Path) -> Arc<HistoryProtocolRuntime> {
    let v2_raw = std::fs::read(V2_METADATA_PATH).expect("read v2-runtime-metadata.bin");
    let v21_raw = std::fs::read(V21_METADATA_PATH).expect("read v21-runtime-metadata.bin");

    // 1. Patch v2 metadata schedule (95_125 -> 5, 210_537 -> 10) and re-pin bank.
    let v2_prefix = 8 + 2 * 32 + 4;
    let mut v2_parts_bytes = v2_raw[v2_prefix..].to_vec();
    replace_u64_exact(&mut v2_parts_bytes, 95_125, 5, 2);
    replace_u64_exact(&mut v2_parts_bytes, 210_537, 10, 2);
    let v2_parts = v2::RuntimeParts::decode_compact(&v2_parts_bytes).expect("decode v2 parts");
    let v2_digests: [[u8; 32]; 2] = [
        v2_raw[8..40].try_into().unwrap(),
        v2_raw[40..72].try_into().unwrap(),
    ];
    let v2_bank = v2::Bank::pin(v2_digests, &v2_parts);
    let v2_encoded =
        noid_miner::v2_artifacts::encode_v2_runtime_metadata(&v2_bank, &v2_parts).unwrap();
    let v2_meta = decode_v2_runtime_metadata_pinned(&v2_encoded, v2_bank.digest()).unwrap();
    let dummy_mat: Arc<[u8]> = Arc::from([1u8].as_slice());
    let v2_rt = Arc::new(
        v2_meta
            .into_runtime([Arc::clone(&dummy_mat), Arc::clone(&dummy_mat)])
            .unwrap(),
    );

    // 2. Patch v21 metadata schedule (279_370 -> 24, 95_125 -> 5, 210_537 -> 10) and re-pin bank.
    let v21_prefix = 8 + 3 * 32 + 4;
    let mut v21_parts_bytes = v21_raw[v21_prefix..].to_vec();
    replace_u64_exact(&mut v21_parts_bytes, 279_370, 24, 3);
    replace_u64_exact(&mut v21_parts_bytes, 95_125, 5, 3);
    replace_u64_exact(&mut v21_parts_bytes, 210_537, 10, 3);
    let v21_parts =
        v21::RuntimeParts::decode_compact(&v21_parts_bytes).expect("decode v21 parts");
    let v21_digests: [[u8; 32]; 3] = [
        v21_raw[8..40].try_into().unwrap(),
        v21_raw[40..72].try_into().unwrap(),
        v21_raw[72..104].try_into().unwrap(),
    ];
    let v21_bank = v21::Bank::pin(v21_digests, &v21_parts);
    let v21_encoded =
        noid_miner::v21_artifacts::encode_v21_runtime_metadata(&v21_bank, &v21_parts).unwrap();
    let v21_meta = decode_v21_runtime_metadata_pinned(&v21_encoded, v21_bank.digest()).unwrap();
    let v21_rt = Arc::new(
        v21_meta
            .into_runtime([
                Arc::clone(&dummy_mat),
                Arc::clone(&dummy_mat),
                Arc::clone(&dummy_mat),
            ])
            .unwrap(),
    );

    let runtime = HistoryProtocolRuntime::new(None, Some(v2_rt), origin_dir.to_path_buf())
        .expect("HistoryProtocolRuntime::new")
        .with_finance_runtime(Some(v21_rt))
        .expect("with_finance_runtime");
    Arc::new(runtime)
}

fn replace_u64_exact(bytes: &mut [u8], from: u64, to: u64, expected_hits: usize) {
    let needle = from.to_le_bytes();
    let replacement = to.to_le_bytes();
    // Walk exact header positions in runtime_parts_codec rather than blind byte scan.
    let mut hits = 0usize;
    let mut i = 0usize;
    while i + 8 <= bytes.len() {
        if bytes[i..i + 8] == needle {
            bytes[i..i + 8].copy_from_slice(&replacement);
            hits += 1;
            i += 8;
        } else {
            i += 1;
        }
    }
    assert!(
        hits >= expected_hits,
        "expected at least {expected_hits} occurrences of {from}, found {hits}"
    );
}

pub fn mempool_config_from_runtime(runtime: &HistoryProtocolRuntime) -> MempoolConfig {
    let v2_budgets = runtime
        .v2_admission_budgets()
        .expect("v2 admission budgets");
    let v21_budget = runtime
        .v21_native_admission_budget()
        .expect("v21 native admission budget");
    MempoolConfig::default()
        .with_v2_block_budgets(v2_budgets)
        .with_v21_native_budget(v21_budget)
}

pub fn chain_view_from_context(ctx: &MdbxChainContext) -> ChainView {
    let tip = ctx.tip_height();
    let mut recent = HashMap::new();
    for h in 0..=tip {
        if let Ok(Some(hdr)) = ctx.get_header_from_store(h) {
            recent.insert(h, hdr);
        }
    }
    let mut hydrated = ctx.state.clone();
    for seg in 0..hydrated.state.num_segments() as u16 {
        if hydrated.state.is_evicted(seg) {
            if let Ok(Some((_, cols))) = ctx.store.get_segment(seg) {
                let _ = hydrated.restore_evicted_segment(seg, cols);
            }
        }
    }
    ChainView::new(
        tip,
        recent,
        hydrated.active_slot_count,
        hydrated.state,
    )
}

pub fn hydrate_all_segments(ctx: &MdbxChainContext) -> ChainState {
    let mut st = ctx.state.clone();
    for seg in 0..st.state.num_segments() as u16 {
        if st.state.is_evicted(seg) {
            if let Ok(Some((_, cols))) = ctx.store.get_segment(seg) {
                st.restore_evicted_segment(seg, cols)
                    .expect("restore evicted segment");
            }
        }
    }
    st
}

pub fn next_block_timestamp(ctx: &MdbxChainContext) -> u64 {
    let prev_ts = ctx.prev_timestamps().expect("prev_timestamps");
    let mtp = noid_chain::consensus::median_time_past(&prev_ts);
    let step = ACTIVE_SCHEDULE.block_time(ctx.tip_height() + 1);
    (ctx.tip_header().timestamp + step).max(mtp + 1)
}

pub fn terminal_bytes_for(header: &BlockHeader, class_id: u8) -> Vec<u8> {
    let meta = HistoryStepTerminalMetadata::new(
        header.height,
        noid_chain::block_header::semantic_header_id(header),
        class_id,
    )
    .expect("valid terminal metadata");
    let mut out = meta.encode_prefix().to_vec();
    out.push(0x01);
    out
}

pub fn mine_and_commit_block(
    ctx: &mut MdbxChainContext,
    miner_address: Address,
    txs: Vec<Transaction>,
    native_openings: &[NativeOpening],
    trade_batches: &[TradeBatch],
) -> Block {
    let parent = *ctx.tip_header();
    let finalized = ctx.finalized_active_counts().unwrap();
    let prev_ts = ctx.prev_timestamps().unwrap();
    let anchor = ctx.anchor_info().unwrap();
    let ts = next_block_timestamp(ctx);
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        ts,
    );
    let hydrated = hydrate_all_segments(ctx);
    let (template, prepared) =
        noid_chain::consensus::template::build_node_owned_block_template_with_native_data(
            &parent,
            &hydrated,
            &finalized,
            txs,
            miner_address,
            ts,
            target,
            &HashSet::new(),
            native_openings,
            trade_batches,
        )
        .expect("build_node_owned_block_template_with_native_data");

    let mut block = template.into_block(0);
    for nonce in 0..u64::MAX {
        block.header.nonce = nonce;
        if noid_chain::consensus::pow::meets_target(&block_id(&block.header), &target) {
            break;
        }
    }
    let class_id = if !prepared.native_openings().is_empty() || !prepared.trade_batches().is_empty()
    {
        2
    } else {
        0
    };
    let term = terminal_bytes_for(&block.header, class_id);
    let bundle = AcceptedBlockBundle::try_from_parts_with_native_data(
        block.to_bytes(),
        term,
        prepared.native_openings().to_vec(),
        prepared.trade_batches().to_vec(),
    )
    .expect("AcceptedBlockBundle");

    ctx.apply_next_block(
        &bundle,
        ts + 1,
        |blk, st| {
            let mut tmp = st.clone();
            let root = validate_block_consensus(
                blk,
                &parent,
                &prev_ts,
                &finalized,
                ts + 1,
                &anchor,
                &mut tmp,
            )?;
            *st = tmp;
            Ok::<_, noid_chain::consensus::ConsensusError>(root)
        },
        |_claim| Ok(()),
    )
    .expect("apply_next_block");
    block
}

/// Commit a manually constructed block (e.g. containing consensus-valid dark ops
/// 49 + 50 that `template/native.rs` filters out) through `MdbxChainContext::apply_next_block`.
pub fn seal_and_commit_custom_native_block(
    ctx: &mut MdbxChainContext,
    miner_address: Address,
    user_txs: Vec<Transaction>,
    native_openings: Vec<NativeOpening>,
    trade_batches: Vec<TradeBatch>,
) -> Result<Block, String> {
    let parent = *ctx.tip_header();
    let child_height = parent.height + 1;
    let finalized = ctx.finalized_active_counts().map_err(|e| e.to_string())?;
    let prev_ts = ctx.prev_timestamps().map_err(|e| e.to_string())?;
    let anchor = ctx.anchor_info().map_err(|e| e.to_string())?;
    let ts = next_block_timestamp(ctx);
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        child_height,
        parent.timestamp,
        ts,
    );
    let new_log_slots = noid_chain::consensus::expected_child_log_slots(
        parent.height,
        parent.log_slots,
        &finalized,
    );
    let alloc = noid_chain::consensus::development_allocation::development_allocation_at_height(
        child_height,
        new_log_slots,
    )
    .map_err(|e| e.to_string())?;
    let hydrated = hydrate_all_segments(ctx);

    // Find unused coinbase slot above 40_000.
    let mut used: HashSet<u32> = user_txs
        .iter()
        .flat_map(|tx| {
            tx.body
                .live_inputs()
                .map(|(_, i)| i.slot_index)
                .chain(tx.body.live_outputs().map(|(_, o)| o.slot_index))
        })
        .collect();
    let mut next_free = 50_000u32 + (child_height as u32) * 4;
    let mut pick_slot = || {
        while used.contains(&next_free) || hydrated.state.slot(next_free) != noid_chain::SlotValue::EMPTY {
            next_free += 1;
        }
        used.insert(next_free);
        next_free
    };
    let cb_slot = pick_slot();
    let prev_hash = block_id(&parent);
    let cb = Transaction::new(TxBody {
        epoch_anchor: prev_hash,
        fee: 0,
        input_owner: Address([0; 32]),
        inputs: [TxInput::dummy(); TX_INPUTS],
        outputs: [
            TxOutput {
                slot_index: cb_slot,
                amount: alloc.miner_subsidy,
                owner: miner_address,
            },
            TxOutput::dummy(),
        ],
        validity_bitmap: output_bitmap_bit(0),
        is_coinbase: true,
    });
    let mut all_txs = vec![cb];
    if let Some(each) = alloc.payout_each {
        let s0 = pick_slot();
        let s1 = pick_slot();
        all_txs.push(Transaction::new(TxBody {
            epoch_anchor: prev_hash,
            fee: 0,
            input_owner: Address([0; 32]),
            inputs: [TxInput::dummy(); TX_INPUTS],
            outputs: [
                TxOutput {
                    slot_index: s0,
                    amount: each,
                    owner: noid_chain::consensus::development_allocation::O1_NETWORK_FUND_ADDRESS,
                },
                TxOutput {
                    slot_index: s1,
                    amount: each,
                    owner: noid_chain::consensus::development_allocation::PARANO1D_LAB_ADDRESS,
                },
            ],
            validity_bitmap: output_bitmap_bit(0) | output_bitmap_bit(1),
            is_coinbase: true,
        }));
    }
    all_txs.extend(user_txs);

    let stream = noid_chain::block::native_assets::validate_scheduled_block_page_stream(
        &all_txs,
        child_height,
    )
    .map_err(|e| e.to_string())?;
    let tx_hashes: Vec<[u8; 32]> = stream
        .logical_txids(&all_txs)
        .into_iter()
        .map(|h| h.0)
        .collect();
    let tx_root = noid_chain::tx_tree::root_from_hashes(&tx_hashes);

    // Materialize state root via the real native block interpreter.
    let window = FinancialHeaderWindow::new(parent, &[]).map_err(|e| e.to_string())?;
    let opening_map: std::collections::BTreeMap<[u8; 32], NativeOpening> =
        native_openings.iter().map(|o| (o.root().0, *o)).collect();
    let mut trial_block = Block {
        header: BlockHeader {
            prev_block_hash: prev_hash,
            state_root: [0; 32],
            tx_root,
            timestamp: ts,
            height: child_height,
            miner_address,
            nonce: 0,
            difficulty_target: target,
            log_slots: new_log_slots,
            active_slot_count: 0,
            alloc_counter: 0,
        },
        transactions: all_txs,
    };
    let mut scratch = hydrated.clone();
    let transition = materialize_accepted_block_state_with_native_data(
        &mut scratch,
        &trial_block,
        &window,
        &trade_batches,
        |r| opening_map.get(&r.0).copied(),
    )
    .map_err(|e| format!("materialize_accepted_block_state_with_native_data: {e}"))?;
    trial_block.header.state_root = transition.new_state_root;
    trial_block.header.active_slot_count = scratch.active_slot_count;
    trial_block.header.alloc_counter = scratch.alloc_counter;

    for nonce in 0..u64::MAX {
        trial_block.header.nonce = nonce;
        if noid_chain::consensus::pow::meets_target(&block_id(&trial_block.header), &target) {
            break;
        }
    }

    // Verify full sequential `validate_block_consensus` ALSO accepts this block!
    let mut seq_check = hydrated.clone();
    let seq_root = validate_block_consensus(
        &trial_block,
        &parent,
        &prev_ts,
        &finalized,
        ts + 1,
        &anchor,
        &mut seq_check,
    )
    .map_err(|e| format!("validate_block_consensus: {e}"))?;
    assert_eq!(seq_root, trial_block.header.state_root);

    let term = terminal_bytes_for(&trial_block.header, 2);
    let bundle = AcceptedBlockBundle::try_from_parts_with_native_data(
        trial_block.to_bytes(),
        term,
        native_openings,
        trade_batches,
    )
    .map_err(|e| format!("AcceptedBlockBundle: {e}"))?;

    ctx.apply_next_block(
        &bundle,
        ts + 1,
        |blk, st| {
            let mut tmp = st.clone();
            let root = validate_block_consensus(
                blk,
                &parent,
                &prev_ts,
                &finalized,
                ts + 1,
                &anchor,
                &mut tmp,
            )?;
            *st = tmp;
            Ok::<_, noid_chain::consensus::ConsensusError>(root)
        },
        |_claim| Ok(()),
    )
    .map_err(|e| format!("apply_next_block: {e}"))?;

    Ok(trial_block)
}

pub fn current_user_epoch_anchor(ctx: &MdbxChainContext) -> [u8; 32] {
    let h = tx_epoch_anchor_height_for_child(ctx.tip_height() + 1);
    let hdr = ctx
        .get_header_from_store(h)
        .unwrap()
        .expect("epoch anchor header");
    block_id(&hdr)
}

pub fn build_ordinary_self_transfer(
    owner_secret: Option<[u8; 32]>,
    owner: Address,
    input: TxInput,
    output_slot: u32,
    fee: u64,
    epoch_anchor: [u8; 32],
    candidate_height: u64,
) -> (PagedSpendIntent, Vec<u8>) {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = input;
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: output_slot,
        amount: input.amount.checked_sub(fee).expect("input covers fee"),
        owner,
    };
    let page = TxPage::new(TxBody {
        epoch_anchor,
        fee,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
    .expect("valid ordinary TxPage");
    let pages = vec![page];
    let auth_bytes = if let Some(raw) = owner_secret {
        prove_paged_spend_authorization_for_version(
            &pages,
            witness(raw),
            noid_chain::consensus::params::owner_authorization_version(candidate_height),
        )
        .expect("prove_paged_spend_authorization_for_version")
        .to_bytes()
        .unwrap()
    } else {
        vec![0xAA; 32]
    };
    let intent = PagedSpendIntent {
        pages,
        authorization_bytes: auth_bytes,
    };
    let encoded = intent.to_bytes().expect("encode PagedSpendIntent");
    (intent, encoded)
}

pub fn sign_native_base(
    mut intent: NativeIntent,
    raw_secret: [u8; 32],
    height: u64,
) -> (NativeIntent, Vec<u8>) {
    let bundle = prove_native_asset_authorization(
        &intent,
        noid_chain::state::native_assets::native_chain_id(),
        height,
        witness(raw_secret),
        OwnerAuthorizationVersion::V4,
    )
    .expect("prove_native_asset_authorization");
    intent.authorization_bytes = bundle.to_bytes().unwrap();
    let bytes = intent.to_bytes().unwrap();
    (intent, bytes)
}

pub fn sign_native_financial(
    mut intent: NativeIntent,
    raw_secret: [u8; 32],
    parent: &BlockHeader,
) -> (NativeIntent, Vec<u8>) {
    let round = NftRoundContext::new(parent.height + 1, block_id(parent), parent.alloc_counter)
        .expect("NftRoundContext");
    let bundle = prove_native_financial_authorization(
        &intent,
        noid_chain::state::native_assets::native_chain_id(),
        round,
        witness(raw_secret),
        OwnerAuthorizationVersion::V4,
    )
    .expect("prove_native_financial_authorization");
    intent.authorization_bytes = bundle.to_bytes().unwrap();
    let bytes = intent.to_bytes().unwrap();
    (intent, bytes)
}

pub fn sign_trade_intent(
    intent: TradeIntent,
    raw_secret: Option<[u8; 32]>,
    epoch_anchor: [u8; 32],
    height: u64,
) -> (TradeOwnerIntent, Vec<u8>) {
    let auth_bytes = if let Some(raw) = raw_secret {
        prove_native_trade_authorization(
            &intent,
            noid_chain::state::native_assets::native_chain_id(),
            epoch_anchor,
            height,
            witness(raw),
            OwnerAuthorizationVersion::V4,
        )
        .expect("prove_native_trade_authorization")
        .to_bytes()
        .unwrap()
    } else {
        vec![0xBB; 32]
    };
    let env = TradeOwnerIntent {
        intent,
        authorization_bytes: auth_bytes,
    };
    let bytes = env.to_bytes().expect("TradeOwnerIntent::to_bytes");
    (env, bytes)
}

/// Minimal in-memory `WalletOps` tracking one active address for loopback RPC tests.
pub struct SimpleWallet {
    pub owner: Address,
    pub utxos: Mutex<Vec<WalletUtxoInfo>>,
}

impl SimpleWallet {
    pub fn new(owner: Address) -> Self {
        Self {
            owner,
            utxos: Mutex::new(Vec::new()),
        }
    }
}

impl WalletOps for SimpleWallet {
    fn status(&self) -> WalletStatus {
        let u = self.utxos.lock().unwrap();
        let total: u64 = u.iter().map(|x| x.value_micronoid).sum();
        WalletStatus {
            exists: true,
            active_address: Some(self.owner.to_bech32()),
            active_key_index: Some(0),
            address_count: 1,
            confirmed_micronoid: total,
            confirmed_noid: total as f64 / 1_000_000.0,
            spendable_micronoid: total,
            spendable_noid: total as f64 / 1_000_000.0,
            utxo_count: u.len(),
            reserved_utxo_count: 0,
            pending_outbound_micronoid: 0,
        }
    }
    fn get_address(&self, index: u32) -> Option<String> {
        (index == 0).then(|| self.owner.to_bech32())
    }
    fn get_balance(&self) -> WalletBalance {
        let u = self.utxos.lock().unwrap();
        let total: u64 = u.iter().map(|x| x.value_micronoid).sum();
        WalletBalance {
            confirmed_micronoid: total,
            confirmed_noid: total as f64 / 1_000_000.0,
            spendable_micronoid: total,
            spendable_noid: total as f64 / 1_000_000.0,
            utxo_count: u.len(),
            reserved_utxo_count: 0,
            pending_outbound_micronoid: 0,
        }
    }
    fn list_utxos(&self) -> Vec<WalletUtxoInfo> {
        self.utxos.lock().unwrap().clone()
    }
    fn history(&self) -> Vec<WalletHistoryEntry> {
        Vec::new()
    }
    fn receipts(&self, _offset: usize, _limit: usize) -> Result<WalletReceiptSlice, String> {
        Ok(WalletReceiptSlice {
            total: 0,
            receipts: Vec::new(),
        })
    }
    fn mined_blocks(&self, _offset: usize, _limit: usize) -> WalletMinedBlockSlice {
        WalletMinedBlockSlice {
            total: 0,
            blocks: Vec::new(),
        }
    }
    fn preview_active_reload(&self) -> Result<WalletActivationPreview, String> {
        Ok(WalletActivationPreview {
            expected_active_index: 0,
            expected_next_index: 1,
            target_index: 0,
            owner: self.owner.0,
        })
    }
    fn preview_address_switch(&self, _index: u32) -> Result<WalletActivationPreview, String> {
        self.preview_active_reload()
    }
    fn create_next_address(&self) -> Result<WalletAddressInfo, String> {
        Err("unsupported".into())
    }
    fn preview_address_discovery(
        &self,
        _max_additional: u32,
    ) -> Result<WalletAddressDiscoveryPreview, String> {
        Err("unsupported".into())
    }
    fn commit_address_discovery(
        &self,
        _a: u32,
        _b: u32,
        _c: u32,
    ) -> Result<Vec<WalletAddressInfo>, String> {
        Err("unsupported".into())
    }
    fn commit_activation_snapshot(
        &self,
        _preview: WalletActivationPreview,
        snapshot: VerifiedOwnerSnapshot,
        reserved_in: &HashSet<u32>,
        reserved_out: &HashSet<u32>,
    ) -> Result<(WalletAddressInfo, WalletScanResult), String> {
        let addr = self.owner.to_bech32();
        let list: Vec<WalletUtxoInfo> = snapshot
            .utxos
            .iter()
            .map(|u| WalletUtxoInfo {
                slot_index: u.slot_index,
                value_micronoid: u.amount,
                creation_id: u.creation_id,
                value_noid: u.amount as f64 / 1_000_000.0,
                address: addr.clone(),
                key_index: 0,
                confirmed_height: snapshot.height,
                reserved: reserved_in.contains(&u.slot_index)
                    || reserved_out.contains(&u.slot_index),
            })
            .collect();
        let total: u64 = list.iter().map(|x| x.value_micronoid).sum();
        *self.utxos.lock().unwrap() = list.clone();
        Ok((
            WalletAddressInfo {
                key_index: 0,
                address: addr.clone(),
                active: true,
            },
            WalletScanResult {
                address: addr,
                utxos_found: list.len(),
                balance_micronoid: total,
                balance_noid: total as f64 / 1_000_000.0,
            },
        ))
    }
    fn on_accepted_block(&self, _block: &Block) -> Result<(), String> {
        Ok(())
    }
    fn plan_send(
        &self,
        _a: u64,
        _f: Option<u64>,
        _s: u64,
        _l: u32,
        _r: u64,
    ) -> Result<WalletSendPlan, WalletSendPlanError> {
        Err(WalletSendPlanError::Other("unsupported".into()))
    }
    fn build_send(
        &self,
        _to: [u8; 32],
        _amt: u64,
        _fee: u64,
        _ea: [u8; 32],
        _sh: Vec<u32>,
        _ls: u32,
        _ch: u64,
    ) -> Result<(Vec<u8>, Vec<u32>), String> {
        Err("unsupported".into())
    }
    fn plan_consolidation(
        &self,
        _m: usize,
        _a: u64,
        _l: u32,
        _r: u64,
    ) -> Result<WalletConsolidationPlan, WalletSendPlanError> {
        Err(WalletSendPlanError::Other("unsupported".into()))
    }
    fn build_consolidation(
        &self,
        _si: Vec<u32>,
        _ov: u64,
        _f: u64,
        _ea: [u8; 32],
        _sh: Vec<u32>,
        _ls: u32,
        _ch: u64,
    ) -> Result<(Vec<u8>, Vec<u32>), String> {
        Err("unsupported".into())
    }
    fn export_receipt(&self, _txhash_hex: &str) -> Result<String, String> {
        Err("unsupported".into())
    }
    fn reserve_pending_submission(
        &self,
        _txid: [u8; 32],
        _in_slots: &[u32],
        _out_slots: &[u32],
        _amt: u64,
        _peer: [u8; 32],
    ) -> Result<(), String> {
        Ok(())
    }
    fn rollback_pending_submission(
        &self,
        _txid: [u8; 32],
        _in_slots: &[u32],
        _out_slots: &[u32],
    ) {
    }
    fn active_address(&self) -> Option<(u32, String)> {
        Some((0, self.owner.to_bech32()))
    }
    fn list_addresses(&self) -> Vec<WalletAddressInfo> {
        vec![WalletAddressInfo {
            key_index: 0,
            address: self.owner.to_bech32(),
            active: true,
        }]
    }
    fn pending_outbound(&self) -> u64 {
        0
    }
}

pub async fn spawn_loopback_rpc(
    chain: Arc<RwLock<MdbxChainContext>>,
    mempool: AsyncMempool,
    wallet: Arc<SimpleWallet>,
    runtime: Arc<HistoryProtocolRuntime>,
    p2p_dir: &Path,
) -> (
    HttpClient,
    jsonrpsee::server::ServerHandle,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let tip = {
        let g = chain.read().await;
        ChainPoint::new(g.tip_height(), g.tip_hash())
    };
    let (p2p, p2p_task) = P2PNetwork::start(
        "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
        Vec::new(),
        Arc::clone(&chain),
        mempool.clone(),
        NetworkTopics::for_network_cfg(&noid_chain::consensus::NetworkConfig::mainnet()),
        [0x55; 32],
        p2p_dir.to_path_buf(),
        BackgroundCapacity::Full,
    )
    .expect("P2PNetwork::start on loopback");
    let health = p2p.health_receiver();
    let cmd = p2p.cmd_tx.clone();
    let (tip_tx, _) = tokio::sync::watch::channel(tip);
    let (_sync_tx, sync_rx) = tokio::sync::watch::channel(true);
    let (_stage_tx, stage_rx) = tokio::sync::watch::channel(NodeSyncStage::Ready);
    let (_mready_tx, mready_rx) = tokio::sync::watch::channel(true);
    let (_peers_tx, peers_rx) = tokio::sync::watch::channel(0usize);
    let (mtmpl_tx, _) = tokio::sync::broadcast::channel(16);

    let mut bound_addr: Option<SocketAddr> = None;
    let mut rpc_handle = None;
    for _ in 0..10 {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        if let Ok((handle, _stop_rx)) = start_rpc_server(
            addr,
            Arc::clone(&chain),
            mempool.clone(),
            wallet.clone(),
            Arc::new(tokio::sync::Mutex::new(())),
            cmd.clone(),
            health.clone(),
            tip_tx.clone(),
            sync_rx.clone(),
            stage_rx.clone(),
            mready_rx.clone(),
            peers_rx.clone(),
            0,
            true,
            false,
            "portable".into(),
            1,
            1,
            Some(Arc::clone(&runtime)),
            None,
            ExternalMiningAttemptInvalidator::new(),
            mtmpl_tx.clone(),
            false,
            None,
            None,
            None,
            false,
        )
        .await
        {
            bound_addr = Some(addr);
            rpc_handle = Some(handle);
            break;
        }
    }
    let addr = bound_addr.expect("bind loopback RPC server");
    let client = HttpClientBuilder::default()
        .build(format!("http://{addr}"))
        .expect("build loopback HttpClient");
    (client, rpc_handle.unwrap(), p2p_task)
}
