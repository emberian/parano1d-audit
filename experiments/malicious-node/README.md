# Snapshot validation, fork replay, and version boundaries

Source: `52938433a30b185ab007afaa15166a632cf2e82e` (`52938433`), vendored under
`pinned/` via `git archive` (see `vendor.log`). `src/snapshot_header_staging.rs`
is the pinned `noid_node` module, blob-verified before every run by `run.sh`.
The tests call production validation, staging, and storage functions directly.
They did not run a node or peer protocol. The main result is a validation
difference: ordinary header admission rejects a future-dated intermediate
header, while snapshot staging accepts that sequence. Local install code
commits it when recursive terminal verification is stubbed. A valid release
HistoryStep terminal, competing live offers, and re-serving have not been
tested together.

## A. Snapshot staging omits intermediate future-drift checks

The coinbase-only test chain has 24 headers. Its height-12 header is dated ten
years ahead; later headers return to the local time window. Release-node
acceptance would also require a real-PoW fork, a genuine recursive terminal,
and selection above the joining node's HeaderDAG work. The component test shows:

1. **Ordinary admission rejects the chain.** `validate_header` with a local clock
   (the exact predicate `validate_bounded_header_extension` applies to every
   live-inventory header, `noid_node/src/snapshot_header_staging.rs:1049`, and
   that `validate_block_checks` applies to admitted blocks) returns
   `BadTimestamp` at h12. A fresh node syncing through native header admission
   therefore rejects this chain at that height.
2. **Snapshot staging accepts the identical headers.** The real
   `SnapshotHeaderStaging::append_batch`/`validate_complete` path validates
   every staged header with `validate_header_timeless_prehashed_parent`
   (`snapshot_header_staging.rs:972`), which excludes exactly one rule from
   `validate_header_inner`: local wall-clock future drift
   (`noid_chain/src/consensus/header.rs:96-156`).
3. **The node's drift gate is tip-only.** After staging, the node checks the
   manifest↔tip binding and `validate_history_step_tip_future_drift`
   (`noid_node/src/main.rs:9255-9269`), whose own comment states the design:
   "Historical header validation is timeless, but a snapshot must not make a
   locally far-future tip authoritative" (`main.rs:4308-4318`). Intermediates
   are never drift-checked on this path.
4. **The tested boundary accepts a stubbed terminal.**
   `MdbxChainContext::verify_snapshot_boundary`
   (`noid_chain/src/storage/mdbx_context.rs:2384-2443`) binds the terminal to
   the tip's height and semantic header id plus the epoch anchor. The harness
   passed an always-accepting verifier closure. Timeless header rules make a
   genuine terminal plausible, but generation and release verification of
   that terminal remain untested.
5. **The local install path commits and finalizes.** The real
   `apply_staged_state_snapshot` + `install_finalized_snapshot_staging` path
   writes the future-dated headers into the canonical archive and sets the
   finalized checkpoint to the snapshot tip immediately
   (`mdbx_context.rs:2686-2694` — `finalized = FinalizedCheckpoint { height:
   tip_height, .. }`). After install, `store.get_header(12)` returns the
   +10-year header and `finalized_checkpoint().height == 24`.
6. **The source predicts divergence below finality.** The reference fork (built
   and natively validated in the test) shares only genesis with the committed
   history. `plan_header_inventory` filters common ancestors to
   `>= header_dag.finalized()` and classifies deeper divergence
   `FinalizedDivergence` (`main.rs:4924-4943`), which the event loop answers
   with `mining_peer_quorum.reject_incompatible(from)` (`main.rs:11657-11668`)
   — the event loop would mark such peers incompatible. Re-staging a reference
   snapshot also meets a structural barrier: `SnapshotHeaderStaging::create`
   requires the base to be
   the first missing canonical height (`snapshot_header_staging.rs:298-313`),
   and after this install there is no such base on the reference fork. Live
   recovery paths have not been tested.
7. **Serving code has no intermediate drift re-check.** Manifest serving answers from the committed
   immutable generation (`noid_p2p/src/network.rs:8032-8075`); no drift
   re-check exists at serve time. Re-serving to another node was not run in
   this harness.

**Separate v2 difficulty test.** With the real `next_target` at v2 heights (where the *parent*
timestamp selects the child target), a far-future parent drives the child
target straight to the unconditional `GENESIS_TARGET` floor, while a
drift-capped (+120 s) parent eases it only marginally:

```text
child_target_sane_parent      = 0800…0000   (hard, tracks the anchor)
child_target_plus120s_parent  = 0e00…0000   (bounded in-drift easing)
child_target_10y_parent       = 0000…400000 (== GENESIS_TARGET floor)
```

