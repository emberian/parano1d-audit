// halt-reorg spamtool: build, GKR-prove and submit fat PagedSpendIntents
// against the live halt-reorg devnet (m24 production geometry).
//
// One intent = one funding input consumed + N dense outputs: N-1 zero-value
// "parking" outputs at a never-spent address (these are the net-new live
// slots that grow active_slot_count) + one change output back to the funding
// address (recycles the unspent value into the next intent's input).
//
// Modes:
//   address  --secret-hex HEX [--parking-hex HEX]
//             print the derived funding (and parking) addresses
//   once     --rpc URL --secret-hex HEX --parking-hex HEX [--outputs N]
//             [--submit] [--exclude-slots 1,2,3]
//             build+prove+submit ONE intent with full timing breakdown
//   batch    --rpc URL --secret-hex HEX --parking-hex HEX --count N
//             [--parallel M] [--outputs N] [--exclude-slots ...] [--json]
//             build+prove N intents (distinct funding inputs, parallel
//             proving), submit each, print one JSON line per intent
//
// The tool is stateless across invocations: the funding pool is discovered
// from getSlotsByOwner(funding) each run; --exclude-slots removes inputs
// already claimed by in-flight (submitted, unconfirmed) intents. Output
// slots come from getSlotHintsSalted (distinct random salt per intent) and
// are re-validated empty via getSlot before use.

use anyhow::{bail, Context, Result};
use noid_chain::consensus::params::owner_authorization_version;
use noid_chain::consensus::{fee_breakdown, pressure_multiplier};
use noid_gkr::wallet_authorization::prove_paged_spend_authorization_for_version;
use noid_gkr::OwnerAuthWitness;
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::{
    TxBody, TxInput, TxOutput, TxPage, PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT,
};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

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
                .timeout(std::time::Duration::from_secs(180))
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
        let payload: serde_json::Value =
            response.json().with_context(|| format!("rpc {method} decode"))?;
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
// Live tip snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Tip {
    height: u64,
    active_slot_count: u64,
    log_slots: u32,
}

fn tip(rpc: &Rpc) -> Result<Tip> {
    let info = rpc.call("getChainInfo", serde_json::json!([]))?;
    let height = info["height"]
        .as_u64()
        .with_context(|| format!("chain info height: {info}"))?;
    let header = rpc.call("getBlockHeader", serde_json::json!([height]))?;
    let Some(header) = header.as_object() else {
        bail!("getBlockHeader returned null for height {height}");
    };
    let active_slot_count = header["active_slot_count"]
        .as_u64()
        .context("active_slot_count")?;
    let log_slots = header["log_slots"].as_u64().context("log_slots")? as u32;
    Ok(Tip {
        height,
        active_slot_count,
        log_slots,
    })
}

// ---------------------------------------------------------------------------
// Funding pool discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Utxo {
    slot_index: u32,
    amount: u64,
    creation_id: u64,
}

/// Live UTXOs owned by `owner`, optionally minus `exclude` (inputs already
/// claimed by in-flight intents), sorted largest-first.
fn funding_pool(
    rpc: &Rpc,
    owner: &Address,
    exclude: &[u32],
    min_amount: u64,
) -> Result<Vec<Utxo>> {
    let owner_bech32 = owner.to_bech32();
    let slots = rpc.call("getSlotsByOwner", serde_json::json!([owner_bech32]))?;
    let Some(list) = slots.as_array() else {
        bail!("getSlotsByOwner returned non-array: {slots}");
    };
    let mut candidates: Vec<Utxo> = Vec::new();
    for slot in list {
        if slot["empty"].as_bool() != Some(false) {
            continue;
        }
        let slot_index = slot["slot_index"].as_u64().context("slot_index")? as u32;
        if exclude.contains(&slot_index) {
            continue;
        }
        let amount = slot["value"].as_u64().context("value")?;
        let creation_id = slot["creation_id"].as_u64().context("creation_id")?;
        if amount >= min_amount {
            candidates.push(Utxo {
                slot_index,
                amount,
                creation_id,
            });
        }
    }
    candidates.sort_by(|a, b| b.amount.cmp(&a.amount));
    Ok(candidates)
}

// ---------------------------------------------------------------------------
// Empty output-slot discovery
// ---------------------------------------------------------------------------

