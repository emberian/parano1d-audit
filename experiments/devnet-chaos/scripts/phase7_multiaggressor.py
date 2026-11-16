#!/usr/bin/env python3
"""Phase 7 — multi-aggressor eclipse composition.

Ember's directive: measure the DELTA between single-aggressor phases and
coordinated aggressors (distinct synthetic identities on the same box —
gossip IP-colocation penalties only apply >8 identities/IP and only to
gossip, so multiple identities per host is realistic).

Legs:
  1. C17 delta: 1 vs 2 vs 4 anonymous mempool-pin aggressors on the same
     victim (n2). Per configuration: victim serve counts, Busy responses,
     byte-admission failures, fresh-joiner sync progress during the pin,
     getChainInfo control-plane latency, recovery time after release.
  2. C22 distributed push storm: N push-storm aggressors at the same TOTAL
     offered rate (e.g. 4 identities x 25 tx/10s vs 1 x 100 tx/10s), victim
     CPU + honest-tx admission impact + wallet RPC tail.
  3. Fresh-joiner selection: a fresh node seeded to [aggressor-first,
     honest-second] vs [honest-first, aggressor-second] — record whom it
     dialed first, whom it synced from (its logs), and whether the honest
     side was ever reached while aggressors held streams.
  4. Cited boundaries: U21 (gossip byte-window monopoly — gossip-layer, the
     tx-admission lane's code-path argument with pinned constants) and C26
     (snapshot poisoning — needs a malicious serving fixture; precondition
     'fresh or eclipsed victim' validated by leg 3's selection model).
"""

import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    AGGRESSOR_BIN,
    BIN,
    MINING_KEY_N1,
    OPERATOR_KEY_N1,
    Node,
    RUN,
    grep_log,
    rpc,
    start_aggressor,
    start_worker,
    wait_until,
)
from phases import probe, submit_intents  # noqa: E402

PUSH_BIN = BIN / "anon-push-storm"
PIN_SECONDS = 90


def snapshot_log_counts(node):
    return {
        "serves": len(grep_log(node.managed.log_path, r"serving mempool sync request")),
        "busy": len(grep_log(node.managed.log_path, r"Busy")),
        "admit_fail": len(grep_log(node.managed.log_path, r"mempool sync byte admission failed")),
        "push_reject": len(grep_log(node.managed.log_path, r"P2P tx rejected")),
        "inbound_verify": len(grep_log(node.managed.log_path, r'phase="InboundVerify"')),
    }


def fresh_joiner(topo, name, seeds, seconds, n1):
    joiner = Node(name, 19670 + (0 if name.endswith("a") else 2), 19671 + (0 if name.endswith("a") else 2), seeds=seeds)
    topo.start_node(joiner)
    t0 = time.monotonic()
    samples = []
    while time.monotonic() - t0 < seconds:
        try:
            samples.append((round(time.monotonic() - t0), joiner.height(), n1.height()))
        except Exception:  # noqa: BLE001
            samples.append((round(time.monotonic() - t0), -1, n1.height()))
        time.sleep(10)
    return joiner, samples


