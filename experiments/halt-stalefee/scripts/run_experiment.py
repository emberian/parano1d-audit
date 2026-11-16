#!/usr/bin/env python3
"""halt-stalefee: drive the stale exact-fee CreateLaunch deadlock on a live
release devnet at PRODUCTION geometry (m24 slot domain).

The required fee for a financial intent is computed from the parent header's
committed active_slot_count/log_slots with a stepwise occupancy multiplier
(1/2/4/8 at 50/75/90% of 2^log_slots). The deadlock arms when the multiplier
DROPS after admission (the intent then overpays, is never evicted, stays
selected, and aborts every template build). At m24 the drop happens through the
slot-domain EXPANSION event: a strict 10-of-18 majority of the finalized
window [P-35, P-18] at >=75% occupancy expands log_slots 24 -> 25, halving
occupancy (75% -> 37.5%) and dropping the multiplier 4 -> 1.

Phases:
  setup      Mac control devnet (shared early-finance binaries): n1 producer
             (--genesis) + stock worker + n2 peer; wait for V2.1 (h>=24)
  control    fresh exact-fee CreateLaunch at multiplier 1 confirms (happy path)
  calibrate  measure the effective per-block user-page budget under pressure
  takeover   start a node on the inherited big state (m24, 12.58M+ slots) and
             report the expansion schedule (first tip P* whose window primes)
  prep       while the window is still short of priming: fund the launch owner
             and split the wallet's coinbases into enough spendable UTXOs for
             the pressure set
  deadlock   freeze the tip at P*-1, load pressure, admit the exact-fee
             CreateLaunch (multiplier 4), resume: block P* (normal) and P*+1
             (expansion) exclude the launch; afterwards required fee drops to
             multiplier 1, the launch is retained/selected and every template
             build fails -> chain halt; sustain --halt-mins minutes
  displace   recovery control: full budget of higher-fee sends mines one block
             without the stale launch; the halt returns once they drain
  restart    recovery control: restart n1 (mempool is RAM) and watch the chain
             advance again
  teardown   stop every process by PID file

Every process gets a PID file under run/. No pattern kills anywhere.
"""

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    INTENTOOL_BIN,
    MINING_KEY_N1,
    OPERATOR_KEY_N1,
    ChaosError,
    Managed,
    Node,
    grep_log,
    rpc,
    wait_until,
)

LANE = Path(__file__).resolve().parents[1]
RUN = LANE / "run"
LOGS = LANE / "logs"
WORKER_BIN = LANE.parent / "devnet-chaos" / "bin" / "parano1d-miner"

N1_P2P = 21000
N1_RPC = 21001
N2_P2P = 21010
N2_RPC = 21011

# m24 production geometry: capacity 2^24 = 16,777,216 slots.
# multiplier boundaries: 50% = 8,388,608; 75% = 12,582,912; 90% = 15,099,494.
# expansion trigger: >=75% for a strict 10-of-18 finalized-window majority.
OCCUPANCY_75 = 12_582_912
FINALITY = 18          # CONSENSUS_FINALITY_DEPTH = EXPANSION_WINDOW
WINDOW_SPAN = 35      # [P-35, P-18]
EXPAND_MAJORITY = 10

# CreateLaunch shape: 3 inputs, 4 outputs, 1 net-new slot:
#   mult 1: 10,600 | mult 2: 13,100 | mult 4: 18,100 | mult 8: 23,100
PRESSURE_FEE = 20_000       # explicit overpay on every pressure send
DISPLACE_FEE = 100_000      # even higher rate for the displacement control
PRESSURE_BLOCKS = 2        # blocks the pressure must span (P* normal + P*+1 expansion)

OWNER_A_SECRET = "11" * 32  # fresh-intent control launch owner
OWNER_B_SECRET = "22" * 32  # stale/deadlock launch owner


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def h(what):
    print(f"\n===== {what} =====\n", flush=True)