`noid_chain/src/consensus/timestamps.rs:11-12` says the drift cap prevents
future timestamps from gaming difficulty. The 24-header snapshot witness is
in the **legacy** difficulty era; this target calculation uses v2 at height
279,506. The two tests do not jointly measure a production-scale alternate fork
or establish a 17-second cost. In-drift forks can also descend toward the
floor over time (ASERT halflife 120 s). The demonstrated difference is that
ordinary validation rejects the future-dated intermediate header while
snapshot staging accepts it.

**Fork choice still applies.** The snapshot
install requires the manifest to improve on the HeaderDAG-selected work
(`validate_snapshot_install_selection`, `main.rs:287-301`). A node that has
learned reference headers holds their cumulative work in its DAG; a competing
fork must exceed that amount. The prior devnet serving stall delayed state
installation while reference headers continued to be selected (`experiments/TAKEOVER.md`).
The release node also compares snapshot candidates over five seconds and
rechecks HeaderDAG work at install time
(`experiments/snapshot-adversary/REPORT.md`).

## B. A transaction can occur on sibling forks

The condition is two forks sharing an epoch anchor, each with the input
unspent. This is normal fork replay; each node chooses one canonical branch.

The fork-replay lane's exact transaction shape (same construction, one
owner-authorization bundle) is pushed through real block admission: the pinned
`build_block_template` builds a complete block (coinbase + the replayed user
transaction + tx root + ASERT target) on each of two forks sharing the epoch
anchor at h279504, each block is solved with real PoW, and each passes full
sequential consensus validation (`validate_block_consensus` +
`validate_mandatory_coinbase` + `validate_block_epoch_anchors` + the paged
spend stream validator + `canonical_owner_auth` + the same reused
`verify_wallet_authorization` bundle). Both children carry byte-identical
transaction bytes (in the fixed `TxPage` wire encoding, with the START|END
paged-spend markers a block-admissible single-page group requires) and the
same txid, with different tx roots. One nuance surfaced by pushing to real
admission: a block-level user transaction is a paged-spend page whose wire
form (`TxPage::encode`, `noid_chain/src/block.rs:602-632`) differs from the
standalone `Transaction::to_bytes` the fork-replay PoC exercised — the replay
survives this stricter encoding unchanged, since the same 323-byte body (now
with marker bits) round-trips identically on both forks.

Nothing prevents same-txid inclusion across sibling forks, and nothing should:
forks are alternative histories and a transaction must remain valid across a
reorg for liveness. (Real admission also surfaced the fee floor: the template
builder silently drops a candidate below `fee_breakdown(...).required_total`
— 5800 μNOID for one input/one output at the toy state — so the replayed
transaction pays it on both forks.) The consumers are fork-correct:

- The same linear state refuses the respend: after fork A applies the block,
  a second template carrying the same transaction is refused by the builder
  (input no longer live), matching the fork-replay `InputMismatch` control.
- The durable tx index is keyed txid → (height, position) and is rewritten
  through the undo log on reorg (`noid_chain/src/storage/mdbx_store.rs:4061-4067`,
  `UNDO_RETENTION_DEPTH`), so a node tracks one canonical fork at a time and
  never merges both inclusions.
- Mempool re-admission after reorg is explicit manual resubmission
  (`noid_mempool/src/pool.rs:909`, confirmed by the ledger-fairness lane §5);
  there is no cross-fork txid registry, and none is needed.

The defect established by the fork-replay lane therefore stays bounded to its
original scope: within any single linear history the transaction applies once;
across same-anchor forks it is valid once per fork, and fork choice decides.
No consumer (UTXO scan, history, RPC) was found that double-applies.

## C. Version boundaries reject the cross-schedule artifacts tested

The shared network identity can still mix peers across schedules. The tested
headers and authorization artifacts did not cross the receiving schedule's
validation gates.

Hunting for an asymmetric pair (an artifact the default-build node *accepts*
under wrong rules), every candidate is rejected by a check that is bound to
the receiving build's own fork schedule:

- **Headers:** the receiving node recomputes the ASERT target with its own
  `ACTIVE_SCHEDULE`; an isolated-V2.1 chain at low heights selects targets by
  the v2 rule while the default schedule expects the legacy rule —
  `BadDifficultyTarget`. Measured with the real `next_target` on the same
  header pair: the isolated-V2.1 build expects target `…ccb2070000` at h24
  (v2 parent-timestamp rule; `results-isolated.txt`), while the default build
  expects `…1864050000` (legacy candidate-timestamp rule; `results.txt`) —
  the tested header pair differs across schedules. Both build profiles show
  the snapshot drift discrepancy at component level; neither demonstrates
  live poisoning.
- **Owner authorization:** `owner_authorization_version` is height- and
  schedule-selected; at h24 the default build demands V3 while the isolated
  build demands V4, so each side rejects the other's bundles structurally
  (the fork-replay `AuthProof` control shows version-bound statements).
