//! State-machine conformance tests for Storage, Native Indexes, Wallet State,
//! Segment Expansion Rollback, and External Miner Fee Accounting.
//!
//! Run with:
//!   CARGO_TARGET_DIR="$PWD/target" cargo test --release --offline -- --nocapture

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

use noid_chain::consensus::fees::{claimable_fee_for_tx_body, fee_breakdown};
use noid_chain::consensus::params::{EXPAND_DENOM, EXPAND_NUM, LOG_SEGMENT_SIZE, LOG_SLOTS_GENESIS};
use noid_chain::exact_segment_cache::ExactSegmentCache;
use noid_chain::fri_state::SlotValue;
use noid_chain::state::ChainState;
use noid_poseidon2b::primitives::Address;
use noid_rpc::wallet_ops::WalletOps;
use storage_runtime_seam::helpers::{
    accept_bundle, apply_p2p_reorg, bootstrap_mainnet_v21_context,
    build_buy_trade_proof_request, build_create_launch_proof_request, make_v21_mempool,
    mine_next_bundle_with_native,
};
use storage_runtime_seam::wallet::{self, SharedWallet, WalletHandle, WalletState};
use tempfile::TempDir;

/// Seam 4 (Mainnet `log_slots = 24 -> 25` LRU capacity check):
/// `ExactSegmentCache::DEFAULT_CACHE_BYTES` (64 MiB) retains at most 9 segments at
/// `LOG_SEGMENT_SIZE = 16` (7 MiB per resident segment), whereas reaching the 75%
/// expansion threshold (`12,582,912` slots) requires at least 192 non-empty segments.
/// `shrink_to_log_slots` flushes `SegmentedFriState::dirty` even on evicted segments,
/// whereas `shrink_exact_metadata_to_log_slots` preserves evicted summaries cleanly.
#[test]
fn conformance_seam4_lru_eviction_vs_shrink_to_log_slots() {
    let min_segments_for_log24_expansion =
        (((1u64 << LOG_SLOTS_GENESIS) * EXPAND_NUM / EXPAND_DENOM) + ((1u64 << LOG_SEGMENT_SIZE) - 1))
            / (1u64 << LOG_SEGMENT_SIZE);
    assert_eq!(min_segments_for_log24_expansion, 192);
    assert_eq!(ExactSegmentCache::DEFAULT_CACHE_BYTES, 64 * 1024 * 1024);

    let mut state = ChainState::new();
    for seg_id in 0u32..10u32 {
        let slot_idx = seg_id << LOG_SEGMENT_SIZE;
        state.state.set_slot(
            slot_idx,
            SlotValue {
                amount: 10_000,
                creation_id: u64::from(seg_id) + 1,
                owner_hi: 1,
                owner_lo: 2,
            },
        );
    }
    state.active_slot_count = 10;
    let pre_expand_root = state.try_state_root().expect("exact root before expansion");
    state.clear_dirty();
    let evicted = state.evict_cold_persisted_segments();
    assert!(evicted >= 1, "10th 7-MiB segment exceeds 64-MiB cache budget");
    assert!(state.state.is_evicted(0), "oldest segment 0 must be evicted");

    // Metadata-only restore path succeeds and preserves exact root.
    let mut safe_clone = state.durable_metadata_clone().expect("durable clone");
    safe_clone.state.expand_one();
    assert_eq!(safe_clone.state.log_slots(), 25);
    safe_clone
        .state
        .shrink_exact_metadata_to_log_slots(24)
        .expect("metadata shrink succeeds with evicted segments");
    let restored_root = safe_clone
        .try_state_root()
        .expect("exact root after metadata shrink");
    assert_eq!(restored_root, pre_expand_root);

    // Reorg path (`mdbx_context.rs:2068`) calls `shrink_to_log_slots`, triggering assertion
    // on evicted FRI-dirty segment 0 even in `--release`.
    state.state.expand_one();
    let unwind = catch_unwind(AssertUnwindSafe(|| {
        let _ = state.state.shrink_to_log_slots(24);
    }));
    assert!(
        unwind.is_err(),
        "shrink_to_log_slots asserted on evicted FRI-dirty segment 0"
    );
}

