use std::error::Error;

use noid_chain::block_header::{block_id, semantic_header_id, BlockHeader};
use noid_chain::consensus::{
    asert_anchor_height, block_work, genesis_header, next_target, params,
    pow::{search_pow, validate_pow},
    tx_epoch_anchor_height_for_child, validate_block_checks, validate_future_drift, AnchorInfo,
};
use noid_chain::fri_state::SlotValue;
use noid_chain::state::ChainState;
use noid_chain::storage::native_segment::{
    encode_snapshot_segment, NATIVE_SEGMENT_HEADER_BYTES, NATIVE_SEGMENT_MAGIC,
};
use noid_chain::storage::{
    encode_sparse_segment_entries, verify_history_step_terminal_candidate,
    AuthenticatedSnapshotMetadata, SnapshotSegmentDescriptor, SnapshotStagingError,
    SnapshotStagingSession,
};
use noid_chain::{add_work, materialize_accepted_block_state, HistoryStepTerminalMetadata};
use noid_core::Block128;
use snapshot_header_staging::{CanonicalHeaderBoundary, SnapshotHeaderStaging};

#[path = "../../../parano1d/noid_node/src/snapshot_header_staging.rs"]
mod snapshot_header_staging;
use noid_poseidon2b::primitives::Address;
use noid_recursive::ChainAccumulator;
use noid_tx::native_assets::{native_creation_id, AssetId, NativeOpening, TokenBalance};

const PINNED_COMMIT: &str = "52938433a30b185ab007afaa15166a632cf2e82e";
const NATIVE_HEIGHT: u64 = 279_370;

fn main() -> Result<(), Box<dyn Error>> {
    println!("pinned_commit={PINNED_COMMIT}");
    terminal_and_accumulator_controls()?;
    snapshot_root_controls()?;
    snapshot_native_sidecar_control()?;
    future_drift_differential()?;
    println!("all_local_assertions=passed");
    Ok(())
}

fn terminal_and_accumulator_controls() -> Result<(), Box<dyn Error>> {
    println!("[terminal-and-accumulator]");
    let epoch = genesis_header();
    let tip = fixture_header(10, [0x42; 32], 1, 1, 3);
    let terminal = HistoryStepTerminalMetadata::new(tip.height, semantic_header_id(&tip), 0)?
        .encode_prefix()
        .to_vec();
    let mut changed_root = tip;
    changed_root.state_root[0] ^= 1;
    let mut verifier_called = false;
    let rejected = verify_history_step_terminal_candidate(changed_root, epoch, terminal, |_| {
        verifier_called = true;
        Ok(())
    });
    assert!(rejected.is_err());
    assert!(!verifier_called);
    println!(
        "changed_state_root_terminal= rejected_before_recursive_verifier verifier_called=false"
    );

    let mut accumulator = accumulator_for(&tip, &epoch);
    let wrong_epoch = BlockHeader {
        nonce: epoch.nonce + 1,
        ..epoch
    };
    let epoch_error = accumulator
        .validate_local_header_boundary(&tip, &wrong_epoch)
        .expect_err("same-height wrong epoch header must not bind");
    println!("same_height_wrong_epoch= {epoch_error:?}");

    accumulator.tip_semantic_id = semantic_header_id(&changed_root);
    let root_error = accumulator
        .validate_local_header_boundary(&changed_root, &epoch)
        .expect_err("accumulator state root must match the exact header");
    println!("accumulator_state_root_mismatch= {root_error:?}");
    Ok(())
}

