# halt-reorg — handoff / live state

Mission: demonstrate (or precisely delimit) a genuine `--release` daemon PANIC when a
valid reorg reverts a slot-domain expansion (SR-1 / FINDINGS.md C30), at
**production configuration** (ember ruling 2026-11-16: no `LOG_SLOTS_GENESIS`
patch; m24 genesis, real 12,582,912-active-slot state, early-finance activation
patch only), on a devnet on hbox loopback. Secondary deliverable: a clean,
quiescent, hbox-local data-dir checkpoint for halt-stalefee once the state-build
is done (they need ≥ 8,388,608 slots at 50% occupancy; ours has 12.58M at 75%).

If resuming fresh: read this file top to bottom, then FINDINGS.md C30
(`/Users/ember/dev/parano1d-break/experiments/FINDINGS.md`), then
`verification-reorg/RESULT.md`, then pick up at "Current plan".

## Established facts (all re-verified in the pinned tree at 52938433)

Panic site — `noid_chain/src/segmented_state.rs:1533`:
`flush_segment`'s `assert!(!self.evicted.contains(&seg_id), "segment commitment
requested for evicted segment {seg_id}; hydrate it first")`. Fires for any
segment that is simultaneously FRI-`dirty` and `evicted` when
`flush_all_dirty` runs.

Reorg call path — `noid_chain/src/storage/mdbx_context.rs:2066-2071`: the revert
loop calls `shrink_to_log_slots(undo.log_slots_before)` for every reverted
block. `shrink_to_log_slots` (segmented_state.rs:1322) calls
`flush_all_dirty()` FIRST (line 1331), before any per-segment checks, and the
flush sweeps **all** dirty segments — not just the ones being shrunk. Only a
revert that crosses an expansion boundary (`undo.log_slots_before < current
log_slots`) enters the shrink loop; ordinary same-depth reorgs never flush.

The revert loop preloads only the segments named by each reverted block's undo
log (`preload_segments_for_undo_in_state`, mdbx_context.rs:2230-2259), and
`revert_block` re-materializes those same segments. Segments NOT named stay
evicted.

How segments become dirty ∩ evicted in a live daemon:
1. Compact restart — `install_evicted_exact_summary` (segmented_state.rs:1113,
   called from `try_load_compact_chain_state`, mdbx_context.rs:545) installs
   every persisted live segment as evicted with `seg_roots=None` and **FRI
   `dirty` set**; `finish_evicted_exact_summaries` (1116) clears only
   mdbx/exact dirty, deliberately keeping the FRI marker. Compact resume is the
   default restart path (mdbx_context.rs:578-593) whenever persisted
   circulating supply + segment summaries are present.
2. Natural cache pressure — writes mark segments FRI-dirty (apply_delta,
   segmented_state.rs:630); nothing on the accepted-block happy path ever
   flushes FRI dirty (the header binds the exact root via `try_state_root`,
   state.rs:680; the seal substitutes an auditor terminal). After each commit,
   `evict_cold_persisted_segments` (segmented_state.rs:1191, called at
   mdbx_context.rs:1185) evicts resident segments not held by the 64 MiB
   `ExactSegmentCache` (~9 segments at production geometry,
   exact_segment_cache.rs:17). At 75% m24 occupancy (~192 live segments) most
   written segments end up evicted while still FRI-dirty.
3. At m17-and-below geometry the panic is escapable (single non-empty segment
   always re-hydrated by the undo preload) — this is why the scaled-genesis
   plan was dropped in favor of m24.

Expansion rule — `noid_chain/src/consensus/slot_expansion.rs:34-62`:
`expected_child_log_slots` expands m→m+1 when ≥10 of the 18 finalized window
headers (`finalized_expansion_window(parent)`, heights P−35..P−18) have
`active_slot_count ≥ 3/4·2^m`. At m24 that is 12,582,912 slots; with ~510 net
slots/block the window fills ~28 blocks (~14 min at 30 s) after the count is
first reached.

Reorg acceptance — `apply_reorg_mdbx_with_objects` (mdbx_context.rs:1904+)
rejects `reorg_depth > CONSENSUS_FINALITY_DEPTH` (18, line 1950) and
replacement counts > 36 (line 1765). So the attacker fork point must be below the
expansion block B* but within 18 blocks of the victim tip T. Fork choice is
ASERT cumulative work; the attacker wins by mining more work from the fork point
(later-forked chains start with a slightly easier first target — see ASERT note
in REPORT when written).

Panic containment — the reorg runs in `tokio::task::spawn_blocking` inside
`apply_exact_suffix_offthread` (noid_node/src/main.rs:4329, spawn at 4365);
the JoinHandle error is CAUGHT at main.rs:~4657
(`"exact suffix worker panicked: {error}"`). So the daemon likely survives the
assert with a swallowed panic and a half-reverted in-RAM chain state — whether
that is a halt, a wedge, or a recovery is THE open empirical question of this
lane (capture: process alive? RPC alive? tip frozen? restart recovers?).

