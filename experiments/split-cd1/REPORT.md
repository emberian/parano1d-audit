# split-cd1: CD-1 coinbase-slot alias — fixture verdict and release-node split

Lane `followup-split-cd1` (parano1d devnet-chaos campaign). Predecessor: commit
`f998c90` (source-verified CD-1 analysis + built fixture). This lane ran the
decisive fixture on hbox, fixed two blockers it exposed, and drove the
release-node split.

## Status

- [x] Tree schedule corrected to true pinned 52938433 (see "Deviation found")
- [x] Producer-side alias mask added (see "Deviation found: producer re-validates")
- [x] Fixture run on hbox (release packs 64/64/16) — ALL asserts held
- [x] Reject-control on the unmodified pinned tree (Mac) — green
- [x] Release-node split (`split_devnet.py`) — SPLIT OBSERVED
- [x] Committed

## Deviation found: the tree copy carried an early-finance schedule

The first fixture run failed at startup:

```
Error: v2 runtime fork schedule differs from this executable
```

(`noid_miner/src/history_protocol.rs:73`, the v2-pack schedule assert.)

Cause: `experiments/split-cd1/tree` was copied from the shared upstream
working tree (`parano1d-break/parano1d`), which is dirty — its
`noid_chain/src/consensus/params.rs` carries the early-finance schedule
(`MAINNET_V2_ACTIVATION_HEIGHT = 10`, `MAINNET_V2_1_ACTIVATION_HEIGHT =
Some(24)`). True upstream `52938433` (verified via `git show` in that repo,
and matched by the clean archive `experiments/comp-system-hunter/pinned`) says
`210_537` / `Some(279_370)`. The release packs match the true schedule (the
same pack set proved terminals in `experiments/verification-wallet`).

Five further files differed from the pinned archive by rustfmt only (import
order / line wrapping; `noid_ivc_core` c1.rs + field_circuit.rs,
`noid_tx/native_assets.rs`, `noid_node/build.rs`, `noid_gui` discovery.rs).
All six were reverted to the pinned versions; after the revert
`diff -rq tree pinned` lists exactly the patch files below and nothing else.

This also matters for the split itself: with v2@10 the fork's blocks 10..24
would have been v2 blocks that stock release nodes (true schedule) reject for
a second, confounding reason. With the true schedule every block 1..24 is
legacy (v1.1 activates at 95,125), matching the HANDOFF design.

## Deviation found: the producer re-validates its own template

The second fixture run mined the honest prefix 1..5 green, then the ALIASED
fork block 6 died inside `PreparedBlockAttempt::prepare`:

```
Error: aliased fork block 6
Caused by:
    PreparedBlockAttempt::prepare: block consensus failed: ShapeMismatch: live output slot is outside block log_slots
```

The producer path (`noid_block/src/history_step_witness.rs`,
`PreparedHistoryStepWitness::finish_template`) re-runs
`validate_block_checks_template` on its own template before proving — the same
sequential slot-domain check (`noid_chain/src/consensus/validation.rs:201`)
that peers use to reject the alias. The six-patch set had left
`validation.rs` deliberately unpatched (it IS the reject mechanism), so the
fixture's miner could never build the aliased block.

Witness construction itself (state application patches, exact-state frontier,
`HistoryStepBlockInput::try_new`) had already succeeded for the aliased
template — only the producer's own re-validation rejected it.

The check fires earliest in the producer's resource preflight:
`validate_nonce_independent_block` -> `validate_block_resource_preflight`
-> `validate_block_resources_with_stream` (validation.rs:163-215, the
slot-domain loop whose ShapeMismatch text is exactly what peers print). An
instrumented run confirmed the three template-validation call sites are never
reached while the preflight rejects first.

Fix: a seventh, env-gated producer-side patch in
`noid_block/src/history_step_witness.rs` (`cd1_producer_alias_allowed`):
when `NOID_CD1_ALIAS_K >= 1` (the same env var that selects the aliased
coinbase slot) the producer skips its OWN slot-domain ShapeMismatch — at the
resource-preflight call AND at the three template-validation call sites
(`validate_block_checks_template` / `validate_block_checks`); every other
consensus error still propagates. Peers, the joiner, the fixture's direct
`validate_block_checks` assert, the peer-chain `apply_next_block` assert,
and the pinned-tree reject control all run the unmodified `noid_chain`
validation. In-domain slots never raise this error, so honest blocks are
unaffected even with the env set.

## Patch set (tree = pinned 52938433 + these files, verified by full diff)

