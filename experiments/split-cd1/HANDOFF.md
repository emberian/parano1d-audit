# split-cd1 lane handoff

## 1) Mission

Build a FULL HistoryStep proof for a block whose coinbase output uses an
out-of-domain `slot_index` (`s + k·2^log_slots`, aliasing leaf `s`; CD-1),
have it ACCEPTED by the production verifier (release packs, 64/64/16), and
demonstrate the snapshot-vs-sequential consensus split on release nodes. If a
real check kills the alias instead, document the exact file:line — a
first-class negative. Heavy execution (patched-prover build + proving) runs
on **hbox** (`/othersys/lanes/parano1d/split-cd1/`); the split devnet runs on
the Mac with stock `experiments/devnet-chaos/bin/parano1d` binaries.

## 2) Established facts (each with evidence)

- **The circuit binds slot bits with no active-depth range check.**
  `bind_paired_exact_state_transition` pins the local walk's
  `directions[0..16]` to `slot_bits[0..16]` and the upper walk's
  `directions[0..16]` to the 16-bit segment id (bits 16..32 of the 32-bit
  wire slot), but nothing forces `slot_bits[log_slots..32] == 0`. Tree:
  `noid_recursive/src/acceptance/block_slots.rs:641-720` (pinned copy:
  same lines). The compaction proves 32-bit strict-ascending uniqueness only:
  `noid_recursive/src/acceptance/trace/action_compaction.rs:106-119`
  (`range_check_bits(b, &action.slot_index, 32)`).
- **The orphaned range check exists as dead code.**
  `bind_structural_frontier_count_from_actions_dynamic`
  (`noid_recursive/src/acceptance/trace/exact_state.rs:961-1002`) contains
  the `slot_bits[d..32] == 0` pin; its only caller is a test
  (`exact_state.rs:1985`).
- **The alias arithmetic.** At `log_slots = 24`, wire index `s + 2^24` has
  low 16 bits = `s`'s (local walk to leaf `s mod 2^16`), bits 16..24 = `s`'s
  (upper walk levels 0..8 — the only levels pinned to the depth-24 roots via
  `select_upper_paired_roots`), bits 24..32 = the alias bits (upper walk
  levels 8..16, hashing beyond the root, pinned to nothing). Segment id
  becomes `(s>>16) + 256k`, distinct and strictly ascending, so the
  in-circuit segment compaction accepts the aliased row. 16-bit segment ids
  hold k ≤ 255 at log_slots 24.
- **Sequential nodes reject the alias.**
  `noid_chain/src/consensus/validation.rs:189/199` —
  `ConsensusError::ShapeMismatch("live input/output slot is outside block
  log_slots")`. This file is UNPATCHED in our tree (byte-identical to
  pinned) and is what release nodes run.
- **Ordinary witness construction rejected the alias at three host-side
  guards** (now patched in our tree copy only):
  `noid_chain/src/state_delta.rs:214/254` (`SlotOutOfRange`),
  `noid_chain/src/sparse_merkle.rs:1088-1118` (`validate_index` /
  `IndexOutOfRange` + level-0 positions from raw indices),
  `noid_chain/src/state.rs:1127/1169` (apply path `SlotOutOfRange`).
- **Producer commit does not re-validate.**
  `commit_locally_proved_next_block`
  (`noid_chain/src/storage/mdbx_context.rs:1227-1316`) checks only the
  prepared post-state bindings + PoW; it installs the producer's state image.
- **Snapshot boundary math (release nodes).**
  `newest_deterministic_snapshot_boundary(peer_height) =
  floor((peer_height - 18) / 6) * 6` (`noid_node/src/main.rs:243-247`,
  `CONSENSUS_FINALITY_DEPTH=18`, `SNAPSHOT_BOUNDARY_INTERVAL=6`). A fresh
  node full-syncs while boundary == 0 (tip ≤ 23); it snapshot-installs when
  boundary > local height. A live node re-plans a snapshot when
  `boundary(peer_tip) > local_height` (`main.rs:11367-11380`).
- **Historical state service uses undo logs, not body re-validation.**
  `UNDO_RETENTION_DEPTH = 36` (`noid_chain/src/consensus/params.rs:250`);
  snapshot generation export from tip to boundary is undo-window bound
  (`noid_chain/src/storage/snapshot_generation.rs:495-519`). A serving node
  can therefore serve the aliased chain's state without ever re-running
  sequential validation on the aliased body.
