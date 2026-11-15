# Wallet-vs-node trust-boundary audit

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

All created files are confined to this directory. The harness drives the REAL GUI backend (`noid_gui::backend::Backend`, constructed exactly as `main.rs` constructs it via `from_env()`) against a scripted impostor node answering every `paranoid_*` JSON-RPC call with attacker-chosen JSON over loopback HTTP. No live network, no real keys, loopback only. The wallet-safety lane audited the wallet against the honest pipeline; this lane asks what a malicious or corrupted node (including the storage lane's C20 zombie node) does to the user through the interfaces the wallet actually trusts.

Copy fidelity: `gui-noid_gui/` is byte-identical to the pinned `noid_gui` crate except one added file `src/lib.rs` (module declarations mirroring `src/main.rs`, so the binary crate can also build as a library; verified with `diff -r`, only `Only in gui-noid_gui/src: lib.rs`). `noid_tx`, `noid_core`, `noid_poseidon2b` are symlinks into the read-only pinned tree. `Cargo.toml` (workspace root) mirrors the pinned root's `[workspace.package]`/`[workspace.dependencies]` verbatim. The only code written by this lane is `harness/tests/deceive.rs` (the impostor server + scenarios).

## The trust boundary

Everything the wallet displays, records, or decides on is one loopback HTTP POST away (`noid_gui/src/backend.rs:1371-1407`; URL from `NOID_RPC` or `http://127.0.0.1:9601`, `backend.rs:49,1592`). One `snapshot()` call consumes 12 distinct RPC methods (measured: S0); the full scenario suite exercised 22. The GUI has no dependency on `noid_chain`, `noid_fri`, `noid_pruv`, or any verifier: no header hash check, no PoW check, no state-root check, no Merkle check, no signature check ever runs client-side.

| What the wallet consumes | RPC method(s) | Who computes it | Client-side verification | Deception demonstrated |
|---|---|---|---|---|
| Balance, spendable, pending in/out | `walletGetBalance` | node (its own UTXO scan) | none | S1/S4: 12.5 NOID invented; real funds zeroed, no alarm |
| Owned slots / UTXO table | `walletUtxoSnapshot`/`walletListUtxos` | node | none | S1: phantom UTXO rendered in the wallet's UTXO table |
| Payment history + receipts | `walletReceipts`, `walletExportReceipt` | node (node-side scanner `noid_node/src/wallet/scanner.rs:162-276`) | none | S2: attacker-authored receipt row |
| Receipt "verification" | `verifyReceipt` | node — including the Merkle math (`noid_rpc/src/server.rs:2454-2486`) | none; GUI relays the verdict (`backend.rs:1320-1332`) | S2: garbage bytes return AUTHENTICATED + CONFIRMED |
| Payment build/sign/submit | `walletSend` | NODE-side builder + prover (`server.rs:1108-1160`); node holds `wallet.key` | request echoed to screen (`backend.rs:820-827`, `present.rs:1994-2046`); no recipient/amount binding (wallet-safety Finding 1) | S3: fabricated txid accepted; node-chosen fee displayed uncapped |
| Inclusion/canonicality (explorer) | `getTx`, `getBlockDetails`, `getBlockHeader(ByHash)`, `getRecentTransactions`, `getSlotsByOwner`, `validateAddress` | node | none | S1: fabricated txid resolves as an included block transaction |
| Fee (AUTO mode) | `walletSend` replan / `walletPlanSend` | node | none (parsed only if user types one, `app.rs:1153-1159`) | S3: 2 NOID fee on a 5 NOID payment |
| Chain status (height, tip, state root, synced) | `getChainInfo`, `getBlockHeader`, `getNodeStatus` | node | none; `terminal_verified: true` is hardcoded (`backend.rs:1083-1086`) | S5: SYNCED + TERMINAL VERIFIED + attacker state root at fake tip 999999 |

## Findings

### W1: the entire user-facing wallet is node-attested; phantom payments and authenticated receipts are indistinguishable from real ones

**Severity: High (user-decision impact: believing they were paid, releasing goods; precondition: malicious/corrupted node or loopback impostor, see W3).**

Every layer a user would use to corroborate a payment is answered by the same node. Demonstrated end-to-end through the real backend (S1+S2):

1. `walletGetBalance` reports `pending_incoming`/balance; the address card renders "INCOMING 12.500000" (`present.rs:528-530`) and the send form offers it as spendable (`present.rs:1852-1855,1974-1980`).
2. The explorer "corroboration" (`backend.rs:1174-1210`) chains `validateAddress` -> `getSlotsByOwner` -> `getRecentTransactions`; the same impostor serves "RECEIVED 12.5 NOID" (`view/explorer.rs:440`) and the fabricated txid resolves as an included block transaction via `getTx` (`backend.rs:1246-1267`).
3. The receipt detail — the flow explicitly built for independent verification — exports node-stored bytes (`walletExportReceipt`) and asks the node to verify them (`verifyReceipt`), where BOTH the Merkle-inclusion math and the canonicality lookup run server-side (`server.rs:2454-2486`). The GUI renders the node's `merkle_valid/canonical/confirmed` booleans as "AUTHENTICATED PAYMENT ... CONFIRMED" (`view/proofs.rs:620-700`). The harness returned all-true with an attacker-chosen summary (fee, height, outputs incl. a 999 NOID output to the attacker) for 256 bytes of `deadbeef`.

There is no second source: wallet history itself is produced by the node-side scanner from the node's own accepted blocks (`scanner.rs:162-276`; pending sends confirmed by txid match only, `scanner.rs:184-193,236-247`), persisted in the node's data dir, and served back by the same RPC.

### W2: the send pipeline executes node-side; under a malicious node the success screen is a work of fiction (escalation of wallet-safety Finding 1)

**Severity: High when the malicious node holds `wallet.key` (real recipient/amount substitution, actually spendable); Medium for a keyless impostor (fake success, induced decisions).**

The wallet-safety lane proved no downstream check can detect a builder divergence and rated it Low because the pinned builder passes the request through. Under this lane's threat model the builder IS the attacker: `paranoid_walletSend` builds, proves, and signs node-side (`server.rs:1108-1160`; the GUI never holds spend keys — `wallet.key` is read by the node daemon). The harness (S3) shows the client side of that: the impostor returned a fabricated txid and a 2 NOID fee for a 5 NOID payment; the real backend returned `PaymentSubmission { recipient: <request>, txid: <fabricated>, fee: 2 NOID }` and the success screen renders "TRANSACTION SENT" + the request recipient + node txid + node fee (`present.rs:2035-2046`). The node also sees the plaintext request (recipient, amount) — the wallet has no privacy from its own node. Fee steering rides the same seam: AUTO mode lets the node pick any fee and the client displays it after the fact, uncapped.

### W3: whoever answers on the RPC port is the wallet's node

**Severity: Medium (enabler; local code execution by any user pre-binding 127.0.0.1:9601, or a node that dies and is replaced; impostor deceives but cannot spend — it never sees `wallet.key`).**

Two adoption paths, both demonstrated against real supervision code:

- `ensure_running` pings first and returns `Ok` if anything answers — the GUI never spawns its node at all (`backend.rs:727-730`). Loopback RPC needs no credential (loopback peers are `RpcAccess::Local`, per the wallet-safety lane's RPC review).
- If the GUI's own node binary dies at startup (crash, corrupted storage), `wait_until_ready` checks the ping BEFORE the child each iteration; an impostor that starts answering makes the GUI go ONLINE with a dead owned child (`backend.rs:1476-1499`). S6(b) proves it: the GUI ran its node binary (a script that exits 7), the child died, and `ensure_running` + full `snapshot()` succeeded against the impostor.

No node identity pin, version handshake, or auth exists on the GUI->node boundary. The "SPENDING SECRET / LOCAL" and "Your secret stays on this device" send-form assurances (`present.rs:1895,1915`) remain true — but they are about the key, not about the answers.

### W4: suppression is silent

**Severity: Medium (extortion/confusion; no direct theft).** A healthy chain at height 4324 with `walletGetBalance=0`, no UTXOs, no receipts renders as a calm, successful, empty wallet (S4). Nothing client-side can distinguish "no funds" from "funds hidden".

### W5: chain-status indicators are decorative

**Severity: Low.** `terminal_verified: true` is hardcoded with a comment acknowledging it means "the node accepted the tip" (`backend.rs:1083-1086`); `synced`, height, and the displayed state root are node fields (S5: fake tip 999999 with attacker state root renders as SYNCED + TERMINAL VERIFIED).

### W6: truncated peer rendering invites lookalike counterparties

**Severity: Low (rendering-level).** Receipt rows render the node-chosen `peer_address` truncated to first 13 + last 7 characters (`view/proofs.rs:1014-1019`); a lookalike sharing those windows renders identically (S7). The send-success screen shows the full request recipient, and the send path performs no local address validation at all (only non-empty, `app.rs:1141-1145`; `validateAddress` is RPC, used by explorer search).

## Negative and control results (what the wallet DOES do itself)

- **Control (S0):** an honest-shaped answer set flows through the real backend unchanged — the impostor speaks the actual wire format (12 methods in one `snapshot()`), so the deceptions are not protocol-shape artifacts.
- **Local input parsing:** amounts and explicit fees are parsed and bounded client-side (`parse_noid_amount`/`parse_optional_noid_fee`, `app.rs:1146-1159`); the empty-recipient check is local.
- **Conditional UTXO snapshots:** the revision discipline in `wallet_utxos` (`backend.rs:1344-1369`) prevents stale-response races — an anti-liveness hardening, not an anti-node control (an impostor simply sends full snapshots).
- **Native-asset submit binding:** `finish_submit` compares the node's submission against the reviewed preview (`noid_gui/src/assets/operations.rs:539-552`). This is a real client-side consistency gate — but the preview is also node-computed (`walletPreviewNative`, `backend/native.rs:66-77`), so it detects a divergent builder, not a malicious node.
- **Key hygiene (from the wallet-safety lane, unchanged):** the secret never transits RPC; the GUI's send path holds no spend key; the impostor in W3 therefore cannot spend, only deceive.
- **Server-side gates still exist** (`server.rs:1164-1167` fee/counts gate; GKR authorization binds outputs) — same-trust controls that constrain an honest builder, not the client's attacker model here.

## PoC implementation and reproduction

- `gui-noid_gui/` — verbatim pinned GUI crate + `src/lib.rs` shim (module decls only).
- `Cargo.toml`, `noid_tx`/`noid_core`/`noid_poseidon2b` (symlinks) — harness workspace mirroring the pinned root's workspace tables.
- `harness/tests/deceive.rs` — hand-rolled loopback HTTP/1.1 impostor (scripted per-method handlers, full request log) and seven scenarios against the real `Backend`: S0 control, S1 phantom payment (balance + explorer + inclusion), S2 phantom authenticated receipt (export + verify), S3 send blindness + fee steering + plaintext-request capture, S4 suppression, S5 status spoof, S6 impostor adoption (pre-bound + dead-child), S7 truncated-peer lookalike.

From this directory:

```bash
cargo fmt --check
cargo test --test deceive -- --nocapture 2>&1 | tee results.txt
```

Final output includes:

```text
PASS [S1-phantom-payment] GUI shows INCOMING 12.500000 NOID that never existed
PASS [S2-phantom-receipt] garbage bytes verify as AUTHENTICATED + CONFIRMED with attacker-chosen outputs
PASS [S3-send-blindness] node-chosen fee (2 NOID on a 5 NOID payment) flows to the success screen uncapped
PASS [S4-suppression] healthy chain + zero balance renders as an empty-but-successful wallet (no alarm)
PASS [S5-status-spoof] GUI reports SYNCED + TERMINAL VERIFIED + attacker state root at a fabricated tip
PASS [S6-impostor-adoption] (a) pre-bound impostor satisfies ensure_running with no node spawned
PASS [S6-impostor-adoption] (b) GUI spawns its node, node dies (marker + exit), GUI still goes ONLINE via impostor
INFO impostor served 77 requests across 22 distinct methods
ALL 15 CHECKS PASSED
```

## Assurance limits

- No iced rendering was executed; assertions are made on the exact model values the view layer renders, with the render sites cited. `short_address` (5 lines, private) was re-implemented verbatim for S7. No GUI process, dialog, or clipboard was driven.
- The impostor stubs the HTTP surface; it demonstrates the absence of client-side verification, not a specific server compromise vector. Realistic triggers are a compromised node binary, the storage lane's C20 corrupted-storage node, or the W3 loopback impostor (which requires local code execution by any local user, and yields deception without key theft).
- W2's key-holding variant (a malicious node genuinely building a divergent payment) was not executed against a live wallet; the wallet-safety lane already proved the builder can emit fee/count-preserving divergent intents that pass every downstream check, and S3 proves the client accepts any result.
- Plain NOID payments only; the native-asset and trade surfaces were read (holdings, preview/submit binding) but not exercised beyond the plan review.
- macOS only; the impostor-adoption race is deterministic in the harness (scripted failure ordering), while in the wild it depends on the impostor answering before the child-exit check.
- No upstream source was edited and no fix was applied. The pinned tree was only read; pre-existing working-tree modifications there were left untouched.
