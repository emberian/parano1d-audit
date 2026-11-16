# Local devnet composition: serving pressure and snapshot selection

Target: pinned checkout `52938433a30b185ab007afaa15166a632cf2e82e`. The experiments use synthetic wallets, private data directories, and loopback P2P/RPC. They did not contact the live network. Measurements and complete phase output are in `results.txt`; node logs remain under `logs/`.

The largest measured anonymous effect was a serving stall. A peer that opened non-reading mempool streams held one node's 64 MiB outbound byte budget and delayed a fresh joiner's state installation. The joiner still learned reference-chain headers while its installed height was zero: its HeaderDAG selected height 150 ([saved log check](header-progress-review.txt)). Phase 7 therefore measured state-sync delay while header selection continued.

Phase 8 compared a clock-shifted 61-block fork with a 31-block reference chain on a small local network. A fresh joining node had both providers as seeds. It rejected the alternate fork's future-dated tip at the release node's HistoryStep drift gate (`logs/p8-victim.log:218`) and installed the reference chain at height 31, matching its hash. This is a **positive control for the tip-drift check**. The related C26 component test uses a future-dated *intermediate* header followed by an in-drift tip and a stubbed recursive terminal (`../malicious-node/`); that release-node configuration remains untested.

These runs establish a bounded serving delay and rejection of the clock-shifted tip. They do not measure the work cost, terminal generation, or peer behavior for the intermediate-header case on the live network.

## Topology

```
                         loopback 127.0.0.1 / ports 196xx
  [anon-mempool-pin] ──▶ ┌──────────────────────────────────────────┐
  anonymous libp2p peer  │  n1-pool  extminer,--genesis, M1+O1 keys │ ◀── [W1 worker]
  (no creds/profile):    │  p2p 19610  rpc 19611                   │     PoW grind
  4 parked /sync/mempool │  C17 SERVING NODE (loaded => 11 MB      │
  /4 pulls, never reads  │  stalled serves) + chain producer       │
                         │   ▲ p2p seeds                            │
                         │   │                                      │
                         │ n2-relay  ordinary  p2p 19620 rpc 19621  │  C20-corrupted
                         │   ▲ p2p seeds       (supply diverged)    │  (phase 2)
                         │   │                                      │
                         │ n3-syncer / n3b / n3c   fresh joiners    │
                         │                                          │
  [W4 worker]──RPC──────▶│ n4-pool2  extminer, M4 key 19640/19641   │  C15 template
                         │                                          │  template consumer
                         │ n5-tip  throwaway (U19) 19720/19721      │
                         └──────────────────────────────────────────┘

  phase 6 (controlled build input):
  [bin/parano1d-tampered on a copy of n1's chain] ◀── [bin/wallet-probe]
     foreign HistoryStep matrix (C16) ; GUI backend model reports it authenticated (C24)

  phase 7 (multiple peers): 1/2/4 x [anon-mempool-pin] + N x [anon-push-storm] vs n1
  phase 4 (docker --network none / hbox): [default-dialect] <-p2p-> [isolated-v2.1-dialect] (C9)
```

Roles: n1 produces the chain via the shipped external-miner protocol (start/stop
the worker = start/stop block production) and serves the C17 fixture (its own mempool
is loaded directly, so the 11 MB serve responses genuinely stall); n2/n3 are
ordinary full nodes and downstream peers; the test peer is a hand-driven libp2p
client (`bin/anon-mempool-pin`) that performs the real noise-XX + yamux handshake
with a fresh random identity, answers the node's Identify probe to stay
connected, sends the real 12-byte `Pull` on
`/noid/mainnet/860e70453390bf81/1/sync/mempool/4`, half-closes, and never reads —
exactly what a yamux peer that stops reading looks like once its 256 KiB stream
window fills.

## What composed, and what did not

