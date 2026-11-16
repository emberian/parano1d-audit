#!/usr/bin/env python3
"""Live U21 gossip-window exhaustion run for the gossip-halt lane.

Topology (loopback, ports 21180-21199):
  n1-producer  genesis, mode=extminer, worker w1 (6 threads)
  n2-victim    seed=n1, mode=extminer, worker w2 (2 threads)  <- aggressors attach here
  n3-control   seed=n1, plain node (no miner)

Phases: baseline, control-single (1 id), control-quad (4 ids),
control-small (1 KB payloads), control-pin (C17 non-reading mempool puller),
STORM (5 ids x 50 x 303,300 B per 10.05 s = 75.8 MB/window > 64 MiB),
recovery. Heights + best hashes sampled every 2 s into run/samples.csv.
"""

import csv
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import chaoslib  # noqa: E402
from chaoslib import (  # noqa: E402
    Node,
    count_log,
    start_pin,
    start_storm,
    start_worker,
    stop_all,
    wait_until,
)

SAMPLE_INTERVAL = 2.0


class Topo:
    def __init__(self):
        self.n1 = Node(
            "n1-producer", 21180, 21181, mode="extminer", genesis=True,
            mining_key=chaoslib.MINING_KEY_N1, operator_key=chaoslib.OPERATOR_KEY_N1,
        )
        self.n2 = Node(
            "n2-victim", 21182, 21183, mode="extminer", seeds=[self.n1.p2p_addr],
            mining_key=chaoslib.MINING_KEY_N2,
        )
        self.n3 = Node("n3-control", 21184, 21185, seeds=[self.n1.p2p_addr])
        self.workers = []
        self.aggressors = []
        self.samples = []
        self.phase = "setup"
        self.t0 = time.time()

    def start_all(self):
        self.n1.start()
        self.workers.append(start_worker("w1-producer", self.n1, threads=6, key=chaoslib.MINING_KEY_N1))
        wait_until(
            "n1 mined first blocks (h>=6)",
            lambda: self.n1.height() if self.n1.height() >= 6 else False,
            timeout=900, interval=5, quiet=True,
        )
        self.n2.start()
        self.workers.append(start_worker("w2-victim", self.n2, threads=2, key=chaoslib.MINING_KEY_N2))
        self.n3.start()
        wait_until(
            "n2 synced to n1",
            lambda: self.n2.tip() if self.n2.tip() == self.n1.tip() else False,
            timeout=600, interval=3, quiet=True,
        )
        wait_until(
            "n3 synced to n1",
            lambda: self.n3.tip() if self.n3.tip() == self.n1.tip() else False,
            timeout=600, interval=3, quiet=True,
        )

    def sample(self):
        row = {
            "t": round(time.time() - self.t0, 1),
            "phase": self.phase,
        }
        for name, node in (("n1", self.n1), ("n2", self.n2), ("n3", self.n3)):
            try:
                height, best = node.tip()
                row[f"{name}_h"] = height
                row[f"{name}_hash"] = (best or "")[:16]
            except Exception as exc:  # noqa: BLE001 - record RPC failures
                row[f"{name}_h"] = f"ERR"
                row[f"{name}_hash"] = str(exc)[:40]
        self.samples.append(row)
        n1h, n2h, n3h = row.get("n1_h"), row.get("n2_h"), row.get("n3_h")
        print(
            f"[sample] t={row['t']:7.1f} {self.phase:14} n1={n1h} n2={n2h} n3={n3h}"
            f" n2==n1:{row.get('n2_hash') == row.get('n1_hash')}"
            f" n3==n1:{row.get('n3_hash') == row.get('n1_hash')}",
            flush=True,
        )

    def sample_for(self, seconds):
        deadline = time.time() + seconds
        while time.time() < deadline:
            self.sample()
            time.sleep(SAMPLE_INTERVAL)

    def announce_counts(self):
        """Victim log counters since last restart marker."""
        path = self.n2.managed.log_path
        return {
            "ann_dropped": count_log(path, r"block announcement rate limit exceeded"),
            "tx_ingress_dropped": count_log(path, r"tx gossip ingress limit exceeded"),
            "ann_decode_fail": count_log(path, r"header announcement decode failed"),
            "headers_fetched": count_log(path, r"serving headers sync request"),
        }

    def write_csv(self):
        chaoslib.RUN.mkdir(parents=True, exist_ok=True)
        out = chaoslib.RUN / "samples.csv"
        if not self.samples:
            return out
        fields = ["t", "phase"] + [
            f"{n}_{k}" for n in ("n1", "n2", "n3") for k in ("h", "hash")
        ]
        with out.open("w", newline="") as fh:
            writer = csv.DictWriter(fh, fieldnames=fields)
            writer.writeheader()
            writer.writerows(self.samples)
        return out


