# Crypto and soundness claim replication

Source tree: `experiments/uc-composition/pinned/`, archived from source commit `52938433a30b185ab007afaa15166a632cf2e82e`. The harness uses synthetic inputs and does not talk to a node. Run it with:

```sh
CARGO_BUILD_JOBS=2 cargo run --release --offline --manifest-path experiments/verification-crypto/Cargo.toml
```

The bounded release run is in `run.log`. All checks passed on the local `aarch64-apple-darwin` host. The only search performs at most 500,000 first-block permutations; the recorded witness appeared at trial 73,494 in 136 ms. The checked output was:

```text
left_64=a8190000000000000000000000000000e2bc00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
right_64=0dd5000000000000000000000000000039500000000000000000000000000000f5d20000000000000000000000000000d0870000000000000000000000000000
digest_32=e5c3f258a47028d37747859698ae3a3e2054b6d68ce43fdc0a9d037db44c5595
```

## PA-06: zero-IV sponge collision, with a strict boundary

**Confirmed:** These distinct 64-byte strings collide under the complete, padded `Poseidon2bSponge::new().update(message).finalize()` and under public `poseidon2b_digest` and `CryptographicHasher::hash_concatenation`. The harness also confirms they *do not* collide under `poseidon2b_hash_bytes(b"verification", ...)`, which uses a tagged IV and message framing, or under `compress_with_tag(TAG_EXSTNOD, ...)`, which matches the exact-state node hash construction.

The construction samples two first blocks with rate words in GF(2^16), whose post-permutation capacity is confined to 32 bits. Once two capacity pairs agree, it sets the second rate block on one message to the XOR of the two first-block rate pairs. Immediately before the second permutation, all four state lanes agree. The code asserts this full-state equality and then independently checks the final padded outputs. This is a chosen-message collision in the generic zero-IV byte-hash API, not merely an unpadded compression collision.

**Production reachability:** No external pinned-tree caller of `poseidon2b_digest` or direct `hash_concatenation` was found. The exposed batch concatenation functions also have only test call sites in this tree. `CryptographicHasher::compress`, used for FRI Merkle inner nodes and segment-cap nodes, overrides the trait default with a `COMPRESS`-IV compression function (`noid_poseidon2b/src/hasher_impl.rs:37-39`, `native/compression.rs:221-240`). `noid_chain::fri_state::compute_segment_root` does pass a zero-IV hasher, but its arithmetic leaves use `hash_pair` / `hash_field` and its internal nodes use `compress`; the segment root is node metadata. Exact consensus slot/state roots use `TAG_EXSTSLT` and `TAG_EXSTNOD` (`noid_chain/src/exact_state_hash.rs:24-34`). The wallet keystore's zero-IV 64-byte input has a 256-bit master-secret first block and fixed `(index, "noid-deri")` second block (`noid_node/src/wallet/keystore.rs:73-91`); this witness's correction block does not meet that schedule. No wallet key compromise, valid Merkle-leaf collision, proof forgery, or consensus-root collision follows from this run.

The `CryptographicHasher` implementation ignores its `self` receiver for `hash_pair`, `hash_field`, and `hash_concatenation`. The harness confirms a `with_iv(...)` receiver produces the same trait outputs as `new()`, while direct `with_iv(...).update(...).finalize()` differs. This is a real API behavior that can surprise an external caller, but no pinned production call relying on receiver IV separation was found.

## PA-01: model selection is wrong only for a constructed mixed profile

`ProductionParameters::load()` returns History classes `(B25, k=19)` and `(B255, k=21)`, each needing 12 query-squeeze permutations. The current `.max()` in `noid_soundness/src/qrom.rs:43-55` therefore agrees with `.min()` for the shipped certificate. In a synthetic profile `(k=22, k=21)`, the per-class counts are `(14, 12)` and the code selects 14. `resource.rs:295-318` divides bad-event density by the squared response cost, so choosing 14 instead of the cheaper actual 12 can understate that density by `14²/12² = 49/36`. `v2.rs:122-128` explicitly selects the minimum. This demonstrates a model bug for heterogeneous future profiles, not a present certificate undercharge or an invalid chain proof. The subfield-blind degree projection in `poseidon2b_cryptanalysis.rs` is a limitation of its stated algebraic model; this harness does not turn it into a proof attack.

## PA-05: portability arithmetic fixture

The host checks correct tower multiplication via `Flat(a).to_flat() * Flat(b).to_flat()` and confirms that the non-x86/non-aarch64 source branch's no-op basis conversion would instead calculate `clmul_gcm(raw_a, raw_b)`. Witness: `a=0x02`, `b=0x80`, tower product `0x1b`, raw flat product `0x100`. The same fallback source branch applies to `Flat<Block128>::invert`; the fixture prints different flat and raw-tower inverses. On every architecture, `Flat<Block128>::from(0x80u32)` differs from the converted flat representation of tower `0x80`. This is a source-level targeted fixture executed on macOS; no wasm/riscv binary was run. The affected `PackedBlock128::packed_mul`/`scalar_mul` route is in `noid_core/src/packed/arith.rs:37-88`; production impact requires a deployed non-x86/non-aarch64 target that executes it.

## PA-07 and PA-08: library boundaries and edge behavior

`noid_fri::verifier` draws its tensor-batching point without absorbing `upper_partial_evals` (`noid_fri/src/verifier.rs:32-74`), and its later checks compare openings to `fri_oracles[round].root`, not an externally committed initial root (`:132-231`). A pinned-tree search found no non-test call to `noid_fri::verifier` or `noid_fri::batch::verify`; wallet authorization uses `noid_fri_binius::zk_capsule_pcs`, and IVC uses `noid_ivc_core::pcs::basefold`. This is a legacy library concern, not a demonstrated production bypass.

The harness confirms three `Poseidon2bChannel` edge properties: the first two squeezes of an empty channel are both zero despite a nonzero capacity IV; a one-lane message aliases the same message followed by an explicit padding lane; and consuming one versus two challenge lanes before the same subsequent absorb produces the same later challenges. It also confirms `RoundPolynomial::from_coeffs(vec![])` reports degree zero and evaluates to zero. In release mode, `Poseidon2bSponge::finalize_no_pad` discards a buffered half-block because its alignment check is only a `debug_assert`. These are API-level reproducible edges. We did not establish an accepted invalid production proof from them.
