# Centralization map: where one party can actually rear its head

Scout lane `scout-centralization`, 2026-11-15/16. Target: pinned `parano1d/` @
`52938433a30b185ab007afaa15166a632cf2e82e` (read-only) plus gentle live reads of
public services. This report consolidates the audit's prior art into one map,
answers "what can the holder DO" for each control point, and runs the cheap
read-only probes that were never measured. Heavy PoCs are left to followup lanes.

Prior art consolidated: operator-power (REPORT + OPERATOR-ATTACKS, DEEPER-CONTROLS,
FUND-OBSERVATION), operator-privileges, network-status, release-chain
(OPERATOR-CONTROL, REPORT), eclipse-topology (results.txt), snapshot-adversary,
operator-exit (REPORT + OPERATOR-THREAT), FINDINGS C16/C22/C24/C26/C27.

## 1. The map

Ranked by (malice reach x holder-plausibility), not by novelty. "Measured" = a
dated observation or local run exists; "unmeasured" = cheapest probe that would
show it rearing its head.

| # | Control point | Holder | What the holder can DO (concrete malice) | Measured | Unmeasured — cheapest probe |
|---|---|---|---|---|---|
| 1 | **Default-seed DNS zone** (4 unpinned `mseed1-4.parano1d.org`, dialled `/dns4/../tcp/9600`, no PeerID pin — `network.rs:111-134`) | Whoever runs the `parano1d.org` zone (plausibly the author; hosting-adjacent, unproven) | Point every fresh/default node at chosen endpoints. With all 4 names (or the zone) controlled, a fresh/NATed node's honest-address pool H=0 -> eclipse escape probability 0.000 (eclipse-topology D(a)); stack C26 snapshot poisoning on the eclipsed victim and it re-serves the poison. Also governs a miner's readiness lease via one peer echoing its tip. Does NOT touch established nodes (retained chainwork + peers.json). | DNS A-records resolved twice with **conflicting results** (operator-power: Namecheap+BL-Networks mix, 11-16 ~00Z; network-status: 6/8 IPs Namecheap shared *web hosting* PTRs, 11-16 ~00:20Z) | **Do the endpoints serve P2P on :9600 at all?** (never tested — TCP connect). Endpoint diversity: how many distinct operators serve the 8 A-records. Probe in 2.1. |
| 2 | **Release channel** (GitHub `proof-native/parano1d` + Forgejo canonical + mirrors; SHA256SUMS same-channel; no detached signature, no verified independent rebuild) | Release publisher (author) | A new binary can change wallet-send destinations, exfiltrate the keystore, censor locally, or fork consensus for new installers — undetectable to a user who verifies only against the channel's own SHA256SUMS. Installed v2.1 nodes have no auto-update, so existing nodes are unaffected until they upgrade. | Process review (C16 boundary: official v2.1 path authenticates packs; standalone builder had the gap) | Is there a newer release than v2.1.0 *now*? Do published checksums still match the downloadable binaries? (channel integrity drift). Probe in 2.2. |
| 3 | **Block production** (200-block window at 2026-11-16 00Z: `o189lhaayzfnsk...` 49.0% + luckypool coinbase `o1sk6hjgwlwpm...` 34.5% = 83.5%) | Unknown solo miner (49%!) + luckypool.io operator | A winning producer chooses which valid transactions to include — omit one holder's deposit while including its own (asymmetric exchange exit). Sustained exclusion needs production share + fork-choice weight; at 49% solo + coordinated 34.5% a reorg up to the 18-block finality boundary is reachable. Also: whoever *buys/rents* that hashrate inherits the same power (~0.9 TH/s Poseidon2b — cheap?). | One dated 200-block window | Is the 49% miner persistent hours/days later? Is it one operator or a pool? Who are they (social)? Probe in 2.3. |
| 4 | **Dev-fund keys** (2 hardcoded addresses, 90/5/5 split through h3,223,778; 599,040 NOID each by h313,800; two unspent 500k-NOID outputs = 8.22% of net supply) | Unknown keyholders (author-adjacent by docs) | Dump 8.22%+ of supply onto a one-venue market doing ~$75k/day — that alone can break the price; no freeze/blacklist power, only ordinary-spend market power. Also *unspent since H225,854/9* is itself a standing overhang. | Balances at h313,800 (one explorer view) | Current balances (spent down or still parked?). Probe in 2.4. |
| 5 | **SafeTrade** (sole venue: NOID/USDT, ~$75k/day, top-1 address = 26.8% of supply sits there) | Exchange operator (independent of author — unverified) | Per-currency deposit/withdrawal switches: freeze new deposits while pre-credited balances (possibly its own) can still trade — the cleanest asymmetric-exit shape. Also delisting = total fiat exit loss. | Listing, price, volume, terms 6.1/7.1 | NOID deposit status / min confirmations row; deposit-node count (one node = single point of halt). Probe in 2.5. |
| 6 | **Snapshot providers** (any full node; 5-second candidate window, advertised-work ranked, rechecked at install) | Any well-connected node operator (incl. seeds) | Serve a chosen initial state to fresh joiners — bounded by work-ranked selection and install recheck unless *every* reachable provider is controlled (eclipse-topology: 4 /16s + seed/DHT control). False-work offers can stall a joiner ~45s+/cycle. | Component runs (C26 boundary: genuine terminal rejected future-dated tip; stubbed middle-header case open) | Fresh-joiner candidate timing against live providers (needs a run — followup lane). |
| 7 | **Pool operators** (luckypool: 47 miners / 600 workers / 350.8 GH/s = 38.5%; gb-pool vestigial 1 miner; AriaPool paused) | luckypool.io operator | Pool accounting host is a spending trust point for paired wallets (operator token -> walletSend); stratum endpoints in 7 regions concentrate 38.5% of hashrate behind one operator's payout honesty; a pool-owner can also censor via template choice. | luckypool /api/stats snapshot | Pool payout honesty over time; miner count trend. Probe in 2.6 (light). |
| 8 | **Explorer (noidexplorer.org)** — community, self-funded | Third party (@noidexplorer) | Lies to *humans* about balances/confirmations (not to nodes); a wrong explorer view can induce bad user decisions (e.g., "deposit confirmed" when it isn't). | Dated agreement with 2 pools + wallet checkpoint | Drift between explorer and independent node/pool views. Probe in 2.6 (light). |
| 9 | **tradedata.parano1d.org** (author) | Author | Substitutes historical DEX candles in GUI (structural checks pass); withholds NFT media. No route into spends/receipts found. | Source review | — (bounded; low priority) |
| 10 | **Canonical source + docs** (git.parano1d.org, docs.parano1d.org) | Author | Social-layer control: what code exists, what new users read; enables silent history rewrite for fresh cloners (tag moves). Same trust shape as #2. | Forgejo API snapshot (last push 2026-11-15) | Tag drift vs GitHub mirrors. Probe in 2.7 (light). |
| 11 | **GUI node selection** (`NOID_GUI_NODE_BIN`/PATH/RPC) | Local machine / whatever installed it | Impostor node feeds the GUI chosen "verified" values, swapped receive address (C24). Local trust, but it is *the* wallet UX for most users. | C24 local runs | — (measured enough) |
| 12 | **Jetsam fork** (own mainnet, no market) | jetsamchain org | Not a NOID control point; context only: forked clients and their users are a separate ecosystem where the same author-adjacent patterns recur. | Live JTM RPC snapshot | — |

Points deliberately *not* re-ranked here: RPC operator token (local-only), mDNS
(LAN), Android wallet checkpoint (third-party publisher), genesis (fixed in source
— substitution requires #2).

## 2. Live probes (gentle reads, dated)

(filled in below as run — sections 2.1-2.7)

## 3. Top followup probes for a PoC lane

(to be ranked at the end)

### 2.1 Seed DNS + endpoint liveness (run 2026-11-16T06:52Z; raw in `seed-dns-*.txt`)

- **A-records are stable**: all 8 addresses identical to operator-power's 2026-11-16 ~00Z
  observation. mseed1 → 66.29.128.164, 209.74.93.214; mseed2 → 203.161.32.161, 162.0.227.4;
  mseed3 → 162.0.227.238, 69.67.172.157; mseed4 → 168.100.9.26, 162.252.198.227.
  `_dnsaddr.parano1d.org` TXT still empty (unpinned format is the only live one).
- **All eight IPs ACCEPT TCP on :9600** — first measurement ever; network-status's
  doubt ("shared hosting cannot obviously run a libp2p seed on :9600") is answered:
  the seeds are live listeners, not parking-grade DNS. Two connect-and-hold reads
  returned no banner bytes, consistent with a libp2p noise listener waiting for the
  initiator (transport confirmed noise+yamux, `noid_p2p/src/network.rs:3741-3742`).
- **Endpoint diversity (network level)**: 4 IPs on Namecheap ranges (NAMEC-4:
  66.29.128.0/19, 209.74.64.0/19, 162.0.224.0/20 — PTRs `nc-ph-*.web-hosting.com`,
  shared web hosting), 1 RIPE stub (203.161.32.161), 3 on BL Networks
  (69.67.172.0/23, 162.252.198.0/23, and **168.100.8.0/22 — the same /22 as the
  canonical site/Forgejo 168.100.11.250**, so mseed4's 168.100.9.26 is almost
  certainly author-hosted).
- **Still unmeasured — operator diversity**: whether the 8 listeners share PeerIDs
  (one operator behind 4 names × 2 IPs) is invisible without a noise handshake.
  Namecheap shared-hosting PTRs mean the *machine tenant* is not even identifiable
  from the IP. This is the single cheapest high-value followup: run a pinned local
  node (or minimal libp2p client using noid_p2p) against the seeds, collect
  identify/Kademlia PeerIDs for all 8 endpoints. One operator → cosmetic diversity;
  also settles "author runs 100% of default bootstrap."

### 2.2 Release channel (run 2026-11-16T07:0xZ; GitHub API + one asset download)

- **No new release**: v2.1.0 (published 2026-10-23) is still latest on GitHub
  (`proof-native/parano1d`) *and* on canonical Forgejo (`ignotusnemo/parano1d`,
  tags list v2.1.0 = newest). No tag drift between channels. Forgejo username
  `ignotusnemo` confirms author identity of canonical source; repo `main` has
  moved past the pinned checkout (e6d324ff70fa) — expected, development continues.
- **Channel integrity spot-check**: downloaded `history-step-pack-v1.tar.gz`
  (237 KB, cheapest asset) from the v2.1.0 release; its SHA-256
  (`3f0e7e32…460231`) matches the published `SHA256SUMS` exactly. (Same-channel
  check — still does not independently authenticate the publisher, per C16 review.)
- **Distribution telemetry (new observation)**: v2.1.0 binaries downloaded only
  ~76 (linux-x86_64 core) + 56 (windows core) + 34 (windows GUI) + ~40 others
  ≈ **150–250 binaries total in 3+ weeks**, versus 3,363 funded addresses and a
  912 GH/s network. The vast majority of holders/users do NOT run a release
  binary from this channel — they are on the exchange, third-party wallets, or
  the (unmapped) web wallet. Every user-facing control point that is NOT the
  release binary therefore has outsized weight; the release channel's *reach* is
  smaller than its trust weight suggests.

### 2.3 Block production concentration (fresh 200-block sample, 2026-11-16T07:1xZ)

Explorer `/api/v1/blocks?limit=200` sample spans ~h313,790–313,990 (blocks
endpoint lags the stats tip h314,190 by ~200 blocks). Raw: `blocks200.json`.

| Coinbase address | Prior window (00Z, h~313.2-313.4k) | This window (~07Z, h313.8-314.0k) |
|---|---|---|
| `o189lhaayzfnskq36fnrt430c35gk8p7j9ql23nctk50tqwd0cs95qsv6p59` | 49.0% | **49.5%** — persistent |
| `o1sk6hjgwlwpmcakad…` (luckypool coinbase) | 34.5% | 25.5% — down |
| `o1sks93zzutcxq98gf…` | 7.0% | 10.5% — up |
| others (5 more) | 0.5–2.5% | 1.0–4.0% |

- **The 49% "solo" miner is persistent across ≥5 hours and two independent
  samples.** It has mined 43,737 blocks all-time, first at h226,979, last at
  h314,191 (still active at probe time). It holds only **6,068 NOID** (666
  UTXOs) — it sweeps its ~1.5M NOID lifetime earnings (90% of ~38.7 NOID/block
  × 43.7k blocks) out continuously.
- **Where the sweeps go (new, from miner tx pages)**: two pass-through addresses
  receive from the miner and forward essentially everything to the **top-1 hot
  wallet** (richlist #1, 3.10M NOID, presumed SafeTrade):
  - `o1xdsqrsjn9047dxpdsvkmx6gyk4s5zze6e70naute725dsl2nu2xq8mw9v4` — small
    transfers ~50–72 NOID (holds 209 NOID; 486 txs).
  - `o1m6u564arh4fmkdlqyfc46c6mca8z9sy9t5p993fwp5ny38k8x39qazkm52` — large
    transfers ~2,390–2,880 NOID every ~250–350 blocks (holds 12,063 NOID; 649
    txs).
  Combined ≈ **~2,850 NOID per ~2.5 h cycle ≈ 27k NOID/day of continuous
  exchange deposits by the dominant miner** — roughly 9% of the venue's daily
  USD volume, structurally persistent sell pressure through the single market.
  (Attribution: the two pass-throughs are either the miner operator's exchange
  accounts or exchange batching addresses; either way the flow is miner → sole
  venue. Not proof of who the miner is.)
- **Richlist (new fetch)**: #1 hot wallet 3,102,952 NOID (25.5%); #2
  `o1gtftzqpcwcp0e0nduj0lje60cus76tv0kx0nklcp446fsd37ch2sexd2wz`
  528,766 NOID (4.35%) — **funded BY the hot wallet** (inbound spends at
  h186,728 and h258,438/264,206/265,991; an exchange-withdrawal whale, not a
  miner); #3/#4 are the two unspent 500,000-NOID fund outputs; #5 ~250.8k.
  **Top-4 addresses ≈ 38% of supply; the two fund outputs are now literally the
  3rd and 4th largest holders.**
- **Fund balances (re-check of FUND-OBSERVATION at h~314,190)**: payout
  addresses unchanged (61,047.4398 / 61,112.241309 NOID); **both 500,000-NOID
  outputs STILL unspent** since H225,854/9. The 8.22%-of-supply overhang
  persists and grows as a *share* rank (now #3/#4 on the rich list).
- Explorer health: alive, same API shape; 3,363 funded addresses, 89,349 live
  UTXOs, mempool 2 txs, fee floor 5,000 µNOID — consistent with the documented
  picture. It self-reports a donation address (community-funded).

### 2.4 SafeTrade (Playwright chromium, 2026-11-16T07:5xZ — curl/WebFetch stay Cloudflare-403)

Status page row captured via headless browser:

> `NOID | ParanO1d | ParanO1d (NOID) | Deposits: **Enabled** | Withdrawals: **Enabled** | Min Confirmations: **85**`

- **85 confirmations ≈ 42.5 minutes** at 30 s blocks — nearly 5× the chain's
  18-block local-finality boundary. A reorg-based double-spend against SafeTrade
  deposits needs >85 blocks of rollback, which local finality already forbids
  (deep-reorg refusal): the exchange-side reorg attack surface is closed for
  practical purposes. The flip side: the "pre-credited balance trades while a new
  depositor waits" asymmetry window is ~42 minutes long, per deposit.
- Deposit-node topology (how many nodes SafeTrade runs) remains invisible from
  outside — still unmeasured; a single-node halt would pause credits while the
  market keeps trading pre-credited inventory.

### 2.5 Pools (2026-11-16T07:2-4xZ)

- **luckypool API moved**: `luckypool.io/api/stats` (documented by
  network-status, worked at 00:14Z) now returns "Not found"; live endpoint is
  **`noid.luckypool.io/api/stats`**. Current: **45 miners / 607 workers /
  349 GH/s** pool hashrate (essentially unchanged from 47/600/350.8 at 00Z).
- **gb-pool**: unchanged, vestigial (1 miner / 2 workers, same `online_since`
  as the 11-14 restart).
- **parano1d-pool.fun: still HTTP 503 "Pool offline for maintenance"** (also
  503 at 00:30Z) — ≥7 h outage; its 56.5K-tip chain identity stays unresolved.
- **Hashrate trend (two independent nodes)**: gb-pool reports network difficulty
  1.79e13, luckypool 2.22e13 (both vs 2.737e13 at ~00Z); my fresh 200-block
  sample averages **32.06 s/block** vs 30 s target. The network is running
  slow and difficulty is falling — a real but unattributed hashrate decline of
  roughly 20–35% in ~7 h, not concentrated in luckypool (whose own rate held).
  Luckypool's *share* is therefore rising by default.

### 2.6 NEW CONTROL POINT — the hosted web wallet `web.parano1d.org` (probed 2026-11-16T07:5xZ)

Not in any prior lane's map. The author's site links it first; the Nov-4 launch
post promotes the "web wallet/launchpad/DEX/NFT suite". Probes (raw:
`web-wallet.html`, `web-app.js`):

- It is a **non-custodial** wallet by design — "Local secret", master key +
  device password, restore-from-master-key; keys live in a browser worker.
- But **every load fetches `/assets/app.js` from the author's server** — the
  author can change wallet code for all web-wallet users instantly, with no
  install step and no version pinning. This is the release-channel trust shape
  (C16 boundary) with *faster reach than the release channel itself* (which has
  only ~150–250 downloads ever).
- And the wallet's whole node interaction is **same-origin on the author's
  server**: `/api/status` (a mainnet node — genesis `860e7045…` matches the
  pinned profile, height 314,209), `/api/prove` (the heavy proof runs
  server-side: local worker `begin` → POST bytes → local `finish`; the server
  reports `completed_jobs: 236`), `/api/relay` (transaction submission),
  `/api/receipt`.
- **Concrete malice (capability, not observation)**: the operator of
  web.parano1d.org can (a) serve changed JS that exfiltrates master keys on
  unlock; (b) see every web-wallet user's addresses and intents (privacy);
  (c) fail `/api/prove` or `/api/relay` for targeted users while others
  succeed — a clean selective-censorship point that the local UI would render
  as its own "sent" semantics (operator-exit's zero-peer result). Usage so far
  is small (236 prove jobs), but this is the promoted retail surface.

**Map amendment**: insert as row #1b (user-reach ranked above release channel,
below only the exchange for value-at-risk): *Hosted web wallet — holder: author;
malice: instant code change + prove/relay/status control for all web-wallet
users; measured: endpoints + architecture (this probe); unmeasured: whether
prove/relay failure is selective in practice, and key-exfil via modified JS.*

### 2.7 Who mines ~0.9 TH/s — public evidence (Kagi, 2026-11-16T07:4xZ)

- **Community GPU mining is real and mostly NVIDIA.** hashrate.no tracks NOID
  with a benchmarks table (RTX 4090 = 179.7 Mh/s … RTX 3050 = 17.9 Mh/s;
  **no AMD submissions**); 921 GH/s network ≈ 5,100 RTX-4090-equivalents;
  luckypool's 607 workers are the visible community fleet. Guides: bytwork.com
  (Windows/HiveOS), YouTube walkthroughs.
- **Open/closed miners**: `lucasan123/noid-gpu-miner` (NVIDIA, lookup-table
  approach, v1.4.0, used by parano1d-pool.fun); `gustlborg/gb-miner` (NOID +
  TAPE, the gb-pool operator's); **ForgeMiner (0xHashRaptor)** — multi-algo,
  **NOID on AMD "RX 400 and newer" incl. RDNA2, 1% dev fee**; a closed
  "Pearl+NOID+CMFD" bitcointalk miner claims ~2× the community per-card rate
  (4× RTX 3070 rig at "342.5 [M/T]h/s"). X-Miner video advertises AMD
  Parano1d support.
- **RX 6700 XT viability for the hbox grinder lane**: ForgeMiner supports
  RDNA2 explicitly; hashrate.no has no AMD benchmark to calibrate against, so
  the lane's measurement would be the first public AMD number. (RTX-3070-class
  ≈ 41 Mh/s ⇒ a 6700 XT plausibly lands ~30–60 Mh/s ≈ $0.10–0.25/day at
  current 5% share economics — rough.)
- **The 49% miner is publicly anonymous.** No source attributes
  `o189lhaayzfnsk…` (first block h226,979 ≈ Oct 23, "scaled up hard",
  network-status). It holds almost nothing (6,068 NOID) and funnels ~27k
  NOID/day into the exchange via two pass-throughs — economically it behaves
  like an operator selling continuously, not a treasury. Whether it is one
  person, a private pool, or (worst case for the audit) the author remains
  **the single biggest open attribution** in the map. Cheapest social probe
  left: ask in the parano1d Discord/bitcointalk ANN (5590832, read 4,344
  times) — not done here (no developer contact discipline).
- Side observations: `noid.network` = "Open Cryptographic AI Autoresearch
  Platform" tied to the author's soundness repo (discourse/narrative surface,
  Namecheap-hosted); AriaPool still paused per prior art.

## 3. Top followup probes, ranked by (impact × probability) / cost

1. **Web-wallet prove/relay/selective-failure PoC** (map row #1b). Impact:
   every current and future retail user of the promoted wallet; probability the
   *capability* exists: 1.0 (it is the author's server by construction). Cost:
   LOW — drive `web.parano1d.org` in Playwright with fetch-interception of
   `/api/prove`//`/api/relay` (fail for one user, succeed for another), and
   demo a modified `app.js` exfil path against a local copy. Deliverable: the
   precise UX a censored web-wallet user sees.
2. **Seed endpoint PeerID census** (map row #1). Impact: settles whether the
   default bootstrap — the root of the eclipse→snapshot-poison chain (C26,
   eclipse-topology D(a)) — is one operator behind 4 names × 8 IPs.
   Probability of concentration: high (mseed4 already sits on the author's
   /22; 3/8 IPs on BL Networks). Cost: LOW-MED — one pinned local node (or a
   minimal libp2p client built on the repo's noid_p2p) connects to all 8
   endpoints, records identify/Kademlia PeerIDs, and does a short DHT walk to
   count distinct operators among ordinary peers. All 8 listeners already
   confirmed live on :9600 by this scout.
3. **Fresh-joiner candidate-window measurement on mainnet** (map row #6).
   Impact: the 5-second work-ranked snapshot selection is what actually decides
   a new user's initial state; combined with probe 2 it determines real-world
   eclipse exposure. Cost: MEDIUM — one fresh data-dir node run with candidate
   logging (gentle: one node, then shut down by PID file). Measures: which
   providers offer, what work they advertise, and whether the recheck at
   install ever flips a selection.

Complement (not a centralization probe): the hbox RX 6700 XT grinder lane —
ForgeMiner supports RDNA2; a measured Mh/s converts "the 49% miner" into
dollars (attack cost) and would be the first public AMD NOID benchmark.

## Method and limits

Read-only pinned checkout (52938433); live probes were single/few gentle GETs
or TCP connects (DNS ×12, :9600 ×8 + 2 holds, explorer ~20 API calls, luckypool
7, GitHub/Forgejo 5, SafeTrade 1 browser load, Kagi 3 searches, web-wallet 3).
No P2P handshake, no node run, no developer or community contact, nothing
written to the public network. Dated observations describe that window only;
attribution of any address to a person remains inference. Raw evidence: files in
this directory (`seed-dns-*.txt`, `blocks200.json`, `addr-*.json`,
`miner49*.json`, `rich2-addr.json`, `top1.json`, `luckypool-now.json`,
`gbpool-now.json`, `web-wallet.html`, `web-app.js`, `kagi-*.json`, …).