# ---------------------------------------------------------------------------
# node helpers
# ---------------------------------------------------------------------------


def height(node):
    return int(rpc(node.rpc_url, "getChainInfo")["height"])


def best_hash(node):
    return rpc(node.rpc_url, "getChainInfo")["best_hash"]


def active_slots(node):
    return int(rpc(node.rpc_url, "getActiveSlotCount"))


def header_at(node, h):
    result = rpc(node.rpc_url, "getBlockHeader", [h])
    if result is None:
        raise ChaosError(f"header {h} unavailable")
    return result


def mempool_size(node):
    return int(rpc(node.rpc_url, "getMempoolSize"))


def wallet_address(node):
    return rpc(node.rpc_url, "walletGetAddress")["address"]


def wallet_utxos(node):
    snapshot = rpc(node.rpc_url, "walletUtxoSnapshot")
    utxos = snapshot.get("utxos") or []
    live = [u for u in utxos if not u.get("reserved")]
    live.sort(key=lambda u: -int(u["value_micronoid"]))
    return live


def wallet_send(node, amount_micronoid, fee_micronoid, timeout=180):
    return rpc(
        node.rpc_url, "walletSend", [wallet_address(node), amount_micronoid, fee_micronoid],
        timeout=timeout,
    )


def intentool(*args, timeout=900):
    return subprocess.run(
        [str(INTENTOOL_BIN), *args], capture_output=True, text=True, timeout=timeout
    )


def owner_address(secret_hex):
    """Derive the launch owner address from the intentool launch output
    (it prints the owner before collecting inputs, so an unfunded owner is
    fine)."""
    proc = intentool(
        "launch", "--rpc", f"http://127.0.0.1:{N1_RPC}", "--secret-hex", secret_hex
    )
    text = proc.stdout + proc.stderr
    match = re.search(r"launch owner (o1\S+)", text)
    if match:
        return match.group(1)
    raise ChaosError(f"could not derive owner address: {text[:500]}")


def fund_owner(node, secret_hex, count, per_send_noid):
    owner_addr = owner_address(secret_hex)
    for i in range(count):
        result = rpc(
            node.rpc_url, "walletSend",
            [owner_addr, int(per_send_noid * 1_000_000), 0], timeout=180,
        )
        log(f"fund[{i}] -> {result.get('tx_hash', result)}")
    return owner_addr


def submit_launch(secret_hex, out_name, retries=2):
    out = RUN / out_name
    for attempt in range(retries):
        proc = intentool(
            "launch", "--rpc", f"http://127.0.0.1:{N1_RPC}",
            "--secret-hex", secret_hex, "--submit", "--out", str(out),
        )
        if proc.returncode == 0:
            return proc
        log(f"launch attempt {attempt} failed: {proc.stdout}\n{proc.stderr}")
        time.sleep(3)
    raise ChaosError(f"launch failed after {retries} attempts")


def wait_drained(node, timeout=600):
    wait_until(
        "mempool drained",
        lambda: mempool_size(node) == 0 or None,
        timeout=timeout, interval=3, quiet=True,
    )


def expansion_window_counts(node, tip_height):
    """Occupancy counts of the finalized window [P-35, P-18] for parent
    height P = tip_height (mirrors finalized_expansion_window)."""
    start = tip_height - WINDOW_SPAN
    end = tip_height - FINALITY
    if start < 1 or end < start:
        return []
    counts = []
    for hh in range(start, end + 1):
        counts.append(int(header_at(node, hh)["active_slot_count"]))
    return counts


def window_majority(counts):
    return sum(1 for c in counts if c >= OCCUPANCY_75)


