# NOID network and market: a dated observation

This report records public data fetched 2026-11-16, 00:05–00:35 UTC. Sources
were explorer and pool APIs, aggregators, DNS, one public RPC endpoint, and
archived social posts. Raw responses are retained in this directory. The
figures describe that observation window; they are not current network facts.

Three public services reported NOID near height 313,400, with roughly
30-second blocks. In a 200-block sample, two coinbase addresses received
83.5% of blocks; one is plausibly a shared pool. This is production
concentration, not proof that one person controls both miners. Explorers
reported 3,355 funded addresses and 89,445 live UTXOs. SafeTrade was the only
NOID/USDT venue found by two market aggregators; its quoted price was near
$0.24, with roughly $70,000–$77,000 in reported daily volume. The roughly
$3 million capitalization is supply multiplied by a thin-market price, not
an amount that could necessarily be sold at that price.

The hashrate estimate near 0.9 trillion Poseidon2b evaluations per second is
specific to this PoW. It does not convert to rented SHA-256 hashrate or a
known GPU count. Neither block concentration nor shared infrastructure
establishes the developer's mining power. Four default DNS seed names share
one domain, but this survey did not connect to their P2P ports or identify
who operates their endpoints.

---

## 1. Chain liveness

| Metric | Value | Source (all fetched 2026-11-16T00:05–00:20Z) |
|---|---|---|
| Tip height | 313,405 → 313,422 within ~7 min | noidexplorer.org `/api/v1/stats` (00:07 fetch), gb-pool `/api/pool` `network_height` 313,410 (00:08), luckypool `/api/stats` `network.height` 313,422 (00:14) — three independent node operators |
| Block rate | 24 h avg 30.10 s, 1 h avg 30.9 s, 10 min avg 24.6 s (target 30 s) | noidexplorer `/api/v1/stats` |
| Genesis | 2026-08-21T16:00:00Z (block 0 ts 1787328000; block 1 +1 s) | noidexplorer `/api/v1/block/height/0` |
| Chain age | ~87 days; 313,422 blocks → lifetime avg ≈ 24 s/block (early era faster than target) | derived |
| Difficulty | `difficulty_bits` 44; target `0051341acd58a678…`; scalar 27.37e13 | noidexplorer `/api/v1/stats`; gb-pool `network_difficulty` 27,370,046,323,310 |
| Network rate | explorer est. 724 GH/s instantaneous / **914 GH/s 4 h avg**; gb-pool 912.3 GH/s; luckypool implies ≈911 GH/s (350.8/0.385); difficulty-derived 27.37e13/30 ≈ **912 GH/s** | same three sources |
| Orphans | 4,079 all-time (1.30%) | noidexplorer `/api/v1/stats` |
| Consensus | protocol v2 active since block 210,537; native (token/DEX) since 279,370; next subsidy step at 1,261,737 | noidexplorer `/api/v1/economics` |

**Hashrate estimate:** H ≈ D/t = 2.737e13 / 30 s ≈ **9.1e11
Poseidon2b evaluations/s**. This uses observed difficulty and the target block
time; it does not measure mining hardware. The third-party wallet's Nov 2
checkpoint reports a compatible earlier estimate of 745 GH/s.

**Height discrepancy:** noidexplorer, gb-pool and luckypool all reported
~313.4K in this window, and
the wallet checkpoint (274,821 @ Nov 2) extrapolates at 30 s/block to
≈313–315K at the time of this survey.
Those sources agree on a chain near 313K. parano1d-pool.fun's reported
~56.5K tip is 257K blocks away. Its own numbers are also internally
consistent with a different network: 5.87 GH/s claiming 12/60 blocks implies a network
of ≈29 GH/s, 31 times below the mainnet estimate. The service may be on
another chain or may simply report stale or incorrect data. It returned HTTP
503 at 00:30 UTC, so its actual chain was not established.

## 2. Who secures it (block production)

- **Recent window (last 200 blocks, ~100 min, noidexplorer `/api/v1/blocks`):**
  `o189lhaayzfnsk…` **49.0%** (43,324 blocks all-time, first at 226,979 — a solo miner
  that scaled up hard), `o1sk6hjgwlwpm…` **34.5%**, `o1sks93zzutc…` 7.0%, six more
  0.5–2.5%. Top two = **83.5%**.
- **`o1sk6hjgw…` = luckypool.io** (inference, high confidence): all-time 11,632 blocks
  vs LuckyPool `blocksFound` 12,354; LuckyPool is **47 miners / 600 workers / 350.8
  GH/s / 38.5% of network rate / 163,370 NOID paid out / 12,354 blocks** (its `/api/stats`,
  00:14Z). Stratum endpoints in eu/us-east/sg/pl/tr/ru, 1% PROP.
- **gb-pool (noid.pool.army, "GB Parano1d Pool" by gustlborg):** alive but vestigial —
  **1 miner**, 7.3 MH/s (≈0.0008% of network), **7 blocks ever, 0 in 24 h**, 99.7 NOID
  paid total, restarted Nov 14 (`online_since` 1794549713) (`/api/pool`, 00:08Z).
