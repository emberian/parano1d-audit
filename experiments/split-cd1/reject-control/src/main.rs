//! split-cd1 reject control: replay the saved fixture blocks through the
//! UNMODIFIED pinned tree (`comp-system-hunter/pinned`, commit 52938433).
//!
//! The pinned `noid_chain` accepts the honest prefix on its native inbound
//! path, must reject the aliased fork block at sequential validation (the
//! terminal callback is a stub that always succeeds, proving rejection
//! happens before terminal verification), and must accept the honest twin.

use std::fs;

use anyhow::{bail, ensure, Context, Result};
use noid_chain::storage::MdbxChainContext;
use noid_chain::AcceptedBlockBundle;

fn artifacts() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("run/artifacts")
}

fn materialize(
    block: &noid_chain::Block,
    state: &mut noid_chain::ChainState,
) -> Result<[u8; 32], String> {
    noid_chain::block::materialize_accepted_block_state(state, block).map_err(|e| format!("{e:?}"))
}

fn main() -> Result<()> {
    let dir = artifacts();
    let pin_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("run/pin-chain");
    if pin_path.exists() {
        let _ = std::fs::remove_dir_all(&pin_path);
    }
    std::fs::create_dir_all(&pin_path)?;
    let mut pin_chain = MdbxChainContext::open_or_create(&pin_path)?;
    let local_time = pin_chain.tip_header().timestamp + 60 * 20;

    // Honest prefix 1..=5.
    for index in 1..=5u64 {
        let block_bytes = fs::read(dir.join(format!("h{index}.bytes")))
            .with_context(|| format!("h{index} bytes"))?;
        let terminal_bytes = fs::read(dir.join(format!("h{index}-terminal.bytes")))?;
        let bundle = AcceptedBlockBundle::try_from_parts(block_bytes, terminal_bytes)
            .with_context(|| format!("bundle h{index}"))?;
        pin_chain
            .apply_next_block(&bundle, local_time, materialize, |_| Ok(()))
            .map_err(|e| anyhow::anyhow!("pinned tree rejected honest block {index}: {e:?}"))?;
        println!("pinned control: honest block {index} ACCEPTED by the unmodified pinned tree");
    }

    // Aliased fork block 6: native validation must reject before the stub
    // terminal callback could matter.
    let aliased_block = fs::read(dir.join("f6-aliased.bytes")).context("aliased block bytes")?;
    let aliased_terminal = fs::read(dir.join("f6-aliased-terminal.bytes"))?;
    let bundle = AcceptedBlockBundle::try_from_parts(aliased_block, aliased_terminal)
        .context("bundle f6")?;
    let err = pin_chain.apply_next_block(&bundle, local_time, materialize, |_| Ok(()));
    match err {
        Err(error) => {
            let text = format!("{error:?}");
            ensure!(
                text.contains("ShapeMismatch"),
                "pinned rejection is not the slot-domain ShapeMismatch: {text}"
            );
            println!(
                "pinned control: ALIASED BLOCK REJECTED by the unmodified pinned tree: {text}"
            );
        }
        Ok(()) => bail!("unmodified pinned tree accepted the aliased block"),
    }

    // Honest twin on the same chain.
    let twin_block = fs::read(dir.join("h6-twin.bytes")).context("twin block bytes")?;
    let twin_terminal = fs::read(dir.join("h6-twin-terminal.bytes"))?;
    let twin_bundle =
        AcceptedBlockBundle::try_from_parts(twin_block, twin_terminal).context("twin bundle")?;
    pin_chain
        .apply_next_block(&twin_bundle, local_time, materialize, |_| Ok(()))
        .map_err(|e| anyhow::anyhow!("pinned tree rejected the honest twin: {e:?}"))?;
    println!("pinned control: honest twin ACCEPTED by the same unmodified pinned tree");
    println!("pinned control COMPLETE");
    Ok(())
}
