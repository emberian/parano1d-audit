# SR-5: local secondary-index corruption at restart

This offline fixture tests the pinned Parano1d source archive at commit
`52938433a30b185ab007afaa15166a632cf2e82e`. It creates a real accepted
chain from genesis using production block templates, PoW, state transitions,
and MDBX commit/restart paths. The HistoryStep terminal cryptographic verifier
is stubbed; the index behavior does not depend on that proof. All databases
are temporary and private to the process. The corruption requires **direct
local write access to the node's MDBX directory**; no peer message, transaction,
or block is used to mutate the index.

Run from the repository root:

```sh
CARGO_BUILD_JOBS=2 \
  CARGO_TARGET_DIR="$PWD/experiments/comp-system-hunter/core-seams/target" \
  cargo run --release --offline \
    --manifest-path experiments/verification-storage-indexes/Cargo.toml
```

The fixture checks, in order:

1. A clean h1 restart retains the coinbase UTXO and its verified owner view.
2. Replacing that `owner_idx` value with one byte lets `open_or_create` restore
   h1, but owner lookup returns `Decode("invalid owner-index value length")`.
3. Deleting the row lets startup restore h1 and makes owner lookup return an
   empty set despite the live coinbase in durable segment state. Another
   coinbase block commits; the old UTXO remains omitted after h2 restart.
4. A clean twin accepts the same h3 block spending the h1 and h2 coinbases.
   The modified node rejects it twice at MDBX commit with
   `missing pre-block owner-index record`, leaving its tip at h2.
5. A different coinbase-only h3 is accepted by the modified node. This is a
   selective local block-admission failure, **not a demonstrated global chain
   halt**.

The malformed-row case fails closed on reads. The deleted-row case exposes a
different limit: `get_verified_utxos_by_owner` validates rows it encounters,
but cannot infer a missing row from a prefix scan. Startup validates the
primary segment/state root and compact summaries, without rebuilding or
cross-checking this secondary owner index. This fixture does not test native
payload indexes or an honest-peer route to corrupt local MDBX files.
