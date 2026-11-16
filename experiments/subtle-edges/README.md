# Consensus-boundary edge audit (subtle-edges)

Source: `/Users/ember/dev/parano1d-break/parano1d` at
`52938433a30b185ab007afaa15166a632cf2e82e` (the pin is re-checked by `build.rs`,
which also asserts the six activation/finality/epoch constants and the
difficulty short-circuit are byte-for-byte the audited source).

All created files are confined to this directory. The PoC drives the pinned
`noid_chain` consensus library directly: real `next_target`, `validate_header`,
`validate_block_consensus`, `median_time_past`, the real wire codecs, and two
**real PoW-mined blocks** across the v2.1 activation boundary. No live node,
peer, RPC server, or recursive prover was started. 142 assertions, all pass
(`results.txt`).

This lane hunts the off-by-ones, activation edges, and canonicalization corners
that survive a big-picture audit. **Every hunt area HELD.** The value is in the
*exact* check that holds each edge, plus two genuinely subtle (benign) corners
that no prior lane recorded. Negatives with named blockers are the deliverable.

---

## Summary verdict per hunt area

| Area | Verdict | The exact edge that holds it |
|------|---------|------------------------------|
| 1. Activation boundary (h=279,370) | **HELD** | v2.1 is absent from the ASERT `ForkSchedule`; every v2.1-gated dispatch (class count, owner-auth version, native-id namespace) flips exactly at the height and never reinterprets a *shared* object |
| 2. Height/time arithmetic | **HELD** | `saturating_*`/`checked_*` on every attacker-reachable subtraction; finality off-by-one is self-consistent (`is_final`@18 ⇔ `!reorg_allowed(19)`) |
| 3. Difficulty edges | **HELD** | `next_target` is total: `net>=46`→floor, `net<=-320`→`MIN_TARGET`, result always in `[MIN_TARGET, GENESIS_TARGET]`, never `0`, never panics |
| 4. Canonical encoding | **HELD** | fixed-width LE codecs reject trailing bytes, truncation, high bitmap bits, and nonzero dead slots; no two-encodings-one-statement object found |
| 5. Epoch/anchor corners | **HELD** | `tx_epoch_anchor_height_for_child` divides-first, so it is total on all `u64` (never overflows, even at `u64::MAX`) |

The subtlest structural observation (benign): **the v2.1 activation height was
chosen off the 144-block transaction-epoch grid, so the activation lands
mid-epoch (`279,370 % 144 == 10`), and the single epoch-anchor window
`279,361..=279,504` is the only place in chain history where one epoch anchor
binds transactions under *two different owner-authorization dialects* (V3 below,
V4 at/above the boundary).** The replay wall is the version-bound ZK proof prefix
(`construction_version` 3 vs 4), which this lane verified by source-read, not by
executing the recursive prover — see Assurance limits.

---

## Hunt area 1 — Activation boundary at height 279,370

**Verdict: HELD. No transaction, authorization, proof, or id valid under V2
rules is reinterpreted under V2.1 rules across the boundary.**

The known datum coming in was that both dialects share a P2P namespace (the
version-domains / C9 finding). This lane checks the **semantic** boundary.

### 1a. Difficulty arithmetic is untouched by v2.1 (the subtle structural fact)

`ForkSchedule` (`noid_chain/src/consensus/forks.rs:42-45`) carries **only**
`v1_1` and `v2`. v2.1 activation (`MAINNET_V2_1_ACTIVATION_HEIGHT = Some(279_370)`,
`params.rs:23`) is *not* a difficulty parameter. Consequently the ASERT target
function is **byte-identical** on both sides of 279,370 for every input profile:
with the same `(anchor, parent-age)` the pre-activation target at 279,369 and the
post-activation target at 279,370 are the same 32 bytes (`A1`). This is the
cleanest possible outcome — there is no difficulty discontinuity to exploit at
v2.1, because v2.1 does not touch PoW at all.

### 1b. Every v2.1-gated dispatch flips at exactly the height, with no retroaction

