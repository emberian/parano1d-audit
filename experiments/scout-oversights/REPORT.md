# Scout lane `scout-oversights`: the oversight ledger

Adversarial review of the PRIOR work (gemini subswarm, codex referee pass, claude
campaign handoffs) for claims that were vetoed and skipped, stubbed at the decisive
step, downgraded on source-reading without a dynamic test, or left open with
operational consequence. Target: pinned checkout
`/Users/ember/dev/parano1d-break/parano1d` @ `52938433` (read-only). 2026-11-15.

Sources attacked: `experiments/comp-verification/{VERIFICATION,REPLICATION-MATRIX}.md`
(codex referee), `experiments/comp-*/REPORT.md` + `comp-system-hunter/native-market-seam/NOTES.md`
(gemini, treated as source analysis), `experiments/FINDINGS.md` (open list),
`experiments/devnet-chaos/README.md` (RV-T markers), the `verification-*` lanes, and
the pinned source itself for the three special-attention traces.

**Campaign overlap rule.** The seven in-flight campaign lanes (halt-stalefee,
halt-binder, halt-reorg, split-cd1, chaos-latch, chaos-headerpin, chaos-asert)
already give dynamic runs for C35/CS-1, CD-3, SR-1, CD-1, C29+NM-2B (at 1,024),
C10, and ASERT. Nothing below duplicates them. Note two campaign facts that gate
everything financial: `experiments/shared-early-finance/bin/` is EMPTY —
`build.sh` must be rerun (chaos-latch's schedule-assert relaxation is already
applied to its tree) before any early-finance dynamic probe; and chaos-latch is
staged but has not run its devnet yet.

---

## The ledger, ranked by "would a positive most change the rebuttal's conclusions"

