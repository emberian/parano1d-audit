# Eclipse topology — node selection mechanics and the eclipse composition math

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e` (read-only).

All created files are confined to this directory. The PoC drives the pinned peer-admission gate verbatim
(`src/peer_diversity_vendored.rs` is `parano1d/noid_p2p/src/peer_diversity.rs` copied byte-identical,
`shasum 6c696510ee34ebec837b1c1fb61c3faa1138ca48`) and models the documented topology controller
(`noid_p2p/src/network.rs`) using its exact pinned constants, cited inline. Loopback/synthetic only: no
socket is opened, no external peer is contacted, no live node is started. This lane owns the **mechanics of
selection and the eclipse composition math**; a parallel lane (operator-power) owns **who** controls the
bootstrap infrastructure, and devnet-chaos runs the live multi-aggressor demonstration this model
instantiates.

## Executive one-liner

parano1d has **no application-level peer scoring, no ban system, no node-side checkpoint, and no
multi-endpoint quorum anywhere in the ecosystem.** Every client either spawns a local node (so eclipse
is a P2P problem, not an RPC-selection one) or points at a single endpoint; and every P2P node's entire
internet root of trust is **four DNS seeds under one apex domain (`parano1d.org`), dialed with no PeerID
verification and no fallback checkpoint**. Selection of ordinary neighbours is **first-come-sticky** — a
healthy eight-peer set is never rotated below target — so an eclipse that wins the initial peer set is
permanent until the attacker's own peers drop.

---

## 1. Selection inventory — how every client surface picks the node it trusts

Classification: **LOCAL-SPAWN** (client runs its own node, loopback RPC, no remote selection) /
**HARDCODED-DEFAULT** (one developer endpoint baked in) / **USER-CONFIGURED-NO-VALIDATION** (user types one
endpoint, no pinning/quorum) / **DISCOVERED** (node picks peers via P2P discovery).

| Surface | How the node address is chosen | Randomized / rotated? | Pinning / validation | What one malicious endpoint gets | Class |
|---|---|---|---|---|---|
| **GUI wallet** (`noid_gui`) | Spawns a local node binary (`backend.rs:1429-1466`); RPC fixed at `http://127.0.0.1:9601` (`backend.rs:49-50`). `NOID_RPC` overrides the URL, `NOID_GUI_NODE_BIN` substitutes the binary (`backend.rs:1592,1604`). "Node identity" is a ping to `blockCount` on loopback — no auth, no pinning (C24/W3) | No — one local endpoint | None; any loopback process is in the node TCB | Everything the node says is rendered as truth (C24 W1–W6: phantom receipts, fictional sends, suppression). **Eclipse moves to this local node's own P2P layer.** | LOCAL-SPAWN |
| GUI's spawned node (P2P) | Dials config/CLI seeds **plus the 4 embedded DNS seeds** (`noid_node/src/main.rs:2256-2298`); default config seeds are empty (`config.rs:67`), so a public user dials exactly `mseed{1..4}.parano1d.org` | Peers discovered via Kademlia (below) | Seed hostnames have **no PeerID** (format 2, `network.rs` consensus `:111-134`); no checkpoint | A malicious seed/DHT eclipses a fresh node (§3) | DISCOVERED |
| **noid-wallet** (Android, 3rd-party) | Phone → a wallet-backend server via `createApi(baseUrl)`; default `https://api.noidusa.com` (`app/app.json:134`), user-editable with an allowlist (`isAllowedServerUrl`: https always, http only to RFC1918). Backend → node RPC at `http://127.0.0.1:9601` (`backend/src/config.ts:3`) | No — one server, no rotation | Phone does **SPV header-chain check from a built-in checkpoint** (`backend/src/headers.ts`); slot proofs deferred to phase 2; balances/assets are the server's word vs a server-supplied state root | Headers can't be forged (SPV), but balances, asset lists, fee floor, tx submission, and "confirmed" status are one server's word | HARDCODED-DEFAULT (+ SPV on headers only) |
| **paramo1d** (messenger+node, 3rd-party) | "Chain data endpoint" supplies slot discovery/proofs/submission; Paranoid mode uses **direct mainnet P2P** (same 4 seeds). Chat relay hardcoded `https://chat.parano1d.org` (`MainActivity.java:290,950`) | P2P in Paranoid mode | Slot contents checked vs an independently-verified State root; chat relay not pinned beyond TLS | Non-paranoid: the endpoint is the chain view. Paranoid: eclipse at P2P | USER-CONFIGURED / DISCOVERED |
| **parano1d-sdk** (TS + Rust) | `new JsonRpcTransport("http://127.0.0.1:9601")` (README example); caller supplies the URL | No | None (plain JSON-RPC) | Full chain view = the endpoint (cross-ref C24) | USER-CONFIGURED-NO-VALIDATION |
| **noidium / parano1d-mcp** bridge | `http://127.0.0.1:9601` default; **loopback IP literals only** — hostnames, credentials, remote hosts refused (`README`/schema) | No | Loopback-pinned | "Any local process reaching 127.0.0.1:9601 already has owner scope" (its own README) | USER-CONFIGURED (loopback-pinned) |
| **permanode** (archiver) | `rpc_url` default `http://127.0.0.1:9601`, single endpoint | No | None | Archive view = the endpoint | USER-CONFIGURED-NO-VALIDATION |
| **peakpool** (pool) | Node RPC on `127.0.0.1:9601` via shared netns, single local endpoint | No | Loopback | Pool's chain view = the local node | LOCAL-SPAWN |
| **agcore** (agent verifier) | Default listen `/ip4/127.0.0.1/tcp/0`; **outbound peers from the native mainnet DNS seed set**; `--peer` for explicit bootstrap | P2P discovery | Wrapper-level receipt cache | Same seed-set dependency as any node | DISCOVERED |