The anonymous mempool puller pinned the serving budget, delaying state/segment delivery. Header synchronization uses a separate path and continued. The source-level Busy escalation for other object requests was not observed in the multi-aggressor run (`Busy +0`). The distributed push storm increased control-plane tails at the tested rates, but its live payloads failed at authorization decoding; the 62x full-proof verification cost belongs to the separate direct mempool harness.

The snapshot install check compares a candidate against the HeaderDAG's **selected cumulative work**, not the installed state height (`noid_node/src/main.rs:282-321`). Phase 7.3 sampled the latter. Its pinned joiner was at installed height zero for about 20 seconds, but the saved log records selected honest heights 24 through 150 during that interval. The observed header selection removes the low-work premise from this run. A restarted node also retains finalized chainwork and therefore presents a different case from a fresh node.

The `../malicious-node/` function harness found a difference between ordinary future-drift validation and timeless historical snapshot staging. Its recursive terminal was stubbed. Phase 8 instead used a fork with a far-future **tip**, which the release node rejected before installation. Genuine-terminal verification for an intermediate-drift fork, selection among competing providers, and post-installation serving remain separate measurements. Default DNS seeds influence first contact, while anonymous serving pressure affects an established connection; candidate selection runs for five seconds and installation rechecks current HeaderDAG work (`../snapshot-adversary/REPORT.md`).

The dated public explorer/pool estimates in `../network-status/REPORT.md` put the observed mainnet work rate near 0.9 TH/s in this chain's own work units. They are not a count of GPUs or a cost estimate for the alternate fork. Phase 8 used floor difficulty, so its local work cost cannot be transferred to mainnet. The earlier 24-header intermediate-timestamp fixture is in the legacy schedule; the separate far-future-parent difficulty-floor example is at a v2 height.

Other compositions stayed inside their demonstrated bounds. C15 destroyed prepared templates when exercised with a mining token, but the honest worker kept pace. A locally altered supply counter diverged after restart; it required direct DB access. A tampered standalone legacy-matrix build changed the real GUI backend's model fields; that is a controlled build/input and node-trust case, not a remote peer capability. Under simultaneous local availability pressure, honest nodes already carrying the chain advanced together from height 124 to 136 at the sampled times.

## Results by phase

All numbers below are from the tee'd `results.txt` and the nodes' own debug
logs. Real binaries (release profile, authenticated packs), real P2P codecs,
real RPC stack, loopback only.

### Phase 1 — C17: an anonymous peer holds the outbound byte budget

Against a running node, `bin/anon-mempool-pin` performs the real noise-XX + yamux handshake with a **fresh
random ed25519 identity, no credentials, no network-profile exchange** (it
answers the node's Identify probe by advertising the full protocol set, which
is all the node's capability check requires, then simply never reads its
streams). It opens 4 `/sync/mempool/4` streams, sends the 12-byte `Pull`, and
parks them.

- Serving node n1 held a 150-intent mempool (each real paged-spend intent is
  ~89 KB — the GKR authorization bundle dominates — so ~13 MB total).
