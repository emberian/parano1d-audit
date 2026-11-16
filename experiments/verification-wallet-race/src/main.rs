//! SR-2 wallet ordering and SR-3 discovery/reorg interleaving against pinned modules.
//! The reorg snapshots below are controlled inputs to the production wallet function;
//! this executable does not invoke the MDBX reorg committer. The SR-3 interleaving
//! models the gap after discovery drops its chain read lock and before its commit.

#[path = "../../comp-system-hunter/pinned/noid_node/src/wallet/mod.rs"]
mod wallet;

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, Mutex};

use anyhow::{anyhow, ensure, Result};
use noid_chain::fri_state::SlotValue;
use noid_chain::state::ChainState;
use noid_chain::storage::{VerifiedOwnerSnapshot, VerifiedOwnerUtxo};
use noid_mempool::{AsyncMempool, ChainView, EvictReason, MempoolConfig, MempoolEvent};
use noid_poseidon2b::primitives::{Address, TxBodyHash};
use noid_tx::{experimental_object::applications, PagedSpendIntent};

fn owner_snapshot(
    owner: [u8; 32],
    slot: u32,
    value: u64,
    creation_id: u64,
) -> VerifiedOwnerSnapshot {
    VerifiedOwnerSnapshot {
        owner,
        height: 1,
        tip_hash: [0x44; 32],
        state_root: [0x55; 32],
        log_slots: 24,
        active_slot_count: 1,
        alloc_counter: creation_id + 1,
        utxos: vec![VerifiedOwnerUtxo {
            slot_index: slot,
            amount: value,
            creation_id,
        }],
    }
}

fn fresh_wallet(
    path: std::path::PathBuf,
    slot: u32,
    value: u64,
    creation_id: u64,
) -> Result<(
    wallet::SharedWallet,
    Arc<dyn noid_rpc::WalletOps + Send + Sync>,
    Address,
)> {
    let mut state = wallet::WalletState::create_or_load(path)?;
    let owner = state.address_at(0);
    state.utxos.insert(
        slot,
        wallet::state::WalletUtxo {
            slot_index: slot,
            value,
            creation_id,
            address: owner,
            key_index: 0,
            confirmed_height: 1,
        },
    );
    let shared = Arc::new(Mutex::new(Some(state)));
    let handle = wallet::WalletHandle::new(shared.clone());
    Ok((shared, handle, owner))
}

