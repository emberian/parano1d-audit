# U21 gossip-window exhaustion — live run report

Lane `gossip-halt`, campaign 2026-11-15/16. Source: `~/dev/parano1d-break/parano1d`
at `52938433a30b185ab007afaa15166a632cf2e82e` (read-only); stock release binaries
`experiments/devnet-chaos/bin/{parano1d,parano1d-miner}`.

**Status: DRAFT — source mechanics section complete; live-run sections pending.**

## 1. The window's real mechanics (source, file:line)

The U21 model ("a shared 64 MiB/10s gossip byte window charged BEFORE decode,
shared with block announcements") is essentially correct, with one correction:
the "10 s window" is a **tumbling byte budget**, not a sliding limiter, and the
push path it imagined is **gossipsub topic publish**, not the `/sync/mempool`
request-response Push.

### 1.1 The window

- `noid_p2p/src/network.rs:492-493`:
  - `GOSSIP_ACCEPT_WINDOW: Duration = 10s`
  - `GOSSIP_ACCEPT_BYTES_PER_WINDOW: usize = 64 * 1024 * 1024`
- `GossipByteWindow` (`network.rs:5143-5172`): a process-global counter
  `bytes: usize` plus `started_at: Instant`. `admit(bytes, max, window)`:
  - if more than 10 s have elapsed since `started_at` → reset `bytes = 0`,
    `started_at = now` (only then);
  - if `bytes + n > max` → **refuse** (return false) — and crucially,
    `started_at` is NOT advanced on refusal, so the window stays full until the
    next tumble, up to 10 s later.
- Exactly one instance exists per node process
  (`let mut gossip_accept_bytes = GossipByteWindow::new()` at `network.rs:3853`),
  so all peers and both traffic classes share the same 64 MiB.

### 1.2 Who charges it, and when

**Transaction gossip (attacker path)** — `network.rs:6542-6630`, the
gossipsub `txs`-topic handler:

1. size gate: `message.data.len() > MAX_TX_INTENT_BYTES_GLOBAL (=303,495)`
   → Reject (`6543-6550`);
2. `admit_tx_gossip_ingress(...)` (`network.rs:5174-5187`), in order:
   - per-peer count: `allow_peer_rate(..., TX_RELAY_RATE_MAX=50, TX_RELAY_RATE_WINDOW=10s)`
     (`network.rs:484-485`) — checked first, and the count is incremented
     **before** any decode, so malformed payloads consume the sender's allowance;
   - then `gossip_accept_bytes.admit(len, 64 MiB, 10s)` — **the shared byte
     window, charged before decode**. The node decodes the intent only later
     (in the node event loop), so an undecodable payload spends the budget.
3. only after admission: duplicate coalescing (`tx_deliveries`), then
   `gossip_event_tx.send(NetworkEvent::NewTx { ... })`.

**Block announcements (victim traffic)** — `network.rs:6479-6541`, the
gossipsub `blocks`-topic handler:

1. `HeaderAnnouncement::decode(&message.data)` (decode happens FIRST here);
2. `sync_paths.is_dispatchable(propagation_source)` gate;
3. per-peer count `BLOCK_RATE_MAX=40 / BLOCK_RATE_WINDOW=10s`;
4. **the same `gossip_accept_bytes.admit(...)`** (`network.rs:6491-6495`).
   On any failure: `report_gossip_validation(..., Ignore)` and the log line
   `block announcement rate limit exceeded — dropped before propagation`
   (`network.rs:6503`).

So: tx gossip (pre-decode) and block announcements (post-decode) compete for
one 64 MiB/10 s domain. When the window is full, announcements receive
`Ignore` until the window tumbles.

### 1.3 What is NOT this window (the phase-7 divergence)

The devnet-chaos phase-7 push storm used the **request-response**
`/sync/mempool/4` `MempoolRequest::Push` arm (`network.rs:8879-8932`). That
path charges a **different** budget: the process-global 64 MiB inbound
*semaphore* (`inbound_budget.rs:19`, acquired at `mempool_sync_codec.rs:141-157`
before the payload is read, permit travels with the event). Its payloads failed
at node-side mempool decode, which releases the semaphore permit quickly — that
is why phase 7 saw control-plane tails but no announcement starvation. This
experiment targets the gossip byte window instead; both are anonymous, but only
the gossip window is shared with announcements. (FetchHeaders responses charge
neither budget — header sync is outside both domains.)

### 1.4 Delivery requirements for the attacker (gossipsub 0.48.0)

The node's gossipsub config (`noid_p2p/src/behaviour.rs:208-245`):

- `MessageAuthenticity::Signed` + `ValidationMode::Strict` — every inbound
  publish must carry `from` (a PeerId whose key verifies the signature), an
  8-byte `seqno`, and an ed25519 signature over
  `b"libp2p-pubsub:" + protobuf(message minus signature/key)`;
- `message_id_fn` = poseidon2b hash of the message data (content-addressed:
  unique payloads ⇒ unique ids ⇒ no dedupe);
- `validate_messages()` — the app (network.rs handler above) runs inline;
- `max_transmit_size = max(303,495, 512) = 303,495` — the **whole RPC
  frame** must fit (quick-protobuf-codec rejects larger frames);
- mesh 4/2/8, heartbeat 700 ms, `flood_publish=false`;
- peer score: default params + `ip_colocation_factor_threshold = 8` with
  **loopback whitelisted** (`behaviour.rs:255-262`) — anonymous identities on
  one host are not penalized for co-location; `Ignore` verdicts carry no
  penalty, so the attacker's identities persist.

Critically, gossipsub 0.48 delivers a received publish to the application
whenever the **receiver** is subscribed to the topic
(`libp2p-gossipsub-0.48.0/src/behaviour.rs:1798`, `mesh.contains_key(&message.topic)`)
— the sender needs no mesh membership, subscription, or credential beyond a
live noise-XX + yamux connection and a `/meshsub/1.1.0` stream. The node
subscribes to both topics at startup (`network.rs:3801-3802`).

### 1.5 Why exhaustion halts header learning (not just delays it)

For a synced node, new headers arrive via announcements; the stale-tip
detection that re-requests headers is armed by `highest_announced`, which only
advances on announcements (`noid_node/src/main.rs:9824-9832`,
`record_authenticated_height` at `9875-9884`). There is no timer-based header
pull for a synced node (FetchHeaders fires at bootstrap and on announcements).
A node whose announcements are all Ignored therefore does not know it is
behind: its HeaderDAG freezes, and its miner keeps building on the stale tip
while peers advance.

### 1.6 Exhaustion arithmetic

- Window: 64 MiB = 67,108,864 B per 10 s.
- Max tx-gossip payload: 303,495 B; the frame (protobuf overhead ~165 B)
  must fit 303,495, so practical data size ≈ 303,300 B.
- Messages to fill one window: 67,108,864 / 303,300 ≈ 221.3 → **222**.
- Per-identity rate: 50 msgs / 10 s → **5 identities × 50 = 250 ≥ 222**,
  offering 75.8 MB per window vs the 64 MiB cap (the tx-admission model's
  arithmetic, confirmed). 4 identities offer 60.7 MB < 64 MiB — below the
  threshold.

### 1.7 Attacker cost (per 10 s window, 5 identities)

- Wire: ~7.6 MB/s loopback (or remote uplink), 250 TCP frames.
- CPU: 250 ed25519 signatures + 250 × 303 KB payload copies (trivial); the
  victim pays a 303 KB poseidon2b message-id hash + one ed25519 verify per
  message plus a decode attempt.
- Identities: 5 fresh noise keys; no stake, no operator credential, no
  whitelist entry; remote-capable (only TCP reachability of the victim's p2p
  listener is needed).

## 2. Live run (pending)

TODO: baseline, controls (single/quad/small/pin), storm, recovery, verdict.

## 3. Controls (pending)

## 4. Verdict (pending)