- **n1 served the anonymous peer 14 mempool-sync responses of `tx_count=128
  total_bytes=11,488,448` (11.0 MB) each** during the 120 s pin (verbatim from
  n1's log). Each response is **44× the 256 KiB yamux stream window**, so every
  one of the 4 concurrent writes stalls waiting for window credit the
  non-reading peer never grants — each holding its full unconditional 16 MiB
  reservation. 4 × 16 MiB = the **entire 64 MiB process-global outbound
  domain, pinned and sustained** across 24 s rotations (under the 30 s inbound
  timeout).
- During the same run, n1 `getChainInfo` latency went from
  **p50 545 µs / max 830 µs (before)** to **p50 2370 µs / max 10183 µs
  (during)** — a 4.3× median and **12× observed maximum** inflation, with zero RPC errors.
- Cost to the requesting peer: a 12-byte request per stream, a free libp2p key, and
  no valid data of any kind. No credential, no mining token, no wallet.
- On release the pin cleared immediately.

The one piece established by source-reading rather than live here is the
delay→`Busy`-rejection escalation for *other peers'* object/segment requests
(the blocked data-plane tasks hold the 6 Live-class slots); the admission
types are `pub(crate)`, so the p2p-resources lane established that leg at the
codec level. Live, the pin itself (64 MiB held by 11 MB stalled writes to an
anonymous peer) and the control-plane collateral are demonstrated directly.

### Phase 3 — C15 + C19: credentialed endpoints and local workload

- **C15 (mining token):** `getBlockTemplate` → `submitBlock(template, invalid
  nonce)` **consumed 3 prepared templates in 60 s** — each invalid-PoW
  submission destroys the node-owned prepared attempt (`begin_proving` runs
  before `validate_pow`) before any block is accepted. On this fast devnet the
  honest external worker W4 still mined 4 blocks (= its 4-block baseline),
  because template regeneration keeps pace at ~20 s block times; the
  destruction-of-prepared-work primitive is confirmed live, and full worker
  starvation (consumption faster than regeneration) is [RV-T] in the
  external-miner lane.
- **C19 (operator token):** `walletQueryReceipts` cost **1401 / 2022 / 2202 µs
  per call, each a complete store rescan + Merkle re-verification with no
  cross-call cache**, on the small devnet receipt store. The cost is per-call
  and scales linearly with receipt count (lane-measured ~205–225 µs/receipt,
  4.1–5.6 s per call at 20k receipts). Credential-gated (operator/loopback),
  no integrity impact.

### Phase 4 — cross-dialect mesh (C9) and the cross-version cap-route check

C9 (shared P2P profile ID / namespace across default, V2 and V2.1 fork
schedules) is [RV] in the version-domains lane; the live mesh of two
different-dialect binaries requires a loopback-only Linux network namespace
(isolated-schedule nodes refuse to start otherwise — `noid_node/src/main.rs:
1735-1768`), so it runs in docker `--network none` / on an x86 Linux box via
`scripts/phase4_docker.sh` (the Mac cannot host the second dialect: docker-for-
Mac's VM carries kernel-builtin tunnel interfaces, so `/proc/net/dev` is never
lo-only; a fresh `unshare -n` on hbox is).

**The specific cross-version cap-route check (the one this lane owns):** is
there any artifact that crosses the wire between nodes carrying a cap/commitment
blob that a *different-dialect* reader consumes, such that the C21 cross-domain
claw (identical commitment bytes valid in both `TAG_CAPSNODE` and `IVCPCSN_`)
becomes reachable node-to-node? **Answer: CLOSED.** `MerkleCap` bytes DO cross
the wire — but only serialized inside `ZkAuthorizationProof` blobs (the wire
scanner walks `source_commitment.cap.hashes: Vec<[u8;32]>` as the first vector,
`noid_gkr/src/zk_authorization_wire.rs:292-297`), i.e. inside transaction
intents. And every consumer of those caps runs the semantic capsule verifier,
which **recomputes each leaf hash from the actual leaf symbols and pins the
tree/cap depths BEFORE any raw cap verification** (`noid_fri_binius/src/
zk_capsule_pcs.rs:999-1041` builds `source_hashes` via `capsule_leaf_hash_mixed`
then calls `verify_source_batched_merkle_proof_to_cap` with the depth constants;
same at `capsule.rs:828-839`). No node-to-node path feeds attacker cap bytes
directly to `verify_source_batched_merkle_proof_to_cap`, and no in-tree caller
compares digests across domains (the tower-digest bridge is `#[cfg(test)]`,
`noid_gkr/src/zk_auth_qrom.rs:1999-2004`). So identical-bytes commitments cannot
be confused across domains over the wire: the verifier rebuilds every leaf hash
it did not receive. The route reopens only for an **external** consumer of the
public raw `verify_source_batched_merkle_proof_to_cap` / `hash_pair` /
serialized-`MerkleCap` APIs (an SDK, bridge, or cap-keyed index) — the same
boundary condition as C21 itself. Survey detail in `notes/wire-cap-survey.md`.

### Phase 4.5 — U19: noncanonical chain_tip accepted at restart, then fail-closed-dead

A 41-byte `chain_tip` (one `0xA5` appended to the 40-byte row, via `bin/
corrupt-db`) was **accepted by `open_or_create` at restart** (`decode_chain_tip`
accepts any ≥40-byte value), and n5 then fails every tip-reading operation
with the exact **`snapshot store read: mdbx: 41`** signature — the node opens
cleanly, reports a healthy tip, and cannot advance, with no recovery path
(restart validation succeeds so the clear-all recovery never triggers). Same
local-write/bit-rot precondition as C20.

### Phase 2 — C20: unauthenticated circulating-supply counter (silent integrity divergence, LIVE)

`bin/corrupt-db` edited n2's `state_meta` row (the one hot counter not bound to
the hash-committed tip header) while n2 was stopped, then n2 restarted. Result,
verbatim:

```
before corruption: n1 (139, 'ab344249475f', 6253847500)  n2 (139, 'ab344249475f', 1439560000)
after corruption + restart: n1 (139, 'ab344249475f', 6253847500)  n2 (139, 'ab344249475f', 1563016789)
-> SAME tip (h139 ab344249475f), DIFFERENT supply (n2 reports 1563016789 vs true 6253847500):
   SILENT INTEGRITY DIVERGENCE while in consensus and RPC-alive
after 3 more blocks: ... -> divergence PERSISTS and propagated through valid commits
after a second restart: ... -> NOT healed; compact restart re-trusts the corrupted counter
```

n2 reports a **false circulating supply while sitting at the EXACT same tip
height and block hash as the honest n1** — it stays in consensus, answers RPC,
accepts every valid block, and the divergence survives restart (the compact
path re-trusts the stored bytes). Nothing user- or network-visible flags it.
(The divergence had in fact persisted silently through the *entire* preceding
composition run from an earlier corruption — ~15 blocks of a node reporting a
wrong supply while fully in consensus.)

The **downward** variant (a zeroed counter *permanently stalling spend
admission* via checked-underflow in `supply_after_slot_updates`) does not
reproduce on a passive syncing relay — it re-follows the honest tip — and needs
the storage-reorg lane's precisely constructed spend of large coinbase UTXOs
through the direct `apply_next_block` path; it is [RV-T] in
`experiments/storage-reorg`. Local-write/bit-rot precondition either way; no
remote vector, no invalid block accepted.

### Phase 5 — the composed scenario: all four attacks at once

Simultaneously on the healthy devnet: the anonymous mempool-pin aggressor on
n3, the C15 invalid-PoW template burn on n4 (**16 templates destroyed**), the
C19 receipt pressure on n1, and a fresh node (n3c) joining through the pinned
relay. Halfway through, the honest producer was resumed to ask the decisive
question: **can consensus still advance under the full composition?**

Timeline (height across all nodes; n1 RPC latency µs):

```
t=0    all h124   n3c joining     n1_rpc 640us
t=45   all h124   n3c SYNCED 124  (fresh joiner reached tip through the mesh)
t=135  all h125   (producer resumed; chain advancing)
t=225  all h127
t=358  all h136   n1_rpc 370us
release: n3c reached tip <=28s after; burn stats: 16 templates destroyed
```

The sampled nodes advanced together from height 124 to 136. No invalid block
or unauthorized spend was observed. The measured effects were slower serving,
RPC latency, destroyed templates, and a diverged local supply counter.

### Phase 6 — a substituted build and the GUI backend's model (C16 + C24)

The local test combined a substituted build with the shipped GUI backend's
model constructor:

1. **Tampered build.** `bin/parano1d-tampered` is a full `--release` node built
   from the pinned tree with one HistoryStep matrix substituted: `history-step-
   c00.field-r1cs.zst` was decompressed, **one coefficient flipped at production
   scale (byte 42,397,884 of the 84 MB canonical matrix)**, recompressed, and fed
   to the build. `build.rs`'s `stage_leaf` embeds it **without ever hashing the
   matrix** (the C16 gap), so it sails through under the pins.env-approved
   metadata identity. The node logs `preflight-authenticated HistoryStep runtime
   images loaded from the executable` — a foreign relation presented as approved.
   (The foreign `c00` digest `72adc877…` differs from the honest `fac7c2ea…`;
   `experiments/build-artifacts/VERIFY.md` independently confirmed the embedded
   decoder accepts the foreign bytes under the approved digest.)
2. **Run it as the user's node.** On a copy of the honest chain (h144, 787 wallet
   receipts), the tampered node comes up healthy and in sync.
3. **The honest chain advances; the tampered node diverges, silently.** As the
   honest producer mined h145–149, the tampered node **FROZE at h144** — it
   rejected the observed honest blocks whose HistoryStep proofs were made against
   the real relation, not the substituted one. `honest=h149
   tampered=h144`.
4. **The GUI backend's model labels the tampered node's data authenticated.**
   The probe invoked the real backend constructor
   (`noid_gui::backend::Backend::from_env()`), pointed at the tampered node,
   and printed balance, UTXO, and receipt model values. Its receipt fields were
   **`merkle_valid=true canonical=true confirmed=true` -> "AUTHENTICATED PAYMENT +
   CONFIRMED"**. The model verdict matched the honest node's. The probe did not
   run an interactive GUI session. The wallet's backend does not independently
   check headers, PoW, state roots, Merkle proofs, or signatures (C24).

The attacker precondition here is control of the build input or a node trusted
by the wallet. The test shows a relation mismatch and the resulting backend
model values; it does not establish a remote peer attack or user action.

### Phase 7 — multiple anonymous peers and a fresh joiner

The test used one, two, or four synthetic libp2p identities on one host and
compared the serving budget held against the same node.

**7.1 — C17 domain pinning, 1 vs 2 vs 4 aggressors (2 streams each, victim n1
holding a 150-intent ~13 MB mempool):**

```
 aggressors | domain held | anon serves | Busy | joiner synced | ctrl-plane p50/max us (during)
     1      |   32 MiB    |     11      |   0  |    True       |  1009 / 2266
     2      |   64 MiB    |     13      |   0  |    True       |   768 / 2362
     4      |   64 MiB    |     12      |   0  |    True       |   347 / 1030
```

The domain fills **progressively with attacker count — 32 MiB at 1 aggressor,
saturated at 64 MiB from 2 aggressors on** (each stream holds the unconditional
16 MiB; 2 streams × 2 aggressors = the whole domain). This is the multi-aggressor
delta a single 4-stream aggressor hides (it alone saturates). Interaction effect:
the attacks **reinforce** on the byte domain (more identities = more held
reservations, and more connections toward the 128-inbound cap), with no
interference — each aggressor is independent.

The fresh joiner synced in every configuration. This connected devnet had
other honest peers and 24-second aggressor rotation. The run therefore shows
serving pressure on one node, not an eclipse of the joiner. Peer-selection
constraints are analyzed separately in `../eclipse-topology/`.

**7.2 — anonymous push traffic:** 40 pushes over ten seconds were split across one, two, and four identities. The largest observed `getChainInfo` tail in each run was 5.0, 6.4, and 9.4 ms respectively; idle medians stayed near 0.3 ms. Those maxima suggest a connection-count effect but are not a controlled scaling law. The live payloads failed at proof decoding. The separate `../tx-admission/` harness measured the cost of a decodable but invalid proof; that full-cost variant was not run through this P2P traffic test.

