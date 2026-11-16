# Parano1d study: current evidence register

Target: the `parano1d/` checkout at commit `52938433a30b185ab007afaa15166a632cf2e82e`. This register
describes the saved local experiments and source review as of 2026-11-15. Runs used isolated data or
loopback peers. A direct run executes the cited production path; a source conclusion traces code without
exercising the full network condition; a model extrapolates measured costs. Public observations are
dated separately. Historical wording and the larger working register remain in Git history.

## Current conclusion

The tested collision witnesses affect raw compressor and Merkle interfaces; inspected semantic consumers
did not turn them into an unauthorized spend, false block, or full proof. The release-node snapshot
control rejected a future-dated **tip** and joined the reference chain. A future-dated **intermediate**
header installed only in a component harness with a stubbed recursive terminal. No local experiment
demonstrated selective prevention of native NOID transfers, a mainnet-wide halt, or a privileged
spend/freeze instruction.

Follow-up local runs reproduced a wallet reservation that persists after its pending transaction is
evicted, and a local fee-floor rule that rejects an otherwise valid exact-fee launch intent. The wallet
run independently verified both HistoryStep terminals on an inbound local chain. Neither result shows
selective control over someone else's ability to sell or a network-wide stop. The claim-by-claim
[Gemini handoff verification](comp-verification/VERIFICATION.md) and [replication
matrix](comp-verification/REPLICATION-MATRIX.md) distinguish completed runs from source hypotheses.

The concrete control surfaces are narrower and useful to study: first-contact DNS and snapshot
providers; manually selected release binaries; local wallet RPC and node binaries; block producers'
inclusion choices; local relay and serving resources; and the separately operated exchange. These
surfaces do not establish common control by one person. A precredited exchange balance can trade without
a new on-chain deposit; whether any party could obstruct others' deposits while trading its own balance
has not been measured. See [operator control](operator-power/OPERATOR-ATTACKS.md), [operator
privileges](operator-privileges/REPORT.md), [exchange path](operator-exit/OPERATOR-THREAT.md), and
[prior-claim triage](review-prior/TRIAGE.md).

## C1–C8: commitments, proofs, and replay

| ID | Result and access condition | Boundary and primary evidence |
| --- | --- | --- |
| **C1** | Production `TAG_CAPSNODE` compression collides for distinct chosen aligned inputs after **72,274 evaluations**; the fixed witness is checked by two production calls. | Raw node hash, not an authorization forgery. The exact UTXO tree uses a different two-permutation hash. [Witness](../poc/src/bin/verify_collision.rs), [capsule experiment](capsule-chains/). |
| **C2** | Production `IVCPCSN_` node hash collides after **111,525 evaluations**; distinct raw openings and a recursive path-column builder accept the same root. | Full BaseFold C0/C1 attempts rejected altered initial leaves (`InitialMerkleFailed`), path shapes (`InvalidProofShape`), and query ordering (`FoldMismatch`). [BaseFold experiment](basefold-chains/), [leaf reachability](leaf-reachability/). |
| **C3** | Two prehashed leaf vectors at production source-tree geometry, depth 13 and cap depth 3, have the same eight-hash commitment; both raw openings verify. | A production `CAPSMIX` leaf substitute failed the cap check. Valid colliding leaf preimages or a full capsule substitution were not found. [Capsule experiment](capsule-chains/). |
| **C4** | A real `CAPSMIX` leaf digest aliases `CAPSNODE(0, X)`; the raw verifier accepts depth-0 and depth-1 descriptions of one root. | The production PCS pins depth and rejects cross-depth extensions; this witness did not reach a full proof. [Capsule experiment](capsule-chains/). |
| **C5** | Legacy `noid_fri` accepts two final-codeword encodings for the **same statement**: an explicit padding lane leaves the 64 query draws unchanged (53 distinct in the fixture). | No false-statement proof or current in-tree wallet caller found; framed challengers and newer BaseFold reject analogous variants. [Transcript run](transcript-chains/output-release.txt), [consumer survey](external-consumers/REPORT.md). |
| **C6** | The same signed transaction and owner proof pass on two eligible forks with a shared epoch anchor; real block admission accepted it on both. | A single canonical State refuses double application, and a changed anchor fails. No double-crediting consumer was found. [Fork replay](fork-replay/), [block follow-up](malicious-node/README.md). |
| **C7** | The published QROM arithmetic yields **44.96 bits** at T=2^32, S=1 and **41.65 bits** at T=S=2^32; its combined bound is vacuous near T=2^41. | A vacuous reduction is not a 2^41-query forgery. [Proof analysis](proof-soundness/REPORT.md), [typed-binding verdict](typed-bind-verdict/README.md). |
| **C8** | The certificate's fixed-permutation residual `Delta_P2b` is assumed below about **0.45067**, not evaluated for the shipped permutation. | Its relation to production node collisions depends on an unresolved definition of `BadTypedBind`; a one-query subfield distinguisher alone is not that residual. [Proof analysis](proof-soundness/REPORT.md), [verdict](typed-bind-verdict/README.md). |

