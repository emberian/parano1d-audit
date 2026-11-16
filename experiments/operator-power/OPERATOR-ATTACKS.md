# What could a NOID operator actually control?

Source and public-document review on 2026-11-15. Target: pinned node source
`52938433a30b185ab007afaa15166a632cf2e82e`. This report treats “operator”
as a set of possible powers, not as one established person. The [privilege
review](../operator-privileges/REPORT.md), [selective-exit map](../operator-exit/OPERATOR-THREAT.md),
[release review](../release-chain/OPERATOR-CONTROL.md), and [wallet/service
review](../wallet-boundary/OPERATOR-CONTROL.md) give the underlying paths and limits.

## Result

The pinned node has no identified developer key, native NOID blacklist, freeze
switch, or on-chain command that exempts a favored spender. The protocol does
mandate two launch-period subsidy payments: 5% each to fixed O(1) Network Fund
and Parano1d Lab addresses, with 90% to miners, through height 3,223,778.
This is disclosed in the [project's economics](https://docs.parano1d.org/protocol/economics)
and enforced by `development_allocation.rs` and `validation.rs`. The source
names the beneficiaries but cannot prove who holds their keys. Receiving
issued coins gives potential inventory to sell, not a right to censor another
holder's signed spend.

The most plausible *selective-exit shape* is operational: an actor already
has NOID credited at an exchange, new deposits stop reaching that exchange,
and the actor sells the precredited balance. [SafeTrade lists a NOID/USDT
market](https://safetrade.com/exchange/NOID-USDT); its deposit status and
NOID confirmation rule were not verified. Neither the developer's exchange
inventory nor their ability to cause a deposit outage was established. Other
holders with balances already credited at the exchange could also trade.

| Power | What the pinned system permits | What remains to connect it to selective exit |
| --- | --- | --- |
| **Fund keyholder** | Spend the two fixed subsidy streams under ordinary owner authorization. Genesis itself starts empty. | Identify key custody and actual balance or sale; fund addresses have no special spend or censorship opcode. |
| **Release publisher** | Offer a new node/GUI binary, including one that could change local wallet or relay behavior if installed. The published workflow uses same-channel SHA256SUMS; no independently verified bit-for-bit binary rebuild or detached artifact signature was found. | Show that a changed binary was shipped and installed by relevant holders, miners, or the exchange. Installed v2.1 has no automatic update feed or remote rule switch. Existing honest binaries need not accept incompatible consensus. |
| **Default-seed DNS controller** | Choose a fresh default node's initial peer endpoints through four unpinned `mseed*.parano1d.org` names. Domain control by the developer is plausible, not established by source alone. | Keep the target from obtaining independent honest peers or work. Configured and persisted peers, candidate work comparison, and install rechecks limit first-contact authority. A DNS change does not evict established connections. |
| **Block producer** | Omit a holder's otherwise valid transaction from blocks it wins while including its own. | Sustain enough work and propagation/fork-choice advantage that honest producers cannot get the holder included. Recent coinbase concentration is a dated observation, not proof of developer-controlled hashpower. |
| **Node or pool RPC operator** | A deliberately configured `--operator-key` may call `walletSend` for *that node's active wallet*. The developer has no default network-wide credential in the source. | Obtain the particular wallet's token, local process, or binary control; this does not reach unrelated holders' wallets. |
| **Exchange operator** | Set deposit/trade policies and hold credited balances outside this codebase. | Evidence of SafeTrade authority/cooperation or a NOID-specific deposit failure. The project source does not grant exchange control. |
| **Optional market-data host** | With External data enabled, `tradedata.parano1d.org` can supply fabricated historical DEX candles that pass the GUI's structural checks; current spot and pool balances come from the local node. | Attribute host control and show users enabled it or acted on a false chart. This is decision misinformation, not a freeze or fake exchange credit. |

The [local wallet experiment](../operator-exit/REPORT.md) makes one boundary
concrete: `walletSend` returned a txid with zero peers while an independent
receiver saw nothing, then the same tx propagated after connection. The GUI's
“TRANSACTION SENT” screen therefore says local admission, not exchange
credit. It does not show who could partition a holder. A malicious local RPC
first-binder can substitute the GUI's receive address, but the ordinary GUI
uses a sibling local node; a remote seed alone has no such RPC authority
([wallet boundary](../wallet-boundary/README.md)).

## What would change the verdict

The decisive evidence is a **connected chain of control**, not another list
of theoretical attacks: (1) a particular actor controls a miner, seed set,
release installation, or exchange deposit service; (2) that control actually
keeps a holder's valid transaction from being credited; and (3) the same
actor has an independent credited balance or working deposit route. None of
those three links is established together. A current block-production sample
must identify miners beyond payout addresses, and exchange deposit status
must be observed independently; stale explorer and pool snippets cannot
establish either.

This does not make the trust concentration imaginary. The allocation is a
real economic stream; clean-install discovery is concentrated; released
binaries depend on publisher trust; and block producers have discretion.
It makes the public conclusion narrower: **no demonstrated operator-only
ability to sell while other native NOID holders cannot**.
