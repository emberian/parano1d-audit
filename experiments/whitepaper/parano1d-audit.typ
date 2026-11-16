// Source: pinned parano1d checkout 52938433a30b185ab007afaa15166a632cf2e82e.
// Detailed evidence and reproductions: experiments/FINDINGS.md and lane reports.
#set document(title: "Parano1d: a protocol and implementation study", author: "ember and the parano1d-break study")
#set page(paper: "a4", margin: (top: 2.1cm, bottom: 2.1cm, x: 2cm), numbering: "1")
#set text(font: "New Computer Modern", size: 10.2pt, lang: "en")
#set par(justify: true, leading: 0.62em)
#set heading(numbering: "1.1.")
#show heading.where(level: 1): it => { v(0.8em, weak: false); it; v(0.35em) }

#align(center)[
  #set par(justify: false)
  #text(size: 21pt, weight: "bold")[Parano1d: a protocol and implementation study]
  #v(0.35em)
  #text(size: 11pt)[Proof commitments, synchronization, wallet trust, and resource costs]
  #v(0.3em)
  #text(size: 9pt)[Pinned source `52938433a30b185ab007afaa15166a632cf2e82e` · 15 November 2026]
]

= Abstract

Parano1d is a live proof-of-work chain with zero-knowledge transaction authorization, recursive history proofs, a GUI wallet, and native assets. This study examines a pinned source revision and exercises its components on synthetic local networks. Its main results concern proof commitments, resource use, and history synchronization.

First, two production Merkle-node compressors admit sub-second collisions. The permutation's round constants and matrix coefficients lie in a 16-bit subfield, leaving a small invariant slice. We reproduced collisions in 72,274 and 111,525 evaluations, and found one digest shared by two tagged domains. These results contradict the stated collision resistance of those interfaces. The inspected transaction and state paths rehash leaves, fix tree geometry, and validate state independently; the collision witnesses did not yield an unauthorized spend or accepted block in the paths tested.

Second, local tests measured resource and trust boundaries. A peer held a node's outbound serving budget with stalled mempool pulls. A separate real-mempool test measured a 62-fold CPU asymmetry for rejecting authorization proofs. The GUI backend renders balances and confirmations supplied by its selected RPC node. Each result has a different access condition and measured scope.

Third, snapshot staging accepts a historical header with a timestamp that ordinary admission rejects. Direct production-function tests installed that header as finalized history using a stubbed recursive terminal. The release-node control tested a different configuration, with a future-dated tip, and rejected it. A genuine release terminal for a future-dated intermediate header remains to be tested. In the serving-stall run, the node's HeaderDAG reached honest height 150 while its installed State remained at zero; those are distinct measures of sync progress.

Native NOID spends use owner authorization, and the reviewed rules contain no address-specific freeze control. The protocol also has fixed development payouts and separate dependencies on peer discovery, software distribution, block production, and exchange credit. The study records what each dependency permits and which relationships remain unmeasured.

= Object and method

The target is the `noid_*` source at commit `52938433a30b185ab007afaa15166a632cf2e82e`. The checkout was treated as read-only. Experiments used copied source where necessary, synthetic keys and balances, and local nodes. No public-network transactions were submitted. `experiments/FINDINGS.md` is the detailed results register; each experiment directory contains its code, output, and scope limits. `experiments/TAKEOVER.md` and `experiments/review-prior/REPORT.md` record how interpretations changed after later controls.

We use three kinds of evidence. A *run* is an executed local test against production code, with output preserved. A *source argument* traces a production path and its guards. A *public observation* is dated and can change. When a test substitutes a verifier or models a network effect, the report states that boundary. Each result describes its tested configuration.

= Commitment collisions and transaction binding

== The invariant subfield and the collisions

The Poseidon2b permutation operates on four 128-bit field lanes. Its round constants and matrix multipliers, however, lie in the embedded field `GF(2^16)`. The four-lane subset `GF(2^16)^4` is therefore invariant under every round. The one-permutation Merkle-node compressor can be given aligned inputs that stay in this subset. On that subset its effective output is 32 bits, so a birthday search finds collisions at roughly `2^16` work.

