# HANDOFF — chaos-headerpin (C10 live: HeaderDAG capacity pin over anonymous P2P)

## 1) Mission

Take C10 live: pin a victim release node's HeaderDAG ancestry capacity with
valid-PoW fork headers over anonymous P2P so the node rejects the next honest
valid child with the exact `Capacity` error and stalls until restart.
Component evidence exists (experiments/ledger-fairness/REPORT.md, DAG
max_nodes=2 fixture); this lane builds the live P2P version on a small devnet
(ports 21080-21099, stock binaries from experiments/devnet-chaos/bin,
READ-ONLY), measures attacker cost, and writes REPORT.md.

## 2) Established facts (all read from the pinned tree at 52938433; paths relative to parano1d/)

### Capacity + protection semantics
- `noid_node/src/networking/header_dag.rs:287` — insert fails with
  `HeaderDagError::Capacity` ("header DAG reached its configured capacity")
  when `nodes.len() >= max_nodes` and no leaf can be evicted.
- `header_dag.rs:318-358` `evict_unprotected_leaf` — protected = best-tip
  ancestry ∪ incoming-candidate's-PARENT ancestry; nodes that are parents of
  other nodes are additionally not evictable. A losing branch always has an
  evictable tip (drained one node per insert) — **the pin requires a single
  linear SELECTED chain filling all 1,024 nodes** (spam best by cumulative
  work) plus the honest chain (the candidate-parent ancestry).
- `noid_node/src/main.rs:4711` — `HEADER_DAG_MAX_NODES = 1024`.
- Insert failure paths + log lines (both set `header_dag_faulted = true`):
  - direct-child announcements: main.rs:10220-10227, log
    "validated direct-child header could not enter the bounded DAG".
  - fetched batches: main.rs:11272-11279, log
    "validated header branch could not enter the bounded DAG".
  - periodic reconcile: main.rs:14433-14468 (re-runs every 10s while
    faulted; if nothing new to insert it succeeds and CLEARS the flag, so the
    flag oscillates but the Capacity rejection persists per arriving child).

### What a spam header must pass (all computable with pinned `noid_chain`)
- `noid_chain/src/consensus/header.rs:188-265` — parent-hash linkage,
  height+1, ASERT target == `next_target(anchor...)`, MTP (median of last
  ≤11 ancestors), future drift ≤ now+120s (`MAX_FUTURE_DRIFT`, params.rs:131),
  Poseidon2b PoW, `log_slots == expected_child_log_slots(parent.height,
  parent.log_slots, finalized_active_counts)`.
- state_root/tx_root/miner_address/active_slot_count/alloc_counter are NOT
  header-validated (bodies/terminals are separate objects never served).
- GENESIS_TARGET = 2^238 ≈ 2^18 hashes/header; ~186 KH/s on 12 cores
  (params.rs:318-338) → ~1.4 s/header parallel. BLOCK_TIME=20s
  (params.rs:11); honest per-header work ≈ spam per-header work at floor.

### Transport / admission (how spam enters a live victim)
- Announcement channel: `/sync/availability/1` push — any *dispatchable*
  peer pushes a 292-byte NHA2 `HeaderAnnouncement` (body/terminal digests
  arbitrary, lens ≥1; provider flags must include BODY|TERMINAL);
  network.rs:7366-7410 turns it into `NetworkEvent::HeaderAnnouncement`
  (rate limit 20/10s per peer, network.rs:7377-7388).
- Dispatchable requires `profile_verified` (network.rs:1159-1172): the
  victim answers ANY `/sync/profile/6` request with its full profile
  (network.rs:7081-7084) — an aggressor harvests it with a zeros expected-id
  request, then sends a second request with the correct id → marked verified.
  PeerConnected then fires → `manifest_peers.insert(peer)` (main.rs:10755).