- **AriaPool (pool.ariabrain.com/noid.html): "PAUSED since 18 October 2026"**,
  balances paid out (page text).
- **parano1d-pool.fun:** HTTP 503 (00:30Z); per §1 its self-reports describe a
  incompatible height; its contribution to this chain was unverified.
- **peakpool:** software, not a service — `peakminer/peakpool` GitHub Stratum-V2
  **solo**-mining deployment; `peakpool.xyz` is a **parked domain** ("lander" redirect).
- **All-time (noidexplorer `/api/v1/miners`, 313,411 blocks):** 122 coinbase addresses;
  top-9 = 87.4%; no address > 16.2%. Current concentration is much higher than the
  all-time picture.

The 200-block window was concentrated: one coinbase address received 49%,
another 34.5%. The second plausibly belongs to a pool with many miners. We
cannot infer developer control or the cost of obtaining majority hashrate
from these payout addresses and pool estimates alone.

## 3. Who uses it

| Metric | Value | Source |
|---|---|---|
| Funded addresses | **3,355** | noidexplorer `/api/v1/stats` |
| Live UTXOs | 89,445 | same |
| Tx count 24 h | 3,920 total; non-coinbase ≈ 1,040 (explorer) / ≈ 1,340 (my 200-block sample: 0.465 non-coinbase tx/block) | noidexplorer stats + `/api/v1/blocks` |
| Mempool at sample time | 1 tx | `/api/v1/mempool` |
| Circulating supply | 12,153,112.7 NOID (emitted 12,153,513.6 − burned 400.9) | `/api/v1/stats`, `/api/v1/economics` |
| Holder concentration | top-1 26.8% (3.25M NOID, 58,341 UTXOs, 49,563 txs — constant small deposits in from many senders, 37K-NOID withdrawals out: **exchange-hot-wallet behavior**, likely SafeTrade, attribution unconfirmed); top-10 49.4%; top-20 58.9%; top-100 84.7% | `/api/v1/richlist` (100 entries), `/api/v1/address/{top1}` |
| Native assets | 24 tokens, 67 NFT collections, 4,440 NFTs, 14 graduated tokens, 2,320 batches, 2,513 fills (35 fills/24 h) | `/api/v1/native/stats` |
| On-chain value locked | **48,962 NOID ≈ $12 K** (pools 36,445 + token backing 9,660 + curves 2,769 + NFT listings 86) | `/api/v1/stats` `native_locked` |
| DEX volume | all-time 564,475 NOID; 24 h **11,817 NOID ≈ $2.9 K** | `/api/v1/native/stats` |

