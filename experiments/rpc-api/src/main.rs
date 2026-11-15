//! PoC: receipt-pagination resource behavior of the parano1d RPC API.
//!
//! Exercises the real `noid_rpc::start_rpc_server` HTTP+JSON-RPC stack (pinned
//! checkout) against a synthetic wallet whose receipt store holds N real,
//! cryptographically valid `ParanoidReceipt`s.
//!
//! Phase A instruments `paranoid_walletQueryReceipts`
//! (noid_rpc/src/server/receipt_query.rs) through the real handler.
//! Phase B mirrors the production `filtered_receipts` scan body
//! (noid_node/src/wallet/mod.rs:910-967; the type is binary-private) behind
//! the real `paranoid_walletReceipts` handler.
//! No network egress; loopback only; synthetic keys only.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use noid_chain::{
    consensus::receipt::{generate_receipt, ParanoidReceipt},
    storage::MdbxChainContext,
};
use noid_poseidon2b::primitives::Address;
use noid_rpc::{
    types::{
        WalletAddressInfo, WalletBalance, WalletConsolidationPlan, WalletHistoryEntry,
        WalletScanResult, WalletSendPlan, WalletStatus, WalletUtxoInfo,
    },
    wallet_ops::{
        WalletActivationPreview, WalletAddressDiscoveryPreview, WalletMinedBlockSlice,
        WalletNativeProofRequest, WalletOps, WalletReceiptQueryIndex, WalletReceiptRecord,
        WalletReceiptSlice, WalletSendPlanError,
    },
};
use noid_tx::{
    output_bitmap_bit, PagedSpendIntent, Transaction, TxBody, TxInput, TxOutput, TxPage,
    PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;

const RECEIPT_COUNT: usize = 20_000;
const PAGE_SIZE: u32 = 50;
const REVISION: &str = "synthetic-receipt-library-v1";

#[derive(Clone, Copy, Debug, Default)]
struct AllocationStats {
    current: usize,
    peak: usize,
}

thread_local! {
    static QUERY_ALLOCATIONS: Cell<Option<AllocationStats>> = const { Cell::new(None) };
}

struct TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            track(layout.size(), 0);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        track(0, layout.size());
        System.dealloc(pointer, layout)
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let pointer = System.realloc(pointer, layout, new_size);
        if !pointer.is_null() {
            track(new_size, layout.size());
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn track(allocated: usize, deallocated: usize) {
    let _ = QUERY_ALLOCATIONS.try_with(|stats| {
        let Some(mut current) = stats.get() else {
            return;
        };
        current.current = current.current.saturating_add(allocated);
        current.current = current.current.saturating_sub(deallocated);
        current.peak = current.peak.max(current.current);
        stats.set(Some(current));
    });
}

fn begin_tracking() {
    let _ = QUERY_ALLOCATIONS.try_with(|stats| {
        if stats.get().is_none() {
            stats.set(Some(AllocationStats::default()));
        }
    });
}

fn finish_tracking() -> AllocationStats {
    QUERY_ALLOCATIONS
        .try_with(|stats| stats.take())
        .unwrap_or_default()
        .unwrap_or_default()
}

struct SyntheticWallet {
    receipts: BTreeMap<[u8; 32], Vec<u8>>,
    observed_allocation_peak: Mutex<Option<AllocationStats>>,
    /// Mirrors the single wallet mutex the production wallet holds across the
    /// whole `filtered_receipts` scan (noid_node/src/wallet/mod.rs:919).
    scan_lock: Mutex<()>,
    scan_micros_total: Mutex<u128>,
    scan_count: Mutex<usize>,
}

impl SyntheticWallet {
    fn new(receipts: BTreeMap<[u8; 32], Vec<u8>>) -> Self {
        Self {
            receipts,
            observed_allocation_peak: Mutex::new(None),
            scan_lock: Mutex::new(()),
            scan_micros_total: Mutex::new(0),
            scan_count: Mutex::new(0),
        }
    }

    fn take_allocation_peak(&self) -> AllocationStats {
        self.observed_allocation_peak
            .lock()
            .unwrap()
            .take()
            .expect("query reached receipt_query_records")
    }

    fn scan_stats(&self) -> (u128, usize) {
        (
            *self.scan_micros_total.lock().unwrap(),
            *self.scan_count.lock().unwrap(),
        )
    }
}

impl WalletOps for SyntheticWallet {
    fn receipt_query_index(&self) -> Result<WalletReceiptQueryIndex, String> {
        Ok(WalletReceiptQueryIndex {
            revision: REVISION.into(),
            ids: self.receipts.keys().copied().collect(),
        })
    }

    fn receipt_query_bytes(&self, revision: &str, id: &[u8; 32]) -> Result<Vec<u8>, String> {
        if revision != REVISION {
            return Err("unexpected synthetic revision".into());
        }
        begin_tracking();
        self.receipts
            .get(id)
            .cloned()
            .ok_or_else(|| "synthetic receipt disappeared".into())
    }

    fn receipt_query_records(
        &self,
        revision: &str,
        ids: &[[u8; 32]],
    ) -> Result<Vec<WalletReceiptRecord>, String> {
        if revision != REVISION {
            return Err("unexpected synthetic revision".into());
        }
        *self.observed_allocation_peak.lock().unwrap() = Some(finish_tracking());
        Ok(ids
            .iter()
            .map(|id| WalletReceiptRecord {
                txid: *id,
                height: 1,
                timestamp: 1,
                amount_micronoid: 1,
                fee_micronoid: 1,
                peer_address: None,
                own_address: None,
                own_key_index: None,
                input_count: 1,
                output_count: 1,
                receipt_bytes: self.receipts[id].len(),
                trade: None,
                native: None,
            })
            .collect())
    }

    /// Faithful mirror of the production scan body in
    /// `noid_node/src/wallet/mod.rs:910-967` (`WalletOps::filtered_receipts`
    /// for the node wallet): a single mutex is held across the entire scan;
    /// every stored receipt is fetched, decoded, identity-checked and
    /// classified on every call, then sorted and sliced for one page.
    /// The synthetic store only contains payment receipts (kind 0).
    fn receipts(&self, offset: usize, limit: usize) -> Result<WalletReceiptSlice, String> {
        let started = std::time::Instant::now();
        let _guard = self.scan_lock.lock().unwrap();
        let result = self.receipts_scan(offset, limit);
        let mut total = self.scan_micros_total.lock().unwrap();
        *total = total.saturating_add(started.elapsed().as_micros());
        *self.scan_count.lock().unwrap() += 1;
        result
    }

    fn status(&self) -> WalletStatus {
        WalletStatus {
            exists: true,
            address: String::new(),
            active_index: 0,
            balance_micronoid: 0,
            balance_noid: 0.0,
            utxo_count: 0,
            address_count: 0,
        }
    }

    fn get_address(&self, _index: u32) -> Option<String> {
        None
    }

    fn get_balance(&self) -> WalletBalance {
        WalletBalance {
            balance_micronoid: 0,
            balance_noid: 0.0,
            utxo_count: 0,
            pending_outbound_micronoid: 0,
            pending_incoming_micronoid: 0,
            spendable_micronoid: 0,
            spendable_noid: 0.0,
        }
    }

    fn list_utxos(&self) -> Vec<WalletUtxoInfo> {
        Vec::new()
    }

    fn history(&self) -> Vec<WalletHistoryEntry> {
        Vec::new()
    }

    fn mined_blocks(&self, _offset: usize, _limit: usize) -> WalletMinedBlockSlice {
        WalletMinedBlockSlice {
            total: 0,
            blocks: Vec::new(),
        }
    }

    fn preview_active_reload(&self) -> Result<WalletActivationPreview, String> {
        Err("not used by this PoC".into())
    }

    fn preview_address_switch(&self, _index: u32) -> Result<WalletActivationPreview, String> {
        Err("not used by this PoC".into())
    }

    fn create_next_address(&self) -> Result<WalletAddressInfo, String> {
        Err("not used by this PoC".into())
    }

    fn preview_address_discovery(
        &self,
        _max_additional: u32,
    ) -> Result<WalletAddressDiscoveryPreview, String> {
        Err("not used by this PoC".into())
    }

    fn commit_address_discovery(
        &self,
        _expected_active_index: u32,
        _expected_next_index: u32,
        _discovered_next_index: u32,
    ) -> Result<Vec<WalletAddressInfo>, String> {
        Err("not used by this PoC".into())
    }

    fn commit_activation_snapshot(
        &self,
        _preview: WalletActivationPreview,
        _snapshot: noid_chain::storage::VerifiedOwnerSnapshot,
        _reserved_input_slots: &std::collections::HashSet<u32>,
        _reserved_output_slots: &std::collections::HashSet<u32>,
    ) -> Result<(WalletAddressInfo, WalletScanResult), String> {
        Err("not used by this PoC".into())
    }

    fn on_accepted_block(&self, _block: &noid_chain::block::Block) -> Result<(), String> {
        Ok(())
    }

    fn plan_send(
        &self,
        _amount_micronoid: u64,
        _explicit_fee_micronoid: Option<u64>,
        _active_slot_count: u64,
        _log_slots: u32,
        _relay_floor: u64,
    ) -> Result<WalletSendPlan, WalletSendPlanError> {
        Err(WalletSendPlanError::Other("not used by this PoC".into()))
    }

    fn build_send(
        &self,
        _to_address: [u8; 32],
        _amount_micronoid: u64,
        _fee_micronoid: u64,
        _epoch_anchor: [u8; 32],
        _slot_hints: Vec<u32>,
        _log_slots: u32,
        _candidate_height: u64,
    ) -> Result<(Vec<u8>, Vec<u32>), String> {
        Err("not used by this PoC".into())
    }

    fn plan_consolidation(
        &self,
        _max_inputs: usize,
        _active_slot_count: u64,
        _log_slots: u32,
        _relay_floor: u64,
    ) -> Result<WalletConsolidationPlan, WalletSendPlanError> {
        Err(WalletSendPlanError::Other("not used by this PoC".into()))
    }

    fn build_consolidation(
        &self,
        _selected_input_slots: Vec<u32>,
        _output_value_micronoid: u64,
        _fee_micronoid: u64,
        _epoch_anchor: [u8; 32],
        _slot_hints: Vec<u32>,
        _log_slots: u32,
        _candidate_height: u64,
    ) -> Result<(Vec<u8>, Vec<u32>), String> {
        Err("not used by this PoC".into())
    }

    fn export_receipt(&self, _txhash_hex: &str) -> Result<String, String> {
        Err("not used by this PoC".into())
    }

    fn reserve_pending_submission(
        &self,
        _txid: [u8; 32],
        _input_slots: &[u32],
        _output_slots: &[u32],
        _amount_micronoid: u64,
        _peer_address: [u8; 32],
    ) -> Result<(), String> {
        Err("not used by this PoC".into())
    }

    fn rollback_pending_submission(
        &self,
        _txid: [u8; 32],
        _input_slots: &[u32],
        _output_slots: &[u32],
    ) {
    }

    fn active_address(&self) -> Option<(u32, String)> {
        None
    }

    fn list_addresses(&self) -> Vec<WalletAddressInfo> {
        Vec::new()
    }

    fn pending_outbound(&self) -> u64 {
        0
    }

    fn build_native_authorization(
        &self,
        _request: WalletNativeProofRequest,
        _store: noid_chain::storage::MdbxStore,
    ) -> Result<noid_rpc::wallet_ops::WalletNativeSubmission, String> {
        Err("not used by this PoC".into())
    }
}

impl SyntheticWallet {
    /// Inner scan executed while holding `scan_lock`, mirroring
    /// noid_node/src/wallet/mod.rs:927-966 (candidate collection, newest-first
    /// sort, offset/limit slice, then per-page record materialization).
    fn receipts_scan(&self, offset: usize, limit: usize) -> Result<WalletReceiptSlice, String> {
        if self.receipts.is_empty() {
            return Ok(WalletReceiptSlice {
                total: 0,
                receipts: Vec::new(),
            });
        }
        let mut candidates = Vec::with_capacity(self.receipts.len());
        for (id, bytes) in &self.receipts {
            let receipt = ParanoidReceipt::from_bytes(bytes).map_err(|e| e.to_string())?;
            if receipt.summary.logical_txid != *id {
                return Err("durable receipt key differs from authenticated identity".into());
            }
            let different_recipient = receipt.summary.inputs.first().is_some_and(|(_, owner)| {
                receipt
                    .summary
                    .outputs
                    .iter()
                    .any(|(_, _, recipient)| recipient != owner)
            });
            let kind = if receipt.is_trade() {
                1u8
            } else if receipt.is_native() {
                2
            } else {
                0
            };
            if (receipt.is_trade() || receipt.is_native() || different_recipient) && kind == 0
            // synthetic store holds payment receipts only
            {
                candidates.push((
                    receipt.claimed_height,
                    receipt.summary.confirmed_unix,
                    *id,
                    kind,
                ));
            }
        }
        candidates.sort_unstable_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| b.2.cmp(&a.2))
        });
        let total = candidates.len();
        let selected: Vec<_> = candidates.into_iter().skip(offset).take(limit).collect();
        // Record materialization mirrors the kind-0 arm of
        // noid_node/src/wallet/receipt_query.rs:117-149.
        let mut records = Vec::with_capacity(selected.len());
        for (_, _, id, kind) in selected {
            assert_eq!(kind, 0, "synthetic store must only hold payments");
            let bytes = &self.receipts[&id];
            let receipt = ParanoidReceipt::from_bytes(bytes).map_err(|e| e.to_string())?;
            let input_owner = receipt.summary.inputs.first().map(|(_, owner)| *owner);
            let outputs: Vec<_> = receipt
                .summary
                .outputs
                .iter()
                .filter(|(_, _, owner)| Some(*owner) != input_owner)
                .collect();
            let amount = outputs
                .iter()
                .map(|(_, amount, _)| *amount)
                .fold(0u64, u64::saturating_add);
            let peer = (outputs.len() == 1).then(|| outputs[0].2.to_bech32());
            records.push(WalletReceiptRecord {
                txid: id,
                height: receipt.claimed_height,
                timestamp: receipt.summary.confirmed_unix,
                amount_micronoid: amount,
                fee_micronoid: receipt.summary.fee_micronoid,
                peer_address: peer,
                own_address: input_owner.map(|owner| owner.to_bech32()),
                own_key_index: None,
                input_count: receipt.summary.inputs.len(),
                output_count: receipt.summary.outputs.len(),
                receipt_bytes: bytes.len(),
                trade: None,
                native: None,
            });
        }
        Ok(WalletReceiptSlice {
            total,
            receipts: records,
        })
    }
}

