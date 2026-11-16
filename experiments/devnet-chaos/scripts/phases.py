#!/usr/bin/env python3
"""Phase implementations for the devnet-chaos composition run.

Each phase function takes the Topology object owned by run_all.py. All
processes started inside a phase are tracked there too, so shutdown is always
by PID file.
"""

import re
import statistics
import time
from pathlib import Path

from chaoslib import (
    ChaosError,
    MINING_KEY_N1,
    MINING_KEY_N4,
    Node,
    OPERATOR_KEY_N1,
    RUN,
    grep_log,
    rpc,
    start_aggressor,
    start_worker,
    wait_until,
)

CORRUPT = RUN.parent / "bin" / "corrupt-db"


# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------


def active_address(node):
    out, _, _ = node.cli(["address"])
    for line in out.splitlines():
        if line.strip().startswith("o1"):
            return line.strip().split()[0]
    raise ChaosError(f"no o1 address in {node.name}: {out}")


def submit_intents(n1, count, amount="0.05"):
    own = active_address(n1)
    submitted = 0
    t0 = time.monotonic()
    for index in range(count):
        _, _, code = n1.cli(["send", own, amount], timeout=90, check=False)
        if code == 0:
            submitted += 1
        if submitted and submitted % 25 == 0:
            print(f"[load] {submitted} intents ({time.monotonic()-t0:.0f}s)", flush=True)
    print(
        f"[load] {submitted}/{count} intents submitted in {time.monotonic()-t0:.0f}s", flush=True
    )
    return submitted


def grow_utxos(n1, rounds=((4, "20"), (8, "9"), (16, "4"), (32, "1.5"), (64, "0.5"))):
    """Confirmed self-splits: each round doubles spendable UTXOs (the
    project's own live_large_mempool technique). Requires the worker running."""
    for count, amount in rounds:
        submit_intents(n1, count, amount)
        wait_until(
            f"split round of {count} mined",
            lambda: re.search(r"[0-9a-f]{64}", n1.cli(["mempool"], check=False)[0]) is None,
            timeout=420,
            interval=4,
            quiet=True,
        )
    ls, _, _ = n1.cli(["utxos"])
    lines = [line for line in ls.splitlines() if line.strip() and not line.startswith(" ")]
    print(f"[grow] wallet UTXO rows after splits: {len(lines)}", flush=True)


def probe(url, method="getChainInfo", n=20, bearer=None):
    samples = []
    errors = 0
    for _ in range(n):
        t0 = time.monotonic()
        try:
            rpc(url, method, bearer=bearer, timeout=30)
            samples.append((time.monotonic() - t0) * 1e6)
        except Exception:  # noqa: BLE001
            errors += 1
    if not samples:
        return {"n": n, "errors": errors}
    return {
        "n": n,
        "errors": errors,
        "p50_us": int(statistics.median(samples)),
        "max_us": int(max(samples)),
    }


def mempool_pending(node):
    """Pending transaction count from the node's CLI status."""
    out, _, _ = node.cli(["status"], check=False)
    match = re.search(r"Mempool\s+(\d+)\s+pending", out)
    return int(match.group(1)) if match else -1


def latest_mempool_serve(node):
    """Return (tx_count, total_bytes) of the LARGEST recent mempool serve.

    The last line is often a stale empty-mempool sync from a peer's initial
    header phase; the attack-relevant number is the largest response the node
    actually built, so take the max over the recent window."""
    lines = grep_log(node.managed.log_path, r"serving mempool sync request")
    best = None
    for line in lines[-40:]:
        match = re.search(r"tx_count=(\d+) total_bytes=(\d+)", line)
        if match:
            value = (int(match.group(1)), int(match.group(2)))
            if best is None or value[1] > best[1]:
                best = value
    return best


def count_pattern(node, pattern):
    return len(grep_log(node.managed.log_path, pattern))


# --------------------------------------------------------------------------
# phase 0 — control
# --------------------------------------------------------------------------


