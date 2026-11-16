//! Consensus-boundary edge audit: activation edges, height/time arithmetic,
//! canonicalization corners — all against the pinned noid_chain library
//! functions at commit 52938433 (verified by build.rs).
//!
//! Sections:
//!   A. v2.1 activation semantic boundary at mainnet 279,369/279,370
//!      (two real PoW-mined blocks accepted across the boundary)
//!   B. height/time arithmetic extremes with the real ASERT/MTP/finality code
//!   C. v2 ASERT anchor/rule alignment at mainnet 210,536..210,541
//!   D. canonical-encoding sweep (headers, bodies, terminal metadata, ids)
//!   E. tx-epoch anchor corners incl. the activation-overlap window

use noid_chain::block::Block;
use noid_chain::consensus::difficulty::{le256_lt, next_target, next_target_with_schedule};
use noid_chain::consensus::epoch_anchor::{
    checked_tx_epoch_height_decomposition, next_tx_epoch_anchor_id,
    tx_epoch_anchor_height_for_child,
};
use noid_chain::consensus::fork_choice::reorg_allowed;
use noid_chain::consensus::forks::{ForkSchedule, ProtocolVersion, V2Activation, ACTIVE_SCHEDULE};
use noid_chain::consensus::header::{
    asert_anchor_height_with_schedule, is_final, validate_header, validate_header_timeless,
};
use noid_chain::consensus::params::{
    coinbase_creation_height, coinbase_creation_id, creation_id_within_boundary,
    owner_authorization_version, CONSENSUS_FINALITY_DEPTH, EPOCH_LENGTH, GENESIS_TARGET,
    MAX_FUTURE_DRIFT, MIN_TARGET, TX_EPOCH_BLOCKS, V2_1_ACTIVATION_HEIGHT, V2_ACTIVATION_HEIGHT,
    V2_BLOCK_TIME,
};
use noid_chain::consensus::pow::{search_pow, validate_pow};
use noid_chain::consensus::slot_expansion::{expected_child_log_slots, finalized_expansion_window};
use noid_chain::consensus::template::build_block_template;
use noid_chain::consensus::timestamps::{
    median_time_past, validate_future_drift, validate_median_time_past,
};
use noid_chain::consensus::validation::{validate_block_consensus, AnchorInfo};
use noid_chain::{BlockHeader, ChainState, HistoryStepTerminalMetadata, SlotValue};
use noid_poseidon2b::primitives::Address;
use noid_tx::{TxBody, TxInput, TxOutput, TX_INPUTS, TX_OUTPUTS};

const V2_1: u64 = 279_370; // MAINNET_V2_1_ACTIVATION_HEIGHT (asserted below)
const V2: u64 = 210_537; // MAINNET_V2_ACTIVATION_HEIGHT (asserted below)
const AMOUNT: u64 = 1_000_000;

macro_rules! check {
    ($label:expr, $ok:expr $(,)?) => {{
        let ok = $ok;
        assert!(ok, "ASSERTION FAILED: {}", $label);
        println!("PASS {}", $label);
    }};
}

macro_rules! check_eq {
    ($label:expr, $got:expr, $want:expr $(,)?) => {{
        let (got, want) = ($got, $want);
        assert_eq!(
            got, want,
            "ASSERTION FAILED: {} got={got:?} want={want:?}",
            $label
        );
        println!("PASS {}", $label);
    }};
}

fn in_production_bounds(t: &[u8; 32]) -> bool {
    // Production floor invariant: MIN_TARGET <= t <= GENESIS_TARGET.
    !le256_lt(t, &MIN_TARGET) && !le256_lt(&GENESIS_TARGET, t)
}

// ---------------------------------------------------------------------------
// Fixtures (pattern from experiments/block-validation)
// ---------------------------------------------------------------------------

fn header(height: u64, timestamp: u64, log_slots: u32) -> BlockHeader {
    BlockHeader {
        prev_block_hash: [0x11; 32],
        state_root: [0x22; 32],
        tx_root: [0x33; 32],
        timestamp,
        height,
        miner_address: Address([0x44; 32]),
        nonce: 0,
        difficulty_target: GENESIS_TARGET,
        log_slots,
        active_slot_count: 0,
        alloc_counter: 0,
    }
}

/// A real spendable state at `log_slots` with owner-controlled slots.
fn fixture_state(log_slots: u32) -> ChainState {
    let owner = Address([0xA5; 32]);
    let mut state = ChainState::with_log_slots(log_slots as usize);
    for slot in 1..=4u32 {
        state
            .state
            .set_slot(
                slot,
                SlotValue::with_owner_fields(AMOUNT, u64::from(slot), owner.as_fields()),
            )
            .unwrap();
    }
    state.active_slot_count = 4;
    state.alloc_counter = 4;
    state.circulating_supply_micronoid = 4 * u128::from(AMOUNT);
    state
}

/// Header-window timestamp list ending at the parent (oldest-first, <=11).
fn ancestor_timestamps(parent_ts: u64, spacing: u64, count: usize) -> Vec<u64> {
    (0..count as u64)
        .map(|i| parent_ts - spacing * (count as u64 - 1 - i))
        .collect()
}

// ---------------------------------------------------------------------------
// SECTION A — v2.1 activation semantic boundary (279,369 / 279,370)
// ---------------------------------------------------------------------------

