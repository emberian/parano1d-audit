# Financial opcode boundary verification

Source: `experiments/comp-system-hunter/pinned/`, a read-only copy of `parano1d` commit `52938433a30b185ab007afaa15166a632cf2e82e`. Tests use only local fabricated headers, state, keys, and transactions. No public network was contacted.

Run with:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=../comp-system-hunter/core-seams/target cargo test --offline --lib -- --nocapture
```

The test crate uses the pinned source, not the mutable `parano1d/` working tree. Its metadata test reads the archived `v21-runtime-metadata.bin` and recomputes the release bank digest `6c280ae869afc25e31d46d73b42870512991b78c9498bc7e5ef939bc07e5ed5e` before checking class parameters.

| Class | Outer m | Pages | Inputs | Contracts | Native headers | Finance | Sequential trades |
| --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| Small | 23 | 63 | 504 | 63 | 0 | None | None |
| Large | 24 | 206 | 504 | 63 | 0 | None | None |
| Native | 24 | 63 | 504 | 63 | 5 | 128 actions, 34 settlement actions, 9 definitions | 4 markets, 48 policies, 64 funding |

All three classes select the v2.1 authorization activation at height 279,370. Small and Large represent zero native headers as `Some(0)` in the decoded config.

The source profile enables financial codes `33, 38, 39, 40, 41, 43, 49, 50, 51, 52, 53` that the sequential Native class excludes. This test constructs a complete native-valid fixture for code 49 paired with code 50. It does not establish that each of the other codes can form a complete valid block.

## Observed boundary for op 49

The fixture creates a launch (op 48), then forms a buyer's `LaunchRequest` (op 49) and its required `RegisterLaunches` suffix (op 50) in a second block. The buyer's V4 owner authorization is generated and verified using the production `noid_gkr` API. The test passes the full op49+50 block through these boundaries:

| Boundary | Observed result | What it establishes |
| --- | --- | --- |
| `NativeIntent::from_financial_claims` and `prove_native_financial_authorization` | Accept and verify op49 owner proof | The request is a typed, owner-authorized intent. |
| `noid_chain::consensus::validation::validate_block_checks_with_schedule` | Accept with `check_pow = false` | Header target, page stream, resource, fee, and coinbase checks accept the synthetic block; no mining is claimed. |
| `materialize_accepted_block_state_with_financial_context` | Accept | Typed native financial semantics accept the complete request/registration pair and produce its claimed post-state. |
| `noid_chain::consensus::template::build_node_owned_block_template_with_native_data` | Reject op49: `financial operation outside the template profile` | The node's normal local template path cannot include the request. |
| `noid_block::candidate_history::prepare_candidate_input_with_native_data::<63>` | Reject: `native operation is outside the selected class` | The Native HistoryStep candidate cannot be prepared for this block. |
| `bind_financial_sequential_headers` R1CS relation | Accept selector 48; reject selector 49 | The actual sequential-header gate also excludes op49. This is a gate unit test, not a full proof. |
| `AsyncMempool::submit_encoded` with real op49 owner proof | Reject: `financial operation is outside the Owner admission profile` | Ordinary mempool admission rejects before proof verification. |

A lone op49 request fails native materialization with `Financial(Registration)`: registration is mandatory, and the group order must place op50 after funding. This is why the test uses a complete pair.

The tests show a mismatch between the broad native operation profile and the restricted sequential proof class. They do **not** show a reachable chain halt. The production accepted-bundle path verifies a HistoryStep terminal before state commit (`noid_chain/src/storage/mdbx_context.rs:1402`, `noid_node/src/main.rs:4239-4263`), and the locally tested candidate is rejected before a terminal can be made. We did not submit a block to the live network, produce a terminal for op49, or demonstrate an MDBX commit of the frozen launch state.

## Earlier fixture corrections

Gemini's `comp-consensus-diff` op49 test had not compiled or run. Its copied setup failed in sequence on an invalid 32-page V2Config, native tagged IDs passed to the non-height-aware sparse state constructor, an arbitrary difficulty target, an op49 request without its required op50 registration, and an epoch anchor at the wrong height. Its standalone one-page paged-spend R1CS probe then panicked because one page is not a supported proof tier. The current test uses a valid 63-page Native config, `from_sparse_utxos_at`, the scheduled ASERT target, an op49+50 pair, the genesis epoch anchor for this local height, and a narrow direct R1CS gate test.

The synthetic parent state and headers are locally consistent for the tested APIs, but are not a mined historical chain. The test does not prove that a public-network node would ever receive or accept this block.
