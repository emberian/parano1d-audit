# chaos-latch HANDOFF

## 1) Mission

Latch the parano1d mempool fee floor at PRODUCTION scale — the 1,024-capacity
pool on a live release daemon — blacking out financial operations
(CreateLaunch / CreateListing / Purchase) for every user of a live devnet,
scaling C29/C33 up from capacity 5/6 and testing Gemini's unproven "minimal
latch" (NM-2B: unfillable TradeOwnerIntents + self-transfers). Pinned commit
52938433; the pinned checkout `/Users/ember/dev/parano1d-break/parano1d` is
READ-ONLY.

## 2) FINAL STATE (2026-11-15, lane wrapped by coordinator order — no devnet run yet)

Everything is code-verified and staged; NO live run has happened (no node
started, no trades submitted, floor never observed live). The one open blocker
was found and fixed on disk: the shared early-finance node build fails
pack-schedule assertions; the fix is applied to the shared tree but the
shared build has NOT been rerun.

FIRST ACTIONS for the resuming agent:
1. `bash /Users/ember/dev/parano1d-break/experiments/shared-early-finance/build.sh`
   (root-owned script; my schedule-assert relaxation is already applied to its
   tree — see §5 patch file. Watch `experiments/shared-early-finance/logs/`
   and wait for `bin/parano1d` + `bin/parano1d-cli`).
2. `cd /Users/ember/dev/parano1d-break/experiments/chaos-latch && python3
   scripts/run_latch.py --phases status` (should report all nodes down).
3. Run phases in order: start → fund → preop → latch → blackout → join
   (see §4 exact commands; phases are idempotent).

## 3) Established facts (verified in source; paths are in the pinned checkout
   /Users/ember/dev/parano1d-break/parano1d — same content in the shared tree)

- Floor formula: `floor = max(5,000, 0.9 × median(last 50 admitted fees))`.
  `noid_mempool/src/floor.rs:104-113`.
- v1.1 pressure latch: engages at count ≥ 80% (count×5 ≥ cap×4) OR bytes ≥
  80%; resets ONLY at count==0 or both count AND bytes strictly < 50%
  (`floor.rs:60-89`). The briefing's "50%/25% model" was loose — the real
  high-water/low-water split is 80%/50%. At capacity 1,024: engage at count
  ≥ 820, reset below 512.
- Production capacity on the live daemon: `MAX_MEMPOOL_TXS = 1024`,
  `MAX_MEMPOOL_BYTES = 384 MiB` (`noid_chain/src/consensus/wire_limits.rs:15-18`);
  the node builds `MempoolConfig::default()` (`noid_node/src/main.rs:2146`).
