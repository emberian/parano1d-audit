#!/usr/bin/env python3
"""Driver for the C10 live HeaderDAG-capacity pin (chaos-headerpin lane).

Phases (resumable; node processes persist via PID files under run/):
  setup   — genesis devnet: producer P + worker, victim V, twin T; pause P.
  attack  — launch the anonymous header-spam aggressor against V; wait for the
            fill (grind + announcements + probe extension) to complete.
  mine    — resume the producer; V's honest-pending grows; watch V's log for
            the Capacity rejection ("could not enter the bounded DAG");
            sustain >= 2 further honest blocks; pause the producer.
  recover — stop the aggressor, restart V, verify it syncs to P's tip.
  status  — sample heights + pin-line counts (safe to run any time).

Every process gets a PID file; shutdown is paranoid_stop then SIGTERM/SIGKILL
to exactly that PID. No pattern kills anywhere. Loopback only, ports
21080-21099.
"""

import argparse
import json
import os
import subprocess
import time
from pathlib import Path

import chaoslib
from chaoslib import (
    ChaosError,
    Managed,
    Node,
    rpc,
    start_worker,
    wait_until,
    MINING_KEY_N1,
)

LANE = Path(__file__).resolve().parents[1]
RUN = LANE / "run"
LOGS = LANE / "logs"

MINING_KEY = "headerpin-mining-p-3d8f92a1b7c4"

PRODUCER = Node(
    "hp-producer",
    21080,
    21081,
    mode="extminer",
    genesis=True,
    mining_key=MINING_KEY,
)
VICTIM = Node("hp-victim", 21082, 21083, seeds=[PRODUCER.p2p_addr])
TWIN = Node("hp-twin", 21084, 21085, seeds=[PRODUCER.p2p_addr])

WORKER_NAME = "hp-worker"
AGGRESSOR_NAME = "hp-aggressor"

STATE = RUN / "state.json"
EVIDENCE = RUN / "pin-evidence.json"

PIN_MARKERS = [
    "could not enter the bounded DAG",
]


def ensure_node(node):
    """Start the node unless its PID file already has a live process."""
    if node.managed:
        return node
    if (RUN / f"{node.name}.pid").exists():
        pid = int((RUN / f"{node.name}.pid").read_text().strip())
        try:
            os.kill(pid, 0)
            print(f"[skip] {node.name} already running (pid {pid})")
            node.managed = "adopted"
            return node
        except ProcessLookupError:
            pass
    node.start()
    return node


def save_state(**kwargs):
    state = {}
    if STATE.exists():
        state = json.loads(STATE.read_text())
    state.update(kwargs)
    STATE.write_text(json.dumps(state, indent=2))


def load_state():
    if STATE.exists():
        return json.loads(STATE.read_text())
    return {}


def log_text(name):
    path = LOGS / f"{name}.log"
    if not path.exists():
        return ""
    return path.read_text(errors="replace")


def count_marker(name, marker):
    return log_text(name).count(marker)


def pin_line_count():
    total = 0
    for marker in PIN_MARKERS:
        total += count_marker("hp-victim", marker)
    return total


def capacity_line_count():
    return count_marker("hp-victim", "header DAG reached its configured capacity")


def worker_alive():
    pid_file = RUN / f"{WORKER_NAME}.pid"
    if not pid_file.exists():
        return False
    try:
        os.kill(int(pid_file.read_text().strip()), 0)
        return True
    except (ProcessLookupError, ValueError):
        return False


def start_producer_worker():
    if worker_alive():
        print("[skip] worker already running")
        return
    start_worker(WORKER_NAME, PRODUCER, threads=6, key=MINING_KEY)


def stop_producer_worker():
    pid_file = RUN / f"{WORKER_NAME}.pid"
    if not pid_file.exists():
        return
    pid = int(pid_file.read_text().strip())
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        pid_file.unlink(missing_ok=True)
        return
    # Managed.stop uses RPC paranoid_stop first; the worker is not a node.
    os.kill(pid, signal.SIGTERM)
    for _ in range(100):
        try:
            os.kill(pid, 0)
            time.sleep(0.1)
        except ProcessLookupError:
            break
    else:
        os.kill(pid, signal.SIGKILL)
        time.sleep(0.5)
    pid_file.unlink(missing_ok=True)
    print(f"[stop] {WORKER_NAME} (pid {pid}) by PID")


import signal  # noqa: E402  (used by stop_producer_worker)


def phase_setup(args):
    ensure_node(PRODUCER)
    start_producer_worker()
    wait_until(
        f"producer height >= {args.genesis_height}",
        lambda: PRODUCER.height() if PRODUCER.height() >= args.genesis_height else False,
        timeout=args.genesis_height * 40,
        interval=5,
    )
    h0 = PRODUCER.height()
    print(f"[setup] producer at h{h0}")
    ensure_node(VICTIM)
    wait_until(
        "victim synced to producer",
        lambda: VICTIM.tip() if VICTIM.tip() == PRODUCER.tip() else False,
        timeout=300,
        interval=3,
    )
    ensure_node(TWIN)
    wait_until(
        "twin synced to producer",
        lambda: TWIN.tip() if TWIN.tip() == PRODUCER.tip() else False,
        timeout=300,
        interval=3,
    )
    stop_producer_worker()
    save_state(h0=h0, setup_done=True)
    print(
        f"[setup] devnet ready: P/V/T at h{h0}; producer worker PAUSED. "
        "Next: run --phase attack"
    )


