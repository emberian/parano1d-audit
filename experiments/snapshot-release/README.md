# Narrow release-terminal test: one future historical header

Target `52938433a30b185ab007afaa15166a632cf2e82e`. This lane uses the
already built `devnet-chaos/bin/parano1d-poison`, whose only relevant changes
are an environment-gated consensus/miner clock offset. The victim uses the
unmodified release binary. The runner is `middle_header.py`; it creates a
unique case directory under this lane's `run/`, binds only loopback ports
19940/19941 and 19950/19951, and records PID files and complete logs there.
It does not reuse or delete phase-8 data.

The phase-8 all-future fork failed the victim's **tip** drift check. This
fixture gives the producer normal time for h1..10, +10 years only for h11,
then normal time again from h12. The process is restarted at each change on
the same data directory. The poison node must create genuine complete blocks
and HistoryStep terminals with its shipped miner; a fresh unmodified node
then tries to snapshot from it. This is the smallest path that can test the
earlier component PoC's missing release-verifier leg.

Why h11: mainnet below v2 uses candidate timestamp in ASERT, and the next
epoch anchor is h12 (`noid_chain/src/consensus/header.rs:273-287`,
`difficulty.rs:129-147`). A future-dated h12 would become the anchor for
h13..17, making their near-present timestamps economically awkward. A single
h11 outlier is below the median of the last 11 blocks when h12 is proposed,
so h12 can return to present time (`timestamps.rs:41-68`). Future h11 fails
ordinary wall-clock admission but is accepted by historical timeless
validation if all other rules hold. At a producer tip >=36, the deterministic
snapshot boundary is >=18 and therefore includes h11 as an intermediate
header. The runner targets tip 42 for a boundary at 24.

First decisive stop: if the producer cannot restart on the future h11 or
cannot prove/commit h12 at normal time within 120 seconds, the shipped miner
cannot construct the fixture this way. There is no reason to continue to a
victim. If h12 succeeds, the remaining 30 blocks are bounded at 600 seconds.
Then the victim gets 240 seconds to install. The logs must establish:

1. h10 and h12 present-day, h11 far-future, tip present-day.
2. Real producer terminal emitted for boundary h18/h24.
3. Victim selected manifest and its HeaderDAG work, then genuine terminal
   verification accepted or rejected.
4. If accepted, canonical h11, finalized height, later honest-peer behavior.

No inference about established mainnet nodes or the public network follows
from this floor-difficulty, one-provider local test. A positive result would
close only the genuine release-terminal and local snapshot-admission legs.