**C14, C21, C23 extend this group.** C14's typed-path witness accepted four native raw paths with common
trace roots, but supplied no valid leaf/descendant openings for the colliding children and did not
instantiate `BadTypedBind` ([typed binding](typed-binding/), [verdict](typed-bind-verdict/README.md)).
C21 found a cross-tag claw after **111,423 evaluations / 0.49 s**: two distinct prehashed two-leaf trees
under `TAG_CAPSNODE` and `IVCPCSN_` share one digest and both raw verifiers accept. The searched
semantic paths recompute leaf hashes; no cross-domain consumer was found in the inspected code ([leaf
reachability](leaf-reachability/README.md), [external consumers](external-consumers/REPORT.md)). C23
records that the paper formalizes one node domain while the shipped compressor has multiple tags;
whether the demonstrated raw equality falls inside its typed-binding bad event is unresolved, so no
numeric failure probability is asserted here ([verdict](typed-bind-verdict/README.md)). The
outside-consumer survey also records a related Jetsam fork's inherited raw structure and its separate,
single-domain `hash_pair` use; it found no cross-domain acceptance route there.

## C9–C16: node and build boundaries

| ID | Result and access condition | Boundary and primary evidence |
| --- | --- | --- |
| **C9** | Three isolated builds with different consensus schedules share one P2P preflight profile ID and namespace. | Isolated builds have loopback guards. Tested V3/V4 owner, matrix/sparse, terminal, and snapshot downgrades rejected; no public cross-schedule acceptance was shown. [Version experiment](version-domains/). |
| **C10** | HeaderDAG rejects the next valid child when selected ancestry fills its **1,024-header** capacity; restart recovers. | Requires valid proof of work and the relevant topology. Mainnet work cost and sustained pinning against alternatives remain unmeasured. [Ledger experiment](ledger-fairness/REPORT.md). |
| **C11** | `getSlotsByOwner` held a chain read lock for **382 vs 62 µs median** on a 4,096-UTXO fixture (**6.16×**). | Pinned credential policy makes this local-only, not an anonymous remote query. [Ledger experiment](ledger-fairness/REPORT.md). |
| **C12** | First-seen input slots can pin a conflicting mempool transaction; coinbase/output-slot conflict and no automatic reorg readmission were reproduced. | Local policy and transaction recovery effects; no invalid transition or durable network-wide exclusion. [Ledger experiment](ledger-fairness/REPORT.md). |
| **C13** | A malformed same-txid response suppresses a corrected transaction from that peer for one recovery scan. | A new scan clears the exclusion; the bad transaction never entered the pool. [Identity experiment](identity-encoding/). |
| **C15** | A holder of the mining API bearer can submit an invalid nonce that consumes the sole prepared template before PoW validation; replay returns `Unavailable`. | Credential/local access required. The composed devnet kept its honest miner advancing; live request rate needed to stall a miner is unmeasured. [Miner experiment](external-miner/README.md), [devnet run](devnet-chaos/README.md). |
| **C16** | The standalone `build_release.sh` default can embed substituted **legacy** matrix bytes under approved metadata identity when pack bytes are controlled; a compiled decoder accepted a foreign same-shape relation. | The test used a Cargo stub for script reachability plus a real compiled decoder probe. Official v2.1 publishing authenticates its pack; v2/v2.1 matrices have separate payload checks, and retired-history builds omit legacy matrices. In a full-node control the substituted node stalled at h144 while reference nodes reached h149; no false payment was shown. [Reproduction](build-artifacts/VERIFY.md), [release review](release-chain/REPORT.md), [devnet](devnet-chaos/README.md). |

