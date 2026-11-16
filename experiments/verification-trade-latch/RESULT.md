# NM-2B verification result

**Observed:** the configured local pool latches a high relay fee floor after its
ordinary high-fee transactions are confirmed in an accepted block. Three
distinct, validly authorized, funded trade policies for an absent asset keep
the pool at half capacity, although those entries do not add fee samples.
An exact-fee `CreateLaunch` passes admission in a fresh pool at the same tip
and fails with `BelowMinFee` in the latched pool.

The release runner exited 0. Its [complete output](run.log) ends:

```text
authentic V4 trade admitted: entry 1 / 6, floor 5000
authentic V4 trade admitted: entry 2 / 6, floor 5000
authentic V4 trade admitted: entry 3 / 6, floor 5000
PASS: accepted block 24 confirmed both ordinary spends; three verified V4 unindexed trades remain at pool 3/6 and floor 90000; fresh pool accepts exact-fee CreateLaunch (8600), latched pool rejects it
```

## Exact local sequence

1. Mine and accept 23 PoW blocks under the isolated v2.1 schedule, creating
   distinct live coinbase funding UTXOs. Initialize `AsyncMempool` at capacity
   6 with its default authorization verifier.
2. Generate and submit three separate V4 trade proofs. The trades reserve
   distinct funding slots; their policy asset `[aa; 32]` was never created in
   this chain. The pool is 3/6 and its floor is 5,000 µNOID.
3. Generate and submit two V4 ordinary self-transfers at 100,000 µNOID each.
   The pool is 5/6 and its floor is 90,000 µNOID. Ordinary block selection
   returns exactly those two transfers, never the trade policies. The fixture
   checks each admitted logical PagedSpend ID against `validate_paged_spend`.
4. Mine and accept block 24 with both transfers. Advance the mempool using
   the accepted chain view and their confirmed logical IDs. It checks that
   the built block actually contains both submitted bodies before applying
   it. The three trade entries remain at 3/6, and the floor remains 90,000
   µNOID.
5. Construct and prove an exact-fee `CreateLaunch` for candidate block 25.
   Its 8,600 µNOID fee is valid and admitted to a fresh pool at the same tip.
   The latched pool rejects the identical bytes with `BelowMinFee` requiring
   90,000 µNOID.

The first attempted setup (capacity 5, one ordinary spend submitted first)
was a useful negative control: it exited at a floor assertion because the
floor stayed at 5,000 µNOID after three authentic V4 trades. The fee sample
was forgotten while occupancy was below 50%. The successful setup puts fee
samples into the window at counts 4 and 5 of 6, then leaves trade occupancy
exactly at the strict 50% reset boundary.

## What this establishes

The source mechanism is visible in `noid_mempool/src/pool.rs`: a trade is
counted as a pool entry, but `retain_admitted_intent` calls `floor.record` only
for nontrades. `FeeFloor::update_pressure` activates at 80% and resets only
when both count and retained bytes are *strictly* below 50%. The ordinary
selector in `noid_chain/src/mempool.rs` skips trade entries. The miner trade
selector in `noid_miner/src/template/trades.rs` skips an asset when
`native_trade_parent_at` returns none. No fee is charged for these retained
policies while the asset remains absent.

This is a **local admission availability issue**, not a chain halt or a
consensus-invalid transaction. A peer's pool is local, and the fixture does
not measure propagation, production scale, or attacker cost. Production
capacity is 1024 rather than 6. The accepted block used real PoW, full native
consensus validation, and MDBX application, with a stubbed HistoryStep proof
verification callback; it does not establish a complete recursive terminal or
release daemon path.

The fixture and command are in [README.md](README.md). No live network was
contacted.
