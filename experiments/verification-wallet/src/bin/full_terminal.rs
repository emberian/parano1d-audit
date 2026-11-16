//! Same wallet/mempool sequence with genuine pinned HistoryStep terminals.

#[path = "../full_history.rs"]
mod full_history;
#[path = "../../../comp-system-hunter/pinned/noid_node/src/wallet/mod.rs"]
mod wallet;

use std::sync::{Arc, Mutex};

use anyhow::{ensure, Context, Result};
use noid_chain::consensus::params::BLOCK_TIME;
use noid_chain::consensus::template::build_node_owned_block_template;
use noid_chain::storage::MdbxChainContext;
use noid_chain::AcceptedBlockBundle;
use noid_mempool::{AsyncMempool, ChainView, EvictReason, MempoolConfig, MempoolEvent};
use noid_poseidon2b::primitives::Address;
use noid_tx::PagedSpendIntent;

#[tokio::main]
async fn main() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut ctx = MdbxChainContext::open_or_create(&temp.path().join("chain"))?;
    let mut inbound = MdbxChainContext::open_or_create(&temp.path().join("inbound-chain"))?;
    let runtime = full_history::load_full_history_protocol_runtime(temp.path().join("origins"))?;
    let ghost = full_history::prepare_v3_ghost_auth()?;

    let wallet_path = temp.path().join("wallet.key");
    let wallet_state = wallet::WalletState::create_or_load(wallet_path.clone())?;
    let owner = wallet_state.address_at(0);
    let shared: wallet::SharedWallet = Arc::new(Mutex::new(Some(wallet_state)));
    let handle = wallet::WalletHandle::new(shared.clone());

    let ts1 = ctx.tip_header().timestamp + BLOCK_TIME;
    let (block1, bundle1) = full_history::produce_and_commit_block(
        &mut ctx, &runtime, &ghost, owner, ts1, 0,
    ).await?;
    accept_on_inbound_chain(&mut inbound, &bundle1, &runtime, &block1)?;
    wallet::update_for_accepted_block(&shared, &block1).map_err(anyhow::Error::msg)?;
    ensure!(ctx.tip_height() == 1 && !handle.list_utxos().is_empty());
    println!("full terminal: proved and independently verifier-accepted funding block 1");

    let parent = *ctx.tip_header();
    let ts2 = parent.timestamp + BLOCK_TIME;
    let anchor = ctx.anchor_info()?;
    let target = noid_chain::consensus::next_target(
        anchor.anchor_height, anchor.anchor_timestamp, &anchor.anchor_target,
        parent.height + 1, parent.timestamp, ts2,
    );
    let counts = ctx.finalized_active_counts()?;
    let (prediction, _) = build_node_owned_block_template(
        &parent, &ctx.state, &counts, Vec::new(), Address([0x77; 32]), ts2, target,
    ).map_err(|e| anyhow::anyhow!("block 2 prediction: {e:?}"))?;
    let conflict_slot = prediction.coinbase.body.live_outputs()
        .next().context("predicted coinbase output")?.1.slot_index;
    let second_slot = (0..1024u32).find(|&s|
        s != conflict_slot && ctx.state.state.slot(s).is_empty()
    ).context("second empty output hint")?;

    let view = ChainView::from_mdbx(&ctx);
    let pool = AsyncMempool::new(view.clone(), MempoolConfig::default());
    let (bytes, inputs) = handle.build_send(
        Address([0x51; 32]).0, 50_000, 9_000, view.user_epoch_anchor_id,
        vec![conflict_slot, second_slot], view.log_slots(), 2,
    ).map_err(anyhow::Error::msg)?;
    let intent = PagedSpendIntent::from_bytes(&bytes)?;
    let txid = intent.logical_txid();
    let outputs: Vec<u32> = intent.pages.iter().flat_map(|p| p.body.live_outputs())
        .map(|(_, out)| out.slot_index).collect();
    ensure!(outputs.contains(&conflict_slot));
    handle.reserve_pending_submission(txid.0, &inputs, &outputs,
        50_000, Address([0x51; 32]).0).map_err(anyhow::Error::msg)?;
    ensure!(pool.submit_encoded(bytes).await? == txid);
    println!("full terminal: wallet proof admitted before unseen competing block");

    let mut events = pool.subscribe();
    let (block2, bundle2) = full_history::produce_and_commit_block(
        &mut ctx, &runtime, &ghost, Address([0x77; 32]), ts2, 1 << 32,
    ).await?;
    accept_on_inbound_chain(&mut inbound, &bundle2, &runtime, &block2)?;
    let actual_slot = block2.transactions[0].body.live_outputs()
        .next().context("actual coinbase output")?.1.slot_index;
    ensure!(actual_slot == conflict_slot,
        "coinbase slot changed between prediction and proof");
    wallet::update_for_accepted_block(&shared, &block2).map_err(anyhow::Error::msg)?;
    pool.on_new_block(&[], 2, ChainView::from_mdbx(&ctx)).await;
    ensure!(ctx.tip_height() == 2 && inbound.tip_height() == 2 && pool.len().await == 0);
    ensure!(matches!(events.try_recv()?, MempoolEvent::TxEvicted {
        hash, reason: EvictReason::OutputSlotOccupied,
    } if hash == txid));
    ensure!(handle.get_balance().spendable_micronoid == 0);
    ensure!(handle.list_utxos().iter().any(|u| u.reserved));
    ensure!(handle.preview_address_discovery(1).is_err());
    println!("full terminal: verifier-accepted block 2 evicted tx; wallet input still reserved");

    let restart = wallet::WalletState::create_or_load(wallet_path)?;
    ensure!(restart.pending_input_slots.is_empty());
    ensure!(restart.history.iter().any(|h| h.tx_hash == txid.0 && h.height == 0));
    println!("full terminal: restart clears reservation; pending history persists");
    Ok(())
}

fn accept_on_inbound_chain(
    inbound: &mut MdbxChainContext,
    bundle: &AcceptedBlockBundle,
    runtime: &noid_miner::HistoryProtocolRuntime,
    block: &noid_chain::Block,
) -> Result<()> {
    inbound.apply_next_block(
        bundle,
        block.header.timestamp,
        |block, state| {
            noid_chain::block::materialize_accepted_block_state(state, block)
                .map_err(|error| format!("{error:?}"))
        },
        |claim| {
            noid_miner::install_inbound_verifier_cpu(|| {
                runtime.verify_terminal(
                    claim.terminal_bytes,
                    &claim.header,
                    &claim.epoch_anchor_header,
                )
            })
            .map_err(|error| format!("HistoryStep verification CPU admission: {error}"))?
        },
    )?;
    ensure!(inbound.tip_height() == block.header.height);
    Ok(())
}
