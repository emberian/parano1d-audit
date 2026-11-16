//! Native-consensus local-chain reproduction of a wallet reservation surviving
//! mempool eviction. The HistoryStep terminal verifier is explicitly stubbed;
//! all other block, PoW, state, and MDBX acceptance checks are production code.

#[path = "../../../comp-system-hunter/pinned/noid_node/src/wallet/mod.rs"]
mod wallet;

use std::sync::{Arc, Mutex};

use anyhow::{ensure, Context, Result};
use noid_chain::consensus::params::BLOCK_TIME;
use noid_chain::consensus::template::build_node_owned_block_template;
use noid_chain::storage::MdbxChainContext;
use noid_chain::{AcceptedBlockBundle, Block, HistoryStepTerminalMetadata};
use noid_mempool::{AsyncMempool, ChainView, EvictReason, MempoolConfig, MempoolEvent};
use noid_poseidon2b::primitives::Address;
use noid_tx::PagedSpendIntent;

fn mine_empty(ctx: &MdbxChainContext, miner: Address) -> Result<(AcceptedBlockBundle, Block)> {
    let parent = *ctx.tip_header();
    let height = parent.height + 1;
    let timestamp = parent.timestamp + BLOCK_TIME;
    let anchor = ctx.anchor_info()?;
    let target = noid_chain::consensus::next_target(
        anchor.anchor_height, anchor.anchor_timestamp, &anchor.anchor_target,
        height, parent.timestamp, timestamp,
    );
    let counts = ctx.finalized_active_counts()?;
    let (template, _) = build_node_owned_block_template(
        &parent, &ctx.state, &counts, Vec::new(), miner, timestamp, target,
    ).map_err(|e| anyhow::anyhow!("template: {e:?}"))?;
    let mut nonce = 0u128;
    let block = loop {
        let candidate = template.clone().into_block(nonce);
        if noid_chain::consensus::validate_pow(&candidate.header).is_ok() {
            break candidate;
        }
        nonce += 1;
    };
    let mut terminal = HistoryStepTerminalMetadata::new(
        height, noid_chain::block_header::semantic_header_id(&block.header), 0,
    ).map_err(|e| anyhow::anyhow!("terminal: {e:?}"))?.encode_prefix().to_vec();
    terminal.push(0xA5);
    let bundle = AcceptedBlockBundle::try_from_parts(block.to_bytes(), terminal)?;
    Ok((bundle, block))
}

fn accept(ctx: &mut MdbxChainContext, bundle: &AcceptedBlockBundle, block: &Block) -> Result<()> {
    ctx.apply_next_block(bundle, block.header.timestamp,
        |b, state| noid_chain::materialize_accepted_block_state(state, b)
            .map_err(|e| format!("{e:?}")),
        |_| Ok(()), // Deliberately skip cryptographic HistoryStep verification.
    )?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut ctx = MdbxChainContext::open_or_create(&temp.path().join("chain"))?;
    let wallet_path = temp.path().join("wallet.key");
    let wallet_state = wallet::WalletState::create_or_load(wallet_path.clone())?;
    let owner = wallet_state.address_at(0);
    let shared: wallet::SharedWallet = Arc::new(Mutex::new(Some(wallet_state)));
    let handle = wallet::WalletHandle::new(shared.clone());

    let (b1, block1) = mine_empty(&ctx, owner)?;
    accept(&mut ctx, &b1, &block1)?;
    wallet::update_for_accepted_block(&shared, &block1).map_err(anyhow::Error::msg)?;
    ensure!(ctx.tip_height() == 1 && !handle.list_utxos().is_empty());
    println!("local chain: PoW-valid coinbase block 1 accepted to MDBX; wallet funded");

    // Precompute a valid empty competing miner block. Its coinbase output is a
    // deterministic slot that the wallet can choose while the block is unseen.
    let (b2, block2) = mine_empty(&ctx, Address([0x77; 32]))?;
    let coinbase_slot = block2.transactions[0].body.live_outputs()
        .next().context("coinbase output")?.1.slot_index;
    let second_slot = (0..1024u32).find(|&s|
        s != coinbase_slot && ctx.state.state.slot(s).is_empty()
    ).context("second empty hint")?;

    let view = ChainView::from_mdbx(&ctx);
    let pool = AsyncMempool::new(view.clone(), MempoolConfig::default());
    let (bytes, inputs) = handle.build_send(
        Address([0x51; 32]).0, 50_000, 9_000, view.user_epoch_anchor_id,
        vec![coinbase_slot, second_slot], view.log_slots(), 2,
    ).map_err(anyhow::Error::msg)?;
    let intent = PagedSpendIntent::from_bytes(&bytes)?;
    let txid = intent.logical_txid();
    let outputs: Vec<u32> = intent.pages.iter().flat_map(|p| p.body.live_outputs())
        .map(|(_, out)| out.slot_index).collect();
    ensure!(outputs.contains(&coinbase_slot));
    handle.reserve_pending_submission(txid.0, &inputs, &outputs,
        50_000, Address([0x51; 32]).0).map_err(anyhow::Error::msg)?;
    ensure!(pool.submit_encoded(bytes).await? == txid);
    ensure!(pool.len().await == 1 && handle.get_balance().spendable_micronoid == 0);
    println!("wallet send: real proof admitted before competing coinbase block");

    let mut events = pool.subscribe();
    accept(&mut ctx, &b2, &block2)?;
    wallet::update_for_accepted_block(&shared, &block2).map_err(anyhow::Error::msg)?;
    pool.on_new_block(&[], 2, ChainView::from_mdbx(&ctx)).await;
    ensure!(ctx.tip_height() == 2 && pool.len().await == 0);
    ensure!(matches!(events.try_recv()?, MempoolEvent::TxEvicted {
        hash, reason: EvictReason::OutputSlotOccupied,
    } if hash == txid));
    ensure!(handle.get_balance().spendable_micronoid == 0);
    ensure!(handle.list_utxos().iter().any(|u| u.reserved));
    ensure!(handle.preview_address_discovery(1).is_err());
    println!("accepted block 2: mempool=0; wallet spendable=0; input reserved; discovery blocked");

    let restart = wallet::WalletState::create_or_load(wallet_path)?;
    ensure!(restart.pending_input_slots.is_empty());
    ensure!(restart.history.iter().any(|h| h.tx_hash == txid.0 && h.height == 0));
    println!("restart: lock clears; unconfirmed send history remains");
    Ok(())
}