/// Seam 1: `MempoolEvent::TxEvicted` (`TradePolicyExpired`) leaves wallet pending
/// reservations active and blocks `preview_address_discovery` until manual reload.
#[tokio::test]
async fn conformance_seam1_trade_eviction_pending_reservation_state() {
    let dir = TempDir::new().unwrap();
    let chain_dir = dir.path().join("chain");
    let wallet_path = dir.path().join("wallet.json");

    let mut ctx = bootstrap_mainnet_v21_context(&chain_dir).unwrap();
    let ws = WalletState::create_or_load(&wallet_path).unwrap();
    let owner = ws.active_owner_address();
    let shared: SharedWallet = std::sync::Arc::new(std::sync::Mutex::new(Some(ws)));
    let handle = WalletHandle::new(shared.clone());

    // Mine coinbase to fund wallet.
    let (fund_bundle, fund_block) =
        mine_next_bundle_with_native(&ctx, owner, Vec::new(), &[], &[], None).unwrap();
    accept_bundle(&mut ctx, &fund_bundle).unwrap();
    wallet::update_for_accepted_block(&shared, &fund_block, &ctx.store);

    // Create a Launchpad asset in the next block.
    let (create_req, asset_id, create_openings) =
        build_create_launch_proof_request(&ctx, &handle, 1_000, 0, [0x44; 32]).unwrap();
    let WalletOpsSubmission { tx_pages, .. } = {
        let noid_rpc::wallet_ops::WalletNativeIntent::Operation(op) = &create_req.intent else {
            unreachable!();
        };
        struct WalletOpsSubmission {
            tx_pages: Vec<noid_tx::Transaction>,
        }
        WalletOpsSubmission {
            tx_pages: op
                .pages
                .iter()
                .map(|p| noid_tx::Transaction::new(p.body.clone()))
                .collect(),
        }
    };
    let (launch_bundle, launch_block) = mine_next_bundle_with_native(
        &ctx,
        Address([0x99; 32]),
        tx_pages,
        &create_openings,
        &[],
        None,
    )
    .unwrap();
    accept_bundle(&mut ctx, &launch_bundle).unwrap();
    wallet::update_for_accepted_block(&shared, &launch_block, &ctx.store);

    // Admit a short-TTL TradeOwnerIntent (`valid_until = tip + 1`) to AsyncMempool.
    let mempool = make_v21_mempool(&ctx);
    let mut events = mempool.subscribe();
    let valid_until = ctx.tip_height() + 1;
    let (trade_req, _trade) =
        build_buy_trade_proof_request(&ctx, &handle, asset_id, 50_000, valid_until).unwrap();
    let proved = handle.prove_native_submission(trade_req).unwrap();
    let _lease = handle.reserve_native_submission(&proved).unwrap();
    mempool.submit_encoded(proved.intent_bytes.clone()).await.unwrap();
    assert_eq!(mempool.len().await, 1);

    // Advance 2 empty blocks so `TradePolicyExpired` evicts the trade intent.
    for _ in 0..2 {
        let (b, blk) =
            mine_next_bundle_with_native(&ctx, Address([0x99; 32]), Vec::new(), &[], &[], None)
                .unwrap();
        accept_bundle(&mut ctx, &b).unwrap();
        wallet::update_for_accepted_block(&shared, &blk, &ctx.store);
        let view = noid_mempool::ChainView::from_mdbx(&ctx);
        mempool.on_new_block(&blk, view).await;
    }
    assert_eq!(mempool.len().await, 0);

    let mut saw_expired = false;
    while let Ok(ev) = events.try_recv() {
        if matches!(
            ev,
            noid_mempool::MempoolEvent::TxEvicted {
                reason: noid_mempool::EvictReason::TradePolicyExpired,
                ..
            }
        ) {
            saw_expired = true;
        }
    }
    assert!(saw_expired, "expected TradePolicyExpired eviction event");

    // Wallet still reports reserved UTXO and rejects address discovery while mempool is empty.
    assert_eq!(handle.get_balance().spendable_micronoid, 0);
    assert!(handle.list_utxos().iter().all(|u| u.reserved));
    assert_eq!(handle.native_pending_submissions().unwrap().len(), 1);
    assert!(handle.preview_address_discovery(4).is_err());

    // Manual reconciliation with empty mempool reservation set releases the slot.
    handle
        .reconcile_native_pending(&HashSet::new(), &HashSet::new())
        .unwrap();
    assert!(handle.get_balance().spendable_micronoid > 0);
}