- **HistoryStep class count** goes 2→3 at the activation height
  (`history_step.rs:67-81`). Class id 2 is rejected at 279,369 and accepted at
  279,370; class id 3 is still rejected (`A2`). Crucially, the *shared* classes
  (id 0, id 1) remain valid on both sides — activation adds a class, it does not
  renumber or invalidate the old ones. The terminal wire-version byte (7) does
  **not** change at v2.1 (only the class range does), and a stale version byte
  on decode is rejected (`A2`).
- **Owner-authorization version** goes V3→V4 at the height
  (`params.rs:46-61`): `A3`.
- **Native creation-id namespace**: a native-tagged id (bit 62, no bit 63) is
  rejected under the pre-activation raw `creation_id <= alloc_counter`
  comparison and accepted post-activation within the sequence bound
  (`params.rs:401-415`): `A4`. Ordinary and coinbase ids have identical
  semantics on both sides — only the native branch is newly reachable.

### 1c. Two real PoW blocks accepted across the boundary (`A6`)

Built with the pinned `build_block_template`, sealed with real
`search_pow` nonces (383303 at h=279,369; 259,764 at h=279,370), and accepted by
the real `validate_block_consensus` with the real state transition applied. Both
also pass standalone `validate_pow` and the timeless historical-header path. The
on-time boundary target stays at the genesis floor exactly, as expected for the
30 s v2 interval.

**The subtle overlap (benign).** `279,370 = 144·1940 + 10`, so the activation
lands at transaction-epoch position 10. Children `279,361..=279,504` all bind the
same start-of-epoch anchor header at height 279,360 (`A5`, `E`). This window
straddles the boundary: a tx mined at 279,369 (V3 owner-auth) and a tx mined at
279,370 (V4 owner-auth) carry the **same** epoch anchor. The epoch anchor binds
only the start-of-epoch header id, not the dialect — so the anchor equality is
real. The wall against replaying one dialect's authorization as the other's is
*not* the anchor; it is that the owner-auth version is bound into the ZK
authorization proof's domain-separation prefix
(`noid_gkr/src/zk_authorization.rs:198`, `values[1] = version.construction_version()`,
= 3 for V3, 4 for V4). A V3 authorization proof and a V4 authorization proof
commit to different prefix constants and cannot cross-validate. This lane
**verified the binding by source-read; it did not execute the recursive prover**
(see Assurance limits).

---

## Hunt area 2 — Height/time arithmetic extremes

**Verdict: HELD. No integer overflow, underflow-on-attacker-data, or off-by-one
window found.**

- **`next_target` at `u64` extremes** (`B1`): eight profiles including
  `height=u64::MAX`, `anchor_ts=u64::MAX`, `ts=0`, all-MAX — every one returns a
  target in `[MIN_TARGET, GENESIS_TARGET]`, no panic. The function uses `i128`
  intermediates and `clamp(i64::MIN, i64::MAX)` before casting (`difficulty.rs:92-156`).
- **MTP boundaries** (`B3`): median of 11 picks the true middle; `ts == mtp`
  rejects, `ts == mtp+1` accepts; the even-count window uses the *upper* median
  (stricter, `timestamps.rs:63-71`); the genesis empty window has median 0 so a
  timestamp of 0 is rejected and 1 accepts; only the last 11 of >11 timestamps
  matter.
- **Future-drift saturation (SUBTLE, benign)** (`B3`): within `MAX_FUTURE_DRIFT`
  (120 s) of `u64::MAX`, `local_time.saturating_add(120)` pins to `u64::MAX`, so
  the drift window stops widening and `block_ts == u64::MAX` is *accepted* rather
  than rejected. This is not a wrap to a false rejection and it does not underflow
  — the only effect is that the window cannot extend past `u64::MAX` (a timestamp
  ~5.8·10¹¹ years out). Harmless. A non-saturating over-limit still rejects.