def phase_stats(topo, phase):
    rows = [r for r in topo.samples if r["phase"] == phase and isinstance(r.get("n1_h"), int)]
    if not rows:
        return {}
    n1 = [r["n1_h"] for r in rows]
    n2 = [r["n2_h"] if isinstance(r.get("n2_h"), int) else None for r in rows]
    n3 = [r["n3_h"] if isinstance(r.get("n3_h"), int) else None for r in rows]
    same12 = sum(
        1 for r in rows if r.get("n2_hash") == r.get("n1_hash")
    )
    same13 = sum(
        1 for r in rows if r.get("n3_hash") == r.get("n1_hash")
    )
    return {
        "phase": phase,
        "samples": len(rows),
        "n1_first_last": f"{n1[0]}->{n1[-1]}",
        "n2_first_last": f"{n2[0]}->{n2[-1]}",
        "n3_first_last": f"{n3[0]}->{n3[-1]}",
        "n2_hash_eq_n1": f"{same12}/{len(rows)}",
        "n3_hash_eq_n1": f"{same13}/{len(rows)}",
        "n2_lag_max": max(
            (r["n1_h"] - r["n2_h"]) for r in rows if isinstance(r.get("n2_h"), int)
        ) if any(isinstance(r.get("n2_h"), int) for r in rows) else None,
    }


