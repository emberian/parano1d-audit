# Circuit edge case verification

This harness rechecks three Gemini PA-02/03/04 claims against the source copy in
`../uc-composition/pinned/` (reported upstream commit
`52938433a30b185ab007afaa15166a632cf2e82e`). It uses only local synthetic
fixtures. Run from the repository root:

```sh
CARGO_BUILD_JOBS=2 cargo run --release --offline \
  --manifest-path experiments/verification-circuit/Cargo.toml
```

`run.log` preserves the exact output from the final binary run (exit `0`).
`build-and-run-final.log` preserves the final Cargo run and exit `0`.
`run-honest.log` (exit `0`) and `run-boolean-pin.log` (exit `0`) are controls.
`c0-reference-full-audit.log` is a fresh rerun of Gemini's seven-check binary
(exit `0`), including its C0/C1 transcript comparison. Earlier failed fixture
logs are retained to show how the standalone fixture was corrected.

The final Cargo command used
`CARGO_TARGET_DIR=experiments/comp-proof-algebra/circuit-audit/target` to reuse a
build cache; this is not part of the proof or test input.

## PA-02: standalone FeedForward Walk-B direction

The harness starts from the source test's honest 10+6 strided Merkle fixture,
then sets one active node's direction field element to `2`. It transforms the carried and
sibling digest values so both Poseidon input lanes remain *exactly* unchanged.
It pins the transformed entry (two field elements) and the unchanged root
(two field elements) as **four public IO fields** in the outer `FieldR1cs`.
The complete field proof with a `MerkleRegionSidecar` post-commit callback
returns `Ok` from `verify_field_with_public_io_and_post_commit`. Replacing the
public entry with the original entry rejects the same proof. The Boolean
control adds `D·(D+1)=0` to the *same outer witness* and finds the R1CS
unsatisfied. The honest `D=0` control verifies end to end.

The standalone outer R1CS pins the public entry and root but deliberately
omits a direction Booleanity constraint. It does **not** show a production
`HistoryStep` proof with an altered Merkle path. The production Link Walk-B
allocator pins direction column 8 via
`r_pcs_region.rs:1194-1196`; the selected Block wallet/meta allocators pin it
via `region_source_binding.rs:1649,1676`. A caller that fails to pin this column
can admit the relation's non-Boolean affine witnesses.

## PA-03: high-half PoW nonce

The harness searches a small local nonce range with `hi=1`, then checks one
winning candidate in `FsChannelTrace`'s *inline sponge* R1CS, including the
16-bit zero filter. A 64-bit decomposition rejects that same field element.
This demonstrates a real circuit-versus-native input-domain difference at the
PoW gadget; it does not show reduced grinding cost, a forged HistoryStep, or
native serialization of the 128-bit nonce. The native proof's nonce is `u64`.

## PA-04: C0 BaseFold zero row-batch

The existing Gemini harness in `../comp-proof-algebra/circuit-audit/` compares
two `post_row_batch_commit` roots for `log_batch_size=0`. C0 native and trace
sumcheck challenges stay equal because the root is not absorbed; C1 native and
trace challenges change because they absorb the root before round zero.
Relevant source is `noid_ivc_core/src/pcs/basefold.rs:1270-1284,1915-1917` and
`noid_recursive/src/acceptance/trace/self_verify.rs:2281-2297,2527-2530`.
The fixture reaches a later verifier error. It therefore demonstrates an
unbound transcript field, **not** an accepted bad PCS proof. The C0 prover also
uses `log_batch_size - 1` in the zero case and cannot create this proof without
repair. Production `HistoryStep` predecessor verification takes the C1 path,
with its pre-round-zero absorption.
