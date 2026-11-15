# agent-10: Test typed binding delta

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Conclusion

At pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`, the supplied CAPSNODE collision **does not by itself rigorously instantiate `BadTypedBind` or refute `Delta_P2b < 0.45067`**.

The distinction is:

- `BadTypedBind` is a **semantic graph/binding event**, not merely equality of two compression outputs (`noid_soundness/docs/category-one.md:313-327`; `v2-correspondence-2026-10.md:131-133`).
- The fixed experiment additionally requires typed domains, **canonical nested-artifact recording**, and native/recursive verifier gates (`category-one.md:847-849`). A detached chosen-input compression call does not demonstrate that two protocol-represented artifacts have different semantics in the measured database `D`.
- **Terminal or genesis reachability is not additionally required.** The all-root event is explicitly evaluated before ancestry traversal, and reachability is used only afterward (`category-one.md:329-340`). Thus an accepted invalid terminal State or a complete from-genesis attack is unnecessary for a `BadTypedBind` witness.
- For a capsule Merkle-tree witness, the missing semantic object is a pair of **protocol-valid represented subgraphs**: valid leaf/descendant openings for the colliding children and represented roots/canonical artifacts containing the collision. A full invalid terminal State is not required.

The exact missing data for the supplied pair are protocol-valid openings for its four child digests—under the applicable `CAPSMIX`, `CAPS256`, or `CAPSLEAF` leaf construction, or valid internal-node descendants—together with canonical represented artifacts demonstrating the binding change. For the selected wallet source tree, those openings must also fit the canonical codeword/relation, not merely hash to the right digest.

## Implemented witness

Created only in `experiments/typed-binding`:

- `Cargo.toml`
- `Cargo.lock`
- `src/main.rs`
- Generated build artifacts under `target/`

The program lifts the collision into two distinct two-leaf subtrees and extends them through identical CAPSMIX-backed sibling subtrees along the selected wallet’s actual ten-level source-to-cap path (`source_tree_depth=13`, `source_cap_path_depth=10`).

It verifies:

- The original CAPSNODE collision.
- Four accepting paths using the real native `verify_merkle_path` with `CapsuleNodeHasher`.
- Four matching roots using the real recursive `build_ff_merkle_path_columns` trace replay.

This is a genuine **Merkle-layer typed-graph collision**, but not full wallet/History verification and not a demonstrated fixed-experiment `BadTypedBind`: no leaf openings for the four colliding children are supplied.

Command run:

```sh
cargo fmt --check --manifest-path experiments/typed-binding/Cargo.toml
CARGO_TARGET_DIR=experiments/typed-binding/target \
  cargo run --release --locked \
  --manifest-path experiments/typed-binding/Cargo.toml
```

Result:

```text
capsnode_collision=b2cf54a478131d318ebef1af2ec0e315245b8ee3af237a21e3fef04e4a20478b
native_paths_accepted=4
shared_source_cap_node=eea0409154dda13fce3a07a24dbf967d43a0e142e9fc27548674c0155b378891
recursive_trace_roots_matched=4
source_tree_depth=13
source_cap_path_depth=10
common_siblings_have_capsmix_leaf_openings=true
colliding_child_leaf_openings_supplied=false
full_wallet_or_history_acceptance_demonstrated=false
bad_typed_bind_instantiated_by_supplied_artifact=false
delta_violation_proven=false
```

Relevant production files and documentation have no diff from the pinned commit. Unrelated concurrent working-tree edits appeared elsewhere in `parano1d`; I did not modify them.

## Strongest defensible wording

> I exhibit a fixed-Poseidon2b CAPSNODE collision lifted to four accepting production-shaped Merkle authentication paths sharing one source-cap node. I do not yet exhibit `BadTypedBind` or a `Delta_P2b` violation, because the colliding child digests lack protocol-valid leaf/descendant openings and canonical represented wallet/History artifacts demonstrating a semantic binding change. Terminal-State reachability is not required by the all-root definition; represented protocol semantics are.

No V16/qPRP distinguisher was used, and the current artifact supplies no lower bound on `Pr[BadCompiler]_fixed` sufficient to infer a Delta violation.