- **HistoryStep terminals:** `verify_terminal` dispatches by schedule
  (`uses_finance_runtime`/`v2_active` by height,
  `noid_miner/src/history_protocol.rs:727-766`). This should route a foreign
  terminal to the receiving schedule's verifier, but that full proof check
  was not executed. The boundary layer itself
  (`verify_snapshot_boundary`) is schedule-blind — it checks only the wire
  cap, metadata decode, and (height, tip-semantic-id) binding — but the
  schedule gate behind it is conventional code, not cryptographic (see
  blocker table).

Building on the version-domains lane's results: terminal *metadata* admission
is schedule-agnostic (`same_metadata_accepted_under_other_v2_schedule: true`,
`results/terminal.json`), while full terminals require the exact proof bank
and verified origin. The recursive verifier under a mismatched schedule was
not executed; the other tested layers reject the foreign artifacts.

## Local test output (abridged)

`snapshot_terminal_boundary=accepted` below means the boundary check accepted
the harness's stubbed verifier result. It is not a release-terminal result.
The literal output keys retain the harness's earlier `poison` naming for the
clock-shifted fixture; they are identifiers, not a statement about live nodes.

```text
[dialect-facts]
build_profile=isolated_v2_1=false
schedule v2_1_active(24)=false
owner_auth_version(24)=V3 owner_auth_version(279506)=V4
[difficulty-collapse]
child_target_sane_parent=0800…            (hard)
child_target_plus120s_parent=0e00…        (bounded in-drift easing)
child_target_10y_parent=0000…00400000     (== GENESIS_TARGET floor)
far_future_parent_floors_child=true
[snapshot-poisoning]
attacker_chain: tip_h=24 tip_ts=<now+60> poison_h=12 poison_ts_delta_years≈10 tip_drift_ok=true
honest_path_rejects_poison_header=BadTimestamp(h=12)
snapshot_path_stages_poison_chain=ok sealed_height=24 staged_headers=24
snapshot_boundary_checks=manifest_ok tip_future_drift_ok=ok
snapshot_terminal_boundary=accepted binding=(height,tip_semantic_id)
control_changed_tip_root=rejected_before_recursive_verifier
snapshot_segments=1 live_entries=24
snapshot_install=committed
committed_history: poison_h12_canonical=true finalized_height=24
honest_chain_native_admission=all_ok(h1..h25)
honest_inventory_classification=FinalizedDivergence(best_common_ancestor_above_finalized=false finalized=24)
honest_snapshot_restage=rejected(base_is_not_first_missing_height)
[replay-publication]
admission_fee_floor=5800
replayed_tx: page_bytes=323 txid=c4f1bbac67b2625aa88dd0cc0fcedd413993ef194ebe432e75f179965dce3ebd
proving one paged-spend owner authorization with the legitimate wallet witness
both_forks_admitted=ok same_tx=true same_txid=true tx_roots_differ=true
same_fork_second_spend=refused_by_template_builder(StateApplyError("UnknownOrSpentInput"))
replay_scope=designed fork tolerance; single linear state refuses the respend
all_local_assertions=passed
```

## Controls

- **Native/suffix path rejects the future-dated chain** — `validate_header` with a
  local clock returns `BadTimestamp` at the future-dated height; every other
  header of the same chain is natively admissible (the differential is
  exactly the one rule).
- **Terminal binding control** — a tip whose state root is tampered with no
  longer matches its terminal and is rejected before the verifier callback
  runs (`terminal does not bind its staged boundary`).
- **Reference chain admissible** — the comparison fork passes `validate_header`
  at every height under the same local clock. The local divergence classifier
  responds to the committed checkpoint, not invalid reference headers.
- **Difficulty contrast** — in the separate v2 target calculation, the
  drift-capped (+120 s) parent does not floor difficulty; the far-future
  parent does.
- **Same-fork respend refused** — after fork A's block applies, the template
  builder refuses the identical transaction (Finding B control).
- **Cross-dialect artifacts rejected** at the layers tested (Finding C).
- **Fork choice compares work** — snapshot install requires improving on
  HeaderDAG-selected cumulative work (`main.rs:287-301`). Learned reference
  headers raise the work threshold, and a node retaining established mainnet
  work has a much higher threshold.

## Checks that constrain stronger variants

