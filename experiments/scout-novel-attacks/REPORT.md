# scout-novel-attacks: novel attack-class hypotheses

Scout lane over the pinned checkout `parano1d/` @ 52938433 (READ-ONLY). Scope: attack
classes NOT already in `experiments/FINDINGS.md`. Each candidate is KILLED (with the
killing evidence) or PROMISING (mechanism + first discriminating probe + expected cost).
Ranked at the end. No devnets were run (scout only); the strongest probe is unit-level.

---

## N1 (PROMISING, top): deterministic native-wallet slot-hint prediction -> output-slot squatting (targeted send-blocking)

**Class:** mempool/wallet race. A remote attacker with **no node control, no mining, no
local access** blocks a victim's native (V4) wallet operation by predicting the victim's
output-slot hints from public data and pre-squatting those slots with first-seen mempool
transactions.

### Mechanism (file:line)

1. **Deterministic seed.** The native planner derives its slot-hint seed from public
   data only: `seed = blake3(serde_json(request))[..8] ^ tip.state_root[..8] ^
   owner_address[..8]` — `noid_rpc/src/server/native/planner.rs:405-408`. The request
   is `NativeWalletRequest` (`noid_rpc/src/native_types.rs:119-179`). For a marketplace
   purchase the attacker *is the seller*: `BuyNft { collection_id, serial,
   maximum_ask_micronoid }` is fully attacker-known (they set the ask); the victim's
   owner address and tip state root are public chain state.
2. **Deterministic hints.** `collect_empty_slot_hints`
   (`noid_rpc/src/wallet_submit.rs:87-209`) picks empty slots as a pure function of
   (seed, reserved set, state emptiness, segment live counts). The reserved set is the
   mempool's pending slots (`noid_mempool/src/pool.rs:968`, snapshot at
   `noid_rpc/src/server/native.rs:183`), observable by any node via gossip.
3. **First-seen admission.** Mempool admission rejects any tx whose output slot is
   already claimed by an admitted tx (`noid_mempool/src/pool.rs:96,277,533-541`).
4. **Single-shot submit with plan check.** Non-Trade native submits have no internal
   retry: `submit` re-prepares and compares `plan_id`
   (`noid_rpc/src/server/native.rs:214-234`); a reserved-set change between preview and
   confirm (i.e., the attacker's squat landing) produces
   `"native plan changed; preview and review the operation again"`.
5. **Post-confirmation lockup (composition).** If the squat confirms while the
   victim's tx is pending, the victim's tx is evicted `OutputSlotOccupied`
   (`pool.rs:816-846`), and C28's demonstrated wallet behavior (input stays reserved,
   zero spendable, until restart) applies — the register's C28 boundary explicitly
   notes "no targeted custom producer ... was shown"; this is the targeted variant
   that needs neither a producer nor node control.

### Attack shapes (all follow from 1-4)

- **Preview/confirm purgatory:** attacker pre-squats the victim's *next* deterministic
  hints (the tree is computable because the reserved set is public + attacker-owned),
  so each confirm re-prepares with new hints and fails the plan-id check. Cost per
  round: 1-2 ordinary attacker txs (fees only; proofs precomputable offline). The
  victim cannot escape by re-previewing — the pursuit stays one deterministic step
  ahead. Mass-squatting whole segments self-defeats (`minimum_headroom` defers scarce
  segments, `wallet_submit.rs:127-137`), but per-round pursuit does not.
- **First-seen rejection at home:** if the squat is gossiped before the victim's tx
  exists, the victim's own node rejects their tx with a slot conflict at admission
  (wallet journal cleaned via `forget_native_activity`,
  `noid_rpc/src/server/native.rs:288-295`). User-visible send failure, repeatable.
- **Miner-race variant (C28 composition):** squat emitted after the victim's tx
  appears in gossip but before miner admission; whichever tx reaches each miner first
  claims the slot there. If the squat wins at the miner and confirms, the victim's
  wallet enters the C28 stuck-reservation state.

### Why prior work missed it

