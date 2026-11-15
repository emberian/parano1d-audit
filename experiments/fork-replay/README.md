# Same-epoch fork replay proof

This is a local, deterministic demonstration against Paranoid commit
`52938433a30b185ab007afaa15166a632cf2e82e`. It uses the production transaction
encoding, public-logic derivation, V4 wallet owner authorization, epoch-anchor
validation, and native state-delta verifier. It starts no node and connects to
no peer.

## Run

```sh
cd /Users/ember/dev/parano1d-break/experiments/fork-replay
./run.sh
```

The seven required crates are vendored under `pinned/` directly from
`git archive 52938433...`; `pinned/Cargo.toml` only supplies their minimal
workspace metadata. Builds therefore do not depend on later changes in the
shared source checkout. `run.sh` uses `--locked` and keeps Cargo output in this
experiment's `target/` directory. The captured stdout is in `output.txt`.

## Construction and result

The fixture creates a shared epoch header at height 279504 and two distinct
parent headers at height 279505. The parent IDs and state roots differ. Both
candidate children are at height 279506, so production selects owner-auth V4
and resolves the same epoch block ID for both children.

One canonical transaction spends slot 3 `(amount=100, creation_id=7)` into
slot 9 `(amount=99)`, with fee 1. Its 323 wire bytes are decoded independently
for the two fork contexts. One proof is produced with the legitimate owner's
secret, serialized and decoded, and the exact same bundle is verified in both
contexts. Each fork has the claimed input and an empty output slot, but they
also contain different unrelated UTXOs; hence even their pre-state roots are
different.

For each fork, `verify_in_fork` in `src/main.rs` composes these real pinned
APIs:

1. `resolve_user_epoch_anchor_id` and `validate_block_epoch_anchors` on a block
   containing a parent-bound coinbase and the replayed user transaction;
2. `validate_public_tx_logic` and `canonical_owner_auth`;
3. `verify_wallet_authorization` on the same decoded proof bundle;
4. `build_state_delta_witness`, followed by checks of the consumed input,
   created output, action counts, and post-state root.

Both compositions succeed. This is the narrow replay condition: the same body
and authorization are valid on two forks while those forks share the required
epoch anchor and each pre-state independently satisfies the UTXO predicates.

Two controls bound the claim:

- Reusing the body on fork A after its successful transition still passes the
  context-free owner check but fails state verification with `InputMismatch`.
- Changing the epoch anchor produces `BadEpochAnchor`, and the original proof
  no longer verifies the changed body (`AuthProof`).

## Exact stdout

Two consecutive `./run.sh` executions produced byte-identical stdout; `diff`
reported no differences. Cargo can emit two pre-existing `noid_chain` dead-code
warnings on stderr, which are not part of this stdout transcript.

```text
pinned_commit=52938433a30b185ab007afaa15166a632cf2e82e
heights: epoch=279504 parents=279505 children=279506 owner_auth=V4
transaction: wire_bytes=323 txid=767d876c015691e73a292e9b65a9c8791f9e5b2e70cbb9aff1e3c158e4ef1622
public_logic: inputs=1 outputs=1 input_sum=100 output_sum=99 fee=1
epoch_anchor=b36673d93a6df346e44f22f454941023fd69159ef42f887b6fdf3382127dd4be
input_owner=85b4384ed4636e699aa6de5e9567fd93c76febeb39ea8bf3d630d335dbc351e0
proving one owner authorization with the legitimate wallet witness
owner_authorization: wire_roundtrip=ok same_bundle_reused=true
parent_A=813f13f6edf7e63d20528e6c46eff4d5d985f19d68bf4add11451bbcdd376676
parent_B=967489114d391df91d303ddd56beb6be14d8dc0d76fc584a2293b2b9f65c5746
fork_A: epoch_anchor=ok public_logic=ok owner_auth=ok state_delta=ok spends=1 mints=1
fork_A: pre_root=a3d0488156abdb1878814d1ee930627953e9cff9fc16e0a32a3f09ea291af7b4
fork_A: post_root=ad9b6092ea99e36eeda553bc8f3f1c6fbc91593414121846bcc7f89039a945f5
fork_B: epoch_anchor=ok public_logic=ok owner_auth=ok state_delta=ok spends=1 mints=1
fork_B: pre_root=9f111520ed0cbb53717e1c6d6ea82400fc8864df9bde35c801c8aadc96edd64f
fork_B: post_root=8546bdc2af55c8a3bfc8a8a9034f146780d7e7c88acf0d6b1c90c70f4adba749
replay_result: same_transaction=true same_proof=true both_fork_context_checks_passed=true
same_fork_second_spend: owner_auth=ok state_delta=InputMismatch { tx_index: 0, input_index: 0 }
changed_epoch_control: epoch_check=BadEpochAnchor owner_auth=AuthProof
scope: exact transaction/proof replay across same-anchor forks; no new proof forged
```

