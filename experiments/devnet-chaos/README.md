# Devnet chaos composition — the strongest local-network degradation from the audit's findings

Composition lane of the parano1d adversarial audit. Target: pinned read-only
checkout `52938433a30b185ab007afaa15166a632cf2e82e`. Everything below runs on
loopback against our own devnet (own data dirs, synthetic identities, our
seeds); no live network is contacted. The question this lane answers: **what
is the most chaos composable from the confirmed findings against a local
devnet, what does each ingredient cost, and what survives.**

**The composition thesis (read this first).** Each finding was scoped in
isolation and most came out "Medium availability, no acceptance impact" or
"local integrity." The *compositional* question — the one that matters — is not
"do they run at the same time" (they do, and consensus survives that, see phase
5) but **"does one finding remove the precondition-barrier of another?"** The
answer is yes, and it reframes the severity: **the audit's one finalized-
integrity break (C26 snapshot poisoning) is gated only by "the victim must be a
fresh or eclipsed node that has not yet received the honest chain's work" — and
C17, the zero-credential anonymous mempool pin, is precisely the tool that
manufactures that precondition.** The availability findings are not a sideshow
to the integrity findings; they are the *delivery mechanism* for the one that
breaks finalized history. See **"Composition analysis"** below — that is the
headline; the per-phase live reproductions are the evidence under it.

## Topology

```
                         loopback 127.0.0.1 / ports 196xx
  [anon-mempool-pin] ──▶ ┌──────────────────────────────────────────┐
  anonymous libp2p peer  │  n1-pool  extminer,--genesis, M1+O1 keys │ ◀── [W1 worker]
  (no creds/profile):    │  p2p 19610  rpc 19611                   │     PoW grind
  4 parked /sync/mempool │  C17 VICTIM (loaded mempool => 11 MB    │
  /4 pulls, never reads  │  stalled serves) + chain producer       │
                         │   ▲ p2p seeds                            │
                         │   │                                      │
                         │ n2-relay  ordinary  p2p 19620 rpc 19621  │  C20-corrupted
                         │   ▲ p2p seeds       (supply diverged)    │  (phase 2)
                         │   │                                      │
                         │ n3-syncer / n3b / n3c   fresh joiners    │
                         │                                          │
  [W4 worker]──RPC──────▶│ n4-pool2  extminer, M4 key 19640/19641   │  C15 template
                         │                                          │  burn victim
                         │ n5-tip  throwaway (U19) 19720/19721      │
                         └──────────────────────────────────────────┘

  phase 6 (developer supply chain):
  [bin/parano1d-tampered on a copy of n1's chain] ◀── [bin/wallet-probe]
     foreign HistoryStep matrix (C16) ; real GUI backend renders it AUTHENTICATED (C24)

  phase 7 (multi-aggressor): 1/2/4 x [anon-mempool-pin] + N x [anon-push-storm] vs n1
  phase 4 (docker --network none / hbox): [default-dialect] <-p2p-> [isolated-v2.1-dialect] (C9)
```

Roles: n1 produces the chain via the shipped external-miner protocol (start/stop
the worker = start/stop block production) and is the C17 victim (its own mempool
is loaded directly, so the 11 MB serve responses genuinely stall); n2/n3 are
ordinary full nodes and downstream peers; the aggressor is a hand-driven libp2p
client (`bin/anon-mempool-pin`) that performs the real noise-XX + yamux handshake
with a fresh random identity, answers the node's Identify probe to stay
connected, sends the real 12-byte `Pull` on
`/noid/mainnet/860e70453390bf81/1/sync/mempool/4`, half-closes, and never reads —
exactly what a yamux peer that stops reading looks like once its 256 KiB stream
window fills.

## Composition analysis — enablement, not parallelism

The myopic reading of this lane (which the first draft fell into) is "run the
findings at once, observe that consensus survives, call availability the
boundary." The real analysis is about **which finding removes which other
finding's precondition**, and **how the cost scales** — because a composition is
only interesting where the composed cost is low relative to the composed damage.

### The enabling chain: C17 (anonymous, free) manufactures C26's precondition

