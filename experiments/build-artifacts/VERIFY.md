# Independent verification: agent-25 HIGH — legacy matrix bytes unauthenticated in the standalone release builder

Verifier: fresh session (independent of the dead agent-25 lane and the dead root).
Target: `/Users/ember/dev/parano1d-break/parano1d` at pinned commit
`52938433a30b185ab007afaa15166a632cf2e82e` (verified with `git rev-parse HEAD`).
The checkout's pre-existing dirty files were checked: the `noid_node/build.rs`
diff is rustfmt-only reformatting of the v2/v2.1 pack sections; the legacy
`stage_leaf` section cited by the finding is the pinned commit's code. The other
dirty files are unrelated. All probe code here runs against `pinned-src/`
(exact-copy of the pinned commit for the relevant files — verified by diff
against `git show HEAD:<file>`) or against `fixture/` scripts verified
byte-identical to `parano1d/scripts/`.

## 1. The claim, restated precisely

> Substituting a legacy HistoryStep matrix file (`v1/history-step-c*.field-r1cs.zst`)
> with different valid zstandard bytes of the same decompressed length does not
> change the pack's approved metadata identity, and `scripts/build_release.sh`
> (standalone release builder) still reaches Cargo and embeds those foreign bytes
> sealed with the canonical, pin-approved shape and statement digest. No later
> stage — build script, `build.rs`, or runtime decoder — ever hashes the matrix
> bytes.

Preconditions:

- The build uses the legacy-embedding profile: `scripts/build_release.sh`
  invoked WITHOUT `--retired-history` (which is that script's *default*), or any
  build exporting `NOID_HISTORY_STEP_PACK_DIR` + the metadata digest without the
  `retired-history` Cargo feature.
- The attacker controls the legacy pack *directory contents* as seen by the
  builder (re-hosted pack, tampered archive extraction, or local tamper), but
  does NOT need to touch `pins.env`, `SHA256SUMS`, or the metadata file.

## 2. Reproduction (mine, independent)

### Run 1 — release-script substitution probe

Script: `verify/probe_release_inputs_verify.py` (mine; adds two controls the
original lacked: an unsubstituted run, and a log scan for any hash/manifest
activity). Command (from `experiments/build-artifacts`):

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

- Exit 91 is the probe's cargo stub, i.e. "Cargo was reached". Both the pristine
  control and the substituted run reach Cargo with the identical exported
  metadata pin; the substituted `c00` (`309fd906…`) differs from the original
  (`12f89b9b…`, both decompress to 1024 bytes) — digests match agent-25's run
  exactly (deterministic fixture).
- The build log contains zero hash/manifest verification lines.
- The same pack is REJECTED by the existing-but-unwired
  `release_verify_sha256_manifest` when invoked explicitly — the check exists in
  `release_common.sh` and simply is not called by `build_release.sh`.

### Run 2 — decoder probe (runtime half, compiled from pinned source)

`verify/decoder-probe/` builds two valid same-shape, same-canonical-length
relations differing in one coefficient (different structural Poseidon digests),
then exercises the exact decoders the node uses. Command:

```
cd verify/decoder-probe && CARGO_NET_OFFLINE=true cargo run --quiet --release
```

Result (exact line):

```
safe_loader_rejected=true embedded_decoder_foreign_accepted=true cache_decoder_foreign_accepted=true ... canonical_len=210
```

- `open_packed` (safe API) REJECTS the foreign relation under the approved
  digest — this is the check that would catch the substitution if anything
  performed it.
- `open_build_authenticated` — the exact decoder `rebuild_from_canonical`
  applies to the binary's EMBEDDED matrix bytes — ACCEPTS the foreign canonical
  bytes and returns a relation whose `statement_digest()` equals the APPROVED
  digest (artifact bytes are the foreign bytes). This leg the original report
  only inferred from code; it is empirically confirmed here.
- `open_build_authenticated_packed_image` (runtime cache path) — same: accepts,
  labels with approved digest. This matches agent-25's cache probe output.

## 3. Code-level chain (all line numbers at the pinned commit)

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
4. Metadata IS genuinely authenticated:
   `decode_history_step_runtime_metadata_pinned`
   (`noid_miner/src/history_step_artifacts.rs:170-239`) requires the Poseidon
   hash of the metadata body to equal both the in-file trailer and the exported
   pin (`ReleasePin` error otherwise), and the body carries the per-class
   `matrix_digests` — the bank KNOWS the correct matrix digests; `stage_leaf`
   just never compares. (The bank is additionally bound to the external
   `--v2-pins` retirement-key pins via `PinnedRetirementKeys::from_release`.)
5. Runtime offers no backstop:
   `noid_miner/src/history_step_artifacts.rs:423-469` — `rebuild_from_canonical`
   uses `open_build_authenticated`; `load_cached_image` uses
   `open_build_authenticated_packed_image`. Both are documented as deliberately
   omitting the structural Poseidon pass; both install `seal.statement_digest`.
   The source comment in `rebuild_from_canonical` states the design assumption:
   "anyone able to swap the embedded leaves can swap the embedded pins beside
   them". That argument does not cover this finding's asymmetric case: swapping
   ONLY matrix bytes leaves every pin and manifest pristine-looking, so any
   existing verifier would flag it — the script just runs none.
6. Consensus relevance: the embedded pack drives history-step verification
   (`noid_node/src/main.rs:2018-2021`); the runtime cache is keyed by the
   metadata digest only (`main.rs:1002-1008`), so a cache minted by a
   substituted build survives a corrected binary (agent-25 medium#2; code
   confirmed).
7. Asymmetry inside the same file: v2/v2.1 embedding DOES bind payload hash,
   bank, shape, statement digest and runs semantic preflight
   (`matrix_checks::authenticate` + `preflight_build_matrix`,
   `noid_node/build.rs:201-320`). The fix pattern for legacy already exists
   in-repo.

## 4. Adversarial analysis — what would have to be true for this to be refuted, and what I found

- "A foreign matrix would fail framing/shape checks" — REFUTED empirically
  (run 2): same-shape relations pass both build-authenticated decoders; only the
  never-invoked safe loader rejects.
- "The decompressed-length check catches substitution" — REFUTED: same-shape
  relations have the same canonical length; and `stage_leaf` measures the length
  of the foreign payload itself, so the seal adapts.
- "`--prechecked-matrices` covers it" — REFUTED: it exports only
  `NOID_V2/V21_MATRIX_CHECKS_SHA256`; nothing exists for legacy (grep of
  `build_release.sh:297-311`).
- "CI always uses the safe path" — PARTIAL: `.github/workflows/release.yml:328-337`
  adds `--prechecked-matrices` only in the RETIRED branch; transition
  (`--with-legacy-history`) builds run `build_release.sh` without any legacy
  input check. CI's `release_material.py extract` verifies the ARCHIVE digest
  against `PACK_SHA256`, a `workflow_dispatch` input (line 60) supplied by the
  publisher — the inner per-file manifest is never checked in CI.
- "No real channel exists" — the official chain (publish → GitHub draft →
  dispatch CI) has two compensating anchors outside the script: publisher-side
  `release_authenticate_pack` (`publish_release.sh:223`, transition mode only)
  and the dispatch-pinned archive digest. The gap bites anyone who runs the
  standalone builder with a pack directory from any other provenance — which is
  precisely the mode the script's own usage text advertises ("The node build
  authenticates the matrix packs" — for legacy matrices it does not).

## 5. Verdict

**CONFIRMED-WITH-NARROWER-SCOPE** (technically confirmed in full; severity
depends on the pack-acquisition channel).

Scope sentence: *Legacy matrix bytes sealed under the pins.env-approved metadata
identity can be substituted without detection whenever a node is built via
`scripts/build_release.sh` without `--retired-history` (that script's default —
the transition/current-generation profile) from a pack whose integrity is not
independently verified out-of-band; but NOT in `--retired-history` builds (no
matrix bytes embedded), NOT for v2/v2.1 matrices (payload-hash + semantic
authentication against external bank pins), NOT in bare `cargo` builds, and NOT
in the official publish chain as scripted, where the publisher-side
`release_authenticate_pack` and the dispatch-input archive digest compensate —
controls that live entirely outside the standalone builder whose documented
posture claims matrix authentication.*

Severity under the realistic attack model: **High for the standalone builder as
an authentication boundary; Medium as a defect of the official release chain**
(which retains two independent compensating controls). Blast radius if
exercised: a node verifying/proving v1 HistoryStep against a foreign relation
under the canonical pinned identity — invalid local acceptance or rejection of
canonical proofs (consensus divergence), silently, with pins.env/SHA256SUMS
looking pristine and the runtime cache keyed so the poisoned artifacts survive a
corrected binary. Recommended fix unchanged from agent-25 (authenticate legacy
matrices in `stage_leaf` via the seal's bank digest — the bank already carries
`matrix_digest` — and/or wire `release_verify_sha256_manifest` into
`build_release.sh`).

One precision note on the original report's wording: "not the default
`retired-history` profile" is correct for `publish_release.sh` (its default is
retired), but `build_release.sh`'s OWN default is the affected legacy-embedding
profile — the flag `--retired-history` is opt-in there. The two scripts'
defaults differ; a rebuttal should say so explicitly.

## 6. Artifacts

- `verify/probe_release_inputs_verify.py` — run-1 probe (mine).
- `verify/decoder-probe/` — run-2 probe source (extends agent-25's cache probe
  with the embedded-path decoder `open_build_authenticated`).
- `verify/release-inputs-verify.json` — run-1 structured result.
- `verify-output.txt` — tee'd final runs of both probes (lane root).
- agent-25's originals (`probe_release_inputs.py`, `probes/`, `fixture/`,
  `pinned-src/`, `tmp/` cases) left untouched.