**No surface in the ecosystem uses more than one node, and none rotates or quorums endpoints.** The only
client-side cryptographic independence anywhere is noid-wallet's SPV header chain and its own on-phone proof
generation. Every balance/asset/fee/confirmation on every surface is one endpoint's word (the wallet-vs-node
lane, C24, is the full treatment).

---

## 2. Peer-management constants (the core: `noid_p2p`)

Discovery stack (`behaviour.rs:1-26, 98-116`): **Kademlia DHT** (address-bearing, primary), **mDNS** (LAN
only), **Identify** (feeds Kad), plus bootstrap DNS seeds. GossipSub PX is deliberately disabled
(`accept_px_threshold=1.0`, fail-closed). No rendezvous.

| Constant | Value | Source | Role in eclipse |
|---|---|---|---|
| `AUTOMATIC_OUTBOUND_TARGET` | **8** | `network.rs:499` | Maintained neighbour count |
| `MAX_UNSELECTED_TOPOLOGY_CREDIT` | **4** (= target/2) | `network.rs:505` | Inbound peers satisfy **at most half** the target — ≥4 must be the node's own outbound dials |
| `INITIAL_BOOTSTRAP_FANOUT` | **2** | `network.rs:522` | Seed transports kept during initial sync |
| `MAX_PENDING_BOOTSTRAP_DIALS` | 4 | `network.rs:509` | One per DNS seed |
| `MAX_AUTOMATIC_PEER_CANDIDATES` | 512 | `network.rs:523` | Candidate-pool cap (evict oldest unconnected) |
| `AUTOMATIC_PEER_HEALTHY_AFTER` | 30 s | `network.rs:525` | A peer is "stable" after 30 s |
| maintenance tick | 2 s | `network.rs:3950` | `maintain_automatic_outbound` cadence — **adds toward target only** |
| `DISCOVERY_RETRY_MIN/MAX` | 30 s / 5 min | `network.rs:530-531` | Kad walk backoff |
| `MAX_PUBLIC_OUTBOUND_PEERS_PER_GROUP` | **2** per IPv4 /16 (IPv6 /32) | `peer_diversity.rs:22` | **4 /16 groups needed to own all 8 outbound** |
| `MAX_PUBLIC_INBOUND_PEERS_PER_IP` | 32 | `peer_diversity.rs:26` | Identities per exact IP (hard reject above) |
| `MAX_PUBLIC_INBOUND_CONNECTIONS_PER_GROUP` | 96 | `peer_diversity.rs:27` | Connections per /16 |
| inbound reserve | 96 unreserved + 8/group reserved | `peer_diversity.rs:33-34` | Last quarter of 128 held for underrepresented /16s |
| established inbound / outbound | 128 / 64 | `behaviour.rs:539-540` | Hard swarm caps |
| established per peer | 2 | `behaviour.rs:546` | Direct + relay during upgrade |
| Kademlia `replication_factor` | 20 | `behaviour.rs:443` | FIND_NODE breadth |
| Kademlia `parallelism` | 1 | `behaviour.rs:449` | One probe at a time |
| Kademlia inserts | `OnConnected` | `behaviour.rs:453` | Only connected peers enter the routing table |
| gossip IP-colocation threshold | **8.0** / IP (loopback whitelisted) | `behaviour.rs:259, 255-257` | Gossip-score penalty only; >8 identities/IP on gossip |
| **application peer score / ban** | **none** | `behaviour.rs:247-273` | Only protocol-behaviour + IP-colocation; **no app ban/score** |
| peer-store cache | 500 peers, outbound-only, evict oldest | `peer_store.rs:21-22,75-87` | Inbound can't poison the restart cache |