| Chain | Stronger version | Blocked by | Where | Cryptographic or conventional |
| --- | --- | --- | --- | --- |
| A | Installing an alternate fork above reference HeaderDAG work | Snapshot install must exceed selected cumulative chainwork (`choose_chain_by_work`); the threshold scales with work already learned | `noid_node/src/main.rs:287-301` (`validate_snapshot_install_selection`) | Conventional comparison over PoW-derived work |
| A | Installing an unverified tip state | The tip state must be authenticated by a genuine HistoryStep terminal bound to the tip semantic header id; a changed tip root is rejected before the verifier | `noid_chain/src/storage/mdbx_context.rs:2422-2435`; PoC control `control_changed_tip_root` | Cryptographic (semantic header id + recursive proof binding) |
| A | Far-future headers surviving *native* sync | Per-header future-drift admission with the local clock on every live header | `noid_node/src/snapshot_header_staging.rs:1049` (`validate_bounded_header_extension`), `noid_chain/src/consensus/header.rs:246` | Conventional (wall-clock policy — intentionally not part of the provable rules) |
| B | Double-apply of the replayed tx across forks | One canonical fork per node; the durable txid index is rewritten through undo logs on reorg | `noid_chain/src/storage/mdbx_store.rs:4061-4067`, `UNDO_RETENTION_DEPTH`; PoC control `same_fork_second_spend` | Conventional persistence logic (sound by single-canonical-history construction) |
| C | Default node accepting an isolated-V2.1 header | Receiver recomputes the ASERT target under its own schedule — foreign dialect target mismatches (`BadDifficultyTarget`) | `noid_chain/src/consensus/header.rs:211-233`; PoC `dialect_expected_target_h24` diverges across builds | Conventional (schedule constants compiled in; nothing cryptographic separates the dialects) |
| C | Default node accepting a foreign owner-authorization | `owner_authorization_version` is height- and schedule-selected; statements are version-bound | `noid_tx` owner-auth versioning; PoC prints V3@h24 (default) vs V4@h24 (isolated) | Cryptographic at the statement level (version-bound proof), conventional at the selection level |
| C | Default node accepting a foreign HistoryStep terminal | `verify_terminal` dispatches finance/v2/legacy by its own schedule before any proof math | `noid_miner/src/history_protocol.rs:727-766` | Conventional dispatch + cryptographic proof behind it; **not executed in this audit** (proof pack is release-only) — flagged as a regression surface |

## PoC implementation and reproduction

`src/main.rs` drives, in order: dialect facts, the difficulty collapse, chain
A (build future-dated chain with real templates/ASERT/PoW → ordinary-path rejection
→ real staging/boundary/tip-drift/terminal-binding → real atomic install →
canonical readback, finalization, reference-chain divergence classifier,
re-stage rejection), and chain B (both-fork real block admission + controls).
The boundary-manifest check (`validate_snapshot_staged_header_boundary`) is
reproduced field-for-field from `main.rs:4180-4235` (it lives in the binary
crate); every other function is the pinned export. The estrangement predicate
is reproduced from `main.rs:4884-4943` using the real
`MdbxChainContext::find_ancestor_height` and finalized checkpoint.

```sh
cd /Users/ember/dev/parano1d-break/experiments/malicious-node
./run.sh          # default schedule -> results.txt; isolated V2.1 -> results-isolated.txt
```

`run.sh` first verifies `src/snapshot_header_staging.rs` is byte-identical to
the pinned commit's blob (`499576e978a3d28cbe2212098c8e56c1e7dab6ba`). Both
runs end with `all_local_assertions=passed`; the default run's full transcript
is `results.txt` and the isolated-schedule run's is `results-isolated.txt`.

## Assurance limits

- **The recursive terminal is stubbed.** Development builds may omit the
  HistoryStep proof pack. This harness passed an always-accepting closure to
  `verify_snapshot_boundary`; it did not generate or verify a release terminal
  for the future-dated chain. The binding surface and timeless provable header
  rules motivate a release-verifier test; that final link remains unmeasured.
- **No live sockets.** The harness calls functions in node order, but does
  not execute peer exchange or the event loop. It reproduces the logic of
  `plan_header_inventory` and `validate_snapshot_staged_header_boundary` from
  the pinned binary source rather than linking those private functions.
- **Offer selection was not simulated.** The real node settles competing
  candidates for five seconds, then rechecks HeaderDAG work at install time
  (`experiments/snapshot-adversary/REPORT.md`). This test does not demonstrate
  which offer would be selected among live providers.
- **Finding B's consumer survey** covers template admission, sequential
  consensus validation, state application, and the durable tx index/undo
  path; it does not fuzz the RPC surface for presentation-level confusion.
- **Finding C's untested layer:** the recursive verifier under a mismatched
  schedule (pack-gated). All executable layers reject; the open question is
  whether the pack layer would too — it is conventional code, so it should be
  treated as a regression surface, not a proven wall.
- The alternate chain uses a coinbase-only fork; user transactions on it
  would additionally need valid owner authorizations, which its producer
  could provide for its own inputs.
