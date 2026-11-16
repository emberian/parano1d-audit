# Bootstrap and distribution trust points in a parano1d node

Target: pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`. This report traces the software and records passive DNS/WHOIS observations from 2026-11-15. It did not connect to the public P2P network. Code paths, dated observations, and operator attribution have different evidentiary weight; the table labels each.

## 1. The trust-point map

| Point | Observed behavior and source | Control or limit |
| --- | --- | --- |
| Default seeds | Four bare `mseed*.parano1d.org` names, `/dns4/.../tcp/9600`, no pinned PeerID (`noid_chain/src/consensus/network.rs:111-134`; `noid_node/src/main.rs:2256-2304`). | The DNS administrator chooses resolved endpoints; its identity was not established. |
| Dial policy | System DNS and libp2p fallback, at most 16 addresses/name, four parallel dials; two seed links retained during initial sync (`main.rs:1434-1513`; `noid_p2p/src/network.rs:506-529`). | Reachability and service were not tested. |
| Alternatives | `--disable-dns-seeds` is hidden; configured seeds add peers (`main.rs:1299-1302,2259-2264`). | Users can replace defaults only via the hidden switch. |
| Peer identity | `dnsaddr` with `/p2p/PEERID` is supported; `_dnsaddr.parano1d.org` published no TXT at observation time (`main.rs:1390-1419`; passive DNS). | Default seeds use the unpinned form. |
| LAN discovery | mDNS can discover peers on a LAN (`noid_p2p/src/network.rs:6857-6905`). | It does not provide general internet bootstrap. |
| Bundled GUI node | GUI chooses a sibling binary, `NOID_GUI_NODE_BIN`, or PATH; it does not pass the disable-seeds flag (`noid_gui/src/backend.rs:1450-1452,1607-1618,1911-1955`). | The binary and environment influence its node. |
| Source and releases | Project README points to the author's Forgejo, with GitHub/GitLab mirrors (`README.md:20-23`; `scripts/publish_release.sh:18-19,63-64`). | Publisher controls the offered artifacts. |
| Release checks | `SHA256SUMS` accompanies the release; surveyed release scripts have no external signature check; platform docs acknowledge unsigned installation (`docs/getting-started/core.md:11-23`, `wallet.md:20-37,56-66`). | Same-channel checksums identify byte differences against the published list but do not independently authenticate the publisher. |
| Updates | GUI does not automatically download binaries (`noid_gui/src/backend.rs:1943-1955`). | Installing a new version is a user action. |
| Media | GUI requests media from `tradedata.parano1d.org` (`noid_gui/src/assets/media/transport.rs:13-17`). | On-chain metadata hashes and content checks constrain substitution (`transport.rs:84-104`, `assets/media.rs:390-411`). |
| Wallet secret | First-run generation is local; keystore file mode 0600; no export RPC found in the surveyed code (`noid_node/src/wallet/keystore.rs:147-207`). | A replacement local binary has access to different capabilities. |
| RPC credentials | User-supplied operator token can reach wallet spending methods; mining token is restricted (`main.rs:1308-1336`). | No hardcoded publisher credential was identified. |
| Android checkpoint | Third-party wallet compiles in 36 headers through block 274,821 (`external-consumers` clone, `core/src/chain.rs:7-74`, `core/src/headers.rs:19-44`). | The third-party wallet publisher selects that checkpoint; the reference node does not use it. |
| `parano1d-pool.fun` | Site self-reported 12/60 blocks, 2,045 workers, 5.87 GH/s at observation time. | Numbers conflict with public mainnet height/rate; the site's operator was not identified. |
| peakpool | Repository advertises self-hosted solo-pool images. | The images are a software distribution point; the listing does not measure pooled hashrate. |
| Genesis | Header and network identifier are fixed in source (`noid_chain/src/consensus/genesis.rs:22-77,169-178`). | A chain substitution must still account for this pin. |

No hardcoded IP peers were found in the inspected non-test code. The four names are the default clean-install internet seeds. Users can add explicit seeds; a returning node can load persisted peers; mDNS can find LAN peers. The report does not establish that the four hosts served a working P2P endpoint when observed.

## 2. Effects of DNS control

A new default node dials the four names without pinning peer identities (`noid_chain/src/consensus/network.rs:111-134`). Their DNS administrator can choose the endpoints reached through those names. If those endpoints are responsive and no configured or LAN peer supplies an independent path, they can shape the node's initial view. The source does not identify the DNS administrator; attributing that role to the developer from the hosting arrangement remains an inference.

The node gathers snapshot candidates for five seconds and compares bridge work (`noid_node/src/main.rs:850-891,11928-11969,14471-14531`). At install it rechecks the selected HeaderDAG work (`main.rs:282-321,9684-9715`). A returning node has saved peers and durable work. The [independent snapshot review](../snapshot-adversary/REPORT.md) gives the exact boundaries. These checks refute the earlier hypothesis that the first seed-served manifest automatically wins.

## 3. Hosting observed on 2026-11-15

| Host | A record(s) | Network |
|---|---|---|
| parano1d.org, git., docs., lab. | 168.100.11.250 | BL Networks 168.100.8.0/22 |
| tradedata. | 216.203.21.142 | BL Networks |
| mseed1. | 209.74.93.214, 66.29.128.164 | Namecheap hosting ×2 |
| mseed2. | 162.0.227.4, 203.161.32.161 | Namecheap; RIPE stub |
| mseed3. | 162.0.227.238, 69.67.172.157 | Namecheap; **BL Networks** |
| mseed4. | 168.100.9.26, 162.252.198.227 | **BL Networks ×2** (same /22 as canonical site) |

The canonical site, Forgejo, docs, tradedata, and two seed slots shared BL Networks addresses at observation time. The other observed seed addresses were on Namecheap or the listed RIPE stub. This shows some hosting concentration, but IP and network membership do not identify each endpoint's operator.

## 4. Consequences and limits

**Fresh nodes.** Unpinned default seeds are a concentrated discovery trust point. DNS-selected endpoints may be the first peers a new node reaches. The degree of isolation depends on configured peers, peer discovery, header arrival, and the work-ranked candidate selection window. In the local C17 stall, installed state remained at height zero while the HeaderDAG selected the independent chain at height 150 ([saved check](../devnet-chaos/header-progress-review.txt)). The C26 component experiment used a stubbed recursive terminal. These experiments did not demonstrate initial-sync state substitution against a release verifier.

**Established nodes.** DNS changes alone do not erase retained chainwork or finalized history. The snapshot install gate compares proposed cumulative work to the HeaderDAG's selected work. Mining power, a sustained header-level partition, and installation of a modified binary are separate capabilities; this study did not measure or attribute them. Manual releases use checksums published with the artifacts, which detect differences against that list but do not independently authenticate its publisher. The standalone builder's legacy-matrix omission (C16) is a specific build-input gap; the current release path omits those matrices.

**Wallets and checkpoints.** The local keystore generates the user's secret; no hardcoded secret or export RPC was found in the inspected paths. The GUI's explicit owner-secret export invokes local node maintenance (`noid_gui/src/backend.rs:547-556`); a modified local executable would have access to the wallet file. The deterministic genesis header fixes network origin. A third-party Android wallet pins a later checkpoint; the reference node does not ship an equivalent later checkpoint. The GUI has no exposed switch to disable default DNS seeds, although users can replace its node or use other software. Its optional `NOID_GUI_SEEDS` setting adds seeds without removing the defaults.

**Pools.** `parano1d-pool.fun` self-reported 5.87 GH/s, 12 of 60 blocks, and a tip near 56.5K at observation time. Those figures do not match the ~313K chain reported by other public observers and a wallet checkpoint at 274,821. They may describe another chain or stale service; this report cannot attribute its miners or operator. `peakpool` appears to publish self-hosted solo-pool images, making its image publisher a software distribution point. These sources do not measure any party's share of mainnet mining work or capacity for sustained censorship.

**Practical configuration.** The node supports `--disable-dns-seeds` and identity-bound explicit `/p2p/PEERID` seed addresses (`main.rs:1299-1302,1390-1419`). The switch is hidden from `--help`, so users are unlikely to discover it unaided. An independently verified recent checkpoint would be a different defense against first-sync history substitution; none was found in the reference node.

## 5. Evidence log

DNS (dig) for mseed1-4, git/tradedata/docs/lab, noid.network, pool.fun, noidusa.com, noidexplorer.org; TXT (empty) for `_dnsaddr.parano1d.org` and `_dnsaddr.noid.network`; whois for the four domains and all IPs (BL Networks 168.100.8.0/22 + 216.203.21.142 + 69.67.172.157 + 162.252.198.227; Namecheap 209.74.64.0/19 + 66.29.128.164 + 162.0.227.x; RIPE stubs 203.161.32.161, 169.58.218.36; Contabo 37.60.251.138); one passive GET of `https://parano1d-pool.fun/`. Nothing else fetched; no node connections; no developer contact. Wallet/pool code cites are to the external-consumers lane's clones under `experiments/external-consumers/repos/`.