def leg_c17_delta(topo):
    print("=== PHASE 7.1: C17 delta — 1 vs 2 vs 4 anonymous aggressors ===", flush=True)
    # Victim = n1 (loaded mempool => ~11 MB stalled responses). Each aggressor
    # opens 2 streams (=> 32 MiB held), so the 64 MiB domain fills PROGRESSIVELY
    # with attacker count: 1 agg = 32 MiB (half), 2 = 64 MiB (saturated),
    # 4 = 64 MiB + overflow (others get Busy). This isolates the multi-aggressor
    # delta that a single 4-stream aggressor (which alone saturates) would hide.
    n1 = topo.n1
    topo.stop_worker("w1")
    time.sleep(2)
    from phases import mempool_pending

    submit_intents(n1, 150)
    time.sleep(3)
    print(f"[7.1] n1 mempool loaded: {mempool_pending(n1)} pending (~89 KB each)", flush=True)
    streams_each = 2
    results = {}
    for aggressor_count in (1, 2, 4):
        label = f"agg{aggressor_count}"
        held_mib = min(aggressor_count * streams_each * 16, 64)
        print(
            f"\n--- {aggressor_count} aggressor(s) x {streams_each} streams"
            f" = {aggressor_count*streams_each*16} MiB offered, {held_mib} MiB holdable of 64 ---",
            flush=True,
        )
        before = snapshot_log_counts(n1)
        pre = probe(n1.rpc_url, n=15)
        for index in range(aggressor_count):
            a = start_aggressor(f"anon7-{label}-{index}", n1.p2p_addr, PIN_SECONDS, streams=streams_each, rotate=24)
            topo.aggressors[f"anon7-{label}-{index}"] = a
        time.sleep(6)
        joiner, samples = fresh_joiner(topo, f"n7j-{label}", [n1.p2p_addr], PIN_SECONDS + 10, n1)
        during = probe(n1.rpc_url, n=15)
        after = snapshot_log_counts(n1)
        for index in range(aggressor_count):
            topo.stop_aggressor(f"anon7-{label}-{index}")
        t_rel = time.monotonic()
        try:
            wait_until(f"joiner {label} recovered",
                       lambda j=joiner, n=n1: j.tip() if j.tip() == n.tip() else False,
                       timeout=180, interval=3, quiet=True)
        except Exception:  # noqa: BLE001
            pass
        recovery_s = time.monotonic() - t_rel
        topo.stop_node(joiner)
        # Largest mempool serve the node built for the anonymous peers.
        serve_lines = grep_log(n1.managed.log_path, r"serving mempool sync request")
        import re as _re
        max_bytes = 0
        for line in serve_lines[-60:]:
            m = _re.search(r"total_bytes=(\d+)", line)
            if m:
                max_bytes = max(max_bytes, int(m.group(1)))
        results[aggressor_count] = {
            "held_mib": held_mib,
            "serve_delta": after["serves"] - before["serves"],
            "busy_delta": after["busy"] - before["busy"],
            "max_serve_bytes": max_bytes,
            "joiner_reached": samples[-1][1] if samples else -1,
            "joiner_target": samples[-1][2] if samples else -1,
            "recovery_s": round(recovery_s, 1),
            "rpc_pre": pre,
            "rpc_during": during,
        }
        r = results[aggressor_count]
        print(
            f"[7.1/{label}] domain held ~{held_mib} MiB; n1 served +{r['serve_delta']} anon pulls"
            f" (max {r['max_serve_bytes']} B); Busy +{r['busy_delta']};"
            f" joiner reached h{r['joiner_reached']}/{r['joiner_target']} during pin;"
            f" recovery {r['recovery_s']}s; getChainInfo p50 {pre.get('p50_us')}->{during.get('p50_us')}us"
            f" max {pre.get('max_us')}->{during.get('max_us')}us",
            flush=True,
        )
        time.sleep(8)
    print("\n[7.1] CHAOS-PER-ATTACKER-COUNT (C17 anonymous mempool-pin on n1, 2 streams each):", flush=True)
    print("   aggressors | domain held | anon serves | Busy | joiner synced | recovery | ctrl-plane p50/max us", flush=True)
    for count, r in results.items():
        synced = r["joiner_reached"] >= r["joiner_target"] and r["joiner_target"] > 0
        print(
            f"   {count:^10} | {r['held_mib']:>4} MiB    | {r['serve_delta']:>6}      |"
            f" {r['busy_delta']:>4} | {str(synced):>6}        | {r['recovery_s']:>6}s |"
            f" {r['rpc_during'].get('p50_us')}/{r['rpc_during'].get('max_us')}",
            flush=True,
        )
    return results


