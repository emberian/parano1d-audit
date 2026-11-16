#!/usr/bin/env python3
"""Staged driver for the chaos-asert ASERT-collapse devnet demonstration.

Stages (run in order; each reads/writes run/state.json and leaves processes
running for the next stage):

  raise   [--minutes 11]  start n1 (genesis, built-in miner) + relays n2/n3,
                         sample every block header while difficulty rises
  freeze                 stop the producer, restart n1 as a relay; snapshot
                         n3's data dir; compute anchor A, ratio R, shift delta
  fork                   promote n3's data dir to the poison miner (clock +delta),
                         mine NFORK floor blocks, verify targets == floor,
                         restart the same dir as serving node S (tip frozen)
  wait                   sleep until the fork tip ages into the honest drift
                         window, sampling relay tips + rejection counts
  adopt                  measure spontaneous adoption; restart relays so they
                         re-sync and choose by cumulative work
  produce [--blocks 15]  restart n2 as an honest built-in miner (real clock,
                         stock binary) on the adopted collapsed chain and sample
  status                 print all tips
  teardown               stop every PID-filed process of this lane
"""

import argparse
import math
import shutil
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    FLOOR_TARGET,
    HALFLIFE,
    LOGS,
    MAX_FUTURE_DRIFT,
    Node,
    POISON_BIN,
    RUN,
    STOCK_BIN,
    ChaosError,
    append_measurement,
    grep_log,
    header_row,
    load_state,
    pid_alive,
    rpc,
    save_state,
    stop_by_pidfile,
    wait_until,
)

NFORK = 18

N1 = dict(name="n1", p2p_port=21100, rpc_port=21101)
N2 = dict(name="n2", p2p_port=21102, rpc_port=21103)
N3 = dict(name="n3", p2p_port=21104, rpc_port=21105)
S = dict(name="poison-server", p2p_port=21106, rpc_port=21107)


def n1_url():
    return f"http://127.0.0.1:{N1['rpc_port']}"


def n2_url():
    return f"http://127.0.0.1:{N2['rpc_port']}"


def n3_url():
    return f"http://127.0.0.1:{N3['rpc_port']}"


def s_url():
    return f"http://127.0.0.1:{S['rpc_port']}"


def tip_of(url):
    info = rpc(url, "getChainInfo", raw_error=True)
    if not isinstance(info, dict):
        return (-1, "?")
    return (int(info.get("height", -1)), str(info.get("best_hash", "?")))


def sample_blocks(url, producer, known_heights, seen_rows):
    """Poll a node; record every newly-seen block header. Returns (height, hash)."""
    height, best = tip_of(url)
    if height < 0:
        return None
    lo = min(known_heights) if known_heights else 0
    for h in range(lo, height + 1):
        if h in known_heights:
            continue
        row = header_row(url, h)
        if row is None:
            continue
        known_heights.add(h)
        ratio = math.log2(FLOOR_TARGET / row["target_int"])
        rec = append_measurement(
            "block",
            producer=producer,
            height=h,
            ts=row["timestamp"],
            target_hex=row["target_hex"],
            target_int=row["target_int"],
            ratio_log2=round(ratio, 3),
            hash=row["hash"][:16],
        )
        seen_rows.append(rec)
    return (height, best)


# ---------------------------------------------------------------- stages ----


