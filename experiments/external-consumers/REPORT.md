# External consumers of the parano1d raw commitment APIs

*Probe lane, 2026-11-15. Verdict: no consumer of the cross-domain raw surface exists; the flat "no external consumer" sentence is nevertheless FALSE as drafted — a production fork (Jetsam) inherited everything and added its own `hash_pair` caller. (Landed by the coordinator from the lane's final report; clones and Kagi logs are under `repos/` and `logs/`.)*

## Verdict, precisely

1. **The claw's verify routine is crate-private.** `verify_source_batched_merkle_proof_to_cap` is `pub(crate)` (`noid_fri_binius/src/interleaved_commit.rs:315` at the pin). GitHub-wide code search: **zero external references**. No fork of parano1d modifies anything (all 10 GitHub forks are ahead=0 stale mirrors; 0 forks of the proof-native mirror; 0 on GitLab). The externally nameable raw surface is: `noid_fri_binius::{MerkleCap, interleaved_commit_cap, expand_source_batched_merkle_proof_to_cap, CommitmentHashBackend}` (pub use), `noid_ivc_core::merkle::hash_pair` (pub, IVCPCSN_ domain, `merkle.rs:125`), the legacy `noid_fri` verifier, and the two single-domain pub wrappers `capsule_verify` / `zk_capsule_pcs_verify` — each wrapper fixes its own hasher and cannot be pointed at the other domain.
2. **Every external consumer found is wrapper-level, single-domain, or pure RPC.** None crosses domains; none compares digests across CAPSNODE/IVCPCSN.
3. **Nobody keys a cache or index on cap bytes** — all caches found key on receipt ids, block heights, slot/tx ids.
4. **Docs never teach the raw APIs** (0 hits for all three identifiers across in-tree `docs/` and the parano1d-docs repo/site).
5. **But external consumers of the crates DO exist**, including a third-party production fork — see hits.

## Hits classified

**Third-party (the real finds):**
- **noidwallet/noid-wallet** (noidusa.com; Primal/@Solid_Primal + @0xdavee_; X @No1d_Wallet) — Android light wallet, **git-deps `github.com/ignotusnemo/parano1d` rev 848209d (v2.1.0)**: noid_core, noid_poseidon2b, noid_tx, noid_gkr. Runtime **prove on phone** via `noid_gkr::wallet_authorization::prove_paged_spend_authorization_for_version`, serializes `bundle.to_bytes()` → relay; honest SPV header chain from a built-in checkpoint (TAG_BLOCKHDR/TAG_POWHDR), explicitly defers "slot proofs (phase 2)"; wrapper-verify in tests only. Rev 848209d is **1 docs-only commit behind audit pin 52938433** — effectively the audited code. No IVC domain, no cap cache (relay caches headers by height).
- **Aquacongas/Parano1c** — Android mobile node built **on parano1d v2.0.3 source**; added crates (mobile_ffi/mobile_node/wallet/sync_apply/networking) are clean; only noid_history_runtime touches noid_ivc_core, and only `field_r1cs` matrix artifacts. v2.0.3 tree has every raw API at **identical line numbers/visibility**.
- **gustlborg/gb-parano1d-permanode** — archiver/explorer backend; **git-deps the canonical Forgejo at tag v2.0.3** (noid_chain, noid_tx, noid_poseidon2b) for byte-identical tx-field decode; receipt verify delegated to node RPC `paranoid_verifyReceipt`; relays v2 proofs between permanodes as opaque block data; SQLite keyed slot/tx/block ids.
- **noidium/parano1d-mcp** (crates.io, 21 dl) — pure JSON-RPC MCP bridge, zero noid deps.
- **jetsam-chain/jetsam** (JTM, own mainnet since ~Sept 2026, org created 2026-09-04) — **full hard-fork of parano1d v1.0.0** (baseline 6a836a5 in-history; CHANGES-FROM-UPSTREAM.md), entire stack renamed `jetsam_*` including `jetsam_ivc_core::merkle::hash_pair` (:125), `jetsam_fri_binius::{MerkleCap :18, verify_source_batched_merkle_proof_to_cap :406 pub(crate)}`. Regenerated all 31 domain.rs tags (CAPSNODE→`E_CAPSND`) and round constants — **but left the merkle.rs local const `IVCPCSN_` byte-identical**. **Added a NEW direct `hash_pair` consumer**: `jetsam_client_agent_batch` builds client-batch Merkle trees (IVCPCSN_ domain) and proves them in-circuit, carried in blocks since v1.5. Single-domain today; C21's structure inherited verbatim under a re-instantiated hash. Plus satellites: jetsam-web-wallet (WASM proving in browser, SOURCE.txt-pinned build), CUDA/CPU miners, explorer, whitepaper.
- Miners (lucasan123 x2, gustlborg x3 incl. a NOID+JTM dual miner, SerpentXSF, Aw3n, SecretArrow x2), peakpool + parano1d-pool.fun (+explorer), noidexplorer.org — PoW/RPC/template class, never touch commitments.
- **Echo-Merlini/parano1d-h132-reproduction** — independent reproduction of the H132 soundness experiment tied to ethresear.ch/t/25730 ("Ethereum lessons from a live end-to-end PQ proof-native protocol") — external engagement with the *soundness claims*, not the raw APIs.

**Author-adjacent, out-of-tree:** `agcore` (agent verifier, vendored 2.0.1, wrapper-level incl. `verify_history_step_terminal_candidate`; receipt cache keyed receipt_id, ≤8, rechecked each request); `parano1d-sdk` (TS = RPC; Rust authorization crate vendors 6 noid crates, uses `noid_gkr` wrapper verifiers + own header/tx-tree hashing with consensus tags; no caching); `paramo1d` (Android messenger+node; wrappers; its courier API spins a raw zero-IV Poseidon2bSponge — new app-layer sponge, different construction, context only); `parano1dx` (EVM/Solana vaults — **no noid crates**; its MerkleCap is Plonky3's namesake; touches parano1d only via HKDF of a wallet secret); `parano1d-soundness`/`frost-gkr`/`o1-lab` (analysis/research, no shared identifiers); **Zenodo 23778742 = the author's own paper** whose public statement is literally "a request identifier and a 32-byte authority commitment," with a reproducibility artifact shipping authenticated production sources — the commitment is a published academic interface.

**Dismissed:** forager-wallet/addr (multi-coin, address layer, no noid deps), kernoid/devflow/ndpm (name collisions), avotrom vault repo (empty), parano1dloveu.github.io (Kanye West squatter page), miasma.cpatino.com (RAG/corpus mirrors of public Cargo.toml text; unreachable directly), all 10 parano1d forks (unmodified), bytwork.com (mining press).

## Auditable search log (query + tool)

- crates.io API: direct `/crates/{noid_fri_binius,noid_ivc_core,noid_fri,noid_core,noid_poseidon2b,parano1d}` (none published); searches `q=noid`, `q=parano1d`.
- gh api: `users/{proof-native,ignotusnemo,noidium}/repos`; `orgs/proof-native` (not an org); forks of proof-native/parano1d and ignotusnemo/parano1d + per-fork HEAD & compare (all ahead=0); `repos/proof-native/parano1d/stargazers` (404).
- gh search code: `"verify_source_batched_merkle_proof_to_cap"` (**0 GitHub-wide**); `"noid_fri_binius"` (only ignotusnemo + jetsam); `"MerkleCap" noid` (0); `noid_ivc_core merkle` (0); `"git.parano1d.org"` (author repos + permanode Cargo.toml); `TAG_CAPSNODE`, `IVCPCSN`, `capsnode`, `noid_extminer` (parano1d, parano1d-soundness, jetsam only).
- gh search repos: `parano1d` (full ecosystem list above).
- Forgejo API `git.parano1d.org/api/v1/repos/search` (9 repos, all ignotusnemo); GitLab API `users/ignotusnemo/projects` (3 mirrors, no forks).
- Kagi: `parano1d block explorer NOID`; `parano1d SDK integration library`; `parano1d H132 soundness ethresear.ch`; `zenodo 23778742`; `"noid_fri" crate`; `parano1d third-party tools wallets miners list` (first batch of 4 consumed by a client-side parse bug, rerun; logs in `logs/`).
- scry twitter.tweets: `parano1d` + {wallet|explorer|sdk|pool|miner|bridge|library} — 12 newest of ≥143 rows: author's parano1dx SOL/ETH demos and PQ-test RTs; no third-party tool beyond GitHub/crates.io finds.
- Direct fetch: zenodo.org/records/23778742 (paper + artifact).
- Local: grep pinned `docs/` for the three raw identifiers (0 hits); shallow clones + targeted greps of all 16 repos under `repos/`.

## Public sentence for the audit (replaces the drafted one)

Full-scope variant:

> As of 2026-11-15, no external code exercises the cross-domain raw verify surface: the verify routine is crate-private, every out-of-tree consumer we found (mobile wallets, SDK, agent verifier, archivers, pools) calls only single-domain wrapper entry points or plain RPC, and none keys storage on cap bytes. The finding is therefore a latent footgun in public commitment-handling APIs, not a live exploit — with one caveat: a production third-party fork (Jetsam) ships the same raw API surface under new tags and has already added its own direct consumer of the IVC-domain node hasher, so the class of bug is one refactor away from being live outside this codebase.

Short variant (fork out of scope):

> As of 2026-11-15 no external consumer of the raw cross-domain APIs exists; all third-party tooling consumes single-domain wrappers or RPC, and no cache or index keys on cap bytes. The claw is a latent footgun in a public API, not a live exploit.

## Version-skew footnote

noid-wallet @ 848209d = 1 docs-only commit behind the pin; Parano1c/permanode @ v2.0.3 with identical raw-API lines; agcore @ 2.0.1 wrapper-level; jetsam @ v1.0.0 with +91 lines drift (verify fn :406 vs :315, visibility unchanged). Upstream main is 11 commits past the pin.
