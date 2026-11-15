# agent-6: Exploit transcript algebra

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Explored the pinned tree at `52938433a30b185ab007afaa15166a632cf2e82e`. All experiment files are confined to `experiments/transcript-chains`; no live targets or shared-source edits.

## Main result: classical FRI transcript collision and proof malleability

The sponge’s partial-block padding is itself a valid explicit second lane:

```text
PAD = 0x01000000000000000000000000000080
absorb(x) + padded flush == absorb(x, PAD) + aligned squeeze
```

This follows from `parano1d/noid_poseidon2b/src/native/compression.rs:80-104` and `:190-208`. It was confirmed against all three unframed channel APIs in `experiments/transcript-chains/src/main.rs:24-118`:

- `Poseidon2bChannel`: `absorb(x)` and `absorb(x, PAD)` produced four identical challenges.
- `Poseidon2bWideChannel`: `absorb_base(x)` and `absorb_wide((x, PAD))` produced the same wide challenge.
- `noid_fri::Channel`: three lanes and four lanes ending in `PAD` produced the same 64 release-mode queries.

This became an end-to-end valid-proof pair in `noid_fri`:

1. Generate and verify an honest four-lane-final-codeword proof.
2. Extend its final codeword to five lanes, preserving the first four.
3. Create a six-lane twin by appending `PAD`.
4. Regenerate openings for their common query set.
5. Both proofs pass the real `noid_fri::verifier::verify`.

The release run used five folding rounds, a 128-position query domain, and 64 random queries:

```text
honest four-lane proof verifies; NUM_QUERIES=64
five-lane verifier: Ok; six-lane verifier: Ok
identical query set (64 draws, 53 distinct)
```

PoC: `experiments/transcript-chains/src/main.rs:190-310`  
Output: `experiments/transcript-chains/output-release.txt:12-17`

The source-level chain is:

- Honest prover documents four final symbols: `parano1d/noid_fri/src/prover.rs:73-77`.
- Verifier checks only nonempty, not exact length: `parano1d/noid_fri/src/verifier.rs:114-121`.
- Final lookup uses `final_codeword[final_sym % final_codeword_len]`: `parano1d/noid_fri/src/verifier.rs:215-223`.

For an odd length `L >= 5`, lengths `L` and `L+1` with last lane `PAD` collide in the transcript. Every final index is in `0..4`, so the extra entries never affect evaluation checks.

**Preconditions and cost**

- Same statement and transcript prefix.
- Variable, attacker-controlled final-codeword length.
- Odd `L >= 5`; first four entries unchanged.
- Openings must match the common query set, available to a prover holding the witness/trees.
- No collision search or grinding. The PoC performs ordinary prover work, rebuilds five round trees, and regenerates the query openings.
- The two variants add 16 and 32 bytes respectively to the honest final codeword.

This proves **proof malleability**, not a false-statement proof or cross-statement replay. The collision occurs after statement and folding challenges, so crossing statements would require a separate earlier transcript collision.

Reachability is limited to the public legacy `noid_fri` prover/verifier API. I found no current Wallet authorization caller of that verifier. The newer Basefold verifier explicitly rejects nonexact final lengths at `parano1d/noid_ivc_core/src/pcs/basefold.rs:1311-1323`.

## Query/PoW manipulation

A real wide-channel Wallet-style nonce search selected a query set containing a duplicate:

```text
selected nonce=379782
candidates=379783
PoW-valid candidates=4
```

PoC: `experiments/transcript-chains/src/main.rs:327-385`  
Output: `experiments/transcript-chains/output-release.txt:18-24`

For 65 queries over 8192 positions:

- Probability of at least one duplicate: `0.224754298`.
- Expected distinct queries without selection: `64.746743`.
- Expected nonce candidates to select a duplicate-containing set:
  `2^16 / 0.224754298 = 291589.5`.

With the real wide-channel schedule, scanning costs:

```text
2 * candidates + 3 * PoW-valid-candidates permutations
```

The measured run therefore cost `759,578` permutations. This is ordinary query grinding, not a free bypass: the verifier accepts any threshold-valid nonce rather than requiring the prover’s smallest found nonce (`parano1d/noid_gkr/src/zk_authorization.rs:1058-1108`), and subsequent seeds determine queries at `:1366-1368`.

A separate framed-lane check accepted nonminimal nonces 73 and 117 at six PoW bits and produced different following challenges. No prover/verifier schedule mismatch was found.

## Negative results and exact blockers

- **Cross-channel replay:** `KSCHANNL`, `KSCH256`, and `FRICHANL` have distinct capacity IVs (`parano1d/noid_poseidon2b/src/native/domain.rs:84-91`). Identical lane inputs produced different outputs in the PoC.
- **Framed lane/byte channels:** operation, kind, and length headers prevent simply appending a raw `PAD` lane or interchanging scalar and one-element slice schedules. See `parano1d/noid_ivc_core/src/challenger.rs:296-340` and `:815-875`; negative controls are in `src/main.rs:74-108`.
- **Wallet capsule shapes:** commitment cap, upper evaluations, `h_evals`, and query-symbol counts are fixed before transcript replay at `parano1d/noid_fri_binius/src/capsule.rs:731-748`. There is no variable trailing codeword analogous to legacy FRI.
- **Bridge cancellation:** wide close requires alignment and returns all four state lanes (`parano1d/noid_poseidon2b/src/channel.rs:167-175`), blocking a half-block close or truncated-rate replay.
- **Wide challenge bias:** `raw_hi` and `raw_hi+1` map to the same challenge, as expected from the exact two-to-one trace map. Under uniform raw lanes this gives uniform coverage of `2^255` outputs and excludes the base subfield; it is not a bias or transcript collision (`parano1d/noid_core/src/tower/block256.rs:78-89`).

## Artifacts and verification

Created:

- `experiments/transcript-chains/Cargo.toml`
- `experiments/transcript-chains/Cargo.lock`
- `experiments/transcript-chains/src/main.rs`
- `experiments/transcript-chains/output-release.txt`

Reproduce with:

```bash
cd /Users/ember/dev/parano1d-break/experiments/transcript-chains
CARGO_TARGET_DIR="$PWD/target" cargo run --release -- all
```

Final verification passed:

- `cargo test --quiet` — build successful, 0 tests.
- `cargo run --release --quiet -- all` — all runtime assertions passed; output saved.
- `cargo fmt` applied.
- Generated 932 MB `target/` directory was removed afterward.

The shared repository HEAD remained pinned. Its final status showed concurrent modifications in five unrelated shared files (`noid_gui`, `noid_ivc_core`, `noid_node`, and `noid_tx`); none were made by this experiment.