def first_expansion_parent(node, from_height=None):
    """Smallest parent height P >= from_height whose window is >= 10-of-18 at
    75%; the child of P is the expansion block. Walks forward using the live
    tip (headers exist only up to the tip)."""
    tip = height(node)
    best = None
    for p in range(from_height or max(tip - 60, 1), tip + 1):
        counts = expansion_window_counts(node, p)
        if len(counts) == FINALITY and window_majority(counts) >= EXPAND_MAJORITY:
            best = p
            break
    return best, tip


def worker_cmd(rpc_url):
    return [
        str(WORKER_BIN), "--rpc", rpc_url, "--threads", "6", "--key", MINING_KEY_N1
    ]


def start_worker(name, node):
    return Managed(name, worker_cmd(node.rpc_url), env={"RUST_LOG": "info"}).start()


def stop_worker(topo):
    worker = topo.pop("w1", None)
    if worker:
        worker.stop()
        log("worker stopped")


# ---------------------------------------------------------------------------
# phases — part 1 (Mac control devnet)
# ---------------------------------------------------------------------------


def phase_setup(topo):
    h("SETUP: control devnet (shared early-finance binaries, m24)")
    n1 = Node(
        "n1-producer", N1_P2P, N1_RPC, mode="extminer", genesis=True,
        mining_key=MINING_KEY_N1, operator_key=OPERATOR_KEY_N1,
    )
    n1.start()
    topo["n1"] = n1
    topo["w1"] = start_worker("w1-worker", n1)
    n2 = Node("n2-peer", N2_P2P, N2_RPC, seeds=[f"127.0.0.1:{N1_P2P}"])
    n2.start()
    topo["n2"] = n2

    wait_until("V2.1 activation (h>=24)", lambda: height(n1) >= 24 or None,
               timeout=1800, interval=5)
    header = header_at(n1, height(n1))
    log(f"tip h={header['height']} log_slots={header['log_slots']} "
        f"active_slot_count={header['active_slot_count']}")
    if header["log_slots"] != 24:
        raise ChaosError(f"expected m24 domain, got log_slots={header['log_slots']}")
    wait_until("n2 follows n1", lambda: height(n2) == height(n1) or None,
               timeout=300, interval=5)
    log(f"n1 h={height(n1)} n2 h={height(n2)} — control devnet up")


def phase_control(topo):
    h("CONTROL: fresh exact-fee CreateLaunch at multiplier 1 confirms")
    n1 = topo["n1"]
    slots = active_slots(n1)
    log(f"active_slots={slots} (multiplier 1 expected at this scale)")
    if slots >= OCCUPANCY_75 // 2:
        raise ChaosError(f"control devnet grew unexpectedly large: {slots}")
    owner_addr = fund_owner(n1, OWNER_A_SECRET, 3, 40.0)
    log(f"control owner funded: {owner_addr}")
    wait_drained(n1)
    h0 = height(n1)
    wait_until("funding confirmed", lambda: height(n1) > h0 or None,
               timeout=300, interval=3)
    proc = submit_launch(OWNER_A_SECRET, "control-launch.hex")
    log(f"control launch: {proc.stdout.strip()}")
    h1_ = height(n1)
    wait_until(
        "control launch confirmed",
        lambda: height(n1) > h1_ and mempool_size(n1) == 0 or None,
        timeout=600, interval=5,
    )
    log(f"CONTROL OK: fresh intent admitted at the exact fee and confirmed; "
        f"height {h1_} -> {height(n1)}")