C26 (snapshot poisoning, [RV-T] in `experiments/malicious-node`) makes a
snapshot-syncing peer **commit, as finalized canonical history, a header
sequence every honest validator rejects by rule** — then re-serves it to the
next syncer (self-propagating), and the victim is permanently estranged
(`FinalizedDivergence`). Its *only* gate (`noid_node/src/main.rs:287-301`,
`validate_snapshot_install_selection`) is that the poison manifest must **improve
on the HeaderDAG-selected cumulative chainwork** — i.e. the victim must be
**fresh or eclipsed**: it must not already hold the honest chain's work.

C17 is the tool that produces "fresh or eclipsed", for free, from an anonymous
peer. Measured live (phase 7.3):

```
fresh joiner, honest relay FREE   : height 0 -> 150 by t=10s
fresh joiner, honest relay PINNED : height 0 held through t=20s, breaks to 150 only at t=30s
```

The pin adds a **~20-30 s window in which the victim has NO honest work at all**
(height 0). During that window, "improve on HeaderDAG-selected chainwork" is
trivially satisfied — the attacker's poison fork out-works *nothing*. So the
composition is: anonymous C17 pin on the honest serving layer -> fresh/rejoining
victim stalls with zero honest work -> a malicious serving node (C26) wins the
selection and commits finalized poison -> permanent estrangement, self-
propagating. **Zero credentials start the chain; the payload is a finalized-
integrity break, not availability.**

### Why "PoW-bounded" is not the boundary here

The honest-boundary claim I kept repeating — "established nodes are safe because
the poison fork must out-work the honest chain, PoW-bound" — is only a barrier if
that PoW is *expensive relative to the damage*. It is not, on this target:

- **Genesis-floor difficulty is `GENESIS_TARGET = 2^238`** → `2^256 / 2^238 =
  2^18 ≈ 262,144` hashes per block. Measured on this lane's single laptop:
  **761-1131 ms (mean 937 ms) of Poseidon2b grinding per block** at the floor. An
  18-block finality window is therefore ~17 CPU-seconds on *one* commodity
  machine; out-pacing a floor-pinned honest network for a finality window costs a
  handful of laptops.
- **ASERT halflife is 120 s** (`EPOCH_LENGTH 6 × BLOCK_TIME 20`), the shortest
  practical — difficulty collapses toward the floor within minutes of any honest
  hashpower dip, and the code comment says this is deliberate for a "young
  network."
- **Finality depth is 18 blocks** (6 minutes); the poison install finalizes at
  commit.
- **The target has no publicly visible live network, explorer, hashrate, or node
  population** (searched; only an early release line and a one-contributor mobile
  fork). The consensus parameters are explicitly tuned for the young/low-hashpower
  regime.

In that regime, out-working the honest tip to poison even an *established* node
is cheap — the "fresh-victims-only" boundary dissolves into "whoever can out-hash
the current (low, floor-pinned) honest rate for a finality window," which on a
nascent network is affordable. **PoW-boundedness bounds nothing when the PoW
price is small next to permanently estranging a node as finalized history.** The
honest statement is therefore not "consensus is safe"; it is "consensus safety
here rests entirely on honest hashpower that is currently unmeasured and, by the
parameters, assumed small."

### How it scales with malicious-node count

- **Anonymous serving-pin cost is ~zero and scales linearly.** One free libp2p
  identity with 4 mempool streams reserves the whole 64 MiB outbound domain of
  one node (phase 1 / 7.1: 2 aggressors × 2 streams already saturate it). To pin
  the honest *serving layer* you need ~one such identity per honest serving node —
  O(H) free identities. The only friction is the inbound diversity caps (32/IP,
  96+8 per /16, 128 total); a handful of IPs or one /16 clears them. There is no
  app-level peer score or ban to attrit the identities (gossipsub defaults +
  IP-colocation only).
- **Eclipse reach grows with malicious *serving* nodes.** Every honest serving
  node pinned is one fewer honest source a bootstrapping victim can reach; every
  malicious serving node added is one more poison source it *can*. As the ratio
  of (pinned honest : available malicious) sources rises, the probability that a
  fresh/rejoining node commits poison during its eclipse window rises toward 1.