- **Finality off-by-one** (`B4`): `is_final(h, h+17)` false, `is_final(h, h+18)`
  true; `reorg_allowed(18)` true (undo exactly the 18 non-final descendants),
  `reorg_allowed(19)` false. The two predicates are consistent:
  `is_final(h, tip) == !reorg_allowed(tip-h+1)` at every boundary height.
- **Expansion-window genesis edge** (`B5`): `finalized_expansion_window(34)` is
  `None` and `(35)` is exactly `(0,17)`; no expansion is possible before the
  first complete window even with `u64::MAX` counts; a wrong-length window never
  expands; the even 9/18 tie never expands and strict 10/18 does
  (`slot_expansion.rs:18-62`).

---

## Hunt area 3 — Difficulty edges (generalizing C26)

**Verdict: HELD. `next_target` is a total function with no attacker-chosen
timestamp pair that rounds it to 0, panics, or overflows.**

C26 recorded that a far-future *parent* timestamp floors the descendant target
to `GENESIS_TARGET`. This lane generalizes: **across every extreme, the result is
bounded in `[MIN_TARGET, GENESIS_TARGET]` and is never 0** (`B1`, `B2`).

- **Parent-timestamp monotonicity** (`B2`): as the parent gets later, the target
  is monotone non-decreasing and caps at the genesis floor; from a genesis anchor
  the floor is reached immediately (an on-time genesis-anchor block is already at
  the floor).
- **Hard-anchor floor crossing** (`B2`): from a 2^200 anchor (38 bits below the
  2^238 genesis floor), 37 half-lives of lateness stays strictly below the floor
  and 38 hits it exactly — the analytic doubling boundary, confirming the frac=0
  factor is exactly 2^16 and the floor clamp is exact.
- **Far-future parent** (`B2`): `next_target(..., parent=u64::MAX, ...)` floors to
  `GENESIS_TARGET` (the C26 datum, now shown to be the generic short-circuit
  `net >= 46 → floor`, `difficulty.rs:191-197`). The result is never the zero
  target.
- **Extreme fast** (`B2`): a far-back anchor with `actual=0` drives
  `net <= -320 → MIN_TARGET` (`difficulty.rs:198-200`). `MIN_TARGET` is 1 — the
  hardening direction also clamps cleanly with no underflow.

`block_work(0)` defensively returns 0 (so an already-invalid zero target cannot
add chainwork, `difficulty.rs:491-497`), but `next_target` never *produces* 0, so
the two agree.

---

## Hunt area 4 — Canonical encoding

**Verdict: HELD. No object admits two distinct byte encodings for one statement
(the legacy-FRI malleability pattern does not recur in the consensus codecs).**

Every consensus-relevant codec is fixed-width little-endian with explicit
truncation/trailing-byte rejection and canonical-form validation on decode:

- **BlockHeader** (`noid_chain/src/wire.rs`): 212 bytes exact; extreme-valued
  header round-trips; trailing byte rejected; **every** truncation length
  rejected; distinct headers never share an encoding (`D1`).
- **TxBody** (`noid_tx/src/wire.rs`): 323 bytes exact; round-trips; a non-`{0,1}`
  `is_coinbase` byte rejected; a high bitmap bit (bit 10) rejected
  (`NonCanonicalBody` — the `validity_bitmap` is not malleable); a nonzero *dead*
  input slot rejected (the "two encodings, one tx" attempt fails); trailing byte
  rejected (`D2`). Decode calls `validate_canonical` so the parser and the
  semantic predicate agree on exactly one byte form per body.
- **HistoryStep terminal metadata** (`noid_chain/src/history_step.rs`): 42-byte
  prefix; version byte 7; an out-of-range class id is rejected on decode; a
  truncated prefix is rejected (`D4`).

