# Admission-cost asymmetry at the transaction-admission layer (tx-admission lane)

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e` (read-only).

All created files are confined to this directory. The PoC drives the real `noid_mempool::AsyncMempool` — real bounded wire decoder, real state/fee/anchor/slot admission checks, real 4-permit verification semaphore, real AuthGKR (`noid_gkr`) authorization verification — against synthetic chain state (real `ChainState`/`ChainView`, real genesis-derived epoch anchor). No live node, peer, or external network was contacted; synthetic keys only.

The question this lane owned: can an attacker cheaply keep the bounded verification queues occupied with many distinct, state-valid transactions carrying invalid proofs — i.e. is the attacker's marginal cost per queue-occupying tx lower than the node's marginal cost to reject it?

**Answer: yes. Measured 62x CPU-time asymmetry per transaction, with zero marginal state cost to the attacker (failed proofs reserve nothing, inputs are freely recyclable, state-validity needs only public data), and the transport caps do not close the gap: 128 anonymous inbound connections offer 640 tx/s against a measured 415 tx/s four-worker drain. On the code's own 84ms-class verify hardware the same attack saturates with 10 identities at 4.6 MB/s.**

## The admission pipeline, measured (order from code, costs measured)

P2P delivery (gossip `network.rs:6542-6631` and direct Push `network.rs:8879-8932`, both anonymous):

| stage | cost / bound | where |
|---|---|---|
| gossip: global byte window | 64 MiB / 10 s **shared with block announcements**, checked before decode | `GOSSIP_ACCEPT_BYTES_PER_WINDOW`, `network.rs:493` |
| gossip/Push: per-peer rate | 50 tx / 10 s per PeerId (count, before decode — garbage burns attacker's own budget) | `TX_RELAY_RATE_MAX`, `network.rs:484` |
| Push: shared inbound byte budget | receiver acquires `payload_len` bytes of a process-global 64 MiB semaphore **before reading the payload**; held until admission completes | `mempool_sync_codec.rs:141-157`, `inbound_budget.rs` |
| delivery slots | 1024 in-flight, non-evictable while unresolved; full → new tx dropped | `tx_delivery.rs:21-22,126-127` |
| event lanes | gossip broadcast(64); Push `Background` lane(64) `try_send` | `event_dispatch.rs:22-29` |
| node loop: per-peer rate again | 50 tx / 10 s (`peer_tx_rate`) | `noid_node/src/main.rs:9819-9890` |
| decode + canonical + shape | ~190-210 µs incl. spawn hop | measured A1-A3 |
| cheap state checks under lock (fee floor, epoch anchor, pool conflict, input liveness, output emptiness) | 116-184 µs, **0 verify executions** | measured C1-C3, `pool.rs:1243-1306` |
| 4-permit semaphore → spawn_blocking → AuthGKR verify | **8.34 ms** (V4; 9.06 ms V3) for a decodable-but-invalid full-size proof; ~190-210 µs if it fails decode | measured A5/A6 vs A1-A3 |
| final admission under lock | only on proof success; failed txs reserve NOTHING | `pool.rs:407-514` |

Production (`noid_node/src/main.rs:2146-2160`) uses `MempoolConfig::default()`: `auth_verify_workers = 4`, capacity 1024, 384 MiB byte cap. The executor routes verification onto the shared Rayon CPU pool as a **work-conserving** `InboundVerify` phase beside PoW and HistoryStep (`noid_miner/src/cpu_budget.rs:324-331`).

## Finding 1: sustained anonymous occupancy of all bounded admission resources at a ~62x CPU discount

**Severity: Medium availability. Reachability: anonymous P2P — any number of free libp2p identities over TCP (noise handshake needs no credential); direct Push requires only an established connection (the MempoolSync request handler is the one request-response arm with no `is_dispatchable` profile gate, `network.rs:8810-8932`); gossip requires mesh grafting, which peer scores do not block (no app scoring; only >8-identities-per-IP colocation penalties, `behaviour.rs:255-273`). NOT reachable via RPC: `paranoid_submitTxIntent`/`paranoid_walletSend` are operator-credential or loopback (server.rs:3850-3872; reusing the rpc-api lane's reachability result).**

Mechanism — every piece the attacker needs is public and nothing is consumed:

1. **State-validity needs no ownership.** `check_input_slots` (`pool.rs:1362-1408`) only compares the *claimed* `(amount, creation_id, owner)` against the on-chain slot value — all readable by anyone with a synced state. Ownership is established solely by the 8 ms proof check. The fee is *declared*, never collectible on rejection. The epoch anchor is deterministic from public tip height.
2. **Failed verification reserves nothing.** `pool.admit` and `admitted_input_slots` insertion happen only after proof success (`pool.rs:495-542`). The same input slot can be re-spent by the next garbage tx immediately — measured: 300+ rejections on one slot, all reaching verification, pool empty afterwards.
3. **No rejection cache.** Byte-identical resubmission re-verifies every time (measured 3/3). The only dedup is the transport-layer byte-exact in-flight coalescer; varying the fee by 1 produces a new payload and new txid for ~134 µs of construction.
4. **One real proof is a perpetual template.** The attacker generates one valid wallet proof once (measured 675-683 ms locally, public prover), then flips a byte at different positions: every variant decodes, passes all cheap checks, and burns a full verify before failing.

Measured numbers (M-series laptop, release, real verifier; V4 tip-300000 profile quoted, V3 tip-0 within 10%):

- Node cost per max-cost reject: **8.34 ms** of one verify worker (drain: 415.5 tx/s at 4 permits; single worker 122 tx/s; peak concurrency pinned at exactly 4).
- Attacker cost per payload: **134.3 µs** construction, 90,414 B on the wire. **CPU-time asymmetry: 62x.** Minimal decode-fail payload is 394 B but costs the node only ~190 µs (that variant is capped at 0.12 core by the rate limits and cannot saturate — the cheap-attack version of this finding is genuinely blocked by check ordering: bundle decode happens before transcript replay).
- Saturation arithmetic: 128 inbound connections (`behaviour.rs:539`) x 5 tx/s per PeerId = **640 tx/s offered vs 415.5 tx/s drain (1.5x)** → ~84 identities at ~58 MB/s saturate this laptop. The code comment itself budgets ~84 ms per verification (`main.rs:10701`); at that cost 10 identities at 4.6 MB/s saturate. Occupancy is structural; only its price is hardware-dependent.

Effects while saturated (from measured latencies + code bounds):

- The 1024 non-evictable delivery slots fill in ~4.6 s; **new honest txs are then dropped** at `TxDeliveries::start` on both paths (`network.rs:6595-6605`, `8913-8915`).
- The 64 MiB process-global inbound budget (Push path) is held by ~742 in-admission 90 KB payloads for their whole queue stay (~2.5 s at saturation) — **block-body, HistoryStep and state-segment inbound responses share this one byte domain** (`inbound_budget.rs`) and stall behind tx garbage.
- Honest *and local* wallet admissions queue on the same semaphore: the RPC wallet path is `mempool.clone()` with a different executor but the **same `Arc<Semaphore>`** (`server.rs:3529-3538`), and wallet-priority CPU admission does not preempt inbound verification (`cpu_budget.rs:325-331`). Uncontended admission is 24.4 ms; at a 256-deep queue the measured p50 is already 303 ms; at 1024-deep it exceeds 2 s.
- Four permanently busy verify permits are 4 of ~10 workers of the node's shared proof Rayon pool — a standing ~40% tax on block-production CPU on the default plan.

### Recommended correction

Make rejection cost scale for the repeat offender: (a) a bounded negative cache keyed by `(logical_txid, authorization bytes hash)` with short TTL, so byte-near-identical resubmission cannot re-verify; (b) charge the per-peer rate budget *by verification cost class* (e.g. a decodable full-size proof costs 10 tokens, decode-fail costs 1) instead of by count; (c) reserve input slots from first *reach-verification*, not first *admission*, releasing on reject — this makes input recycling cost real UTXOs; (d) give the shared inbound byte domain a tx-only sub-budget so tx pushes cannot starve block/state sync. (a) alone removes the 62x discount for the observed attack shape.

## Finding 2: the global gossip byte window is exhaustible by ~5 identities of pure garbage, dropping honest tx AND block gossip

**Severity: Low-Medium availability, anonymous P2P.**

`admit_tx_gossip_ingress` (`network.rs:5174-5187`) admits gossip bytes from a **single process-global** 64 MiB / 10 s window shared with block announcements (`network.rs:6491-6495`), checked *before* decode and before any per-payload work beyond the per-peer count. Undecodable 303,495 B payloads (the wire cap) pass the size check and consume the window. Five identities at 50 tx/10 s each ship 75.8 MB per window against the 64 MB budget (~6.4 MB/s of garbage, no state validity, no proofs): from that point every further gossip message — honest transactions *and block announcements* — is `Ignore`-dropped for the rest of the window. `Ignore` carries no gossipsub peer-score penalty (`behaviour.rs` scoring has no app-specific behaviour scores), so the exhaustion is sustainable and unpenalized. This is reasoned from the code paths and libp2p constants, not executed against a live swarm (see limits).

### Recommended correction

Make the byte window per-peer (like the count limit), or deduct bytes only after the payload passes the cheap validity boundary, or maintain a small per-peer share of the global window (weighted fair queueing) so one source cannot monopolize it.

## Negative and control results

- **The cheap-attack variant is blocked.** Garbage/framing-random/truncated classes cost ~190 µs (decode fails inside the verification task before transcript replay) and cannot saturate: 640/s offered x 190 µs = 0.12 core. The expensive class requires a real-proof-shaped bundle.
- **Cheap checks really do run pre-verification, at near-zero cost, with zero verify executions**: fee-below-floor 116 µs, wrong epoch anchor 174 µs, input-not-in-state 174 µs, and slot-conflict-after-admission 338 µs — all `verify_exec=0` (executor-observed).
- **Zero residue after a 300+-rejection storm**: pool length 0, no reserved input/output slots, dynamic fee floor still `MIN_FEE_BASE` — rejected txs cannot poison the floor, the pool, or the slot index.
- **No replacement/churn vector**: admission is strictly first-seen-wins (`SlotConflict` for competing spends, `AlreadyAdmitted` idempotent); there is no fee-based replacement to reset eviction, and eviction is block-driven only (`on_new_block`, `pool.rs:736-846`).
- **Oversized-authorization path is unreachable through the decoder**: `PagedSpendIntent::new` enforces the 256 KiB cap at construction and decode re-enforces it; the mempool's `ProofTooLarge` is defense-in-depth (the PoC drops its planned oversized class for this reason).
- **Valid control admits**: a real proof over the same synthetic state admits in 24.4 ms (one verification + bookkeeping), proving the fixtures are honest and the rejection costs are not artifacts of broken state.
- **Identical-payload resubmission re-verifies 3/3 times** — the mempool itself has no rejection cache; only the transport coalesces byte-exact duplicates while in flight.
- **The per-peer rate limiter counts before decode** — an attacker's garbage burns its own per-peer budget; the limiter is real, it is just per-identity and there are 128 inbound connection slots.
- **RPC submission is not an anonymous surface** (rpc-api lane's result): `paranoid_submitTxIntent` is operator-allowlisted; a non-loopback listener without credentials refuses to start.
- **No panics or state corruption** on any exercised path; every garbage class surfaces as a typed `SubmitError`.

## PoC implementation and reproduction

`src/main.rs` (single binary, path-pinned to the read-only checkout):

1. Builds synthetic `ChainView` fixtures exactly as `noid_mempool`'s own tests do (genesis-derived epoch anchor via `tx_epoch_anchor_height_for_child`, `ChainState::with_log_slots`, one live slot), mirroring `noid_node`'s wallet-prover test for owner/witness shape.
2. Generates one real wallet authorization proof with the public prover (`prove_paged_spend_authorization_for_version`) at both the V3 (tip 0) and V4 (tip 300,000) profiles; derives two decodable corruptions (tail/owner byte-flip found by walking inward until `WalletAuthorizationBundle::from_bytes` still accepts).
3. Installs a counting executor (inline execution like the default, plus peak-concurrency/executed counters) and measures per-class sequential rejection cost, a 256-tx occupancy burst at 4 and 1 workers, attacker-side construction of 1000 distinct payloads, and all controls above, asserting each expected outcome.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final output includes (`results.txt` holds the complete tee'd run):

```text
A5 real-proof, tail corrupted      n=20  mean=   8.340ms ... outcomes={"InvalidProof[authorization verify]": 20}
C1 fee below floor                 n=5   mean= 116.000µs ... verify_exec=0   outcomes={"consensus": 5}
burst 256 txs: wall=616.112208ms throughput=415.5 tx/s rejected  verify_exec=256  max_concurrent_verify=4 (semaphore=4)
construction: 1000 distinct state-valid payloads in 134.26125ms (134.261µs/tx), distinct wire forms: 1000/1000, payload size=90414B
pool.len=0 ... fee_floor=5000 ... reserved_in=0 reserved_out=0
asymmetry ratio (node CPU s per reject / attacker construction s per tx): 62x
delivery slots fill in 4.6s at max inflow; honest txs are then dropped at TxDeliveries::start (both paths)
ALL ASSERTIONS PASSED
```

## Assurance limits

- One laptop, one release build, 4 async workers; per-item costs will differ on other hardware. The 8.34 ms verify is this machine's cost — the audited code itself budgets ~84 ms (`main.rs:10701`), which would make every occupancy conclusion easier (10 identities instead of 84); no configuration was found where the direction reverses.
- The production executor runs verification on the shared Rayon pool (work-conserving beside PoW/HistoryStep). The PoC's inline executor measures the verification cost itself; the 4-of-10-worker CPU-share figure for production is derived from `cpu_budget.rs` plans, not measured against a mining node.
- Transport-level effects (1024 delivery slots, 64 MiB inbound semaphore, 64-byte event lanes, 64 MiB/10 s gossip window, connection caps) are established from the pinned code, libp2p configuration, and the crate's own unit tests (`tx_delivery.rs` tests pin the non-evictable in-flight behaviour), then combined with measured per-tx costs in a rate model. No multi-peer swarm was run; a live demonstration would need the full swarm harness and is the natural follow-up.
- Finding 2 (gossip window exhaustion) is a code-path argument with constants, not an executed attack; the per-window arithmetic (5 identities x 50 x 303,495 B > 64 MiB) uses the pinned limits.
- V3 and V4 authorization profiles measured identically here (8.1-8.3 ms); trade and native intent classes were not measured (they share the same semaphore and AuthGKR verify per `trade.rs:44-53`/`native.rs:65-74`; construction cost is higher, so they cannot be cheaper attack vectors).
- Whether a maximum-shape (256 KiB) decodable-but-invalid bundle costs meaningfully more than 8.34 ms to reject was not explored; PCS verification uses a fixed 65 queries, so we do not expect payload size to change verify cost, but this was not measured.
- Attacker bandwidth is the binding constraint on fast hardware (~58 MB/s to saturate this laptop's 4 workers); this is what keeps Finding 1 at Medium rather than High severity. The 62x ratio is CPU-time-only and ignores bandwidth, memory, and connection setup on both sides.