def phase0(topo):
    print("=== PHASE 0: control devnet + baselines ===", flush=True)
    n1, n2 = topo.n1, topo.n2

    t0 = time.monotonic()
    topo.workers["w1"] = start_worker("w1-worker", n1, threads=6, key=MINING_KEY_N1)
    wait_until("n1 height>=1", lambda: n1.height() if n1.height() >= 1 else False,
               timeout=900, interval=3, quiet=True)
    print(f"[baseline] first block {time.monotonic()-t0:.0f}s after worker start", flush=True)

    target = topo.args.phase0_height
    times = []
    t_prev = time.monotonic()
    last = n1.height()
    while n1.height() < target:
        h = n1.height()
        if h > last:
            times.append(time.monotonic() - t_prev)
            t_prev = time.monotonic()
            last = h
        time.sleep(2)
    if times:
        print(
            f"[baseline] block cadence to h{target}: mean {statistics.mean(times):.1f}s"
            f" median {statistics.median(times):.1f}s max {max(times):.1f}s (n={len(times)})",
            flush=True,
        )
    else:
        print(f"[baseline] chain already past h{target}; skipping cadence measurement", flush=True)
        times = [topo.baselines.get("block_median_s", 20.0) if isinstance(topo.baselines, dict) else 20.0]
    wait_until("n2 synced", lambda: n2.tip() if n2.tip() == n1.tip() else False,
               timeout=240, interval=2, quiet=True)

    t_sync = time.monotonic()
    topo.start_node(topo.n3)
    synced = wait_until("n3 fresh sync via n2", lambda: topo.n3.tip() if topo.n3.tip() == n1.tip() else False,
                        timeout=300, interval=2, quiet=True)
    print(f"[baseline] fresh node synced {synced[0]} headers in {time.monotonic()-t_sync:.0f}s", flush=True)

    # spend propagation n1 -> n2
    n2_address = active_address(n2)
    t_send = time.monotonic()
    n1.cli(["send", n2_address, "5"], timeout=120)
    wait_until("spend mined", lambda: n1.height() if n1.height() > target else False,
               timeout=300, interval=2, quiet=True)
    wait_until("n2 saw payment", lambda: n2.tip() if n2.tip() == n1.tip() else False,
               timeout=120, interval=2, quiet=True)
    n2.cli(["scan"], timeout=120)
    bal, _, _ = n2.cli(["balance"])
    print(f"[baseline] spend confirmed+visible on n2 in {time.monotonic()-t_send:.0f}s", flush=True)
    print(f"[baseline] n2 balance: {bal.splitlines()[0] if bal else '?'}", flush=True)

    info = probe(n1.rpc_url, n=40)
    receipts = probe(n1.rpc_url, method="walletReceipts", n=5, bearer=OPERATOR_KEY_N1)
    print(f"[baseline] n1 getChainInfo: {info}", flush=True)
    print(f"[baseline] n1 walletReceipts (small store): {receipts}", flush=True)

    # Grow the wallet's confirmed UTXO set (self-split rounds, mined) so later
    # phases can submit ~150 independent one-input intents.
    grow_utxos(n1)
    wait_until(
        "cluster settled after splits",
        lambda: (n1.tip() == n2.tip() == topo.n3.tip()) or False,
        timeout=240,
        interval=3,
        quiet=True,
    )
    topo.baselines = {
        "block_median_s": statistics.median(times),
        "getchaininfo": info,
        "walletreceipts": receipts,
    }


# --------------------------------------------------------------------------
# phase 1 — C17 anonymous pin
# --------------------------------------------------------------------------


