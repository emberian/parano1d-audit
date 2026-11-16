# Native NOID operator powers in the pinned source

Target: `parano1d/` commit `52938433a30b185ab007afaa15166a632cf2e82e`.
This is a source review of the current mainnet consensus, transaction, mempool,
node configuration, and repository documentation. It did not contact the public
network or establish who controls any address, seed, miner, wallet, or exchange.

## Result

The code contains a **mandatory development allocation**, not a hidden
administrator key. Through height **3,223,778**, miners receive 90% of the
block subsidy, while the O(1) Network Fund and ParanO(1)d Lab each receive
up to 5%, materialized as two outputs in a mandatory daily system payout.
Those outputs go to hardcoded addresses. Consensus rejects a missing payout,
a changed recipient, or a changed amount. The repository openly describes the
allocation and says the Lab supports founders and developers. That identifies
the intended beneficiaries, but source code and labels alone do **not** prove
which person holds the private spending material or how much of the issued
NOID remains in either wallet.

I found **no source-defined address blacklist, native NOID freeze switch,
administrator spend override, or special permission for those two addresses
to bypass ordinary spending authorization** in the surveyed paths. This is a
bounded negative finding, not a proof that all future releases or all external
wallets and exchanges are benign. A block producer may omit a holder's valid
transaction from its *own* blocks, but other producers can include it; the
source does not grant the development recipients exclusive block-production
rights. No reviewed code path lets the apparent operator alone make everyone
else unable to spend native NOID while retaining an intrinsic right to spend
its own.

## Economic privilege, precisely

| Rule | Evidence | Consequence |
| --- | --- | --- |
| Built-in genesis is empty | `noid_chain/src/consensus/genesis.rs:4-13,36-50`; `block.rs:391-420` | State starts empty and genesis carries no transaction or spendable coinbase. “No premine” is accurate in this narrow genesis sense. |
| Hardcoded fund outputs | `noid_chain/src/consensus/development_allocation.rs:4-49,101-155` | Two fixed addresses receive the launch allocation. The code names them; it does not authenticate a human controller. |
| V2-adjusted horizon | `development_allocation.rs:81-95`; `consensus/params.rs:11-23,69-77`; `README.md:169-185` | The 20-second legacy interval changes to 30 seconds at H210,537. The original three 365-day *target-time* years therefore end at H3,223,778, not at a wall-clock date guaranteed by consensus. |
| Exact payout validation | `consensus/validation.rs:310-368`; `block.rs:428-455` | The payout is required only when due and must use the exact two addresses and amounts. It is not an optional miner donation or a mutable operator parameter. |
| Disclosure in project docs | `README.md:169-186`; `docs/protocol/economics.md:54-71` | The project states 90/5/5 and describes the Lab as supporting founders, core developers, researchers, and contributors. This is a source claim about purpose, not proof of present key custody. |

The fund share is taken from **subsidy**, not a transfer of existing holders'
coins. The primary coinbase ceiling includes only the miner subsidy plus
claimable transaction fees after deterministic state-growth burn
(`consensus/emission.rs:105-161`; `consensus/validation.rs:539-560`). The code
notes that a state-depth reward change during a payout interval can leave part
of the theoretical 5% shares unissued (`development_allocation.rs:4-11,201-207`).
It cannot redirect a holder's existing UTXO to a fund address through this
allocation rule.

The source's own canonical address test encodes the Fund as
`o1r3djxaz54k47kr54x76cwqkhl6xqucescv943m5m8uvc5w6x7euqct3h07` and
the Lab as `o1xcjdp3udp5sgwcvnmjlu9s53u4fx5m3hpquvg0uem2pr2mrr9dqq38jple`
(`development_allocation.rs:250-259`). These are destinations, not evidence
of their current owners or balances.

## Spend, inclusion, and upgrade boundaries

1. **Ordinary spend:** A non-coinbase body requires live inputs and exact
   input/output/fee conservation (`noid_tx/src/public_logic.rs:66-117`). The
   mempool verifies the detached owner authorization before admission and
   rechecks state afterward (`noid_mempool/src/pool.rs:369-445,447-513`). The
   surveyed fund-address references outside tests are output construction,
   payout validation, and wallet display/scanning; none branches on those
   addresses as an input-owner exception. The two zero-input system mints are
   separately constrained by shape and placement (`block.rs:428-455`;
   `consensus/validation.rs:310-368`).
2. **Producer censorship:** Transaction ordering in the reference builder is
   explicitly miner policy, not a consensus inclusion rule
   (`consensus/ordering.rs:4-15`). Fork choice selects among valid chains by
   cumulative work while preserving local finality
   (`consensus/fork_choice.rs:37-82`). Thus a miner can choose not to include a
   transaction in blocks it wins. A lasting, network-wide selective exclusion
   requires enough production/relay control to defeat alternative inclusion;
   the source does not attribute such power to the developer or fund addresses.
3. **Protocol changes:** V1.1, V2, and V2.1 activation heights and owner-proof
   versions are compiled constants (`consensus/params.rs:16-63,100-114`;
   `consensus/forks.rs:100-107`). The reviewed source has no on-chain
   administrator transaction that changes these heights or installs a new
   blacklist. A publisher can offer different code, but adoption of that
   binary or hard fork is a distribution and social trust question, not a
   privilege exercised inside the pinned node.
4. **Local operator RPC:** The node optionally accepts a separately configured
   operator bearer token for pool accounting and payouts; its CLI warns that
   this credential can spend from that node's **active wallet**
   (`noid_node/src/main.rs:1324-1336,2398-2406`). This is real local-wallet
   authority granted by a node operator, not a network-wide developer key or
   a default credential embedded in consensus. The mining token is a different
   role limited to template and submit methods (`main.rs:1308-1322`).
5. **Bootstrap:** Four default DNS names share the `parano1d.org` zone and do
   not pin peer IDs (`consensus/network.rs:99-135`). Controlling those answers
   could influence fresh-node discovery. The DNS entries are not consensus
   checkpoints or native spending keys. The separate
   [operator-power review](../operator-power/REPORT.md) examines candidate
   selection and retained work; it has not shown a release node accepting a
   poisoned snapshot.

## Limits and next evidence

The negative privilege conclusion covers the inspected pinned code paths,
not arbitrary future binaries, compromised local RPC, third-party exchange
custody, or an unknown producer majority. It does not prove who controls the
two fund addresses or how much hashpower any actor has. To claim a selective
exit, evidence must connect one actor to a concrete submission/deposit choke
point **and** a working alternative exit for that actor; the protocol's
development allocation alone supplies neither link.
