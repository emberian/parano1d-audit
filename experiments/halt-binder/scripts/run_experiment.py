#!/usr/bin/env python3
"""halt-binder live experiment: five graduating CreateLaunch intents through a
stock release daemon's own mempool and template proving.

Question tree under test (CD-3 / C36 at the live-daemon boundary):
(a) does the daemon's selection ever pick more definitions than the class
    capacity (mainnet Native class: headers 5 + markets 4 = 9)?
(b) where does witness binding run (getBlockTemplate proving / internal miner)?
(c) does the assert kill the daemon or is it caught?

Phases (each appends to run/results.json; idempotent per phase):
  setup    start node + worker, wait for v2.1 (h>=24), mine to a wallet target
  control2 two graduating launches (4 defs <= 9): one block, proving fine
  probe5   five graduating launches (10 defs > 9): the daemon must select at
           most four; first block carries 4, the next carries the fifth
  tight9   four graduating + one non-graduating (9 defs = exactly the cap):
           all five in one block, proving fine (the budget is exact)
  teardown stop worker + node by PID

Every process gets a PID file; nothing is killed by pattern.
"""

import argparse
import json
import os
import signal
import sys
import time
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import haltlib as H  # noqa: E402

LANE = H.LANE
RESULTS = RUN = LANE / "run" / "results.json"

# Minimum launch: 100_000 whole tokens, supply 1e11 units, sale = 3/5 = 6e10.
WHOLE_TOKENS = "100000"
SALE_UNITS = "60000000000"

NODE_NAME = "hb-n1"
WORKER_NAME = "hb-w1"
P2P, RPC = 21021, 21022

WORKER_THREADS = 6


def load_results():
    if RESULTS.exists():
        return json.loads(RESULTS.read_text())
    return {"phases": {}}


def save(results):
    RESULTS.parent.mkdir(parents=True, exist_ok=True)
    RESULTS.write_text(json.dumps(results, indent=2, sort_keys=True))


def note(results, phase, key, value):
    results["phases"].setdefault(phase, {})[key] = value
    save(results)
    print(f"[{phase}] {key} = {value}", flush=True)


def make_node():
    return H.Node(
        NODE_NAME,
        P2P,
        RPC,
        mode="extminer",
        genesis=True,
        mining_key=H.MINING_KEY,
        log="debug",
    )




def wait_tip_stable(node, rounds=3, interval=2.0):
    last = None
    stable = 0
    while stable < rounds:
        tip = node.tip()
        if tip == last:
            stable += 1
        else:
            stable = 0
        last = tip
        time.sleep(interval)
    return last


def wallet_balance(node):
    bal = H.rpc(node.rpc_url, "walletGetBalance")
    return int(bal["balance_micronoid"]), int(bal["utxo_count"])


def submit_launch(node, index, graduating, results, phase):
    """Preview + submit one CreateToken through the node's own wallet."""
    request = {
        "action": "create_token",
        "whole_tokens": WHOLE_TOKENS,
        "metadata_hash": f"{index:064x}",
    }
    if graduating:
        request["initial_buy_units"] = SALE_UNITS
    preview = H.rpc(
        node.rpc_url,
        "walletPreviewNative",
        [request],
        timeout=300,
        raw_error=True,
    )
    if not isinstance(preview, dict) or "plan_id" not in preview:
        return None, preview
    plan_id = preview["plan_id"]
    submit = H.rpc(
        node.rpc_url,
        "walletSubmitNative",
        [request, plan_id],
        timeout=300,
        raw_error=True,
    )
    if not isinstance(submit, dict) or "txid" not in submit:
        return None, submit
    return {
        "index": index,
        "graduating": graduating,
        "txid": submit["txid"],
        "plan_id": plan_id,
        "fee_micronoid": preview.get("fee_micronoid"),
        "reserve_deposit_micronoid": preview.get("reserve_deposit_micronoid"),
        "initial_buy_cost_micronoid": preview.get("initial_buy_cost_micronoid"),
    }, None


def mempool_size(node):
    return H.rpc(node.rpc_url, "getMempoolSize")


def pending_native(node):
    pending = H.rpc(node.rpc_url, "walletNativePending", raw_error=True)
    if isinstance(pending, list):
        return [p.get("txid") for p in pending]
    return pending


def template_probe(node, label, results, phase):
    """Drive the daemon's own proving path (getBlockTemplate) directly."""
    t = H.rpc(
        node.rpc_url,
        "getBlockTemplate",
        [""],
        bearer=H.MINING_KEY,
        timeout=600,
        raw_error=True,
    )
    if isinstance(t, dict) and "template_id" in t:
        summary = {
            "label": label,
            "ok": True,
            "height": t["height"],
            "n_txs": t["n_txs"],
            "template_id": t["template_id"],
            "expires_in_seconds": t.get("expires_in_seconds"),
            "tx_input_counts": t.get("tx_input_counts"),
            "tx_output_counts": t.get("tx_output_counts"),
            "claimable_fees_micronoid": t.get("claimable_fees_micronoid"),
        }
    else:
        summary = {"label": label, "ok": False, "error": t}
    print(f"[template] {label}: {json.dumps(summary)[:400]}", flush=True)
    return summary