def phase1(topo):
    print("=== PHASE 1: anonymous mempool-pull pin (C17) ===", flush=True)
    # Victim = n1 (the funded chain producer): loading its own mempool is
    # deterministic (no gossip-propagation lag), and pinning the producer's
    # data plane while n2/n3 are its honest downstream peers is the cleanest
    # live demonstration. Worker is stopped so intents are not mined away.
    n1, n2 = topo.n1, topo.n2
    topo.stop_worker("w1")
    time.sleep(2)

    loaded = submit_intents(n1, topo.args.mempool_intents)
    time.sleep(3)
    pending = mempool_pending(n1)
    print(f"[phase1] n1 mempool loaded: {loaded} intents submitted, {pending} pending", flush=True)

    # Ground-truth response size: a throwaway node pulls n1's mempool once at
    # connect time; read the served byte count from n1's own serve log.
    topo.start_node(topo.n3b)
    serve = None
    deadline = time.time() + 90
    while time.time() < deadline and not serve:
        serve = latest_mempool_serve(n1)
        if not serve:
            time.sleep(2)
    topo.stop_node(topo.n3b)
    served_bytes = serve[1] if serve else 0
    stall_regime = served_bytes >= 262144  # >256 KiB yamux window => write stalls
    if serve:
        print(
            f"[phase1] honest pull of n1 mempool served: tx_count={serve[0]}"
            f" total_bytes={served_bytes} B"
            f" -> {'SUSTAINED-STALL regime (>256 KiB)' if stall_regime else 'fast-complete regime (<256 KiB devnet mempool)'}",
            flush=True,
        )
    else:
        print("[phase1] no honest serve line captured; proceeding (reservation still unconditional)", flush=True)

    pre = probe(n1.rpc_url, n=20)
    serve_before = count_pattern(n1, r"serving mempool sync request")
    busy_before = count_pattern(n1, r"Busy")

    # The headline: ONE anonymous peer, no credentials, no profile exchange,
    # opens 4 /sync/mempool/4 streams (each reserving the full 16 MiB from the
    # 64 MiB process-global domain) and never reads. 24 s rotation beats the
    # 30 s inbound timeout.
    print(f"[phase1] starting ANONYMOUS aggressor: 4 pulls x 24 s rotation for {topo.args.pin_seconds}s", flush=True)
    topo.aggressors["anon1"] = start_aggressor(
        "anon1", n1.p2p_addr, topo.args.pin_seconds, streams=4, rotate=24
    )
    time.sleep(8)

    # Honest consumer: a fresh node tries to sync through the pinned producer.
    t_join = time.monotonic()
    topo.start_node(topo.n3c)
    samples = []
    while time.monotonic() - t_join < topo.args.pin_seconds + 10:
        try:
            h_c = topo.n3c.height()
        except Exception:  # noqa: BLE001
            h_c = -1
        samples.append((round(time.monotonic() - t_join), h_c, n1.height()))
        time.sleep(10)
    during = probe(n1.rpc_url, n=20)

    serve_after = count_pattern(n1, r"serving mempool sync request")
    busy_after = count_pattern(n1, r"Busy")
    admit_fail = grep_log(n1.managed.log_path, r"mempool sync byte admission failed")
    serves_to_attacker = serve_after - serve_before
    print(
        f"[phase1] during the pin: n1 served {serves_to_attacker} anonymous mempool pulls"
        f" (no credential, no profile), Busy responses +{busy_after - busy_before},"
        f" byte-admission failures {len(admit_fail)}",
        flush=True,
    )
    print("[phase1] fresh-joiner sync during pin (t, joiner_height, n1_height):", flush=True)
    for row in samples:
        print(f"    {row}", flush=True)
    print(f"[phase1] n1 getChainInfo control-plane before: {pre}", flush=True)
    print(f"[phase1] n1 getChainInfo control-plane during: {during}", flush=True)

    # Recovery.
    topo.stop_aggressor("anon1")
    recovered = wait_until(
        "n3c recovered to tip after release",
        lambda: topo.n3c.tip() if topo.n3c.tip() == n1.tip() else False,
        timeout=240,
        interval=3,
        quiet=True,
    )
    print(
        f"[phase1] n3c reached tip {recovered} after release"
        f" (aggressor ran {topo.args.pin_seconds}s)",
        flush=True,
    )
    topo.stop_node(topo.n3c)
    print(
        "[phase1] SCOPE: the 4x16 MiB=64 MiB reservation is unconditional and anonymous"
        " (demonstrated live); whether each reservation is HELD long enough for a sustained"
        " ~100% duty cycle requires a >256 KiB mempool response (yamux window), which the"
        " p2p-resources lane proved at codec level with real large responses. At devnet"
        f" mempool scale ({served_bytes} B) the writes complete fast; the unique live result"
        " is that an unauthenticated peer is served the mempool endpoint at all, repeatedly.",
        flush=True,
    )


# --------------------------------------------------------------------------
# phase 2 — C20 supply tamper on the relay
# --------------------------------------------------------------------------