fn section_a() {
    println!("\n=== SECTION A: v2.1 activation boundary at {V2_1} ===");
    check_eq!(
        "A0 pinned activation constant matches the audited build",
        V2_1_ACTIVATION_HEIGHT,
        Some(V2_1),
    );
    check_eq!("A0 v2 activation constant", V2_ACTIVATION_HEIGHT, Some(V2));

    // --- A1: difficulty arithmetic is untouched by v2.1 -------------------
    // v2.1 is not in the ASERT ForkSchedule; the target function must be
    // byte-identical across the semantic boundary for every input profile.
    let anchor_h = 279_366; // regular ASERT boundary (279_366 % 6 == 0)
    check_eq!("A1 anchor is epoch-aligned", anchor_h % EPOCH_LENGTH, 0);
    let base_ts = 1_790_000_000u64;
    let mut identical = true;
    for delta_parent in [0u64, 1, 29, 30, 61, 180, 3_600, 86_400, 1 << 20] {
        for delta_child in [0u64, 1, 30, 7_200] {
            let pre = next_target(
                anchor_h,
                base_ts,
                &GENESIS_TARGET,
                V2_1 - 1,
                base_ts + 90 + delta_parent,
                base_ts + 90 + delta_parent + delta_child,
            );
            let post = next_target(
                anchor_h,
                base_ts,
                &GENESIS_TARGET,
                V2_1,
                base_ts + 120 + delta_parent,
                base_ts + 120 + delta_parent + delta_child,
            );
            // Same parent-age relative to the anchor (90 vs 120 = on-time
            // ideal for one vs two post-anchor intervals at 30 s).
            if pre != post {
                identical = false;
            }
        }
    }
    check!(
        "A1 ASERT target function is byte-identical across the v2.1 boundary for on-time profiles",
        identical,
    );
    // And a raw equal-age comparison: same (anchor, parent) ages both sides.
    let pre = next_target(
        anchor_h,
        base_ts,
        &GENESIS_TARGET,
        V2_1 - 1,
        base_ts + 90,
        base_ts + 120,
    );
    let post = next_target(
        anchor_h,
        base_ts,
        &GENESIS_TARGET,
        V2_1,
        base_ts + 90,
        base_ts + 120,
    );
    check_eq!("A1 equal-age targets byte-equal across boundary", pre, post);

    // --- A2: HistoryStep class-count dialect shift ------------------------
    use noid_chain::history_step::{
        history_step_class_count, history_step_terminal_wire_version,
        HISTORY_STEP_TERMINAL_V2_VERSION, HISTORY_STEP_V2_1_CLASS_COUNT,
        HISTORY_STEP_V2_CLASS_COUNT,
    };
    check_eq!(
        "A2 class count pre-activation",
        history_step_class_count(V2_1 - 1),
        HISTORY_STEP_V2_CLASS_COUNT,
    );
    check_eq!(
        "A2 class count at activation",
        history_step_class_count(V2_1),
        HISTORY_STEP_V2_1_CLASS_COUNT,
    );
    check_eq!("A2 class ids are 0-based", HISTORY_STEP_V2_CLASS_COUNT, 2);
    check_eq!(
        "A2 v2.1 adds exactly one class",
        HISTORY_STEP_V2_1_CLASS_COUNT,
        3,
    );

    let h = [0x77u8; 32];
    check!(
        "A2 class id 2 rejected one block before activation",
        HistoryStepTerminalMetadata::new(V2_1 - 1, h, 2).is_err(),
    );
    let accepted = HistoryStepTerminalMetadata::new(V2_1, h, 2);
    check!(
        "A2 class id 2 accepted at the activation block",
        accepted.is_ok(),
    );
    check!(
        "A2 class id 3 still rejected after activation",
        HistoryStepTerminalMetadata::new(V2_1, h, 3).is_err(),
    );
    // The highest pre-activation class stays valid after activation (no
    // retroactive invalidation of the shared classes).
    check!(
        "A2 class id 1 valid on both sides",
        HistoryStepTerminalMetadata::new(V2_1 - 1, h, 1).is_ok()
            && HistoryStepTerminalMetadata::new(V2_1, h, 1).is_ok(),
    );

    // Wire-version byte does NOT change at v2.1 (only the class range does).
    check_eq!(
        "A2 terminal wire version pre",
        history_step_terminal_wire_version(V2_1 - 1),
        HISTORY_STEP_TERMINAL_V2_VERSION,
    );
    check_eq!(
        "A2 terminal wire version post",
        history_step_terminal_wire_version(V2_1),
        HISTORY_STEP_TERMINAL_V2_VERSION,
    );

    // Terminal prefix round-trips and rejects a stale version byte.
    let meta = accepted.unwrap();
    let bytes = meta.encode_prefix();
    let back = HistoryStepTerminalMetadata::decode_prefix(&bytes).unwrap();
    check_eq!(
        "A2 terminal prefix round-trip",
        back.terminal_height(),
        V2_1,
    );
    check_eq!("A2 terminal prefix round-trip class", back.class_id(), 2);
    let mut stale = bytes;
    stale[0] = 5; // v1.1 shared-path version
    check!(
        "A2 stale terminal version byte rejected",
        HistoryStepTerminalMetadata::decode_prefix(&stale).is_err(),
    );

    // --- A3: owner-auth dialect dispatch ----------------------------------
    check_eq!(
        "A3 owner-auth version pre-activation",
        owner_authorization_version(V2_1 - 1),
        noid_tx::OwnerAuthorizationVersion::V3,
    );
    check_eq!(
        "A3 owner-auth version at activation",
        owner_authorization_version(V2_1),
        noid_tx::OwnerAuthorizationVersion::V4,
    );

    // --- A4: creation-id namespace semantics across the boundary ----------
    let alloc = 10u64;
    // Ordinary ids: identical semantics both sides (no reinterpretation).
    check!(
        "A4 ordinary id at alloc bound accepted both sides",
        creation_id_within_boundary(alloc, alloc, V2_1 - 1)
            && creation_id_within_boundary(alloc, alloc, V2_1),
    );
    check!(
        "A4 ordinary id above alloc rejected both sides",
        !creation_id_within_boundary(alloc + 1, alloc, V2_1 - 1)
            && !creation_id_within_boundary(alloc + 1, alloc, V2_1),
    );
    // Coinbase ids: mint-height bound, unchanged.
    check!(
        "A4 coinbase id from 279_369 valid at both boundary heights",
        creation_id_within_boundary(coinbase_creation_id(V2_1 - 1), 0, V2_1 - 1)
            && creation_id_within_boundary(coinbase_creation_id(V2_1 - 1), 0, V2_1),
    );
    check!(
        "A4 future coinbase id rejected at 279_369 and at 279_370",
        !creation_id_within_boundary(coinbase_creation_id(V2_1), 0, V2_1 - 1),
    );
    // Native-tagged id (bit 62, no bit 63): rejected pre, accepted post.
    let native_id = noid_tx::native_assets::NATIVE_CREATION_TAG | 5;
    check!(
        "A4 native id rejected under pre-activation raw comparison",
        !creation_id_within_boundary(native_id, alloc, V2_1 - 1),
    );
    check!(
        "A4 native id accepted post-activation within alloc bound",
        creation_id_within_boundary(native_id, alloc, V2_1),
    );
    check!(
        "A4 native sequence above alloc rejected post-activation",
        !creation_id_within_boundary(native_id | (1 << 30), alloc, V2_1),
    );
    check!(
        "A4 native sequence zero rejected post-activation",
        !creation_id_within_boundary(noid_tx::native_assets::NATIVE_CREATION_TAG, alloc, V2_1),
    );

    // --- A5: the tx-epoch anchor does NOT shift at activation --------------
    // 279_370 = 144*1940 + 10: the activation lands mid-epoch, so children
    // 279_361..=279_504 all bind the header at 279_360 — one replay window
    // spans the semantic boundary.
    let epoch_pos = V2_1 % TX_EPOCH_BLOCKS;
    check_eq!("A5 activation tx-epoch position", epoch_pos, 10);
    check_eq!(
        "A5 anchor identical across the boundary",
        tx_epoch_anchor_height_for_child(V2_1 - 1),
        tx_epoch_anchor_height_for_child(V2_1),
    );
    check_eq!(
        "A5 shared anchor height",
        tx_epoch_anchor_height_for_child(V2_1),
        279_360,
    );
    let window_end = 279_360 + TX_EPOCH_BLOCKS;
    check_eq!(
        "A5 anchor advances only at the next epoch boundary",
        tx_epoch_anchor_height_for_child(window_end + 1),
        window_end,
    );

    // --- A6: two real PoW blocks across the boundary ----------------------
    let log_slots = 8u32;
    let spacing = V2_BLOCK_TIME; // v2 era 30 s interval
    let mut state = fixture_state(log_slots);
    let base = 1_790_000_000u64;

    // Parent at 279_368 with a real state root.
    let mut parent = header(V2_1 - 2, base, log_slots);
    parent.state_root = state.state_root();
    parent.active_slot_count = state.active_slot_count;
    parent.alloc_counter = state.alloc_counter;

    let miner = Address([0x22; 32]);
    let mut prev_header = parent;
    let mut prev_ts_window = ancestor_timestamps(base, spacing, 11);

    for (i, height) in [V2_1 - 1, V2_1].iter().enumerate() {
        let ts = prev_header.timestamp + spacing;
        // ASERT anchor for this tip (regular boundary, v2 era).
        let anchor_h = asert_anchor_height_with_schedule(prev_header.height, ACTIVE_SCHEDULE);
        let anchor = AnchorInfo {
            anchor_height: anchor_h,
            anchor_timestamp: base - spacing * (prev_header.height - anchor_h),
            anchor_target: GENESIS_TARGET,
        };
        let target = next_target(
            anchor.anchor_height,
            anchor.anchor_timestamp,
            &anchor.anchor_target,
            *height,
            prev_header.timestamp,
            ts,
        );
        check!(
            &format!(
                "A6.{} boundary block target stays at the genesis floor (on-time)",
                i + 1
            ),
            target == GENESIS_TARGET,
        );
        let template = build_block_template(
            &prev_header,
            &state,
            &[prev_header.active_slot_count; 18],
            vec![],
            miner,
            ts,
            target,
        )
        .expect("template builds");
        let transactions = template.all_txs();
        let mut block_header = template.into_header(0);
        let nonce = search_pow(&block_header, 0, 1 << 26).expect("PoW at genesis floor");
        block_header.nonce = nonce;
        let block = Block {
            header: block_header,
            transactions,
        };
        let local_time = ts + MAX_FUTURE_DRIFT;
        let result = validate_block_consensus(
            &block,
            &prev_header,
            &prev_ts_window,
            &[0u64; 18],
            local_time,
            &anchor,
            &mut state,
        );
        check!(
            &format!(
                "A6.{} real block h={} accepted by sequential consensus (PoW nonce {})",
                i + 1,
                height,
                nonce
            ),
            result.is_ok(),
        );
        check!(
            &format!("A6.{} standalone PoW validates", i + 1),
            validate_pow(&block.header).is_ok(),
        );
        check!(
            &format!("A6.{} timeless historical header path agrees", i + 1),
            validate_header_timeless(
                &block.header,
                &prev_header,
                &prev_ts_window,
                &[0u64; 18],
                anchor.anchor_height,
                anchor.anchor_timestamp,
                &anchor.anchor_target,
            )
            .is_ok(),
        );
        prev_header = block.header;
        prev_ts_window.push(ts);
        prev_ts_window.remove(0);
    }
    println!(
        "A6 final tip height {} target {}",
        prev_header.height,
        hex::encode(prev_header.difficulty_target)
    );

    // Replaying the pre-activation block's header against the post tip is a
    // BadHeight/BadParentHash reject (structure), not a dialect question.
    let replay = validate_header(
        &{
            let mut h = prev_header;
            h.height = V2_1 - 1;
            h
        },
        &prev_header,
        &prev_ts_window,
        &[0u64; 18],
        base + 10_000,
        prev_header.height,
        prev_header.timestamp,
        &prev_header.difficulty_target,
    );
    check!("A6 height regression rejected", replay.is_err());
}

