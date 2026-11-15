# MDBX persistence, reorg, pruning and crash-consistency audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. The PoCs drive the real
`MdbxChainContext`/`MdbxStore` production APIs against throwaway local MDBX
databases under a tempdir. No live node, peer, RPC server, or user database was
touched; nothing outside this lane directory was modified. The `pinned/` crate
sources were verified byte-identical to `git archive 52938433` output before
building (the read-only checkout's unrelated working-tree modifications are not
part of the build).

## Finding 1: the persisted circulating-supply counter is unauthenticated and its corruption is silently accepted by compact restart

**Severity: Medium — integrity/liveness impact on a single node, requiring
local write access to the MDBX file (attacker with file access, bit rot, or a
tampered/restored backup). No remote vector, no invalid block accepted, no
funds minted: every UTXO amount remains authenticated by the exact state root.**

`state_meta` v2 (`noid_chain/src/storage/serial.rs:520-559`) appends a 16-byte
`circulating_supply_micronoid` to the 20-byte v1 counters. On restart, the v1
counters are cross-checked against the tip header
(`mdbx_context.rs:781-788`), whose hash is committed by `consensus_meta` and
`chain_tip` — but the supply is read verbatim
(`mdbx_context.rs:770`) and passed through the compact restart path
(`try_load_compact_chain_state`, `mdbx_context.rs:503-567`) into
`ChainState::from_evicted_parts` (`state.rs:441-467`), which authenticates only
the per-segment exact-root set. Nothing binds the supply to the header. The
dense fallback (`load_dense_chain_state`, `mdbx_context.rs:400-498`) proves the
value is derivable from authenticated data by recomputing it from raw slot
amounts — the compact path simply trusts the stored bytes.

Demonstrated, all through `MdbxChainContext::open_or_create`,
`apply_next_block`, real templates and real PoW:

1. **Upward corruption is accepted and propagated.** Adding 123,456,789
   micronoid to the supply in `state_meta` is accepted on restart with the
   authenticated state root unchanged; a subsequently accepted, fully valid
   block persists the corrupted value (`commit_block` writes
   `self.state.circulating_supply_micronoid`, `mdbx_context.rs:1148-1160`), and
   `store.get_circulating_supply()` serves it. Clearing
   `segment_summaries` (forcing the dense verifier) recomputes the true supply —
   confirming the compact path is the difference, not the corruption mechanism.
2. **Downward corruption permanently stalls spend admission.** Zeroing the
   supply is likewise accepted silently at a header-authenticated tip. A block
   that spends two tagged coinbase UTXOs (90,000,000 micronoid, built by the
   production template builder with the production fee schedule and accepted
   end-to-end on a clean twin context) is then rejected **deterministically and
   permanently**: `supply_after_slot_updates` (`state.rs:585-612`) folds slot
   deltas with checked arithmetic from the corrupted base, underflows
   (`ApplyError::CirculatingSupplyInvariant`, `state.rs:1200-1202`), and the
   rollback of the uncommitted candidate hits the same underflow
   (`mdbx_context.rs:983-988`), so the surfaced error is
   `corrupt: uncommitted block supply rollback failed` after a full hot-state
   reload from MDBX. The node keeps accepting coinbase-only blocks, so it looks
   alive while diverged (45,000,000 reported vs 180,000,000 on the clean twin
   at the same height).

The project's own restart posture is to authenticate or reject
("authenticates the compact exact segment-root set against the canonical tip's
`state_root`", and clears every chain table on `MdbxContextError::Corrupt`,
`mdbx_context.rs:674-701`). The supply is the one hot counter exempt from both,
and its failure mode never triggers the clear-and-resync recovery.

### Recommended correction

Bind the supply to authenticated data: either include a supply commitment in
the tip header (it already carries `active_slot_count` and `alloc_counter`), or
store per-segment amount sums alongside the authenticated segment summaries and
cross-check their total against the persisted value at restart, falling back to
the dense verifier on mismatch. On mismatch, treat it as `Corrupt` so the
existing clear-and-reinitialize path recovers the node.

## Finding 2: a noncanonical `chain_tip` encoding passes restart validation but permanently stalls every successor commit

**Severity: Low-to-Medium liveness — same local-write/bit-rot precondition as
Finding 1; no invalid state accepted; the node fails closed but with no
recovery path.**

`decode_chain_tip` (`serial.rs:511-518`) accepts any value of at least 40
bytes, silently ignoring trailing bytes, while `decode_segment_summary`
(`serial.rs:579-586`) and `decode_consensus_meta` require exact lengths.
Appending one byte (0xA5) to the 40-byte `tip` row therefore passes every
restart consistency check (`get_chain_tip` reads lenient `Vec<u8>`,
`mdbx_store.rs:1936-1941`; the `consensus_meta` cross-check in
`restore_from_mdbx` still matches). But `commit_block` re-reads the tip with a
typed `Option<[u8; 40]>` inside the write transaction
(`mdbx_store.rs:3829-3838`; the same typed read exists at `mdbx_store.rs:213`
and `:374`), so libmdbx decode fails and **every** subsequent block commit —
including reorgs — returns `store: mdbx: 41`. The node opens cleanly, reports a
healthy tip, and can never advance; because restart validation succeeds, the
clear-all recovery never triggers. Demonstrated at height 0 with the successor
block rejected and no partial tip mutation.

### Recommended correction

Make `decode_chain_tip` exact-length (like the segment-summary decoder) so the
corrupt row is rejected during `restore_from_mdbx`, letting the documented
clear-and-reinitialize recovery run; or have the commit path use the same
lenient decode as restart and treat a mismatch as corruption at restart time.

## Negative and control results