The build lane also reports **U2**: the pinned cache decoder accepts a foreign same-shape relation under
an approved digest (canonical length 210, image length 391), but persistence/consensus impact needs
controlled cache or data-dir writes ([build verification](build-artifacts/VERIFY.md)). **U3**: an
inherited `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` changes query parameters from FRI/capsule/grind
**64/64/16** to **10/8/6** in the standalone release script. No evidence showed this override in
checked-in CI or official artifacts ([build lane](build-artifacts/VERIFY.md)). U1 and U5 are represented
by C16 and C15.

## C17–C27: availability, storage, wallet, and snapshot

| ID | Result and access condition | Boundary and primary evidence |
| --- | --- | --- |
| **C17** | An anonymous 12-byte mempool pull reserves **16 MiB** of the shared **64 MiB** outbound response budget, even for a tiny answer. Four stalled large writes can hold all permits; the codec control delayed a fifth response until release (**27 ms** after one permit freed). | Responses must exceed the yamux window, about 256 KiB, to remain stalled; rotation is needed past the 30 s request timeout. A live phase-7 joiner delayed State install at h0 while learning honest HeaderDAG work through h150. The multi-peer run recorded **zero Busy**; object-serve refusal remains a source-derived composition. [P2P run](p2p-resources/README.md), [phase-7 log review](devnet-chaos/header-progress-review.txt). |
| **C18** | Header responses bypass the shared byte budget. A codec control with 64 parked maximum responses retained **75,497,472 canonical bytes** plus **35,195,688 compressed bytes**. | A real-wire control on **4,096 consensus-invalid synthetic rows** parked 40 requests from five identities: RSS **331 → 422–449 MiB**; honest 4,096-header reads completed in **86–101 ms** baseline, **157–203 ms** during preparation, **84 ms** after release. Eight requests on a 150-block valid fixture did not stall. Long-valid-chain exhaustion remains unmeasured. [P2P run](p2p-resources/README.md), [wire control](network-abuse/README.md). |
| **C19** | Local or operator-allowlisted receipt paging rescans and re-verifies all **20,000** valid stored receipts per page: **4.1–5.6 s/page**, about **225 µs/receipt**. | One-permit gate rejects concurrent scans; 50-row output is bounded. A 400-page walk extrapolates to roughly 28 minutes of one core. [RPC experiment](rpc-api/README.md). |
| **C20** | With local MDBX write access, compact restart trusts an uncommitted supply counter: fixture **45,000,000 → 168,456,789** at unchanged state root, then a valid block persisted **213,456,789** instead of **90,000,000**. | Downward tamper can stall direct spend admission; coinbase-only blocks and later passive relay still progressed in controls. No remote write path, mint, or spend forgery. [Storage experiment](storage-reorg/README.md), [devnet](devnet-chaos/README.md). |
| **C22** | Real mempool proof rejection averaged **8.340 ms** versus **134.261 µs** to make a distinct invalid payload (**62×** local CPU ratio); a four-permit burst drained **415.5/s**. | A real-wire control sent **40** corrupted roughly 90 KiB proofs through `AuthProof` from four identities; a separate honest transaction admitted (48.8 ms during burst vs 31.1 ms alone, single samples). No queue saturation or transaction loss observed. At the measured rate 84 identities imply about 38 MB/s, while 128 identities imply about 58 MB/s; identity/bandwidth delivery at scale is unmeasured. [Admission run](tx-admission/README.md), [wire control](network-abuse/C22-LIVE.md). |
| **C24** | The GUI backend uses its selected node for balances, receipt verdicts, history, status, and ordinary send construction/signing; a scripted loopback node caused the backend to accept chosen “verified” values. A service bound first to `127.0.0.1:9601` supplied a different receive address, and the production builder signed a synthetic payment to it. | Requires local endpoint, configured-node, or substituted-binary control. The normal sibling node derives its own receive address and verifies receipts; the impostor does not get the wallet key. The graphical view mapping was traced, but no mined substituted payment was demonstrated. [GUI backend](wallet-vs-node/README.md), [receive-address control](wallet-boundary/README.md). |
| **C25** | A real isolated v2.1 block assigned **248 NFTs** to a non-signing address for **0.8002 NOID** in fees. With `unlock_height = u64::MAX`, verified holdings listed all 248 and every spend failed `Timelock`. | Native NOID transfers were unaffected. Four warm 64-item storage pages took **10.056 ms p50** in total; an empty page took **16 µs**. RPC and GUI rendering at larger inventory sizes remain unmeasured. [Creation](creation-spam/README.md), [max-lock run](economic-state/REPORT.md), [timing](valid-spam/REPORT.md). |
| **C26** | Ordinary admission rejected a future-dated **intermediate** header; timeless snapshot staging accepted it and a direct harness installed a 24-header fork with a **stubbed** recursive terminal, finalizing h24. | The release-node phase-8 control instead put the future date at the **tip**: genuine terminal rejected `BadTimestamp`; joiner and second joiner chose honest h31. A genuine terminal for the middle-header case, candidate work at selection, and current-height PoW cost remain open. The node rechecks selected HeaderDAG work before install; phase-7 learned honest h150 despite State h0. [Component run](malicious-node/README.md), [snapshot review](snapshot-adversary/REPORT.md), [phase-8 control](devnet-chaos/PHASE8_REVIEW.md). |
| **C27** | Four default, unpinned DNS seed names share one domain. DNS can influence fresh discovery; ownership of each endpoint was not established. | Explicit/persisted peers and retained HeaderDAG work provide alternatives. Candidates are observed for **five seconds**, ranked by advertised bridge work, then rechecked; first contact is not final authority. Live seed service and independent-peer selection need measurement. [Operator map](operator-power/REPORT.md), [snapshot review](snapshot-adversary/REPORT.md). |

