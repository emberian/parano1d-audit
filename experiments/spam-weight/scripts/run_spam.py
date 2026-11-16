#!/usr/bin/env python3
"""spam-weight: sustained full-rate collection-creation spam on a live
early-finance devnet (V2@10, V2.1@24), real per-block measurements.

Phases (idempotent; run subsets with --phases):
  start    n1 (genesis, extminer, --v2-large-blocks) + W1 worker + n2 (seeded)
           + J-held (no seeds, stays at genesis); wait for v2.1 height
  fund     3 x 200 NOID UTXOs to the spammer wallet from n1's daemon wallet
  smoke    one collection + one small mint, verified on-chain (first live
           admission of CreateCollection/MintNft through submitTxIntent)
  control  --control-blocks baseline blocks, no spam submitted
  spam     the main loop: on every tip change, confirm collections, build and
           submit two max-batch MintNft intents (unlock_height = u64::MAX)
           into the two ping-pong collections; per-block CSV rows
  post     --post-blocks recovery blocks, no spam
  joiner   cold-sync the held-at-genesis node against the spam-laden chain,
           timed; inventory-query timings on the spammer wallet
  status   height table for all live nodes
  stop     stop everything by PID

Ports 21140-21159, all 127.0.0.1, data dirs under run/. Every process gets a
PID file; stopped only by PID (paranoid_stop then signals). Never kill by
pattern; never git stash; commit named files only.
"""

import argparse
import csv
import json
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    ChaosError,
    FACTORY_BIN,
    LANE,
    MINING_KEY_N1,
    Node,
    OPERATOR_KEY_N1,
    rpc,
    start_worker,
    stop_all,
    wait_until,
)

TAG = "[spam]"
RUN = LANE / "run"
CSV_PATH = RUN / "blocks.csv"
STATE_PATH = RUN / "factory-state.json"
LAST_GOOD_PATH = RUN / "collection-last-good.json"

WORKER_NAME = "w1-worker"
COLLECTIONS = ["spamA", "spamB"]
# Per-intent batch: 248 is the intent page cap (128 pages); two of those
# need 256 block page positions > 255, so the spam rounds default to 246
# per mint (2 x 127 pages = 254 <= 255, 2 x 248 outputs = 496 <= 510).
# Override with SPAM_MINT_COUNT after smoke if the measured layout differs.
MINT_COUNT = int(__import__("os").environ.get("SPAM_MINT_COUNT", "246"))
MINT_METADATA = "51000000000000000000000000000000000000000000000000000000c7"
COLLECTION_METADATA = "51000000000000000000000000000000000000000000000000000000a5"
SMOKE_METADATA = "51000000000000000000000000000000000000000000000000000000b6"

CSV_FIELDS = [
    "height", "phase", "wall_time", "block_secs", "user_pages", "logical_txs",
    "live_inputs", "live_outputs", "total_fees", "reward", "block_bytes",
    "history_step_bytes", "state_bytes", "active_slots", "n1_dir_kb", "n2_dir_kb",
    "difficulty_bits", "build_secs", "submit", "mempool_before", "notes",
]


def factory(rpc_url, *args, timeout=1800, check=False):
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


def parse_mint(out):
    """Parse a mint invocation result -> (line, fields dict)."""
    for line in out.splitlines():
        if line.startswith("mint ADMITTED") or line.startswith("mint REJECTED"):
            fields = dict(re.findall(r"(\w+)=([^ ]+)", line))
            return line, fields
    lines = out.strip().splitlines()
    return (lines[-1] if lines else "no output"), {}


def wallet_utxos(rpc_url, wallet="spammer"):
    out = factory(rpc_url, "utxos", wallet)
    utxos = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 3:
            utxos.append(
                {"slot": int(parts[0]), "amount": int(parts[1]), "creation_id": int(parts[2])}
            )
    return utxos


def mempool_size(rpc_url):
    try:
        return int(rpc(rpc_url, "getMempoolInfo").get("size") or 0)
    except (ChaosError, TypeError):
        return -1


def du_kb(path):
    proc = subprocess.run(
        ["du", "-sk", str(path)], text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )
    try:
        return int(proc.stdout.split()[0])
    except (IndexError, ValueError):
        return -1


def load_json(path, default):
    if path.exists():
        try:
            return json.loads(path.read_text())
        except json.JSONDecodeError:
            return default
    return default


