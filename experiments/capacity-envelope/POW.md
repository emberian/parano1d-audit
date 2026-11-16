# Block-production capacity envelope

**Target:** `parano1d/` at `52938433a30b185ab007afaa15166a632cf2e82e`. **Method:** pinned-source review and the dated passive public observations in [network-status](../network-status/REPORT.md). No mining, live P2P contact, stress run, or new accepted-block experiment was performed for this note. Numbers below describe conditional capacity, not a measurement of any person's hardware or intent.

## The missing conversion from twelve machines to block share

Let `h_i` be machine `i`'s *measured* Poseidon2b PoW evaluations per second, `H = Σ h_i` for the machines actually mining, and `N` the rate of all other miners in the same work units. The ideal candidate-block share is

```text
q = H / (H + N).
```

This is a first-order estimate of the share of **accepted** blocks under comparable propagation and complete valid proofs. Proof construction, stale work, propagation, and node readiness can lower effective production; network position and mining strategy may also change the accepted share from raw hash share. Computers allocated to proof generation or peer service need not add to `H`; a pool of IP addresses adds no PoW. If the machines are already part of a reported *total* network rate, the denominator is that total rather than `N + H`. Thus one must not add them to the denominator twice.

On 2026-11-16 00:05–00:35 UTC, three independent public services gave a difficulty-derived total near **0.9 TH/s**, or about `9.1 × 10^11` Poseidon2b evaluations/s, and a chain near height 313,400 ([dated observations](../network-status/REPORT.md)). This is a network-work estimate, not a count of GPUs, an external SHA-256 hashrate, or a measurement of the developer's share. The same 200-block sample gave 49% and 34.5% to its two largest payout addresses; one plausibly represents a shared pool. Payout addresses do not identify equipment owners or operators. I found no verified per-GPU Poseidon2b rate in this audit, so **twelve GPUs/computers cannot be assigned a numerical `q`**. The conditional scenarios below are shares, not claims about twelve machines.

At the v2 target of 30 seconds per block ([consensus parameters](../../parano1d/noid_chain/src/consensus/params.rs), [fork schedule](../../parano1d/noid_chain/src/consensus/forks.rs)), the ideal expected number of won blocks in duration `T` is `qT/30s`. Under a steady-rate Poisson approximation, the chance of at least one win is `1 − exp(−qT/30s)`. Difficulty adjustment, competing work, and individual miner performance make this an expectation rather than a clock or guarantee.

| Assumed accepted-block share `q` | Expected blocks per target day | Chance of ≥1 block in one target hour |
| ---: | ---: | ---: |
| 0.1% | 2.9 | 11.3% |
| 1% | 28.8 | 69.9% |
| 10% | 288 | >99.999% |
| 50% | 1,440 | effectively certain |

