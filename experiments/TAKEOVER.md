# Codex takeover: operator powers and evidence review

2026-11-15. Target remains `52938433a30b185ab007afaa15166a632cf2e82e`.
Recovered Claude session `12c30422-430b-4039-80ed-63701530c6af`, including
its latest corrections and devnet worker `a4df6f18844e968dc`.

## Material correction: state height is not HeaderDAG height

The claimed C17 -> C26 enabling step is **not established by phase 7.3**.
Its conclusion that the pinned joiner had zero honest work is contradicted by
its saved log. `devnet-chaos/scripts/phase7_multiaggressor.py:69` samples
`Node.height()`, which reads `getChainInfo.height` (`chaoslib.py:264`).
That is not the selected HeaderDAG tip or cumulative work.

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

Consequences:

- Preserve the demonstrated serving delay. Withdraw the claim that this run
  manufactures a genesis-only DAG for 20-30 seconds.
- A race before honest headers arrive remains possible in principle, but was
  not measured here. C17 alone has not been shown to prevent header learning.
- C26's staging/install differential remains a separate finding. Its original
  demonstration used a stubbed recursive terminal and no live sockets; genuine
  release-terminal generation and end-to-end offer selection remain outstanding.
- At the first handoff read, the recovered phase-8 worker's latest recorded
  action was starting the poison-node build. During this review a concurrent
  worker added `scripts/phase8_live_c26.py` and continued editing the README.
  Preserve that live work; no successful phase-8 result was verified in this
  review. Wording that phase 8 already demonstrates poisoning needs run evidence.
- A restarted or eclipsed node does not automatically lose its retained work
  or finalized history. Do not group it with a genesis-only fresh node.

The current whitepaper PDF and portions of the historical lane reports still
contain stronger wording. **They are not publication-ready.** This correction
supersedes those composition claims; it does not retract all component findings.

## The selective-selling question

No evidence reviewed so far establishes a developer-only sell privilege or a
developer key that freezes arbitrary NOID holders. This is an open audit question,
not a verified absence of such mechanisms.

| Possible power | What the evidence supports | What is still needed |
| --- | --- | --- |
| Default discovery control | Four default seed hostnames share one DNS zone. Fresh installs depend on bootstrap unless given alternatives. Author control of that zone is inferred. | Fresh-node isolation and release-verifier snapshot experiment; distinguish seed discovery from established-peer control. |
| Wallet/node deception | Existing lanes demonstrate that the GUI trusts its node's answers and that a substituted legacy matrix can survive some build checks. | Establish an economic outcome with an honest recipient/exchange analogue. A misleading confirmation is not evidence of an accepted deposit or theft. |
| Block inclusion censorship | Builders choose transactions; existing sequencing work documents this. | Attribute enough sustained block-production control to the developer. Seed or source control does not establish mining control. |
| Selective inability to deposit | Could follow from targeted wallet, propagation, or inclusion obstruction while an attacker retains an honest path. | Local two-holder/control experiment, explicit attacker powers, and an independent deposit receiver. No live interference. |
| Selling already deposited coins | This is an exchange-account operation, separate from broadcasting a chain transaction. | SafeTrade's actual deposit/trading behavior and any operator relationship are unknown. A chain halt alone does not show that existing exchange balances cannot trade. |
| Release distribution | Users installing a malicious release entrust it with local execution. No automatic update was reported. | Separate generic malicious-publisher power from a defect in the existing deployed binary. Publisher signatures alone would not protect against a malicious publisher. |

Native NOID, on-chain launched assets, and SafeTrade's internal balances must be
kept distinct throughout the report. Token-launch restrictions are not automatically
restrictions on NOID deposits or exchange sales.

## Outstanding work, owned by Codex

1. Correct the whitepaper's composition and seed-control claims throughout and
   regenerate/render its PDF. Preserve the actual component evidence and limits.
2. Recover the poison build's status without interfering with other sessions;
   build the release-terminal snapshot control and malicious-provider experiment.
   Record HeaderDAG work, canonical state, finality, provider selection, and recovery.
3. Trace privileged addresses, operation gates, release/profile controls, and all
   transaction-selection paths for asymmetric effects on native NOID holders.
4. Implement a local selective-deposit experiment with an independent receiver.
   Demonstrate both the victim's failure and the operator's success under identical
   conditions, then test the honest-peer escape path.
5. Recheck external observations and attribution. Explorer estimates are observations,
   not an independently measured hashrate or proof of who owns the miners. Avoid
   unsupported GPU-count conversions, market-cap loss ceilings, or intent claims.

## Public wording supported at this stage

“We're auditing $NOID's operator trust assumptions: who controls bootstrap,
wallet software, and transaction inclusion—and whether those powers could let
an operator keep selling while other holders cannot deposit. We have local
findings, but have not established that selective-selling scenario.”

No public post or third-party message has been sent. Existing checkout changes,
the earlier report, and the poison source tree were preserved.

