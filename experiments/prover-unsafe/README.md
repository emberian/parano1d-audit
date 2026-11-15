# Unsafe / SIMD / loader-layer audit (noid_ivc_core, noid_ivc_prover, noid_poseidon2b)

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e` (verified with `git rev-parse HEAD`).

All created files are confined to this directory. Everything below is a local library-level PoC against the pinned crates' real production functions; no node, network peer, or RPC server was started, no live target was touched. This lane deliberately does not duplicate the prior capsule-Merkle, compressor-collision, FRI-malleability, or BaseFold raw-API depth-acceptance results.

A previous agent died mid-lane leaving an unverified 298-line PoC. Every function it contained was re-audited against the pinned source (each called API located and its contract checked) before being trusted; it was sound, and this lane extends it with two new modes (`mul-kernels`, `c1-uninit`) and one new panic probe.

## Finding 1: zero-round BaseFold provers emit never-written memory as proof fields

**Severity: Low-to-Medium (latent library bug; not reachable from any production code path at the pinned commit, and the resulting proofs fail verification).**

`prove_with_precomputed_round0_prime` takes its ping-pong buffers from the process-global scratch pool (`noid_ivc_core/src/pcs/basefold.rs:820-837`, `noid_ivc_core/src/scratch.rs:57-68`) or `alloc_uninit_vec` (`noid_ivc_core/src/lib.rs:135-145`), both of which document a strict write-before-read contract. The entire population of those buffers happens inside `for round in 0..log_msg_len`. The entry assertions accept `a_init.len() == 1` (a power of two, non-empty), i.e. `log_msg_len == 0`, in which case the loop body never executes and the epilogue reads slots no code path ever wrote:

- `noid_ivc_core/src/pcs/basefold.rs:1074` — `let final_a = a_active[0];`
- `noid_ivc_core/src/pcs/basefold.rs:1076` — `final_codeword = codeword_active[..cw_len]` (also never written at `log_dim == 0`: no FRI round runs)

The C1 twin has the identical defect with a worse memory source (`alloc_uninit_vec` directly — genuine uninitialized malloc memory, not just stale pool data):

- `noid_ivc_core/src/pcs/basefold.rs:1602-1610` (allocations), `:1811` (`final_a = a_active[0]`), `:1813` (`final_codeword`). Its `codeword_active` is pre-filled at `log_batch_size == 0` (`:1618-1631`), so only `final_a` is affected.

Demonstrated locally (deterministic across runs):

- F128 path, pool poisoned with a marker: the emitted `BaseFoldProof.final_a` is exactly the marker `0x1EAD_ED00_0000_0004 : 0xD15C_0000_0000_0004` — i.e. proof bytes derived from buffer contents the prover never wrote. In a warm production pool those buffers hold earlier-phase/earlier-prove witness data; cold, they are uninitialized heap (formally UB to read; `alloc_uninit_vec`'s own safety comment says so).
- C1 path: `C1BaseFoldProof.final_a` reads as `0` where the honest value is the lifted witness `F256::from_base(0x11,0x22)`; a malloc-recycle bait of 128 poisoned 32-byte chunks was NOT handed back (`marker observed: false` — macOS malloc returned fresh zero pages), so the C1 demonstration rests on the value differing from the only honest possibility plus the code path never writing the buffer, not on a captured marker.
- Both emitted proofs fail their own verifier: `basefold::verify_c1` returns `Err(SumcheckFinalMismatch)` (printed in `results.txt`). The verifier binds `final_a` to the sumcheck target (`basefold.rs:1307` / the C1 equivalent), so garbage `final_a` cannot be exploited for acceptance; an honest prover that did hit this path would simply always produce rejected proofs (2^-256 coincidence aside), plus (F128 case) leak stale pool contents into the serialized proof bytes.

Preconditions and why this is not production-reachable at the pinned commit: every in-tree caller passes the packed witness of length `2^(m - LOG_PACKING)` with commitment params fixed by the history-step class registry — `HISTORY_STEP_CURRENT_CLASS_MS = [22, 24]` with `log_batch_size = 5` (`noid_recursive/src/acceptance/history_step_bank.rs:110,48`), so production `log_msg_len` is 22 or 24 (`log_dim` 17 or 19). `log_msg_len == 0` requires `m == LOG_PACKING` with `log_batch_size == 0`, which no in-tree caller constructs. The pcs caller even special-cases its own different degeneracy (`lo_len == 1`, `noid_ivc_core/src/pcs.rs:299-301`) but still always has `a_init.len() ≥ 2`.

### Recommended correction

Reject `log_msg_len == 0` (or all of `a_init.len() < 2`) at the entry assertions of both provers, or write the degenerate-case `final_a`/`final_codeword` from the borrowed inputs before the epilogue reads them.

## Finding 2: panics on degenerate raw-API parameters (NTT construction, FRI query derivation)

**Severity: Low (library-misuse robustness; both panics sit behind commitment-bound parameters in every production caller).**

- `AdditiveNttF128::standard(0)` panics with index-out-of-bounds at `noid_ivc_core/src/ntt/additive_ntt_f128.rs:72`: `generate_evals_from_subspace` pushes the empty basis as row 0, then the normalize loop indexes `row[0]` on it. Production calls `standard(k_code())` with `k_code = log_dim + log_inv_rate ≥ 17 + 2`.
- `basefold::default_fri_queries(0, 1)` panics at `noid_ivc_core/src/pcs/basefold.rs:229` ("unsupported BaseFold UDR configuration … FiniteLengthTheoremPrecondition") because `checked_fri_configuration` rejects the tiny domain and the `unwrap_or_else(panic!)` fires. The same panic would occur inside `verify`/`verify_c1` for such a commitment shape — but the shape is a commitment parameter there, not proof data, and the SECURITY comment at `basefold.rs:1227-1231` shows the author deliberately derives shapes from params, not from attacker bytes. `log_inv_rate = 5` at `log_dim = 0` passes the precondition, which is how the C1 PoC runs end to end.

## Finding 3: allocation amplification in the unused `R1csProofBundle` parser

**Severity: Low (no in-tree caller of the format; local file parsing only).**

`noid_ivc_prover::proof_io::R1csProofBundle::from_bytes` (`noid_ivc_prover/src/proof_io.rs:55-58`) is magic-bytes + unconfigured bincode. A 73-byte crafted bundle drives a single 1,048,576-byte allocation (~14,000x input size) through bincode's length-prefixed `Vec` deserialization before failing cleanly with an EOF error. Amplification is bounded by bincode's element-wise reads (it errors as soon as the stream truncates); no panic, no OOB. A repo-wide grep finds no caller of `R1csProofBundle` outside `proof_io.rs` itself (its tests included), so no untrusted input reaches it at the pinned commit. The result is recorded as a defense-in-depth observation, not a reachable DoS.

## Negative and control results

- **F128/F256/F8 multiplication kernels agree everywhere tested** (this is the consensus-split probe): 8,241 input pairs (4,096 random, random self-products, and the full 7x7 edge cross product including zero, one, generator, all-ones) agree bit-exactly across the production NEON dispatch (`F128::mul` → `ghash_mul_binius`), `software::ghash_mul`, `ghash_mul_schoolbook`, `ghash_mul_karatsuba`, `ghash_mul_karatsuba_barrett`, the 2-lane `ghash_mul_vec2_neon` (with lane-independence control), and both unreduced pipelines (`mul_unreduced().reduce()`, `ghash_mul_unreduced_neon().reduce()` vs `software::ghash_mul_unreduced().reduce()`). 1,024 pairs confirm `F256::from_base(a) * F256::from_base(b) == F256::from_base(a*b)` (C1 lift consistency). 1,024 random 16-lane blocks confirm `gf2_8::neon::gf8_mul_vec16` == scalar `F8::mul` per lane. No native-vs-SIMD disagreement found.
- **NTT dispatch differentials clean**: for `log_d` 1..=10, `forward_transform` (dispatched) == `forward_transform_scalar` and roundtrips through `inverse_transform`; `forward_transform_interleaved` == `..._scalar` for lanes 1/2/4/8; `InvNttTableByteSingleGf8::apply` == `apply_scalar` for `k` 3..=7 over 128 random byte strings each.
- **Poseidon2b batch-vs-native clean**: `batch::leaf_sponge_flat_batch_with_iv_into` == sequential `native::Poseidon2bFlatSponge::with_iv_flat` + `finalize_no_pad` for 1-17 leaves of 64 bytes with random IVs.
- **Matrix loader does not panic on 2,689 adversarial inputs**: every truncation of a real serialized `FieldR1cs` (canonical length 688 bytes) plus 2,000 random 1-8-byte bit flips and truncations/extensions, fed to `read_artifact_unbound`, returned `Ok`/`Err` without panicking. (Control: the unmutated artifact round-trips through `read_artifact` with matching digest.)
- **Verifier shape-gate audit (by reading, not PoC)**: `basefold::verify`/`verify_c1` pin `round_messages`, `round_commitments`, `plaintext_tail`, `queries`, per-query leaf and Merkle-path lengths to commitment-derived constants before any indexing (`basefold.rs:1232-1260`, `1903-1914`); `zerocheck::verify` rejects `m < k_skip + 7` and pins all message lengths (`zerocheck.rs:494-518`). No index-arithmetic panic path was found that attacker-controlled proof bytes can reach at production shapes.

## PoC implementation and reproduction

`src/main.rs` is a single binary with modes; `all` (default) runs everything. It uses a counting global allocator to measure the parser's largest single allocation, a deterministic splitmix-style RNG, a no-op `Challenger` (the real trait with default F256/grinding methods), and the real production functions listed above — no reimplementation of target logic.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target" cargo run --release 2>&1 | tee results.txt
```

