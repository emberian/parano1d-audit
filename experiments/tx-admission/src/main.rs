// SPDX-License-Identifier: Apache-2.0
// tx-admission lane: admission-cost asymmetry against the real noid_mempool.
//
// Source: /Users/ember/dev/parano1d-break/parano1d @ 52938433a30b185ab007afaa15166a632cf2e82e
// All fixtures are synthetic (loopback, no node, no peers). The mempool,
// decoder, admission checks, semaphore and AuthGKR verifier are the real ones.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use noid_chain::consensus::genesis::genesis_header;
use noid_chain::consensus::params::{
    owner_authorization_version, v2_1_active, v2_active, BLOCK_TIME, MIN_FEE_BASE,
};
use noid_chain::consensus::pow::block_id;
use noid_chain::consensus::tx_epoch_anchor_height_for_child;
use noid_chain::consensus::wire_limits::{MAX_MEMPOOL_TXS, MAX_TX_INTENT_BYTES_GLOBAL};
use noid_chain::fri_state::SlotValue;
use noid_chain::mempool::BlockSelectionBudget;
use noid_chain::state::ChainState;
use noid_gkr::wallet_authorization::{
    prove_paged_spend_authorization_for_version, OwnerAuthWitness, WalletAuthorizationBundle,
};
use noid_mempool::{
    AsyncMempool, AuthorizationVerificationExecutor, ChainView, MempoolConfig, SubmitError,
};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret, TxBodyHash};
use noid_tx::{
    output_bitmap_bit, PagedSpendIntent, TxBody, TxInput, TxOutput, TxPage, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

// ---------------------------------------------------------------------------
// Instrumentation: mirrors the default inline executor plus counters.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Counters {
    executed: AtomicUsize,     // tasks that actually ran under a permit
    inflight: AtomicUsize,     // currently running
    max_inflight: AtomicUsize, // peak concurrency observed
}

fn counting_executor(counters: Arc<Counters>) -> AuthorizationVerificationExecutor {
    Arc::new(move |task| {
        let current = counters.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        counters.max_inflight.fetch_max(current, Ordering::SeqCst);
        counters.executed.fetch_add(1, Ordering::SeqCst);
        let result = task();
        counters.inflight.fetch_sub(1, Ordering::SeqCst);
        result
    })
}

// ---------------------------------------------------------------------------
// Fixtures (mirror noid_mempool/src/pool.rs tests + noid_node wallet prover)
// ---------------------------------------------------------------------------

struct Fixture {
    view: ChainView,
    anchor: [u8; 32],
    secret_bytes: [u8; 32],
    owner: Address,
    live_input: TxInput,
}

/// Build a synthetic chain view with one live input slot owned by a fixed secret.
fn fixture(tip_height: u64) -> Fixture {
    let anchor_height = tx_epoch_anchor_height_for_child(tip_height + 1);
    let mut epoch_header = genesis_header();
    epoch_header.height = anchor_height;
    epoch_header.timestamp += anchor_height * BLOCK_TIME;
    let anchor = block_id(&epoch_header);

    let secret_bytes: [u8; 32] =
        core::array::from_fn(|i| (0xA5 ^ i as u8).wrapping_mul(3).wrapping_add(11));
    let secret = SpendSecret::from_bytes(secret_bytes);
    let owner = derive_address(&secret);

    let live_input = TxInput {
        slot_index: 11,
        amount: 1_000_000_000,
        creation_id: 7,
    };
    let mut state = ChainState::with_log_slots(14);
    state
        .state
        .set_slot(
            live_input.slot_index,
            SlotValue::with_owner_fields(
                live_input.amount,
                live_input.creation_id,
                owner.as_fields(),
            ),
        )
        .expect("fixture slot write");

    let headers = HashMap::from([(anchor_height, epoch_header)]);
    let view = ChainView::new(tip_height, headers, 1, state.state);
    Fixture {
        view,
        anchor,
        secret_bytes,
        owner,
        live_input,
    }
}

