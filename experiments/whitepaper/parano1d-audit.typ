// Source: pinned parano1d checkout 52938433a30b185ab007afaa15166a632cf2e82e.
// Detailed evidence and reproductions: experiments/FINDINGS.md and lane reports.
#set document(title: "Parano1d: an adversarial audit", author: "ember and the parano1d-break audit")
#set page(paper: "a4", margin: (top: 2.2cm, bottom: 2.2cm, x: 2.1cm), numbering: "1")
#set text(font: "New Computer Modern", size: 10.5pt, lang: "en")
#set par(justify: true, leading: 0.64em)
#set heading(numbering: "1.1.")
#show heading.where(level: 1): it => { v(0.8em, weak: false); it; v(0.35em) }

#align(center)[
  #text(size: 21pt, weight: "bold")[Parano1d: an adversarial audit]
  #v(0.35em)
  #text(size: 11pt)[Cryptographic claims, node trust, and the cost of abuse]
  #v(0.3em)
  #text(size: 9pt)[Pinned source `52938433a30b185ab007afaa15166a632cf2e82e` · 15 November 2026]
]

= Abstract

Parano1d is a live proof-of-work chain with zero-knowledge transaction authorization, recursive history proofs, a GUI wallet, and native assets. This audit examined a pinned source revision and exercised its components on synthetic local networks. Its main results fall in three places.

First, two production Merkle-node compressors admit sub-second collisions. The permutation's round constants and matrix coefficients lie in a 16-bit subfield, leaving a small invariant slice. We reproduced collisions in 72,274 and 111,525 evaluations, and found one digest shared by two tagged domains. These are real failures of the advertised commitment properties. We found no way to turn them into an unauthorized spend or a forged block: the inspected value-bearing paths rehash leaves, fix tree geometry, and validate state independently. The gap between a broken primitive and an exploitable transaction is central to the result.

Second, local tests exposed resource and trust failures. An anonymous peer held a node's outbound serving budget with stalled mempool pulls. A separate real-mempool test measured a 62-fold CPU asymmetry for rejecting authorization proofs. A malicious node can make the real GUI backend report fabricated balances and confirmations as verified model fields. These observations concern different attacker powers and have different limits; the report does not combine them into an untested network takeover.

Third, snapshot staging accepts a historical header with a timestamp that ordinary admission rejects. Direct production-function tests installed that header as finalized history using a stubbed recursive terminal. A genuine release terminal and a live victim accepting and re-serving the fork have not yet been demonstrated. The earlier claim that a serving stall leaves a fresh node with no honest chainwork was refuted by the node's own log: its HeaderDAG reached honest height 150 while its installed state remained at zero.

We found no developer-only privilege to freeze another holder's native NOID or to sell while that holder cannot. The operator's possible control over default discovery, software distribution, and block inclusion deserves scrutiny, but control of one does not establish control of the others or of an exchange account. The audit identifies specific tests that would settle those questions.

= Object and method

The target is the `noid_*` source at commit `52938433a30b185ab007afaa15166a632cf2e82e`. The checkout was treated as read-only. Experiments used copied source where necessary, synthetic keys and balances, and local nodes. We did not send transactions to the public network. `experiments/FINDINGS.md` is the detailed finding register; each experiment directory contains its code, output, and scope limits. The more recent evidence reviews in `experiments/TAKEOVER.md` and `experiments/review-prior/REPORT.md` correct claims made in earlier drafts.

We use three kinds of evidence. A *run* is an executed local test against production code, with output preserved. A *source argument* traces a production path and its guards, but does not claim a live outcome. A *public observation* is dated and can change. Where a demonstration substitutes a verifier or models a network effect, we state the substitution. A successful local test proves its stated case; it does not prove the absence of other attacks.

= A weak commitment layer with a stronger semantic boundary

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

A real mempool harness measured 8.34 ms of node CPU to reject a decodable but invalid authorization proof and 134 microseconds to construct a new invalid candidate from a reusable proof template: roughly a 62-fold CPU ratio (`experiments/tx-admission/`). Rejected candidates reserve no input slots and the same bytes are verified again on resubmission. The reported queue-saturation and honest-transaction-drop rates are models built from measured per-item costs and source limits; a live multi-peer saturation test has not established those outcomes. A separate devnet push storm reached an earlier decode rejection, not the full proof cost.

