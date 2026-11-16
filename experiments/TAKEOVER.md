# Research log: sync progress and operating boundaries

2026-11-15. Target remains `52938433a30b185ab007afaa15166a632cf2e82e`.
Recovered Claude session `12c30422-430b-4039-80ed-63701530c6af`, including
its latest corrections and devnet worker `a4df6f18844e968dc`.

## Installed State height and HeaderDAG work are different

The phase-7.3 C17 → C26 model used installed State height as a proxy for
HeaderDAG work. The saved log records a different outcome.
`devnet-chaos/scripts/phase7_multiaggressor.py:69` samples
`Node.height()`, which reads `getChainInfo.height` (`chaoslib.py:264`).
That field is distinct from the selected HeaderDAG tip and cumulative work.

`devnet-chaos/logs/n7s-a.log:128-223` shows installed height zero alongside
HeaderDAG-selected heights 24, 49, 74, 99, 124, 149, and 150. In particular,
line 181 at 01:39:25 reports `our_height=0 target_height=150`.
The relevant source log call is `noid_node/src/main.rs:14938`; the separate
snapshot-route log at line 11452 corroborates the selected targets. The
selection gate (`main.rs:282-321`) compares against HeaderDAG ancestry/work,
not merely installed state. The headers continued to arrive during the stall.

Run `python3 experiments/devnet-chaos/scripts/check_header_progress.py` to
extract and check the saved evidence. Its preserved output is
`devnet-chaos/header-progress-review.txt`. No new network experiment is implied.

Interpretation:

- The measured serving delay affected State installation while selected
  HeaderDAG work continued to advance.
- The interval before independent headers arrive was not measured in this run.
- C26's staging/install differential remains a separate finding. Its original
  demonstration used a stubbed recursive terminal and no live sockets; genuine
  release-terminal generation and end-to-end offer selection remain outstanding.
- The phase-8 release-node control used a higher-work fork whose *tip* was
  ten years in the future. The joining node rejected its inventory and
  genuine HistoryStep terminal for `BadTimestamp`, installed the reference
  snapshot, and reached height 31. A second joiner seeded only to that node
  also reached height 31. `devnet-chaos/results.txt` records both outcomes.
  Its unconditional `PHASE8 OK` marks script completion; the chain-selection
  fields are `victim_on_poison=False` and `reserved_to_next=False`. A
  future-dated *intermediate* header under a present-day tip remains a
  different test case.
- A restarted node retains its saved work and finalized history. Its initial
  conditions differ from a fresh node's.

The paper and leading reports use these distinctions. Detailed historical
notes remain available as a record of the earlier model and its revision.

## Native NOID delivery and exchange credit

The reviewed native NOID rules require owner authorization and contain no
address-specific freeze operation. An exchange sale also depends on
transaction relay, inclusion, deposit credit, and the exchange's internal
trading service. These are separate control surfaces.

| Control surface | Observed or coded behavior | Open measurement |
| --- | --- | --- |
| Default discovery | Four default seed hostnames share one DNS zone. Fresh installs use these unless configured otherwise. | Endpoint operators and fresh-node behavior with differing providers. |
| Wallet and local node | The GUI uses its selected node for address and receipt data. Custom legacy build inputs have a separate authentication gap. | Compare endpoint substitution with a normal local node and independent receiver; examine published-binary reproducibility. |
| Block inclusion | Producers select valid transactions for their blocks. | Current producer identities and the network's inclusion behavior under differing relay paths. |
| Deposit delivery | Local `walletSend` can succeed before another peer receives the transaction. | Independent receiver and exchange-credit behavior across connectivity conditions. |
| Credited balances | Exchange-account trades can use NOID already credited, without a new chain transfer. | SafeTrade's NOID-specific deposit and confirmation policy. |
| Release distribution | Newly installed binaries determine local wallet and node behavior; the GUI has no built-in automatic updater. | Reproducibility and independent verification of distributed binaries. |

Native NOID, on-chain launched assets, and SafeTrade account balances are
separate ledgers with different transaction and custody rules.