def phase_calibrate(topo, pressure_fee=PRESSURE_FEE):
    h("CALIBRATE: effective per-block user budget under pressure")
    n1 = topo["n1"]
    stop_worker(topo)
    log("worker stopped; loading calibration pressure")
    sent = submit_pressure(n1, pressure_fee, count=90)
    pool = mempool_size(n1)
    log(f"pool={pool} (sent {sent})")
    topo["w1"] = start_worker("w1-worker", n1)
    h0 = height(n1)
    wait_until("calibration block mined", lambda: height(n1) > h0 or None,
               timeout=300, interval=5)
    block = rpc(n1.rpc_url, "getBlock", [h0 + 1])
    txs = block.get("transactions") or []
    user = [t for t in txs if not t.get("coinbase")]
    budget = len(user)
    remaining = mempool_size(n1)
    log(f"calibration block {h0+1}: user txs={budget} pool left={remaining}")
    # let the rest drain
    wait_drained(n1, timeout=900)
    result = {
        "block": h0 + 1,
        "user_txs_in_block": budget,
        "pool_after": remaining,
        "sent": sent,
    }
    (RUN / "calibration.json").write_text(json.dumps(result, indent=2))
    log(f"CALIBRATE OK: per-block user budget ~= {budget}")
    return budget


# ---------------------------------------------------------------------------
# pressure helpers (shared by calibrate / deadlock / displace)
# ---------------------------------------------------------------------------


def submit_pressure(n1, fee, count):
    """Overpaying wallet sends, largest-first selection:
    2-in/1-out shrinkers first (rate fee/3), then 1-in/1-out fillers (fee/2).
    All rates must exceed the launch's fee_rate at the admission multiplier."""
    sent = {"two": 0, "one": 0}
    for i in range(count):
        utxos = wallet_utxos(n1)
        if len(utxos) < 2:
            log(f"pressure stopped: only {len(utxos)} unreserved UTXOs")
            break
        top = [int(u["value_micronoid"]) for u in utxos[:2]]
        if i < count // 2 and len(utxos) >= 4:
            amount = top[0] + top[1] - fee
            shape = "two"
        else:
            amount = top[0] - fee
            shape = "one"
        if amount <= 0:
            log(f"pressure stopped: top UTXO too small ({top[0]})")
            break
        try:
            rpc(n1.rpc_url, "walletSend", [wallet_address(n1), amount, fee],
                timeout=180)
            sent[shape] += 1
        except ChaosError as exc:
            log(f"pressure send {i} failed: {exc}")
            break
        if (i + 1) % 20 == 0:
            log(f"pressure {i+1}/{count} ({sent})")
    log(f"pressure submitted: {sent}")
    return sent


def split_round(n1, count):
    """Self-sends (auto fee) to double spendable UTXOs; needs a block to
    confirm before the outputs are spendable."""
    own = wallet_address(n1)
    sent = 0
    for _ in range(count):
        try:
            rpc(n1.rpc_url, "walletSend", [own, 50_000, 0], timeout=180)
            sent += 1
        except ChaosError as exc:
            log(f"split send failed: {exc}")
            break
    log(f"split round: {sent} sends submitted")
    return sent


# ---------------------------------------------------------------------------
# phases — part 2 (big-state expansion deadlock)
# ---------------------------------------------------------------------------


