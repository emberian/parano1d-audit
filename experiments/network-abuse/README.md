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

## Source-derived large-chain risk, not yet a live finding

`HeaderSyncCodec` permits at most 4096 fixed 288-byte records, or 1,179,648
canonical bytes per response (`header_sync_codec.rs:37-40`). It holds the
canonical vector and compressed vector through the final wire write
(`header_sync_codec.rs:288-336`). Header responses bypass the shared 64 MiB
outbound budget by design (`network.rs:93-95`); their eight-slot preparation
semaphore is released when the response reaches the event loop, before the
stream finishes writing (`network.rs:7659-7795`). A large, poorly compressed
response *could* stall on a non-reading peer after the yamux window fills,
retaining memory outside that budget. The prior `p2p-resources` lane drove
this condition with the real codec and synthetic stopped readers, but no live
4096-header swarm run exists. Mainnet response compressibility and actual
retained memory remain unmeasured here.

The new, narrower candidate is cheap repeated **inventory preparation**: a
16-byte request invokes bounded MDBX reads, body decoding for the retained
suffix, object digest/cache work, and zstd compression. There is no
per-peer preparation limit in the header request handler; only eight global
preparation slots. The small-chain run establishes an asymmetry, but not
starvation of honest clients. Test a realistic 4096-header local chain and
measure CPU, RSS, `Busy` replies, and honest header response latency before
assigning severity. A valid network-profile handshake is not required by the
header-serving handler; the anonymous probe succeeded using only Noise,
yamux, Identify, and Ping.

## Reproduction

The source-only client is `src/main.rs`, reusing the previously tested
transport in `../devnet-chaos/src/aggressor_common.rs` without modifying it.
Build with `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/target" cargo build
--release`. The probe rejects non-loopback destinations.

The run used a copy of `../devnet-chaos/run/n7s-a/` made while that node was
stopped. Start the pinned release binary with `--mode node --data-dir
"$PWD/run/victim" --p2p-listen 127.0.0.1:19880 --rpc-listen
127.0.0.1:19881 --disable-dns-seeds --log debug`, then:

```
./target/release/noid-header-probe 127.0.0.1:19880 read 0 1
./target/release/noid-header-probe 127.0.0.1:19880 read 1 1
./target/release/noid-header-probe 127.0.0.1:19880 park 1 8
```

During the 12-second park, repeat the first command from another shell to
measure an honest header client. The node exited cleanly by Ctrl-C; no
listener remains. The copied data, binaries and logs live only under ignored
`run/`, `target/`, and `logs/`.

## Relation to the larger audit

The earlier C17 mempool pin delays body/segment/object serving. It did not
prevent honest headers from reaching the victim's HeaderDAG (see
`../TAKEOVER.md`). Header serving is a different resource surface. Our
150-block live control shows it survives eight parked small responses;
the 4096-header wire-stall and preparation-starvation scenarios still need
their own large-chain reproduction. No impact on established-chain
consensus, selective selling, or snapshot fork choice follows from this run.
