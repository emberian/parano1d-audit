// Wallet-vs-node adversarial harness.
//
// Drives the REAL GUI backend (`noid_gui::backend::Backend`, verbatim copy of
// the pinned crate) against a scripted impostor node that answers every
// `paranoid_*` JSON-RPC call with attacker-chosen JSON over loopback HTTP.
// Each scenario asserts what the real backend returns to the view layer --
// i.e. what present.rs/proofs.rs/explorer.rs would render and what the app
// would record -- proving the deception end-to-end through production client
// code. No live network, no real keys, loopback only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use noid_gui::backend::Backend;
use noid_gui::backend::ExplorerLookup;
use noid_gui::model::{format_micronoid, ReceiptFilter};

const NOID: u64 = 1_000_000_000; // micronoid per NOID (model.rs format_micronoid)

// Two 64-character counterparty strings sharing the first 13 and last 7
// characters (the trusted party and a poisoned lookalike).
fn trusted_peer() -> String {
    format!("o1alice000000{}trusted", "0".repeat(44))
}
fn lookalike_peer() -> String {
    format!("o1alice000000{}trusted", "q".repeat(44))
}

// ----------------------------------------------------------------------
// Scripted impostor node
// ----------------------------------------------------------------------

type Handler = Arc<dyn Fn(&[Value]) -> Value + Send + Sync>;

/// Answers every paranoid_* JSON-RPC request from a mutable script and logs
/// every request the real GUI backend sends. A handler returns the `result`
/// payload, or `{"__rpc_error__": {code, message}}` to emit a JSON-RPC error.
struct Impostor {
    handlers: Mutex<HashMap<String, Handler>>,
    requests: Mutex<Vec<(String, Value)>>,
}

impl Impostor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            handlers: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, method: &str, result: Value) {
        let method = method.to_owned();
        self.handlers
            .lock()
            .unwrap()
            .insert(method, Arc::new(move |_| result.clone()));
    }

    fn set_fn(&self, method: &str, f: impl Fn(&[Value]) -> Value + Send + Sync + 'static) {
        self.handlers
            .lock()
            .unwrap()
            .insert(method.to_owned(), Arc::new(f));
    }

    /// Makes `method` answer with a JSON-RPC error for its first `n` calls.
    fn fail_first(&self, method: &str, n: usize, code: i64, message: &str) {
        let remaining = Arc::new(Mutex::new(n));
        let code = code;
        let message = message.to_owned();
        self.set_fn(method, move |_| {
            let mut guard = remaining.lock().unwrap();
            if *guard > 0 {
                *guard -= 1;
                return json!({"__rpc_error__": {"code": code, "message": message}});
            }
            json!(4321)
        });
    }

    fn calls(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == method)
            .map(|(_, params)| params.clone())
            .collect()
    }

    fn call_count(&self, method: &str) -> usize {
        self.calls(method).len()
    }

    fn distinct_methods(&self) -> Vec<String> {
        let mut methods: Vec<String> = self
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        methods.sort();
        methods.dedup();
        methods
    }
}

async fn serve(listener: TcpListener, impostor: Arc<Impostor>) {
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        let impostor = Arc::clone(&impostor);
        tokio::spawn(async move {
            let _ = handle_connection(stream, impostor).await;
        });
    }
}

