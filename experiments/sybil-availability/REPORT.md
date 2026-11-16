# Transaction relay under inbound Sybil fan-in

Target: `parano1d` commit `52938433a30b185ab007afaa15166a632cf2e82e`. All network tests used isolated loopback. No public network was contacted.

**Measured result:** In a real `noid_p2p` transport test, eight inbound Sybils did **not** delay delivery to one honest outbound peer. The sender connected to two Sybils first, then six more and the honest peer. All nine were profile-verified and dispatchable before the first broadcast and before the first GossipSub heartbeat. The honest peer received all 12 early tagged payloads by GossipSub within 0–2 ms, including eight for which the independent direct push did not select it. All four later payloads also arrived. The honest-first control delivered 12/12 early and 4/4 later. The tagged bytes were not proof-bearing transactions, so this measures first-hop transport, not mempool admission or confirmation.

The reason is visible in [`attack.log`](attack.log): the honest peer's own GRAFT entered the sender's transaction mesh before the first broadcast (`GRAFT: Mesh link added`, line 382; first `SEND`, line 391). The first two Sybils had already entered the mesh (lines 158 and 200), but their arrival order did not exclude the honest outbound peer. Other ordinary `noid_p2p` Sybils likewise sent reciprocal GRAFTs, so the sender's mesh grew beyond two before heartbeat maintenance. The probability model below requires that the honest peer *not* GRAFT; this test refutes its use as an observed delay in this topology.

## Mechanism

A node broadcasts each admitted transaction on two paths. It calls GossipSub `publish`, then shuffles **all** connected, dispatchable peers and pushes the transaction directly to four of them once there are more than eight peers. The direct sample has no reserved outbound slot and no failure-domain rule. See [`network.rs`](../../parano1d/noid_p2p/src/network.rs) lines 479–483, 690–696, 5804–5845. Profile verification and identification are required for direct dispatch (same file, lines 1159–1175), but an ordinary protocol-speaking Sybil can pass those checks.

GossipSub is a separate path. The target sets `mesh_n=4`, `mesh_n_low=2`, `mesh_outbound_min=1`, `heartbeat_interval=700 ms`, and `flood_publish=false` in [`behaviour.rs`](../../parano1d/noid_p2p/src/behaviour.rs) lines 209–234. The pinned dependency, `libp2p-gossipsub 0.48.0`, publishes to mesh peers and draws random subscribed peers only if the mesh has fewer than four (`behaviour.rs` lines 584–756 in the local Cargo registry). It does **not** send every transaction to all connected peers. The two paths have independently chosen recipients.

At node startup the first heartbeat is scheduled after the dependency's default five-second initial delay **plus** the configured 700 ms interval (`libp2p-gossipsub` `config.rs:422–425`, `behaviour.rs:465`). An incoming subscription is inserted into the mesh only while it contains fewer than `mesh_n_low=2` peers (`behaviour.rs:1870–1960`). Two Sybils whose subscriptions arrive first can occupy those initial slots. Later honest subscriptions alone do not displace them, but an honest peer can enter immediately by sending its own GRAFT. In the real transport run, it did. The GRAFT admission guard refuses a full-mesh request only from a peer that is **not outbound** (`behaviour.rs:1460–1463`); filling eight slots with inbound Sybils cannot block a later honest outbound GRAFT.

The first heartbeat checks the outbound reserve and grafts one eligible outbound peer if the mesh has none (`libp2p-gossipsub` `behaviour.rs:2223–2250`). If all outbound peers are honest, this repairs the **future** transaction path. The old transaction is not re-published to the newly grafted peer, but the heartbeat also emits IHAVE notices for cached messages to a random subset of subscribed nonmesh peers (`behaviour.rs:2423,2464–2520`). A receiver may then request the transaction. Mempool pull is another recovery path, but the node requests one finite scan from at most four *locally selected outbound* peers on connection/initial sync, not every inbound peer or a periodic poll (`noid_node/src/main.rs:937–951,10820–10833,14756–14778`; `noid_p2p/src/mempool_recovery.rs:1–24,76–89`). An already completed scan does not recover a later missed broadcast.

## Conditional model that the test did not realize

The public inbound policy allows 32 identities per IP, 96 per IPv4 /16 or IPv6 /32, and an unreserved pool of 96 inbound connections ([`peer_diversity.rs`](../../parano1d/noid_p2p/src/peer_diversity.rs) lines 22–34, 145–181). GossipSub score starts with no application topic score and penalizes more than eight identities sharing one exact IP (`behaviour.rs:245–270`; `libp2p-gossipsub` `peer_score/params.rs:164–180`). Its topic-delivery penalties require configured topic parameters; simply receiving and withholding a transaction does not incur one (`libp2p-gossipsub` `peer_score.rs:240–315`). Hence 96 inbound Sybils can fit one /16 using 12 IP addresses with eight identities each and avoid that specific mesh penalty. Only three IPs are needed to occupy 96 inbound slots for the **direct-push** calculation; those peers incur GossipSub colocation penalties and should not be assumed eligible for mesh capture. The target's libp2p limits are 128 inbound and 64 outbound connections (`noid_p2p/src/behaviour.rs:536–546`), so 96 attacker inbound plus eight honest outbound fits both limits.