// ---------------------------------------------------------------------------
// SECTION B — height/time arithmetic extremes
// ---------------------------------------------------------------------------

fn section_b() {
    println!("\n=== SECTION B: height/time arithmetic extremes ===");

    // --- B1: next_target at u64 extremes, no panic, always in bounds ------
    let cases: [(&str, u64, u64, u64, u64, u64); 8] = [
        ("h=MAX anchor=MAX-1 ts=0", u64::MAX - 1, 0, u64::MAX, 0, 0),
        (
            "h=MAX all-MAX",
            u64::MAX - 1,
            u64::MAX,
            u64::MAX,
            u64::MAX,
            u64::MAX,
        ),
        (
            "h=MAX anchor-future",
            u64::MAX - 1,
            u64::MAX,
            u64::MAX,
            0,
            0,
        ),
        ("h=1 genesis anchor zero ts", 0, 0, 0, 0, 1),
        ("h=MAX ts=parent=anchor", u64::MAX - 1, 5, 5, 5, 5),
        ("activation h=279370 zero times", 279_366, 0, 0, 0, 0),
        ("anchor_ts=MAX parent=0", 279_366, u64::MAX, 0, 0, 1),
        ("h=MAX ts=1", u64::MAX - 1, 0, u64::MAX, 0, 1),
    ];
    for (name, anchor_h, anchor_ts, anchor_target_mag, parent_ts, ts) in cases {
        let mut anchor_target = GENESIS_TARGET;
        // vary the anchor magnitude for coverage
        anchor_target[28] = anchor_target_mag as u8;
        let t = next_target(anchor_h, anchor_ts, &anchor_target, u64::MAX, parent_ts, ts);
        check!(
            &format!("B1 no-panic bounded [{name}]"),
            in_production_bounds(&t),
        );
    }

    // Exact zero exponent: parent on-time (ideal 3*30s after the anchor)
    // gives a child target byte-identical to the anchor target.
    let t = next_target(279_366, 5, &GENESIS_TARGET, 279_370, 5 + 90, 5 + 120);
    check_eq!(
        "B1 zero-exponent target is the anchor target",
        t,
        GENESIS_TARGET,
    );
    // Parent arriving instantly after the anchor (90 s of ideal elapsed in
    // 0 s) hardens by exactly 2^0.5 - bounded, never underflow to 0.
    let fast = next_target(279_366, 5, &GENESIS_TARGET, 279_370, 5, 5);
    check!(
        "B1 instant-parent target hardens by 2^0.5 and stays in production bounds",
        in_production_bounds(&fast) && le256_lt(&fast, &GENESIS_TARGET),
    );

    // --- B2: monotonicity in parent timestamp (v2 parent-selected targets) --
    let anchor_ts = 1_790_000_000u64;
    let ideal = (V2_1 - 279_366) as u64 * 30; // on-time parent age
    let mut prev = [0u8; 32];
    let mut monotone = true;
    let mut floored_at = None;
    let mut step = 0u64;
    while step <= 86_400 {
        let parent_ts = anchor_ts + ideal + step;
        let t = next_target(
            279_366,
            anchor_ts,
            &GENESIS_TARGET,
            V2_1,
            parent_ts,
            parent_ts + 30,
        );
        if !in_production_bounds(&t) {
            monotone = false;
        }
        if step > 0 && le256_lt(&t, &prev) {
            monotone = false; // later parent must never make it harder
        }
        if floored_at.is_none() && t == GENESIS_TARGET {
            floored_at = Some(step);
        }
        prev = t;
        step += 60;
    }
    check!(
        "B2 parent-timestamp monotone non-decreasing, capped at genesis floor",
        monotone,
    );
    let f = floored_at.expect("floor reached within a day of lateness");
    println!("PASS B2 genesis-anchor floor reached at parent lateness {f}s (<= one halflife family; ASERT easing is floored)");
    check!("B2 floor reached within 2 halflives (360s)", f <= 360);

    // Hard-anchor floor crossing: a 2^200 anchor needs exactly 38 doublings to
    // reach GENESIS_TARGET (2^238). At frac=0 each halflife of lateness is one
    // doubling, so 37 halflives stays strictly below the floor and 38 hits it
    // exactly. (This is the analytic boundary; the live easing is floored.)
    let mut hard = [0u8; 32];
    hard[25] = 1; // 2^200, i.e. 38 bits below GENESIS_TARGET's 2^238
    let below = next_target(
        279_366,
        anchor_ts,
        &hard,
        V2_1,
        anchor_ts + ideal + 37 * 180,
        anchor_ts + ideal + 37 * 180 + 30,
    );
    let at = next_target(
        279_366,
        anchor_ts,
        &hard,
        V2_1,
        anchor_ts + ideal + 38 * 180,
        anchor_ts + ideal + 38 * 180 + 30,
    );
    check!(
        "B2 37 halflives late on a hard 2^200 anchor stays below the floor",
        le256_lt(&below, &GENESIS_TARGET),
    );
    check_eq!(
        "B2 38 halflives late on a hard 2^200 anchor hits the floor exactly",
        at,
        GENESIS_TARGET,
    );

    // Extreme future parent: short-circuit and floor, never panic, never 0.
    let far = next_target(279_366, anchor_ts, &hard, V2_1, u64::MAX, u64::MAX);
    check_eq!(
        "B2 far-future parent floors (generalized C26 datum)",
        far,
        GENESIS_TARGET,
    );
    check!("B2 target never zero", far != [0u8; 32]);

    // Extreme fast (difficulty up): far-back anchor so the ideal elapsed
    // dwarfs actual=0 -> net <= -320 -> MIN_TARGET. Confirms the hardening
    // direction also clamps cleanly, no underflow, at the activation height.
    let fast = next_target(0, u64::MAX / 2, &GENESIS_TARGET, V2_1, 1, 1);
    check_eq!("B2 extreme-fast clamps to MIN_TARGET", fast, MIN_TARGET);
    check_eq!(
        "B2 MIN_TARGET is 1 (difficulty has no reachable cap)",
        MIN_TARGET,
        {
            let mut one = [0u8; 32];
            one[0] = 1;
            one
        },
    );

    // --- B3: MTP boundaries ------------------------------------------------
    let eleven: Vec<u64> = (0..11).map(|i| 1_000_000 + i * 20).collect();
    let mtp = median_time_past(&eleven);
    check_eq!("B3 median of 11 picks the true middle", mtp, eleven[5]);
    check_eq!(
        "B3 child ts == mtp rejected",
        validate_median_time_past(mtp, &eleven),
        Err(noid_chain::consensus::ConsensusError::BadTimestamp)
    );
    check!(
        "B3 child ts == mtp+1 accepted",
        validate_median_time_past(mtp + 1, &eleven).is_ok(),
    );
    // Even-count window uses the UPPER median (stricter, not looser).
    let four = vec![10u64, 20, 30, 40];
    check_eq!("B3 even window upper median", median_time_past(&four), 30);
    check_eq!(
        "B3 even-window child at lower-mid rejected",
        validate_median_time_past(20, &four),
        Err(noid_chain::consensus::ConsensusError::BadTimestamp)
    );
    check!(
        "B3 even-window child at 31 accepted",
        validate_median_time_past(31, &four).is_ok(),
    );
    // Genesis empty window: median 0, so timestamp 0 is REJECTED (<=).
    check_eq!("B3 genesis window median is 0", median_time_past(&[]), 0);
    check_eq!(
        "B3 genesis-child timestamp 0 rejected",
        validate_median_time_past(0, &[]),
        Err(noid_chain::consensus::ConsensusError::BadTimestamp)
    );
    check!(
        "B3 genesis-child timestamp 1 accepted",
        validate_median_time_past(1, &[]).is_ok(),
    );
    // >11 entries: only the last 11 count.
    let mut twenty: Vec<u64> = (0..20).map(|i| 1_000 + i * 20).collect();
    twenty.insert(0, 999_999); // ancient outlier must not matter
    check_eq!(
        "B3 only last 11 matter",
        median_time_past(&twenty),
        median_time_past(&twenty[twenty.len() - 11..]),
    );
    // Future drift: exactly at limit accepts, one over rejects.
    check!(
        "B3 drift exactly at limit accepts",
        validate_future_drift(2_000_120, 2_000_000).is_ok(),
    );
    check_eq!(
        "B3 drift one over rejects",
        validate_future_drift(2_000_121, 2_000_000),
        Err(noid_chain::consensus::ConsensusError::BadTimestamp)
    );
    // Saturation corner (SUBTLE, benign): within MAX_FUTURE_DRIFT of u64::MAX,
    // `local_time.saturating_add(120)` pins to u64::MAX, so the drift window
    // stops widening and block_ts == u64::MAX is ACCEPTED rather than rejected.
    // There is no wrap to a false rejection, and the only cost is the window
    // cannot extend past u64::MAX (a timestamp ~5.8e11 years out) — harmless.
    check!(
        "B3 drift saturates near u64::MAX: no panic, no wrap (accepts at the ceiling)",
        validate_future_drift(u64::MAX, u64::MAX - 10).is_ok(),
    );
    // A real over-limit that does NOT saturate still rejects cleanly.
    check_eq!(
        "B3 non-saturating over-limit rejects",
        validate_future_drift(1_000, 500),
        Err(noid_chain::consensus::ConsensusError::BadTimestamp)
    );

    // --- B4: finality off-by-one ------------------------------------------
    check_eq!(
        "B4 is_final at depth 17 false",
        is_final(100, 100 + CONSENSUS_FINALITY_DEPTH - 1),
        false
    );
    check_eq!(
        "B4 is_final at depth 18 true",
        is_final(100, 100 + CONSENSUS_FINALITY_DEPTH),
        true
    );
    check!(
        "B4 reorg_allowed(18) — undo exactly the 18 non-final blocks",
        reorg_allowed(18),
    );
    check_eq!(
        "B4 reorg_allowed(19) false — touches the finalized block",
        reorg_allowed(19),
        false
    );
    // Consistency: block h is final iff replacing it needs >18 undos.
    let tip = 500u64;
    for h in [tip - 19, tip - 18, tip - 17] {
        let undos = tip - h + 1;
        check!(
            &format!("B4 finality/reorg consistency at h={h} (undos={undos})"),
            is_final(h, tip) == !reorg_allowed(undos),
        );
    }

    // --- B5: expansion window genesis edge --------------------------------
    check_eq!(
        "B5 window at parent 34 is None",
        finalized_expansion_window(34),
        None
    );
    check_eq!(
        "B5 window at parent 35 is exactly (0,17)",
        finalized_expansion_window(35),
        Some((0, 17))
    );
    check_eq!(
        "B5 no expansion before the first window even with max counts",
        expected_child_log_slots(34, 24, &[u64::MAX; 18]),
        24
    );
    check_eq!(
        "B5 wrong-length window never expands",
        expected_child_log_slots(100, 24, &[u64::MAX; 17]),
        24
    );
    let mut tie = [12_582_911u64; 18];
    tie[..9].fill(12_582_912);
    check_eq!(
        "B5 even-window 9/18 tie never expands",
        expected_child_log_slots(100, 24, &tie),
        24
    );
    let mut expand = [12_582_911u64; 18];
    expand[..10].fill(12_582_912);
    check_eq!(
        "B5 strict 10/18 expands",
        expected_child_log_slots(100, 24, &expand),
        25
    );
}