**C21 and C23** are recorded with the cryptographic group above. **U4** was the earlier source/component
observation behind C26; its missing genuine terminal remains the same. The phase-8 script's “OK” denotes
completion; its chain-selection fields show the candidate was not installed.

## C28–C37: Gemini handoff follow-up

| ID | Observed result | Boundary and primary evidence |
| --- | --- | --- |
| **C28** | A wallet-built ordinary spend with a real authorization proof entered the mempool. Two PoW-valid local blocks with genuine HistoryStep terminals were independently accepted on a second MDBX chain through the pinned inbound terminal verifier; block 2's coinbase occupied the spend's chosen output slot. The mempool evicted the spend, but the wallet kept its input reserved, reported zero spendable balance, and refused address discovery. | The fixture chose a colliding wallet output hint. The stock miner normally avoids known pending slots; no targeted custom producer, daemon P2P/RPC, or token-sale suppression was shown. Restart cleared the reservation. [Full run](verification-wallet/README.md), [operator boundary](verification-operator-wallet/README.md). |
| **C29** | At isolated v2.1 activation, four signed ordinary spends filled 4/5 local pool entries and raised the fee floor to **90,000 μNOID**. The same signed exact-fee CreateLaunch was accepted in a fresh pool and rejected by the pressured pool at **8,600 μNOID**, with one slot free. | The accepted setup blocks used native consensus with a stubbed HistoryStep terminal callback; pool size and fee window were scaled down. This is local admission policy, not consensus exclusion or a network-wide market freeze. [Run](verification-fee-floor/README.md), [output](verification-fee-floor/result.log). |
| **C30** | The production reorg shrink primitive panicked in release mode when a dirty evicted segment crossed a 25→24 slot-domain contraction; a resident-state control shrank and preserved its root. | Component test only. A mainnet expansion first requires at least **12,582,912** active slots through the finalized window and then a valid competing fork. No completed MDBX reorg or daemon halt was observed. [Component result](verification-reorg/RESULT.md). |
| **C31** | The production pure template builder returned `NoCoinbaseSlot` after a selected spend reserved the only two holes in a loaded segment while another populated segment was evicted. With the same root and that segment loaded, the control built a template. | Scaled-state component test, not a valid persisted mainnet chain or release miner failure. [Template control](verification-template/README.md). |
| **C32** | Two distinct chosen 64-byte inputs collided under the *fully padded* generic zero-IV Poseidon2b sponge after **73,494** restricted first-block trials. The same digest came from its public concatenation trait method; a tagged-hash control differed. | No pinned production caller of the generic 64-byte digest was found. Consensus Merkle nodes use tagged compression; wallet key derivation fixes its input format. No valid leaf collision, proof forgery, or wallet compromise follows. [Witness and caller review](verification-crypto/RESULTS.md). |
| **C33** | Three genuine V4-funded trade intents for an unindexed asset stayed in a capacity-six local pool after two signed high-fee ordinary spends entered an accepted PoW/MDBX block. The local floor remained **90,000 μNOID** at 3/6 occupancy; the same **8,600 μNOID** launch intent passed fresh-pool admission and failed latched-pool admission. | The block's recursive terminal callback was stubbed, and production capacity is 1,024. Network propagation, mainnet-scale funding/proof cost, and market-wide exclusion remain unmeasured. [Local run](verification-trade-latch/RESULT.md). |
| **C34** | A shorter-heavier local PoW/MDBX reorg orphaned a height-five payment. Normal wallet reorg cleanup removed its receipt; a cut after chain commit but before wallet cleanup left it visible through the wallet receipt path, including after a new height-five block. A later restart removed it. | The cut was modeled by preserving wallet files, not a killed daemon; HistoryStep verification was stubbed. This is a temporary wallet artifact error, not a chain halt. [Receipt run](verification-receipts/README.md). |
| **C35** | A signed CreateLaunch admitted under a synthetic high-pressure view at **13,100 μNOID** survived an accepted block after its exact fee became **10,600 μNOID**. The production chain template builder rejected the selected stale intent with a financial fee error; empty and freshly signed exact-fee controls built templates. | The admission pressure view was fabricated, terminal verification stubbed, and the full miner `TemplateBuilder`/daemon path was not run. Mainnet 50% occupancy requires 8,388,608 live slots at initial depth. [Conditional run](verification-stale-fee/RESULT.md). |
| **C36** | Real V4-authorized launches passed native validation and candidate preparation in a supported 63-page research class. Two launches used four auxiliary definitions under its cap of five and passed the binder; three used six and triggered its assertion. | Synthetic parent and research class, not a pinned mainnet-class full prover or accepted block. Authenticated mainnet bank capacity is nine; five-launch/ten-definition reachability is a source extrapolation. Normal mempool selection kept to the tested cap. [Capacity control](verification-definition-capacity/README.md). |
| **C37** | On an isolated v2.1 signed launch template, the RPC fee expression reported **10,600 μNOID** while the coinbase exceeded schedule-aware subsidy by **8,100 μNOID**. The **2,500 μNOID** difference equaled the state-growth burn; a signed ordinary spend matched exactly. | The HTTP endpoint and full terminal verifier were not run; the Native block passed production checks and state materialization. This is reporting, not a burn bypass. [Fee control](verification-rpc-fee/README.md). |