fn config(workers: usize, v2: bool) -> MempoolConfig {
    let mut config = MempoolConfig::default().with_auth_verify_workers(workers);
    if v2 {
        // Same class budgets as noid_mempool::config::test_config().
        config = config.with_v2_block_budgets([63, 206].map(|pages| BlockSelectionBudget {
            pages,
            live_inputs: 504,
            contract_calls: 63,
            native_headers: 0,
            native_records: 0,
            owner_authorizations: pages,
            financial: None,
        }));
    }
    config
}

/// One-page state-valid PagedSpend body spending `live_input` with a distinct fee.
fn attack_pages(anchor: [u8; 32], live_input: TxInput, owner: Address, fee: u64) -> Vec<TxPage> {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = live_input;
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 4000,
        amount: live_input.amount.saturating_sub(fee),
        owner: Address([0xB1; 32]),
    };
    vec![TxPage::new(TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
    .unwrap()]
}

fn intent_bytes(pages: &[TxPage], auth: &[u8]) -> Vec<u8> {
    PagedSpendIntent::new(pages.to_vec(), auth.to_vec())
        .expect("intent construction")
        .to_bytes()
        .expect("intent encoding")
}

/// Corrupt `bytes` so the bundle still DECODES but fails verification.
/// Walks inward from one end until `WalletAuthorizationBundle::from_bytes` accepts.
fn corrupt_decodable(bytes: &[u8], from_end: bool) -> Vec<u8> {
    let mut candidate = bytes.to_vec();
    let order: Vec<usize> = if from_end {
        (0..bytes.len()).rev().collect()
    } else {
        (0..bytes.len()).collect()
    };
    for &index in &order {
        candidate[index] ^= 0xFF;
        if WalletAuthorizationBundle::from_bytes(&candidate).is_ok() {
            return candidate;
        }
        candidate[index] ^= 0xFF;
    }
    panic!("no decodable single-byte corruption found");
}

// ---------------------------------------------------------------------------
// Measurement helpers
// ---------------------------------------------------------------------------

fn stats(samples: &[Duration]) -> (Duration, Duration, Duration) {
    let mut sorted: Vec<u128> = samples.iter().map(|d| d.as_micros()).collect();
    sorted.sort_unstable();
    let mean = sorted.iter().sum::<u128>() / sorted.len() as u128;
    let p50 = sorted[sorted.len() / 2];
    let max = sorted[sorted.len() - 1];
    (
        Duration::from_micros(mean as u64),
        Duration::from_micros(p50 as u64),
        Duration::from_micros(max as u64),
    )
}

fn label(result: &Result<TxBodyHash, SubmitError>) -> String {
    match result {
        Ok(hash) => format!("ADMITTED({hash:?})"),
        Err(SubmitError::InvalidProof(reason)) => {
            format!("InvalidProof[{}]", reason.split(':').next().unwrap_or("?"))
        }
        Err(error) => error
            .to_string()
            .split(&['(', ':', ' '])
            .next()
            .unwrap_or("?")
            .to_string(),
    }
}

