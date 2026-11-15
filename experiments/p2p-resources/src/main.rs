//! Local, deterministic PoCs for the p2p-resources audit lane.
//!
//! Target: /Users/ember/dev/parano1d-break/parano1d @ 52938433 (read-only).
//! All scenarios drive the real production codecs from `noid_p2p` and the
//! real process-global outbound byte budget (`OutboundResponseBudget`).
//! Synthetic in-memory writers simulate a peer that stops reading mid-write
//! (yamux flow control stalls a non-reading peer after ~256 KiB).
//!
//! Scenarios (default: all):
//!   header-aggregate — header-sync response writes are outside every
//!       process-wide byte domain; concurrent stalled max-batch writes retain
//!       unbounded aggregate canonical+compressed bytes.
//!   mempool-pin      — four concurrent full mempool responses reserve the
//!       entire 64 MiB process-global outbound domain (production reserves
//!       MAX_MEMPOOL_SYNC_BYTES unconditionally per served pull), blocking
//!       every later budget-participating write process-wide.

use futures::{io::Cursor, AsyncWrite};
use libp2p::{request_response::Codec, StreamProtocol};
use noid_p2p::{
    header_protocol::HeaderInventoryRecord,
    header_sync_codec::{HeaderSyncCodec, MAX_HEADERS_PER_BATCH, MAX_UNCOMPRESSED_HEADER_BYTES},
    mempool_sync_codec::MempoolSyncCodec,
    object_codec::ObjectCodec,
    object_protocol::{BlockBodyClaimId, BlockBodyObjectId, DataResponseStatus, ObjectId},
    protocol::{GetHeadersResponse, GetMempoolResponse},
};
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

/// 64 MiB, matching noid_p2p/src/outbound_budget.rs:20
/// (OUTBOUND_RESPONSE_BUDGET_BYTES; the constant itself is `pub(crate)`).
const OUTBOUND_BUDGET_BYTES: usize = 64 * 1024 * 1024;

