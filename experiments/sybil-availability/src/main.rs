//! Loopback-only first-hop test using the target's actual P2P stack.
//!
//! Arbitrary tagged bytes enter `P2PNetwork::broadcast_tx`; they are not
//! consensus-valid transactions. Receivers observe `NewTx` then return Ignore,
//! so this isolates transport selection and GossipSub recovery from proving.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use libp2p::{gossipsub::MessageAcceptance, Multiaddr};
use noid_chain::{consensus::NetworkConfig, storage::MdbxChainContext};
use noid_mempool::{AsyncMempool, ChainView, MempoolConfig};
use noid_p2p::{BackgroundCapacity, NetworkEvent, NetworkTopics, P2PNetwork};
use tokio::{sync::RwLock, task::JoinHandle};

#[derive(Clone)]
struct Receipt {
    at: Instant,
    via: &'static str,
}

type Receipts = Arc<Mutex<HashMap<String, Vec<Receipt>>>>;

struct Node {
    name: String,
    network: P2PNetwork,
    p2p_task: JoinHandle<anyhow::Result<()>>,
    event_task: JoinHandle<()>,
    connected: Arc<AtomicUsize>,
    _data: tempfile::TempDir,
}

async fn start_node(name: String, port: u16, started: Instant, receipts: Receipts) -> Result<Node> {
    let data = tempfile::Builder::new()
        .prefix("sybil-relay-")
        .tempdir()
        .context("temp data directory")?;
    let ctx = MdbxChainContext::open_or_create(data.path()).context("genesis store")?;
    let mempool = AsyncMempool::new(ChainView::from_mdbx(&ctx), MempoolConfig::default());
    let listen: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse()?;
    let (network, p2p_task) = P2PNetwork::start(
        listen,
        vec![],
        Arc::new(RwLock::new(ctx)),
        mempool,
        NetworkTopics::for_network_cfg(&NetworkConfig::mainnet()),
        [0; 32],
        data.path().to_path_buf(),
        BackgroundCapacity::Full,
    )?;
    let mut events = network.subscribe();
    let event_name = name.clone();
    let connected = Arc::new(AtomicUsize::new(0));
    let event_connected = connected.clone();
    let event_task = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(NetworkEvent::PeerConnected {
                    peer,
                    locally_selected,
                    ..
                }) => {
                    event_connected.fetch_add(1, Ordering::Release);
                    println!(
                        "EVENT t_ms={} node={} connected={} locally_selected={}",
                        started.elapsed().as_millis(),
                        event_name,
                        peer,
                        locally_selected
                    );
                }
                Ok(NetworkEvent::NewTx {
                    intent_bytes,
                    delivery,
                    inbound_memory_permit,
                    ..
                }) => {
                    let tag = String::from_utf8_lossy(&intent_bytes).into_owned();
                    let via = if inbound_memory_permit.is_some() {
                        "direct"
                    } else {
                        "gossip"
                    };
                    let at = Instant::now();
                    receipts
                        .lock()
                        .unwrap()
                        .entry(format!("{event_name}/{tag}"))
                        .or_default()
                        .push(Receipt { at, via });
                    println!(
                        "RECV t_ms={} node={} via={} tag={}",
                        started.elapsed().as_millis(),
                        event_name,
                        via,
                        tag
                    );
                    // A Sybil withholds; the honest receiver also stops here
                    // because this is a transport-only, invalid-byte fixture.
                    delivery.complete(MessageAcceptance::Ignore);
                }
                Ok(_) => {}
                Err(error) => {
                    println!("EVENT_ERROR node={} error={error:?}", event_name);
                    break;
                }
            }
        }
    });
    Ok(Node {
        name,
        network,
        p2p_task,
        event_task,
        connected,
        _data: data,
    })
}

