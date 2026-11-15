# Block/header/mempool/miner validation audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All experiment files are confined to this directory. No live nodes or peers were used, and the fork-replay experiment was not duplicated.

## Outcome

No remotely exploitable block/template acceptance bug or low-cost deterministic consensus DoS was demonstrated in the exercised scope. One informational standalone-API gap was reproduced.

### Informational: standalone sequential validation does not enforce the user epoch anchor

`validate_block_consensus` (`parano1d/noid_chain/src/consensus/validation.rs:574`) has no expected user-epoch-anchor input and never calls `validate_block_epoch_anchors`. The harness built a state-valid, fee-valid, correctly rooted, real-PoW block whose user transaction had the wrong epoch anchor. `validate_block_consensus` accepted it (`nonce=74502`), just as it accepted the correct control (`nonce=46587`).

This is not a remote vulnerability in the pinned node. The production admission path calls `validate_block_epoch_anchors` first (`parano1d/noid_chain/src/storage/mdbx_context.rs:1353`), and a repository-wide call search found no production caller of `validate_block_consensus`. A real `MdbxChainContext::apply_next_block` control rejected the same wrong-anchor construction with `BadEpochAnchor` before invoking the terminal-verifier callback.

Severity is therefore **informational for this repository**, rising only for a downstream caller that treats this public utility as a complete acceptance predicate. The utility is documented as a test/interpreter path and does not verify wallet proofs either. Consider documenting the epoch-anchor precondition next to the function, taking the expected anchor as an argument, or making the acceptance boundary harder to misuse.

`materialize_accepted_block_state` also does not enforce epoch anchors by design: its contract requires prior full terminal verification. Recomputing `tx_root` after an anchor mutation is not an epoch check.

## Executable coverage

`src/main.rs` uses the real chain, transaction, Poseidon, template, state, MDBX, and wallet-authorization APIs.

- Transaction/root binding: stale `tx_root` and stale `state_root` substitutions were rejected. Independent user transactions were accepted in either order only after recomputing their distinct ordered roots/state. Duplicate user transactions and duplicate primary coinbases were rejected.
- Coinbase/system records: inflated value, wrong owner, wrong parent anchor, missing development payout, wrong payout amount, and payout moved after a user page were rejected. A valid height-4320 development-payout template was accepted.
- Fees/subsidy: a one-below-minimum fee was rejected (`required=5800`, `actual=5799`). A net-growth transaction was accepted with `burned=2500` and `claimable=6500`; adding one unit of burn to the coinbase was rejected as inflation.
- Header/template mutation: parent hash, height, difficulty target, MTP, future drift, and unexpected state-depth expansion mutations were rejected. Nonce and miner-address mutations changed the PoW digest. Only the nonce is excluded from the semantic header id.
- Proof/transaction substitution: a real wallet authorization verified for its original PagedSpend and failed after output-owner and epoch-anchor mutations. Structural HistoryStep terminal substitution across semantic headers failed; nonce-only reuse succeeded as intended.
- Production epoch boundary: temporary local MDBX admission rejected the wrong-epoch block before terminal verification.

The ordinary mempool admission path was compared statically with block/template predicates. It enforces the current anchor, fee floor plus consensus fee, state preimages, output emptiness, and pool conflicts; the underlying `Mempool::admit_entry` checks all four input/output conflict directions. Template construction reparses complete groups and reapplies state. No invalid accepted block was derived from a mempool/template divergence.

## Commands and results

From this directory:

```bash
git -C ../../parano1d rev-parse HEAD
# 52938433a30b185ab007afaa15166a632cf2e82e

cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final result: `ALL ASSERTIONS PASSED`. See `results.txt` for the complete output. Key lines:

```text
ACCEPT fully sealed sequential-consensus block: nonce=46587
POC standalone validate_block_consensus ACCEPTED wrong user epoch anchor; production boundary rejected it: nonce=74502
REJECT body mutation with stale tx_root: HeaderTxRootMismatch
REJECT output substitution with rebound tx_root but stale state_root
ACCEPT both independent-transaction orders with distinct rebound tx/state roots
REJECT duplicate user transaction: Err(SlotConflict)
REJECT coinbase subsidy/fee ceiling +1: Err(InflatedCoinbase)
REJECT one-below-minimum fee: Err(BelowMinFee { required: 5800, actual: 5799 })
REJECT claiming state-growth burn: Err(InflatedCoinbase)
REJECT future-drift mutation: Err(BadTimestamp)
REJECT structural terminal substitution across semantic headers; ACCEPT same terminal across nonce-only change (intended)
REJECT real wallet proof substituted across output-owner and epoch-anchor mutations
REJECT production MdbxChainContext::apply_next_block wrong epoch before terminal verifier
ALL ASSERTIONS PASSED
```

## Assurance limits

- The two `validate_block_consensus` blocks were really PoW-sealed and state-validated, but this utility path does not generate or verify a recursive HistoryStep proof. They are not claimed to be network-accepted blocks.
- The MDBX setup block used an explicitly stubbed terminal-verifier callback, matching the injection point in the real API. The wrong-epoch child was rejected before that callback. This tests native admission ordering, not recursive-proof soundness.
- Structural terminals contain a real metadata prefix plus one opaque byte. They test bundle/header binding only; no full recursive terminal was generated or substituted. The wallet authorization proof is real and fully verified.
- Fixtures use small in-memory state for most checks and real production difficulty arithmetic with `GENESIS_TARGET`. Native financial operation families and every fork-height/proof-class combination were not exhaustively exercised.
- No consensus DoS is claimed: the audit did not demonstrate a cheap deterministic trigger. No repository tests were changed and no source fixes were made.