- **Reorg rollback completeness (passed).** A depth-1 MDBX reorg from a real
  competitor branch removed the old canonical header/hash-index/tx-index/UTXO
  entries, installed the replacement tip, left the displaced body only in the
  content-addressed object cache, and moved owner-index UTXOs between owners
  exactly.
- **Staged-reorg atomicity under post-apply failure (passed).** Injecting a
  rejection after the second replacement block validated and staged restored
  the hot tip/root/supply and the durable store byte-for-byte (tip, header 3
  absent, rejected tx absent from the index), survived close/reopen, and
  historical reconstruction to h0/h1 matched each header's committed
  `state_root`.
- **Crash recovery (passed).** A child process committed three blocks and was
  killed with SIGABRT between durable commits (no destructors). Reopen resumed
  at height 3 with a header-matched exact root and the deterministic supply
  (135,000,000), and accepted the next block. This is the P.18 single-transaction
  guarantee behaving as documented.
- **Retention/pruning boundaries (passed).** At tip 45 with
  `RETAINED_BLOCK_SERVING_DEPTH`=42 and `UNDO_RETENTION_DEPTH`=36: h2 fully
  pruned (body, terminal, tx index), h3 terminal-only, h4 body+tx retained,
  undo h9 pruned / h10 retained — exactly the documented boundaries.
- **Historical-state window (passed).** `reconstruct_historical_exact_state`
  reproduced the exact root at the oldest retained height 9 and rejected h8
  with `TargetOutsideUndoWindow`.
- **Snapshot boundary export (passed).** `export_snapshot_boundary_generation`
  at h9 produced a manifest bound to the canonical header; reopening the
  generation and reading each manifest segment authenticated length, digest,
  sparse decode, exact segment root and native bindings. (The inherited draft
  of this check hardcoded segment id 0 and a wrong length formula — the
  manifest carries only live segments; it was corrected before any result was
  recorded.)
- **Rejected attack:** corrupting the supply does **not** let an invalid block
  validate — every state root, header counter, PoW and structural check still
  binds; only the unauthenticated counter diverges. No cross-fork state
  confusion was observed in the reorg paths exercised.

## PoC implementation and reproduction

`src/main.rs` uses the pinned `noid_chain` storage APIs directly. Blocks are
produced by the real `build_node_owned_block_template` (epoch anchors, fee
schedule, expansion counters), sealed with real PoW grinding
(`validate_pow`, genesis-floor targets, ideal ASERT spacing), and admitted
through `MdbxChainContext::apply_next_block` with full native validation. The
recursive HistoryStep terminal verifier is stubbed `Ok(())` (see limits). DB
corruption is applied by a second process opening the same file read-write with
production-identical MDBX options and rewriting single rows.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
# scenarios: all | supply | stall | metadata | reorg | retention | crash
```

Final output includes:

```text
VULN compact-restart: clean_supply=45000000 corrupted_supply=168456789 tip=... state_roots_equal=true
VULN propagated-by-valid-commit: height=2 clean_supply=90000000 persisted_corrupt_supply=213456789 tip=...
CONTROL dense-restart-recomputes: cleared segment_summaries; recovered_supply=90000000 at_height=2
CONTROL clean context accepted the two-input spend of tagged coinbase UTXOs 45000000+45000000 micronoid at height 4 (supply=180000000)
VULN zeroed-supply restart: supply=0 accepted silently at height 3, state root still header-authenticated
VULN spend-admission stall: valid block rejected twice with `corrupt: uncommitted block supply rollback failed`; tip pinned at height 3, no header 4 written
LIMIT zeroed supply still admits coinbase-only blocks (supply=45000000 at height 4, clean chain had 180000000); ...
VULN noncanonical-tip: 41-byte chain_tip accepted by open_or_create at height=0
LIMIT noncanonical-tip: next valid commit rejected with `store: mdbx: 41`; no partial tip mutation
CONTROL reorg: old canonical hash/tx/body removed; replacement tip=... old_body_remains_content_addressed_only=true
CONTROL rejected-reorg: post-apply failure restored hot/durable tip, root, supply; reopen and historical rollback to h0/h1 matched
CONTROL crash-recovery: SIGABRT after 3 durable commits; reopen resumed at height 3 with header-matched root and supply=135000000; block 4 accepted after restart
CONTROL retention: h2 fully pruned; h3 terminal-only; h4 body/tx retained; undo h9 pruned and h10 retained
CONTROL historical/snapshot: exact state at oldest retained h9 matched header; h8 rejected; boundary generation reopened and authenticated 1 live segment(s) ids=[148]
```

## Assurance limits

- The recursive HistoryStep terminal verifier is stubbed: the PoC constructs
  terminal metadata itself. Nothing tested here depends on proof soundness, and
  no claim is made about it; all native header/PoW/anchor/state-root checks are
  real.
- Chains are short (up to 45 blocks, one miner, 1-3 transactions per block,
  `log_slots`=24, genesis-floor difficulty). Network-scale blocks, expansion
  events beyond +1, and multi-epoch anchors were not exercised.
- The crash control kills between durable commits, not inside the MDBX commit
  window; a kill mid-transaction relies on MDBX's own atomicity and was not
  separately demonstrated.
- Corruption is applied offline by a second writer with the DB closed in the
  victim process; concurrent online corruption during operation was not tested.
- The spend-stall demonstration requires the spent total to exceed the
  corrupted base plus the block's own coinbase, and depends on delta ordering
  (output slot above input slots); both preconditions are stated in the code.
  Upward corruption never stalls admission — it only diverges bookkeeping.
- The supply/stall results were verified on a clean twin context in the same
  run; line numbers refer to the pinned sources in `pinned/` (byte-identical to
  the audited commit).
