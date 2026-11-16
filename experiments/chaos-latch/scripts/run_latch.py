#!/usr/bin/env python3
"""chaos-latch: production-scale fee-floor latch on a live early-finance devnet.

Phases (idempotent; run subsets with --phases):
  start    n1 (genesis + mining) + W1 worker + n2 (seeded); wait for v2.1 height
  fund     fund factory wallets; fan out ~850 trader UTXOs; fund n2's wallet
  preop    pre-latch controls: CreateLaunch + NFT lifecycle (collection/mint/
           listings) + control Purchase, all admitted at base floor AND mined
  latch    pause W1; 819 unfillable trades; ONE minimal sample (floor engages);
           probes; +49 samples at 100k -> floor 90,000; resume W1; mine samples
  blackout post-latch probes from 2 funded wallets on both nodes; ordinary
           spends >= floor still admit; chain keeps advancing
  join     fresh n3 joiner after the latch: does a new node inherit the pool?
  status   print floor/size/height table for all live nodes

Ports 21060-21079, all 127.0.0.1, data dirs under run/. Every process gets a
PID file; stopped only by PID (paranoid_stop then signals).
"""

import argparse
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    ChaosError,
    CLI_BIN,
    FACTORY_BIN,
    LANE,
    MINING_KEY_N1,
    Node,
    OPERATOR_KEY_N1,
    WORKER_BIN,
    rpc,
    start_worker,
    stop_all,
    wait_until,
)

TAG = "[latch]"


def factory(rpc_url, *args, timeout=1200, check=True):
    proc = subprocess.run(
        [str(FACTORY_BIN), rpc_url, *[str(a) for a in args]],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        cwd=str(LANE),
    )
    out = (proc.stdout + proc.stderr).strip()
    if check and proc.returncode != 0:
        raise ChaosError(f"factory {args} failed: {out[:2000]}")
    return out


def floor_size(node):
    out = factory(node.rpc_url, "floor")
    match = re.search(r"floor=(\d+) size=(\d+)", out)
    if not match:
        raise ChaosError(f"floor parse failed: {out[:300]}")
    return int(match.group(1)), int(match.group(2))


def utxo_count(rpc_url, wallet):
    out = factory(rpc_url, "utxos", wallet)
    return len([line for line in out.splitlines() if line.strip()])


def wallet_address(rpc_url, wallet):
    return factory(rpc_url, "address", wallet).strip().splitlines()[-1]


