use noid_chain::block_header::{block_id, semantic_header_id};
use noid_chain::consensus::genesis::genesis_header;
use noid_chain::history_step::HistoryStepTerminalMetadata;
use noid_chain::storage::MdbxChainContext;
use serde_json::json;

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let work = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("work");
    std::fs::create_dir_all(&work)?;
    let directory = tempfile::Builder::new()
        .prefix("snapshot-")
        .tempdir_in(work)?;
    let context = MdbxChainContext::open_or_create(directory.path())?;
    let genesis = genesis_header();
    let mut candidate = genesis;
    candidate.height = 1;
    candidate.prev_block_hash = block_id(&genesis);
    candidate.timestamp += 1;
    candidate.nonce += 1;

    let mut other = candidate;
    other.timestamp += 1;
    assert_ne!(semantic_header_id(&candidate), semantic_header_id(&other));
    let foreign_terminal =
        HistoryStepTerminalMetadata::new(other.height, semantic_header_id(&other), 0)?
            .encode_prefix()
            .to_vec();
    let mut foreign_calls = 0usize;
    let foreign = context.verify_snapshot_boundary(candidate, genesis, foreign_terminal, |_| {
        foreign_calls += 1;
        Ok(())
    });

    let matching_terminal =
        HistoryStepTerminalMetadata::new(candidate.height, semantic_header_id(&candidate), 0)?
            .encode_prefix()
            .to_vec();
    let mut matching_calls = 0usize;
    let matching = context.verify_snapshot_boundary(candidate, genesis, matching_terminal, |_| {
        matching_calls += 1;
        Ok(())
    });

    let mut wrong_epoch = genesis;
    wrong_epoch.height = 1;
    let wrong_epoch_terminal =
        HistoryStepTerminalMetadata::new(candidate.height, semantic_header_id(&candidate), 0)?
            .encode_prefix()
            .to_vec();
    let wrong_epoch_result =
        context.verify_snapshot_boundary(candidate, wrong_epoch, wrong_epoch_terminal, |_| Ok(()));

    assert!(foreign.is_err());
    assert_eq!(foreign_calls, 0);
    assert!(matching.is_ok());
    assert_eq!(matching_calls, 1);
    assert!(wrong_epoch_result.is_err());
    drop(context);

    let output = json!({
        "check": "snapshot-terminal-boundary-only",
        "foreign_header_terminal_accepted": foreign.is_ok(),
        "foreign_header_error": format!("{:?}", foreign.unwrap_err()),
        "foreign_terminal_verifier_calls": foreign_calls,
        "matching_terminal_with_completion_accepted": matching.is_ok(),
        "matching_terminal_verifier_calls": matching_calls,
        "wrong_epoch_anchor_accepted": wrong_epoch_result.is_ok(),
        "scope": "real snapshot boundary preflight; matching case uses an explicit completion closure, not a full HistoryStep or segment install"
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