def collection_state():
    state = load_json(STATE_PATH, None)
    if state is None:
        return None
    return state


def save_json(path, obj):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2))


def block_row(n1, n2, height, phase, wall_time, block_secs, round_info, notes):
    """Collect one block's measurements into a CSV row."""
    details = rpc(n1.rpc_url, "getBlockDetails", [height]) or {}
    retained = details.get("retained") or {}
    header = details.get("header") or {}
    try:
        state_info = rpc(n1.rpc_url, "getStateInfo")
    except ChaosError:
        state_info = {}
    try:
        bits = rpc(n1.rpc_url, "getMiningInfo").get("difficulty_bits")
    except ChaosError:
        bits = ""
    return {
        "height": height,
        "phase": phase,
        "wall_time": round(wall_time, 1),
        "block_secs": round(block_secs, 1) if block_secs else "",
        "user_pages": retained.get("user_pages", ""),
        "logical_txs": retained.get("logical_transactions", ""),
        "live_inputs": retained.get("live_inputs", ""),
        "live_outputs": retained.get("live_outputs", ""),
        "total_fees": retained.get("total_fees_micronoid", ""),
        "reward": retained.get("reward_micronoid", ""),
        "block_bytes": retained.get("block_bytes", ""),
        "history_step_bytes": retained.get("history_step_bytes", ""),
        "state_bytes": state_info.get("state_bytes", ""),
        "active_slots": state_info.get("active_slots", ""),
        "n1_dir_kb": du_kb(RUN / "n1"),
        "n2_dir_kb": du_kb(RUN / "n2"),
        "difficulty_bits": bits,
        "build_secs": round_info.get("build_secs", "") if round_info else "",
        "submit": round_info.get("submit", "") if round_info else "",
        "mempool_before": round_info.get("mempool_before", "") if round_info else "",
        "notes": notes + f" ts={header.get('timestamp', '')}",
    }


def csv_append(row):
    CSV_PATH.parent.mkdir(parents=True, exist_ok=True)
    new = not CSV_PATH.exists()
    with open(CSV_PATH, "a", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=CSV_FIELDS)
        if new:
            writer.writeheader()
        writer.writerow(row)


def node_active(node):
    try:
        node.info(check=True)
        return True
    except Exception:  # noqa: BLE001
        return False


def make_n1():
    return Node(
        "n1",
        p2p_port=21140,
        rpc_port=21141,
        mode="extminer",
        genesis=True,
        mining_key=MINING_KEY_N1,
        operator_key=OPERATOR_KEY_N1,
        v2_large_blocks=True,
    )


def make_n2():
    return Node("n2", p2p_port=21142, rpc_port=21143, seeds=["127.0.0.1:21140"])


def make_jheld():
    # Held at genesis: no seeds, so it never leaves height 0 until phase joiner.
    return Node("jheld", p2p_port=21144, rpc_port=21145)


def make_jlive():
    return Node("jlive", p2p_port=21146, rpc_port=21147, seeds=["127.0.0.1:21140"])


def cli(node, *args, timeout=300, check=True):
    return node.cli(list(args), timeout=timeout, check=check)


def daemon_balance(n1):
    out, _err, _code = cli(n1, "balance", check=False)
    for line in out.splitlines():
        if "NOID" in line:
            try:
                return float(line.split()[0])
            except (ValueError, IndexError):
                pass
    return 0.0


def wait_height(node, target, timeout=3600):
    wait_until(
        f"{node.name} reaches h{target}",
        lambda: node.height() >= target,
        timeout=timeout,
        interval=5,
    )


# ---------------------------------------------------------------------------
# Phases
# ---------------------------------------------------------------------------


def phase_start(_args):
    n1 = make_n1()
    if not node_active(n1):
        n1.start()
    worker_pid = RUN / f"{WORKER_NAME}.pid"
    if not worker_pid.exists():
        start_worker(WORKER_NAME, n1, threads=6, key=MINING_KEY_N1)
    n2 = make_n2()
    if not node_active(n2):
        n2.start()
    jheld = make_jheld()
    if not node_active(jheld):
        jheld.start()
    wait_height(n1, 26)
    wait_until(
        "n2 tracks n1",
        lambda: n2.height() >= n1.height() - 1,
        timeout=600,
    )
    proto = rpc(n1.rpc_url, "getNativeProtocol")
    print(
        f"{TAG} start: n1 h={n1.height()} n2 h={n2.height()} jheld h={jheld.height()}; "
        f"native protocol pages={proto.get('pages')} headers={proto.get('headers')} "
        f"actions={proto.get('actions')} nft_batch_limit={proto.get('nft_batch_limit')} "
        f"active_at_next={proto.get('active_at_next')}",
        flush=True,
    )