For `A` attacker and `H=8` honest connected peers, all subscribed and dispatchable, suppose the first two mesh subscriptions were attacker-controlled. GossipSub then chooses two extra recipients uniformly from the remaining `A−2+H` peers. The direct path chooses four uniformly from all `A+H` peers. Under independent random draws:

`P(gossip misses honest) = C(A−2, 2) / C(A+H−2, 2)`

`P(direct misses honest) = C(A, 4) / C(A+H, 4)`

| Attacker connections | IPs at 8 per IP | Gossip misses | Direct misses | Both miss |
| ---: | ---: | ---: | ---: | ---: |
| 32 | 4 | 61.88% | 39.35% | 24.35% |
| 64 | 8 | 78.30% | 61.76% | 48.36% |
| 96 | 12 | 84.86% | 72.25% | 61.31% |

These are conditional *first-hop* probabilities, not measured transaction loss. They require that attacker nodes receive but do not forward, the victim has no other application relay, the honest peers neither enter its mesh nor send GRAFT, and the broadcast occurs before the initial heartbeat. The 5.7-second timer starts when GossipSub is constructed, not when the peers connect; if connections arrive later, the remaining window is shorter. Real timing, GRAFTs, recipient scoring, IHAVE/IWANT recovery, and other peers' network topology can reduce or end the delay. The real `noid_p2p` run failed this model's no-GRAFT premise. A lasting all-attacker mesh would also need an attacker-controlled outbound member or another way around the outbound reserve. The table does not estimate how often that topology occurs.

## What survives the attack

Inbound saturation did not produce a first-hop delay in the tested topology, and does not by itself produce a durable eclipse. An honest outbound peer can GRAFT immediately even when the mesh is full; heartbeat maintenance later reserves one eligible outbound mesh peer. The node's separate locally selected outbound set also owns proactive chain discovery and mempool pulls. The source shows **direct-push redundancy erosion** under inbound fan-in. We have no measured transaction delivery failure and no proof that an ordinary attacker can prevent confirmation.

## Reproduction and limits

The harness is [`src/main.rs`](src/main.rs), built from [`Cargo.toml`](Cargo.toml). It starts ten target `P2PNetwork` instances: sender, eight Sybils, and one honest peer. Each receiver logs `NewTx` and returns `Ignore`, so no receiver forwards a tagged payload. The sender uses the production `BroadcastTx` command. `inbound_memory_permit` distinguishes direct Push from GossipSub delivery. In the attack run, Sybils 0–1 connect first; in the control, the honest outbound peer connects first. Both runs wait for all nine sender peers to be profile-verified before sending 12 early and four later unique payloads. The attack sender reached nine peers at age 4.026 s; the control at 3.990 s. Every early broadcast completed before the scheduled 5.7 s first heartbeat.

| Run | Honest early receipts | Honest later receipts | Honest direct Push receipts among early payloads | First honest delivery |
| --- | ---: | ---: | ---: | ---: |
| Sybils first ([log](attack.log)) | 12/12 | 4/4 | 4/12 | 0–2 ms |
| Honest first ([log](control.log)) | 12/12 | 4/4 | 4/12 | 0 ms |

The loopback IP is whitelisted from GossipSub colocation scoring, so this test does not reproduce the public-IP cost model above. The honest peer was a fresh singleton: its own mesh had fewer than two members, so the target's normal subscription handler sent GRAFT to the sender. An established honest peer whose mesh already has at least two members would not send that automatic GRAFT (`libp2p-gossipsub` `behaviour.rs:1938–1960`). That topology remains untested; deliberately silent custom Sybils are another possible variant. A future decisive test would use an established honest outbound peer, observe IHAVE/IWANT and mempool recovery, and eventually use a genuine proof-bearing transaction. None of those outcomes is implied by this run.

Run commands (separate ports; never run against a public peer):

```sh
CARGO_TARGET_DIR=/Users/ember/dev/parano1d-break/experiments/p2p-resources/target cargo build --release --manifest-path experiments/sybil-availability/Cargo.toml -j 2
RUST_LOG='warn,libp2p_gossipsub::behaviour=debug' experiments/p2p-resources/target/release/sybil-relay-transport control 19930 > experiments/sybil-availability/control.log 2>&1
RUST_LOG='warn,libp2p_gossipsub::behaviour=debug' experiments/p2p-resources/target/release/sybil-relay-transport attack 19910 > experiments/sybil-availability/attack.log 2>&1
```

The direct relay could reserve some of its four slots for locally selected outbound peers in distinct public groups, then fill the remainder from all dispatchable peers. The cold-start mesh path could also send to a bounded set of those outbound peers before its first heartbeat. Both changes keep bounded fanout while removing the easiest inbound-only dilution. They still cannot identify a dishonest outbound peer; that requires broader topology and delivery measurements.