def stage_raise(minutes):
    state = load_state()
    already = pid_alive("n1") is not None
    if already:
        print("[raise] n1 already running (pid file); attaching sampler", flush=True)
    else:
        n1 = Node(N1["name"], N1["p2p_port"], N1["rpc_port"],
                  binary=STOCK_BIN, mode="miner", genesis=True, seeds=[])
        n1.start()
    known = set(state.get("known_heights", []))
    rows = []
    t0 = time.monotonic()
    wait_until("n1 height >= 4", lambda: (lambda h: h if h >= 4 else False)(tip_of(n1_url())[0]),
               timeout=600, interval=3, quiet=True)
    if pid_alive("n2") is None:
        n2 = Node(N2["name"], N2["p2p_port"], N2["rpc_port"],
                  binary=STOCK_BIN, mode="node", seeds=[f"127.0.0.1:{N1['p2p_port']}"],
                  log="debug")
        n2.start()
    if pid_alive("n3") is None:
        n3 = Node(N3["name"], N3["p2p_port"], N3["rpc_port"],
                  binary=STOCK_BIN, mode="node", seeds=[f"127.0.0.1:{N1['p2p_port']}"])
        n3.start()
    print(f"[raise] mining for {minutes} minutes; sampling every block", flush=True)
    next_relay_sample = 0.0
    while time.monotonic() - t0 < minutes * 60:
        res = sample_blocks(n1_url(), "n1", known, rows)
        if res and time.time() >= next_relay_sample:
            next_relay_sample = time.time() + 30
            append_measurement(
                "tip_sample", t=round(time.monotonic() - t0),
                n1=res[0], n2=tip_of(n2_url())[0], n3=tip_of(n3_url())[0],
            )
        time.sleep(2)
    res = sample_blocks(n1_url(), "n1", known, rows)
    tip = tip_of(n1_url())
    cadences = []
    prev = None
    for r in rows[-24:]:
        if prev is not None:
            cadences.append(r["wall"] - prev)
        prev = r["wall"]
    summary = {
        "tip_height": tip[0],
        "tip_hash": tip[1],
        "blocks_sampled": len(rows),
        "last24_cadence_s": {
            "mean": round(statistics.mean(cadences), 1) if cadences else None,
            "median": round(statistics.median(cadences), 1) if cadences else None,
        },
        "last_block_target_hex": rows[-1]["target_hex"] if rows else None,
        "last_block_ratio_log2": rows[-1]["ratio_log2"] if rows else None,
    }
    print(f"[raise] done: {summary}", flush=True)
    state = load_state()
    state["raise"] = summary
    state["known_heights"] = sorted(known)
    save_state(state)