Header serving is outside the same outbound byte budget. An earlier direct codec test retained about 75 MiB across 64 stalled header streams; the effect at the full connection cap is an extrapolation (`experiments/p2p-resources/`). Gossip's shared byte window can be consumed before payload decoding according to a source and rate analysis, but a live swarm effect remains untested (`experiments/tx-admission/`). These paths are being tested separately because they have different request sizes, concurrency guards, and implications for honest header delivery.

= Snapshot validation and discovery

A local differential supplied two forks from genesis. Ordinary header admission rejected the far-future timestamp at height 12. Snapshot staging accepted the same 24 headers because it uses a timeless historical validator; the later wall-clock check tests the tip only. Direct calls to the production staging and install functions then committed the future-dated intermediate header and advanced finality (`experiments/malicious-node/results.txt`). The default-schedule test demonstrates the validation difference at component level.

The proof boundary in that test was an always-accepting stub. It did not generate a genuine HistoryStep terminal for the poisoned fork or make a release node accept it over the wire. Source inspection suggests that the recursive historical statement checks timeless rules, but proof generation and verification remain the decisive missing test. Likewise, the code path for rejecting an incompatible finalized fork and re-serving a snapshot is visible in source; permanent estrangement and secondary propagation have not been demonstrated together in a live release-node experiment.

The attacker must supply a real fork from the pinned genesis and exceed the node's *HeaderDAG-selected cumulative work*. A node holding much of the observed public chain has a substantial work barrier. A fresh node with no honest headers has a small one; a partially synchronized node lies between. A separate v2 difficulty test shows that a far-future parent can lower its child's target, but the 24-header snapshot test is in the legacy era. Their results cannot be multiplied into a measured present-day attack price.

Fresh installations use four default DNS seed names under one domain, without a pinned peer identity (`experiments/operator-power/REPORT.md`). Control of their DNS and responsive endpoints could bias initial discovery. The domain's operator is inferred, not established by registration data, and the public audit did not test whether all four seeds answer the protocol. Explicit seeds and persisted peers offer other routes. The node waits five seconds for snapshot candidates, compares their advertised bridge work, and rechecks the chosen offer against the current HeaderDAG before installation (`experiments/snapshot-adversary/REPORT.md`). The first manifest therefore has no automatic authority. A malicious provider set could still matter if it excludes honest work through installation; that condition needs a controlled experiment.

The earlier composition claim used a joining node's installed `height=0` as proof that its HeaderDAG knew no honest work. The saved log shows HeaderDAG-selected honest targets up to height 150 during that interval (`experiments/devnet-chaos/header-progress-review.txt`). The serving stall delayed state installation; it did not create the claimed genesis-only work window. No cheap anonymous C17-to-snapshot-poisoning chain has been shown. The phase-8 live experiment remains separate until its terminal, selection, and retained-work controls can be checked.

= Operator power and a holder's ability to exit

No privileged native-NOID address freeze or developer-only sell path was found in this review. A miner chooses which valid transactions to include, while consensus enforces authorization, inputs, and fees; the earlier ordering tests found no mandatory-inclusion rule (`experiments/sequencing/`). This is ordinary producer discretion. Sustained censorship would require enough continuing block-production or propagation control, which has not been attributed to the developer.

There are more conditional ways to make a holder believe a deposit is proceeding. In a two-node local test, `walletSend` returned a transaction ID and the GUI's success path would say “TRANSACTION SENT” while both nodes had zero connected peers. The independent receiver had no mempool entry five seconds later. After the nodes connected, the receiver acquired the same transaction (`experiments/operator-exit/REPORT.md`). The response thus certifies local admission, not delivery. A holder using an isolated node could see this message while someone with a healthy path could submit normally. The run does not attribute that isolation to the developer or show permanent censorship. A transaction visible only in a wallet's local node is not a credit at an independent exchange.

