# mainnet-observe — public parano1d mainnet state, read-only

**Lane:** `mainnet-observe` · **Observation window:** 2026-11-16T07:00:40Z → 07:09:09Z (local
2026-11-15 23:00–23:09 PST; node stopped by `run/node.pid` at window close). Final reading
before stop: tip 314,223, 112,635 slots, 11 peers. · **Node:** stock release binary
`experiments/devnet-chaos/bin/parano1d` v2.1.0, `--mode node` (default, no mining),
`--data-dir experiments/mainnet-observe/run/node`, RPC 127.0.0.1:21161, P2P 127.0.0.1:21162,
`--log info`. No `--genesis`, no `--disable-dns-seeds`, no `--seed` — pure default discovery.
Everything sent to the network was ordinary peer traffic of one syncing node (header/snapshot
requests, one gossipsub mempool reception). No transactions, no mining, no RPC writes.

Node PID file: `run/node.pid`. Full node log (evidence for every timing below):
`logs/node.log`. Raw DNS/whois answers taken during the window from this machine.

---

## 1. Seed discovery

Compiled-in mainnet DNS seeds (source: `noid_chain/src/consensus/network.rs`, `NetworkConfig::mainnet()`),
dialed as bare hostnames `/dns4/<host>/tcp/9600` — the "no PeerID verification" format:

| seed | A records (dig, 07:01Z) | whois attribution |
|---|---|---|
| mseed1.parano1d.org | 66.29.128.164, 209.74.93.214 | both Namecheap (NAMEC-4 / Web-hosting.com, 66.29.128.0/19) |
| mseed2.parano1d.org | 203.161.32.161, 162.0.227.4 | 203.161.32.161 unattributed stub net (ZZ); 162.0.227.4 Namecheap 162.0.224.0/20 |
| mseed3.parano1d.org | 69.67.172.157, 162.0.227.238 | BL Networks (BNL-77); 162.0.227.238 Namecheap 162.0.224.0/20 |
| mseed4.parano1d.org | 168.100.9.26, 162.252.198.227 | BL Networks (BLNETWORKS-01); BL Networks (BNL-77, 162.252.198.0/23) |

- All four hostnames resolve; 8 distinct IPs, no shared single IP.
- Zone/IP concentration: mseed2 and mseed3 share the Namecheap **162.0.224.0/20** for one
  record each; mseed3 and mseed4 each hold a **BL Networks** record. Two hosting providers
  (Namecheap shared hosting, BL Networks) cover 7 of 8 records; domain NS is
  dns1/dns2.registrar-servers.com (Namecheap registrar). Seeds are not a Sybil-resistant spread.
- **Peers gathered:** first peer connected **T+7s** (07:00:47Z); 17 distinct peer IDs connected
  within the first ~10 min (log `peer connected` lines); steady-state **10 concurrent**
  (`paranoid_getPeerCount`), 2 circuit-relay reservations (loopback listener → relayed
  reachability works). Docs: peer store caps at 500, Kademlia + mDNS continue after bootstrap;
  topology manager targets 8 ordinary neighbours.
- **First headers:** header staging began T+13s (07:00:53Z, `snapshot: staging exact boundary
  headers target_height=314190`); 16,385 headers on disk by T+16s; full boundary 314,190
  staged by **T+53s** (07:01:33Z).

## 2. Chain state at observation time (2026-11-16T07:04–07:06Z, post-sync)

All from `paranoid_getChainInfo` / `paranoid_getStateInfo` / `paranoid_getMiningInfo` /
`paranoid_getBlockHeader` on the synced local node:

| fact | value | source |
|---|---|---|
| tip height / hash | **314,214** `aef50aac47cc5530eeb039616c553d7e32254c98e64b4846391d7027b6430732` (tip block ts 07:02:01Z; 314,220 by 07:06Z) | getChainInfo |
| **log_slots** | **24** (capacity 2^24 = 16,777,216 slots; log_slots_max 32 protocol-wide) | getChainInfo/getStateInfo |
| **active_slot_count** | **112,625** (at h=314,214; 112,632 at h=314,220) — **0.67% fill** | getChainInfo |
| distance to 50% (8,388,608) | 8,275,983 slots (1.34% of the way there) | arithmetic |
| distance to 75% (12,582,912) | 12,470,287 slots — matches RPC `slots_until_expand` exactly (expand_trigger_pct 75) | getStateInfo |
| difficulty target | `89a1827bdc0a2217db029f0142beef73889dc32cf5b9a3bed1f3070000000000` (LE) = 2^211, **difficulty_bits 45** — 2^27× the genesis target (2^238) | getMiningInfo + params.rs |
| chainwork | **not exposed by any RPC** (ChainInfo/BlockHeaderInfo carry the per-block target only) | api.rs/types.rs |
| supply (live UTXO sum) | 12,164,762.1575 NOID at h=314,214; ≈ +16 NOID/block gross (tier-0), one +4,651 NOID object→UTXO flow observed in-window (DEX/pool/backing flows move the UTXO sum) | getChainInfo |
| state size | 5.4 MB encoded / 27.0 MB materialized / 800 MB encoded at 2^24 domain max / 200 GB at 2^32 protocol max | getStateInfo |
| mempool | 2 pending TXs, 179,263 B of 384 MiB, fee floor 5,000 μNOID | getMempoolStats |
| block reward | 16 NOID (tier 0, post-fork schedule) | getMiningInfo |
| genesis | H1 ts 2026-08-21T00:00:01Z → chain age ~87 days; 314,214 blocks / 87 d ≈ 23.9 s/block lifetime average | getBlockHeader[1] |

**Observed block rate:** 30s nominal. **31.2 s/block over the last 300 blocks**
(313,914→314,214: 9,353s); post-v2-fork average **exactly 30.0 s/block**
(H210,537→314,214: 3,111,188s / 103,677 blocks); pre-fork ≈ 20–21 s/block (genesis target is set
"twice as fast" for young networks per params.rs; ASERT then converges to 30s — visible in the
height↔timestamp series).

**Slot-occupancy history** (`active_slot_count` from getBlockHeader at sampled heights):

| height | date | slots |
|---|---|---|
| 1 | 2026-08-21 | 1 |
| 10,000 | 08-24 | 6,264 |
| 50,000 | 09-03 | 22,681 |
| 100,000 | 09-15 | 41,546 |
| 150,000 | 09-27 | 64,925 |
| 210,537 (v2 fork) | 10-11 | 88,079 |
| 250,000 | 10-24 | 98,365 |
| 300,000 | 11-11 | 111,482 |
| 305,000 | 11-13 | 112,263 |
| 310,000 | 11-14 | 112,317 |
| 312,000 | 11-15 | 112,247 |
| 313,000 | 11-15 | 112,638 |
| 314,000 | 11-16 | 112,772 |
| **314,214 (tip)** | **11-16** | **112,625** |

Net growth: **~1,294 slots/day** lifetime average → **~+115/day** over the last 3 days →
**flat/declining** over the last day (112,638 → 112,625). Gross coinbase mints are ~2,880
slots/day (1 coinbase UTXO per block), so ~96% of minted slots are subsequently spent or
consolidated away.

## 3. Sync behavior (fresh node, this window)

From `logs/node.log` (info level):

| T+ | event |
|---|---|
| 0 s | launch, empty data dir, wallet created |
| 7 s | first peer connected (07:00:47Z) |
| 13 s | header staging begins; 16,385 headers staged by T+16s |
| 53 s | full header range 1→314,190 staged (`header_disk_validation`: O(H), 314,190 headers, 66.6 MB, 12.9 s active work) |
| 54–59 s | HistoryStep terminal verified — **O(1)**, 1 proof, 878,827 bytes, 2.66 s, accepted |
| 59–80 s | state transfer: 15 segments, 6.49 MB canonical (1.74 MB wire, all compressed), 6 requests, 20.4 s wall; segment install 9.0 s |
| **79 s** | **atomic State install at boundary 314,190** (07:01:59Z) — `snapshot install completed height=314190` |
| 80 s | post-snapshot tail pinned to 314,212; node **fully synced** (`synced:true, sync_stage:"tip"`) |
| 97 s / 126 s | node already *serves* its own snapshot generations (314,190 full, 314,196 incremental 12-reuse/3-rebuild) |

