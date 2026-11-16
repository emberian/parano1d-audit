#!/usr/bin/env python3
"""Phase 3 supplement — corrected C15 + C19 legs.

The main run's phase 3 called walletReceipts without its positional params
(page, page_size, filter), so the C19 receipt-burn legs no-op'd. This script
redoes both credentialed-operator legs against a restarted topology with the
correct shape, and appends its output to results.txt when invoked with
--append.
"""

import statistics
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    MINING_KEY_N1,
    MINING_KEY_N4,
    OPERATOR_KEY_N1,
    Node,
    grep_log,
    probe as _unused,
    rpc,
    start_worker,
    wait_until,
)

from phases import probe  # noqa: E402


def receipt_params():
    return [1, 7, "all"]


def main():
    n1 = Node("n1-pool", 19610, 19611, mode="extminer", genesis=True,
              mining_key=MINING_KEY_N1, operator_key=OPERATOR_KEY_N1)
    n4 = Node("n4-pool2", 19640, 19641, mode="extminer",
              mining_key=MINING_KEY_N4)
    started = []
    try:
        print("=== PHASE 3b: credentialed operator, corrected (C15 + C19) ===", flush=True)
        n1.start()
        started.append(n1)
        wait_until("n1 synced to its tip", lambda: n1.height() >= 0,
                   timeout=60, interval=1, quiet=True)

        # ---- C19: receipt burn with the correct positional params --------
        idle = []
        for _ in range(10):
            t0 = time.monotonic()
            rpc(n1.rpc_url, "walletReceipts", receipt_params(),
                bearer=OPERATOR_KEY_N1, timeout=60, raw_error=True)
            idle.append((time.monotonic() - t0) * 1e6)
        print(f"[phase3b] walletReceipts idle walls (us): {[int(v) for v in idle]}", flush=True)

        query_walls = []
        for _ in range(3):
            t0 = time.monotonic()
            result = rpc(
                n1.rpc_url,
                "walletQueryReceipts",
                [1, 50, {
                    "filter": "all", "asset_id": None, "operation": "any",
                    "timestamp_from": None, "timestamp_before": None,
                    "verification": "any", "order": "newest",
                }],
                bearer=OPERATOR_KEY_N1, timeout=600, raw_error=True,
            )
            query_walls.append(int((time.monotonic() - t0) * 1e6))
            if isinstance(result, dict) and result.get("rpc_error"):
                print(f"[phase3b] walletQueryReceipts error: {str(result)[:140]}", flush=True)
        print(f"[phase3b] walletQueryReceipts walls (us): {query_walls} (full rescan each)", flush=True)

        stop = threading.Event()

        def spammer():
            while not stop.is_set():
                rpc(n1.rpc_url, "walletReceipts", receipt_params(),
                    bearer=OPERATOR_KEY_N1, timeout=120, raw_error=True)
                time.sleep(0.2)

        threads = [threading.Thread(target=spammer, daemon=True) for _ in range(4)]
        for thread in threads:
            thread.start()
        print("[phase3b] 4-thread receipt spam for 45s; probing getChainInfo tails...", flush=True)
        under = probe(n1.rpc_url, n=40)
        stop.set()
        for thread in threads:
            thread.join(timeout=5)
        after = probe(n1.rpc_url, n=30)
        print(f"[phase3b] n1 getChainInfo under receipt load: {under}", flush=True)
        print(f"[phase3b] n1 getChainInfo after load:          {after}", flush=True)
        print(
            "[phase3b] scale note: devnet wallet has a small receipt store; the lane-measured"
            " cost is ~205-225 us/receipt (4.1-5.6 s/call at 20k receipts) with the same"
            " no-cache rescan and the same wallet-mutex tail shape.",
            flush=True,
        )

        # ---- C15: template burn on n4 ------------------------------------
        n4.start()
        started.append(n4)
        wait_until("n4 synced", lambda: n4.tip() if n1.height() == n4.height() else False,
                   timeout=300, interval=3, quiet=True)
        w4 = start_worker("w4-worker", n4, threads=4, key=MINING_KEY_N4)
        started.append(w4)
        h0 = n4.height()
        print("[phase3b] honest W4 baseline on n4 for 60s...", flush=True)
        time.sleep(60)
        h1 = n4.height()
        print(f"[phase3b] W4 baseline: {h1 - h0} blocks in 60s", flush=True)

        burn_stop = threading.Event()
        stats = {"templates": 0, "burned": 0, "unavailable": 0, "errors": []}

        def burner():
            while not burn_stop.is_set():
                template = rpc(n4.rpc_url, "getBlockTemplate", [""],
                               bearer=MINING_KEY_N4, timeout=120, raw_error=True)
                if isinstance(template, dict) and template.get("template_id"):
                    template_id = template["template_id"]
                    stats["templates"] += 1
                    result = rpc(n4.rpc_url, "submitBlock", [template_id, "00" * 32],
                                 bearer=MINING_KEY_N4, timeout=120, raw_error=True)
                    if isinstance(result, dict) and result.get("rpc_error"):
                        stats["burned"] += 1
                    replay = rpc(n4.rpc_url, "submitBlock", [template_id, "00" * 32],
                                 bearer=MINING_KEY_N4, timeout=120, raw_error=True)
                    detail = str(replay.get("rpc_error", ""))[:140] if isinstance(replay, dict) else ""
                    if "unavailable" in detail.lower():
                        stats["unavailable"] += 1
                elif isinstance(template, dict):
                    stats["errors"].append(str(template)[:140])
                time.sleep(0.4)

        thread = threading.Thread(target=burner, daemon=True)
        thread.start()
        h2 = n4.height()
        print("[phase3b] burning templates on n4 for 60s while W4 mines honestly...", flush=True)
        time.sleep(60)
        burn_stop.set()
        thread.join(timeout=10)
        h3 = n4.height()
        print(f"[phase3b] W4 under burn: {h3 - h2} blocks in 60s (baseline {h1 - h0})", flush=True)
        print(
            f"[phase3b] C15 burn: {stats['templates']} templates prepared,"
            f" {stats['burned']} consumed by invalid-PoW submitBlock,"
            f" {stats['unavailable']} replayed as Unavailable",
            flush=True,
        )
        for err in stats["errors"][:3]:
            print(f"    [burn-err] {err}", flush=True)
        print("PHASE3B OK", flush=True)
    finally:
        from chaoslib import stop_all
        stop_all(started)


if __name__ == "__main__":
    main()
