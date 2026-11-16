# Valid native holdings: recurring wallet and GUI costs

Target: `parano1d/` at `52938433a30b185ab007afaa15166a632cf2e82e`.
This lane reads the pinned source and reuses measurements from
`creation-spam` and `economic-state`. It has not used the public network.

## Finding: a one-time mint forces repeated full wallet work

The existing local block proof minted 248 `u64::MAX`-locked NFT items to a
non-signing victim for 800,200 µNOID (0.8002 NOID), of which 620,000 µNOID
was burned. The victim cannot burn, transfer, or refuse those items at any
practical height. This report examines the *subsequent* cost to the victim.

The local node's `native_wallet_page` returns at most 256 holdings per call,
and the GUI requests 64. Each page reads the current tip, walks the native
owner index, fetches every opening, verifies its owner and carrier against
the same MDBX snapshot, then serializes the opening as hex in RPC JSON
(`noid_chain/src/storage/mdbx_store/native_wallet.rs:68-205`,
`noid_rpc/src/server/native.rs:474-549`,
`noid_gui/src/backend.rs:248-274`). A native NFT opening has 140 bytes
(`noid_tx/src/native_assets.rs:109-127,872-883`), so `opening_hex` alone is
280 characters per item. This excludes the root, metadata around the field,
other RPC fields, framing, and GUI allocations. At the measured low-occupancy
fee, one NOID spent on this attack buys about 310 items, imposing **at least
86.8 KB of opening hex on every complete victim wallet refresh**. The node
and GUI also repeat per-item lookup and validation. The attacker pays once;
the recipient pays this cost whenever their wallet reloads.

The [local timing control](reproduce_page_timing.py) repeated 100 full scans
of the existing 248-item, `u64::MAX`-locked fixture after it was warm in
MDBX. Across 400 verified 64-item `native_wallet_page` calls, the median was
**2,546 µs** and the 95th percentile was **2,693 µs**. The four-page full
scan took 10,056 µs median and 10,302 µs at the 95th percentile. An empty
victim-independent address took 16 µs median and 95th percentile across 100
calls. The output is in [page-timing-results.txt](page-timing-results.txt).
These are direct production storage-query timings on one small warm database;
they exclude RPC, network transfer, JSON encoding/decoding, and GUI work.

The GUI polls its snapshot every second (`app.rs:2878,3092-3103,1419-1433`)
and gives its native-holdings cache a key containing height, tip hash, and
state root (`app.rs:1441-1474`, `assets/live.rs:13-25`). A changed key calls
`reload`, discarding progress through an incomplete refresh. When an old
complete snapshot exists, the GUI keeps that old view while building an
entire replacement in a second `State`; it publishes the replacement only
after the final page (`assets/live.rs:150-205`). The node rejects a
continuation pinned to an older tip, returning RPC error `-32041`
(`native_wallet.rs:80-89`, `noid_rpc/src/server/native.rs:495-507`). The GUI
does not resume a stale cursor after a tip change. Thus a sufficiently large
victim inventory can make refreshes fail to finish before the next block;
this is a **source-derived possibility, not a reproduced outage**.

The NFT gallery has a separate cost: `nft_cards` walks every owned NFT,
constructs cards or collection groups, then sorts the resulting vector
(`assets/nft_gallery.rs:88-198`). The NFT view calls it before showing the
12-card page (`view/assets/nft_gallery.rs:328-347,145-170`), and media
maintenance calls it again on refresh ticks (`app.rs:3380-3428`). The visible
page size does not bound that work. How many junk holdings make a stock GUI
slow remains unmeasured.

## Conditional external-request amplification

Each minted NFT can carry its own arbitrary 32-byte metadata digest
(`noid_tx/src/native_assets/transition.rs:411-447`,
`noid_tx/src/native_assets.rs:593-602`). If a
victim opens the NFT section with media downloads enabled, the GUI collects
up to 8,192 missing metadata names and asks its media client for them
(`app.rs:3440-3475`). The client attempts at most four missing manifests per
pass, remembers failures for only 60 seconds, and tries again afterward
(`backend/native_media.rs:220-277`). Refresh ticks occur once per second.
Consequently, 248 **distinct nonexistent** metadata digests could keep a
responsive media origin receiving roughly four GETs per second from one
victim GUI while that section stays open. The default origin is
`https://tradedata.parano1d.org/` (`assets/media/transport.rs:10-14`), and
each request has a 30-second timeout (`:43-49`). The actual rate is lower
when requests are slow; it is bounded by four per pass and the serial client
loop. The existing 248-item control used a repeated metadata digest, so it
does **not** demonstrate this composition. Media downloads default to off
(`backend.rs:130-136`). No request was sent to that service in this audit.

