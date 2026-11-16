# halt-binder HANDOFF

## 1) Mission

Determine whether a stock release parano1d daemon can hit the financial
definition-capacity assert (`bind_financial_definition_table`,
`noid_recursive/src/acceptance/trace/native_assets/financial_definitions.rs:276`)
while proving its own template — CD-3/C36 taken to the live-daemon boundary —
and if yes, demonstrate the daemon crash/stall live against the pinned mainnet
Native class (capacity 9; five immediately-graduating CreateLaunches = 10
definitions). If five launches cannot be driven through the daemon's own
mempool+selection, find the exact selection cap and demonstrate it live; that
precise guard is a first-class negative.

## 2) Established facts (with evidence)

Pinned checkout: `/Users/ember/dev/parano1d-break/parano1d` @ 52938433
(READ-ONLY; the checkout has reformat-only local drift in 5 files — build
from `git archive 52938433`, which is what this lane did).

### (a) Can the daemon's own selection ever pick > 9 definitions? — NO (exact guard)

- The Native-class selection budget is built from the class's bank config:
  `noid_miner/src/template.rs:652-676` (`v2_full_selection_budget`) sets
  `definitions: financial.kernel.definition_capacity` (line 659); applied per
  class at `template.rs:380-397` (`v2_selection_budget` → `trades::select_candidates`
  at `noid_miner/src/template/trades.rs:75-148`).
- Capacity formula: `noid_recursive/src/acceptance/history_step/v2/config.rs:174-175`
  — `definition_capacity = headers + trade.markets`; mainnet Native class =
  5 headers + 4 markets = **9** (bank digest 6c280ae8… authenticated by
  `experiments/verification-opcodes/`; the live node also exposes it via the
  `getNativeProtocol` RPC: `headers`, `markets`).
- Per-entry accounting counts DISTINCT auxiliary opening roots declared by the
  intent (inputs+outputs+dependencies): `noid_chain/src/mempool.rs:241-254`
  (`native_definition_roots`), `:783-836` (`select_for_block_matching` —
  `native.definitions = roots.difference(&definitions).count()` against the
  budget, `try_consume` at `:472-501`); admission stores the same usage
  (`MempoolEntry::from_native` mempool.rs:141-165 via
  `NativeResourceUsage::for_intent` mempool.rs:396-433).
