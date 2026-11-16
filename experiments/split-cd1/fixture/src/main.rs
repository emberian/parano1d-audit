//! split-cd1 local PoC: an aliased coinbase slot (s + 1 * 2^log_slots)
//! through the full production pipeline.
//!
//! Produces two chains from one parent prefix:
//!   honest-chain: blocks 1..5 honest, 6 = honest TWIN, 7 honest
//!   fork-chain:   blocks 1..5 honest (copied), 6 = ALIASED, 7..24 honest
//!
//! Decisive checks:
//!   * the production verifier ACCEPTS the aliased block-6 terminal;
//!   * the unmodified sequential validation path REJECTS the aliased block;
//!   * the honest twin and the aliased block have the IDENTICAL state root.

#[path = "full_history.rs"]
mod full_history;

use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use noid_chain::consensus::params::BLOCK_TIME;
use noid_chain::consensus::tx_epoch_anchor_height_for_child;
use noid_chain::consensus::validate_block_checks;
use noid_chain::storage::MdbxChainContext;
use noid_chain::{AcceptedBlockBundle, Block, BlockHeader};
use noid_miner::HistoryProtocolRuntime;
use noid_poseidon2b::primitives::Address;

const MINER: Address = Address([0x77; 32]);

/// Honest chain height before the copy point (shared prefix).
const FORK_BASE_HEIGHT: u64 = 5;
/// Honest chain final height (node A's head).
const HONEST_TIP_HEIGHT: u64 = 7;
/// Fork chain final height (node B's head; joiner boundary = 6).
const FORK_TIP_HEIGHT: u64 = 24;

