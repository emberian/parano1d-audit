# chaos-asert: live devnet demonstration of the ASERT/MTP difficulty collapse

Lane `chaos-asert` of the parano1d devnet-chaos campaign. Target: pinned commit
`52938433a30b185ab007afaa15166a632cf2e82e` (checkout
`/Users/ember/dev/parano1d-break/parano1d`, read-only). This document is the
choreography derived from the sources **before** the run; measured numbers are
in `REPORT.md`.

## 1. Consensus facts used (all read from the pinned tree)

| Fact | Value | Source |
|---|---|---|
| BLOCK_TIME (v1/v1.1) | 20 s | `noid_chain/src/consensus/params.rs:11` |
| EPOCH_LENGTH | 6 | `params.rs:95` |
| HALFLIFE (v1/v1.1) | 120 s (6 × 20) | `params.rs:98` |
| MAX_FUTURE_DRIFT | 120 s | `params.rs:131` |
| MEDIAN_TIME_BLOCKS | 11 (median = 6th of 11 sorted) | `params.rs:134`, `timestamps.rs` |
| GENESIS_TARGET | 2^238 (LE byte 29 = 0x40), work = 2^18 = 262,144 hashes | `params.rs:332` |
| ASERT floor | unconditional: `next_target` never returns a target easier than GENESIS_TARGET in non-test builds | `difficulty.rs:186-219` |
| Finality depth | 18 blocks (`is_final`) | `params.rs:241`, `header.rs:289` |
| ASERT anchor | largest multiple of 6 ≤ parent height; blocks 6k+1..6k+6 all key off anchor 6k; block 6k+7 is the first keyed off the new anchor 6k+6 | `header.rs:273-286`, `mdbx_context.rs:2801` |
| v1.1 / v2 activation | 95,125 / 210,537 on the release profile — a genesis devnet runs the **v1** rule | `params.rs:19,106` |

**Height-regime subtlety (differs from the mainnet model).** The prior-art
model (`comp-sync-finality` conformance + `verification-asert` controls) is a
**v2-heights** construction: there the *parent's* timestamp selects the child's
target, so the attacker must mine one full-difficulty block (6k+1) and only 5
far-future interior blocks are needed to hand the new anchor (6k+6) the floor
target while keeping a present-day tip. A devnet from genesis is **pre-v2**:
`next_target_with_activation` keys every block off *its own* timestamp
(`difficulty.rs:92-117`, `header.rs:211-230`). Consequences, all favorable to
the attacker at these heights:

1. A far-future-timestamped block's *own* target is eased — the attacker's
   entire fork is mined at the floor; **no hard block is ever mined** (the
   "first hard block unmined" objection from `verification-asert` does not
   apply pre-v2).
2. The next epoch-boundary block inside the fork carries a **floor target with
   a far-future timestamp** and becomes the new anchor. Every block after it
   computes `excess = own_ts − anchor_ts − steps×20 < 0`, which saturates to
   zero elapsed (`difficulty.rs:103-107`), so its target = the anchor's = the
   floor. The collapse holds for anyone who builds on the fork.