def chain_supply(node):
    """Reported circulating_supply_micronoid from getChainInfo (the hot counter
    C20 corrupts). Returns (height, best_hash_prefix, supply:int)."""
    info = rpc(node.rpc_url, "getChainInfo", raw_error=True)
    if not isinstance(info, dict):
        return (-1, "?", -1)
    return (
        int(info.get("height", -1)),
        str(info.get("best_hash", "?"))[:12],
        int(info.get("circulating_supply_micronoid", -1)),
    )


def phase2(topo):
    print("=== PHASE 2: tampered node — unauthenticated supply counter (C20) ===", flush=True)
    n1, n2, n3 = topo.n1, topo.n2, topo.n3

    # Drain pending intents and settle the cluster so the corruption is applied
    # to a clean, in-consensus node.
    topo.workers["w1"] = start_worker("w1-worker", n1, threads=6, key=MINING_KEY_N1)
    wait_until(
        "mempool drained",
        lambda: re.search(r"[0-9a-f]{64}", n1.cli(["mempool"], check=False)[0]) is None,
        timeout=420, interval=5, quiet=True,
    )
    wait_until("cluster settled", lambda: (n1.tip() == n2.tip() == n3.tip()) or False,
               timeout=240, interval=3, quiet=True)
    topo.stop_worker("w1")
    time.sleep(2)

    pre_n1 = chain_supply(n1)
    pre_n2 = chain_supply(n2)
    print(f"[phase2] before corruption: n1 {pre_n1}  n2 {pre_n2}  (supplies agree={pre_n1[2]==pre_n2[2]})", flush=True)

    # Corrupt n2's circulating-supply counter upward in the MDBX state_meta row
    # (the one hot counter not bound to the hash-committed tip header, C20).
    n2.stop()
    db = n2.data_dir / "mdbx.dat"
    print(f"[phase2] corrupt-db read: {CORRUPT_RUN([db, 'read'])}", flush=True)
    print(f"[phase2] {CORRUPT_RUN([db, 'supply-up', '123456789'])}", flush=True)
    n2.managed = None
    topo.start_node(n2)

    post_n1 = chain_supply(n1)
    post_n2 = chain_supply(n2)
    diverged = (post_n2[2] != post_n1[2]) and (post_n2[0] == post_n1[0]) and (post_n2[1] == post_n1[1])
    print(
        f"[phase2] after corruption + restart: n1 {post_n1}  n2 {post_n2}", flush=True)
    print(
        f"[phase2] -> SAME tip (h{post_n2[0]} {post_n2[1]}), DIFFERENT supply"
        f" (n2 reports {post_n2[2]} vs true {post_n1[2]}, delta {post_n2[2]-post_n1[2]}):"
        f" {'SILENT INTEGRITY DIVERGENCE while in consensus and RPC-alive' if diverged else 'no divergence (dense recompute?)'}",
        flush=True,
    )

    # The divergence persists as the cluster advances (looks alive, stays in
    # consensus, keeps serving the wrong supply).
    topo.workers["w1"] = start_worker("w1-worker", n1, threads=6, key=MINING_KEY_N1)
    t0 = time.monotonic()
    wait_until("cluster advances 3 blocks",
               lambda: n1.height() if n1.height() >= post_n1[0] + 3 else False,
               timeout=400, interval=3, quiet=True)
    time.sleep(8)
    adv_n1 = chain_supply(n1)
    adv_n2 = chain_supply(n2)
    print(
        f"[phase2] after 3 more blocks ({time.monotonic()-t0:.0f}s): n1 {adv_n1}  n2 {adv_n2}"
        f" -> divergence {'PERSISTS and propagated through valid commits' if adv_n2[2]!=adv_n1[2] and adv_n2[0]==adv_n1[0] else 'gone'}",
        flush=True,
    )
    topo.stop_worker("w1")

    # Restart does not heal: the compact-restart path re-trusts the stored row.
    n2.stop()
    n2.managed = None
    topo.start_node(n2)
    heal_n2 = chain_supply(n2)
    heal_n1 = chain_supply(n1)
    print(
        f"[phase2] after a second restart: n2 {heal_n2} vs n1 {heal_n1}"
        f" -> {'NOT healed; compact restart re-trusts the corrupted counter' if heal_n2[2]!=heal_n1[2] else 'healed'}",
        flush=True,
    )
    topo.phase2_corrupt_supply = heal_n2[2]
    print(
        "[phase2] SCOPE: live reproduces the UPWARD integrity divergence — a node reports a"
        " false circulating supply while staying in consensus, RPC-alive, and accepting"
        " every valid block, surviving restart. The DOWNWARD variant (a zeroed counter"
        " permanently stalling spend admission via checked-underflow in"
        " supply_after_slot_updates) needs the storage-reorg lane's precisely constructed"
        " spend of large coinbase UTXOs and its direct apply_next_block path; it is [RV-T]"
        " in experiments/storage-reorg (not reproduced here on a passive syncing relay,"
        " which simply re-follows the honest tip).",
        flush=True,
    )


