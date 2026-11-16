// spam-weight: offline intent factory + submitter for the sustained
// full-rate creation-spam PoC (collections + max-batch MintNft at
// unlock_height = u64::MAX on a live early-finance devnet).
//
// Forked from the chaos-latch factory (same RPC/claim layout); changes:
//   - new fixed wallet "spammer"
//   - collections created with maximum_items = u64::MAX
//   - minted items carry unlock_height = u64::MAX (permanently locked)
//   - mint funding UTXO selectable (largest by default)
//   - per-item state recording dropped (collection issued-count only)
// Builds real authorized intents against live chain state fetched over the
// node's JSON-RPC, then submits them with `paranoid_submitTxIntent`.

use anyhow::{bail, Context, Result};
use noid_chain::consensus::params::owner_authorization_version;
use noid_chain::consensus::fee_breakdown;
use noid_gkr::wallet_authorization::{
    prove_native_asset_authorization, prove_native_financial_authorization,
    prove_native_trade_authorization, prove_paged_spend_authorization_for_version,
};
use noid_gkr::OwnerAuthWitness;
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::asset_backing::TokenDenomination;
use noid_tx::native_assets::financial::{FinancialOperation, NativeParentRef};
use noid_tx::native_assets::launch::{
    launch_supply_from_whole, LaunchDefinition, LaunchOpening, LaunchState, LaunchSide,
};
use noid_tx::native_assets::market::MARKET_QUANTITY_MAX;
use noid_tx::native_assets::nft_market::{
    ListingDefinition, ListingState, NftMarketOpening, NftRoundContext, NFT_FEE_CREDIT,
};
use noid_tx::native_assets::trade::{wire::TradeOwnerIntent, TradeIntent, TradePolicy};
use noid_tx::native_assets::transition::{
    NativeContext, NativeInputClaim, NativeOperation, NativeOutputClaim,
};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    AssetId, CollectionId, FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening,
    NftCollection, NftItem, TokenReserve, TOKEN_DECIMALS,
};
use noid_tx::{
    output_bitmap_bit, PagedSpendIntent, TxBody, TxInput, TxOutput, TxPage, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

// ---------------------------------------------------------------------------
// Minimal loopback JSON-RPC client (plain HTTP/1.1 over TcpStream).
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
    let mut value: serde_json::Value = serde_json::from_slice(&body_bytes)
        .with_context(|| format!("decode rpc response from {method}: {:.300}", text))?;
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
        pos += size + 2; // skip trailing CRLF
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Wallets (fixed offline secrets; funded on-chain by the daemon wallet).
// ---------------------------------------------------------------------------

fn wallet_secret(name: &str) -> Result<SpendSecret> {
    let mut seed = [0u8; 32];
    seed[0] = match name {
        "trader" => 0x71,
        "launch1" => 0x72,
        "launch2" => 0x73,
        "market" => 0x74,
        "buyer" => 0x75,
        "spammer" => 0x76,
        _ => bail!("unknown wallet {name:?}"),
    };
    seed[1..6].copy_from_slice(b"chaos");
    Ok(SpendSecret::from_bytes(seed))
}

// ---------------------------------------------------------------------------
// Live chain view
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct View {
    tip_height: u64,
    parent_hash: [u8; 32],
    alloc_counter: u64,
    active_slot_count: u64,
    log_slots: u32,
    epoch_anchor: [u8; 32],
}

macro_rules! bail_if {
    ($cond:expr, $($arg:tt)*) => {
        if $cond {
            anyhow::bail!($($arg)*);
        }
    };
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    bail_if!(s.len() != 64, "expected 64 hex chars, got {}", s.len());
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

fn fetch_view(url: &str) -> Result<View> {
    let info = rpc(url, "getChainInfo", serde_json::json!([]))?;
    let height = info["height"].as_u64().context("height")?;
    let header = rpc(url, "getBlockHeader", serde_json::json!([height]))?;
    let anchor_hex = rpc(url, "getEpochAnchor", serde_json::json!([]))?
        .as_str()
        .context("epoch anchor")?
        .to_string();
    Ok(View {
        tip_height: height,
        parent_hash: hex32(header["hash"].as_str().context("header hash")?)?,
        alloc_counter: header["alloc_counter"].as_u64().context("alloc_counter")?,
        active_slot_count: header["active_slot_count"]
            .as_u64()
            .context("active_slot_count")?,
        log_slots: header["log_slots"].as_u64().context("log_slots")? as u32,
        epoch_anchor: hex32(&anchor_hex)?,
    })
}

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

fn get_slot(url: &str, index: u32) -> Result<(u64, u64)> {
    let slot = rpc(url, "getSlot", serde_json::json!([u64::from(index)]))?;
    Ok((
        slot["value"].as_u64().context("slot value")?,
        slot["creation_id"].as_u64().context("slot creation_id")?,
    ))
}

/// Find `count` empty output slots (probe getSlot; a young devnet is nearly
/// all empty, so scanning from `start` is quick).
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

/// Take `count` fresh empty slots using a persisted cursor so repeated factory
/// invocations never reuse an output slot that a pending intent already claims.
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

fn submit_intent(url: &str, bytes: &[u8]) -> Result<String> {
    rpc(
        url,
        "submitTxIntent",
        serde_json::json!([hexs(bytes)]),
    )?
    .as_str()
    .map(|s| s.to_string())
    .context("submitTxIntent result")
}

fn mempool_floor_and_size(url: &str) -> Result<(u64, usize)> {
    let info = rpc(url, "getMempoolInfo", serde_json::json!([]))?;
    Ok((
        info["fee_floor"].as_u64().context("fee_floor")?,
        info["size"].as_u64().context("size")? as usize,
    ))
}

// ---------------------------------------------------------------------------
// Persisted factory state (collections / items / listings), JSON under
// run/factory-state.json. Reconstructed openings must match on-chain records.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct State {
    json: serde_json::Value,
    path: std::path::PathBuf,
}

impl State {
    fn load() -> Result<Self> {
        let path = std::path::PathBuf::from("run/factory-state.json");
        let json = if path.exists() {
            serde_json::from_str(&std::fs::read_to_string(&path)?)?
        } else {
            serde_json::json!({"collections": {}, "items": [], "listings": []})
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

// ---------------------------------------------------------------------------
// Intent builders (ported from verification-fee-floor / verification-trade-latch
// and the nft-marketplace lane's claim layouts).
// ---------------------------------------------------------------------------

fn ordinary_intent(
    secret: SpendSecret,
    input: TxInput,
    output_slot: u32,
    anchor: [u8; 32],
    height: u64,
    fee: u64,
) -> Result<PagedSpendIntent> {
    let owner = derive_address(&secret);
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = input;
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
    Ok(PagedSpendIntent::new(pages, proof.to_bytes()?)?)
}

fn unindexed_trade(
    secret: SpendSecret,
    input: TxInput,
    epoch_anchor: [u8; 32],
    candidate_height: u64,
    fee_limit: u64,
    gross_input: u64,
) -> Result<TradeOwnerIntent> {
    let owner = derive_address(&secret);
    let intent = TradeIntent {
        policy: TradePolicy {
            chain: noid_chain::state::native_assets::native_chain_id(),
            epoch_anchor,
            owner,
            asset: AssetId([0xaa; 32]),
            recipient: owner,
            side: LaunchSide::Buy,
            gross_input,
            minimum_output: MARKET_QUANTITY_MAX,
            fee_limit,
            valid_from: candidate_height,
            valid_until: u64::MAX,
        },
        funding: vec![NativeInputClaim {
            input,
            root: owner,
            opening: None,
        }],
    };
    let proof = prove_native_trade_authorization(
        &intent,
        noid_chain::state::native_assets::native_chain_id(),
        epoch_anchor,
        candidate_height,
        OwnerAuthWitness::new(secret),
        noid_tx::OwnerAuthorizationVersion::V4,
    )?;
    Ok(TradeOwnerIntent {
        intent,
        authorization_bytes: proof.to_bytes()?,
    })
}

fn round_for(view: &View) -> Result<NftRoundContext> {
    NftRoundContext::new(view.tip_height + 1, view.parent_hash, view.alloc_counter)
        .map_err(anyhow::Error::from)
}

fn create_launch(
    owner_secret: SpendSecret,
    inputs: Vec<TxInput>,
    parent: &View,
    epoch_anchor: [u8; 32],
    metadata: [u8; 32],
) -> Result<NativeIntent> {
    let owner = derive_address(&owner_secret);
    let height = parent.tip_height + 1;
    let supply = launch_supply_from_whole(100_000).context("launch supply")?;
    let deposit = TokenDenomination::new(TOKEN_DECIMALS)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .minimum_backing(supply)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let fee = fee_breakdown(
        inputs.len() as u64,
        4,
        parent.active_slot_count,
        parent.log_slots,
    )
    .required_total;
    let genesis = GenesisAnchor {
        chain: noid_chain::state::native_assets::native_chain_id(),
        owner,
        input: inputs[0],
    };
    let token = FungibleDefinition::from_genesis(&genesis, supply, IssuanceRoute::Launchpad, metadata)
        .context("fungible definition")?;
    let launch = LaunchDefinition::from_genesis(&genesis, token).context("launch definition")?;
    let launch_state = LaunchState::initial(launch).context("launch state")?;
    let reserve = TokenReserve::from_definition(&token, deposit).context("token reserve")?;
    let definition = NativeOpening::Launch(LaunchOpening::Definition(launch));
    let token_opening = NativeOpening::FungibleDefinition(token);
    let reserve_opening = NativeOpening::TokenReserve(reserve);
    let state_opening = NativeOpening::Launch(LaunchOpening::State(launch_state));
    let outputs = vec![
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: 0,
                owner: token_opening.root(),
            },
            opening: Some(token_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: deposit,
                owner: reserve_opening.root(),
            },
            opening: Some(reserve_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: 0,
                owner: state_opening.root(),
            },
            opening: Some(state_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: inputs.iter().map(|input| input.amount).sum::<u64>() - deposit - fee,
                owner,
            },
            opening: None,
        },
    ];
    let round = round_for(parent)?;
    let context = NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height,
        authority: owner,
        fee,
    };
    let mut intent = NativeIntent::from_financial_claims(
        &context,
        epoch_anchor,
        FinancialOperation::CreateLaunch {
            funding_input: 0,
            definition: definition.root(),
        },
        round,
        inputs
            .into_iter()
            .map(|input| NativeInputClaim {
                input,
                root: owner,
                opening: None,
            })
            .collect(),
        outputs,
        |root| (root == definition.root()).then_some(definition),
    )
    .context("financial claims")?;
    let proof = prove_native_financial_authorization(
        &intent,
        context.chain,
        round,
        OwnerAuthWitness::new(owner_secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok(intent)
}

/// Assign distinct fresh empty output slots to every claim with an unset
/// slot (0). Returns the slots in page/output order.
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

fn financial_op_fee(intent: &NativeIntent) -> u64 {
    intent.pages[0].body.fee
}

// --- NFT marketplace lifecycle ----------------------------------------------

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
        input: TxInput {
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
    funding: TxInput,
    parent: &View,
    epoch_anchor: [u8; 32],
    metadata: [u8; 32],
    output_slot: u32,
) -> Result<NativeIntent> {
    let owner = derive_address(&secret);
    let height = parent.tip_height + 1;
    let anchor = GenesisAnchor {
        chain: noid_chain::state::native_assets::native_chain_id(),
        owner,
        input: funding,
    };
    let collection =
        NftCollection::from_genesis(&anchor, u64::MAX, metadata).context("collection")?;
    let fee = fee_breakdown(1, 2, parent.active_slot_count, parent.log_slots).required_total;
    let collection_opening = NativeOpening::NftCollection(collection);
    let outputs = vec![
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_slot,
                amount: 0,
                owner: collection_opening.root(),
            },
            opening: Some(collection_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: funding.amount - fee,
                owner,
            },
            opening: None,
        },
    ];
    let context = NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height,
        authority: owner,
        fee,
    };
    let mut intent = NativeIntent::from_claims(
        &context,
        epoch_anchor,
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
    let proof = prove_native_asset_authorization(
        &intent,
        context.chain,
        height,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok(intent)
}

fn mint_items(
    url: &str,
    secret: SpendSecret,
    collection: &CollectionRecord,
    cash: TxInput,
    count: usize,
    metadata: [u8; 32],
    parent: &View,
    epoch_anchor: [u8; 32],
    output_slot: u32,
) -> Result<(NativeIntent, Vec<NftItem>, Vec<u32>)> {
    let owner = derive_address(&secret);
    let height = parent.tip_height + 1;
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
            unlock_height: u64::MAX,
        });
    }
    let minted = NftCollection {
        issued_items: collection.collection.issued_items + items.len() as u64,
        ..collection.collection
    };
    let n_in = 2usize;
    let n_out = 1 + items.len() + 1;
    let fee = fee_breakdown(n_in as u64, n_out as u64, parent.active_slot_count, parent.log_slots)
        .required_total;
    let old_opening = NativeOpening::NftCollection(collection.collection);
    let new_opening = NativeOpening::NftCollection(minted);
    let item_slots = take_slots(url, items.len(), output_slot.saturating_add(1))?;
    let mut outputs = vec![NativeOutputClaim {
        output: TxOutput {
            slot_index: output_slot,
            amount: 0,
            owner: new_opening.root(),
        },
        opening: Some(new_opening),
    }];
    for (i, item) in items.iter().enumerate() {
        let opening = NativeOpening::NftItem(*item);
        outputs.push(NativeOutputClaim {
            output: TxOutput {
                slot_index: item_slots[i],
                amount: 0,
                owner: opening.root(),
            },
            opening: Some(opening),
        });
    }
    outputs.push(NativeOutputClaim {
        output: TxOutput {
            slot_index: 0,
            amount: cash.amount - fee,
            owner,
        },
        opening: None,
    });
    let context = NativeContext {
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
                input: TxInput {
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
    let proof = prove_native_asset_authorization(
        &intent,
        context.chain,
        height,
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
    cash: TxInput,
    ask: u64,
    parent: &View,
    epoch_anchor: [u8; 32],
    output_slot: u32,
) -> Result<(NativeIntent, ListingDefinition, ListingState)> {
    let owner = derive_address(&secret);
    let height = parent.tip_height + 1;
    let definition =
        ListingDefinition::from_item(*item, item_creation_id, ask, height + 100, height)
            .context("listing definition")?;
    let listing = ListingState::initial(definition).context("listing state")?;
    let fee = fee_breakdown(2, 2, parent.active_slot_count, parent.log_slots).required_total;
    let item_opening = NativeOpening::NftItem(*item);
    let listing_opening = NativeOpening::NftMarket(NftMarketOpening::Listing(listing));
    let definition_opening = NativeOpening::NftMarket(NftMarketOpening::Definition(definition));
    let outputs = vec![
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_slot,
                amount: NFT_FEE_CREDIT,
                owner: listing_opening.root(),
            },
            opening: Some(listing_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: cash.amount - fee - NFT_FEE_CREDIT,
                owner,
            },
            opening: None,
        },
    ];
    let round = round_for(parent)?;
    let context = NativeContext {
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
                input: TxInput {
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
    let proof = prove_native_financial_authorization(
        &intent,
        context.chain,
        round,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok((intent, definition, listing))
}

fn purchase(
    url: &str,
    secret: SpendSecret,
    listing_slot: u32,
    listing_creation_id: u64,
    listing: ListingState,
    definition: ListingDefinition,
    cash: TxInput,
    ask: u64,
    seller: Address,
    parent: &View,
    epoch_anchor: [u8; 32],
    output_slot: u32,
) -> Result<NativeIntent> {
    let buyer = derive_address(&secret);
    let height = parent.tip_height + 1;
    let fee = fee_breakdown(2, 3, parent.active_slot_count, parent.log_slots).required_total;
    let listing_opening = NativeOpening::NftMarket(NftMarketOpening::Listing(listing));
    let definition_opening = NativeOpening::NftMarket(NftMarketOpening::Definition(definition));
    let buyer_item = definition.item(buyer).context("buyer item")?;
    let item_opening = NativeOpening::NftItem(buyer_item);
    let outputs = vec![
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: ask + NFT_FEE_CREDIT,
                owner: seller,
            },
            opening: None,
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_slot,
                amount: 0,
                owner: item_opening.root(),
            },
            opening: Some(item_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 0,
                amount: cash.amount - ask - fee,
                owner: buyer,
            },
            opening: None,
        },
    ];
    let round = round_for(parent)?;
    let context = NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height,
        authority: buyer,
        fee,
    };
    let mut intent = NativeIntent::from_financial_claims(
        &context,
        epoch_anchor,
        FinancialOperation::Purchase {
            parent: NativeParentRef {
                slot_index: listing_slot,
                creation_id: listing_creation_id,
                root: listing_opening.root(),
            },
        },
        round,
        vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: listing_slot,
                    amount: NFT_FEE_CREDIT,
                    creation_id: listing_creation_id,
                },
                root: listing_opening.root(),
                opening: Some(listing_opening),
            },
            NativeInputClaim {
                input: cash,
                root: buyer,
                opening: None,
            },
        ],
        outputs,
        |root| {
            if root == listing_opening.root() {
                Some(listing_opening)
            } else if root == definition_opening.root() {
                Some(definition_opening)
            } else {
                None
            }
        },
    )
    .context("purchase claims")?;
    assign_output_slots(url, &mut intent)?;
    let proof = prove_native_financial_authorization(
        &intent,
        context.chain,
        round,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok(intent)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn tx_input(utxo: &Utxo) -> TxInput {
    TxInput {
        slot_index: utxo.slot_index,
        amount: utxo.amount,
        creation_id: utxo.creation_id,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(error) = run(args) {
        eprintln!("factory error: {error:#}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<()> {
    let url = args
        .get(1)
        .context("usage: chaos-factory <rpc-url> <command> [args...]")?
        .clone();
    let cmd = args.get(2).context("missing command")?.as_str();
    let rest: Vec<String> = args[3..].to_vec();
    match cmd {
        "address" => {
            let name = rest.first().map(|s| s.as_str()).unwrap_or("");
            let secret = wallet_secret(name)?;
            println!("{}", derive_address(&secret).to_bech32());
        }
        "floor" => {
            let (floor, size) = mempool_floor_and_size(&url)?;
            println!("floor={floor} size={size}");
        }
        "utxos" => {
            let name = rest.first().context("wallet name")?;
            let secret = wallet_secret(name)?;
            let address = derive_address(&secret);
            for utxo in fetch_utxos(&url, &address)? {
                println!("{} {} {}", utxo.slot_index, utxo.amount, utxo.creation_id);
            }
        }
        "trades" => run_trades(&url, &rest)?,
        "sample" => run_sample(&url, &rest)?,
        "launch" => run_launch(&url, &rest)?,
        "collection" => run_collection(&url, &rest)?,
        "confirm" => run_confirm(&url)?,
        "mint" => run_mint(&url, &rest)?,
        "list" => run_list(&url, &rest)?,
        "buy" => run_buy(&url, &rest)?,
        _ => bail!("unknown command {cmd:?}"),
    }
    Ok(())
}

fn run_sample(url: &str, rest: &[String]) -> Result<()> {
    // sample <wallet> <fee-micronoid> — one ordinary 1-in/1-out exact-fee self-transfer
    let name = rest.first().context("wallet name")?;
    let fee: u64 = rest.get(1).context("fee")?.parse()?;
    let secret = wallet_secret(name)?;
    let address = derive_address(&secret);
    let view = fetch_view(url)?;
    let utxos = fetch_utxos(url, &address)?;
    let utxo = utxos
        .iter()
        .find(|u| u.amount > fee + 1_000)
        .context("no utxo with enough value for the sample fee")?;
    let slot = take_slots(url, 1, 9000)?[0];
    let height = view.tip_height + 1;
    let intent = ordinary_intent(secret, tx_input(utxo), slot, view.epoch_anchor, height, fee)?;
    match submit_intent(url, &intent.to_bytes()?) {
        Ok(txid) => println!("sample ADMITTED txid={txid} fee={fee}"),
        Err(error) => println!("sample REJECTED fee={fee} error={error}"),
    }
    Ok(())
}

fn run_launch(url: &str, rest: &[String]) -> Result<()> {
    // launch <wallet> [metadata-hex-2-bytes...32]
    let name = rest.first().context("wallet name")?;
    let secret = wallet_secret(name)?;
    let address = derive_address(&secret);
    let view = fetch_view(url)?;
    let metadata = rest
        .get(1)
        .map(|h| hex32(h))
        .transpose()?
        .unwrap_or([0x71; 32]);
    let utxos = fetch_utxos(url, &address)?;
    let supply = launch_supply_from_whole(100_000)?;
    let deposit = TokenDenomination::new(TOKEN_DECIMALS)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .minimum_backing(supply)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let fee = fee_breakdown(utxos.len() as u64, 4, view.active_slot_count, view.log_slots)
        .required_total;
    let needed = deposit + fee;
    let mut inputs = Vec::new();
    let mut total = 0;
    for utxo in &utxos {
        if total >= needed {
            break;
        }
        inputs.push(tx_input(utxo));
        total += utxo.amount;
    }
    bail_if!(total < needed, "launch wallet needs {needed} µNOID, has {total}");
    let mut intent = create_launch(secret, inputs, &view, view.epoch_anchor, metadata)?;
    assign_output_slots(url, &mut intent)?;
    let fee_paid = financial_op_fee(&intent);
    match submit_intent(url, &intent.to_bytes()?) {
        Ok(txid) => println!("launch ADMITTED txid={txid} fee={fee_paid}"),
        Err(error) => println!("launch REJECTED fee={fee_paid} error={error}"),
    }
    Ok(())
}

fn run_collection(url: &str, rest: &[String]) -> Result<()> {
    // collection <wallet> [record-name] [metadata-hex-2]
    // The record name defaults to the wallet name; distinct records let one
    // wallet own several collections for ping-pong minting.
    let name = rest.first().context("wallet name")?;
    let record_name = rest.get(1).cloned().unwrap_or_else(|| name.to_string());
    let secret = wallet_secret(name)?;
    let address = derive_address(&secret);
    let view = fetch_view(url)?;
    let metadata = rest
        .get(2)
        .map(|h| hex32(h))
        .transpose()?
        .unwrap_or([0xa1; 32]);
    let utxos = fetch_utxos(url, &address)?;
    let funding = utxos.first().context("collection wallet needs a utxo")?;
    let slot = take_slots(url, 1, 10_000)?[0];
    let intent = create_collection(
        url, secret, tx_input(funding), &view, view.epoch_anchor, metadata, slot,
    )?;
    let fee = financial_op_fee(&intent);
    match submit_intent(url, &intent.to_bytes()?) {
        Ok(txid) => {
            let mut state = State::load()?;
            state.json["collections"][&record_name] = serde_json::json!({
                "owner": address_hex(&address),
                "funding": {"slot": funding.slot_index, "amount": funding.amount,
                            "creation_id": funding.creation_id},
                "maximum_items": u64::MAX,
                "metadata": hexs(&metadata),
                "slot": slot,
                "creation_id": null,
                "issued": 0,
            });
            state.save()?;
            println!("collection ADMITTED txid={txid} fee={fee} slot={slot} record={record_name}");
        }
        Err(error) => println!("collection REJECTED fee={fee} error={error}"),
    }
    Ok(())
}

fn run_confirm(url: &str) -> Result<()> {
    // Fill on-chain creation_ids for recorded records whose op is mined.
    let mut state = State::load()?;
    let url_owned = url.to_string();
    for (_name, record) in state.json["collections"].as_object_mut().context("collections")? {
        if record["creation_id"].is_null() {
            let (value, creation_id) =
                get_slot(&url_owned, record["slot"].as_u64().context("slot")? as u32)?;
            if value == 0 && creation_id != 0 {
                record["creation_id"] = serde_json::json!(creation_id);
                println!(
                    "collection slot {} confirmed creation_id={creation_id}",
                    record["slot"]
                );
            } else {
                println!(
                    "collection slot {} not live yet (value={value} creation_id={creation_id})",
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
                get_slot(url, item["slot"].as_u64().context("slot")? as u32)?;
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
                get_slot(url, listing["slot"].as_u64().context("slot")? as u32)?;
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
    Ok(())
}

fn run_mint(url: &str, rest: &[String]) -> Result<()> {
    // mint <wallet> <collection-name> <count> [metadata-hex-2] [funding-slot]
    // funding-slot omitted => the largest unclaimed UTXO is used.
    let name = rest.first().context("wallet name")?;
    let collection_name = rest.get(1).context("collection name")?.clone();
    let count: usize = rest.get(2).context("count")?.parse()?;
    let metadata = rest
        .get(3)
        .map(|h| hex32(h))
        .transpose()?
        .unwrap_or([0xb1; 32]);
    let funding_slot: Option<u32> = match rest.get(4) {
        Some(raw) => Some(raw.parse().context("funding slot")?),
        None => None,
    };
    let secret = wallet_secret(name)?;
    let address = derive_address(&secret);
    let view = fetch_view(url)?;
    let state = State::load()?;
    let record = state
        .json
        .get("collections")
        .and_then(|c| c.get(&collection_name))
        .cloned()
        .context("collection not recorded")?;
    let collection = reconstruct_collection(&record)?;
    let utxos = fetch_utxos(url, &address)?;
    let cash = match funding_slot {
        Some(slot) => utxos
            .iter()
            .find(|u| u.slot_index == slot)
            .with_context(|| format!("funding slot {slot} not found or spent"))?,
        None => utxos
            .iter()
            .max_by_key(|u| u.amount)
            .context("mint wallet needs a utxo")?,
    };
    let mint_fee = fee_breakdown(2, (count + 2) as u64, view.active_slot_count, view.log_slots)
        .required_total;
    bail_if!(
        cash.amount < mint_fee,
        "funding utxo {} too small for mint fee {mint_fee}",
        cash.amount
    );
    let out_slot = take_slots(url, 1, 20_000)?[0];
    let build_start = std::time::Instant::now();
    let (intent, _items, item_slots) = mint_items(
        url,
        secret,
        &collection,
        tx_input(cash),
        count,
        metadata,
        &view,
        view.epoch_anchor,
        out_slot,
    )?;
    let build_secs = build_start.elapsed().as_secs_f64();
    let fee = financial_op_fee(&intent);
    match submit_intent(url, &intent.to_bytes()?) {
        Ok(txid) => {
            let mut state = State::load()?;
            let record = state
                .json
                .get_mut("collections")
                .and_then(|c| c.get_mut(&collection_name))
                .context("collection record vanished")?;
            // The successor collection output replaces the consumed input slot;
            // its creation_id is confirmed from the chain after the block lands
            // (`confirm`). If the mint is evicted instead, the driver rolls
            // this record back from its per-round snapshot.
            record["slot"] = serde_json::json!(out_slot);
            record["creation_id"] = serde_json::Value::Null;
            record["issued"] =
                serde_json::json!(collection.collection.issued_items + count as u64);
            if !state.json.get("mints_log").is_some_and(|m| m.is_array()) {
                state.json["mints_log"] = serde_json::json!([]);
            }
            state.json["mints_log"]
                .as_array_mut()
                .context("mints_log")?
                .push(serde_json::json!({
                    "txid": txid,
                    "collection": collection_name,
                    "count": count,
                    "fee": fee,
                    "funding_slot": cash.slot_index,
                    "build_secs": build_secs,
                    "first_item_slot": item_slots.first().copied().unwrap_or(0),
            }));
            state.save()?;
            println!(
                "mint ADMITTED txid={txid} fee={fee} count={count} funding_slot={} build_secs={build_secs:.2} first_item_slot={}",
                cash.slot_index,
                item_slots.first().copied().unwrap_or(0)
            );
        }
        Err(error) => println!(
            "mint REJECTED fee={fee} count={count} build_secs={build_secs:.2} error={error}"
        ),
    }
    Ok(())
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

fn run_list(url: &str, rest: &[String]) -> Result<()> {
    // list <seller-wallet> <item-index> <ask-micronoid>
    let name = rest.first().context("wallet name")?;
    let item_index: usize = rest.get(1).context("item index")?.parse()?;
    let ask: u64 = rest.get(2).context("ask")?.parse()?;
    let secret = wallet_secret(name)?;
    let address = derive_address(&secret);
    let view = fetch_view(url)?;
    let state = State::load()?;
    let item_record = state
        .json
        .get("items")
        .and_then(|i| i.as_array())
        .context("items")?
        .get(item_index)
        .cloned()
        .context("item index not recorded")?;
    let collection_name = item_record["collection"]
        .as_str()
        .context("item collection name")?
        .to_string();
    let collection_record = state
        .json
        .get("collections")
        .and_then(|c| c.get(&collection_name))
        .cloned()
        .context("collection record")?;
    let collection = reconstruct_collection(&collection_record)?;
    let item = reconstruct_item(&{
        let mut record = item_record.clone();
        record["owner"] = serde_json::json!(address_hex(&address));
        record
    }, &collection.collection)?;
    let item_slot = item_record["slot"].as_u64().context("item slot")? as u32;
    let item_creation_id = item_record["creation_id"]
        .as_u64()
        .context("item creation_id not confirmed (run `confirm` after mining)")?;
    let cash = fetch_utxos(url, &address)?
        .into_iter()
        .find(|u| u.amount > NFT_FEE_CREDIT + 20_000)
        .context("seller wallet needs cash for escrow+fee")?;
    let out_slot = take_slots(url, 1, 40_000)?[0];
    let (intent, definition, listing) = create_listing(
        url,
        secret,
        &item,
        item_slot,
        item_creation_id,
        tx_input(&cash),
        ask,
        &view,
        view.epoch_anchor,
        out_slot,
    )?;
    let fee = financial_op_fee(&intent);
    match submit_intent(url, &intent.to_bytes()?) {
        Ok(txid) => {
            let mut state = State::load()?;
            state.json["listings"].as_array_mut().unwrap().push(serde_json::json!({
                "seller_wallet": name,
                "item_index": item_index,
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
            }));
            state.save()?;
            println!(
                "list ADMITTED txid={txid} fee={fee} listing_slot={out_slot} listing_root={}",
                hexs(&NativeOpening::NftMarket(NftMarketOpening::Listing(listing))
                    .root()
                    .0)
            );
        }
        Err(error) => println!("list REJECTED fee={fee} error={error}"),
    }
    Ok(())
}

fn run_buy(url: &str, rest: &[String]) -> Result<()> {
    // buy <buyer-wallet> <listing-index>
    let name = rest.first().context("wallet name")?;
    let listing_index: usize = rest.get(1).context("listing index")?.parse()?;
    let secret = wallet_secret(name)?;
    let buyer = derive_address(&secret);
    let view = fetch_view(url)?;
    let state = State::load()?;
    let listing = state.json["listings"]
        .as_array()
        .context("listings")?
        .get(listing_index)
        .cloned()
        .context("listing index not recorded")?;
    let definition_record = &listing["definition"];
    let definition = ListingDefinition {
        collection: CollectionId(hex32(
            definition_record["collection"].as_str().context("collection")?,
        )?),
        seller: address_from_hex(definition_record["seller"].as_str().context("seller")?)?,
        metadata: hex32(definition_record["metadata"].as_str().context("metadata")?)?,
        serial: definition_record["serial"].as_u64().context("serial")?,
        source_creation_id: definition_record["source_creation_id"]
            .as_u64()
            .context("source_creation_id")?,
        ask: definition_record["ask"].as_u64().context("ask")?,
        expiry_height: definition_record["expiry_height"]
            .as_u64()
            .context("expiry_height")?,
    };
    let listing_state = ListingState::initial(definition).context("listing state")?;
    let listing_slot = listing["slot"].as_u64().context("listing slot")? as u32;
    let listing_creation_id = listing["creation_id"]
        .as_u64()
        .context("listing creation_id not confirmed (run `confirm` after mining)")?;
    let ask = definition.ask;
    let seller = definition.seller;
    let cash = fetch_utxos(url, &buyer)?
        .into_iter()
        .find(|u| u.amount > ask + NFT_FEE_CREDIT + 20_000)
        .context("buyer wallet needs cash for ask+fee")?;
    let out_slot = take_slots(url, 1, 60_000)?[0];
    let intent = purchase(
        url,
        secret,
        listing_slot,
        listing_creation_id,
        listing_state,
        definition,
        tx_input(&cash),
        ask,
        seller,
        &view,
        view.epoch_anchor,
        out_slot,
    )?;
    let fee = financial_op_fee(&intent);
    match submit_intent(url, &intent.to_bytes()?) {
        Ok(txid) => println!("buy ADMITTED txid={txid} fee={fee} ask={ask}"),
        Err(error) => println!("buy REJECTED fee={fee} ask={ask} error={error}"),
    }
    Ok(())
}

fn run_trades(url: &str, rest: &[String]) -> Result<()> {
    // trades <wallet> <count> [fee-limit] [gross-input]
    let name = rest.first().context("wallet name")?.clone();
    let count: usize = rest.get(1).context("count")?.parse()?;
    let fee_limit: u64 = rest
        .get(2)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(6_600);
    let gross: u64 = rest.get(3).map(|s| s.parse()).transpose()?.unwrap_or(1);
    let address = derive_address(&wallet_secret(&name)?);
    let view_start = fetch_view(url)?;
    let utxos = fetch_utxos(url, &address)?;
    let usable: Vec<Utxo> = utxos
        .into_iter()
        .filter(|u| u.amount >= gross + fee_limit + 1)
        .take(count)
        .collect();
    bail_if!(
        usable.len() != count,
        "wallet {name} has {} usable UTXOs (need {count}, each >= {})",
        usable.len(),
        gross + fee_limit + 1
    );
    let height = view_start.tip_height + 1;
    println!(
        "building {count} trades at candidate height {height} (fee_limit={fee_limit}, gross={gross})"
    );
    let t0 = std::time::Instant::now();
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
        .max(1);
    let chunk = (count + worker_count - 1) / worker_count;
    let results: Vec<Vec<(usize, Result<Vec<u8>>)>> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (worker, chunk_utxos) in usable.chunks(chunk).enumerate() {
            let name = name.as_str();
            let anchor = view_start.epoch_anchor;
            handles.push(scope.spawn(move || {
                chunk_utxos
                    .iter()
                    .enumerate()
                    .map(|(i, utxo)| {
                        let built = (|| -> Result<Vec<u8>> {
                            let secret = wallet_secret(name)?;
                            let trade = unindexed_trade(
                                secret,
                                tx_input(utxo),
                                anchor,
                                height,
                                fee_limit,
                                gross,
                            )?;
                            Ok(trade.to_bytes()?)
                        })();
                        (worker * chunk + i, built)
                    })
                    .collect::<Vec<_>>()
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("trade worker panicked"))
            .collect()
    });
    let mut bytes_list: Vec<Option<Vec<u8>>> = vec![None; count];
    let mut build_errors = 0usize;
    for chunk_results in results {
        for (index, result) in chunk_results {
            match result {
                Ok(bytes) => bytes_list[index] = Some(bytes),
                Err(error) => {
                    build_errors += 1;
                    eprintln!("trade {index} build failed: {error:#}");
                }
            }
        }
    }
    println!(
        "built {} trade intents in {:.1}s ({} build errors); total bytes {}",
        bytes_list.iter().flatten().count(),
        t0.elapsed().as_secs_f64(),
        build_errors,
        bytes_list.iter().flatten().map(|b| b.len()).sum::<usize>()
    );
    let view_end = fetch_view(url)?;
    bail_if!(
        view_end.tip_height != view_start.tip_height,
        "tip moved during build ({} -> {}); retry with the miner paused",
        view_start.tip_height,
        view_end.tip_height
    );
    let mut admitted = 0usize;
    let t1 = std::time::Instant::now();
    for (i, bytes) in bytes_list.iter().enumerate() {
        let Some(bytes) = bytes else { continue };
        match submit_intent(url, bytes) {
            Ok(_txid) => {
                admitted += 1;
                if admitted % 50 == 0 {
                    let (floor, size) = mempool_floor_and_size(url)?;
                    println!(
                        "[{}/{}] admitted (elapsed {:.0}s) pool_size={size} floor={floor}",
                        i + 1,
                        count,
                        t1.elapsed().as_secs_f32()
                    );
                }
            }
            Err(error) => println!("[{}/{}] REJECTED: {error}", i + 1, count),
        }
    }
    let (floor, size) = mempool_floor_and_size(url)?;
    println!(
        "trades done: {admitted} admitted in {:.0}s; pool_size={size} floor={floor}",
        t1.elapsed().as_secs_f32()
    );
    Ok(())
}
