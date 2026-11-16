# Shared early-finance build

ONE canonical early-finance node + CLI for every financial-op lane
(halt-stalefee, halt-binder, chaos-latch). Built once by the campaign root on
2026-11-15 after two lanes started duplicate ~40-min laptop builds of the same
patch.

- **Patch:** `noid_chain/src/consensus/params.rs` —
  `MAINNET_V2_ACTIVATION_HEIGHT` 210_537 → **10**,
  `MAINNET_V2_1_ACTIVATION_HEIGHT` 279_370 → **Some(24)**. Owner-authorization
  V4 follows V2.1 (params.rs:34). Nothing else changed; the tree is otherwise
  the pinned commit `52938433`. Source: chaos-latch's patched tree (its patch
  became this one).
- **Why not the cargo feature:** `isolated-v2-1-fork-testnet` chains to
  `isolated-v1-1-testnet`, whose startup guard (noid_node/src/main.rs:1735)
  requires a Linux loopback-only netns — unusable on the Mac. A plain
  constant patch keeps the mainnet P2P profile and runs everywhere.
- **Build recipe:** `build.sh` — the exact pack/bank/retirement-key env set
  recorded in devnet-chaos's poison-build fingerprint. Any release node build
  needs it: noid_node/build.rs:78 asserts the three retirement vars whenever a
  V2 schedule exists (i.e. always, even unpatched — that assert is what
  killed both lane builds).
- **Binaries:** `bin/parano1d`, `bin/parano1d-cli` (macOS arm64 release). All
  nodes in a financial lane's devnet must run this same build. The external
  PoW worker stays stock (`../devnet-chaos/bin/parano1d-miner`).
- **Warm target:** `target/` was seeded from halt-binder's partially built
  `target-early` (947 MB of registry deps) — the warm-dir reuse that made the
  rebuild cheap.

Lanes: treat `bin/` as read-only shared infrastructure; do not rebuild here
without the root's word. The heavy lanes (halt-reorg, split-cd1) build their
own differently-patched nodes on hbox (see ~/swarm/MACHINES.md).
