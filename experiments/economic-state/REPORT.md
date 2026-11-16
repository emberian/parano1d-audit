# Unsolicited, practically unspendable NFT holdings

Audited target: `parano1d/` at `52938433a30b185ab007afaa15166a632cf2e82e`.
All experiments used synthetic keys/state on the local machine. No public
network peer or exchange was contacted. This lane refines the existing
`creation-spam` finding about forced NFT inventory; it did not discover a
way to spend or lock another person's native NOID.

## Finding

An account can mint NFT items into an arbitrary other address and set each
item's `unlock_height` to `u64::MAX`. The recipient cannot transfer, burn,
or list those items until height `u64::MAX`, yet the node and GUI include them
in the recipient's native holdings. At 30 seconds per block, reaching that
height from 2026 would take roughly 18 trillion years. This is a cheap,
targeted, persistent wallet pollution attack. The prior report said cleanup
costs the victim a fee; for this locked variant the victim cannot clean up at
all under the current input rules.

This is a native-asset availability issue, not a freeze of the victim's raw
NOID UTXOs. A custom wallet can filter the items locally. That changes the
display, not the on-chain records or the stock GUI's holdings query. We have
not measured the inventory size at which the GUI becomes unusably slow.

## Mechanism and verification

- `noid_tx/src/native_assets/transition.rs:411-447` (`mint_nft`) checks
  collection identity, serial continuity, and collection successor, but
  neither the new owners nor their unlock heights. The input authority check
  at `transition.rs:165-176` covers the collection and funding inputs.
- `noid_tx/src/native_assets.rs:593-602` validates `NftItem` without an upper
  bound on `unlock_height`; `:625-635` exposes that field as its spend lock;
  `:998-1008` rejects input consumption while `height < unlock_height`.
  The output's opening root commits to the recipient and lock value.
- `noid_chain/src/state/native_assets.rs:115` invokes that transition check;
  the block path in the reproduced `creation-spam` control then accepted a
  mined, locally committed block carrying the modified items.
- `noid_chain/src/storage/mdbx_store/native_wallet.rs:68-205` pages verified
  holdings by recipient owner without filtering on unlock height. The RPC
  maps every returned opening into a `NativeHolding` with its unlock height
  (`noid_rpc/src/server/native.rs:488-531`). The GUI requests pages of 64 and
  accumulates NFT entries (`noid_gui/src/backend.rs:268`,
  `noid_gui/src/assets/live.rs:267-392`). Its NFT detail view displays the
  unlock height and the send/list/burn controls (`view/assets.rs:358-375`),
  but those operations cannot consume the locked input.

`timelocked-pollution.rs` is a small standalone direct production-opening
check compiled against existing pinned rlibs. It confirms a MAX-locked
output validates, its input binding validates, and input consumption returns
`Timelock` at the activation height and at `u64::MAX - 1`; only
`u64::MAX` passes. `reproduce_full_mint.py` derives an isolated variant of
the existing creation-spam production-code PoC without modifying that lane.
Its output is preserved in `full-mint-results.txt`:

```text
MAX-locked victim NFT minted into real ChainState; victim cannot spend at activation height
CONTROL 24 empty coinbase blocks accepted (miner = attacker); tip height 24
CONTROL collection block committed at height 25; collection slot=9699353
MEASURE mint block (248 victim-owned items): intent_construction=17484us template_build+materialize=119286us
CONTROL victim wallet: 248 MAX-locked NFTs across 4 verified 64-item pages; every input rejected Timelock
ALL PHASES COMPLETED
```

The 248-item mint transaction required **800,200 µNOID** (0.8002 NOID),
of which 620,000 µNOID was burned, at the test's low-occupancy fee. That is
about **3,227 µNOID per item** including shared transaction overhead. The
one-time collection creation required 5,800 µNOID. Under occupancy pressure
the fee schedule raises this cost; those multipliers are printed in the
results. The test's 248 locked items added 12,450 bytes to the sparse state
representation, and the victim's native owner index returned 248 roots in
1,863 µs on this machine. These are local measurements, not estimates of
mainnet latency.

## Limits and next experiments

The full-block test uses the repository's isolated-v2.1 schedule and local
genesis. It checks real native intent construction, block-template
materialization, PoW, MDBX commit, owner index, verified wallet pagination,
and the spend-timelock predicate. It does not run the GUI end to end or mine
at mainnet difficulty. The direct opening check uses prebuilt release rlibs;
the full-block control compiles from the pinned checkout and isolates its
temporary crate and database under this lane's ignored `tmp/` directory.

The strongest untested composition is a recipient with enough locked junk
to make the GUI's full holdings refresh span multiple tip changes. The GUI
loads 64 items per page and publishes a complete replacement only after
all pages arrive; the node rejects a continuation whose tip changed. That
suggests refresh starvation at sufficient item count, but no threshold or
end-to-end reproduction has been measured. A useful follow-up would time the
stock GUI against a larger local dataset while an honest miner advances the
tip. There is no need to infer a raw-NOID freeze from this result.