1. `noid_chain/src/consensus/template.rs` — coinbase slot += k*2^log_slots
   when `NOID_CD1_ALIAS_K` set (miner's slot choice).
2. `noid_chain/src/state.rs` — effective-leaf masking in the apply path.
3. `noid_chain/src/state_delta.rs` — drop `SlotOutOfRange`; masked pre-state.
4. `noid_chain/src/sparse_merkle.rs` — `effective_leaf` mask at every
   level-0 position site.
5. `noid_recursive/src/acceptance/trace/exact_state.rs` —
   `paired_upper_witness` pads directions at levels >= active depth from
   `update.index` bits (zero for honest blocks).
6. `noid_chain/src/storage/mdbx_context.rs` — `segment_ids_for_block` masks
   wire indices before deriving segment ids.
7. `noid_block/src/history_step_witness.rs` — producer-side alias mask
   (this lane; see above).

## Conditions

- Pinned sources: upstream `52938433a30b185ab007afaa15166a632cf2e82e` (clean
  archive `experiments/comp-system-hunter/pinned` as reference).
- Prover/verifier: release packs 64/64/16 from
  `experiments/devnet-chaos/packs/history-step-v1` +
  `experiments/devnet-chaos/downloads/{v2,v21}-pack/...` (hbox mirror
  `/othersys/lanes/parano1d/packs`, all four metadata/retirement pins
  verified by the fixture's pinned decoders).
- Fixture binary: `/othersys/lanes/parano1d/split-cd1/fixture/target/release/split-cd1-fixture`
  (release, offline, built under `swarm-build`, `SWARM_MEM_MAX=8G`).
- Fork topology: honest chain 1..5->(honest twin 6)->7; fork = copy of the
  depth-5 prefix + ALIASED block 6 + honest 7..24; `NOID_CD1_ALIAS_K=1`
  only during the aliased block's production.

## Decisive asserts (fixture)

- twin block 6 and aliased block 6 produce the IDENTICAL `state_root`
  while their `tx_root`s differ (same effective leaf, different bodies).
- production `verify_terminal` ACCEPTS the aliased block-6 terminal
  (release packs; a verifier-accepted proof, not a witness assertion).
- `validate_block_checks` REJECTS the aliased block (ShapeMismatch, slot
  domain) on the same patched tree the producer used.
- the full inbound peer path (`apply_next_block` + terminal verification)
  REJECTS the aliased block (ShapeMismatch) and then ACCEPTS the honest twin.
- reject-control: the UNMODIFIED pinned tree replays honest 1..5 + twin 6
  green and rejects the aliased block (Mac, stub terminal callback).

Results — ALL HELD (hbox run, `run/fixture-run.log`, final attempt):

```
ALIASED fork block 6 committed in 8.019620498s
ALIAS CONFIRMED: wire slots 26528637 (aliased) and 9751421 (honest) produce the identical state root 10244a8437a90abc8969c3b857421d435a4a9642d38a5487a4bd38b4b43650d9
honest block 7 committed; honest chain complete
ALIASED TERMINAL ACCEPTED by production verify_terminal in 349.633838ms (release packs, 64/64/16)
peer chain synced the honest prefix 1..=5 through the full inbound path
ALIASED BLOCK REJECTED by sequential validate_block_checks: ShapeMismatch(live output slot is outside block log_slots)
ALIASED BLOCK REJECTED by the full inbound peer path: Consensus(ShapeMismatch("live output slot is outside block log_slots"))
control: honest TWIN ACCEPTED by the same peer chain that rejected the alias
split-cd1 fixture COMPLETE
```

- The aliased slot is exactly `9751421 + 1*2^24 = 26528637` (k=1, log_slots=24);
  the twin's deterministic slot selection landed on the same leaf 9751421,
  and the two blocks' `tx_root`s differ while their `state_root`s are
  byte-identical.
- The mask fired exactly twice in the aliased produce (resource preflight +
  finish-template validation), logged as `[cd1-mask] ... -> true`; it never
  fires for honest blocks (zero mask lines outside the aliased produce).
- Verdict: the release verifier ACCEPTS a terminal proving a block whose
  coinbase output slot is outside the block's own log_slots domain, while the
  same block is rejected by sequential consensus — CD-1 is real at the
  relation level, with a verifier-accepted proof (not a witness assertion).

Negative attempts preserved: `run/fixture-run-attempt1.log` (schedule
mismatch), `run/fixture-run-attempt2.log` through `-attempt4.log` (producer
slot-domain wall, with the instrumented run showing the mask unconsulted
before the preflight fix). `run/artifacts/summary.json` is the machine-
readable verdict; all block/terminal bytes are in `run/artifacts/`.

## Release-node split

Topology (HANDOFF section 4; all stock binaries
`experiments/devnet-chaos/bin/parano1d`, loopback ports 21120-21127):

```
S_H = stock + injected honest-chain dir (tip 7)
S_F = stock + injected fork-chain dir   (tip 24)
A   = fresh, seeded [S_H]   -> full-sync to 7 (boundary(7)=0);
      restart seeded [S_H,S_F] -> fetches fork block 6 -> ShapeMismatch -> stays at 7
B   = fresh, seeded [S_F]   -> boundary(24)=6 -> SNAPSHOT installs the aliased
      boundary block (terminal verified by the release verifier) -> suffix-syncs to 24
SPLIT: A@7 vs B@24, both release binaries, both serving.
```

Observed (stock binaries, loopback, ports 21120-21127; full transcript in
`run/split-devnet-console.log`, machine-readable verdict in
`run/devnet/split-summary.json`):

```
[evidence] S_H opened injected honest chain: height=7 tip=5f6966f30f8b631d818a5678b8271564fe10c6eab19c54cd2a0b2ffdab6bdf2b
[evidence] S_F opened injected fork chain:   height=24 tip=e7132819a4ba5fd14a7d825b5633b72bb5e6d827f2d54ec32a40524ff201f82f
[ok] A full-syncs the honest chain to 7: True
[evidence] A after fork exposure: height=7 tip=5f6966f30f8b631d... (unchanged)
[a-reject] ERROR reorg: failed to apply block height=6 err=Consensus(ShapeMismatch("live output slot is outside block log_slots"))
[a-reject] WARN  exact suffix rejected before canonical selection ... target_height=24 terminal_rejected=false
   (B's join:)
[b-path] DEBUG requesting HistoryStep terminal for snapshot verification ... height=6
[b-path] DEBUG received HistoryStep terminal from peer ... terminal_len=971732
[b-path] INFO  snapshot HistoryStep verification started off-thread
[b-path] INFO  (phase history_step_terminal count=1 bytes=971732 elapsed_ms=1839 outcome=accepted)
[b-path] INFO  snapshot authority accepted — starting exact state staging tip=6 segments=1
[b-path] INFO  snapshot boundary State installed snapshot_height=6 applied_height=6
[b-path] INFO  exact suffix terminal verified outside the chain writer height=24 elapsed_ms=193
[result] A at 7 (5f6966f30f8b31d...) vs B at 24 (e7132819a4ba5fd1...): SPLIT OBSERVED
```

Reading of the evidence:

- The stock node A attempts the reorg ("reorg: reverting height 7..5
  depth=2 new_blocks=19"), fails to apply the fork's block 6 at exactly the
  sequential slot-domain check, and stays at the honest tip; the suffix is
  rejected BEFORE canonical selection with `terminal_rejected=false` — the
  rejection is consensus-level, upstream of any terminal check.
- The stock joiner B negotiates a snapshot at boundary 6 (boundary(24) = 6
  by the release algebra), fetches the boundary terminal — 971,732 bytes,
  the byte length of `run/artifacts/f6-aliased-terminal.bytes`, i.e. the
  ALIASED block's terminal — verifies it off-thread with the release
  verifier (1839 ms, accepted), accepts the snapshot authority, installs
  the boundary state, and suffix-syncs to the fork tip 24.
- Both nodes serve RPC at the end: A at the honest tip, B at the fork tip.
  Two release binaries, one network, two heads — the CD-1 split.
- Reproduce: `python3 split_devnet.py` (requires `run/honest-chain` +
  `run/fork-chain` from the fixture and the devnet-chaos bin/ binaries).

## Controls

- Honest twin accepted by the same peer chain that rejected the alias
  (fixture) — the reject is alias-specific, not structural.
- Reject-control on the unmodified pinned tree — the rejection is the
  sequential check itself, not an artifact of the patches.
- `run/pin-chain` (reject-control's chain) accepts honest 1..5 then the
  twin, rejecting only the aliased block.

Reject-control observed (Mac, unmodified `comp-system-hunter/pinned`,
stub terminal callback so rejection must precede any terminal check):

```
pinned control: honest block 1 ACCEPTED by the unmodified pinned tree
... (blocks 2-5 likewise)
pinned control: ALIASED BLOCK REJECTED by the unmodified pinned tree: Consensus(ShapeMismatch("live output slot is outside block log_slots"))
pinned control: honest twin ACCEPTED by the same unmodified pinned tree
pinned control COMPLETE
```

## Remaining questions

- All three HANDOFF §6 risks were resolved by observation, not assumption:
  the MDBX chain dirs moved hbox (Linux x86-64) -> Mac (aarch64) and both
  serving nodes opened them (`getChainInfo` returned the injected tips); a
  stock node opening an injected dir trusts its DB (the network-storage
  marker self-repairs against the mainnet genesis at height 0); and B's
  snapshot install survived work comparison trivially (B saw only S_F).
- The fork's serving state came from the fixture-committed undo/segment data
  (S_F generated the boundary-6 snapshot on request; no body re-validation).
- What is NOT shown here: mainnet-cost economics (the fork needs to out-work
  the honest chain; both chains here use floor difficulty), and any route to
  1B (intra-block mint->spend through the alias) — that remains
  circuit-level analysis per the predecessor's HANDOFF §3.
- The eprintln diagnostic in `cd1_producer_alias_allowed` fires only when a
  producer error is being masked; it is part of the evidence trail
  (`[cd1-mask] ... -> true` lines in `run/fixture-run.log`).