### Further tested resource and delivery behavior

- **U17–U18:** Out-of-range receipt pages retain the full key set; on 20,000 receipts an empty far page
  took **4.162 s** and about **2.90 MiB** handler peak allocation versus **20,805 B** for page 1. The
  same local/operator access applies as C19. A faithful mirror of the wallet scan's mutex and crypto on
  a four-worker HTTP runtime found 16 concurrent calls serialized; unrelated chain-info latency reached
  **310,884 µs** versus **101 µs** idle p50. The production private wallet function itself was
  source-traced, not linked into that mirror. [RPC experiment](rpc-api/README.md).
- **U19:** A local one-byte append to the stored 40-byte `chain_tip` made a **41-byte** row pass restart
  checks, while every next valid commit failed `store: mdbx: 41`; no partial tip mutation. This needs
  local write/corruption, as does C20. [Storage experiment](storage-reorg/README.md).
- **U21:** Source inspection places a shared **64 MiB/10 s** gossip byte window before decode and shares
  it with block announcements. The rate model says about five identities each sending 50 × 303,495-byte
  undecodable pushes per window could consume **75.8 MB/10 s**, causing later gossip to be ignored; the
  full multi-peer outcome was not run. [Admission report](tx-admission/README.md).
- **Sybil relay:** Source direct-recipient selection can dilute an honest peer with inbound identities.
  In the eight-identity-first loopback control, the outbound honest peer GRAFTed before heartbeat and
  received **12/12** payloads in **0–2 ms**. The independent first-hop miss model did not describe that
  topology; already meshed or more concentrated peer sets remain open. [Sybil
  report](sybil-availability/REPORT.md).