**The subtle corner (benign)** (`D3`): the coinbase `creation_id` is
`COINBASE_CREATION_TAG | mint_height` with `COINBASE_CREATION_TAG = 1 << 63`
(`params.rs:369-394`). For `mint_height >= 2^63` the tag bit collides with the
height, so two different heights `h` and `h + 2^63` alias to the **same**
creation id, and `coinbase_creation_height` decodes the aliased id back to the
lower height. The boundary check then accepts that id at *both* heights. This is
a genuine two-heights-one-id corner — but height `2^63` is unreachable (it would
require ~2^63 blocks), so it is a latent-but-dead encoding ambiguity, not a live
collision. The source comment at `params.rs:370-376` asserts the disjointness
that this corner would violate only past the unreachable boundary.

---

## Hunt area 5 — Epoch / anchor corners

**Verdict: HELD. The transaction-epoch anchor machinery is total on all `u64`
and advances only on exact non-zero boundaries.**

- **Anchor resolution** (`E`): `tx_epoch_anchor_height_for_child` is
  `((child-1)/144)*144` for `child != 0` (`epoch_anchor.rs:62-68`). A boundary
  block `k·144` still consumes the *preceding* anchor; its own id becomes active
  only one block later. Verified at 0, 1, 143, 144, 145, 288, 289.
- **`u64::MAX` corner (SUBTLE, benign)** (`E`): the function **divides first**, so
  it is total even at `u64::MAX` and never overflows. Note that the *next*
  boundary above that anchor, `anchor + 144`, overflows `u64` — a naive consumer
  that computed "anchor + epoch length" at the very top of the height space would
  wrap, where the pinned function does not. (`checked_add` returns `None` there;
  the pinned code never does that addition.) The documented claim that "every
  `u64` height has a checked decomposition" holds at `u64::MAX`.
- **Accumulator transition** (`E`): `next_tx_epoch_anchor_id`
  (`epoch_anchor.rs:95-108`) advances to the accepted child id only when the
  child height is a non-zero boundary; child 0 (genesis, installed not accepted)
  never advances, and non-boundary children keep the old anchor.

---

## Negative and control results

- **Activation is not a difficulty event.** `ForkSchedule` has no v2.1 field, so
  ASERT targets are byte-identical across 279,370 — there is no difficulty jump
  to straddle (`A1`).
- **No retroactive invalidation.** Shared HistoryStep classes (0,1) stay valid
  after v2.1; shared owner-auth and ordinary/coinbase creation-ids are unchanged
  across the boundary (`A2`, `A4`).
- **No overflow/underflow/panic** anywhere in `next_target`, MTP, future-drift,
  finality, or expansion under `u64`/timestamp extremes (`B1`–`B5`).
- **No `next_target` input returns 0 or panics**; far-future→floor and
  extreme-fast→MIN_TARGET both clamp (`B2`).
- **No malleable consensus encoding**: trailing bytes, truncation, high bitmap
  bits, nonzero dead slots, non-bool discriminators all rejected (`D1`,`D2`,`D4`).
- **v2 ASERT boundary (section C)** is clean too: the anchor is derived from the
  tip and the rule from the child, and both flip at exactly the activation height
  (`header.rs:277-286`); the first v2 block inherits the parent target exactly
  (no activation jump); v2 siblings share one parent-derived target while pre-v2
  siblings each select their own; block time switches 20→30 s and the ASERT
  half-life 120→180 s at the height. The mainnet v2 height (210,537) is itself
  mid-epoch (`% 6 == 3`); a hypothetical epoch-aligned activation re-anchors
  identically (`C`).

---

## Blocker table (which exact check holds each edge)