Slot economics (from creation-spam + pow-resource READMEs, pinned tree):
- A 1-in/1-out `CreateCollection` does NOT grow `active_slot_count`; slots need
  net-new outputs. Plain UTXO outputs: block ceiling `BLOCK_MAX_USER_OUTPUTS
  = 510` + 1 coinbase (params.rs:161). Fee = 5,000 + 100·in + 700·out +
  2,500·mult·max(0, out−in); mult 2 at 50% occupancy, 4 at 75% (fees.rs).
- A `PagedSpendIntent` may carry ≤128 pages / 256 outputs / 1,020 inputs
  (noid_tx/src/paged_spend.rs:42-44); block user-page budget 255 → ~510
  outputs/block. Mempool accepts raw `PagedSpendIntent::to_bytes()` via
  `paranoid_submitTxIntent` (noid_rpc server.rs:2428 → pool.submit_encoded).
- Owner authorization below the (patched) v2.1 activation 24 is V3; above 24
  V4 (`owner_authorization_version`, params.rs:47). Prove via
  `noid_gkr::prove_paged_spend_authorization_for_version` (public; used the
  same way in experiments/malicious-node/src/main.rs:638).
- Coinbase at v2 heights = 16 NOID/block (`V2_REWARDS_MICRONOID[0]`,
  emission.rs); state-build burn ≈ 37k NOID total — mining income covers it.

## Ruled out, and why

- Scaled genesis (LOG_SLOTS_GENESIS 17, patched root/nonce computed and
  verified on the Mac under `experiments/halt-reorg/source/`): SUPERSEDED by
  ember's production-config ruling (2026-11-16). Keep only as reference; do
  not build it. Its genesis block id would be
  56dee3310bafff2bc96313d85174a84d3c6f76aa06a92781 2f4cd919c3ea4fdb.
- m16-or-lower geometry for the panic: only one non-empty segment; the undo
  preload always hydrates it before the shrink flush. Needs ≥2 non-empty
  segments; at m24 we get ~192.
- Attacker fork from spam-completion height: reorg depth would exceed 18.
- Mining materially faster than 30 s/block sustained: ASERT converges block
  time to the target; timestamps are bounded by MTP below and +120 s future
  drift above (params.rs:131), so ~24.7k blocks ≈ 8.6 days of wall clock is
  the honest state-build cost. Measure and re-extrapolate after the first
  max-rate blocks before committing to the full run.
- A daemon-level panic during the expansion block's own apply: expand() only
  flushes in the single-segment branch (segmented_state.rs:1275); at m24→m25
  the multi-segment branch rebuilds the FRI tree from (mostly None) seg_roots
  without flushing, and the header binds the exact root, not the FRI root.

## Current plan (production config, hbox)

Tree: `experiments/shared-early-finance/tree` (V2@10, V2.1@24, log_slots 24
UNTOUCHED — verified 2026-11-16). Build ONCE on hbox at
`/othersys/lanes/parano1d/halt-reorg/source/` via `swarm-build` with
`SWARM_MEM_MAX=8G`, env recipe from
`experiments/shared-early-finance/build.sh` with pack dirs repointed at
`/othersys/lanes/parano1d/packs/` (v1: `packs/history-step-v1`; v2:
`packs/downloads/v2-pack/history-step-pack-v2`; v21:
`packs/downloads/v21-pack/history-step-pack-v21`; retirement keys inside the v2
pack dir). Banks/pins are in `build.sh` on the Mac.

Devnet on hbox loopback, ports 21040-21059: P (producer, `--mode extminer`
`--genesis` + external PoW worker `parano1d-miner` with a mining token), V
(victim, seeds P), A (attacker, seeds P). All nodes use the one patched build.
Driver + spamtool run on hbox; PID files under the lane dir; stop by PID only.

1. Measure: fund the spamtool wallet from P's mining income, submit max-rate
   fat intents, confirm ~510 slots/block, prove s/tx, apply ms/block.
   Extrapolate to ~24.7k blocks; report the number before the long run.
2. State-build: drive active_slot_count to ≥ 12,582,912 at ~510/block
   (epoch-anchor refresh every 144 blocks; keep the mempool topped with
   pre-proved intents; top up the tool wallet from P as the burn drains it).
3. Stalefee checkpoint: the moment the count is reached, stop P's worker
   (chain frozen, log_slots still 24), stop V cleanly, notify the stalefee
   lane (via root) that V's data dir is copyable. Restart V (compact resume —
   its log must say `resumed exact state from compact segment summaries`).
4. Expansion: resume P's worker; ~28 blocks later the template flips to
   log_slots 25 — that block is B*. Let P mine 3 more (tip T). A must have
   been stopped (RPC stop, PID file) at a tip below B* but ≥ T−18.
5. Attack: relaunch A isolated (`--mode extminer`, no seeds) + its own worker;
   mine until its cumulative work above the fork point exceeds P's T-side work
   (poll `getChainInfo`). Then relaunch A with `--seed V.p2p`.