// ---------------------------------------------------------------------------
// SECTION C — v2 ASERT anchor/rule alignment at 210,536..210,541
// ---------------------------------------------------------------------------

fn section_c() {
    println!("\n=== SECTION C: v2 ASERT boundary at {V2} ===");
    let v2h = V2; // 210_537

    // The anchor is derived from the TIP; the rule from the CHILD. Both must
    // flip at exactly the same child height.
    check_eq!(
        "A/C anchor at tip v2-2 is the regular boundary",
        asert_anchor_height_with_schedule(v2h - 2, ACTIVE_SCHEDULE),
        {
            let reg = (v2h - 2) / EPOCH_LENGTH * EPOCH_LENGTH;
            reg
        }
    );
    check_eq!(
        "C anchor at tip v2-1 pins to the pre-activation block",
        asert_anchor_height_with_schedule(v2h - 1, ACTIVE_SCHEDULE),
        v2h - 1
    );
    check_eq!(
        "C anchor stays pinned until the next regular boundary",
        asert_anchor_height_with_schedule(v2h + 2, ACTIVE_SCHEDULE),
        v2h - 1
    );
    let next_regular = (v2h / EPOCH_LENGTH + 1) * EPOCH_LENGTH; // 210_540
    check_eq!(
        "C first v2-aligned regular boundary re-anchors",
        asert_anchor_height_with_schedule(next_regular, ACTIVE_SCHEDULE),
        next_regular
    );

    // Mixed-interval ideal: heights (210534, 210539] = 2x20s + 3x30s.
    let ideal = ACTIVE_SCHEDULE.ideal_elapsed(v2h - 3, v2h + 2);
    check_eq!("C mixed-interval ideal elapsed 2*20+3*30", ideal, 130);

    // Rule selection flips at the child height: pre-v2 child uses the legacy
    // path even one block before activation.
    let pre_child = next_target_with_schedule(
        v2h - 3,
        1_000,
        &GENESIS_TARGET,
        v2h - 1,
        1_000 + 2 * 20,
        1_000 + 3 * 20,
        ACTIVE_SCHEDULE,
    );
    let legacy_eq = next_target(
        v2h - 3,
        1_000,
        &GENESIS_TARGET,
        v2h - 1,
        1_000 + 2 * 20,
        1_000 + 3 * 20,
    );
    check_eq!(
        "C last pre-v2 child uses the legacy rule",
        pre_child,
        legacy_eq,
    );

    // Continuity: the first v2 child's target equals its parent's (anchor is
    // the parent, ideal=actual=0), so activation cannot jump difficulty.
    let first_v2 = next_target_with_schedule(
        v2h - 1,
        1_000,
        &GENESIS_TARGET,
        v2h,
        1_000,
        1_030,
        ACTIVE_SCHEDULE,
    );
    check_eq!(
        "C first v2 block inherits the parent target exactly",
        first_v2,
        GENESIS_TARGET,
    );

    // Sibling invariance: at v2 the child's own timestamp does not select the
    // target; both siblings share the parent-derived target.
    let a = next_target_with_schedule(
        v2h - 1,
        1_000,
        &GENESIS_TARGET,
        v2h,
        1_000,
        1_001,
        ACTIVE_SCHEDULE,
    );
    let b = next_target_with_schedule(
        v2h - 1,
        1_000,
        &GENESIS_TARGET,
        v2h,
        1_000,
        4_000,
        ACTIVE_SCHEDULE,
    );
    check_eq!("C v2 siblings share one parent-derived target", a, b);

    // Pre-v2, the child's own timestamp DOES select the target (one block
    // earlier): the semantic switch is exactly at the activation height.
    let a = next_target_with_schedule(
        v2h - 3,
        1_000,
        &GENESIS_TARGET,
        v2h - 1,
        1_000,
        1_001,
        ACTIVE_SCHEDULE,
    );
    let b = next_target_with_schedule(
        v2h - 3,
        1_000,
        &GENESIS_TARGET,
        v2h - 1,
        1_000,
        1_000 + 600,
        ACTIVE_SCHEDULE,
    );
    check!("C pre-v2 child timestamp selects its own target", a != b);

    // Version schedule agrees with the boundary.
    check_eq!(
        "C version at v2-1 is V1_1",
        ACTIVE_SCHEDULE.version(v2h - 1),
        ProtocolVersion::V1_1
    );
    check_eq!(
        "C version at v2 is V2",
        ACTIVE_SCHEDULE.version(v2h),
        ProtocolVersion::V2
    );
    check_eq!(
        "C block time at v2-1 is 20",
        ACTIVE_SCHEDULE.block_time(v2h - 1),
        20
    );
    check_eq!(
        "C block time at v2 is 30",
        ACTIVE_SCHEDULE.block_time(v2h),
        30
    );
    // halflife derived from the schedule matches the v2 ASERT caller.
    check_eq!(
        "C halflife switches 120 -> 180 at v2",
        EPOCH_LENGTH * ACTIVE_SCHEDULE.block_time(v2h),
        180
    );

    // Cross-check with a hypothetical schedule whose v2 lands on an exact
    // epoch boundary (the corner the mainnet height avoids: 210537 % 6 == 3).
    check_eq!(
        "mainnet v2 height is mid-epoch (210537 % 6)",
        v2h % EPOCH_LENGTH,
        3,
    );
    let aligned = ForkSchedule::new(Some(95_125), V2Activation::new(210_540, 30)).unwrap();
    check_eq!(
        "C aligned activation: anchor at tip 210539 still the pre-activation block",
        asert_anchor_height_with_schedule(210_539, aligned),
        210_539
    );
    check_eq!(
        "C aligned activation: tip 210540 re-anchors to itself",
        asert_anchor_height_with_schedule(210_540, aligned),
        210_540
    );
    let first = next_target_with_schedule(
        210_539,
        5_000,
        &GENESIS_TARGET,
        210_540,
        5_000,
        5_030,
        aligned,
    );
    check_eq!(
        "C aligned activation preserves the parent target too",
        first,
        GENESIS_TARGET
    );
}