def phase_fund(args):
    n1 = make_n1()
    spammer_addr = factory(n1.rpc_url, "address", "spammer").strip().splitlines()[-1]
    print(f"{TAG} spammer address {spammer_addr}", flush=True)
    wait_until(
        "daemon wallet has >= 700 NOID",
        lambda: daemon_balance(n1) >= 700,
        timeout=2400,
        interval=30,
    )
    sent = 0
    while True:
        utxos = [u for u in wallet_utxos(n1.rpc_url) if u["amount"] >= 150_000_000]
        if len(utxos) >= 3:
            break
        out, _err, _code = cli(n1, "send", spammer_addr, "200", check=False)
        sent += 1
        tail = out.strip().splitlines()[-1][:160] if out.strip() else "no output"
        print(f"{TAG} send {sent} (200 NOID): {tail}", flush=True)
        time.sleep(args.fund_pace)
    print(f"{TAG} spammer UTXOs: {[(u['slot'], u['amount']) for u in wallet_utxos(n1.rpc_url)]}", flush=True)


def phase_smoke(_args):
    n1 = make_n1()
    h0 = n1.height()
    out = factory(
        n1.rpc_url, "collection", "spammer", "spamA", COLLECTION_METADATA, check=True
    )
    print(f"{TAG} smoke collection A: {out.strip().splitlines()[-1]}", flush=True)
    wait_until("collection A mined", lambda: n1.height() > h0, timeout=300, interval=3)
    h1 = n1.height()
    out = factory(
        n1.rpc_url, "collection", "spammer", "spamB", COLLECTION_METADATA, check=True
    )
    print(f"{TAG} smoke collection B: {out.strip().splitlines()[-1]}", flush=True)
    wait_until("collection B mined", lambda: n1.height() > h1, timeout=300, interval=3)
    print(f"{TAG} confirm: {factory(n1.rpc_url, 'confirm')}", flush=True)
    h2 = n1.height()
    out = factory(n1.rpc_url, "mint", "spammer", "spamA", "3", SMOKE_METADATA, check=True)
    print(f"{TAG} smoke mint: {out.strip().splitlines()[-1]}", flush=True)
    wait_until("smoke mint mined", lambda: n1.height() > h2, timeout=300, interval=3)
    print(f"{TAG} confirm: {factory(n1.rpc_url, 'confirm')}", flush=True)
    addr = factory(n1.rpc_url, "address", "spammer").strip().splitlines()[-1]
    page = rpc(n1.rpc_url, "getNativeHoldings", [addr, None, 64])
    holdings = page.get("holdings", [])
    print(
        f"{TAG} smoke holdings: {len(holdings)} entries; first: "
        f"{json.dumps(holdings[0])[:400] if holdings else 'none'}",
        flush=True,
    )
    unlocked = [h for h in holdings if h.get("unlock_height") == 2**64 - 1]
    assert len(unlocked) == 3, f"expected 3 MAX-locked items, got {len(unlocked)}"
    details = rpc(n1.rpc_url, "getBlockDetails", [n1.height()])
    print(f"{TAG} smoke block retained: {json.dumps(details.get('retained'))[:400]}", flush=True)
    mem = rpc(n1.rpc_url, "getMempoolInfo")
    print(f"{TAG} mempool after smoke: size={mem.get('size')} txs={str(mem.get('txs'))[:200]}", flush=True)


def phase_control(args):
    n1 = make_n1()
    n2 = make_n2()
    run_window(n1, n2, blocks=args.control_blocks, phase_name="control", spam=False)


def phase_spam(args):
    n1 = make_n1()
    n2 = make_n2()
    jlive = make_jlive()
    if not node_active(jlive):
        jlive.start()
    sync_start = time.time()
    wait_until(
        "jlive reaches pre-spam tip",
        lambda: jlive.height() >= n1.height() - 1,
        timeout=900,
        interval=5,
    )
    print(
        f"{TAG} jlive pre-spam sync {time.time() - sync_start:.1f}s to h{jlive.height()}",
        flush=True,
    )
    run_window(n1, n2, blocks=args.blocks, phase_name="spam", spam=True,
               max_height=args.max_height)