def phase_takeover(topo, state_dir):
    h(f"TAKEOVER: start n1 on the inherited big state ({state_dir})")
    # stop the control devnet nodes but keep the run dir separate
    for name in ("w1", "n2", "n1"):
        item = topo.pop(name, None)
        if item:
            try:
                item.stop()
            except Exception as exc:  # noqa: BLE001
                log(f"stop {name}: {exc}")
    data_dir = RUN / "n1-producer"
    if data_dir.exists() and not state_dir:
        pass  # reuse
    elif state_dir:
        if data_dir.exists():
            raise ChaosError(
                f"{data_dir} already exists; move it aside before takeover"
            )
        # the node data dir layout: copy the inherited state as the node's
        # data dir root (chain + wallet files travel together)
        subprocess.run(["cp", "-R", str(state_dir), str(data_dir)], check=True)
        log(f"copied {state_dir} -> {data_dir}")
    n1 = Node(
        "n1-producer", N1_P2P, N1_RPC, mode="extminer",
        mining_key=MINING_KEY_N1, operator_key=OPERATOR_KEY_N1,
    )
    n1.start()
    topo["n1"] = n1
    topo["w1"] = start_worker("w1-worker", n1)
    n2 = Node("n2-peer", N2_P2P, N2_RPC, seeds=[f"127.0.0.1:{N1_P2P}"])
    n2.start()
    topo["n2"] = n2
    wait_until("n2 follows n1", lambda: height(n2) == height(n1) or None,
               timeout=900, interval=5)
    slots = active_slots(n1)
    log(f"takeover: h={height(n1)} active_slots={slots} "
        f"({100*slots/16_777_216:.1f}% of m24)")
    if slots < OCCUPANCY_75:
        log(f"WARNING: state below 75% ({slots} < {OCCUPANCY_75})")
    bal = rpc(n1.rpc_url, "walletGetBalance")
    log(f"wallet on takeover: {bal}")
    pstar, tip = first_expansion_parent(n1)
    log(f"first expansion parent P* = {pstar} (tip {tip})")
    if pstar is not None and pstar <= tip:
        log("WARNING: window already primed — the next child expands!")
    result = {
        "takeover_height": tip,
        "active_slots": slots,
        "first_expansion_parent": pstar,
        "wallet_balance": bal,
    }
    (RUN / "takeover.json").write_text(json.dumps(result, indent=2))


def phase_prep(topo):
    h("PREP: fund the launch owner and build the pressure UTXO supply")
    n1 = topo["n1"]
    # 1. fund the deadlock launch owner (3 UTXOs, ~40 NOID each; the deposit
    #    for a 100k-token launch is 100 NOID)
    fund_owner(n1, OWNER_B_SECRET, 3, 40.0)
    wait_drained(n1)
    h0 = height(n1)
    wait_until("owner funding confirmed", lambda: height(n1) > h0 or None,
               timeout=300, interval=3)
    # 2. split rounds until enough spendable UTXOs for the pressure set
    target = int(topo.get("pressure_target", 160))
    while len(wallet_utxos(n1)) < target:
        before = len(wallet_utxos(n1))
        split_round(n1, before)
        wait_drained(n1, timeout=900)
        h0 = height(n1)
        wait_until("split round confirmed", lambda: height(n1) > h0 or None,
                   timeout=300, interval=3)
        after = len(wallet_utxos(n1))
        log(f"spendable UTXOs: {before} -> {after} (target {target})")
        if after <= before:
            raise ChaosError("split round did not grow the UTXO set")
        # keep the window from priming while we still need blocks: checked by
        # the caller; splits add +1 slot each (negligible at 12.58M+).
    log(f"PREP OK: {len(wallet_utxos(n1))} spendable UTXOs, "
        f"launch owner funded, tip h={height(n1)}")


