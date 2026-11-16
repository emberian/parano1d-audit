# Chain-malice review: finality and confirmation boundary

Target: `parano1d/` at `52938433a30b185ab007afaa15166a632cf2e82e`.
Method: independent source review of the pinned production path, its unit tests,
and its user documentation. No build, live node, or network traffic in this
review. This is a source-backed finding, not a run-proven exploit.

## CM-1 — Exactly 18 confirmations remain reorgable

The public protocol documentation says the maximum accepted reorganization is
**17** blocks (`docs/protocol/consensus.md:48-53`, `docs/protocol/parameters.md:11`,
`docs/reference/glossary.md:201-205`). The production code permits **18**:

1. `noid_chain/src/storage/mdbx_context.rs:950-964` computes the finalized
   checkpoint at `tip_height.saturating_sub(CONSENSUS_FINALITY_DEPTH)` and
   stores it on ordinary block commits (`:1106-1111`). The depth is 18
   (`noid_chain/src/consensus/params.rs`, `CONSENSUS_FINALITY_DEPTH`).
2. The actual MDBX reorg entry point rejects an ancestor *below* the finalized
   checkpoint, and rejects a reorg depth *greater than* 18 (`mdbx_context.rs:
   1926-1952`). An ancestor **at** the finalized checkpoint is accepted when
   its hash matches. The preverified recursive suffix authority applies the
   same `> 18` rejection (`:2587-2592`).
3. The network's inventory planner admits an ancestor `>=` HeaderDAG finality
   (`noid_node/src/main.rs:4924-4943`). The public fork-choice helper also
   explicitly asserts `reorg_allowed(18)` and `!reorg_allowed(19)` in
   `noid_chain/src/consensus/fork_choice.rs` tests.

For a transaction in block **B** with tip at **B+17**, tip-inclusive
confirmations are 18 (`noid_rpc/src/server.rs:2753-2758`,
`noid_gui/src/backend.rs:1143`). But the finalized boundary is **B-1**. A
heavier valid competing suffix from B-1 can replace B through B+17: 18
blocks. At tip B+18, the boundary advances to B; replacing B would require
an ancestor B-1 below finality and is rejected. A recipient who treats 18
confirmations as irrevocable based on the documented 18/17 rule is one
confirmation early. The safe threshold under this node's ordinary commit
semantics is **19 confirmations**, subject to the caveat that different nodes
may already hold incompatible finalized branches (CM-2).

This is a documented-boundary error with possible integration impact, not
proof that any exchange uses 18 confirmations or that the developer can reorg
at will. A successful double-spend still requires an alternate suffix with
greater cumulative work (or equal work plus the tip-hash tie-break), a valid
terminal, and transmission before the local boundary advances. Noid's fork
choice has no special operator or fund-key privilege here.

## CM-2 — Two finalized mining partitions cannot reconcile by work alone

If two internally connected groups each produce at least 19 blocks after a shared tip H,
and their block H+1 differs, each locally finalizes its own H+1 when its tip
reaches H+19. When connectivity returns, neither branch preserves the other's
finalized checkpoint. `choose_chain_by_work` compares work only among eligible
candidates (`fork_choice.rs:31-78`); `plan_header_inventory` classifies a
branch without a common ancestor inside its nonfinal window as
`FinalizedDivergence` (`main.rs:4915-4943`); the event loop marks such peers
incompatible (`main.rs:11657-11667`). The MDBX reorg authority rejects an
ancestor below the checkpoint even if a competing chain has more work
(`mdbx_context.rs:1928-1952, 2587-2592`). A normal restart reloads the durable
checkpoint (`mdbx_context.rs:920-938`), so it does not clear this conflict.

This is a **conditional safety/liveness tradeoff of the documented hard
finality rule**. It is not a newly discovered remote exploit. To make it an
attack, someone must partition *all relevant chain-view traffic* for long
enough that both sides mine 19 blocks, while each side retains mining
readiness. Header traffic surviving a body-serving stall defeats that
precondition; C17 alone has not shown the necessary partition. Default mining
requires one compatible peer/failure domain and exact frontier authority
(`main.rs:533`, `networking/mining_readiness.rs:165-227`); one isolated miner
normally stalls. Two or more nodes per side, or an independently validated
way to satisfy the peer gate, are required for a realistic test. The supplied
`--genesis` isolated mode is a test control, not proof the public network can
be partitioned cheaply.

For scale intuition only, if a partition has fraction f of the current total
hashrate, its expected time for 19 blocks is `19 × 30 seconds / f`, with high
PoW variance. The earlier public snapshot's block shares near 49% and 35%
would imply roughly 19 and 27 minutes respectively *if* those shares were
stable and could be separated cleanly. Neither control of those miners nor a
partition mechanism has been established. A shorter outage with one side
advancing fewer than 19 blocks can still reconcile through an eligible reorg
if the stronger chain arrives before finalization.

## Tests that would advance this finding

1. On isolated release nodes, mine two independently valid branches from one
   common ancestor. Present an 18-block heavier replacement before the first
   node reaches 19 confirmations for the replaced transaction; then advance
   one further block and repeat with a still-heavier branch. Observe first
   acceptance and second refusal through the real network, not a copied gate.
2. Partition two *multi-node* mining groups while preserving each group's
   quorum. Record tip hashes, work, finalized checkpoints, and mining readiness
   before and after 19 blocks and reconnection. Include a control where one
   side reconnects at 18 blocks and follows the heavier branch.
3. Audit any exchange or external wallet that uses the documented 17-block
   maximum reorg or an 18-confirmation threshold. Until its actual deposit
   policy is known, do not claim a realized SafeTrade exposure.

The key open question for the broader operator-power audit is how cheaply a
party could create and sustain a *header-level* partition. Work on body-path
availability and DNS bootstrap does not answer that for established nodes.