fn next_salt(counter: &AtomicU64) -> String {
    let n = counter.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let word = nanos ^ (n.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    hex::encode(word.to_le_bytes())
}

/// `count` validated-empty slot indices, excluding `used`.
fn empty_output_slots(
    rpc: &Rpc,
    count: usize,
    used: &mut std::collections::HashSet<u32>,
    salt_counter: &AtomicU64,
) -> Result<(Vec<u32>, u64)> {
    let started = Instant::now();
    let mut out = Vec::with_capacity(count);
    let salt = next_salt(salt_counter);
    let hints = rpc.call(
        "getSlotHintsSalted",
        serde_json::json!([count as u32, salt]),
    )?;
    let Some(list) = hints.as_array() else {
        bail!("getSlotHintsSalted returned non-array: {hints}");
    };
    for hint in list {
        if out.len() == count {
            break;
        }
        let index = hint.as_u64().context("hint index")? as u32;
        if used.contains(&index) {
            continue;
        }
        let info = rpc.call("getSlot", serde_json::json!([index]))?;
        if info["empty"].as_bool() != Some(true) {
            continue;
        }
        used.insert(index);
        out.push(index);
    }
    if out.len() < count {
        bail!(
            "only {} empty hints for {} requested (salt {salt})",
            out.len(),
            count
        );
    }
    Ok((out, started.elapsed().as_millis() as u64))
}

// ---------------------------------------------------------------------------
// Intent construction
// ---------------------------------------------------------------------------

struct BuiltIntent {
    intent_bytes: Vec<u8>,
    txid: String,
    fee: u64,
    input_slot: u32,
    change_slot: u32,
    output_slots: Vec<u32>,
    build_ms: u64,
    prove_ms: u64,
    auth_bytes: usize,
}

/// Fee with boundary margin: if the current count is within `margin` slots
/// below a multiplier boundary (50/75/90% of the domain), compute the fee at
/// the boundary so the intent survives the multiplier step that lands while
/// it waits in the mempool.
fn margined_fee(outputs: u64, active_slot_count: u64, log_slots: u32, margin: u64) -> u64 {
    let cap = 1u64 << log_slots;
    let mut effective = active_slot_count;
    for &boundary in &[
        cap / 2,
        cap * 3 / 4,
        cap * 9 / 10,
    ] {
        if active_slot_count < boundary && active_slot_count + margin >= boundary {
            effective = boundary;
        }
    }
    fee_breakdown(1, outputs, effective, log_slots).required_total
}

#[allow(clippy::too_many_arguments)]
fn build_intent(
    secret_bytes: [u8; 32],
    parking: &Address,
    funding: &Address,
    utxo: Utxo,
    outputs: usize,
    output_slots: &[u32],
    epoch_anchor: [u8; 32],
    fee: u64,
    child_height: u64,
) -> Result<BuiltIntent> {
    let started = Instant::now();
    let auth_version = owner_authorization_version(child_height);
    let pages_needed = outputs.div_ceil(2);
    anyhow::ensure!(
        pages_needed <= 128,
        "{outputs} outputs need {pages_needed} pages (> 128)"
    );
    anyhow::ensure!(outputs >= 2, "need at least 2 outputs (dense page shape)");
    anyhow::ensure!(output_slots.len() == outputs, "slot count mismatch");
    let change_amount = utxo
        .amount
        .checked_sub(fee)
        .context("funding UTXO smaller than fee")?;

    // Dense live outputs across all pages: the change is the LAST live
    // output; everything before it is zero-value parking dust.
    let mut page_bodies: Vec<TxBody> = Vec::with_capacity(pages_needed);
    for page_index in 0..pages_needed {
        let mut bitmap: u16 = 0;
        if page_index == 0 {
            bitmap |= 1 << 0; // live input slot 0
            bitmap |= PAGED_SPEND_START_BIT;
        }
        let mut body_outputs = [TxOutput::dummy(); 2];
        for (slot_in_page, output) in body_outputs.iter_mut().enumerate() {
            let flat = page_index * 2 + slot_in_page;
            if flat < outputs {
                bitmap |= 1 << (8 + slot_in_page);
                let is_change = flat + 1 == outputs;
                *output = TxOutput {
                    slot_index: output_slots[flat],
                    amount: if is_change { change_amount } else { 0 },
                    owner: if is_change { *funding } else { *parking },
                };
            }
        }
        if page_index + 1 == pages_needed {
            bitmap |= PAGED_SPEND_END_BIT;
        }
        let mut inputs = [TxInput::dummy(); 8];
        if page_index == 0 {
            inputs[0] = TxInput {
                slot_index: utxo.slot_index,
                amount: utxo.amount,
                creation_id: utxo.creation_id,
            };
        }
        page_bodies.push(TxBody {
            epoch_anchor,
            fee: if page_index == 0 { fee } else { 0 },
            input_owner: *funding,
            inputs,
            outputs: body_outputs,
            validity_bitmap: bitmap,
            is_coinbase: false,
        });
    }
    let pages: Vec<TxPage> = page_bodies
        .iter()
        .map(|body| TxPage::new(body.clone()))
        .collect::<Result<_, _>>()
        .context("page shape rejected")?;
    let build_ms = started.elapsed().as_millis() as u64;

    let prove_started = Instant::now();
    let secret = SpendSecret::from_bytes(secret_bytes);
    let bundle = prove_paged_spend_authorization_for_version(
        &pages,
        OwnerAuthWitness::new(secret),
        auth_version,
    )
    .context("GKR owner authorization proof")?;
    let prove_ms = prove_started.elapsed().as_millis() as u64;
    let auth = bundle.to_bytes()?;
    let auth_bytes_len = auth.len();
    let intent =
        noid_tx::PagedSpendIntent::new(pages, auth).context("intent assembly")?;
    let txid = hex::encode(intent.logical_txid().0);
    let intent_bytes = intent.to_bytes()?;
    Ok(BuiltIntent {
        intent_bytes,
        txid,
        fee,
        input_slot: utxo.slot_index,
        change_slot: output_slots[outputs - 1],
        output_slots: output_slots.to_vec(),
        build_ms,
        prove_ms,
        auth_bytes: auth_bytes_len,
    })
}

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

fn value_of(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn secret_from(args: &[String], flag: &str) -> Result<SpendSecret> {
    let hex_str = value_of(args, flag).with_context(|| format!("{flag} required"))?;
    let bytes: [u8; 32] = hex::decode(hex_str)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("{flag} must be 32 bytes"))?;
    Ok(SpendSecret::from_bytes(bytes))
}

fn exclude_slots(args: &[String]) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    if let Some(list) = value_of(args, "--exclude-slots") {
        for part in list.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            out.push(part.parse::<u32>().context("bad --exclude-slots entry")?);
        }
    }
    Ok(out)
}

