# Legacy matrix authentication in the standalone release builder

Target: `/Users/ember/dev/parano1d-break/parano1d` at
`52938433a30b185ab007afaa15166a632cf2e82e`. The probes use copies of the
relevant pinned source in `pinned-src/` and release scripts in `fixture/`;
these were compared against the target commit. The target checkout's existing
`noid_node/build.rs` diff only reformats v2/v2.1 sections and does not change
the legacy `stage_leaf` path discussed here.

## Result and scope

The standalone `scripts/build_release.sh` validates the legacy pack layout and
metadata pin, but does not verify each legacy matrix against its digest. A
same-shape substituted matrix can pass the build script's checks while the
approved metadata remains unchanged. The build-script path seals the substituted
bytes with the approved shape and statement digest, and the runtime decoders
accept bytes under that seal without recomputing the matrix digest.

The release-script probe replaced one matrix and confirmed that the script
reached Cargo with the same metadata pin. It used a Cargo stub, so that run did
not produce a binary. A separate decoder probe compiled from pinned source
confirmed that the embedded and cache decoders accept a foreign same-shape
matrix under the approved digest. The source path connects those two results;
the combined substitution-to-running-node sequence was not executed end to end.

Preconditions:

- The build uses the legacy-embedding profile: `scripts/build_release.sh`
  without `--retired-history` (that script's default), or any
  build exporting `NOID_HISTORY_STEP_PACK_DIR` + the metadata digest without the
  `retired-history` Cargo feature.
- The attacker can alter the legacy pack directory consumed by the builder,
  through a substituted archive or local files. The attack does not require
  editing `pins.env`, `SHA256SUMS`, or the metadata file. The official release
  workflow has additional checks described below.

## Reproduction

### Run 1 — release-script substitution probe

`verify/probe_release_inputs_verify.py` runs an unsubstituted control, a
substituted pack, and an explicit manifest-verifier control. From
`experiments/build-artifacts`:

```
python3 verify/probe_release_inputs_verify.py 2>&1 | tee verify-output.txt
```

Result (exact lines from `verify-output.txt`):

```
"control":  { "build_release_exit": 91, "cargo_reached": true,
              "metadata_pin_seen_by_cargo": "a1a1...a1a1" }
"substituted": { "build_release_exit": 91, "cargo_reached": true,
                 "metadata_pin_seen_by_cargo": "a1a1...a1a1",
                 "leaf_pin_exported_to_cargo": false,
                 "manifest_stale_for_c00": true,
                 "log_lines_mentioning_hash_or_manifest": [] }
"explicit_manifest_verifier_exit": 1
"explicit_manifest_verifier_stderr": "error: SHA-256 mismatch for .../legacy/v1/history-step-c00.field-r1cs.zst"
```

- Exit 91 is the probe's Cargo stub: both runs reached Cargo with the same
  metadata pin. The substituted `c00` has SHA-256 `309fd906…`, versus
  `12f89b9b…` for the original; both decompress to 1024 bytes.
- The build-script log contains no hash or manifest-verification line. The
  explicitly invoked `release_verify_sha256_manifest` rejects the substituted
  file. This proves the manifest check exists and catches this fixture, while
  the standalone builder does not call it.

### Run 2 — decoder probe (runtime half, compiled from pinned source)

`verify/decoder-probe/` builds two same-shape, same-length relations differing
in one coefficient and hence in structural Poseidon digest. It then calls the
decoders used by the node:

```
cd verify/decoder-probe && CARGO_NET_OFFLINE=true cargo run --quiet --release
```

Result (exact line):

```
safe_loader_rejected=true embedded_decoder_foreign_accepted=true cache_decoder_foreign_accepted=true ... canonical_len=210
```

- `open_packed` rejects the foreign relation under the approved digest. This
  API performs the missing structural check.
- `open_build_authenticated`, used by `rebuild_from_canonical` for embedded
  bytes, accepts the foreign bytes and returns a relation labeled with the
  approved statement digest.
- `open_build_authenticated_packed_image`, used for the runtime cache, also
  accepts the foreign bytes under that label.

## Code path at the pinned commit

1. `scripts/build_release.sh:143-149` — legacy branch calls
   `release_validate_pack_layout "$PACK_DIR" 1` (layout only: names, regular,
   non-empty, no symlinks, exactly 3 v1 files; requires `SHA256SUMS` to EXIST
   but never reads it) and `release_read_pin_file` (format check only: two
   assignments, 64/128 lowercase hex). `RELEASE_METADATA_DIGEST` is taken from
   `pins.env` verbatim.
2. `scripts/build_release.sh:275-276` — unsets
   `NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST` and
   `NOID_HISTORY_STEP_PACK_LEAF_DIGESTS`; line 305 re-exports ONLY the metadata
   digest. The reviewed leaf pins are read and discarded. `grep` confirms
   `NOID_HISTORY_STEP_PACK_LEAF_DIGESTS` appears nowhere in `noid_node/build.rs`.
3. `noid_node/build.rs:397-416` — legacy embedding passes each matrix to
   `stage_leaf` with `entry.shape()` / `entry.matrix_digest()` taken from the
   metadata bank. `stage_leaf` (`build.rs:442-470`) zstd-decodes to a sink,
   measures decoded length, and seals `{shape, statement_digest, canonical_bytes}`.
   It never hashes the matrix bytes (compressed or decompressed).
4. The metadata is authenticated:
   `decode_history_step_runtime_metadata_pinned`
   (`noid_miner/src/history_step_artifacts.rs:170-239`) requires the Poseidon
   hash of the metadata body to equal both the in-file trailer and the exported
   pin (`ReleasePin` error otherwise), and the body carries the per-class
   `matrix_digests`. The bank contains the expected matrix digests, but
   `stage_leaf` does not compare them. The bank is also bound to the external
   `--v2-pins` retirement-key pins via `PinnedRetirementKeys::from_release`.
5. The runtime does not repeat the structural check:
   `noid_miner/src/history_step_artifacts.rs:423-469` — `rebuild_from_canonical`
   uses `open_build_authenticated`; `load_cached_image` uses
   `open_build_authenticated_packed_image`. Both are documented as deliberately
   omitting the structural Poseidon pass; both install `seal.statement_digest`.
   The source comment in `rebuild_from_canonical` states the design assumption:
   "anyone able to swap the embedded leaves can swap the embedded pins beside
   them". Here the matrix bytes change while the pins remain unchanged; the
   explicit manifest verifier would detect the change, but the standalone
   builder does not invoke it.
6. Consensus relevance: the embedded pack drives history-step verification
   (`noid_node/src/main.rs:2018-2021`); the runtime cache is keyed by the
   metadata digest only (`main.rs:1002-1008`). The code does not invalidate a
   cache solely because a later binary has corrected matrix bytes. Cache
   persistence across such a binary replacement was not exercised in this probe.
7. The v2/v2.1 embedding path binds payload hash,
   bank, shape, statement digest and runs semantic preflight
   (`matrix_checks::authenticate` + `preflight_build_matrix`,
   `noid_node/build.rs:201-320`). This check offers an implementation pattern
   for the legacy path.

## Checks and compensating controls

- Framing and length checks do not establish matrix identity. The probe's
  same-shape relations pass the build-authenticated decoders, and `stage_leaf`
  records the substituted payload's own decompressed length in its seal.
- `--prechecked-matrices` exports `NOID_V2/V21_MATRIX_CHECKS_SHA256`; it does
  not add a legacy matrix check (`build_release.sh:297-311`).
- In the official transition release path, `publish_release.sh:223` calls
  `release_authenticate_pack`. CI then checks the archive digest against the
  publisher-supplied `PACK_SHA256` (`release_material.py extract`,
  `.github/workflows/release.yml:60,328-337`). These controls protect the
  official publish chain as scripted. CI does not independently run the inner
  per-file manifest check for the transition build. A standalone builder using
  a pack from another source lacks the publisher-side controls.
- `--retired-history` does not embed the legacy matrices. V2/v2.1 matrices
  have separate payload-hash and semantic authentication. The two scripts
  have different defaults: `publish_release.sh` defaults to retired history,
  while `build_release.sh` defaults to the legacy-embedding profile.

## Conclusion

The legacy matrix identity check is missing from the standalone builder's
default path. The measured substitution reached Cargo with unchanged metadata;
the separate decoder test accepted a foreign relation under the approved seal.
A build from an unverified legacy pack could therefore use a different
HistoryStep relation while presenting the approved metadata identity. That
could change local proof acceptance, but this report did not build such a
binary or show consensus divergence between nodes.

Authenticate each legacy matrix against the digest already present in the
metadata bank during `stage_leaf`. Calling
`release_verify_sha256_manifest` from `build_release.sh` would also detect
tampered pack files, provided the manifest itself has an authenticated origin.
The official publisher-side authentication and archive pin should remain in
place; the standalone builder must enforce its own advertised boundary.

## Artifacts

- `verify/probe_release_inputs_verify.py` — release-script probe.
- `verify/decoder-probe/` — decoder probe source, including the embedded-path
  `open_build_authenticated` call.
- `verify/release-inputs-verify.json` — run-1 structured result.
- `verify-output.txt` — captured output of both probes.
- `probe_release_inputs.py`, `probes/`, `fixture/`, `pinned-src/`, and `tmp/` —
  prior fixtures and probe files.