def watch_confirmations(node, txids, timeout=900, interval=3.0):
    """Watch heights until every txid leaves the mempool; record per-height
    confirmation sets."""
    remaining = set(txids)
    timeline = []
    start_height = node.height()
    last_height = start_height
    deadline = time.time() + timeout
    while remaining and time.time() < deadline:
        height = node.height()
        if height != last_height:
            confirmed_here = []
            for txid in list(remaining):
                entry = H.rpc(
                    node.rpc_url, "getMempoolEntry", [txid], raw_error=True
                )
                if entry is None:
                    confirmed_here.append(txid)
                    remaining.discard(txid)
            timeline.append(
                {
                    "height": height,
                    "confirmed": confirmed_here,
                    "mempool_size": mempool_size(node),
                }
            )
            print(
                f"[watch] height={height} confirmed={len(confirmed_here)} "
                f"remaining={len(remaining)} mempool={mempool_size(node)}",
                flush=True,
            )
            last_height = height
        time.sleep(interval)
    return timeline, sorted(remaining)


def phase_health(node, marker):
    """Grep the node log since the last start for panic/prove markers."""
    patterns = {
        "panics": r"panicked at",
        "prepare_task_panicked": r"HistoryStep preparation task panicked",
        "prepare_failed": r"HistoryStep (preparation|witness preparation) failed",
        "template_build_failed": r"template build failed",
        "template_proved": r"external mining template proved",
        "cpu_phase_failed": r"template CPU phase failed",
        "v21_class": r"proof_class=V21\(Native\)|V21\(Native\)",
    }
    out = {}
    for key, pattern in patterns.items():
        try:
            out[key] = len(H.grep_log(node.managed.log_path, pattern))
        except Exception as exc:  # noqa: BLE001
            out[key] = f"error: {exc}"
    return out


def stop_worker_by_pid():
    """Stop the mining worker by its recorded PID only (never by pattern)."""
    pid_file = RUN / f"{WORKER_NAME}.pid"
    if not pid_file.exists():
        return False
    pid = int(pid_file.read_text().strip())
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
            time.sleep(0.5)
        except ProcessLookupError:
            break
    else:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    pid_file.unlink(missing_ok=True)
    print(f"[worker] stopped pid={pid}", flush=True)
    return True


def start_worker(node):
    return H.start_worker(WORKER_NAME, node, threads=WORKER_THREADS, key=H.MINING_KEY)


def run_setup(results, args):
    node = make_node()
    node.start()  # resumes the chain if the data dir already exists
    note(results, "setup", "node_started", True)

    # extminer mode mines nothing by itself: the worker must run first
    # (devnet-chaos convention — worker start, then wait for heights).
    start_worker(node)
    note(results, "setup", "worker_started", True)

    deadline = time.time() + 1200
    while time.time() < deadline:
        h = node.height()
        if h >= 24:
            break
        time.sleep(5)
    note(results, "setup", "height_at_v21", node.height())

    proto = H.rpc(node.rpc_url, "getNativeProtocol", raw_error=True)
    note(results, "setup", "native_protocol", proto)

    # mine until the wallet reaches the target balance.
    target = args.balance_target_micronoid
    t0 = time.time()
    last_report = 0
    while True:
        bal, utxos = wallet_balance(node)
        elapsed = int(time.time() - t0)
        if bal >= target:
            note(results, "setup", "wallet_balance_micronoid", bal)
            note(results, "setup", "wallet_utxo_count", utxos)
            note(results, "setup", "mining_seconds", elapsed)
            note(results, "setup", "height_at_target", node.height())
            break
        if elapsed > last_report + 120:
            last_report = elapsed
            print(
                f"[setup] mining: height={node.height()} "
                f"balance={bal / 1e6:.1f} NOID target={target / 1e6:.1f}",
                flush=True,
            )
        if elapsed > args.setup_timeout:
            raise H.HaltError(
                f"setup timeout: balance {bal} < {target} after {elapsed}s"
            )
        time.sleep(10)
    stop_worker_by_pid()
    note(results, "setup", "tip_at_probe_start", node.tip())
    return node


class _LiveNode:
    """Adopt the already-running node by its PID file (never restart it)."""

    def __init__(self, name, pid_file, log_path):
        self.name = name
        self.pid_file = pid_file
        self.log_path = log_path

    def pid(self):
        try:
            return int(self.pid_file.read_text().strip())
        except (FileNotFoundError, ValueError):
            return None

    def alive(self):
        pid = self.pid()
        if pid is None:
            return False
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False

    def stop(self, rpc_url=None, timeout=15):
        pid = self.pid()
        if pid is None:
            return False
        if rpc_url:
            try:
                H.rpc(rpc_url, "stop", timeout=3)
            except Exception:  # noqa: BLE001
                pass
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                return True
        os.kill(pid, signal.SIGTERM)
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                os.kill(pid, 0)
                time.sleep(0.5)
            except ProcessLookupError:
                return True
        os.kill(pid, signal.SIGKILL)
        return True