- **The two reinforce.** C17 subtracts honest serving capacity (free, anonymous);
  C26 adds malicious serving capacity (needs cheap PoW). Together they move the
  bootstrap-source lottery from "mostly honest" to "mostly malicious" at a cost
  dominated by hashpower that the parameters assume is scarce.

### The convergent pattern: every availability finding erodes C26's one barrier

The reason this is systemic and not a one-off chain: C26's sole gate is "out-work
the honest chain," i.e. it is bounded by **honest hashpower and honest block
propagation** — and *multiple independent findings each attack exactly that
resource*:

- **C17 (anonymous serving pin)** removes honest serving capacity, producing
  fresh/eclipsed victims (the precondition) — demonstrated live (7.3).
- **U21 / C22 (gossip-window monopoly / admission occupancy)** throttle honest
  block *and transaction* propagation; blocks that do not propagate are honest
  work that does not count against the attacker's fork — lowering the out-work bar.
- **C15 (invalid-PoW template burn)** halts honest *external* miners outright;
  less honest hashpower is a lower bar still.

So the availability findings are not a separate, lesser category from the one
integrity break — they are a toolkit for driving down the exact quantity
(effective honest hashpower / propagation) that is the *only* thing standing
between C26 and network-wide poisoning. On a network whose parameters already
assume that quantity is small and whose floor difficulty is a ~1 s-per-block CPU
job, that toolkit is cheap relative to the damage. **This is the composition the
audit's per-finding severities (each "Medium availability") structurally hide.**

### What genuinely does NOT compose (kept honest)

- The **primitive-collision findings (C1-C3, C21) hit the leaf wall** and do not
  feed the eclipse chain — different layer; the semantic verifiers recompute leaf
  hashes. The **cross-version cap route is CLOSED** over the wire (phase 4).
- **Nodes that already hold the honest chain's work** still reject the poison
  fork *at a given honest hashpower* — the composition's damage is bounded by how
  much hashpower the attacker must out-run, which is the one real lever the
  network has and the one number nobody has published.
- **C15/C17 (two availability findings) reinforce but do not enable** — running
  them together is parallelism, not composition.

### Scale caveat (do not over-read the live numbers)

Everything live here ran against a **3-4 node loopback devnet with 1-4
aggressors** — that is a *mechanism* demonstration, not a network-scale one. The
per-attacker-count table (phase 7.1) shows the domain-pin saturating at 2
coordinated identities *against one victim*; the network-degradation-vs-M curve
(how fast the honest serving layer falls as malicious nodes scale against the
real N) is reasoned from the caps and costs above, not measured at scale. The
real severity denominator — N honest nodes and their aggregate hashpower — is
exactly the quantity the project does not publish, and it is what every number
below should be read against.

## Findings composed (per phase)

All numbers below are from the tee'd `results.txt` and the nodes' own debug
logs. Real binaries (release profile, authenticated packs), real P2P codecs,
real RPC stack, loopback only.

### Phase 1 — C17: one anonymous peer pins a node's entire outbound byte domain (LIVE, sustained)

The headline, demonstrated end-to-end against a real running node. `bin/
anon-mempool-pin` performs the real noise-XX + yamux handshake with a **fresh
random ed25519 identity, no credentials, no network-profile exchange** (it
answers the node's Identify probe by advertising the full protocol set, which
is all the node's capability check requires, then simply never reads its
streams). It opens 4 `/sync/mempool/4` streams, sends the 12-byte `Pull`, and
parks them.

- Victim n1 held a 150-intent mempool (each real paged-spend intent is
  ~89 KB — the GKR authorization bundle dominates — so ~13 MB total).