- Trades occupy pool entries but SKIP fee recording (Gemini's claim REAL):
  `noid_mempool/src/pool.rs:536-549` (`if !is_trade { st.floor.record(fee) }`).
- Trade admission: `fee_limit ≥ max(floor, consensus_min)`; consensus min for
  a 1-funding trade = 6,600 µNOID; funding UTXOs must sum ≥ gross_input +
  fee_limit (`noid_mempool/src/trade.rs:117-131`, `noid_tx/.../trade.rs:142-193`).
  Trades evict on 144-block epoch-anchor change, funding consumption, or
  consensus-fee growth past fee_limit (`noid_mempool/src/trade.rs:200-230`).
- Financial intents MUST pay EXACTLY the deterministic minimum (no tipping):
  `noid_mempool/src/native.rs:162`; the floor gates them at `native.rs:167-175`
  (`required = floor.max(minimum)` → `consensus: BelowMinFee: required=N
  actual=M`). No fee escape — that IS the blackout.
- Fee schedule: 5,000 + 100/in + 700/out + 2,500/net-new-slot
  (`noid_chain/src/consensus/params.rs:433-447`). Op fees at low occupancy:
  CreateLaunch 4-in/4-out = 8,200; CreateListing 2-in/2-out = 6,600;
  Purchase 2-in/3-out = 9,800; ordinary self-transfer 1-in/1-out = 5,800.
- Floor observable live: `paranoid_getMempoolInfo` → `{size, fee_floor, txs}`
  (`noid_rpc/src/types.rs:872-879`); `estimateFeeDetailed` → `relay_floor`.
- The shared tree carries MY activation patch: MAINNET_V2_ACTIVATION_HEIGHT
  210_537→10, MAINNET_V2_1_ACTIVATION_HEIGHT 279_370→Some(24)
  (`experiments/shared-early-finance/tree/noid_chain/src/consensus/params.rs:19,23`).
  Block time 20s→30s at V2 (h10); user epoch anchor rotates every 144 blocks
  (~72 min) — trades lock ONE anchor and evict at rotation.
- The authenticated V2 and V2.1 packs pin schedule {v1_1: 95125, v2: 210537}
  (proved by the failed build assertion message and by phase-4's isolated
  build passing with the same v2.1 pack: its per-class activation is 24).
  The pack schedule is embedded in HistoryStep proof encodings
  (`noid_recursive/src/acceptance/history_step/runtime_parts_codec.rs:468-469`)
  but only COMPARED against the executable at build.rs asserts and runtime
  startup guards — relaxing those comparisons keeps the devnet self-consistent
  (same pack on both sides of every proof).
- Genesis log_slots = 24 (16.7M slots; high slot indexes safely empty).
  NFT_FEE_CREDIT = 100,000 µNOID. native_chain_id() = hash of the built-in
  genesis header (constant; same for every node). `block_id` = H_BLOCK hash
  (available via `getBlockHeader`).

## 4) Plan + exact commands (phases idempotent)

From `experiments/chaos-latch/`:

```sh
python3 scripts/run_latch.py --phases start    # n1 21060/21061 (genesis, mining key
                                                # M1) + W1 worker (8 threads, stock
                                                # devnet-chaos miner) + n2 21062/21063;
                                                # waits h>=30 (V2.1 active at 24)
python3 scripts/run_latch.py --phases fund      # ~25 min: n1 wallet sends 0.03 NOID ×
                                                # ~890 to trader wallet (paced by UTXO
                                                # supply), 4×60 NOID each to launch1/2,
                                                # 6×10 NOID to market/buyer, 50 NOID to
                                                # n2's daemon wallet
python3 scripts/run_latch.py --phases preop     # pre-latch controls at base floor, ALL
                                                # mined: CreateLaunch(launch1), collection+
                                                # mint 3 items + listings #0/#1, control
                                                # Purchase(buyer buys listing #1).
                                                # Listing #0 stays LIVE (post-latch buy
                                                # probe), item #2 stays FREE (post-latch
                                                # list probe). `factory confirm` fills
                                                # on-chain creation_ids after each mine.
python3 scripts/run_latch.py --phases latch      # stops W1 by PID; 819 unfillable trades
                                                # (fee_limit 6600, gross 1); ONE sample at
                                                # 11,000 µNOID → count 820 → floor 9,900
                                                # (MINIMAL LATCH: 819 trades + 1
                                                # self-transfer = 0.011 NOID; beats
                                                # Gemini's 26/0.578 NOID claim); then 49
                                                # more samples at 100,000 → floor 90,000
                                                # (C29/C33 parity); resume W1; samples mine;
                                                # floor must STAY 90,000 at 770 entries
python3 scripts/run_latch.py --phases blackout   # probes from 2 funded wallets on BOTH
                                                # nodes (launch1+launch2, market list,
                                                # buyer+launch2 buys, via n1 AND n2 RPC);
                                                # ordinary ≥floor control (factory sample
                                                # 95,000 + n2 daemon-wallet auto-fee
                                                # send); chain keeps advancing
python3 scripts/run_latch.py --phases join       # fresh n3 joiner (21064/21065) AFTER the
                                                # latch: does a new node inherit the pool?
python3 scripts/run_latch.py --phases status
```

Factory direct (debug; run with cwd=experiments/chaos-latch — state file
run/factory-state.json is relative): `factory/target/release/chaos-factory
http://127.0.0.1:21061 <address|floor|utxos|trades|sample|launch|collection|
confirm|mint|list|buy> ...`. Wallets (fixed secrets): trader/launch1/launch2/
market/buyer.

After blackout (recovery, if time allows): wait for the h144 epoch-anchor
rotation (trades evicted → count < 512 → floor resets; ops admit again) and/or
restart n2 (does a restarted node re-sync the latched pool?).

Then write experiments/chaos-latch/REPORT.md (conditions, code path, floor
value trajectory, blackout evidence with exact rejection lines, minimal latch
cost, controls, remaining questions) and commit named files on dev (unsigned
OK, no push).

## 5) Artifacts on disk

- `experiments/chaos-latch/HANDOFF.md` — this file.
- `experiments/chaos-latch/scripts/chaoslib.py` — driver library (Node/Managed
  with PID files; NODE_BIN/CLI_BIN → shared bin; worker = stock
  `experiments/devnet-chaos/bin/parano1d-miner`).
- `experiments/chaos-latch/scripts/run_latch.py` — the phase choreography.
- `experiments/chaos-latch/factory/` — offline intent factory (Rust; path deps
  on the shared tree). BUILT: `factory/target/release/chaos-factory` (compiled
  against identical source). It builds real authorized intents: ordinary
  1-in/1-out exact-fee self-transfers, unfillable V4 TradeOwnerIntents
  (asset [0xaa;32], valid_until u64::MAX), CreateLaunch (fee computed =
  fee_breakdown(inputs,4)), and the NFT market lifecycle
  (CreateCollection/MintNft/CreateListing/Purchase) ported from
  experiments/nft-marketplace/poc/src/lib.rs claim layouts + the C29/C33
  harness builders. Slot picks use a persisted cursor (run/factory-state.json)
  so repeated probes never SlotConflict.
- `experiments/chaos-latch/scripts/shared-tree-schedule-patch.diff` — the
  FULL diff of the shared tree vs the pinned checkout: my activation patch
  (params.rs 10/24) PLUS the schedule-assert relaxation (below).
- `experiments/shared-early-finance/tree/` — canonical patched tree (untracked).
  My relaxation applied but NOT yet rebuilt: `noid_node/build.rs` (both pack
  schedule asserts now compare only v1_1 height + V2 block time, allowing the
  pack's V2 activation height 210_537 vs the executable's 10, with a
  cargo:warning) and `noid_miner/src/history_protocol.rs` (two runtime startup
  guards, same relaxation; activation-equality checks kept).
- `experiments/shared-early-finance/build.sh` — root's build with the full
  pack/retirement-key env recipe (from devnet-chaos's poison fingerprint).
  Its last run FAILED at the v2 pack schedule assert; my tree fix addresses it;
  RERUN IT (first action).
- `experiments/chaos-latch/run/`, `logs/` — empty scaffolding (PID files,
  node data dirs, factory state will land here).

## 6) Open questions / risks

- The shared build must be rerun after my relaxation — untested end-to-end
  (compile of the touched files not yet run; expect only these two crates to
  rebuild in the shared target).
- Whether n2 (gossip peer) actually inherits the full 819-trade pool (the
  "every user" leg) — phase blackout checks it live; either result is
  reportable. Same for the fresh joiner n3.
- Trade proof generation rate at batch scale is estimated (~1-2 s each ÷ 8
  threads); if the tip moves during the batch build the factory aborts with a
  clear message — retry with the worker paused (phase latch already pauses it).
- Trades lock ONE epoch anchor: load + demo must fit before the next 144-block
  boundary; if it hits, funding UTXOs return on eviction and trades resubmit.
- Launch deposit size is unknown until the first `launch` run (wallets funded
  240 NOID across 4 UTXOs each; the C33 harness precedent used 8 coinbases).
- Recovery (epoch rotation at h144, restart behavior) is characterized only
  after blackout if time allows.

## 7) If resuming fresh

Read: this file → `experiments/chaos-latch/scripts/run_latch.py` →
`experiments/chaos-latch/factory/src/main.rs` (top comment + command
handlers) → FINDINGS.md C29/C33. Then: run the shared build
(`bash experiments/shared-early-finance/build.sh`), wait for
`experiments/shared-early-finance/bin/parano1d`, then
`python3 scripts/run_latch.py --phases start` from experiments/chaos-latch and
continue phase by phase. Discipline: stop processes by PID file only, never
pattern-kill, never git stash, commit named files only.