**7.3 — fresh joiner:** with one pinned reference relay, installed height remained zero through the 20-second sample and reached 150 by 30 seconds; the free control reached 150 by ten seconds. The pinned joiner's log shows HeaderDAG-selected reference heights 24, 49, 74, 99, 124, 149, and 150 while its installed state was zero. The sampler observed state installation, not header work. The measured state-sync delay therefore coexisted with substantial selected work. See `header-progress-review.txt` for the log hash and exact lines.

The 1/2/4-peer budget hold saturated one node at two coordinated identities. Joiners in other phase-7 controls still reached tip through available peers. No phase-7 run changed reference consensus or finalized history.

### Phase 8 — future-dated tip rejected

The phase-8 script assembled a 31-block reference chain and a separate 61-block fork mined by a clock-shifted node. Height 10 on the alternate fork was roughly ten years in the future. An anonymous mempool puller held the reference relay's serving budget. The fresh joining node had both providers as seeds.

The joining node received the alternate manifest and staged its associated headers. Its native header inventory probe rejected the future-dated first header (`BadTimestamp`, `logs/p8-victim.log:125`). The HistoryStep snapshot path then rejected the alternate candidate because the **tip** exceeded local future drift (`logs/p8-victim.log:218`). The node retired that snapshot plan, selected and installed the reference chain, and reached height 31 with reference hash `d6fc8017364927c8` (`results.txt`, phase-8 tip progression and final comparison). It stayed there throughout the 232-second observation window. A second fresh joiner seeded only to the first also reached reference height 31 with the same hash. The script's final `SCOPE` line is a historical, unconditional success label; the recorded `RESULT: victim_on_poison=False reserved_to_next=False` is the run's measured verdict.

This run exercised the release node's snapshot selection path through its
tip-drift check; rejection happened before cryptographic terminal verification.
The narrower C26 intermediate-header configuration remains untested at this
release-node boundary. Its producer must create a valid HistoryStep terminal for a
fork whose intermediate header is too far ahead but whose tip is inside the
current drift window. The cost of such a fork, offer-selection outcome, and
any relay to a second node remain unknown.

### Results across the composed runs

- Reference nodes already carrying the same chain advanced from height 124 to 136 under the phase-5 mix. No invalid block or unauthorized spend was accepted in that run.
- Phase 7 held serving budget and delayed one joiner's state installation, while reference headers still entered its HeaderDAG. Its multiple-peer logs recorded `Busy +0`.
- Phase 8 rejected the future-dated alternate fork at both the native header probe and the HistoryStep tip gate. Both joiners installed the reference chain.
- The phase-6 tampered-build test requires control of the standalone legacy-matrix build input. C20/U19 require local DB writes; C15 requires a mining token. These are distinct from anonymous P2P pressure.

The lowest-cost recurring pressure measured here is a non-reading mempool pull against one serving node. It reduces response capacity. Header propagation and finalized history continued in the tested topology.

## PoC implementation and reproduction

Binaries (all built from the pinned tree; see "Assurance limits" for profile
choices):

- `bin/parano1d`, `bin/parano1d-cli` — the daemon and CLI, built
  `--release` with the full authenticated artifact chain embedded: the v1
  HistoryStep pack **generated locally** by the project's own
  `scripts/generate_history_step_pack.sh` (~19 min on this machine), plus the
  v2 pack (with retirement keys) and v21 pack downloaded from the project's
  public v2.1.0 GitHub release and verified against their shipped
  SHA256SUMS (the build.rs semantic preflight re-authenticates all of them
  independently).
- `bin/parano1d-miner` — the shipped external PoW worker
  (`cargo build --release -p noid-extminer`).