6. Observe V: the revert of B* → shrink 25→24 → flush_all_dirty → the assert.
   Capture with `RUST_BACKTRACE=1` on V: panic text, backtrace, process/RPC
   liveness, tip frozen vs advancing, then restart-recovery of V. P (never
   restarted; segments resident) is the resident control: same fork must
   reorg P cleanly.
7. Write `experiments/halt-reorg/REPORT.md` (conditions, code path, measured
   panic, controls, mainnet-extrapolation clearly labeled as a model — at
   production config the extrapolation is now mostly moot, say so). Commit
   NAMED files on branch dev, unsigned OK, do not push.

## Artifacts and how to verify

- `/Users/ember/dev/parano1d-break/experiments/halt-reorg/` — this lane
  (scripts/, spamtool/, logs/, run/ are local; REPORT.md + HANDOFF.md are the
  tracked deliverables).
- `experiments/halt-reorg/source/` — superseded m17 reference copy (Mac only;
  its `target/` from the genesis-constant verification is deletable lane-local
  scratch).
- hbox `/othersys/lanes/parano1d/halt-reorg/` — source/ (early-finance tree),
  bin/ (Linux release binaries), run/ (node data dirs), logs/, scripts/,
  spamtool/. Every detached job has a PID file; verify with `pgrep -fl` read
  by hand, never pattern-kill.
- Genesis-constant verification for the (superseded) m17 copy:
  `cargo test -p noid_chain --lib consensus::genesis` in it → 5 passed,
  1 stale-array failure fixed by pasting the new id (all 6 green after).

## Open questions

- Actual packed slots/block at max rate (measure; the 510 ceiling assumes the
  template packs every page and the fee selector keeps our intents).
- Auth-bundle bytes at 128 pages vs `MAX_TX_AUTHORIZATION_BYTES` (256 KiB) —
  measure on the first intents; drop to 64-page txs if it exceeds.
- Whether V's swallowed panic is a halt, a wedge, or a recovery (step 6).
- Whether A's isolated re-mining keeps enough work parity (ASERT first-target
  easing) — poll work, add blocks as needed.

## Final state at lane handoff (2026-11-16, wrap-up ordered mid-build)

The lane was wrapped up by ember's order while the FIRST hbox release build was
in flight. Nothing else is running; no devnet node was ever launched.

**On hbox (ssh alias `hbox`), detached and MAY STILL BE RUNNING:**
- Release node build: PID **3503349** in
  `/othersys/lanes/parano1d/halt-reorg/build.pid`, log
  `/othersys/lanes/parano1d/halt-reorg/logs/build-node.log` (it was compiling
  libp2p/tokio deps at handoff; expect ~30-60 min total under swarm-build
  SWARM_MEM_MAX=8G). Check: `ssh hbox 'tail -5 /othersys/lanes/parano1d/halt-reorg/logs/build-node.log; ls /othersys/lanes/parano1d/halt-reorg/bin'`.
  Success line: `HALT_REORG_BUILD_DONE`; success artifacts:
  `bin/parano1d`, `bin/parano1d-cli`, `bin/parano1d-miner` (Linux release,
  early-finance patch, log_slots 24). If it died, rerun:
  `ssh hbox 'cd /othersys/lanes/parano1d/halt-reorg && nohup setsid env SWARM_MEM_MAX=8G swarm-build bash scripts/build-node.sh > logs/build-node.log 2>&1 & echo $! > build.pid'`.
  Stop it only by the PID in build.pid.
- Tree on hbox: `/othersys/lanes/parano1d/halt-reorg/source/` (40 MB,
  early-finance copy, verified `LOG_SLOTS_GENESIS: u32 = 24` at
  params.rs:295, V2@10/V2.1@24 at lines 19/23). Packs:
  `/othersys/lanes/parano1d/packs/` (v1 + v2 + v21 + retirement keys, shared).
- If the build finished before you read this: verify the binaries exist and
  `parano1d --version` answers, then go straight to "Current plan" step 1.

**On the Mac (tracked, committed):**
- `experiments/halt-reorg/HANDOFF.md` (this file), `scripts/build-node.sh`
  (the exact env recipe used), this REPORT placeholder is not yet written —
  the lane's REPORT.md deliverable is still pending the run itself.
- `experiments/halt-reorg/source/` — superseded m17 scaled-genesis reference
  copy, KEEP OR DELETE at will (it is gitignored like all `source/` dirs; its
  Mac `target/` debug artifacts are lane-local scratch, deletable).

**First action for the next agent:** check the hbox build (commands above);
once `bin/parano1d` exists, write the spamtool (fat PagedSpendIntent builder +
`paranoid_submitTxIntent` submitter, recipe in "Established facts"), run the
measure-first phase, and re-extrapolate the ~24.7k-block / ~8.6-day
state-build before committing to it.

## Not yet done (beyond the build)

No spamtool code written yet; no driver script; no nodes launched; no state
built; no expansion observed; no fork fed; no panic captured. The whole
execution side (plan steps 1-7 above) is pending. The mechanism analysis,
geometry, funding arithmetic, reorg-depth and ASERT constraints, and the
stalefee-checkpoint sequencing are all worked out above and ready to execute.
