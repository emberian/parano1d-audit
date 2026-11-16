# Contract state machines under reorgs (semantic idempotence audit)

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. The PoC drives the real
`MdbxChainContext` production chain APIs (real templates, real PoW grinding,
real `apply_next_block` and `apply_reorg_mdbx_with_applier` reorgs) against
throwaway local MDBX databases under a tempdir, with real `ObjectOpening`
contract pages (refundable payments, allowance wallets, period budgets)
inside the reorged blocks. No live node, peer, RPC server, or user database
was touched. The `pinned/` crate sources were extracted with
`git archive 52938433` and verified byte-identical to the storage-reorg
lane's pinned copy (which was itself verified against the archive) before
building.

## Orientation: what this lane crossed

Two completed lanes each proved something strong, and this lane tested their
composition:

- **Lane A (contract-rights)**: contract semantics are sound under sequential
  adversarial execution — replay, early unlock, excess allowance, wrong
  recipients all rejected.
- **Lane B (storage-reorg)**: reorg rollback is complete at the STORAGE layer
  (old hash/tx/body removed, owner index moved, displaced body
  content-addressed only) and crash recovery is clean.

The unasked question was whether rollback-completeness at the storage layer
implies SEMANTIC idempotence at the contract layer: does any consumption
counter, phase, or allowance state diverge from a never-reorged execution
when a refund is paid on a losing fork and the winner replays a related
claim?

## Outcome

**No semantic divergence was found.** Every reorged chain, including chains
whose reorged blocks carried live contract state machines (refund closes,
budget draws, allowance races, same-block chained mint+spend transitions),
ended byte-identical to a never-reorged twin that received the same block
sequence: exact state root, circulating supply, `alloc_counter`,
`active_slot_count`, tip height, and every party's owner-index UTXO set.
Cross-fork replay of a losing-fork page after the winner consumed the same
root is rejected (`UnknownOrSpentInput`). Each scenario, its verdict, and
the mechanism that restores consistency:

| # | Scenario | Verdict | Mechanism |
|---|----------|---------|-----------|
| 1 | Refund paid on losing fork, reorg, equivalent replay on winner | **held** | Contract state lives only in the UTXO slot `(amount, creation_id, owner=root)`; the reorg reverts the exact slot set, so the paid state is un-entered and the obligation UTXO resurrects. Replay executes once on the surviving chain; second replay rejected `UnknownOrSpentInput`. |
| 2 | Phase machine under a height-DECREASING reorg across a deadline (payer recovery executed on the long fork at `h >= deadline`, payee claim on the shorter heavier fork at `h < deadline`) | **held** | Phase is a pure function of `(block height, deadline)` evaluated per block: `authority_at`/`recipient_at` flip at the deadline; the terminal recipient binding makes the loser's page invalid in the other phase (`Recipient`), and the authorization statement binds `(txid, height-selected authority)`, so the long fork's payer proof cannot be re-verified below the deadline. The recovery payout UTXO evaporates with the fork. |
| 3 | Consumption-counter divergence (budget drawn on A, reorg to B, draw again on B vs a never-reorged twin) | **held** | The budget counter is program state packed into the successor root; the root lives in the UTXO slot; the undo log restores slot preimages and both counters (`restore_state_counters`, `noid_chain/src/consensus/reorg.rs:25`). Twin equality over root/supply/alloc/active proves no leakage. Budget exhaustion still binds on the winner's own counter. |
| 4 | Allowance race: two authorizations, one per fork, reorg resolves to one | **held** | The contract layer serializes through single-input spend exclusivity: both pages spend the same funded root; the surviving chain contains exactly one, the other's payout UTXO does not exist, and retrying the loser is rejected. A builder can choose WHICH fork to extend, but cannot get both. |
| 5 | Multiple contract machines advancing in ONE block, rolled back and re-applied in both directions; plus the planned same-block mint+spend chaining | **held + structurally excluded** | Same-block chaining of contract calls (spend a successor minted earlier in the same block) is IMPOSSIBLE: the block page-stream validator requires the whole user stream's input-slot set and output-slot set to be disjoint (`noid_chain/src/consensus/paged_spend.rs:325-328`), and `validate_block_slot_conflicts` touches every slot of every tx (coinbase included) exactly once — the PoC's chaining attempt is rejected at the stream level before any state apply. The undo-log comment "a slot may be minted and spent within the same block" (`da_prune.rs`) is defensive-only; no valid block can produce that case. Two INDEPENDENT contract state machines drawn in one block (distinct roots) then reorged in and out restore counters, roots and both funded UTXOs to the twin exactly. |
| 6 | Height-pinning taxonomy of schedules (recurring/budget-reset pinned to one inclusion height; tranche and in-window draws unpinned) | **structural** | The successor root commits to `next = execute(body, height)`; for inclusion-relative schedules (`State1 = Height + period`) the baked successor owner pins the page to exactly one height, so a miner cannot shift a schedule by delaying inclusion and a reorged-out page cannot silently land at another height. Constant-advance schedules (tranche: `State1 += period`) are height-unpinned by construction. |

