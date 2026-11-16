# P2P response budgets and stalled readers

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

The local PoCs use production `noid_p2p` codecs and the process-global outbound byte budget. Synthetic writers stop accepting bytes mid-write to model a peer that stops reading after its yamux stream window fills. This report's experiments used no node or socket. A later [live loopback probe](../network-abuse/README.md) tests the header-serving path under narrower, stated conditions.

The two questions are whether stalled mempool responses can reserve the full shared outbound budget, and how many bytes stalled header responses retain outside that budget.

## Finding 1: four stalled mempool responses can reserve the outbound byte budget

**Availability risk: medium if the mempool response exceeds the stream window.** No consensus acceptance effect was found. The shared-budget blocking was reproduced at codec level; an end-to-end mempool pull against a live node was not run in this lane.

Production reserves the complete legal maximum — `MAX_MEMPOOL_SYNC_BYTES` = 16 MiB — from the single process-global 64 MiB outbound domain (`OUTBOUND_RESPONSE_BUDGET_BYTES`, `noid_p2p/src/outbound_budget.rs:20,43-50`) for **every** served mempool pull, before cloning any bytes and regardless of the actual response size:

1. `noid_p2p/src/network.rs:319-344` (`prepare_mempool_response_after_admission`) unconditionally calls `budget.acquire(MAX_MEMPOOL_SYNC_BYTES)`; even a node with an **empty mempool** reserves 16 MiB.
2. Called for every `Pull`/`PullMissing` request at `network.rs:8845`; the permit rides inside `GetMempoolResponse` until the codec finishes its last wire write (`mempool_sync_codec.rs:286`, `_memory_permits` held across the write loop at `:298-301`).
3. Exactly `64 MiB / 16 MiB = 4` concurrent mempool responses therefore consume the entire domain.

A peer can open four mempool sync streams within the configured eight per connection (`behaviour.rs:422`; up to two connections per peer identity, `behaviour.rs:546`). If each response exceeds yamux's ~256 KiB initial stream window, a peer that stops reading can stall the writes while all four 16 MiB permits remain held. A single legal intent may be 303,495 bytes (`noid_tx/src/paged_spend.rs:55`), although this experiment did not show such an intent admitted into a live node's mempool. While the domain is reserved:

- Later budget-participating writes wait on the same semaphore: exact-object responses (`network.rs:7229` `acquire_with_serving`), history-step terminals (`network.rs:7871`, `history_step_codec.rs:149`), state segments (`state_segment_codec.rs:454`), and fork origins (`fork_origin.rs:240-242`). This is a source-path consequence, not a live-node observation from this lane.
- The exact-object path takes a data-plane serving slot before the byte permit (`network.rs:7226`). It has six Live-class active slots (`resource_profile.rs:30-39,80-85`). If six such requests wait behind the pinned byte budget, later requests receive `Busy` (`network.rs:7192-7211`). The PoC does not drive this network-layer composition.
- Verified in the PoC at codec level: four full mempool writes reserve the domain; a later 1 KiB budget-participating write blocks until one stalled writer finishes; releasing that writer admits the queued response in milliseconds.

Cost and limits:

- The request is 12 bytes per stream. The mempool protocol's 30 s timeout (`behaviour.rs:418`) bounds each stalled write: the handler aborts it at `InboundTimeout` (`libp2p-request-response-0.28.0/src/handler.rs:136-155,418`) and releases the permit. Sustained pressure would require repeated pulls timed around those releases; duty cycle was not measured.
- The node's mempool must hold more than roughly 256 KiB of response data for the write to stall under the default yamux window. Whether that condition holds on a given network, or can be maintained by a particular peer, was not measured here.
- Four slow readers of full mempool responses can create the same budget contention regardless of intent.
- No invalid data is accepted; consensus is untouched. Purely serving liveness.

### Recommended correction

Reserve the actual response size (bounded by the length table / clone result) instead of the unconditional maximum, or give mempool serving its own bounded sub-domain so bulk mempool fan-out cannot consume the byte budget shared by object/segment/terminal serving. Releasing blocked data-plane serving slots when the requesting stream times out would also avoid slots being held by dead requests.

