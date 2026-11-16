//! End-to-end conformance test of timeless snapshot header staging, MTP-11
//! intermediate timestamp windows, genuine `HistoryProtocolRuntime` recursive
//! terminal proving/verification, and `validate_snapshot_install_selection`
//! (`SnapshotRebaseAnchor`) behavior on loopback release daemons.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use comp_sync_finality::{
    load_full_history_protocol_runtime, prepare_v3_ghost_auth, produce_and_commit_block, unix_now,
    LocalDaemon, PINNED_COMMIT,
};
use noid_chain::block_header::block_id;
use noid_chain::consensus::{
    asert_anchor_height, validate_future_drift, validate_header, ConsensusError,
};
use noid_chain::storage::MdbxChainContext;
use noid_node::snapshot_header_staging::{CanonicalHeaderBoundary, SnapshotHeaderStaging};
use noid_poseidon2b::primitives::Address;
use serde_json::json;

const SHIFT_OFFSET_SECS: u64 = 10 * 365 * 24 * 3600;

fn copy_dir_files(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if file_type.is_file() {
            fs::copy(entry.path(), dst_path)?;
        } else if file_type.is_dir() {
            copy_dir_files(&entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

async fn wait_for_log_substring(
    daemon: &LocalDaemon,
    needle: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let log = daemon.read_clean_log();
        if let Some(line) = log.lines().find(|l| l.contains(needle)) {
            return Ok(line.to_string());
        }
        if Instant::now() >= deadline {
            bail!(
                "timeout waiting for {:?} in {}; tail:\n{}",
                needle,
                daemon.name,
                daemon.log_tail(30)
            );
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

async fn wait_for_height_at_least(
    daemon: &LocalDaemon,
    min_height: u64,
    timeout: Duration,
) -> Result<(u64, String)> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok((h, hash)) = daemon.chain_tip().await {
            if h >= min_height {
                return Ok((h, hash));
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "timeout waiting for {} height >= {}; tail:\n{}",
                daemon.name,
                min_height,
                daemon.log_tail(30)
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn verify_v21_accumulator_timelessness(now: u64) -> serde_json::Value {
    let v21_height = 279_370u64;
    let mut epoch_hdr = noid_chain::consensus::genesis_header();
    epoch_hdr.height = 279_360;
    epoch_hdr.timestamp = now - 600;

    let mut parent = epoch_hdr;
    parent.height = v21_height - 1;
    parent.prev_block_hash = [0x44; 32];
    parent.timestamp = now + SHIFT_OFFSET_SECS;

    let mut child = parent;
    child.height = v21_height;
    child.prev_block_hash = block_id(&parent);
    child.timestamp = now + 15;

    let start_acc = noid_recursive::ChainAccumulator {
        height: parent.height,
        tip_semantic_id: noid_chain::block_header::semantic_header_id(&parent),
        state_root: parent.state_root,
        log_slots: parent.log_slots,
        active_slot_count: parent.active_slot_count,
        alloc_counter: parent.alloc_counter,
        epoch_anchor_id: block_id(&epoch_hdr),
    };
    start_acc
        .validate_local_header_boundary(&parent, &epoch_hdr)
        .expect("start boundary valid with shifted parent timestamp");
    let end_acc = start_acc
        .advance(&parent, &child)
        .expect("v2.1 ChainAccumulator::advance is timeless and accepts present-day child after shifted parent");
    end_acc
        .validate_local_header_boundary(&child, &epoch_hdr)
        .expect("end boundary valid at v2.1 height");

    json!({
        "v2_1_height": v21_height,
        "parent_timestamp_offset_secs": SHIFT_OFFSET_SECS,
        "child_timestamp": child.timestamp,
        "accumulator_advance_accepted": true,
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    assert!(!cfg!(debug_assertions), "run with --profile fast");
    let work_dir = tempfile::tempdir()?;
    let origin_dir = work_dir.path().join("origins");
    let t_load = Instant::now();
    let runtime = load_full_history_protocol_runtime(origin_dir)?;
    let ghost = prepare_v3_ghost_auth()?;
    let runtime_load_ms = t_load.elapsed().as_millis() as u64;

    let base_now = unix_now();
    let branch_a_dir = work_dir.path().join("branch_a_src");
    let branch_b54_dir = work_dir.path().join("branch_b54_src");
    let branch_b42_dir = work_dir.path().join("branch_b42_src");

    // 1. Build Branch A (31 blocks, strictly standard timestamps) with genuine HistoryStep proofs.
    let t_build_a = Instant::now();
    let (branch_a_tip_hash, branch_a_work) = {
        let mut ctx_a = MdbxChainContext::open_or_create(&branch_a_dir)?;
        let miner_a = Address([0xA1; 32]);
        for h in 1..=31u64 {
            let ts = base_now - 900 + h * 20;
            produce_and_commit_block(&mut ctx_a, &runtime, &ghost, miner_a, ts, u128::from(h) << 32)
                .await
                .with_context(|| format!("Branch A block {h}"))?;
        }
        assert_eq!(ctx_a.tip_height(), 31);
        assert_eq!(ctx_a.finalized_checkpoint().height, 13);
        (hex::encode(ctx_a.tip_hash()), *ctx_a.tip_chain_work())
    };
    let build_a_ms = t_build_a.elapsed().as_millis() as u64;

    // 2. Build Branch B (54 blocks: h12..h16 shifted +10y, h17..h54 returned to <= now via MTP-11),
    //    saving a snapshot of the store at h42 (boundary h24) before extending to h54 (boundary h36).
    let t_build_b = Instant::now();
    let (
        branch_b42_tip_hash,
        branch_b54_tip_hash,
        branch_b24_hash,
        branch_b36_hash,
        direct_verification_json,
    ) = {
        let mut ctx_b = MdbxChainContext::open_or_create(&branch_b54_dir)?;
        let miner_b = Address([0xB2; 32]);
        let mut b42_hash = String::new();
        for h in 1..=54u64 {
            let ts = if (12..=16).contains(&h) {
                base_now + SHIFT_OFFSET_SECS + h
            } else if h <= 11 {
                base_now - 900 + h * 20
            } else {
                base_now - 600 + h * 8
            };
            produce_and_commit_block(
                &mut ctx_b,
                &runtime,
                &ghost,
                miner_b,
                ts,
                (1u128 << 64) | (u128::from(h) << 32),
            )
            .await
            .with_context(|| format!("Branch B block {h}"))?;
            if h == 42 {
                b42_hash = hex::encode(ctx_b.tip_hash());
                // Copy durable MDBX files at height 42 before continuing to 54.
                copy_dir_files(&branch_b54_dir, &branch_b42_dir)?;
            }
        }
        assert_eq!(ctx_b.tip_height(), 54);
        assert_eq!(ctx_b.finalized_checkpoint().height, 36);

        let mut headers_b = Vec::with_capacity(55);
        for h in 0..=54u64 {
            headers_b.push(ctx_b.store.get_header(h)?.expect("header present"));
        }
        let h24_hash = block_id(&headers_b[24]);
        let h36_hash = block_id(&headers_b[36]);
        let h36_work = ctx_b.store.get_chain_work(36)?.expect("h36 chainwork");
        let h36_terminal = ctx_b
            .store
            .get_history_step_terminal_at(36, h36_hash)?
            .expect("h36 HistoryStep terminal persisted");

        // Verify live `validate_header` rejects h12..h16 at `now` with BadTimestamp.
        let now_check = unix_now();
        for shifted_h in 12..=16usize {
            let parent = headers_b[shifted_h - 1];
            let start = shifted_h.saturating_sub(11);
            let prev_ts: Vec<u64> = headers_b[start..shifted_h]
                .iter()
                .map(|hdr| hdr.timestamp)
                .collect();
            let anchor_h = asert_anchor_height(parent.height);
            let anchor = headers_b[anchor_h as usize];
            let err = validate_header(
                &headers_b[shifted_h],
                &parent,
                &prev_ts,
                &[],
                now_check,
                anchor_h,
                anchor.timestamp,
                &anchor.difficulty_target,
            )
            .expect_err("live admission must reject shifted intermediate header");
            assert_eq!(err, ConsensusError::BadTimestamp);
        }

        // Direct SnapshotHeaderStaging + genuine HistoryProtocolRuntime::verify_terminal check on h36:
        let clean_db_dir = work_dir.path().join("direct_verify_db");
        let clean_ctx = MdbxChainContext::open_or_create(&clean_db_dir)?;
        let base_boundary = CanonicalHeaderBoundary::load(&clean_ctx.store, 0)?;
        let stage_path = work_dir.path().join("direct_stage.headers");
        let mut staging =
            SnapshotHeaderStaging::create(&stage_path, &clean_ctx.store, base_boundary)?;
        staging.append_batch(&clean_ctx.store, &headers_b[1..=36])?;
        let validated = staging.validate_complete(&clean_ctx.store, 36, h36_hash, h36_work)?;
        let boundary = validated.boundary();
        validate_future_drift(boundary.tip_header.timestamp, now_check)
            .expect("h36 boundary tip satisfies present future-drift window");

        let t_verify = Instant::now();
        let verified_boundary = clean_ctx.verify_snapshot_boundary(
            boundary.tip_header,
            boundary.epoch_anchor_header,
            h36_terminal.clone(),
            |claim| {
                runtime.verify_terminal(
                    claim.terminal_bytes,
                    &claim.header,
                    &claim.epoch_anchor_header,
                )
            },
        )?;
        let verify_ms = t_verify.elapsed().as_millis() as u64;
        assert_eq!(verified_boundary.header().height, 36);

        // Negative control: 1-bit mutation of the terminal payload is rejected by the real verifier.
        let mut corrupted_terminal = h36_terminal.clone();
        let mid = corrupted_terminal.len() / 2;
        corrupted_terminal[mid] ^= 0x01;
        let corrupted_res = clean_ctx.verify_snapshot_boundary(
            boundary.tip_header,
            boundary.epoch_anchor_header,
            corrupted_terminal,
            |claim| {
                runtime.verify_terminal(
                    claim.terminal_bytes,
                    &claim.header,
                    &claim.epoch_anchor_header,
                )
            },
        );
        assert!(corrupted_res.is_err(), "corrupted terminal must be rejected");

        assert!(
            noid_chain::work_gt(&h36_work, &branch_a_work),
            "h36 work must exceed h31 work"
        );

        let direct_json = json!({
            "shifted_heights": [12, 13, 14, 15, 16],
            "shifted_timestamp_example_h12": headers_b[12].timestamp,
            "returned_timestamp_h17": headers_b[17].timestamp,
            "boundary_h36_timestamp": headers_b[36].timestamp,
            "tip_h54_timestamp": headers_b[54].timestamp,
            "live_validate_header_rejects_h12_to_h16": "BadTimestamp",
            "snapshot_header_staging_accepts_h1_to_h36": true,
            "boundary_tip_future_drift_ok": true,
            "genuine_history_step_terminal_bytes": h36_terminal.len(),
            "genuine_history_step_terminal_verified": true,
            "genuine_terminal_verify_ms": verify_ms,
            "mutated_terminal_rejected": true,
        });

        (
            b42_hash,
            hex::encode(ctx_b.tip_hash()),
            hex::encode(h24_hash),
            hex::encode(h36_hash),
            direct_json,
        )
    };
    let build_b_ms = t_build_b.elapsed().as_millis() as u64;

    // 3. Live Loopback Release-Daemon Differential Runs (`bin/parano1d`)
    let run_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("logs/run_snapshot");
    let _ = fs::remove_dir_all(&run_root);
    fs::create_dir_all(&run_root)?;

    // --- Experiment 3A: boundary = 36 > branch_a_tip = 31 ---
    // Tests live adoption over a healthy un-throttled Branch A node due to
    // `validate_snapshot_install_selection` returning `Ok(Some(SnapshotRebaseAnchor))`
    // which `try_start_ready_snapshot_install!` discards via `.err()` at `main.rs:9699`.
    let exp_3a = {
        let dir_a = run_root.join("exp3a_node_a");
        let dir_b = run_root.join("exp3a_node_b54");
        let dir_j1 = run_root.join("exp3a_joiner1");
        let dir_j2 = run_root.join("exp3a_joiner2");
        copy_dir_files(&branch_a_dir, &dir_a)?;
        copy_dir_files(&branch_b54_dir, &dir_b)?;

        let mut node_a = LocalDaemon::spawn(&run_root, "exp3a_node_a", dir_a, 24110, 24111, &[])?;
        let mut node_b =
            LocalDaemon::spawn(&run_root, "exp3a_node_b54", dir_b, 24120, 24121, &[])?;
        node_a.wait_rpc_ready(Duration::from_secs(45)).await?;
        node_b.wait_rpc_ready(Duration::from_secs(45)).await?;

        let pub_a = wait_for_log_substring(
            &node_a,
            "published bounded disk snapshot generation",
            Duration::from_secs(45),
        )
        .await?;
        let pub_b = wait_for_log_substring(
            &node_b,
            "published bounded disk snapshot generation",
            Duration::from_secs(45),
        )
        .await?;

        let seed_a = node_a.p2p_addr();
        let seed_b = node_b.p2p_addr();
        let mut joiner_1 = LocalDaemon::spawn(
            &run_root,
            "exp3a_joiner1",
            dir_j1,
            24130,
            24131,
            &["--seed", &seed_a, "--seed", &seed_b],
        )?;
        joiner_1.wait_rpc_ready(Duration::from_secs(45)).await?;

        let (j1_height, j1_hash) =
            wait_for_height_at_least(&joiner_1, 54, Duration::from_secs(90)).await?;
        // Wait for joiner_1 to publish its own snapshot generation before testing propagation to joiner_2,
        // and allow peer probes against node_a to record FinalizedDivergence.
        let j1_pub = wait_for_log_substring(
            &joiner_1,
            "published bounded disk snapshot generation",
            Duration::from_secs(45),
        )
        .await?;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let j1_log = joiner_1.read_clean_log();
        let native_bad_ts_logged = j1_log.contains("BadTimestamp");
        let snapshot_installed_logged = j1_log.contains("snapshot install completed");
        let finalized_divergence_logged =
            j1_log.contains("no common ancestor inside the accepted non-final window");

        // Second fresh joiner seeded ONLY to joiner_1 (verifies self-propagation).
        let seed_j1 = joiner_1.p2p_addr();
        let mut joiner_2 = LocalDaemon::spawn(
            &run_root,
            "exp3a_joiner2",
            dir_j2,
            24140,
            24141,
            &["--seed", &seed_j1],
        )?;
        joiner_2.wait_rpc_ready(Duration::from_secs(45)).await?;
        let (j2_height, j2_hash) =
            wait_for_height_at_least(&joiner_2, 54, Duration::from_secs(90)).await?;

        joiner_2.shutdown();
        joiner_1.shutdown();
        node_b.shutdown();
        node_a.shutdown();

        assert_eq!(j1_hash, branch_b54_tip_hash);
        assert_eq!(j2_hash, branch_b54_tip_hash);

        json!({
            "branch_a_tip": [31, branch_a_tip_hash],
            "branch_b54_boundary": [36, branch_b36_hash],
            "branch_b54_tip": [54, branch_b54_tip_hash],
            "node_a_snapshot_log": pub_a,
            "node_b_snapshot_log": pub_b,
            "joiner_1_final_tip": [j1_height, j1_hash],
            "joiner_1_adopted_shifted_branch": j1_hash == branch_b54_tip_hash,
            "joiner_1_logged_native_bad_timestamp": native_bad_ts_logged,
            "joiner_1_logged_snapshot_install": snapshot_installed_logged,
            "joiner_1_logged_finalized_divergence_vs_branch_a": finalized_divergence_logged,
            "joiner_1_published_snapshot_log": j1_pub,
            "joiner_2_final_tip": [j2_height, j2_hash],
            "joiner_2_propagated_shifted_branch": j2_hash == branch_b54_tip_hash,
        })
    };

    // --- Experiment 3B (Control): boundary = 24 <= branch_a_tip = 31 (bridge_tip = 42 > 31) ---
    // Branch B42 still wins the 5-second manifest candidate selection (`42 > 31`),
    // stages `1..24`, and passes genuine HistoryStep terminal verification,
    // THEN hits `validate_snapshot_install_selection` with `24 <= selected_tip (31)`,
    // which checks `point_at_height(31, 24)` (`main.rs:308-314`) and REJECTS install!
    let exp_3b = {
        let dir_a = run_root.join("exp3b_node_a");
        let dir_b42 = run_root.join("exp3b_node_b42");
        let dir_jctrl = run_root.join("exp3b_joiner_ctrl");
        copy_dir_files(&branch_a_dir, &dir_a)?;
        copy_dir_files(&branch_b42_dir, &dir_b42)?;

        let mut node_a = LocalDaemon::spawn(&run_root, "exp3b_node_a", dir_a, 24150, 24151, &[])?;
        let mut node_b42 =
            LocalDaemon::spawn(&run_root, "exp3b_node_b42", dir_b42, 24160, 24161, &[])?;
        node_a.wait_rpc_ready(Duration::from_secs(45)).await?;
        node_b42.wait_rpc_ready(Duration::from_secs(45)).await?;

        wait_for_log_substring(
            &node_a,
            "published bounded disk snapshot generation",
            Duration::from_secs(45),
        )
        .await?;
        wait_for_log_substring(
            &node_b42,
            "published bounded disk snapshot generation",
            Duration::from_secs(45),
        )
        .await?;

        let seed_a = node_a.p2p_addr();
        let seed_b42 = node_b42.p2p_addr();
        let mut joiner_ctrl = LocalDaemon::spawn(
            &run_root,
            "exp3b_joiner_ctrl",
            dir_jctrl,
            24170,
            24171,
            &["--seed", &seed_a, "--seed", &seed_b42],
        )?;
        joiner_ctrl.wait_rpc_ready(Duration::from_secs(45)).await?;

        let (jc_height, jc_hash) =
            wait_for_height_at_least(&joiner_ctrl, 31, Duration::from_secs(90)).await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let jc_log = joiner_ctrl.read_clean_log();
        let left_selected_ancestry_logged =
            jc_log.contains("verified snapshot left the HeaderDAG-selected ancestry");

        joiner_ctrl.shutdown();
        node_b42.shutdown();
        node_a.shutdown();

        assert_eq!(jc_height, 31);
        assert_eq!(jc_hash, branch_a_tip_hash);

        json!({
            "branch_a_tip": [31, branch_a_tip_hash],
            "branch_b42_boundary": [24, branch_b24_hash],
            "branch_b42_tip": [42, branch_b42_tip_hash],
            "joiner_ctrl_final_tip": [jc_height, jc_hash],
            "joiner_ctrl_adopted_reference_branch_a": jc_hash == branch_a_tip_hash,
            "joiner_ctrl_logged_header_dag_ancestry_rejection": left_selected_ancestry_logged,
        })
    };

    let v21_check = verify_v21_accumulator_timelessness(base_now);

    let report = json!({
        "pinned_commit": PINNED_COMMIT,
        "debug_assertions": cfg!(debug_assertions),
        "timings_ms": {
            "runtime_load_ms": runtime_load_ms,
            "build_branch_a_31_blocks_ms": build_a_ms,
            "build_branch_b_54_blocks_ms": build_b_ms,
        },
        "direct_genuine_terminal_verification": direct_verification_json,
        "v2_1_accumulator_timelessness": v21_check,
        "live_release_daemon_differential": {
            "boundary_above_header_dag_tip_36_gt_31": exp_3a,
            "boundary_at_or_below_header_dag_tip_24_le_31": exp_3b,
        },
    });

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