def main():
    topo = Topo()
    started = []
    try:
        print("=== setup ===", flush=True)
        topo.start_all()
        started = [topo.n3, topo.n2, topo.n1] + topo.workers
        print(f"[setup] all synced at n1 tip {topo.n1.tip()}", flush=True)

        print("=== baseline (60s) ===", flush=True)
        topo.phase = "baseline"
        topo.sample_for(60)
        base_counts = topo.announce_counts()

        print("=== control-single: 1 identity x 50 x 303300B = 15.2 MB/window (90s) ===", flush=True)
        topo.phase = "control-single"
        c0 = topo.announce_counts()
        topo.aggressors.append(start_storm("storm-single", topo.n2.p2p_addr, identities=1, duration_secs=90))
        topo.sample_for(92)
        c1 = topo.announce_counts()
        wait_until("n2 follows n1 again", lambda: topo.n2.tip() if topo.n2.tip() == topo.n1.tip() else False,
                   timeout=180, interval=3, quiet=True)

        print("=== control-quad: 4 identities = 60.7 MB/window < 64 MiB (90s) ===", flush=True)
        topo.phase = "control-quad"
        c0q = topo.announce_counts()
        topo.aggressors.append(start_storm("storm-quad", topo.n2.p2p_addr, identities=4, duration_secs=90))
        topo.sample_for(92)
        c1q = topo.announce_counts()
        wait_until("n2 follows n1 again", lambda: topo.n2.tip() if topo.n2.tip() == topo.n1.tip() else False,
                   timeout=180, interval=3, quiet=True)

        print("=== control-small: 5 identities x 50 x 1KB = 0.25 MB/window (90s) ===", flush=True)
        topo.phase = "control-small"
        c0s = topo.announce_counts()
        topo.aggressors.append(start_storm("storm-small", topo.n2.p2p_addr, identities=5, payload_bytes=1024, duration_secs=90))
        topo.sample_for(92)
        c1s = topo.announce_counts()
        wait_until("n2 follows n1 again", lambda: topo.n2.tip() if topo.n2.tip() == topo.n1.tip() else False,
                   timeout=180, interval=3, quiet=True)

        print("=== control-pin: C17 non-reading mempool puller (90s) ===", flush=True)
        topo.phase = "control-pin"
        c0p = topo.announce_counts()
        topo.aggressors.append(start_pin("pin-c17", topo.n2.p2p_addr, streams=8, duration_secs=90))
        topo.sample_for(92)
        c1p = topo.announce_counts()
        wait_until("n2 follows n1 again", lambda: topo.n2.tip() if topo.n2.tip() == topo.n1.tip() else False,
                   timeout=180, interval=3, quiet=True)

        print("=== STORM: 5 identities x 50 x 303300B = 75.8 MB/window > 64 MiB (300s) ===", flush=True)
        topo.phase = "storm"
        storm_start = time.time()
        c0S = topo.announce_counts()
        topo.aggressors.append(start_storm("storm-main", topo.n2.p2p_addr, identities=5, duration_secs=300))
        topo.sample_for(302)
        c1S = topo.announce_counts()
        storm_end = time.time()

        print("=== recovery ===", flush=True)
        topo.phase = "recovery"
        rejoin = None
        deadline = time.time() + 240
        while time.time() < deadline:
            topo.sample()
            try:
                if topo.n2.tip() == topo.n1.tip():
                    rejoin = time.time() - storm_end
                    break
            except Exception:  # noqa: BLE001
                pass
            time.sleep(SAMPLE_INTERVAL)
        print(f"[recovery] n2 rejoined n1 tip {rejoin:.1f}s after storm end" if rejoin else "[recovery] n2 did not rejoin within 240s", flush=True)
        topo.sample_for(20)

        # ---- results ----
        print("\n===== RESULTS =====", flush=True)
        def delta(a, b, key):
            return b[key] - a[key]
        print(f"baseline ann_dropped_total={base_counts['ann_dropped']}", flush=True)
        print(f"control-single: ann_dropped=+{delta(c0, c1, 'ann_dropped')} tx_ingress_dropped=+{delta(c0, c1, 'tx_ingress_dropped')}", flush=True)
        print(f"control-quad:   ann_dropped=+{delta(c0q, c1q, 'ann_dropped')} tx_ingress_dropped=+{delta(c0q, c1q, 'tx_ingress_dropped')}", flush=True)
        print(f"control-small:  ann_dropped=+{delta(c0s, c1s, 'ann_dropped')} tx_ingress_dropped=+{delta(c0s, c1s, 'tx_ingress_dropped')}", flush=True)
        print(f"control-pin:    ann_dropped=+{delta(c0p, c1p, 'ann_dropped')}", flush=True)
        print(f"STORM:          ann_dropped=+{delta(c0S, c1S, 'ann_dropped')} tx_ingress_dropped=+{delta(c0S, c1S, 'tx_ingress_dropped')}", flush=True)
        for phase in ("baseline", "control-single", "control-quad", "control-small", "control-pin", "storm", "recovery"):
            stats = phase_stats(topo, phase)
            if stats:
                print(f"{phase}: {stats}", flush=True)
        csv_path = topo.write_csv()
        print(f"samples csv: {csv_path}", flush=True)

        # victim block-rate during storm for cadence context
        producer_rates = [r for r in topo.samples if r["phase"] == "storm"]
        if producer_rates:
            n1_first = producer_rates[0].get("n1_h")
            n1_last = producer_rates[-1].get("n1_h")
            print(f"storm window: n1 {n1_first}->{n1_last} over {len(producer_rates) * SAMPLE_INTERVAL:.0f}s", flush=True)
    finally:
        print("=== teardown ===", flush=True)
        stop_all(list(reversed(topo.aggressors)) + started)


if __name__ == "__main__":
    main()
