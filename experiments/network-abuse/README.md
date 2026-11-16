# Header serving: live anonymous probe and resource boundary

Audited the pinned `52938433a30b185ab007afaa15166a632cf2e82e` tree. The
target checkout was read-only. This lane uses an isolated copy of a **stopped**
150-block devnet MDBX store and a node listening only on `127.0.0.1:19880`
(RPC `:19881`). It never contacted a mainnet or third-party peer.

## Result

An anonymous peer can issue repeated, overlapping 16-byte `NHQ5` header
requests without having any headers, a network-profile exchange, or RPC
credentials. The server answers from its canonical store. Six alternating
requests for the same height-1..4096 range all succeeded against a 150-block
node. The real response contained 150 headers; later requests were not
deduplicated at the server.

The `include_inventory` bit substantially changes the server work. On the
warm local fixture, a plain response was 17,749 bytes in 3 ms; an inventory
response was 18,368 bytes in 8-9 ms. One cold inventory request took 62 ms.
This is a **bounded local observation**, not a mainnet throughput estimate.
The source explains the difference: inventory serving reads retained block
bodies and decodes them for exact claims (`network.rs:7704-7756`,
`mdbx_store.rs:2564-2591`, `header_protocol.rs:78-121`). The cost applies to
the retained suffix, whose floor is set by
`RETAINED_BLOCK_SERVING_DEPTH` (not all 4096 historical bodies).

Eight non-reading streams did **not** block an honest header client on the
150-block fixture. The response is only ~18 KiB, below yamux's initial stream
window, so these writes finish before the client reads. The honest control
returned in 3 ms. This directly refutes any claim that a small-chain header
request alone creates an eclipse. A 16-stream one-identity burst caused the
connection to close after 12 issued requests; the configured limit is eight
concurrent `chain_sync` streams **per connection** (`behaviour.rs:311-331`),
with at most two established connections per PeerID (`behaviour.rs:68`).
The chain-sync request timeout is 30 seconds (`behaviour.rs:328`), so a
persistently stalled stream is eventually torn down. Public inbound admission
caps a single IP at 32 identities and one IPv4 /16 group at 96 connections,
with eight additional reserved slots (`peer_diversity.rs:22-32`); the swarm
caps established inbound connections at 128 (`behaviour.rs:539`). Our five
identities used loopback, which the score configuration whitelists. We did
not test a distributed Sybil set.

## Large wire response: live serving test with invalid synthetic rows

`HeaderSyncCodec` permits at most 4096 fixed 288-byte records, or 1,179,648
canonical bytes per response (`header_sync_codec.rs:37-40`). It holds the
canonical vector and compressed vector through the final wire write
(`header_sync_codec.rs:288-336`). Header responses bypass the shared 64 MiB
outbound budget by design (`network.rs:93-95`); their eight-slot preparation
semaphore is released when the response reaches the event loop, before the
stream finishes writing (`network.rs:7659-7795`). A large, poorly compressed
response can stall on a non-reading peer after the yamux window fills,
retaining memory outside that budget. The prior `p2p-resources` lane drove
this condition with the real codec and synthetic stopped readers.

To cross-check through the real node and yamux, `extend_header_rows` inserted
3946 **syntactically decodable but consensus-invalid** rows at heights
151..4096 into this lane's stopped MDBX copy. It did **not** update the
canonical tip, which remained height 150. The node restarted and served a
4096-record `NHB5` response of **409,436 bytes** for the same 16-byte request.
The response is above the default 256 KiB yamux stream window, so stopping
reads can stall a real wire write. This is a **serving-path fixture only**:
it says nothing about accepting an invalid chain or the compressibility of a
real 4096-block mainnet range.

After a fresh restart, node RSS was 330,656 KiB. With 40 requests parked by
five anonymous identities (eight streams each), sampled RSS reached 422,240
KiB, an increase of about 89 MiB; a second bounded run reached 449,072 KiB.
RSS includes allocator retention and process effects, so these differences
are **not** a byte-exact retained-buffer measurement. After release it fell
to 355,664 KiB. The reactor health log continued to show
`outbound_bytes_in_use=0`: header memory was outside the data-serving byte
budget. All five clients issued their eight requests and completed a
12-second park; the server logged eight preparation `Busy` replies in the
first burst, so at most 32 of that burst's 40 requests reached a full
response. Do not equate requests sent with simultaneously stalled writes.

The new, narrower candidate is cheap repeated **inventory preparation**: a
16-byte request invokes bounded MDBX reads, body decoding for the retained
suffix, object digest/cache work, and zstd compression. There is no
per-peer preparation limit in the header request handler; only eight global
preparation slots. On the 4096-row wire fixture, three baseline honest reads
were 86, 100, and 101 ms. An honest read during the first 40-request park
was 87 ms; a second trial deliberately overlapped honest reads with the
initial preparation burst and saw 157, 182, 203, and 167 ms. Every honest
response was a full 4096-record `Ready`, not `Busy`; after the parked clients
released, an honest read completed in 84 ms. Thus the observed impact at
this scale is transient memory pressure and roughly doubled latency during
the concurrent preparation burst, **not a persistent header eclipse**.
The test lasted 12 seconds per park, below the 30-second chain-sync timeout.
Sustained rotation, out-of-memory failure, or mainnet-scale impact were not
tested.
The eight preparation slots can return `Busy` during a burst, but parked
wire writes do not continue holding those slots. A valid network-profile
handshake is not required by the header-serving handler; the anonymous
probe succeeded using only Noise, yamux, Identify, and Ping.

## Reproduction

The source-only client is `src/main.rs`, reusing the previously tested
transport in `../devnet-chaos/src/aggressor_common.rs` without modifying it.
Build with `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/target" cargo build
--release`. The probe rejects non-loopback destinations.

The run used a copy of `../devnet-chaos/run/n7s-a/` made while that node was
stopped. For the 4096-row serving test, run
`./target/release/extend_header_rows "$PWD/run/victim/mdbx.dat"` **while the
copy is stopped**. This deliberately makes it unsuitable for consensus tests.
Start the pinned release binary with `--mode node --data-dir
"$PWD/run/victim" --p2p-listen 127.0.0.1:19880 --rpc-listen
127.0.0.1:19881 --disable-dns-seeds --log debug`, then:

```
./target/release/noid-header-probe 127.0.0.1:19880 read 0 1
./target/release/noid-header-probe 127.0.0.1:19880 read 1 1
./target/release/noid-header-probe 127.0.0.1:19880 park 1 8
./target/release/noid-header-probe 127.0.0.1:19880 park 0 8
```

During the 12-second park, repeat the first command from another shell to
measure an honest header client. The node exited cleanly by Ctrl-C; no
listener remains. The copied data, binaries and logs live only under ignored
`run/`, `target/`, and `logs/`.

## Relation to the larger audit

The earlier C17 mempool pin delays body/segment/object serving. It did not
prevent honest headers from reaching the victim's HeaderDAG (see
`../TAKEOVER.md`). Header serving is a different resource surface. Our
150-block live control shows it survives eight parked small responses. The
4096-row serving test demonstrates anonymous amplification and resource
growth on a **synthetic invalid database**, while 40 coordinated requests
did not stop honest header reads. No impact on established-chain consensus,
selective selling, or snapshot fork choice follows from this run. A
consensus-valid long-chain reproduction would be needed to price the risk
for ordinary mainnet peers.