## Finding 2: stalled header writes retain buffers outside the shared byte budget

**Availability risk: conditional memory pressure.** This codec-level experiment measures retained buffers for synthetic maximum-size responses. The later live probe measured a smaller and more qualified effect.

`GetHeadersResponse` carries no permit and `HeaderSyncCodec` never consults any budget: header serving "deliberately bypasses" the data-plane admission (`network.rs:93-95`), and the response struct has no permit field (`protocol.rs:55-63`). During `write_response` the codec holds **both** the canonical buffer (up to `MAX_UNCOMPRESSED_HEADER_BYTES` = 4096 × 288 = 1,179,648 bytes) and the compressed buffer while the final payload write is stalled by a non-reading peer (`header_sync_codec.rs:288-335`; `canonical` is not dropped before the payload write at `:335`).

The only serving admission is `header_response_prepare_semaphore` (8 slots in full-node mode, `network.rs:3906-3908`, `resource_profile.rs:23-28`), and its permit covers only storage preparation: it is held by the spawned task until the response is handed to the event loop (`network.rs:7659-7693`) and dropped at task exit (`:7785-7795`) — it does not cover the wire write. Concurrent stalled header writes are bounded only by streams × connections: 8 chain_sync streams per connection (`behaviour.rs:329`), 2 connections per peer, 128 inbound connections, 32 per IP / 96+8 per /16 (`behaviour.rs:539,546`; `peer_diversity.rs:22-32`).

In the codec PoC, 64 concurrent stalled maximum-batch writers retained 75,497,472 canonical bytes plus 35,195,688 compressed bytes for the synthetic content. The canonical figure follows from 64 × 4096 × 288-byte records; compressed size depends on content. At that synthetic size, ~39 stalled streams exceed 64 MiB of retained canonical-plus-compressed bytes, and ~57 exceed it using canonical bytes alone. The configured 128 inbound connections and eight streams per connection permit a larger *theoretical* aggregate, but simultaneous reachability, memory use, and sustained rotation were not established by this experiment. The 30 s chain-sync timeout bounds each stalled write.

The budget exclusion is intentional: header service remains available when bulk data serving is constrained. It also means stalled header buffers can grow with concurrent streams. The source comment says the outbound byte domain is meant to avoid scaling with peer count (`outbound_budget.rs:18-19`); that guarantee does not cover header writes.

The later [loopback node test](../network-abuse/README.md) found that eight non-reading streams on a 150-header chain did **not** delay another header read: the ~18 KiB responses fit inside the yamux window. A separate serving-path fixture with 4096 syntactically decodable but consensus-invalid header rows produced a 409,436 B response and showed increased RSS and transient latency for other header reads during a 40-request burst. Every concurrent response still returned `Ready`. That fixture establishes a wire-stall path; it does not show persistent loss of header access on a valid chain.

### Recommended correction

Admit header response bytes into the outbound domain (or a dedicated header sub-budget sized to the per-stream cap × desired stream count), acquiring the permit for the canonical+compressed lifetime inside the codec as the mempool/segment codecs already do.

## Negative and control results

- **Object codec control:** `ObjectCodec::write_response` completes during a full-domain pin; it does not acquire the budget (`object_codec.rs:186-247`). The production exact-object path acquires the permit earlier, in the network layer (`network.rs:7229`). The PoC confirms that a 1 KiB budget-participating mempool write remains blocked while the unbudgeted object codec write completes. An earlier check based on cooperative yields had mistaken scheduling delay for budget blocking.
- **Malformed frames:** Review of the listed sync codec read paths found length checks before allocation, checked arithmetic, and no identified remotely triggerable panic. This was static review, not an exhaustive proof.
- **Header Busy backpressure works as designed:** when the 8 preparation slots are full, new header requests receive a payload-free `Busy` response with a retry hint (`network.rs:7659-7688`); legacy v4 peers degrade to an empty canonical response (`header_sync_codec.rs:275-284`).
- **Guardrails that bound both findings:** per-stream codec caps (4096-record batches; 16 MiB mempool responses; intent ≤ 303,495 B); 8 streams per connection per protocol; 2 connections per peer; 128 inbound / 64 pending-inbound connections; per-IP 32 and per-/16-group 96+8 inbound diversity caps; 30-60 s request timeouts that abort stalled inbound writes and release permits/buffers; inbound decoded payloads symmetrically bounded by a separate 64 MiB domain (`inbound_budget.rs`).
- Peer scoring uses gossipsub defaults plus an IP-colocation factor (threshold 8, loopback whitelisted, `behaviour.rs:255-273`). Some protocol violations trigger `disconnect_peer_id` (for example profile mismatch, `network.rs:7124-7162`). The outbound peer cache records successfully dialed public peers (`peer_store.rs:7-12`), so the reviewed inbound requests do not enter that cache.
- The mempool prepare path drops the request when its single-slot semaphore is busy (`network.rs:8829-8837`) — excess concurrent pulls are rejected rather than queued; the pin needs the write phase, not preparation, which is why 4 concurrent stalls are still reachable.