/// Submit sequentially; return per-submit durations.
async fn measure_class(
    pool: &AsyncMempool,
    counters: &Arc<Counters>,
    name: &str,
    payloads: &[Vec<u8>],
) -> Vec<Duration> {
    let before = counters.executed.load(Ordering::SeqCst);
    let mut durations = Vec::with_capacity(payloads.len());
    let mut outcomes: HashMap<String, usize> = HashMap::new();
    for bytes in payloads {
        let started = Instant::now();
        let result = pool.submit_encoded(bytes.clone()).await;
        durations.push(started.elapsed());
        *outcomes.entry(label(&result)).or_default() += 1;
    }
    let count = payloads.len();
    let executed = counters.executed.load(Ordering::SeqCst) - before;
    let (mean, p50, max) = stats(&durations);
    println!(
        "{name:<34} n={count:<3} mean={mean:>10.3?} p50={p50:>10.3?} max={max:>10.3?}  verify_exec={executed:<3} outcomes={outcomes:?}"
    );
    durations
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    println!("parano1d tx-admission lane — admission cost asymmetry PoC");
    println!("target: noid_mempool @ 52938433a30b185ab007afaa15166a632cf2e82e (read-only)\n");
    println!(
        "constants: MAX_TX_INTENT_BYTES_GLOBAL={MAX_TX_INTENT_BYTES_GLOBAL} MAX_MEMPOOL_TXS={MAX_MEMPOOL_TXS}"
    );
    println!("production MempoolConfig::default(): auth_verify_workers=4 capacity=1024 max_total_intent_bytes=384MiB\n");

    let heights: [(u64, &str, bool); 2] = [
        (0, "V3 (tip 0, pre-v2)", false),
        (300_000, "V4 (tip 300000, post-v2.1)", true),
    ];

    let mut summary_tail: Option<(Duration, f64, usize, Duration, usize, Duration)> = None; // V4 stats

    for (tip, profile, v2) in heights {
        println!("===================== authorization profile: {profile} =====================");
        let fixture = fixture(tip);
        let counters = Arc::new(Counters::default());
        let pool = AsyncMempool::new(fixture.view.clone(), config(4, v2))
            .with_authorization_verification_executor(counting_executor(Arc::clone(&counters)));

        // ---- real proof generation (attacker one-time cost / honest wallet cost) ----
        let version = owner_authorization_version(tip + 1);
        let prove_started = Instant::now();
        let real_bundle = prove_paged_spend_authorization_for_version(
            &attack_pages(fixture.anchor, fixture.live_input, fixture.owner, 40_000),
            OwnerAuthWitness::new(SpendSecret::from_bytes(fixture.secret_bytes)),
            version,
        )
        .expect("real authorization proof");
        let prove_cost = prove_started.elapsed();
        let real_bytes = real_bundle.to_bytes().expect("bundle encode");
        println!(
            "real proof: generate={prove_cost:?} wire={:?}B (profile {version:?}, v2={} v2.1={})",
            real_bytes.len(),
            v2_active(tip + 1),
            v2_1_active(tip + 1)
        );

        // decodable corruptions of the real proof
        let corrupt_tail = corrupt_decodable(&real_bytes, true);
        let corrupt_head = corrupt_decodable(&real_bytes, false);

        // payload families — every family is STATE-VALID (same live input, fee >= min)
        let mk = |fee: u64, auth: &[u8]| -> Vec<u8> {
            intent_bytes(
                &attack_pages(fixture.anchor, fixture.live_input, fixture.owner, fee),
                auth,
            )
        };
        let fees = |offset: u64, n: usize| (0..n).map(move |i| 40_000 + offset + i as u64);

        let garbage: Vec<Vec<u8>> = fees(0, 20).map(|f| mk(f, &[0x42; 64])).collect();
        let framing_random_inner: Vec<Vec<u8>> = {
            // valid bundle header + pseudo-random bytes of the real proof's length
            let inner: Vec<u8> = (0..real_bytes.len() - 8)
                .map(|i| (i as u8).wrapping_mul(31).wrapping_add(7))
                .collect();
            fees(100, 20).map(|f| mk(f, &inner)).collect()
        };
        let truncated: Vec<Vec<u8>> = fees(200, 20).map(|f| mk(f, &real_bytes[..12])).collect();
        let tail: Vec<Vec<u8>> = fees(400, 20).map(|f| mk(f, &corrupt_tail)).collect();
        let head: Vec<Vec<u8>> = fees(500, 20).map(|f| mk(f, &corrupt_head)).collect();

        // ---- Phase A: sequential per-class node-side rejection cost ----
        println!(
            "\n-- A. node-side cost per rejected class (sequential, semaphore uncontended) --"
        );
        let _ = pool.submit_encoded(garbage[0].clone()).await; // warmup (lazy tables)
        let _ = counters.executed.fetch_sub(1, Ordering::SeqCst); // exclude warmup

        let d_garbage =
            measure_class(&pool, &counters, "A1 garbage auth (64B)", &garbage[1..]).await;
        let d_framing = measure_class(
            &pool,
            &counters,
            "A2 framing+random inner",
            &framing_random_inner,
        )
        .await;
        let d_trunc = measure_class(&pool, &counters, "A3 truncated bundle", &truncated).await;
        let d_tail = measure_class(&pool, &counters, "A5 real-proof, tail corrupted", &tail).await;
        let d_head = measure_class(&pool, &counters, "A6 real-proof, owner corrupted", &head).await;

        // cheap-path controls (must NOT reach verification)
        let bad_fee: Vec<Vec<u8>> = (0..5)
            .map(|i| {
                intent_bytes(
                    &attack_pages(fixture.anchor, fixture.live_input, fixture.owner, 1 + i),
                    &[0x42; 64],
                )
            })
            .collect();
        let d_fee = measure_class(&pool, &counters, "C1 fee below floor", &bad_fee).await;
        let mut bad_anchor_pages =
            attack_pages(fixture.anchor, fixture.live_input, fixture.owner, 40_000);
        bad_anchor_pages[0].body.epoch_anchor = [0x77; 32];
        let bad_anchor: Vec<Vec<u8>> = (0..5)
            .map(|i| {
                let mut pages = bad_anchor_pages.clone();
                pages[0].body.fee += i as u64;
                pages[0].body.outputs[0].amount -= i as u64;
                intent_bytes(&pages, &[0x42; 64])
            })
            .collect();
        let d_anchor = measure_class(&pool, &counters, "C2 wrong epoch anchor", &bad_anchor).await;
        let dead_input = TxInput {
            slot_index: 999_999,
            amount: 1_000_000_000,
            creation_id: 9,
        };
        let dead: Vec<Vec<u8>> = (0..5)
            .map(|i| {
                intent_bytes(
                    &attack_pages(fixture.anchor, dead_input, fixture.owner, 40_000 + i as u64),
                    &[0x42; 64],
                )
            })
            .collect();
        let d_dead = measure_class(&pool, &counters, "C3 input not in state", &dead).await;

        // ---- Phase B: occupancy burst ----
        println!(
            "\n-- B. occupancy burst: 256 distinct state-valid invalid-proof txs, 4 workers --"
        );
        let burst: Vec<Vec<u8>> = fees(10_000, 256).map(|f| mk(f, &corrupt_tail)).collect();
        let before_exec = counters.executed.load(Ordering::SeqCst);
        counters.max_inflight.store(0, Ordering::SeqCst);
        let burst_started = Instant::now();
        let mut handles = Vec::with_capacity(burst.len());
        for bytes in burst {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                let started = Instant::now();
                let result = pool.submit_encoded(bytes).await;
                (started.elapsed(), label(&result))
            }));
        }
        let mut latencies = Vec::with_capacity(handles.len());
        let mut outcomes: HashMap<String, usize> = HashMap::new();
        for handle in handles {
            let (elapsed, outcome) = handle.await.expect("burst task");
            latencies.push(elapsed);
            *outcomes.entry(outcome).or_default() += 1;
        }
        let burst_wall = burst_started.elapsed();
        let executed_burst = counters.executed.load(Ordering::SeqCst) - before_exec;
        let (mean_l, p50_l, max_l) = stats(&latencies);
        let service_rate = 256.0 / burst_wall.as_secs_f64();
        let max_conc = counters.max_inflight.load(Ordering::SeqCst);
        println!(
            "burst 256 txs: wall={burst_wall:?} throughput={service_rate:.1} tx/s rejected  verify_exec={executed_burst}  max_concurrent_verify={max_conc} (semaphore=4)"
        );
        println!(
            "per-tx admission latency under load: mean={mean_l:?} p50={p50_l:?} max={max_l:?} outcomes={outcomes:?}"
        );

        // single-worker bound for comparison
        let pool1 = AsyncMempool::new(fixture.view.clone(), config(1, v2))
            .with_authorization_verification_executor(counting_executor(Arc::clone(&counters)));
        let burst1: Vec<Vec<u8>> = fees(20_000, 64).map(|f| mk(f, &corrupt_tail)).collect();
        let started1 = Instant::now();
        let mut handles1 = Vec::with_capacity(burst1.len());
        for bytes in burst1 {
            let pool1 = pool1.clone();
            handles1.push(tokio::spawn(
                async move { pool1.submit_encoded(bytes).await },
            ));
        }
        for handle in handles1 {
            let _ = handle.await.expect("burst1 task");
        }
        println!(
            "single-worker control: 64 txs wall={:?} throughput={:.1} tx/s",
            started1.elapsed(),
            64.0 / started1.elapsed().as_secs_f64()
        );

        // ---- Phase C: attacker-side construction cost ----
        println!("\n-- C. attacker-side cost to construct one distinct queue-occupying payload --");
        let template_pages =
            attack_pages(fixture.anchor, fixture.live_input, fixture.owner, 40_000);
        let construction_started = Instant::now();
        let constructed: Vec<Vec<u8>> = (0..1000)
            .map(|i| {
                let mut pages = template_pages.clone();
                let fee = 40_000 + i as u64;
                pages[0].body.fee = fee;
                pages[0].body.outputs[0].amount = fixture.live_input.amount - fee;
                intent_bytes(&pages, &corrupt_tail)
            })
            .collect();
        let construction = construction_started.elapsed();
        let per_payload = construction / constructed.len() as u32;
        let distinct: HashSet<&[u8]> = constructed.iter().map(|b| b.as_slice()).collect();
        println!(
            "construction: 1000 distinct state-valid payloads in {construction:?} ({per_payload:?}/tx), distinct wire forms: {}/1000, payload size={:?}B",
            distinct.len(),
            constructed[0].len()
        );
        let tiny = mk(40_001, &[0x42; 64]);
        println!("minimal (decode-fail) payload wire size: {:?}B", tiny.len());

        // ---- Phase D: no-residue / recycling / no-rejection-cache controls ----
        println!("\n-- D. admission bookkeeping after rejection storm --");
        let len = pool.len().await;
        let floor = pool.fee_floor().await;
        let (in_slots, out_slots) = pool.reserved_slots().await;
        println!(
            "pool.len={len} (expect 0) fee_floor={floor} (expect MIN_FEE_BASE={MIN_FEE_BASE}) reserved_in={} reserved_out={} (expect 0/0)",
            in_slots.len(),
            out_slots.len()
        );
        assert_eq!(len, 0, "rejected storm left entries");
        assert!(
            in_slots.is_empty() && out_slots.is_empty(),
            "rejected storm reserved slots"
        );
        assert_eq!(floor, MIN_FEE_BASE, "rejected storm moved the fee floor");

        // identical-bytes resubmission still re-verifies (no rejection cache in the mempool)
        let identical = mk(40_777, &corrupt_tail);
        let exec_before = counters.executed.load(Ordering::SeqCst);
        for _ in 0..3 {
            let _ = pool.submit_encoded(identical.clone()).await;
        }
        let reverified = counters.executed.load(Ordering::SeqCst) - exec_before;
        println!(
            "identical-payload resubmission x3: verify executions = {reverified} (expect 3: no mempool-side rejection cache)"
        );

        // valid control: same state, real proof — must ADMIT (proves fixtures honest)
        let exec_before = counters.executed.load(Ordering::SeqCst);
        let valid_started = Instant::now();
        let valid = pool.submit_encoded(mk(40_000, &real_bytes)).await;
        let valid_wall = valid_started.elapsed();
        let valid_exec = counters.executed.load(Ordering::SeqCst) - exec_before;
        println!(
            "valid control: {} wall={valid_wall:?} verify_exec={valid_exec} pool.len={}",
            label(&valid),
            pool.len().await
        );
        assert!(valid.is_ok(), "valid control must admit");

        // conflict control: second spend of the now-reserved input — cheap reject, no verify
        let exec_before = counters.executed.load(Ordering::SeqCst);
        let conflict_started = Instant::now();
        let conflict = pool.submit_encoded(mk(41_000, &real_bytes)).await;
        let conflict_wall = conflict_started.elapsed();
        let conflict_exec = counters.executed.load(Ordering::SeqCst) - exec_before;
        println!(
            "conflict control (same input, admitted): {} wall={conflict_wall:?} verify_exec={conflict_exec} (expect 0)",
            label(&conflict)
        );
        assert!(matches!(conflict, Err(SubmitError::Consensus(_))));
        assert_eq!(conflict_exec, 0, "slot-conflict rejection must not verify");

        println!(
            "\nprofile summary {profile}: prove={prove_cost:?} (proof wire {}B) | reject mean garbage={:?} framing={:?} trunc={:?} tail-corrupt={:?} owner-corrupt={:?} | cheap controls fee={:?} anchor={:?} dead-input={:?} conflict={:?} | valid-admit={valid_wall:?} | burst256@4w={burst_wall:?} = {service_rate:.1}/s max_conc={max_conc} | attacker construct={per_payload:?}/tx",
            real_bytes.len(),
            stats(&d_garbage).0,
            stats(&d_framing).0,
            stats(&d_trunc).0,
            stats(&d_tail).0,
            stats(&d_head).0,
            stats(&d_fee).0,
            stats(&d_anchor).0,
            stats(&d_dead).0,
            conflict_wall,
        );
        if v2 {
            summary_tail = Some((
                burst_wall,
                service_rate,
                max_conc,
                stats(&d_tail).0,
                constructed[0].len(),
                per_payload,
            ));
        }
        println!();
    }

    // ---- Rate model (measured constants -> sustainable occupancy) ----
    println!("===================== rate model (V4 profile measurements) =====================");
    if let Some((burst_wall, service_rate, max_conc, tail_mean, payload_len, per_payload)) =
        summary_tail
    {
        let per_peer_cap = 5.0_f64; // 50 tx / 10 s, enforced on both delivery paths
        let inbound_conns = 128.0_f64; // libp2p connection limits
        let verify_ms = tail_mean.as_secs_f64() * 1000.0;
        println!(
            "measured full-pipeline reject cost (tail-corrupt): {tail_mean:?} ({verify_ms:.1} ms)"
        );
        println!("measured 4-worker drain rate: {service_rate:.1} tx/s (burst wall {burst_wall:?}, peak concurrency {max_conc})");
        let attacker_us = per_payload.as_micros() as f64;
        println!(
            "attacker construction cost: {attacker_us:.0} us/tx, payload {payload_len} B on the wire"
        );
        let peers_needed = (service_rate / per_peer_cap).ceil();
        println!("peers needed to hold all 4 verify permits busy: {peers_needed:.0} (each capped at {per_peer_cap}/s)");
        let max_inflow = inbound_conns * per_peer_cap;
        println!(
            "max anonymous inflow: {inbound_conns:.0} peers x {per_peer_cap}/s = {max_inflow:.0} tx/s = {:.1}x drain rate",
            max_inflow / service_rate
        );
        let node_cpu_s_per_tx = tail_mean.as_secs_f64();
        let attacker_s_per_tx = attacker_us * 1e-6;
        println!(
            "asymmetry ratio (node CPU s per reject / attacker construction s per tx): {:.0}x",
            node_cpu_s_per_tx / attacker_s_per_tx
        );
        let mbps = max_inflow * payload_len as f64 / 1e6;
        println!(
            "attacker bandwidth to saturate: {mbps:.0} MB/s ({max_inflow:.0} tx/s x {payload_len} B); decode-fail variant (394 B) burns only 0.12 core at the same rate"
        );
        let slots = 1024.0;
        let fill_s = slots / (max_inflow - service_rate);
        let drain_s = slots / service_rate;
        println!("delivery slots fill in {fill_s:.1}s at max inflow; honest txs are then dropped at TxDeliveries::start (both paths); drain once attacker stops: {drain_s:.1}s");
        let budget = 64.0 * 1024.0 * 1024.0;
        let holders = (budget / payload_len as f64) as usize;
        println!(
            "shared 64MiB inbound budget (Push path): {holders} in-admission {payload_len}B pushes ({:.1}% of slots) hold all of it; also filled by {} max-size intents",
            100.0 * holders as f64 / slots,
            (budget / MAX_TX_INTENT_BYTES_GLOBAL as f64) as usize
        );
    }
    println!("\nstructural constants (code, pinned commit):");
    println!("  per-peer tx admission rate  : 50 tx / 10 s (p2p TX_RELAY_RATE_MAX + node peer_tx_rate, gossip AND direct Push)");
    println!("  inbound connections         : 128 (libp2p connection_limits, behaviour.rs:539)");
    println!("  in-flight delivery slots    : 1024, non-evictable while unresolved (tx_delivery.rs MAX_TRACKED)");
    println!("  shared inbound byte budget  : 64 MiB process-global, held for full admission (inbound_budget.rs; Push path)");
    println!("  verify workers              : 4 semaphore permits (MempoolConfig::default), work-conserving on the shared Rayon pool");
    println!("  gossip global byte window   : 64 MiB / 10 s (GOSSIP_ACCEPT_BYTES_PER_WINDOW)");
    println!("ALL ASSERTIONS PASSED");
}
