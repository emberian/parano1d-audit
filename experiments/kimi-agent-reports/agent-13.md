# agent-13: Attack recursive sync semantics

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Implemented and ran local PoCs at pinned `52938433a30b185ab007afaa15166a632cf2e82e`. All edits are under `experiments/recursive-sync`; no live nodes were contacted, and the typed-binding Merkle lane was not duplicated.

## Confirmed gap: tip-only future-drift admission

Snapshot header staging validates every header timelessly (`noid_node/src/snapshot_header_staging.rs:938`), while the daemon checks future drift only on the final snapshot tip (`noid_node/src/main.rs:4308`). Exact suffix application instead runs `validate_block_checks` on every body using `unix_now()` (`noid_chain/src/storage/mdbx_context.rs:1532`, `noid_node/src/main.rs:4517`).

The PoC at `experiments/recursive-sync/src/main.rs:173` builds four real, PoW-valid blocks and executes both native validation and the actual snapshot header stager:

```text
native_block height=1 timestamp=1787328940 result=accepted
native_block height=2 timestamp=1787328960 result=accepted
native_block height=3 timestamp=1787330120 result=BadTimestamp
native_block height=4 timestamp=1787329000 result=accepted
snapshot_timeless_header_stage= sealed height=4 tip_future_drift=pass intermediate_height_3_native=BadTimestamp
all_local_assertions=passed
```

Thus, **assuming an otherwise valid height-4 HistoryStep terminal**, the snapshot header path admits a sequence that ordinary native suffix application rejects at height 3. This is a wall-clock admission mismatch, not a demonstrated invalid state-root or proof forgery. A fix would check future drift for every staged header or explicitly document tip-only admission as intended.

## Other concrete results

- Changed terminal state root was rejected before invoking the recursive verifier.
- Same-height wrong epoch header failed with `EpochAnchorId`; accumulator/header root mismatch failed with `StateRoot`.
- Valid snapshot finalized with root:
  `6aa81a306b3f8e5b4c62167351ecc9932396427e22e816856463af07c0528888`.
- Changed leaf under the old segment root: `snapshot segment 0 exact root mismatch`.
- Root-matching segments under a wrong global header root: `snapshot exact root mismatch`.
- Root-bound native opening finalized successfully; a root-matching raw slot carrying NOID amount 1 with a zero-NOID `TokenBalance` opening failed with `snapshot segment 0 native amount binding`.
- Deferred matrix-claim tests passed:
  - 6 legacy cache tests, including fresh-claim rechecking, changed-point rejection, eviction, and poisoned-cache fallback.
  - 1 v2 exact-cache test.
- Native atomic snapshot installation test passed:
  `snapshot_install_atomically_commits_headers_state_terminal_and_tip`.
- `cargo check` and `cargo fmt -- --check` pass.

Full output is saved in `experiments/recursive-sync/poc-output.txt`.

## Remaining assumptions and limits

- No production-sized height-4 HistoryStep was generated. The snapshot finding therefore proves the header-admission differential, with a valid terminal as an explicit precondition—not an end-to-end cryptographic acceptance claim.
- The full daemon build requires unavailable independently pinned retirement-key artifacts. The PoC compiles the exact production `snapshot_header_staging.rs` module directly rather than weakening or bypassing that requirement.
- Production origin certificates, genesis transitions, every class/profile, and release-pinned matrix banks were not exercised. Those paths received code review plus small-matrix cache tests only.
- Native sidecar binding assumes opening-root collision resistance; raw UTXO roots alone do not commit the sidecar bytes.
- If tip-only wall-clock admission is intentional snapshot policy, the reported mismatch is a design tradeoff rather than a consensus bug.

Files created:

- `experiments/recursive-sync/Cargo.toml`
- `experiments/recursive-sync/Cargo.lock`
- `experiments/recursive-sync/src/main.rs`
- `experiments/recursive-sync/poc-output.txt`
- `experiments/recursive-sync/target/` generated build artifacts

The pre-existing modifications inside `parano1d` were left unchanged.