During the header phase the installed State height stays 0 while the HeaderDAG stages to the
boundary, then one atomic install lands the State at the same height
(`getNodeStatus` stages: `headers` → `state` → `tip`; serving_limit 314,232 at install).
Data dir after sync: 370 MB. A fresh laptop node reaches full mainnet state in **~80 seconds**.

## 4. Explorer cross-check

- The only public explorer found (Kagi; linked as "Parano1d Explorer") is
  **https://parano1d-pool.fun/explorer/** — it (and the whole pool site + its API paths) returned
  **503 "Pool offline for maintenance"** throughout the window. No alternative hosted explorer
  exists (docs describe only the wallet's local Scope view; parano1d.org links none;
  explorer.parano1d.org doesn't resolve). Internet Archive/Wayback was also temporarily offline —
  no archived snapshot cross-check possible. **Live tip/supply cross-check was therefore not
  performable today; recorded as a finding in itself (public mainnet has no live hosted explorer
  at the moment).**
- Cross-checks performed instead:
  - **Supply vs documented schedule** (docs.parano1d.org/protocol/economics + `noid_chain`
    params.rs `BASE_REWARD_MICRONOID = 50 NOID`): post-fork 103,677 blocks × 16 NOID =
    1,658,832 NOID + pre-fork 210,537 × ≈50 NOID ≈ 12.19M gross vs **12,164,762 observed live sum**
    — within ~0.2%, gap consistent with fee/growth burns. Agrees.
  - `getMiningInfo.block_reward_noid = 16.0` matches the documented tier-0 row (16 NOID from
    H210,537). Agrees.
  - `slots_until_expand` 12,470,287 = 12,582,912 − 112,625 exactly. Internally consistent.
  - Post-fork mean block time 30.0 s/block matches the 30 s ASERT target. Consistent.

## 5. What this means for the growth-armed findings

- Mainnet is **0.67% occupied** — 8,275,983 slots below the 50% (8,388,608) and 12,470,287 below
  the 75% (12,582,912) threshold where the occupancy-fee multipliers (1×/2×/4×/8× at
  50%/75%/90%) and the state-expansion trigger engage. Today's pressure multiplier is 1×.
- Net occupancy growth has **decelerated an order of magnitude** (~1,294/day lifetime → ~115/day
  this week → ~0/day yesterday) because ~96% of minted coinbase slots are spent/consolidated
  away. At the recent rate the 50% threshold is **~2 centuries** out; even at the lifetime
  average it is **~17 years**; reaching it inside the audit's horizon needs a sustained >10×
  acceleration in net slot creation (i.e., real adoption), not the current trajectory.
- So the growth-armed halt constructions (halt-stalefee's 8.4M-slot 50% fee-pressure
  deadlock, halt-reorg's 12.58M-slot 75% state) are **devnet/hbox-scale constructions, not
  imminent mainnet events** — their premise (occupancy pressure) is 75× away in slot count and
  shows no current growth trend toward it.
- Network-health facts that do bear on those findings: the mainnet is **healthy and fast**
  (30.0 s/block post-fork, 17 peers seen by a fresh node, mempool non-empty, difficulty 2^27×
  genesis), the O(1) snapshot sync works on mainnet exactly as the whitepaper claims
  (full state in 79 s from nothing, 2.66 s HistoryStep), and a fresh synced node immediately
  becomes a snapshot *server* (publishes generations at T+97s) — snapshot-serving
  centralization is low today (any node can serve), while seed/zone centralization is real
  (two hosting providers, one /20 shared across seeds).

---

*Method note: all RPC values above were taken read-only via JSON-RPC POST to 127.0.0.1:21161
(methods prefixed `paranoid_`); timestamps from the node log and block headers; DNS/whois from
this machine during the window. The node was stopped by `run/node.pid` at the end of the window.
Node binary identical to the project's release build; nothing on mainnet was written by us.*