- `bin/anon-mempool-pin` — the anonymous non-reading mempool puller: a
  hand-driven libp2p client (noise-XX via the real `libp2p-noise` handshake,
  raw `yamux` 0.13 with the default 256 KiB stream windows, hand-rolled
  multistream-select — shared transport in `src/aggressor_common.rs`) that
  dials the victim, opens `/noid/mainnet/860e70453390bf81/1/sync/mempool/4`
  streams, writes the 12-byte `NMR3` Pull request, half-closes the write side
  (the codec requires request EOF), and never reads the stream again. It
  answers `/ipfs/ping/1.0.0` echoes AND the node's `/ipfs/id/1.0.0` Identify
  probe — advertising the full network-v7 protocol set, which is all the
  node's capability check needs to keep the endpoint open (the one thing
  that, missing, got the connection dropped at ~16 s in early testing). NO
  network-profile exchange, NO credentials (mempool serving is gated on
  neither). Rotation replaces the parked set every 24 s, under the 30 s
  inbound timeout.
- `bin/anon-push-storm` — anonymous `MempoolRequest::Push` storm (phase 7,
  C22 leg): same hand-driven transport, builds state-shaped PagedSpend
  intents against the victim's real epoch anchor + a real live slot (fetched
  over loopback RPC) with decode-failing authorization bundles, and streams
  them as pushes. Each push drives the node's `InboundVerify` CPU phase before
  rejection; the full decodable-but-invalid 62× verify cost needs a real proof
  from an attacker-owned UTXO ([RV-T] in the tx-admission lane).
- `bin/wallet-probe` — the real GUI backend (`noid_gui::backend::Backend::
  from_env()`, the constructor the shipped wallet uses) driven against
  whatever node answers `NOID_RPC`; prints model values consumed by the view
  layer, including the receipt-verification booleans used for
  "AUTHENTICATED PAYMENT / CONFIRMED". Built against the pinned `noid_gui`
  crate copy in `experiments/wallet-vs-node/`.
- `bin/corrupt-db` — offline single-row MDBX editor (state_meta supply up/zero,
  chain_tip append) using production-identical MDBX options; same technique as
  the storage-reorg lane's second-writer corruption.
- `bin/parano1d-tampered` — a full `--release` node with one HistoryStep matrix
  coefficient flipped at production scale (the C16 substitution), embedded under
  the approved metadata identity because `build.rs` never hashes the matrix.
- `bin/parano1d-poison` — the clock-offset test binary, a full `--release` node built from a copy of the pinned
  tree (read-only checkout untouched) with one surgical change: `unix_now()` and
  the built-in miner's template timestamp take an env-gated offset
  (`NOID_POISON_CLOCK_OFFSET_SECS`, default 0 = pinned behaviour). Run with
  `+10 years`, it mines a genesis fork whose headers carry far-future timestamps
  that its own shifted clock accepts. The receiving node uses real time and
  rejected this fork. The offset is a test fixture; it does not exercise the
  intermediate-header release-verification case. Transport clock
  (`SystemTime::now()`) remains real so the node can serve peers.
- `bin/parano1d-v21` — isolated-schedule build for phase 4 (Linux-only; see
  below).

Reproduction (what actually ran here):

```sh
cd /Users/ember/dev/parano1d-break/experiments/devnet-chaos
# phases 0-5 on one persistent devnet (control, C17 pin, C20 tamper,
# C15+C19 operator, U19 tip, composed). Fault-tolerant per phase.
python3 scripts/run_all.py --phases 0,1,2,3,4.5,5 --mempool-intents 150 >> results.txt 2>&1
# phases 6/7/8 are separate because each drives the same devnet data dir
# (they cannot run concurrently):
python3 scripts/phase6_supply_chain.py            >> results.txt 2>&1   # tampered build -> wallet
python3 scripts/phase7_multiaggressor.py          >> results.txt 2>&1   # multi-aggressor table
python3 scripts/phase8_live_c26.py                >> results.txt 2>&1   # future-tip rejection control
# phase 4 (cross-dialect mesh) needs a loopback-only Linux netns:
bash   scripts/run_linux_builds.sh                # or build on an x86 Linux box
bash   scripts/phase4_docker.sh                   # docker --network none
python3 scripts/cleanup.py                        # emergency stop-by-PID
```