/// External Miner (`noid_rpc/src/server.rs:1318-1329` vs `validation.rs:248-286`):
/// Per-body `claimable_fee_for_tx_body` sum omits state-growth burn on multi-page
/// v2.1 Native Asset groups where page 0 (`NATIVE_HEADER_BIT`) has 0 live inputs/outputs
/// and carries `fee = total_fee`, while pages `1..k` carry `fee = 0`.
#[test]
fn conformance_external_miner_claimable_fee_native_vs_consensus() {
    let dir = TempDir::new().unwrap();
    let chain_dir = dir.path().join("chain");
    let wallet_path = dir.path().join("wallet.json");

    let mut ctx = bootstrap_mainnet_v21_context(&chain_dir).unwrap();
    let ws = WalletState::create_or_load(&wallet_path).unwrap();
    let owner = ws.active_owner_address();
    let shared: SharedWallet = std::sync::Arc::new(std::sync::Mutex::new(Some(ws)));
    let handle = WalletHandle::new(shared.clone());

    let (fund_bundle, fund_block) =
        mine_next_bundle_with_native(&ctx, owner, Vec::new(), &[], &[], None).unwrap();
    accept_bundle(&mut ctx, &fund_bundle).unwrap();
    wallet::update_for_accepted_block(&shared, &fund_block, &ctx.store);

    let (create_req, _asset_id, create_openings) =
        build_create_launch_proof_request(&ctx, &handle, 1_000, 1_000_000, [0x55; 32]).unwrap();
    let noid_rpc::wallet_ops::WalletNativeIntent::Operation(op) = &create_req.intent else {
        unreachable!();
    };
    let tx_pages: Vec<noid_tx::Transaction> = op
        .pages
        .iter()
        .map(|p| noid_tx::Transaction::new(p.body.clone()))
        .collect();

    let parent = *ctx.tip_header();
    let (_bundle, block) = mine_next_bundle_with_native(
        &ctx,
        Address([0x88; 32]),
        tx_pages.clone(),
        &create_openings,
        &[],
        None,
    )
    .unwrap();

    // Consensus logical-group fee breakdown (`1` input -> `5` outputs = 4 net new slots).
    let logical_fb = fee_breakdown(1, 5, parent.active_slot_count, parent.log_slots);
    assert_eq!(logical_fb.burned, 4 * 2_500);

    // RPC `prepare_external_mining_attempt` (`noid_rpc/src/server.rs:1318-1329`) per-body sum:
    let rpc_reported_claimable: u64 = tx_pages
        .iter()
        .map(|tx| {
            claimable_fee_for_tx_body(&tx.body, parent.active_slot_count, parent.log_slots)
        })
        .sum();

    let subsidy = noid_chain::consensus::miner_subsidy(block.header.height, block.header.log_slots);
    let coinbase_amount = block.transactions[0].body.outputs[0].amount;
    let consensus_claimable = coinbase_amount - subsidy;

    assert_eq!(consensus_claimable, logical_fb.claimable);
    assert_eq!(
        rpc_reported_claimable - consensus_claimable,
        logical_fb.burned,
        "per-body RPC calculation omits burned state-growth fee on v2.1 Native groups"
    );
}
