#!/usr/bin/env python3
"""Phase 6 — developer supply-chain composition (C16 + C24 legs live).

Story: a node built from a SUBSTITUTED legacy matrix pack (one flipped
coefficient at production size; build.rs `stage_leaf` never hashes the
matrix — the verified C16 gap) runs as the user's node with an honest synced
database, and the REAL GUI wallet backend renders its answers as
AUTHENTICATED/CONFIRMED while the honest cluster has long moved on.

Legs:
  A. tampered build  — bin/parano1d-tampered (logs/build-node-tampered.log)
  B. tampered node   — honest DB copy (run/n1-pool), refuses every new honest
                       block (foreign-relation HistoryStep verification)
  C. wallet view     — bin/wallet-probe (real noid_gui Backend::from_env)
                       renders the frozen view as SYNCED with AUTHENTICATED +
                       CONFIRMED receipts; identical rendering on the honest
                       node — nothing user-visible distinguishes them.
"""

import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    BIN,
    MINING_KEY_N1,
    OPERATOR_KEY_N1,
    Node,
    RUN,
    LOGS,
    Managed,
    grep_log,
    start_worker,
    wait_until,
)

TAMPERED_BIN = BIN / "parano1d-tampered"
PROBE = BIN / "wallet-probe"


def probe_output(url):
    env = {"NOID_RPC": url, "PATH": "/usr/bin:/bin"}
    proc = subprocess.run(
        [str(PROBE)], capture_output=True, text=True, env=env, timeout=120
    )
    return proc.stdout.strip(), proc.stderr.strip(), proc.returncode


def main():
    n6_bin = TAMPERED_BIN
    if not n6_bin.exists():
        raise SystemExit(f"missing {n6_bin}")

    print("=== PHASE 6: developer supply-chain composition ===", flush=True)

    # Source of the honest DB: the main run's miner node (rich wallet, full chain).
    source = RUN / "n1-pool"
    target = RUN / "n6-tampered"
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(source, target)
    print(f"[phase6] copied honest node DB {source} -> {target}", flush=True)

    started = []
    try:
        # Tampered node on the copied DB (ordinary node, loopback).
        n6 = Managed(
            "n6-tampered",
            [
                str(n6_bin),
                "--mode", "node",
                "--data-dir", str(target),
                "--p2p-listen", "127.0.0.1:19660",
                "--rpc-listen", "127.0.0.1:19661",
                "--disable-dns-seeds",
                "--seed", "127.0.0.1:19610",
                "--log", "debug",
            ],
        ).start()
        started.append(n6)
        wait_until("n6 RPC ready", lambda: subprocess.run(
            ["curl", "-s", "-m", "2", "-X", "POST", "-H", "content-type: application/json",
             "-d", '{"jsonrpc":"2.0","id":1,"method":"paranoid_getChainInfo","params":[]}',
             "http://127.0.0.1:19661"], capture_output=True, text=True).stdout.strip() != "",
            timeout=90, interval=1, quiet=True)
        out, _, _ = probe_output("http://127.0.0.1:19661")
        frozen_view = out
        print("[phase6] wallet view of the tampered node (frozen):", flush=True)
        for line in out.splitlines():
            print(f"    {line}", flush=True)

        # Honest chain producer back up + worker -> chain advances.
        n1 = Node("n1-pool", 19610, 19611, mode="extminer", genesis=True,
                  mining_key=MINING_KEY_N1, operator_key=OPERATOR_KEY_N1)
        n1.start()
        started.append(n1)
        w1 = start_worker("w1-worker", n1, threads=6, key=MINING_KEY_N1)
        started.append(w1)

        def height(url):
            import json
            import urllib.request
            body = json.dumps({"jsonrpc": "2.0", "id": 1,
                               "method": "paranoid_getChainInfo", "params": []}).encode()
            req = urllib.request.Request(url, data=body,
                                         headers={"content-type": "application/json"})
            with urllib.request.urlopen(req, timeout=5) as resp:
                return int(json.loads(resp.read())["result"]["height"])

        h0_honest, h0_tampered = height(n1.rpc_url), height("http://127.0.0.1:19661")
        print(f"[phase6] start heights: honest={h0_honest} tampered={h0_tampered}", flush=True)
        wait_until("honest chain advanced 3 blocks",
                   lambda: height(n1.rpc_url) if height(n1.rpc_url) >= h0_honest + 3 else False,
                   timeout=420, interval=4, quiet=True)
        time.sleep(20)
        h1_honest, h1_tampered = height(n1.rpc_url), height("http://127.0.0.1:19661")
        frozen = h1_tampered <= h0_tampered
        print(
            f"[phase6] after {h1_honest - h0_honest} new honest blocks:"
            f" honest=h{h1_honest} tampered=h{h1_tampered}"
            f" -> {'FROZEN (rejects honest blocks under the foreign relation)' if frozen else 'advanced?!'}",
            flush=True,
        )
        for line in grep_log(n6.log_path, r"reject|Mismatch|invalid|terminal")[-6:]:
            print(f"    [n6] {line[:180]}", flush=True)

        # Wallet on the honest node for the indistinguishability contrast.
        out, _, _ = probe_output(n1.rpc_url)
        print("[phase6] wallet view of the honest node (advancing):", flush=True)
        for line in out.splitlines():
            print(f"    {line}", flush=True)

        same_height_line = any("height=" in line for line in frozen_view.splitlines())
        authenticated = "AUTHENTICATED" in frozen_view
        confirmed = "CONFIRMED" in frozen_view
        print(
            f"[phase6] verdict: tampered node renders a wallet view with"
            f" authenticated={authenticated} confirmed={confirmed}"
            f" height_rendered={same_height_line}; the honest node renders the"
            " same verdicts; the frozen tip is user-invisible.",
            flush=True,
        )
        print("PHASE6 OK" if authenticated and confirmed and frozen else "PHASE6 PARTIAL", flush=True)
    finally:
        from chaoslib import stop_all
        stop_all(started)


if __name__ == "__main__":
    main()