`scripts/chaoslib.py` is the driver library (the `Node`/`Managed` process
model, PID files, RPC, log parsing); `scripts/phases.py` holds the phase-0..5
implementations; `scripts/run_all.py` is the per-phase-fault-tolerant runner.
Every process gets a PID file under `run/`; shutdown is `paranoid_stop` then
SIGTERM/SIGKILL to exactly that PID (`scripts/cleanup.py` refuses PIDs whose
command line is not ours — never a pattern kill). All listeners bind 127.0.0.1.
The devnet is fully synthetic: persistent data dirs under `run/`, node-generated
wallets, our own seeds, `--disable-dns-seeds`, our own locally-generated genesis
pack.

## Assurance limits

- The node binary is the project's own release pipeline output (release
  profile, all packs embedded and preflight-authenticated by build.rs), but
  the run itself is a **local devnet at mainnet genesis-floor difficulty**:
  block times, sync times and RPC tails measured here are devnet-scale, not
  mainnet-scale. Where scale matters (receipt counts, mempool depth) the
  relevant lane's measured numbers are cited alongside.
- Phase 1's sustained stall **fired live** here: a real paged-spend intent is
  ~89 KB (the GKR authorization bundle dominates), so a 150-intent mempool gave
  11 MB serve responses — 44× the yamux default 256 KiB stream window — and the
  writes genuinely stalled. The one leg established by source-reading rather than
  live is the delay->`Busy`-rejection escalation for *other* peers' object
  requests (the admission types are `pub(crate)`, unreachable from an external
  client); the p2p-resources lane covered that at the codec level.
- The aggressor is a hand-rolled transport client, not the shipped swarm: it
  exercises the server side exactly (real codecs, real outbound-budget singleton,
  real yamux semantics) and answers ping + Identify, but does not model
  kademlia/gossipsub beyond what the capability check requires.
- The push-storm's anonymous pushes reach the node's `InboundVerify` phase and
  are rejected at authorization *decode* (InvalidMagic); forcing the full ~8 ms
  GKR verify per push needs a decodable-but-invalid bundle derived from a real
  proof, which an attacker generates from a UTXO they own — the tx-admission lane
  proved that 62× asymmetry [RV-T]. Live, the distributed admission-engagement
  across identities is shown; the maximal per-push cost is cited.
- C20/U19 corruption is applied offline by a second writer while the victim node
  is stopped — bit-rot/tampered-backup preconditions, not a remote vector. The
  C20 **downward** spend-stall does not reproduce on a passive syncing relay
  (it re-follows the honest tip); the **upward** divergence is shown live and the
  downward stall is [RV-T] in storage-reorg.
- C15's template-regeneration cost is observed through getBlockTemplate cadence;
  on this fast devnet the honest worker kept pace, so template *destruction* is
  shown (16 in the composed window) rather than full worker starvation (which the
  external-miner lane proved [RV-T] with faster consumption).
- Phase 6's tampered node is a production-size C16 artifact (one flipped
  coefficient in the 84 MB canonical matrix, embedded under the approved digest);
  it froze off the honest chain and the GUI backend probe reported authenticated
  model fields.
  The HistoryStep terminal verifier is real in this release build (the node
  rejected honest blocks under the foreign relation); no claim is made about
  proof *soundness* beyond relation mismatch.
- Phase 4's live mesh runs in docker with `--network none` (and, because
  docker-for-Mac's VM carries kernel-builtin tunnel interfaces that make the
  node's lo-only-netns guard refuse to start, on an x86 Linux box / hbox where a
  fresh `unshare -n` is genuinely lo-only); the profile-ID collision itself is
  [RV] in the version-domains lane. The cross-version cap-route check is reported
  below.