- **Unavailable objects and mining:** Source tracing shows a valid-work header advertising unavailable
  body/terminal objects can enter exact suffix sync and close non-isolated mining readiness; repeated
  `Busy` replies retain the source and may avoid the transport-extinction watchdog. No dynamic miner
  pause was run. A stronger branch or source loss can change the plan; `--genesis` bypasses the
  readiness gate. [Header/object review](new-avenues/REPORT.md).
- **Local fee floor:** The source model with **770** low-fee and **50** high-fee valid transactions
  reaches **820/1,024** mempool entries and a **90,000 µNOID** local floor. After 50 high-fee
  inclusions, a backlog above the low-water reset may retain that floor. This is relay policy, not
  consensus inclusion, and has not been measured across a live node set. [Fee-floor
  review](fee-floor/REPORT.md).
- **Local send acknowledgment:** `walletSend` returned a txid with zero peers while an independent
  receiver had no mempool entry; connecting the nodes delivered the same txid. Local admission, peer
  delivery, block inclusion, and exchange credit are distinct observations. [Sender/receiver
  test](operator-exit/REPORT.md).

## Valid state, markets, and less exposed interfaces

- **State growth (U29):** Creation fees are enforced at admission, block validation, and template
  selection; block caps bound issuance. A miner that wins its own block can reclaim the **5,800 µNOID**
  fee of a one-input/one-output collection, but sustained fresh funding slots burn **2,500 µNOID** each.
  The split-funding model gives about **102 collections/block** before packing and proof limits; no
  full-rate valid block was built. The earlier NFT-item saturation model estimated about **71 MB/day**
  State and **700 MB/day** node-local storage at **4,609 NOID/day**; cold-sync cost was source-composed,
  not run at that scale. The 512-segment reorg cap cannot bind at m24 (256 possible segments) or m25
  (512); a union above cap first becomes possible at m26 after about **25.16 million** active slots.
  [Creation lane](creation-spam/README.md), [producer analysis](pow-resource/README.md).
- **Marketplace reachability (U20, U30, U34):** At the pin, profile gates disable offers and escrow
  rounds (ops 67–75); **17** operations are disabled, **10** are sequencer-only, **10** have wallet
  construction paths, and **2** (`Swap` and `LaunchRequest`) are consensus-enabled without a shipped
  construction path. The available NFT listing path is create/instant-buy/cancel. Synthetic round-escrow
  economics were tested **below** the profile gate. [Marketplace tests](nft-marketplace/README.md),
  [operation map](dark-surface/README.md).
- **Curve rounding (U16):** The cumulative ceiling can make the final unit cost zero at sold = cap−1 and
  make a 1-NOID first purchase receive 299 units. The terminal rounding surplus is only **1 µNOID**;
  graduation cash, locked pool liquidity, and tested round trips conserve value. The free final unit
  requires earlier buyers to have funded nearly the whole curve. [DEX
  economics](dex-economics/README.md).
