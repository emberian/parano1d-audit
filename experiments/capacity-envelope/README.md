# Capacity envelope: multiple hosts and public addresses

This study asks what roughly twelve computers with varied public addresses
could cause other nodes to observe. It uses the pinned source, saved isolated
tests, and a dated passive network observation. It did not run a twelve-host
test or contact public peers. Hardware count alone is not a protocol capacity
measure: peer identities, address groups, upload bandwidth, proof-verification
throughput, and accepted block share limit different paths.

| Resource | Strongest supported effect | Scope still to measure |
| --- | --- | --- |
| Shared outbound serving budget | Four stalled legal mempool responses filled one node's 64 MiB reservation budget. A joiner's State installation was delayed while honest headers continued to advance. | Whether diverse providers can be constrained together, and the effect on current valid histories. |
| Anonymous proof ingress | A real node reached `AuthProof` rejection on 40 direct pushes and still admitted the honest control. The saved crate fixture drained about 415 invalid proofs/s on one laptop. | Sustained live verifier saturation, transaction drops, and independent delivery paths. |
| Peer and gossip topology | Many identities can occupy permitted inbound slots. In the tested topology, an honest outbound peer joined the gossip mesh and received all 12 early payloads. | Established honest meshes, actual address diversity, and cold-joiner outcomes. |
| Accepted proof of work | A producer chooses valid contents in blocks it wins; its expected share depends on measured work rate relative to other miners, not the count of GPUs or IPs. | Per-machine Poseidon2b rate, effective accepted-block share, and proof construction throughput. |
| Miner view under isolation | Source shows one compatible peer can renew the exact-tip readiness lease. A peer that is a miner's sole route could keep its view stale or let its lease expire. | Whether deployed miners have this topology and how independent peers restore the view. |

The [P2P study](P2P.md) gives the node limits, measured controls, and
conditional resource arithmetic. The [block-production study](POW.md) gives
share-based bounds for inclusion, valid state growth, and short reorgs. The
[operational review](../operator-power/DEEPER-CONTROLS.md) traces the one-peer
readiness dependency and the scope of seed influence.

These results identify per-node availability and freshness questions. They do
not establish a sustained network-wide outage or a holder-specific restriction
on native NOID transfers or exchange deposits.
