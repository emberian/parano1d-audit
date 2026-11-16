# chaos-asert — live devnet demonstration of the ASERT/MTP difficulty collapse

Lane `chaos-asert`, parano1d devnet-chaos campaign. Run of 2026-11-15,
22:17–22:57 PST, on ember's Mac (12-core, contended by sibling lanes).
Pinned consensus sources: commit `52938433` (`parano1d/noid_chain/src/consensus/`,
read-only). This report states only what the logs and node RPC show; every
number below was re-verified against raw evidence (headers via
`paranoid_getBlockHeader`, `run/measurements.jsonl`, node logs) after the run.

## 1. Headline

A devnet of release binaries raised its difficulty honestly to 3.48x the
genesis floor (anchor h42, `ratio_log2` 1.799); a poison node with a
**+420 s consensus clock** then mined a fork from the frozen honest tip whose
difficulty **collapsed to the floor**: 16 of 23 fork blocks at the *exact*
genesis target 2^238, the remaining 7 at the rule's own epoch-start envelope
(<= 0.362 doublings above floor — bit-identical targets to honest epoch-start
blocks). The collapse arithmetic (delta sized from the measured anchor ratio)
worked on the first block **first try, no retry**. Honest relays rejected the
future-dated fork at the drift gate **91 times each** while it aged in, then
**adopted it spontaneously — no restarts** — as wall clock caught up, and a
stock honest binary then mined the collapsed chain at the floor with a real
clock. The demonstration is complete: raise -> shift -> collapse -> gate ->
adoption -> floor-mining, all live on release nodes.

## 2. Conditions

- Binaries: `experiments/devnet-chaos/bin/parano1d` (stock) and
  `parano1d-poison` (env-gated clock, `NOID_POISON_CLOCK_OFFSET_SECS`; unset =
  stock behavior). Consensus: BLOCK_TIME 20 s, EPOCH_LENGTH 6, HALFLIFE 120 s,
  MAX_FUTURE_DRIFT 120 s, MEDIAN_TIME_BLOCKS 11, GENESIS_TARGET 2^238
  (LE byte 29 = 0x40), finality depth 18 — `params.rs` at `52938433`.
- Devnet from genesis => **pre-v2 heights** (< 210,537): a block's *own*
  timestamp selects its target (`difficulty.rs:92-117`, `header.rs:211-230`),
  so a far-future-stamped fork is itself mined at the floor — no hard block,
  unlike the v2-heights mainnet model in `comp-sync-finality`/
  `verification-asert`.
- Topology (loopback): n1 21100/21101, n2 21102/21103, n3->poison-P->S
  21104/21105->21106/21107. Data dirs under `run/`. Every process PID-filed;
  stops by PID file only (RPC `paranoid_stop` first).
- Machine contention: a sibling lane's debug tests held load ~100-140 during the
  raise; during the fork/produce, `chaos-headerpin`'s devnet and a
  `shared-early-finance` cargo build were running. Honest-miner prepare_ms was
  6.7-12.3 s across the run (calibration: 6.5-7.5 s).

## 3. Choreography as run