## Initial work plan and status

1. The paper now separates installed State from HeaderDAG work and describes
   the release-node phase-8 control. Its PDF was rebuilt and inspected.
2. The release-node control rejected the clock-shifted *tip* fixture. The
   intermediate-header case remains a component result with a stubbed
   terminal; a related local exploit run was stopped by automatic review.
3. The [operational control study](operator-power/OPERATOR-ATTACKS.md) traces
   issuance, release distribution, seed discovery, wallet RPC, production,
   and exchange-account stages.
4. The [two-node send test](operator-exit/REPORT.md) measured local admission
   before peer delivery. Independent exchange-credit behavior remains open.
5. [Network observations](network-status/REPORT.md) are dated and separate
   payout addresses, measured work, service operators, and people.

## Short public description

“We are studying NOID's consensus, synchronization, wallet, and operating
interfaces. The reports distinguish source behavior, local tests, public
observations, and open measurements.”

No public post or third-party message has been sent. Existing checkout changes
and earlier experimental data were preserved.

## Parallel study lanes started 2026-11-15

The wider study began across eight independent questions. Each lane owns
only its own `experiments/<lane>/` directory, uses the pinned checkout as a
read-only target, and reports positive and negative results with concrete
access conditions. Heavy builds were coordinated with the phase-8 devnet
run. Earlier reports supplied leads for these controls.

| Lane | Core question | Evidence needed |
| --- | --- | --- |
| `operator-exit` | How do local sends, relay, inclusion, and exchange credit differ? | Two-sender control and independent receiver across connectivity conditions. |
| `snapshot-adversary` | Does C26/C27 survive the release verifier, candidate selection, retained HeaderDAG, and restart? | Valid terminal, controlled competing offers, persisted work/finality. |
| `economic-state` | What are the effects and costs of valid native token, DEX, and NFT operations? | Production transitions and measured user-facing cost. |
| `chain-malice` | How do reorg, finality, timing, and consensus transitions behave under competing forks? | Work requirement and post-reconnection behavior. |
| `wallet-boundary` | Which wallet conclusions come from the local node or recovery instructions? | Actual address/receiver outcome alongside displayed status. |
| `network-abuse` | How do P2P and RPC resource limits behave under load? | Wire-level cost, local effect, and header-path controls. |
| `proof-soundness` | How closely do deployed relations and verifiers match native checks? | Exact pinned relation, witness, and verifier path. |
| `review-prior` | Which publication claims exceed the saved evidence? | Claim, counterevidence, and replacement wording. |

The first pass produced bounded results:

- `wallet-boundary` reproduced a different photo-key derivation context in three
  guides: the published recipe produces a different real wallet address.
  Official GUI recovery from the original photo still works. A separate
  local-RPC substitution test replaced the GUI backend's receive address with a
  test address; the production wallet builder signed a synthetic payment
  to it. A later control prebound the literal default port with `NOID_RPC`
  unset and reproduced adoption by the shipped backend. No block was mined;
  a running reference node keeps its port. The measured condition was local
  RPC endpoint control.
- `economic-state` committed 248 unsolicited NFTs with maximum unlock height
  in an isolated v2.1 block for 0.8002 NOID. The recipient's verified wallet
  pages returned them, and every attempted input was rejected as timelocked.
  Native NOID remained spendable; GUI slowdown at greater scale is unmeasured.
- `operator-exit` showed `walletSend` returning a txid with zero connected
  peers while an independent receiver had no entry. Connection later delivered
  the same txid. This locates the RPC acknowledgment at local admission,
  before peer delivery or exchange credit.
- `chain-malice` traced an 18-block replaceable suffix despite documentation
  saying at most 17. Tip-inclusive 18 confirmations remain replaceable by a
  heavier valid fork; 19 cross the node's local finality boundary. SafeTrade's
  NOID credit threshold is unverified.