The hint machinery *looks* salted: `mix_slot_hint_seed`
(`noid_rpc/src/server.rs:299-301`) and the ordinary-send path salts with
`SystemTime::now().subsec_nanos()` (`server.rs:1028-1031`) — unpredictable. But the
**native planner path** (`planner.rs:405-408`, used by preview, submit, and the
client planner `noid_rpc/src/server/native/client.rs:190,202`) mixes in no time and no
per-call nonce. Prior lanes treated slot conflicts as accidental (C28: "the fixture
chose a colliding wallet output hint") or node-controlled (C24); nobody priced the
*remote deterministic prediction* position.

### First discriminating probe (cheap)

In a scratch checkout (not the pinned tree): fixture chain + `planner::prepare` twice
with the same `(request, owner, chain)` — identical hints; then recompute the hints in
a standalone script from **only** (request JSON, tip state_root, owner address, the
gossip-visible reserved set, state emptiness) and match. Determinism is already
readable in source; the probe also sizes how many attacker txs per round (the
`slots(count)` at `planner.rs:145-151`). ~1-2 h, no devnet.

Followup lane: live two-node run — seller-listed NFT, precomputed squats, observe
either the purgatory loop or admission rejection, and after a confirming squat the C28
lockup on the victim wallet.

### Expected demo cost / confidence

Medium (one devnet or local node pair; attacker txs are ordinary spends with
precomputed proofs). Mechanism confidence 0.85 (determinism verbatim in source);
end-to-end blocking 0.55 (wallet/GUI auto-recovery behavior is the open variable; the
C28 composition raises impact where it applies). Boundary: this is admission-policy
griefing plus wallet-recovery behavior, not a consensus violation.

---

## N2 (MINOR, composition worth registering): epoch-anchor eviction timing silently kills pending sends

`on_new_block` evicts every pool entry whose `epoch_anchor` no longer matches the
view's `user_epoch_anchor_id` (`noid_mempool/src/pool.rs:775-814`), and that anchor
changes every `EPOCH_LENGTH = 6` blocks (`noid_mempool/src/view.rs:112-126`). So
every admitted tx has a bounded pool lifetime (admission epoch + at most one more) —
if it is not mined within ~2-12 blocks it is evicted `EpochAnchorChanged`. Composed
with the registered fee-floor latch (C29) or simple miner inactivity, a victim's
low-fee tx dies after at most two epochs; what the *wallet* does on this eviction
(re-issue vs. stuck reservation) was never measured — C28 measured only the
coinbase-occupancy eviction. Discriminating probe: local pool at v2.1 params, admit
a low-fee tx at an epoch boundary minus one, advance blocks past the boundary, watch
the wallet's reservation and re-send behavior. Cheap (component-level). Why prior work
missed it: the register prices the fee-floor latch and the output-slot eviction but not
the epoch clock as the *third*, time-based eviction that needs no attacker action
beyond keeping the floor latched. Boundary: policy-level; a re-issuing wallet reduces
it to latency.

## Map refinement (not an attack): the inbound decoded-payload domain

`inbound_budget.rs` documents that accepted-block, HistoryStep, state-segment and
mempool codecs share one 64 MiB decoded-payload domain, permit acquired before
allocation and held until node-side consumption. This is distinct from U21's
pre-decode gossip window (also 64 MiB/10 s, `network.rs:486-493`). Exhausting the
decoded domain from one peer is bounded (single recovery lane, 8 in-flight segments,
object-fetcher outstanding caps), so I KILL it as a standalone attack — but future
resource lanes should name both budgets separately; U21's model conflates nothing
today, yet a "cross-budget" composition (gossip window + decoded domain + C18 header
parking) is the natural next resource experiment.

---

## Kills (checked; each with the reason it cannot happen)

- **zstd decompression bombs (state segments / headers):** state segments cap
  canonical length at 64 KiB (`MAX_STATE_SEGMENT_BATCH_BYTES`,
  `noid_p2p/src/protocol.rs:745`) before decompression, require zstd content size to
  match, window-log 22, one frame (`noid_p2p/src/state_segment_codec.rs:46,254,617-640`);
  header batches bound `MAX_UNCOMPRESSED_HEADER_BYTES`, window-log 21, single-frame
  validation before zstd (`noid_p2p/src/header_sync_codec.rs:33-44,181-195`).
- **Fork-origin fetch storm (48 MiB certificates):**
  `MAX_V2_FORK_ORIGIN_TRANSPORT_BYTES = 48 MiB`
  (`noid_chain/src/consensus/wire_limits.rs:67`) but recovery is single-flight,
  5 s-spaced, and the requested origin is pinned to the *local* terminal, not
  attacker-chosen (`noid_node/src/origin_recovery.rs:45-74,161-233`;
  `noid_p2p/src/fork_origin.rs:22` MAX_PENDING=4).
- **Inbound peer-store poisoning (eclipse persistence):** only successful *outbound*
  connections enter the persistent cache (500, evict-oldest;
  `noid_p2p/src/peer_store.rs:51-87`); inbound fan-in bounded per-IP (32) and per
  IPv4 /16 group (96, 32 reserved for fresh groups;
  `noid_p2p/src/peer_diversity.rs:22-34`); topology keeps >= 4 self-selected
  outbound neighbors (`network.rs:499-505`). Residual Sybil-dial entries are the
  register's already-open "concentrated peer sets" question.
- **Object-fetcher provider diversion:** 3-strike transport-failure exhaustion,
  hedged in-flight peers, per-plan `unavailable_sources`, Busy backoff
  (`noid_node/src/networking/object_fetcher.rs:37-86`); the register's new-avenues
  item already traces Busy retention.
- **VerifierPool class starvation:** `VerifierPool`
  (`noid_node/src/networking/verifier_pool.rs`) is referenced nowhere in production
  at the pin outside its own tests — the class-starvation game has no live target.
- **Gossipsub peer-score weaponization against honest peers:** penalties are
  self-inflicted (per-peer behavior); the only cross-peer term is IP colocation,
  which penalizes Sybil farms (`noid_p2p/src/behaviour.rs:247-273`).
- **`canonicalize_direct` PeerId grinding:** the duplicate-transport tiebreak is
  symmetric (`local.to_bytes() < peer.to_bytes()`, `network.rs:1057-1077`) so
  grinding changes only which *direction* survives, and inbound-transport topology
  credit is already discounted (`MAX_UNSELECTED_TOPOLOGY_CREDIT`).
- **Mempool-recovery scan amplification:** one global lane, <= 4 peers, 30 s
  spacing, finite pages (20), failure budget 7, no idle polling, no retry resets
  (`noid_p2p/src/mempool_recovery.rs:21-34,258-279`).
- **tradedata media origin (GUI phishing via forged manifests):** manifests are
  hash-pinned to the on-chain `metadata_hash` (`noid_gui/src/assets/media/transport.rs`,
  `fetch_manifest` hash check); the origin's residual power (availability, stale
  media) is already registered under operator-power (DEEPER-CONTROLS.md).
- **extminer protocol beyond C15:** the extminer binary is operator-local; its
  attack surface (template burn, lying node) is C15/C24 territory.
- **ASERT/MTP beyond the collapse:** owned by the live `chaos-asert` lane
  (constants, drift window, epoch anchors in its HANDOFF); no distinct new MTP game
  at scout depth.
- **Mempool replacement (RBF) games:** no replacement policy exists — admission is
  strictly first-seen on input and output slots; the eviction clocks (fee, anchor,
  context) are registered (C12/C29/C33/C35).
- **Inbound decoded-budget exhaustion:** bounded by the single recovery lane, 8
  in-flight segments, and object-fetcher outstanding caps (see map refinement).

---

## Ranking by (probability real x impact) / cost

1. **N1** — deterministic hint prediction + squat: 0.85 mechanism x targeted
   send-blocking (with C28 lockup composition) / cheap probe. The one candidate a
   followup PoC lane should take.
2. **N2** — epoch-anchor eviction timing: high-mechanism certainty, low novelty
   (composition of registered pieces), very cheap probe; register as a follow-up
   measurement on the existing verification-wallet/fee-floor lanes.
3. Everything else above: killed with evidence.