## Fresh Codex swarm, 2026-11-15

The wider audit is active across eight independent questions. Each lane owns
only its own `experiments/<lane>/` directory, uses the pinned checkout as a
read-only target, and reports positive and negative results with concrete
attacker powers. Heavy builds are coordinated with the existing phase-8 devnet
run. Earlier reports are leads, not an upper bound on attack ideas.

| Lane | Core question | Evidence needed |
| --- | --- | --- |
| `operator-exit` | Can someone obstruct holders' NOID transfers or deposits while retaining their own exit? | Two-holder asymmetric control and independent receiver, including honest-path escape. |
| `snapshot-adversary` | Does C26/C27 survive the release verifier, candidate selection, retained HeaderDAG, and restart? | Valid terminal, controlled competing offers, persisted work/finality. |
| `economic-state` | Can legal native token/DEX/NFT operations extract value, trap holdings, or create unremovable state? | Production transitions and costed user impact. |
| `chain-malice` | Can malicious producers or partitions exploit reorg, finality, timing, or consensus transition? | Plausible attack cost and post-reconnection behavior. |
| `wallet-boundary` | Can wallet, recovery, or node trust cause real loss or false payment acceptance? | Actual address/receiver outcome, not just displayed status. |
| `network-abuse` | Are there resource or peer-control paths missed by the prior P2P/RPC lanes? | Wire-level cost, live local effect, and header-path controls. |
| `proof-soundness` | Do deployed relations or verifiers admit unauthorized value or serious denial? | Exact pinned relation, witness and verifier path. |
| `review-prior` | Which publication claims exceed the saved evidence? | Claim, counterevidence, and replacement wording. |

The first pass produced bounded results:

- `wallet-boundary` reproduced a wrong photo-key derivation context in three
  guides: the published recipe produces a different real wallet address.
  Official GUI recovery from the original photo still works. A separate
  local-RPC impostor test replaced the GUI backend's receive address with an
  attacker-owned one; the production wallet builder signed a synthetic payment
  to it. No block was mined, and a remote P2P peer alone was not shown to
  control the GUI's local RPC endpoint.
- `economic-state` committed 248 unsolicited NFTs with maximum unlock height
  in an isolated v2.1 block for 0.8002 NOID. The victim's verified wallet
  pages returned them, and every attempted input was rejected as timelocked.
  Native NOID remained spendable; GUI slowdown at greater scale is unmeasured.
- `operator-exit` showed `walletSend` returning a txid with zero connected
  peers while an independent receiver had no entry. Connection later delivered
  the same txid. This is an acknowledgment error, not proof of operator-run
  censorship or an exchange deposit.
- `chain-malice` traced an 18-block replaceable suffix despite documentation
  saying at most 17. Tip-inclusive 18 confirmations remain replaceable by a
  heavier valid fork; 19 cross the node's local finality boundary. SafeTrade's
  NOID credit threshold is unverified.
- `network-abuse` drove anonymous header requests against a live 150-block
  local node. Eight parked small responses did not stall an honest request;
  a large-chain stall remains a hypothesis.
- `proof-soundness` found no new unauthorized spend along the reviewed
  authorization and version dispatch paths. A native-versus-recursive
  differential at v2.1 activation remains high-value work.
- `snapshot-adversary` confirmed the historical timestamp validation
  differential at component level, found a five-second work-ranked candidate
  window and an install-time HeaderDAG recheck, and left genuine release
  terminal acceptance unresolved. A false advertised-work offer may delay
  honest snapshot selection; sustained starvation is untested.

The paper has been rewritten and rendered as a six-page bounded argument.
`FINDINGS.md` and the operator-power report now lead with evidence and limits.
Older detailed reports are being edited separately. The concurrently running
phase-8 release-node test remains an open experiment until it records terminal,
selection, and victim outcomes. Commit `0feaeec` accidentally captured files
staged by multiple lanes in the shared Git index; later path-scoped commits
recorded their completed results. No history was rewritten.

## Exchange boundary checked during takeover

SafeTrade's [NOID listing](https://support.safetrade.com/hc/en-us/articles/48550294413581-Parano1d-NOID-has-been-listed-on-SafeTrade)
and [market page](https://safetrade.com/markets/coin?tab=new_listings)
confirm a live NOID/USDT market (search-indexed pages inspected 2026-11-15).
Neither page establishes the chain confirmations SafeTrade requires for NOID,
the behavior of its NOID deposit node during a partition, or any relationship
between SafeTrade and the developer. Its [deposit guide](https://safetrade.com/help/faq/how-to-deposit)
describes the exchange-account credit flow separately from an on-chain send.
Therefore an accepted local `walletSend` txid is not a credited exchange
balance; a holder who deposited earlier might still trade internally during
a chain-side outage. The chain-malice lane is checking the precise 18-block
reorg boundary before this can inform a deposit-risk claim.