| # | Item | Oversight class | Decisive probe | In place | Cost | Conclusion change if positive |
|---|---|---|---|---|---|---|
| 1 | **U21 gossip byte-window exhaustion** (+ Push-path inbound-budget composition) | Downgraded to source-only; never run (gemini model; codex never probed; FINDINGS open item 2's sharpest edge) | 5-6 anonymous libp2p identities stream 50 x 303,495 B undecodable gossipsub payloads per 10 s window at a live victim (release daemon + miner) while honest peers advance the chain; measure victim tip lag, miner-template parent staleness, gossip `Ignore` counts, recovery after storm stop. Then compose: hold the 64 MiB Push-path inbound semaphore and watch block-body/HistoryStep/state-segment fetches queue behind it | devnet-chaos live devnet + honest miner scripts; chaos-headerpin's live-P2P identity tooling; stock binary (no schedule patch needed) | ~1 lane-day | First **anonymous, remote, credential-free** availability halt candidate: victim node misses block announcements (window shared with them, `network.rs:493/5174-5187/6491-6495`, charged before decode, no gossipsub penalty for `Ignore`) and its miner builds on a stale tip. Every halt the campaign chases needs special state or a credential; this needs 5 IPs |
| 2 | **NM-3A/B 64-input trade-budget displacement** | **Vetoed** by codex auto-safety-review, "no retry" (REPLICATION-MATRIX NM-3A/B row); scholar bounded it source-only at 31 sells + a buy | Build ONE real 64-funding-input `TradeOwnerIntent` (1 principal + 63 dust, ~13,600 uNOID consensus fee, V4 owner proof over the full intent hash), admit to a production-capacity mempool alongside a *second market's* live trades on the shared-early-finance node, mine blocks, inspect the assembled `ExecuteTrades` batch: which policies/market groups starve (`trades.rs:471` `funding > financial.funding -> continue`). Repeat across blocks to measure how often the attacker's market ranks first (parent-bound digest re-randomizes per block) | shared-early-finance node (after rebuild); verification-trade-latch already admitted real V4-proved funded policies; chaos-latch's factory for wallets/UTXOs; custom intent builder needed (the RPC planner stops at "enough" inputs — `planner.rs:872`) | 1-2 lane-days | Upgrades a whole downgraded family from "resource-accounting design concern, no valid admitted-and-mined example" to a demonstrated production-config cross-market trade freeze with a per-block cost (~13,600 microNOID) and settles gemini's 47-sells vs the scholar's 31+1 dispute |
| 3 | **C26 future-dated *intermediate* header, genuine release terminal, fresh-node selection** | **Vetoed** (dynamic snapshot probe "excluded after earlier automatic safety rejection"; comp-sync-finality's snapshot-terminal harness result is 0 bytes); FINDINGS open item 1 | Build a chain whose *middle* header is future-dated under a present-day tip with a **genuine** HistoryStep terminal at the snapshot boundary (reuse split-cd1's full-proof machinery), serve it as a snapshot to a fresh release node with independent providers; log candidate vs selected HeaderDAG work at install; second joiner seeded only to the first | split-cd1's genuine-terminal proving path + hbox packs (`/othersys/lanes/parano1d/packs/`); malicious-node's staging harness; devnet-chaos phase-7/8 joiner scripts; release joiners | 2-3 lane-days (highest of the three) | Consensus-split class: if a fresh release node installs the future-dated-intermediate fork, snapshot acceptance is broken for fresh joiners — currently bounded only by "rejected future *tip*" (phase-8) and a stubbed-terminal middle-header install (C26) |
| 4 | **CD-2: ten un-executed excluded financial opcodes** (33, 38, 39, 40, 41, 43, 51, 52, 53; op50 only rode along with op49) | Stubbed/downgraded: only op49+50 was executed ("does not establish that each of the other codes can form a complete valid block") | Sweep each code through the verification-opcodes fixture pattern (valid V4-signed intent -> `validate_block_checks_with_schedule` -> `materialize_accepted_block_state_with_financial_context`), hunting a validation-path panic (peer blocks reach block checks BEFORE the terminal check) and confirming graceful rejection everywhere | `experiments/verification-opcodes/` crate builds and runs offline; the op49 fixture is the template | < 1 lane-day | Low-moderate: a panic in pre-terminal validation = remote crash from a single peer block (chain-halt class); a clean pass closes the "other ten were source-only" cell and the wasted-PoW own-goal note. See special attention (i): admitted-and-mined is impossible by construction |
| 5 | **C22 live decodable-but-invalid proof storm at identity scale** | RV-T in devnet-chaos ("needs a real proof from an attacker-owned UTXO — the tx-admission lane proved that 62x asymmetry [RV-T]") | Real V4 proofs from attacker-owned UTXOs, corrupted to fail verify (8.34 ms each), streamed from ~10-84 identities against a live node with honest traffic; measure drain saturation (415.5/s measured single-node) vs honest admission | devnet-chaos `bin/anon-push-storm` pattern + tx-admission's proof tooling; `network-abuse/C22-LIVE.md` already did 40 corrupted proofs from 4 identities | ~1 lane-day | Folds into #1 (availability ceiling): amplifies the same anonymous vector with real proof-shaped work |
| 6 | **CS-2 `NoCoinbaseSlot` on a valid persisted chain / full `TemplateBuilder`** | Stubbed at decisive step (verification-template reproduced only the allocator predicate on a scaled state) | A persisted valid chain whose hydrated segment's last empty slots are reserved by selected spends with an alternative populated segment evicted; drive the release miner end-to-end | halt-stalefee's live miner path proves the *class* (template -> `None` -> 1 s retry forever); CS-2's state shape is new | 1-2 lane-days | Second instance of the demonstrated halt class — supports, does not change, the campaign's headline |
| 7 | **C15 external-miner starvation** | RV-T ("external-miner lane proved [RV-T] with faster consumption") | Consume prepared templates faster than regeneration on a live devnet: invalid-nonce `submitBlock` at a cadence above template regeneration (~20 s on the fast devnet) | devnet-chaos phase-3 scripts; mining bearer credential | < 1 lane-day | Miner-RPC starvation demonstrated live — but credential-gated (operator-adjacent), so a weaker rebuttal headline than #1 |
| 8 | **Unavailable-object sync closing mining readiness** (new-avenues) | Source-only composition; "no dynamic miner pause was run" | A valid-work header advertising unavailable body/terminal objects into exact suffix sync on a NON-isolated miner; watch readiness close and `Busy` retention | new-avenues trace + devnet-chaos node control | ~1 lane-day | Miner pause without credentials — same anonymous class as #1, less certain mechanics |
| 9 | **C20 downward supply-counter stall** | RV-T in storage-reorg ("needs the precisely constructed spend of large coinbase UTXOs through the direct apply_next_block path") | That spend, on the m24 state copy | halt-reorg's m24 state (once landed) | ~1 lane-day on hbox | Local-write precondition unchanged — conclusion ceiling is "bit-rot can stall spend admission", already bounded |
| 10 | **FINDINGS open items 3-5**: SafeTrade deposit policy / operator-market separation; valid-State full-rate block + million-row scaling; `BadTypedBind`/`Delta_P2b` proof assurance | Open-with-consequence | External measurement (SafeTrade), full-rate producer run, or formal definition work | operator-exit/operator-power docs; creation-spam models; proof-soundness lane | days-weeks each | Item 3 changes the "who controls what" section if SafeTrade credits at <=18; items 4-5 refine magnitude, not direction |

**Downgrades that were fair (checked, not re-litigated):** NM-4A `noid_pruv`
(dev-tool contract, codex right), NM-4B zero-address/planner (funding voluntary),
PA-01 certificate undercount (masked: both current classes use 12 squeeze
permutations), PA-04 C0 BaseFold (production is C1), PA-07 legacy `noid_fri`
(no production caller), CS-8 dead fast path, U10 send post-build check (needs a
regression to matter). PA-05 non-x86 portability stays open but niche (wasm32/riscv64
targets).

---

## Special attention (i): the ten un-executed CD-2 opcodes

The excluded set is `{33, 38, 39, 40, 41, 43, 49, 50, 51, 52, 53}`; verification-opcodes
executed the op49+op50 pair, leaving 33/38/39/40/41/43/51/52/53 un-executed (op50
rode along as op49's mandatory registration suffix).

**Admitted-and-mined is impossible at production config, by construction.** The
production accepted-bundle path verifies the HistoryStep terminal before state commit
(`noid_chain/src/storage/mdbx_context.rs:1402`, `noid_node/src/main.rs:4239-4263`),
and no v2.1 bank can prove a block containing any of the eleven
(`bind_financial_sequential_headers` hard-pins financial ops to
`{48, 64, 65, 66, 80}`). A custom producer who includes one wastes the PoW — a
self-DoS, not an attack.

**The most dangerous-looking one is the op33 Swap + op38 RegisterPools pair**, via
gemini's Finding 1A.3 (native-market-seam NOTES.md): a `PoolPhase::Frozen` market
in the store plus ONE `TradeOwnerIntent` for it in the mempool wedges the node's
entire `TemplateBuilder` — `native_trade_parent_at` doesn't filter on phase
(`noid_chain/src/storage/mdbx_store.rs:248-329`), `stable_policies`'
`Err(Phase)` is non-declinable (`sequencing.rs:121, 178-191`), and the `?` at
`noid_miner/src/template.rs:394-397` aborts template production across ALL classes.
**Codex's referee pass never adjudicated this claim** — the CD-2 matrix row says
"no chain halt" from the op49 execution and the mempool/template gates alone.

**My trace closes it for the pinned mainnet:** `Frozen` is set only by round
registration (op 38 `market/queue.rs` `phase: PoolPhase::Frozen`, op 50
`launch/queue.rs`, NFT op 70 — disabled in profile), all creation paths set `Idle`
(`market.rs` bootstrap, `launch.rs:244`, `nft_market.rs:202`), graduation requires
`Idle` (`launch.rs:296`), and registration itself requires `before.phase == Idle`
(`market/queue.rs:72-73`). With only allowed ops `{48, 64, 65, 66, 80}` on chain,
no pool-like state can ever enter `Frozen` — and there are **zero** construction
sites for the cure ops (SettlePool/ResetPool/SettleLaunch/ResetLaunch) anyway.
The wedge is dead code until a non-sequential `V2Config::with_native_finance` class
ships (NOTES.md 1A.2's future-bank caveat). Record this reasoning in the paper; run
the #4 sweep for the validation-panic residual.

## Special attention (ii): NM-3A — gemini's 47 sells vs the scholar's 31 + a buy

**Both are right about different quantities; the scholar wins the arithmetic, gemini
oversold the magnitude, and neither ran anything.** Verified in source
(`noid_tx/src/native_assets/trade.rs:139-193`): a Sell needs >=1 `TokenBalance`
claim (side/asset/owner must match; carries the tokens) AND >=1 `None`-opening NOID
claim with `amount != 0` to cover `fee_limit` (TokenBalance claims do not add to
`totals.noid`). So >=2 funding inputs per sell, and under the shared 64-funding budget
(decoded from the pinned v2.1 metadata: markets 4 / policies 48 / funding 64) at
most 31 two-input sells plus a one-input buy fit concurrently — the scholar's bound.
Gemini's 47 counted policy slots (policies = 48). Also verified: the priority
anchor is chosen by `min_by_key((Reverse(units), creation_id, slot_index))` and the
priority digest hashes `(parent_id, asset, anchor.creation_id, anchor.slot_index)`
(`trade.rs:230-260`) — amount is not the score, but an attacker holding many UTXOs
can grind anchor choice, and `parent_id` re-randomizes market order every block.

**The displacement claim survives both corrections and is cheap:** ONE 64-input
policy consumes the whole funding budget for that block, skipping every later-ranked
market group (`trades.rs:471`, `funding > financial.funding -> continue`). The decisive live example needs
exactly what #2 in the ledger describes. If it works, the rebuttal gains its only
dynamic market-liveness witness with a sustained cost model (~13,600 microNOID per epoch).

## Special attention (iii): U21 gossip-window live run

Design (ledger #1): the window is a single process-global 64 MiB / 10 s budget
charged **before decode**, shared with block announcements
(`noid_tx` gossip ingress `network.rs:5174-5187`; config `network.rs:493`;
block-announcement share at `network.rs:6491-6495`); a 303,495 B undecodable
payload passes the size check; per-peer count limit is 50 tx/10 s **before decode**;
`Ignore` has no gossipsub score consequence. Five identities x 50 x 303,495 B =
75.8 MB/10 s > 64 MiB. The unmeasured parts are identity retention on a live swarm
(connection caps, colocation penalties — 128 inbound connections) and delivery rate —
which is precisely what the live run answers, along with the miner impact (stale-tip
templates) and recovery. Compose with the Push-path inbound semaphore
(`inbound_budget.rs`: block-body, HistoryStep, and state-segment responses share the
64 MiB permit domain) for the full availability ceiling. Stock binaries suffice —
no schedule patch, so this can start before the shared-early-finance rebuild.

---

## Top 3 — run these next

1. **U21 gossip-window exhaustion, live** (ledger #1, + #5's proof storm as a second
   phase). Anonymous remote availability halt; stock binaries; no campaign overlap.
2. **NM-3A/B dynamic 64-input displacement on the shared-early-finance node** (ledger
   #2). The vetoed probe; settles 47-vs-31; prerequisite: rerun
   `experiments/shared-early-finance/build.sh` and wait for `bin/parano1d`.
3. **C26 genuine-terminal future-dated-middle-header snapshot run** (ledger #3). The
   other vetoed probe; reuses split-cd1's full-proof machinery and hbox packs.

Runners-up: CD-2 ten-opcode sweep (#4, cheap completeness), CS-2 full-chain
NoCoinbaseSlot (#6). Discipline for whoever runs these: pinned checkout read-only;
own lane dirs; PID files for every detached job (never kill by pattern); no
`git stash`; commit NAMED files only; heavy builds on hbox under `swarm-build`.