SafeTrade #link("https://support.safetrade.com/hc/en-us/articles/48550294413581-Parano1d-NOID-has-been-listed-on-SafeTrade")[lists NOID/USDT], so deposits and internal exchange balances matter to this analysis. A chain-side outage can prevent new deposits without necessarily preventing someone who deposited earlier from placing an internal order. SafeTrade's NOID confirmation threshold, deposit-node behavior under a partition, and any relationship with the developer remain unknown. The record supports a pointed question about selective exit; it does not support accusing anyone of exercising it.

= User-facing boundaries

== A node can attest to itself

The real GUI backend accepts receipt, balance, inclusion, and send results from its configured RPC node. An impostor node made the backend return attacker-authored “verified” and “confirmed” model fields for meaningless proof bytes (`experiments/wallet-vs-node/`). The cited view code maps those fields to authentication labels; the iced GUI was not executed in that test. Local amount parsing and preview checks still run, but the client does not independently verify the node's inclusion proofs. The demonstrated harm is misleading a user who treats that node's answer as an independent payment check. It is not a demonstrated accepted exchange deposit or a remote spend from an honest node.

The same trust boundary reaches the receive address. A keyless impostor answering the local wallet RPC replaced the address returned by the real GUI backend. The receive card and Copy Address use that field. A separate production wallet-builder check signed a valid synthetic payment to the substituted, attacker-controlled address (`experiments/wallet-boundary/README.md`). No live transfer or independent recipient acceptance was run. This requires control of the GUI's local node interface; an ordinary remote P2P peer has not been shown to acquire that control.

The standalone release builder's default legacy path can embed substituted v1 HistoryStep matrix bytes under unchanged approved metadata (`experiments/build-artifacts/VERIFY.md`). A local devnet test showed a tampered node presenting fabricated history through the real wallet backend. The scripted official publish path has compensating controls, and v2/v2.1 matrix paths authenticate their payloads. The finding is a specific builder trust-boundary flaw, not a claim that every released matrix is unchecked. The releases and checksum file share a distribution channel; a checksum from the same channel does not authenticate the publisher or a compromised channel. The GUI has no automatic binary updater.

== Recovery documentation differs from the wallet

The GUI derives a photo-backed secret using BLAKE3 context `ParanO(1)d master secret from canonical image pixels v1`. The English, Russian, and Chinese recovery guides specify `Parano1d master secret from canonical image pixels v1`. The same synthetic canonical pixels produced two different real wallet addresses under those contexts (`experiments/wallet-boundary/README.md`). An independent tool implementing the published recipe cannot restore a wallet made by the shipped GUI. Importing the preserved original photo through the same GUI still works. The safe correction is to change the guides and publish a fixed test vector; changing the deployed derivation context would strand existing wallets without migration.

== Unsolicited native assets and the advertised marketplace

The production mint path accepts NFT outputs assigned to an address that did not consent or sign. In an isolated v2.1 local chain, a mined and committed block placed 248 such items with `unlock_height = u64::MAX` in the victim's owner index. Verified wallet pagination returned all 248; the production input check rejected every spend as timelocked. The attacker paid 0.8002 NOID for the batch at the local fee level (`experiments/economic-state/REPORT.md`). The victim can filter them in a custom wallet, but cannot transfer, burn, or list them under the current consensus rule. This does not lock the victim's native NOID. The scale at which such items make the stock GUI slow has not been measured.

Several marketplace offer and escrow operations have source definitions but are disabled by the shipped operation and kind profiles. The live surface is narrower: base NFT operations and listing create, instant buy, and cancel (`experiments/nft-marketplace/`). Descriptions of the disabled offer system as a functioning market would misstate this release.

= Confirmation and finality

The protocol documentation says that at depth 18 the maximum reorganization is 17 blocks. The storage code instead finalizes `tip - 18` and accepts a valid, heavier suffix replacing up to 18 blocks. With a payment in block B and the tip at B+17, a wallet reports 18 confirmations, but the finalized checkpoint is B-1. A heavier valid fork from B-1 may replace B. At B+18, the checkpoint reaches B and ordinary reorganization cannot remove it (`experiments/chain-malice/REPORT.md`). Thus 19 tip-inclusive confirmations, rather than 18, cross this node's local finality boundary. This is a source-backed documentation discrepancy, not an observed double-spend or a claim about SafeTrade's threshold.