The `TAG_CAPSNODE` search found a pair after 72,274 evaluations, taking 0.128 seconds on the audit machine. The verifier in `poc/src/bin/verify_collision.rs` checks the two distinct hardcoded inputs against the production compressor; it performs no search. The `IVCPCSN_` search found a second pair after 111,525 evaluations. Production raw Merkle-opening checks accepted the colliding node digests. A cross-tag search produced a digest valid under both node tags after 111,423 evaluations (`experiments/leaf-reachability/`). These executions refute collision resistance for those interfaces and the source comment that tag whitening prevents distinct trees from sharing collisions.

The observed work is an empirical result. The `2^16` expectation uses a random-mapping heuristic on the aligned slice; it is not a proven lower bound. Nor does a collision between supplied node digests produce a valid authorization proof. In the inspected capsule and BaseFold verifiers, leaves are recomputed from data, path geometry is fixed, and fold equations are checked. An aligned node collision inserted as a leaf failed. The exact UTXO state tree uses a different two-permutation hash path; its tested leaf hashes did not enter the invariant slice. `experiments/leaf-reachability/README.md` gives the source sweep, the 65,536-leaf control, and the limited cost argument for lifting an aligned collision through a valid leaf.

We inspected in-tree and identified external consumers. No inspected consumer exercised the cross-domain raw-verification seam. A third-party Jetsam fork inherits the raw API and has its own direct node-hasher use, so “no external consumer” would be false (`experiments/external-consumers/REPORT.md`). The justified result is narrower: a broken commitment interface, with no demonstrated route from it to value loss in the inspected systems.

== What the soundness certificate establishes

The executable security-bound checker reports a conditional `PASS`. Its multi-instance bound gives about 41.65 proved bits at an adversary budget of `2^32` and becomes vacuous near `2^41` (`experiments/typed-bind-verdict/`). Vacuity here means the bound no longer limits failure probability; it is not a measured attack at `2^41` queries.

The checker assumes a bound on the fixed permutation's residual failure, called `Delta_P2b`, but does not derive that bound for the shipped constants. Its typed-binding event also has two descriptions and the paper formalizes one node hash while production uses several tagged node domains. The executed cross-tag collision conflicts with one natural reading of that event; a narrower reading excludes the shipped multi-domain interface. Thus the certificate does not justify the advertised protection for the shipped commitment layer. This is a claim about the proof's coverage, not a claim that every ideal-model theorem is false or that the transaction verifier has been forged.

= Network availability

== Holding the serving budget

Each mempool-sync pull is 12 bytes. Before constructing a response, the node reserves the maximum 16 MiB response size from one process-wide 64 MiB outbound budget. Four concurrent pulls can reserve the entire budget. If the requester stops reading a large response, stream flow control stalls the write while the reservation remains held. The local devnet reproduced stalled serves with an approximately 11 MiB mempool response, and a joining node installed state later than the unpinned control (`experiments/devnet-chaos/results.txt`, phases 1 and 7).

The direct budget/codec test also showed an honest request waiting behind stalled streams (`experiments/p2p-resources/`). The phase-7 devnet recorded zero `Busy` responses. Source analysis predicts that class-slot exhaustion can turn delay into `Busy` responses for additional requests, but that escalation was not observed in this run. The 30-second request timeout bounds each individual hold; repeated pulls can renew it. This is an availability issue, including for accidentally slow peers. It does not confer chain-selection authority.

== Verification and gossip costs

A real mempool harness measured 8.34 ms of node CPU to reject a decodable but invalid authorization proof and 134 microseconds to construct a new invalid candidate from a reusable proof template: roughly a 62-fold CPU ratio (`experiments/tx-admission/`). Rejected candidates reserve no input slots and the same bytes are verified again on resubmission. A later real-wire control delivered 40 anonymous pushes to the release node's full `AuthProof` rejection path. A separate honest transaction was admitted during that two-second burst (`experiments/network-abuse/C22-LIVE.md`). Queue saturation and honest-transaction loss remain models, not observed outcomes.

