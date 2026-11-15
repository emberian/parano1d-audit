# Wallet receipt-pagination audit (RPC API surface)

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. The PoC drives the real `noid_rpc::start_rpc_server` HTTP/JSON-RPC stack in-process over loopback with a synthetic wallet holding 20,000 real, cryptographically valid `ParanoidReceipt`s (each one passes `verify_merkle_inclusion` at generation). No live node, peer, or external network was contacted; synthetic keys only.

This lane inherited an unrunnable draft from a dead agent: its `src/main.rs` did not compile (three imports pointed at the wrong module) and had never been executed. The draft's allocator instrumentation and synthetic receipt library were kept after verification; the measurement plan was corrected and extended (its original framing treated the far-page heap as the whole story and missed that every page pays the full rescan).

## Finding 1: every `paranoid_walletQueryReceipts` call rescans and re-verifies the entire receipt store

**Severity: Medium availability, requiring the operator bearer credential or loopback owner access. No integrity impact; anonymous remote clients cannot reach the method.**

`paranoid_walletQueryReceipts` is on the operator allowlist (`noid_rpc/src/server.rs:3861`). For every call — any page, including page 1 — `scan_receipts` (`noid_rpc/src/server/receipt_query.rs:97-171`):

1. fetches and **clones every stored receipt** (`receipt_query_bytes`, production impl at `noid_node/src/wallet/mod.rs:978-989`, per-receipt clone under the wallet mutex),
2. decodes it (`ParanoidReceipt::from_bytes`) and checks key identity,
3. **re-runs Merkle-inclusion verification and bech32 metadata construction** for every receipt (`ReceiptSummaryMetadata::authenticated` -> `ReceiptSummaryInfo::authenticated`, `noid_rpc/src/types.rs:68-75`, which calls `verify_merkle_inclusion`),

and only then keeps a top-k heap of size `required = offset + page_size` for the requested page. Nothing is cached between calls.

Measured with the real handler end-to-end (20,000 receipts): **every page-1 call costs 4.1-5.6 s wall (~205-225 us/receipt)**, and the cost is identical for every other page (A1-A4, A6). A wallet GUI walking 400 pages of a 20k-receipt wallet burns ~28 minutes of one core for a single pass; a credentialed or local client can keep the blocking-pool thread hot indefinitely at ~zero bandwidth cost. Cost is bounded by existing guardrails: a **single-permit gate** rejects concurrent queries immediately (A5; `receipt_query.rs:11,85-87`), the scan runs on `spawn_blocking` (async workers not blocked), output is bounded (`page_size <= 50`), and stored receipts are size-capped (`MAX_RPC_RECEIPT_BYTES = 128 KiB`, `noid_chain/src/consensus/wire_limits.rs:112`). So this is a serialized, authenticated, self-inflicted-or-malicious CPU burn that scales linearly with receipt count — not an unauthenticated amplification.

### Recommended correction

Persist the sort key `(height, timestamp, id, kind)` alongside each stored receipt (it is already authenticated data) and paginate the index directly instead of re-decoding and re-verifying every receipt per call; alternatively cache the verified key vector keyed by the existing `query_revision()` (it already invalidates on any receipt mutation). Keep the single-permit gate.

## Finding 2: out-of-range pages retain the complete key set in memory

**Severity: Low (allocation amplification only), same preconditions as Finding 1.**

`required = offset + page_size` is caller-controlled (`page` up to `u32::MAX` makes `required` ~2.1e11), so for any page at or past the end of the result set the top-k bound never engages and the `BinaryHeap` retains all N keys (~88 bytes each after padding). Measured: handler allocation peak **2,898,757 B for a page returning 0 rows vs 20,805 B for page 1** (+2,879,952 B; A4-A1). Retention begins at the first page past the end (A3, page 401), not only at extreme pages; in-range pages stay bounded (A2, 26,437 B). The retention is bounded by the receipt count and transient (freed with the response), so it is an allocation multiple of Finding 1's own O(N) working set rather than an independent unbounded sink.

### Recommended correction

Cap `required` at the running `total` once known is not possible in one pass; instead bound the heap by `total`-agnostic means: skip heap insertion once `total > required` cannot change the retained set, or simply reject/clamp `page` to `total_pages` computed from a cheap count, or bound `required` by a sane maximum page number.

## Finding 3: `paranoid_walletReceipts` scans inline on async worker threads, wallet mutex held, no gate

**Severity: Low-Medium availability. Dynamically demonstrated with a faithful mirror of the production scan; production behavior inferred from code identity (see limits).**

Unlike the query variant, `wallet_receipts` calls `filtered_receipts` directly inside the async handler (`noid_rpc/src/server.rs:2643-2646`) — no `spawn_blocking`, no semaphore, and the `wallet_operation_gate` that serializes send/scan operations (server.rs:872, 2772) is **not** taken. The production scan (`noid_node/src/wallet/mod.rs:910-967`) holds one `std::sync::Mutex` across the entire O(N) pass, so each concurrent call parks a tokio worker thread on that mutex while the scans themselves serialize.

Measured with the mirrored scan behind the real handler and real HTTP stack (decode-only cost ~1.3 us/receipt, 25 ms per 20k-receipt page): 16 concurrent calls serialize (walls ~0.31 s each; cumulative mutex-held window 1.41 s), and unrelated `paranoid_getChainInfo` probes stall up to **310,884 us vs 101 us idle p50 (~3078x tail inflation)** on a 4-worker runtime (B1-B6). Production nodes default to CPU-count workers; the stall window scales linearly with the receipt count and per-receipt decode cost. Same credential preconditions as above (`paranoid_walletReceipts` is operator-allowlisted at server.rs:3860).

### Recommended correction