Hard finality has a second conditional consequence: if two mining groups remain partitioned long enough to finalize incompatible blocks, cumulative work cannot make either group accept the other's finalized prefix. The source path rejects divergence below the checkpoint. The audit has not found a cheap way to keep both mining groups separated at the header layer for that long. The C17 serving stall does not meet that condition, since honest headers continued to arrive in its local run.

= Costs and defenses

The strongest measurements concern local resources. Four stalled 12-byte pulls held the 64 MiB serving budget. A decodable invalid authorization cost the node 8.34 ms to reject against 134 microseconds of attacker construction in a direct mempool test. A node owner with a mining credential can consume prepared templates by submitting invalid work; the composed devnet destroyed templates while its honest miner kept pace (`experiments/external-miner/`, `experiments/devnet-chaos/`). These costs apply to different actors and cannot be summed into one zero-cost attack.

Native creation has a different shape. Fees and per-block caps are enforced, but object carriage persists and the price does not rise with accumulated state. `experiments/creation-spam/` estimates growth under sustained maximum creation and prices the required fees. Its one-year saturation figure is a modeled upper scenario, not a measured public-network trajectory. The attacker pays for valid transactions; the resulting long-lived node and cold-sync cost falls on every full node. Whether this is economically cheap depends on the market value of NOID and the attacker’s objective.

The tests also found substantial working boundaries. Native token issuance and redemption checks enforced reserve and conservation equations in the cases run (`experiments/token-accounting/`). The inspected NFT races consumed a single escrow UTXO consistently (`experiments/nft-races/`). Contract reorg tests restored state through undo logs (`experiments/contract-reorg/`). Authorization checks bound proofs to transaction bodies and owners, and the release path selected verifier schedules by height (`experiments/proof-soundness/`). These are bounded negative results, not a proof that every recursive constraint or future version is sound. We did not demonstrate an unauthorized spend, accepted invalid block, owner-key recovery, or arbitrary holder freeze.

= Work that would change the conclusions

1. Generate a genuine release HistoryStep terminal for a fork containing a future-dated intermediate header. Present competing honest and malicious manifests to a fresh release node, recording selected HeaderDAG work, finality, and re-serve behavior. Repeat after restart with retained honest work.
2. Extend the two-node local-send control into a two-holder deposit experiment with an independent receiver. Show which connectivity or producer powers keep one holder's transaction local while the other's lands, then restore an honest path and measure recovery.
3. Compare native block validation with the deployed recursive verifier on adversarial v2.1 bodies, including activation and reorg boundaries. Producer-side native checks alone do not prove verifier soundness for a malicious producer.
4. Measure the stock GUI's response to larger unsolicited, permanently locked NFT holdings. The 248-item full-block and wallet-pagination control establishes carriage and spend rejection; interface slowdown remains unmeasured.
5. Confirm current seed service, producer concentration, and NOID deposit policy from independent sources before making public-network or exchange-specific claims. Public price and hashrate estimates are dated observations, not control attribution.

= Reproduction index

The detailed register is `experiments/FINDINGS.md`. Source and output for the principal claims are in `poc/` (compressor witness), `experiments/leaf-reachability/` (cross-tag search and leaf boundary), `experiments/typed-bind-verdict/` (certificate interpretation), `experiments/p2p-resources/` and `experiments/devnet-chaos/` (serving), `experiments/tx-admission/` (verification), `experiments/malicious-node/` and `experiments/snapshot-adversary/` (snapshot), `experiments/operator-power/` (discovery), `experiments/wallet-vs-node/` and `experiments/wallet-boundary/` (wallet), `experiments/build-artifacts/` (matrix), `experiments/chain-malice/` (finality), and `experiments/token-accounting/` (conservation). The pinned checkout and experiment provenance are described in `PROVENANCE.md`. Some tests require generated proof packs; their lane README files specify which checks run with a stub or a production verifier.
