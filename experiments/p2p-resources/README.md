# P2P transport byte-budget and serving-admission audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e` (verified `git rev-parse HEAD`; `noid_p2p` and `noid_chain` clean at the pin; pre-existing unrelated working-tree modifications elsewhere in the checkout were left untouched).

All created files are confined to this directory. The PoCs drive the real production codecs and the real process-global outbound byte budget from `noid_p2p` with synthetic in-memory writers that stop reading mid-write (what a non-reading libp2p/yamux peer looks like after its ~256 KiB stream window fills). No node, swarm, socket, or network was started; no peer was contacted.

Prior confirmed findings (profile collision across version domains; per-scan recovery-txid suppression) are not re-litigated here.

## Finding 1: four cheap mempool pulls can reserve the entire process-wide outbound byte domain and stall every other large-response protocol

**Severity: Medium availability impact, unauthenticated remote peer, no acceptance impact.**

Production reserves the complete legal maximum — `MAX_MEMPOOL_SYNC_BYTES` = 16 MiB — from the single process-global 64 MiB outbound domain (`OUTBOUND_RESPONSE_BUDGET_BYTES`, `noid_p2p/src/outbound_budget.rs:20,43-50`) for **every** served mempool pull, before cloning any bytes and regardless of the actual response size:

1. `noid_p2p/src/network.rs:319-344` (`prepare_mempool_response_after_admission`) unconditionally calls `budget.acquire(MAX_MEMPOOL_SYNC_BYTES)`; even a node with an **empty mempool** reserves 16 MiB.
2. Called for every `Pull`/`PullMissing` request at `network.rs:8845`; the permit rides inside `GetMempoolResponse` until the codec finishes its last wire write (`mempool_sync_codec.rs:286`, `_memory_permits` held across the write loop at `:298-301`).
3. Exactly `64 MiB / 16 MiB = 4` concurrent mempool responses therefore consume the entire domain.

A peer that opens 4 mempool sync streams (the protocol allows 8 per connection, `behaviour.rs:422`; up to 2 connections per peer identity, `behaviour.rs:546`) and then stops reading stalls each response write after ~256 KiB (yamux initial window; a single legal intent may be 303,495 bytes, `noid_tx/src/paged_spend.rs:55`, so one large mempool transaction suffices to make the write stall). While the domain is pinned:

- Every later budget-participating write blocks on the same semaphore FIFO: exact-object responses (`network.rs:7229` `acquire_with_serving`), history-step terminals (`network.rs:7871`, `history_step_codec.rs:149`), state segments (`state_segment_codec.rs:454`), fork origins (`fork_origin.rs:240-242`).
- The blocked exact-object tasks each **hold an active data-plane serving slot** (class + peer + global permits taken at `network.rs:7226` before the byte acquire). With 6 Live-class active slots (`resource_profile.rs:30-39,80-85`), six blocked requests exhaust the class; further object requests then receive `Busy` (`network.rs:7192-7211`). The starvation therefore converts from delay to rejection for **all** peers of the node, not just the attacker.
- Verified in the PoC at codec level: four full mempool writes pin the domain; a later 1 KiB budget-participating write blocks until one aggressor finishes; a freed aggressor admits the queued victim in milliseconds.

Cost and limits, honestly scoped:

- The request is 12 bytes per stream. Sustaining the pin past the mempool protocol's 30 s request timeout (`behaviour.rs:418`) requires re-issuing pulls every ≤30 s — the handler aborts a stalled inbound write at `InboundTimeout` (`libp2p-request-response-0.28.0/src/handler.rs:136-155,418`), which drops the permit. Rotation is trivial and keeps the domain pinned with near-100 % duty cycle.
- Precondition: the node's mempool must hold > ~256 KiB of intents at pull time for the write to actually stall (smaller responses complete into the yamux buffer and release quickly). On an active network this is ordinary; an attacker able to keep valid intents in the mempool can also arrange it themselves.
- Four honest peers on slow links syncing a full mempool produce the same cross-protocol starvation by accident; the consequence is not exclusive to malice.
- No invalid data is accepted; consensus is untouched. Purely serving liveness.

### Recommended correction

Reserve the actual response size (bounded by the length table / clone result) instead of the unconditional maximum, or give mempool serving its own bounded sub-domain so bulk mempool fan-out cannot consume the byte budget shared by object/segment/terminal serving. Releasing blocked data-plane serving slots when the requesting stream times out would also avoid slots being held by dead requests.

## Finding 2: header-sync serving memory is outside every process-wide budget; aggregate retention scales with attacker connections

**Severity: Low-to-Medium availability/memory impact, unauthenticated remote peer, by-design exclusion with quantified cost.**

`GetHeadersResponse` carries no permit and `HeaderSyncCodec` never consults any budget: header serving "deliberately bypasses" the data-plane admission (`network.rs:93-95`), and the response struct has no permit field (`protocol.rs:55-63`). During `write_response` the codec holds **both** the canonical buffer (up to `MAX_UNCOMPRESSED_HEADER_BYTES` = 4096 × 288 = 1,179,648 bytes) and the compressed buffer while the final payload write is stalled by a non-reading peer (`header_sync_codec.rs:288-335`; `canonical` is not dropped before the payload write at `:335`).