Header serving is outside the same outbound byte budget. An earlier direct codec test retained about 75 MiB across 64 stalled header streams; the effect at the full connection cap is an extrapolation (`experiments/p2p-resources/`). A real-wire probe then parked 40 requests from five anonymous peers against a copied store extended to 4,096 *consensus-invalid* header rows. Node RSS rose from 331 MiB to 422–449 MiB in two bursts. Honest 4,096-header responses still succeeded: 86–101 ms at baseline, 157–203 ms when overlapped with preparation, and 84 ms after release (`experiments/network-abuse/`). The fixture tests serving pressure, not acceptance of an invalid chain or a sustained eclipse. Gossip's shared byte window can be consumed before payload decoding according to a source and rate analysis, but a live swarm effect remains untested (`experiments/tx-admission/`).

Transaction relay presents a different Sybil surface. Direct push selects four peers from all connected dispatchable identities, with no reserved outbound recipient. A source-only first-hop model says that early Sybils could also dominate the gossip mesh *if* an honest peer does not join it itself (`experiments/sybil-availability/`). That assumption failed in a real P2P loopback control: with eight Sybils connected first, an honest outbound peer sent GRAFT before the first heartbeat and received all 12 pre-heartbeat test payloads by gossip in 0–2 ms. The pinned GossipSub code admits an outbound GRAFT even when eight inbound peers fill the mesh. An established honest peer already meshed elsewhere may not GRAFT; that topology remains untested. These were transport payloads, not accepted transactions, and no relay outage was shown.

= Snapshot validation and discovery

A local differential supplied two forks from genesis. Ordinary header admission rejected the far-future timestamp at height 12. Snapshot staging accepted the same 24 headers because it uses a timeless historical validator; the later wall-clock check tests the tip only. Direct calls to the production staging and install functions then committed the future-dated intermediate header and advanced finality (`experiments/malicious-node/results.txt`). The default-schedule test demonstrates the validation difference at component level.

The proof boundary in that test was an always-accepting stub. The test did not generate a genuine HistoryStep terminal for the clock-shifted fork or submit it to a release node over the wire. Source inspection suggests that the recursive historical statement checks timeless rules; production proof generation and verification are the remaining test. The source also defines how a node handles a fork below its finalized checkpoint and how it re-serves a snapshot. Their combined behavior has not been measured in a live release-node experiment.

A competing fork must descend from the pinned genesis and exceed the node's *HeaderDAG-selected cumulative work*. A node holding much of the observed public chain has a substantial work barrier. A fresh node with no independently learned headers has a smaller one; a partially synchronized node lies between. A separate v2 difficulty test shows that a far-future parent can lower its child's target, while the 24-header snapshot test is in the legacy era. A current-height cost would need a combined measurement.

Fresh installations use four default DNS seed names under one domain, without a pinned peer identity (`experiments/operator-power/REPORT.md`). DNS answers select initial endpoints; this study has not identified the endpoint operators or tested all four P2P services. Explicit seeds and persisted peers offer other routes. The node waits five seconds for snapshot candidates, compares their advertised bridge work, and rechecks the chosen offer against the current HeaderDAG before installation (`experiments/snapshot-adversary/REPORT.md`). The first manifest therefore has no automatic authority. The remaining question is how a fresh node behaves when its reachable providers supply different histories or availability.

The earlier composition model used a joining node's installed `height=0` as a proxy for HeaderDAG work. The saved log shows HeaderDAG-selected honest targets up to height 150 during that interval (`experiments/devnet-chaos/header-progress-review.txt`). The serving stall delayed State installation while headers continued to arrive. A later release-node experiment supplied a higher-work fork whose *tip* was ten years in the future. The joining node rejected its genuine HistoryStep terminal for `BadTimestamp` and installed honest State to height 31 (`experiments/devnet-chaos/results.txt`, phase 8). A future-dated *intermediate* header under a present-day tip remains a separate release-node case.

= Issuance, operating controls, and exchange settlement

Native NOID spends require owner authorization; the reviewed rules contain no address-specific freeze operation. Through height 3,223,778, miners receive 90% of each block subsidy, while two fixed Fund and Lab addresses receive up to 5% each in daily payouts (`experiments/operator-privileges/REPORT.md`). The source defines the destinations, not their keyholders. Transaction inclusion is a producer choice within consensus validity and resource rules (`experiments/sequencing/`). Persistent omission of a transaction depends on block production and propagation across the network.