def stage_freeze():
    state = load_state()
    n1 = Node(N1["name"], N1["p2p_port"], N1["rpc_port"],
              binary=STOCK_BIN, mode="miner", genesis=True, seeds=[])
    n1_url_live = n1.rpc_url
    print("[freeze] stopping producer n1 (RPC stop + PID file)", flush=True)
    stop_by_pidfile("n1", rpc_url=n1_url_live)
    n1_node = Node(N1["name"], N1["p2p_port"], N1["rpc_port"],
                  binary=STOCK_BIN, mode="node", seeds=[f"127.0.0.1:{N2['p2p_port']}"],
                  log="debug")
    n1_node.start()
    tip_h, tip_hash = n1_node.tip()
    print(f"[freeze] n1 restarted as relay at frozen tip h{tip_h} {tip_hash[:16]}", flush=True)

    # Relay n2 must agree with the frozen tip before we snapshot n3.
    wait_until("n2 at frozen tip",
               lambda: tip_of(n2_url())[0] if tip_of(n2_url())[0] >= tip_h else False,
               timeout=240, interval=3, quiet=True)
    wait_until("n3 at frozen tip",
               lambda: tip_of(n3_url())[0] if tip_of(n3_url())[0] >= tip_h else False,
               timeout=240, interval=3, quiet=True)

    # Stop n3 and snapshot its data dir so a collapse retry can start from T.
    stop_by_pidfile("n3", rpc_url=n3_url())
    src = RUN / "n3"
    dst = RUN / "n3-snapshot"
    if dst.exists():
        shutil.rmtree(dst)
    print(f"[freeze] snapshotting {src} -> {dst}", flush=True)
    shutil.copytree(src, dst)

    anchor_h = (tip_h // 6) * 6
    anchor = header_row(n1_node.rpc_url, anchor_h)
    if anchor is None:
        raise ChaosError(f"anchor header h{anchor_h} unavailable")
    ratio = FLOOR_TARGET / anchor["target_int"]
    # Collapse condition for the fork's first block (height T+1, own ts = real+delta):
    #   excess = (t_start + delta - A_ts) - (T+1-A_h)*20 >= HALFLIFE*log2(ratio)
    t_start_estimate = time.time() + 45  # poison node boot + template prep
    need = HALFLIFE * math.log2(ratio)
    schedule_correction = (tip_h + 1 - anchor_h) * 20 - (t_start_estimate - anchor["timestamp"])
    delta = int(math.ceil(need + 30 + schedule_correction + 60))
    delta = max(240, min(1500, delta))
    freeze = {
        "tip_height": tip_h,
        "tip_hash": tip_hash,
        "anchor_height": anchor_h,
        "anchor_ts": anchor["timestamp"],
        "anchor_target_hex": anchor["target_hex"],
        "anchor_target_int": anchor["target_int"],
        "ratio_R": round(ratio, 2),
        "ratio_log2": round(math.log2(ratio), 3),
        "need_excess_secs": round(need, 1),
        "schedule_correction_secs": round(schedule_correction, 1),
        "delta_secs": delta,
        "frozen_at": time.time(),
    }
    print(f"[freeze] {freeze}", flush=True)
    append_measurement("freeze", **freeze)
    state["freeze"] = freeze
    save_state(state)


def stage_fork(delta_override=None):
    state = load_state()
    fr = state["freeze"]
    delta = int(delta_override) if delta_override else fr["delta_secs"]
    tip_h = fr["tip_height"]

    p = Node("poison-miner", N3["p2p_port"], N3["rpc_port"],
             binary=POISON_BIN, mode="miner", seeds=[f"127.0.0.1:{N1['p2p_port']}",
                                                    f"127.0.0.1:{N2['p2p_port']}"],
             env={"NOID_POISON_CLOCK_OFFSET_SECS": str(delta)},
             data_dir=RUN / "n3")
    p.start()
    wait_until("poison node at frozen tip",
               lambda: tip_of(p.rpc_url)[0] if tip_of(p.rpc_url)[0] >= tip_h else False,
               timeout=300, interval=2, quiet=True)
    print(f"[fork] poison miner live, clock +{delta}s; mining {NFORK} floor blocks", flush=True)

    fork_target = tip_h + NFORK
    wait_until(f"poison fork reaches h{fork_target}",
               lambda: tip_of(p.rpc_url)[0] if tip_of(p.rpc_url)[0] >= fork_target else False,
               timeout=900, interval=3, quiet=True)

    # Self-check: every fork block must be at the exact genesis floor target.
    rows = []
    for h in range(tip_h + 1, tip_h + NFORK + 1):
        row = header_row(p.rpc_url, h)
        if row is None:
            raise ChaosError(f"fork header h{h} missing on poison node")
        rows.append(row)
        append_measurement("fork_block", height=h, ts=row["timestamp"],
                            target_hex=row["target_hex"],
                            target_int=row["target_int"], hash=row["hash"][:16])
    bad = [r["height"] for r in rows if r["target_int"] != FLOOR_TARGET]
    if bad:
        raise ChaosError(
            f"fork blocks NOT at floor target (heights {bad}); delta={delta}s was too "
            f"small for R={fr['ratio_R']}. Stop everything, restore run/n3 from "
            f"run/n3-snapshot and rerun freeze/fork with a larger delta."
        )
    first, last = rows[0], rows[-1]
    print(f"[fork] all {len(rows)} fork blocks at floor target "
          f"({first['target_hex'][:20]}...)", flush=True)

    fork_tip_h, fork_tip_hash = tip_of(p.rpc_url)
    deadline = last["timestamp"] - MAX_FUTURE_DRIFT + 15
    fork = {
        "delta_secs": delta,
        "first_fork_height": tip_h + 1,
        "fork_tip_height": fork_tip_h,
        "fork_tip_hash": fork_tip_hash,
        "first_fork_ts": first["timestamp"],
        "fork_tip_ts": last["timestamp"],
        "all_floor": not bad,
        "admissibility_deadline": deadline,
        "fork_built_at": time.time(),
    }
    print(f"[fork] {fork}", flush=True)
    append_measurement("fork_built", **fork)

    # Fix the tip: stop the miner, serve the fork from a plain node (same data
    # dir, same shifted clock) so honest peers see a bounded fork.
    stop_by_pidfile("poison-miner", rpc_url=p.rpc_url)
    s = Node(S["name"], S["p2p_port"], S["rpc_port"],
             binary=POISON_BIN, mode="node", seeds=[f"127.0.0.1:{N1['p2p_port']}",
                                                    f"127.0.0.1:{N2['p2p_port']}"],
             env={"NOID_POISON_CLOCK_OFFSET_SECS": str(delta)},
             data_dir=RUN / "n3")
    s.start()
    got = tip_of(s.rpc_url)
    print(f"[fork] serving node S up at {got}", flush=True)
    state["fork"] = fork
    save_state(state)


def stage_wait():
    state = load_state()
    fork = state["fork"]
    tip_h = state["freeze"]["tip_height"]
    deadline = fork["admissibility_deadline"]

    def reject_counts():
        out = {}
        for node in ("n1", "n2"):
            log = LOGS / f"{node}.log"
            if not log.exists():
                out[node] = -1
                continue
            lines = grep_log(log, r"BadTimestamp|future.?drift|drift gate|reject", since_last_start=True)
            out[node] = len(lines)
        return out

    t0 = time.time()
    last_counts = reject_counts()
    print(f"[wait] fork tip ts={fork['fork_tip_ts']}; admissible at unix "
          f"{deadline} ({max(0, int(deadline - time.time()))}s from now)", flush=True)
    while time.time() < deadline + 60:
        sample = {
            "t": round(time.time() - t0),
            "n1": tip_of(n1_url())[0],
            "n2": tip_of(n2_url())[0],
            "s": tip_of(s_url())[0],
        }
        counts = reject_counts()
        append_measurement("wait_sample", relays_frozen_ok=(sample["n1"] == tip_h and sample["n2"] == tip_h),
                           **sample, **{f"rejects_{k}": v for k, v in counts.items()})
        if counts != last_counts:
            print(f"[wait] t+{sample['t']}s relays={sample['n1']}/{sample['n2']} "
                  f"fork={sample['s']} rejects={counts}", flush=True)
            last_counts = counts
        time.sleep(20)
    final = {
        "relay_tips_final": {"n1": tip_of(n1_url())[0], "n2": tip_of(n2_url())[0]},
        "rejects_total": reject_counts(),
        "wait_ended_at": time.time(),
    }
    print(f"[wait] done: {final}", flush=True)
    append_measurement("wait_end", **final)
    state["wait"] = final
    save_state(state)


def stage_adopt():
    state = load_state()
    fork = state["fork"]
    fork_tip = fork["fork_tip_height"]
    fork_hash = fork["fork_tip_hash"]

    # 1) Spontaneous adoption window on the live relays.
    print("[adopt] watching live relays for spontaneous adoption (120s)...", flush=True)
    t0 = time.monotonic()
    spontaneous = {}
    while time.monotonic() - t0 < 120:
        for label, url in (("n1", n1_url()), ("n2", n2_url())):
            h, bh = tip_of(url)
            if h >= fork_tip and label not in spontaneous:
                spontaneous[label] = round(time.monotonic() - t0, 1)
                print(f"[adopt] {label} ADOPTED the fork at +{spontaneous[label]}s", flush=True)
        time.sleep(10)
    print(f"[adopt] spontaneous adoption: {spontaneous or 'none'}", flush=True)
    append_measurement("spontaneous_adoption", nodes=spontaneous)

    # 2) Force the question: restart n2 so it re-syncs headers from n1 + S.
    adopted = {}
    if "n2" not in spontaneous:
        print("[adopt] restarting n2 (fresh header sync from [n1, S])", flush=True)
        stop_by_pidfile("n2", rpc_url=n2_url())
        n2 = Node(N2["name"], N2["p2p_port"], N2["rpc_port"],
                  binary=STOCK_BIN, mode="node",
                  seeds=[f"127.0.0.1:{N1['p2p_port']}", f"127.0.0.1:{S['p2p_port']}"],
                  log="debug")
        t1 = time.monotonic()
        n2.start()
        got = wait_until("n2 selects the collapsed fork",
                         lambda: tip_of(n2_url()) if tip_of(n2_url())[0] >= fork_tip else False,
                         timeout=600, interval=5, quiet=True)
        adopted["n2"] = {
            "seconds": round(time.monotonic() - t1, 1),
            "tip": got,
            "matches_fork": got[1] == fork_hash,
        }
        print(f"[adopt] n2 re-sync chose: {got} (fork tip {fork_tip} {fork_hash[:16]}) "
              f"in {adopted['n2']['seconds']}s", flush=True)
        append_measurement("forced_adoption", node="n2", **adopted["n2"])

    # 3) n1: follow from n2, or restart it too.
    h1, bh1 = tip_of(n1_url())
    if h1 < fork_tip:
        print("[adopt] n1 still honest; watching 60s for follow-from-n2...", flush=True)
        t2 = time.monotonic()
        while time.monotonic() - t2 < 60:
            h1, bh1 = tip_of(n1_url())
            if h1 >= fork_tip:
                break
            time.sleep(10)
        if h1 < fork_tip:
            print("[adopt] restarting n1 (fresh sync from [n2, S])", flush=True)
            stop_by_pidfile("n1", rpc_url=n1_url())
            n1 = Node(N1["name"], N1["p2p_port"], N1["rpc_port"],
                      binary=STOCK_BIN, mode="node",
                      seeds=[f"127.0.0.1:{N2['p2p_port']}", f"127.0.0.1:{S['p2p_port']}"],
                      log="debug")
            t3 = time.monotonic()
            n1.start()
            got = wait_until("n1 selects the collapsed fork",
                             lambda: tip_of(n1_url()) if tip_of(n1_url())[0] >= fork_tip else False,
                             timeout=600, interval=5, quiet=True)
            adopted["n1"] = {"seconds": round(time.monotonic() - t3, 1), "tip": got}
        else:
            adopted["n1"] = {"seconds": round(time.monotonic() - t2, 1), "tip": (h1, bh1),
                            "via": "followed n2 without restart"}
        print(f"[adopt] n1: {adopted['n1']}", flush=True)
        append_measurement("forced_adoption", node="n1", **adopted["n1"])
    else:
        adopted["n1"] = {"via": "spontaneous", "tip": (h1, bh1)}

    state["adopt"] = {"spontaneous": spontaneous, "forced": adopted,
                     "fork_tip": [fork_tip, fork_hash]}
    save_state(state)


def stage_produce(blocks):
    state = load_state()
    fork_tip = state["fork"]["fork_tip_height"]

    print("[produce] restarting n2 as an honest built-in miner (stock, real clock)", flush=True)
    stop_by_pidfile("n2", rpc_url=n2_url())
    n2 = Node(N2["name"], N2["p2p_port"], N2["rpc_port"],
              binary=STOCK_BIN, mode="miner", seeds=[f"127.0.0.1:{N1['p2p_port']}"])
    n2.start()
    start_tip = tip_of(n2_url())
    print(f"[produce] producer tip {start_tip}", flush=True)

    known = set(range(0, fork_tip + 1))
    rows = []
    t0 = time.monotonic()
    while len(rows) < blocks and time.monotonic() - t0 < 40 * 60:
        sample_blocks(n2.rpc_url, "n2-postcollapse", known, rows)
        time.sleep(2)
    cadences = []
    prev = None
    for r in rows:
        if prev is not None:
            cadences.append(r["wall"] - prev)
        prev = r["wall"]
    summary = {
        "start_tip": start_tip,
        "blocks": [
            {k: r[k] for k in ("height", "ts", "target_hex", "ratio_log2")}
            for r in rows
        ],
        "n_blocks": len(rows),
        "cadence_s": {
            "mean": round(statistics.mean(cadences), 1) if cadences else None,
            "median": round(statistics.median(cadences), 1) if cadences else None,
        },
        "all_floor": all(r["target_int"] == FLOOR_TARGET for r in rows),
    }
    print(f"[produce] {summary['n_blocks']} blocks; cadence {summary['cadence_s']}; "
          f"all at floor: {summary['all_floor']}", flush=True)
    append_measurement("produce_summary", **{k: v for k, v in summary.items() if k != "blocks"})
    state["produce"] = summary
    save_state(state)


def stage_status():
    for label, url in (("n1", n1_url()), ("n2", n2_url()), ("n3/P", n3_url()), ("S", s_url())):
        print(f"{label}: {tip_of(url)}", flush=True)


def stage_teardown():
    for name, url in (("n1", n1_url()), ("n2", n2_url()), ("n3", n3_url()),
                      ("poison-miner", n3_url()), ("poison-server", s_url())):
        stop_by_pidfile(name, rpc_url=url)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("stage", choices=["raise", "freeze", "fork", "wait", "adopt",
                                          "produce", "status", "teardown"])
    parser.add_argument("--minutes", type=int, default=11)
    parser.add_argument("--blocks", type=int, default=15)
    parser.add_argument("--delta", type=int, default=None)
    args = parser.parse_args()
    if args.stage == "raise":
        stage_raise(args.minutes)
    elif args.stage == "freeze":
        stage_freeze()
    elif args.stage == "fork":
        stage_fork(args.delta)
    elif args.stage == "wait":
        stage_wait()
    elif args.stage == "adopt":
        stage_adopt()
    elif args.stage == "produce":
        stage_produce(args.blocks)
    elif args.stage == "status":
        stage_status()
    elif args.stage == "teardown":
        stage_teardown()


if __name__ == "__main__":
    main()