The two `thread 'main' panicked` blocks inside `results.txt` are expected: they are the caught panics of Finding 2 printed by the default panic hook before `catch_unwind` returns them. Final lines:

```text
zero-round final_a=1eaded0000000004:d15c000000000004
c1 zero-round final_a=(0000000000000000:0000000000000000,0000000000000000:0000000000000000) expected_honest=(0000000000000022:0000000000000011)
c1 zero-round malloc-recycle marker observed: false
c1 zero-round verify verdict: Err(SumcheckFinalMismatch)
parser hostile: input=73 declared=10000000 max_single_allocation=1048576 result=IVC proof bundle bincode error: io error: unexpected end of file
small input: AdditiveNttF128::standard(0) panicked at empty eval row
small input: basefold::default_fri_queries(0, 1) panicked (finite-length precondition -> unwrap_or_else(panic))
differential: NTT, inverse-table, and Poseidon native/batch checks passed
mul kernels: 8241 F128 pairs agree across dispatch/unreduced-reduce/schoolbook/karatsuba/barrett/binius/vec2; F256 lift consistent; F8 vec16 == scalar
matrix loader: 2689 truncation/mutation cases returned without panic
```

The F128 marker result is deterministic (verified across repeated runs); the C1 result's zeros are the allocator's fresh zero pages and are not claimed as a captured marker.

## Assurance limits

- All results are library-level against the pinned crates on this machine (aarch64, AES/NEON compiled in, rustc 1.96.0). The x86 `ghash_mul_clmul`/AVX-512 and `mul_f256_avx2` kernels do not compile here and were audited by reading only; the F256 AVX2-vs-portable differential is untested.
- Release profile with `debug-assertions = true` and `overflow-checks = true` (lane Cargo.toml). Production nodes ship release without those; no finding depends on a debug assertion firing, but arithmetic-overflow behavior may differ.
- "Not production-reachable" for Finding 1 is established by enumerating in-tree callers of the basefold provers at this commit (pcs open paths + the class registry constants), not by proving no future caller can construct `m == LOG_PACKING`.
- The matrix-fuzz corpus is one artifact shape (m=7/k_log=7/k_skip=6, identity/zero matrices) with bit flips, truncations, and short extensions — not coverage-guided fuzzing, and `read_artifact` with the bound-shape/digest path is exercised only in the positive control.
- The `R1csProofBundle` "no in-tree callers" claim is a grep of all workspace crates' `src` trees at the pinned commit.
- No upstream source was edited; the parano1d checkout remains read-only at `52938433`.