fn synthetic_receipts() -> BTreeMap<[u8; 32], Vec<u8>> {
    (0..RECEIPT_COUNT)
        .map(|offset| {
            let index = offset as u32 + 1;
            let mut inputs = [TxInput::dummy(); TX_INPUTS];
            inputs[0] = TxInput {
                slot_index: index,
                amount: u64::from(index) + 10,
                creation_id: u64::from(index),
            };
            let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
            outputs[0] = TxOutput {
                slot_index: index + 100_000,
                amount: u64::from(index) + 9,
                owner: Address([2; 32]),
            };
            let mut epoch_anchor = [0; 32];
            epoch_anchor[..4].copy_from_slice(&index.to_le_bytes());
            let transaction = Transaction::new(TxBody {
                epoch_anchor,
                fee: 1,
                input_owner: Address([1; 32]),
                inputs,
                outputs,
                validity_bitmap: 1
                    | output_bitmap_bit(0)
                    | PAGED_SPEND_START_BIT
                    | PAGED_SPEND_END_BIT,
                is_coinbase: false,
            });
            let logical_txid = PagedSpendIntent::new(
                vec![TxPage {
                    body: transaction.body.clone(),
                }],
                Vec::new(),
            )
            .unwrap()
            .logical_txid();
            let hashes = [[0xcb; 32], logical_txid.0];
            let mut header = noid_chain::consensus::genesis_header();
            header.height = u64::from(index);
            header.timestamp += u64::from(index);
            header.tx_root = noid_chain::tx_tree::root_from_hashes(&hashes);
            let receipt = generate_receipt(&header, std::slice::from_ref(&transaction), 1, &hashes);
            assert!(noid_chain::consensus::receipt::verify_merkle_inclusion(
                &receipt
            ));
            (logical_txid.0, receipt.to_bytes())
        })
        .collect()
}

