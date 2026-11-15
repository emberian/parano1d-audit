# Wallet/GUI/key and secret-handling audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. The Rust harness compiles the node's real `noid_node::wallet` module in via `#[path]`, builds intents with the production wallet builder and GKR prover, and validates them with the production verifier. The Python harness asserts the pinned source shape and drives the real node binary's wallet-maintenance CLI with synthetic secrets. No live node, wallet, peer, RPC server, or real user key file was used; every secret below is synthetic.

Prior-lane note: a dead agent left this harness unverified. Its source assertions were independently re-verified line-by-line against the pinned commit before being trusted; three of its Rust tests were kept, and its hand-rolled divergence test was replaced with production-builder equivalents. All wallet-relevant files (`noid_rpc/src/server.rs`, `noid_rpc/src/types.rs`, `noid_gui/src/{app,backend,secret}.rs`, `noid_node/src/wallet/*`, `noid_poseidon2b/src/primitives.rs`, `noid_gkr/src/wallet_authorization.rs`) are clean against the pin; the pre-existing working-tree modifications are rustfmt-only and in files outside this surface.

## Finding 1: nothing downstream of the builder can detect a recipient/amount substitution

**Severity: Low (hardening / defense-in-depth). No divergent production path was found; see preconditions.**

The ordinary payment path (`paranoid_walletSend`) builds and signs node-side from the request, then verifies its own build — but every human-visible artifact mirrors the *request*, not the *signed intent*:

1. The post-build divergence gate (`noid_rpc/src/server.rs:1164-1167`) checks only `actual_fee != fee_micronoid || input_count != expected || output_count != expected`. Recipient and amount are not checked.
2. `WalletSendResult` (`noid_rpc/src/types.rs:453-459`) has no recipient or intent field, and its `amount_micronoid` is assigned the *request* variable (`server.rs:1202-1208`), not a value re-derived from the signed pages.
3. The GUI success screen renders `PaymentSubmission.recipient` — the local request string (`noid_gui/src/backend.rs:820-827`, `noid_gui/src/view/present.rs:1994-2046`) — and the wallet's pending history is recorded from the request (`reserve_pending_send(..., to_address)` at `server.rs:1174-1187`), then confirmed by txid match only (`noid_node/src/wallet/scanner.rs:184-248`).

The PoC builds two intents through the real `build_send` pipeline (`extract_build_data` + `build_and_prove_tx_for_version`, the exact call chain of `noid_node/src/wallet/mod.rs:1416-1461`) from the same funded wallet: same fee, same live input/output counts, different recipient and amount. Both pass the real GKR authorization and the exact server gate condition; the wire result cannot express the difference; only the txid differs.

**Preconditions and limits:** within the pinned code the request's recipient/amount are passed straight through the builder into the signed page (verified by the control test below), so there is no reachable mismatch today. The gap matters only if a builder regression, a compromised node binary, or a divergent fork produces an intent that differs from the request while preserving fee and counts — then neither the gate, the JSON-RPC result, the GUI success screen, nor the wallet history would reveal it; the only binding artifact is the txid. The codebase already demonstrates the stronger pattern elsewhere: native-asset submissions bind `plan_id`/`txid`/`fee` against the reviewed preview (`noid_gui/src/assets/operations.rs`, `finish_submit`), and `wallet_send_reviewed` supports an `expected_sender` binding — but the public `walletSend` passes `None` (`server.rs:2873-2881`) and the GUI never calls `walletPlanSend`.

### Recommended correction

Echo a digest (or the recipient and amount) re-derived from the signed pages in `WalletSendResult`, and extend the gate condition to compare them against the request — the facts to do this are already computed by `validate_paged_spend` and the decoded intent is in scope at the check site. Optionally have the GUI call `walletPlanSend` and pass `expected_sender`, mirroring the native-asset flow.

## Finding 2: exported master secret persists in the system clipboard

**Severity: Low. Precondition: the user opens Export and presses Copy.**

`Message::CopyExportedSecret` writes the master secret to the system clipboard (`noid_gui/src/app.rs:2373-2377`). Closing the dialog clears the GUI-side `exported_master_secret` (`SensitiveString`, zeroizing) but no clipboard write ever clears or overwrites it — there are exactly three `iced::clipboard::write` sites in the GUI (generic value, address, secret) and no clearing logic or documented exposure window. Any application on the machine can read the secret for as long as it remains the clipboard content. On macOS/Wayland there is no portable clear API in iced, so this is partly accepted platform design; a timed overwrite with an innocuous value after the dialog closes (or after N seconds) would bound the window.

## Finding 3: an in-memory wallet survives an on-disk master-secret replacement (library-level)

**Severity: Informational. No production path found.**

`import_generated_master_secret` (`noid_node/src/wallet/state.rs:620-767`) atomically replaces the key file and moves old artifacts aside, but nothing can revoke a `WalletState` already loaded in a process: a stale handle keeps the OLD master secret and still builds validly-authorized intents with it, and its later writes (e.g. `record_pending_send`) land in the NEW wallet's history journal, producing history entries attributed to the old address (demonstrated by the test). In the pinned code this is unreachable: import runs only in the wallet-maintenance CLI, which exits before the daemon starts (`noid_node/src/main.rs:1995-2005`); the GUI stops its owned node before importing and refuses entirely when the node is externally managed (`noid_gui/src/backend.rs:575-579`). It is a footgun for future library consumers, not a live defect.

