//! Local anonymous mempool Pull/Push probe. Saves one already-admitted
//! wallet-generated intent from an isolated node, or replays its raw bytes.
use std::{path::Path, time::Duration};

use anyhow::{bail, Context, Result};
use futures::{future::poll_fn, AsyncReadExt, AsyncWriteExt};

#[path = "../../../devnet-chaos/src/aggressor_common.rs"]
mod transport;

async fn exchange_on(
    connection: &mut transport::YamuxConnection,
    request: Vec<u8>,
) -> Result<Vec<u8>> {
    let stream = poll_fn(|cx| connection.poll_new_outbound(cx)).await?;
    let action = Box::pin(async move {
        let mut stream = stream;
        transport::negotiate_outbound(&mut stream, transport::MEMPOOL_PROTOCOL).await?;
        stream.write_all(&request).await?;
        stream.flush().await?;
        stream.close().await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        Result::<Vec<u8>>::Ok(response)
    });
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        transport::conn_aware(connection, action),
    )
    .await???;
    Ok(response)
}

async fn exchange(addr: &str, request: Vec<u8>) -> Result<Vec<u8>> {
    let mut connection = transport::connect(addr).await?;
    exchange_on(&mut connection, request).await
}

fn push_request(mut intent: Vec<u8>, flip: Option<usize>) -> Result<Vec<u8>> {
    if let Some(from_end) = flip {
        if from_end == 0 || from_end > intent.len() {
            bail!("flip offset outside intent");
        }
        let index = intent.len() - from_end;
        intent[index] ^= 0xff;
    }
    let mut request = b"NMR3\x01\0\0\0".to_vec();
    request.extend_from_slice(&(intent.len() as u32).to_le_bytes());
    request.extend_from_slice(&intent);
    Ok(request)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let addr = args
        .get(1)
        .context("usage: mempool_probe 127.0.0.1:PORT pull|push <lane-run-file> [flip-from-end]")?;
    if !addr.starts_with("127.0.0.1:") {
        bail!("loopback only");
    }
    let mode = args.get(2).context("mode")?.as_str();
    let path = Path::new(args.get(3).context("lane-run-file")?);
    if !path
        .to_string_lossy()
        .contains("/experiments/network-abuse/run/")
    {
        bail!("refusing file outside this lane's ignored run directory");
    }
    if mode == "pull" {
        let request = b"NMR3\0\0\0\0\0\0\0\0".to_vec();
        let response = exchange(addr, request).await?;
        if response.len() < 12 || &response[..4] != b"NMS3" {
            bail!("bad mempool response");
        }
        let count = u16::from_le_bytes(response[4..6].try_into().unwrap()) as usize;
        if count == 0 {
            bail!("mempool empty");
        }
        let selected: usize = args.get(4).map(|v| v.parse()).transpose()?.unwrap_or(0);
        if selected >= count {
            bail!("selected mempool entry out of range");
        }
        let lengths: Vec<usize> = response[8..8 + 4 * count]
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) as usize)
            .collect();
        let start = 8 + 4 * count;
        let intent = response
            .get(
                start + lengths[..selected].iter().sum::<usize>()
                    ..start + lengths[..=selected].iter().sum::<usize>(),
            )
            .context("truncated selected intent")?;
        std::fs::write(path, intent)?;
        println!(
            "pulled count={count} selected={selected} intent_bytes={} total_response_bytes={}",
            intent.len(),
            response.len()
        );
    } else if mode == "push" {
        let intent = std::fs::read(path)?;
        let flip = args.get(4).map(|value| value.parse()).transpose()?;
        let request = push_request(intent.clone(), flip)?;
        let response = exchange(addr, request).await?;
        println!(
            "pushed intent_bytes={} response_bytes={} prefix={:02x?}",
            intent.len(),
            response.len(),
            &response[..response.len().min(12)]
        );
    } else if mode == "push-repeat" {
        let intent = std::fs::read(path)?;
        let flip: usize = args
            .get(4)
            .context("push-repeat needs flip-from-end")?
            .parse()?;
        let count: usize = args.get(5).context("push-repeat needs count")?.parse()?;
        let interval_ms: u64 = args
            .get(6)
            .context("push-repeat needs interval-ms")?
            .parse()?;
        if count > 50 || interval_ms < 200 {
            bail!("bounded probe: <=50 requests, >=200 ms pacing");
        }
        let mut connection = transport::connect(addr).await?;
        for index in 0..count {
            let request = push_request(intent.clone(), Some(flip))?;
            let response = exchange_on(&mut connection, request).await?;
            println!("push {index} response_bytes={}", response.len());
            let pause = Box::pin(tokio::time::sleep(Duration::from_millis(interval_ms)));
            transport::conn_aware(&mut connection, pause).await?;
        }
    } else {
        bail!("unknown mode {mode}");
    }
    Ok(())
}
