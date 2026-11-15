# agent-1: Audit state-tree equivocation

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Verdict

At commit `52938433a30b185ab007afaa15166a632cf2e82e`, I found **no protocol exploit against the exact UTXO state tree, state-transition proofs, segment roots, snapshots, or block acceptance from the reported feed-forward collisions/fixed points**.

The decisive reason is that the production exact-state tree does **not** use the vulnerable compressor. I also did not find a complete authorization forgery from CAPSNODE alone. The artifact proves a real primitive failure, but not two semantically valid committed vectors/states with one root or an accepted proof for an unauthorized statement.

A limited same-epoch fork replay condition does exist; it is not a blanket missing-`chain_id` vulnerability.

## 1. Exact UTXO state is unaffected by this attack

### Different compressor

The reported primitive is:

```text
Node_tag(a,b) = rate(P(a, b xor IV_tag)) xor a
```

implemented by `compress_flat_feed_forward_with_tag` at:

- `parano1d/noid_poseidon2b/src/native/compression.rs:311-341`

The exact UTXO node instead calls `compress_with_tag(TAG_EXSTNOD, ...)`:

- `parano1d/noid_chain/src/exact_state_hash.rs:31-34`

That function performs **two permutations**, with the second child absorbed between them:

- `parano1d/noid_poseidon2b/src/native/compression.rs:219-260`

The recursive circuit matches this two-permutation construction:

- `parano1d/noid_gkr/src/merkle_circuit.rs:4-10`
- `parano1d/noid_gkr/src/merkle_circuit.rs:40-45`
- `parano1d/noid_recursive/src/acceptance/trace/exact_state.rs:1285-1303`
- `parano1d/noid_recursive/src/acceptance/trace/paired_merkle_update.rs:4-11`
- `parano1d/noid_recursive/src/acceptance/trace/paired_merkle_update.rs:217-249`

The artifact’s `TAG_EXSTNOD` fixed-point check directly invokes the **flat feed-forward helper**:

- `benchmarks/src/bin/native-node-model.rs:116-120`

That check does not invoke production `state_node_hash`. Thus its `cross_tag_relabeling_checks` result is not an exact-state collision.

### `TAG_SEGMENTTREE` is dead as a tree implementation

`TAG_SEGMENTTREE` occurs only in its declaration and a re-export:

- `parano1d/noid_poseidon2b/src/native/domain.rs:95-97`
- `parano1d/noid_poseidon2b/src/native/mod.rs`

There is no production segment tree using that tag. The comment describing a segment-level global root is stale relative to the current exact-state implementation.

The legacy/raw segment-cache tree uses generic `compress`, also the two-permutation sponge construction:

- `parano1d/noid_chain/src/segmented_state.rs:258-265`
- `parano1d/noid_poseidon2b/src/hasher_impl.rs:38-40`

Raw segment commitments similarly use `Poseidon2bSponge` through `interleaved_commit_cap`, not CAPSNODE:

- `parano1d/noid_chain/src/fri_state.rs:230-255`
- `parano1d/noid_fri_binius/src/interleaved_commit.rs:596-639`

### Actual acceptance recomputes the safe exact root

Block application checks the header against a newly computed exact post-state root:

- `parano1d/noid_chain/src/block.rs:191-224`

The compact exact segment hierarchy also uses `state_node_hash` throughout:

- `parano1d/noid_chain/src/state.rs:181-214`
- `parano1d/noid_chain/src/state.rs:270-309`
- `parano1d/noid_chain/src/exact_segment_cache.rs:27-55`

Snapshot verification reconstructs exact segment and global roots from decoded slots and compares them to authenticated metadata:

- `parano1d/noid_chain/src/storage/snapshot_staging.rs:584-617`
- `parano1d/noid_chain/src/storage/snapshot_staging.rs:840-873`
- `parano1d/noid_chain/src/storage/snapshot_generation.rs:1302-1329`
- `parano1d/noid_chain/src/storage/snapshot_generation.rs:1489-1490`

Therefore, even a forged proof based on the flat compressor would not supply an alternate raw state accepted by native materialization.

## 2. Leaf validity and depth are pinned

### State leaves and transitions

A slot leaf commits the complete packed amount/creation ID and both owner lanes:

- `parano1d/noid_chain/src/exact_state_hash.rs:20-29`
- `parano1d/noid_chain/src/fri_state.rs:39-95`

The recursive transition relation enforces:

- spend: old leaf equals the claimed packed input and owner, new leaf empty;
- mint: old leaf empty, new leaf equals allocator-packed output and owner.

Evidence:

- `parano1d/noid_recursive/src/acceptance/trace/exact_state.rs:217-272`

Native execution independently requires an exact input match and an empty output slot:

- `parano1d/noid_chain/src/state.rs:1124-1142`
- `parano1d/noid_chain/src/state.rs:1167-1197`

There is no special subfield-aligned digest validity rule, but that does not help this attack because EXSTNOD uses the other compressor.

### Depth

Production exact-state depth is constrained to `24..=32`:

- `parano1d/noid_recursive/src/acceptance/trace/exact_state.rs:505-509`

The frontier’s `active_depth` comes from the block header:

- `parano1d/noid_block/src/history_step_witness.rs:743-767`
- `parano1d/noid_block/src/history_step_witness.rs:805-815`

The child depth must equal the parent depth or grow by exactly one, and the old root is bound to the appropriate expanded parent root:

- `parano1d/noid_recursive/src/acceptance/trace/exact_state.rs:1234-1315`
- `parano1d/noid_recursive/src/acceptance/trace/exact_state.rs:1347-1409`

Consensus independently enforces the expansion schedule and maximum:

- `parano1d/noid_chain/src/consensus/header.rs:257-262`
- `parano1d/noid_chain/src/consensus/slot_expansion.rs:24-61`
- `parano1d/noid_chain/src/consensus/params.rs:294-301`

Sparse multiproof topology, directions, indices, and sibling count are verifier-derived, not prover-selected:

- `parano1d/noid_chain/src/sparse_merkle.rs:416-528`
- `parano1d/noid_chain/src/sparse_merkle.rs:530-565`
- `parano1d/noid_recursive/src/acceptance/trace/exact_state.rs:113-169`

Thus a fixed point cannot be used to select a shorter or longer exact-state path.

## 3. CAPSNODE: real primitive failure, no completed protocol exploit

CAPSNODE is genuinely used by the authorization PCS:

- `parano1d/noid_fri_binius/src/capsule.rs:176-201`
- `parano1d/noid_fri_binius/src/zk_capsule_pcs.rs:897-912`
- `parano1d/noid_fri_binius/src/zk_capsule_pcs.rs:1025-1046`

Its source and mid path depths are fixed:

- source tree depth 13, cap depth 3, hence ten hashed levels;
- mid tree depth 9, cap depth 3, hence six hashed levels.

See `parano1d/noid_fri_binius/src/zk_capsule_pcs.rs:63-80` and `:108-129`.

The artifact demonstrates:

- 32 one-inverse-query fixed points `Node_tag(a,b)=a`;
- scalar/batch agreement;
- relabeling of the same fixed point under CAPSNODE and EXSTNOD.

Evidence:

- `benchmarks/src/bin/native-node-model.rs:87-143`
- `artifacts/native-node-model.json`

It also constructs 32 shaped source-leaf/internal-node digest aliases by zeroing the rate before the final leaf permutation:

- `benchmarks/src/bin/native-source-extraction.rs:51-80`
- `benchmarks/src/bin/native-source-extraction.rs:258-295`
- `artifacts/native-source-extraction.json`

These are valid primitive results, and they invalidate an unqualified assumption that CAPSNODE is collision-resistant.

They do **not** establish the requested protocol exploit:

- The fixed-point generator chooses `(a,c)=P⁻¹(0,z)`; it does not preserve an already existing valid leaf digest chosen by the attacker.
- The leaf/node alias supplies a shaped leaf payload and a node interpretation, but not two complete fixed-depth codewords or UTXO states with the same root.
- No artifact check produces an alternate `ZkAuthorizationProof` accepted for an unauthorized statement.
- No demonstrated witness connects the specially chosen child digests to all required valid leaves and then through the fixed-depth source/mid paths.

Accordingly, I would report CAPSNODE as a **serious unresolved primitive/security-assumption failure**, not as a proven unauthorized spend or alternate-state attack.

## 4. Replay concern: confirmed only under narrow fork conditions

Base `TxBody` has no independent chain ID:

- `parano1d/noid_tx/src/types.rs:89-102`

Owner authorization receives only body hash and expected owner:

- `parano1d/noid_gkr/src/owner_auth.rs:37-50`
- `parano1d/noid_gkr/src/owner_auth.rs:81-94`

However, `epoch_anchor` is included in the body hash:

- `parano1d/noid_tx/src/body_hash.rs:31-35`

Every user transaction must equal the canonical epoch block ID:

- `parano1d/noid_chain/src/consensus/epoch_anchor.rs:57-90`
- `parano1d/noid_recursive/src/acceptance/block_slots.rs:1565-1604`

Therefore:

- Different-genesis networks do not freely replay these transactions.
- Same-chain replay after spending is stopped by exact input/creation-ID checks.
- **Two forks sharing the epoch anchor can replay a transaction and its authorization while the claimed inputs remain unspent and output slots remain empty on the destination fork.** This can last until their epoch anchors diverge.
- Native assets’ `native_chain_id()` is the genesis hash, so it also does not distinguish two forks sharing genesis: `parano1d/noid_chain/src/state/native_assets.rs:24-27`.

This is a real, conditional chain-split replay property, not evidence of arbitrary cross-network replay or an authorization bypass on one canonical chain.

## Best local reproduction

From the supplied artifact, with the checkout at the pinned commit:

```bash
cd /Users/ember/paperbin/zenodo_23778742_signatureless_auth/artifact/signatureless-authorization-artifact

python3 benchmarks/run.py native-model \
  --source /Users/ember/dev/parano1d-break/parano1d

python3 benchmarks/run.py native-extraction \
  --source /Users/ember/dev/parano1d-break/parano1d
```

Expected recorded results are in:

- `artifacts/native-node-model.json`: 32 native and 32 batch fixed points, 32 cross-tag checks.
- `artifacts/native-source-extraction.json`: 32 forced source-node digest aliases.

The launcher authenticates the source revision and file hashes at `benchmarks/run.py:31-45`. It writes build files under the checkout’s `target/` and rerun JSON under `data/` (`benchmarks/run.py:47-50`, `:88-91`), so I did not execute it during this read-only audit. The checkout remained clean at the pinned commit.