fn snapshot_root_controls() -> Result<(), Box<dyn Error>> {
    println!("[snapshot-exact-roots]");
    let parent = tempfile::tempdir()?;
    let fixture = ordinary_snapshot()?;

    let mut valid =
        SnapshotStagingSession::new(parent.path(), fixture.metadata, vec![fixture.descriptor])?;
    valid.accept_segment(0, 3, &fixture.encoded)?;
    let finalized = valid.finalize()?;
    println!(
        "root_matching_snapshot= finalized segments={} state_root={}",
        finalized.descriptors().len(),
        hex(&fixture.metadata.header().state_root)
    );
    drop(finalized);

    let mut leaf_tampered = fixture.encoded.clone();
    let last = leaf_tampered.last_mut().expect("non-empty segment");
    *last ^= 1;
    let mut wrong_leaf =
        SnapshotStagingSession::new(parent.path(), fixture.metadata, vec![fixture.descriptor])?;
    let leaf_error = wrong_leaf
        .accept_segment(0, 3, &leaf_tampered)
        .expect_err("changed leaf cannot retain the old exact segment root");
    assert!(matches!(
        &leaf_error,
        SnapshotStagingError::ExactSegmentRootMismatch { segment_id: 0 }
    ));
    println!("changed_leaf_old_segment_root= {leaf_error}");

    let mut wrong_global_header = *fixture.metadata.header();
    wrong_global_header.state_root[0] ^= 1;
    let wrong_global_metadata = AuthenticatedSnapshotMetadata::from_authenticated_header(
        wrong_global_header,
        block_id(&wrong_global_header),
        3,
    )?;
    let mut wrong_global = SnapshotStagingSession::new(
        parent.path(),
        wrong_global_metadata,
        vec![fixture.descriptor],
    )?;
    wrong_global.accept_segment(0, 3, &fixture.encoded)?;
    let global_error = match wrong_global.finalize() {
        Err(error) => error,
        Ok(_) => panic!("root-matching segments reconstructed a different header root"),
    };
    assert!(matches!(
        &global_error,
        SnapshotStagingError::StateRootMismatch { .. }
    ));
    println!("matching_segment_wrong_global_root= {global_error}");
    Ok(())
}

fn snapshot_native_sidecar_control() -> Result<(), Box<dyn Error>> {
    println!("[snapshot-native-sidecar]");
    assert!(params::v2_1_active(NATIVE_HEIGHT));
    let opening = NativeOpening::TokenBalance(TokenBalance {
        asset: AssetId([0x51; 32]),
        owner: Address([0x52; 32]),
        units: 7,
        unlock_height: 0,
    });
    opening.validate()?;
    let root = opening.root();

    let valid_slot = SlotValue::with_owner_fields(0, native_creation_id(1)?, root.as_fields());
    let valid = native_snapshot(valid_slot, opening, false)?;
    let parent = tempfile::tempdir()?;
    let mut valid_session =
        SnapshotStagingSession::new(parent.path(), valid.metadata, vec![valid.descriptor])?;
    valid_session.accept_segment(0, 3, &valid.encoded)?;
    valid_session.finalize()?;
    println!("root_bound_native_opening= finalized amount_micronoid=0");

    let wrong_amount_slot =
        SlotValue::with_owner_fields(1, native_creation_id(1)?, root.as_fields());
    let invalid = native_snapshot(wrong_amount_slot, opening, true)?;
    let mut invalid_session =
        SnapshotStagingSession::new(parent.path(), invalid.metadata, vec![invalid.descriptor])?;
    let error = invalid_session
        .accept_segment(0, 3, &invalid.encoded)
        .expect_err("raw amount 1 must not bind a zero-NOID TokenBalance opening");
    assert!(matches!(
        &error,
        SnapshotStagingError::NativeAmountBinding { segment_id: 0 }
    ));
    println!("root_matching_raw_with_wrong_native_amount= {error}");
    println!("native_sidecar_assumption= opening root is collision-resistant; raw slot root alone does not commit sidecar bytes");
    Ok(())
}