def phase_post(args):
    n1 = make_n1()
    n2 = make_n2()
    run_window(n1, n2, blocks=args.post_blocks, phase_name="post", spam=False)


def run_window(n1, n2, blocks, phase_name, spam, max_height=None):
    """The measurement loop shared by control/spam/post phases.

    On every tip change: record the landed block(s) (with the previous round's
    build/submit info), then (spam only) run one spam round against the new
    tip. max_height stops the window before that tip height (epoch-anchor
    rotation at h144 evicts in-flight intents).
    """
    start_height = n1.height()
    done = 0
    last_h = start_height
    prev_header_ts = None
    round_info = None
    print(f"{TAG} {phase_name}: window of {blocks} blocks starts at h{start_height}", flush=True)
    while done < blocks and (max_height is None or n1.height() < max_height):
        h = n1.height()
        if h > last_h:
            now = time.time()
            for hh in range(last_h + 1, h + 1):
                try:
                    hdr = rpc(n1.rpc_url, "getBlockHeader", [hh]) or {}
                except ChaosError:
                    hdr = {}
                block_secs = ""
                if prev_header_ts is not None and hdr.get("timestamp"):
                    block_secs = float(hdr["timestamp"]) - prev_header_ts
                if hdr.get("timestamp"):
                    prev_header_ts = float(hdr["timestamp"])
                row = block_row(
                    n1, n2, hh, phase_name, now, block_secs, round_info,
                    notes=f"window_h{start_height}",
                )
                csv_append(row)
                round_info = None
                done += 1
            last_h = h
            if spam:
                round_info = spam_round(n1)
            if done % 10 == 0 or done >= blocks:
                print(
                    f"{TAG} {phase_name}: {done}/{blocks} blocks (tip h{n1.height()})",
                    flush=True,
                )
        time.sleep(0.25)
    print(f"{TAG} {phase_name}: window done at h{n1.height()}", flush=True)


def spam_round(n1):
    """One spam round against the current tip.

    confirm collections -> roll back evicted collection records from
    last-good -> build+submit up to two 248-item mints with distinct funding
    UTXOs. Returns round info for the next CSV row.
    """
    factory(n1.rpc_url, "confirm")
    state = collection_state()
    last_good = load_json(LAST_GOOD_PATH, {})
    if state is None or "collections" not in state:
        return {"build_secs": "", "submit": "no-state", "mempool_before": ""}
    changed = False
    for name in COLLECTIONS:
        record = state["collections"].get(name)
        if record is None:
            continue
        if record.get("creation_id") is None and name in last_good:
            # previous round's mint into this collection was evicted, not mined
            state["collections"][name] = last_good[name]
            changed = True
            print(f"{TAG} rolled back evicted collection {name}", flush=True)
        elif record.get("creation_id") is not None:
            last_good[name] = record
    if changed:
        save_json(STATE_PATH, state)
    save_json(LAST_GOOD_PATH, last_good)

    pending = mempool_size(n1.rpc_url)
    if pending >= 2:
        return {"build_secs": "", "submit": "skip-pending", "mempool_before": pending}

    utxos = sorted(wallet_utxos(n1.rpc_url), key=lambda u: -u["amount"])
    build_secs = []
    submits = []
    for i in range(2 - pending):
        collection = COLLECTIONS[i]
        funding = utxos[i] if i < len(utxos) else None
        if funding is None or funding["amount"] < 800_200:
            submits.append("no-funding")
            continue
        out = factory(
            n1.rpc_url, "mint", "spammer", collection, str(MINT_COUNT),
            MINT_METADATA, str(funding["slot"]),
        )
        line, fields = parse_mint(out)
        build_secs.append(fields.get("build_secs", "?"))
        submits.append("A" if "ADMITTED" in line else "R")
        if "REJECTED" in line:
            print(f"{TAG} mint rejected: {line[:300]}", flush=True)
    return {
        "build_secs": "/".join(str(x) for x in build_secs),
        "submit": "+".join(submits),
        "mempool_before": pending,
    }