**Rotation / stickiness.** `maintain_automatic_outbound` (`network.rs:5365`) only ever *adds* toward the
target or *prunes excess above it*: `ordinary_release_needed` requires `topology_peer_count() >
AUTOMATIC_OUTBOUND_TARGET` (`network.rs:5645-5647`). There is **no idle rotation and no periodic re-dial of a
healthy sub-target peer.** A healthy neighbour is dropped only when its own connection closes (then
`accelerate_discovery`, `network.rs:1873`, dials a replacement from the candidate pool). This is the
**eclipse-stickiness constant**: `topology_peers > 8`.

---

## 3. The eclipse model

Assumptions, labelled **[code]** (driven/cited from pinned source) vs **[model]** (derived under stated
inputs).

### 3a. Fresh node (before first honest contact)

**[code]** A fresh node with no `peers.json` derives its *entire* candidate pool from the DNS-seed Kademlia
walk: it dials the 4 embedded seeds (`main.rs:2256-2298`), runs `GetClosestPeers(random)` off them
(`network.rs:4819-4823`), and feeds every returned address into the candidate pool via `add_peer_candidate`
(`network.rs:6948`). The seeds are `mseed{1..4}.parano1d.org`, one apex, dialed `/dns/…/tcp/9600` with **no
PeerID** and **no node-side checkpoint** to cross-check what they return (operator-power lane, confirmed).

Two sub-cases (PoC §D):

- **(a) Attacker controls the bootstrap** — the seed operator, the apex DNS, or simply the set of addresses
  the seed's DHT returns. Then the honest-address share of the candidate pool is `H = 0`, and
  **honest-escape probability = 0** for a NATed fresh node (no honest peer can dial *in* either). This is the
  decisive case, and it needs **no P2P sophistication at all** — it is purely a bootstrap-trust failure. The
  four seeds are a single point of trust with a single administrative domain.
- **(b) Seeds honest, attacker floods the DHT** across `G` /16 groups while `H` honest addresses survive in
  the pool. The node dials toward 8 through the real diversity gate; the **2-per-/16 cap works *for* the
  defender here** — the attacker cannot crowd honest /16s out of the shuffled dial set. Escape probability
  (20 000 trials each, real gate):

  | honest /16s `H` | attacker /16s `G`=4 | `G`=16 | `G`=64 |
  |---|---|---|---|
  | 1 | 0.458 | 0.082 | 0.022 |
  | 2 | 0.689 | 0.157 | 0.041 |
  | 4 | 0.883 | 0.295 | 0.079 |

  So even with honest seeds, a *heavily* DHT-flooded fresh node (G≥16, H≤2) is eclipsed with 84–98%
  probability per boot. But the clean fix for (b) is a larger honest `H`; there is no clean fix for (a)
  without a second, independent root of trust (a pinned checkpoint or a diverse seed set).

