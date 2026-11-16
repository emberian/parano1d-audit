# Can a NOID operator sell while holders cannot?

Source and public-document review, 2026-11-15. Code target:
`52938433a30b185ab007afaa15166a632cf2e82e`. This report concerns native
NOID, not the chain's separately launched assets. It uses no live P2P traffic,
exchange account, or exchange contact. The earlier local wallet experiment is
documented in [REPORT.md](REPORT.md); nothing here claims to reproduce an
attack on SafeTrade or its users.

## The transaction path that matters

An on-chain holder who wants to sell through SafeTrade must produce a signed
NOID spend, get it to a producer, have it included on a chain that SafeTrade
recognizes, meet SafeTrade's deposit conditions, have the balance credited to
their exchange account, and then place a sell order. SafeTrade's [deposit
instructions](https://safetrade.com/help/faq/how-to-deposit) require a chosen
coin/network, deposit address, and minimum deposit amount. Its [terms, §§6.1
and 7.1](https://safetrade.com/terms) distinguish funding a SafeTrade account
from trading the credited assets. The [NOID/USDT market](https://safetrade.com/exchange/NOID-USDT)
is publicly listed. SafeTrade's [currency status page](https://safetrade.com/status)
has separate deposit, withdrawal, and minimum-confirmation fields, but its
publicly indexed first page did not expose the NOID row to this review.
**NOID's current deposit-enabled state, minimum confirmations, minimum deposit,
and exchange node topology remain unverified.** An on-chain wallet's txid and
the GUI's “TRANSACTION SENT” message are not an exchange credit.

An actor who already holds NOID credited inside a SafeTrade account need not
make a new NOID-chain deposit to submit a sale order while the market remains
open. That is a structural distinction, not evidence that the developer has
such a balance, caused a deposit disruption, or owns exchange controls.

## Powers and required extra premises

| Power or route | What is demonstrated or coded | What an attacker would additionally need to obstruct other sellers while selling themselves | Escape or limit |
| --- | --- | --- | --- |
| **Preloaded exchange inventory** | SafeTrade's terms separate funding from spot trading. | An existing exchange balance and a live NOID/USDT market while new deposits are delayed or disabled. The actor could then sell without a contemporaneous chain transfer. | Existing credited balances of *other* holders can also trade; a deposit outage alone does not grant exclusive selling. SafeTrade's account and trading policy is outside this node code. |
| **Default seed DNS** | Four embedded bare-hostname seeds share `parano1d.org` (`noid_chain/src/consensus/network.rs:111-134`), with no PeerID pin in that format. The node dials configured seeds *and* embedded seeds (`noid_node/src/main.rs:2256-2300`). | Control of the zone or replies, a fresh default-install victim, and enough control of discovered peers to keep honest producers out. Author control of the zone is inferred from hosting, not proved by code. | Explicit seeds, locally discovered peers, stored peers, or later honest discovery can bypass first-contact control. Changing DNS alone does not disconnect established nodes, erase their chainwork, or block already credited exchange sales. |
| **Relaying and local wallet acknowledgment** | The local [reproduction](REPORT.md) obtained `walletSend` success with zero connected peers while an independent node had no tx. `server.rs:1189-1208` returns on local admission; `main.rs:2354-2373` gossips asynchronously. | A targeted partition or sustained suppression of every usable submission path available to the holder, while the actor uses a healthy path. | One honest reachable relay/producer defeats the simple partition; the same tx propagated after reconnection in the control. The GUI's success claim is a delivery-status flaw, not proof of exchange acceptance. |
| **Mempool pressure** | Local admission requires the greater of local dynamic floor and consensus fee (`noid_mempool/src/pool.rs:1243-1271`); the floor is local and pressure-latched (`floor.rs:1-102`). Existing lanes show anonymous verify/serving pressure and first-seen slot conflicts. | Saturate the relevant holder's submission/relay path while leaving a private path for the actor. Raising the fee floor requires *valid admitted high-fee txs* plus occupancy; invalid-proof floods alone cannot do it. | `walletSend` auto-fee replans up to three times (`server.rs:1029-1059`), and another node may have a lower floor. Slot conflicts require attacker-controlled inputs, observed output slots, and repeated wins; users can choose new slots. No network-wide selective block has been shown. |
| **Block producer selection** | `validate_block_consensus` checks validity, coinbase, resources, fees, conflicts, roots and state, without maximal inclusion (`noid_chain/src/consensus/validation.rs:574-650`). A producer may omit a victim and include its own valid payment. | Enough sustained production or fork-choice control to prevent honest producers' victim-including blocks from becoming canonical, or independent relay suppression before those producers see it. A producer's ability to omit in one block does not equal a permanent freeze. | Honest producers that see the tx can include it; once a spend is deeply finalized the node refuses a conflicting deep reorg (`consensus/params.rs:241`, `fork_choice.rs:81`). Public block concentration is not proof of developer mining control. |
| **Wallet software or RPC endpoint** | The GUI displays node-supplied state/receipt conclusions; a developer-controlled update or impostor endpoint could mislead a holder. The GUI's regular spend uses its node's `walletSend` (`noid_gui/src/backend.rs:793-829`). A node's configured operator bearer token *can* invoke that node's `walletSend` (`noid_rpc/src/server.rs:3846-3867`). | Install of malicious software, control of the user's selected RPC endpoint, local environment, or that specific node's operator token. The user's wallet secret is locally generated and the token is user-supplied; no developer default key or universal remote wallet credential was found (`../operator-power/REPORT.md`). | Running independently verified software and checking the chain from an independent node are available in principle. There is no reported automatic update path, so release control is not immediate control of already installed wallets. |
| **SafeTrade-specific deposit/trade switch** | SafeTrade publishes separate currency deposit/withdrawal statuses and terms granting it account/trade controls. | SafeTrade's own authority or cooperation, or an operational failure of its NOID integration. We found no public evidence that the NOID developer has this authority or can selectively toggle SafeTrade accounts. | The exchange is a separate actor. A global deposit pause can still leave all previously credited holders able to trade, depending on SafeTrade's independent market policy. |

The first route is the most literal “sell while others cannot deposit” scenario:
preposition inventory inside the exchange, then exploit or benefit from a
deposit disruption. It needs no exotic consensus flaw, but the *cause* of the
disruption and the actor's inventory are unproved. A claim that the developer
can do this deliberately requires evidence about exchange custody, account
balances, or reliable network-wide obstruction, which this audit lacks.

## Public attribution boundary

The previous passive [network-status report](../network-status/REPORT.md)
observed a NOID/USDT listing and a concentrated recent block-production
window. It explicitly could not attribute the largest apparent exchange
address or the dominant solo producer to SafeTrade or the developer. The
[operator-power review](../operator-power/REPORT.md) inferred developer control
of the seed DNS zone from the hosted project infrastructure; it did not show
control of SafeTrade, the independent mining pool, other holders' keys, or
exchange accounts. These observations were snapshots, not a standing present
tense claim about who operates the current network.

SafeTrade's [terms, §6.1](https://safetrade.com/terms) say funding time depends
partly on third parties and is not guaranteed; [§7](https://safetrade.com/terms)
governs orders and trading within its service. The public
[status page](https://safetrade.com/status) supports the existence of
per-currency deposit/withdrawal switches, but did not expose a verified NOID
setting or a rule connecting a NOID deposit pause to NOID/USDT trading.
Nothing in the reviewed public material identifies the NOID developer as a
SafeTrade operator, administrator, or account holder.

## Claims supported today

The node has user-visible local-send acknowledgment before peer delivery,
first-contact DNS concentration, local admission pressure, and discretionary
block inclusion. Each can be part of a selective-exit scenario under additional
attacker powers. We have **no demonstrated developer-only sell privilege, no
NOID address-freeze opcode, no demonstrated live-network selective censorship,
and no observed SafeTrade-specific deposit bypass.** The decisive missing
evidence is actor attribution and the exchange's independent deposit/trading
state during a disruption.
