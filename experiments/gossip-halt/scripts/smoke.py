#!/usr/bin/env python3
"""Smoke test: 2 nodes, 1 aggressor identity, 20 s. Verifies the hand-rolled
gossipsub wire reaches the victim's tx-gossip ingress before the full run."""

import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import chaoslib  # noqa: E402
from chaoslib import Node, count_log, start_storm, start_worker, stop_all, wait_until  # noqa: E402


def main():
    n1 = Node("smoke-n1", 21190, 21191, mode="extminer", genesis=True,
              mining_key=chaoslib.MINING_KEY_N1, operator_key=chaoslib.OPERATOR_KEY_N1)
    n2 = Node("smoke-n2", 21192, 21193, seeds=[n1.p2p_addr])
    procs = []
    try:
        n1.start()
        procs.append(n1)
        w1 = start_worker("smoke-w1", n1, threads=6, key=chaoslib.MINING_KEY_N1)
        procs.append(w1)
        wait_until("n1 h>=3", lambda: n1.height() if n1.height() >= 3 else False,
                   timeout=900, interval=5, quiet=True)
        n2.start()
        procs.append(n2)
        wait_until("n2 synced", lambda: n2.tip() if n2.tip() == n1.tip() else False,
                   timeout=600, interval=3, quiet=True)
        print(f"[smoke] synced at {n1.tip()}", flush=True)

        storm = start_storm("smoke-storm", n2.p2p_addr, identities=1, per_burst=10,
                            payload_bytes=303_300, cycle_ms=10_050, duration_secs=25)
        procs.append(storm)
        deadline = time.time() + 30
        while time.time() < deadline and storm.alive():
            time.sleep(2)
        time.sleep(2)

        print(f"[smoke] storm log tail:", flush=True)
        tail = subprocess.run(["tail", "-8", str(chaoslib.LOGS / "smoke-storm.log")],
                              capture_output=True, text=True).stdout
        print(tail, flush=True)

        path = n2.managed.log_path
        for pat in [
            r"tx gossip too large",
            r"tx gossip ingress limit exceeded",
            r"Invalid signature",
            r"Invalid message",
            r"Message already received",
            r"network-v7",
            r"new block",
            r"block announcement rate limit exceeded",
        ]:
            lines = chaoslib.grep_log(path, pat)
            print(f"[smoke] n2 {pat!r}: {len(lines)}", flush=True)
            for line in lines[-3:]:
                print(f"    {line[:200]}", flush=True)
        # gossipsub-specific: look for any meshsub or gossip mention
        gl = chaoslib.grep_log(path, r"[Gg]ossip")
        print(f"[smoke] n2 gossip-ish lines: {len(gl)}", flush=True)
        for line in gl[-12:]:
            print(f"    {line[:220]}", flush=True)
        print(f"[smoke] n2 tip {n2.tip()} vs n1 {n1.tip()}", flush=True)
    finally:
        stop_all(list(reversed(procs)))


if __name__ == "__main__":
    main()