**Fresh-node eclipse cost:** to *be* the pool and win all 8 outbound slots against the 2-per-/16 gate the
attacker needs **4 distinct /16 groups and 8 peer identities** (PoC §B, driven on the real gate). Controlling
the seed/DNS collapses the honest share to zero regardless.

### 3b. Established node (displace a healthy set)

**[code + model]** The set is **sticky** (§2): no rotation below target. An established victim is only
re-dialed when a current neighbour disconnects. So eclipse requires a **churn attack**: force each honest
neighbour to drop, then win its replacement dial. Mechanics:

1. No app-level score means the attacker pays no reputational cost for repeatedly connecting/disconnecting,
   and an honest peer that flaps is treated identically to an attacker peer on re-dial.
2. On an honest drop, `accelerate_discovery` pulls the next 2-s tick forward; the replacement is shuffled
   from the candidate pool. If the attacker has stuffed the pool (via FIND_NODE responses it serves) and owns
   the free /16 budget, the replacement is attacker **[model]**.
3. Inbound can only ever hold **4** of the 8 topology slots (`MAX_UNSELECTED_TOPOLOGY_CREDIT`), so the
   attacker must win **≥4 outbound** dials — i.e. control **≥2 /16 groups** for the outbound minimum, **4
   /16 groups** for the full set (PoC §B, real gate). Inbound-only pressure **cannot** eclipse (PoC §C).

**Escape for an established, non-NATed victim** is still possible through honest *inbound* contact (a healthy
public node that dials in occupies one of the 4 credited inbound slots), which is why a NATed/GUI victim —
undialable, so outbound-only — is the realistic eclipse target. This matches C26's "fresh-or-eclipsed-victim"
precondition exactly.

### 3c. Honest-escape summary

| Victim | Sticky? | Honest escape | Minimum attacker footprint to eclipse |
|---|---|---|---|
| Fresh, NATed, attacker-controlled bootstrap | n/a (no set yet) | **0** | own the 4 seeds / apex DNS (operator-power), or 4 /16s + the DHT pool |
| Fresh, honest seed, DHT-flooded | n/a | 2–46% per boot (table) | 4 /16 groups, ~24+ identities to flood the pool |
| Established, NATed (GUI) | **yes** (`>8`) | only via attacker-peer churn/drop | 4 /16 groups + churn each honest peer |
| Established, public (dialable) | yes | honest inbound reaches it | eclipse impractical without also blocking inbound |

---

## 4. Chaos-per-attacker-box (finding constants → footprint)

One **box** = one machine with one public IP (the unit the diversity policy gates). libp2p keypairs are free,
so "identities" are cheap; IPs and /16 groups are the scarce resource. All rows are grounded in the named
finding's measured constants.

