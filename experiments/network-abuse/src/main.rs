//! Local, anonymous header-sync probe against a loopback node only.
//! Sends the production 16-byte NHQ5 request and can either read or park.
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::{future::poll_fn, AsyncReadExt, AsyncWriteExt};

#[path = "../../devnet-chaos/src/aggressor_common.rs"]
mod transport;

const HEADER_PROTOCOL: &str = "/noid/mainnet/860e70453390bf81/1/sync/headers/5";

async fn request(mut stream: yamux::Stream, inventory: bool) -> Result<yamux::Stream> {
    transport::negotiate_outbound(&mut stream, HEADER_PROTOCOL).await?;
    let mut bytes = [0u8; 16];
    bytes[..4].copy_from_slice(b"NHQ5");
    bytes[4..12].copy_from_slice(&1u64.to_le_bytes());
    bytes[12..14].copy_from_slice(&4096u16.to_le_bytes());
    bytes[14] = u8::from(inventory);
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    stream.close().await?;
    Ok(stream)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let addr = args
        .get(1)
        .context("usage: noid-header-probe 127.0.0.1:PORT [read|park] [inventory=0|1] [streams]")?;
    if !addr.starts_with("127.0.0.1:") {
        bail!("loopback only");
    }
    let mode = args.get(2).map(String::as_str).unwrap_or("read");
    let inventory = args.get(3).map(String::as_str).unwrap_or("1") == "1";
    let streams: usize = args.get(4).map(String::as_str).unwrap_or("1").parse()?;
    let mut connection = transport::connect(addr).await?;
    let started = Instant::now();
    let mut pending = Vec::new();
    for index in 0..streams {
        let stream = poll_fn(|cx| connection.poll_new_outbound(cx)).await?;
        let issue = Box::pin(request(stream, inventory));
        let stream = tokio::time::timeout(
            Duration::from_secs(10),
            transport::conn_aware(&mut connection, issue),
        )
        .await???;
        println!(
            "issued {index} inventory={inventory} at_ms={}",
            started.elapsed().as_millis()
        );
        pending.push(stream);
    }
    if mode == "park" {
        let pause = Box::pin(tokio::time::sleep(Duration::from_secs(12)));
        tokio::time::timeout(
            Duration::from_secs(15),
            transport::conn_aware(&mut connection, pause),
        )
        .await??;
        println!("parked {streams} streams for 12s");
    } else if mode == "read" {
        for (index, mut stream) in pending.into_iter().enumerate() {
            let read = Box::pin(async move {
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).await?;
                Result::<Vec<u8>>::Ok(bytes)
            });
            let bytes = tokio::time::timeout(
                Duration::from_secs(15),
                transport::conn_aware(&mut connection, read),
            )
            .await???;
            println!(
                "read {index} bytes={} elapsed_ms={} prefix={:02x?}",
                bytes.len(),
                started.elapsed().as_millis(),
                &bytes[..bytes.len().min(12)]
            );
        }
    } else {
        bail!("unknown mode {mode}");
    }
    Ok(())
}
