//! Bounded wallet/mempool state-machine reproduction on the pinned release source.
//! This uses a real wallet authorization and real mempool verification. The
//! second ChainView is a controlled local state transition, not an accepted block.

#[path = "../../comp-system-hunter/pinned/noid_node/src/wallet/mod.rs"]
mod wallet;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{ensure, Result};
use noid_chain::fri_state::SlotValue;
use noid_chain::state::ChainState;
use noid_mempool::{AsyncMempool, ChainView, EvictReason, MempoolConfig, MempoolEvent};
use noid_poseidon2b::primitives::Address;
use noid_tx::PagedSpendIntent;

#[tokio::main]
async fn main() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let wallet_path = temp.path().join("wallet.key");
    let mut wallet_state = wallet::WalletState::create_or_load(wallet_path.clone())?;
    let owner = wallet_state.address_at(0);
    let input_slot = 3u32;
    let input_value = 100_000u64;
    let creation_id = 42u64;
    wallet_state.utxos.insert(
        input_slot,
        wallet::state::WalletUtxo {
            slot_index: input_slot,
            value: input_value,
            creation_id,
            address: owner,
            key_index: 0,
            confirmed_height: 1,
        },
    );
    let shared: wallet::SharedWallet = Arc::new(Mutex::new(Some(wallet_state)));
    let handle = wallet::WalletHandle::new(shared.clone());

    let genesis = noid_chain::consensus::genesis_header();
    let recent_headers = HashMap::from([(0, genesis)]);
    let mut state = ChainState::with_log_slots(24);
    state.state.apply_delta(&[(input_slot, SlotValue::with_owner_fields(
        input_value, creation_id, owner.as_fields(),
    ))]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let view = ChainView::new(0, recent_headers.clone(), 1, state.state.clone());
    let pool = AsyncMempool::new(view.clone(), MempoolConfig::default());

    let (bytes, inputs) = handle.build_send(
        Address([0x51; 32]).0, 50_000, 9_000, view.user_epoch_anchor_id,
        vec![100, 101], 24, 1,
    ).map_err(anyhow::Error::msg)?;
    let intent = PagedSpendIntent::from_bytes(&bytes)?;
    let txid = intent.logical_txid();
    let output_slots: Vec<u32> = intent.pages.iter().flat_map(|p| p.body.live_outputs())
        .map(|(_, output)| output.slot_index).collect();
    ensure!(output_slots.len() == 2);
    handle.reserve_pending_submission(txid.0, &inputs, &output_slots,
        50_000, Address([0x51; 32]).0).map_err(anyhow::Error::msg)?;

    let admitted = pool.submit_encoded(bytes).await?;
    ensure!(admitted == txid && pool.len().await == 1);
    ensure!(handle.get_balance().spendable_micronoid == 0);
    println!("admitted: real wallet proof accepted; mempool=1; spendable=0");

    let mut events = pool.subscribe();
    pool.on_new_block(&[], 0, view).await;
    ensure!(pool.len().await == 1);
    ensure!(events.try_recv().is_err());
    println!("control: unchanged view retains transaction and reservation");

    state.state.apply_delta(&[(output_slots[0], SlotValue::with_owner_fields(
        1, 999, Address([0x77; 32]).as_fields(),
    ))]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let occupied_view = ChainView::new(1, recent_headers, 2, state.state);
    pool.on_new_block(&[], 1, occupied_view).await;
    ensure!(pool.len().await == 0);
    let event = events.try_recv()?;
    ensure!(matches!(event, MempoolEvent::TxEvicted { hash, reason: EvictReason::OutputSlotOccupied }
        if hash == txid));
    ensure!(handle.get_balance().spendable_micronoid == 0);
    ensure!(handle.list_utxos().iter().any(|u| u.slot_index == input_slot && u.reserved));
    ensure!(handle.preview_address_discovery(1).is_err());
    println!("evicted: mempool=0; wallet spendable=0; input remains reserved; address discovery blocked");

    let restarted = wallet::WalletState::create_or_load(wallet_path)?;
    ensure!(restarted.pending_input_slots.is_empty());
    ensure!(restarted.pending_output_slots.is_empty());
    ensure!(restarted.history.iter().any(|h| h.tx_hash == txid.0 && h.height == 0));
    println!("restart: in-memory reservation clears; unconfirmed send history persists");

    handle.rollback_pending_submission(txid.0, &inputs, &output_slots);
    ensure!(handle.get_balance().spendable_micronoid == input_value);
    println!("manual rollback control: wallet spendable restored");
    Ok(())
}