async fn reservation_ordering() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let input_slot = 3u32;
    let input_value = 100_000u64;
    let creation_id = 42u64;
    let wallet_path = temp.path().join("wallet.key");
    let (shared, handle, owner) =
        fresh_wallet(wallet_path.clone(), input_slot, input_value, creation_id)?;
    let genesis = noid_chain::consensus::genesis_header();
    let headers = HashMap::from([(0, genesis)]);
    let mut chain_state = ChainState::with_log_slots(24);
    chain_state
        .state
        .apply_delta(&[(
            input_slot,
            SlotValue::with_owner_fields(input_value, creation_id, owner.as_fields()),
        )])
        .map_err(|e| anyhow!("{e:?}"))?;
    let view = ChainView::new(0, headers.clone(), 1, chain_state.state.clone());
    let pool = AsyncMempool::new(view.clone(), MempoolConfig::default());
    let (bytes, inputs) = handle
        .build_send(
            Address([0x51; 32]).0,
            50_000,
            9_000,
            view.user_epoch_anchor_id,
            vec![100, 101],
            24,
            1,
        )
        .map_err(anyhow::Error::msg)?;
    let intent = PagedSpendIntent::from_bytes(&bytes)?;
    let txid = intent.logical_txid();
    let outputs: Vec<_> = intent
        .pages
        .iter()
        .flat_map(|p| p.body.live_outputs())
        .map(|(_, output)| output.slot_index)
        .collect();
    handle
        .reserve_pending_submission(txid.0, &inputs, &outputs, 50_000, Address([0x51; 32]).0)
        .map_err(anyhow::Error::msg)?;
    ensure!(pool.submit_encoded(bytes).await? == txid);
    ensure!(pool.len().await == 1);
    let (reserved_inputs, reserved_outputs) = pool.reserved_slots().await;
    ensure!(reserved_inputs.contains(&input_slot) && reserved_outputs.contains(&outputs[0]));

    // This is the production order: capture mempool reservations, install the
    // replacement owner snapshot, and only then revalidate the mempool.
    wallet::install_reorg_snapshot_and_artifacts(
        &shared,
        0,
        1,
        owner.0,
        owner_snapshot(owner.0, input_slot, input_value, creation_id),
        &reserved_inputs,
        &reserved_outputs,
        &[],
        &[],
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(handle
        .list_utxos()
        .iter()
        .any(|u| u.slot_index == input_slot && u.reserved));
    chain_state
        .state
        .apply_delta(&[(
            outputs[0],
            SlotValue::with_owner_fields(1, 999, Address([0x77; 32]).as_fields()),
        )])
        .map_err(|e| anyhow!("{e:?}"))?;
    let changed_view = ChainView::new(1, headers, 2, chain_state.state);
    let mut events = pool.subscribe();
    pool.on_new_block(&[], 1, changed_view).await;
    pool.readmit_after_reorg(vec![]).await;
    ensure!(pool.len().await == 0);
    ensure!(matches!(events.try_recv()?, MempoolEvent::TxEvicted {
        hash, reason: EvictReason::OutputSlotOccupied
    } if hash == txid));
    let (after_inputs, after_outputs) = pool.reserved_slots().await;
    ensure!(after_inputs.is_empty() && after_outputs.is_empty());
    ensure!(handle.get_balance().spendable_micronoid == 0);
    ensure!(handle.preview_address_discovery(1).is_err());
    let reopened = wallet::WalletState::create_or_load(wallet_path)?;
    ensure!(reopened.pending_input_slots.is_empty() && reopened.pending_output_slots.is_empty());
    println!("SR-2: mempool evicted real wallet-proof transaction; prior snapshot reservation stayed in wallet (spendable=0)");
    println!("SR-2 restart bound: direct keystore reload has no pending slot reservations");

    // Control: applying the *same* snapshot after eviction, with current
    // empty reservations, restores spendability.
    wallet::install_reorg_snapshot_and_artifacts(
        &shared,
        0,
        1,
        owner.0,
        owner_snapshot(owner.0, input_slot, input_value, creation_id),
        &HashSet::new(),
        &HashSet::new(),
        &[],
        &[],
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(handle.get_balance().spendable_micronoid == input_value);
    ensure!(handle.preview_address_discovery(1).is_ok());
    println!("SR-2 control: snapshot with post-eviction reservations restores spendability");
    Ok(())
}

fn install_after_discovery_race() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("wallet.key");
    let (shared, handle, owner) = fresh_wallet(path.clone(), 3, 100, 42)?;
    let orphan = [0x66; 32];
    {
        let mut guard = shared.lock().unwrap();
        let wallet = guard.as_mut().unwrap();
        wallet.receipts.insert(orphan, vec![1, 2, 3]);
        wallet.save_receipts().map_err(anyhow::Error::msg)?;
        wallet.history.push(wallet::state::TxHistoryEntry {
            tx_hash: orphan,
            block_hash: None,
            height: 9,
            direction: wallet::state::TxDirection::Sent,
            is_coinbase: false,
            amount_micronoid: 7,
            peer_address: None,
            timestamp: 8,
            own_address: Some(owner.to_bech32()),
            own_key_index: Some(0),
        });
        wallet.save_history().map_err(anyhow::Error::msg)?;
    }

    let (selection_tx, selection_rx) = mpsc::channel();
    let (continue_tx, continue_rx) = mpsc::channel();
    let worker_wallet = shared.clone();
    let worker = std::thread::spawn(move || -> Result<String> {
        let selection = {
            let guard = worker_wallet.lock().unwrap();
            let state = guard.as_ref().unwrap();
            (
                state.active_index,
                state.next_index,
                state.active_address().0,
            )
        };
        selection_tx.send(selection)?;
        continue_rx.recv()?;
        let error = wallet::install_reorg_snapshot_and_artifacts(
            &worker_wallet,
            selection.0,
            selection.1,
            selection.2,
            owner_snapshot(selection.2, 3, 100, 42),
            &HashSet::new(),
            &HashSet::new(),
            &[TxBodyHash(orphan)],
            &[],
        )
        .unwrap_err();
        Ok(error)
    });
    let selection = selection_rx.recv()?;
    ensure!(selection == (0, 1, owner.0));
    let preview = handle
        .preview_address_discovery(1)
        .map_err(anyhow::Error::msg)?;
    ensure!(preview.expected_next_index == 1);
    let addresses = handle
        .commit_address_discovery(0, 1, 2)
        .map_err(anyhow::Error::msg)?;
    ensure!(addresses.len() == 2);
    continue_tx.send(())?;
    let error = worker
        .join()
        .map_err(|_| anyhow!("reorg worker panicked"))??;
    ensure!(error.contains("wallet address selection changed"));
    wallet::invalidate_active_cache(&shared); // Caller behavior on error.
    {
        let guard = shared.lock().unwrap();
        let state = guard.as_ref().unwrap();
        ensure!(state.receipts.contains_key(&orphan));
        ensure!(state.history.iter().any(|entry| entry.tx_hash == orphan));
    }
    let reloaded = wallet::WalletState::create_or_load(path)?;
    ensure!(reloaded.receipts.contains_key(&orphan));
    ensure!(reloaded.history.iter().any(|entry| entry.tx_hash == orphan));
    println!("SR-3: forced discovery in the post-read-lock commit gap rejects snapshot; orphan artifacts survive invalidation and restart");

    // No-discovery control on a second wallet.
    let control = tempfile::tempdir()?;
    let (shared, _, owner) = fresh_wallet(control.path().join("wallet.key"), 3, 100, 42)?;
    {
        let mut guard = shared.lock().unwrap();
        let state = guard.as_mut().unwrap();
        state.receipts.insert(orphan, vec![1, 2, 3]);
        state.history.push(wallet::state::TxHistoryEntry {
            tx_hash: orphan,
            block_hash: None,
            height: 9,
            direction: wallet::state::TxDirection::Sent,
            is_coinbase: false,
            amount_micronoid: 7,
            peer_address: None,
            timestamp: 8,
            own_address: Some(owner.to_bech32()),
            own_key_index: Some(0),
        });
    }
    wallet::install_reorg_snapshot_and_artifacts(
        &shared,
        0,
        1,
        owner.0,
        owner_snapshot(owner.0, 3, 100, 42),
        &HashSet::new(),
        &HashSet::new(),
        &[TxBodyHash(orphan)],
        &[],
    )
    .map_err(anyhow::Error::msg)?;
    let guard = shared.lock().unwrap();
    let state = guard.as_ref().unwrap();
    ensure!(!state.receipts.contains_key(&orphan));
    ensure!(!state.history.iter().any(|entry| entry.tx_hash == orphan));
    println!("SR-3 control: same install without discovery prunes orphan artifacts");
    Ok(())
}

fn opening_persistence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (_, handle, _) = fresh_wallet(temp.path().join("wallet.key"), 3, 100, 42)?;
    let opening = applications::timelocked_vault(Address([7; 32]), 20, 3);
    let path = temp
        .path()
        .join("objects")
        .join(format!("{}.opening", hex::encode(opening.root().0)));
    ensure!(!path.exists());
    handle
        .remember_object_opening(&opening)
        .map_err(anyhow::Error::msg)?;
    ensure!(path.is_file());
    ensure!(
        handle
            .load_object_opening(opening.root().0)
            .map_err(anyhow::Error::msg)?
            == opening
    );
    println!("object artifact: remember_object_opening durably wrote opening before any mempool admission");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    reservation_ordering().await?;
    install_after_discovery_race()?;
    opening_persistence()?;
    Ok(())
}