- An immediately-graduating CreateLaunch necessarily declares exactly **2**
  auxiliary roots (LaunchDefinition = the op-48 operand + MarketDefinition via
  the market-pool carrier's dependency): forced by
  `noid_tx/src/native_assets/wire.rs:1031-1059` (`resolve_dependencies`) +
  `noid_tx/src/native_assets/closure.rs:52-82` (carrier roots must not be
  auxiliary; dependency roots are fetched transitively); the node wallet pushes
  both for a full initial buy at `noid_rpc/src/server/native/planner.rs:556-566`.
- The template payload openings are the DISTINCT required closure of the
  selected intents' declared openings — a subset of what the budget counted:
  `noid_miner/src/template.rs:495-528` (declared openings collected into a
  root-keyed map) → `noid_chain/src/consensus/template.rs:733-745, 686-687`
  → `noid_chain/src/consensus/template/native.rs:172-208` (`selected_data` →
  `resolve_native_closure`, output is a `BTreeMap` = distinct roots).
- The witness list is that payload filtered to auxiliary openings, sorted,
  NOT re-deduplicated, then passed to the asserted binder:
  `noid_recursive/src/acceptance/block_slots.rs:3090-3096` (build) and
  `:3108-3115` (call) → `financial_kernel.rs:143-161`
  (`bind_financial_sequential_preparation`, capacity =
  `config.definition_capacity`) → `financial_definitions.rs:398-406` →
  `:271-297` (`bind_financial_definition_table`, `assert!(capacity > 0 &&
  witness.len() <= capacity)` at `:276`).
- Therefore witness.len() ≤ selected distinct roots ≤ budget = 9. **The honest
  daemon's own mempool+template selection can never hand the binder more than
  `definition_capacity` definitions.** With 2 roots per graduating launch the
  mainnet Native class admits at most **4 graduating launches per block**
  (8 ≤ 9; a 5th would need 10 > 9). A 9th single-definition op (e.g. a
  NON-graduating launch: planner.rs:556-559 pushes only the
  LaunchDefinition — 1 root) can fill the budget to exactly 9/9.
- Admission itself has NO definition cap: `noid_mempool/src/native.rs:28-131`
  (`submit_native` checks size/canonical bytes/proof presence/fee/context and
  verifies the V4 owner auth; no class-budget check) — all five launch
  intents are admitted; the cap is selection-side only.
- Trade intents are accounted exactly too: assembled trade groups consume
  their new distinct auxiliary roots
  (`noid_miner/src/template/trades.rs:361-387, 507-542`;
  `trade_policy_resources` noid_chain/src/mempool.rs:357-386), so trades
  cannot open a gap.
- The wallet planner checks each op against a FRESH budget but does not
  account pending intents (`noid_rpc/src/server/native/planner.rs:1033-1036`)
  — the node wallet will happily prepare all 5 launches.

### (b) Where does witness binding run? — proving, not template build; at getBlockTemplate / internal miner; NOT at submitBlock

- The call chain: `PreparedBlockAttempt::prepare_inner`
  (`noid_miner/src/block_production.rs:123-310`) →
  `prepare_candidate_input_with_native_data` (candidate preflight — does NOT
  check definition capacity; `noid_block/src/candidate_history.rs`, the CD-3
  preflight omission) → `v21::prepare_for_pow`
  (`noid_recursive/src/acceptance/history_step/v2/capacity_banked/assembly.rs:526-546`)
  → `prepare_assembly` → `build_selected_zk_block_slots_core`
  (`noid_recursive/src/acceptance/block_slots.rs:2262`, definitions witness
  `:3090-3096`, binder call `:3108-3115`) → the asserted table binder.
- In the daemon this runs in exactly two places, both
  `tokio::task::spawn_blocking`:
  - external mining: `paranoid_getBlockTemplate` →
    `prepare_external_mining_attempt` → `noid_rpc/src/server.rs:1346-1356`
    (proves the nonce-independent HistoryStep BEFORE the worker gets the
    template; C15's "begin_proving runs before validate_pow" is this).
  - internal miner: `noid_miner/src/miner.rs:483-497`
    (`prepare_cancellable`).
- `paranoid_submitBlock` (`noid_rpc/src/server.rs:2537-2577`) does NOT
  re-prove: `begin_proving` only consumes the cached prepared attempt and
  seals the nonce (`ExternalMiningAttemptSlot::begin_proving` server.rs:529-555).
  A crafted block handed to the daemon is only ever VERIFIED (VK check),
  never re-proven, so the submit path cannot reach the assert either.

### (c) Does the assert kill the daemon? — NO: both proving sites are spawn_blocking; panic → JoinError, caught

- Release profile has no `panic = "abort"` (`Cargo.toml:47-49`: lto thin,
  codegen-units 1 only).
- getBlockTemplate: the JoinError is mapped to an RPC error at
  server.rs:1355-1356 ("HistoryStep preparation task failed").
- Internal miner: caught at `noid_miner/src/miner.rs:558-561` — logs
  "HistoryStep preparation task panicked" and retries the next template.
- So even a firing assert is a caught panic. A functional halt would need the
  SAME over-capacity template to be re-selected every round — the (a) guard
  makes that impossible via the daemon's own mempool.

### The early-finance build wall (my two build attempts; blocks the live run)

- Attempt 1 (missing retirement keys): release + scheduled V2 requires
  `NOID_RETIREMENT_KEYS_DIR`, `NOID_RETIREMENT_KEY_0_PIN`,
  `NOID_RETIREMENT_KEY_1_PIN` — `noid_node/build.rs:75-81`. Pins live in
  `experiments/devnet-chaos/downloads/v2-pack/history-step-pack-v2/pins.env`.
- Attempt 2 (with full mainnet pack env): panicked at
  `noid_node/build.rs:268-272` — "v2 artifact schedule differs from the
  executable profile: left (pack) v2@210537, right (executable) v2@10".
  The shipped mainnet packs store the schedule/activation in their runtime
  metadata and build.rs asserts pack == executable. Additionally:
  - `embed_v21_pack` asserts every class's `authorization_activation` equals
    `V2_1_ACTIVATION_HEIGHT` (build.rs:182-196) — mainnet v21 pack says
    279370, so a V2.1@24 build fails there even if the v2 pack is dropped.
  - Deeper, the frozen MATRICES bake the origin pin:
    `capacity_banked/assembly.rs:165-171` pins `cells[ORIGIN_ACC] ==
    config.activation_height()-1` (a pack-config constant; 279369 mainnet),
    and `bank.rs:133` seeds ORIGIN_ACC from the installed fork-origin
    boundary. On a 10/24 devnet the real origin heights are 9/23, so even with
    the build.rs asserts patched out, the first v2/v2.1 block cannot be
    proved/verified with mainnet packs. Patching `params.rs` alone is
    structurally insufficient — the packs must match the schedule.
- Correct fix: isolated-schedule packs. The v2 pack generator exists in-tree:
  `bench_prover` bin `noid_v2_capacity joint …`
  (`bench_prover/src/bin/v2_capacity_support/banked.rs`, requires cargo
  feature `isolated-v1-1-testnet`; hardcodes schedule V1.1@5 + V2@10).
  No in-tree generator writes the v21 pack
  (`encode_v21_runtime_metadata` `noid_miner/src/v21_artifacts.rs:116` has
  no caller outside build.rs) — a small driver would need to freeze the three
  v21 matrices + write metadata for the early schedule. Cost: matrix
  generation at v1-pack scale took ~19 min (devnet-chaos README); v21
  (3 classes incl. 206-page Large) is likely hours.
- Corroboration that no prior live run settles this: devnet-chaos's phase-4
  isolated-dialect node never ran (docker /proc mount failure —
  `experiments/devnet-chaos/logs/phase4-docker.log` tail), and devnet-chaos's
  own devnet never reached v2.1 (heights ~150 << 279370).
- The isolated cargo feature route (`isolated-v2-1-fork-testnet`, exactly
  10/24) is blocked on macOS by the startup guard
  `noid_node/src/main.rs:1735-1769` (`/proc/net/dev` must be lo-only).
  A poison-style params patch avoids the guard (guard fires only when the
  `isolated-v1-1-testnet` FEATURE is active) — the pack wall is the only
  blocker for the poison-style build.
- The root is building the canonical early-finance node at
  `experiments/shared-early-finance/bin/{parano1d,parano1d-cli}` with the
  mainnet pack fingerprint; unless it also regenerates packs (or relaxes
  build.rs AND accepts the runtime origin pin — which I believe it cannot),
  it will hit build.rs:268-272 exactly as my build did.

### Daemon/RPC details needed for the live run (verified in source)

- RPC auth: with `--mining-key` configured, loopback requests WITHOUT a
  bearer get Local scope (all methods); the mining bearer is restricted to
  getBlockTemplate/submitBlock (server.rs:3746-3813, 3846-3872).
- Wallet flow for launches: `paranoid_walletPreviewNative` →
  `paranoid_walletSubmitNative` (server.rs:1603-1616; internally re-prepares
  and re-proves the V4 owner auth via the node's own wallet,
  server/native.rs:205-309). Request shape (internally tagged):
  `{"action":"create_token","whole_tokens":"100000","metadata_hash":"<64hex>","initial_buy_units":"60000000000"}`
  — 100000 whole tokens is the minimum (LAUNCH_MIN_WHOLE_TOKENS,
  launch.rs:26); supply 1e11 units; sale_units = 3/5 = 6e10 (launch.rs:156-158);
  buying exactly sale_units graduates immediately (launch.rs:178-190).
  Cost per graduating launch ≈ 400 NOID curve cash + 100 NOID backing
  (asset_backing.rs:6,34-39) + fee ≈ 500+ NOID; a non-graduating launch
  (omit initial_buy_units) costs ~100 NOID + fee.
- `walletSubmitNative` requires a stable tip and stable active wallet address
  between preview and submit (server/native.rs:230-247) — stop the mining
  worker while submitting, then restart it.
- `paranoid_getBlockTemplate` response carries `n_txs`, `height`,
  `template_id`, `expires_in_seconds` (noid_rpc/src/types.rs:182-210);
  template TTL 30s (server.rs:59) — after a direct template probe, wait 31s
  before restarting the worker.
- Emission: pre-v2 50 NOID/block; from V2 activation 14.4 NOID to the miner
  (emission.rs:40-43, development_allocation.rs) — ~430 blocks to fund
  ~6.2 kNOID for all phases; devnet cadence measured ~3.6 s/block
  (devnet-chaos n1-pool log: height 150 at +9 min).
- `getMempoolSize` / `getMempoolEntry` / `walletNativePending` give the
  admission + confirmation timeline; `getNativeProtocol` reports the live
  class caps.

## 3) Ruled out (and why)

- **Five graduating launches selected by the daemon's own mempool**: ruled out
  by the exact guard in (a) — budget 9, 2 roots/launch, max 4 selected.
  (This is the first-class negative the task accepts.)
- **Over-capacity via submitBlock / a crafted 10-def block handed to the
  daemon**: submit only verifies (VK), never re-proves — the assert is
  producer-side only (b).
- **Under-declared intents** (declare 1 root, create 2): the payload is the
  REQUIRED closure resolved from DECLARED openings; a missing required root
  fails template build (missing-opening error), it cannot inflate the count.
- **Duplicate-root inflation**: payload openings are root-keyed (BTreeMap /
  `payloads.insert` with conflict check, template.rs:733-745); the witness
  list is the payload's, so no duplicates reach the binder.
- **Trades opening a budget gap**: trade groups consume their new distinct
  auxiliary roots exactly (trades.rs:361-387).
- **Panic killing the daemon**: spawn_blocking + no panic=abort (c).
- **Poison-style params-only build with the shipped mainnet packs**: fails at
  build.rs asserts, and even past them the origin pin makes the first v2/v2.1
  block unprovable (see 2, build wall).
- **Isolated cargo feature on macOS**: startup guard reads /proc/net/dev
  (main.rs:1735-1769).

## 4) Current plan and exact next commands

1. Wait for the root's canonical early-finance build
   (`experiments/shared-early-finance/bin/{parano1d,parano1d-cli}`); before
   trusting it for a v2.1 run, verify it actually embeds packs matching its
   schedule (a node whose packs mismatch will fail at the first v2 block at
   height 10 with "HistoryStep witness preparation failed"-class errors).
2. Install: `cp experiments/shared-early-finance/bin/parano1d
   experiments/halt-binder/bin/parano1d-early` (haltlib.py expects that
   path; worker + cli come from devnet-chaos/bin read-only).
3. Run the experiment (phases idempotent, results to
   `run/results.json`):
   ```sh
   cd /Users/ember/dev/parano1d-break/experiments/halt-binder
   python3 scripts/run_experiment.py --phase setup      # node+worker, mine to ~6.2 kNOID
   python3 scripts/run_experiment.py --phase control2   # 2 graduating: one block, proving fine
   python3 scripts/run_experiment.py --phase probe5    # 5 graduating: expect 4+1 over two blocks
   python3 scripts/run_experiment.py --phase tight9    # 4 graduating + 1 non-grad: 9/9 in one block
   python3 scripts/run_experiment.py --phase teardown
   ```
   `probe5`'s `template_probe` (getBlockTemplate with the mining bearer)
   exercises the daemon's own proving path directly; `confirmed_per_block`
   and `log_health` (panic/prove greps) are recorded per phase.
4. Write `REPORT.md` and commit named files on branch dev (unsigned OK).

## 5) Artifacts on disk

- `scripts/haltlib.py` — chaoslib-derived driver (PID files under `run/`,
  ports 21021-21022, stop only by recorded PID). Run: import by
  run_experiment.py.
- `scripts/run_experiment.py` — the full phase driver (setup/control2/
  probe5/tight9/teardown; see commands above). Syntax-checked
  (`py_compile` + `--help` pass). Not yet run (waits on the shared binary).
  Worker/node stops are by recorded PID only (`stop_worker_by_pid`,
  `_LiveNode`).
- `logs/build-early.log` — my failed build attempts' log (retirement-key
  assert, then the v2-pack schedule assert at build.rs:268-272 — the
  evidence for the pack wall).
- `bin/` — empty, awaiting the shared binary as `parano1d-early`.
- `run/`, `logs/` — data dirs for the experiment.
- `HANDOFF.md` — this file.

## 6) Open questions / risks

- Will the root's shared build embed EARLY-SCHEDULE packs? If it embeds the
  mainnet packs with patched params, it fails at build.rs:268-272 (as mine
  did); if it relaxes build.rs but keeps mainnet packs, the first v2 block
  (height 10) will fail to prove at runtime. Packs must be regenerated for
  the early schedule (v2 via `noid_v2_capacity joint`; v21 via a new small
  generator driver using `encode_v21_runtime_metadata` + matrix freezing).
- v21 pack generation cost (3 classes, 206-page Large) — possibly hours on
  this laptop. The project's public v2.1.0 GitHub release
  (github.com/ignotusnemo/parano1d, tag v2.1.0) publishes ONLY mainnet
  packs (history-step-pack-{v1,v2,v21}.tar.gz) — no isolated-schedule packs
  to download, so local generation is required.
- Whether a v2.1-active devnet at 30 s ASERT cadence slows the ~430-block
  funding phase (measured 3.6 s/block pre-v2 on devnet-chaos hardware; the
  V2-era target is 30 s but genesis-floor difficulty keeps early cadence fast).
- Fee magnitude for a ~35-input launch funding (the preview reports
  `fee_micronoid`; balance target 6.2 kNOID has margin).

## 7) If resuming fresh

Read, in order:
1. `experiments/halt-binder/HANDOFF.md` (this file),
2. `experiments/verification-definition-capacity/README.md` (prior component
   result),
3. `experiments/comp-consensus-diff/REPORT.md` § CD-3 (lines 164-186),
4. `experiments/FINDINGS.md` C36,
5. `experiments/devnet-chaos/README.md` § "PoC implementation and
   reproduction" (build fingerprints).

Then: check whether
`experiments/shared-early-finance/bin/parano1d` exists and (if the run is
to reach v2.1) whether it embeds early-schedule packs; if yes, install it as
`bin/parano1d-early` and run
`python3 scripts/run_experiment.py --phase setup` from
`/Users/ember/dev/parano1d-break/experiments/halt-binder`.