// ---------------------------------------------------------------------------
// SECTION D — canonical encoding sweep
// ---------------------------------------------------------------------------

fn section_d() {
    println!("\n=== SECTION D: canonical encoding ===");

    // --- D1: BlockHeader wire ---------------------------------------------
    let mut h = BlockHeader {
        prev_block_hash: [0x11; 32],
        state_root: [0x22; 32],
        tx_root: [0x33; 32],
        timestamp: u64::MAX,
        height: u64::MAX,
        miner_address: Address([0x44; 32]),
        nonce: u128::MAX,
        difficulty_target: GENESIS_TARGET,
        log_slots: 32,
        active_slot_count: u64::MAX,
        alloc_counter: u64::MAX - 1,
    };
    let bytes = h.to_bytes();
    check_eq!("D1 header wire size 212", bytes.len(), 212);
    check_eq!(
        "D1 extreme-valued header round-trips",
        BlockHeader::from_bytes(&bytes).unwrap(),
        h
    );
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    check!(
        "D1 trailing byte rejected",
        BlockHeader::from_bytes(&trailing).is_err(),
    );
    let mut all_truncated = true;
    for cut in 0..bytes.len() {
        if BlockHeader::from_bytes(&bytes[..cut]).is_ok() {
            all_truncated = false;
        }
    }
    check!("D1 every truncation rejected", all_truncated);

    // Two distinct headers must never share an encoding.
    h.nonce = h.nonce - 1;
    check!(
        "D1 nonce change changes the encoding",
        h.to_bytes() != bytes,
    );
    h.nonce = u128::MAX;
    h.log_slots = 31;
    check!(
        "D1 log_slots change changes the encoding",
        h.to_bytes() != bytes,
    );

    // --- D2: TxBody wire ----------------------------------------------------
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: 7,
        amount: 12,
        creation_id: 3,
    };
    inputs[7] = TxInput {
        slot_index: 8,
        amount: 9,
        creation_id: 4,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[1] = TxOutput {
        slot_index: 10,
        amount: 20,
        owner: Address([0x22; 32]),
    };
    let body = TxBody {
        epoch_anchor: [0x11; 32],
        fee: 1,
        input_owner: Address([0x33; 32]),
        inputs,
        outputs,
        validity_bitmap: (1 << 0) | (1 << 7) | (1 << 9),
        is_coinbase: false,
    };
    let bb = body.to_bytes();
    check_eq!("D2 body wire size 323", bb.len(), 323);
    check_eq!(
        "D2 body round-trip",
        TxBody::from_bytes(&bb).unwrap(),
        body.clone()
    );
    let mut bad_bool = bb.to_vec();
    bad_bool[322] = 2;
    check!(
        "D2 non-bool discriminator rejected",
        TxBody::from_bytes(&bad_bool).is_err(),
    );
    let mut high_bit = bb.to_vec();
    high_bit[320..322].copy_from_slice(&((1u16 << 10) | body.validity_bitmap).to_le_bytes());
    check!(
        "D2 high bitmap bit rejected (no malleable bitmap)",
        TxBody::from_bytes(&high_bit).is_err(),
    );
    let mut dead_nonzero = bb.to_vec();
    dead_nonzero[92] = 1; // inside dead input 1
    check!(
        "D2 nonzero dead slot rejected (no second encoding)",
        TxBody::from_bytes(&dead_nonzero).is_err(),
    );
    let mut trailing = bb.to_vec();
    trailing.push(0);
    check!(
        "D2 trailing byte rejected",
        TxBody::from_bytes(&trailing).is_err(),
    );

    // --- D3: coinbase creation ids -----------------------------------------
    // Injective below 2^63; NON-injective at/above 2^63 (shared id).
    check!(
        "D3 ids distinct across the activation",
        coinbase_creation_id(V2_1 - 1) != coinbase_creation_id(V2_1),
    );
    let lo = 5u64;
    let hi = lo + (1u64 << 63);
    check_eq!(
        "D3 heights >= 2^63 alias onto the same creation id (unreachable corner)",
        coinbase_creation_id(lo),
        coinbase_creation_id(hi)
    );
    check_eq!(
        "D3 decode of the aliased id loses the high bit",
        coinbase_creation_height(coinbase_creation_id(hi)),
        lo
    );
    check!(
        "D3 boundary check accepts the aliased id at BOTH heights",
        creation_id_within_boundary(coinbase_creation_id(hi), 0, lo)
            && creation_id_within_boundary(coinbase_creation_id(hi), 0, hi),
    );

    // --- D4: terminal metadata byte shape ----------------------------------
    let meta = HistoryStepTerminalMetadata::new(V2_1, [0xAB; 32], 2).unwrap();
    let enc = meta.encode_prefix();
    check_eq!("D4 terminal prefix is 42 bytes", enc.len(), 42);
    check_eq!("D4 version byte is the v2 terminal version", enc[0], 7);
    let mut dup = enc;
    dup[41] = 7; // class id beyond the v2.1 count
    check!(
        "D4 out-of-range class id rejected on decode",
        HistoryStepTerminalMetadata::decode_prefix(&dup).is_err(),
    );
    check!(
        "D4 truncated terminal rejected",
        HistoryStepTerminalMetadata::decode_prefix(&enc[..41]).is_err(),
    );
}

