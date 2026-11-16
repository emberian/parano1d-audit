# Wallet recovery and user trust boundaries

Target: pinned `52938433a30b185ab007afaa15166a632cf2e82e`. Local loopback
HTTP and synthetic wallet state only; no mainnet contact or user keys. This
lane sought concrete financial harm and recovery failures beyond the earlier
GUI-vs-node and wallet-safety reports.

## W7: a keyless impostor can replace the receiving address

The real GUI asks its loopback RPC node for `walletListAddresses` and
`walletActiveAddress` on every snapshot (`noid_gui/src/backend.rs:892-903`).
It copies the returned address string into `AddressSnapshot` without deriving
it from the local wallet secret (`backend.rs:955-1008`). The receive card renders
`snapshot.active_address().address` and its copy button dispatches
`Message::CopyAddress` (`view/present.rs:412-445,650-657`), which copies that
same string (`app.rs:1259-1267`). No independent binding to `wallet.key` runs
in the GUI.

An attacker who can answer this local RPC endpoint can therefore show a valid
address that *they* own as the victim's receiving address. A payer following
the displayed/copied address sends funds to the attacker. This needs no access
to the victim's spending key. It also does not require deceiving consensus:
a payer following the copied address signs an output that names the attacker.
The earlier
wallet-vs-node W3 demonstrated how a local process can pre-bind the loopback
port and be adopted by the GUI. A remote P2P peer alone cannot answer the GUI's
loopback RPC; a malicious release/binary or local impostor is the precondition.

The new `impostor_receive.rs` includes the earlier saved HTTP fixture and
drives the **real GUI backend** against it on loopback. Its control response
reports a valid synthetic victim address; the second response reports a
different valid synthetic attacker address for both wallet address methods.
The backend's active receive address changes to the attacker address, while
neither secret is sent to the impostor. `receive-results.txt` records the
passing test. The Copy Address path is source-traced rather than driven in an
Iced window.

`payment_destination.rs` supplies an independent payer control. It funds a
synthetic `WalletState`, asks the pinned production builder to pay the same
attacker address returned by the impostor, and verifies the resulting real
GKR authorization. The signed output owner is the attacker address. This
is a valid signed intent in local state, **not** a mined or independently
accepted block; `payment-results.txt` records the result. The attacker
address was derived from a known synthetic secret by the real WalletState in
the photo-context differential below. A complete devnet experiment should
mine this intent and confirm the victim's node cannot spend its output.

Impact: conditional direct payment diversion for users copying a receive
address from a compromised GUI-to-node boundary. The existing report's
"keyless impostor can only deceive" wording is too weak: here deception
changes the payer's actual destination. The fix needs a local, independently
trusted binding between the displayed receive address and the wallet's
derivation, or a trusted display/confirmation from a separate signer.

## W8: the published photo-key recipe derives a different wallet

The production GUI uses BLAKE3 derive-key context
`ParanO(1)d master secret from canonical image pixels v1`
(`noid_gui/src/secret.rs:15,141-149`). The English, Russian, and Chinese
`docs/.../wallet/photo-key.md:21` guides instead publish
`Parano1d master secret from canonical image pixels v1`. BLAKE3 derive-key
binds the context, so a standards-compliant independent recovery tool following
the project guide computes a different 32-byte master secret from the exact
same image pixels. That secret yields a different address set and cannot spend
the original wallet's funds. The text is a byte-exact key derivation recipe,
and the discrepancy is three characters plus capitalization, not a prose
translation.

`photo_context_diff.py` verifies the pinned source and all three guides,
constructs a 32x32 image with at least 64 distinct RGBA pixels (meeting the
GUI's minimum check), serializes the exact production preimage, and derives
both keys with BLAKE3. `photo_address.rs` imports each synthetic secret into
the real pinned `WalletState` and prints its address. Its binary was built by
directly linking the preexisting wallet-safety harness library, with no Cargo
rebuild or modification to the pinned tree. Run with the locally installed
Python BLAKE3 library:

```sh
PYTHONPATH=experiments/wallet-boundary/target python3 experiments/wallet-boundary/photo_context_diff.py
```

The saved output is `results.txt`. This is a recovery-interoperability bug,
not remote key extraction: the official GUI still uses its own correct
production context, so importing the unmodified original photo through the
same GUI recovers the wallet. The affected path is an independent implementation
from the published guide (including a future migration or lost-GUI recovery),
especially if the user kept only the photo as recommended in the guide.
The three guides describe the official GUI's photo import and recommend the
original photo, which is a working recovery path. They also specify exact
derivation bytes for interoperable recovery; those bytes are wrong at the
pinned version. No independent recovery attempt by a real user is known.

The correction is to publish the *shipped* context verbatim in every language
and add a fixed image-to-secret test vector. Changing the production context
would strand every existing photo-derived wallet unless an explicit migration
were provided.

## Other boundary checks

- The existing `wallet-vs-node` lane already demonstrates fabricated receipt,
  balance, inclusion, and send responses through the real GUI backend. Those
  are economic-decision harms (for example, releasing goods against a phantom
  payment) under a malicious/local impostor node, but they do not show theft
  from an honest node or acceptance by an independent recipient.
- An impostor RPC node cannot use the GUI's master-secret export route:
  `backend.rs:547-556` runs the local node binary's maintenance CLI on the
  local data directory instead of calling `walletExportSecret` over RPC.
  `backend.rs:559-591` also refuses secret import when the daemon is externally
  managed. A keyless loopback impostor cannot spend the victim's existing
  outputs, but can replace the receiving address shown to payers as W7 shows.
- `noid_rpc/src/server.rs:3753-3817` rejects requests bearing an `Origin`
  header before RPC dispatch and grants originless local access from the actual
  accepted socket peer address. The browser-to-loopback route therefore has a
  concrete guard; ordinary local processes remain inside the node's OS trust
  boundary. We found no browser-only spend path in this review.
- Ordinary wallet-send mismatch blindness remains as scoped in wallet-safety:
  the production builder passes recipient and amount straight through; a
  malicious node binary holding `wallet.key` can already spend with that key.
  The missing client check does not itself give an outside party spending power.

## Open end-user harm experiments

An independent merchant/exchange receiver should be added to a local devnet:
run a payer GUI against a dishonest node, and have the receiver verify its own
chain and deposit address. Measure what the GUI displays versus what the
receiver sees before and after network recovery. This would quantify the
existing receipt-deception impact without conflating a fake confirmation with
an accepted deposit. It is not run here because the phase-8 devnet and builds
are concurrent and own their data directories.