In a two-node local test, `walletSend` returned a transaction ID and the GUI's success path would say “TRANSACTION SENT” while both nodes had zero connected peers. The independent receiver had no mempool entry five seconds later. After connection, it acquired the same transaction (`experiments/operator-exit/REPORT.md`). The RPC result marks local admission. Peer delivery, inclusion, and exchange credit are later stages with their own observations and controls.

SafeTrade #link("https://support.safetrade.com/hc/en-us/articles/48550294413581-Parano1d-NOID-has-been-listed-on-SafeTrade")[lists NOID/USDT]. Trading an already credited balance and depositing new on-chain coins are separate operations. SafeTrade's NOID confirmation threshold, current deposit status, and deposit-node behavior during a chain disruption remain unmeasured (`experiments/operator-exit/OPERATOR-THREAT.md`).

= User-facing boundaries

== A node can attest to itself

The GUI normally starts a sibling full node and queries it at `127.0.0.1:9601`. That node derives receive addresses from its local wallet secret and checks receipt inclusion against its selected chain. The GUI delegates those checks; it does not independently verify headers or Merkle paths. A substituted test RPC service made the real backend return fixture-supplied “verified” and “confirmed” fields for meaningless proof bytes (`experiments/wallet-vs-node/`). The cited view code maps those fields to authentication labels; the iced GUI was not run. The tested trust boundary is the selected RPC endpoint. An ordinary P2P peer supplies data to the honest node's separate verification path.

The same trust boundary reaches the receive address. A test service prebound to the *default* loopback RPC port satisfied the GUI's startup ping and supplied a different address to the backend; the receive card and Copy Address use that field. A separate production wallet-builder check signed a valid synthetic payment to it (`experiments/wallet-boundary/README.md`). The test stopped before block inclusion. The condition requires a local process to bind before the honest daemon, or an explicitly selected alternate endpoint. A running honest node retains its port. The GUI's `NOID_RPC` environment override can name another service, but it sends no bearer token; an ordinary remote node refuses wallet RPC without credentials.

The standalone builder's default legacy path can embed substituted v1 HistoryStep matrix bytes under unchanged approved metadata (`experiments/build-artifacts/VERIFY.md`). The official v2.1 workflow retires those matrices; a transition publish authenticates its legacy pack, and v2/v2.1 paths check their payloads (`experiments/release-chain/REPORT.md`). In a full-node local control, the substituted custom binary stalled at height 144 while reference nodes reached 149. The GUI backend verified a genuine older receipt on both. This result concerns control of custom build inputs; the published v2.1 workflow uses a different profile. Release checksums are supplied through the same distribution channel, and the GUI has no automatic binary updater.

== Recovery documentation differs from the wallet

The GUI derives a photo-backed secret using BLAKE3 context `ParanO(1)d master secret from canonical image pixels v1`. The English, Russian, and Chinese recovery guides specify `Parano1d master secret from canonical image pixels v1`. The same synthetic canonical pixels produced two different real wallet addresses under those contexts (`experiments/wallet-boundary/README.md`). An independent tool implementing the published recipe cannot restore a wallet made by the shipped GUI. Importing the preserved original photo through the same GUI still works. The safe correction is to change the guides and publish a fixed test vector; changing the deployed derivation context would strand existing wallets without migration.

== Unsolicited native assets and the advertised marketplace

The production mint path accepts NFT outputs assigned to an address that did not consent or sign. In an isolated v2.1 local chain, a mined and committed block placed 248 such items with `unlock_height = u64::MAX` in the recipient's owner index. Verified wallet pagination returned all 248; the production input check rejected every spend as timelocked. Creation fees totaled 0.8002 NOID for the batch at the local fee level (`experiments/economic-state/REPORT.md`). A custom wallet can filter the items; the recipient cannot transfer, burn, or list them under the current consensus rule. Native NOID spends are unaffected. Four warm verified storage pages took 10.06 ms median for those 248 items; the timing excludes RPC and GUI work (`experiments/valid-spam/`). The GUI reloads owned items in 64-item pages, and its NFT gallery walks and sorts the whole collection before showing 12 cards. Creation is paid once while these read costs recur; the stock GUI's response at larger scales remains to be measured.