- **n1 served the anonymous peer 14 mempool-sync responses of `tx_count=128
  total_bytes=11,488,448` (11.0 MB) each** during the 120 s pin (verbatim from
  n1's log). Each response is **44× the 256 KiB yamux stream window**, so every
  one of the 4 concurrent writes stalls waiting for window credit the
  non-reading peer never grants — each holding its full unconditional 16 MiB
  reservation. 4 × 16 MiB = the **entire 64 MiB process-global outbound
  domain, pinned and sustained** across 24 s rotations (under the 30 s inbound
  timeout).
- Collateral on the honest control plane: n1 `getChainInfo` latency went from
  **p50 545 µs / max 830 µs (before)** to **p50 2370 µs / max 10183 µs
  (during)** — a 4.3× median and **12× tail** inflation, with zero RPC errors.
- Cost to the attacker: a 12-byte request per stream, a free libp2p key, and
  no valid data of any kind. No credential, no mining token, no wallet.
- On release the pin cleared immediately.

The one piece established by source-reading rather than live here is the
delay→`Busy`-rejection escalation for *other peers'* object/segment requests
(the blocked data-plane tasks hold the 6 Live-class slots); the admission
types are `pub(crate)`, so the p2p-resources lane established that leg at the
codec level. Live, the pin itself (64 MiB held by 11 MB stalled writes to an
anonymous peer) and the control-plane collateral are demonstrated directly.

### Phase 3 — C15 + C19: credentialed-operator chaos

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

**The chain advanced 124 -> 136 (12 blocks) with every node in lockstep the
entire time.** Consensus, block validity and finality never wavered under the
composed attack. What degraded was availability and bookkeeping: serving
latency, RPC tails, a diverged supply counter on the tampered node. What died:
nothing that consensus depends on. The composed headline is therefore a sharp
boundary — the cheapest sustained full-domain degradation an anonymous peer can
inflict (phase 1) runs *concurrently* with credentialed burn and local tamper
without ever threatening the ledger's integrity.

### Phase 6 — the developer supply-chain composition (C16 + C24), LIVE end-to-end

Every leg of the developer/build-chain attack, live, in one demo:

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
   rejects every honest block because their HistoryStep proofs were made against
   the real relation, not the developer's substituted one. `honest=h149
   tampered=h144`.
4. **The wallet renders the tampered node's reality as authenticated.** The real
   GUI backend (`noid_gui::backend::Backend::from_env()`, exactly as the shipped
   wallet constructs it — `bin/wallet-probe`) pointed at the tampered node renders
   its balance, UTXO table, and receipts, and its receipt verification as
   **`merkle_valid=true canonical=true confirmed=true` -> "AUTHENTICATED PAYMENT +
   CONFIRMED"**. The verdict is **identical to the honest node's** — nothing
   client-side (no header/PoW/state-root/Merkle/signature check runs in the wallet,
   C24) distinguishes the frozen, developer-chosen reality from the real one.

The chain: tampered build (passes identity) -> node "verifies" against a
developer-chosen relation and freezes off the honest chain -> the user's own
wallet displays that frozen reality as AUTHENTICATED/CONFIRMED -> the user acts
on it. **Honest severity framing:** the attacker here is the developer / build
chain, not a stranger (C16's precondition is a pack whose integrity is not
verified out-of-band; C24's is a malicious or corrupted node). The point is not
a new privilege escalation — it is that *every leg is live in one demonstration
and nothing user-visible can catch it.*

### Phase 7 — multi-aggressor eclipse (chaos-per-attacker-count)

Coordinated anonymous aggressors (distinct synthetic libp2p identities on one
box — realistic, since gossip IP-colocation penalties only bite above 8
identities/IP and only on gossip, not these request-response streams) vs the
honest devnet, measuring the DELTA over the single-aggressor case.

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

The honest-eclipse answer ember asked for: **the fresh joiner still synced in
every configuration.** On this connected devnet with 24 s rotation gaps and a
gossip mesh, a node seeded to the pinned upstream discovers other honest peers
and catches up in the windows between rotations — a *single* pinned upstream
degrades serving latency and holds the domain, but does not by itself eclipse a
joiner that has mesh alternatives. A real eclipse needs to also own or exhaust
the victim's other peer slots (the per-IP / per-/16 diversity caps and the
128-inbound limit are the relevant knobs; the eclipse-topology lane models the
selection logic from the code). So: domain-pin scales cleanly with attacker
count; joiner eclipse does not, absent control of the joiner's other sources.

**7.2 — C22 distributed admission pressure, same total rate (40 pushes/10 s)
spread across 1/2/4 identities:** each anonymous push drives the node's real
`InboundVerify` CPU phase before rejecting. The clean interaction effect: at the
**same aggregate rate, spreading across more identities inflates the control-
plane max tail — `getChainInfo` max 5011 µs (1 identity) -> 6441 µs (2) ->
9407 µs (4)** — because more identities mean more concurrent connections and
inbound-verify work with no per-stream pacing (idle p50 ~290 µs throughout). So
distributing the *same* offered load across coordinated identities is strictly
worse for the victim than concentrating it. The full decodable-but-invalid 62×
per-push verify cost needs a proof from an attacker-owned UTXO (the tx-admission
lane's [RV-T] result); live, the distributed admission-*engagement* and its
tail-amplification-with-identity-count are what is shown.

**7.3 — fresh-joiner selection:** a joiner's first sync against a busy/pinned
honest source is materially slower than against a free one; its logs show it
dials the seed and then widens to mesh-discovered peers — which is exactly why
7.1's joiner escaped the pin. This is the selection precondition the snapshot-
poisoning finding (C26, [RV-T] in the malicious-node lane) needs: a *fresh or
eclipsed* victim. Full snapshot poisoning (C26) and the gossip-window monopoly
(U21) are cited, not re-run live here — they need a malicious serving fixture
and a gossipsub publisher respectively, both established in their own lanes.

**Chaos-per-attacker-count summary:** domain-pin is the attack that scales with
coordination (and saturates at just 2 coordinated identities); admission
pressure scales with aggregate rate regardless of spread; neither buys integrity;
and eclipse of a fresh joiner needs more than pinning one upstream.

## The collapse timeline, by vantage point

From the composed run (phase 5), what each observer sees as the attacks land
and the chain is driven forward:

- **The anonymous outside peer (no credentials):** opens 4 mempool streams to a
  node, is served 11 MB responses it never reads, and holds that node's entire
  64 MiB outbound serving domain for as long as it keeps rotating (<=24 s). Costs
  it 12-byte requests and a free key. Sees nothing of the chain — doesn't need to.
- **An honest peer / fresh joiner:** headers still arrive (header sync bypasses
  the byte domain by design, C18), but body/segment/object serving from a pinned
  node blocks behind the held domain; a fresh node seeded only to a pinned node
  stalls on bodies until the pin rotates or lifts. In the composed run the joiner
  still reached tip (the mesh gave it other sources); against a *single* pinned
  upstream it waits. Recovery after release: <=28 s.
- **The operator (mining token):** every invalid-PoW `submitBlock` destroys a
  prepared template (16 destroyed in the composed window); on a fast devnet the
  honest worker keeps pace, on a slower one the prepared-work loss compounds.
- **The operator (operator token):** every `walletQueryReceipts` pays a full
  store rescan; at scale that is minutes of one core per pass.
- **The node operator with file access / bit rot:** a one-row edit makes the node
  report a false supply forever while staying in consensus (C20), or a one-byte
  append freezes it fail-closed (U19) — both invisible until you compare against
  a second node.
- **The developer / build chain (phase 6):** ships a node whose embedded
  HistoryStep relation is attacker-chosen but passes every build-identity check,
  and the user's own wallet renders that node's answers as AUTHENTICATED /
  CONFIRMED.
- **Consensus itself:** advanced 124 -> 136 throughout, every honest node in
  lockstep. Never forked on validity, never accepted an invalid block, never
  admitted an unauthorized spend.

## Negative and control results — what SURVIVED, and under what condition

What survived is real but **narrower and more conditional than the first draft
claimed.** Stated honestly:

- **No invalid block was ever accepted, and no consensus split occurred, among
  nodes that already held the honest chain** (124 -> 136 in lockstep under the
  full parallel composition, phase 5). This is a genuine guardrail: block
  validity is checked independently by every node.
- **No unauthorized spend, no minting, no theft, no owner-key recovery.** Every
  value amount stays root-authenticated; the collision findings hit the leaf wall
  and the cross-version cap route is closed (phase 4). This boundary held.
- **BUT "no anonymous integrity impact" is WITHDRAWN as an unconditional claim.**
  Via the composition above (C17 eclipse -> C26 poison), a zero-credential
  anonymous peer *can* drive a fresh or rejoining node to commit finalized,
  honest-rejecting history. What stops this from being network-wide is **honest
  hashpower, not a code guardrail** — an *established* node rejects the poison
  fork only while the attacker cannot out-work the honest chain, and the
  parameters (2^18 floor, 120 s ASERT) put that bar low on a young network. So
  the survival statement is conditional: *consensus survives to the extent that
  honest hashpower exceeds what an attacker can muster for a finality window* —
  and that quantity is unmeasured here.
- **Local-integrity findings are operator/attacker-local** (C20 supply counter,
  U19 tip) — real divergence, but requiring file access / bit rot, not remote.
- **The tampered-build attack (phase 6) is a developer/build-chain attack**, not a
  stranger's; its honesty is as a supply-chain finding — every leg live in one
  demo, nothing user-visible catches it — and it *composes* with C17 (pinning the
  honest peers a victim might cross-check against removes the only informal
  defense against a lying node).

**Cheapest sustained degradation an anonymous peer can inflict — and it is the
first stage of the eclipse chain, not a dead-end:** one TCP connection, a free
libp2p identity, 4 × 12-byte mempool `Pull`s re-issued every <=24 s, never
reading — holds a target node's entire 64 MiB outbound serving domain
indefinitely for ~zero cost (phase 1, live: 14 × 11 MB stalled serves,
control-plane tail 12×). Scaled to ~one identity per honest serving node (O(H)
free identities, defeating the 32/IP diversity cap with a handful of IPs), it
subtracts the honest serving layer — which is exactly what turns C26 from
"needs a fresh/eclipsed victim" into "reachable against bootstrapping nodes."

**What minimum credentials buy *on top of* the free anonymous chain:** a mining
token adds prepared-template destruction (external mining halts if consumption
outpaces regeneration — and note that *reducing honest block production* also
lowers the hashpower bar for the C26 step); an operator token adds per-call
full-store receipt rescans and the wallet-mutex tail inflation (U18, [RV-T]).
The integrity escalation, though, needs no credential at all — only cheap PoW.

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
  from_env()`, the exact constructor the shipped wallet uses) driven against
  whatever node answers `NOID_RPC`; prints the model values the view layer
  renders, including the receipt-verification booleans the GUI turns into
  "AUTHENTICATED PAYMENT / CONFIRMED". Built against the pinned `noid_gui`
  crate copy in `experiments/wallet-vs-node/`.
