//! Wallet probe for phase 6: drives the REAL GUI backend
//! (`noid_gui::backend::Backend::from_env()`, exactly as the shipped GUI's
//! main constructs it) against whatever node answers `NOID_RPC`, and prints
//! the model values the view layer renders. No impostor: the answers come
//! from a live parano1d daemon.
//!
//! Usage: NOID_RPC=http://127.0.0.1:PORT ./wallet-probe [receipt-txid]
//!
//! Prints:
//!   - the wallet's chain view (height, tip, peers, mempool, terminal/sync
//!     status) as the GUI renders it
//!   - the active address balance/UTXO counts (send-form spendable)
//!   - up to N receipts; for the first, the receipt_detail flow
//!     (walletExportReceipt -> verifyReceipt) and the exact booleans the GUI
//!     turns into "AUTHENTICATED PAYMENT / CONFIRMED" (view/proofs.rs:680)

use noid_gui::backend::Backend;
use noid_gui::model::ReceiptFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = std::env::var("NOID_RPC").unwrap_or_else(|_| "http://127.0.0.1:9601".into());
    println!("wallet-probe: Backend::from_env() -> {url}");
    let backend = Backend::from_env();

    let snapshot = match backend.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            println!("wallet-probe: snapshot FAILED: {error}");
            std::process::exit(2);
        }
    };
    let network = &snapshot.snapshot.network;
    println!("chain view : height={} tip={}", network.height, network.block_hash);
    println!("             peers={} mempool={}/{} txs", network.peers, network.mempool_transactions, network.mempool_capacity_transactions);
    for (index, address) in snapshot.snapshot.addresses.iter().enumerate() {
        println!(
            "address[{index}] {} balance={} μNOID incoming={} μNOID pending_out={} μNOID",
            address.address,
            address.balance_micronoid,
            address.incoming_micronoid,
            address.pending_outbound_micronoid,
        );
    }
    println!("utxos      : {} rows rendered in the wallet's UTXO table", snapshot.snapshot.utxos.len());

    let receipts = match backend.receipts_snapshot(1, ReceiptFilter::All).await {
        Ok(receipts) => receipts,
        Err(error) => {
            println!("wallet-probe: receipts_snapshot FAILED: {error}");
            return;
        }
    };
    println!("receipts   : {} rows (page 1)", receipts.receipts.len());
    for receipt in receipts.receipts.iter().take(3) {
        println!(
            "  txid={} height={} amount={} μNOID peer={:?}",
            receipt.txid, receipt.height, receipt.amount_micronoid, receipt.peer_address
        );
    }
    if let Some(first) = receipts.receipts.first() {
        let detail = match backend.receipt_detail(first.txid.clone()).await {
            Ok(detail) => detail,
            Err(error) => {
                println!("wallet-probe: receipt_detail FAILED: {error}");
                return;
            }
        };
        let verdict = &detail.verification;
        let authenticated = verdict.merkle_valid && verdict.canonical;
        println!(
            "verify     : merkle_valid={} canonical={} confirmed={} error={:?}",
            verdict.merkle_valid, verdict.canonical, verdict.confirmed, verdict.error
        );
        println!(
            "GUI renders: {}{}",
            if authenticated { "AUTHENTICATED PAYMENT " } else { "NOT AUTHENTICATED " },
            if verdict.confirmed { "+ CONFIRMED" } else { "+ UNCONFIRMED" },
        );
        if let Some(summary) = &verdict.authenticated_summary {
            println!(
                "summary    : txid={} height={} fee={} μNOID outputs={}",
                summary.txid,
                summary.claimed_height,
                summary.fee_micronoid,
                summary.outputs.len(),
            );
        }
    }
}
