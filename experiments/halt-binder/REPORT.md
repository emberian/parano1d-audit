# halt-binder REPORT — CD-3/C36 at the live-daemon boundary

Lane `followup-binder` (predecessor staged the experiment in `1771dc8`,
`18f22f5`; source findings (a)(b)(c) are established there — see
`HANDOFF.md` §2). Run of 2026-11-15, 23:04 PST onward, on ember's Mac
(loopback devnet, ports 21021-21022, data dirs under `run/`, every process
PID-filed, stops by PID file only).

## 0. Headline

The campaign's canonical early-finance binary
(`experiments/shared-early-finance/bin/parano1d`, sha256
`20b3763f054090d0be3f217bde2dc50c8d8896ead80c74b073743cd8e5778db7`, built
by the root 2026-11-15 22:55) **cannot start a node at all**: every startup
exits with `Error: native runtime differs from the executable schedule`
(live log: `logs/hb-n1.log`). The root's build-time relaxations
(`root-v21-activation-fix.diff`) fixed `noid_node/build.rs` but the runtime
startup guard `with_finance_runtime`
(`noid_miner/src/history_protocol.rs:137-157`) still requires the v2.1 pack's
class `authorization_activation` (279_370) to equal the executable's
`OWNER_AUTHORIZATION_ACTIVATION_HEIGHT` (Some(24)) — the chaos-latch
relaxation covered the schedule comparison only, keeping "activation-equality
checks" strict.

With that one guard relaxed (lane-local rebuild, `scripts/build-early2.sh`,
patch in `tree-early/noid_miner/src/history_protocol.rs`), the node starts,
mines the legacy era, and **stalls at the v2 boundary (height 10)**:
the shipped mainnet packs pin their own fork boundary inside the bank config,
and the legacy origin certificate for an early fork is rejected
(`LegacyOriginCertificate::verify`,
`noid_recursive/.../v2/banked/origin_codec.rs:76-79`). [Live evidence for
this second layer: §3.2 — filled in by the run below.]