Real community artifacts: ONEFEST festival-pass collection (252 items,
noone.parano1d.org), "Noidz" NFT series with burn mechanics (noidz.xyz), meme tokens
("$58k mc" per twitter, Nov 4), a paid explorer service (USD-priced receipts $1.00,
per-address wallet history fees). The explorer self-describes "Independent community
project" (@noidexplorer on X); the Android wallet is noidwallet/noid-wallet
(noidusa.com, 3rd party); mining guides by bytwork.com; trackers hashrate.no and
MiningChamp. Active shill/holder community on X ($noid, @parano1dscan, @noidium,
@No1d_Wallet; arch #gpumining). Twitter-archive coverage extends only to 2026-11-09
(scry), so last-week chatter is not measured.

## 4. Value (markets)

- **Venue: SafeTrade only.** Coinpaprika `noid-parano1d` markets = exactly one entry:
  SafeTrade NOID/USDT, 100% of volume (fetched 00:12Z). CoinGecko API: one ticker,
  $74,984/24 h (00:18Z). SafeTrade's own page: NOID/USDT 0.2478 −8.99% (Kagi snippet).
  bytwork guide: "Trading and deposits of NOID are confirmed only on the SafeTrade
  exchange, NOID/USDT pair." No DEX/OTC/XT/telegram market found. SafeTrade direct API
  is Cloudflare-403 to curl; aggregator data is concordant across paprika/CG/explorer.
- **Price:** $0.2412 (paprika + explorer ticker, 00:10Z) / $0.2472 (CoinGecko) /
  $0.244 (hashrate.no widget). These are dated quotes.
- **Market cap:** CoinGecko **$2,998,315** (circ 12.13M) / FDV $3,003,880. Supply has
  no hard cap (permanent 1 NOID/block tail), so FDV grows with the tail.
- **Volume:** $69.6 K (SafeTrade page) / $75.0 K (CG) / $77.2 K (paprika) per 24 h.
- **History:** listed since ~mid-Sept 2026 at ~$0.0025 (explorer premium/price/history);
  ran to ATH $0.6374 on 2026-10-20 (paprika) / $0.4995 on 2026-10-25 (CG); at observation −37%
  (30 d), −50% from ATH. A ~$3 M cap coin doing ~$75 K/day volume on one small CEX
  with 26.8% of supply sitting in one address whose exchange attribution is
  unverified. Reported volume is not order-book depth or sale capacity; market
  capitalization cannot bound actual holder losses.
- Dev-fund value: cumulative **599,040 NOID ≈ $148 K to each of two funds** (O(1)
  Network Fund + ParanO(1)d Lab, consensus-mandated 5% each of subsidy for 3 years;
  `/api/v1/economics` + `development_allocation.rs`); both funds spend down regularly
  (78/68 total txs each, ~1/day in, periodic sweeps out; not in top-100 richlist).

## 5. The fork — Jetsam (JTM)

Jetsam (hard-fork of parano1d v1.0.0, `jetsam-chain` GitHub org, created 2026-09-04)
runs its own mainnet: **live and producing** — height **54,273** via public RPC
`rpc.jetsamchain.com` (`jetsam_getChainInfo`, 00:20Z; the node reports 41 connected
peers, p2p healthy); genesis block 1 ts 1788454101 = **2026-09-03**; recent 150-block
window avg **189.4 s** vs 180 s target; difficulty 27 bits, TowerWalk PoW
(`pow_walk: true`, hashrate in H/s explicitly null — memory-walk PoW has no honest
H/s conversion); circulating 2,074,122 JTM of a 21 M hard cap, 25 JTM/block.
Concentration: recent 150 blocks top miner **42.0%**, #2 20.7% (my header sample);
all-time sample (121 headers) top 35.5%. Pools: none — solo CPU/GPU mining
(jetsamchain.com: "mined by ordinary CPUs"; org ships solo miners; bitcointalk ANN
5593269 "Pool miners" mention refers to pool *operators running the pool's node*).
**No observable market:** LiveCoinWatch tracks JTM with "No trading data", empty
markets table, price/mcap/volume all "-"; absent from Coinpaprika and CoinGecko.
Community: bitcointalk ANN read 558 times, OP "jetsamofficiel" (Newbie, activity 7);
Discord/Telegram exist (unmeasured); repo last push 2026-10-30 (2 weeks stale).
No JTM market was found in the sources checked; that is not proof that private
trades or unindexed venues do not exist.

## 6. Developer footprint (author = @ignotus_nemo / "Paranoid Zero")

Author-controlled touchpoints:
- **Canonical source & releases:** git.parano1d.org Forgejo, 9 repos (parano1d,
  docs, site, soundness, o1-lab, agcore, parano1dx, frost-gkr), last pushes
  2026-09-07 → **2026-11-15 (site, yesterday)** (Forgejo `/api/v1/repos/search`).
- **Website/docs/research:** parano1d.org, docs.parano1d.org, lab.parano1d.org — all
  author-run (README), all live (HTTP 200).
- **DNS seeds:** all four (`mseed1..4.parano1d.org`, port 9600, extracted from
  `noid_chain/src/consensus/network.rs:129-134`) live in the author's DNS zone. Caveat:
  each resolves to **two IPs whose PTR is `nc-ph-*.web-hosting.com` — Namecheap shared
  web hosting** (6/8 IPs have such PTR; dig, 2026-11-16). Shared hosting cannot
  obviously run a libp2p seed on :9600; whether the seeds actually bootstrap peers is
  UNTESTED (p2p contact out of scope). If non-functional, day-to-day discovery relies
  on persisted peers/community seed lists — i.e., the *documented* bootstrap is
  author-domain DNS that may be parking-grade.
- **Media origin:** NFT media fetched from `https://tradedata.parano1d.org/`
  (explorer `/api/v1/native/stats` `media.origins`) — author infrastructure inside
  third-party apps' data path.
- **Money:** 10% of every block subsidy (3 years) to two hardcoded author-side funds
  (§4). Not a premine (funds accrue via consensus, miners get 90%).
- Promotion: ethresear.ch t/25730, Zenodo 23778742, active X account announcing the
  web wallet/launchpad/DEX/NFT suite (Nov 4 launch post).

Apparently separate services (ownership was not independently audited): noidexplorer.org (community, sells its own
USD-priced services), SafeTrade listing, luckypool.io, gb-pool (gustlborg), AriaPool
(paused), noid-wallet/noidusa.com (@Solid_Primal/@0xdavee_), noidz.xyz, ONEFEST,
@parano1dscan/@noidium, bytwork/hashrate.no/MiningChamp coverage, the mining community
on X, and — per the audit's external-consumers lane — the entire Jetsam fork.
The source and release channel, public site and seed DNS names, and two
consensus-designated development payments are associated with the author.
The explorer, exchange, mining pools, Android wallet, and several applications
are separately operated or attributed to other parties. This does not settle
who controls the live seed endpoints or major miners.

## Limits for risk assessment

NOID had a live chain, third-party software, and a quoted SafeTrade market in
this observation window. This makes user-facing failures consequential. The
$3 million quote is a market-cap calculation, not liquid value or a cap on
losses. We did not measure exchange order-book depth, the number of distinct
people behind funded addresses, or the developer's mining power.

The seed endpoints' actual P2P service, the largest address's exchange
attribution, SafeTrade's deposit rules, recent social activity after the
archive cutoff, and the chain served by parano1d-pool.fun remain unknown.