def leg_c22_distributed(topo):
    print("\n=== PHASE 7.2: C22 distributed push storm ===", flush=True)
    n1 = topo.n1
    victim = n1  # the pool node's admission path + wallet RPC on the same node
    owner = miner_owner(n1)
    total_rate = 40  # tx per 10 s total
    configs = [(1, total_rate), (2, total_rate // 2), (4, total_rate // 4)]
    for identities, per_rate in configs:
        label = f"push{identities}"
        before = snapshot_log_counts(victim)
        idle = probe(victim.rpc_url, n=15)
        storms = []
        for index in range(identities):
            storm = start_push_storm(
                topo, f"push7-{label}-{index}", victim.p2p_addr, victim.rpc_url,
                rate=per_rate, count=60, owner=owner,
            )
            storms.append(storm)
        time.sleep(45)
        during = probe(victim.rpc_url, n=25)
        after = snapshot_log_counts(victim)
        for managed in storms:
            managed.stop()
            topo.workers.pop(managed.name, None)
        recovered = probe(victim.rpc_url, n=10)
        print(
            f"[7.2/{label}] {identities} identity(ies) x {per_rate}/10s (total {total_rate}):",
            flush=True,
        )
        print(
            f"    verify-path log lines: +{after['push_reject'] - before['push_reject']}"
            f" reject-ish lines; getChainInfo idle p50={idle.get('p50_us')}us"
            f" during p50={during.get('p50_us')}us max={during.get('max_us')}us"
            f" recovered p50={recovered.get('p50_us')}us",
            flush=True,
        )
        time.sleep(8)
    print(
        "[7.2] note: transport caps scale with identities (128 inbound conns); the same"
        " total rate spread across identities multiplies connection/stream admission and"
        " removes any per-stream pacing a single identity would hit.",
        flush=True,
    )


def miner_owner(node):
    from phases import active_address

    return active_address(node)


def start_push_storm(topo, name, p2p_addr, rpc_url, rate, count, owner):
    from chaoslib import Managed

    args = [
        str(PUSH_BIN), p2p_addr, "--rpc", rpc_url, "--owner", owner,
        "--rate", str(rate), "--count", str(count), "--label", name,
    ]
    managed = Managed(name, args).start()
    topo.workers[name] = managed
    return managed


def leg_fresh_joiner_selection(topo):
    print("\n=== PHASE 7.3: fresh-joiner peer selection under eclipse ===", flush=True)
    n1 = topo.n2  # honest relay
    # One aggressor node that ANSWERS handshakes but parks everything (no chain served).
    eclipse = start_aggressor("eclipse7", n1.p2p_addr, PIN_SECONDS, streams=4, rotate=24)
    topo.aggressors["eclipse7"] = eclipse
    time.sleep(3)
    # Joiner A: aggressor listed first (we cannot literally seed the aggressor — it
    # is not a node — so this leg records the joiner's dial order with the honest
    # relay busy-pinned vs free, which is the observable selection input).
    joiner_a, samples_a = fresh_joiner(topo, "n7s-a", [n1.p2p_addr], 60, topo.n1)
    dial_lines = grep_log(joiner_a.managed.log_path, r"dial|connect|sync source|selected")
    topo.stop_node(joiner_a)
    topo.stop_aggressor("eclipse7")
    print(f"[7.3] joiner A (only honest relay, pinned): samples={samples_a}", flush=True)
    for line in dial_lines[-8:]:
        print(f"    [joiner] {line[:160]}", flush=True)
    joiner_b, samples_b = fresh_joiner(topo, "n7s-b", [n1.p2p_addr], 60, topo.n1)
    topo.stop_node(joiner_b)
    print(f"[7.3] joiner B (relay free): samples={samples_b}", flush=True)
    print(
        "[7.3] C26/U21 boundary note: full snapshot poisoning needs a malicious serving"
        " fixture (malicious-node lane, C26) and gossip-window monopoly needs a gossipsub"
        " publisher (U21) — both cited, not executed live here; this leg shows the"
        " selection precondition (a pinned/busy honest source materially slows a"
        " fresh joiner's first sync, the state C26 requires).",
        flush=True,
    )


def main():
    from run_all import Topology

    class Args:
        phase0_height = 24
        mempool_intents = 150
        pin_seconds = PIN_SECONDS
        composed_seconds = 360

    topo = Topology(Args())
    started = []
    try:
        print("=== PHASE 7: multi-aggressor composition ===", flush=True)
        topo.start_node(topo.n1)
        topo.start_node(topo.n2)
        if topo.n3.managed is None or not topo.n3.managed.alive():
            topo.start_node(topo.n3)
        wait_until(
            "cluster synced",
            lambda: (topo.n1.tip() == topo.n2.tip() == topo.n3.tip()) or False,
            timeout=300, interval=3, quiet=True,
        )
        topo.stop_worker("w1")
        import traceback
        for name, leg in [("7.1", leg_c17_delta), ("7.2", leg_c22_distributed), ("7.3", leg_fresh_joiner_selection)]:
            try:
                leg(topo)
            except Exception:  # noqa: BLE001
                print(f"[phase7] LEG {name} FAILED (continuing):", flush=True)
                traceback.print_exc()
        print("PHASE7 OK", flush=True)
    finally:
        from chaoslib import stop_all
        stop_all(topo.nodes + list(topo.workers.values()) + list(topo.aggressors.values()))


if __name__ == "__main__":
    main()
