# Ledger fairness and liveness experiments

Source: `52938433a30b185ab007afaa15166a632cf2e82e` (`52938433`). All execution was local; no node, peer, socket, or live network was used. The native-trade worst-fill and identical-intent/same-anchor replay experiments were not repeated.

## Result summary

| Experiment | Demonstrated result | Classification and severity |
| --- | --- | --- |
| Selected HeaderDAG ancestry at capacity | The next valid child is rejected with `Capacity`; the best tip does not advance. | **Medium, conditional liveness bug.** Requires a full DAG of valid PoW headers; not a cheap attack and not a finality/soundness violation. |
| Unbounded owner query | 4,096 records versus 32 returned; median full/page cost was 347 µs/60 µs (**5.78×**) in the final warm-cache run. | **Low RPC cost externality.** Local-only RPC in this revision; linear in owner holdings, not a demonstrated consensus DoS. |
| First-seen output conflict | A 9,000-μNOID transaction stayed admitted while a non-conflicting-input transaction paying 100,000 μNOID for the same output slot was rejected with `SlotConflict`. | **Low fairness/liveness policy.** Documented first-seen behavior, not a soundness bug. |
| Remote coinbase/output race | A block that omitted the pending spend filled its output slot and caused `OutputSlotOccupied` eviction. The local pending-aware template avoided the slot. | **Low fairness/liveness policy.** Best-effort local avoidance is not a consensus reservation. |
| Reorg reclaim | After the old confirmation was removed and the replacement restored spendability, `readmit_after_reorg` left the pool empty. A newly constructed intent was accepted manually; identical-intent replay was not tested. | **Low documented liveness trade-off.** Inclusion depends on an online wallet or another retained-intent source. |

No invalid state transition was accepted and no soundness failure was demonstrated. The experiments also do not establish a deterministic, low-cost, network-wide consensus DoS: the HeaderDAG case has a substantial valid-header precondition, while the query case is local RPC work.

## 1. HeaderDAG selected-ancestry capacity stall

The experiment constructs two linear best-chain headers above a finalized root with `max_nodes = 2`. The third child, which has greater cumulative work, returns `HeaderDagError::Capacity`. Advancing finality manually and retrying makes the same child succeed.

Relevant production behavior:

- `noid_node/src/networking/header_dag.rs:287` rejects at capacity when no leaf can be evicted.
- `noid_node/src/networking/header_dag.rs:321` protects the complete best ancestry and incoming-parent ancestry. On a full linear selected chain, every node is protected, so the next child cannot be inserted.
- `noid_node/src/main.rs:4793` inserts a missing canonical child during reconciliation, but finality is advanced only at `noid_node/src/main.rs:4805`. Thus reconciliation retries the failing insertion rather than reaching the pruning step.
- A capacity failure sets `header_dag_faulted` (`noid_node/src/main.rs:14463`), and exact-suffix application is disabled while that flag is set (`noid_node/src/main.rs:9737`).

Production uses 1,024 nodes (`noid_node/src/main.rs:4711`), not the small test value two.

**Preconditions:** enough independently valid PoW headers to fill the selected DAG ancestry to capacity. The experiment does not show that an attacker can obtain those headers cheaply or that ordinary operation reaches the cap. A restart or snapshot/finality-driven reconstruction can recover; this is not an irreversible fork or finalized-prefix violation.

**Severity:** Medium conditional liveness. Once the precondition holds, failure is deterministic and affects normal children, not only hostile siblings.

## 2. Unbounded owner-query work

`paranoid_getSlotsByOwner` acquires `chain.read()` and synchronously calls the complete owner lookup (`noid_rpc/src/server.rs:2023`). The store iterates every index record, authenticates it against durable sparse state, and appends every UTXO (`noid_chain/src/storage/mdbx_store.rs:2798`, `:2894`). A separate API already provides pages capped at 256 (`noid_chain/src/storage/mdbx_store.rs:2691`).

The fixture committed one real PoW-valid block and then compared the real storage calls for one owner with 4,096 UTXOs in one sparse segment:

- full query: 4,096 records, 347 µs median, 371 µs cold;
- 32-record page: 32 records, 60 µs median, 116 µs cold;
- median ratio: 5.78×; result-count ratio: 128×.

These are small, warm-cache local measurements. They exclude JSON serialization, network transport, larger multi-segment reads, and any direct measurement of writer delay, so they should not be presented as a global latency estimate.

**Preconditions:** native loopback RPC access and a target address with many UTXOs. In this revision, remote mining/operator credentials do not allow this method (`noid_rpc/src/server.rs:3846` and `:3850`), so an unauthenticated Internet caller cannot invoke it through the standard listener.

**Severity:** Low RPC cost-externalization/liveness risk. The unbounded response and verification loop are real, but no consensus-layer impact or remote attack was demonstrated.

## 3. First-seen output-conflict pinning

Two transactions with different input owners and different live inputs claimed the same empty output slot. The first paid 9,000 μNOID; the second paid 100,000 μNOID. `AsyncMempool::submit` accepted the first and rejected the second with `Consensus(SlotConflict)`. Anchored block selection retained only the first.