def adopt_node():
    """Adopt the running node from setup (by PID file) for later phases."""
    node = make_node()
    pid_file = RUN / f"{NODE_NAME}.pid"
    if not pid_file.exists():
        raise H.HaltError("node is not running (no PID file); run setup first")
    node.managed = _LiveNode(NODE_NAME, pid_file, H.LOGS / f"{NODE_NAME}.log")
    if not node.alive():
        raise H.HaltError("node process is not alive")
    return node


def run_launch_phase(results, args, phase, graduating, non_graduating):
    """Shared driver for control2 / probe5 / tight9."""
    node = adopt_node()

    # 1. stop the worker, wait for a stable tip
    stop_worker_by_pid()
    tip = wait_tip_stable(node)
    note(results, phase, "tip", tip)

    # 2. submit the launches
    launches = []
    errors = []
    for i in range(graduating):
        launch, err = submit_launch(node, i, True, results, phase)
        if launch:
            launches.append(launch)
            print(f"[{phase}] launch {i} (graduating) txid={launch['txid']}", flush=True)
        else:
            errors.append({"index": i, "graduating": True, "error": err})
            print(f"[{phase}] launch {i} FAILED: {json.dumps(err)[:300]}", flush=True)
        if len(launches) % 1 == 0:
            note(results, phase, "mempool_size_after_submits", mempool_size(node))
    for j in range(non_graduating):
        launch, err = submit_launch(node, 100 + j, False, results, phase)
        if launch:
            launches.append(launch)
            print(
                f"[{phase}] launch {100 + j} (non-graduating) txid={launch['txid']}",
                flush=True,
            )
        else:
            errors.append({"index": 100 + j, "graduating": False, "error": err})
            print(f"[{phase}] launch {100 + j} FAILED: {json.dumps(err)[:300]}", flush=True)
    note(results, phase, "launches_submitted", launches)
    note(results, phase, "launch_errors", errors)
    note(results, phase, "expected_definitions", 2 * graduating + 1 * non_graduating)

    if not launches:
        note(results, phase, "outcome", "no launches admitted; aborting phase")
        return

    # 3. verify the mempool holds everything we submitted
    note(results, phase, "mempool_size_final", mempool_size(node))
    note(results, phase, "pending_native", pending_native(node))

    # 4. drive the daemon's proving path directly (getBlockTemplate)
    template = template_probe(node, f"{phase}-probe", results, phase)
    note(results, phase, "template_probe", template)

    # 5. let mining resume and watch which launches land in which block
    time.sleep(31)  # let the probe's template attempt expire (TTL 30s)
    start_worker(node)
    timeline, unconfirmed = watch_confirmations(
        node, [l["txid"] for l in launches], timeout=args.confirm_timeout
    )
    note(results, phase, "confirmation_timeline", timeline)
    note(results, phase, "unconfirmed", unconfirmed)
    stop_worker_by_pid()

    # 6. health: any panic/prove failure in the node log?
    note(results, phase, "log_health", phase_health(node, phase))

    # 7. summarize per-block selection counts
    counts = [len(step.get("confirmed", [])) for step in timeline]
    note(results, phase, "confirmed_per_block", counts)
    print(f"[{phase}] done: per-block {counts}", flush=True)


def run_teardown(results, args):
    stop_worker_by_pid()
    try:
        node = adopt_node()
        node.stop(rpc_url=node.rpc_url)
    except H.HaltError:
        pass
    note(results, "teardown", "stopped", True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--phase",
        default="setup",
        choices=["setup", "control2", "probe5", "tight9", "teardown", "all"],
    )
    parser.add_argument(
        "--balance-target-micronoid",
        type=int,
        default=6_200_000_000,
        help="wallet NOID target before the launch phases (default 6200 NOID)",
    )
    parser.add_argument("--setup-timeout", type=int, default=7200)
    parser.add_argument("--confirm-timeout", type=int, default=1800)
    args = parser.parse_args()

    results = load_results()
    try:
        if args.phase in ("setup", "all"):
            run_setup(results, args)
        if args.phase in ("control2", "all"):
            run_launch_phase(results, args, "control2", graduating=2, non_graduating=0)
        if args.phase in ("probe5", "all"):
            run_launch_phase(results, args, "probe5", graduating=5, non_graduating=0)
        if args.phase in ("tight9", "all"):
            run_launch_phase(results, args, "tight9", graduating=4, non_graduating=1)
        if args.phase in ("teardown", "all"):
            run_teardown(results, args)
    except Exception:
        traceback.print_exc()
        note(results, args.phase, "fatal", traceback.format_exc()[-2000:])
        raise
    print("[main] phase complete", flush=True)


if __name__ == "__main__":
    main()