def cli(node, *args, timeout=180, check=False):
    proc = subprocess.run(
        [str(CLI_BIN), "--rpc", node.rpc_url, *[str(a) for a in args]],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    return proc.stdout + proc.stderr, proc.returncode


def wait_mined(node, desc, timeout=600):
    """Wait until the node mempool drains below its pre-submit size."""
    before = floor_size(node)[1]
    wait_until(
        f"{desc} mined (mempool {before} ->)",
        lambda: floor_size(node)[1] < before,
        timeout=timeout,
        interval=4,
    )


def wait_height(node, target, timeout=2400):
    wait_until(
        f"{node.name} reaches h{target}",
        lambda: node.height() >= target,
        timeout=timeout,
        interval=5,
    )


def node_active(node):
    try:
        node.info(check=True)
        return True
    except Exception:  # noqa: BLE001
        return False


def make_n1():
    return Node(
        "latch-n1",
        p2p_port=21060,
        rpc_port=21061,
        genesis=True,
        mining_key=MINING_KEY_N1,
        operator_key=OPERATOR_KEY_N1,
    )


def make_n2():
    return Node(
        "latch-n2",
        p2p_port=21062,
        rpc_port=21063,
        seeds=["127.0.0.1:21060"],
    )


def make_n3():
    return Node(
        "latch-n3",
        p2p_port=21064,
        rpc_port=21065,
        seeds=["127.0.0.1:21060"],
    )


WORKER_NAME = "latch-w1"


def phase_start(_args):
    n1 = make_n1()
    if not node_active(n1):
        n1.start()
    n2 = make_n2()
    if not node_active(n2):
        n2.start()
    worker_pid = Path(f"run/{WORKER_NAME}.pid")
    if not worker_pid.exists():
        start_worker(WORKER_NAME, n1, threads=8, key=MINING_KEY_N1)
    wait_height(n1, 30)
    wait_until(
        "n2 tracks n1",
        lambda: n2.height() >= n1.height() - 1,
        timeout=300,
    )
    print(f"{TAG} start: n1 h={n1.height()} n2 h={n2.height()}", flush=True)


def worker_running():
    pid_file = Path(f"run/{WORKER_NAME}.pid")
    if not pid_file.exists():
        return False
    try:
        import os

        os.kill(int(pid_file.read_text().strip()), 0)
        return True
    except (ProcessLookupError, ValueError):
        return False


def stop_worker():
    pid_file = Path(f"run/{WORKER_NAME}.pid")
    if pid_file.exists():
        import os
        import signal as sig

        pid = int(pid_file.read_text().strip())
        try:
            os.kill(pid, 0)
            os.kill(pid, sig.SIGTERM)
            time.sleep(2)
            try:
                os.kill(pid, 0)
                os.kill(pid, sig.SIGKILL)
            except ProcessLookupError:
                pass
        except ProcessLookupError:
            pass
        pid_file.unlink()
        print(f"{TAG} worker stopped by pid {pid}", flush=True)


def send_amount(n1, addr, amount_noid, tries=3):
    """One wallet send; retry across block boundaries on transient failures."""
    for attempt in range(tries):
        out, code = cli(n1, "send", addr, amount_noid, timeout=240)
        if code == 0:
            return out
        time.sleep(6)
    raise ChaosError(f"send {amount_noid} to {addr} failed: {out[:400]}")


def phase_fund(args):
    n1 = make_n1()
    n2 = make_n2()
    targets = [
        ("trader", 0.03, args.trader_utxos + 20),
        ("launch1", 60, 4),
        ("launch2", 60, 4),
        ("market", 10, 6),
        ("buyer", 10, 6),
    ]
    n2_addr_out, _ = cli(n2, "address")
    n2_addr = next(
        (line.strip().split()[0] for line in n2_addr_out.splitlines() if line.strip().startswith("o1")),
        None,
    )
    if n2_addr:
        # fund n2's daemon wallet once for the post-latch ordinary control
        balance_out, _ = cli(n2, "balance")
        if "NOID" not in balance_out or "0.000000" in balance_out:
            send_amount(n1, n2_addr, "50")
            print(f"{TAG} funded n2 wallet {n2_addr} with 50 NOID", flush=True)
    for wallet, amount, want in targets:
        addr = wallet_address(n1.rpc_url, wallet)
        have = utxo_count(n1.rpc_url, wallet)
        if have >= want:
            print(f"{TAG} {wallet}: {have} UTXOs already >= {want}", flush=True)
            continue
        if have == 0:
            send_amount(n1, addr, str(max(amount, 1)))
            wait_mined(n1, f"seed send to {wallet}")
            have = utxo_count(n1.rpc_url, wallet)
        if wallet == "trader":
            print(f"{TAG} fanning out trader UTXOs: {have} -> {want}", flush=True)
            t0 = time.monotonic()
            sent = 0
            while utxo_count(n1.rpc_url, "trader") < want:
                out, code = cli(n1, "send", addr, "0.03", timeout=240)
                if code != 0:
                    time.sleep(6)  # wait for a block / change confirmation
                else:
                    sent += 1
                    if sent % 50 == 0:
                        have = utxo_count(n1.rpc_url, "trader")
                        print(
                            f"{TAG} trader sends={sent} confirmed_utxos={have} "
                            f"({time.monotonic()-t0:.0f}s)",
                            flush=True,
                        )
                time.sleep(0.05)
            print(
                f"{TAG} trader has {utxo_count(n1.rpc_url, 'trader')} UTXOs "
                f"after {time.monotonic()-t0:.0f}s",
                flush=True,
            )
        else:
            while utxo_count(n1.rpc_url, wallet) < want:
                send_amount(n1, addr, str(amount))
                wait_mined(n1, f"send to {wallet}")
            print(f"{TAG} {wallet}: {utxo_count(n1.rpc_url, wallet)} UTXOs", flush=True)


def phase_preop(_args):
    n1 = make_n1()
    n2 = make_n2()

    def run(label, *fargs):
        out = factory(n1.rpc_url, *fargs)
        print(f"{TAG} {label}: {out.strip().splitlines()[-1]}", flush=True)
        return out

    run("control launch (launch1)", "launch", "launch1", "e1700000000000000000000000000000000000000000000000000000aa")
    wait_mined(n1, "control launch")
    run("collection (market)", "collection", "market", "a1000000000000000000000000000000000000000000000000000000b1")
    wait_mined(n1, "collection")
    factory(n1.rpc_url, "confirm")
    run("mint 3 items (market)", "mint", "market", "market", "3",
        "b1000000000000000000000000000000000000000000000000000000c1")
    wait_mined(n1, "mint")
    factory(n1.rpc_url, "confirm")
    run("listing #0 (market, item 0)", "list", "market", "0", "500000")
    run("listing #1 (market, item 1)", "list", "market", "1", "500000")
    wait_mined(n1, "listings")
    factory(n1.rpc_url, "confirm")
    run("control purchase (buyer, listing 1)", "buy", "buyer", "1")
    wait_mined(n1, "control purchase")
    floor, size = floor_size(n1)
    print(f"{TAG} preop done: n1 floor={floor} size={size} h={n1.height()}", flush=True)
    floor2, size2 = floor_size(n2)
    print(f"{TAG} preop n2 floor={floor2} size={size2} h={n2.height()}", flush=True)


def phase_latch(args):
    n1 = make_n1()
    n2 = make_n2()
    stop_worker()
    time.sleep(2)
    print(f"{TAG} worker paused; tip h={n1.height()}", flush=True)
    factory(n1.rpc_url, "trades", "trader", str(args.trades), "6600", "1")
    floor, size = floor_size(n1)
    print(f"{TAG} after trades: n1 floor={floor} size={size}", flush=True)
    # minimal-latch sample: fee chosen so 0.9*fee exceeds every probe fee seen
    # in preop (launch/list/buy print their exact fees)
    fee = args.min_sample_fee
    out = factory(n1.rpc_url, "sample", "trader", str(fee))
    print(f"{TAG} minimal sample: {out.strip().splitlines()[-1]}", flush=True)
    floor, size = floor_size(n1)
    print(f"{TAG} after minimal sample: n1 floor={floor} size={size} (expect {int(fee*0.9)})", flush=True)
    floor2, size2 = floor_size(n2)
    print(f"{TAG} n2 mirrors: floor={floor2} size={size2}", flush=True)
    # durability samples
    for i in range(args.extra_samples):
        factory(n1.rpc_url, "sample", "trader", "100000")
        if (i + 1) % 10 == 0:
            floor, size = floor_size(n1)
            print(f"{TAG} durability sample {i+1}/{args.extra_samples}: floor={floor} size={size}", flush=True)
    floor, size = floor_size(n1)
    print(f"{TAG} durable latch: n1 floor={floor} size={size} (expect 90000)", flush=True)
    # resume mining: the samples confirm; the floor must survive the drain
    resume_height = n1.height()
    start_worker(WORKER_NAME, n1, threads=8, key=MINING_KEY_N1)
    print(f"{TAG} worker resumed at h{resume_height}", flush=True)
    wait_until(
        "post-resume block arrives",
        lambda: n1.height() > resume_height,
        timeout=240,
    )
    time.sleep(60)  # let several blocks mine the samples away
    floor, size = floor_size(n1)
    floor2, size2 = floor_size(n2)
    print(
        f"{TAG} post-drain: n1 h={n1.height()} floor={floor} size={size}; "
        f"n2 h={n2.height()} floor={floor2} size={size2}",
        flush=True,
    )


def phase_blackout(args):
    n1 = make_n1()
    n2 = make_n2()
    probes = [
        ("CreateLaunch wallet launch1", n1, ("launch", "launch1", "e2700000000000000000000000000000000000000000000000000000bb")),
        ("CreateLaunch wallet launch2", n1, ("launch", "launch2", "e3700000000000000000000000000000000000000000000000000000cc")),
        ("CreateLaunch wallet launch1 via n2", n2, ("launch", "launch1", "e4700000000000000000000000000000000000000000000000000000dd")),
        ("CreateListing wallet market", n1, ("list", "market", "2", "500000")),
        ("CreateListing wallet market via n2", n2, ("list", "market", "2", "500000")),
        ("Purchase wallet buyer", n1, ("buy", "buyer", "0")),
        ("Purchase wallet buyer via n2", n2, ("buy", "buyer", "0")),
        ("Purchase wallet launch2", n1, ("buy", "launch2", "0")),
    ]
    for label, node, fargs in probes:
        out = factory(node.rpc_url, *fargs)
        print(f"{TAG} PROBE {label}: {out.strip().splitlines()[-1]}", flush=True)
    # ordinary spends at >= floor still admit (chain alive control)
    out = factory(n1.rpc_url, "sample", "trader", "95000")
    print(f"{TAG} ordinary >=floor control (95000): {out.strip().splitlines()[-1]}", flush=True)
    # n2's own wallet auto-fee send (a real user's daemon wallet)
    out, code = cli(n2, "send", wallet_address(n2.rpc_url, "trader"), "0.5", timeout=240)
    verdict = "ADMITTED" if code == 0 else "REJECTED"
    print(f"{TAG} n2 wallet auto-fee send verdict={verdict}: {out.strip().splitlines()[-1][:200]}", flush=True)
    time.sleep(35)
    print(f"{TAG} heights: n1={n1.height()} n2={n2.height()} (chain advancing)", flush=True)
    floor, size = floor_size(n1)
    floor2, size2 = floor_size(n2)
    print(f"{TAG} final: n1 floor={floor} size={size}; n2 floor={floor2} size={size2}", flush=True)


def phase_join(_args):
    n1 = make_n1()
    n3 = make_n3()
    if not node_active(n3):
        n3.start()
    time.sleep(90)
    floor, size = floor_size(n3)
    print(
        f"{TAG} fresh joiner n3: h={n3.height()} floor={floor} size={size} "
        f"(n1 floor={floor_size(n1)[0]} size={floor_size(n1)[1]})",
        flush=True,
    )


def phase_status(_args):
    for maker in (make_n1, make_n2, make_n3):
        node = maker()
        if not node_active(node):
            print(f"{TAG} {node.name}: down", flush=True)
            continue
        try:
            floor, size = floor_size(node)
            print(f"{TAG} {node.name}: h={node.height()} floor={floor} size={size}", flush=True)
        except ChaosError as exc:
            print(f"{TAG} {node.name}: h={node.height()} (floor fetch failed: {exc})", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--phases",
        default="start,fund,preop,latch,blackout,join",
        help="comma list: start,fund,preop,latch,blackout,join,status",
    )
    parser.add_argument("--trader-utxos", type=int, default=890)
    parser.add_argument("--trades", type=int, default=819)
    parser.add_argument("--min-sample-fee", type=int, default=11000)
    parser.add_argument("--extra-samples", type=int, default=49)
    args = parser.parse_args()
    phases = {
        "start": phase_start,
        "fund": phase_fund,
        "preop": phase_preop,
        "latch": phase_latch,
        "blackout": phase_blackout,
        "join": phase_join,
        "status": phase_status,
    }
    for name in args.phases.split(","):
        name = name.strip()
        if name not in phases:
            raise SystemExit(f"unknown phase {name}")
        print(f"{TAG} === phase {name} ===", flush=True)
        phases[name](args)


if __name__ == "__main__":
    main()