fn address_mode(args: &[String]) -> Result<()> {
    let secret = secret_from(args, "--secret-hex")?;
    println!("funding={}", derive_address(&secret).to_bech32());
    if let Ok(parking) = secret_from(args, "--parking-hex") {
        println!("parking={}", derive_address(&parking).to_bech32());
    }
    Ok(())
}

struct Plan {
    rpc: Rpc,
    secret_bytes: [u8; 32],
    funding: Address,
    parking: Address,
    outputs: usize,
    exclude: Vec<u32>,
}

fn plan_from(args: &[String]) -> Result<Plan> {
    let url = value_of(args, "--rpc").context("--rpc URL required")?;
    // SpendSecret is zeroize-on-drop and not Clone; keep the raw bytes so
    // parallel proving threads can each import their own copy.
    let secret_hex = value_of(args, "--secret-hex").context("--secret-hex required")?;
    let secret_bytes: [u8; 32] = hex::decode(&secret_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("--secret-hex must be 32 bytes"))?;
    let parking_hex = value_of(args, "--parking-hex").context("--parking-hex required")?;
    let parking_bytes: [u8; 32] = hex::decode(&parking_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("--parking-hex must be 32 bytes"))?;
    let funding = derive_address(&SpendSecret::from_bytes(secret_bytes));
    let parking = derive_address(&SpendSecret::from_bytes(parking_bytes));
    let outputs = value_of(args, "--outputs")
        .map(|v| v.parse::<usize>())
        .transpose()
        .context("bad --outputs")?
        .unwrap_or(255);
    Ok(Plan {
        rpc: Rpc::new(&url),
        secret_bytes,
        funding,
        parking,
        outputs,
        exclude: exclude_slots(args)?,
    })
}