3. The 5-shifted-blocks-in-an-11-window MTP minority trick from the model is
   still satisfied (the fork's timestamps are monotone under the poison clock),
   but it is not the binding constraint pre-v2; the binding constraint is the
   future-drift admission of the fork's tip on honest nodes, which ages out
   after `tip_ts − 120` wall seconds.

The v1 (legacy) fractional polynomial differs from v1.1's by < 0.013%
(`difficulty.rs:20-49`); the integer-shift arithmetic, anchor rule and floor are
identical, so the collapse semantics are the same rule the mainnet lead models.

## 2. Hardware calibration (measured in `devnet-chaos`, same machine)

From `devnet-chaos/logs/p8-poison.log` (built-in miner, `--mode miner`,
neon+pmull backend, 10 threads): `prepare_ms` (HistoryStep proving) ≈ 6.5–7.5 s
per block, `pow_ms` at floor ≈ 0.6–1.9 s (≈ 375 KH/s). Block cadence at floor
≈ 7.4 s/block. The external-worker path (node-side prep + worker PoW) ran at
~15–20 s/block at floor in `devnet-chaos` phase 0 — the devnet "runs at
genesis-floor difficulty" precisely because that path is proving-bound at ~BLOCK_TIME.

## 3. The raise

Use the **built-in miner** (`--mode miner`), not the external worker: at 7.4
s/block the chain runs 12.6 s/block ahead of schedule, so ASERT tightens one
doubling per 120 s of gain ≈ every 9.5 blocks. Predicted equilibrium
`prepare + pow(d) = 20 s` → `d* ≈ (20 − 6.7)/0.7 ≈ 19×` floor. Plan: mine
~11 minutes, then measure the plateau (expect target 2^238 → 2^234-ish, i.e.
3–4 fewer bits, R ≈ 12–19 depending on contention). Every block's header
(height, unix ts, 64-char LE target hex) is sampled for the report.

Control 1 (unshifted epochs keep difficulty high) falls out of this data: at
the plateau the last several epochs hold a flat raised target across epoch
boundaries with present-day timestamps.

## 4. The freeze and the shift size

Stop the producer (restart it as `--mode node`; no pause-mining RPC exists).
Let T = frozen tip, A = 6⌊T/6⌋ its ASERT anchor, with measured target
`T_A < GENESIS_TARGET`. The fork's first block (height T+1) gets the floor
iff its excess reaches one halflife per factor of 2 of R = GENESIS_TARGET/T_A:

```
excess₁ = (t_start + Δ) − A_ts − (T+1−A_h)×20  ≥  120×log2(R)
Δ = ceil(120×log2(R)) + 30 margin + (T+1−A_h)×20 − (t_start − A_ts) + 60 slack
```

`R ≈ 16` → `120×log2(R) ≈ 480 s` → **Δ ≈ 8–9 minutes**, clamped to [240, 1500] s.
The collapse self-check runs before committing to the wait: the poison node's
first mined block must show `difficulty_target == 2^238` exactly.

## 5. The fork

Promote relay n3 (synced to the honest chain) to the poison node: the
`devnet-chaos/bin/parano1d-poison` binary (env-gated clock,
`NOID_POISON_CLOCK_OFFSET_SECS=+Δ`; with env unset it is byte-equivalent to the
pinned behaviour) restarted on n3's data dir as `--mode miner`, seeded to the
honest relays. Its `unix_now()` = real + Δ (node consensus clock and built-in
miner template timestamp; libp2p transport clock untouched —
`poison-src/noid_node/src/main.rs:16088`, `poison-src/noid_miner/src/miner.rs:424`).

The extminer RPC path is **not** clock-shifted (`noid_rpc/src/server.rs:1252`
reads `SystemTime` directly), so the fork must be mined by the built-in miner —
an external worker on the poison node would stamp present-day templates.

Poison timeline: it syncs (chain already local), then mines NFORK = 18 floor
blocks from T (≈ 2.5 min), crossing ≥ 2 epoch boundaries — each new anchor at
the floor. Its own drift gate runs on the shifted clock, so it accepts its own
headers; honest relays reject them as future-dated until wall clock catches up.
Then stop the poison miner and restart the same data dir as a serving node S
(poison binary, `--mode node`, same Δ) so the fork tip is **fixed** — this is
the C26 shape (future-dated intermediate headers, tip that ages into the
drift window), not the phase-8 all-future-tip shape.

## 6. The wait and the gates

Admissibility deadline: `real_now ≥ fork_tip_ts − 120` (+15 s slack), i.e.
≈ Δ + mining time − 120 s after the fork starts. During the wait the honest
relays' logs are harvested for the drift-gate rejections (control 2: the same
headers, rejected while future-dated, accepted after the wait — the gate, live).
Relay tips must stay at T throughout.

## 7. Adoption and what follows

After the deadline: watch the live relays for spontaneous adoption, then force
the question — restart relay n2 (fresh header sync from [honest n1, poison S]),
then n1 if it does not follow. The fork shares ancestry with the honest chain up
to T and the honest chain is frozen, so this is an extension, not a reorg: no
finality interaction. Fork choice compares cumulative work: fork adds NFORK × 2^18
work vs the honest chain's zero post-T work, so the collapsed fork must win any
re-syncing node's selection.

Then restart an honest producer on the adopted chain (n2 as `--mode miner`,
**stock binary, real clock**). Its templates key off the fork's floor anchors:
targets stay at 2^238 while the chain mines ~7.4 s/block, and ASERT only
re-tightens one doubling per 120 s of gained schedule. Measured aftermath:
targets (floor), block cadence (~7.4 s vs the plateau's 20 s — the *work per
block* collapses by R×; the block *rate* gains only ~2.7× because block
production is HistoryStep-proving-bound on this hardware, not PoW-bound, at the
floor).

Fork-choice corollary (measured rates, no run needed): an *honestly mining*
chain at plateau R out-works a same-machine floor fork by `R×20/7.4 ≈ 3.4–5×`
— the collapse propagates through fork choice only when honest production
stops or follows; a live honest producer at raised difficulty keeps its relays.
This mirrors the mainnet model's 18.4M-floor-blocks-per-hard-block at devnet
scale. A live negative control (relays stay honest while the producer mines
through a fork+wait) is run as round B if time allows.

## 8. Topology and discipline

Ports 21100–21119 on 127.0.0.1: n1 21100/21101, n2 21102/21103, n3→P/S
21104/21105. Data dirs under `experiments/chaos-asert/run/`. Every process is
PID-filed (`run/<name>.pid`) and stopped only by that PID (RPC `paranoid_stop`
first). No `pkill`, no `git stash`, named-file commits only. Binaries are used
read-only from `experiments/devnet-chaos/bin/`; the pinned checkout is never
written.