- **Ordering (U22–U26, sequencing):** Canonical trade-batch encoding sorts authorization keys;
  **execution** sorts a parent-bound funding-priority digest. With K=8 against M=4 tickets, observed
  trade wins were **0.672** versus **0.667** count expectation over 2,000 parents; NFT K=12/M=4 yielded
  **0.733** versus **0.750** over 400 rounds. Fees are exact minima for financial natives, not tips;
  ordinary template choice is fee-rate then txid, and consensus lets a producer choose inclusion. U22: a
  1 µNOID free curve tail costs at least **9,800 µNOID** to contest and cannot be sold alone. U23: a
  tested pre-graduation buy/pool-sell lost **12 µNOID** at the graduation seam. U24: the head is a
  rounding overcharge. U25: fees do not change trade priority. U26: consensus leaves inclusion to the
  producer. These are deterministic priority and pricing properties, not a measured extraction route.
  [Sequencing](sequencing/README.md), [DEX games](dex-games/README.md), [DEX
  economics](dex-economics/README.md).
- **NFT wash cost (U36):** Self-list and self-buy cost **16,400 µNOID** per floor-geometry cycle, with
  **2,500** burned and **13,900** miner-claimable; a self-mining participant could reclaim the latter.
  State pressure raises the cost. [NFT race tests](nft-races/README.md).
- **Wallet/API edges (U10–U12, U31–U33):** U10: ordinary send's post-build check binds fee and
  input/output counts but not recipient and amount; current builder pass-through was verified, so
  divergence needs a regression or substituted node/binary. U11: exported master secret remains in the
  OS clipboard after dialog close. U12: a stale in-memory wallet after on-disk replacement was
  reproduced without a production import path. U31: the RPC map has **79** methods, **11** unused by any
  shipped client; local `paranoid_stop` terminated a loopback daemon from an unauthenticated local
  process. U32: five hidden node flags include wallet export/import and `--genesis`; U33:
  `NOID_GUI_NODE_BIN` selects the GUI's node executable. These are local trust and maintenance surfaces.
  [Wallet safety](wallet-safety/README.md), [method map](dark-surface/README.md), [GUI
  boundary](wallet-boundary/README.md).
- **Prover/API limits (U13–U15, U28):** U13: a zero-round BaseFold raw prover read never-written scratch
  data (poison marker `0x1EAD_ED00_0000_0004` observed) and its proof self-rejected; production callers
  at the pin use nonzero rounds. U14: two degenerate raw parameters panic, behind production shape
  choices. U15: a 73-byte `R1csProofBundle` requested a **1,048,576-byte** allocation before clean EOF,
  with no in-tree caller. U28: the v2.1 compact-arithmetic path had no direct in-tree test before this
  audit; the lane's **8,870-configuration** directed battery found no second witness. [Prover
  tests](prover-unsafe/README.md), [constraint tests](constraint-systems/README.md).
- **Early auxiliary results (U6–U9):** Agent runs reported U6: a negative 20-case state matrix; U7: a
  standalone block utility missing the production epoch gate; U8: same-txid proof bytes not replacing a
  cached entry; U9: a statement-scoped UC extraction gap. Independent root reruns were not recorded for
  those four; their bounds and outputs remain in [state-consensus](state-consensus/),
  [block-validation](block-validation/), [identity-encoding](identity-encoding/), and [UC
  composition](uc-composition/REPORT.md). U35 is the real-block fork replay control already under C6.

## Negative controls that delimit the claims

- **Semantic integrity:** The exact State tree and full capsule/BaseFold checks blocked tested
  raw-collision lifts. Full contract tests found **0 mismatches in 428,880 executions**; the later
  **8,870-configuration** R1CS battery rejected directed alternative witnesses. These are bounded tests,
  not a full proof of all recursive relations. [Leaf reachability](leaf-reachability/README.md),
  [contract rights](contract-rights/RESULTS.md), [constraint tests](constraint-systems/README.md).
- **Value conservation:** Nine DEX economic experiments found no accepted extraction; **72 pool + 120
  curve/graduation** R1CS cases matched host fields, and invalid funding/output cases rejected
  atomically. Token launch and redemption controls conserved supply and rejected under/overfunding,
  duplicate slots, and forged redemptions. Listing double-buys, cancel/buy races, and a real reorg
  produced one canonical owner; the op-80 State path accepted honest execution and rejected a declinable
  whole batch. [DEX economics](dex-economics/README.md), [token accounting](token-accounting/README.md),
  [NFT races](nft-races/README.md).
