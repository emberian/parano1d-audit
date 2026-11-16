# Transaction relay under inbound Sybil fan-in

Target: `parano1d` commit `52938433a30b185ab007afaa15166a632cf2e82e`. This is a source audit with exact combinatorial calculations, not a measured delivery failure. No public network was contacted.

## Mechanism

A node broadcasts each admitted transaction on two paths. It calls GossipSub `publish`, then shuffles **all** connected, dispatchable peers and pushes the transaction directly to four of them once there are more than eight peers. The direct sample has no reserved outbound slot and no failure-domain rule. See [`network.rs`](../../parano1d/noid_p2p/src/network.rs) lines 479–483, 690–696, 5804–5845. Profile verification and identification are required for direct dispatch (same file, lines 1159–1175), but an ordinary protocol-speaking Sybil can pass those checks.

GossipSub is a separate path. The target sets `mesh_n=4`, `mesh_n_low=2`, `mesh_outbound_min=1`, `heartbeat_interval=700 ms`, and `flood_publish=false` in [`behaviour.rs`](../../parano1d/noid_p2p/src/behaviour.rs) lines 209–234. The pinned dependency, `libp2p-gossipsub 0.48.0`, publishes to mesh peers and draws random subscribed peers only if the mesh has fewer than four (`behaviour.rs` lines 584–756 in the local Cargo registry). It does **not** send every transaction to all connected peers. The two paths have independently chosen recipients.

At node startup the first heartbeat is scheduled after the dependency's default five-second initial delay **plus** the configured 700 ms interval (`libp2p-gossipsub` `config.rs:422–425`, `behaviour.rs:465`). An incoming subscription is inserted into the mesh only while it contains fewer than `mesh_n_low=2` peers (`behaviour.rs:1870–1960`). Thus two Sybils whose subscriptions arrive first can occupy both initial mesh slots. Later honest subscriptions alone do not displace them. An honest peer can still enter early by sending its own GRAFT; the calculation below assumes none does.

The first heartbeat checks the outbound reserve and grafts one eligible outbound peer if the mesh has none (`libp2p-gossipsub` `behaviour.rs:2223–2250`). If all outbound peers are honest, this repairs the **future** transaction path. The old transaction is not re-published to the newly grafted peer, but the heartbeat also emits IHAVE notices for cached messages to a random subset of subscribed nonmesh peers (`behaviour.rs:2423,2464–2520`). A receiver may then request the transaction. Mempool pull is another recovery path, but the node requests one finite scan from at most four *locally selected outbound* peers on connection/initial sync, not every inbound peer or a periodic poll (`noid_node/src/main.rs:937–951,10820–10833,14756–14778`; `noid_p2p/src/mempool_recovery.rs:1–24,76–89`). An already completed scan does not recover a later missed broadcast.

## Cost and conditional first-hop odds

The public inbound policy allows 32 identities per IP, 96 per IPv4 /16 or IPv6 /32, and an unreserved pool of 96 inbound connections ([`peer_diversity.rs`](../../parano1d/noid_p2p/src/peer_diversity.rs) lines 22–34, 145–181). GossipSub score starts with no application topic score and penalizes more than eight identities sharing one exact IP (`behaviour.rs:245–270`; `libp2p-gossipsub` `peer_score/params.rs:164–180`). Its topic-delivery penalties require configured topic parameters; simply receiving and withholding a transaction does not incur one (`libp2p-gossipsub` `peer_score.rs:240–315`). Hence 96 inbound Sybils can fit one /16 using 12 IP addresses with eight identities each and avoid that specific mesh penalty. Only three IPs are needed to occupy 96 inbound slots for the **direct-push** calculation; those peers incur GossipSub colocation penalties and should not be assumed eligible for mesh capture. Eight honest outbound connections plus 96 inbound Sybils remain below the 128-connection swarm cap.

For `A` attacker and `H=8` honest connected peers, all subscribed and dispatchable, suppose the first two mesh subscriptions were attacker-controlled. GossipSub then chooses two extra recipients uniformly from the remaining `A−2+H` peers. The direct path chooses four uniformly from all `A+H` peers. Under independent random draws:

`P(gossip misses honest) = C(A−2, 2) / C(A+H−2, 2)`

`P(direct misses honest) = C(A, 4) / C(A+H, 4)`

| Attacker connections | IPs at 8 per IP | Gossip misses | Direct misses | Both miss |
| ---: | ---: | ---: | ---: | ---: |
| 32 | 4 | 61.88% | 39.35% | 24.35% |
| 64 | 8 | 78.30% | 61.76% | 48.36% |
| 96 | 12 | 84.86% | 72.25% | 61.31% |

These are conditional *first-hop* probabilities, not measured transaction loss. They require that attacker nodes receive but do not forward, the victim has no other application relay, the honest peers are not already in its mesh, and the broadcast occurs before the initial heartbeat. The 5.7-second timer starts when GossipSub is constructed, not when the peers connect; if connections arrive later, the remaining window is shorter. Real timing, GRAFTs, recipient scoring, IHAVE/IWANT recovery, and other peers' network topology can reduce or end the delay. A lasting all-attacker mesh would also need an attacker-controlled outbound member or another way around the outbound reserve. The table does not estimate how often that topology occurs.

## What survives the attack

Inbound saturation does not by itself produce a durable eclipse. With at least one eligible honest outbound peer and no malicious outbound, heartbeat maintenance inserts an honest mesh peer. The node's separate locally selected outbound set also owns proactive chain discovery and mempool pulls. We therefore have evidence of **redundancy erosion** and a bounded startup first-hop risk, not proof that an ordinary attacker can prevent the network from confirming a transaction or block.

## Useful next test and fix

A decisive isolated test would start an honest sender with two Sybil subscribers first, then honest subscribed outbound peers and further Sybil inbound peers. Broadcast distinct transactions before and after the first heartbeat, recording exact mesh members, direct recipients, IHAVE/IWANT recipients, and time until an honest peer admits each transaction. Include a control with no Sybils and a control where one honest peer sends GRAFT before broadcast. The local phase-8 devnet was in use, so this audit did not run a competing network harness.

The direct relay could reserve some of its four slots for locally selected outbound peers in distinct public groups, then fill the remainder from all dispatchable peers. The cold-start mesh path could also send to a bounded set of those outbound peers before its first heartbeat. Both changes keep bounded fanout while removing the easiest inbound-only dilution. They still cannot identify a dishonest outbound peer; that requires broader topology and delivery measurements.
