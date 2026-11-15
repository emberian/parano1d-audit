# External-miner and PoW-template audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. The PoCs are local library/template validations; no live node, worker, peer, or RPC server was started.

## Finding: invalid PoW consumes the only prepared template

**Severity: Medium availability impact, requiring mining API authorization (or local owner access).**

`submitBlock` consumes the node-owned attempt before it validates the submitted nonce:

1. `noid_rpc/src/server.rs:2537-2563` decodes the request and calls `ExternalMiningAttemptSlot::begin_proving`.
2. `begin_proving` (`noid_rpc/src/server.rs:529-555`) removes the `Ready` value and changes the sole slot to `Proving`.
3. Only afterward does `finalize_node_owned_template_after_nonce` call `validate_pow` (`noid_rpc/src/server.rs:1470-1491`).
4. On `InvalidPoW`, the proving lease drops and `finish_proving` makes the slot `Idle` (`noid_rpc/src/server.rs:557-567`, `720-736`). The old prepared value is not restored.
5. Replaying that template ID returns `Unavailable`. The next `getBlockTemplate` must repeat snapshot selection and complete nonce-independent HistoryStep preparation (`noid_rpc/src/server.rs:1257-1358`).

The PoC used the real chain template builder and PoW validator with the pinned server's actual generic attempt-slot implementation. Invalid nonce `0` was rejected by `validate_pow`, but the template became permanently unavailable exactly as it did after a valid submission. The extracted slot transitioned `Ready -> Proving -> Idle`, and the next reservation required a fresh template.

A worker with the shared mining bearer token can therefore submit cheap, canonically encoded invalid nonces after each expensive template preparation. Repetition destroys every prepared attempt and can prevent all external workers on the node from producing blocks while repeatedly consuming node CPU. This is not an unauthenticated remote attack and does not accept an invalid block. It also turns an accidental worker/node PoW mismatch into loss of the whole prepared attempt rather than a retryable nonce error.

The documented lifecycle says templates are invalidated by a canonical tip change, successful submission, or node-side cancellation (`docs/operate/external-miner.md:100-108`); rejection of a nonce that never had valid PoW should not destroy reusable node work.

### Recommended correction

Validate the nonce against an immutable borrow of the matching, unexpired, current-parent `Ready` attempt before transitioning to `Proving`. A slot method can perform the single Poseidon2b check while holding the attempt lock, followed by the existing atomic consume/parent recheck. Alternatively, restore the identical `Ready` value on `InvalidPoW` only after rechecking its ID, expiry, and parent. Keep successful submissions single-use. Request-rate limiting can remain defense in depth, but is not a substitute for retaining work after invalid PoW.

## Negative and control results

- Two real, distinct-payout templates passed full sequential consensus validation with their own real PoW nonces: A `981105`, B `703732`, using `GENESIS_TARGET`.
- Reusing A's nonce on B returned `InvalidPoW`. The different payout changes the state root, semantic header, and PoW schedule.
- A nonce-only change preserved `SEMHDR` but changed `BLOCKHDR` and `POWHDR`, as intended. Reusing a nonce-independent terminal across nonces is therefore not itself a vulnerability; PoW still has to validate for each sealed header.
- Changing the header payout without changing the coinbase returned `BadCoinbaseOwner`.
- Submitting template A against competing parent B returned `BadParentHash`.
- Direct slot controls confirmed canonical-tip invalidation, expiry, and successful-submission single use.
- Static review found the mining credential restricted to `getBlockTemplate` and `submitBlock` (`noid_rpc/src/server.rs:3846-3848`). Remote requests without a recognized credential are rejected; loopback owner access is intentional. Custom coinbase requires both the mining credential and explicit node opt-in.
- No unauthorized block acceptance, cross-template valid-PoW reuse, stale-parent acceptance, payout theft, or block-body substitution was demonstrated.

## PoC implementation and reproduction

`build.rs` reads `noid_rpc/src/server.rs` using `git show` at the pinned commit. It verifies the `begin_proving -> finalizer/validate_pow` order and extracts the generic `ExternalMiningAttemptSlot` source verbatim into Cargo's lane-local `OUT_DIR`. `src/main.rs` executes that code with real `noid_chain` templates, `GENESIS_TARGET`, sequential consensus validation, semantic/block/PoW identities, and stale-parent checks.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final output includes:

```text
CONTROL real templates accepted: A nonce=981105, B nonce=703732, target=0000000000000000000000000000000000000000000000000000000000400000
REJECT nonce A reused on different-payout template B: InvalidPoW
CONTROL nonce-only change preserves SEMHDR, changes BLOCKHDR and POWHDR
REJECT payout substitution at the coinbase boundary: BadCoinbaseOwner
REJECT solved template after competing parent: BadParentHash
POC invalid nonce=0 consumed prepared template 07000000000000000100000000000000; replay=Unavailable; expensive_template_reusable=false
CONTROL valid nonce is also single-use: replay=Unavailable (intentional)
CONTROL canonical-tip invalidation drops the ready attempt
CONTROL expired ready attempt is unavailable
ALL ASSERTIONS PASSED
```

## Assurance limits

- The lifecycle PoC carries a real chain `BlockTemplate` in the server's actual generic slot, but does not construct a full recursive `PreparedBlockAttempt` or measure HistoryStep proving time. The expensive preparation before template issuance is established directly by the pinned production call path, not benchmarked here.
- The valid template controls use small in-memory state (`log_slots=8`) and the sequential consensus interpreter. They are real PoW/state/root validations, not network-admitted blocks and not recursive-proof soundness tests.
- No live JSON-RPC authorization test was added; authorization conclusions are from middleware and startup review. No live worker behavior, multi-node race, fork-height matrix, or every proof class was exercised.
- No upstream source was edited and no fix was applied. Pre-existing unrelated working-tree modifications in the repository were left untouched.