def phase_joiner(_args):
    n1 = make_n1()
    jheld = make_jheld()
    tip = n1.height()
    print(f"{TAG} joiner: chain tip h{tip}; restarting held-at-genesis node with seed", flush=True)
    if jheld.managed and jheld.alive():
        jheld.stop()
    else:
        pid_file = RUN / "jheld.pid"
        if pid_file.exists():
            import os
            import signal as sig

            pid = int(pid_file.read_text().strip())
            try:
                os.kill(pid, sig.SIGTERM)
                time.sleep(3)
            except ProcessLookupError:
                pass
            pid_file.unlink()
    jheld_seeded = Node("jheld", p2p_port=21144, rpc_port=21145, seeds=["127.0.0.1:21140"])
    t0 = time.time()
    jheld_seeded.start()
    heights = []
    while jheld_seeded.height() < tip:
        time.sleep(5)
        h = jheld_seeded.height()
        if not heights or h != heights[-1][0]:
            heights.append((h, round(time.time() - t0, 1)))
    sync_secs = time.time() - t0
    print(
        f"{TAG} cold sync: h0 -> h{tip} in {sync_secs:.1f}s; trajectory tail={heights[-12:]}",
        flush=True,
    )
    addr = factory(n1.rpc_url, "address", "spammer").strip().splitlines()[-1]
    t0 = time.time()
    total = 0
    cursor = None
    pages = 0
    while True:
        page = rpc(n1.rpc_url, "getNativeHoldings", [addr, cursor, 64], timeout=120)
        total += len(page.get("holdings", []))
        pages += 1
        cursor = page.get("next")
        if not cursor or pages > 2000:
            break
    holdings_secs = time.time() - t0
    t0 = time.time()
    slots = rpc(n1.rpc_url, "getSlotsByOwner", [addr], timeout=120)
    slots_secs = time.time() - t0
    print(
        f"{TAG} inventory: getNativeHoldings {total} rows in {pages} pages "
        f"({holdings_secs:.2f}s); getSlotsByOwner {len(slots)} rows ({slots_secs:.3f}s)",
        flush=True,
    )
    jheld_seeded.stop()


def phase_status(_args):
    for maker in (make_n1, make_n2, make_jheld, make_jlive):
        node = maker()
        if not node_active(node):
            print(f"{TAG} {node.name}: down", flush=True)
            continue
        try:
            print(f"{TAG} {node.name}: h={node.height()}", flush=True)
        except ChaosError as exc:
            print(f"{TAG} {node.name}: down ({exc})", flush=True)


def phase_stop(_args):
    managed = []
    for maker in (make_jlive, make_jheld, make_n2, make_n1):
        node = maker()
        if node_active(node):
            managed.append(node)
    worker_pid = RUN / f"{WORKER_NAME}.pid"
    if worker_pid.exists():
        import os
        import signal as sig

        pid = int(worker_pid.read_text().strip())
        try:
            os.kill(pid, sig.SIGTERM)
            time.sleep(2)
            try:
                os.kill(pid, 0)
                os.kill(pid, sig.SIGKILL)
            except ProcessLookupError:
                pass
        except ProcessLookupError:
            pass
        worker_pid.unlink()
        print(f"{TAG} worker stopped by pid {pid}", flush=True)
    stop_all(managed)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--phases",
        default="start,fund,smoke,control,spam,post,joiner",
        help="comma list: start,fund,smoke,control,spam,post,joiner,status,stop",
    )
    parser.add_argument("--blocks", type=int, default=100, help="spam blocks")
    parser.add_argument("--max-height", type=int, default=141,
                       help="stop the spam window before this tip (h144 anchor rotation)")
    parser.add_argument("--control-blocks", type=int, default=8)
    parser.add_argument("--post-blocks", type=int, default=3)
    parser.add_argument("--fund-pace", type=float, default=8.0)
    args = parser.parse_args()
    phases = {
        "start": phase_start,
        "fund": phase_fund,
        "smoke": phase_smoke,
        "control": phase_control,
        "spam": phase_spam,
        "post": phase_post,
        "joiner": phase_joiner,
        "status": phase_status,
        "stop": phase_stop,
    }
    for name in args.phases.split(","):
        name = name.strip()
        if name not in phases:
            raise SystemExit(f"unknown phase {name}")
        print(f"{TAG} === phase {name} ===", flush=True)
        phases[name](args)


if __name__ == "__main__":
    main()
