# Operator-controlled inputs to the shipped GUI and local wallet node

Source-only audit at pinned `52938433a30b185ab007afaa15166a632cf2e82e`.
This traces what control of default seeds, official web services, and releases
actually buys. The separate [README](README.md) contains the local impostor
and payment-builder experiments. This report does not attribute control of
every named service to the developer; DNS-zone ownership and release-account
control require their own evidence.

| Input the operator might control | How the shipped GUI consumes it | Can it substitute a receive address or fabricate a confirmed payment by itself? |
| --- | --- | --- |
| Four embedded `mseed*.parano1d.org` DNS names | The GUI passes any extra seeds to its spawned local node as `--seed` (`backend.rs:1450-1452`); the node adds the embedded DNS set (`main.rs:2256-2304`, `noid_chain/src/consensus/network.rs:111-134`). Seeds provide peer discovery and P2P data. | **No direct RPC authority.** The honest daemon's `walletActiveAddress`/`walletListAddresses` read its local wallet state (`noid_rpc/src/server.rs:2980-2994`), whose active address derives from the local secret (`noid_node/src/wallet/state.rs:249-250`). Its `verifyReceipt` checks Merkle inclusion against the selected local header (`server.rs:2457-2486`). An eclipse or accepted malicious snapshot could change the selected chain, but that is a separate conditional consensus attack; its release-verifier/live-network leg is still under audit. |
| `tradedata.parano1d.org` default media and price-history origin | External data defaults **Off** (`backend.rs:134-136,1636-1638`; `docs/wallet/asset-media.md`). When enabled, HTTPS media is fetched from that host (`assets/media/transport.rs:13-59`). Manifest bytes must match the metadata hash recorded in State; images must match their manifest hash (`transport.rs:86-124`, `assets/media.rs:390-411`). | **No receive/receipt authority.** It can withhold media. It cannot substitute a name, picture, owner or transfer without defeating the committed hashes or the node's State. |
| Same price-history service | With External data On, the GUI checks response size, chain and asset identifiers, internal header continuity, and the last header hash against its node (`backend/market_data.rs:28-89`; `market_data.rs:110-151`). It plots returned candles (`view/assets/chart.rs:55-116`). | **Can misstate historical candles.** The GUI does not rederive the candle from block trades or check every intermediate header against its node. Even a one-block response ending in a genuine tip hash can carry an arbitrary positive candle that satisfies `Candle::valid`. Current spot price and pool balances come from the local node and are labeled as such (`view/assets/chart.rs:13-17,107-123`). This can mislead a trading decision; it does not change consensus execution or create a deposit confirmation. A malicious service could tailor responses per visitor, but no such behavior was observed. |
| Official release channel | GUI selects a sibling node executable, falling back to PATH (`backend.rs:1918-1952`). The GUI release self-check tests that the sibling file exists (`noid_gui/src/main.rs:60-91`). No automatic update path was found in the GUI. | A **manually installed malicious binary** could read `wallet.key` or falsify every wallet response. That is publisher/supply-chain control, stronger than seed or service control, and does not require W7's loopback race. An unsigned checksum from the same release channel cannot establish publisher honesty. This is a conditional publisher power, not evidence a malicious release was shipped. |
| Environment or local process | `NOID_RPC` may point the GUI at an arbitrary HTTP(S) JSON-RPC URL, while the GUI's persisted settings and user-facing Network settings contain no RPC URL (`backend.rs:119-130,1592-1597`; `docs/wallet/settings.md:49`). `NOID_GUI_NODE_BIN` selects a node binary; neither is remotely set through P2P. The default endpoint is `127.0.0.1:9601` (`backend.rs:49`). | A local first-binder or configured RPC impostor can substitute the receiving address and all node-attested confirmation fields; the focused default-port and address/payment tests in README demonstrate the boundary. This **requires local endpoint or launch-environment control**, not only seed/service control. |

The media price-history gap is a narrower, operator-service-only finding than
the wallet-node deception: a fabricated chart can be served to otherwise
uncompromised GUI users who enabled External data. The schema's `Candle::valid`
checks positive values and OHLC ordering, not that those trades happened
(`market_data.rs:20-76`). `History::validate` checks the response's own
header links, not their authenticity (`market_data.rs:110-151`). The last hash
is compared with the user's node by `external_price_history_from`, but candle
content is independent of that hash in the response. The GUI labels its current
spot as local-node data; this limits the deception to the historical plot.

For the rugpull question, the direct path from *default seeds or media service
alone* to an attacker receive address or forged `CONFIRMED` payment was not
found. The two remaining bridges have distinct preconditions: (1) cause an
honest local node to accept a false chain/snapshot despite its verification
and fork-choice gates; or (2) get the user to run a substituted binary or
connect the GUI to a substituted local/RPC service. Neither is implied by
owning a DNS zone or a media host. A selected-chain receipt is a claim about
that node's chain, and `confirmed` tests current canonical inclusion rather
than the 18-block finalized window (`server.rs:2470-2480`); the GUI itself
does not compare independent peers or verify the receipt mathematics.