### The blocker table's structural claim

Contract-visible state is exactly: the UTXO slot set (authenticated by the
exact state root in every header), `alloc_counter` / `active_slot_count`
(restored from the undo log, not re-derived), and the derived supply. There
is **no contract-level side table**: no nonce or consumption registry
outside the UTXO set, no per-contract index that survives a fork, and the
tx-index entries the node keeps are fork-scoped and removed by the same
reorg (lane B). Semantic idempotence under reorg is therefore **structural
(by state-carrier design), not incidental**: the contract's entire state
machine is carried by a content-addressed UTXO root, and reverting the UTXO
set reverts the machine. The height dimension — the one input that does NOT
revert — is re-derived per block by `check_call(page, block_height)` at
every producer and verifier (template witness preparation
`noid_block/src/candidate_history.rs:180`, receipt verification
`noid_block/src/contract_receipt.rs:129`, mempool re-admission checks
`noid_mempool/src/contracts.rs:88`).

Two design properties found while driving the scenarios are worth stating
because they carry the security argument:

1. **The authorization statement binds `(tx_body_hash, expected_address)`,
   never a height** (`OwnerAuthPublicInputs`, `noid_gkr/src/owner_auth.rs`).
   The height selects WHICH authority a page demands
   (`authority_at(height)`), and the terminal recipient binding flips with
   the phase. A page proven for one phase therefore cannot be re-verified in
   the other: the proof names the wrong key and the wrong recipient.
2. **Post-reorg, the node re-derives, it does not trust**: the mempool sweep
   `on_new_block(&[], view)` re-runs every admitted contract call at the new
   candidate height and evicts on any context change
   (`noid_mempool/src/pool.rs:802`, `contracts.rs::eviction_reason` also
   comparing the authority against `admitted_height + 1` so a deadline
   crossing between admission and inclusion evicts); reclaimed txs are NOT
   re-admitted automatically (authorizations are not persisted;
   `readmit_after_reorg` only removes duplicate resubmissions) — wallets
   resubmit through the full admission path. Wallet receipts are re-derived
   from the canonical chain scan (`retain_object_receipts_at_tip`), and a
   receipt verifies only against the canonical header
   (`contract_receipt.rs:156`).

### The one genuinely new mechanical result: height-DECREASING reorgs are real and were exercised with contracts

Fork choice is heaviest-chain regardless of length (upstream test
`more_work_wins_over_longer_chain`). Under the v2 ASERT variant the child
target is computed from the PARENT's timestamp
(`difficulty.rs:141-149`), so two children of one parent always carry equal
work — a same-height fork rivalry can only be decided by the documented
lexicographic hash tie-break, and a shorter fork can never win by a single
block. But a stalled fork compounds compression: each successive stall
block re-anchors from the previous block's already-compressed target, and
after a few blocks the per-block work multiple exceeds the floor blocks the
relaxed competitor accrues. The PoC built a 5-block stalled fork that
outweighs a 6-block floor fork and drove the reorg through the production
`apply_reorg_mdbx_with_applier`: the tip height DECREASED 16 -> 15 across a
contract deadline, rolling back a payer recovery close executed at
`h = 16 >= deadline` while the payee's claim at `h = 15 < deadline`
survived. The result is internally consistent (exactly one satisfaction on
the surviving chain, twin-equal state) — but it demonstrates that the
height-decreasing class is not hypothetical: any reasoning of the form
"height only moves forward after a reorg" is false, and contract code that
assumes monotone height across reorgs would be wrong. The refund/vault
phase machines here do not assume it (phases re-derive per block), which is
why the scenario holds.

## Negative and control results

- Twin equality after every reorg and post-reorg extension (exact root,
  supply, alloc counter, active slots, tip height, owner-index totals) —
  six independent comparisons across scenarios 1-5.
- Losing-fork payouts evaporate; the funded root UTXO resurrects with the
  original `(slot, amount, creation_id)`.
- Cross-fork replay of a losing page after the winner consumed the root:
  rejected (`UnknownOrSpentInput`) at template build; payee total
  unchanged.
- Sequential double-spend of one allowance root on a single chain:
  rejected (re-verified lane A's result locally).
- Budget exhaustion after the reorg-redraw still enforced against the
  surviving chain's own counter (the second full draw cannot even be
  built).
- Same-page double satisfaction on the winner: rejected.
- Terminal close page from one phase cannot be re-verified in the other
  phase (`Recipient` at the boundary; authority flip below it).