| Finding | Chaos delivered | Identities | Distinct IPs / /16s | Gate / caveat |
|---|---|---|---|---|
| **C17** | Pin the whole 64 MiB outbound serving domain → serving flips from delay to **rejection for ALL peers** | 1 identity, 4 stalled pull streams (≤8/conn, 2 conn/id) | **1 IP** | request-response; rotate pulls ≤30 s; no acceptance impact |
| **U21** | Monopolize the global gossip byte window → **silences tx *and* block gossip** every window | ~5 identities × 303 KB undecodable (<8/IP, no colocation penalty) | **1 IP** | pre-decode window, shared with block announcements |
| **C22** | Occupy all admission/delivery slots at a **62× CPU discount**; honest tx dropped | ~10 (code's 84 ms budget) … ~84 (this laptop) Push identities | **1 IP** (≤32/id per IP) … **3 IPs** (84/32) | `MempoolRequest::Push`, the one un-profile-gated arm; anonymous |
| **ECL-fresh** | Own all 8 outbound of a fresh/NATed victim | 8 peer-ids (2 per /16) | **4 /16 groups** + control of the seed/DHT pool | first-come sticky; needs `H=0` honest in pool |
| **ECL-est** | Churn-displace a healthy set | 8 outbound (2/ /16) + inbound fill | **≥4 /16** outbound, **≥2 /16** to flood 128 inbound | must evict each honest peer *and* win its replacement |
| **C26** | Snapshot-poison the isolated victim → **permanent estrangement, self-propagating re-serve** | 1 serving identity | 1 IP to serve + the ECL footprint to isolate | fork-choice: victim must be fresh or eclipsed first |

**Per-tier (one box == one public IP):**

- **1 box** — C17 serving starvation **+** U21 gossip silence **+** (hardware permitting) C22 queue occupancy.
  A single IP denies one victim's serving, gossip, and admission simultaneously, with **zero** ability to
  eclipse (eclipse needs 4 /16s). Pure availability DoS, no isolation, no acceptance impact.
- **2 boxes (2 /16s)** — adds capture of the **4-minimum locally-selected outbound** (2 /16 × 2 peers). Half
  eclipse; honest inbound still leaks to a dialable victim. Against a NATed GUI victim this is already most of
  the way to isolation.
- **4 boxes (4 /16s)** — **full 8-outbound capture** of a fresh/NATed victim → stack **C26** snapshot
  poisoning on the now-eclipsed victim → rule-invalid history committed *and* finalized, permanent
  one-sided estrangement, and the victim **re-serves the poison to the next syncer**. Four boxes = isolation
  **+** poison **+** self-propagation. The dark-surface hidden `--genesis` flag is the attacker-chain
  generator (cross-ref C26 composition note).

---

## 5. RPC-side equivalent (clients that do use remote RPC)

For the surfaces in §1 that talk to a remote node rather than spawning one (noid-wallet backend, SDK,
permanode, any user-pointed endpoint): **there is no multi-endpoint quorum or rotation in any client.** The
math is therefore trivial and maximal — **one malicious endpoint is the entire chain view** (C24 is the full
demonstration: phantom receipts, fictional "sent", suppressed balances, decorative status). The only
mitigations present anywhere are: noid-wallet's **SPV header check from a built-in checkpoint** (so headers,
and only headers, cannot be forged by the wallet server) and parano1d-mcp's **loopback-literal pin** (which
narrows *who* can be the endpoint to local processes, not *how many*). Neither is a quorum; neither rotates.
A single compromised/hostile wallet-server or RPC endpoint deceives every balance, asset list, fee quote, and
confirmation it serves, bounded only by what the client re-derives itself (SPV headers on noid-wallet; nothing
on the SDK/GUI).

---

## 6. Negative and control results — what prevents worse

Each guardrail named with its constant; these localize exactly how far one-to-few nodes get.

- **Inbound alone cannot eclipse** — `MAX_UNSELECTED_TOPOLOGY_CREDIT = 4` (`network.rs:505`): inbound peers
  satisfy at most half the 8-target; ≥4 slots must be the victim's own outbound dials. PoC §C drives this.
- **/16 diversity caps the cheap path** — `MAX_PUBLIC_OUTBOUND_PEERS_PER_GROUP = 2` (`peer_diversity.rs:22`):
  owning all 8 outbound needs **4 distinct /16 groups**, not 8 cheap identities on one host. REAL gate, PoC §B.
  This same cap *protects* a fresh node whose honest seed is reachable (§3b, PoC §D).
- **Per-IP / per-/16 inbound caps** — 32 identities/IP, 96 connections//16 (`peer_diversity.rs:26-27`),
  with a 32-slot reserve for underrepresented /16s (`:33-34`): one source cannot fill the 128 inbound budget.
  REAL gate, PoC §C.
- **Peer-store cannot be poisoned by inbound** — only successful *outbound* peers are cached
  (`peer_store.rs:51-52, 75-87`), 500 cap, evict-oldest: a Sybil inbound flood does not survive the victim's
  restart.
- **Kademlia `OnConnected` inserts** (`behaviour.rs:453`): phantom FIND_NODE hearsay does not enter the
  routing table; only peers with live connections do (though hearsay *does* enter the dial *candidate* pool).