Route `wallet_receipts` through the same pattern as the query variant: `spawn_blocking` plus the existing (or a shared) single-permit gate, or acquire `wallet_operation_gate` before entering the scan.

## Negative and control results

- **Concurrency gate works**: of two simultaneous `walletQueryReceipts` calls, exactly one runs; the other is rejected immediately with "a receipt query is already running; retry shortly" (A5). Parallel amplification on the query path is capped at 1 per node.
- **No cross-call caching**: repeated identical page-1 calls each pay the full rescan (A6: 4.1-4.5 s each) — this is both the defect of Finding 1 and the control showing there is no hidden memoization that would blunt it.
- **No forgery path**: the handler re-verifies every receipt's Merkle inclusion and rejects key/identity mismatch (`saved receipt identity differs from its key`); all 20,000 synthetic receipts pass real verification and are returned with correct totals (total = 20,000 on every page).
- **Anonymous remote RPC is structurally prevented**: a non-loopback listener without a configured bearer credential refuses to start (`noid_rpc/src/server.rs:3520-3522`); loopback without credentials is served (C2), which the documentation calls intentional owner access.
- **Browser-origin rejection**: a request carrying any `Origin` header is refused HTTP 403 before JSON-RPC dispatch (C1; server.rs:3750-3757) — no drive-by loopback access from web pages.
- **Credential scoping**: the operator credential is restricted to an explicit 18-method allowlist (server.rs:3850-3872); the mining credential to `getBlockTemplate`/`submitBlock` (server.rs:3846-3848, consistent with the external-miner lane).
- **No credential exposure to logs**: `Authorization` headers are counted and removed before jsonrpsee tracing sees the request (server.rs:3759-3767).
- **Request-size and schema bounds**: HTTP request bodies are capped at `2*MAX_RECEIPT_BYTES + 16 KiB` (server.rs:63-64), `ReceiptQuery` uses `deny_unknown_fields` with validated ranges, and page/page_size bounds are enforced before any scan — no oversized-payload or unknown-field path found.
- **No panics observed** on any exercised path; malformed data would surface as JSON-RPC errors via the `map_err(rpc_err)` chains.

## PoC implementation and reproduction

`src/main.rs` starts the real pinned RPC server (`noid_rpc::start_rpc_server`) against an MDBX chain context and a `SyntheticWallet` implementing the real `WalletOps` trait:

- Phase A (Findings 1-2) uses the wallet's genuine query hooks (`receipt_query_index`/`receipt_query_bytes`/`receipt_query_records`), so the full production handler path in `server/receipt_query.rs` executes. A thread-local tracking global allocator measures the handler's allocation peak on the blocking thread between the first receipt fetch and record materialization (the window covers the scan loop, heap growth, sort, and page slice).
- Phase B (Finding 3) implements `receipts()` as a faithful mirror of the production scan body (`noid_node/src/wallet/mod.rs:910-967`: one mutex held across the whole pass, per-receipt decode + identity check + recipient classification, newest-first sort, offset/limit slice) and of the kind-0 record arm (`noid_node/src/wallet/receipt_query.rs:117-149`). The production `WalletState` could not be linked directly: `noid_node`'s library (`src/lib.rs`) exports no wallet module, so the type is binary-private.

The synthetic receipts are real payments to a different recipient (the inclusion rule of `types/receipt_query.rs:189-199`), each generated with the real `generate_receipt` and asserted to pass `verify_merkle_inclusion`.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final output includes (`results.txt` holds the complete tee'd run):

```text
A1 page 1      (offset     0, required    50): 50 rows, wall 5616038 us, handler alloc peak    20805 B
A3 page  401   (offset 20000, required 20050): 0 rows, wall 4451312 us, handler alloc peak 2898757 B
A4 page u32::MAX (offset 214748364700, ...): 0 rows, wall 4162763 us, handler alloc peak 2898757 B
A4-A1 out-of-range peak amplification: 2877952 B
A5 concurrent-query gate: winner returned 50 rows; loser error: a receipt query is already running; retry shortly
A6 repeated page-1 walls: [4514004, 4442464, 4118766] us (each a complete rescan; no cross-call cache)
A7 query-path per-receipt cost: ~225 us/receipt (fetch+clone+decode+identity+merkle re-verification+bech32 metadata)
B4 getChainInfo during load: p50 64 us, max 310884 us over 1096 probes
B6 worker starvation factor: max_during/idle_p50 = 3078x
C1 browser Origin header -> HTTP 403 Forbidden (must be 403 Forbidden)
ALL ASSERTIONS PASSED
```

## Assurance limits

- Phase B runs a re-implementation of the production scan (line-cited, same real receipt crypto and lock shape), not the linked production function; per-receipt constants may differ slightly (production uses a `HashMap` store, the mirror a `BTreeMap`). The Phase A numbers are the real handler end-to-end against the real trait plumbing; only the three innermost `WalletOps` hooks are synthetic.
- The allocation instrumentation excludes the initial full id-list clone in `receipt_query_index` (~640 KB at 20k) and response serialization, so true per-call allocation is slightly higher than the reported peaks; the A4-A1 delta is unaffected.
- All measurements are from one laptop, one process, 20,000 receipts, `worker_threads=4`; query-path cost scales ~linearly with receipt count and the Phase B stall window scales with scan duration. No multi-gigareceipt, reorg-window (`canonical headers changed`), multi-wallet, or GUI-behavior scenario was exercised.
- All findings require the operator bearer credential or loopback owner access — the same trust class as the external-miner lane's finding; loopback owner access is documented as intentional, and the operator credential is what the node's own accounting/payout tooling holds. Nothing here is reachable by an anonymous remote client.
- No live node, peer, or real credentials were used; no upstream source was edited; the pre-existing unrelated working-tree modifications in the parano1d checkout were left untouched.