- `network-abuse` drove header requests against a live 150-block local node.
  Eight parked small responses did not stall the comparison request.
  A copied store extended with consensus-invalid rows to 4,096 headers
  produced a large real-wire serving response: five identities and 40
  requests raised RSS by 89–116 MiB across two bursts; concurrent reads slowed
  during preparation but all succeeded. This is transient serving pressure.
- `proof-soundness` found owner authorization enforced along the reviewed
  authorization and version dispatch paths. A native-versus-recursive
  differential at v2.1 activation remains high-value work.
- `snapshot-adversary` confirmed the historical timestamp validation
  differential at component level, found a five-second work-ranked candidate
  window and an install-time HeaderDAG recheck, and left genuine release
  terminal acceptance unresolved. A false advertised-work offer may delay
  honest snapshot selection; sustained starvation is untested.
- `release-chain` found C16 limited to custom legacy pack inputs: the
  official v2.1 path retires those matrices and authenticates a transition
  pack. The phase-6 substituted binary stalled at h144 while reference nodes
  reached h149; its GUI verified a genuine older receipt.

The paper has been rewritten and rendered as a six-page study.
`FINDINGS.md`, the operator-power report, and the crypto, asset, availability,
wallet, and network reports now lead with evidence and limits. Further lanes
found source-level direct-recipient dilution, repeated wallet work from
unspendable unsolicited NFTs, and producer fee recapture bounded by new-slot
burn and block capacity (`sybil-availability/`, `valid-spam/`, `pow-resource/`).
Their first-hop odds and sustained-growth rates are models. A later real P2P
control with eight early identities delivered all 12 pre-heartbeat test
payloads to an outbound peer, which had
GRAFTed into the mesh. The valid-spam lane measured a warm four-page storage scan
of the 248 locked items at 10.06 ms median; RPC and GUI latency remain
unmeasured. The phase-8 joining node rejected a fork with a future-dated tip
and installed the reference State; the release-terminal intermediate-header
case remains open. A separate real-wire proof control drove 40 pushes through
`AuthProof` rejection while a valid nonconflicting intent was admitted
([C22 live report](network-abuse/C22-LIVE.md)). A source review found a
node-local dynamic-fee-floor latch under sustained valid mempool congestion
([fee-floor report](fee-floor/REPORT.md)).
Commit `0feaeec` accidentally captured files
staged by multiple lanes in the shared Git index; later path-scoped commits
recorded their completed results. No history was rewritten.

## Exchange-account boundary

SafeTrade's [NOID listing](https://support.safetrade.com/hc/en-us/articles/48550294413581-Parano1d-NOID-has-been-listed-on-SafeTrade)
and [market page](https://safetrade.com/markets/coin?tab=new_listings)
confirm a NOID/USDT market (search-indexed pages inspected 2026-11-15).
Its [deposit guide](https://safetrade.com/help/faq/how-to-deposit) describes
account credit as a separate stage after an on-chain send. An accepted local
`walletSend` txid is therefore distinct from an exchange balance; an already
credited balance can be traded without a new chain transfer. SafeTrade's
NOID-specific confirmation rule and deposit-node topology remain open
questions. The chain-malice lane found that a heavier fork can replace an
18-block suffix in the reference node.

## Operating-control follow-up

The [control-surface map](operator-power/OPERATOR-ATTACKS.md) integrates four
source/public-document reviews. The reviewed native NOID rules contain no
address-specific freeze control or remote upgrade path. The development
allocation directs up to 5% of launch subsidy to each of two fixed Fund and Lab addresses
through height 3,223,778. A [dated public-explorer snapshot](operator-power/FUND-OBSERVATION.md)
reported two separate 500,000-NOID outputs from the payout addresses still
unspent at their recipients; key custody and transfer purpose remain open.
Default unpinned seed DNS concentrates fresh discovery,
new installations trust publisher binaries, and miners choose which valid
transactions their blocks include. Optional external price history can
misstate DEX candles; local-node receipts and current spot use separate data.

Further study can compare a signed spend's progress through relay,
inclusion, exchange credit, and trade under different connectivity and
producer conditions. SafeTrade's NOID-specific deposit status and
confirmation rule remain unmeasured.
