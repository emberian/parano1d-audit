# agent-18: Probe ledger fairness liveness

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Implemented five local experiments against archived commit `52938433a30b185ab007afaa15166a632cf2e82e`. No live nodes or peers were used, upstream files were not edited, and native-trade worst-fill and identical-intent/same-anchor replay were not repeated.

### Findings

- **Medium, conditional liveness bug — HeaderDAG capacity stall.**
  With a selected linear ancestry at capacity, the next higher-work child returns `HeaderDagError::Capacity` and the best tip remains unchanged. Every node is protected by eviction rules. Production reconciliation inserts the child before advancing finality, so it retries the failure while `header_dag_faulted` blocks exact-suffix application.
  Preconditions: filling the 1,024-node production DAG requires valid PoW headers; this is not a free attack. Restart, snapshot, or finality-driven reconstruction can recover. Relevant code: `noid_node/src/networking/header_dag.rs:287`, `noid_node/src/main.rs:4793`, `noid_node/src/main.rs:9737`.

- **Low, local RPC cost externality — unbounded owner query.**
  `paranoid_getSlotsByOwner` authenticates and returns every UTXO while holding `chain.read()`. For 4,096 UTXOs versus a 32-record page, final median costs were **347 µs versus 60 µs (5.78×)**; result counts differed 128×. These are small warm-cache measurements, not network-wide latency estimates. The method is local-only under the pinned credential policy, so no remote consensus DoS was demonstrated. Code: `noid_rpc/src/server.rs:2023`, `noid_chain/src/storage/mdbx_store.rs:2798`.

- **Low, first-seen fairness policy — higher-fee conflict rejected.**
  A 9,000-μNOID transaction remained selected while a 100,000-μNOID transaction with different inputs/owners but the same output slot was rejected with `SlotConflict`. An attacker must observe the slot, own a valid input, and win admission; impact is temporary pinning and forced reproving, not theft or permanent censorship. Code: `noid_mempool/src/pool.rs:1296`.

- **Low, mempool-versus-block race — coinbase collision evicts pending spend.**
  A remote-style block omitted the victim and minted its coinbase into pending output slot 161, causing `OutputSlotOccupied` eviction. The local pending-aware template avoided the collision by choosing slot 162. A miner must choose the colliding empty slot and win the block; it can already censor by omission, so the additional impact is forced reauthorization. Code: `noid_chain/src/consensus/template.rs:501`, `noid_mempool/src/pool.rs:816`.

- **Low, documented reorg liveness trade-off — no automatic reclaim.**
  After confirmation removal and a replacement restoring spendability, `readmit_after_reorg` left the pool empty. A fresh, different intent was accepted manually. Inclusion therefore depends on an online wallet or another retained-intent source. Only non-final replacement was tested; identical-intent replay was explicitly not tested. Code: `noid_mempool/src/pool.rs:909`, `noid_node/src/main.rs:4676`.

No soundness violation, finalized-prefix reversal, concrete stale-authorization acceptance, or deterministic low-cost network-wide consensus DoS was established.

### Deliverables

- `experiments/ledger-fairness/src/main.rs` — all five real-API experiments.
- `experiments/ledger-fairness/results.json` — final machine-readable results.
- `experiments/ledger-fairness/REPORT.md` — detailed evidence, preconditions, severity, and negative checks.
- `experiments/ledger-fairness/Cargo.toml` and `Cargo.lock`.
- `experiments/ledger-fairness/pinned/` — archived source; exactly one disclosed fixture-only change at `pinned/noid_chain/src/storage/mdbx_context.rs:1159` enables the existing full owner-index rebuild for batch-seeded state. Query and consensus logic are otherwise unchanged.

### Verification

All experiment assertions passed. Additional checks:

- `noid_node header_dag::tests`: **8 passed**
- `noid_mempool reorg` tests: **6 passed**
- `noid_chain owner_pages` test: **1 passed**
- Harness `rustfmt --check`: passed
- `results.json`: valid JSON

Runs used the optimized dev profile. Release mode was not used because the standard build requires external authenticated matrix packs and retirement keys; those checks were not bypassed. Mempool experiments used an accept-all authorization executor, and the storage fixture used a test terminal verifier, so authorization/proof soundness is outside these claims. Build artifacts were removed; pre-existing upstream modifications were left untouched.
