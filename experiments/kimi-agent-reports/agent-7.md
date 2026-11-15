# agent-7: Demonstrate fork replay

(A final message from the kimi session wire; status may be completed OR died mid-lane)

Implemented a runnable, self-contained demonstration in `experiments/fork-replay`.

### Run

```sh
cd /Users/ember/dev/parano1d-break/experiments/fork-replay
./run.sh
```

The harness uses the actual pinned transaction encoding, public logic, V4 owner-auth prover/verifier, epoch-anchor validation, and state-delta verification. Required pinned crates are vendored under `pinned/`, so later shared-checkout changes cannot affect the experiment.

### Demonstrated result

- One 323-byte transaction and one serialized owner-auth bundle are reused across two fork contexts.
- The forks have distinct parent IDs and distinct pre-state roots but share epoch block 279504.
- Both independently pass epoch, public-logic, owner-auth, and UTXO state-transition checks.
- Replaying again on fork A’s updated state fails with `InputMismatch`.
- Changing the epoch produces `BadEpochAnchor` and `AuthProof`.

Exact stdout, also saved in `experiments/fork-replay/output.txt`:

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

### Relevant pinned code

- `noid_tx/src/types.rs:89-106`: `TxBody` has an epoch anchor but no chain/fork ID.
- `noid_tx/src/body_hash.rs:30-63`: the epoch anchor and body fields are hash-bound.
- `noid_tx/src/owner_auth.rs:29-40`: owner authorization contains only `(tx_body_hash, input_owner)`.
- `noid_gkr/src/wallet_authorization.rs:283-305` and `:642-659`: proving and verification derive that statement without chain context.
- `noid_block/src/history_step_witness.rs:575-586`: block authorization public inputs likewise use transaction hash and owner.
- `noid_chain/src/consensus/epoch_anchor.rs:57-117`: user transactions bind the canonical epoch block ID; only coinbase binds the immediate parent.
- `noid_chain/src/state_delta.rs:213-314` and `:454-493`: state verification requires the claimed unspent input and empty output slots, then applies the transition.

Thus, the anchor blocks arbitrary cross-epoch replay but does not distinguish forks sharing that anchor. Independent pre-state checks allow the exact transaction to succeed once on each such fork.

### Latest origin

`git ls-remote origin refs/heads/main` returned `e6d324ff70fa7d1da9965ba4221ad72c4aebc2e3`.

The transaction, authorization, epoch, state, and block-history files above are unchanged from the pinned commit. Post-pin additions such as `fork_origin_carrier.rs` and creation-header lookup do not add a transaction chain domain or alter these predicates. Latest origin therefore does not change the conclusion.

### Limitations

- This is a local composition of real validation APIs, not full node/block acceptance: no PoW, chainwork, transaction roots, recursive HistoryStep terminal, mempool admission, nodes, or peers.
- It demonstrates exact replay of an existing transaction/proof, not transaction modification, proof creation without the owner’s witness, arbitrary cross-chain replay, or fresh-request EUF-CMA forgery.
- Competing-fork validity is not a double-spend within one linear state or two finalized canonical spends.

### Files and verification

Created:

- `experiments/fork-replay/Cargo.toml`
- `experiments/fork-replay/Cargo.lock`
- `experiments/fork-replay/run.sh`
- `experiments/fork-replay/src/main.rs`
- `experiments/fork-replay/README.md`
- `experiments/fork-replay/output.txt`
- `experiments/fork-replay/pinned/Cargo.toml`
- `experiments/fork-replay/pinned/{noid_core,noid_poseidon2b,noid_fri,noid_fri_binius,noid_tx,noid_chain,noid_gkr}/**`

Verification passed:

- `cargo check --release --locked`
- `cargo fmt -- --check`
- Repeated `./run.sh` executions with byte-identical stdout
- README transcript matched `output.txt`

Only two pre-existing `noid_chain` dead-code warnings appeared on stderr. Build artifacts were removed. Unrelated tracked modifications that appeared concurrently in the shared checkout were left untouched.
