# External inputs to the shipped GUI and local wallet node

Source-only audit at pinned `52938433a30b185ab007afaa15166a632cf2e82e`.
This traces the authority of default seeds, external data services, installed
releases, and the GUI's RPC endpoint. The separate [README](README.md) records
the local endpoint substitution and payment-builder experiments. Service and
release-account ownership was not established by this source review.

The shipped GUI normally queries its own local node. A P2P peer or external
data service does not directly answer the GUI's wallet RPC. Endpoint
substitution requires a process that answers the local port or a configured
RPC URL; a substituted installed binary has broader access. A P2P influence
on the honest node's selected chain is a separate consensus question.

| Input | How the shipped GUI consumes it | Wallet or user-visible authority |
| --- | --- | --- |
| Four embedded `mseed*.parano1d.org` DNS names | The GUI passes any extra seeds to its spawned local node as `--seed` (`backend.rs:1450-1452`); the node adds the embedded DNS set (`main.rs:2256-2304`, `noid_chain/src/consensus/network.rs:111-134`). Seeds provide peer discovery and P2P data. | No direct RPC authority. The local daemon's `walletActiveAddress`/`walletListAddresses` read its wallet state (`noid_rpc/src/server.rs:2980-2994`), whose active address derives from its local secret (`noid_node/src/wallet/state.rs:249-250`). Its `verifyReceipt` checks Merkle inclusion against the selected local header (`server.rs:2457-2486`). An eclipse or accepted false snapshot could change the selected chain; the release-verifier/live-network leg of that conditional route remains under audit. |
| `tradedata.parano1d.org` default media origin | External data defaults **Off** (`backend.rs:134-136,1636-1638`; `docs/wallet/asset-media.md`). When enabled, HTTPS media is fetched from that host (`assets/media/transport.rs:13-59`). Manifest bytes must match the metadata hash recorded in State; images must match their manifest hash (`transport.rs:86-124`, `assets/media.rs:390-411`). | The origin can withhold media. Substituting a name, picture, owner, or transfer would require defeating the committed hashes or the node's State. It has no receive-address or receipt authority through this path. |
| Same service's price-history endpoint | With External data On, the GUI checks response size, chain and asset identifiers, internal header continuity, and the last header hash against its node (`backend/market_data.rs:28-89`; `market_data.rs:110-151`). It plots returned candles (`view/assets/chart.rs:55-116`). | **Historical candles can be misstated.** The GUI does not rederive candles from block trades or check every intermediate header against its node. A one-block response ending in a genuine tip hash can carry an arbitrary positive candle satisfying `Candle::valid`. Current spot price and pool balances come from the local node and are labeled separately (`view/assets/chart.rs:13-17,107-123`). This can affect a trading decision; it does not change consensus execution or create a deposit confirmation. Responses could differ by visitor; this review did not observe live service behavior. |
| Installed GUI and node binaries | The GUI selects a sibling node executable, falling back to PATH (`backend.rs:1918-1952`). Its release self-check tests that the sibling file exists (`noid_gui/src/main.rs:60-91`). No automatic update path was found in the GUI. | A substituted binary, if installed and run, could read `wallet.key` or alter wallet responses. This is an installed-software trust boundary, distinct from seed and data-service inputs. A checksum published through the same channel alone would not independently establish the binary's provenance. No release substitution was observed. |
| Launch environment and local RPC listener | `NOID_RPC` may point the GUI at an arbitrary HTTP(S) JSON-RPC URL, while persisted and user-facing Network settings contain no RPC URL (`backend.rs:119-130,1592-1597`; `docs/wallet/settings.md:49`). `NOID_GUI_NODE_BIN` selects a node binary. The default endpoint is `127.0.0.1:9601` (`backend.rs:49`). | A process that binds the default port before the node, or a service selected through `NOID_RPC`, can supply receiving addresses and node-attested confirmation fields. The focused default-port and address/payment tests in README demonstrate this boundary. Neither variable is set through P2P. |

The price-history gap concerns the historical chart for users who enable
External data. The schema's `Candle::valid` checks positive values and OHLC
ordering, rather than whether the trades occurred (`market_data.rs:20-76`).
`History::validate` checks the response's own header links
(`market_data.rs:110-151`). `external_price_history_from` compares the last
hash with the user's node, while accepting candle content independently of
that hash. The GUI labels current spot as local-node data, limiting the
presentation discrepancy to the historical plot.

An honest local node's selected-chain receipt is a claim about that node's
chain view. Its `confirmed` value tests current canonical inclusion, rather
than the 18-block finalized window (`server.rs:2470-2480`); the GUI does not
compare independent peers or verify the receipt mathematics. Whether network
inputs can make that node select a false chain depends on the separate
snapshot and fork-choice analyses. This source review found no direct path
from the default seeds or external data service to the local wallet's
receiving address or a fabricated `CONFIRMED` payment.
