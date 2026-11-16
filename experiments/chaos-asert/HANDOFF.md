# chaos-asert HANDOFF — resumable from disk alone

Lane `chaos-asert` (ember's parano1d devnet-chaos campaign). Working dir:
`/Users/ember/dev/parano1d-break/experiments/chaos-asert/`. Keep this file
current at every milestone — especially around the long admissibility wait.

## 1) Mission

Demonstrate the ASERT/MTP difficulty collapse **dynamically on a live devnet** of
release nodes: raise difficulty above the genesis floor by fast mining, then
collapse it back to the floor with a timestamp-shifted fork mined by the poison
binary (`NOID_POISON_CLOCK_OFFSET_SECS`), wait for the fork tip to age into the
honest drift window, and characterize what follows (adoption by fork choice,
everyone mining at floor, block-rate). Deliver `REPORT.md` + measurements +
commit (named files, branch `dev`, no push). Gemini proved the arithmetic;
codex refuted the *demonstrated competing chain*; this lane runs it live.

## 2) Established facts (with evidence)

- Consensus constants: BLOCK_TIME=20, EPOCH_LENGTH=6, HALFLIFE=120,
  MAX_FUTURE_DRIFT=120, MEDIAN_TIME_BLOCKS=11, GENESIS_TARGET=2^238 (LE byte
  29 = 0x40), finality depth 18 —
  `parano1d/noid_chain/src/consensus/params.rs` (read-only pinned checkout at
  commit 52938433; NEVER write into it).
- The ASERT floor is unconditional in non-test builds:
  `parano1d/noid_chain/src/consensus/difficulty.rs:186-219` (`floor_active`).
- Pre-v2 (< height 210,537) a block's **own** timestamp selects its target
  (`difficulty.rs:92-117`, caller `header.rs:211-230`); anchor =
  largest multiple of 6 ≤ parent height (`header.rs:273-286`,
  `mdbx_context.rs:2801`). Consequence: the attacker's shifted fork blocks are
  themselves mined at the floor (no hard block needed — differs from the
  v2-heights mainnet model in `comp-sync-finality`).
- The RPC `getBlockTemplate` path uses `SystemTime::now()` directly
  (`poison-src/noid_rpc/src/server.rs:1252`) — NOT clock-shifted — so the fork
  must be mined by the poison node's **built-in miner** (`--mode miner`), whose
  template ts is env-shifted (`poison-src/noid_miner/src/miner.rs:424-440`,
  node clock `poison-src/noid_node/src/main.rs:16088`).
- Built-in miner calibration (same machine, phase-8 log):
  prepare_ms ≈ 6.5-7.5s, pow_ms ≈ 0.6-1.9s at floor (~375 KH/s, 10 threads) —
  `devnet-chaos/logs/p8-poison.log`.
- **Live raise behavior measured 2026-11-15 ~22:17-22:28 PST** (see
  `run/measurements.jsonl`, kind=block): the chain starts at floor; from h7
  difficulty tightens. Empirical law fitted on two epochs (h7..h12, h13..h16,
  residual <2s): `ratio_log2(h) = (16.33*h + b_epoch − ts_h)/120`, anchor
  target = FLOOR each epoch, `b_epoch` jumps +28.4s at each boundary, plus a
  +0.30 ratio jump on each epoch-start block. The exact pre-95125 rule could
  NOT be re-derived exactly from headers (no (BT,HL,poly,anchor) combo
  matches h7/h8/h9/h12 exactly); do not trust a closed-form delta — use the
  fork-stage floor self-check + snapshot retry instead.
- Machine contention: a sibling lane (`halt-reorg` debug tests) pushed load to
  ~140 on the 12-core Mac during the raise; prepare_ms degraded to 22-30s and
  the raise plateaued near ratio_log2 ≈ 1.5 (R ≈ 2.8, target ≈ 2^236.5)
  instead of the predicted ~4 (R≈16).

## 3) Ruled out (and why)

- External worker on the poison node for the fork — its templates would be
  present-day (`server.rs:1252`); only the built-in miner's ts is shifted.
- Freeze-without-restart — no pause-mining RPC exists (checked
  `poison-src/noid_rpc/src/api.rs`); the producer must be stopped by PID file
  and restarted as `--mode node`.
- Competing-fork variant against a live honest producer at equal hashrate:
  work rate is prep-bound (~1 block/7.4s at floor vs R×2^18/20s at plateau);
  the fork loses by ~R×20/7.4. Adoption requires the honest producer frozen
  (the "on-chain continuation" variant — this is what the stages implement).
  Mirrors the mainnet model's "18.4M floor blocks per hard block" at devnet
  scale. (Live negative control optional as round B; not scheduled.)
- Closed-form delta sizing from the source constants — the live v1-height rule
  deviates from the naive reading (fact above); deltas are sized generously
  (+240s margin) and verified by the fork-stage self-check.

## 4) Current plan and exact next commands

Stages (all `cd /Users/ember/dev/parano1d-break/experiments/chaos-asert &&
python3 scripts/collapse.py <stage>`):

1. `raise --minutes 11` — DONE-ish (see 5 for state): n1 (stock, `--mode
   miner --genesis`, ports 21100/21101) + relays n2 (21102/21103, debug log),
   n3 (21104/21105). Node data dirs under `run/`. All PID-filed.
2. `freeze` — stop n1 by PID file, restart as `--mode node` (relay), snapshot
   `run/n3` → `run/n3-snapshot`, compute anchor A = 6⌊T/6⌋, ratio R, and
   delta Δ = ceil(120·log2 R + 30 + schedule_correction + 240), clamp
   [240,1800]. Prints the numbers; **record them here**.
3. `fork [--delta N]` — promote `run/n3` to the poison miner (poison binary,
   `--mode miner`, env `NOID_POISON_CLOCK_OFFSET_SECS=Δ`, seeds n1+n2), mine
   NFORK=18 floor blocks from frozen tip T, verify every fork target ==
   2^238 exactly (`0000000000000000000000000000000000000000000000000000400000000000`),
   then stop P and restart the same dir as serving node S (poison binary,
   `--mode node`, same Δ, ports 21106/21107). On floor-check failure: stop
   poison-miner by PID file, `rm -rf run/n3 && cp -R run/n3-snapshot run/n3`,
   recompute delta = old + 120·log2(observed_target/2^238) + 60, rerun
   `fork --delta <new>`.
4. `wait` — sleeps until `fork_tip_ts − 120 + 15` wall clock (~Δ−120+mining
   seconds after the fork builds), sampling relay tips + rejection counts every
   20s into `run/measurements.jsonl`. Relays must stay frozen at T
   (`relays_frozen_ok` true) — that plus the growing reject counts is the
   drift-gate control.
5. `adopt` — 120s spontaneous-adoption watch on live relays; then restart n2
   (seeds [n1, S]) → must select the fork by cumulative work (fork shares
   ancestry; frozen honest chain = extension, not reorg); then n1 follows or
   is restarted. Record timings + hash match.
6. `produce --blocks 15` — restart n2 as honest built-in miner (stock binary,
   real clock, same data dir). Sample post-collapse blocks: targets (expect
   2^238 floor), cadence (~7.4s unloaded; slower under contention), and the
   subsequent ASERT re-tightening.
7. Write `REPORT.md`, commit named files (branch dev, no push).

State as of last update (UPDATE THIS EACH MILESTONE):
- Raise stage ran 22:16:45–22:28:30 PST 2026-11-15 under load ~140
  (sibling halt-reorg lane). Tip ≈ h33+, ratio_log2 ≈ 1.5 (R ≈ 2.8). Stage
  summary line printed by the stage: `[raise] done: {...}` — copy it here.
- Freeze/fork/wait/adopt/produce: NOT YET RUN.

## 5) Artifacts on disk

- `CHOREOGRAPHY.md` — the pre-run design with exact derivations (read first).
- `scripts/chaoslib.py` — lane lib: Managed/PID-file discipline (verbatim
  rules: stop only by the PID in `run/<name>.pid`; RPC `paranoid_stop`
  first; never kill by pattern), `rpc()`, `Node` (binary/mode/env/genesis),
  `pid_alive`, `stop_by_pidfile`, measurement appender, state I/O.
- `scripts/collapse.py` — the staged driver (see §4). `scripts/summarize.py` —
  prints the measurement tables for REPORT.md.
- `run/measurements.jsonl` — every block header seen (kind=block/fork_block),
  tip samples, wait samples, stage summaries. `run/state.json` — stage state.
- `run/n1|n2|n3/` — node data dirs; `run/n3-snapshot` after freeze (pristine
  frozen tip T for fork retries). `logs/<name>.log` — node logs (n2 debug).
- Binaries (READ-ONLY, devnet-chaos lane): `devnet-chaos/bin/parano1d`,
  `.../parano1d-poison` (env-gated clock; unset env = stock behavior).
- Verify any time: `python3 scripts/collapse.py status`; tips via RPC
  `paranoid_getChainInfo` on 127.0.0.1:21101/21103/21105/21107.

## 6) Open questions / risks

- The exact pre-95125 ASERT arithmetic is not fully explained (empirical law
  in §2); the fork self-check covers it, but if the floor check fails twice,
  escalate the delta by full halflives (120s) rather than the computed
  deficit.
- Contention: if load stays >50, fork mining (18 blocks) may take ~10 min and
  the produce cadence will be slow — record the load in the report.
- Spontaneous adoption by live relays is unknown (does a node re-request
  headers it rejected as future-dated?); the adopt stage measures it, and the
  restart fallback forces the choice either way. Whichever happens is a
  finding — document it.
- The wait is ~Δ−120+fuzz seconds (~5-8 min). A fresh agent resuming mid-wait:
  do NOT restart the wait stage from scratch; read `run/state.json`
  (`fork.admissibility_deadline`) and just re-run `wait` (it recomputes the
  deadline from state and sleeps the remainder).

## 7) If resuming fresh

Read, in order: this file, `CHOREOGRAPHY.md`, `run/state.json`, the last 20
lines of `logs/stage-*.log`. Then `python3 scripts/collapse.py status` to see
what is alive. Then continue with the next un-run stage from §4 (the stage list
is ordered; each is idempotent-safe only in sequence — never re-run `raise` on
existing data dirs with `--genesis`). Commit discipline: named files only,
never `git add -A`, no `git stash`, stop processes only by their PID files.