- **Node/storage recovery:** Reorg rollback, abort, crash restart, and retention controls held on their
  fixtures; C20/U19 need local corruption. The devnet's reference chain kept advancing during the
  combined resource run. The phase-8 joining node and a second joiner stayed on honest h31.
  [Storage](storage-reorg/README.md), [devnet](devnet-chaos/README.md), [phase
  8](devnet-chaos/PHASE8_REVIEW.md).
- **Wallet and network gates:** Remote RPC without credentials and browser `Origin` requests failed
  before dispatch; wrong credentials did not fall back to Local. Mining credentials reached only
  template methods. Wallet secrets were absent from RPC/wire probes. Cross-schedule cryptographic
  artifacts and invalid snapshot tips rejected. [RPC](rpc-api/README.md), [dark
  surface](dark-surface/README.md), [wallet safety](wallet-safety/README.md), [version
  domains](version-domains/), [phase 8](devnet-chaos/PHASE8_REVIEW.md).

## Operational controls and public observations

Consensus fixes the subsidy split at **90% miner / 5% Fund / 5% Lab** through height **3,223,778**;
built-in genesis is empty. No address-specific freeze, administrator spend, or direct value-transfer
override was found in inspected source. Source identifies payout addresses, not their keyholders. A
dated explorer view at h313,800 reported **599,040 NOID** to each fund; two **500,000-NOID** transfers
from original addresses were unspent at their destinations in that view, about **8.22%** of reported net
supply together. Custody and purpose are unknown. [Privilege review](operator-privileges/REPORT.md),
[fund observation](operator-power/FUND-OBSERVATION.md).

The release publisher controls what new users choose to install; no automatic remote update switch was
found in an installed v2.1 node. Same-channel SHA256SUMS bind a download to that channel; an independent
reproduction or detached artifact signature was not found in this review. Default seed DNS affects new
discovery, while established nodes retain peers and work. Optional external price history can change
displayed DEX candles when enabled; local spot prices and receipt checks come from the selected node. A
separate [wallet guide check](wallet-boundary/README.md) found that the three photo-key guides use a
BLAKE3 context different from the GUI; the same photo in the official GUI still recovers the original
address. [Release control](release-chain/OPERATOR-CONTROL.md), [wallet/service
control](wallet-boundary/OPERATOR-CONTROL.md).

Source permits replacing an **18-block** suffix although text elsewhere says 17; **19 tip-inclusive
confirmations** cross this local finality boundary. Actual double-spend significance needs a service
that credits at 18 and a heavier valid fork; SafeTrade's NOID deposit and confirmation policy was not
measured. [Chain review](chain-malice/REPORT.md), [exchange review](operator-exit/OPERATOR-THREAT.md).

## Open measurements with operational consequence

1. **Fresh-node selection:** Run a release-verifier terminal for the future-dated *middle* header, with
candidate and selected HeaderDAG work logged at install; test independent providers and re-serving. The
stubbed C26 install and the rejected future-tip phase-8 run answer different questions.
2. **Public availability ceiling:** Measure C17/C18/C22/U21 against a long valid chain and realistic
mempool with multi-IP peers, honest transaction and header success, bandwidth, RSS, worker use,
timeouts, peer churn, and recovery. Include unavailable-object sync on a non-isolated miner. Current
results show resource costs and bounded delay, not a network halt.
3. **Operator and market separation:** Measure default seed reachability, independent producer and relay
paths, SafeTrade deposit policy and observed credit latency. Distinguish key custody, release control,
DNS, mining, local wallet endpoint, and exchange account control before attributing any composed
outcome.
4. **Valid-State scaling:** Build a full-rate valid collection/NFT block and measure million-row State,
cold sync, prover marginal cost, GUI inventory rendering, and 512-segment reorg behavior after the m26
threshold. Current daily/yearly figures are models.
5. **Proof assurance:** Define the deployed multi-tag `BadTypedBind` event and `Delta_P2b`; analyze
full-sponge/padded constructions and the complete recursive trade kernel. Existing collision witnesses
and directed constraint tests do not settle those questions.