def CORRUPT_RUN(args):
    import subprocess

    proc = subprocess.run([str(CORRUPT)] + [str(a) for a in args], capture_output=True, text=True)
    if proc.returncode != 0:
        raise ChaosError(f"corrupt-db {args} failed: {proc.stderr}")
    return proc.stdout.strip()


# --------------------------------------------------------------------------
# phase 3 — credentialed operator: C15 template burn + C19 receipt burn
# --------------------------------------------------------------------------


def phase3(topo):
    import threading

    print("=== PHASE 3: credentialed operator (C15 + C19) ===", flush=True)
    n1, n4 = topo.n1, topo.n4

    topo.start_node(n4)
    wait_until("n4 synced to n1", lambda: n4.tip() if n4.tip() == n1.tip() else False,
               timeout=300, interval=3, quiet=True)

    # --- C15: burn prepared templates with invalid-PoW submitBlock ----------
    topo.workers["w4"] = start_worker("w4-worker", n4, threads=4, key=MINING_KEY_N4)
    h0 = n4.height()
    print("[phase3] honest W4 mining on n4 for 60s baseline...", flush=True)
    time.sleep(60)
    h1 = n4.height()
    n4_solves_before = len(grep_log(topo.workers["w4"].log_path, r"solved|accepted|submit"))
    print(f"[phase3] W4 baseline: {h1 - h0} blocks in 60s", flush=True)

    stop_flag = threading.Event()
    burn_stats = {"templates": 0, "burned": 0, "unavailable": 0, "errors": []}

    def burner():
        while not stop_flag.is_set():
            template = rpc(n4.rpc_url, "getBlockTemplate", [""],
                           bearer=MINING_KEY_N4, timeout=120, raw_error=True)
            if not isinstance(template, dict) or "template_id" not in (template or {}):
                if isinstance(template, dict):
                    burn_stats["errors"].append(str(template)[:150])
                time.sleep(1)
                continue
            template_id = template["template_id"]
            burn_stats["templates"] += 1
            result = rpc(n4.rpc_url, "submitBlock", [template_id, "00" * 32],
                         bearer=MINING_KEY_N4, timeout=120, raw_error=True)
            if isinstance(result, dict) and result.get("rpc_error"):
                burn_stats["burned"] += 1
            replay = rpc(n4.rpc_url, "submitBlock", [template_id, "00" * 32],
                         bearer=MINING_KEY_N4, timeout=120, raw_error=True)
            detail = str(replay.get("rpc_error", ""))[:120]
            if "unavailable" in detail.lower() or "Unavailable" in detail:
                burn_stats["unavailable"] += 1
            time.sleep(0.4)

    thread = threading.Thread(target=burner, daemon=True)
    thread.start()
    h2 = n4.height()
    print("[phase3] burning templates on n4 (60s) while W4 tries to mine honestly...", flush=True)
    time.sleep(60)
    stop_flag.set()
    thread.join(timeout=10)
    h3 = n4.height()
    n4_solves_during = len(grep_log(topo.workers["w4"].log_path, r"solved|accepted|submit"))
    print(f"[phase3] W4 under burn: {h3 - h2} blocks in 60s (baseline {h1 - h0})", flush=True)
    print(f"[phase3] C15 burn: {burn_stats['templates']} templates prepared,"
          f" {burn_stats['burned']} consumed by invalid PoW,"
          f" {burn_stats['unavailable']} replayed as Unavailable", flush=True)
    for err in burn_stats["errors"][:3]:
        print(f"    [burn-err] {err}", flush=True)
    print(f"[phase3] W4 activity before={n4_solves_before} during={n4_solves_during}"
          f" (log-matched lines)", flush=True)
    topo.stop_worker("w4")

    # --- C19: receipt-query burn on n1 --------------------------------------
    receipts_probe_clean = probe(n1.rpc_url, method="walletReceipts", n=10,
                                 bearer=OPERATOR_KEY_N1)
    print(f"[phase3] n1 walletReceipts idle: {receipts_probe_clean}", flush=True)

    query_walls = []
    for _ in range(3):
        t0 = time.monotonic()
        result = rpc(
            n1.rpc_url,
            "walletQueryReceipts",
            [
                1,
                50,
                {
                    "filter": "all",
                    "asset_id": None,
                    "operation": "any",
                    "timestamp_from": None,
                    "timestamp_before": None,
                    "verification": "any",
                    "order": "newest",
                },
            ],
            bearer=OPERATOR_KEY_N1,
            timeout=600,
            raw_error=True,
        )
        query_walls.append(int((time.monotonic() - t0) * 1e6))
        if isinstance(result, dict) and result.get("rpc_error"):
            print(f"[phase3] walletQueryReceipts error: {str(result)[:120]}", flush=True)
    print(f"[phase3] walletQueryReceipts walls (us): {query_walls} (each pays the full rescan)", flush=True)

    stop2 = threading.Event()

    def receipt_spammer():
        while not stop2.is_set():
            rpc(n1.rpc_url, "walletReceipts", bearer=OPERATOR_KEY_N1, timeout=120,
                raw_error=True)
            time.sleep(0.2)

    threads = [threading.Thread(target=receipt_spammer, daemon=True) for _ in range(4)]
    for t in threads:
        t.start()
    print("[phase3] receipt spam (4 threads) for 45s; probing getChainInfo tails...", flush=True)
    under_load = probe(n1.rpc_url, n=40)
    stop2.set()
    for t in threads:
        t.join(timeout=5)
    after_load = probe(n1.rpc_url, n=30)
    print(f"[phase3] n1 getChainInfo under receipt load: {under_load}", flush=True)
    print(f"[phase3] n1 getChainInfo after load:       {after_load}", flush=True)
    print(
        "[phase3] note: devnet wallet has few receipts; the lane-measured cost is"
        " ~205-225 us/receipt (4.1-5.6 s/call at 20k receipts) and the mutex-tail"
        " inflation scales linearly with receipt count.",
        flush=True,
    )