- `bin/corrupt-db` — offline single-row MDBX editor (state_meta supply up/zero,
  chain_tip append) using production-identical MDBX options; same technique as
  the storage-reorg lane's second-writer corruption.
- `bin/parano1d-tampered` — a full `--release` node with one HistoryStep matrix
  coefficient flipped at production scale (the C16 substitution), embedded under
  the approved metadata identity because `build.rs` never hashes the matrix.
- `bin/parano1d-v21` — isolated-schedule build for phase 4 (Linux-only; see
  below).

Reproduction (what actually ran here):

```sh
cd /Users/ember/dev/parano1d-break/experiments/devnet-chaos
# phases 0-5 on one persistent devnet (control, C17 pin, C20 tamper,
# C15+C19 operator, U19 tip, composed). Fault-tolerant per phase.
python3 scripts/run_all.py --phases 0,1,2,3,4.5,5 --mempool-intents 150 >> results.txt 2>&1
# phase 6 (developer supply-chain) and phase 7 (multi-aggressor) are separate
# because each drives the same devnet data dir (they cannot run concurrently):
python3 scripts/phase6_supply_chain.py            >> results.txt 2>&1
python3 scripts/phase7_multiaggressor.py          >> results.txt 2>&1
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
- Phase 6's tampered node is a genuine production-scale C16 artifact (one flipped
  coefficient in the 84 MB canonical matrix, embedded under the approved digest);
  it froze off the honest chain and the real wallet rendered it authenticated.
  The HistoryStep terminal verifier is real in this release build (the node
  rejected honest blocks under the foreign relation); no claim is made about
  proof *soundness* beyond relation mismatch.
- Phase 4's live mesh runs in docker with `--network none` (and, because
  docker-for-Mac's VM carries kernel-builtin tunnel interfaces that make the
  node's lo-only-netns guard refuse to start, on an x86 Linux box / hbox where a
  fresh `unshare -n` is genuinely lo-only); the profile-ID collision itself is
  [RV] in the version-domains lane. The cross-version cap-route check is reported
  below.

