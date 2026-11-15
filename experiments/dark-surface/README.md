# Dark surface map: node capabilities vs. GUI/docs exposure

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e` (verified `git rev-parse HEAD`). Target checkout treated read-only; all artifacts live in this directory. Live demonstrations (D1-D5 in `results.txt`) ran against real node binaries of this pinned source, loopback-only, with synthetic keys and scratch data dirs: D1-D4 used the pack-free debug binary at `experiments/wallet-safety/target/cargo/debug/parano1d`; D5 used the full-pack release binary at `experiments/devnet-chaos/target-rel/release/parano1d` (same pin; read-only use — the lane also rebuilt the source itself in `target/`, which links but lacks the v2 pack and refused startup, recorded as a fail-closed control). No mainnet peer, wallet or user file was touched; every started process was stopped by its PID file (`demo/*.pid`, all dead).

Method: enumerate each surface from the code (RPC trait registration, clap definitions, TOML schema, env reads, operation profile tables), then diff against (a) what the GUI actually calls, (b) what `parano1d-cli` calls, (c) what `docs/` states. "GUI" means `noid_gui` source call sites, excluding `#[cfg(test)]` fixtures.

## 1. RPC surface (79 methods, one registration point)

Every method is registered from a single trait: `ParanoidApi` (`noid_rpc/src/api.rs:17-496`, namespace `paranoid_`), implemented once on `RpcHandler` (`noid_rpc/src/server.rs:1593`), converted by `handler.into_rpc()` at `server.rs:3598`. No second module is merged into the server (`grep -rn "into_rpc\|\.merge("` hits only that one site), so this list is exhaustive — 79 methods.

### Credential tiers (server.rs:3729-3914)

| Tier | How obtained | Scope |
|---|---|---|
| Local (owner) | TCP peer is loopback **and no `Authorization` header** (server.rs:3805-3813); or non-loopback bind refused at startup without any key (server.rs:3520-3522) | all 79 methods, including `stop`, wallet spend, secret-free wallet export-by-receipt |
| Mining | `Authorization: Bearer <mining-key>` matching blake3 digest (server.rs:3685-3697) | exactly `getBlockTemplate`, `submitBlock` (server.rs:3846-3848) |
| Operator | matching operator key | 18-method accounting/payout allowlist incl. `walletSend` (spends from the active wallet) (server.rs:3850-3872) |

Fail-closed properties verified live (see `results.txt`): non-loopback listener without credentials refuses startup; equal mining/operator keys refuse startup; browser `Origin` headers are rejected 403 before dispatch (server.rs:3750-3757); the `Authorization` header is removed before jsonrpsee can trace it (server.rs:3759-3767). A supplied-but-wrong credential never falls back to Local (server.rs:3773-3801).

### The diff: reachability of all 79 methods

Full matrix in `method-matrix.txt`. Summary (GUI = `noid_gui` call sites; CLI = `parano1d-cli`; docs = backtipped name anywhere in `docs/`):

- GUI calls 54 methods (it does call `stop`, but only for a node it owns itself — `stop_owned`, backend.rs:1506-1512); CLI calls 45. **25 methods are never called by the GUI**; 14 of those are CLI-reachable (`estimateFeeDetailed getBlock getBlockTemplate getEpochAnchor getHeaderByHeight getHistoryStepTerminal getMempoolEntry getMempoolInfo getMempoolSize submitBlock walletGetAddress walletGetObjectOpening walletHistory walletScan`) and **11 also never by the CLI** — reachable only by a hand-crafted JSON-RPC client on loopback (or a valid operator/mining token where allowlisted):
  `estimateFee`, `getActiveSlotCount`, `getHeaderByHash`, `getNativeHoldings`, `getNativeMarket`, `getSlotHints`, `getSlotHintsSalted`, `prepareNativeIntent`, `submitTxIntent`, `walletMinedBlocks`, `walletStatus`.
- Of those 11, all are documented in `docs/reference/rpc.md` / `native-rpc.md` / `client-native-rpc.md` tables. **The only method with zero documentation anywhere is `walletDismissNativePending`** (docs grep = 0 files) — which is GUI-reachable, i.e. the inverse dark direction: live in the UI, absent from docs.

Class notes on the 11 GUI+CLI-dark methods: all are READ-INFO except `submitTxIntent` (raw `PagedSpendIntent` hex straight into mempool admission, server.rs:2428-2436 — STATE-MANIP class but proof-gated; it is operator-allowlisted, so a stolen operator token can inject arbitrary crafted intents). `stop` (CLI+docs reachable; GUI uses it only to stop a node it owns) kills any daemon from any loopback process — server.rs:3018-3029; demonstrated below. `prepareNativeIntent` enumerates any address's full live UTXO set (up to 16,384 sources) into an external-client plan (`noid_rpc/src/server/native/client.rs:14-64`) — same class as the known `getSlotsByOwner` local-only enumeration fragment. `getMempoolInfo`/`getMempoolEntry` dump full pending-tx metadata (CLI-reachable, documented). No RPC method returns key material (wallet-safety lane, verified: the master secret never transits RPC).

### State-mutating / dangerous methods, by tier

- Local only: `walletSend`, `walletConsolidate`, `walletFundObject`, `walletCallObject`, `walletSubmitNative`, `walletSetActiveAddress`, `walletScan`, `walletNextAddress`, `walletWatchObject`, `walletImportObjectReceipt`, `walletDismissNativePending`, `createObject`, `stop`.
- Local or Operator: `walletSend`, `walletConsolidate`, `walletPlanSend` (allowlist at server.rs:3850-3872 — accepted, documented design that the operator token can spend).
- Local or Mining: `getBlockTemplate`, `submitBlock` (mining tier); custom coinbase inside the template additionally requires `--mode extminer --allow-custom-coinbase` + mining key (server.rs:1271, CLI gate main.rs:1860-1862; demonstrated by the external-miner lane).
- Raw object/intake: `submitTxIntent` (hex bytes, bounded `MAX_TX_INTENT_BYTES_GLOBAL`), `createObject` (full `ObjectDefinition` incl. custom program — but the GUI contract file-import path also reaches it; limits enforced in `noid_rpc/src/server/objects.rs`).

## 2. CLI surface

Shipped binaries (release build `scripts/build_release.sh:322-344`): `parano1d` (node), `parano1d-cli` (thin client), `parano1d-miner` (extminer worker), `parano1d-gui`. Also `pruv` (contract/native-op tool) built separately; `bench_prover` and matrix tools are developer-only.

### `parano1d` hidden flags (defined `noid_node/src/main.rs:1235-1381`; `--help` output captured in `demo/help.txt`)

| Flag | hide | Class | What it does |
|---|---|---|---|
| `--genesis` | yes (1266) | STATE-MANIP/FOOTGUN | permits isolated block production without peer quorum (mining gate). Undocumented in docs (grep = 0). GUI retains spawn plumbing (`noid_gui/src/backend.rs:1447-1449`) and an orphaned i18n string "Genesis mode" (`noid_gui/src/i18n.rs:856`), but no code path sets `genesis=true` anymore — the toggle UI was removed pending the "public GUI merge" (`noid_gui/Cargo.toml:11-12`). |
| `--disable-dns-seeds` | yes (1301) | NETWORK-SHAPE | skips the embedded DNS bootstrap set; used by every local test harness (also by this lane and devnet-chaos). |
| `--export-wallet-secret` | yes (1371) | **KEY-MATERIAL** | prints the wallet master secret as 64 hex chars to stdout (main.rs:1989-1994). This is exactly what the GUI Export dialog invokes (`run_wallet_maintenance(..., "--export-wallet-secret")`, backend.rs:552); docs instead tell users to copy `wallet.key` (maintenance.md:43-49). Demonstrated below. |
| `--import-wallet-secret` | yes (1375) | **KEY-MATERIAL** | reads 64 hex chars from **stdin** and atomically replaces the wallet (main.rs:1995-2005). GUI Import uses it (backend.rs:582,667). Replaces the spending authority of a data dir in one command. |
| `--prepare-history-step-cache B25\|B255` | yes (1379) | benign | materializes packed HistoryStep matrix cache images (main.rs:2006-2041). |

Guardrails verified in source: wallet-maintenance flags refuse combination with any mining/node action (main.rs:1863-1880); extminer requires a mining key (1850-1852); `--allow-custom-coinbase` requires extminer (1860-1862). `--purge-state` is documented and fails safe: it only clears when the stored epoch marker mismatches this network, otherwise refuses (main.rs:1662-1711). `--v2-large-blocks` is documented (docs/reference/cli.md, parameters.md).

### `parano1d-cli` (noid_node/src/bin/noid_cli.rs:146-380)

23 subcommands + `contract` subtree (watch/payment/vault/allowance/budget/recurring/vesting/protocol/create/fund/status/instances/call/receipt...). Everything it calls is in the allowlisted/documented RPC set; `contract create` accepts an SDK definition JSON "including custom programs" — the arbitrary-contract intake path (documented in docs/contracts/). `--rpc` endpoint from env `NOID_RPC` (documented). No hidden subcommands (no `hide = true` in this binary).

### `parano1d-miner` (noid_extminer/src/main.rs:63-108)

Plain flags only (`--rpc/--key/--key-file/--threads/--coinbase/--poll-ms/--rpc-timeout/--blocks/--log/--check-hardware`); no hidden flags; custom coinbase honored only when the node opts in (external-miner lane demonstrated enforcement).

### `pruv` (noid_pruv/src/main.rs:15-75)

`check|build|smt|verify` + `native check|build|submit` — offline contract compilation and native-op request building. Notably `native build` can emit a `walletPreviewNative` request for ops the GUI can build; it cannot sign or submit by itself.

## 3. Config and env surface

Config file (schema `noid_node/src/config.rs`; `docs/operate/configuration.md` documents almost all keys): `network.listen`, `network.seeds`, `network.public_addresses` (advertised reachability — **absent from configuration.md's "complete file" example**, only in the code's doc comment, config.rs:30-35; consumed at main.rs:1940-1944), `storage.backend` ("mdbx"|"ram" — RAM is test-only, documented), `storage.path`, `rpc.listen`, `mining.enabled`, `mining.miner_address`. Malformed TOML fails startup, never silently replaced (main.rs load_or_create_config). CLI flags override file values (main.rs:1923 comment). Note the default `network.listen` binds `0.0.0.0:9600` (public P2P by default, documented) and the GUI-spawned node inherits the same default (`noid_gui/src/backend.rs:51`).

Env vars read (code-verified):

| Var | Where | Documented | Note |
|---|---|---|---|
| `NOID_RPC` | cli (noid_cli.rs:152), gui (backend.rs:1592) | yes | RPC endpoint |
| `NOID_GUI_SETTINGS_FILE`, `NOID_GUI_DATA_DIR`, `NOID_GUI_RPC_LISTEN`, `NOID_GUI_P2P_LISTEN`, `NOID_GUI_SEEDS`, `NOID_GUI_LOG`, `NOID_GUI_LANGUAGE` | backend.rs:1581-1634 | **no** | GUI config override; `NOID_GUI_RPC_LISTEN` choosing a non-loopback value would make the GUI spawn the node with that listener (the node then requires credentials and refuses to start — fail-closed, but a confusing silent-break) |
| `NOID_GUI_NODE_BIN` | backend.rs:1604 | **no** | GUI spawns this binary as its node (and runs `--check-hardware`/maintenance through it). Supply-chain footgun: whatever the env points at runs with the user's data dir. |
| `NOID_GUI_MOCK` | backend.rs:1639 | no | mock (presentation-only) GUI mode |
| `NOID_GUI_PREVIEW_SCREEN` | assets.rs:281 | no | screenshot/testing hook |
| `NOID_HISTORY_QUOTA_MIB` | media service (assets/media/service.rs:365) | yes | price-history cache quota |
| `NOID_GUI_MEDIA_ORIGIN` | media service | yes | remote media origin |
| `NO_COLOR`/`TERM`/`CI`/`HOME`... | cosmetic/path | — | — |

Build-time env (pack embedding, `noid_node/build.rs:22-32`): `NOID_HISTORY_STEP_PACK_DIR`, `NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST`, `NOID_V2/V21/AUTHORIZATION_PACK_DIR` + bank pins, `NOID_RETIREMENT_KEYS_DIR`... — these select what proof material is embedded in the binary. Not runtime surface, but they are the mechanism behind the version-domains lane's finding that different builds silently carry different consensus capacity (a pack-free or finance-pack build refuses v2.1-era startup outright — "scheduled authorization runtime unavailable", `noid_miner/src/history_protocol.rs:123`; observed live, see results.txt — rather than degrading).

## 4. Profile gates: the complete operation table

Two gates compose: the shipped **operation profile** (`noid_tx/src/native_assets/profile.rs`) and per-mode node behavior. Ops exist as complete code in `noid_tx/src/native_assets/financial.rs:188-222` (codes), `transition.rs:39-58` (base ops). Enforced identically at construction (`financial.rs:224`), wire decode (`wire.rs:975`) and inside the recursive acceptance trace (`noid_recursive/src/acceptance/trace/native_assets/header.rs:260`, `financial_operation.rs:66-73`) — the gate is consensus-level, so disabled ops cannot be proven even by a crafted client.

39 operations total:

| Codes | Operation family | Enabled | Wallet/GUI builder | Who can execute |
|---|---|---|---|---|
| 1 CreateToken, 4 TopUpReserve | standalone fungible issuance | **no** (transition.rs:54) | — | nobody (dead; issuance is launch-only) |
| 2 TransferToken, 3 RedeemToken | token move/burn | yes | GUI `TransferToken`/`BurnToken` | wallet |
| 5 CreateCollection, 6 MintNft, 7 TransferNft, 8 BurnNft | NFT base | yes | GUI | wallet |
| 32 BootstrapPool, 34-37 Deposit/Withdraw/Burn/TransferLiquidity, 42 ClaimLiquidity | manual pool liquidity | **no** | — | nobody (liquidity forms only via launch graduation; docs describe exactly that) |
| 33 Swap | DEX swap | yes | **no builder anywhere** (grep: no construction site outside decode/validate) | hand-crafted intent via `submitTxIntent` only |
| 38 RegisterPools, 39 SettlePool, 40 RefundMarket, 41 ResetPool, 43 ClaimSwap | pool sequencing | yes | — | block producer only (`check_financial_public` requires authority = zero address, `financial/public.rs:348-355`) |
| 48 CreateLaunch | token launch | yes | GUI `CreateToken` (with optional initial buy) | wallet |
| 49 LaunchRequest | curve trade request | yes | **no builder anywhere** | hand-crafted intent only |
| 50-53 Register/Settle/Refund/ResetLaunches | launch sequencing | yes | — | block producer only |
| 64 CreateListing, 65 Purchase, 66 CancelListing | NFT instant-sale listings | yes | GUI List/Buy/Cancel | wallet |
| 67-75 CreateOffer..RefundOffer (offers + funded rounds/escrow) | NFT offers & round settlement | **no** (profile omits 67-75; `kind_enabled` also omits `NftPurchase`/`NftOffer` openings) | — | nobody (U20, nft-marketplace lane) |
| 80 ExecuteTrades | trade settlement | yes | — | block producer (synthesized by template builder from mempool `TradeIntent`s, `noid_miner/src/template.rs:598`) |

Dark classes found here: (a) **implemented-but-never-enabled**: 17 ops (1, 4, 32, 34-37, 42, 67-75) — dead code, consensus-gated, undocumented (no doc claims them, verified); (b) **enabled-but-never-built**: `Swap` (33) and `LaunchRequest` (49) — valid in consensus, owner-executable, but no shipped GUI/CLI/miner code constructs them, and `git log -S "FinancialOperation::Swap"` / `-S "FinancialOperation::LaunchRequest"` over all 740 commits shows **no construction site ever existed** (contrast: `CreateListing` appears in the planner's history). The node will admit and prove an operation no official client can produce; only a hand-crafted intent via `submitTxIntent` (built with the workspace crates) can execute them; (c) **sequencer-only**: 10 ops whose authority must be the zero address — reachable only by whoever produces blocks (a rogue operator's weaponizable set; see malicious-node cross-check below).

Kind gates mirror this (`profile.rs:35-70`): `NftPurchase`/`NftOffer` openings rejected; `sequential_kind_enabled` omits `MarketRequest`/`MarketClaim` (round requests only exist in the disabled/manual families).

Node build-profile variants (`noid_chain/Cargo.toml:9-12`, params.rs:24-114): `isolated-v1-1-testnet` (v1.1 at height 5), `isolated-v2-fork-testnet` (v2 at 10), `isolated-v2-1-fork-testnet` (v2.1 at 24). These are not shippable dark modes: the daemon refuses to run them without explicit config/data-dir/loopback listeners **and** a network namespace whose only interface is `lo` (`main.rs:1735-1769`). Release builds embed mainnet activations (params.rs:19-23).

## 5. Maintenance and diagnostic paths

- **Wallet maintenance CLI** (`--export/--import-wallet-secret`): the only `println!` of secret material in the tree; secret arrives via stdin, never argv (wallet-safety lane verified `ps`-invisibility, 0600 keystore, O_NOFOLLOW). GUI drives the same flags (backend.rs:1731-1770) — and the GUI's exported-secret clipboard persistence issue is wallet-safety Finding 2.
- **`paranoid_stop`** (Local tier): any loopback process can gracefully kill the daemon (server.rs:3018-3029). Demonstrated below. The GUI calls it only for a node it owns itself (`stop_owned`, backend.rs:1506-1512, with a process-kill fallback); the CLI exposes `parano1d-cli stop`.
- **No admin socket, no signal-triggered dumps**: `tokio::signal::ctrl_c` is the only signal handler (main.rs:2651); no SIGUSR-style debug dumps exist (grep across shipped crates). Log filter is `--log` (an EnvFilter directive string — accepts per-target syntax, documented as "tracing filter"); `RUST_LOG` itself is not read (EnvFilter::new at main.rs:1819). No `tracing::{debug,trace,info}` statement logs secret/token/key material (grep over wallet + main: only paths and non-secret ids); Authorization is stripped before jsonrpsee tracing (server.rs:3759-3767).
- **`getNodeStatus`** leaks internal queue depths (p2p_control_queue, header_queue, data_queue, pending_requests, relay_reservations) — GUI-reachable, informational.
- **Media service** (`noid_gui/src/assets/media/service.rs:362-367`): a second HTTP listener in the GUI process; refuses non-loopback bind; sandboxed CSP responses; documented (docs/reference/media-service.md).
- **P2P control surface**: `NetworkCommand` (noid_p2p/src/network.rs:3117-3157) has dial/announce/broadcast commands, but no RPC method exposes them — the only `p2p_cmd` use on the RPC path is the node announcing its own `submitBlock`-sealed block (server.rs:1570-1586); RPC reads only `getPeerCount` (server.rs:2324) and `getNodeStatus` health fields. Negative result: no network-shape control from the RPC tier.

## 6. Dark-surface table (the deliverable diff)

"UI" = GUI; "docs" = any doc mention; tier = minimum credential needed.

| Capability | UI | CLI | docs | Tier | Class |
|---|---|---|---|---|---|
| `--export-wallet-secret` prints master secret | via GUI Export dialog only | hidden flag | no (told to copy file) | local user | **KEY-MATERIAL** |
| `--import-wallet-secret` replaces wallet from stdin | GUI Import only | hidden flag | no | local user | **KEY-MATERIAL** |
| `--genesis` isolated block production | removed toggle (plumbing remains) | hidden flag | no | local user | STATE-MANIP / FOOTGUN |
| `--disable-dns-seeds` untrack network | no | hidden flag | no | local user | NETWORK-SHAPE |
| `paranoid_stop` kills daemon | only for a GUI-owned node | yes | yes | Local (any loopback process) | STATE-MANIP (DoS) |
| `submitTxIntent` raw intent injection | no | no | yes | Local + Operator | STATE-MANIP (proof-gated) |
| `prepareNativeIntent` + `getSlotsByOwner` arbitrary-address UTXO enumeration | partial (getSlotsByOwner yes) | yes / no | yes | Local | READ-INFO |
| `getMempoolInfo`/`getMempoolEntry` full pending set | no | yes | yes | Local | READ-INFO |
| `getNativeMarket`, `getNativeHoldings`, `getActiveSlotCount`, `getHeaderByHash`, `getSlotHints[Salted]`, `estimateFee`, `walletStatus`, `walletMinedBlocks` | no | no | yes | Local (walletMinedBlocks also Operator) | READ-INFO |
| `walletDismissNativePending` | yes | no | **no** | Local | STATE-MANIP (dismiss local pending record) |
| `Swap` (33), `LaunchRequest` (49) consensus-valid owner ops | no | no | (economics only) | Local + custom client | STATE-MANIP (protocol-capable, unbuilt) |
| Ops 67-75 + liquidity family (17 ops) | no | no | no | **nobody — profile-gated** | dead code |
| Sequencer ops (38-41,43,50-53,80) | no | no | (economics only) | block producer only | STATE-MANIP (producer privilege) |
| custom coinbase payout | no | miner `--coinbase` | yes | Mining key + node opt-in | STATE-MANIP (reward routing) |
| `--v2-large-blocks` class switch | no | flag | yes | local user | STATE-MANIP (block capacity) |
| `--purge-state` chain wipe | no | flag | yes | local user | STATE-MANIP (fail-safe gate verified) |
| `NOID_GUI_NODE_BIN` GUI spawns arbitrary binary | n/a | n/a | no | user env | FOOTGUN |
| `NOID_GUI_*` overrides (7 undocumented vars) | n/a | n/a | no | user env | FOOTGUN |
| ops 1/4 CreateToken/TopUpReserve | no | no | no | nobody | dead code |

## 7. Demonstrated capabilities (reproductions in `results.txt`)

1. **Hidden flags work though absent from `--help`** (`demo/help.txt`): export/import/genesis/disable-dns-seeds/prepare-history-step-cache appear nowhere in help; `--export-wallet-secret` still runs.
2. **Master-secret roundtrip on a scratch dir**: `--import-wallet-secret` installs a synthetic 64-hex secret (wallet.key 48 bytes, 0600), `--export-wallet-secret` prints the identical secret to stdout. One command replaces the spending authority of a data dir.
3. **Credential fallback matrix (live)**: `--rpc-listen 0.0.0.0:…` without keys → "requires --mining-key[-file] or --operator-key[-file]"; equal mining/operator keys → "must be different". Both refuse before any listener opens.
4. **Loopback = owner for every process** (D5, full transcript in `results.txt` + `demo/d5-part*.txt`): with the real node running loopback-only and no credentials, an unrelated `curl` (no GUI, no CLI, no token — only `Content-Type: application/json`, which jsonrpsee requires) calls GUI+CLI-dark methods successfully: `walletStatus` (full wallet posture), `getActiveSlotCount`, `getNativeMarket` (state-root-anchored market detail), `getSlotHints`, `getSlotsByOwner` (arbitrary-address UTXO enumeration), `walletDismissNativePending` (returns success), `submitTxIntent` (reaches mempool decode: `Wire(BadMarker)` on garbage — proof-gated past that), and **`paranoid_stop` — the daemon exited** ("RPC stop command received — initiating graceful shutdown"). Browser-origin control: 403. Any local process can stop the daemon, read wallet posture, or enumerate any address's UTXOs; nothing distinguishes "the GUI" from "malware on the same machine" (accepted same-host trust model, but it makes every local process part of the node's TCB).
5. **Pack-gated startup refusal**: a pack-free/finance-only binary refuses startup on mainnet v2.1 ("scheduled authorization runtime unavailable"); a build with the legacy pack but without the v2 pack refuses with "scheduled v2 HistoryStep runtime unavailable". The runtime pack surface fails closed — availability-relevant, never silently degrading (both observed live, `results.txt` D4/D5 header).

## 8. Negative and control results (checked and properly gated)

- Non-loopback RPC without a credential: **refuses startup** (live demo 3; also main.rs:1199-1210).
- Wrong/duplicate Authorization on loopback: 401 before dispatch, no fallback to Local (server.rs:3773-3815; unit tests 4405-4533).
- Browser-origin requests: 403 pre-dispatch (server.rs:3750-3757).
- Mining credential: hard-restricted to the two template methods (server.rs:3846-3848; external-miner lane's live PoW controls).
- Custom coinbase: needs extminer mode + mining key + explicit opt-in (main.rs:1860-1862, server.rs:1271; external-miner lane demonstrated `BadCoinbaseOwner`).
- `--purge-state`: refuses to delete a genesis-matching or unmarked-empty DB; only purges on epoch mismatch (main.rs:1662-1711).
- Disabled native ops (incl. 67-75): gated at operand(), wire decode **and** recursive acceptance — unreachable even by crafted clients (nft-marketplace lane end-to-end).
- Isolated fork-test builds: refuse non-namespaced, non-loopback, DNS-enabled runs (main.rs:1735-1769).
- No RPC method exposes p2p dial/ban/announce; no admin socket; no signal-triggered state dumps; no secret material in logs (greps cited in §5).
- Keystore hygiene (from wallet-safety lane, re-verified live here): wallet.key 0600/48 bytes; secret via stdin only; no symlink load.
- `--mining-key` in argv exists (visible in `ps` to same-user processes) but the documented owner-only `--mining-key-file` alternative enforces 0600+ownership (main.rs:1139-1190).

## 9. Assurance limits

- Reachability was mapped by static call-site analysis of GUI/CLI sources plus targeted live calls; a dynamic trace of the GUI could call methods only at runtime-triggered sites (all call sites are direct string literals, so the risk is low but not zero).
- The live node ran a fresh mainnet-profile chain (height 0, no peers); methods that need synced state or funded wallets were exercised only to the extent they answered on that state (empty enumerations, error paths) — their full-state behavior is taken from source reading.
- The `Swap`/`LaunchRequest` "unbuilt" claim rests on exhaustive `grep` of construction sites in shipped crates plus reading the planner; research/ and bench harnesses were excluded from "shipped" but checked for builders too (none found outside noid_tx's own tests).
- Danger classification is by realistic threat as requested; the operator-token threat model assumes the documented deployment (private/firewalled transport).
- The D5 live node was the devnet-chaos lane's release build of the same pinned commit with the full pack set (v1 history + v2 + v21 pins in its build log), not an artifact rebuilt start-to-finish by this lane (this lane's own rebuild linked but lacked the v2 pack — itself recorded as the fail-closed control in D5's header). Any behavior dependent on different pack sets (retired-history builds) was not run live.
- `prepareNativeIntent` was exercised with a malformed request shape (decode error) — reachability proven, full plan output not (needs a funded owner).