## Threshold model, not an outage claim

With 64 serial items/page and target block interval 30 s, a full scan needs
at least `ceil(N/64)` pages. If the average *end-to-end* page latency is `t`
ms, its ideal uninterrupted duration is at least `ceil(N/64) × t`. The table
uses `N = 64 × ceil(30,000/t) + 1`, which ensures one more **nonempty** page
than fits in 30 seconds at the assumed latency. It ignores snapshot RPC
time, GUI rendering, native media, and variable block intervals. It is a
necessary workload calculation, not a sufficient attack proof. The prior
1.8-3.2 ms measurement was for **248 owner roots in one index scan**, not
verified 64-item wallet pages or RPC, so it cannot be used as `t`.

| Assumed end-to-end page latency | Items for >30 s | Minimum mint fee at 0.8002 NOID/248-item batch | Blocks at 496 items/block | Saturated chain time |
|---:|---:|---:|---:|---:|
| 1 ms | 1,920,001 | 6,195.15 NOID | 3,871 | 32.3 h |
| 5 ms | 384,001 | 1,239.51 NOID | 775 | 6.46 h |
| 10 ms | 192,001 | 620.16 NOID | 388 | 3.23 h |
| 25 ms | 76,801 | 248.06 NOID | 155 | 1.29 h |
| 100 ms | 19,201 | 62.42 NOID | 39 | 0.33 h |

The table is a sensitivity analysis, not measured latency. It prices only
mint transactions, rounds up full 248-item batches, and assumes the low
occupancy fee schedule. It omits the one-time 0.0058-NOID collection fee,
variable fee multipliers, competition for block space, and proof-production
cost. At v2's 30-second target, ordinary timing variance can interrupt a
scan below the table's nominal threshold. An attacker need not control the victim's
node or be the miner, but miners must include the valid mint batches.
The measured storage-only median, 2.546 ms/page, would correspond to about
754,000 items in this simple 30-second arithmetic **if** that page latency
remained constant at scale and equaled the end-to-end RPC latency. Neither
assumption has been tested, so this is not a measured starvation threshold.

## Guardrails and alternate costs

- Pagination is bounded and owner-prefix indexed. This does **not** imply
  an unbounded query per page or a node-wide `getVerifiedOwnerPage` scan.
- The existing 248-item local control confirms four verified wallet pages,
  not GUI starvation. It represents only 0.13% of the 10-ms/page threshold.
- NFT gallery work is conditional on the user opening an NFT view. The native
  holdings refresh itself runs in the online, synced GUI irrespective of
  which assets page is visible.
- Optional external media fetches require the user's media-download setting;
  the default is off. The per-pass cap and request timeout bound the rate.
- No generic NFT payload inflation exists: metadata is a fixed 32-byte
  digest. The amplification is in *count*, the repeated verification, RPC
  encoding, and gallery traversal.

## Needed measurement

The attempted RPC measurement could not use this fixture: the isolated-v2.1
daemon refuses to start on this macOS host because its isolated-fork test
requires a Linux loopback namespace; the ordinary daemon rejects the
fixture's synthetic HistoryStep terminal when reopening MDBX. Both failures
occurred before an RPC holdings request. No RPC latency is reported. The
small fixture's direct storage timing does not substitute for an end-to-end
wallet or GUI measurement.

Time a valid-release-terminal fixture through RPC and the stock GUI. A
cursor-invalidation control would prove restart behavior but not starvation.
A full availability claim
needs a valid fixture near a measured threshold with advancing local tips
and a stock GUI observation. Until then, the defensible claim is perpetual
cost transfer from a paid, valid, non-consensual mint to the recipient's
wallet and every node that stores/syncs those items.