## Why the pinned code permits this

Paths below are relative to `/Users/ember/dev/parano1d-break/parano1d` at the
pinned commit.

- `noid_tx/src/types.rs:89-106` defines every `TxBody` field. There is an epoch
  anchor but no chain ID, genesis ID, fork ID, or parent block ID for a user
  transaction. `Transaction` contains exactly one body (`:181-198`).
- `noid_tx/src/body_hash.rs:30-63` shows that the body hash commits to the
  epoch anchor and all body fields. Thus the anchor is transaction-binding,
  but it can only distinguish contexts whose expected anchors differ.
- `noid_tx/src/owner_auth.rs:29-40` reduces canonical owner authorization to
  exactly `(tx_body_hash, input_owner)`.
- `noid_gkr/src/wallet_authorization.rs:283-305` derives proving inputs from
  that statement, and `:642-659` independently reconstructs the same statement
  during verification. Neither API accepts chain/fork context.
- `noid_block/src/history_step_witness.rs:575-586` likewise constructs the
  block authorization public inputs from logical transaction hash and input
  owner, not a separate chain domain.
- `noid_chain/src/consensus/epoch_anchor.rs:57-68` derives the epoch-header
  height. `:70-90` requires user bodies to equal the supplied epoch anchor;
  only coinbase bodies bind the immediate parent ID. `:110-117` resolves that
  anchor through canonical header lookup.
- `noid_chain/src/state_delta.rs:213-314` checks that an input equals the
  current slot value and an output slot is empty. `:454-493` applies the same
  relation atomically and returns pre/post roots. It has no transaction-level
  replay registry or chain-domain check.

Consequently, the epoch anchor prevents arbitrary cross-epoch replay but does
not distinguish sibling forks sharing that anchor. Independent pre-state UTXO
checks are what allow the exact transaction to be valid once on each fork.

## Latest origin comparison

`git ls-remote origin refs/heads/main` returned
`e6d324ff70fa7d1da9965ba4221ad72c4aebc2e3`. The experiment itself remains
pinned to 52938433.

`noid_tx`, `noid_gkr`, `noid_block`, `noid_chain/src/state.rs`,
`noid_chain/src/state_delta.rs`, and `noid_chain/src/consensus/` have no diff
between the pinned commit and that `origin/main`. The post-pin `noid_chain`
changes add `fork_origin_carrier.rs` (proof-origin transport compression) and
`storage/mdbx_store/creation_headers.rs` (creation-time header lookup), with
their module/dependency wiring. They do not add a transaction chain domain or
change the demonstrated authorization, epoch, or state predicates. Therefore
latest origin does not change this analysis.

## Limitations

- This is a synthetic local composition of real validation APIs, not a full
  node acceptance test. It does not construct valid PoW, chainwork, a complete
  canonical ancestry, transaction roots, a recursive HistoryStep terminal, or
  mempool admission. The candidate block headers exist to exercise the real
  epoch predicate; the state proof is for the user body, not the coinbase.
- It demonstrates exact replay of an already authorized transaction and proof.
  It does not demonstrate modification of a transaction, creation of a proof
  without the owner's witness, arbitrary cross-chain/cross-epoch replay, or a
  fresh-request EUF-CMA forgery.
- It does not permit two spends in one linear state: the same-fork control
  fails after the input is consumed. Competing-fork validity is distinct from
  both spends becoming canonical under final fork choice.