struct Gate {
    release: AtomicBool,
    /// Set on the first poll_write, i.e. the codec reached the wire at all.
    wrote_anything: AtomicBool,
    /// Set when the gated write index is reached.
    started: AtomicBool,
    payload_bytes: AtomicUsize,
    waker: Mutex<Option<Waker>>,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            release: AtomicBool::new(false),
            wrote_anything: AtomicBool::new(false),
            started: AtomicBool::new(false),
            payload_bytes: AtomicUsize::new(0),
            waker: Mutex::new(None),
        })
    }

    fn release(&self) {
        self.release.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

/// AsyncWrite that accepts everything but parks forever once it reaches
/// `gate_at`-th poll_write (0-based), simulating a peer whose yamux receive
/// window is exhausted. `gate_at = usize::MAX` never parks.
struct GatedWriter {
    gate: Arc<Gate>,
    writes: usize,
    gate_at: usize,
}

impl GatedWriter {
    fn new(gate: Arc<Gate>, gate_at: usize) -> Self {
        Self {
            gate,
            writes: 0,
            gate_at,
        }
    }
}

impl AsyncWrite for GatedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = &mut *self;
        this.gate.wrote_anything.store(true, Ordering::Release);
        let gate = this.writes == this.gate_at;
        this.writes += 1;
        if gate {
            this.gate.payload_bytes.store(buf.len(), Ordering::Release);
            this.gate.started.store(true, Ordering::Release);
            if !this.gate.release.load(Ordering::Acquire) {
                *this.gate.waker.lock().unwrap() = Some(cx.waker().clone());
                return Poll::Pending;
            }
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Waits until `predicate` holds, with real wall-clock settling time.
async fn wait_until(description: &str, timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        if Instant::now() > deadline {
            panic!("timed out waiting for {description}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn header_records(stream: u64) -> Vec<HeaderInventoryRecord> {
    let mut records = Vec::with_capacity(MAX_HEADERS_PER_BATCH);
    let mut random = stream.wrapping_add(1);
    for index in 0..MAX_HEADERS_PER_BATCH {
        let mut header = noid_chain::consensus::genesis_header();
        header.height = index as u64 + 1;
        header.nonce = ((stream as u128) << 64) | index as u128;
        for field in [
            &mut header.state_root,
            &mut header.tx_root,
            &mut header.miner_address.0,
            &mut header.difficulty_target,
        ] {
            for byte in field {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                *byte = random as u8;
            }
        }
        records.push(HeaderInventoryRecord::header_only(header));
    }
    records
}

/// Scenario 1: header-sync response writes retain canonical + compressed
/// bytes per stalled stream with zero process-wide admission.
async fn header_aggregate() {
    const STREAMS: usize = 64;
    let protocol = StreamProtocol::new("/noid/test/sync/headers/5");
    let mut gates = Vec::new();
    let mut tasks = Vec::new();
    for stream in 0..STREAMS {
        let gate = Gate::new();
        gates.push(gate.clone());
        let protocol = protocol.clone();
        tasks.push(tokio::spawn(async move {
            let response = GetHeadersResponse {
                status: DataResponseStatus::Ready,
                records: header_records(stream as u64),
                snapshot_boundary: None,
            };
            // Gate at the compressed payload write (write #1): the codec has
            // built `canonical`, built `compressed`, written the fixed header,
            // and now holds both buffers while the peer refuses to read.
            let mut writer = GatedWriter::new(gate, 1);
            HeaderSyncCodec
                .write_response(&protocol, &mut writer, response)
                .await
                .unwrap();
        }));
    }

    wait_until(
        "all header payload writes stalled",
        Duration::from_secs(60),
        || {
            gates
                .iter()
                .filter(|g| g.started.load(Ordering::Acquire))
                .count()
                == STREAMS
        },
    )
    .await;

    let compressed_bytes: usize = gates
        .iter()
        .map(|gate| gate.payload_bytes.load(Ordering::Acquire))
        .sum();
    let canonical_bytes = STREAMS * MAX_UNCOMPRESSED_HEADER_BYTES;
    let aggregate = canonical_bytes + compressed_bytes;
    let per_stream = MAX_UNCOMPRESSED_HEADER_BYTES + (compressed_bytes / STREAMS);
    let streams_to_exceed_budget = (OUTBOUND_BUDGET_BYTES + per_stream - 1) / per_stream;

    println!("header-aggregate PoC");
    println!("  stalled concurrent max-batch header payload writers: {STREAMS}");
    println!("  canonical bytes retained (content-independent lower bound): {canonical_bytes}");
    println!("  compressed bytes retained (this synthetic content): {compressed_bytes}");
    println!("  aggregate canonical+compressed: {aggregate}");
    println!("  documented process-wide outbound domain all other large protocols share: {OUTBOUND_BUDGET_BYTES}");
    println!("  header-sync bytes admitted by that domain: 0");
    println!("  streams needed to exceed the shared 64 MiB domain at this per-stream cost: ~{streams_to_exceed_budget}");
    println!("  per-stream canonical cap: {MAX_UNCOMPRESSED_HEADER_BYTES} bytes (4096 records x 288 wire bytes)");
    println!("  production per-connection chain_sync stream cap: 8; connections per peer: 2; inbound connections: 128 (behaviour.rs:329,539,546)");
    assert_eq!(
        gates
            .iter()
            .filter(|g| g.wrote_anything.load(Ordering::Acquire))
            .count(),
        STREAMS,
        "every stalled writer reached the wire unconditionally"
    );
    assert!(
        canonical_bytes > OUTBOUND_BUDGET_BYTES,
        "canonical-only retention across {STREAMS} streams must already exceed the shared 64 MiB domain"
    );

    for gate in &gates {
        gate.release();
    }
    for task in tasks {
        task.await.unwrap();
    }
}

async fn empty_mempool_response() -> GetMempoolResponse {
    let protocol = StreamProtocol::new("/noid/test/sync/mempool/4");
    let wire = Cursor::new(b"NMS3\0\0\0\0".to_vec());
    let mut codec = MempoolSyncCodec::default();
    codec.read_response(&protocol, &mut { wire }).await.unwrap()
}

fn fill_mempool_response(
    mut response: GetMempoolResponse,
    marker: u8,
    total: usize,
) -> GetMempoolResponse {
    let count = 56usize;
    let base = total / count;
    response.txs = (0..count)
        .map(|index| {
            let len = if index + 1 == count {
                total - base * (count - 1)
            } else {
                base
            };
            vec![marker.wrapping_add(index as u8); len]
        })
        .collect();
    assert_eq!(response.txs.iter().map(Vec::len).sum::<usize>(), total);
    response
}

async fn object_response() -> noid_p2p::object_protocol::GetObjectsResponse {
    let payload = vec![42u8];
    let object = BlockBodyObjectId::from_bytes(
        BlockBodyClaimId {
            height: 1,
            block_hash: [7; 32],
        },
        &payload,
    )
    .unwrap();
    let mut wire = Vec::new();
    wire.extend_from_slice(b"NOS2");
    wire.extend_from_slice(&[1, 0, 0, 0]);
    wire.push(0);
    wire.extend_from_slice(&1u64.to_le_bytes());
    wire.extend_from_slice(&[7; 32]);
    wire.extend_from_slice(&object.byte_digest);
    wire.extend_from_slice(&1u32.to_le_bytes());
    wire.push(0);
    let protocol = StreamProtocol::new("/noid/test/sync/objects/2");
    let mut codec = ObjectCodec::default();
    let mut response = codec
        .read_response(&protocol, &mut Cursor::new(wire))
        .await
        .unwrap();
    assert!(matches!(response.objects[0].object, ObjectId::BlockBody(_)));
    response.objects[0].bytes = Some(payload);
    response
}

/// Scenario 2: four full mempool responses reserve the entire process-global
/// outbound byte domain; later budget-participating writes block; the exact-
/// object codec (no budget acquire) and header codec (outside the domain) do
/// not — isolating exactly which layer production blocks at.
async fn mempool_pin() {
    let total = noid_chain::consensus::wire_limits::MAX_MEMPOOL_SYNC_BYTES;
    let protocol = StreamProtocol::new("/noid/test/sync/mempool/4");
    let aggressor_count = OUTBOUND_BUDGET_BYTES / total;
    println!("mempool-pin PoC");
    println!("  process-global outbound domain: {OUTBOUND_BUDGET_BYTES} bytes");
    println!("  production reservation per served mempool pull: {total} bytes (network.rs:327-330, unconditional)");
    println!("  full-domain aggressors needed: {aggressor_count}");

    let mut gates = Vec::new();
    let mut tasks = Vec::new();
    for marker in 0..aggressor_count as u8 {
        let gate = Gate::new();
        gates.push(gate.clone());
        let protocol = protocol.clone();
        tasks.push(tokio::spawn(async move {
            let response = fill_mempool_response(empty_mempool_response().await, marker, total);
            // Gate at the first intent write (write #1), after the codec's
            // fallback acquired `total` permits from the process-global
            // domain (mempool_sync_codec.rs:284). Production holds the same
            // permit count from preparation until the write completes.
            let mut writer = GatedWriter::new(gate, 1);
            MempoolSyncCodec::default()
                .write_response(&protocol, &mut writer, response)
                .await
                .unwrap();
        }));
    }
    wait_until(
        "all mempool aggressors stalled mid-write",
        Duration::from_secs(60),
        || {
            gates
                .iter()
                .filter(|g| g.started.load(Ordering::Acquire))
                .count()
                == aggressor_count
        },
    )
    .await;
    println!(
        "  aggressors holding the entire domain mid-write: {aggressor_count}/{aggressor_count}"
    );

    // Victim: a 1 KiB mempool response — a budget-participating write.
    let victim_gate = Gate::new();
    let victim_wrote = victim_gate.clone();
    let victim_protocol = protocol.clone();
    let victim = tokio::spawn(async move {
        let response = fill_mempool_response(empty_mempool_response().await, 0xAA, 1024);
        let mut writer = GatedWriter::new(victim_wrote, usize::MAX);
        MempoolSyncCodec::default()
            .write_response(&victim_protocol, &mut writer, response)
            .await
            .unwrap();
    });
    // Control 1: an exact-object response — its codec never acquires the
    // budget, so the codec write itself must complete during the pin.
    let object_done = Arc::new(AtomicBool::new(false));
    let object_done_write = object_done.clone();
    let object_control = tokio::spawn(async move {
        let response = object_response().await;
        let mut writer = GatedWriter::new(Gate::new(), usize::MAX);
        ObjectCodec::default()
            .write_response(
                &StreamProtocol::new("/noid/test/sync/objects/2"),
                &mut writer,
                response,
            )
            .await
            .unwrap();
        object_done_write.store(true, Ordering::Release);
    });
    // Control 2: a max-batch header response reaches its payload write
    // during the pin (header codec is outside the domain).
    let header_gate = Gate::new();
    let header_control_gate = header_gate.clone();
    let header_control = tokio::spawn(async move {
        let response = GetHeadersResponse {
            status: DataResponseStatus::Ready,
            records: header_records(999),
            snapshot_boundary: None,
        };
        let mut writer = GatedWriter::new(header_control_gate, 1);
        HeaderSyncCodec
            .write_response(
                &StreamProtocol::new("/noid/test/sync/headers/5"),
                &mut writer,
                response,
            )
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    let victim_blocked = !victim_gate.wrote_anything.load(Ordering::Acquire);
    println!("  1 KiB mempool response (budget-participating) blocked: {victim_blocked}");
    println!(
        "  exact-object codec write completed during the pin: {}",
        object_done.load(Ordering::Acquire)
    );
    println!(
        "  header max-batch payload write reached during the pin: {}",
        header_gate.started.load(Ordering::Acquire)
    );
    assert!(
        victim_blocked,
        "small budget-participating write must block while the domain is fully reserved"
    );
    assert!(
        object_done.load(Ordering::Acquire),
        "object codec write itself must not block (production blocks it at the network-layer acquire, network.rs:7229)"
    );
    assert!(
        header_gate.started.load(Ordering::Acquire),
        "header codec write must proceed: it is outside the byte domain by design"
    );

    // FIFO release: freeing one aggressor admits the queued victim.
    let released = Instant::now();
    gates[0].release();
    wait_until(
        "victim admitted after one release",
        Duration::from_secs(10),
        || victim_gate.wrote_anything.load(Ordering::Acquire),
    )
    .await;
    println!(
        "  victim admitted after releasing one aggressor: true ({} ms)",
        released.elapsed().as_millis()
    );

    header_gate.release();
    for gate in gates.iter().skip(1) {
        gate.release();
    }
    victim.await.unwrap();
    object_control.await.unwrap();
    header_control.await.unwrap();
    for task in tasks {
        task.await.unwrap();
    }
    println!("  all writers drained after release: true");
}

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("header-aggregate") => header_aggregate().await,
        Some("mempool-pin") => mempool_pin().await,
        _ => {
            header_aggregate().await;
            mempool_pin().await;
            println!("ALL ASSERTIONS PASSED");
        }
    }
}