The absolute protocol-side ceiling is `q = 1` (2,880 target blocks/day). That ceiling says what a producer *could* choose to put in blocks if it supplied all accepted work; it says nothing about the feasibility of reaching it with twelve machines. Miner subsidy at the observed height tier is 14.4 NOID per accepted block, with the gross 16 NOID split 90% miner and 5% to each fund until the allocation ends; fees after the deterministic state-growth burn are miner-claimable ([emission](../../parano1d/noid_chain/src/consensus/emission.rs), [allocation](../../parano1d/noid_chain/src/consensus/development_allocation.rs), [official economics](https://docs.parano1d.org/protocol/economics)). This is reward accounting, not a profitability estimate.

## What won blocks let a producer do

**Choose contents and omit transactions in its own blocks.** A producer can omit a valid payment or choose its own valid transactions in the blocks it wins. At share `q`, that is approximately `q` of inclusion opportunities over time, with high short-window variance. Omission from those blocks does not prevent independent producers from including the same transaction. Durable selective exclusion would require substantially broader control of block production, relay, or the recipient's network view; this audit has not established that control. A producer cannot use an ordinary block to make another party's valid signature cease to be valid or obtain an operator-specific spend privilege.

**Create bounded, valid state growth.** The prior [resource-pricing audit](../pow-resource/README.md) checked that a self-producing miner can recover the claimable 5,800 μNOID fee of its own one-input/one-output `CreateCollection`, while the creation consumes its distinct funding UTXO. Replenishing funding inputs by splitting pays a **2,500 μNOID burn per net-new slot** at low occupancy; that burn rises at occupancy thresholds ([fees](../../parano1d/noid_chain/src/consensus/fees.rs)). The finite effective budget is 255 non-primary-coinbase pages per block ([parameters](../../parano1d/noid_chain/src/consensus/params.rs)). A pre-funded burst is bounded by **127** two-page collections per won block. Amortizing the split pages gives an illustrative steady-state **about 102 collections per won block**, before discrete packing, scheduled payouts, and actual proof throughput. Neither figure is a demonstrated full-block recursive proof.

If the producer filled only its own won blocks at that modeled steady rate, with target block timing and low occupancy:

```text
collections/day            ≈ 102 × 2,880 × q = 293,760q
new-slot burn/day           ≈ 293,760q × 0.0025 = 734.4q NOID
logical collection rows/day ≈ 130q MB across every full node
extra logical row bytes/day ≈ 100q MB relative to plain UTXOs
later cold-sync payload/day ≈ 56q MB
```

These are object-layout estimates from the [earlier local collection experiment](../creation-spam/README.md) and the page arithmetic in [resource-pricing](../pow-resource/README.md), **not MDBX file-allocation or measured sync growth**. At `q=1%`, for example, the formulas yield about 2,938 collections and 7.34 NOID of minimum burn per target day *if the modeled load is actually included*. A miner can also pay to submit valid transactions to other producers, so `q` limits what it can place unilaterally in its own blocks, not all possible fee-funded activity. Sustained load needs fresh funding outputs and working proof construction. The resource price remains real even when a producer recaptures ordinary fees.

**Select a valid competing suffix inside local finality.** Fork choice compares cumulative valid work among eligible branches ([fork choice](../../parano1d/noid_chain/src/consensus/fork_choice.rs)). There is no special operator key in this rule. The reference node permits replacement of **18** descendants of a finalized ancestor; it refuses an ancestor below its finalized checkpoint ([finality review](../chain-malice/REPORT.md)). Hence a payment at 18 tip-inclusive confirmations can still be replaced by a timely, heavier, fully valid suffix, while 19 confirmations places that payment in this node's ordinary finalized prefix. This is a local rule, not proof that any exchange uses an unsafe threshold or that a particular producer can win such a race. Separate nodes that finalize incompatible branches after a sustained partition may not reconcile by work alone; that scenario additionally requires a partition that preserves mining on both sides, which has not been demonstrated.

**Possibly occupy a miner's readiness state with unavailable objects.** The source-only [higher-work-header review](../new-avenues/REPORT.md) found that a valid higher-work header can be admitted before its body and terminal are fetched, with a conditional mining-readiness pause during exact-object retrieval. Repeated canonical `Busy` replies appear to have no cumulative no-progress ceiling in that state machine. This has **not** been shown in a non-isolated devnet or on the live network. It requires real valid PoW, a selected header, and a suitable peer/transport state; an honest stronger branch or peer loss can end the plan. Permanently withholding the body also forgoes that block's subsidy and claimable fees. A pause of one miner would not itself pause the chain; any chain-rate effect depends on that miner's effective share and other producers' response. No pause duration, network-wide effect, or twelve-machine leverage factor is established. The defensive question is whether retrieval failure can keep a miner off its last committed tip for longer than a bounded interval.

## What the twelve-machine framing does and does not establish

Twelve machines could divide work among hashing, block construction/proving, and node service. More network identities may affect peer reachability or rate limits, but they cannot multiply consensus work; peer-capacity analysis belongs in [the companion P2P study](P2P.md). High-page blocks consume bounded verification/proving resources and may lower the producer's effective accepted-block share if construction or propagation lags. We lack measurements of that tradeoff on the specified hardware. We also lack the operator's actual hardware, pool privileges, or mining share. Consequently, the defensible ceiling is expressed by `q` and hard block/finality limits, not by a guessed GPU-to-network conversion.

The strongest supported user-facing effects are delayed inclusion in the producer's own blocks, paid and finite full-node state growth, and ordinary work-based short reorg exposure up to the local finality boundary. A header-availability mining pause is a source-level candidate with potentially larger availability consequences, still awaiting a controlled observation. None of these observations establishes a unilateral ability to stop other holders from selling while a favored holder can sell.