- **Loopback-whitelisted colocation** (`behaviour.rs:255-257`): local multi-process test clusters keep
  realistic mesh behaviour; the >8/IP gossip penalty applies to public identity farms only.
- **Profile + namespace gate** (`network.rs:7070-7103`, C9): a peer on a different consensus schedule is not
  dispatchable for sync; eclipse mixing yields incompatible-peer DoS, not cross-version acceptance.
- **No invalid acceptance anywhere in this layer**: every chaos row is availability/liveness or isolation;
  none accepts an invalid block or forges a spend (consistent with the whole audit's escalation barrier).

**What is absent and would have helped (cited by absence):** no application peer score, no ban/greylist, no
node-side bootstrap checkpoint, no second independent seed authority (all four under `parano1d.org`), no
endpoint quorum/rotation in any client, no idle peer rotation to shed a captured set.

---

## 7. PoC implementation and reproduction

`src/peer_diversity_vendored.rs` is the pinned `noid_p2p/src/peer_diversity.rs` copied **byte-identical**
(`shasum 6c696510…`); the PoC compiles and drives it directly, so every diversity decision in §B/§C/§D is the
production gate, not a reimplementation. The topology-controller logic (`AutomaticPeerState` has private
fields and pulls the whole node crate, so it is not compiled here) is modelled from `network.rs` with its
exact constants cited inline (§A/§E).

From this directory:

```bash
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Key output (`results.txt`):

```text
A1 outbound /16 cap: 2 admitted, 3rd distinct peer in same /16 -> Err(OutboundGroupFull { group: Ipv4([11, 5]) }) [REAL]
B4 attacker /16 groups = 4: outbound slots won = 8/8  (FULL outbound capture)
B2 attacker /16 groups = 2: outbound slots won = 4/8  (captures the >=4 locally-selected minimum)
C1 one public IP: admitted 32 inbound identities, cap = 32 [REAL]
C2 one /16 (many IPs): admitted 96 inbound connections, cap = 96 [REAL]
(a) honest_addrs = 0  -> escape probability = 0.000  [definitional: no honest addr exists]
(b) H=1 honest /16s, attacker G=16 /16s: P(dial >=1 honest) = 0.082
E topology_peers=8: ordinary_release_needed = false  (healthy set retained — NO rotation)
E topology_peers=9: ordinary_release_needed = true  (prunes EXCESS above target)
ALL SECTIONS COMPLETE
```

---

## 8. Assurance limits

- **The gate is real; the controller is a model.** §B/§C/§D run the pinned `PeerDiversity`. §A/§E reproduce
  `ordinary_release_needed` and the diversity caps but the full `AutomaticPeerState` dial scheduler, the
  Kademlia walk, and the 2-s maintenance loop are *modelled from cited constants*, not executed against a
  live swarm. The stickiness verdict rests on reading every drop path in `network.rs` (release requires
  `topology_peers > 8`; disconnect-triggered replacement is the only other path) — a source argument, not a
  live-churn measurement.
- **The fresh-node escape probabilities are model outputs** over a synthetic candidate pool with the stated
  honest/attacker /16 structure, through the real gate. They are sensitive to the assumed honest-address
  count `H` and attacker group count `G`; they are an argument about the *shape* of the escape curve (and the
  decisive `H=0` collapse), not a field measurement of real mainnet DHT composition.
- **Case (a) rests on operator-power's finding** that the four seeds share one apex with no PeerID and no
  checkpoint; this lane does not re-establish *who* controls them, only the consequence (`H=0 → escape 0`).
- **devnet-chaos cross-check pending.** The live multi-aggressor lane runs C17/U21/C22 composition and
  eclipse topology against real libp2p (negotiated yamux windows, measured timeouts). This model predicts:
  1 box = DoS-no-eclipse; 4 /16s = full outbound capture; sticky set once formed. Agreements/disagreements
  to be reconciled when that lane lands; no live swarm was run here.
- No upstream source was edited. The vendored gate is a verbatim copy for compilation; the pinned tree is
  untouched. Pre-existing unrelated working-tree modifications were left alone.