def phase_attack(args):
    if not load_state().get("setup_done"):
        raise ChaosError("run --phase setup first")
    # The fork point is the victim's CURRENT tip: the producer keeps mining a
    # few blocks past the genesis-height milestone during V/T sync, so the
    # state's h0 can lag the true paused tip. Re-derive and re-save.
    h0 = VICTIM.height()
    if TWIN.height() < h0 or PRODUCER.height() < h0:
        raise ChaosError("victim ahead of producer/twin; sync not settled")
    save_state(h0=h0)
    argv = [
        chaoslib.AGGRESSOR_BIN,
        VICTIM.p2p_addr,
        "--rpc",
        VICTIM.rpc_url,
        "--spam-count",
        str(args.spam_count),
        "--threads",
        str(args.threads),
        "--harvest-count",
        "65",
    ]
    aggressor = Managed(AGGRESSOR_NAME, argv, log_name=AGGRESSOR_NAME)
    aggressor.start()
    wait_until(
        "aggressor fill phase complete (grind + announcements)",
        lambda: "fill phase complete" in log_text(AGGRESSOR_NAME),
        timeout=args.grind_timeout,
        interval=10,
    )
    time.sleep(5)
    markers = {
        "announced": log_text(AGGRESSOR_NAME).count("announced spam tip"),
        "served_fetches": log_text(AGGRESSOR_NAME).count("served headers fetch"),
        "victim_selected_other": log_text("hp-victim").count(
            "HeaderDAG selected another tip"
        ),
        "victim_snapshot_sync_line": log_text("hp-victim").count(
            "requires snapshot synchronization"
        ),
    }
    save_state(attack_done=True, aggressor_markers=markers)
    print(f"[attack] aggressor fill: {markers}")
    print(f"[attack] victim height now {VICTIM.height()} (should stay h{h0})")
    print("Next: run --phase mine (long: ~2h45m of honest mining)")


def phase_mine(args):
    state = load_state()
    if not state.get("attack_done"):
        raise ChaosError("run --phase attack first")
    h0 = state["h0"]
    start_producer_worker()
    print(f"[mine] producer resumed; victim at h{VICTIM.height()} (h0={h0})")

    samples = []
    started = time.time()
    deadline = started + args.max_wait_secs
    pin_seen_at = None
    pin_producer_h = None
    while time.time() < deadline:
        time.sleep(args.sample_interval)
        try:
            p_h, p_hash = PRODUCER.tip()
            v_h, v_hash = VICTIM.tip()
            t_h, t_hash = TWIN.tip()
        except Exception as exc:  # noqa: BLE001
            print(f"[mine] RPC hiccup: {exc}")
            continue
        pins = pin_line_count()
        caps = capacity_line_count()
        samples.append(
            {
                "t": round(time.time() - started, 1),
                "producer": [p_h, p_hash[:12]],
                "victim": [v_h, v_hash[:12]],
                "twin": [t_h, t_hash[:12]],
                "pin_lines": pins,
                "capacity_errors": caps,
            }
        )
        if pins > 0 and pin_seen_at is None:
            pin_seen_at = time.time()
            pin_producer_h = p_h
            print(
                f"[mine] *** PIN OBSERVED at t+{pin_seen_at - started:.0f}s: "
                f"victim frozen at h{v_h}, producer h{p_h}, pin_lines={pins} ***"
            )
        if pin_seen_at is not None:
            # Sustain: >=2 honest blocks mined after the pin while the victim
            # stays frozen. Measured from the pin-observation producer height,
            # not the mine-phase start.
            if p_h - pin_producer_h >= 2 and v_h == h0:
                print(
                    f"[mine] sustained across >=2 honest blocks "
                    f"(producer h{p_h}); pausing producer worker"
                )
                break
        if int(time.time() - started) % 300 < args.sample_interval:
            print(
                f"[mine] t+{int(time.time() - started)}s P={p_h} V={v_h} T={t_h} "
                f"pins={pins} caps={caps}",
                flush=True,
            )
    stop_producer_worker()
    evidence = {
        "h0": h0,
        "pin_observed": pin_seen_at is not None,
        "pin_seen_at_t": (pin_seen_at - started) if pin_seen_at else None,
        "pin_producer_height": pin_producer_h,
        "pin_lines": pin_line_count(),
        "capacity_errors": capacity_line_count(),
        "final": samples[-1] if samples else None,
        "samples": samples,
    }
    EVIDENCE.write_text(json.dumps(evidence, indent=2))
    print(f"[mine] evidence written to {EVIDENCE}")
    print(f"[mine] pin observed: {pin_seen_at is not None}; leaving nodes up")
    print("Next: run --phase recover")