def phase_deadlock(topo, halt_minutes=5):
    h("DEADLOCK: stale exact-fee CreateLaunch across the expansion boundary")
    n1 = topo["n1"]
    slots = active_slots(n1)
    log(f"active_slots={slots} ({100*slots/16_777_216:.1f}%)")
    pstar, tip = first_expansion_parent(n1)
    log(f"P*={pstar} tip={tip}")
    if pstar is None:
        raise ChaosError("expansion parent not yet derivable — mine more blocks")
    if tip >= pstar:
        raise ChaosError(
            f"window already primed (tip {tip} >= P* {pstar}) — the expansion "
            "fired; the multiplier-4 fee window is spent on this chain"
        )
    # mine forward to P*-1, prepping finished
    wait_until(
        f"tip reaches P*-1 = {pstar-1}",
        lambda: height(n1) >= pstar - 1 or None,
        timeout=3600, interval=5,
    )
    if height(n1) >= pstar:
        raise ChaosError("overshot P* — expansion already fired")
    # freeze the tip at exactly P*-1
    stop_worker(topo)
    frozen_tip = height(n1)
    log(f"tip frozen at {frozen_tip} (P*-1); window at 9-of-18 expected")
    counts = expansion_window_counts(n1, frozen_tip)
    log(f"window majority at frozen tip: {window_majority(counts)}/18")

    # load the pressure set spanning TWO blocks (P* normal + P*+1 expansion)
    pressure_target = int(topo.get("pressure_target", 160))
    sent = submit_pressure(n1, PRESSURE_FEE, pressure_target)
    pool_after_pressure = mempool_size(n1)
    log(f"pool={pool_after_pressure} after pressure")

    # admit the launch at the exact fee for the frozen tip (multiplier 4)
    proc = submit_launch(OWNER_B_SECRET, "stale-launch.hex")
    log(f"stale launch admitted: {proc.stdout.strip()}")
    if mempool_size(n1) != pool_after_pressure + 1:
        log(f"WARNING: pool={mempool_size(n1)} expected {pool_after_pressure+1}")

    # resume: block P* (normal) then P*+1 (expansion) exclude the launch
    topo["w1"] = start_worker("w1-worker", n1)

    def crossed_or_halted():
        if height(n1) >= pstar + 1:
            return "expanded"
        fee_lines = [
            line for line in grep_log(n1.managed.log_path, r"template build failed")
            if "Fee" in line
        ]
        if fee_lines and height(n1) > frozen_tip:
            return "halted-early"
        return None

    outcome = wait_until(
        "expansion block mined (or halt arrived early)",
        crossed_or_halted, timeout=600, interval=5,
    )
    log(f"crossing outcome: {outcome}")
    top = height(n1)
    expansion_header = header_at(n1, top)
    log(f"tip h={top}: log_slots={expansion_header['log_slots']} "
        f"active={expansion_header['active_slot_count']}")
    if expansion_header["log_slots"] != 25:
        log("WARNING: expected expansion to m25 at the crossing tip")

    # the halt: every template including the launch now fails
    halt_started = time.time()
    samples = []
    deadline = halt_started + halt_minutes * 60
    while time.time() < deadline:
        time.sleep(30)
        samples.append({
            "t": round(time.time() - halt_started),
            "height": height(n1),
            "active_slots": active_slots(n1),
            "mempool": mempool_size(n1),
        })
        log(f"halt+{samples[-1]['t']}s h={samples[-1]['height']} "
            f"slots={samples[-1]['active_slots']} pool={samples[-1]['mempool']}")
    frozen = all(s["height"] == samples[0]["height"] for s in samples)

    n1_failures = grep_log(n1.managed.log_path, r"template build failed")
    n1_fee = [line for line in n1_failures if "Fee" in line]
    worker_failures = grep_log(LOGS / "w1-worker.log", r"template fetch failed")
    n2_h = height(topo["n2"])
    n2_pool = mempool_size(topo["n2"])
    log(f"evidence: n1 template-build-failed={len(n1_failures)} "
        f"(Fee: {len(n1_fee)}) worker fetch-failed={len(worker_failures)}")
    if n1_fee:
        log(f"first Fee line: {n1_fee[0]}")
        log(f"last  Fee line: {n1_fee[-1]}")
    report = {
        "frozen": frozen,
        "frozen_height": samples[0]["height"],
        "crossing_outcome": outcome,
        "crossing_tip": top,
        "expansion_height": pstar + 1,
        "frozen_tip_at_admission": frozen_tip,
        "samples": samples,
        "n1_failures": len(n1_failures),
        "n1_fee_failures": len(n1_fee),
        "first_fee_line": n1_fee[0] if n1_fee else None,
        "worker_failures": len(worker_failures),
        "n2_height": n2_h,
        "n2_pool": n2_pool,
        "mempool_retains_launch": mempool_size(n1) >= 1,
    }
    (RUN / "deadlock-evidence.json").write_text(json.dumps(report, indent=2))
    log(f"HALT {'CONFIRMED' if frozen else 'NOT CONFIRMED'} — "
        f"evidence in {RUN / 'deadlock-evidence.json'}")
    return frozen


