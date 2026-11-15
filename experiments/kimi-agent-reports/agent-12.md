# agent-12: Audit block validation gaps

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Audit result

Completed the hands-on audit at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`. All new files are confined to `experiments/block-validation`; no `parano1d` source files or existing working-tree changes were modified.

**No remotely exploitable block/template acceptance vulnerability or low-cost deterministic consensus DoS was demonstrated.**

### Informational finding: standalone interpreter omits epoch-anchor enforcement

`validate_block_consensus` (`parano1d/noid_chain/src/consensus/validation.rs:574`) does not call `validate_block_epoch_anchors` and has no expected user-anchor parameter.

The PoC produced a state-valid, fee-valid, correctly rooted, real-PoW block with a wrong user epoch anchor:

```text
ACCEPT fully sealed sequential-consensus block: nonce=46587
POC standalone validate_block_consensus ACCEPTED wrong user epoch anchor; production boundary rejected it: nonce=74502
```

This is **not a production-node vulnerability**:

- Production calls `validate_block_epoch_anchors` before native block checks at `parano1d/noid_chain/src/storage/mdbx_context.rs:1353`.
- No production caller of `validate_block_consensus` was found; it is documented as a test/utility interpreter and does not verify wallet proofs.
- A real `MdbxChainContext::apply_next_block` control rejected the wrong-anchor block with `BadEpochAnchor` **before invoking the terminal verifier**.

Severity is informational here, but downstream callers should not treat this public utility as a complete acceptance predicate. Consider documenting its epoch-anchor precondition or accepting the expected anchor explicitly.

## PoC coverage

The executable matrix confirmed:

- Stale transaction-root and state-root substitutions are rejected.
- Independent transaction reordering works only with appropriately rebound ordered roots/state.
- Duplicate user transactions and duplicate primary coinbases are rejected.
- Coinbase inflation, owner mismatch, and parent-anchor mismatch are rejected.
- Missing, incorrect, and misplaced mandatory development payouts are rejected; valid height-4320 payout accepted.
- Below-minimum fee rejected at `5799 < 5800`.
- State-growth accounting accepted `burned=2500`, `claimable=6500`; claiming burn through coinbase rejected.
- Parent, height, difficulty, MTP, future-drift, and state-depth mutations rejected.
- Nonce and miner-address mutations change the PoW digest; only nonce is excluded from semantic-header identity.
- A **real wallet authorization proof** failed after transaction output-owner and epoch-anchor substitution.
- Structural HistoryStep terminal substitution across semantic headers failed; nonce-only terminal reuse succeeded as intended.

## Commands and verification

From `experiments/block-validation`:

```bash
git -C ../../parano1d rev-parse HEAD
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final result:

```text
REJECT production MdbxChainContext::apply_next_block wrong epoch before terminal verifier
ALL ASSERTIONS PASSED
```

The complete output is in `experiments/block-validation/results.txt`. The generated 316 MiB build directory was removed afterward.

## Files added

- `experiments/block-validation/Cargo.toml`
- `experiments/block-validation/Cargo.lock`
- `experiments/block-validation/src/main.rs`
- `experiments/block-validation/README.md`
- `experiments/block-validation/results.txt`

## Limitations

- Sealed sequential blocks used real PoW and state validation, but not recursive HistoryStep proofs; they are not claimed to be network-accepted blocks.
- The MDBX setup block used an explicitly stubbed terminal-verifier callback. The tested wrong-epoch child was rejected before that callback.
- Structural terminals contained genuine metadata plus an opaque byte; no full recursive terminal substitution was performed.
- Native financial operation families and every fork/proof-class combination were not exhaustively exercised.
- No live nodes, peers, or fork-replay scenarios were used.