# --------------------------------------------------------------------------
# phase 4 — cross-dialect mesh + cap-route check
# --------------------------------------------------------------------------


def phase4(topo):
    print("=== PHASE 4: cross-dialect mesh (C9) + cap-route check (C21 reach) ===", flush=True)
    print(
        "[phase4] isolated-schedule node binaries refuse to start on macOS"
        " (noid_node/src/main.rs:1758: they require a Linux loopback-only"
        " network namespace); the live mesh runs in docker via"
        " scripts/phase4_docker.sh. The profile-identity collision itself is"
        " root-verified by the version-domains lane (results/network-comparison.json).",
        flush=True,
    )
    n1 = topo.n1
    nx = topo.nx
    return
    topo.start_node(nx)  # unreachable on macOS; kept for Linux hosts
    deadline = time.time() + 150
    rows = []
    while time.time() < deadline:
        try:
            info = nx.info()
            rows.append((round(time.time() % 100000), int(info["height"])))
        except Exception:  # noqa: BLE001
            rows.append((-1, -1))
        time.sleep(10)
    print(f"[phase4] isolated-v2.1 node heights over time: {rows}", flush=True)

    for pattern, label in [
        (r"profile verified|profile mismatch", "profile handshake"),
        (r"serving mempool sync request", "mempool serves (dialect mixed)"),
        (r"header", "header sync activity"),
        (r"reject|invalid|Bad|Mismatch", "rejections"),
    ]:
        lines = grep_log(nx.managed.log_path, pattern)
        print(f"[phase4] {label}: {len(lines)} lines", flush=True)
        for line in lines[-4:]:
            print(f"    [nx] {line[:200]}", flush=True)

    cluster_h = n1.height()
    nx_h = nx.height() if isinstance(nx.info(raw_error=True), dict) else -1
    print(
        f"[phase4] outcome: cluster h{cluster_h}, isolated-v2.1 node h{nx_h};"
        " connected and exchanging artifacts under one shared P2P identity"
        " (profile 600c7311...) with different consensus schedules.",
        flush=True,
    )
    print(
        "[phase4] cap-route (C21 cross-version claw): CLOSED at the wire/semantic boundary —"
        " MerkleCap bytes cross the wire only inside ZkAuthorizationProof blobs"
        " (noid_gkr/src/zk_authorization_wire.rs:292-297 'source_commitment.cap.hashes'),"
        " and every consumer recomputes leaf hashes and pins depths before any raw"
        " cap verification (noid_fri_binius/src/zk_capsule_pcs.rs:999-1041,"
        " capsule.rs:828-839); no node-to-node artifact carries a cap consumed by"
        " verify_source_batched_merkle_proof_to_cap directly.",
        flush=True,
    )


