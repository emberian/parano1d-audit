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
