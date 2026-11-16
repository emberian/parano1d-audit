# Control surfaces affecting NOID transaction delivery

Source and public-document review on 2026-11-15. Target: pinned node source
`52938433a30b185ab007afaa15166a632cf2e82e`. Here “operator” refers to
separate roles that may be held by different people or organizations. The
[issuance and authority review](../operator-privileges/REPORT.md),
[transaction-delivery map](../operator-exit/OPERATOR-THREAT.md),
[release review](../release-chain/OPERATOR-CONTROL.md), and
[wallet/service review](../wallet-boundary/OPERATOR-CONTROL.md) give the
underlying paths and limits.

## Result

The pinned node has no identified administrator key, native NOID blacklist, freeze
switch, or on-chain command that exempts a particular spender. The protocol does
mandate two launch-period subsidy payments: 5% each to fixed O(1) Network Fund
and Parano1d Lab addresses, with 90% to miners, through height 3,223,778.
This is disclosed in the [project's economics](https://docs.parano1d.org/protocol/economics)
and enforced by `development_allocation.rs` and `validation.rs`. The source
names the recipients but cannot establish who holds their keys. Fund receipts
are spendable under the same owner-authorization rule as other native NOID;
they do not grant transaction-inclusion authority.

A [dated public-explorer snapshot](FUND-OBSERVATION.md) reported 599,040
NOID issued to each fund by height 313,800. It showed one 500,000-NOID
transfer from each fund payout address to a separate recipient address, with
both recipient outputs still unspent in that explorer view. The two outputs
are about 8.22% of the explorer's reported net supply. Their keyholders,
purpose, and later disposition are unknown. The explorer view does not
establish mining, release, seed, or exchange control.

Exchange trading and on-chain deposits are separate systems. If NOID deposits
were interrupted, an account already credited at an exchange could still trade
its existing balance while a new depositor waited. [SafeTrade lists a NOID/USDT
market](https://safetrade.com/exchange/NOID-USDT); its deposit status and
NOID confirmation rule were not verified. This review has not established a
deposit interruption, any specific account balance, or control of the exchange
or the interruption by a particular party. Other precredited accounts would
have the same trading option.

| Control surface | What the pinned system permits | Evidence needed to assess its reach |
| --- | --- | --- |
| **Fund payout keys** | Spend the two fixed subsidy streams under ordinary owner authorization. Genesis itself starts empty. | Identify key custody and actual balance or transfer purpose; fund addresses have no special spend or inclusion rule. |
| **Release publisher** | Offer a new node/GUI binary, which could change local wallet or relay behavior if installed. The published workflow uses same-channel SHA256SUMS; no independently verified bit-for-bit binary rebuild or detached artifact signature was found. | Establish which binaries holders, miners, and the exchange run. Installed v2.1 has no automatic update feed or remote rule switch. Existing binaries need not accept incompatible consensus. |
| **Default-seed DNS controller** | Choose a fresh default node's initial peer endpoints through four unpinned `mseed*.parano1d.org` names. Source code does not identify the domain controller. | Determine whether independent peers and work remain reachable. Configured and persisted peers, candidate work comparison, and install rechecks limit first-contact authority. A DNS change does not evict established connections. |
| **Block producer** | Choose the transactions in blocks it wins, including omitting one otherwise valid transaction while including another. | Measure production share and whether other producers can include the omitted transaction. Recent coinbase concentration is a dated observation, not attribution of hashpower. |
| **Node or pool RPC operator** | A configured `--operator-key` may call `walletSend` for *that node's active wallet*. No default network-wide credential appears in the source. | Establish access to the particular wallet's token, local process, or binary; this does not reach unrelated holders' wallets. |
| **Exchange operator** | Set deposit/trade policies and hold credited balances outside this codebase. | Observe the exchange's NOID deposit status and policy. The project source does not grant exchange control. |
| **Optional market-data host** | With External data enabled, `tradedata.parano1d.org` can supply inaccurate historical DEX candles that pass the GUI's structural checks; current spot and pool balances come from the local node. | Identify host control and whether users enabled the feed or relied on its chart. The feed does not determine native NOID spend validity or exchange credit. |

The [local wallet experiment](../operator-exit/REPORT.md) makes one boundary
concrete: `walletSend` returned a txid with zero peers while an independent
receiver saw nothing, then the same tx propagated after connection. The GUI's
“TRANSACTION SENT” screen therefore says local admission, not exchange
credit. It does not show who could interrupt a holder's peer connection. A
local RPC process that binds first can substitute the GUI's receive address,
but the ordinary GUI uses a sibling local node; a remote seed alone has no
such RPC authority
([wallet boundary](../wallet-boundary/README.md)).

## Evidence needed for an asymmetric-delivery scenario

The scenario requires three observed links: (1) a particular party controls
a miner, peer route, release installation, or exchange deposit service;
(2) that control prevents a specific valid transaction from being credited;
and (3) the party has an independent credited balance or working deposit
route. This review has not established those links together. A current
block-production sample would need attribution beyond payout addresses, and
exchange deposit status would need independent observation; stale explorer
and pool snippets cannot establish either.

The present source defines a real fund allocation, concentrated clean-install
discovery, publisher-dependent binaries, and block-producer discretion. Those
are distinct control surfaces. The reviewed evidence does not connect them
into an observed difference in native NOID transaction delivery between
holders.