def phase_displace(topo):
    h("DISPLACE: budget-saturating higher-fee sends mine one block")
    n1 = topo["n1"]
    before_h = height(n1)
    sent = submit_pressure(n1, DISPLACE_FEE, int(topo.get("pressure_target", 160)))
    log(f"displacement pressure: {sent}")
    wait_until("displacement block mined", lambda: height(n1) > before_h or None,
               timeout=600, interval=5)
    after_h = height(n1)
    log(f"displacement block: {before_h} -> {after_h}")
    time.sleep(90)
    failures = grep_log(n1.managed.log_path, r"template build failed")
    fee_lines = [line for line in failures if "Fee" in line]
    result = {
        "displacement_block": [before_h, after_h],
        "mempool_after": mempool_size(n1),
        "fee_failures_total": len(fee_lines),
        "halt_returned": height(n1) == after_h,
    }
    log(f"DISPLACE OK: {result}")
    (RUN / "displace-evidence.json").write_text(json.dumps(result, indent=2))


def phase_restart(topo):
    h("RESTART: RAM mempool cleared, chain advances")
    n1 = topo["n1"]
    frozen_h = height(n1)
    n1.stop()
    time.sleep(3)
    n1b = Node(
        "n1-producer", N1_P2P, N1_RPC, mode="extminer",
        mining_key=MINING_KEY_N1, operator_key=OPERATOR_KEY_N1,
    )
    n1b.start()
    topo["n1"] = n1b
    log(f"restarted at h={height(n1b)} pool={mempool_size(n1b)}")
    wait_until("chain advances after restart",
               lambda: height(n1b) > frozen_h or None, timeout=600, interval=5)
    result = {
        "frozen_height": frozen_h,
        "advanced_to": height(n1b),
        "mempool_after_restart": mempool_size(n1b),
    }
    log(f"RESTART OK: {result}")
    (RUN / "restart-evidence.json").write_text(json.dumps(result, indent=2))


def phase_teardown(topo):
    h("TEARDOWN")
    for name in ("w1", "n2", "n1"):
        item = topo.get(name)
        if item:
            try:
                item.stop()
            except Exception as exc:  # noqa: BLE001
                log(f"stop {name}: {exc}")
    log("all stopped (PID files removed)")


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--phases", default="setup,control",
        help="setup,control,calibrate,takeover,prep,deadlock,displace,restart,teardown",
    )
    parser.add_argument("--halt-mins", type=int, default=5)
    parser.add_argument("--state-dir", default=None,
                        help="inherited big-state data dir for takeover")
    parser.add_argument("--pressure-target", type=int, default=160)
    args = parser.parse_args()

    RUN.mkdir(parents=True, exist_ok=True)
    LOGS.mkdir(parents=True, exist_ok=True)
    topo = {"pressure_target": args.pressure_target}
    try:
        for phase in args.phases.split(","):
            phase = phase.strip()
            if phase == "setup":
                phase_setup(topo)
            elif phase == "control":
                phase_control(topo)
            elif phase == "calibrate":
                phase_calibrate(topo)
            elif phase == "takeover":
                phase_takeover(topo, args.state_dir)
            elif phase == "prep":
                phase_prep(topo)
            elif phase == "deadlock":
                frozen = phase_deadlock(topo, args.halt_minutes)
                if not frozen:
                    log("WARNING: height did not freeze — inspect logs")
            elif phase == "displace":
                phase_displace(topo)
            elif phase == "restart":
                phase_restart(topo)
            elif phase == "teardown":
                phase_teardown(topo)
            else:
                raise ChaosError(f"unknown phase {phase}")
    finally:
        state = {k: type(v).__name__ for k, v in topo.items()
                 if not isinstance(v, int)}
        (RUN / "topology.json").write_text(json.dumps(state, indent=2))
    log("done")


if __name__ == "__main__":
    main()