Consequence: the five-launch live experiment (5 graduating CreateLaunches
through the daemon's own mempool+selection at v2.1 heights) is **unreachable
with the shipped packs at any early-finance schedule**. The predecessor's
"pack wall" conclusion (HANDOFF §2, "the packs must match the schedule") is
confirmed live at two of its four layers. The (a)/(b)/(c) source findings
stand as committed; the live boundary needs isolated-schedule packs
(v2 + v21), not a params patch.

## 1. Conditions

- Node binary: `experiments/shared-early-finance/bin/parano1d` — the root's
  shared early-finance build (pinned commit `52938433` + chaos-latch params
  patch `MAINNET_V2_ACTIVATION_HEIGHT=10`, `MAINNET_V2_1_ACTIVATION_HEIGHT=Some(24)`
  + build.rs schedule-assert relaxations + history_protocol.rs schedule-guard
  relaxations + env-gated poison clock default-off). Embedded packs are the
  shipped **mainnet** packs (v2 bank `c2a6df73…`, v2.1 bank `6c280ae8…`),
  whose runtime metadata pins activation 210_537 / 279_370 — the build prints:
  - `v2 pack activation height Some(210537) differs from executable Some(10)`
  - `v2.1 pack V2 activation height Some(210537) differs from executable Some(10)`
  - `v2.1 pack authorization activation Some(279370) differs from executable Some(24)`
  (`experiments/shared-early-finance/logs/build-rerun2.log`.)
- External PoW worker + CLI: stock `experiments/devnet-chaos/bin/parano1d-miner`
  / `parano1d-cli` (fork-agnostic RPC clients), reused read-only.
- Devnet: single genesis node `hb-n1` (`--mode extminer`, `--mining-key`,
  `--log debug`), loopback 127.0.0.1:21021 (p2p) / 21022 (rpc); worker `hb-w1`
  (6 threads, mining key). Driver: `scripts/run_experiment.py` phases
  setup/control2/probe5/tight9/teardown over `scripts/haltlib.py`.
- Consensus at the executable: BLOCK_TIME 20 s (V2 30 s), ASERT halflife
  120 s, genesis target 2^238; V2@10, V2.1/owner-auth-V4@24
  (`tree-early/noid_chain/src/consensus/params.rs`).
- Launch plan per phase (unchanged from the staged driver): control2 = 2
  graduating; probe5 = 5 graduating (10 defs > capacity 9); tight9 = 4
  graduating + 1 non-graduating (9 defs = exactly capacity).

## 2. The four-layer pack wall (source; each layer blocks an early-finance devnet that uses shipped mainnet packs)

1. **Build-time asserts** — `noid_node/build.rs` compares each pack's
   schedule/activation to the executable. The root relaxed these to
   v1_1-height + block-time equality with warnings for the rest
   (`root-v21-activation-fix.diff`). Layer 1 passes: the binary builds.
2. **Startup guards** — `noid_miner/src/history_protocol.rs`:
   - constructor (line ~62) and `with_finance_runtime` (~137): schedule
     comparison relaxed by chaos-latch (v1_1 + block time only), but
     `with_finance_runtime` kept `config.class(Small).authorization_activation()
     != params::OWNER_AUTHORIZATION_ACTIVATION_HEIGHT` strict. Pack 279_370 vs
     executable Some(24) → `Err("native runtime differs from the executable
     schedule")` → daemon exits at startup. **Live-confirmed (§3.1).**
3. **Fork-origin boundary** — at the v2 fork block the miner seals the
   legacy origin; `LegacyOriginCertificate::verify`
   (`noid_recursive/.../v2/banked/origin_codec.rs:76-79`) requires
   `parent.height + 1 == next.config().activation_height()` where `next` is
   the pack bank (210_537). An early fork at parent height 9 gives 10 ≠
   210_537 → `V2Error::Boundary` → no template at height 10 can be prepared.
   **Live-confirmed (§3.2).**
4. **Proving-relation origin pin** — even past 3, the frozen relation pins
   the origin accumulator's height lane to `runtime.bank.config().
   activation_height() - 1` (capacity_banked/assembly.rs ~165-171 and
   v2.rs:622-636), i.e. 210_536 in the pack vs 9 on an early devnet.
   Source-level only (layers fire in order; 3 precedes 4).

None of these is a "bug" in the executable — they are the pack
authentication doing its job: the shipped packs *are* mainnet packs. An
early-finance devnet at 10/24 therefore needs **isolated-schedule packs**
(v2 via the `bench_prover noid_v2_capacity joint` route — note it hardcodes
V1.1@5, which also needs matching the executable's V1.1@95_125; v2.1 needs
a new generator driver around `encode_v21_runtime_metadata`
(`noid_miner/src/v21_artifacts.rs:116` has no in-tree pack writer) plus
matrix freezing for the three classes incl. the 206-page Large). Cost
estimate from the predecessor: ~19 min for a v1-scale matrix; v2.1 likely
hours on this laptop.

## 3. Live observations

### 3.1 Shared binary (attempt 1, 23:04-23:07 PST)

`python3 scripts/run_experiment.py --phase setup` — node start, then:

```
===== START hb-n1 2026-11-15 23:04:20 =====
07:04:21 DEBUG preflight-authenticated HistoryStep runtime images loaded from the executable embedded_matrix_mib=18
Error: native runtime differs from the executable schedule
```

(`logs/hb-n1.log`; process exit before RPC; driver failed
`timeout waiting for hb-n1 RPC ready; last=<urlopen error [Errno 61]
Connection refused>` — `run/results.json` setup.fatal.) The images load;
the exit is exactly the `with_finance_runtime` guard.

### 3.2 Patched binary (attempt 2)

[LIVE SECTION — filled in after the rerun: heights 1-9 legacy mining, the
height-10 template failures with the exact Boundary/origin error lines, and
the worker's view. If the node instead crosses 10/24, this section records
that refutation and the launch phases run as planned.]

## 4. Verdict on (a)(b)(c) and CD-3/C36 severity

[PENDING the run — expected: (a) selection-cap 4 and admission-uncapped
remain source-verified (HANDOFF §2a), (b) proving sites confirmed in
source, (c) caught-JoinError confirmed in source; live five-launch
confirmation blocked by the pack wall. If §3.2 shows the stall: CD-3/C36
stays a producer-side caught-panic robustness finding that the honest daemon
cannot even reach with its own mempool — the campaign's live answer needs
isolated-schedule packs or a mainnet-height devnet (>= 210_537 blocks).]

## 5. Controls

- Shared binary hash recorded (§0) and re-verified before install
  (`shasum -a 256` match with `shared-early-finance/bin/parano1d`).
- The lane-local rebuild changes exactly one guard (§2 layer 2); the patch is
  in `tree-early/noid_miner/src/history_protocol.rs` with an in-source
  comment; the build recipe is `scripts/build-early2.sh` (same pack/retirement
  env as the root's `build.sh`).
- No pattern kills: all stops by PID file (`run/*.pid`), RPC `paranoid_stop`
  first.
- `run/results.json` carries per-phase machine-readable results incl. the
  attempt-1 root cause annotation.