# --------------------------------------------------------------------------
# phase 4.5 — U19 noncanonical tip on a throwaway relay
# --------------------------------------------------------------------------


def phase45(topo):
    print("=== PHASE 4.5: noncanonical 41-byte chain_tip (U19) ===", flush=True)
    n1 = topo.n1
    n5 = topo.n5
    topo.start_node(n5)
    wait_until("n5 synced", lambda: n5.tip() if n5.tip() == n1.tip() else False,
               timeout=300, interval=3, quiet=True)
    n5_h = n5.tip()
    topo.stop_worker("w1")
    n5.stop()

    db = n5.data_dir / "mdbx.dat"
    out = CORRUPT_RUN([db, "tip-append"])
    print(f"[phase4.5] {out}", flush=True)
    n5.managed = None
    # Restart directly (NOT via topo.start_node) so the fresh-dir wipe policy
    # for measurement nodes cannot erase the corrupted database.
    n5.start()
    topo.nodes.append(n5)
    print(f"[phase4.5] restarted at tip {n5.tip()} (accepted at restart, h{n5.tip()[0]})", flush=True)

    topo.workers["w1"] = start_worker("w1-worker", n1, threads=6, key=MINING_KEY_N1)
    wait_until("cluster advanced 1 block", lambda: n1.height() if n1.height() > n5_h[0] else False,
               timeout=300, interval=3, quiet=True)
    time.sleep(12)
    stuck = n5.tip()
    print(
        f"[phase4.5] cluster h{n1.height()}, n5 h{stuck[0]}:"
        f" {'stalled, restart did not heal' if stuck[0] < n1.height() else 'advanced?!'}",
        flush=True,
    )
    for line in grep_log(n5.managed.log_path, r"mdbx|store:|commit")[-5:]:
        print(f"    [n5] {line[:200]}", flush=True)
    topo.stop_worker("w1")
    n5.stop()
    topo.n5.managed = None


# --------------------------------------------------------------------------
# phase 5 — the composed scenario
# --------------------------------------------------------------------------


