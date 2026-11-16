// halt-stalefee intentool: build and submit an exact-fee CreateLaunch
// NativeIntent against a live parano1d devnet node, plus genesis
// regeneration for the small-slot-domain poison build.
//
// Modes:
//   genesis                 print the regenerated genesis state root / nonce /
//                          block id for the patched LOG_SLOTS_GENESIS
//   fee                    print the current required CreateLaunch fee (3-in/4-out)
//                          at the live tip, plus the pressure multiplier
//   launch                 build + prove + submit the CreateLaunch intent
//
// The tool must run against the same patched tree as the devnet nodes: the
// fee schedule constants and owner-authorization version boundary come from
// noid_chain at the pin used for this lane.

use anyhow::{bail, Context, Result};
use noid_chain::block_header::block_id;
use noid_chain::consensus::genesis::{genesis_header, GENESIS_BURN_ADDRESS, GENESIS_TIMESTAMP};
use noid_chain::consensus::params::{
    owner_authorization_version, GENESIS_TARGET, LOG_SLOTS_GENESIS,
};
use noid_chain::consensus::pow::search_pow;
use noid_chain::consensus::{fee_breakdown, pressure_multiplier};
use noid_gkr::wallet_authorization::prove_native_financial_authorization;
use noid_gkr::OwnerAuthWitness;
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::asset_backing::TokenDenomination;
use noid_tx::native_assets::financial::FinancialOperation;
use noid_tx::native_assets::launch::{
    launch_supply_from_whole, LaunchDefinition, LaunchOpening, LaunchState,
};
use noid_tx::native_assets::nft_market::NftRoundContext;
use noid_tx::native_assets::transition::{NativeContext, NativeInputClaim, NativeOutputClaim};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening, TokenReserve,
    TOKEN_DECIMALS,
};
use noid_tx::{TxInput, TxOutput};

// ---------------------------------------------------------------------------
// Minimal JSON-RPC client (blocking; loopback devnet only)
// ---------------------------------------------------------------------------

struct Rpc {
    url: String,
    client: reqwest::blocking::Client,
}

impl Rpc {
    fn new(url: &str) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("reqwest client"),
        }
    }

    fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": format!("paranoid_{method}"),
            "params": params,
        });
        let response = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .with_context(|| format!("rpc {method} transport"))?;
        let status = response.status();
        let payload: serde_json::Value = response.json().with_context(|| format!("rpc {method} decode"))?;
        if !status.is_success() {
            bail!("rpc {method} http {status}: {payload}");
        }
        if let Some(error) = payload.get("error") {
            bail!("rpc {method} error: {error}");
        }
        Ok(payload
            .get("result")
            .cloned()
            .with_context(|| format!("rpc {method} missing result"))?)
    }
}

// ---------------------------------------------------------------------------
// Genesis regeneration for the patched slot domain
// ---------------------------------------------------------------------------

fn genesis_mode() -> Result<()> {
    let mut state = noid_chain::state::ChainState::with_log_slots(LOG_SLOTS_GENESIS as usize);
    let root = state.state_root();
    let mut header = genesis_header();
    anyhow::ensure!(
        header.log_slots == LOG_SLOTS_GENESIS,
        "genesis header log_slots {} != constant {}",
        header.log_slots,
        LOG_SLOTS_GENESIS
    );
    header.state_root = root;
    header.nonce = 0;
    let nonce = search_pow(&header, 0, 2_000_000_000).context("genesis pow search")?;
    header.nonce = nonce;
    let id = block_id(&header);
    println!("LOG_SLOTS_GENESIS = {LOG_SLOTS_GENESIS}");
    println!("GENESIS_STATE_ROOT = {}", hex::encode(root));
    println!("  as const array:");
    print!("const GENESIS_STATE_ROOT: [u8; 32] = [");
    for (i, b) in root.iter().enumerate() {
        if i % 16 == 0 {
            print!("\n    ");
        }
        print!("0x{b:02x}, ");
    }
    println!("\n];");
    println!("GENESIS_NONCE = {nonce}");
    println!("GENESIS_BLOCK_ID = {}", hex::encode(id));
    println!("GENESIS_TIMESTAMP = {GENESIS_TIMESTAMP}");
    println!("GENESIS_BURN_ADDRESS = {}", hex::encode(GENESIS_BURN_ADDRESS.0));
    println!("GENESIS_TARGET = {}", hex::encode(GENESIS_TARGET));
    Ok(())
}

// ---------------------------------------------------------------------------
// Live tip header snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Tip {
    height: u64,
    hash: [u8; 32],
    active_slot_count: u64,
    log_slots: u32,
    alloc_counter: u64,
}

