# Contract state across chain reorganization

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

The PoC calls the production `MdbxChainContext` chain APIs against temporary
MDBX databases. It builds block templates, grinds PoW, applies blocks through
`apply_next_block`, and reorganizes them through
`apply_reorg_mdbx_with_applier`. Reorganized blocks carry real `ObjectOpening`
pages for refundable payments, allowance wallets, and period budgets. No live
node, peer, RPC server, or user database was used. `pinned/` was extracted
with `git archive 52938433` and checked against the storage-reorg experiment's
verified copy.

## Question and result

The contract-rights experiment tested sequential contract checks, and the
storage-reorg experiment tested storage rollback. This experiment combines
them: after a fork replaces blocks containing contract calls, do counters,
allowances, payouts, or deadline phases differ from execution of the winning
blocks on a fresh chain?

**No divergence appeared in the tested reorganizations.** For each surviving
block sequence, the reorganized chain matched a fresh chain on state root,
circulating supply, `alloc_counter`, `active_slot_count`, tip height, and each
party's owner-index UTXO set. Replaying a losing-fork page after the winning
chain consumed its input failed `UnknownOrSpentInput`. The table identifies
the relevant state carrier or check in each scenario:

| # | Scenario | Verdict | Mechanism |
|---|----------|---------|-----------|
| 1 | Refund paid on losing fork, then equivalent claim on winner | Matched fresh chain | Contract state lives in the UTXO slot `(amount, creation_id, owner=root)`. Reverting the fork restores the obligation slot. The winning claim executes once; a second replay fails `UnknownOrSpentInput`. |
| 2 | Shorter, heavier fork lowers tip height across a deadline: payer recovery at `h >= deadline` loses; payee claim at `h < deadline` survives | Matched fresh chain | `authority_at` and `recipient_at` use the inclusion height. The losing page has the wrong recipient in the other phase (`Recipient`); its authorization also names the height-selected authority. The losing payout disappears with its block. |
| 3 | Budget drawn on fork A, then drawn on fork B | Matched fresh chain | The budget counter is packed into the successor root in the UTXO slot. The undo log restores slot preimages and state counters (`restore_state_counters`, `noid_chain/src/consensus/reorg.rs:25`). Budget exhaustion still applies on B. |
| 4 | Two allowances spend the same root, one per fork | One survived | Both pages require the same input. Only the winning chain's payout exists; retrying the losing page fails. |
| 5 | Two independent contract calls in one block, then rollback and reapply; attempted same-block mint and spend | Independent calls matched; chaining rejected | Input and output slot sets of the entire page stream must be disjoint (`noid_chain/src/consensus/paged_spend.rs:325-328`). `validate_block_slot_conflicts` also touches each slot once. The chaining attempt fails before state apply. Two calls on distinct roots restore both roots and state counters after reorganization. |
| 6 | Move a schedule call to another inclusion height | Recurring/budget-reset call rejected at new height | The successor root commits to `next = execute(body, height)`. For inclusion-relative schedules (`State1 = Height + period`), the committed successor pins the call to its built height. Constant-advance schedules (tranche: `State1 += period`) do not pin the height this way. |

## Why rollback restores contract state

Contract-visible state is in the UTXO slot set, authenticated by each header's
state root, plus `alloc_counter`, `active_slot_count`, and derived supply. The
undo log restores the counters along with the slots. There is no separate
contract nonce or consumption table that could survive a fork. Fork-scoped
transaction-index entries are removed on reorganization (storage-reorg
experiment). Thus rollback restores the carrier for the contract's state
machine. Each relevant path supplies the current inclusion height to
`check_call(page, block_height)`: template witness preparation
(`noid_block/src/candidate_history.rs:180`), receipt verification
(`noid_block/src/contract_receipt.rs:129`), and mempool re-admission
(`noid_mempool/src/contracts.rs:88`).

Two checks matter when the replacement fork crosses a deadline:

1. **The authorization statement binds `(tx_body_hash, expected_address)`**
   (`OwnerAuthPublicInputs`, `noid_gkr/src/owner_auth.rs`). The height selects
   the authority a page demands
   (`authority_at(height)`), and the terminal recipient binding flips with
   the phase. A page made for one phase fails verification in the other
   because it names the wrong authority and recipient.
2. **After a reorg, the node rechecks admitted calls**: the mempool sweep
   `on_new_block(&[], view)` re-runs every admitted contract call at the new
   candidate height and evicts on any context change
   (`noid_mempool/src/pool.rs:802`, `contracts.rs::eviction_reason` also
   comparing the authority against `admitted_height + 1` so a deadline
   crossing between admission and inclusion evicts). Reclaimed transactions
   are not re-admitted automatically (authorizations are not persisted;
   `readmit_after_reorg` only removes duplicate resubmissions) — wallets
   resubmit through the full admission path. Wallet receipts are re-derived
   from the canonical chain scan (`retain_object_receipts_at_tip`), and a
   receipt verifies only against the canonical header
   (`contract_receipt.rs:156`).

## A shorter, heavier fork crossed the deadline

Fork choice compares accumulated work regardless of length (upstream test
`more_work_wins_over_longer_chain`). In the v2 ASERT variant, a child's
target derives from its parent's timestamp (`difficulty.rs:141-149`), so
siblings have equal per-block work. A stalled fork can nevertheless gain
enough difficulty over successive blocks to outweigh a longer competitor.
The PoC built a five-block stalled fork heavier than a six-block floor fork
and applied the reorganization through `apply_reorg_mdbx_with_applier`. Tip
height fell from 16 to 15 across a contract deadline. A payer recovery
executed at `h = 16 >= deadline` was rolled back; a payee claim at
`h = 15 < deadline` survived. The surviving chain contained one satisfaction
and matched the fresh-chain control. This is an executed counterexample to
assuming tip height can only increase across reorganization; these phase
machines recompute their phase at each inclusion height.

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
- Fork control can determine which of two rival authorizations survives, and
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
