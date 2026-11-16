# Parano1d system study

This repository examines the parano1d / NOID node, wallet, and native asset
system. The subject is the source at commit
`52938433a30b185ab007afaa15166a632cf2e82e` in `parano1d/`.
Local experiments use synthetic keys and loopback nodes. Public-network and
market observations are dated separately.

The study follows four interfaces: proof and consensus rules, history and
transaction propagation, wallet interpretation, and operational control.
Each report distinguishes code behavior, local measurements, models, and
public observations.

## Start here

- [Findings](experiments/FINDINGS.md) is the indexed technical record.
- [Multi-source capacity study](experiments/capacity-envelope/README.md)
  summarizes peer, verifier, and block-production limits.
- [Takeover notes](experiments/TAKEOVER.md) track active experiments and
  corrections to earlier claims.
- [Publication review](experiments/review-prior/REPORT.md) compares draft
  statements with the saved measurements.
- [Study paper](experiments/whitepaper/parano1d-audit.pdf) and
  [source](experiments/whitepaper/parano1d-audit.typ) give the concise account.
  The experiment reports remain the authority for measurements and limits.

## Current technical picture

Ordinary header admission checks a header's wall-clock drift; snapshot staging
uses historical validation without that check. A component test installed a
future-dated intermediate header with a stubbed recursive terminal. Genuine
release-terminal verification and snapshot choice among competing providers
are separate, unmeasured steps. See the [snapshot review](experiments/snapshot-adversary/REPORT.md).

In a serving-stall run, a joiner's installed State stayed at height zero while
its HeaderDAG learned honest headers through height 150. The
[saved log check](experiments/devnet-chaos/header-progress-review.txt) separates
those two forms of progress. A later [release-node control](experiments/devnet-chaos/PHASE8_REVIEW.md)
rejected a higher-work fork whose tip was ten years in the future; two joiners
finished on honest height 31. A future-dated intermediate header beneath a
present-day tip has only the component-level result above.

An [anonymous header-serving test](experiments/network-abuse/README.md) made
40 requests from five identities against a copied store with 4,096 synthetic,
consensus-invalid header rows. RSS rose by 89–116 MiB in two bursts and
honest responses slowed during overlapping preparation, but all completed.
The [Sybil relay analysis](experiments/sybil-availability/REPORT.md) finds
that inbound identities can consume direct transaction recipients. Its
initial gossip-mesh delay model assumed an honest peer would not GRAFT itself.
In a real P2P loopback control with eight Sybils connected first, the honest
outbound peer did GRAFT and received all 12 pre-heartbeat test payloads in
0–2 ms. The measured topology delivered the payloads; an established peer
with a different mesh topology remains to be tested.

A [real-wire proof-ingress control](experiments/network-abuse/C22-LIVE.md)
delivered 40 invalid proof-shaped transactions to the release node's
authorization verifier. A separate honest transaction was accepted during
the burst. The CPU-heavy rejection path is reachable over P2P; throughput
limits and behavior under sustained load remain open measurements.

The [producer resource analysis](experiments/pow-resource/README.md) shows
that a miner can reclaim collection-creation fees through its coinbase, while
continued growth still requires fresh funding slots, state-growth burn, and
winning blocks. Its full-rate state-growth figures are bounds; a full-block
run and current-network production cost have not been measured.

Two [source-only availability leads](experiments/new-avenues/REPORT.md) concern
a valid-work header whose advertised objects are withheld, and repeated
`Busy` replies that may keep a non-isolated miner waiting for those objects.
The readiness transition has not been measured in a non-isolated miner. A
separate [fee-floor review](experiments/fee-floor/REPORT.md) describes
node-local relay-fee stickiness under sustained valid congestion.

The consensus code permits replacing an 18-block suffix, while two protocol
guides state a maximum of 17. A payment with exactly 18 confirmations is
still in the replaceable suffix; it becomes protected at 19. Replacing that
block would require a valid, heavier fork. SafeTrade's NOID credit threshold
has not been verified. See the [chain review](experiments/chain-malice/REPORT.md).

The GUI's photo-key derivation uses a different domain string from the one
published in its English, Russian, and Chinese recovery guides. The official
GUI still recovers a wallet from the same original photo. An independent
implementation of the published recipe derives a different key. See
[wallet review](experiments/wallet-boundary/README.md).

The same wallet review tests receive-address sourcing. A local service bound
to the GUI's default loopback RPC port first supplied a different address,
and a production wallet builder signed a synthetic payment to that address.
The normal GUI starts a local node, which derives receive addresses from its
wallet secret. The tested substitution required control of the local RPC
endpoint.

A full local block test minted 248 unsolicited NFTs to a recipient with a
maximal unlock height. The recipient's wallet query returned every item,
while the production spend check rejected them as locked. The
[economic-state report](experiments/economic-state/REPORT.md) records 0.8002
NOID in creation fees. Native NOID remains spendable; stock-GUI latency with
larger collections has not been measured.

The [operational control map](experiments/operator-power/OPERATOR-ATTACKS.md)
separates fund keys, release publishing, seed DNS, miners, wallet RPC, and
exchange authority. Consensus directs up to 5% of launch-period subsidy to
each of two hardcoded Fund and Lab addresses. A [dated explorer snapshot](experiments/operator-power/FUND-OBSERVATION.md)
records two separate, unspent 500,000-NOID outputs sent from the fund payout
addresses; recipient key control and purpose remain open questions. The
[deeper control review](experiments/operator-power/DEEPER-CONTROLS.md) identifies
a conditional one-peer dependency in miner readiness and traces how default
seed discovery can affect an initially isolated miner's view. In a
[two-node local test](experiments/operator-exit/REPORT.md), `walletSend`
returned success with zero peers; the independent receiver saw no transaction
until the nodes connected. The transaction-to-exchange path therefore has
distinct stages: local admission, relay, inclusion, exchange credit, and trade.
The [exchange-path study](experiments/operator-exit/OPERATOR-THREAT.md) traces
their separate control points.

## How to read a claim

Each substantive report states the required conditions, relevant production
code, measured result, comparison or control, and remaining question. Source
reading explains a mechanism; a local test describes its tested configuration.
Public observations identify their time and source.

The code checkout and its pre-existing modifications are preserved. Each
experiment has its own directory. Live observations of markets, DNS, and
hashrate are dated because they change.
