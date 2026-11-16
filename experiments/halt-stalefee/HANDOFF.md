# halt-stalefee — handoff state (lane wrapped 2026-11-15 ~23:00)

Lane: `halt-stalefee` (devnet-chaos campaign, ~/dev/parano1d-break). Mission:
demonstrate a REAL CHAIN HALT on a live release devnet via the stale exact-fee
financial intent deadlock (C35's missing last step), or precisely delimit why
the live daemon escapes it. **Lane wrapped by coordinator order before the live
run; everything up to the run is prepared.**

## 1. Mission (one paragraph)

A financial intent (CreateLaunch: 3-in/4-out, one net-new slot) is admitted
only at the EXACT current required fee; the required fee comes from the parent
header's committed `active_slot_count`/`log_slots` through a stepwise occupancy
multiplier (fees.rs:76-89: 1/2/4/8 at 50/75/90% of 2^log_slots). When the
multiplier later DROPS (at m24: the slot-domain EXPANSION event — a strict
10-of-18 majority of the finalized window [P-35,P-18] at ≥75% expands
log_slots 24→25, halving occupancy 75%→37.5%, multiplier 4→1), an admitted
intent now OVERPAYS: the mempool recheck evicts only `fee < required`
(pool.rs:789-798) so it is retained; selection is fee-rate descending so it
stays selected; and one selected financial intent whose signed fee ≠ required
aborts the WHOLE template build (template.rs:787 `?`), which the miner maps
to `tracing::warn!("template build failed")` → no template → no block → the
pool drains only via blocks → deadlock. Drive a real node/worker pair into
this state, hold ≥5 minutes, with controls, at production geometry (m24).

## 2. Established facts (with evidence)

- Fee formula: `required = 5_000 + 100·in + 700·out + 2_500·multiplier·max(0, out-in)`;
  multiplier 1/2/4/8 at occupancy ≥50/75/90% of 2^log_slots
  (`noid_chain/src/consensus/fees.rs:24-26,76-122`). CreateLaunch 3-in/4-out:
  10,600 / 13,100 / 18,100 / 23,100 µNOID at mult 1/2/4/8.
- The template fee check reads the PARENT header:
  `noid_chain/src/block/native_assets/financial.rs:157-162` (`context.fee != fee`
  → `FinancialBlockError::Fee` → Display "financial block: Fee"), via
  `MarketFeeContext::from_parent` (`state/native_assets/market/liquidity.rs:24-29`).
- Whole-template abort on one bad intent: `checked_candidate(...)?` at
  `noid_chain/src/consensus/template.rs:787` (loops all selected groups, no skip).
- Miner mapping: `noid_miner/src/template.rs:550-568` — build error →
  `tracing::warn!(err = ?error, "template build failed")` → `None`; the RPC
  returns `template build failed` to the worker (`noid_rpc/src/server.rs:1292-1295`);
  the worker prints `template fetch failed: … — retrying in 2s` and retries
  (`noid_extminer/src/main.rs:415-426`). Three grep-able halt observables.
- Admission requires the exact fee for financial intents:
  `noid_mempool/src/native.rs:155-166`.
- Eviction asymmetry: `noid_mempool/src/pool.rs:789-798` evicts only
  `fee < required_total`. Overpayers are never evicted by this rule; the
  native-context recheck does not catch them either (C35 observed retention:
  `experiments/verification-stale-fee/RESULT.md`).
- Selection is fee-rate descending (`noid_chain/src/mempool.rs:730-762`,
  `fee_index` BTreeMap); `fee_rate = fee/(in+out+4·net_new)` (mempool.rs:111-120).
  Launch at mult 4 has rate 1,645; wallet overpayers at fee 20,000 have rate
  6,666 (2-in/1-out) and 10,000 (1-in/1-out) — the exclusion lever.
- The owner authorization survives new blocks (not tip-hash/height bound):
  `checked_financial_owner` rebuilds context from the round
  (`noid_tx/src/native_assets/wire.rs:563-593`); C35 passed it after a new
  block. The miner's per-intent pre-check (`noid_miner/src/template.rs:472-494`)
  re-runs only the owner check, NOT the fee check — the stale intent passes it
  and still aborts the build.
- Expansion rule: `noid_chain/src/consensus/slot_expansion.rs:18-62` —
  window [P-35, P-18] (CONSENSUS_FINALITY_DEPTH=18, EXPANSION_WINDOW=18),
  strict >9-of-18 at ≥75% (12,582,912 of 2^24) expands child log_slots+1;
  earliest at parent height 35. Expansion halves occupancy → multiplier drops.
  `expected_child_log_slots` is used by the miner's template placement
  (`noid_miner/src/template/trades.rs:156-163`).
- Mempool floor only rises at ≥80% pool pressure (count/bytes), 90% of the
  recent-fee median (`noid_mempool/src/floor.rs:60-113`); capacity 1024/384MB
  (`wire_limits.rs:15-18`) — this lane's load stays far below; floor stays 5,000.
- Wallet send selection: largest-first, exact-single preference, pending-input
  exclusion (`noid_node/src/wallet/state.rs:533-576`); explicit overpay fees
  allowed above the minimum (`noid_rpc/src/wallet_ops.rs:320-333`).
  Deterministic shapes: 1-in/1-out via amount=vmax−fee; 2-in/1-out via
  amount=v1+v2−fee. Deposit for a 100k-token launch = 100 NOID
  (1,000 µNOID/token, `noid_tx/src/asset_backing.rs:6,34-39`); block reward
  50 NOID (`params.rs:360`).
- Default nodes mine Small+Native classes only (`--v2-large-blocks` opt-in,
  `noid_node/src/main.rs:1354-1358`; `noid_miner/src/template.rs:384-397`).
- `native_chain_id() = hash(genesis_header())`
  (`noid_chain/src/state/native_assets.rs:24-27`) — intentool MUST be built
  against the same tree as the devnet binaries or intents carry a foreign
  chain id.
- RPC surface used by the driver: `submitTxIntent` (server.rs:2428-2436 →
  `mempool.submit_encoded`), `getSlotsByOwner`, `getSlotHintsSalted`,
  `getEpochAnchor`, `getBlockHeader` (full fields), `getActiveSlotCount`,
  `walletSend(to, amount, fee)` (0 = auto), `walletUtxoSnapshot`,
  `getMempoolSize`, `getBlock`, `getBlockTemplate`/`submitBlock` (mining
  bearer). Loopback = Local scope, no bearer needed for wallet ops.
- Shared build status at wrap-up: the Mac shared-early-finance build FAILED at
  22:27 on `noid_node/build.rs` (v2-pack schedule assert, old text) — but the
  tree ALREADY carries the relaxed assertion (only the V2 activation height may
  differ; compares `(v1_1, block_time)` = (95125, 30) both sides — passes;
  the authorization-pack embed is unset in build.sh so the
  authorization-activation assert is not reached for the "v2" label call
  with activation=None). **A plain rerun of
  `experiments/shared-early-finance/build.sh` should succeed.** The root
  believed "still building" at wrap-up — it is not; it died at 22:27.

## 3. Ruled out / decided (with why)

- **log_slots=8 scaled domain: RULED OUT by the root (2026-11-15).**
  Production m24 required; the 50%-crossing/expansion story is the
  centerpiece. Scaled fallback kept SHELVED + LABELED at `tree/`
  (LOG_SLOTS_GENESIS=8 + genesis root fe3b4afa…01f, nonce 121149,
  block id 6df0a35a…f110, computed via production
  `ChainState::with_log_slots(8).state_root()` + `search_pow` by
  `intentool genesis`; verify via the tree's `#[ignore] print_new_genesis`
  test). Use ONLY if the root re-authorizes it.
- **Fee-drop via state shrink at m24**: shedding ~4.2M slots via wallet
  spends is infeasible → the m24 fee-drop is the EXPANSION event.
- Building a second early-finance node on the Mac: ruled out by the
  coordinator (shared build).
- Stock worker (`devnet-chaos/bin/parano1d-miner`) confirmed fine
  (PoW-only, schedule-agnostic).

## 4. Current plan (m24, production geometry) — NOT YET RUN

Part 1 — Mac control devnet (shared binaries, m24):
1. `setup`: n1 producer (`--mode extminer --genesis`, mining+operator keys,
   127.0.0.1:21000/21001) + stock worker (6 threads, mining key) + n2 peer
   (21010/21011); wait h≥24 (V2.1); smoke: log_slots=24, wallet ops.
2. `control`: fund launch owner A (3×40 NOID; intentool derives the address),
   fresh exact-fee CreateLaunch (10,600 at mult 1) via intentool, confirm.
3. `calibrate`: stop worker, load 90 overpaying sends, mine one block, count
   user txs in it → per-block budget (~62 expected from the pinned native
   class; this sizes the pressure set).

Part 2 — big-state expansion deadlock (state from halt-reorg's hbox build,
12.58M+ active slots, m24, same early-finance patch):
4. `takeover --state-dir <dir>`: copy the finished state as run/n1-producer,
   start n1 WITHOUT --genesis + worker + n2; report active %, wallet balance,
   and P* (first parent height whose window is ≥10-of-18 at 75%).
5. `prep`: while the window is short of priming, fund launch owner B
   (3×40 NOID) and split wallet coinbases until ≥160 spendable UTXOs
   (splits need confirming blocks; ~28 blocks of margin exist between
   takeover and P*; each split round = 1 block).
6. `deadlock`: mine to tip P*−1, STOP the worker (window ≤9-of-18 frozen),
   load ~160 overpaying pressure sends (fee 20,000; rates 6,666/10,000 ≫
   launch's 1,645 at mult 4), admit the CreateLaunch at the exact frozen-tip
   fee (18,100), RESTART the worker: block P* (normal) and P*+1 (expansion,
   log 24→25) both exclude the launch by budget saturation; afterwards the
   required fee drops to 10,600, the launch is retained+selected, and every
   template build fails → HALT ≥5 min. Evidence: height timeline, node
   `template build failed err=…Fee…` lines, worker `template fetch failed`,
   n2 frozen height + mempool retention (gossip spread).
7. Controls: `displace` (full budget of fee-100,000 sends mines one block
   without the launch; halt returns once they drain — per-block escape only),
   `restart` (n1 restart clears the RAM mempool; chain advances).

Exact resume commands (after the shared build succeeds and reorg's state exists):
```
cd ~/dev/parano1d-break/experiments/halt-stalefee/intentool
cargo build --release        # REBUILD against ../shared-early-finance/tree (Cargo.toml already points there)
cd ~/dev/parano1d-break/experiments/halt-stalefee
python3 scripts/run_experiment.py --phases setup,control,calibrate
python3 scripts/run_experiment.py --phases takeover --state-dir <big-state-dir>
python3 scripts/run_experiment.py --phases prep
python3 scripts/run_experiment.py --phases deadlock --halt-mins 5
python3 scripts/run_experiment.py --phases displace,restart,teardown
```
The driver recomputes P* from live headers (`first_expansion_parent`) — no
guessed heights. If the window is already primed at takeover, the multiplier-4
fee window is spent on that chain (one-shot); take the state copy EARLIER.

## 5. Artifacts on disk (verify each)

- `HANDOFF.md` — this file.
- `intentool/` — Rust tool. **Current binary is STALE (built against the
  shelved log-8 tree) — rebuild before ANY use** (chain id!). Modes:
  `genesis` (prints genesis constants; verified against production code),
  `fee --rpc URL` (live tip occupancy/multiplier/required launch fee),
  `launch --rpc URL --secret-hex HEX [--submit] [--out FILE]` (builds, GKR-
  proves via `prove_native_financial_authorization`, submits via
  `submitTxIntent`; prints the owner address before collecting inputs).
  Cargo.toml path-deps → `../../shared-early-finance/tree/*`.
- `scripts/chaoslib.py` — devnet-chaos copy; BIN paths → shared binaries +
  stock worker; ports 21000-21019; PID-file process model (stop-by-PID only).
- `scripts/run_experiment.py` — the full driver (§4). Untested beyond
  import-level review (no binaries existed to run it against).
- `tree/` — SHELVED scaled-fallback tree (pin + 10/24 + log-8 genesis).
- `logs/intentool-build.log` — successful (stale) intentool build log.
- Nothing running: run/ has no PID files; no lane processes alive at wrap-up.

## 6. Open questions

- The effective per-block user budget at runtime (calibrate in Part 1; the
  fixture said 63 native user pages) — sizes the pressure set (2× budget + margin).
- halt-reorg's state: how many of the last 35 blocks are ≥75% at handover
  (P* derivation handles it live); the state's block cadence/difficulty at
  takeover (fast blocks compress the prep margin — stop the worker to freeze
  the tip if needed; note splits need confirming blocks, so prep happens
  while mining toward P*−1).
- Whether the fixture wallet in reorg's state dir has spendable UTXOs
  (walletGetBalance at takeover) — if yes, pressure prep is trivial.
- Run on hbox vs Mac state copy (state size at decision time; the driver
  assumes a LOCAL node — for hbox, copy the state to the Mac or adapt the
  driver to ssh-driven process management).
- The pinned checkout (experiments/parano1d-break/parano1d) is DIRTY:
  params.rs 10/24 + five rustfmt-only files (root/chaos-latch's doing).
  Do not "clean" it.

## 7. If resuming fresh

1. Read this file, then `experiments/verification-stale-fee/RESULT.md` (C35)
   and `experiments/FINDINGS.md` rows C35/C29/C33/C37.
2. Check `ls ../shared-early-finance/bin/` — if empty, rerun
   `bash ../shared-early-finance/build.sh` (the 22:27 failure is fixed by the
   relaxed assert already in its tree; expect ~40 min).
3. REBUILD intentool (`cd intentool && cargo build --release`) — required,
   chain-id correctness.
4. Run Part 1 (`--phases setup,control,calibrate`), then coordinate the
   big state with the root/halt-reorg, then Part 2 phases in order.
5. Discipline: PID-file kills only; no unfiltered `-p <crate>` test runs;
   commit NAMED files on dev; never `git stash`; the shared tree and reorg's
   state are other lanes' property — copy, don't modify.