async fn handle_connection(mut stream: TcpStream, impostor: Arc<Impostor>) -> std::io::Result<()> {
    // Minimal HTTP/1.1: read headers, then Content-Length body. The reqwest
    // client speaks standard HTTP; we answer and close (Connection: close).
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(pos) = find_subslice(&buffer, b"\r\n\r\n") {
            break pos + 4;
        }
        if buffer.len() > 64 * 1024 {
            return Ok(());
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = &buffer[header_end..header_end + content_length];
    let request: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let params = request.get("params").cloned().unwrap_or_else(|| json!([]));
    let short_method = method
        .strip_prefix("paranoid_")
        .unwrap_or(&method)
        .to_owned();
    impostor
        .requests
        .lock()
        .unwrap()
        .push((short_method.clone(), params.clone()));

    let handler = impostor
        .handlers
        .lock()
        .unwrap()
        .get(&short_method)
        .cloned();
    let payload = match handler {
        None => json!({"__rpc_error__": {"code": -32601, "message": "method not found"}}),
        Some(handler) => handler(params.as_array().map(Vec::as_slice).unwrap_or(&[])),
    };
    let response_body = if let Some(error) = payload.get("__rpc_error__") {
        json!({"jsonrpc": "2.0", "id": id, "error": error})
    } else {
        json!({"jsonrpc": "2.0", "id": id, "result": payload})
    };
    let bytes = response_body.to_string();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(bytes.as_bytes()).await?;
    stream.shutdown().await
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ----------------------------------------------------------------------
// Plausible baseline script (control) and per-scenario poison
// ----------------------------------------------------------------------

fn header_at(height: u64) -> Value {
    json!({
        "height": height,
        "hash": format!("{:064x}", height.wrapping_mul(0x9e3779b97f4a7c15)),
        "prev_hash": format!("{:064x}", height.wrapping_mul(0x9e3779b97f4a7c15) ^ 1),
        "state_root": format!("{:064x}", height.wrapping_mul(0x51ed270b) | 0x1000),
        "tx_root": format!("{:064x}", height.wrapping_mul(0x2545f491) | 0x2000),
        "timestamp": 1_760_000_000u64 + height * 15,
        "miner": "o1minerqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
        "nonce_hex": "00".repeat(32),
        "difficulty_target": format!("{:064x}", 0x0000_ffffu128 << 96),
        "log_slots": 32,
        "active_slot_count": 123_456,
        "alloc_counter": height * 4
    })
}

/// Installs an internally-consistent, honest-shaped answer set: one address,
/// `balance` NOID confirmed, no pending traffic, chain at `height`.
fn script_baseline(node: &Impostor, height: u64, balance_micronoid: u64, utxo_count: usize) {
    let alice = trusted_peer();
    node.set(
        "getChainInfo",
        json!({
            "height": height,
            "best_hash": format!("{:064x}", height.wrapping_mul(0x9e3779b97f4a7c15)),
            "difficulty_target": format!("{:064x}", 0x0000_ffffu128 << 96),
            "active_slot_count": 123_456,
            "log_slots": 32,
            "circulating_supply_micronoid": "1000000000000"
        }),
    );
    node.set(
        "getStateInfo",
        json!({"log_slots": 32, "capacity": 4_294_967_296u64, "active_slots": 123_456u64}),
    );
    node.set(
        "getStateMap",
        json!({
            "log_slots": 32,
            "bucket_capacity": 65_536,
            "live_counts": vec![483u64; 256]
        }),
    );
    node.set(
        "getMiningInfo",
        json!({
            "height": height,
            "difficulty_bits": 16,
            "difficulty_target": format!("{:064x}", 0x0000_ffffu128 << 96),
            "block_reward_micronoid": 3_000_000_000u64,
            "matrix_cache_classes": ["b25", "b255"]
        }),
    );
    node.set(
        "getNodeStatus",
        json!({"testnet": true, "synced": true, "sync_stage": "tip", "backend": "full"}),
    );
    node.set("getPeerCount", json!(3));
    node.set(
        "getMempoolStats",
        json!({"size": 12, "capacity": 4096, "intent_bytes": 8192, "max_intent_bytes": 1_048_576}),
    );
    node.set(
        "walletListAddresses",
        json!([{"address": alice.clone(), "key_index": 0, "is_active": true}]),
    );
    node.set(
        "walletActiveAddress",
        json!({"address": alice.clone(), "key_index": 0, "is_active": true}),
    );
    node.set(
        "walletGetBalance",
        json!({
            "balance_micronoid": balance_micronoid,
            "balance_noid": (balance_micronoid / NOID) as f64,
            "utxo_count": utxo_count,
            "pending_outbound_micronoid": 0,
            "pending_incoming_micronoid": 0,
            "spendable_micronoid": balance_micronoid,
            "spendable_noid": (balance_micronoid / NOID) as f64
        }),
    );
    let value_each = if utxo_count > 0 {
        balance_micronoid / utxo_count as u64
    } else {
        0
    };
    let utxos: Vec<Value> = (0..utxo_count)
        .map(|index| {
            json!({
                "slot_index": 1000 + index as u32,
                "value_micronoid": value_each,
                "creation_id": height * 100 + index as u64,
                "value_noid": (value_each / NOID) as f64,
                "address": alice.clone(),
                "key_index": 0,
                "confirmed_height": height - 10,
                "reserved": false
            })
        })
        .collect();
    node.set_fn(
        "walletUtxoSnapshot",
        move |_| json!({"revision": format!("rev-{height}-{utxo_count}"), "utxos": utxos.clone()}),
    );
    node.set_fn("getBlockHeader", move |params| {
        let height = params.first().and_then(Value::as_u64).unwrap_or(height);
        header_at(height)
    });
    node.set(
        "walletReceipts",
        json!({
            "page": 1, "page_size": 7, "filter": "all", "total": 0, "total_pages": 1, "receipts": []
        }),
    );
    node.set(
        "getRecentTransactions",
        json!({
            "page": 1, "page_size": 8, "total": 0, "total_pages": 1,
            "tip_height": height, "retained_from_height": height.saturating_sub(18),
            "transactions": []
        }),
    );
}

// ----------------------------------------------------------------------
// Scenario runner
// ----------------------------------------------------------------------

struct Report {
    failures: Vec<String>,
    passed: usize,
}

impl Report {
    fn check(&mut self, scenario: &str, claim: &str, condition: bool, detail: String) {
        if condition {
            self.passed += 1;
            println!("PASS [{scenario}] {claim}");
        } else {
            self.failures
                .push(format!("[{scenario}] {claim}: {detail}"));
            println!("FAIL [{scenario}] {claim}: {detail}");
        }
    }
}

#[tokio::test]
async fn malicious_node_deceives_real_gui_backend() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let impostor = Impostor::new();
    tokio::spawn(serve(listener, Arc::clone(&impostor)));

    let data_dir = tempfile::tempdir().expect("temp data dir");
    let data_dir_path = data_dir.path().join("gui-data");
    std::fs::create_dir_all(&data_dir_path).expect("create gui data dir");
    std::env::set_var("NOID_RPC", format!("http://127.0.0.1:{port}"));
    std::env::set_var("NOID_GUI_DATA_DIR", &data_dir_path);
    std::env::set_var(
        "NOID_GUI_SETTINGS_FILE",
        data_dir.path().join("absent-settings.json"),
    );
    std::env::remove_var("NOID_GUI_MOCK");

    let alice = trusted_peer();
    let lookalike = lookalike_peer();
    let mut report = Report {
        failures: Vec::new(),
        passed: 0,
    };

    // The real production backend, constructed exactly as main.rs constructs
    // it (noid_gui/src/main.rs -> Backend::from_env()).
    let backend = Backend::from_env();

    // ------------------------------------------------------------------
    // S0 control: honest-shaped node answers flow through unchanged.
    // ------------------------------------------------------------------
    script_baseline(&impostor, 4_321, 2 * NOID, 2);
    let snapshot = backend.snapshot().await.expect("control snapshot");
    let methods_before = impostor.distinct_methods().len();
    report.check(
        "S0-control",
        "real backend consumes impostor wire format and displays node answers verbatim",
        snapshot.snapshot.addresses[0].balance_micronoid == 2 * NOID
            && snapshot.snapshot.network.height == 4_321
            && snapshot.snapshot.utxos.len() == 2
            && methods_before >= 11,
        format!(
            "balance={} height={} utxos={} methods={}",
            snapshot.snapshot.addresses[0].balance_micronoid,
            snapshot.snapshot.network.height,
            snapshot.snapshot.utxos.len(),
            methods_before
        ),
    );
    let baseline_methods = impostor.distinct_methods();
    println!(
        "INFO [S0-control] snapshot() consumed {} distinct RPC methods: {:?}",
        baseline_methods.len(),
        baseline_methods
    );

    // ------------------------------------------------------------------
    // S1 phantom incoming payment: user believes they were paid.
    // backend.rs:978-982 maps balance.pending_incoming_micronoid to the
    // address card; present.rs:528-530 renders it as "INCOMING".
    // ------------------------------------------------------------------
    script_baseline(&impostor, 4_322, 12_500_000_000, 1);
    impostor.set(
        "walletGetBalance",
        json!({
            "balance_micronoid": 12_500_000_000u64,
            "balance_noid": 12.5,
            "utxo_count": 1,
            "pending_outbound_micronoid": 0,
            "pending_incoming_micronoid": 12_500_000_000u64,
            "spendable_micronoid": 12_500_000_000u64,
            "spendable_noid": 12.5
        }),
    );
    // Explorer address lookup "corroborates" the phantom payment:
    // backend.rs:1174-1210 chains validateAddress -> getSlotsByOwner ->
    // getRecentTransactions, all answered by the impostor.
    let phantom_txid = format!("{:064x}", 0xdead_beefu64);
    impostor.set(
        "validateAddress",
        json!({"valid": true, "bech32": alice.clone(), "error": null}),
    );
    impostor.set("getSlotsByOwner", json!([
        {"slot_index": 777, "value": 12_500_000_000u64, "creation_id": 99, "owner": alice.clone(), "empty": false}
    ]));
    impostor.set(
        "getRecentTransactions",
        json!({
            "page": 1, "page_size": 8, "total": 1, "total_pages": 1,
            "tip_height": 4_322, "retained_from_height": 4_304,
            "transactions": [{
                "height": 4_321, "timestamp": 1_760_000_000u64, "position": 3,
                "txid": phantom_txid, "live_inputs": 1, "live_outputs": 2,
                "fee_micronoid": 5_800, "coinbase": false,
                "address_spent_micronoid": null,
                "address_received_micronoid": "12500000000"
            }]
        }),
    );
    let snapshot = backend.snapshot().await.expect("phantom snapshot");
    let incoming = snapshot.snapshot.addresses[0].incoming_micronoid;
    report.check(
        "S1-phantom-payment",
        "GUI shows INCOMING 12.500000 NOID that never existed",
        incoming == 12_500_000_000,
        format!("incoming displayed as {}", format_micronoid(incoming)),
    );
    let spendable = snapshot.snapshot.addresses[0].balance_micronoid
        - snapshot.snapshot.addresses[0].pending_outbound_micronoid;
    report.check(
        "S1-phantom-payment",
        "phantom funds are spendable-candidate (send form FROM line, present.rs:1853-1855/1974-1980)",
        spendable == 12_500_000_000,
        format!("send form would show spendable {}", format_micronoid(spendable)),
    );
    let lookup = backend.explorer_search(alice.clone(), 1).await;
    let corroborated = matches!(
        &lookup,
        Ok(ExplorerLookup::Result(
            noid_gui::model::ExplorerSearchResultSnapshot::Address(address)
        )) if address.balance_micronoid == 12_500_000_000
            && address.recent_transactions.transactions.len() == 1
            && address.recent_transactions.transactions[0].address_received_micronoid
                .as_deref() == Some("12500000000")
    );
    report.check(
        "S1-phantom-payment",
        "explorer 'corroboration' of the phantom payment is served by the same impostor",
        corroborated,
        format!("explorer_search returned {:?}", lookup.is_ok()),
    );
    // The fabricated txid "verifies" as included via getTx (backend.rs:1247).
    impostor.set("getTx", json!({"height": 4_321, "tx_position": 3}));
    let retained = json!({
        "position": 3, "txid": phantom_txid, "page_count": 1,
        "live_inputs": 1, "live_outputs": 2, "fee_micronoid": 5_800,
        "coinbase": false, "epoch_anchor": format!("{:064x}", 7u64),
        "input_sum_micronoid": "13000000000", "output_sum_micronoid": "12999994200",
        "page_hashes": [format!("{:064x}", 8u64)],
        "inputs": [{"owner": "o1sender", "page": 0, "lane": 0, "slot_index": 555,
                    "amount_micronoid": 13_000_000_000u64, "creation_id": 98}],
        "outputs": [
            {"page": 0, "lane": 0, "slot_index": 777, "amount_micronoid": 12_500_000_000u64,
             "owner": alice.clone(), "creation_id": 99},
            {"page": 0, "lane": 0, "slot_index": 556, "amount_micronoid": 499_942_000u64,
             "owner": "o1sender", "creation_id": 99}
        ]
    });
    impostor.set_fn("getBlockDetails", move |params| {
        let height = params.first().and_then(Value::as_u64).unwrap_or(4_321);
        json!({"header": header_at(height), "retained": {
            "proof_class": "b25", "logical_transactions": 4, "user_pages": 2,
            "live_inputs": 5, "live_outputs": 6, "reward_micronoid": 3_000_000_000u64,
            "reward_noid": 3.0, "total_fees_micronoid": "11600", "block_bytes": 9000,
            "history_step_bytes": 4000, "bundle_bytes": 5000,
            "transactions": [retained.clone()]
        }})
    });
    let tx_lookup = backend.explorer_search(phantom_txid.clone(), 1).await;
    let included = matches!(
        &tx_lookup,
        Ok(ExplorerLookup::Transaction { position: 3, .. })
    );
    report.check(
        "S1-phantom-payment",
        "fabricated txid resolves as an included block transaction (explorer detail)",
        included,
        format!("tx search returned ok={}", tx_lookup.is_ok()),
    );

    // ------------------------------------------------------------------
    // S2 phantom AUTHENTICATED receipt: the verify button asks the attacker.
    // backend.rs:1310-1317 walletExportReceipt -> backend.rs:1328-1331
    // verifyReceipt, both answered by the impostor; proofs.rs:680 renders
    // "AUTHENTICATED PAYMENT / CONFIRMED".
    // ------------------------------------------------------------------
    let receipt_txid = format!("{:064x}", 0xc0ffeeu64);
    impostor.set(
        "walletReceipts",
        json!({
            "page": 1, "page_size": 7, "filter": "all", "total": 1, "total_pages": 1,
            "receipts": [{
                "txid": receipt_txid, "height": 4_320, "timestamp": 1_759_999_000u64,
                "amount_micronoid": 5 * NOID, "fee_micronoid": 5_800,
                "peer_address": lookalike.clone(), "own_address": alice.clone(), "own_key_index": 0,
                "input_count": 1, "output_count": 2, "receipt_bytes": 410
            }]
        }),
    );
    let receipts = backend
        .receipts_snapshot(1, ReceiptFilter::All)
        .await
        .expect("receipts page");
    report.check(
        "S2-phantom-receipt",
        "receipts page shows attacker-authored payment row (proofs.rs receipt_row)",
        receipts.receipts.len() == 1
            && receipts.receipts[0].amount_micronoid == 5 * NOID
            && receipts.receipts[0].peer_address.as_deref() == Some(lookalike.as_str()),
        format!("{} receipt rows", receipts.receipts.len()),
    );
    let fake_receipt_hex = "deadbeef".repeat(64); // 256 bytes of nothing
    impostor.set("walletExportReceipt", json!(fake_receipt_hex));
    impostor.set(
        "verifyReceipt",
        json!({
            "merkle_valid": true, "canonical": true, "confirmed": true, "error": null,
            "authenticated_summary": {
                "txid": receipt_txid, "claimed_height": 4_320,
                "confirmed_unix": 1_759_999_000u64, "tx_index": 2, "tx_count": 5,
                "fee_micronoid": 5_800,
                "inputs": [{"slot_index": 431, "owner": alice.clone()}],
                "outputs": [
                    {"slot_index": 777, "amount_micronoid": 5 * NOID, "owner": lookalike.clone()},
                    {"slot_index": 432, "amount_micronoid": 999 * NOID, "owner": "o1attacker"}
                ]
            }
        }),
    );
    let detail = backend
        .receipt_detail(receipt_txid.clone())
        .await
        .expect("receipt detail");
    let summary = detail.verification.authenticated_summary.expect("summary");
    report.check(
        "S2-phantom-receipt",
        "garbage bytes verify as AUTHENTICATED + CONFIRMED with attacker-chosen outputs",
        detail.verification.merkle_valid
            && detail.verification.canonical
            && detail.verification.confirmed
            && summary.outputs.len() == 2
            && summary.outputs[1].amount_micronoid == 999 * NOID,
        format!(
            "merkle_valid={} canonical={} confirmed={} outputs={}",
            detail.verification.merkle_valid,
            detail.verification.canonical,
            detail.verification.confirmed,
            summary.outputs.len()
        ),
    );

    // ------------------------------------------------------------------
    // S3 balance inflation drives a send; node returns fabricated txid/fee,
    // GUI success screen shows the REQUEST (backend.rs:820-827,
    // present.rs:1994-2046). Fee steering folded in: automatic fee mode,
    // node "charged" 2 NOID on a 5 NOID payment and the client accepts it.
    // ------------------------------------------------------------------
    script_baseline(&impostor, 4_323, 500 * NOID, 3);
    let fabricated_txid = format!("{:064x}", 0x15bad_beefu64);
    let fabricated_txid_echo = fabricated_txid.clone();
    let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let seen = Arc::clone(&calls);
    impostor.set_fn("walletSend", move |params| {
        seen.lock()
            .unwrap()
            .push(("walletSend".into(), json!(params)));
        json!({
            "txid": fabricated_txid_echo,
            "amount_micronoid": 5 * NOID,
            "fee_micronoid": 2 * NOID,
            "input_count": 1,
            "output_count": 2
        })
    });
    let validate_before = impostor.call_count("validateAddress");
    let submission = backend
        .send_payment(alice.clone(), 5 * NOID, 0)
        .await
        .expect("send against impostor");
    report.check(
        "S3-send-blindness",
        "send 'succeeds' against impostor; GUI success shows request recipient + fabricated txid",
        submission.recipient == alice && submission.txid == fabricated_txid,
        format!(
            "recipient={} txid={} (request was to {} for {} NOID)",
            submission.recipient, submission.txid, alice, 5
        ),
    );
    report.check(
        "S3-send-blindness",
        "node-chosen fee (2 NOID on a 5 NOID payment) flows to the success screen uncapped",
        submission.fee_micronoid == 2 * NOID,
        format!(
            "fee displayed as {} NOID on the success screen (present.rs send_result_metric)",
            submission.fee_micronoid / NOID
        ),
    );
    let logged = calls.lock().unwrap().clone();
    let validate_after = impostor.call_count("validateAddress");
    report.check(
        "S3-send-blindness",
        "impostor received the plaintext payment request (recipient, amount, auto-fee)",
        logged.len() == 1
            && logged[0].1.as_array().map(|params| params.len()) == Some(3)
            && validate_after == validate_before,
        format!(
            "walletSend params={:?}; validateAddress calls during send={}",
            logged.get(0).map(|(_, value)| value.clone()),
            validate_after - validate_before
        ),
    );

    // ------------------------------------------------------------------
    // S4 suppression: healthy chain, zero funds. User believes funds
    // vanished or a payment never arrived; no independent source exists.
    // ------------------------------------------------------------------
    script_baseline(&impostor, 4_324, 0, 0);
    impostor.set(
        "getRecentTransactions",
        json!({
            "page": 1, "page_size": 8, "total": 0, "total_pages": 1,
            "tip_height": 4_324, "retained_from_height": 4_306, "transactions": []
        }),
    );
    let snapshot = backend.snapshot().await.expect("suppressed snapshot");
    let address = &snapshot.snapshot.addresses[0];
    report.check(
        "S4-suppression",
        "healthy chain + zero balance renders as an empty-but-successful wallet (no alarm)",
        snapshot.snapshot.network.height == 4_324
            && address.balance_micronoid == 0
            && snapshot.snapshot.utxos.is_empty()
            && address.incoming_micronoid == 0,
        format!(
            "height={} balance={} utxos={} incoming={}",
            snapshot.snapshot.network.height,
            address.balance_micronoid,
            snapshot.snapshot.utxos.len(),
            address.incoming_micronoid
        ),
    );

    // ------------------------------------------------------------------
    // S5 chain-status spoofing: synced/terminal-verified indicators are
    // node-asserted (backend.rs:1081-1086 hardcodes terminal_verified=true).
    // ------------------------------------------------------------------
    script_baseline(&impostor, 999_999, 1 * NOID, 1);
    impostor.set(
        "getNodeStatus",
        json!({"testnet": true, "synced": true, "sync_stage": "tip", "backend": "full"}),
    );
    let fake_root = format!("{:064x}", 0xbaadf00du64);
    let fake_root_echo = fake_root.clone();
    impostor.set_fn("getBlockHeader", move |params| {
        let height = params.first().and_then(Value::as_u64).unwrap_or(999_999);
        let mut header = header_at(height);
        header["state_root"] = json!(fake_root_echo);
        header
    });
    let snapshot = backend.snapshot().await.expect("spoofed snapshot");
    report.check(
        "S5-status-spoof",
        "GUI reports SYNCED + TERMINAL VERIFIED + attacker state root at a fabricated tip",
        snapshot.snapshot.network.synced
            && snapshot.snapshot.network.terminal_verified
            && snapshot.snapshot.network.height == 999_999
            && snapshot.snapshot.network.state_root == fake_root,
        format!(
            "synced={} terminal_verified={} height={} state_root={}",
            snapshot.snapshot.network.synced,
            snapshot.snapshot.network.terminal_verified,
            snapshot.snapshot.network.height,
            snapshot.snapshot.network.state_root
        ),
    );

    // ------------------------------------------------------------------
    // S6 impostor adoption: the GUI accepts whoever answers on the RPC port.
    // (a) pre-bound impostor: backend.rs:728 returns Ok without spawning.
    // (b) the GUI's own node binary dies; the impostor answers the next
    //     ping and wait_until_ready (backend.rs:1476-1499) returns Ok
    //     because the ping check precedes the child-exit check.
    // The impostor answers blockCount (the GUI's ping, backend.rs:1334-1337),
    // so the GUI considers its node already running and never spawns one.
    impostor.set("blockCount", json!(4_321));
    let ensure = backend.ensure_running().await;
    let block_count_calls = impostor.call_count("blockCount");
    report.check(
        "S6-impostor-adoption",
        "(a) pre-bound impostor satisfies ensure_running with no node spawned",
        ensure.is_ok() && block_count_calls >= 1,
        format!("ensure_running={ensure:?} blockCount={block_count_calls}"),
    );
    // (b) fresh backend whose node binary passes --check-hardware then dies.
    let marker = data_dir.path().join("spawn-marker");
    let fake_node = data_dir.path().join("fake-node.sh");
    std::fs::write(
        &fake_node,
        format!("#!/bin/sh\nif [ \"$1\" = \"--check-hardware\" ]; then exit 0; fi\necho spawned >> \"{}\"\nexit 7\n", marker.display()),
    )
    .expect("write fake node");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_node, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake node");
    }
    std::env::set_var("NOID_GUI_NODE_BIN", &fake_node);
    std::env::set_var("NOID_GUI_DATA_DIR", data_dir.path().join("gui-data-b"));
    let backend_b = Backend::from_env();
    // blockCount fails exactly once, then answers forever.
    let block_count_before_b = impostor.call_count("blockCount");
    impostor.fail_first("blockCount", 1, -32603, "temporarily unavailable");
    let ensure_b = backend_b.ensure_running().await;
    let block_count_after_b = impostor.call_count("blockCount");
    // The child is async: poll briefly for its marker (fork+exec of /bin/sh
    // is slower than the impostor's loopback round trip).
    let mut spawned = false;
    for _ in 0..40 {
        if marker.exists() {
            spawned = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let node_log_exists = data_dir
        .path()
        .join("gui-data-b/parano1d-node.log")
        .exists();
    println!(
        "INFO [S6-impostor-adoption] phase (b) probes: blockCount calls={} marker={} node-log={}",
        block_count_after_b - block_count_before_b,
        spawned,
        node_log_exists
    );
    let snapshot_b = backend_b.snapshot().await;
    report.check(
        "S6-impostor-adoption",
        "(b) GUI spawns its node, node dies (marker + exit), GUI still goes ONLINE via impostor",
        ensure_b.is_ok() && spawned && snapshot_b.is_ok(),
        format!(
            "ensure_running={ensure_b:?} spawned={spawned} snapshot={}",
            snapshot_b.is_ok()
        ),
    );

    // ------------------------------------------------------------------
    // S7 address poisoning: receipt rows truncate peer addresses to
    // first 13 + last 7 chars (proofs.rs:1014-1018). The impostor chooses
    // the peer string, so it renders as any lookalike it likes.
    // ------------------------------------------------------------------
    fn short_address(value: &str) -> String {
        // verbatim re-implementation of noid_gui/src/view/proofs.rs:1014-1019
        if value.len() <= 24 {
            return value.to_owned();
        }
        format!("{}...{}", &value[..13], &value[value.len() - 7..])
    }
    report.check(
        "S7-address-poisoning",
        "lookalike counterparty renders identically to the trusted one in receipt rows",
        lookalike.len() > 24
            && alice.len() > 24
            && short_address(&lookalike) == short_address(&alice),
        format!("{} vs {}", short_address(&lookalike), short_address(&alice)),
    );

    // ------------------------------------------------------------------
    // (Locks are taken in separate statements: a MutexGuard temporary in a
    // println! argument list lives to the end of the statement, and locking
    // the same mutex twice within one statement self-deadlocks.)
    let total_requests = impostor.requests.lock().unwrap().len();
    let distinct = impostor.distinct_methods();
    println!(
        "INFO impostor served {} requests across {} distinct methods: {:?}",
        total_requests,
        distinct.len(),
        distinct
    );
    assert!(
        report.failures.is_empty(),
        "{} of {} checks failed:\n{}",
        report.failures.len(),
        report.failures.len() + report.passed,
        report.failures.join("\n")
    );
    println!("ALL {} CHECKS PASSED", report.passed);
}
