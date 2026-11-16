# Operator power: every trust point the parano1d author controls by default

Lane `operator-power`, 2026-11-15. Code-reading of the pinned tree (`parano1d/`, read-only) + passive DNS/whois + one GET of the public pool homepage. No node connections, no developer contact. Labels: **[PROVEN-CODE]** (cited lines at the pin), **[VERIFIED-WEB]** (observed 2026-11-15), **[INFERRED]**. *(Report landed by the coordinator from the lane's final text; the lane could not write files.)*

## 1. The trust-point map

| # | Mechanism | Code cite (base `parano1d/`) | Who controls it | Label |
|---|---|---|---|---|
| 1 | Embedded DNS seeds `mseed1..4.parano1d.org`, bare hostnames, dialed `/dns4/<host>/tcp/9600` with **"No PeerID verification"** — the code's own comment | `noid_chain/src/consensus/network.rs:111-134`; consumed `noid_node/src/main.rs:2256-2304` | Whoever holds the `parano1d.org` zone | Zone **[VERIFIED-WEB]**: Namecheap, created 2026-07-26, WHOIS-private (Withheld for Privacy LLC), default registrar NS; operator **[INFERRED]** the author — the zone hosts his canonical Forgejo |
| 2 | Seed resolution: system DNS + libp2p fallback, ≤16 addrs/seed, 4 parallel bootstrap dials, 2 seed connections retained during initial sync | `main.rs:1434-1513`; `noid_p2p/src/network.rs:506-529` | Same as #1 | [PROVEN-CODE] |
| 3 | `--disable-dns-seeds` — the ONLY removal switch — is `hide=true`, documented as "for isolated multi-node protocol tests"; config seeds only ADD peers | `main.rs:1299-1302, 2259-2264` | exists but hidden (U32) | [PROVEN-CODE] |
| 4 | `dnsaddr:` format (TXT multiaddrs, cryptographic PeerID binding) exists in code but is UNUSED: `_dnsaddr.parano1d.org` has no TXT records | `main.rs:1390-1419`; live dig | — | [VERIFIED-WEB] — shipped seeds are exactly the unverified format |
| 5 | mDNS is LAN-scoped ("On the public internet mDNS never fires") | `network.rs:6857-6905` | LAN only | [PROVEN-CODE] |
| 6 | GUI spawns bundled sibling node binary, no seeds by default, never passes `--disable-dns-seeds`; binary = sibling of app bundle, else `NOID_GUI_NODE_BIN`, else PATH | `noid_gui/src/backend.rs:1450-1452, 1607-1618, 1911-1955` | whatever shipped beside the GUI + the embedded seed set | [PROVEN-CODE] |
| 7 | Canonical source+releases: `git.parano1d.org/ignotusnemo/parano1d/releases` (README's link) + GitHub `proof-native/parano1d` + GitLab mirror | `README.md:20-23`; `scripts/publish_release.sh:18-19, 63-64` | author account `ignotusnemo` on his own server | [PROVEN-CODE] + hosting [VERIFIED-WEB] |
| 8 | Release integrity = `SHA256SUMS` from the same release; **zero** gpg/codesign/minisign/cosign hits in `.github/` or `release_common.sh`; Windows "Until releases are Authenticode-signed…"; macOS path is a Gatekeeper bypass | `docs/getting-started/core.md:11-23`, `wallet.md:20-37, 56-66` | self-attestation; composes with C16 | [PROVEN-CODE] |
| 9 | No auto-update anywhere (GUI never downloads binaries) | `backend.rs:1943-1955` | manual installs from his channel | [PROVEN-CODE] |
| 10 | GUI media origin `https://tradedata.parano1d.org/` | `noid_gui/src/assets/media/transport.rs:13-17` | author zone; **availability-only power** — manifests must hash-match on-chain `metadata_hash` and images are content-verified (`transport.rs:84-104`, `assets/media.rs:390-411`) | [PROVEN-CODE] + [VERIFIED-WEB] (BL Networks IP) |
| 11 | Wallet master secret: first-run LOCAL generation (`rand::thread_rng()`, 0600, atomic hard-link); no shipped defaults; **no RPC export/escrow path** (0 grep hits in `noid_rpc/`); export/import only via hidden local CLI flags (U32) | `noid_node/src/wallet/keystore.rs:147-207`; `main.rs:1370-1376` | the user | [PROVEN-CODE] |
| 12 | Mining/operator tokens: user-supplied; operator token can spend from the wallet; mining token restricted to getBlockTemplate/submitBlock | `main.rs:1308-1336` | the installing operator | [PROVEN-CODE] |
| 13 | noid-wallet (Android) SPV checkpoint: 36 headers ending at block **274,821**, "fetched from our node on Nov 2, 2026", pins height + block id + work rate 745 GH/s; everything after validated on-phone (work floor ≥1/16 rate, checked-work 120 s, fork depth ≤18); compiled in via `include_str!` | wallet repo (external-consumers clone): `core/src/chain.rs:7-74, 54`, `core/src/headers.rs:19-44` | **the WALLET authors** (noidwallet/noidusa.com — third party, GoDaddy domain created 2026-10-20, Contabo VPS); moves only with an app update from their channel | [PROVEN-CODE] + [VERIFIED-WEB] |
| 14 | `parano1d-pool.fun`: live shared pool, proportional, 3% fee, self-reports **12 of last 60 chain blocks**, 2045 workers, 5.87 GH/s | external | registrant redacted (Register SPA, created 2026-08-22, RIPE-region IP); **no hosting overlap with the author's footprint** | page [VERIFIED-WEB]; author-operation [INFERRED: unsupported either way] |
| 15 | peakpool: SELF-HOSTED solo pool (non-custodial, coinbase direct) via Docker images `peakminer/parano1d-node:2.0.2` etc. | repo README (clone) | peakminer org — an **image-publisher supply-chain point**, not a hashrate hub | [PROVEN-CODE] |
| 16 | Deterministic genesis: hardcoded header (timestamp 2026-08-21, block id `860e7045…` bound into the protocol ID) | `noid_chain/src/consensus/genesis.rs:22-77, 169-178` | nobody — the one thing he cannot quietly change | [PROVEN-CODE] |

Sweeps: no hardcoded IP peers in non-test code; relays are peer-advertised, not author-configured. The four DNS names are the **entire** internet bootstrap.

## 2. What a seed operator gets

**[PROVEN-CODE]** A fresh node (no `peers.json`, mDNS LAN-only) dials the four hostnames, keeps up to two seed connections during initial sync, and runs Kademlia walks from the seeds' routing tables (`noid_p2p/src/behaviour.rs:100`). Seeds therefore determine **100% of a fresh node's first peers** and its first snapshot source: `validate_snapshot_install_selection` (`main.rs:282-321`) accepts the first manifest whose tip is taller and out-works the genesis-only HeaderDAG. That is exactly C26's precondition, behind which C26 proved commit→finalize→permanent estrangement→self-propagation.

## 3. Hosting concentration [VERIFIED-WEB]

| Host | A record(s) | Network |
|---|---|---|
| parano1d.org, git., docs., lab. | 168.100.11.250 | BL Networks 168.100.8.0/22 |
| tradedata. | 216.203.21.142 | BL Networks |
| mseed1. | 209.74.93.214, 66.29.128.164 | Namecheap hosting ×2 |
| mseed2. | 162.0.227.4, 203.161.32.161 | Namecheap; RIPE stub |
| mseed3. | 162.0.227.238, 69.67.172.157 | Namecheap; **BL Networks** |
| mseed4. | 168.100.9.26, 162.252.198.227 | **BL Networks ×2** (same /22 as canonical site) |

Canonical site, Forgejo, docs, tradedata and two seed slots all sit in BL Networks; every other seed IP is Namecheap budget hosting. Domain created 4 weeks before genesis. **[INFERRED]** one operator (the author) plausibly controls the effective seed set — no diverse orgs, no dnsaddr PeerIDs, no seed policy anywhere.

## 4. Composition verdict

**(a) Can the author, alone, TODAY, no code changes, poison a FRESH node's snapshot (C26)? YES — by proven mechanism.** Exactly three things stand between his seeds and that outcome, none cryptographic: (1) the genesis pin forces the poison chain to be a real-PoW fork of mainnet genesis — C26's far-future-timestamp trick floors the difficulty to `GENESIS_TARGET` instantly; (2) fork choice requires out-working the victim's DAG tip — for a fresh node that is genesis, so the *first* seed-served manifest wins; (3) honest peers must not arrive first with more cumulative work — but reachability at t=0 is precisely what the seed zone controls. Honest peers WOULD out-work a short attacker fork on work — they just never get the chance to be seen.

**(b) Existing nodes:** not by DNS alone (seed connections are released once ordinary peers exist; `peers.json` re-seeds Kademlia, `network.rs:3808-3826`). Three levers remain: majority hashrate (a clean reorg needs it; the one shared pool self-reports ~20% of blocks — unverified and internally inconsistent, so NOT shown to be in his hand); eclipse (C17+C22, partially reapplies to restarted/churned nodes); or a release update — the no-exploit path, since users verify with `SHA256SUMS` published by the same unsigned channel (and C16 showed the default builder authenticates metadata, not bytes). Updates are manual, i.e. social, not cryptographic.

**(c) Minimal user-side defense today:** hidden `--disable-dns-seeds` + explicit `--seed /ip4/H/tcp/P/p2p/PEERID` (format-3 multiaddr binds identity, `main.rs:1403-1410`); build from a pinned source commit; treat SHA256SUMS as availability only. **Independent checkpoint verification does not exist for nodes** — no assumevalid/assumeutxo pin anywhere in the tree; the ecosystem's only checkpoint is the phone wallet's, and it belongs to a third party. GUI users have no defense surface at all: even the undocumented `NOID_GUI_SEEDS` env only adds peers; the embedded seeds still dial.

## 5. Pools

`parano1d-pool.fun` is live and real-looking but its registrant is redacted with zero hosting overlap with the author — **not demonstrably author-operated, not demonstrably independent**. Two honesty caveats: its 5.87 GH/s is ~0.8% of the wallet-measured 745 GH/s network rate yet it claims ~20% of blocks; and its tip height (~56.5K) vs noid-wallet's Nov-2 checkpoint (274,821) are mutually incompatible chain histories at face value. All its numbers are self-reported. peakpool is self-hosted/solo — a Docker-image supply-chain point (peakminer publishes the node image), not hashrate concentration. A future majority pool composes with: sequencer-only ops (U34 ordering/censorship), C26 fork-choice abuse on fresh/eclipsed victims, C24 total GUI trust, and the release channel as fallback.

## 6. What the author does NOT control (verified in the pin)

User wallet secrets (local generation, no escrow, no RPC path — the only export is the hidden local CLI flag); the genesis pin (a silent chain-swap still needs visible real PoW from real genesis); the phone wallet's checkpoint (a different party's pin — though the wallet vendors his crates one commit behind the audit pin, so his code still runs on the phone); pool users' keys.

## 7. Evidence log

DNS (dig) for mseed1-4, git/tradedata/docs/lab, noid.network, pool.fun, noidusa.com, noidexplorer.org; TXT (empty) for `_dnsaddr.parano1d.org` and `_dnsaddr.noid.network`; whois for the four domains and all IPs (BL Networks 168.100.8.0/22 + 216.203.21.142 + 69.67.172.157 + 162.252.198.227; Namecheap 209.74.64.0/19 + 66.29.128.164 + 162.0.227.x; RIPE stubs 203.161.32.161, 169.58.218.36; Contabo 37.60.251.138); one passive GET of `https://parano1d-pool.fun/`. Nothing else fetched; no node connections; no developer contact. Wallet/pool code cites are to the external-consumers lane's clones under `experiments/external-consumers/repos/`.
