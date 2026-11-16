# N1 — deterministic slot-hint prediction → output-slot squatting

Lane `n1-squat` (devnet-chaos campaign). Pinned checkout `parano1d/` @ `52938433`
(read-only); live runs use the shared early-finance build
(`experiments/shared-early-finance/bin/`, V2@10 / V2.1@24, production m24 domain).
Ports 21200-21219, loopback only, data dirs under `experiments/n1-squat/run/`.

Finding under test (scout-novel-attacks N1, commit 9d64971): the native wallet
planner's hint seed is computed only from public data, so an attacker who knows a
victim's intended request can precompute the victim's exact output slots, pre-squat
them (fees only, proofs precomputable), and either keep the victim in
"plan changed" purgatory or evict their pending tx at confirmation.

## Static ground truth (all verified against the pinned tree)

The seed (noid_rpc/src/server/native/planner.rs:405-408):

```rust
let encoded = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
let seed = u64::from_le_bytes(blake3::hash(&encoded).as_bytes()[..8].try_into().unwrap())
    ^ u64::from_le_bytes(chain.tip_header().state_root[..8].try_into().unwrap())
    ^ u64::from_le_bytes(owner.0[..8].try_into().unwrap());
```

No time term, no per-call nonce, no secret. The seed feeds
`collect_empty_slot_hints` (planner.rs:146 -> noid_rpc/src/wallet_submit.rs:87-210):
a pure `splitmix64` walk over the tip's public state (segment live counts via
`getStateMap`, per-slot emptiness via `getSlot`, `alloc_counter` and `log_slots`
from the header). The same request JSON + same tip + same owner reproduces the
same hint tree bit-for-bit.

The contrast that makes this a bug-shaped asymmetry — the ordinary-send path
salts every attempt (noid_rpc/src/server.rs:1028-1031):

```rust
let call_nonce = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .subsec_nanos() as u64;
```

mixed into the hint seed at server.rs:1069-1074
(`unique_seed = tip_seed + (call_nonce + attempt) * 0x9e3779b97f4a7c15`), so a
retried ordinary send re-rolls its slots; a re-previewed native plan replays them.

Attacker-predictability of the request itself: the GUI fills a BuyNft from the
listing terms the seller created (noid_gui/src/assets.rs:1032-1036) —
`BuyNft { collection_id: hex(listing.collection), serial, maximum_ask_micronoid:
listing.ask }`, and the enum is internally tagged (`tag = "action"`, snake_case —
noid_rpc/src/native_types.rs:117-119), so the serialized JSON is fully determined
by the listing. The owner is the buyer's address (public on-chain once funded /
from any prior purchase). The plan's slot count for BuyNft is 2 + change
(planner.rs:783), slots assigned in order [seller payment, NFT item, change]
(planner.rs:787-817); the count-2 hint list is a prefix of the count-3 list
(same walk, earlier stop), so squatting the first three predicted slots covers
both cases.

Mempool first-seen on output slots (noid_mempool/src/pool.rs:96, 533-541,
1297, 1334-1356): a later tx touching an admitted output slot is rejected with
`SlotConflict`, regardless of type — an ordinary paged spend can squat a native
plan's slot. Non-Trade native submits re-prepare and compare `plan_id` (blake3
over parent + key_index + owner + epoch_anchor + request JSON + full intent
bytes — planner.rs:1052-1060); any slot change flips it and the victim sees
"native plan changed; preview and review the operation again"
(noid_rpc/src/server/native.rs:214-234). The planner's reserved set is the
mempool's admitted input + output slots (native.rs:126-190, pool.rs:992-1006),
so an in-mempool squat changes the victim's next plan deterministically.

C28 composition: a block occupying a pending tx's output slot evicts it with
`TxEvicted(OutputSlotOccupied)` (pool.rs:816-846 — the check is generic over
block txs, not just coinbase). The native wallet reservation
(`PendingAdmissionGuard::reserve_native`, native.rs:276-301) then persists
until `reconcile_pending` runs (native.rs:20-46 — every
`walletPreviewNative`/`walletNativePending` call rolls back txs no longer in the
mempool). The ordinary-send reservation has no such reconcile and persists until
wallet reload — that is the original C28 demonstrated in
experiments/verification-wallet/.

## Stage 1 — determinism gate (live)

Setup: n1 (genesis + mining, 21200/21201), n3 (relay, 21204/21205), n2 (victim
daemon wallet, 21202/21203, peered via n3 only so the victim's tx gossip needs
two hops — used later for the C28 race). External PoW worker W1 on n1 (8
threads). Miner paused ("tip frozen") for every determinism read.

(running — results below)

## Stage 2 — live squat

(pending Stage 1 gate)
