# agent-15: Find version replay flaws

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Found one confirmed domain-separation gap. I did not find downgrade or cross-version acceptance in the tested transaction, capsule, sparse/matrix proof, terminal-metadata, or snapshot-boundary paths.

## Confirmed: P2P profiles collide across different fork schedules

`NetworkProfile` commits the genesis hash and one caller-supplied proof-bank ID, but no V1.1/V2/V2.1 activation heights, owner-authorization activation, block-time schedule, or V2/V2.1 bank identities:

- `parano1d/noid_p2p/src/network_profile.rs:27-52`
- `parano1d/noid_p2p/src/network_profile.rs:82-109`

The node supplies only the legacy pack metadata digest:

- `parano1d/noid_node/src/main.rs:2018-2020`

This is incomplete because V2/V2.1 bank identities separately include their fork schedule and authorization activation:

- `parano1d/noid_recursive/src/acceptance/history_step/v2/config.rs:289-335`

Three real feature builds at pinned `52938433a30b185ab007afaa15166a632cf2e82e`, using the same legacy bank ID, all produced:

- Profile ID: `600c7311c529b2a8547263851007924e5269f68881c1052bab4456c384fc6f85`
- Genesis/native chain ID: `860e70453390bf815718e933aa4927167a13d098b0151391eefd722ee1add610`
- P2P namespace: `/noid/mainnet/860e70453390bf81/1`

Yet their rules differ:

| Build | V1.1 | V2 | V2.1 / owner-auth | Rules at height 24 |
|---|---:|---:|---:|---|
| Default | 95125 | 210537 | 279370 | terminal v4, 4 classes, owner V3 |
| isolated-v2 | 5 | 10 | 279370 | terminal v7, 2 classes, owner V3 |
| isolated-v2.1 | 5 | 10 | 24 | terminal v7, 3 classes, owner V4 |

Exact evidence:

- `experiments/version-domains/results/network-default.json`
- `experiments/version-domains/results/network-v2.json`
- `experiments/version-domains/results/network-v21.json`
- `experiments/version-domains/results/network-comparison.json`

**Impact and scope:** mixed fork binaries pass the same transport preflight and can exchange incompatible transactions or verifier artifacts. The isolated builds are guarded to loopback-only namespaces by `noid_node/src/main.rs:1733-1767`, so the demonstrated impact is local fork/test isolation and denial of service, not remote mainnet proof acceptance. Full proof validation still rejects the cross-version cases tested below. A fix should bind the complete schedule and scheduled V2/V2.1 bank identities, potentially through height-versioned profiles.

## Negative verification results

- **Owner transactions/capsules:** real V3 and V4 PagedSpend proofs verified only under their selected versions. V3-as-V4 and V4-as-V3 both returned `AuthProof`; the default V4 API rejected V3. Bundle sizes were 89,988 and 89,124 bytes.  
  Evidence: `experiments/version-domains/results/authorization.json`.

- **Sparse proofs:** changed 32-byte context and same-shape/different-matrix key both failed with `Algebra`. A stale preprocessing-key pin was rejected.  
  Evidence: `experiments/version-domains/results/proofs.json`.

- **Matrix folds:** the reduction verified, and its outgoing claim matched the original matrix but not a same-shape substituted matrix. Relabeling the fresh claim to the second matrix also failed. The primitive remains deferral-based; callers must perform the final authenticated matrix evaluation.

- **Verifier artifacts:** safe `CompactFieldR1cs::open` rejected a stale digest with `StructuralDigestMismatch`; the correct digest succeeded. The legacy runtime cache directory is keyed by the full metadata digest at `noid_node/src/main.rs:1002-1008,1041-1046`.

- **HistoryStep metadata:** Native class 2 at height 24 was accepted only with V2.1 active at 24. Activation at 25 or no V2.1 activation rejected it, as did terminal-version downgrades to v4/v5. The same metadata was accepted under another V2 schedule, confirming metadata alone is not a bank/fork authenticator; full proof-bank verification remains necessary.  
  Evidence: `experiments/version-domains/results/terminal.json`.

- **Snapshots:** a foreign-header terminal was rejected with `BadHistoryStepTerminal("snapshot terminal does not bind its staged boundary")`, before the verifier closure was called. A wrong epoch anchor was also rejected.  
  Evidence: `experiments/version-domains/results/snapshot.json`.

- **Mempool downgrade handling:** source review found version changes during verification rejected at `noid_mempool/src/pool.rs:486-493` and cached entries evicted on authorization-profile changes at `noid_mempool/src/pool.rs:753-777`. I did not build a separate asynchronous mempool race PoC.

No same-anchor transaction replay was duplicated, and no live targets were contacted.

## Latest-origin comparison

Local `origin/main` is `e6d324ff70fa7d1da9965ba4221ad72c4aebc2e3`. The network-profile, activation, network-namespace, and terminal-metadata files are unchanged from the pinned revision, so the profile collision remains present there. Later retirement work adds certificate/artifact handling but does not alter this gap.

## Deliverables and verification

Created only under `experiments/version-domains`:

- `Cargo.toml`, `Cargo.lock`
- `src/main.rs`
- `src/network_profile.rs`
- `src/authorization.rs`
- `src/proofs.rs`
- `src/terminal.rs`
- `src/snapshot.rs`
- `results/{network-default,network-v2,network-v21,network-comparison,authorization,proofs,terminal,snapshot}.json`
- `pinned/`: archived source of the exact pinned commit, avoiding unrelated dirty-checkout edits

Validation completed successfully:

- `cargo check --offline`
- `cargo check --offline --features isolated-v2-fork-testnet`
- `cargo check --offline --features isolated-v2-1-fork-testnet`
- Release runs of every individual PoC and `cargo run --release --offline -- all`
- Cross-build JSON assertions comparing profile identity and schedules

Only the two pre-existing `noid_chain` dead-code warnings appeared. Generated `target/` and temporary snapshot files were removed.

**Not exercised:** production-size Small/Large/Native terminal generation or a complete snapshot segment install. Those conclusions are limited to source-level bank/origin binding review plus the metadata and boundary PoCs above.