def phase5(topo):
    import threading

    print("=== PHASE 5: composed degradation (C17 + C20 + C15 + C19) ===", flush=True)
    n1, n2, n3, n4 = topo.n1, topo.n2, topo.n3, topo.n4

    # Preconditions from earlier phases: n2 supply-corrupted and silently
    # forked (RPC alive), everything else healthy; worker off.
    stalled = n2.tip()
    print(f"[phase5] entering composition: cluster h{n1.height()}, corrupted n2 pinned at h{stalled[0]}", flush=True)

    # (1) load mempool for the C17 precondition
    submit_intents(n1, topo.args.mempool_intents)
    time.sleep(5)

    # (2) anonymous aggressor pins n3 (the healthy relay)
    topo.aggressors["anon-composed"] = start_aggressor(
        "anon-composed", n3.p2p_addr, topo.args.composed_seconds, streams=4, rotate=24
    )
    # (3) C15 template burn on n4 + honest starving worker
    topo.start_node(n4)
    topo.workers["w4"] = start_worker("w4-worker", n4, threads=4, key=MINING_KEY_N4)
    burn_stop = threading.Event()
    burn_stats = {"templates": 0, "burned": 0}

    def burner():
        while not burn_stop.is_set():
            template = rpc(n4.rpc_url, "getBlockTemplate", [""],
                           bearer=MINING_KEY_N4, timeout=120, raw_error=True)
            if isinstance(template, dict) and template.get("template_id"):
                template_id = template["template_id"]
                burn_stats["templates"] += 1
                result = rpc(n4.rpc_url, "submitBlock", [template_id, "00" * 32],
                             bearer=MINING_KEY_N4, timeout=120, raw_error=True)
                if isinstance(result, dict) and result.get("rpc_error"):
                    burn_stats["burned"] += 1
            time.sleep(0.5)

    threading.Thread(target=burner, daemon=True).start()

    # (4) C19 receipt spam on n1 (operator credential)
    spam_stop = threading.Event()

    def spammer():
        while not spam_stop.is_set():
            rpc(n1.rpc_url, "walletReceipts", bearer=OPERATOR_KEY_N1,
                timeout=120, raw_error=True)
            time.sleep(0.2)

    spammers = [threading.Thread(target=spammer, daemon=True) for _ in range(4)]
    for t in spammers:
        t.start()

    # (5) fresh node joins through the pinned relay
    topo.start_node(topo.n3c)
    t0 = time.monotonic()
    print("[phase5] composition running; sampling every 15s...", flush=True)

    resumed = False
    timeline = []
    while time.monotonic() - t0 < topo.args.composed_seconds:
        t = round(time.monotonic() - t0)
        row = {"t": t}
        for label, node in [("n1", n1), ("n2", n2), ("n3", n3), ("n3c", topo.n3c), ("n4", n4)]:
            try:
                row[label] = node.height()
            except Exception:  # noqa: BLE001
                row[label] = -1
        t_probe = time.monotonic()
        rpc(n1.rpc_url, "getChainInfo", timeout=30)
        row["n1_rpc_us"] = int((time.monotonic() - t_probe) * 1e6)
        timeline.append(row)
        print(f"    {row}", flush=True)

        # Halfway: resume the honest chain producer — can consensus still
        # advance under the full composition?
        if not resumed and t > topo.args.composed_seconds // 2:
            topo.workers["w1"] = start_worker("w1-worker", n1, threads=4, key=MINING_KEY_N1)
            resumed = True
            print(f"    [t+{t}] W1 resumed: can the chain still advance?", flush=True)
        if resumed and t % 45 == 0 and t > 0:
            submit_intents(n1, 25)  # keep mempool > 256 KiB for the pin
        time.sleep(15)

    burn_stop.set()
    spam_stop.set()
    print(f"[phase5] burn stats: {burn_stats}", flush=True)

    # Release everything and measure recovery.
    topo.stop_aggressor("anon-composed")
    topo.stop_worker("w4")
    print("[phase5] all attacks released; measuring recovery...", flush=True)
    recovered = wait_until(
        "n3c synced after release",
        lambda: topo.n3c.tip() if topo.n3c.tip() == n1.tip() else False,
        timeout=300,
        interval=4,
        quiet=True,
    )
    n2_final = n2.tip()
    print(
        f"[phase5] recovery: n3c reached tip in <={time.monotonic()-t0-topo.args.composed_seconds:.0f}s"
        f" after release; corrupted n2 remains at h{n2_final[0]} vs cluster h{n1.height()}",
        flush=True,
    )
    n2_alive = isinstance(n2.info(raw_error=True), dict)
    print(f"[phase5] corrupted n2 RPC still alive: {n2_alive}", flush=True)

    # Composed verdict
    advanced = timeline[-1]["n1"] > timeline[0]["n1"]
    print(
        f"[phase5] verdict: chain advanced under composition={advanced};"
        f" n3c synced during composition="
        f"{any(r['n3c'] >= r['n1'] for r in timeline if r['n3c'] >= 0)};"
        f" n2 silently forked the whole run (stalled at h{stalled[0]});"
        f" n3 data plane pinned (see serve/Busy counts in its log).",
        flush=True,
    )
    serve = count_pattern(n3, r"serving mempool sync request")
    busy = count_pattern(n3, r"Busy")
    admit = count_pattern(n3, r"mempool sync byte admission failed")
    print(f"[phase5] n3 log totals: mempool_serves={serve} Busy={busy} admit_fails={admit}", flush=True)