The only serving admission is `header_response_prepare_semaphore` (8 slots in full-node mode, `network.rs:3906-3908`, `resource_profile.rs:23-28`), and its permit covers only storage preparation: it is held by the spawned task until the response is handed to the event loop (`network.rs:7659-7693`) and dropped at task exit (`:7785-7795`) — it does not cover the wire write. Concurrent stalled header writes are bounded only by streams × connections: 8 chain_sync streams per connection (`behaviour.rs:329`), 2 connections per peer, 128 inbound connections, 32 per IP / 96+8 per /16 (`behaviour.rs:539,546`; `peer_diversity.rs:22-32`).

Verified in the PoC: 64 concurrent stalled max-batch writers retained 75,497,472 canonical bytes (content-independent lower bound — already larger than the entire 64 MiB shared domain all other large protocols respect) plus 35,195,688 compressed bytes for this synthetic content. ~39 stalled streams exceed the 64 MiB domain-equivalent at this content's compressibility; ~57 streams using the canonical lower bound alone. Worst case with the full 128-connection inbound budget (~1024 streams) is on the order of 1-2 GB of resident buffers, reached with 16-byte requests. Each individual write is aborted after the 30 s chain_sync timeout, so the attacker rotates streams; every rotation re-runs storage preparation (MDBX read of up to 4096 headers), so the attack also costs repeated node I/O.

The exclusion is documented as intentional — keeping bulk clients from occupying the control plane — and per-stream caps are real; the unquantified consequence is that header serving itself becomes the one large-response protocol whose retained bytes scale with peer count, against the stated goal of the byte domain ("without scaling with peer count", `outbound_budget.rs:18-19`).

### Recommended correction

Admit header response bytes into the outbound domain (or a dedicated header sub-budget sized to the per-stream cap × desired stream count), acquiring the permit for the canonical+compressed lifetime inside the codec as the mempool/segment codecs already do.

## Negative and control results

- **Rejected: "exact-object codec write blocked by the shared domain."** An earlier unverified variant of this PoC (inherited from a dead agent, never run to completion — it also deadlocked by never releasing a writer gate) asserted that an object response write was blocked behind four mempool reservations, and its assert passed. Instrumentation showed this was a **scheduling artifact**: the check used 32 cooperative `yield_now()`s instead of real time. With 300 ms of wall-clock the same object write completes during a full-domain pin — `ObjectCodec::write_response` contains no budget acquire at all (`object_codec.rs:186-247`). The production blocking point is the earlier network-layer acquire (`network.rs:7229`), which an external PoC cannot reach because the budget type is `pub(crate)`. The final PoC keeps this as an explicit control: the object codec write completes during the pin while a 1 KiB budget-participating mempool write stays blocked.
- **Rejected: malformed-frame panics in the sync codecs.** Static review of every read path (`header_sync_codec`, `mempool_sync_codec`, `object_codec`, `history_step_codec`, `state_manifest_codec`, `manifest_page_codec`, `state_segment_codec`, `availability_codec`, `fork_origin`): attacker-controlled lengths are checked against count-relative caps before allocation (`validate_count`, `validate_compressed_len`, `parse_lengths`, `response_payload_len`), arithmetic is checked, and all `unwrap()`s found are `try_into().unwrap()` on compile-time-fixed slice sizes or test-only. No remotely triggerable panic identified by review; this is not a proof of absence.
- **Header Busy backpressure works as designed:** when the 8 preparation slots are full, new header requests receive a payload-free `Busy` response with a retry hint (`network.rs:7659-7688`); legacy v4 peers degrade to an empty canonical response (`header_sync_codec.rs:275-284`).
- **Guardrails that bound both findings:** per-stream codec caps (4096-record batches; 16 MiB mempool responses; intent ≤ 303,495 B); 8 streams per connection per protocol; 2 connections per peer; 128 inbound / 64 pending-inbound connections; per-IP 32 and per-/16-group 96+8 inbound diversity caps; 30-60 s request timeouts that abort stalled inbound writes and release permits/buffers; inbound decoded payloads symmetrically bounded by a separate 64 MiB domain (`inbound_budget.rs`).
- **No application-level ban/score system exists to bypass:** peer scoring is gossipsub's defaults plus an IP-colocation factor (threshold 8, loopback whitelisted, `behaviour.rs:255-273`); enforcement observed is immediate `disconnect_peer_id` at specific protocol violations (e.g. network-profile mismatch, `network.rs:7124-7162`). The outbound peer cache records only successfully dialed public peers (`peer_store.rs:7-12`), so inbound identities cannot poison it.
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
- Severity preconditions for Finding 1 (mempool > ~256 KiB at pull time, attacker rotation every ≤30 s) and Finding 2 (≈5-8 fully occupied connections or ≳39 stalled streams) are stated, not benchmarked end-to-end; mempool admission of attacker-supplied intents is another lane's open question and is not claimed here.
- Static review of decode paths is not exhaustive; "no panic found" is not "no panic exists". Gossip replay, eclipse/Sybil topology dynamics beyond the diversity caps cited, and recovery-stall behavior were not exercised.
