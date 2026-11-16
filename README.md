# An audit of parano1d

This repository examines the parano1d / NOID node, wallet, and native asset
system. The subject is the source at commit
`52938433a30b185ab007afaa15166a632cf2e82e` in `parano1d/`.
The local experiments use synthetic keys and loopback nodes. No result here
implies that the live network has been attacked.

The central question is what an adversary can make another user *do or
believe*. A hash collision matters if it reaches a spend or accepted block.
A resource stall matters if it stops a deposit or lets a false history win.
Control of a seed, miner, wallet binary, and exchange account are different
powers. The reports name which one each argument needs.

## Start here

- [Findings](experiments/FINDINGS.md) is the indexed technical record.
- [Takeover notes](experiments/TAKEOVER.md) track active experiments and
  corrections to earlier claims.
- [Publication review](experiments/review-prior/REPORT.md) lists statements
  in the draft paper that exceed the saved evidence.
- [Audit paper](experiments/whitepaper/parano1d-audit.pdf) and
  [source](experiments/whitepaper/parano1d-audit.typ) give the concise argument.
  The experiment reports remain the authority for measurements and limits.

## What the evidence says now

The earlier audit found a real difference between ordinary header admission
and snapshot staging: an old header with a far-future timestamp is rejected
on the ordinary path but accepted by the timeless staging check. Its local
component test installed that history with a stubbed recursive terminal.
Whether a release verifier accepts a genuine terminal for it, and whether a
running node chooses the malicious snapshot against honest candidates, remain
open. See [snapshot review](experiments/snapshot-adversary/REPORT.md).

The measured anonymous serving stall delayed an honest joiner's state install.
It did **not** keep the joiner's header DAG at genesis: its logs show the
selected honest tip reach height 150 while installed height stayed zero.
Therefore that run does not establish the claimed stall-to-poison chain.
The [saved evidence check](experiments/devnet-chaos/header-progress-review.txt)
records the distinction.
The later [release-node control](experiments/devnet-chaos/PHASE8_REVIEW.md)
also rejected a heavier poison fork because its tip was ten years in the
future; both fresh joiners ended on honest height 31. A future-dated middle
header under a present-day tip remains a separate, untested release case.

An [anonymous header-serving test](experiments/network-abuse/README.md) made
40 requests from five identities against a copied store with 4,096 synthetic,
consensus-invalid header rows. RSS rose by 89–116 MiB in two bursts and
honest responses slowed during overlapping preparation, but all completed.
The [Sybil relay analysis](experiments/sybil-availability/REPORT.md) finds
that inbound identities can consume direct transaction recipients. Its
initial gossip-mesh delay model assumed an honest peer would not GRAFT itself.
In a real P2P loopback control with eight Sybils connected first, the honest
outbound peer did GRAFT and received all 12 pre-heartbeat test payloads in
0–2 ms. Neither result is a measured sustained eclipse.

A [real-wire proof-ingress control](experiments/network-abuse/C22-LIVE.md)
delivered 40 invalid proof-shaped transactions to the release node's
authorization verifier. A separate honest transaction was accepted during
the burst. The earlier CPU asymmetry is reachable over P2P, but saturation
and honest transaction loss remain unproved.

The [producer resource analysis](experiments/pow-resource/README.md) shows
that a miner can reclaim collection-creation fees through its coinbase, while
continued growth still requires fresh funding slots, state-growth burn, and
winning blocks. Its full-rate state-growth figures are bounds, not a mined
full-block demonstration or current-network attack price.

Two [source-only availability leads](experiments/new-avenues/REPORT.md) concern
a valid-work header whose advertised objects are withheld, and repeated
`Busy` replies that may keep a non-isolated miner waiting for those objects.
No paused miner was observed. A separate [fee-floor review](experiments/fee-floor/REPORT.md)
finds node-local relay-fee stickiness under sustained valid congestion, not
a consensus or developer-only selling restriction.

The consensus code permits replacing an 18-block suffix, while two protocol
guides state a maximum of 17. A payment with exactly 18 confirmations is
still in the replaceable suffix; it becomes protected at 19. Exploiting that
boundary requires a heavier valid fork. SafeTrade's NOID credit threshold has
not been verified. See [chain review](experiments/chain-malice/REPORT.md).

The GUI's photo-key derivation uses a different domain string from the one
published in its English, Russian, and Chinese recovery guides. The official
GUI still recovers a wallet from the same original photo. An independent
implementation of the published recipe derives a different key. See
[wallet review](experiments/wallet-boundary/README.md).

The same wallet review shows a separate receive-address risk: an impostor
prebound to the GUI's default loopback RPC port can replace the address the
receive card offers to a payer. A production wallet builder signed a
synthetic payment to that substituted address. The normal GUI starts a
local node whose address answers derive from its wallet secret; a
public-network peer alone has not been shown to replace them.

A full local block test minted 248 unsolicited NFTs to a victim with a maximal
unlock height. The victim's wallet query returned every item, but the
production spend check rejected them as locked. The [economic-state report](experiments/economic-state/REPORT.md)
measures the attacker's 0.8002 NOID cost. Native NOID remains spendable;
stock-GUI slowdown from larger collections has not been measured.

The [operator attack map](experiments/operator-power/OPERATOR-ATTACKS.md)
separates fund keys, release publishing, seed DNS, miners, wallet RPC, and
exchange authority. Consensus directs up to 5% of launch-period subsidy to
each of two hardcoded Fund and Lab addresses; those payouts do not grant a
freeze or privileged spend. A [dated explorer snapshot](experiments/operator-power/FUND-OBSERVATION.md)
records two separate, unspent 500,000-NOID outputs sent from the fund payout
addresses; recipient key control and purpose are unknown. No experiment has
shown a developer-only ability
to sell NOID while preventing other holders from selling. In a
[two-node local test](experiments/operator-exit/REPORT.md), `walletSend`
returned success with zero peers; the independent receiver saw no transaction
until the nodes connected. Local admission is not an exchange deposit. An
exchange balance already credited can trade without a new chain transfer.

## How to read a claim

Each substantive report should state the attacker’s required control, the
production code or observed behavior used, a positive result, a control that
could have refuted it, and the remaining gap. Source reading supports a
mechanism; a local test supports the tested configuration; neither alone
establishes a live-network incident or a person's intent. An absence of a
demonstrated theft path is a result about our coverage, not a proof of safety.

The code checkout and its pre-existing modifications are preserved. Each
experiment has its own directory. Live observations of markets, DNS, and
hashrate are dated because they change.