| Stage | Wall (PST) | Outcome |
|---|---|---|
| raise (11 min) | 22:17-22:28 | n1 stock miner (genesis) + n2/n3 relays; 44 blocks sampled; tip h43, last-block `ratio_log2` 2.072; plateau cadence median 16.1 s under load |
| freeze | 22:28:26 | n1 stopped by PID file, restarted as relay; n3 stopped, snapshot at `run/n3-snapshot` (tip h44 `92d1fde1...`); anchor h42 measured: R = 3.479 (`ratio_log2` 1.799), need_excess 215.8 s, **delta = 420 s** |
| fork | 22:28:44-22:46:52 | poison miner (clock +420 s, built-in miner — the RPC template path is not clock-shifted) mined 23 blocks h45-h67 from the frozen tip; see section 4. Stage self-check tripped at 22:45:34 (section 4); miner stopped by RPC at 22:46:52, same data dir served by S (`--mode node`, +420 clock) |
| wait | 22:47:24-22:52:45 | sampled relay tips + rejection counts every 20 s; relays cascaded 62->67 at ~22:51:24; drift rejections 81->92 per relay, frozen at 92 once adopted |
| adopt | 22:52:30-22:54:45 | **spontaneous adoption**: both relays at the fork tip at +0.2 s into the 120 s watch; no restarts (the choreography's restart fallback never ran) |
| produce | 22:54:45-22:57:38 | n2 stopped by PID file (pid 1383), restarted as **stock honest miner, real clock** (pid 78073) on the collapsed chain; 15 blocks sampled (section 6); RUN COMPLETE 22:57:38 |

Deviations from the plan, both documented in commit `08d75bf`:
1. The fork stage's original self-check ("every fork block at the exact floor")
   tripped at 22:45:34 on 6 blocks — all epoch-start pairs (h49/50, 55/56,
   61/62). The corrected criterion (first epoch + every epoch-boundary anchor at
   exact floor; above-floor only at heights = 1,2 mod 6 within ratio <= 0.5)
   is what the live run satisfies; the epoch-start envelope is shift-invariant
   (section 7) and no delta could have removed it.
2. The poison miner mined 5 blocks past the planned 18 (h63-h67) while the
   stage was failing; they are part of the served fork and of the record
   (`measurements.jsonl` fork_block rows h45-h67).

## 4. Measured collapse

**Raise** (honest, no shift): genesis floor through h6; from h7 difficulty
tightens — the raise curve is the no-shift control. Epoch anchors:
h6 floor, h12 0.646, h18 1.279, h24 1.182, h30 1.455, h36 1.319, **h42 1.799**
(`ratio_log2`, doublings above floor). Tip h43 at 2.072. Cadence median 16.1 s
(plateau, contended) — faster than the 20 s schedule, hence the tightening.

**Freeze**: frozen honest tip T = h44 `92d1fde1...`; ASERT anchor A = h42,
ts 1794810425, target `...888699b622...` => **R = 3.479x floor**. Collapse condition
for the fork's first block: excess >= 120*log2(R) = 215.8 s. Delta computed
420 s (incl. +30 margin, -66.3 s schedule correction, +240 slack, clamped).

**Fork** (clock +420 s), 23 blocks h45-h67, from `paranoid_getBlockHeader` on
the poison node / S:

| height | ts | target | ratio_log2 |
|---|---|---|---|
| h45-h48 | 1794810944-0968 | **exact 2^238 floor** | 0 |
| h49 | 1794810976 | 0x...e007340000 | 0.299 |
| h50 | 1794810984 | 0x...20ce310000 | 0.362 |
| h51-h54 | 1794811281-1303 | **exact floor** | 0 |
| h55 | 1794811311 | 0x...e007340000 | 0.299 |
| h56 | 1794811319 | 0x...20ce310000 | 0.362 |
| h57-h60 | 1794811616-1639 | **exact floor** | 0 |
| h61 | 1794811647 | 0x...e007340000 | 0.299 |
| h62 | 1794811655 | 0x...20ce310000 | 0.362 |
| h63-h66 | 1794811952-1980 | **exact floor** | 0 |
| h67 | 1794811991 | 0x...4096340000 | 0.283 |

- 16/23 blocks at the **exact** genesis floor (hex `0000...004000...`),
  including *every* epoch-boundary anchor (h48, h54, h60, h66) and the whole
  first fork epoch h45-h48 — the delta-controlled part.
- The 7 above-floor blocks are exactly the epoch-start pairs (heights = 1,2
  mod 6) at 0.283-0.362 doublings: the rule's own epoch-start envelope
  (section 7), not an attack artifact.
- **First-try collapse**: h45's excess vs anchor h42 was 459 s >= 215.8 s
  needed; h45 landed at the exact floor with no retry. The +420 shift removed
  84-100 % of the raised difficulty (R 3.48 -> 1.00-1.28).
- Fork build: 22:28:44-22:46:52 (1056 s for 23 blocks). Unstalled cadence
  8.25 s/block (prepare 6.7-7.7 s + pow 0.7-1.4 s, ~375 KH/s). Three ~290 s
  frontier-authorization stalls (before h50, h56, h62): the built-in miner's
  peer authorization lapsed whenever the honest relays' tips fell behind the
  shifted frontier and returned when they caught up — the fork's build rate is
  bounded by honest drift admission, not hashrate (section 9).

## 5. Drift-gate control (live)

Both honest relays rejected the future-dated fork on every probe until wall
clock reached `ts - 120`, then admitted it:

- **91 `BadTimestamp` rejection lines per relay** (n1 since its 22:28:30
  restart, n2 since 22:17; first at 22:28:53 — a *compact-block precheck* of
  h45, last at 22:50:53-58 — the aging h63-h67 inventory). The wait stage's
  broader drift-reject regex counted 92.
- Per-height: h45 x18 (+2 prechecks), h51 x19-20, h57 x19, h63 x18 (+2
  prechecks), singles at mid-epoch heights — i.e. each post-stall header was
  re-rejected on every ~20-30 s probe for its whole future-dated lifetime
  (~5 min), while prompt mid-epoch headers were rejected once or twice.
- The same headers, once aged in, were **accepted** — rejections froze at 92
  the moment the relays reached the tip (22:51:24). Rejected-then-admitted,
  same node, same bytes: the gate, live.
- Relay tip timeline (from `steady authenticated tip probe` lines): 44
  (22:28:40) -> 49 (22:34:21/30) -> 55 (22:39:58/22:40:06) -> 61/62
  (22:45:38-49) -> 67 (22:51:24) — monotone, no reorgs: the fork shares ancestry
  to h44, so this was extension, never reorganization. Admission lagged naive
  admissibility (`ts - 120`) by ~11-110 s (probe-cycle granularity; section 9).

## 6. What followed

**Spontaneous adoption** — the run's open question, answered: live relays *do*
re-request and adopt aging future-dated headers; no restart was ever needed.
The choreography's "relays stay frozen at T" premise was falsified in the good
direction: the relays followed the admissible prefix continuously (h45 from
22:34) and reached the fork tip during the wait (67/67 at 22:51:24, recorded
`spontaneous_adoption: {'n1': 0.2, 'n2': 0.2}` into the adopt watch). Fork
choice followed cumulative work — the fork's ~24.7 floor-block-equivalents vs
the honest chain's zero post-h44 work — with the honest chain frozen.

**Honest floor-mining** (produce stage): n2 restarted as a **stock binary,
real clock** on the collapsed chain (start tip h67, the fork tip). 15 blocks,
cadence mean 10.5 s / median 10.0 s (contended). Verified against
`paranoid_getBlockHeader` directly:

| height | ts | ratio_log2 (doublings above floor) |
|---|---|---|
| h68-h72 | 1794812094-2141 | **0.0 — exact 2^238 floor** |
| h73 | 1794812151 | 0.288 (epoch-start envelope) |
| h74 | 1794812172 | 0.283 (target bit-identical to fork h67) |
| h75-h78 | 1794812186-2210 | 0.314 / 0.373 / 0.445 / 0.515 (climbing ~+0.07/block) |
| h79 | 1794812217 | 0.819 (new epoch start: anchor 0.515 + ~0.30) |
| h80-h82 | 1794812225-2245 | 0.882 / 0.948 / 0.994 |

The post-collapse regime, measured: the honest stock binary mines at the exact
floor while its excess stays negative (h68-h72), hits the rule's epoch-start
envelope at h73/h74 (same values as the fork's and the raise's), and then
re-tightens at ~0.07 doublings/block — back to ~2x floor within 15 blocks —
exactly the raise law running forward again. The collapse is thus a
*difficulty-level reset to the floor*, and ASERT re-erects the raised
difficulty at the honest cadence's own pace; relays n1 and S followed the
producer throughout (tips matched at every check).

## 7. Pre-95,125 ASERT empirical finding

The naive source reading (anchor ts + steps*20, own-ts selection) does **not**
fit the live chain. What fits — raise and fork alike, residual < 2 s — is:

- within an epoch, `ratio_log2(h) ~= (16.33*(h-anchor_h) + 28.4 - (ts_h -
  anchor_ts)) / 120` — an effective schedule step of **16.33 s/block**, not
  20, plus a **+28.4 s** effective anchor-offset jump at each epoch boundary;
- a **+0.30 `ratio_log2` jump on each epoch-start block** (h = 1 mod 6), with
  the pair (h = 1,2 mod 6) landing at 0.299/0.362 for ~8 s cadence — the
  epoch-start envelope;
- the floor clamp is exact and unconditional: targets clamp to precisely 2^238.

Three independent confirmations of the envelope: raise h7/h8 (honest, no
shift) = 0.299/0.362 with targets **bit-identical** (0x...e007340000 /
0x...20ce310000) to the fork's h49/50, h55/56, h61/62; the fork's h67 (11 s
post-anchor gap) = 0.283, matching the law's gap term; and the raise's
within-epoch climb (h9-h11: 0.428/0.503/0.569, ~ +0.065/block at ~8.5 s cadence)
matches the same law. No (BT, HL, poly, anchor) combination re-derives this
exactly from source (predecessor's finding, unchanged) — 16.33, +28.4 and
the +0.30 jump remain unexplained.

**Shift-invariance lesson (falsifies the original retry runbook):** within
fork-anchored epochs, anchor and block timestamps *both* carry the +delta, so
the shift cancels — a larger delta cannot remove the epoch-start envelope. The
shift eases only the first epoch (keyed to the honest raised anchor h42) and
every new anchor lands at the floor. The corrected fork criterion (commit
`08d75bf`) asserts exactly this.

## 8. Controls

1. **Drift gate** (section 5): 91 rejections/relay, then admission of the
   same bytes.
2. **No-shift control** (the raise itself): present-day timestamps at 16 s
   cadence *tightened* difficulty monotonically (floor -> 2.07 doublings);
   collapse happened only under the shifted clock.
3. **Epoch-start envelope on honest mining** (section 7): the fork's 7
   above-floor blocks are the rule's own, bit-identical to honest h7/h8 — so
   the collapse claim is not "every block at floor" but "floor or the rule's
   own envelope, with all delta-controlled blocks at exact floor," which the
   run satisfies.
4. **MTP boundary** (from `comp-sync-finality` conformance, cited): median =
   6th of 11 sorted timestamps; in the v2-heights model the sixth shifted block
   in an 11-window forces the MTP future-drift violation
   (`asert_mtp_conformance.json`). Pre-v2 the binding gate here was simple
   future drift (ts - 120), measured live.
5. **Fork choice / no reorg**: monotone relay tips 44->...->67; finality
   depth 18 never threatened (the reorg was zero blocks deep — pure extension).

## 9. Remaining questions

- The pre-95125 arithmetic (16.33 s step, +28.4 s boundary jump, +0.30
  epoch-start jump) is still not re-derived from the pinned source.
- The h63-h67 admission burst lagged naive `ts - 120` by ~30-110 s (other
  bursts: +11-37 s) — probe-cycle granularity or an additional gate; unmeasured.
- Frontier-authorization stalls: the poison miner's built-in mining needed an
  authenticated peer's frontier authorization, which lapsed ~290 s whenever the
  relays fell behind the shifted frontier. A poison miner that self-authorizes
  (or an isolated quorum) would remove the drift-admission rate limit.
- Round B (live honest producer at plateau vs the fork — the negative fork-
  choice control) not run, as the choreography allowed. Measured rates bound
  it: honest-at-plateau (16.1 s, contended) vs floor fork (8.25 s) at
  R = 3.48 => honest out-works the fork ~1.8x (3.1x at the calibrated 9.4 s
  honest cadence) — proving-bound at devnet difficulty, far below the
  PoW-bound R*20/7.4 ~ 9.4x, but > 1 either way: adoption required the freeze,
  which the run enforced.

## 10. Mainnet work-cost extrapolation — a model, not a measurement

Model inputs (`verification-asert/results.json`, sourced from
`comp-sync-finality` conformance, v2-heights construction):

| Quantity | Devnet (measured here) | Mainnet model, case h210536 | Extreme case, epoch 279504 |
|---|---|---|---|
| Difficulty ratio vs floor R | 3.479 | ~1.84e7 (hard block 4.83e12 hashes = 18,409,222 floor blocks) | ~4.4e71 |
| Clock shift | 420 s (= 120*log2 R + margins) | 4375 s (incl. MTP margin) | 11190 s |
| Fork shape | 23 blocks, all at floor/envelope (pre-v2: own-ts => no hard block) | 1 hard block + 23 floor headers (v2: parent-ts => one hard block) | same |
| Fork work | ~24.7 floor-equivalents ~ 6.5e6 hashes (+163 s proving) | 1 hard block + 23*2^18 | 1 hard block ~ 1.16e77 hashes |
| Honest chain during wait | frozen (0 blocks) | 145.8 blocks at 30 s nominal; anchor stays in the 18-block finality window only if honest interval >= 243 s | 373 blocks |

Scaling statement, labeled a model: the *mechanism* demonstrated live —
clock-shifted fork eases targets to the floor (first-try delta from the
measured anchor), the drift gate quarantines then admits as wall clock catches
up, fork choice follows cumulative work, and honest production must stop for
adoption — is exactly the mechanism the mainnet model scales. At mainnet's R
the shift grows as 120*log2(R) (~48 min of clock), the fork's floor headers
stay ~2^18 hashes each, and the asymmetry becomes the full R: one honest hard
block outweighs 18.4M floor blocks, so the binding constraints are honest
production rate and finality-window residency, not the collapse arithmetic.
The devnet's proving-bound caveat (work-rate ratio only 1.8-3.1x, section 9)
does not transfer: at mainnet PoW scale the fork's per-block cost advantage is
R, not HistoryStep time.

## 11. Artifacts

- `run/measurements.jsonl` — every block header (raise h0-h43, fork h45-h67
  incl. the h63-h67 rows appended at harvest), wait samples, stage events.
- `run/state.json` — freeze/fork/wait/adopt/produce records with hashes.
- `logs/` — `stage-{raise,freeze,fork,wait,adopt,produce,finish,finish2}.log`,
  `n1.log`/`n2.log` (relay debug), `poison-miner.log`, `poison-server.log`.
- `scripts/` — `collapse.py` (staged driver, corrected fork criterion),
  `finish_run2.sh` (post-rescue wrapper), `chaoslib.py`, `summarize.py`,
  `verify_live.py` (read-only RPC verification used for this report).
- Commits: `64b57a6` (run start / self-driving wrapper), `08d75bf` (corrected
  fork criterion + rescue), and this report's commit.
- Nodes at run end: n1 relay, n2 honest stock-binary producer on the collapsed
  chain (re-tightening past the envelope by h82), S serving the fork — all
  PID-filed under `run/`, left running per the choreography's end state.