| Edge | What would break it | The check that holds it | Structural / incidental |
|------|---------------------|-------------------------|-------------------------|
| v2.1 difficulty straddle | v2.1 changing ASERT | v2.1 absent from `ForkSchedule` (`forks.rs:42`) | **Structural** |
| v2.1 class/auth/id straddle | shared object reinterpreted | per-height dispatch adds, never renumbers (`history_step.rs:72`, `params.rs:52`, `:408`) | **Structural** |
| Cross-dialect auth replay in the straddling epoch window | same auth bytes valid both sides | owner-auth version bound into ZK prefix (`zk_authorization.rs:198`) | **Structural** (source-read; not executed) |
| `next_target` overflow/panic | large timestamp diffs | `i128` math + `clamp(i64::MIN,i64::MAX)` (`difficulty.rs:113-114,144-154`) | **Structural** |
| `next_target` → 0 | timestamp pair rounding to 0 | `net<=-320 → MIN_TARGET`(=1) short-circuit (`difficulty.rs:198`) | **Structural** |
| Difficulty floor break | far-future parent easing below genesis | production floor clamp `le256_lt(GENESIS, clamped)` (`difficulty.rs:215-217`) | **Structural** |
| Future-drift wrap near `u64::MAX` | `local+120` overflow | `saturating_add` (`timestamps.rs:54`) — accepts at ceiling, no wrap | **Incidental** (dead region) |
| Finality off-by-one | reorg across the finalized block | `reorg_allowed(n) = n <= 18` vs `is_final(h,tip)=tip>=h+18` — consistent | **Structural** |
| Expansion before first window | fabricated early counts forcing a resize | `finalized_expansion_window` returns `None` below h=35 (`slot_expansion.rs:18-22,39`) | **Structural** |
| Epoch anchor overflow at `u64::MAX` | `anchor+epoch` wrap | function divides-first (`epoch_anchor.rs:66`) | **Structural** |
| Coinbase id aliasing ≥2^63 | two heights → one id | disjointness holds strictly below 2^63; aliasing region unreachable | **Incidental** (dead region) |
| Tx-body malleability | nonzero dead slot / high bitmap bit | `validate_canonical` on decode (`wire.rs:160`), `BitmapHighBits`/`DeadInputNotZero` (`public_logic.rs:69-83`) | **Structural** |

---

## PoC implementation and reproduction

`build.rs` re-checks the pin and asserts six consensus constants
(`MAINNET_V2_1_ACTIVATION_HEIGHT=279_370`, `MAINNET_V2_ACTIVATION_HEIGHT=210_537`,
`CONSENSUS_FINALITY_DEPTH=18`, `TX_EPOCH_BLOCKS=144`, `EPOCH_LENGTH=6`,
`MAX_FUTURE_DRIFT=120`) and the difficulty short-circuit are byte-for-byte the
audited source, so the compiled assertions cannot silently drift from the cited
`file:line` evidence. `src/main.rs` drives the pinned `noid_chain` library
directly across five sections (A–E).

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

Final line:

```text
ALL ASSERTIONS PASSED
```

(142 `PASS` assertions; full transcript in `results.txt`.)

---

## Assurance limits

- **The owner-auth cross-dialect replay wall is established by source-read, not
  execution.** This lane verified that `construction_version` (3 vs 4) is bound
  into the ZK authorization prefix (`noid_gkr/src/zk_authorization.rs:198`); it did
  **not** build a V3 proof and attempt to verify it under V4 (that needs the heavy
  `noid_gkr`/`noid_recursive` prover, out of scope for a boundary-arithmetic lane).
  The anchor-equality half of the straddle *is* executed (`A5`, `E`); the
  authorization-binding half is a read of the production circuit constants.
- **Blocks are validated through the sequential interpreter**
  (`validate_block_consensus`), not the live full-node fused-recursive path. The
  PoW, state transition, roots, and header rules are real; the recursive proof
  envelope is not exercised (the same scope boundary every prior arithmetic lane
  used).
- **Heights are simulated.** The two "boundary" blocks are built at heights
  279,369/279,370 over a fresh fixture chain, not replayed from mainnet history;
  the point is that the pinned *rule-selection* code treats those heights exactly
  as it will on mainnet, which the constant-pinning `build.rs` guarantees.
- **The coinbase-id aliasing and the `u64::MAX` epoch-anchor overflow are latent
  corners in unreachable regions of the height space.** They are recorded as the
  subtle encoding/overflow observations the brief asked for, explicitly classified
  as benign (dead region), not as live defects.
- No upstream source was edited and no fix was applied. Pre-existing unrelated
  working-tree modifications in the repository were left untouched.