## Negative and control results

- **Honest-path binding (control):** a payment built by the production builder from request (recipient R, amount A) signs exactly `outputs[0] = (R, A)` with change to the active address, passes the real GKR verification and the server gate. Displayed == signed within honest code.
- **Authorization binds outputs:** tampering with the recipient of a hand-assembled page after proving makes `verify_paged_spend_authorization` fail.
- **No secret in wire artifacts:** the serialized intent contains no window equal to the 32-byte synthetic secret (harness test); the node's own prover tests additionally pin this for the authorization bundle.
- **Secret typing hygiene:** `SpendSecret`, `OwnerAuthWitness`, `MasterSecret` and the GUI's `SensitiveString` are zeroize-on-drop, have no `Clone`/`PartialEq`/`Serialize`/`Deserialize` (pinned by static assertions in this harness and in-tree), and their `Debug` renders redacted. `SpendSecret`'s only cross-crate seam (`with_exposed_prover_fields`) zeroizes its field limbs around the closure.
- **Keystore file handling (live CLI repro against the real binary, synthetic secret):** created `wallet.key` is 48 bytes with mode 0600; export round-trips the secret exactly; loading through a symlink at the key path is rejected without leaking the secret in output. In-tree, `load_plain` additionally enforces 0600 + current-euid ownership and opens with `O_NOFOLLOW|O_CLOEXEC`; creation uses `create_new` + atomic hard-link; import stages a 0600 key, validates it by loading, swaps under a 0700 rollback directory, and deletes the old artifacts.
- **The master secret never transits RPC:** import/export run via the node maintenance CLI with the secret on stdin (never argv, so not visible in `ps`), stdout captured by the GUI; there is no RPC method that installs or returns a master secret (`walletExportReceipt` is receipts, not keys). The only secret `println!` in the tree is the deliberate export command.
- **RPC authorization:** loopback TCP peers get `RpcAccess::Local` (headers deliberately irrelevant); a non-loopback listener without a configured credential fails closed at startup; the mining credential is restricted to `getBlockTemplate`/`submitBlock`; the operator credential's spend-capable allowlist (`walletSend`, `walletConsolidate`, ...) is explicit accepted design. GUI<->node is plain loopback HTTP — same trust domain as the 0600 key file, and no authorization confusion between scopes was found.
- **GUI-written files** (settings, address labels) are created 0600 via atomic temp-file + rename with directory syncs.
- **Photo-derived secrets** hash canonical decoded pixels only (container metadata does not change the key), with minimum-dimension and 64-distinct-pixel entropy gates and a bounded preview; low-entropy sources are rejected. Weak-photo entropy remains a user choice, documented as the intended mechanism.

## PoC implementation and reproduction

- `src/lib.rs` — compiles the node's real wallet module into the harness.
- `tests/repro.rs` — five tests: authorization-binds-outputs and wire-bytes control; honest-path builder control; gate/result blindness PoC (production builder); secret-typing/zeroization assertions; stale-wallet-after-import demonstration.
- `source_repro.py` — pinned-source assertions for the GUI send flow (no plan/review binding on ordinary sends; success screen uses request values; native flow's binding present), the RPC gate/result shape, the clipboard behavior, and a live CLI keystore round-trip (0600, 48 bytes, symlink rejection) using `PARANO1D_BIN`.

From this directory:

```bash
cargo fmt --check
CARGO_TARGET_DIR="$PWD/target/cargo" cargo test --test repro 2>&1 | tee results.txt
PARANO1D_BIN="$PWD/target/cargo/debug/parano1d" python3 -I source_repro.py 2>&1 | tee -a results.txt
```

(`target/cargo/debug/parano1d` is the node binary built from the pinned tree by the prior lane and reused here; rebuild with `CARGO_TARGET_DIR="$PWD/target/cargo" cargo build -p parano1d` inside the parano1d workspace if needed.)

Final output includes:

```text
running 5 tests
test secret_owners_stay_opaque_redacted_and_zeroizing ... ok
test authorization_binds_outputs_and_wire_bytes_omit_the_synthetic_secret ... ok
test production_builder_signs_exactly_the_requested_payment ... ok
test import_does_not_revoke_a_loaded_wallet_or_isolate_replacement_artifacts ... ok
test divergence_gate_and_result_cannot_detect_recipient_or_amount_substitution ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 45.05s
...
Ran 7 tests in 0.167s
OK
```

## Assurance limits

- No divergent build path exists in the pinned code; Finding 1 is demonstrated as blindness of the gate/result/display contract, not as an end-to-end attack. No compromised-node or cross-process scenario was executed.
- The gate-blindness PoC uses one funded UTXO, one page, V4 authorization, and fixed slot hints; multi-page spends and consolidation were reviewed in source but not exercised in the PoC. The consolidation and object-funding paths pass their amounts through the same builder seam.
- The CLI keystore repro ran the debug binary on macOS only; permission checks are unix-specific and were not tested on Windows. No TOCTOU race was attempted against the staged-import swap (it is rename-based under one directory).
- GUI behavior (clipboard persistence, dialog clearing) was established from source; no GUI process was launched.
- The stale-wallet finding was demonstrated with the node's own wallet API in-process; no production daemon was tricked into holding a wallet across an import.
- No upstream source was edited and no fix was applied. Pre-existing unrelated working-tree modifications in the repository were left untouched.