async fn wait_dispatchable(node: &Node, count: usize, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        // PeerConnected is emitted only after `try_mark_announced` confirms
        // the identified/profile-verified dispatchable path.
        let ready = node.connected.load(Ordering::Acquire);
        if ready >= count {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "{} only has {} dispatchable peers; wanted {}",
                node.name,
                ready,
                count
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn dial(dialer: &Node, port: u16) -> Result<()> {
    let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse()?;
    dialer.network.dial(addr).await;
    Ok(())
}

async fn send_batch(
    sender: &Node,
    phase: &str,
    count: usize,
    started: Instant,
    sent: &mut HashMap<String, Instant>,
) {
    for index in 0..count {
        let tag = format!("relay-{phase}-{index:02}");
        let at = Instant::now();
        sent.insert(tag.clone(), at);
        println!("SEND t_ms={} tag={tag}", started.elapsed().as_millis());
        sender.network.broadcast_tx(tag.into_bytes()).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_target(true)
        .init();
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .context("usage: sybil-relay-transport attack|control PORT_BASE")?;
    if mode != "attack" && mode != "control" {
        bail!("mode must be attack or control");
    }
    let base: u16 = args.next().context("missing PORT_BASE")?.parse()?;
    if args.next().is_some() {
        bail!("unexpected argument");
    }
    let started = Instant::now();
    let receipts: Receipts = Arc::new(Mutex::new(HashMap::new()));
    let mut nodes = Vec::new();
    for index in 0..8 {
        nodes.push(
            start_node(
                format!("sybil{index}"),
                base + index,
                started,
                receipts.clone(),
            )
            .await?,
        );
    }
    nodes.push(start_node("honest".into(), base + 8, started, receipts.clone()).await?);
    nodes.push(start_node("sender".into(), base + 9, started, receipts.clone()).await?);
    let sender_index = 9;
    let sender_started_ms = started.elapsed().as_millis();
    println!("TOPOLOGY mode={mode} sender_started_ms={sender_started_ms} sybil_count=8 honest_count=1 sender_outbound_honest=true");
    tokio::time::sleep(Duration::from_millis(150)).await;

    if mode == "attack" {
        dial(&nodes[0], base + 9).await?;
        dial(&nodes[1], base + 9).await?;
        wait_dispatchable(&nodes[sender_index], 2, Duration::from_secs(4)).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        println!(
            "FIRST_TWO_DISPATCHABLE t_ms={}",
            started.elapsed().as_millis()
        );
        for index in 2..8 {
            dial(&nodes[index], base + 9).await?;
        }
        dial(&nodes[sender_index], base + 8).await?;
    } else {
        dial(&nodes[sender_index], base + 8).await?;
        wait_dispatchable(&nodes[sender_index], 1, Duration::from_secs(4)).await?;
        println!(
            "HONEST_FIRST_DISPATCHABLE t_ms={}",
            started.elapsed().as_millis()
        );
        for index in 0..8 {
            dial(&nodes[index], base + 9).await?;
        }
    }
    wait_dispatchable(&nodes[sender_index], 9, Duration::from_secs(5)).await?;
    let pre_ms = started.elapsed().as_millis();
    println!(
        "ALL_DISPATCHABLE t_ms={pre_ms} sender_age_ms={}",
        pre_ms - sender_started_ms
    );
    // If first heartbeat has already run, retain results but label the
    // intended pre-heartbeat test inconclusive.
    println!(
        "PRE_HEARTBEAT_ELIGIBLE {}",
        pre_ms - sender_started_ms < 5200
    );

    let mut sent = HashMap::new();
    send_batch(&nodes[sender_index], "pre", 12, started, &mut sent).await;
    let post_due = started + Duration::from_millis(sender_started_ms as u64 + 6500);
    if Instant::now() < post_due {
        tokio::time::sleep_until(post_due.into()).await;
    }
    send_batch(&nodes[sender_index], "post", 4, started, &mut sent).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let observed = receipts.lock().unwrap().clone();
    for phase in ["pre", "post"] {
        for index in 0..if phase == "pre" { 12 } else { 4 } {
            let tag = format!("relay-{phase}-{index:02}");
            let key = format!("honest/{tag}");
            let first = observed.get(&key).and_then(|events| events.first());
            match first {
                Some(event) => println!(
                    "RESULT mode={mode} tag={tag} honest=received via={} delay_ms={}",
                    event.via,
                    event.at.duration_since(sent[&tag]).as_millis()
                ),
                None => {
                    println!("RESULT mode={mode} tag={tag} honest=missing_in_observation_window")
                }
            }
        }
    }
    for node in &nodes {
        node.event_task.abort();
        node.p2p_task.abort();
    }
    Ok(())
}