fn run_intents(plan: &Plan, count: usize, submit: bool, parallel: usize) -> Result<()> {
    let t = tip(&plan.rpc)?;
    if t.height < 25 {
        bail!("tip height {} below 25 (V4 auth window); wait for V2.1 activation", t.height);
    }
    let anchor_hex = plan
        .rpc
        .call("getEpochAnchor", serde_json::json!([]))?
        .as_str()
        .context("epoch anchor hex")?
        .to_string();
    let epoch_anchor: [u8; 32] = hex::decode(anchor_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("epoch anchor not 32 bytes"))?;
    // Boundary margin: two blocks of full-rate growth (2 intents x 255
    // outputs) keeps the fee valid while the intent sits in the mempool.
    let fee = margined_fee(plan.outputs as u64, t.active_slot_count, t.log_slots, 1020);
    let pool = funding_pool(&plan.rpc, &plan.funding, &plan.exclude, fee.saturating_add(1))?;
    if pool.len() < count {
        bail!(
            "funding pool has {} UTXOs (>= {fee} uNOID each), need {count}; top up via walletSend",
            pool.len()
        );
    }
    println!(
        "{}",
        json!({
            "tip": {
                "height": t.height,
                "active_slot_count": t.active_slot_count,
                "log_slots": t.log_slots,
                "multiplier": pressure_multiplier(t.active_slot_count, t.log_slots),
                "fee": fee,
                "outputs_per_intent": plan.outputs,
                "pool_utxos": pool.len(),
            }
        })
    );

    // Reserve distinct inputs + validated empty output slots for each intent.
    let salt_counter = AtomicU64::new(0);
    let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut specs: Vec<(Utxo, Vec<u32>, u64)> = Vec::with_capacity(count);
    for i in 0..count {
        let (slots, hint_ms) =
            empty_output_slots(&plan.rpc, plan.outputs, &mut used, &salt_counter)?;
        specs.push((pool[i], slots, hint_ms));
    }
    let child_height = t.height + 1;

    // Prove in parallel, submit serially.
    let parallel = parallel.max(1);
    let next_spec = AtomicU64::new(0);
    let built: Mutex<Vec<Option<(usize, BuiltIntent, u64)>>> =
        Mutex::new((0..count).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..parallel {
            scope.spawn(|| loop {
                let i = next_spec.fetch_add(1, Ordering::SeqCst) as usize;
                if i >= count {
                    break;
                }
                let (utxo, slots, hint_ms) = &specs[i];
                match build_intent(
                    plan.secret_bytes,
                    &plan.parking,
                    &plan.funding,
                    *utxo,
                    plan.outputs,
                    slots,
                    epoch_anchor,
                    fee,
                    child_height,
                ) {
                    Ok(built_intent) => {
                        built.lock().unwrap()[i] = Some((i, built_intent, *hint_ms));
                    }
                    Err(error) => {
                        eprintln!("intent {i} build/prove failed: {error:#}");
                    }
                }
            });
        }
    });

    let mut ok = 0usize;
    let mut submitted = Vec::new();
    for entry in built.into_inner().unwrap().into_iter().flatten() {
        let (i, intent, hint_ms) = entry;
        let mut line = serde_json::Map::new();
        line.insert("i".into(), json!(i));
        line.insert("txid".into(), json!(intent.txid));
        line.insert("input_slot".into(), json!(intent.input_slot));
        line.insert("change_slot".into(), json!(intent.change_slot));
        line.insert("outputs".into(), json!(plan.outputs));
        line.insert("fee".into(), json!(intent.fee));
        line.insert("hint_ms".into(), json!(hint_ms));
        line.insert("build_ms".into(), json!(intent.build_ms));
        line.insert("prove_ms".into(), json!(intent.prove_ms));
        line.insert("auth_bytes".into(), json!(intent.auth_bytes));
        line.insert("intent_bytes".into(), json!(intent.intent_bytes.len()));
        if submit {
            let submit_started = Instant::now();
            let hex_str = hex::encode(&intent.intent_bytes);
            let submit_field = match plan
                .rpc
                .call("submitTxIntent", serde_json::json!([hex_str]))
            {
                Ok(_) => {
                    ok += 1;
                    submitted.push(intent.txid.clone());
                    json!("ok")
                }
                Err(error) => json!(format!("error: {error}")),
            };
            line.insert("submit".into(), submit_field);
            line.insert(
                "submit_ms".into(),
                json!(submit_started.elapsed().as_millis() as u64),
            );
        } else {
            ok += 1;
        }
        println!("{}", serde_json::Value::Object(line));
    }
    println!(
        "{}",
        json!({
            "summary": {
                "requested": count,
                "built": ok,
                "submitted": submitted.len(),
                "height": t.height,
                "active_slot_count": t.active_slot_count,
                "fee": fee,
                "outputs_per_intent": plan.outputs,
            }
        })
    );
    if ok == 0 {
        bail!("no intents built");
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: spamtool <address --secret-hex HEX [--parking-hex HEX]> \
| <once --rpc URL --secret-hex HEX --parking-hex HEX [--outputs N] [--submit] [--exclude-slots a,b]> \
| <batch --rpc URL --secret-hex HEX --parking-hex HEX --count N [--parallel M] [--outputs N] [--exclude-slots a,b]>";
    let mode = args
        .get(1)
        .map(String::as_str)
        .with_context(|| usage.to_string())?;
    match mode {
        "address" => address_mode(&args[2..]),
        "once" => {
            let plan = plan_from(&args[2..])?;
            let submit = args.iter().any(|a| a == "--submit");
            run_intents(&plan, 1, submit, 1)
        }
        "batch" => {
            let plan = plan_from(&args[2..])?;
            let count = value_of(&args[2..], "--count")
                .context("--count required")?
                .parse::<usize>()
                .context("bad --count")?;
            let parallel = value_of(&args[2..], "--parallel")
                .map(|v| v.parse::<usize>())
                .transpose()
                .context("bad --parallel")?
                .unwrap_or(1);
            run_intents(&plan, count, true, parallel)
        }
        _ => bail!("{usage}"),
    }
}