- Victim fetches headers via `/sync/headers/5` request-response: request =
  16 bytes `NHQ5 {start_height u64, count u16, include u8}`; response =
  54-byte `NHB5` header + zstd(1)-compressed records, 288 bytes each
  (212-byte header + 76 zero bytes = header-only record).
  `validate_general_header_response` (network.rs:3083-3098): response count
  ≤ requested count, FIRST record's height == start_height.
- Fetch ranges: announcement path starts at `floor = our_height-18`
  (main.rs:10091-10113, count ≤ 512 → top = floor+511); the probe path
  `unresolved_selected_tip_probe_range` (main.rs:14856-14939, interval 1s)
  starts at the common ancestor of committed tip and DAG best (= the spam
  fork point!) → top = fork+511. Probes query rotating manifest peers
  (victim-side P, twin, AND the aggressor — all in `manifest_peers`).
- Fetched batches are natively validated
  (`plan_header_inventory` main.rs:4884-5013; anchor = highest record the
  victim already knows — MUST include at least one known honest header;
  `validate_bounded_header_extension` caps 4,096 headers per batch) and
  inserted one-by-one; the first Capacity error aborts the rest of the batch.

### Pin topology arithmetic (the load-bearing analysis)
- With the victim committed at H_0 and spam forked at H_0:
  - announcement fetches admit ≤ 493 spam (slice = 19 honest anchors + 493);
  - probes admit spam to fork+511 = 511 (probe start = fork point);
  - honest-pending (honest chain above H_0, delivered by P's announcements
    to floor+493 and by probes to fork+511) can reach 511;
  - DAG = 18 (honest window above finality) + 511 spam + pending → **fills
    exactly 1,024 when pending = 495**; the honest children from H_0+496 on
    arrive via probes → insert hits full-DAG all-protected → `Capacity`.
- Honest commits are BLOCKED while spam is best (main.rs:10231-10239
  "validated direct child retained; HeaderDAG selected another tip") → the
  victim's committed tip freezes at H_0 WITHOUT the FinalizedOffBestChain
  fault (fault requires commits; no commits happen).
- Spam stays best while honest lead k < 511 blocks of work; at k ≥ 511-512
  the honest tip out-works the spam → best flips → honest commits resume
  (self-recovery without restart — a bounded pin window to document).
- The 512-record fetch cap + "two independently validated branches" slack
  (main.rs:4711-4716) means announcement-only spam can fill at most
  18+493+511 = 1,022 — the probe path (querying the aggressor as a manifest
  peer) supplies the +18 that overfills to 1,024.
- P's snapshot manifests are REJECTED while the spam out-works them
  (main.rs:282-321 `validate_snapshot_install_selection`) — no snapshot
  install steals recovery while k < 511.

## 3) Ruled out (with why)

- **Big single batch (>512 records)**: response count ≤ request count
  (network.rs:3089). Snapshot-kind fetches have no count check but stage to
  disk, bypassing the DAG (main.rs:4715 comment).
- **Fill without honest-chain growth**: announcement spam alone caps the DAG
  at ~1,022 (two-branch slack) — Capacity unreachable without the honest
  pending branch growing ~495 nodes.
- **Two co-resident spam branches pinning**: the losing branch's tip is an
  evictable leaf (header_dag.rs:331-334) — the honest child evicts and
  inserts; only a single selected spam chain + honest chain pins.
- **FinalizedOffBestChain fault as the pin mechanism**: it requires honest
  commits past fork+18, but honest commits are blocked while spam is best
  (selected-another-tip) — the fault never fires in the chosen topology.
- **gossipsub for announcements**: not needed; direct availability push is
  sufficient and simpler.
- **Speeding up honest mining**: ASERT + BLOCK_TIME=20s self-corrects;
  ~495-505 honest blocks ≈ 2h45m of mining is irreducible for the fill.

## 4) Current plan and exact next commands

