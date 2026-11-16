# Independent review of C26/C27 snapshot and bootstrap claims

Target: pinned `52938433a30b185ab007afaa15166a632cf2e82e` in
`parano1d/`. Review date: 2026-11-15. This lane read the release-source path,
the existing component PoC and its saved output, and the phase-7 joiner log.
It did not launch a node, run a build, or touch the live network. Concurrent
`devnet-chaos` work remains untouched. Source line numbers below refer to the
pinned target unless another file is named.

## Verdict

**The historical timestamp validation differential survives.** Ordinary
header admission passes local time to `validate_header_inner` and rejects
far-future timestamps (`noid_chain/src/consensus/header.rs:195-251`). Snapshot
header staging calls the timeless validator
(`noid_node/src/snapshot_header_staging.rs:972`) and the post-staging drift
check tests the boundary **tip only** (`noid_node/src/main.rs:4306-4317,
9255-9269`). The `malicious-node` lane's saved default-schedule result
(`results.txt:128,141-154`) stages and installs a 24-header chain with a
far-future h12, returning the tip timestamp to the local window. That is a
real component-level discrepancy in the release schedule, not merely in the
isolated v2.1 profile.

**The claimed live poisoning chain remains conditional.** The component PoC
passes an always-accepting closure to `verify_snapshot_boundary` in place of
the genuine recursive terminal (`malicious-node/src/main.rs:405-426`), and
drives source-equivalent event-loop checks without sockets. A release terminal
generated for the poison history has not been shown to pass the actual
`HistoryProtocolRuntime::verify_terminal` call (`main.rs:4242-4259`). The
boundary verifier does bind the terminal's tip identity/epoch anchor, and the
provable historical header rule is timeless, so satisfiability is plausible;
proof execution is the missing experiment. Local state installation under a
stub does not by itself prove release-node acceptance.

**C27 overstates seed-source determinism.** Four bare DNS seed hostnames are
hardcoded under one domain with no PeerID pin (`noid_chain/src/consensus/
network.rs:111-134`). Their operator can choose which endpoint a new node
dials *through those names*. Yet the node dials configured seeds as well
(`noid_node/src/main.rs:2256-2305`) and can learn more peers. Snapshot offers
are collected in a five-second window (`main.rs:850-857,875-891`), compared
by bridge chainwork (`main.rs:879-891,11928-11969`), then selected only when
the window settles (`main.rs:14471-14531`). At installation, the selected
manifest is checked **again** against the current HeaderDAG and an obsolete
candidate is retired (`main.rs:9684-9715`). Thus neither the first DNS
connection nor the first manifest has unconditional authority. If the seed
operator succeeds in making *every reachable provider* malicious until
installation, this guard does not create an honest alternative; that is a
stronger and still unmeasured eclipse precondition.

The peer-connect path starts a native header probe alongside the manifest
request (`main.rs:10765-10806`). A malformed or far-future inventory produces
a warning on the direct-header path (`main.rs:11660-11673`); that error arm
does not itself mark the peer as a rejected manifest provider. This matters
for the proposed race: direct header rejection can coexist with a pending
snapshot offer, but the release-node timing and outcome require a local run.

The candidate comparator uses **advertised** bridge chainwork before native
headers prove it (`main.rs:879-891,11940-11960`). The manifest's digest and
geometry authenticate its *own bytes*, not its work claim (`noid_p2p/src/
protocol.rs:232-260,547-568`). A peer claiming enormous work could therefore
win this scheduling window and fail later, temporarily displacing a genuine
honest offer. Retry and provider rotation are present (`main.rs:8782-8807,
8921-8930`); neither persistent starvation nor an integrity bypass has been
demonstrated. Repeated false-work rounds against an honest provider are a
useful separate availability experiment.

**The C17 serving stall did not create a genesis-only DAG.** Its phase-7
sampler recorded `getChainInfo.height`, the installed chain height
(`devnet-chaos/scripts/phase7_multiaggressor.py:66-73`,
`devnet-chaos/scripts/chaoslib.py:264-265`). Saved `logs/n7s-a.log:128-223`
records `our_height=0` while HeaderDAG-selected targets rise through 24,
49, 74, 99, 124, 149, and 150. The install gate compares cumulative work
against `header_dag.best_tip()`, not the installed height
(`main.rs:282-321`). The parent takeover independently captured the log
hash and these lines in `devnet-chaos/header-progress-review.txt`. This
experiment shows delayed state installation, **not** removal of honest
chainwork from fork choice.

**Restarted nodes retain stronger authority.** On startup the HeaderDAG is
reconstructed from the durable finalized checkpoint and canonical non-final
window, including stored cumulative work (`main.rs:4721-4757`, called at
`:8436-8445`). A successful outbound peer cache is also loaded from
`peers.json` and supplied to the topology/Kademlia manager
(`noid_p2p/src/peer_store.rs:93-135`, `network.rs:3806-3833`). DNS
redirection alone cannot reduce a restarted node's retained work to genesis.

## Additional cost-scope correction

The far-future-**parent** difficulty-floor demonstration in
`malicious-node/results.txt:135-139` calls v2 target selection at height
279,506 (`malicious-node/src/main.rs:121-137`). Release v2 activates at
210,537 (`noid_chain/src/consensus/params.rs:15-18,62-70`). Below that,
legacy difficulty uses the candidate's own timestamp
(`noid_chain/src/consensus/difficulty.rs:59-71,129-147`). The 24-header
default-schedule drift differential still exists, as demonstrated, but the
v2 far-future-parent cost drop is not its h12 mechanism. Descriptions of
17 CPU-seconds for a fresh victim apply only to an attacker fork beating a
genesis-only DAG at floor difficulty; they cannot be carried over to a
restarted node, a node that already has honest headers, or the established
mainnet chain. The exact minimum production-scale cost remains unmeasured.

## What to test next

1. In isolated local topology, generate a **real release-profile terminal**
   for a far-future historical header and run the genuine terminal verifier.
   Record whether the terminal is actually producible by the shipped miner.
2. Give a fresh victim both an honest and malicious source. Record the
   five-second candidate set, HeaderDAG best work, selected provider, and
   outcome when honest headers arrive before, during, and after the window.
3. Restart a victim with a durable honest finalized checkpoint and verified
   peer cache; redirect only seed addresses. Confirm the malicious manifest
   loses to retained work. Then vary the attacker capability explicitly.
4. If any malicious snapshot installs, reconnect an honest peer and restart
   the victim without wiping its data. Verify the claimed persistent
   estrangement and its actual operator recovery path.
5. Feed repeated self-consistent manifests with inflated, unverified bridge
   work beside an honest source; measure whether retries eventually choose
   the honest source or can be held in false-candidate cycles.

Publication wording should say: “A snapshot path omits wall-clock future
drift for historical headers, and a local component demonstration installed
an otherwise rejected header. End-to-end release-node poisoning, seed-only
control of source selection, and mainnet-scale economics are not yet
demonstrated.”
