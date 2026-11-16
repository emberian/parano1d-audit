//! Conformance verification of v2/v2.1 ASERT difficulty adjustment, MTP-11
//! median-time-past windowing, chainwork accounting, and the pinned h210536
//! mainnet retirement boundary.

use std::sync::Arc;

use anyhow::Result;
use comp_sync_finality::{
    legacy_pack_dir, mainnet_retirement_origin_path, parse_pin_hex, read_bounded, v2_pack_dir,
    LEGACY_METADATA_PIN_HEX, PINNED_COMMIT, RETIREMENT_KEY_0_PIN_HEX, RETIREMENT_KEY_1_PIN_HEX,
    V2_BANK_PIN_HEX,
};
use noid_chain::block_header::{block_id, BlockHeader};
use noid_chain::consensus::difficulty::{
    add_work, block_work, next_target, target_leading_zero_bits, work_gt,
};
use noid_chain::consensus::params::{
    EPOCH_LENGTH, GENESIS_TARGET, MAX_FUTURE_DRIFT, MEDIAN_TIME_BLOCKS, MIN_TARGET,
    V2_1_ACTIVATION_HEIGHT, V2_ACTIVATION_HEIGHT, V2_BLOCK_TIME,
};
use noid_chain::consensus::pow::{search_pow, validate_pow};
use noid_chain::consensus::timestamps::median_time_past;
use noid_chain::consensus::{
    asert_anchor_height, validate_future_drift, validate_header, validate_header_timeless,
    ConsensusError,
};
use noid_miner::history_step_artifacts::{
    decode_history_step_runtime_metadata_pinned, HISTORY_STEP_PACK_VERSION_DIRECTORY,
    HISTORY_STEP_RUNTIME_METADATA_FILE, HISTORY_STEP_RUNTIME_METADATA_MAX_BYTES,
};
use noid_miner::v2_artifacts::{
    decode_v2_runtime_metadata_pinned, V2_METADATA_FILE, V2_METADATA_MAX_BYTES,
};
use noid_poseidon2b::primitives::Address;
use noid_recursive::acceptance::history_step::v2::banked as v2;
use noid_recursive::{
    CanonicalHistoryStepClassId, HistoryStepMatrixLease, HistoryStepMatrixSource,
    HistoryStepMatrixSourceError, HistoryStepRuntime,
};
use serde_json::json;

struct NoLegacyRows;
impl HistoryStepMatrixSource for NoLegacyRows {
    fn load(
        &self,
        _: CanonicalHistoryStepClassId,
    ) -> std::result::Result<HistoryStepMatrixLease, HistoryStepMatrixSourceError> {
        Err(HistoryStepMatrixSourceError)
    }
}

fn le256_to_u128_low(bytes: &[u8; 32]) -> u128 {
    assert!(
        bytes[16..].iter().all(|&b| b == 0),
        "work value exceeds u128 in test display"
    );
    u128::from_le_bytes(bytes[..16].try_into().unwrap())
}

fn synthetic_header(
    parent: &BlockHeader,
    timestamp: u64,
    difficulty_target: [u8; 32],
    mine_pow: bool,
) -> BlockHeader {
    let mut hdr = BlockHeader {
        prev_block_hash: block_id(parent),
        state_root: parent.state_root,
        tx_root: [0x11; 32],
        timestamp,
        height: parent.height + 1,
        miner_address: Address([0x22; 32]),
        nonce: 0,
        difficulty_target,
        log_slots: parent.log_slots,
        active_slot_count: parent.active_slot_count,
        alloc_counter: parent.alloc_counter,
    };
    if mine_pow {
        let mut cursor = 0u128;
        loop {
            if let Some(nonce) = search_pow(&hdr, cursor, 131_072) {
                hdr.nonce = nonce;
                break;
            }
            cursor += 131_072;
        }
        validate_pow(&hdr).expect("valid PoW for floor target");
    }
    hdr
}

