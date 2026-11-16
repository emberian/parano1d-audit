# Sustained full-rate creation spam: permanent state weight per fee paid

Lane: `spam-weight` (devnet-chaos campaign). Question, from ember: didn't we
find spam/storage-DoS issues — trading very mild amounts of PoW for permanent
network weight? Build the PoC the earlier agents never got around to: a
sustained full-rate creation-spam run with REAL numbers for how much permanent
state weight a modest miner buys per day.

Status: RUNNING — this report is written incrementally; every measured number
below comes from the live run recorded in `run/blocks.csv` unless labeled
otherwise.

## Prior art this run replaces with live numbers

- `creation-spam` (C25 family): modeled the ~496-items/block ceiling from
  block caps (510 live outputs, 128 pages per 248-item intent) and computed
  ~71 MB/day sparse state, ~700 MB/day node-local records at 4,609 NOID/day.
  Never run on a live chain; no node, no P2P, no sync.
- `pow-resource`: a producer recaptures ordinary creation fees through its
  coinbase (fees minus the 2,500 µNOID/slot state-growth burn); ~102
  collections/block split-funded ceiling; arithmetic, not an accepted block.
- `economic-state` (C25's 248-NFT block): one isolated v2.1 block minted 248
  max-lock NFTs for 0.8002 NOID — a single block, template-sealed locally.
- `valid-spam` / FINDINGS.md U29: the ~71 MB/day / ~700 MB/day figures are
  MODELS; this lane runs them.

## Conditions

- Binaries: `experiments/shared-early-finance/bin/parano1d` (+`-cli`), the
  early-finance shared build (V2 @ h10, V2.1 @ h24, production m24 domain;
  schedule-assert relaxation per chaos-latch's patch). External PoW worker:
  `experiments/devnet-chaos/bin/parano1d-miner` (6 threads).
- Producer n1: `--mode extminer --genesis --v2-large-blocks`, ports
  21140/21141, mining key holder (coinbase lands in its daemon wallet).
  `--v2-large-blocks` is the documented producer opt-in that permits
  large-class (full-page) v2 templates; the default produces the small v2
  class. Verification (n2, joiners) always supports both classes.
- n2: seeded full node (validation cost of every spam block, no production).
- jheld: joiner held at genesis (no seeds) for the timed cold sync at the
  end; jlive: fresh node synced just before the spam window (its pre-spam
  sync time doubles as the pre-spam cold-sync baseline; its tip lag during
  spam is the tracking measurement).
- Spammer wallet: fixed offline secret (`spammer`, factory scheme), funded
  with 3 x 200 NOID UTXOs from n1's daemon wallet.
- Factory: `factory/` (`spam-factory`), forked from chaos-latch's factory —
  real authorized intents over live RPC. Lane changes:
  `prove_native_asset_authorization` for CreateCollection/MintNft (the
  node's own wallet uses the asset prover for base ops; the inherited
  financial prover would have been rejected), `maximum_items = u64::MAX`,
  `unlock_height = u64::MAX` on minted items, per-mint successor-slot
  bookkeeping, explicit funding-slot selection.
- Spam shape: every block carries up to two 248-item MintNft intents (the
  128-page per-intent cap: header + 2 input pages + 125 output pages = 250
  outputs = successor collection + 248 items + change), items owned by the
  spammer, `unlock_height = u64::MAX` (permanently unspendable — the
  strongest form of permanent weight; the unsolicited-victim variant was
  already demonstrated by economic-state). Two collections (spamA, spamB)
  ping-pong: a block's two mints spend the two collections' prior states.
- Slot assignment: sequential empty-slot scan from a persistent cursor —
  spam concentrates in few segments (relevant to the 64 MiB exact-segment
  cache and the 256-segment block cap; see Limits).

## Mechanism (why each block is full-rate)

The per-block ceiling is the producer's template budget. On this build the
legacy B25 page gate (v1.1 activation at h95,125) applies only to pre-v2
selection; v2+ selection uses the pack's class budgets. The default is the
small v2 class; `--v2-large-blocks` opts into the large class. The live
budget is read from `getNativeProtocol` (pages/headers/actions,
nft_batch_limit) at start, and the measured `user_pages` of every spam block
is recorded. Two 248-item mints need 256 pages > 255 positions, so the
expected fit is one full 248-item mint plus a second trimmed by pages; the
REAL packing is whatever `user_pages`/`live_outputs` show per block.

Fees (µNOID, multiplier 1 at our occupancy): mint = 5,000 + 200 + 700*250
+ 2,500*248 = 800,200 per 248-item intent, of which 620,000 is burned; the
producer's coinbase recaptures the remaining 180,200. Collection creation =
9,000 (1-in/2-out), 2,500 burned.

## Measured results

TO FILL: per-block table (mean over spam window), fees/recapture, state
growth, node-local growth, proving time, joiner sync delta, inventory
queries.

## Extrapolation

TO FILL: NOID per permanent GB at devnet cadence; per-day weight for a
modest miner; mainnet labeling.

## Limits

TO FILL.
