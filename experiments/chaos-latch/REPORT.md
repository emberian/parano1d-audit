# chaos-latch REPORT — production-scale fee-floor latch (1,024-entry pool)

Lane: `followup-latch` (successor of the chaos-latch staging lane, HANDOFF.md).
Status: IN PROGRESS — phases running; live numbers filled in as they land.

## 1) Claim under test

C29/C33 scaled to production: the parano1d mempool fee floor
`floor = max(5,000, 0.9 × median(last 50 admitted fees))` can be latched by an
attacker occupying the pool with intents that do not record fees, blacking out
every financial operation that must pay EXACTLY the floor (no tipping), at the
live daemon's real capacity (1,024 entries / 384 MiB), and at a cost far below
Gemini's unproven NM-2B estimate (26 transfers / 0.578 NOID).

## 2) Conditions

- Binary: `experiments/shared-early-finance/bin/parano1d` (shared early-finance
  build; commit 52938433 lineage + campaign patch set, see §4). Production m24
  domain, `--mode node`, `MempoolConfig::default()` → `MAX_MEMPOOL_TXS = 1024`.
- External miner: `experiments/devnet-chaos/bin/parano1d-miner` (stock), 8
  threads, mining key M1. Loopback devnet, ports 21060-21079, genesis node n1,
  gossip peer n2, fresh joiner n3 (phase join).
- Schedule patch: `MAINNET_V2_ACTIVATION_HEIGHT = 10`,
  `MAINNET_V2_1_ACTIVATION_HEIGHT = Some(24)` (30 s blocks at V2, V2.1
  financial ops from h24), applied in the shared tree
  (`experiments/shared-early-finance/tree/noid_chain/src/consensus/params.rs`).
- Attack intents: `experiments/chaos-latch/factory/target/release/chaos-factory`
  — real authorized claims built against live chain state over the node's own
  JSON-RPC (ordinary 1-in/1-out exact-fee self-transfers; unfillable V4
  TradeOwnerIntents with fee_limit 6,600 µNOID and gross input 1 µNOID —
  admitted but never fillable, occupying pool entries without recording fees;
  CreateLaunch / CreateCollection / MintNft / CreateListing / Purchase in the
  shipped claim layouts).

## 3) Code path (verified in source, pinned checkout 52938433)

- Floor: `noid_mempool/src/floor.rs:104-113` — median of last 50 admitted
  fees, `max(5_000, 0.9 × median)`.
- Latch: `floor.rs:60-89` — engages at count×5 ≥ cap×4 (≥ 820/1,024) or bytes ≥
  80%; resets only at count==0 or both count and bytes < 50% (< 512).
- Trades occupy entries but SKIP fee recording: `noid_mempool/src/pool.rs:536-549`
  (`if !is_trade { st.floor.record(fee) }`).
- Financial intents must pay EXACTLY the deterministic minimum — no tipping:
  `noid_mempool/src/native.rs:162`; floor gate at `native.rs:167-175` rejects
  with `consensus: BelowMinFee: required=N actual=M`. A latched floor is a hard
  blackout for these ops; fee schedule `noid_chain/src/consensus/params.rs:433-447`
  (5,000 + 100/in + 700/out + 2,500/net-new-slot → CreateLaunch 8,200;
  CreateListing 6,600; Purchase 9,800 at low occupancy).
- Observable live: `paranoid_getMempoolInfo` → `{size, fee_floor, txs}`.

## 4) Shared-binary runtime guard fix (this lane's unblocking work)

The staged shared binary (built 2026-11-15 22:55) crashed every node at
startup: `Error: native runtime differs from the executable schedule`
(`latch-n1.log`). Root cause: the shipped v2.1 pack encodes the MAINNET
authorization activation `Some(279_370)` in every class bank (proved by the
build warning `v2.1 pack authorization activation Some(279370) differs from
executable Some(24)` in `shared-early-finance/logs/build-rerun2.log`), while
the early-finance executable activates at 24. The build-time assert had already
been relaxed to a warning (root follow-up in `noid_node/build.rs`), but the
RUNTIME startup guard `noid_miner/src/history_protocol.rs:149-153` still
compared them exactly. Fix (this lane): same relaxation pattern at runtime —
keep the (v1_1 height, V2 block time) equality and the "executable must
schedule an activation" requirement, drop the pack-activation equality with a
`tracing::warn`. Rebuilt via `shared-early-finance/build.sh`
(`logs/build-rerun3.log`).

Note the HANDOFF's reference for "the pack's per-class activation is 24"
(phase-4 isolated build) was wrong twice over: the phase-4 isolated binary
embedded NO v2.1 pack at all (`GENERATED_V21_PACK = None` in
`devnet-chaos/target-linux/isolated/.../out/v21_pack.rs`), and the pack's real
class activation is 279_370, not 24.

## 5) Run log (live numbers)

(To be filled by phase.)

## 6) Blackout evidence

(To be filled: exact BelowMinFee rejection lines from ≥2 wallets via ≥2 nodes.)

## 7) Minimal latch cost

(To be filled: 819 unfillable trades + 1 sample at 11,000 µNOID; intents,
µNOID, wall time; vs Gemini's 26-transfer / 0.578 NOID claim.)

## 8) Controls and limits

(To be filled: pre-latch mined financial ops; ordinary spends ≥ floor admitted;
fresh joiner; epoch-anchor rotation; scale labeling.)

## 9) Open questions

- Pack-embedded activation: the v2.1 pack's class banks pin their activation
  (279_370) into proof traces (`BlockRelationProfile::ScheduledV2(bank config)`
  in `capacity_banked/assembly.rs`; `before_activation` header pins in
  `block_slots.rs`; ORIGIN_ACC pin at `activation_height() - 1`). Whether
  financial blocks can PROVE at h24-h279369 with this pack is settled
  empirically by the preop controls below.