Live-variant design (victim stays up throughout):
1. P (producer, extminer, --genesis) mines to H_0 ≈ 80. V (victim) and T
   (twin control) sync; P's worker paused.
2. Aggressor connects to V, harvests+verifies profile, harvests honest
   headers, grinds 511 spam children of H_0 (~15 min, 10 threads).
3. Aggressor announces tips H_0+100..H_0+493 (distinct fetch keys, no
   dedup), probes extend spam to H_0+511. DAG = 529, best = spam.
4. Resume P's worker: honest chain grows; V's pending grows +1/block
   (~2h45m to k=495). Fill at pending=495 → children H_0+496+ rejected
   with Capacity (the pin line, repeating on ~1s probe rounds).
5. Pause P at k≈500-505 (< 511 — spam stays best). Pin sustained.
6. Recovery: stop aggressor (PID), stop V (paranoid_stop+PID), restart V →
   syncs to P's tip. Document self-recovery bound (k ≥ 511).

Next commands:
```sh
cd /Users/ember/dev/parano1d-break/experiments/chaos-headerpin
CARGO_TARGET_DIR=target cargo build --release   # build aggressor
# then run the driver:
python3 scripts/run_headerpin.py --phase setup    # genesis + sync
python3 scripts/run_headerpin.py --phase attack   # aggressor fill
python3 scripts/run_headerpin.py --phase mine     # resume worker, wait for pin
python3 scripts/run_headerpin.py --phase recover   # restart demo
```

## 5) Artifacts on disk

- `Cargo.toml` — headerpin-tools crate (same dep versions as devnet-chaos).
- `src/aggressor_common.rs` — copy of the devnet-chaos hand-driven transport.
- `src/spam_chain.rs` — spam fork builder (ASERT/MTP/log_slots per header,
  parallel nonce grind, self-validation via production validators).
- `src/bin/anon-header-pin.rs` — the aggressor binary (profile handshake,
  honest harvest, grind, announce, serve).
- `scripts/run_headerpin.py` — the driver (TO BE WRITTEN next; adapt
  experiments/devnet-chaos/scripts/chaoslib.py: Node/Managed/PID files,
  RPC helpers).
- Binaries used read-only: `/Users/ember/dev/parano1d-break/experiments/devnet-chaos/bin/{parano1d,parano1d-miner}`.
- Ports: P 21080/21081, V 21082/21083, T 21084/21085 (all 127.0.0.1).
- Data dirs: experiments/chaos-headerpin/run/{producer,victim,twin}.

## 6) Open questions / risks

- Does the victim's `/sync/headers/5` SERVER side serve without profile
  gating? (Client harvest assumes yes; if not, harvest from P instead.)
- Probe/manifest interplay: manifest rounds to P (answered+rejected) block
  probes only briefly; manifest requests to the aggressor time out at 30s
  and pause probes — the aggressor stops announcing after the fill so
  last_announcement_peer stays mostly P.
- ASERT drift between the spam's constant timestamps (target flat at
  floor) and the honest 20s cadence could shift the work margin; pause P at
  k≤505 and watch for the honest tip out-working early (V starts committing).
- If the pending stalls below 495 (probe path blocked), the DAG sits at
  1,022 — no Capacity line; the stall then has a different signature
  (selected-another-tip) and must be reported as the two-short negative.
- Honest child H_0+496..H_0+511 arrive ONLY via probes (announcement fetch
  top is floor+511 = H_0+493); if probes are persistently off, no rejection
  lines despite a full DAG.

## 7) If resuming fresh

Read, in order:
1. `/Users/ember/dev/parano1d-break/experiments/chaos-headerpin/HANDOFF.md` (this file)
2. `/Users/ember/dev/parano1d-break/experiments/ledger-fairness/REPORT.md` (component C10)
3. `parano1d/noid_node/src/networking/header_dag.rs` (insert/evict semantics)
4. `parano1d/noid_node/src/main.rs` lines 9900-10330 (announcement handling),
   11074-11460 (batch handling), 14830-14940 (probe loop)