fn future_drift_differential() -> Result<(), Box<dyn Error>> {
    println!("[snapshot-vs-native-future-drift]");
    let genesis = genesis_header();
    let local_time = genesis.timestamp + 1_000;
    let timestamps = [
        local_time - 60,
        local_time - 40,
        local_time + params::MAX_FUTURE_DRIFT + 1_000,
        local_time,
    ];
    let fixture = pow_valid_chain(&timestamps, local_time)?;
    for block in &fixture.blocks {
        let result = if block.native_result.is_some() {
            "BadTimestamp"
        } else {
            "accepted"
        };
        println!(
            "native_block height={} timestamp={} result={result}",
            block.header.height, block.header.timestamp
        );
    }
    assert_eq!(fixture.blocks[2].native_result, Some(ErrTimestamp));
    assert!(fixture.blocks[3].native_result.is_none());

    let db = tempfile::tempdir()?;
    let staging = tempfile::tempdir()?;
    let context = noid_chain::storage::MdbxChainContext::open_or_create(db.path())?;
    let base = CanonicalHeaderBoundary::load(&context.store, 0)?;
    let path = staging.path().join("candidate.nhstage");
    let mut candidate = SnapshotHeaderStaging::create(&path, &context.store, base)?;
    let headers = fixture
        .blocks
        .iter()
        .map(|block| block.header)
        .collect::<Vec<_>>();
    candidate.append_batch(&context.store, &headers)?;
    let tip = headers.last().expect("four headers");
    let validated =
        candidate.validate_complete(&context.store, tip.height, block_id(tip), fixture.tip_work)?;
    validate_future_drift(tip.timestamp, local_time)?;
    println!(
        "snapshot_timeless_header_stage= sealed height={} tip_future_drift=pass intermediate_height_3_native=BadTimestamp",
        tip.height
    );
    println!("snapshot_gap_scope= real PoW-valid headers and real snapshot staging; a valid h4 HistoryStep terminal is assumed, not generated");
    validated.discard()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ErrTimestamp;

struct NativeBlock {
    header: BlockHeader,
    native_result: Option<ErrTimestamp>,
}

struct ChainFixture {
    blocks: Vec<NativeBlock>,
    tip_work: [u8; 32],
}

fn pow_valid_chain(timestamps: &[u64; 4], local_time: u64) -> Result<ChainFixture, Box<dyn Error>> {
    let genesis = genesis_header();
    let mut headers = vec![genesis];
    let mut state = ChainState::with_log_slots(genesis.log_slots as usize);
    let mut blocks = Vec::new();
    let mut work = block_work(&genesis.difficulty_target);

    for &timestamp in timestamps {
        let parent = *headers.last().expect("parent header");
        let anchor_height = asert_anchor_height(parent.height);
        let anchor = headers[anchor_height as usize];
        let target = next_target(
            anchor_height,
            anchor.timestamp,
            &anchor.difficulty_target,
            parent.height + 1,
            parent.timestamp,
            timestamp,
        );
        let template = noid_chain::consensus::build_block_template(
            &parent,
            &state,
            &[],
            Vec::new(),
            parent.miner_address,
            timestamp,
            target,
        )
        .map_err(|error| format!("template fixture: {error:?}"))?;
        let mut block = template.into_block(0);
        block.header.nonce = search_pow(&block.header, 0, 2_000_000_000)
            .ok_or("PoW fixture did not find a nonce")?;
        validate_pow(&block.header)?;

        let previous_timestamps = headers
            .iter()
            .map(|header| header.timestamp)
            .collect::<Vec<_>>();
        let native = validate_block_checks(
            &block,
            &parent,
            &previous_timestamps,
            &[],
            local_time,
            &AnchorInfo {
                anchor_height,
                anchor_timestamp: anchor.timestamp,
                anchor_target: anchor.difficulty_target,
            },
        );
        let native_result = match native {
            Ok(()) => None,
            Err(noid_chain::consensus::ConsensusError::BadTimestamp) => Some(ErrTimestamp),
            Err(error) => {
                return Err(format!("unexpected native validation failure: {error:?}").into())
            }
        };
        if block.header.height != 3 {
            assert!(native_result.is_none());
        }

        materialize_accepted_block_state(&mut state, &block)
            .map_err(|error| format!("fixture materialization: {error:?}"))?;
        work = add_work(&work, &block_work(&block.header.difficulty_target));
        headers.push(block.header);
        blocks.push(NativeBlock {
            header: block.header,
            native_result,
        });
    }
    assert_eq!(headers.len(), 5);
    assert!(validate_future_drift(headers[3].timestamp, local_time).is_err());
    assert!(validate_future_drift(headers[4].timestamp, local_time).is_ok());
    Ok(ChainFixture {
        blocks,
        tip_work: work,
    })
}

struct SnapshotFixture {
    metadata: AuthenticatedSnapshotMetadata,
    descriptor: SnapshotSegmentDescriptor,
    encoded: Vec<u8>,
}

fn ordinary_snapshot() -> Result<SnapshotFixture, Box<dyn Error>> {
    let first = slot(50, 2, 0xA1);
    let second = slot(70, 5, 0xB2);
    let entries = [(1u16, first), (6u16, second)];
    let encoded = encode_sparse_segment_entries(3, &entries)
        .ok_or("ordinary sparse segment encoding failed")?;
    let exact = ChainState::from_sparse_utxos(3, &[(1, first), (6, second)], 5)
        .map_err(|error| format!("ordinary state fixture: {error:?}"))?;
    let header = fixture_header(7, exact.utxo_root, 2, 5, 3);
    let metadata =
        AuthenticatedSnapshotMetadata::from_authenticated_header(header, block_id(&header), 3)?;
    let descriptor = SnapshotSegmentDescriptor {
        segment_id: 0,
        segment_root: segment_root(3, &entries)?,
        encoded_len: encoded.len() as u32,
    };
    Ok(SnapshotFixture {
        metadata,
        descriptor,
        encoded,
    })
}

fn native_snapshot(
    slot: SlotValue,
    opening: NativeOpening,
    manually_frame_invalid: bool,
) -> Result<SnapshotFixture, Box<dyn Error>> {
    let entries = [(1u16, slot)];
    let raw = encode_sparse_segment_entries(3, &entries)
        .ok_or("native sparse segment encoding failed")?;
    let exact = ChainState::from_sparse_utxos_at(3, &[(1, slot)], 1, NATIVE_HEIGHT)
        .map_err(|error| format!("native state fixture: {error:?}"))?;
    let header = fixture_header(NATIVE_HEIGHT, exact.utxo_root, 1, 1, 3);
    let metadata =
        AuthenticatedSnapshotMetadata::from_authenticated_header(header, block_id(&header), 3)?;
    let descriptor = SnapshotSegmentDescriptor {
        segment_id: 0,
        segment_root: segment_root(3, &entries)?,
        encoded_len: 0,
    };
    let encoded = if manually_frame_invalid {
        frame_one_native_record(raw, opening)?
    } else {
        encode_snapshot_segment(raw, NATIVE_HEIGHT, |requested| {
            assert_eq!(requested, opening.root());
            Some(opening)
        })
        .ok_or("valid native snapshot encoding failed")?
    };
    let descriptor = SnapshotSegmentDescriptor {
        encoded_len: encoded.len() as u32,
        ..descriptor
    };
    Ok(SnapshotFixture {
        metadata,
        descriptor,
        encoded,
    })
}

fn segment_root(log: u8, entries: &[(u16, SlotValue)]) -> Result<[u8; 32], Box<dyn Error>> {
    use noid_chain::exact_state_hash::{slot_leaf_hash, state_node_hash, zero_slot_roots};

    let zero = zero_slot_roots(usize::from(log));
    let mut level = vec![zero[0]; 1usize << log];
    for &(index, slot) in entries {
        let leaf = level
            .get_mut(usize::from(index))
            .ok_or("segment fixture index is out of range")?;
        *leaf = slot_leaf_hash(slot);
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

fn frame_one_native_record(
    raw: Vec<u8>,
    opening: NativeOpening,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let opening = opening.to_bytes()?;
    let mut encoded =
        Vec::with_capacity(NATIVE_SEGMENT_HEADER_BYTES + raw.len() + 2 + opening.len());
    encoded.extend_from_slice(NATIVE_SEGMENT_MAGIC);
    encoded.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&1u32.to_le_bytes());
    encoded.extend_from_slice(&raw);
    encoded.extend_from_slice(&(opening.len() as u16).to_le_bytes());
    encoded.extend_from_slice(&opening);
    Ok(encoded)
}

fn accumulator_for(tip: &BlockHeader, epoch: &BlockHeader) -> ChainAccumulator {
    assert_eq!(epoch.height, tx_epoch_anchor_height_for_child(tip.height));
    ChainAccumulator {
        height: tip.height,
        tip_semantic_id: semantic_header_id(tip),
        state_root: tip.state_root,
        log_slots: tip.log_slots,
        active_slot_count: tip.active_slot_count,
        alloc_counter: tip.alloc_counter,
        epoch_anchor_id: block_id(epoch),
    }
}

fn fixture_header(
    height: u64,
    state_root: [u8; 32],
    active_slot_count: u64,
    alloc_counter: u64,
    log_slots: u32,
) -> BlockHeader {
    BlockHeader {
        prev_block_hash: [0x11; 32],
        state_root,
        tx_root: [0x22; 32],
        timestamp: 1_700_000_000,
        height,
        miner_address: Address([0x33; 32]),
        nonce: 9,
        difficulty_target: [0xff; 32],
        log_slots,
        active_slot_count,
        alloc_counter,
    }
}

fn slot(amount: u64, creation_id: u64, owner: u128) -> SlotValue {
    SlotValue::with_owner_fields(
        amount,
        creation_id,
        [Block128::from(owner), Block128::from(owner + 1)],
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
