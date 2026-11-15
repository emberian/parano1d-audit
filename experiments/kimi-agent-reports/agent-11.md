# agent-11: Attack UTXO state machine

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Result

No accepted-invalid UTXO/state transition was found at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`.

The local matrix exercised **20 adversarial cases and 2 valid controls**. Every invalid case was rejected by transaction semantics, full native block checks with actual PoW, exact action-surface construction, or block materialization. This is a negative result for the tested matrix, not a proof that the protocol is vulnerability-free.

## Deliverables

Created only in `experiments/state-consensus`:

- `Cargo.toml` — standalone local harness with isolated v2.1 activation.
- `Cargo.lock` — reproducible dependency resolution.
- `src/main.rs` — executable PoC and adversarial matrix.
- `output.txt` — complete final run output.

Important locations:

- Full PoW, block checks, epoch anchors, native exact surface, materialization, and wire roundtrip: `experiments/state-consensus/src/main.rs:282`
- Native valid control: `experiments/state-consensus/src/main.rs:340`
- Native conservation/authorization/slot attacks: `experiments/state-consensus/src/main.rs:355`
- Coinbase/counter/expansion/allocator attacks: `experiments/state-consensus/src/main.rs:526`
- Ordinary-vs-native differential materialization: `experiments/state-consensus/src/main.rs:644`
- Ordinary UTXO matrix: `experiments/state-consensus/src/main.rs:675`

Run locally with:

```bash
cd /Users/ember/dev/parano1d-break/experiments/state-consensus
CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/target cargo run --release
```

Final verification:

- `cargo fmt -- --check` — passed.
- `cargo check --release` — passed.
- Final `cargo run --release` — exited successfully; output saved to `output.txt`.
- Only two pre-existing dead-code warnings in `noid_chain` were emitted.

## Evidence

Valid native `CreateCollection` control passed all tested boundaries:

```text
accept valid CreateCollection control:
active=6 alloc=13 supply=16397500
native_slot_10_id=4611686018427387916 nonempty=true
```

The native ID is `2^62 | 12`, confirming correct native allocator sequencing after the reserved coinbase increment. Supply independently matches:

```text
2,000,000 initial - 9,000 fee
+ 14,400,000 miner subsidy + 6,500 claimable fees
= 16,397,500
```

Rejected cases included:

- NOID output inflation: `Semantic(NoidConservation)`
- Forged collection identity: `Semantic(Supply)`
- Non-owner NFT transfer: `Semantic(Authority)`
- Duplicate native inputs/outputs: `InputLayout` / `OutputLayout`
- Same native input in two groups: `SlotConflict`
- Occupied or out-of-range output slots: `OutputSlotNotEmpty` / `SlotOutOfRange`
- Coinbase subsidy `+1`: `InflatedCoinbase`
- Claiming the 2,500 state-growth burn: `InflatedCoinbase`
- Header `active_slot_count +1`: `HeaderActiveSlotCountMismatch`
- Header `alloc_counter +1`: `HeaderAllocCounterMismatch`
- Untriggered expansion: `BadLogSlotsExpansion`
- Mint beyond native allocator maximum: `AllocCounterOverflow`
- Ordinary output inflation: `BalanceMismatch`
- Wrong input owner or `creation_id`: `UnknownOrSpentInput`
- Duplicate ordinary inputs/outputs and input/output overlap: explicit duplicate/conflict errors

The ordinary zero-amount, zero-owner control was also accepted consistently by both materializers and both exact surfaces:

```text
accept ordinary zero/zero output control:
slot_10_id=12 nonempty=true;
ordinary/native materializers and exact surfaces agree
```

## Designed behavior distinguished from bugs

- A zero-amount, zero-owner ordinary output is legal. Its fresh nonzero `creation_id` makes the stored slot nonempty; it is not an empty mint or supply-conservation failure.
- Zero-amount native openings likewise remain nonempty through their tagged native `creation_id`.
- Cheap block checks intentionally leave exact counters to the transition layer. The forged-counter cases passed the state-free portion but were rejected by exact action-surface/materialization checks, as designed.
- The height-25 miner receives the scheduled 90% subsidy while the development share accrues; using the gross 16,000,000 reward as the miner payment would be incorrect.
- Caller-trusted lower-level APIs such as direct transaction application or verifier-sealed transition commitment were not treated as standalone network acceptance. Production inbound materializers recompute roots and counters.

## Preconditions and exact limitations

- The fixture is synthetic and local: `log_slots=8`, four initial UTXOs, allocator 10, initial supply 2,000,000, child height 25.
- `isolated-v2-1-fork-testnet` lowers activation heights only; tested fee, state, allocator, and typed-operation logic remains the production implementation.
- Adversarial blocks are sealed with real PoW using a bounded 2,000,000-nonce local search.
- The harness exercises full native cheap block checks, exact epoch anchors, real block/state-delta APIs, native and ordinary materializers, and wire roundtrips.
- It does **not** generate recursive `HistoryStep` or owner-authorization proofs and does not claim full node end-to-end acceptance.
- Financial owner/public predicates were reviewed, but every financial operation, trade settlement, reorg/snapshot path, and resource-exhaustion case was not exhaustively executed.
- Same-anchor replay was intentionally excluded.

No `parano1d/` source was edited. The repository already had unrelated modifications; the only modified direct dependency file was `noid_tx/src/native_assets.rs`, whose diff is module-declaration ordering only. An unrelated untracked `parano1d/domain-audit.json` appeared during concurrent work; its origin could not be established, so it was left untouched.