- **The proven working template for genuine terminals** is
  `experiments/verification-wallet/src/full_history.rs` + `src/bin/
  full_terminal.rs` (release packs from devnet-chaos dirs; terminal verified
  by a second node's `runtime.verify_terminal`; `full-inbound-run.log`).

## 3) Ruled out + why

- **Break 1B (intra-block mint→spend via alias) via the production host
  audit:** `exact_action_surface_from_surface` keys leaves per WIRE index
  (fine, 3 rows), but the frontier plan/evaluation
  (`derive_structural_frontier_plan` + `evaluate_structural_frontier`) is
  per-POSITION: wire indices 7 and 7+2^24 mask to the same position and the
  BTreeMap dedupe drops a row, so the old-root evaluation fails. The CIRCUIT
  would accept a 1B witness (per-row leaves), but building it needs a
  sequential-multiproof host variant. 1A (aliased coinbase output) is the
  primary PoC; 1B documented as circuit-level analysis only.
- **Persistent split with an EXTENSION aliased block:** impossible by the
  boundary algebra — the joiner's boundary must exceed the aliased height,
  but then the rejecting node's own height is below that boundary and it
  re-snapshots past the alias. The FORK design (below) decouples them.
- **Patching any schedule/domain constants:** ruled out by ember. Our patches
  are witness-constructor masking + the miner's own coinbase slot choice
  (`NOID_CD1_ALIAS_K` env, default 0 = pinned behavior) — code, not config.

## 4) Current plan and exact next commands

Topology (all stock release binaries on the Mac; the ONLY patched artifact is
the fixture prover on hbox):

```
S_H = stock parano1d + injected honest-chain dir (tip 7)
S_F = stock parano1d + injected fork-chain dir   (tip 24; fork = honest 1..5,
     ALIASED block 6, honest 7..24; out-works S_H)
A   = stock parano1d, fresh, seeded [S_H] -> full-syncs (boundary(7)=0) to 7;
     then restart seeded [S_H, S_F] -> reorg attempt -> fetches fork block 6
     -> ShapeMismatch -> STAYS AT 7 (boundary(24)=6 <= 7, no re-snapshot)
B   = stock parano1d, fresh, seeded [S_F] -> boundary(24)=6 > 0 -> SNAPSHOT
     -> verifies the ALIASED block-6 terminal at the boundary -> installs ->
     suffix-syncs 7..24 -> AT 24
SPLIT: A at 7 (honest) vs B at 24 (fork), both release binaries.
```

Chain heights are minimal: fork tip ≥ 24 (joiner boundary > 0), aliased
block ≤ boundary, honest chain ≥ boundary so A never re-snapshots.

Patch set (tree copy `experiments/split-cd1/tree/`, all marked
"CD-1 alias witness (split-cd1 lane)"; all are no-ops for in-domain slots):

1. `noid_chain/src/consensus/template.rs` — coinbase slot += k<<log_slots
   when `NOID_CD1_ALIAS_K` set (the miner's slot choice).
2. `noid_chain/src/state.rs` — `apply_tx_checked_deferred_root` masks
   input/output wire indices to the effective leaf for checks/lookups/deltas.
3. `noid_chain/src/state_delta.rs` — drops the two `SlotOutOfRange`
   rejections; pre-state lookups via the masked leaf (overlay keyed by leaf).
4. `noid_chain/src/sparse_merkle.rs` — `effective_leaf(index, depth)`
   mask at every level-0 position site: `validate_indices` (returned set),
   `derive_structural_frontier_plan`, `expand_multiproof_sequential_updates`
   (node keys + per-index position derivation), `reconstruct_root`.
   `validate_index` relaxed to depth-check only.
5. `noid_recursive/src/acceptance/trace/exact_state.rs` —
   `paired_upper_witness` pads directions at levels ≥ active_upper_depth from
   `update.index` bits (the alias bits; zero for honest blocks).
6. `noid_chain/src/storage/mdbx_context.rs` — `segment_ids_for_block`
   masks wire indices before deriving segment ids.

Next commands (hbox, all builds under `swarm-build` with SWARM_MEM_MAX=8G;
PID files for detached jobs; never pkill):

```sh
# 1. ship the lane to hbox (from the Mac):
rsync -a /Users/ember/dev/parano1d-break/experiments/split-cd1/ \
  hbox:/othersys/lanes/parano1d/split-cd1/
# packs already at hbox:/othersys/lanes/parano1d/packs/ (root rsyncs them)

# 2. build the fixture (patched prover) on hbox:
ssh hbox 'cd /othersys/lanes/parano1d/split-cd1/fixture && \
  cp ../tree/Cargo.lock Cargo.lock && \
  SWARM_MEM_MAX=8G swarm-build cargo build --release --offline'

# 3. run the fixture on hbox (produces honest-chain/ + fork-chain/ + artifacts):
ssh hbox 'cd /othersys/lanes/parano1d/split-cd1 && \
  SWARM_MEM_MAX=8G swarm-build fixture/target/release/split-cd1-fixture \
  2>&1 | tee run/fixture-run.log'

# 4. rsync artifacts + the two chain dirs back to the Mac, then run the
#    split devnet (ports 21120-21139) with devnet-chaos/bin stock binaries;
#    reject-control builds on the Mac against comp-system-hunter/pinned.
```

The fixture (needs rewriting to the two-chain design, see §5) must: produce
honest 1..5, copy the dir, extend honest 6..7 (block 6 = the honest TWIN),
extend the fork copy with the ALIASED block 6 then honest 7..24, assert
`verify_terminal` accepts the aliased terminal, assert
`validate_block_checks` rejects the aliased block (ShapeMismatch), assert
the twin and aliased state_roots are IDENTICAL, and dump artifacts.

## 5) Artifacts on disk + how to run/verify each

- `tree/` — the patched tree copy (pinned 52938433 + the six patches above).
  Verify patches: `git -C tree diff --stat` is not possible (no .git);
  `grep -rn "CD-1 alias witness" tree/` lists every patch site.
- `fixture/` — the local-PoC crate (path deps into `../tree/`).
  Run: see §4 step 3. Currently `src/main.rs` still has the ORIGINAL
  single-chain design (honest block 1, aliased block 2 + twin chain 3) —
  it works for the core PoC but must be rewritten to the H/F two-chain
  design for the devnet injection (heights 5/6/7 and 24, see §4).
- `reject-control/` — replays saved blocks through the UNMODIFIED pinned tree
  (`comp-system-hunter/pinned`); stub terminal callback proves rejection
  happens before terminal verification. Run on the Mac:
  `cd reject-control && cp ../tree/Cargo.lock Cargo.lock && cargo run --release --offline`.
  Expects `../run/artifacts/block1.bytes`, `block1-terminal.bytes`,
  `block2-aliased.bytes`, `block2-aliased-terminal.bytes`,
  `block2-twin-honest.bytes`, `block2-twin-honest-terminal.bytes`.
- `run/` — outputs (chains, artifacts, logs). Empty so far.
- Pack dirs (Mac, read-only reuse): `experiments/devnet-chaos/packs/
  history-step-v1`, `experiments/devnet-chaos/downloads/{v2,v21}-pack/...`;
  on hbox: `/othersys/lanes/parano1d/packs/`.

## 6) Open questions / risks

- **MDBX data-dir portability hbox (Linux x86-64) → Mac (aarch64 macOS):**
  believed fine (byte-oriented, same page size, no float keys) but MUST be
  tested as soon as the first chain dir exists — open it with the stock
  binary on the Mac and `getChainInfo`. Fallback: run the devnet on hbox
  (needs Linux stock binaries) or re-verify.
- **A stock node opening an injected chain dir:** expected to trust its DB
  (no startup re-validation of history); must be verified with the first
  short chain. Fallback: patched serving node on hbox.
- **Does B's joiner install survive `validate_snapshot_install_selection`
  (work comparison)?** B sees only S_F, so yes; verify in the log.
- **Retirement-origin file for the runtime on hbox:**
  `full_history.rs::mainnet_retirement_origin_path` points at the Mac's
  `comp-system-hunter/pinned/noid_node/assets/mainnet-retirement.origin`;
  the rsync of split-cd1/ does NOT include it — copy it to
  hbox lane dir too (or point the path at /othersys/lanes/parano1d/...).
- **hbox pack layout:** confirm the rsynced pack paths match
  `experiments_dir().join("devnet-chaos/...")` expectations — on hbox the
  fixture's `experiments_dir()` resolves under the lane dir, so packs must
  sit at `<lane>/../devnet-chaos/packs/...` or the paths must be adjusted
  (likely: symlink `<lane>/experiments/devnet-chaos` -> the packs dir).
- **Per-block proving time** unknown until the first run; 26-27 blocks total.
  If it exceeds ~3 h, trim the fork to tip 24 exactly (already the minimum).

## 7) If resuming fresh

Read, in order: this file; `experiments/comp-consensus-diff/REPORT.md`
(CD-1 section); `experiments/verification-wallet/README.md` (the proven
terminal pipeline); `~/swarm/MACHINES.md` + `~/swarm/RULES.md` (hbox is
binding). Then: check the hbox build (below); once green, run the fixture on
hbox; then the Mac devnet. The single decisive artifact is
`run/artifacts/summary.json` + `run/fixture-run.log` (terminal ACCEPT +
sequential REJECT + twin-root equality), then the devnet split logs under
`run/`.

## 8) Final state at lane wrap (2026-11-15/16)

- **Code complete and building on hbox.** The fixture was REWRITTEN to the
  two-chain design (§4) — `fixture/src/main.rs` produces honest 1..5, cold
  -copies the dir to `fork-chain/`, mines the ALIASED fork block 6
  (`NOID_CD1_ALIAS_K=1`) plus honest 7..24, then the honest TWIN 6 + block 7
  on `honest-chain/`, asserts twin/aliased state-root equality, runs
  `runtime.verify_terminal` on the aliased terminal (ACCEPT), and
  `validate_block_checks` + a peer-chain `apply_next_block` on the aliased
  block (ShapeMismatch REJECT). `reject-control/src/main.rs` replays the
  saved blocks through the UNMODIFIED pinned tree.
- **hbox build COMPLETE AND GREEN (nothing running):** the fixture binary is
  built at `/othersys/lanes/parano1d/split-cd1/fixture/target/release/
  split-cd1-fixture` (release, offline, `swarm-build` with SWARM_MEM_MAX=8G
  CARGO_BUILD_JOBS=8; log at `fixture/build.log` — note the log's two
  `^error` lines are from the FIRST attempt before a one-line
  `Ok(()) -> Ok(_)` fix in main.rs; the final state is `Finished release
  profile`). The whole patched tree (all six patch sites across noid_chain +
  noid_recursive) compiles clean. A relaunch helper exists at
  `fixture/launch-build.sh` (writes `build.pid`). Disk on /othersys held at
  36G free.
- **Fixture run command (hbox, after build):**
  `cd /othersys/lanes/parano1d/split-cd1 && NOID_SPLIT_CD1_PACKS_ROOT=/othersys/lanes/parano1d/packs SWARM_MEM_MAX=8G swarm-build fixture/target/release/split-cd1-fixture 2>&1 | tee run/fixture-run.log`
  (make `run/` first; it writes `run/honest-chain/`, `run/fork-chain/`,
  `run/peer-chain/`, `run/artifacts/`).
- **Then, on the Mac:** rsync `run/` back; build+run `reject-control`
  (small, no packs: `cd reject-control && cp ../tree/Cargo.lock Cargo.lock
  && cargo run --release --offline`); run the §4 devnet split with stock
  `experiments/devnet-chaos/bin/parano1d` binaries (ports 21120-21139, PID
  files, never pkill).
- **Verified so far (compile-level):** the FULL patched tree plus fixture
  binary compile clean in release against the pinned 52938433 sources. NOT
  yet run: no terminal has been proved with the alias; the
  acceptance/rejection prints are designed, not observed. Claims discipline:
  treat §2 as source-verified facts only; the PoC claims await the fixture
  run (§8 run command).
- **Devnet script not yet written.** Model it on
  `experiments/devnet-chaos/scripts/phase8_live_c26.py` + `chaoslib.py`
  (Node/Managed, PID files, rpc(getChainInfo)); topology in §4. The two
  serving nodes get the injected chain dirs as their `--data-dir`.
- **No processes left running anywhere by this lane** (the hbox build
  completed green before wrap; PID file and log remain for reference).
