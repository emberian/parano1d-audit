# What the GUI trusts from its node

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.

The test drives the production GUI backend (`noid_gui::backend::Backend`,
constructed with `from_env()`) against a scripted loopback JSON-RPC server.
The server chooses its answers; the test uses synthetic data and no live
network. This isolates the question: what does the client verify when its node
is dishonest? The iced GUI itself was not run. The findings below identify
backend values and the view code that would render them.

The copied GUI crate differs from the pinned crate only by `src/lib.rs`,
which exposes its existing modules to the harness. The other dependencies
refer to the pinned checkout. The test logic lives in
`harness/tests/deceive.rs`; reproduction details appear below.

## The trust boundary

The GUI backend reads wallet and chain state from a local HTTP endpoint:
`NOID_RPC` or `http://127.0.0.1:9601` (`backend.rs:49,1371-1407,1592`). A
single `snapshot()` calls 12 RPC methods; the tests exercised 22 in total.
The GUI crate has no chain or proof verifier dependency. It parses and checks
some local inputs, but it does not verify headers, work, state roots, receipt
Merkle paths, or owner proofs returned by the node.

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

### W1: node answers become payment and receipt status

If a malicious service answers the GUI's RPC calls, the backend accepts its
balance, transaction and receipt fields. A user could act on a false payment
display. The tests demonstrate this at the backend boundary; no user or
exchange was deceived. Scenarios S1 and S2 establish the data flow:

1. `walletGetBalance` reports `pending_incoming`/balance; the address card renders "INCOMING 12.500000" (`present.rs:528-530`) and the send form offers it as spendable (`present.rs:1852-1855,1974-1980`).
2. The explorer path (`backend.rs:1174-1210`) calls `validateAddress`,
   `getSlotsByOwner`, and `getRecentTransactions` on the same service. The
   impostor supplied a receipt of 12.5 NOID and an included transaction for
   its fabricated txid (`view/explorer.rs:440`; `backend.rs:1246-1267`).
3. The backend exports receipt bytes through `walletExportReceipt` and asks
   the node to verify them through `verifyReceipt`. Merkle and canonicality
   checks run on the server (`server.rs:2454-2486`). The view maps the returned
   booleans to “AUTHENTICATED PAYMENT” and “CONFIRMED”
   (`view/proofs.rs:620-700`). The impostor returned `true` for those fields
   and an invented summary for 256 bytes of `deadbeef`.

The wallet history comes from the node's own scanner and storage
(`scanner.rs:162-276`). The explorer and receipt views do not provide an
independent corroborating source.

### W2: sending and the success response are node-side

`paranoid_walletSend` builds and proves the payment in the node process
(`server.rs:1108-1160`), which holds `wallet.key`. The GUI sends the recipient
and amount in plaintext. In S3 the impostor returned a fabricated txid and a
2 NOID fee for a 5 NOID request. The backend accepted those response fields;
the view code would show “TRANSACTION SENT” with the requested recipient,
fabricated txid, and returned fee (`present.rs:2035-2046`). AUTO fee selection
also comes from the node, after the user's request.

This proves a keyless impostor can report false success. It does not prove
that impostor can spend funds. A compromised key-holding node could submit a
different valid payment, but this lane did not execute that variant.

### W3: the backend accepts any responding loopback service

An attacker who can run a process on the user's machine can bind the default
RPC port before the node, or answer after the node dies. The harness exercised
both adoption paths against the production supervisor:

- `ensure_running` pings first and returns `Ok` if anything answers — the GUI never spawns its node at all (`backend.rs:727-730`). Loopback RPC needs no credential (loopback peers are `RpcAccess::Local`, per the wallet-safety lane's RPC review).
- If the GUI's own node binary dies at startup (crash, corrupted storage), `wait_until_ready` checks the ping BEFORE the child each iteration; an impostor that starts answering makes the GUI go ONLINE with a dead owned child (`backend.rs:1476-1499`). S6(b) proves it: the GUI ran its node binary (a script that exits 7), the child died, and `ensure_running` + full `snapshot()` succeeded against the impostor.

The GUI does not pin the node identity or authenticate this local endpoint.
The key still remains with the node; the keyless impostor can deceive the GUI
but cannot authorize an on-chain spend from that fact alone.

### W4: a false zero balance looks ordinary

A service reporting height 4324, zero balance, no UTXOs and no receipts made
the backend produce an ordinary empty-wallet state (S4). The GUI has no second
source with which to distinguish that answer from hidden funds.

### W5: chain status repeats the node's claims

`terminal_verified: true` is hardcoded in the backend, where a comment says it
means the node accepted its tip (`backend.rs:1083-1086`). The `synced`, height,
and state-root fields come from the node. S5 supplied a fabricated tip at
999999, which the model marked synced and terminal-verified.

### W6: receipt counterparties are abbreviated

Receipt rows show the first 13 and last 7 characters of the node-supplied
`peer_address` (`view/proofs.rs:1014-1019`). Two addresses sharing those
windows look identical there (S7). The send-success view shows the full
requested address. Local send validation checks that it is nonempty;
`validateAddress` is an RPC used by explorer search (`app.rs:1141-1145`).

## Controls and limits

- **Control (S0):** an honest-shaped answer set flows through the real backend unchanged. The impostor speaks the expected wire format for all 12 methods in one `snapshot()`.
- **Local input parsing:** amounts and explicit fees are parsed and bounded client-side (`parse_noid_amount`/`parse_optional_noid_fee`, `app.rs:1146-1159`); the empty-recipient check is local.
- **Conditional UTXO snapshots:** revision checks in `wallet_utxos` (`backend.rs:1344-1369`) prevent stale-response races. An impostor can still send a complete false snapshot.
- **Native-asset submit binding:** `finish_submit` compares the submission with the reviewed preview (`noid_gui/src/assets/operations.rs:539-552`). The node also computes that preview (`backend/native.rs:66-77`), so the check detects a divergent response, not jointly false answers.
- **Key hygiene:** the secret does not transit this RPC. A keyless loopback impostor cannot spend by answering wallet queries.
- **Server-side gates:** the honest node still checks fees and counts (`server.rs:1164-1167`), and authorization binds outputs. Those gates protect its own construction path; they do not authenticate the GUI's RPC endpoint.

## PoC implementation and reproduction

- `gui-noid_gui/` — verbatim pinned GUI crate + `src/lib.rs` shim (module decls only).
- `Cargo.toml`, `noid_tx`/`noid_core`/`noid_poseidon2b` (symlinks) — harness workspace mirroring the pinned root's workspace tables.
- `harness/tests/deceive.rs` — hand-rolled loopback HTTP/1.1 impostor (scripted per-method handlers, full request log) and seven scenarios against the real `Backend`: S0 control, S1 phantom payment (balance + explorer + inclusion), S2 phantom authenticated receipt (export + verify), S3 send blindness + fee steering + plaintext-request capture, S4 suppression, S5 status spoof, S6 impostor adoption (pre-bound + dead-child), S7 truncated-peer lookalike.

From this directory:

```bash
cargo fmt --check
cargo test --test deceive -- --nocapture 2>&1 | tee results.txt
```

The saved test output includes the following labels. “GUI shows” in these
labels means the backend produced fields used by the cited view code; the
iced renderer was not executed.

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

- No iced rendering was executed. Assertions cover the exact backend model
  values used by the cited view code. `short_address` was reproduced for S7.
  No GUI process, dialog, or clipboard was driven.
- The scripted HTTP server proves what the client accepts, not that a remote
  network peer can impersonate the local node. W3 requires local process
  execution or a compromised node binary. C20 is a distinct local-storage
  corruption scenario.
- A key-holding malicious node building a divergent payment was not executed
  here. The wallet-safety lane studied that separate condition; S3 establishes
  only client acceptance of an arbitrary reported result.
- Plain NOID payments only; the native-asset and trade surfaces were read (holdings, preview/submit binding) but not exercised beyond the plan review.
- macOS only; the impostor-adoption race is deterministic in the harness (scripted failure ordering), while in the wild it depends on the impostor answering before the child-exit check.
- The pinned source was read without editing it.
