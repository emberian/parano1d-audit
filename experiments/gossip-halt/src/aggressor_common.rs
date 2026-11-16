//! Shared hand-driven libp2p transport for the gossip-halt aggressor.
//!
//! Copied from ../devnet-chaos/src/aggressor_common.rs (do not modify that
//! lane) and extended: connect_as() takes a caller-supplied identity so one
//! process can hold several distinct PeerIds, the identify responder
//! advertises /meshsub/1.1.0, and inbound meshsub proposals are drained
//! (the node opens its gossipsub stream to us on connect).

#![allow(dead_code)]

use std::future::Future;

use anyhow::{bail, Result};
use futures::future::poll_fn;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, FutureExt};
use libp2p_core::upgrade::OutboundConnectionUpgrade;
use libp2p_identity::ed25519;
use tokio_util::compat::TokioAsyncReadCompatExt;

pub const MEMPOOL_PROTOCOL: &str = "/noid/mainnet/860e70453390bf81/1/sync/mempool/4";
pub const PING_PROTOCOL: &str = "/ipfs/ping/1.0.0";
pub const IDENTIFY_PROTOCOL: &str = "/ipfs/id/1.0.0";
pub const GOSSIPSUB_PROTOCOL: &str = "/meshsub/1.1.0";
pub const PREFIX: &str = "/noid/mainnet/860e70453390bf81/1";

pub fn advertised_protocols() -> Vec<String> {
    let mut list = vec![
        PING_PROTOCOL.to_string(),
        IDENTIFY_PROTOCOL.to_string(),
        "/ipfs/id/push/1.0.0".to_string(),
        GOSSIPSUB_PROTOCOL.to_string(),
        "/meshsub/1.2.0".to_string(),
        "/meshsub/1.0.0".to_string(),
    ];
    for suffix in [
        "/sync/profile/6", "/sync/objects/2", "/sync/availability/1",
        "/sync/headers/5", "/sync/headers/4", "/sync/history-step/1",
        "/sync/fork-origin/1", "/sync/manifest/7", "/sync/manifest-page/1",
        "/sync/segment/7", "/sync/mempool/4", "/sync/mempool/3",
    ] {
        list.push(format!("{PREFIX}{suffix}"));
    }
    list.push(format!("{PREFIX}/kad/1.0.0"));
    list
}

pub fn identify_response_bytes() -> Vec<u8> {
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
    push_string(0x32, "gossip-halt-anon/1.0");
    out
}

pub fn write_varint(buf: &mut Vec<u8>, mut value: usize) {
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

pub async fn read_message<T: AsyncRead + Unpin>(io: &mut T) -> Result<String> {
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

pub async fn negotiate_outbound<T: AsyncRead + AsyncWrite + Unpin>(
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

pub async fn negotiate_inbound<T: AsyncRead + AsyncWrite + Unpin>(
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
            io.write_all(&mut message).await?;
            io.flush().await?;
            Ok(Some((*protocol).to_string()))
        }
        None => {
            write_varint(&mut message, 3);
            message.extend_from_slice(b"na\n");
            io.write_all(&mut message).await?;
            io.flush().await?;
            Ok(None)
        }
    }
}

pub type PlainIo = tokio_util::compat::Compat<tokio::net::TcpStream>;
pub type NoiseIo = libp2p_noise::Output<PlainIo>;
pub type YamuxConnection = yamux::Connection<NoiseIo>;

/// TCP + noise + yamux handshake to a loopback victim with a caller-chosen
/// identity, returning a client-mode yamux Connection ready for outbound
/// streams. The noise static key IS the libp2p identity, so the PeerId the
/// victim sees is derived from this keypair.
pub async fn connect_as(
    addr: &str,
    keypair: &ed25519::Keypair,
) -> Result<YamuxConnection> {
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let mut plain = tcp.compat();
    negotiate_outbound(&mut plain, "/noise").await?;
    let identity = libp2p_identity::Keypair::from(keypair.clone());
    let noise = libp2p_noise::Config::new(&identity)?;
    let (_peer, noise_io) = noise.upgrade_outbound(plain, "/noise").await?;
    let mut noise_io = noise_io;
    negotiate_outbound(&mut noise_io, "/yamux/1.0.0").await?;
    Ok(yamux::Connection::new(noise_io, yamux::Config::default(), yamux::Mode::Client))
}

/// TCP + noise + yamux handshake with a fresh random identity.
pub async fn connect(addr: &str) -> Result<YamuxConnection> {
    connect_as(addr, &ed25519::Keypair::generate()).await
}

/// Drive the connection (answering inbound ping/identify, draining inbound
/// meshsub, parking the rest) while `fut` makes progress.
pub async fn conn_aware<F>(connection: &mut YamuxConnection, mut fut: F) -> Result<F::Output>
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
                        let supported = [
                            PING_PROTOCOL,
                            IDENTIFY_PROTOCOL,
                            GOSSIPSUB_PROTOCOL,
                            "/meshsub/1.2.0",
                            "/meshsub/1.0.0",
                            MEMPOOL_PROTOCOL,
                        ];
                        if let Ok(Some(protocol)) = negotiate_inbound(&mut stream, &supported).await {
                            if protocol == PING_PROTOCOL {
                                let mut buffer = [0u8; 32];
                                while stream.read_exact(&mut buffer).await.is_ok() {
                                    if stream.write_all(&buffer).await.is_err() || stream.flush().await.is_err() {
                                        break;
                                    }
                                }
                            } else if protocol == IDENTIFY_PROTOCOL {
                                let response = identify_response_bytes();
                                let _ = stream.write_all(&response).await;
                                let _ = stream.flush().await;
                                let _ = stream.close().await;
                            } else if protocol == GOSSIPSUB_PROTOCOL {
                                // The node's gossipsub stream to us: drain its
                                // control frames so its writer never stalls.
                                let mut buffer = [0u8; 4096];
                                while stream.read(&mut buffer).await.unwrap_or(0) > 0 {}
                            } else {
                                futures::future::pending::<()>().await;
                            }
                        }
                    });
                    continue;
                }
                std::task::Poll::Ready(None) => {
                    return std::task::Poll::Ready(Err(anyhow::anyhow!("yamux connection closed by remote")))
                }
                std::task::Poll::Pending => break,
            }
        }
        fut.poll_unpin(cx).map(Ok)
    })
    .await
}