Admission checks conflicts against already admitted reservations (`noid_mempool/src/pool.rs:1296`), and the synchronous pool has no fee-based replacement path (`noid_chain/src/mempool.rs:623`).

**Preconditions:** the racer observes the victim's output slot, controls an unrelated valid input, and wins the admission race. It must also pay authorization and eventual transaction costs. A miner remains free to omit the attacker's transaction, and the victim can choose a fresh slot and reprove.

**Severity:** Low fairness/liveness policy. It enables temporary pinning and forced reauthorization, not theft, double-spending, or permanent protocol-level censorship.

## 4. Coinbase-versus-pending-output race

A pending spend chose output slot 161. A remote-style template built without pending-output knowledge placed its coinbase in slot 161 and omitted the spend. Applying that block caused the real mempool cleanup to evict the spend with `OutputSlotOccupied`. Passing the pending set to the node-owned template selected slot 162 instead.

The avoidance loop is explicitly a local preference (`noid_chain/src/consensus/template.rs:501`), while `on_new_block` evicts any pending output occupied by the new state (`noid_mempool/src/pool.rs:816`).

**Preconditions:** a miner chooses an empty slot already selected by a pending transaction, either deliberately or by natural allocation race, and its block wins. The targeted transaction is not included.

**Severity:** Low fairness/liveness policy. The collision forces fresh slot hints and reproving. A miner can already censor by omission, so this adds a reproducible invalidation mechanism rather than a new consensus violation or fund-loss primitive.

## 5. Reorged transactions require external resubmission

The experiment first confirmed and removed a transaction, then installed a same-height replacement that restored its input and output availability. The production sequence `on_new_block` followed by `readmit_after_reorg` left the pool empty. A fresh, different intent was then admitted manually; no identical-intent replay was performed.

This behavior is explicit in `noid_mempool/src/pool.rs:909`: original authorization bytes are not retained for automatic re-admission, and wallets must resubmit. The node invokes that hook after a reorg at `noid_node/src/main.rs:4676`.

**Preconditions:** a non-final reorg displaces a previously confirmed transaction, the replacement does not include it, and the original wallet or another intent source must resubmit. Offline wallets can leave the transaction delayed indefinitely.

**Severity:** Low documented liveness trade-off, not an implementation surprise or finality failure. The experiment did not cross the finalized checkpoint.

## Checked without a positive exploit finding

- **Stale-state authorization:** ordinary admission repeats state, anchor, resource, and authorization-version checks after proof verification (`noid_mempool/src/pool.rs:447`); native admission similarly recomputes its current context (`noid_mempool/src/native.rs:106`). No concrete stale-authorization acceptance was established, and no replay experiment was substituted for one.
- **Builder ordering and incentives:** arbitrary block ordering is explicitly miner policy rather than consensus (`noid_chain/src/consensus/ordering.rs:4`). The experiments therefore measure specific inclusion/eviction behavior instead of claiming that discretionary ordering itself is a bug.
- **Verification-cost asymmetry / cheap consensus DoS:** block validation performs bounded page/resource preflights before proof and state-heavy work (`noid_chain/src/consensus/validation.rs:151`). The examined fixed caps did not yield a concrete zero- or low-cost consensus DoS. This is a negative result for the tested paths, not a proof that none exists.
- **Finality:** the reorg experiment exercised only a replaceable, non-final confirmation. It did not show finalized-prefix reversal or same-anchor replay.

## Reproduction and verification

From `experiments/ledger-fairness`:

```sh
CARGO_TARGET_DIR="$PWD/target/cargo" cargo run | tee results.json
CARGO_TARGET_DIR="$PWD/target/cargo" cargo test -p noid_node header_dag::tests --lib
CARGO_TARGET_DIR="$PWD/target/cargo" cargo test -p noid_mempool reorg --lib
CARGO_TARGET_DIR="$PWD/target/cargo" cargo test -p noid_chain owner_pages --lib
rustfmt --edition 2021 --check src/main.rs
```

Results:

- experiment binary: all behavioral assertions passed; output is in `results.json`;
- HeaderDAG unit tests: 8 passed;
- mempool reorg-filtered tests: 6 passed;
- owner-page storage test: 1 passed;
- harness formatting check passed.

The run used the optimized dev profile. A release build was not used because this checkout's standard release build requires external authenticated matrix packs and retirement keys; those checks were not disabled or bypassed.

## Fixture boundaries and touched files

`src/main.rs` contains all five experiments. Mempool submissions use the real `AsyncMempool` API with an accept-all authorization executor hook, so authorization soundness is outside these claims; canonical decoding, fees, state checks, conflicts, selection, and eviction are real. The MDBX fixture uses a test HistoryStep terminal verifier but real PoW, public transition validation, state materialization, and query code.

The archived `pinned/` tree came from the pinned commit. Exactly one fixture-only adjustment exists at `pinned/noid_chain/src/storage/mdbx_context.rs:1159`: the existing full owner-index rebuild flag is enabled so batch-seeded pre-parent state is indexed. Query and consensus logic are otherwise unchanged. The upstream repository was not edited.