Then: `cargo build --release` is already done (`target/release/anon-header-pin`
built clean); go straight to the driver.

## 8) FINAL STATE at lane wrap (2026-11-15, context-burn wrap order)

**Code complete and compiling; live run NOT yet started** (no devnet was
launched; run/ and logs/ empty; no lane processes or PID files exist; nothing
to stop by PID).

- `cargo build --release` finished clean (2 dead-code warnings:
  `SpamChain::fork_parent`, one unused const in spam_chain.rs).
- `target/release/anon-header-pin` exists. It implements, in order: RPC
  getChainInfo, TCP+noise-XX+yamux with one fresh identity, two-round
  profile handshake (harvest then verify), honest-header harvest over the
  real `/sync/headers/5` client path, parallel PoW grind (10 threads,
  disjoint nonce ranges, batched digests) of `--spam-count` (default 511)
  children of the victim's tip stamped `now+60s`, each self-validated via
  the production validators (`validate_header_timeless_prehashed_parent` +
  `validate_future_drift` + PoW), NHA2 availability pushes for tips
  `--announce` (default 100,200,300,400,493), inbound `/sync/headers/5`
  serving (288-byte header-only records, zstd(1), NHB5 framing, first
  record at requested start_height, count capped at the request), then
  parking (ping/identify/profile/headers inbound; manifest/object streams
  parked unread = objects unavailable).
- `scripts/chaoslib.py` — copied from devnet-chaos, BIN repointed to the
  read-only devnet-chaos/bin, AGGRESSOR_BIN to this lane's target/release.
- `scripts/run_headerpin.py` — the five-phase driver (setup/attack/mine/
  recover/status), PID-file process model, ports 21080-21099, P=extminer
  genesis + worker, V victim seeded to P, T twin control. `--phase mine`
  polls producer/victim/twin tips + pin-line counts into
  run/pin-evidence.json and pauses the producer after the pin + >=2 further
  honest blocks. KNOWN NITS: the `import signal` is below its first use
  (works); one dead expression line in phase_recover (harmless);
  worker stop/start uses PID files per the rules.

**First action for the next agent: run the experiment.**
```sh
cd /Users/ember/dev/parano1d-break/experiments/chaos-headerpin
python3 scripts/run_headerpin.py --phase setup      # ~20-30 min
python3 scripts/run_headerpin.py --phase attack     # ~15-20 min grind
nohup python3 scripts/run_headerpin.py --phase mine \
  > logs/mine-phase.log 2>&1 & echo $! > run/mine-driver.pid
# poll: python3 scripts/run_headerpin.py --phase status
# (~2h45m of honest mining to pending=495, then the Capacity lines)
python3 scripts/run_headerpin.py --phase recover
```
Then write REPORT.md (conditions, code path, the measured rejection lines,
frozen-vs-advancing heights, the unpinned twin control, attacker cost:
1 identity, 511 headers, grind wall-time, bytes served — the aggressor
logs fetch/record counts; plus the two-short negative (announcement-only
fill = 1,022) and the self-recovery bound (honest work > spam at
k >= 511-512) if observed) and commit NAMED files on dev.

**Watch for during the run (from the mechanics analysis):**
- After `--phase attack`, V's log must show "HeaderDAG selected another tip"
  (spam best; honest commits blocked) — if V's height advances past h0 the
  spam never became best (check the aggressor's fetch logs).
- If the aggressor's `served headers fetch` shows start= h0-18 only and
  never start= h0 (the probe range), the probe path isn't querying the
  aggressor; the DAG will cap at 1,022 and no Capacity line will appear —
  that negative (design slack) then becomes the report's headline instead.
- The honest chain must NOT exceed ~510 blocks past h0 while the pin is
  shown (spam stays best only while k < 511); the mine phase pauses the
  producer after the pin is sustained.