fn find_min_floor_offset_secs(anchor_height: u64, anchor_ts: u64, anchor_target: &[u8; 32]) -> u64 {
    let child_height = anchor_height + 2;
    let mut lo = 0u64;
    let mut hi = 100_000u64;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let t = next_target(
            anchor_height,
            anchor_ts,
            anchor_target,
            child_height,
            anchor_ts + mid,
            anchor_ts + 30,
        );
        if t == GENESIS_TARGET {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

fn verify_mainnet_h210536_origin() -> Result<BlockHeader> {
    let legacy_v1 = legacy_pack_dir().join(HISTORY_STEP_PACK_VERSION_DIRECTORY);
    let legacy_meta = decode_history_step_runtime_metadata_pinned(
        &read_bounded(
            &legacy_v1.join(HISTORY_STEP_RUNTIME_METADATA_FILE),
            HISTORY_STEP_RUNTIME_METADATA_MAX_BYTES,
        )?,
        parse_pin_hex(LEGACY_METADATA_PIN_HEX)?,
    )?;
    let (legacy_bank, legacy_parts) = legacy_meta.into_parts();
    let legacy_runtime = Arc::new(HistoryStepRuntime::new(
        legacy_bank,
        Box::new(NoLegacyRows),
        legacy_parts,
    )?);

    let v2_dir = v2_pack_dir();
    let v2_meta = decode_v2_runtime_metadata_pinned(
        &read_bounded(&v2_dir.join(V2_METADATA_FILE), V2_METADATA_MAX_BYTES)?,
        parse_pin_hex(V2_BANK_PIN_HEX)?,
    )
    .map_err(anyhow::Error::msg)?;

    let key0 = read_bounded(&v2_dir.join("retirement-keys/class-0.key"), 512)?;
    let key1 = read_bounded(&v2_dir.join("retirement-keys/class-1.key"), 512)?;
    let retirement_keys = v2::PinnedRetirementKeys::from_release(
        legacy_runtime.bank(),
        [&key0, &key1],
        [
            parse_pin_hex(RETIREMENT_KEY_0_PIN_HEX)?,
            parse_pin_hex(RETIREMENT_KEY_1_PIN_HEX)?,
        ],
    )
    .map_err(|e| anyhow::anyhow!("retirement keys: {e}"))?;

    let origin_bytes = read_bounded(
        &mainnet_retirement_origin_path(),
        v2::MAX_RETIREMENT_ORIGIN_BYTES,
    )?;
    let cert = v2::RetirementOriginCertificate::from_bytes(&origin_bytes)
        .map_err(|e| anyhow::anyhow!("decode retirement cert: {e}"))?;
    let header = *cert.legacy_certificate().header();
    let verified = cert
        .verify(&legacy_runtime, v2_meta.bank(), &retirement_keys)
        .map_err(|e| anyhow::anyhow!("verify retirement cert: {e}"))?;
    assert_eq!(verified.origin().activation_height().unwrap(), 210_537);
    assert_eq!(header.height, 210_536);
    Ok(header)
}

fn run_epoch_anchor_reset_test(
    label: &str,
    base_epoch_height: u64,
    base_ts: u64,
    hard_target: [u8; 32],
    mine_floor_pow: bool,
) -> serde_json::Value {
    assert_eq!(base_epoch_height % EPOCH_LENGTH, 0);
    assert!(Some(base_epoch_height + 1) >= V2_ACTIVATION_HEIGHT);

    let min_offset_secs = find_min_floor_offset_secs(base_epoch_height, base_ts, &hard_target);
    let below_offset_target = next_target(
        base_epoch_height,
        base_ts,
        &hard_target,
        base_epoch_height + 2,
        base_ts + min_offset_secs - 1,
        base_ts + 30,
    );
    assert_ne!(below_offset_target, GENESIS_TARGET);

    // Seed 11 canonical ancestors ending at `base_epoch_height` (`6k`).
    let mut headers: Vec<BlockHeader> = Vec::new();
    let mut prev_hash = [0xAA; 32];
    for idx in 0..MEDIAN_TIME_BLOCKS {
        let h = base_epoch_height - (MEDIAN_TIME_BLOCKS as u64 - 1) + idx as u64;
        let ts = base_ts - ((MEDIAN_TIME_BLOCKS as u64 - 1 - idx as u64) * V2_BLOCK_TIME);
        let hdr = BlockHeader {
            prev_block_hash: prev_hash,
            state_root: [0x33; 32],
            tx_root: [0x11; 32],
            timestamp: ts,
            height: h,
            miner_address: Address([0x22; 32]),
            nonce: h as u128,
            difficulty_target: hard_target,
            log_slots: 24,
            active_slot_count: 89_000,
            alloc_counter: 120_000,
        };
        prev_hash = block_id(&hdr);
        headers.push(hdr);
    }
    assert_eq!(headers.last().unwrap().height, base_epoch_height);

    let base_work = [0u8; 32];
    let mut cum_work = base_work;
    let local_time_at_tip = base_ts + 25 * V2_BLOCK_TIME;

    // Construct 24 headers `6k+1 ..= 6k+24`:
    // - `6k+1 ..= 6k+5` (exactly 5 interior blocks): shifted timestamp `base_ts + min_offset_secs + k`
    // - `6k+6` (next ASERT anchor!): present-day timestamp `base_ts + 1` -> locks in GENESIS_TARGET!
    // - `6k+7 ..= 6k+24`: present-day timestamps advancing by V2_BLOCK_TIME (30s) -> stays at GENESIS_TARGET!
    let mut hard_blocks = 0usize;
    let mut floor_blocks = 0usize;

    for step in 1..=24u64 {
        let height = base_epoch_height + step;
        let parent = *headers.last().unwrap();
        let prev_11: Vec<u64> = headers[headers.len() - MEDIAN_TIME_BLOCKS..]
            .iter()
            .map(|h| h.timestamp)
            .collect();
        let mtp = median_time_past(&prev_11);

        let ts = if step <= 5 {
            base_ts + min_offset_secs + step
        } else if step == 6 {
            // At step == 6, 5 of 11 ancestors are shifted and 6 of 11 are <= base_ts,
            // so MTP equals `base_ts`!
            assert_eq!(mtp, base_ts, "MTP at 6k+6 must equal ts[6k]");
            base_ts + 1
        } else {
            base_ts + 1 + (step - 6) * V2_BLOCK_TIME
        };
        assert!(ts > mtp, "timestamp must strictly exceed MTP at step {step}");

        let anchor_h = asert_anchor_height(parent.height);
        let anchor_hdr = headers
            .iter()
            .find(|h| h.height == anchor_h)
            .copied()
            .expect("ASERT anchor in window");
        let expected_target = next_target(
            anchor_h,
            anchor_hdr.timestamp,
            &anchor_hdr.difficulty_target,
            height,
            parent.timestamp,
            ts,
        );

        if step == 1 {
            assert_eq!(expected_target, hard_target, "first block 6k+1 uses parent 6k target");
            hard_blocks += 1;
        } else {
            assert_eq!(
                expected_target, GENESIS_TARGET,
                "block at step {step} (h={height}) must be at GENESIS_TARGET"
            );
            floor_blocks += 1;
        }

        // Step 1 uses `hard_target` (which may be MIN_TARGET = 1 where PoW is infeasible);
        // Steps 2..=24 are at GENESIS_TARGET so we mine real Poseidon2b PoW on them!
        let do_pow = mine_floor_pow && step >= 2;
        let child = synthetic_header(&parent, ts, expected_target, do_pow);

        if do_pow {
            // Timeless validation (used by SnapshotHeaderStaging) MUST accept every step >= 2!
            validate_header_timeless(
                &child,
                &parent,
                &prev_11,
                &[],
                anchor_h,
                anchor_hdr.timestamp,
                &anchor_hdr.difficulty_target,
            )
            .expect("SnapshotHeaderStaging timeless validator accepts header");

            // Live wall-clock validation rejects steps 2..=5 and accepts steps 6..=24!
            let live_res = validate_header(
                &child,
                &parent,
                &prev_11,
                &[],
                local_time_at_tip,
                anchor_h,
                anchor_hdr.timestamp,
                &anchor_hdr.difficulty_target,
            );
            if step <= 5 {
                assert_eq!(live_res, Err(ConsensusError::BadTimestamp));
            } else {
                assert!(live_res.is_ok(), "live header check passes once returned to present");
            }
        }

        cum_work = add_work(&cum_work, &block_work(&child.difficulty_target));
        headers.push(child);
    }

    // Verify that by step 17 (`6k + 17`), all 5 far-future timestamps have completely aged out
    // of the 11-block MTP window.
    let window_at_17: Vec<u64> = headers
        .iter()
        .filter(|h| h.height >= base_epoch_height + 6 && h.height <= base_epoch_height + 16)
        .map(|h| h.timestamp)
        .collect();
    assert_eq!(window_at_17.len(), 11);
    assert!(window_at_17.iter().all(|&t| t <= local_time_at_tip));

    // Tightness negative control: if a 6th block (`6k+6`) also used a far-future timestamp,
    // then for `6k+7`, 6 of 11 ancestors are far-future, pushing MTP to `base_ts + min_offset_secs + 1`
    // and rejecting any present-day timestamp!
    let mut six_shifted = headers[..MEDIAN_TIME_BLOCKS + 5].to_vec();
    let parent_5 = *six_shifted.last().unwrap();
    let mut sixth_far = parent_5;
    sixth_far.height = base_epoch_height + 6;
    sixth_far.prev_block_hash = block_id(&parent_5);
    sixth_far.timestamp = base_ts + min_offset_secs + 6;
    six_shifted.push(sixth_far);
    let prev_11_after_6: Vec<u64> = six_shifted[six_shifted.len() - MEDIAN_TIME_BLOCKS..]
        .iter()
        .map(|h| h.timestamp)
        .collect();
    let mtp_after_6 = median_time_past(&prev_11_after_6);
    assert_eq!(mtp_after_6, base_ts + min_offset_secs + 1);
    assert!(
        validate_future_drift(mtp_after_6 + 1, local_time_at_tip).is_err(),
        "6th shifted block forces MTP past local future drift window"
    );

    // Compare cumulative chainwork against:
    // (a) 1 honest hard block `6k+1` (work = W(hard_target))
    // (b) 2 honest hard blocks `6k+1..6k+2` (work = 2 * W(hard_target))
    let one_hard_work = block_work(&hard_target);
    let two_hard_work = add_work(&one_hard_work, &one_hard_work);
    let floor_unit_work = block_work(&GENESIS_TARGET);

    let tip = headers.last().unwrap();
    assert!(validate_future_drift(tip.timestamp, local_time_at_tip).is_ok());

    json!({
        "label": label,
        "base_epoch_height": base_epoch_height,
        "v2_active": Some(base_epoch_height + 1) >= V2_ACTIVATION_HEIGHT,
        "v2_1_active": V2_1_ACTIVATION_HEIGHT.is_some_and(|h| base_epoch_height + 1 >= h),
        "hard_target_leading_zero_bits": target_leading_zero_bits(&hard_target),
        "floor_target_leading_zero_bits": target_leading_zero_bits(&GENESIS_TARGET),
        "min_parent_future_offset_secs_to_floor": min_offset_secs,
        "min_parent_future_offset_hours": (min_offset_secs as f64) / 3600.0,
        "hard_blocks_required": hard_blocks,
        "floor_blocks_mined_with_real_pow": if mine_floor_pow { floor_blocks } else { 0 },
        "new_anchor_height": base_epoch_height + 6,
        "new_anchor_timestamp": headers[MEDIAN_TIME_BLOCKS + 5].timestamp,
        "new_anchor_target_is_genesis_floor": headers[MEDIAN_TIME_BLOCKS + 5].difficulty_target == GENESIS_TARGET,
        "mtp_window_clean_by_height": base_epoch_height + 17,
        "sixth_shifted_block_forces_mtp_future_drift_violation": true,
        "tip_height": tip.height,
        "tip_drift_valid_at_present_clock": true,
        "floor_block_work": le256_to_u128_low(&floor_unit_work),
        "beats_base_checkpoint_work": work_gt(&cum_work, &base_work),
        "beats_one_honest_hard_block_work": work_gt(&cum_work, &one_hard_work),
        "beats_two_honest_hard_blocks_work": work_gt(&cum_work, &two_hard_work),
    })
}

fn main() -> Result<()> {
    assert!(!cfg!(debug_assertions), "use --profile fast or --release");
    let h210536 = verify_mainnet_h210536_origin()?;
    let h210536_work = block_work(&h210536.difficulty_target);
    let genesis_work = block_work(&GENESIS_TARGET);
    let work_ratio = le256_to_u128_low(&h210536_work) as f64 / le256_to_u128_low(&genesis_work) as f64;

    let mainnet_v2_case = run_epoch_anchor_reset_test(
        "mainnet_h210536_target_at_v2_epoch_210540",
        210_540,
        h210536.timestamp + 120,
        h210536.difficulty_target,
        true,
    );

    let extreme_v21_case = run_epoch_anchor_reset_test(
        "extreme_min_target_at_v2_1_epoch_279504",
        279_504,
        1_795_000_000,
        MIN_TARGET,
        true,
    );

    let report = json!({
        "pinned_commit": PINNED_COMMIT,
        "debug_assertions": cfg!(debug_assertions),
        "constants": {
            "epoch_length": EPOCH_LENGTH,
            "median_time_blocks": MEDIAN_TIME_BLOCKS,
            "max_shifted_in_mtp_window": MEDIAN_TIME_BLOCKS / 2,
            "interior_blocks_per_asert_epoch": EPOCH_LENGTH - 1,
            "epoch_interior_equals_mtp_minority_cap": (EPOCH_LENGTH - 1) as usize == (MEDIAN_TIME_BLOCKS / 2),
            "v2_block_time_secs": V2_BLOCK_TIME,
            "v2_halflife_secs": EPOCH_LENGTH * V2_BLOCK_TIME,
            "max_future_drift_secs": MAX_FUTURE_DRIFT,
        },
        "verified_mainnet_h210536_origin": {
            "height": h210536.height,
            "block_hash": hex::encode(block_id(&h210536)),
            "timestamp": h210536.timestamp,
            "difficulty_target_hex": hex::encode(h210536.difficulty_target),
            "leading_zero_bits": target_leading_zero_bits(&h210536.difficulty_target),
            "block_work_u128": le256_to_u128_low(&h210536_work),
            "genesis_floor_work_u128": le256_to_u128_low(&genesis_work),
            "difficulty_ratio_vs_genesis_floor": work_ratio,
            "active_slot_count": h210536.active_slot_count,
            "alloc_counter": h210536.alloc_counter,
            "retirement_certificate_cryptographically_verified": true,
        },
        "asert_mtp_anchor_reset_cases": [
            mainnet_v2_case,
            extreme_v21_case,
        ],
    });

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
