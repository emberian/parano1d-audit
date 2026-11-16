# C22: bounded live-node proof ingress control

Target: pinned `52938433a30b185ab007afaa15166a632cf2e82e` release node.
This test used a copy of the **stopped** funded `n1-pool` devnet data under
this lane's ignored `run/c22-victim/`, with DNS seeds disabled and listeners
only at `127.0.0.1:19890` (P2P) and `:19891` (RPC). No mainnet contact or
third-party data was involved. The node exited cleanly and those ports are
closed. The target checkout and the other lane's store were not edited.

## What reached the node

The existing `anon-push-storm` sends decode-failing authorization bytes; its
phase-7 measurements do **not** exercise the 8 ms AuthGKR rejection observed
in `tx-admission`. To bridge that gap, this lane:

1. Called the copied node's own `walletSend` twice, producing two honest
   intents that coexisted in its mempool. The wallet had 604 funded UTXOs;
   concurrent admission of the two intents confirms nonconflicting inputs.
2. Used anonymous `NMR3 Pull` over real Noise/yamux to save both canonical
   intent byte strings (90,446 B and 90,286 B). Restarting the node cleared
   its volatile mempool while preserving both live inputs. The intent files
   are under ignored `run/` and are synthetic wallet material, not published.
3. Replayed four one-byte-corrupted variants of the first intent through
   anonymous `NMR3 Push`, using four separate libp2p identities. Each peer
   sent ten requests at 200 ms intervals (40 requests over roughly two
   seconds; about 3.62 MB total payload). The byte flips were within the
   detached authorization, leaving the transaction body and claimed input
   unchanged. All four variants had been checked individually to reach
   `authorization verify: AuthProof`, rather than failing at wire decode.
4. Pushed the second honest intent during that burst, from a fifth peer.
   The node admitted it, and mempool size became one. Its log recorded
   **40** `P2P tx rejected ... authorization verify: AuthProof` and **41**
   `InboundVerify` entries (40 invalid plus the honest control). The client
   outputs recorded ten successful Push acknowledgments per attack peer.

The honest intent took **48.8 ms** from P2P client start to observed mempool
size one during the burst. The same intent, alone after another restart,
took **31.1 ms**. These are single measurements that include Noise handshake,
P2P scheduling, verification, and 10 ms resolution RPC polling; the 17.7 ms
difference is **not** a stable latency estimate. The attack did not deny the
honest admission. The node's Push acknowledgment is an 8-byte empty response
and arrives before a verdict; the log and mempool check, not that ack, are
the admission evidence.

The first attempted control used the *same* input for corruptions and the
honest intent. Once the honest intent entered the mempool, later corruptions
were rejected cheaply for slot conflict: only 19 of 40 reached `AuthProof`.
This was a test-design error; the two-input run above fixes it and is the
verdict. It also shows why a report must count verifier executions rather
than merely sent P2P frames.

## What the test says about availability

The real node, not just the `AsyncMempool` fixture, accepts anonymous Push
traffic all the way to the expensive authorization verifier when the payload
is shaped from an honest proof. Rejections leave the input available; the
honest second intent admitted normally. This confirms the **reachability**
leg of C22 and its immediate negative control. It does **not** demonstrate
saturation, a lost honest transaction, mining delay, or a network outage.
The node log exposes no per-rejection duration; the previous 8.34 ms cost is
from the separate `tx-admission` crate-level benchmark, not newly measured
per Push here.

The bounded run offered at most 4 peers × 5 Push/s = 20 Push/s, about
**1.81 MB/s** of 90.4 KB payloads. The previous local four-worker benchmark
drained about 415.5 invalid proofs/s. Matching that drain would require
at least 84 simultaneous rate-limited identities and about **38 MB/s** of
payload traffic (84 × 5 × 90,414 B/s). At the 128-inbound-connection ceiling
the nominal offer is 640/s and **57.9 MB/s**. The earlier report's phrase
"84 identities / 58 MB/s" conflated these two points. One public IP admits
at most 32 identities; a run at the modeled 84-identity point would need
multiple source IPs and might encounter other P2P or host bottlenecks.
The four-worker drain is one machine's crate-level measurement, so even
these thresholds are only hypotheses for a live node.

## Reproduction and limits

`src/bin/mempool_probe.rs` sends the actual fixed-frame Pull and Push over
the shared hand-driven Noise/yamux transport. It refuses non-loopback peers
and files outside this lane's ignored `run/`. Example command shapes:

```
./target/release/mempool_probe 127.0.0.1:19890 pull "$PWD/run/honest-intent.bin" 0
./target/release/mempool_probe 127.0.0.1:19890 push "$PWD/run/honest-intent.bin" 1
./target/release/mempool_probe 127.0.0.1:19890 push-repeat "$PWD/run/honest-intent.bin" 1 10 200
```

The exact run used four `push-repeat` processes with flip offsets 1..4 and
the separate honest intent saved as `run/mempool-1.bin`. Build used
`CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/target" cargo build --release`.
Raw node/client logs remain in ignored `logs/`. No sustained high-rate run
was performed; rate limiting, IP diversity, bandwidth, proof worker
scheduling, and honest peer escape at saturation remain to be tested before
claiming an outage. Run results are summarized in `C22-results.txt`.
