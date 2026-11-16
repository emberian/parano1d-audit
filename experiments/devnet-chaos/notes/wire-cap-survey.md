# Phase 4 cap-route survey (cross-version claw reach through the P2P wire)

Question: can any node-to-node exchanged or serialized artifact carry a
cap/commitment blob that a different-dialect node's reader consumes, such
that the identical commitment bytes could be interpreted in the wrong domain
(C21's TAG_CAPSNODE <-> IVCPCSN_ claw)?

## Wire inventory (pinned commit)

Every 32-byte field crossing the P2P wire (noid_p2p/src/protocol.rs:75-221,
plus opaque blobs in object/history-step/mempool codecs):

| Field | Where | Domain that produced it | Receiver consumption |
|---|---|---|---|
| block_hash / tip_hash | header records, terminal req/resp | BLOCKHDR/SEMHDR ids | structural compare vs computed header ids |
| cumulative_chainwork | header records | PoW accumulator | work accounting |
| state_root, segment_roots | manifest/headers | exact UTXO tree = TAG_EXSTNOD two-permutation compressor (NOT a claw domain) | exact-root recomputation |
| manifest_digest | manifest | poseidon2b_hash_bytes over manifest params | recomputed by computed_manifest_digest |
| known_txids | mempool requests | content ids | set membership |
| byte_digest (object ids) | availability/objects | content hash | content-addressed fetch + verify |
| intent bytes (mempool/gossip) | PAGEDTX_ domain | full tx validation incl. authorization proof |  |
| block bytes / terminal bytes (object codec) | opaque | header+body+HistoryStep terminal | semantic block/terminal verification |

## Caps DO cross the wire — inside authorization proofs

`ZkAuthorizationProof` embeds `MerkleCap` / `SourceBatchedMerkleProof`
structs, and these serialize into transaction intents (hence mempool sync,
gossip, blocks): the wire layout scanner walks
`source_commitment.cap.hashes: Vec<[u8; 32]>` as the first vector
(noid_gkr/src/zk_authorization_wire.rs:296-297). So cap bytes DO move
peer-to-peer inside every spend.

## Why that closes the cross-version claw route

Every consumer of those wire-supplied caps goes through the semantic capsule
verifier, which recomputes leaf hashes and pins tree/cap depths BEFORE any
raw cap check:

- noid_fri_binius/src/zk_capsule_pcs.rs:999-1041: builds `source_hashes` by
  `capsule_leaf_hash_mixed(source_leaf)` / `capsule_leaf_hash_wide(mid_leaf)`
  (leaf hashes recomputed from actual leaf symbols), enforces the
  source->mid->tail fold equations, then calls
  `verify_source_batched_merkle_proof_to_cap(..., ZK_CAPSULE_PCS_SOURCE_CAP_DEPTH,
  ..., ZK_CAPSULE_PCS_SOURCE_TREE_DEPTH, ..., &CapsuleNodeHasher)` with
  depth constants pinned at :66-69.
- Same pattern at noid_fri_binius/src/capsule.rs:828-839.
- The raw cross-domain comparison caller does not exist in-tree
  (leaf-reachability lane sweep; the tower-digest bridge is #[cfg(test)],
  noid_gkr/src/zk_auth_qrom.rs:1999-2004).

Therefore identical-bytes commitments (the C21 object) can only be
distinguished-from or confused-by a consumer that feeds cap bytes DIRECTLY to
`verify_source_batched_merkle_proof_to_cap` or compares digests across
domains — and no node-to-node path does. The wire's caps die at the leaf
wall: an attacker cannot supply "the same cap" with different leaf semantics
because the verifier rebuilds every leaf hash from symbols it separately
validates through the fold equations.

## Verdict

Cross-version claw route via P2P artifacts: CLOSED. Boundary condition: an
EXTERNAL consumer of the public raw APIs (SDK/bridge/index keyed by cap
bytes) would reopen it — identical scope to C21 itself.