fn tip(rpc: &Rpc) -> Result<Tip> {
    let info = rpc.call("getChainInfo", serde_json::json!([]))?;
    let height = info["height"]
        .as_u64()
        .with_context(|| format!("chain info height: {info}"))?;
    let hash_hex = info["best_hash"]
        .as_str()
        .with_context(|| format!("chain info best_hash: {info}"))?;
    let hash = hex::decode(hash_hex)
        .with_context(|| "best_hash hex")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("best_hash not 32 bytes"))?;
    let header = rpc.call("getBlockHeader", serde_json::json!([height]))?;
    let Some(header) = header.as_object() else {
        bail!("getBlockHeader returned null for height {height}");
    };
    let active_slot_count = header["active_slot_count"]
        .as_u64()
        .context("active_slot_count")?;
    let log_slots = header["log_slots"].as_u64().context("log_slots")? as u32;
    let alloc_counter = header["alloc_counter"].as_u64().context("alloc_counter")?;
    let header_hash_hex = header["hash"].as_str().context("hash")?;
    let header_hash: [u8; 32] = hex::decode(header_hash_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("header hash not 32 bytes"))?;
    anyhow::ensure!(
        header_hash == hash,
        "getBlockHeader hash disagrees with getChainInfo best_hash"
    );
    Ok(Tip {
        height,
        hash,
        active_slot_count,
        log_slots,
        alloc_counter,
    })
}

// ---------------------------------------------------------------------------
// fee mode
// ---------------------------------------------------------------------------

fn fee_mode(rpc: &Rpc) -> Result<()> {
    let t = tip(rpc)?;
    let required = fee_breakdown(3, 4, t.active_slot_count, t.log_slots).required_total;
    let multiplier = pressure_multiplier(t.active_slot_count, t.log_slots);
    println!(
        "tip height={} active_slots={} log_slots={} pressure_multiplier={}",
        t.height, t.active_slot_count, t.log_slots, multiplier
    );
    println!("required CreateLaunch fee (3-in/4-out) = {required} uNOID");
    Ok(())
}

// ---------------------------------------------------------------------------
// launch mode
// ---------------------------------------------------------------------------

struct LiveInput {
    input: TxInput,
}

fn collect_inputs(rpc: &Rpc, owner: &Address, need: usize, minimum_total: u64) -> Result<Vec<LiveInput>> {
    let owner_bech32 = owner.to_bech32();
    let slots = rpc.call("getSlotsByOwner", serde_json::json!([owner_bech32]))?;
    let Some(list) = slots.as_array() else {
        bail!("getSlotsByOwner returned non-array: {slots}");
    };
    let mut candidates: Vec<(u32, u64, u64)> = Vec::new();
    for slot in list {
        if slot["empty"].as_bool() != Some(false) {
            continue;
        }
        let slot_index = slot["slot_index"].as_u64().context("slot_index")? as u32;
        let value = slot["value"].as_u64().context("value")?;
        let creation_id = slot["creation_id"].as_u64().context("creation_id")?;
        candidates.push((slot_index, value, creation_id));
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    if candidates.len() < need {
        bail!(
            "owner {owner_bech32} has {} live UTXOs, need {need}",
            candidates.len()
        );
    }
    let selected: Vec<(u32, u64, u64)> = candidates[..need].to_vec();
    let total: u64 = selected.iter().map(|s| s.1).sum();
    if total < minimum_total {
        bail!(
            "owner {owner_bech32} top {need} UTXOs sum to {total} uNOID, need >= {minimum_total}"
        );
    }
    Ok(selected
        .into_iter()
        .map(|(slot_index, amount, creation_id)| LiveInput {
            input: TxInput {
                slot_index,
                amount,
                creation_id,
            },
        })
        .collect())
}

fn free_output_slots(rpc: &Rpc, count: usize) -> Result<Vec<u32>> {
    let salt = hex::encode([0x5a; 8]);
    let hints = rpc.call(
        "getSlotHintsSalted",
        serde_json::json!([count as u32, salt]),
    )?;
    let Some(list) = hints.as_array() else {
        bail!("getSlotHintsSalted returned non-array: {hints}");
    };
    let mut slots = Vec::with_capacity(count);
    for hint in list {
        let index = hint.as_u64().context("hint index")? as u32;
        let info = rpc.call("getSlot", serde_json::json!([index]))?;
        if info["empty"].as_bool() != Some(true) {
            bail!("hint slot {index} is not empty: {info}");
        }
        slots.push(index);
    }
    Ok(slots)
}

#[allow(clippy::too_many_arguments)]
fn build_launch_intent(
    secret: SpendSecret,
    inputs: Vec<TxInput>,
    parent_hash: [u8; 32],
    parent_height: u64,
    parent_alloc_counter: u64,
    active_slot_count: u64,
    log_slots: u32,
    epoch_anchor: [u8; 32],
    output_slots: [u32; 4],
) -> Result<NativeIntent> {
    let owner = derive_address(&secret);
    let height = parent_height + 1;
    let supply = launch_supply_from_whole(100_000).context("launch supply")?;
    let deposit = TokenDenomination::new(TOKEN_DECIMALS)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .minimum_backing(supply)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let fee = fee_breakdown(inputs.len() as u64, 4, active_slot_count, log_slots).required_total;
    let genesis = GenesisAnchor {
        chain: noid_chain::state::native_assets::native_chain_id(),
        owner,
        input: inputs[0],
    };
    let token = FungibleDefinition::from_genesis(
        &genesis,
        supply,
        IssuanceRoute::Launchpad,
        [0x71; 32],
    )
    .context("fungible definition")?;
    let launch = LaunchDefinition::from_genesis(&genesis, token.clone())
        .context("launch definition")?;
    let launch_state = LaunchState::initial(launch).context("launch state")?;
    let reserve = TokenReserve::from_definition(&token, deposit).context("token reserve")?;
    let definition = NativeOpening::Launch(LaunchOpening::Definition(launch));
    let token_opening = NativeOpening::FungibleDefinition(token);
    let reserve_opening = NativeOpening::TokenReserve(reserve);
    let state_opening = NativeOpening::Launch(LaunchOpening::State(launch_state));
    let outputs = vec![
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_slots[0],
                amount: 0,
                owner: token_opening.root(),
            },
            opening: Some(token_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_slots[1],
                amount: deposit,
                owner: reserve_opening.root(),
            },
            opening: Some(reserve_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_slots[2],
                amount: 0,
                owner: state_opening.root(),
            },
            opening: Some(state_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: output_slots[3],
                amount: inputs.iter().map(|input| input.amount).sum::<u64>() - deposit - fee,
                owner,
            },
            opening: None,
        },
    ];
    let round = NftRoundContext::new(height, parent_hash, parent_alloc_counter)?;
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
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok(intent)
}

