# Operator exit and deposit-path audit

Target: pinned `parano1d/` checkout at `52938433a30b185ab007afaa15166a632cf2e82e`.
Run: 2026-11-15. `repro.py` and `results.txt` use only loopback, cloned synthetic
devnet state, synthetic wallets, and the previously built `devnet-chaos/bin` node
and CLI. No live-network transaction or exchange interaction occurred. The
original pinned checkout and another session's devnet were left untouched.

## Confirmed: a wallet can report "TRANSACTION SENT" before any peer receives it

An honest sender and an independent receiver started from identical synthetic
chain state at height 150, with zero connected peers on both sides. The sender
used `walletSend` to pay 100,000 μNOID to the receiver's address. It returned a
txid, fee 9,000 μNOID, and successful result while both nodes still had zero
peers. Five seconds later the receiver had no mempool entry for the txid.
After the receiver was restarted with the sender as its seed, it connected to
one peer and acquired **that same txid**. See `results.txt`.

The code explains the observation:

- `noid_rpc/src/server.rs:864-934` plans `walletSend` from local wallet/state and
  local fee floor. Unlike mining, it has no network-readiness gate.
- `noid_rpc/src/server.rs:1189-1208` returns success immediately after local
  `wallet_submission_mempool.submit`, before an independent peer acknowledges it.
- `noid_node/src/main.rs:2354-2373` forwards `TxAdmitted` over an asynchronous
  channel; lagged messages can be skipped and the result of sending the
  `BroadcastTx` command is discarded.
- `noid_gui/src/view/present.rs:2066-2071` labels the successful local result
  **"TRANSACTION SENT"**, with recipient and txid shown underneath.

This is a real acknowledgment semantics issue: the user can receive a success
screen for a transaction known only to their own node. The transaction remains
valid and the independent receiver sees it after connectivity returns in our
control. The run does not show final confirmation, fund loss, or permanent
failure to reach a healthy network.

The code has a stronger readiness gate for miner use (`server.rs:968-975`) and
wallet address discovery (`server.rs:2790-2804`); the payment path deliberately
or accidentally does not use those gates. A useful fix would label local
admission as **pending broadcast**, report peer observation separately, and
require a confirmation before claiming delivery to an exchange.

### Reproduction and limits

Run `python3 experiments/operator-exit/repro.py` from the repository root.
The script clones existing *synthetic* devnet `n1-pool` and `n2-relay` data
using copy-on-write into a uniquely named directory under this lane's `run/`,
removes only the copied P2P identity files, binds 127.0.0.1 on ports 19910,
19911, 19920, 19921, records process IDs, and stops those exact processes.
It asserts `getPeerCount == 0` both before and after the sender's RPC success;
then it checks the receiver's missing mempool entry and connected control.
The default software has mDNS even with `--disable-dns-seeds`, so the host
run does not guarantee that discovery packets stay inside this experiment.
The peer-count assertions do establish the zero-connected-peer condition
during the measured window. There was no external mainnet contact.

A separate attempt to run the prebuilt Linux debug node inside Docker
`--network none` failed before RPC startup because that binary lacked the
scheduled v2 HistoryStep runtime for the cloned state. This failed attempt
is not counted as reproduction evidence. Its copied data under `run/` was
left in place for inspection; no other session's files were deleted.

## Selective-exit model: feasible only with additional control

Suppose an adversary can isolate a *particular* holder's node while retaining
an independent path to honest producers. That holder can obtain the local
"sent" result above while their attempted exchange deposit remains unseen;
the adversary can transmit their own signed payment through the healthy path.
This asymmetry is straightforward under the stated partition but **this audit
has not shown that the developer can produce such a sustained partition of
existing holders on the live network**. Four DNS seeds affect first discovery;
they do not, by themselves, revoke existing peer connections or remove
retained HeaderDAG work. Another route or later reconnection defeats the
demonstrated delivery gap. Deposits also depend on exchange acceptance rules
which we have not observed. Balances already credited inside SafeTrade need
no fresh NOID deposit to be traded, so this scenario cannot establish a
blanket "only the operator can sell" power.

There are two related, narrower control points:

1. **Local relay policy.** The fee floor is calculated separately by every
   node from its own admitted transactions (`noid_mempool/src/floor.rs:1-102`),
   with an 80% occupancy latch after v1.1 activation. Admission requires
   `actual >= max(local floor, consensus minimum)` (`pool.rs:1243-1271`).
   Saturation or fee-floor manipulation of a holder's sole node can reject its
   payment while another node accepts an ordinary payment. High-fee valid
   transactions and occupancy are required for the fee-floor route; the
   anonymous invalid-proof flood in the other lane does not by itself raise
   it. The GUI's zero-fee automatic mode replans on fee rejection, up to
   three attempts (`server.rs:1024-1059`), limiting this route.
2. **Producer inclusion.** The consensus validator checks block validity,
   resources and roots, without any maximal-inclusion rule
   (`noid_chain/src/consensus/validation.rs:574-650`). A winning producer
   can omit a holder's transaction while including its own. This is conditional
   on enough sustained production control or suppression of honest producers;
   the auditor has not attributed that power to the developer. A non-colluding
   producer with the holder's transaction can include it. Existing
   `ledger-fairness` experiments also show that a first-seen output-slot
   conflict can force the victim to reprove, but the wallet chooses fresh
   slots and that is temporary pressure, not a privileged freeze.

The mandatory development-fund outputs are exactly two hardcoded recipients
of a defined subsidy share (`noid_chain/src/consensus/development_allocation.rs`);
they are payment recipients, not an admin authority over holder inputs.
The ordinary spend validation examines owner and authorization rather than a
developer permission bit (`noid_tx/src/paged_spend.rs:377-475`,
`noid_gkr/src/wallet_authorization.rs:635-671`). This code review did not find
an address-specific NOID freeze or developer-only transfer opcode. That is
bounded negative evidence, not a proof over every runtime, release, exchange,
or alternative client.

## Next discriminating experiment

Use three isolated nodes with separately funded sender, adversary, and
receiver wallets on one common synthetic chain. Let the receiver represent
an exchange deposit verifier. Compare victim and adversary sends during a
targeted partition, and repeat with one independent honest peer. Record
`getPeerCount`, per-node mempool entry, block inclusion, confirmation depth,
and wallet UI state. This distinguishes transient local acknowledgment from
actual selective inclusion. Nothing in this run justifies publishing a claim
that the developer can selectively sell or freeze holders.