def phase_recover(args):
    state = load_state()
    if not state.get("attack_done"):
        raise ChaosError("run --phase attack first")
    if not worker_alive():
        start_producer_worker()
        print("[recover] producer worker resumed (to generate fresh honest children)")
    # Stop the aggressor by PID.
    aggr_pid = RUN / f"{AGGRESSOR_NAME}.pid"
    if aggr_pid.exists():
        pid = int(aggr_pid.read_text().strip())
        try:
            os.kill(pid, signal.SIGTERM)
            for _ in range(100):
                os.kill(pid, 0)
                time.sleep(0.1)
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        aggr_pid.unlink(missing_ok=True)
        print(f"[recover] aggressor stopped (pid {pid})")
    pins_before = pin_line_count()
    caps_before = capacity_line_count()
    # Restart the victim with the SAME data dir: the spam stays in its DAG, so
    # the pin should SURVIVE the restart (fresh capacity lines on new children).
    vic_pid = RUN / "hp-victim.pid"
    if vic_pid.exists():
        pid = int(vic_pid.read_text().strip())
        try:
            result = rpc(VICTIM.rpc_url, "stop")
            print(f"[recover] paranoid_stop: {result}")
        except Exception as exc:  # noqa: BLE001
            print(f"[recover] paranoid_stop failed ({exc}); falling back to SIGTERM")
        try:
            os.kill(pid, signal.SIGTERM)
            for _ in range(300):
                os.kill(pid, 0)
                time.sleep(0.1)
        except ProcessLookupError:
            pass
        vic_pid.unlink(missing_ok=True)
    print("[recover] restarting victim with the SAME data dir (pin expected to persist)")
    VICTIM.managed = None
    VICTIM.start()
    # Wait for the post-restart pin: a NEW capacity line on a fresh honest child
    # while the victim stays below the producer.
    try:
        wait_until(
            "post-restart capacity rejection (pin survived restart)",
            lambda: (pin_line_count() + capacity_line_count())
            if (pin_line_count() > pins_before or capacity_line_count() > caps_before)
            else False,
            timeout=420,
            interval=10,
        )
        survived = True
    except ChaosError:
        survived = False
    v_h, _ = VICTIM.tip()
    p_h, _ = PRODUCER.tip()
    print(
        f"[recover] pin survived restart: {survived} "
        f"(pins {pins_before}->{pin_line_count()}, caps {caps_before}->{capacity_line_count()}, "
        f"V h{v_h} vs P h{p_h})"
    )
    # Fresh-state recovery: stop the worker, wipe the lane's own victim data
    # dir, restart, and sync cleanly from the producer.
    stop_producer_worker()
    import shutil

    shutil.rmtree(VICTIM.data_dir, ignore_errors=True)
    print(f"[recover] victim data dir wiped ({VICTIM.data_dir}); fresh restart")
    VICTIM.managed = None
    VICTIM.start()
    synced = wait_until(
        "victim fresh-synced to producer",
        lambda: VICTIM.tip() if VICTIM.tip() == PRODUCER.tip() else False,
        timeout=600,
        interval=5,
    )
    print(f"[recover] victim recovered with fresh state: {synced}")
    # Let the producer advance and show the victim follows.
    start_producer_worker()
    wait_until(
        "victim follows one more honest block after fresh sync",
        lambda: VICTIM.height() if VICTIM.height() > synced[0] else False,
        timeout=180,
        interval=5,
    )
    stop_producer_worker()
    save_state(
        recovered=True,
        pin_survived_restart=survived,
        pins_before_restart=pins_before,
        caps_before_restart=caps_before,
        pins_after_restart=pin_line_count(),
        caps_after_restart=capacity_line_count(),
    )
    print("[recover] done: restart-persistence and fresh-state recovery both documented")


def phase_status(_args):
    state = load_state()
    try:
        p = PRODUCER.tip()
    except Exception as exc:  # noqa: BLE001
        p = f"down ({exc})"
    try:
        v = VICTIM.tip()
    except Exception as exc:  # noqa: BLE001
        v = f"down ({exc})"
    try:
        t = TWIN.tip()
    except Exception as exc:  # noqa: BLE001
        t = f"down ({exc})"
    print(f"state: {state}")
    print(f"P={p} V={v} T={t}")
    print(f"worker_alive={worker_alive()}")
    print(f"pin_lines={pin_line_count()} capacity_errors={capacity_line_count()}")
    if EVIDENCE.exists():
        print(f"evidence: {json.loads(EVIDENCE.read_text())['final']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--phase", required=True,
        choices=["setup", "attack", "mine", "recover", "status"],
    )
    parser.add_argument("--genesis-height", type=int, default=48)
    parser.add_argument("--spam-count", type=int, default=511)
    parser.add_argument("--threads", type=int, default=10)
    parser.add_argument("--grind-timeout", type=int, default=3600)
    parser.add_argument("--max-wait-secs", type=int, default=4 * 3600)
    parser.add_argument("--sample-interval", type=float, default=20.0)
    args = parser.parse_args()
    globals()[f"phase_{args.phase}"](args)


if __name__ == "__main__":
    main()
