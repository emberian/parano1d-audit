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
- [Whitepaper source](experiments/whitepaper/parano1d-audit.typ) is being
  rewritten. The existing PDF predates several corrections; do not cite it
  as the final audit.

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

No experiment has yet shown a developer-only ability to sell NOID while
preventing other holders from selling. The audit is testing routes through
transaction propagation, wallet display, block inclusion, and exchange
deposit credit. A local `walletSend` success is not an exchange deposit, and
an exchange balance already credited can trade without a new chain transfer.

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