Several marketplace offer and escrow operations have source definitions but are disabled by the shipped operation and kind profiles. The live surface is narrower: base NFT operations and listing create, instant buy, and cancel (`experiments/nft-marketplace/`). Descriptions of the disabled offer system as a functioning market would misstate this release.

= Confirmation and finality

The protocol documentation says that at depth 18 the maximum reorganization is 17 blocks. The storage code instead finalizes `tip - 18` and accepts a valid, heavier suffix replacing up to 18 blocks. With a payment in block B and the tip at B+17, a wallet reports 18 confirmations, but the finalized checkpoint is B-1. A heavier valid fork from B-1 may replace B. At B+18, the checkpoint reaches B and ordinary reorganization cannot remove it (`experiments/chain-malice/REPORT.md`). Thus 19 tip-inclusive confirmations cross this node's local finality boundary. SafeTrade's NOID confirmation policy has not been measured.

Hard finality has a second conditional consequence: if two mining groups remain partitioned long enough to finalize incompatible blocks, cumulative work cannot make either group accept the other's finalized prefix. The source path rejects divergence below the checkpoint. The C17 local serving stall delayed State installation while honest headers continued to arrive, so that run covered a different condition. Long-lived header partitions remain an open network measurement.

= Costs and defenses

The strongest measurements concern local resources. Four stalled 12-byte pulls held the 64 MiB serving budget. A decodable invalid authorization cost the node 8.34 ms to reject against 134 microseconds of payload construction in a direct mempool test. A mining credential holder can consume prepared templates by submitting invalid work; the composed devnet destroyed templates while its reference miner kept pace (`experiments/external-miner/`, `experiments/devnet-chaos/`). The access conditions and resource costs differ across these paths.

Native creation has a different shape. Fees and per-block caps are enforced, while object carriage persists and the price does not rise with accumulated state. A self-producing miner can recover the entire 5,800 µNOID fee of a one-input, one-output collection creation through its coinbase while converting a plain UTXO into a larger native record (`experiments/pow-resource/`). Sustained creation requires a distinct funding input for each collection; splitting fresh inputs burns at least 2,500 µNOID per net-new slot at low occupancy, as well as block space and PoW opportunity. The source-derived split-funded rate is about 102 collections per block before packing and proof limits. `experiments/creation-spam/` models longer-run node and cold-sync growth; a full-block creation run and current-network production costs remain open measurements.

Several tested boundaries worked as designed. Native token issuance and redemption checks enforced reserve and conservation equations in the cases run (`experiments/token-accounting/`). The inspected NFT races consumed a single escrow UTXO consistently (`experiments/nft-races/`). Contract reorg tests restored state through undo logs (`experiments/contract-reorg/`). Authorization checks bound proofs to transaction bodies and owners, and the release path selected verifier schedules by height (`experiments/proof-soundness/`). These results cover the tested cases and versions; broader recursive-constraint equivalence remains a separate question.

= Open measurements

1. Generate a genuine release HistoryStep terminal for a fork containing a future-dated intermediate header. Present competing provider manifests to a fresh release node, recording selected HeaderDAG work, finality, and re-serve behavior. Repeat after restart with retained independent work.
2. Extend the two-node local-send control into a two-sender deposit experiment with an independent receiver. Vary connectivity and producer access, then restore a peer path and measure recovery.
3. Compare native block validation with the deployed recursive verifier on crafted v2.1 bodies, including activation and reorg boundaries. Compare the producer's native checks with release-verifier results on identical inputs.
4. Measure the stock GUI's response to larger unsolicited, permanently locked NFT holdings. The 248-item full-block and wallet-pagination control establishes carriage and spend rejection; interface slowdown remains unmeasured.
5. Confirm current seed service, producer concentration, and NOID deposit policy from independent sources. Record timestamps and distinguish payout addresses from producer or service operators.

= Reproduction index

The detailed register is `experiments/FINDINGS.md`. Each result above names its experiment directory, where the code, commands, saved output, and limits are recorded. `PROVENANCE.md` identifies the pinned checkout. The snapshot reports distinguish component staging under a stubbed terminal from release-verifier behavior.