- Staged-reorg abort with contract pages inside: injected post-apply
  rejection restored hot and durable tip/root/supply exactly.
- Same-block mint+spend chaining (contract successor minted by an earlier
  call spent by a later call in one block): rejected at the page-stream
  level (`InputOutputSlotOverlap`) before any state transition - the
  cross-contract-call double-advance-in-one-block attack class is
  structurally empty.
- Rejected attack surface: an attacker controlling both the mempool and
  fork extension can choose WHICH of two rival authorizations survives, and
  can lower the tip height across a deadline, but in every exercised
  construction the surviving chain contains exactly one satisfaction of the
  obligation and ends twin-equal to the honest chain with the same blocks.
  No double-draw, resurrected allowance, phase re-entry with the other
  party's funds, or schedule drift was observed.

## PoC implementation and reproduction

`src/main.rs` uses the pinned `noid_chain` + `noid_tx` APIs directly, with
`isolated-v2-fork-testnet` (v2 contracts active at height 10). Blocks are
produced by the real `build_node_owned_block_template` with the production
fee schedule (pages are built at exactly the template's fee floor — the
builder silently drops under-fee groups), sealed with real PoW grinding
(`validate_pow`, genesis-floor targets, ideal or compressed ASERT spacing
per fork), and admitted through `MdbxChainContext::apply_next_block` with
full native validation. Reorgs go through
`apply_reorg_mdbx_with_applier` with undo-log rollback, exactly as in the
storage-reorg lane. Fork rivalry is decided by the production
`choose_chain_by_work` (work, then lexicographic hash tie-break; the PoC
grinds the tie-break where the construction needs it). The recursive
HistoryStep terminal verifier is stubbed `Ok(())` (see limits); contract
program semantics are enforced by running `ObjectOpening::check_call` at
each inclusion height — the same check the production witness preparation
runs per contract page.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
# scenarios: all | s1 | s2 | s3 | s4 | s5 | s6
```

Final output is reproduced in `results.txt`; closing lines:

```text
CONTROL s2 height-decreasing reorg across deadline: tip 16->15 payer_recovery_evaporated=true payee_claim_survived=true twin_equal=true
CONTROL s3 counters reverted with the fork: draw-on-A vanished, draw-on-B stands, twin_equal=true (no double-draw, no resurrected budget)
CONTROL s4 reorg resolved the allowance race to exactly one authorization: recipient_a=0 recipient_b=15000000 twin_equal=true
CONTROL s5 same-block mint+spend chaining rejected at the page-stream level (input/output slot intersection must be empty)
CONTROL s5 rolling back the two-contract block restored alloc_counter/active_slot_count/root and both roots to the twin exactly
CONTROL pinning: recurring claim valid at its built height 12 only; at 13 the successor root mismatches (a miner cannot shift the schedule by delaying inclusion)
contract-reorg-audit: all assertions passed for all
```

## Assurance limits

- The recursive HistoryStep terminal verifier is stubbed, as in the
  storage-reorg lane: the PoC constructs terminal metadata itself. The
  contract-layer conclusions rest on the native `check_call` semantics
  (which the witness preparation and receipts also run) plus the exact
  state-root binding of every accepted block; no claim is made about
  recursive proof soundness.
- Authorization proofs are not constructed (lane A covered real
  prove/verify); the state-level pages here enforce spend exclusivity and
  program semantics. The phase/authority argument for the authorization
  layer is from code (`OwnerAuthPublicInputs` binds txid + height-selected
  authority; `check_call` supplies the authority), not from an executed
  proof.
- Chains are short (<= ~30 blocks per context, one miner family, 1-2
  transactions per block, `log_slots`=24, genesis-floor difficulty with
  modest compression, v2 activated at height 10 by the isolated-testnet
  feature, tx epochs of 144 blocks so every page shares the genesis
  anchor). Multi-epoch anchor changes across a reorg, v2.1 native-asset
  state machines (markets, launches, NFT escrow — other lanes' turf), and
  network-level double-propagation races were not exercised.
- The height-decreasing construction requires the stalled fork's
  accumulated compression to outweigh the incumbent's floor blocks; it was
  demonstrated at depth 6 (5 vs 6 blocks). Finality depth (18) bounds all
  reorgs; reorgs crossing the finalized prefix are rejected by the storage
  layer (lane B also covered this).
- `expect_spend_rejected` treats a template-build rejection as a pass; the
  rejecting layer is printed so the reader can see which check fired
  (`UnknownOrSpentInput` in every exercised replay).
- The template builder silently DROPS candidate groups below the consensus
  fee floor rather than erroring (found while building this PoC — pages are
  built at the floor to compensate). This is a builder ergonomics
  observation, not a security finding: an under-fee page is excluded from
  the block either way.