fn launch_mode(rpc: &Rpc, secret: SpendSecret, submit: bool, out: Option<&str>) -> Result<()> {
    let t = tip(rpc)?;
    let fee = fee_breakdown(3, 4, t.active_slot_count, t.log_slots).required_total;
    let supply = launch_supply_from_whole(100_000).context("launch supply")?;
    let deposit = TokenDenomination::new(TOKEN_DECIMALS)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .minimum_backing(supply)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let owner = derive_address(&secret);
    println!(
        "tip height={} active_slots={} log_slots={} multiplier={}",
        t.height,
        t.active_slot_count,
        t.log_slots,
        pressure_multiplier(t.active_slot_count, t.log_slots)
    );
    println!(
        "launch owner {} fee={fee} deposit={deposit}",
        owner.to_bech32()
    );
    let live = collect_inputs(rpc, &owner, 3, deposit + fee)?;
    let inputs: Vec<TxInput> = live.into_iter().map(|l| l.input).collect();
    let anchor_hex = rpc
        .call("getEpochAnchor", serde_json::json!([]))?
        .as_str()
        .context("epoch anchor hex")?
        .to_string();
    let epoch_anchor: [u8; 32] = hex::decode(anchor_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("epoch anchor not 32 bytes"))?;
    let slots = free_output_slots(rpc, 4)?;
    let output_slots: [u32; 4] = [slots[0], slots[1], slots[2], slots[3]];
    println!(
        "inputs {:?} output slots {output_slots:?}",
        inputs
            .iter()
            .map(|i| (i.slot_index, i.amount))
            .collect::<Vec<_>>()
    );
    let intent = build_launch_intent(
        secret,
        inputs,
        t.hash,
        t.height,
        t.alloc_counter,
        t.active_slot_count,
        t.log_slots,
        epoch_anchor,
        output_slots,
    )?;
    anyhow::ensure!(
        intent.pages[0].body.fee == fee,
        "built fee {} != intended {fee}",
        intent.pages[0].body.fee
    );
    let bytes = intent.to_bytes()?;
    let hex_str = hex::encode(&bytes);
    println!("intent bytes={} hex chars={}", bytes.len(), hex_str.len());
    if let Some(path) = out {
        std::fs::write(path, &hex_str)?;
        println!("wrote {path}");
    }
    if submit {
        let txid = rpc.call("submitTxIntent", serde_json::json!([hex_str]))?;
        println!("submitted txid={txid}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: intentool <genesis> | <fee --rpc URL> | <launch --rpc URL --secret-hex HEX [--submit] [--out FILE]>";
    let mode = args
        .get(1)
        .map(String::as_str)
        .with_context(|| usage.to_string())?;
    match mode {
        "genesis" => genesis_mode(),
        "fee" => {
            let rpc = rpc_from_args(&args)?;
            fee_mode(&rpc)
        }
        "launch" => {
            let rpc = rpc_from_args(&args)?;
            let secret_hex = value_of(&args, "--secret-hex").context("--secret-hex required")?;
            let secret_bytes: [u8; 32] = hex::decode(secret_hex)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("secret must be 32 bytes"))?;
            let submit = args.iter().any(|a| a == "--submit");
            let out = value_of(&args, "--out");
            launch_mode(
                &rpc,
                SpendSecret::from_bytes(secret_bytes),
                submit,
                out.as_deref(),
            )
        }
        _ => bail!("{usage}"),
    }
}

fn value_of(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| {
        args.get(i + 1).cloned()
    })
}

fn rpc_from_args(args: &[String]) -> Result<Rpc> {
    let url = value_of(args, "--rpc").context("--rpc URL required")?;
    Ok(Rpc::new(&url))
}
