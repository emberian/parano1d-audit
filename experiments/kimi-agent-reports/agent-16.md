# agent-16: Attack artifact identity

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Implemented and ran local PoCs at pinned `52938433a30b185ab007afaa15166a632cf2e82e`. Only `experiments/identity-encoding` was changed.

## Finding: rejected proofs suppress later recovery of the same txid

**Low-severity, scan-bounded availability flaw. No unauthorized state change.**

In `noid_node/src/main.rs:10619-10631`, mempool recovery:

1. Decodes an intent and derives its logical txid.
2. Appends that txid to `observed_txids` at `noid_node/src/main.rs:10623`.
3. Only afterward calls `submit_decoded`, ignoring rejection for recovery bookkeeping.
4. `MempoolRecovery::finish` inserts the rejected txid into `state.seen` at `noid_p2p/src/mempool_recovery.rs:240-245`.
5. Subsequent `known()` calls exclude it at `noid_p2p/src/mempool_recovery.rs:142-159`.

Consequently, a malformed or malleated authorization can cause a later **valid** intent with the same body/txid from that peer to be skipped for the remainder of the current scan.

The real PoC at `experiments/identity-encoding/src/main.rs:101` submits a nonempty malformed authorization to the actual `AsyncMempool`, obtains `InvalidProof`, confirms the txid is absent from the pool, and then drives the real `MempoolRecovery` state machine:

```text
RECOVERY_BUG txid=96cdb47a268d4bb7f39e2144bb49c07b0b7ac6e662b6c2fa39ead7b634d84e31 admission=InvalidProof pool_contains=false current_scan_known=1 fresh_scan_known=0
```

The fresh-scan check confirms this is **not persistent mempool poisoning**: reconnection/a new scan clears the exclusion. It also does not admit the transaction or alter chain state. A malicious peer can already withhold its own transactions, so the main concern is incorrect exclusion after malformed/malleated relay rather than a new consensus attack.

Recommended fix: add txids to `observed_txids` only after `Ok(hash)` or a confirmed `AlreadyAdmitted(hash)` result. Invalid submissions should produce no recovery progress rather than becoming known-good exclusions.

## Proof and artifact identity assessment

- `experiments/identity-encoding/src/main.rs:138` constructs two different intents with identical pages and different authorization bytes. They have the same txid. Using an explicitly synthetic first-proof verifier to isolate cache behavior, the actual mempool admits the first, rejects the second as `AlreadyAdmitted`, invokes the verifier only once, and retains the first bytes:

  ```text
  PROOF_IDENTITY txid=96cdb47a268d4bb7f39e2144bb49c07b0b7ac6e662b6c2fa39ead7b634d84e31 distinct_intents=true second=AlreadyAdmitted verifier_calls=1 retained=proof-A pool_len=1
  ```

  The synthetic verifier is **not** evidence that either blob is a valid proof. It demonstrates that proof-only malleability does not create a second txid or replace an existing cache entry. The production block fast path also compares canonical proof bytes and falls back to full verification for changed bytes (`noid_mempool/src/pool.rs:1144-1177`).

- `experiments/identity-encoding/src/main.rs:168` assesses the transcript lane’s system-level implications without reproducing its final-codeword primitive. Two terminal byte strings with identical public claim metadata produce:

  ```text
  TERMINAL_IDENTITY block_hash=31b664924d2771bd2d7605fbf47b3828940fcf0c12b35cb24b6d87cb45d33867 same_claim=true distinct_object_ids=true original_matches_mutant=false framing_only=true
  ```

  Thus, **if** a legacy-FRI mutation is cryptographically accepted, it retains the block hash and terminal claim but receives a different `TerminalObjectId`; the original object ID does not match the mutated bytes. This prevents byte-addressed artifact-cache confusion. The PoC proves public bundle framing and identity only, not cryptographic acceptance of the synthetic terminal.

- `experiments/identity-encoding/src/main.rs:272` confirms uppercase and surrounding-whitespace Bech32 aliases decode to the same 32-byte address. This is a textual canonicality/documentation mismatch, not a binary identity or state vulnerability:

  ```text
  ADDRESS_TEXT lowercase=true uppercase=true surrounding_whitespace=true same_32_bytes=true binary_identity_changed=false
  ```

- Canonical-wire and duplicate controls passed:

  ```text
  NEGATIVE_CONTROLS trailing=reject invalid_bool=reject duplicate_group=DuplicateInputSlot duplicate_block=SlotConflict
  ALL_ASSERTIONS_PASSED
  ```

I found no different-state/one-root collision or duplicate-transaction state transition in the inspected transaction, native-opening, trade, and block-tree paths. Exact duplicate groups are rejected before application.

## Verification

Commands run successfully:

```bash
cd /Users/ember/dev/parano1d-break/experiments/identity-encoding
cargo check --locked
cargo run --locked
```

Upstream controls:

- `cargo test --locked -p noid_tx wire::tests`: **4 passed**
- `cargo test --locked -p noid_chain derived_txid_keys_and_duplicate_admission`: **1 passed**
- `cargo test --locked -p noid_p2p mempool_recovery`: **12 passed**

## Files created

- `experiments/identity-encoding/Cargo.toml`
- `experiments/identity-encoding/Cargo.lock`
- `experiments/identity-encoding/src/main.rs`

Generated `target/` artifacts were removed. Existing `parano1d` modifications were left untouched. An untracked `parano1d/domain-audit.json` appeared during the run window; its origin is uncertain, so it was also left untouched.
