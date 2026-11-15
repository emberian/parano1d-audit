# agent-9: Attack address preimage

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Implemented and ran the ADDRFIX cryptanalysis lane at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`. No shared source or live targets were used.

## Main findings

- **No unrestricted full-round key recovery was found.**
- ADDRFIX is exactly one 66-round permutation followed by two-lane truncation, with no feed-forward: `parano1d/noid_poseidon2b/src/primitives.rs:298-305`.
- Generic unrestricted preimage cost remains `2^256` classical evaluations or `2^128` ideal Grover queries. Thus these experiments found no cheaper attack on the ledger’s 128-bit post-quantum address-preimage premise at `parano1d/noid_gkr/src/zk_auth_qrom.rs:92-98`.

### Subfield and Frobenius results

- Round constants lie in `GF(2^8)`; all MDS coefficients require only `GF(2^16)`.
- `ADDRFIX_IV0` requires `GF(2^128)` and `ADDRFIX_IV1` requires `GF(2^64)`.
- Therefore direct lane-wise `K^4` permutation invariance cannot apply for any proper subfield: the fixed IV is outside it.
- The full permutation commutes with Frobenius powers divisible by 16. However, no nontrivial Frobenius automorphism fixes both IV lanes, blocking conjugate-key equivalence and orbit-search speedups.
- Verified:
  - 16/16 native/reference full-round matches.
  - 16/16 full inverse round trips.
  - 48/48 Frobenius relations at powers 16, 32, and 64.
  - 0/112 synthetic secret-only Frobenius equivalent-key or conjugated-address relations.

Implementation: `experiments/address-preimage/src/field128.rs:58-74,173-282`.

### Inversion does not reduce generic work

Guessing the two hidden output lanes and inverting the permutation still gives `2^256` candidates. Each candidate must match the fixed 256-bit capacity IV, with random-model success probability `2^-256`.

The inverse S-box exponent and inverse MDS implementation are at `experiments/address-preimage/src/field128.rs:41-56,94-170`.

### Exact full-round weak-key recovery

For the promised class where each secret lane is a raw 8-bit value:

- Exhausted all `65,536 = 2^16` native full-round candidates in **1.778591 s** (**36,847 evaluations/s**).
- Recovered the unique target key `s0=0xa5, s1=0x5a`.
- All 65,536 class addresses were distinct.
- Zero addresses had both output lanes in `GF(2^8)`, confirming that small-subfield keys do not produce the corresponding output invariant.
- The public `derive_address` API was checked against the direct permutation target.

This class occurs with probability `2^-240` under uniform 256-bit keys. It is a low-entropy weakness, not a uniform-key attack. See `experiments/address-preimage/src/field128.rs:284-350`.

### Reduced-field control

The separate `GF(2^8)` experiment retains all 66 rounds but uses 16-bit secrets/addresses and a synthetic IV:

- Forward and inverse searches both required `2^16` candidates.
- Both recovered preimages `0x5f9e` and `0xc237`.
- Found 41,466 distinct addresses and 32,602 colliding key pairs.
- Inverse search was slower in this run: 0.068753 s versus 0.037699 s forward.

This demonstrates equivalent keys in a same-size truncated mapping without an asymptotic preimage gain. **It does not apply as full-field Parano1d key recovery.** See `experiments/address-preimage/src/gf8.rs:130-201`.

## Files delivered

- `experiments/address-preimage/Cargo.toml`
- `experiments/address-preimage/Cargo.lock`
- `experiments/address-preimage/src/main.rs`
- `experiments/address-preimage/src/field128.rs`
- `experiments/address-preimage/src/gf8.rs`
- `experiments/address-preimage/README.md`
- `experiments/address-preimage/results.txt`

Run with:

```bash
cd /Users/ember/dev/parano1d-break/experiments/address-preimage
export CARGO_TARGET_DIR="$PWD/target"

cargo check
cargo run --release -- structure
cargo run --release -- weak-key
cargo run --release -- reduced-field
cargo run --release -- all
```

Final `cargo check` and the complete `all` run passed with all assertions. Full output is in `results.txt`. Generated build directories were cleaned afterward.

Remaining research blockers are algebraic/CICO or other nonlinear invariant attacks not exhausted here; the direct subfield, Frobenius-orbit, hidden-lane inversion, and tested weak-key directions provide no cheaper unrestricted attack.
