#!/usr/bin/env python3
"""Master runner for the devnet-chaos composition.

Owns every process (nodes, workers, aggressors). Loopback only, PID files
for everything, stop strictly by recorded PID. Run as:

    python3 scripts/run_all.py 2>&1 | tee results.txt

Phases: 0 control, 1 anonymous pin (C17), 2 supply tamper (C20), 3 operator
(C15+C19), 4 cross-dialect (C9) + cap-route check, 4.5 tip tamper (U19),
5 composed. A topology file records state so a crashed run can be cleaned
with cleanup.py (stops exactly the PIDs still on disk).
"""

import argparse
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from chaoslib import (  # noqa: E402
    BIN,
    MINING_KEY_N1,
    MINING_KEY_N4,
    Node,
    OPERATOR_KEY_N1,
    RUN,
    stop_all,
)
import phases  # noqa: E402


class Topology:
    def __init__(self, args):
        self.args = args
        self.n1 = Node(
            "n1-pool", 19610, 19611, mode="extminer", genesis=True,
            mining_key=MINING_KEY_N1, operator_key=OPERATOR_KEY_N1,
        )
        self.n2 = Node("n2-relay", 19620, 19621, seeds=[self.n1.p2p_addr])
        self.n3 = Node("n3-syncer", 19630, 19631, seeds=[self.n2.p2p_addr])
        self.n3b = Node("n3b-pinned", 19632, 19633, seeds=[self.n2.p2p_addr])
        self.n3c = Node("n3c-join", 19634, 19635, seeds=[self.n3.p2p_addr])
        self.n4 = Node(
            "n4-pool2", 19640, 19641, mode="extminer", seeds=[self.n1.p2p_addr],
            mining_key=MINING_KEY_N4,
        )
        self.nx = Node(
            "nx-v21", 19710, 19711, seeds=[self.n1.p2p_addr],
        )
        self.nx.managed = None
        # The isolated-dialect binary, if built.
        v21_bin = BIN / "parano1d-v21"
        if v21_bin.exists():
            import chaoslib

            chaoslib.NODE_BIN  # document: nx uses the v21 binary via args below
            self.nx_args_binary = v21_bin
        else:
            self.nx_args_binary = None
        self.n5 = Node("n5-tip", 19720, 19721, seeds=[self.n1.p2p_addr])
        self.nodes = []
        self.workers = {}
        self.aggressors = {}
        self.baselines = {}

    FRESH_EVERY_RUN = ("n3b-pinned", "n3c-join", "n5-tip", "nx-v21")

    def start_node(self, node):
        import shutil as _shutil

        if node.managed and node.managed.alive():
            return  # already running
        if node.name in self.FRESH_EVERY_RUN and node.data_dir.exists():
            _shutil.rmtree(node.data_dir)
        if node is self.nx and self.nx_args_binary:
            node.managed = _start_with_binary(node, self.nx_args_binary)
            from chaoslib import wait_until

            wait_until(f"{node.name} RPC ready", lambda: node.info(check=True),
                       timeout=90, interval=0.5, quiet=True)
            self.nodes.append(node)
            return
        node.start()
        self.nodes.append(node)

    def stop_node(self, node):
        if node.managed:
            node.stop()
            node.managed = None
            if node in self.nodes:
                self.nodes.remove(node)

    def stop_worker(self, name):
        worker = self.workers.pop(name, None)
        if worker:
            worker.stop()

    def stop_aggressor(self, name):
        aggressor = self.aggressors.pop(name, None)
        if aggressor:
            aggressor.stop()

    def shutdown(self):
        stop_all(self.nodes)
        for name in list(self.workers):
            self.stop_worker(name)
        for name in list(self.aggressors):
            self.stop_aggressor(name)


def _start_with_binary(node, binary):
    import chaoslib

    original = chaoslib.NODE_BIN
    try:
        chaoslib.NODE_BIN = binary
        node.start()
    finally:
        chaoslib.NODE_BIN = original


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase0-height", type=int, default=24)
    parser.add_argument("--mempool-intents", type=int, default=150)
    parser.add_argument("--pin-seconds", type=int, default=120)
    parser.add_argument("--composed-seconds", type=int, default=360)
    parser.add_argument("--phases", default="0,1,2,3,4.5,5")
    args = parser.parse_args()

    topo = Topology(args)
    selected = {p.strip() for p in args.phases.split(",")}
    try:
        if "0" not in selected:
            # Resume mode: the chain and wallets already exist in run/; bring
            # the persistent nodes back up before later phases.
            topo.start_node(topo.n1)
            topo.start_node(topo.n2)
            if "1" not in selected:
                # Phases >=2 expect n3 in the cluster; phase 1 starts it itself.
                topo.start_node(topo.n3)
                from chaoslib import wait_until as _w3
                try:
                    _w3("n3 synced", lambda: topo.n3.tip() if topo.n3.tip() == topo.n1.tip() else False,
                        timeout=300, interval=3, quiet=True)
                except Exception:  # noqa: BLE001
                    print("[runner] n3 resume-sync timed out; continuing", flush=True)
        else:
            topo.start_node(topo.n1)
            topo.start_node(topo.n2)
            phases.phase0(topo)
        import traceback

        def run_phase(tag, fn):
            if tag not in selected:
                return
            try:
                fn(topo)
            except Exception:  # noqa: BLE001 - one phase's failure must not abort the rest
                print(f"[runner] PHASE {tag} FAILED (continuing):", flush=True)
                traceback.print_exc()

        if "1" in selected:
            if topo.n3.managed is None or not topo.n3.managed.alive():
                topo.start_node(topo.n3)
                from chaoslib import wait_until as _w
                try:
                    _w("n3 synced", lambda: topo.n3.tip() if topo.n3.tip() == topo.n1.tip() else False,
                       timeout=300, interval=3, quiet=True)
                except Exception:  # noqa: BLE001
                    print("[runner] n3 sync wait timed out; continuing", flush=True)
        run_phase("1", phases.phase1)
        run_phase("2", phases.phase2)
        run_phase("3", phases.phase3)
        run_phase("4", phases.phase4)
        run_phase("4.5", phases.phase45)
        run_phase("5", phases.phase5)
        print("ALL PHASES COMPLETE", flush=True)
    finally:
        topo.shutdown()
        print("[runner] topology shut down by PID files", flush=True)


if __name__ == "__main__":
    main()
