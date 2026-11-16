# Cost of rejecting invalid transaction proofs

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e` (read-only).

The experiment drives `noid_mempool::AsyncMempool`, including its decoder, admission checks, four-permit verification semaphore, and AuthGKR authorization verifier. It uses a synthetic `ChainState`/`ChainView`, a genesis-derived epoch anchor, and synthetic keys. It does not start a node or connect to a peer.

Question: Can distinct transactions with valid public state claims and invalid authorization proofs occupy the bounded verification queue at less attacker CPU cost than the node spends rejecting them?

The local experiment measured 134.3 µs to construct each distinct payload and 8.34 ms for the node to reject it: a 62× ratio in CPU time. A rejected proof spends no fee and reserves no input, so the same public input can be claimed again. For direct Push, combining these measurements with the configured 50 transactions per peer per 10 seconds and 128 inbound connections gives a *modeled* maximum offer of 640 transactions/s against the measured four-worker drain of 415.5/s. Gossip has a separate global byte window that prevents offering this payload rate through gossip alone. Neither calculation establishes a live swarm's sustained delivery rate or cross-protocol impact.

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

## Finding 1: invalid proofs are expensive to reject and leave no input reservation

**Availability risk: medium, conditional on sufficient P2P delivery.** Direct Push is handled for an established peer without an `is_dispatchable` profile gate (`network.rs:8810-8932`); gossip requires mesh delivery. The Noise handshake does not require an application credential. Peer colocation penalties and connection limits still apply (`behaviour.rs:255-273`). RPC submission is operator-credential or loopback restricted (`server.rs:3850-3872`). The experiment did not establish the number of identities that a live node would admit concurrently or the rate they could deliver.

The rejection path has four relevant properties:

1. **State-validity needs no ownership.** `check_input_slots` (`pool.rs:1362-1408`) only compares the *claimed* `(amount, creation_id, owner)` against the on-chain slot value — all readable by anyone with a synced state. Ownership is established solely by the 8 ms proof check. The fee is *declared*, never collectible on rejection. The epoch anchor is deterministic from public tip height.
2. **Failed verification reserves nothing.** `pool.admit` and `admitted_input_slots` insertion happen only after proof success (`pool.rs:495-542`). The same input slot can be re-spent by the next garbage tx immediately — measured: 300+ rejections on one slot, all reaching verification, pool empty afterwards.
3. **No rejection cache.** Byte-identical resubmission re-verifies every time (measured 3/3). The transport coalesces byte-exact payloads only while in flight; varying the fee by 1 produces a new payload and txid for ~134 µs of construction.
4. **A valid proof supplies a reusable template.** The public prover generated one valid wallet proof in 675–683 ms locally. Byte changes at selected positions produced variants that decoded, passed cheap checks, and reached the full verifier before failing.

Measured numbers (M-series laptop, release, real verifier; V4 tip-300000 profile quoted, V3 tip-0 within 10%):

- Node cost per max-cost reject: **8.34 ms** of one verify worker (drain: 415.5 tx/s at 4 permits; single worker 122 tx/s; peak concurrency pinned at exactly 4).
- Attacker cost per payload: **134.3 µs** construction, 90,414 B on the wire. **CPU-time ratio: 62×.** A 394 B decode-failing payload costs the node only ~190 µs; at the configured rate limit, even 128 peers offer only ~0.12 CPU-core of this work. Bundle decoding precedes transcript replay.
- Direct-Push rate model: 128 inbound connections (`behaviour.rs:539`) × 5 tx/s per PeerId = **640 tx/s offered**, if every connection delivers at its limit. Against the measured 415.5 tx/s drain, ~84 identities at ~58 MB/s would match this laptop's drain. The code comment budgets ~84 ms per verification (`main.rs:10701`); using that estimate gives 10 identities at 4.6 MB/s. The global gossip byte window rules out this rate for gossip payloads of this size. Neither scenario was measured on a live node.

Predicted effects if the offered rate exceeds the drain (derived from measurements and code bounds, not observed in a live swarm):

- If that net arrival rate reaches the delivery tracker, its 1024 non-evictable slots fill in ~4.6 s; new transactions are then dropped at `TxDeliveries::start` (`network.rs:6595-6605`, `8913-8915`). Push traffic also faces the inbound byte budget, so this slot calculation cannot simply be applied to 1024 simultaneous Push payloads.
- On the Push path, 64 MiB permits can hold roughly 742 in-admission 90,414 B payloads. Block-body, HistoryStep, and state-segment inbound responses share this byte domain (`inbound_budget.rs`). They would wait if Push submissions held all permits; the PoC did not demonstrate this composition on a live node.
- Local wallet admissions use a clone of the mempool with the same verification `Arc<Semaphore>` (`server.rs:3529-3538`); wallet-priority CPU scheduling does not preempt ongoing inbound verification (`cpu_budget.rs:325-331`). The PoC measured 24.4 ms uncontended admission, p50 303 ms with a 256-deep queue, and more than 2 s at 1024 depth. Those are local queue measurements, not wallet latency on a live mining node.
- Four continuously busy verify permits can occupy 4 of ~10 workers in the default shared proof Rayon pool. The effect on block production was not measured.

### Recommended correction

Bound verifier work per peer and keep a short-lived negative cache for repeated authorization bytes. A cache keyed by `(logical_txid, authorization bytes hash)` catches exact retries but fee changes alter the logical transaction, so its effectiveness against variants must be tested. A cost-weighted peer rate limit would charge more for transactions that reach AuthGKR than for decode failures. A separate tx ingress byte budget would protect block and state responses from Push traffic. Reserving inputs before verification would also slow recycling, but would let invalid submissions temporarily lock honest inputs; it needs an explicit fairness design.

## Finding 2: the gossip byte window is shared across peers and message classes

**Severity: Low-Medium availability, anonymous P2P.**

`admit_tx_gossip_ingress` (`network.rs:5174-5187`) charges transaction gossip bytes against a **single process-global** 64 MiB / 10 s window shared with block announcements (`network.rs:6491-6495`). It does so before decoding, after the per-peer count check. An undecodable payload of 303,495 B, the wire cap, passes the size check and consumes the window. Five identities sending 50 such payloads each would offer 75.8 MB per window against the 64 MiB budget. If those payloads reach this ingress path, later gossip, including block announcements, receives `Ignore` until the window resets. The source configures no application-specific gossipsub score for this `Ignore` result. Delivery and identity retention have not been measured in a live swarm.

### Recommended correction

Make the byte window per-peer (like the count limit), or deduct bytes only after the payload passes the cheap validity boundary, or maintain a small per-peer share of the global window (weighted fair queueing) so one source cannot monopolize it.

## Negative and control results

- **Decode failures are cheap to reject.** Garbage, random framing, and truncated classes cost ~190 µs because decoding fails before transcript replay. Even the modeled 640/s offer consumes ~0.12 core; the expensive class requires a real-proof-shaped bundle.
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

- One laptop, one release build, four async workers; per-item costs will differ on other hardware. The 8.34 ms verify is this machine's measurement. The ~84 ms figure in `main.rs:10701` is a code comment, not a benchmark of the deployed network.
- The production executor runs verification on the shared Rayon pool (work-conserving beside PoW/HistoryStep). The PoC's inline executor measures the verification cost itself; the 4-of-10-worker CPU-share figure for production is derived from `cpu_budget.rs` plans, not measured against a mining node.
- Transport-level effects (1024 delivery slots, 64 MiB inbound semaphore, 64-byte event lanes, 64 MiB/10 s gossip window, connection caps) are established from the pinned code, libp2p configuration, and the crate's own unit tests (`tx_delivery.rs` tests pin the non-evictable in-flight behaviour), then combined with measured per-tx costs in a rate model. No multi-peer swarm was run; a live demonstration would need the full swarm harness and is the natural follow-up.
- Finding 2 (gossip window exhaustion) is a code-path argument with constants, not an executed attack; the per-window arithmetic (5 identities x 50 x 303,495 B > 64 MiB) uses the pinned limits.
- V3 and V4 authorization profiles measured identically here (8.1-8.3 ms); trade and native intent classes were not measured (they share the same semaphore and AuthGKR verify per `trade.rs:44-53`/`native.rs:65-74`; construction cost is higher, so they cannot be cheaper attack vectors).
- Whether a maximum-shape (256 KiB) decodable-but-invalid bundle costs meaningfully more than 8.34 ms to reject was not explored; PCS verification uses a fixed 65 queries, so we do not expect payload size to change verify cost, but this was not measured.
- The modeled offer needs ~58 MB/s to saturate this laptop's four workers. The 62× ratio compares CPU time only; bandwidth, connection setup, peer retention, and scheduling could change the outcome.
