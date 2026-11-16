# ASERT/MTP reset: evidence and limits

This is a read-only audit of the saved `comp-sync-finality` ASERT result at pinned
source commit `52938433a30b185ab007afaa15166a632cf2e82e`. Reproduce the
arithmetic and timing checks with:

```sh
python3 experiments/verification-asert/model.py
```

The script reads the saved JSON and source harness, then writes `results.json`.
It opens no node, mines no blocks, and makes no network request. In particular,
it does not exercise snapshot installation.

## What the saved experiment proves

The h210536 retirement-origin certificate was cryptographically checked by the
original harness. Its header carries target work `4,825,867,084,694`, versus
`262,144` at the genesis difficulty floor. These are real properties of that
historical header and the consensus `block_work` formula. They do **not**
establish the target or cumulative work at a later mainnet epoch.

For a **synthetic** v2 epoch ending at h210540, the original harness constructs
eleven ancestor headers, gives the anchor h210536's target and a timestamp
120 seconds later, and then constructs 24 descendants. The target computation
is real: h210541 retains the hard target; h210542 reaches the floor when its
parent's timestamp is 4,375 seconds after the synthetic anchor. Five shifted
headers remain a minority of the eleven-header MTP window. The sixth header
can return to anchor time +1 second, and the next epoch inherits the floor.
Our independent integer model reproduces the floor threshold (the target is
still harder at 4,374 seconds), all 24 MTP checks, and the claimed work sums.

The source harness mines real Poseidon2b PoW for the 23 floor-target headers
at steps 2–24, and runs `validate_header_timeless` on each of them. This
demonstrates the individual low-difficulty header predicates **conditional on
their parent**. The first hard header at step 1 is left at nonce zero and
is neither mined nor passed through a PoW validator. Consequently, the
experiment has no complete valid branch, no valid HistoryStep terminal or
block bodies, and no devnet chain acceptance/reorg demonstration.

## Time and work needed for a live branch

At the harness's local clock, anchor time +750 seconds, **all five** shifted
headers fail the 120-second future-drift check, even though the later tip
passes it. A descendant's passing timestamp does not make the rejected prefix
admissible. The fifth shifted header can first satisfy future drift at anchor
time +4,375 seconds (1 h 12 m 55 s). This is an absolute clock condition:
if the anchor was already old when the branch was prepared, the remaining
wall-clock wait could be shorter.

A branch from the synthetic anchor can only replace a live branch while that
anchor remains in the local finalized prefix. With 18-block finality, more
than 18 honest descendants before admission would rule out that reorg. If the
anchor time was approximately current when preparation began, 4,375 seconds
allows about 146 nominal 30-second block intervals; keeping the ancestor in
the window would require a mean honest interval of at least 243 seconds.
These are conditional timing comparisons, **not** a measurement of actual
network production or an estimate of reorg probability.

The harness sets base chainwork to zero. Its “beats base checkpoint” field
therefore compares positive declared work to zero, not to a current mainnet
checkpoint. Its alleged branch work is `4,825,873,114,006`: one **unmined**
hard block plus only `6,029,312` work from the 23 mined floor blocks.
One additional equal-target honest hard block would require **18,409,222**
floor blocks to exceed in work, even after mining the attacker's first hard
block. The low target does not reduce the total PoW needed to overtake a
continuing hard branch; it adds considerable per-block proof and validation
work. Actual canonical headers, chainwork, difficulty, hashrate, and terminal
generation costs were not measured here.

The second synthetic case uses `MIN_TARGET = 1`, whose first block has an
expected `2^256` PoW trials and is also unmined. The source's `net >= 46`
early clamp sets its child directly to the genesis floor after 11,190 seconds;
plain integer ASERT shifting at that point would produce target `2^62`, far
below the genesis floor `2^238`. This reveals an extreme-target shortcut in
`difficulty.rs`, not a feasible demonstrated fork. The u256 chainwork value
for `MIN_TARGET` saturates, so its fork-work comparisons cannot be read as
ordinary additive work comparisons.

## Conclusion

The ASERT/MTP reset is a reproducible **consensus arithmetic possibility**.
The saved experiment does not show a live chain halt, a successful competing
branch, or a mainnet-relevant cost. The strongest supported concern is the
separate timeless-header versus live future-drift distinction; its snapshot
consequences need their own end-to-end evidence and are outside this audit.

Source checkpoints: `comp-sync-finality/src/bin/asert_mtp_conformance.rs`
(synthetic ancestors 187–211; skipped PoW 271–305; zero work comparison
342–371), `noid_chain/src/consensus/difficulty.rs` (v2 parent timestamp
122–154; early clamp 191–197; block work 491–505),
`noid_chain/src/consensus/header.rs` (validation 199–263),
`noid_chain/src/consensus/timestamps.rs` (MTP and drift 41–67), and
`noid_chain/src/consensus/fork_choice.rs` (finalized prefix 69–82).