## PoC implementation and reproduction

`src/main.rs` (single binary, `p2p-resources-poc`) drives the pinned tree's real `HeaderSyncCodec`, `MempoolSyncCodec`, `ObjectCodec` and, through them, the real `OutboundResponseBudget::process_global()` singleton. Synthetic `GatedWriter`s accept writes and then park forever at a chosen write index, which is what a yamux peer that stops reading does to the server once its ~256 KiB stream window is full. No sockets.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final output (full listing in `results.txt`):

```text
header-aggregate PoC
  stalled concurrent max-batch header payload writers: 64
  canonical bytes retained (content-independent lower bound): 75497472
  compressed bytes retained (this synthetic content): 35195688
  aggregate canonical+compressed: 110693160
  documented process-wide outbound domain all other large protocols share: 67108864
  header-sync bytes admitted by that domain: 0
  streams needed to exceed the shared 64 MiB domain at this per-stream cost: ~39
  ...
mempool-pin PoC
  process-global outbound domain: 67108864 bytes
  production reservation per served mempool pull: 16777216 bytes (network.rs:327-330, unconditional)
  full-domain aggressors needed: 4
  aggressors holding the entire domain mid-write: 4/4
  1 KiB mempool response (budget-participating) blocked: true
  exact-object codec write completed during the pin: true
  header max-batch payload write reached during the pin: true
  victim admitted after releasing one aggressor: true (27 ms)
  all writers drained after release: true
ALL ASSERTIONS PASSED
```

Both scenarios can also be run individually: `cargo run --release -- header-aggregate` / `-- mempool-pin`.

## Assurance limits

- No live libp2p swarm was constructed (no network by lane rules). Everything demonstrated runs at the codec/budget layer of the pinned production code; the production-path claims for exact-object/history-step/segment blocking (`network.rs:7192-7235`, `:7863-7875`) and for the mempool write-phase permit lifetime are established by source reading at the pinned commit, not by driving the swarm — the admission types are `pub(crate)` and cannot be driven externally.
- The stall model assumes the libp2p yamux default initial stream window of 256 KiB (`yamux-0.13.10` `DEFAULT_CREDIT`, selected by `libp2p-yamux-0.47.0` default config, which the node uses via `yamux::Config::default` at `network.rs:3742`). A different negotiated window would shift the >256 KiB precondition, not the mechanism.
- The 30 s write-abort bound comes from `libp2p-request-response-0.28.0` handler sources (`substream_timeout` wraps the whole inbound exchange); it was not measured against a live swarm.
- The header-aggregate compressed-bytes figure depends on synthetic record content; the canonical-bytes lower bound (75,497,472 for 64 streams) is content-independent. Real mainnet headers compress differently.
- Finding 1 requires a response larger than the stream window and repeated requests for sustained pressure. Finding 2's ~39-stream figure applies to the maximum-size synthetic payload; a valid chain's compressed headers may be much smaller. Neither was benchmarked end to end in this lane.
- Codec review was not exhaustive. This lane did not exercise gossip replay, Sybil topology, or recovery behavior. The later live header test observed transient latency, not a persistent eclipse.