async fn rpc_call(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Value {
    client
        .post(url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn query_receipts(client: &reqwest::Client, url: &str, page: u32) -> (Value, u128) {
    let started = std::time::Instant::now();
    let response = rpc_call(
        client,
        url,
        "paranoid_walletQueryReceipts",
        json!([page, PAGE_SIZE, {}]),
    )
    .await;
    let elapsed = started.elapsed().as_micros();
    assert!(response.get("error").is_none(), "{response}");
    (response["result"].clone(), elapsed)
}

/// Like `query_receipts` but tolerates the single-permit gate rejecting the
/// call; used for the concurrency control where either side may win the gate.
async fn query_receipts_gated(
    client: &reqwest::Client,
    url: &str,
    page: u32,
) -> (Result<Value, String>, u128) {
    let started = std::time::Instant::now();
    let response = rpc_call(
        client,
        url,
        "paranoid_walletQueryReceipts",
        json!([page, PAGE_SIZE, {}]),
    )
    .await;
    let elapsed = started.elapsed().as_micros();
    if let Some(error) = response.get("error") {
        (
            Err(error["message"].as_str().unwrap_or_default().to_owned()),
            elapsed,
        )
    } else {
        (Ok(response["result"].clone()), elapsed)
    }
}

async fn receipts_page(client: &reqwest::Client, url: &str, page: u32) -> (Value, u128) {
    let started = std::time::Instant::now();
    let response = rpc_call(
        client,
        url,
        "paranoid_walletReceipts",
        json!([page, PAGE_SIZE, null]),
    )
    .await;
    let elapsed = started.elapsed().as_micros();
    assert!(response.get("error").is_none(), "{response}");
    (response["result"].clone(), elapsed)
}

async fn chain_info_latency(client: &reqwest::Client, url: &str) -> Duration {
    let started = std::time::Instant::now();
    let response = rpc_call(client, url, "paranoid_getChainInfo", json!([])).await;
    assert!(response.get("error").is_none(), "{response}");
    started.elapsed()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    println!("== setup ==");
    println!(
        "parano1d RPC receipt-pagination PoC; tokio worker_threads=4 (production nodes default to CPU count)"
    );
    let scratch = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp");
    std::fs::create_dir_all(&scratch).unwrap();
    let directory = tempfile::tempdir_in(&scratch).unwrap();
    let context = MdbxChainContext::open_or_create(directory.path()).unwrap();
    let mempool = noid_mempool::AsyncMempool::new(
        noid_mempool::ChainView::from_mdbx(&context),
        noid_mempool::MempoolConfig::default(),
    );
    let tip = noid_p2p::object_protocol::ChainPoint::new(context.tip_height(), context.tip_hash());
    let chain = Arc::new(RwLock::new(context));
    let (network, task) = noid_p2p::P2PNetwork::start(
        "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
        vec![],
        chain.clone(),
        mempool.clone(),
        noid_p2p::NetworkTopics::for_network_cfg(&noid_chain::consensus::NetworkConfig::mainnet()),
        [0; 32],
        directory.path().to_path_buf(),
        noid_p2p::BackgroundCapacity::Full,
    )
    .unwrap();
    task.abort();
    let _ = task.await;

    let wallet = Arc::new(SyntheticWallet::new(synthetic_receipts()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let (server, _stop) = noid_rpc::start_rpc_server(
        address,
        chain,
        mempool,
        wallet.clone(),
        Arc::new(tokio::sync::Mutex::new(())),
        network.cmd_tx.clone(),
        tokio::sync::watch::channel(noid_p2p::P2PHealthSnapshot::default()).1,
        tokio::sync::watch::channel(tip).0,
        tokio::sync::watch::channel(true).1,
        tokio::sync::watch::channel(noid_rpc::types::NodeSyncStage::Tip).1,
        tokio::sync::watch::channel(false).1,
        tokio::sync::watch::channel(0).1,
        0,
        true,
        false,
        "synthetic-test-backend".into(),
        1,
        1,
        None,
        None,
        noid_rpc::ExternalMiningAttemptInvalidator::new(),
        tokio::sync::broadcast::channel(1).0,
        false,
        None,
        None,
        None,
        false,
    )
    .await
    .unwrap();

    let url = format!("http://{address}");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // ------------------------------------------------------------------
    println!("\n== phase A: paranoid_walletQueryReceipts (gated, spawn_blocking) ==");
    println!(
        "synthetic valid receipts: {RECEIPT_COUNT}; page_size: {PAGE_SIZE}; query filter: default"
    );

    let (first, first_us) = query_receipts(&client, &url, 1).await;
    let first_peak = wallet.take_allocation_peak();
    let (second, second_us) = query_receipts(&client, &url, 2).await;
    let second_peak = wallet.take_allocation_peak();
    let past_end_page = (RECEIPT_COUNT / PAGE_SIZE as usize + 1) as u32;
    let (past_end, past_end_us) = query_receipts(&client, &url, past_end_page).await;
    let past_end_peak = wallet.take_allocation_peak();
    let (far, far_us) = query_receipts(&client, &url, u32::MAX).await;
    let far_peak = wallet.take_allocation_peak();

    assert_eq!(first["total"], RECEIPT_COUNT);
    assert_eq!(
        first["receipts"].as_array().unwrap().len(),
        PAGE_SIZE as usize
    );
    assert_eq!(
        second["receipts"].as_array().unwrap().len(),
        PAGE_SIZE as usize
    );
    assert_eq!(past_end["total"], RECEIPT_COUNT);
    assert!(past_end["receipts"].as_array().unwrap().is_empty());
    assert_eq!(far["total"], RECEIPT_COUNT);
    assert!(far["receipts"].as_array().unwrap().is_empty());

    println!(
        "A1 page 1      (offset     0, required    50): {} rows, wall {first_us} us, handler alloc peak {:>8} B",
        first["receipts"].as_array().unwrap().len(),
        first_peak.peak
    );
    println!(
        "A2 page 2      (offset    50, required   100): {} rows, wall {second_us} us, handler alloc peak {:>8} B",
        second["receipts"].as_array().unwrap().len(),
        second_peak.peak
    );
    println!(
        "A3 page {past_end_page:4}   (offset {:-6}, required {:-6}): {} rows, wall {past_end_us} us, handler alloc peak {:>8} B",
        (past_end_page as usize - 1) * PAGE_SIZE as usize,
        (past_end_page as usize - 1) * PAGE_SIZE as usize + PAGE_SIZE as usize,
        past_end["receipts"].as_array().unwrap().len(),
        past_end_peak.peak
    );
    let far_offset = (u32::MAX as usize - 1) * PAGE_SIZE as usize;
    println!(
        "A4 page u32::MAX (offset {far_offset}, required {required}): {} rows, wall {far_us} us, handler alloc peak {:>8} B",
        far["receipts"].as_array().unwrap().len(),
        far_peak.peak,
        required = far_offset + PAGE_SIZE as usize
    );

    let amplification = far_peak.peak.saturating_sub(first_peak.peak);
    println!("A4-A1 out-of-range peak amplification: {amplification} B");
    assert!(
        amplification >= 1_000_000,
        "expected the far page to retain the complete synthetic key set"
    );
    assert!(
        past_end_peak.peak.saturating_sub(first_peak.peak) >= 1_000_000,
        "retention must begin at the first page past the end, not only at extreme pages"
    );
    assert!(
        second_peak.peak < first_peak.peak + 100_000,
        "in-range pages must keep the bounded top-k working set"
    );

    // Gate control: a second concurrent query must be rejected while one runs.
    let url_gate = url.clone();
    let client_gate = client.clone();
    let side_a =
        tokio::spawn(async move { query_receipts_gated(&client_gate, &url_gate, 1).await });
    let (side_b, _) = query_receipts_gated(&client, &url, 1).await;
    let (side_a, _) = side_a.await.unwrap();
    let _ = wallet.take_allocation_peak();
    let (winner_result, gate_error) = match (&side_a, &side_b) {
        (Ok(result), Err(error)) | (Err(error), Ok(result)) => (result, error.clone()),
        _ => panic!("expected exactly one gated query to succeed: A={side_a:?} B={side_b:?}"),
    };
    println!(
        "A5 concurrent-query gate: winner returned {} rows; loser error: {gate_error}",
        winner_result["receipts"].as_array().unwrap().len()
    );
    assert_eq!(winner_result["total"], RECEIPT_COUNT);
    assert!(
        gate_error.contains("already running"),
        "expected the single-permit gate to reject the concurrent query"
    );

    // No-cache control: repeated identical page-1 queries each rescan.
    let mut repeat_walls = Vec::new();
    for _ in 0..3 {
        let (repeat, repeat_us) = query_receipts(&client, &url, 1).await;
        let _ = wallet.take_allocation_peak();
        assert_eq!(
            repeat["receipts"].as_array().unwrap().len(),
            PAGE_SIZE as usize
        );
        repeat_walls.push(repeat_us);
    }
    println!(
        "A6 repeated page-1 walls: {repeat_walls:?} us (each a complete rescan; no cross-call cache)"
    );
    assert!(
        repeat_walls.iter().all(|wall| *wall > 1_000_000),
        "every page-1 call must pay the full O(N) rescan"
    );
    let query_per_receipt = repeat_walls[0] / RECEIPT_COUNT as u128;
    println!(
        "A7 query-path per-receipt cost: ~{query_per_receipt} us/receipt (fetch+clone+decode+identity+merkle re-verification+bech32 metadata)"
    );

    // ------------------------------------------------------------------
    println!("\n== phase B: paranoid_walletReceipts (ungated, inline async scan) ==");

    let (idle_p50, idle_max) = {
        let mut samples = Vec::new();
        for _ in 0..20 {
            samples.push(chain_info_latency(&client, &url).await.as_micros());
        }
        samples.sort_unstable();
        (samples[samples.len() / 2], *samples.last().unwrap())
    };
    println!("B1 idle getChainInfo latency: p50 {idle_p50} us, max {idle_max} us over 20 probes");

    let (page_one, page_one_us) = receipts_page(&client, &url, 1).await;
    assert_eq!(page_one["total"], RECEIPT_COUNT);
    assert_eq!(
        page_one["receipts"].as_array().unwrap().len(),
        PAGE_SIZE as usize
    );
    println!(
        "B2 walletReceipts page 1 (production-scan mirror): {} rows, wall {page_one_us} us",
        page_one["receipts"].as_array().unwrap().len()
    );

    // Starvation demo: enough concurrent walletReceipts calls to occupy every
    // tokio worker thread (4 here) while the wallet mutex serializes the scans.
    const CONCURRENT: usize = 16;
    let mut handles = Vec::new();
    for _ in 0..CONCURRENT {
        let client_handle = client.clone();
        let url_handle = url.clone();
        handles.push(tokio::spawn(async move {
            receipts_page(&client_handle, &url_handle, 1).await.1
        }));
    }
    let mut loaded = Vec::new();
    let load_started = std::time::Instant::now();
    while load_started.elapsed() < Duration::from_millis(1500) {
        loaded.push(chain_info_latency(&client, &url).await.as_micros());
        if handles.iter().all(|handle| handle.is_finished()) {
            break;
        }
    }
    let mut scan_walls = Vec::new();
    for handle in handles {
        scan_walls.push(handle.await.unwrap());
    }
    loaded.sort_unstable();
    let loaded_p50 = loaded[loaded.len() / 2];
    let loaded_max = *loaded.last().unwrap();
    let (mirror_total_us, mirror_count) = wallet.scan_stats();
    println!(
        "B3 {CONCURRENT} concurrent walletReceipts calls completed in walls {scan_walls:?} us (serialized on the wallet mutex)"
    );
    println!(
        "B4 getChainInfo during load: p50 {loaded_p50} us, max {loaded_max} us over {} probes",
        loaded.len()
    );
    println!(
        "B5 mirror scans executed: {mirror_count}, cumulative scan time {} us (mutex-held window)",
        mirror_total_us
    );
    let starvation = loaded_max as f64 / idle_p50.max(1) as f64;
    println!("B6 worker starvation factor: max_during/idle_p50 = {starvation:.0}x");
    assert!(
        loaded_max > idle_p50.saturating_mul(10),
        "expected unrelated RPC latency to inflate while workers block on the inline scan"
    );

    // ------------------------------------------------------------------
    println!("\n== controls ==");

    let origin_status = client
        .post(&url)
        .header("Origin", "https://attacker.example")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "paranoid_getChainInfo",
            "params": []
        }))
        .send()
        .await
        .unwrap()
        .status();
    println!("C1 browser Origin header -> HTTP {origin_status} (must be 403 Forbidden)");
    assert_eq!(origin_status, reqwest::StatusCode::FORBIDDEN);

    println!("C2 loopback without credentials served all calls above (documented intentional owner access)");
    println!("C3 non-loopback listener without a bearer credential is refused at startup (server.rs:3520-3522; static)");

    server.stop().unwrap();
    server.stopped().await;
    println!("\nALL ASSERTIONS PASSED");
}
