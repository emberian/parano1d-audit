# Release-chain reassessment of C16

Target: `parano1d/` at `52938433a30b185ab007afaa15166a632cf2e82e`.
This is a source and saved-experiment review; I did not build or publish a new
binary or contact a NOID peer. The `v2.1.0` tag has no diff from this commit in
the release scripts, workflow, or `noid_node/build.rs`. The public
[v2.1.0 release](https://github.com/proof-native/parano1d/releases/tag/v2.1.0)
is the published version examined here; newer releases would require a fresh
comparison.

## Verdict

**C16 is a real check missing from the standalone builder's legacy-matrix
path. It is not a remote attack against the published v2.1 release.** A
builder using untrusted or damaged legacy pack files can embed a same-shape
foreign matrix while retaining the metadata's approved matrix digest. The
saved C16 script and decoder probes establish those two halves separately;
the devnet phase-6 run additionally built a full substituted node. That node
started but froze at height 144 while an honest node advanced to 149. It did
not forge a payment or cause honest nodes to accept invalid history.

The official release path has two additional barriers. First, the publisher
defaults to **retired legacy history**, so the current build omits the two old
matrix files (`scripts/publish_release.sh:41-48`,
`.github/workflows/release.yml:22-27,324-344`; `.github/RELEASING.md:35-43`).
The [v2.0.4 release notes](https://github.com/proof-native/parano1d/releases/tag/v2.0.4)
say this retirement began before v2.1. Second, a transition release made with
`--with-legacy-history` calls `release_authenticate_pack` before archiving;
that function checks the manifest and recomputes matrix pins against
`pins.env` (`scripts/publish_release.sh:218-223`,
`scripts/release_common.sh:374-391`). Native CI checks each archived pack's
publisher-supplied SHA-256 before unpacking and passes the same verified packs
to the platform builders (`.github/workflows/release.yml:172-207,299-337`).
Those checks close the demonstrated same-pack substitution in the scripted
publisher workflow, assuming the publisher's source, pack review, and supplied
archive digests are trustworthy. We did not independently reproduce every
published build or inspect its compiled matrix bytes.

| Path | Attacker must control | Observed or source-backed effect |
| --- | --- | --- |
| Standalone `build_release.sh` without `--retired-history` | The local legacy pack directory or an archive accepted by its builder | Same-shape foreign matrix passes the shell probe; the legacy Rust build path does not compare its digest with the approved metadata (`../build-artifacts/VERIFY.md`). |
| Official transition publisher | A way around or control over `release_authenticate_pack`, the approved archive digest, or the publisher host | No such way was found. The scripted preflight rejects the saved substitution. |
| Official v2.1 retired-history build | Control of the release/tag/CI channel or another defect in v2/v2.1 checks | The C16 legacy matrix is absent. V2 and v2.1 matrix payloads have separate SHA-256 and semantic checks (`noid_node/build.rs:201-320`). |
| User's GUI using a substituted standalone node | Control of the node binary/build input the user installs | The node may stop following honest blocks and the GUI may display its stale state as locally authenticated. No forged receipt was demonstrated. |

## Exact evidence boundary

The standalone script checks the legacy pack's layout and reads `pins.env` at
`scripts/build_release.sh:143-149`. It does not call
`release_verify_sha256_manifest` and exports the metadata digest but not the
reviewed leaf digests (`:275-305`). In `noid_node/build.rs:397-470`, the
legacy `stage_leaf` zstd-decodes and length-checks each file, then seals the
canonical bytes with shape and statement digest taken from authenticated
metadata. It never structurally hashes those bytes. Runtime embedded and cache
decoders trust that build seal (`noid_miner/src/history_step_artifacts.rs:423-469`).
The existing [C16 probe](../build-artifacts/VERIFY.md) changed a same-shape
matrix, kept metadata unchanged, reached a Cargo stub, and separately showed
that the runtime decoder accepted a foreign relation under the old label.

The [phase-6 full-node run](../devnet-chaos/README.md) is more useful for
impact than the stub. Its `results.txt` records a full release-profile binary
with a coefficient flipped in the 84 MB legacy matrix. On a copied honest
database it displayed real old receipts and balance. After the honest chain
advanced five blocks, the tampered node stayed at height 144. The GUI backend
marked a receipt at height 110 `merkle_valid=true canonical=true
confirmed=true` against both the tampered and honest nodes. That is a **stale
view of a genuine old receipt**, not an invented payment accepted under the
foreign matrix. The test never constructed a proof for a false payment or got
an honest peer to accept the tampered node's alternate history.

The GUI has a separate trust boundary: it asks its configured node to verify
receipts rather than checking proofs itself (`../wallet-vs-node/README.md`).
A scripted RPC impostor can return an invented `verified` verdict to the GUI
backend. That experiment requires control of the GUI's RPC endpoint and does
not depend on C16. Combining the two as if one caused the other would inflate
the build finding. A malicious distributor could ship an arbitrary malicious
wallet or node; C16 does not grant a distributor power it would otherwise
lack. The GUI has no automatic binary updater in the reviewed source.

## Distribution integrity

The workflow generates `SHA256SUMS` for the finished assets and the publisher
checks it when mirroring (`.github/workflows/release.yml:428-456`,
`scripts/publish_release.sh:347-368`). That detects damaged downloads against
the same release channel. It is not an independent signature that protects a
user from someone able to replace both assets and checksum file on that
channel. Conversely, the absence of such a signature is not evidence that
the operator has shipped a malicious binary. The manual publication path
requires an authorized workflow dispatch and matching annotated tag; the
review found no lower-trust input through which an ordinary network peer
could alter a release. An independent reproducible-build or external-signing
control would reduce publisher/channel trust, but its absence is an
operational assurance limit, not a demonstrated rugpull mechanism.

## Practical disposition

Fix the standalone builder by comparing each decoded legacy matrix's
structural digest with the authenticated metadata entry during `stage_leaf`.
This prevents a future nonretired/custom build from trusting substituted pack
bytes. The public claim should be: **"A custom legacy release build can
mislabel a tampered matrix if its pack input is changed; the official current
workflow omits legacy matrices and authenticates the transition path."** Do
not say that C16 presently lets a remote peer forge a wallet confirmation or
that the developer has used it to censor holders.
