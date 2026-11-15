# agent-25: Audit build release artifacts

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Audited pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`. Found **one High** and **two Medium** issues. No repository files were modified; all fixtures and probes are under `experiments/build-artifacts`.

## High — Legacy matrices are embedded without authenticating their bytes

`scripts/build_release.sh:143-149` validates layout and reads `pins.env`, but only uses the metadata digest. It never calls `release_authenticate_pack` or `release_verify_sha256_manifest`, and discards `NOID_HISTORY_STEP_PACK_LEAF_DIGESTS`.

`noid_node/build.rs:388-408` passes each legacy matrix to `stage_leaf`. At `noid_node/build.rs:442-470`, that function only:

- bounds and decompresses the payload;
- measures its decoded length;
- pairs it with the shape and statement digest from metadata.

It never hashes the compressed bytes against the reviewed leaf pins or performs the structural matrix authentication used for v2/v2.1. Runtime subsequently calls the build-authenticated decoder (`noid_miner/src/history_step_artifacts.rs:462-469`), which deliberately omits the structural Poseidon digest check (`noid_ivc_core/src/field_r1cs.rs:3799-3844`).

**Concrete check:** `probe_release_inputs.py` replaced a legacy matrix with different valid Zstandard bytes having the same decompressed length, leaving `pins.env` and `SHA256SUMS` stale. The release script reached Cargo with the unchanged metadata pin and did not export the leaf pin. Explicitly invoking `release_verify_sha256_manifest` on the same fixture rejected it:

```text
cargo_reached=true
legacy_leaf_pin_exported_to_cargo=false
explicit_manifest_verifier_exit=1
error: SHA-256 mismatch ... history-step-c00.field-r1cs.zst
```

Results: `experiments/build-artifacts/probes/release-inputs.json`.

**Impact:** a well-formed foreign R1CS with compatible shape and length can be embedded while receiving the canonical pinned identity. This can make the resulting node prove or verify a different relation from canonical peers, causing invalid local acceptance, rejected proofs, or consensus divergence.

**Scope/mitigations:** affects builds retaining legacy matrices, not the default `retired-history` profile. `publish_release.sh` has an additional `release_authenticate_pack` gate, but the standalone build script does not, and that publisher preflight is not a substitute for authenticating every legacy class during embedding.

**Recommended fix:** authenticate every legacy matrix during `build.rs` staging with the equivalent of `CompactFieldR1cs::open`, and also verify the reviewed compressed-leaf pins or call the existing pack-authentication gate before building.

## Medium — Packed runtime cache accepts a foreign relation under the approved digest

The cache directory is keyed only by legacy metadata digest (`noid_node/src/main.rs:1002-1008`), not by the embedded matrix payload. Cache loading at `noid_miner/src/history_step_artifacts.rs:395-423` reads the packed file and invokes the unsafe build-authenticated decoder.

That decoder explicitly performs no structural hash or packed-layout semantic revalidation (`noid_ivc_core/src/field_r1cs.rs:3986-4005`) and installs the seal’s statement digest at `noid_ivc_core/src/field_r1cs.rs:4062-4071`.

**Concrete check:** `probes/cache/src/main.rs` constructed two valid, same-shape, same-length relations with different structural digests. Against archived pinned source:

```text
foreign_image_accepted=true approved_digest_retained=true safe_loader_rejected=true canonical_lengths=210 image_lengths=391
```

The safe loader rejected the foreign relation, while the packed cache decoder accepted it and labeled it with the approved digest.

**Impact:** a poisoned or stale cache can change verifier semantics. A cache created by a substituted legacy build can also survive installation of a corrected binary because the metadata-derived cache path remains unchanged.

**Precondition:** write access to the node data directory, or a previously poisoned build/cache. This limits severity but contradicts the source comment that cache corruption can only affect availability.

**Recommended fix:** key caches by embedded payload identity and cryptographically reauthenticate loaded cache contents before applying the pinned statement digest. Payload-derived paths alone prevent accidental stale reuse but do not stop deliberate cache replacement.

## Medium — Inherited Cargo environment can turn release verification into debug semantics

`scripts/build_release.sh:273-303` clears `RUSTFLAGS` and target selection but does not clear or set Cargo profile overrides such as `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS`.

The override reached Cargo in the isolated release-script fixture. A tiny Cargo release probe demonstrated:

```text
normal:     debug_assertions=false fri_queries=64 capsule_queries=64 capsule_grind_bits=16
overridden: debug_assertions=true  fri_queries=10 capsule_queries=8  capsule_grind_bits=6
```

These constants are security- and wire-relevant:

- `noid_fri/src/channel.rs:28-35`: 64 versus 10 queries.
- `noid_fri_binius/src/capsule.rs:92-98`: 64 versus 8 queries.
- `noid_fri_binius/src/capsule.rs:142-147`: 16 versus 6 grinding bits.

**Impact:** an accidental or injected environment variable can produce a nominal `--release` binary with debug-strength, incompatible verifier behavior. There is no evidence the checked-in CI currently sets this override.

**Recommended fix:** explicitly set `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=false` for release builds and add a post-build assertion covering `debug_assertions` and production query/grind constants.

## Controls that held

- v2/v2.1 metadata is checked against independent bank pins, source schedule, and activation; ordinary and prechecked matrix paths bind payload hash, bank, shape, statement digest, and canonical length (`noid_node/matrix_checks.rs:66-100`, `noid_node/matrix_checks.rs:110-160`).
- Release scripts select the explicit host triple, unset `CARGO_BUILD_TARGET`, and use portable target CPU flags; no host/target substitution was found.
- Archive extraction rejects traversal, links, duplicate/unexpected members, and digest mismatches. All **17** copied release material/input tests passed.
- The embedded retirement certificate SHA-256 matches its documented value: `845dd84a…abf`. Startup verifies it under pinned retirement keys before retention.
- Local `origin/main` (`e6d324ff…`) still contains the size-only legacy staging, unchanged cache/query code, and the profile-environment gap. No remote fetch was performed.

## Limitations and artifacts

I did not run a full production node release, create a production-size foreign R1CS/proof, or demonstrate a live network fork. The release-script probe deliberately used a Cargo stub; the cache probe used small valid relations and the actual pinned decoder. Consensus impact beyond those directly demonstrated trust failures is therefore inferred from the verifier paths.

Created only in `experiments/build-artifacts`:

- `probe_release_inputs.py`
- `probes/release-inputs.json`
- `probes/results.json`
- `probes/profile/` and `probes/cache/`
- `fixture/` containing pinned release scripts/tests
- `pinned-src/` containing pinned Rust source for the cache probe

Temporary Cargo target directories and generated test fixtures were removed. The source checkout’s pre-existing dirty files remain unchanged.