fn coinbase_output_slot(block: &Block) -> u32 {
    block.transactions[0]
        .body
        .live_outputs()
        .next()
        .expect("primary coinbase output")
        .1
        .slot_index
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn epoch_anchor_header_for(ctx: &MdbxChainContext, header: &BlockHeader) -> Result<BlockHeader> {
    let height = tx_epoch_anchor_height_for_child(header.height);
    ctx.get_header_from_store(height)?
        .with_context(|| format!("epoch anchor header at height {height}"))
}

fn accept_on_inbound_chain(
    inbound: &mut MdbxChainContext,
    bundle: &AcceptedBlockBundle,
    runtime: &HistoryProtocolRuntime,
    local_time: u64,
) -> Result<()> {
    inbound
        .apply_next_block(
            bundle,
            local_time,
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
        )
        .map_err(|e| anyhow::anyhow!("apply_next_block: {e:?}"))?;
    Ok(())
}

fn materialize(
    block: &Block,
    state: &mut noid_chain::ChainState,
) -> std::result::Result<[u8; 32], String> {
    noid_chain::block::materialize_accepted_block_state(state, block).map_err(|e| format!("{e:?}"))
}

fn dump_header(path: &Path, header: &BlockHeader) -> Result<()> {
    let json = serde_json::json!({
        "height": header.height,
        "log_slots": header.log_slots,
        "state_root": hex::encode(header.state_root),
        "tx_root": hex::encode(header.tx_root),
        "active_slot_count": header.active_slot_count,
        "alloc_counter": header.alloc_counter,
        "timestamp": header.timestamp,
    });
    fs::write(path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

fn save_block(artifacts: &Path, name: &str, block: &Block, bundle: &AcceptedBlockBundle) -> Result<()> {
    fs::write(artifacts.join(format!("{name}.bytes")), block.to_bytes())?;
    fs::write(
        artifacts.join(format!("{name}-terminal.bytes")),
        bundle.history_step_terminal_bytes(),
    )?;
    dump_header(&artifacts.join(format!("{name}-header.json")), &block.header)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let run = full_history::lane_dir().join("run");
    let artifacts = run.join("artifacts");
    fs::create_dir_all(&artifacts)?;
    let honest_dir = run.join("honest-chain");
    let fork_dir = run.join("fork-chain");
    let peer_dir = run.join("peer-chain");
    for dir in [&honest_dir, &fork_dir, &peer_dir] {
        if dir.exists() {
            bail!("{} already exists; move it away for a fresh run", dir.display());
        }
    }

    let runtime = full_history::load_full_history_protocol_runtime(run.join("origins"))?;
    let ghost = full_history::prepare_v3_ghost_auth()?;

    // ------------------------------------------------------------------
    // Shared honest prefix 1..=5 on the honest chain.
    // ------------------------------------------------------------------
    std::env::remove_var("NOID_CD1_ALIAS_K");
    let mut honest = MdbxChainContext::open_or_create(&honest_dir)?;
    let genesis = *honest.tip_header();
    println!(
        "split-cd1: genesis log_slots={} state_root={}",
        genesis.log_slots,
        hex::encode(genesis.state_root)
    );
    ensure!(genesis.log_slots == 24, "expected depth-24 genesis");

    let mut parent = genesis;
    let mut honest_bundles: Vec<(Block, AcceptedBlockBundle)> = Vec::new();
    let t0 = Instant::now();
    for index in 0..FORK_BASE_HEIGHT {
        let ts = parent.timestamp + BLOCK_TIME;
        let (block, bundle) = full_history::produce_and_commit_block(
            &mut honest,
            &runtime,
            &ghost,
            MINER,
            ts,
            index as u128,
        )
        .await
        .with_context(|| format!("honest block {}", index + 1))?;
        println!(
            "honest block {} committed (coinbase slot {}) [{:?} elapsed]",
            block.header.height,
            coinbase_output_slot(&block),
            t0.elapsed()
        );
        parent = block.header;
        honest_bundles.push((block, bundle));
    }
    drop(honest);

    // Cold copy of the depth-5 prefix becomes the fork chain.
    copy_dir_recursive(&honest_dir, &fork_dir)?;
    println!("fork-chain seeded from the honest depth-5 prefix");

    // ------------------------------------------------------------------
    // Fork chain: ALIASED block 6, then honest 7..=24.
    // ------------------------------------------------------------------
    let mut fork = MdbxChainContext::open_or_create(&fork_dir)?;
    ensure!(fork.tip_height() == FORK_BASE_HEIGHT, "fork seed height");
    let fork_parent = *fork.tip_header();
    std::env::set_var("NOID_CD1_ALIAS_K", "1");
    let ts6 = fork_parent.timestamp + BLOCK_TIME;
    let t_alias = Instant::now();
    let produce_aliased =
        full_history::produce_and_commit_block(&mut fork, &runtime, &ghost, MINER, ts6, 100);
    let (aliased_block, aliased_bundle) = match produce_aliased.await {
        Ok(parts) => parts,
        Err(error) => {
            std::env::remove_var("NOID_CD1_ALIAS_K");
            return Err(error.context("aliased fork block 6"));
        }
    };
    std::env::remove_var("NOID_CD1_ALIAS_K");
    println!(
        "ALIASED fork block 6 committed in {:?}",
        t_alias.elapsed()
    );

    let domain = 1u32 << aliased_block.header.log_slots;
    let aliased_slot = coinbase_output_slot(&aliased_block);
    let aliased_leaf = aliased_slot & (domain - 1);
    ensure!(
        aliased_slot >= domain && aliased_slot < 2 * domain,
        "fork coinbase slot {aliased_slot} is not the k=1 alias"
    );
    println!(
        "aliased coinbase wire slot = {aliased_slot} = leaf {aliased_leaf} + 1 * 2^{}",
        aliased_block.header.log_slots
    );

    let mut fork_parent_hdr = aliased_block.header;
    for index in 0..(FORK_TIP_HEIGHT - FORK_BASE_HEIGHT - 1) {
        let ts = fork_parent_hdr.timestamp + BLOCK_TIME;
        let (block, bundle) = full_history::produce_and_commit_block(
            &mut fork,
            &runtime,
            &ghost,
            MINER,
            ts,
            200 + index as u128,
        )
        .await
        .with_context(|| format!("fork block {}", FORK_BASE_HEIGHT + 2 + index))?;
        println!(
            "fork block {} committed (coinbase slot {}) [{:?} elapsed]",
            block.header.height,
            coinbase_output_slot(&block),
            t0.elapsed()
        );
        fork_parent_hdr = block.header;
    }
    ensure!(fork.tip_height() == FORK_TIP_HEIGHT);
    drop(fork);

    // ------------------------------------------------------------------
    // Honest chain: twin block 6 and final block 7.
    // ------------------------------------------------------------------
    let mut honest = MdbxChainContext::open_or_create(&honest_dir)?;
    ensure!(honest.tip_height() == FORK_BASE_HEIGHT, "honest resume height");
    let honest_parent = *honest.tip_header();
    let ts_twin = honest_parent.timestamp + BLOCK_TIME;
    let (twin_block, twin_bundle) = full_history::produce_and_commit_block(
        &mut honest,
        &runtime,
        &ghost,
        MINER,
        ts_twin,
        300,
    )
    .await
    .context("honest twin block 6")?;
    println!("honest TWIN block 6 committed");

    // The twin's slot selection is deterministic from the shared parent.
    let twin_slot = coinbase_output_slot(&twin_block);
    ensure!(twin_slot == aliased_leaf,
        "twin slot {twin_slot} differs from the aliased leaf {aliased_leaf}");
    ensure!(twin_slot < domain, "twin slot out of domain");
    ensure!(
        twin_block.header.state_root == aliased_block.header.state_root,
        "twin and aliased state roots differ"
    );
    ensure!(
        twin_block.header.tx_root != aliased_block.header.tx_root,
        "twin and aliased bodies must differ"
    );
    println!(
        "ALIAS CONFIRMED: wire slots {aliased_slot} (aliased) and {twin_slot} (honest) \
         produce the identical state root {}",
        hex::encode(aliased_block.header.state_root)
    );

    let (honest_tip_block, honest_tip_bundle) = full_history::produce_and_commit_block(
        &mut honest,
        &runtime,
        &ghost,
        MINER,
        twin_block.header.timestamp + BLOCK_TIME,
        301,
    )
    .await
    .context("honest block 7")?;
    ensure!(honest.tip_height() == HONEST_TIP_HEIGHT);
    println!("honest block 7 committed; honest chain complete");
    drop(honest);

    // ------------------------------------------------------------------
    // THE ACCEPTANCE: production verifier on the aliased terminal. The fork
    // context is reopened read-only for the epoch anchor header.
    // ------------------------------------------------------------------
    let fork_ro = MdbxChainContext::open_or_create(&fork_dir)?;
    let epoch_header = epoch_anchor_header_for(&fork_ro, &aliased_block.header)?;
    let t_verify = Instant::now();
    runtime
        .verify_terminal(
            aliased_bundle.history_step_terminal_bytes(),
            &aliased_block.header,
            &epoch_header,
        )
        .map_err(|e| anyhow::anyhow!("aliased terminal rejected by production verifier: {e}"))?;
    println!(
        "ALIASED TERMINAL ACCEPTED by production verify_terminal in {:?} (release packs, 64/64/16)",
        t_verify.elapsed()
    );

    // ------------------------------------------------------------------
    // THE REJECTION: a peer chain holding the honest prefix 1..=5 must
    // reject the aliased block at unmodified sequential validation.
    // ------------------------------------------------------------------
    let mut peer = MdbxChainContext::open_or_create(&peer_dir)?;
    let peer_time = honest_parent.timestamp + 10 * BLOCK_TIME;
    for (index, (block, bundle)) in honest_bundles.iter().enumerate() {
        accept_on_inbound_chain(&mut peer, bundle, &runtime, block.header.timestamp)
            .with_context(|| format!("peer accepts honest block {}", index + 1))?;
    }
    println!("peer chain synced the honest prefix 1..=5 through the full inbound path");

    let prev_timestamps = peer.prev_timestamps()?;
    let counts = peer.finalized_active_counts()?;
    let anchor = peer.anchor_info()?;
    let err = validate_block_checks(
        &aliased_block,
        &honest_parent,
        &prev_timestamps,
        &counts,
        peer_time,
        &anchor,
    );
    match err {
        Err(noid_chain::consensus::ConsensusError::ShapeMismatch(message)) => {
            println!(
                "ALIASED BLOCK REJECTED by sequential validate_block_checks: ShapeMismatch({message})"
            );
        }
        other => bail!("expected ShapeMismatch from validate_block_checks, got {other:?}"),
    }

    let err2 = peer.apply_next_block(
        &aliased_bundle,
        peer_time,
        materialize,
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
    );
    match err2 {
        Err(error) => {
            let text = format!("{error:?}");
            ensure!(
                text.contains("ShapeMismatch"),
                "peer rejection is not the slot-domain ShapeMismatch: {text}"
            );
            println!("ALIASED BLOCK REJECTED by the full inbound peer path: {text}");
        }
        Ok(_) => bail!("peer chain accepted the aliased block (CD-1 refuted)"),
    }

    // Control: the honest twin is fully accepted by the same peer chain.
    accept_on_inbound_chain(&mut peer, &twin_bundle, &runtime, ts_twin)?;
    println!("control: honest TWIN ACCEPTED by the same peer chain that rejected the alias");
    let (tip_block, tip_bundle) = &honest_bundles[0];
    let _ = (tip_block, tip_bundle);
    drop(peer);

    // ------------------------------------------------------------------
    // Artifacts.
    // ------------------------------------------------------------------
    for (index, (block, bundle)) in honest_bundles.iter().enumerate() {
        save_block(&artifacts, &format!("h{}", index + 1), block, bundle)?;
    }
    save_block(&artifacts, "h6-twin", &twin_block, &twin_bundle)?;
    save_block(&artifacts, "h7-tip", &honest_tip_block, &honest_tip_bundle)?;
    save_block(&artifacts, "f6-aliased", &aliased_block, &aliased_bundle)?;
    fs::write(
        artifacts.join("summary.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "pinned_commit": full_history::PINNED_COMMIT,
            "fork_base_height": FORK_BASE_HEIGHT,
            "honest_tip_height": HONEST_TIP_HEIGHT,
            "fork_tip_height": FORK_TIP_HEIGHT,
            "aliased_wire_slot": aliased_slot,
            "aliased_leaf": aliased_leaf,
            "alias_k": 1u32,
            "honest_twin_slot": twin_slot,
            "log_slots": aliased_block.header.log_slots,
            "aliased_state_root": hex::encode(aliased_block.header.state_root),
            "twin_state_root": hex::encode(twin_block.header.state_root),
            "terminal_accepted_by_production_verifier": true,
            "sequential_rejection": "ShapeMismatch(live output slot is outside block log_slots)",
            "joiner_boundary_height": 6u64,
        }))?,
    )?;
    println!(
        "split-cd1 fixture COMPLETE; chains in {}/ and {}/, artifacts in {}",
        honest_dir.display(),
        fork_dir.display(),
        artifacts.display()
    );
    Ok(())
}