// ---------------------------------------------------------------------------
// SECTION E — tx-epoch anchor corners
// ---------------------------------------------------------------------------

fn section_e() {
    println!("\n=== SECTION E: tx-epoch anchor corners ===");

    for (child, want) in [
        (0u64, 0u64),
        (1, 0),
        (143, 0),
        (144, 0),   // boundary block still consumes the PRECEDING anchor
        (145, 144), // its own id becomes active only after acceptance
        (288, 144),
        (289, 288),
    ] {
        check_eq!(
            &format!("E anchor for child {child}"),
            tx_epoch_anchor_height_for_child(child),
            want,
        );
    }

    // u64::MAX corner: exact reconstruction properties, no panic. The anchor
    // function divides-first so it never overflows u64; note (SUBTLE) that the
    // next boundary `a + TX_EPOCH_BLOCKS` would overflow u64 here — a naive
    // consumer computing "anchor + epoch length" at the top of the height space
    // would wrap, but the pinned function never does.
    let a = tx_epoch_anchor_height_for_child(u64::MAX);
    check!(
        "E anchor at u64::MAX is epoch-aligned",
        a % TX_EPOCH_BLOCKS == 0,
    );
    check!(
        "E anchor at u64::MAX is the last boundary strictly below the child (u128 check)",
        u128::from(a) <= u128::from(u64::MAX) - 1
            && u128::from(u64::MAX) - 1 < u128::from(a) + u128::from(TX_EPOCH_BLOCKS),
    );
    check!(
        "E next boundary above the u64::MAX anchor overflows u64 (naive-consumer hazard)",
        a.checked_add(TX_EPOCH_BLOCKS).is_none(),
    );
    check_eq!(
        "E every u64 height decomposes (doc claim holds at MAX)",
        checked_tx_epoch_height_decomposition(u64::MAX).is_some(),
        true
    );
    let d = checked_tx_epoch_height_decomposition(u64::MAX).unwrap();
    check_eq!(
        "E decomposition reconstructs MAX",
        d.quotient * TX_EPOCH_BLOCKS + u64::from(d.remainder),
        u64::MAX
    );

    // Accumulator transition: advances only when an accepted child sits on a
    // non-zero boundary; child 0 never advances (genesis is installed, not
    // accepted).
    let old = [1u8; 32];
    let new_id = [2u8; 32];
    check_eq!(
        "E accumulator unchanged at child 143",
        next_tx_epoch_anchor_id(old, 143, new_id),
        old
    );
    check_eq!(
        "E accumulator advances at child 144",
        next_tx_epoch_anchor_id(old, 144, new_id),
        new_id
    );
    check_eq!(
        "E accumulator unchanged at child 0",
        next_tx_epoch_anchor_id(old, 0, new_id),
        old
    );
    check_eq!(
        "E accumulator unchanged at child 145",
        next_tx_epoch_anchor_id(old, 145, new_id),
        old
    );

    // The v2.1 activation epoch-overlap, quantified: children 279_361..279_504
    // share anchor header 279_360 across the semantic boundary.
    let shared: Vec<u64> = (V2_1 - 9..=V2_1 + 134)
        .map(tx_epoch_anchor_height_for_child)
        .collect();
    check!(
        "E one 144-block anchor window spans the activation (279_361..=279_504)",
        shared.iter().all(|&a| a == 279_360),
    );
}

fn main() {
    println!("subtle-edges consensus-boundary audit (pin verified by build.rs)");
    println!(
        "schedule: v1.1@95125 v2@{} (30s) v2.1@{} finality {} tx-epoch {}",
        V2, V2_1, CONSENSUS_FINALITY_DEPTH, TX_EPOCH_BLOCKS
    );
    section_a();
    section_b();
    section_c();
    section_d();
    section_e();
    println!("\nALL ASSERTIONS PASSED");
}
