#!/usr/bin/env python3
"""halt-reorg state-build driver (hbox).

Drives the m24 production-geometry devnet through:
  devnet   P (extminer + worker) / V / A on loopback ports 21040-21045
  measure  fund the spamtool, submit max-rate fat intents, measure
           slots/block + prove time + block time, extrapolate the build
           duration (STOP marker if > 10 days)
  build    the multi-day state build to 12,582,912 active slots:
           - 50% crossing: freeze, stop V, write STALEFEE_READY, wait for
             STALEFEE_DONE, compact-restart V, resume
           - 75% crossing: keep spamming, compute the projected expansion
             block B*, stop A inside [B*-15, B*-1], mine to T = B*+3, stop
             the worker, write EXPANSION_READY
  attack   (later, manual) relaunch A isolated + worker, out-work P above
           the fork point, then reseed A to V and capture the panic

Every process gets a PID file under run/. Stop by PID only — never by
pattern. Loopback only.
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

LANE = Path(__file__).resolve().parents[1]
BIN = LANE / "bin"
RUN = LANE / "run"
LOGS = LANE / "logs"
SPAMTOOL = LANE / "target" / "release" / "spamtool"

P_P2P, P_RPC = 21040, 21041
V_P2P, V_RPC = 21042, 21043
A_P2P, A_RPC = 21044, 21045

MINING_KEY_P = "halt-reorg-mining-p-6e0c5a1b"
OPERATOR_KEY_P = "halt-reorg-operator-p-9d4f2b7c"
MINING_KEY_A = "halt-reorg-mining-a-3b8d17e4"
OPERATOR_KEY_A = "halt-reorg-operator-a-5c2f9a6d"

FUNDING_SECRET = "33" * 32   # spamtool funding address (change recycles here)
PARKING_SECRET = "44" * 32   # zero-value dust outputs land here, never spent

SLOT_CAP = 1 << 24
HALF = SLOT_CAP // 2                    # 8,388,608
THREE_QUARTERS = (SLOT_CAP * 3) // 4     # 12,582,912
FINALITY = 18
WINDOW_SPAN = 35
EXPAND_MAJORITY = 10
STOP_DAYS = 10.0

MEMPOOL_WATERMARK = 40      # keep ~this many proven intents in the pool
BATCH_COUNT = 20           # intents per spamtool batch (bounded by pool depth)
BATCH_PARALLEL = 4          # proving threads
TARGET_POOL_DEPTH = 8      # funding UTXOs to maintain (one walletSend each)
TOPUP_NOID = 150            # walletSend chunk to the spamtool (NOID)
TOPUP_TRIGGER_NOID = 60     # pool value below which we top up (NOID)
PROGRESS_EVERY = 20         # blocks between STATE_PROGRESS.md rewrites
STALEFEE_TIMEOUT_S = 24 * 3600


class ChaosError(Exception):
    pass


def log(msg):
    print(f"[{time.strftime('%F %T')}] {msg}", flush=True)


def rpc(url, method, params=None, timeout=60, raw_error=False):
    headers = {"content-type": "application/json"}
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": f"paranoid_{method}", "params": params or []}
    ).encode()
    req = urllib.request.Request(url, data=body, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            payload = json.loads(resp.read().decode())
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode(errors="replace")[:400]
        if raw_error:
            return {"http_error": exc.code, "detail": detail}
        raise ChaosError(f"RPC {method} HTTP {exc.code}: {detail}")
    except urllib.error.URLError as exc:
        if raw_error:
            return {"transport_error": str(exc.reason)}
        raise
    if "error" in payload:
        if raw_error:
            return {"rpc_error": payload["error"]}
        raise ChaosError(f"RPC {method} error: {payload['error']}")
    return payload.get("result")


def wait_until(desc, predicate, timeout=120, interval=1.0, quiet=False):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            value = predicate()
            if value:
                if not quiet:
                    log(f"[ok] {desc}: {value}")
                return value
            last = value
        except Exception as exc:  # noqa: BLE001
            last = exc
        time.sleep(interval)
    raise ChaosError(f"timeout waiting for {desc}; last={last}")


class Managed:
    """Detached process with a PID file. Stop = RPC (optional) + PID only."""

    def __init__(self, name, args, log_name=None, env=None):
        self.name = name
        self.args = [str(a) for a in args]
        self.pid_file = RUN / f"{name}.pid"
        self.log_path = LOGS / (log_name or f"{name}.log")
        self.env = env
        self.proc = None
        self.log_file = None

    def start(self):
        RUN.mkdir(parents=True, exist_ok=True)
        LOGS.mkdir(parents=True, exist_ok=True)
        if self.pid_file.exists():
            existing = int(self.pid_file.read_text().strip())
            try:
                os.kill(existing, 0)
                raise ChaosError(
                    f"{self.name}: PID {existing} from {self.pid_file} is still alive"
                )
            except ProcessLookupError:
                self.pid_file.unlink()
        self.log_file = open(self.log_path, "ab", buffering=0)
        self.log_file.write(
            f"\n\n===== START {self.name} {time.strftime('%F %T')} =====\n".encode()
        )
        full_env = dict(os.environ)
        if self.env:
            full_env.update(self.env)
        self.proc = subprocess.Popen(
            self.args, stdout=self.log_file, stderr=subprocess.STDOUT, env=full_env,
            start_new_session=True,
        )
        self.pid_file.write_text(str(self.proc.pid))
        log(f"[start] {self.name}: pid={self.proc.pid} log={self.log_path.name}")
        return self

    def pid(self):
        if self.proc and self.proc.poll() is None:
            return self.proc.pid
        return None

    def alive(self):
        return self.pid() is not None

    def stop(self, rpc_url=None, timeout=30):
        pid = self.pid()
        # Resumed processes (proc is None) are stopped via the recorded PID.
        if pid is None and self.pid_file.exists():
            try:
                recorded = int(self.pid_file.read_text().strip())
                os.kill(recorded, 0)
                pid = recorded
            except (ProcessLookupError, ValueError, OSError):
                pid = None
        if pid is None:
            self._cleanup()
            return False
        log(f"[stop] {self.name} pid={pid}")
        if rpc_url:
            try:
                rpc(rpc_url, "stop", timeout=5, raw_error=True)
            except Exception:  # noqa: BLE001
                pass
            if self.proc is not None:
                try:
                    self.proc.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    pass
        def _still_alive():
            try:
                os.kill(pid, 0)
                return True
            except ProcessLookupError:
                return False
        if _still_alive():
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            deadline = time.time() + 10
            while _still_alive() and time.time() < deadline:
                time.sleep(0.5)
            if _still_alive():
                os.kill(pid, signal.SIGKILL)
        if self.proc is not None:
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                pass
        self._cleanup()
        return True

    def _cleanup(self):
        if self.log_file:
            self.log_file.close()
            self.log_file = None
        if self.pid_file.exists():
            self.pid_file.unlink()


class Node:
    def __init__(self, name, p2p_port, rpc_port, mode="node", genesis=False,
                 seeds=(), mining_key=None, operator_key=None):
        self.name = name
        self.p2p_port = p2p_port
        self.rpc_port = rpc_port
        self.mode = mode
        self.genesis = genesis
        self.seeds = list(seeds)
        self.mining_key = mining_key
        self.operator_key = operator_key
        self.data_dir = RUN / name
        self.managed = None

    @property
    def rpc_url(self):
        return f"http://127.0.0.1:{self.rpc_port}"

    @property
    def p2p_addr(self):
        return f"127.0.0.1:{self.p2p_port}"

    def args(self):
        args = [
            BIN / "parano1d",
            "--mode", self.mode,
            "--data-dir", str(self.data_dir),
            "--p2p-listen", f"127.0.0.1:{self.p2p_port}",
            "--rpc-listen", f"127.0.0.1:{self.rpc_port}",
            "--disable-dns-seeds",
            "--log", "info",
        ]
        if self.genesis:
            args.append("--genesis")
        for seed in self.seeds:
            args.extend(["--seed", seed])
        if self.mining_key:
            args.extend(["--mining-key", self.mining_key])
        if self.operator_key:
            args.extend(["--operator-key", self.operator_key])
        return args

    def start(self, extra_env=None):
        self.data_dir.mkdir(parents=True, exist_ok=True)
        env = {"RUST_BACKTRACE": "1"}
        if extra_env:
            env.update(extra_env)
        self.managed = Managed(self.name, self.args(), env=env).start()
        wait_until(
            f"{self.name} RPC ready",
            lambda: rpc(self.rpc_url, "getChainInfo", raw_error=True)
            and isinstance(rpc(self.rpc_url, "getChainInfo", raw_error=True), dict)
            and rpc(self.rpc_url, "getChainInfo", raw_error=True).get("height") is not None,
            timeout=300, interval=2, quiet=True,
        )
        return self

    def stop(self):
        if self.managed:
            self.managed.stop(rpc_url=self.rpc_url)

    def info(self):
        return rpc(self.rpc_url, "getChainInfo")

    def height(self):
        return int(self.info()["height"])

    def header(self, height):
        return rpc(self.rpc_url, "getBlockHeader", [height])

    def tip(self):
        info = self.info()
        header = rpc(self.rpc_url, "getBlockHeader", [int(info["height"])])
        return {
            "height": int(info["height"]),
            "hash": info["best_hash"],
            "active_slot_count": int(header["active_slot_count"]),
            "log_slots": int(header["log_slots"]),
            "timestamp": int(header["timestamp"]),
            "difficulty_target": header["difficulty_target"],
        }

    def work_above(self, fork_height):
        """Cumulative PoW work of blocks (fork_height, tip] (LE u256 int),
        replicating consensus block_work = 2^256 / target."""
        total = 0
        for height in range(fork_height + 1, self.height() + 1):
            target_hex = self.header(height)["difficulty_target"]
            target = int.from_bytes(bytes.fromhex(target_hex), "little")
            if target <= 0:
                continue
            work = (1 << 256) // target
            total += min(work, (1 << 256) - 1)
        return total


def node_p():
    return Node(
        "p-producer", P_P2P, P_RPC, mode="extminer", genesis=True,
        mining_key=MINING_KEY_P, operator_key=OPERATOR_KEY_P,
    )


def node_v():
    return Node("v-victim", V_P2P, V_RPC, seeds=[f"127.0.0.1:{P_P2P}"])


def node_a():
    return Node("a-attacker", A_P2P, A_RPC, seeds=[f"127.0.0.1:{P_P2P}"])


def worker_p():
    return Managed(
        "worker-p",
        [BIN / "parano1d-miner", "--rpc", f"http://127.0.0.1:{P_RPC}",
         "--threads", "6", "--key", MINING_KEY_P],
        env={"RUST_LOG": "info"},
    )


def worker_a():
    return Managed(
        "worker-a",
        [BIN / "parano1d-miner", "--rpc", f"http://127.0.0.1:{A_RPC}",
         "--threads", "6", "--key", MINING_KEY_A],
        env={"RUST_LOG": "info"},
    )


# ---------------------------------------------------------------------------
# spamtool helpers
# ---------------------------------------------------------------------------

def spamtool_address():
    out = subprocess.run(
        [SPAMTOOL, "address", "--secret-hex", FUNDING_SECRET,
         "--parking-hex", PARKING_SECRET],
        capture_output=True, text=True, timeout=120, check=True,
    ).stdout
    funding = parking = None
    for line in out.splitlines():
        if line.startswith("funding="):
            funding = line.split("=", 1)[1].strip()
        if line.startswith("parking="):
            parking = line.split("=", 1)[1].strip()
    return funding, parking


def spamtool_batch(rpc_url, count, exclude, parallel=BATCH_PARALLEL):
    args = [
        SPAMTOOL, "batch",
        "--rpc", rpc_url,
        "--secret-hex", FUNDING_SECRET,
        "--parking-hex", PARKING_SECRET,
        "--count", str(count),
        "--parallel", str(parallel),
    ]
    if exclude:
        args.extend(["--exclude-slots", ",".join(str(s) for s in sorted(exclude))])
    started = time.time()
    proc = subprocess.run(args, capture_output=True, text=True, timeout=1800)
    elapsed = time.time() - started
    lines = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            lines.append(json.loads(line))
        except json.JSONDecodeError:
            log(f"spamtool non-JSON: {line[:200]}")
    if proc.returncode != 0:
        log(f"spamtool batch failed rc={proc.returncode}: {proc.stderr[-800:]}")
    return lines, elapsed, proc.returncode


def funding_pool_value(rpc_url, address):
    slots = rpc(rpc_url, "getSlotsByOwner", [address])
    total = 0
    count = 0
    for slot in slots or []:
        if slot.get("empty") is False:
            total += int(slot.get("value", 0))
            count += 1
    return count, total


# ---------------------------------------------------------------------------
# expansion window math (mirrors finalized_expansion_window)
# ---------------------------------------------------------------------------

_HEADER_COUNT_CACHE = {}


def header_count(node, height):
    if height not in _HEADER_COUNT_CACHE:
        _HEADER_COUNT_CACHE[height] = int(node.header(height)["active_slot_count"])
    return _HEADER_COUNT_CACHE[height]


def window_counts(node, parent_height, assume_ge=None):
    """Occupancy counts of the finalized window [P-35, P-18].

    Headers above the node's tip are assumed >= assume_ge (the projection
    used once the count is pinned at/above 75%)."""
    start = parent_height - WINDOW_SPAN
    end = parent_height - FINALITY
    if start < 1:
        return []
    tip_h = node.height()
    counts = []
    for hh in range(start, end + 1):
        if hh <= tip_h:
            counts.append(header_count(node, hh))
        elif assume_ge is not None:
            counts.append(assume_ge)
        else:
            return []
    return counts


def window_majority(counts):
    return sum(1 for c in counts if c >= THREE_QUARTERS)


def projected_expansion_parent(node, assume_ge):
    """First parent height P >= tip-2 whose window reaches 10-of-18 (future
    headers assumed at assume_ge). None if not yet projectable."""
    tip_h = node.height()
    for p in range(max(tip_h - 2, 1), tip_h + 60):
        counts = window_counts(node, p, assume_ge=assume_ge)
        if len(counts) == FINALITY and window_majority(counts) >= EXPAND_MAJORITY:
            return p
    return None


# ---------------------------------------------------------------------------
# progress + markers
# ---------------------------------------------------------------------------

def write_progress(kind, **fields):
    lines = [f"# {kind} — {time.strftime('%F %T')}", ""]
    for key, value in fields.items():
        lines.append(f"**{key}**: {value}")
    lines.append("")
    lines.append(f"run by PID {Path('/proc/self').readlink() if Path('/proc/self').exists() else '?'}"
                 f" (statebuild.py); stop only via run/*.pid files")
    (RUN / "STATE_PROGRESS.md").write_text("\n".join(lines) + "\n")


def disk_usage():
    total = 0
    if RUN.exists():
        out = subprocess.run(
            ["du", "-sk", str(RUN)], capture_output=True, text=True, timeout=120
        )
        if out.returncode == 0:
            total = int(out.stdout.split()[0])
    df = subprocess.run(
        ["df", "-k", "/othersys"], capture_output=True, text=True, timeout=60
    )
    free_kb = None
    if df.returncode == 0:
        try:
            free_kb = int(df.stdout.splitlines()[1].split()[3])
        except (IndexError, ValueError):
            pass
    return total, free_kb


# ---------------------------------------------------------------------------
# phases
# ---------------------------------------------------------------------------

def phase_devnet(topo):
    log("DEVNET: P (extminer, genesis) + worker + V + A")
    p = node_p().start()
    topo["p"] = p
    topo["worker-p"] = worker_p().start()
    v = node_v().start()
    topo["v"] = v
    a = node_a().start()
    topo["a"] = a
    wait_until("V2.1 window (h>=25)", lambda: p.height() >= 25 or None,
               timeout=7200, interval=10)
    log(f"P h={p.height()} V h={v.height()} A h={a.height()}")
    for node in (v, a):
        wait_until(f"{node.name} follows P", lambda n=node: n.height() >= 25 or None,
                   timeout=600, interval=5)
    tip = p.tip()
    log(f"tip: h={tip['height']} log_slots={tip['log_slots']} "
        f"count={tip['active_slot_count']}")
    if tip["log_slots"] != 24:
        raise ChaosError(f"expected m24, got log_slots={tip['log_slots']}")


def phase_measure(topo):
    log("MEASURE: funding, max-rate intents, extrapolation")
    p = topo["p"]
    funding, parking = spamtool_address()
    log(f"spamtool funding={funding}")
    log(f"spamtool parking={parking}")

    # 1. fund the spamtool from P's wallet (mining income). Two sends give
    #    the pool depth 2 (one UTXO per send) for the 2-intents/block rate.
    wait_until("wallet has >= 90 NOID",
               lambda: int(rpc(p.rpc_url, "walletGetBalance")["balance_micronoid"] or 0)
               >= 90_000_000 or None,
               timeout=600, interval=10)
    for i in range(2):
        result = rpc(p.rpc_url, "walletSend", [funding, 40_000_000, 0])
        log(f"walletSend 40 NOID #{i} -> {funding}: {json.dumps(result)[:200]}")
        time.sleep(2)
    wait_until("topups confirmed",
               lambda: funding_pool_value(p.rpc_url, funding)[0] >= 2 or None,
               timeout=300, interval=5)
    count, value = funding_pool_value(p.rpc_url, funding)
    log(f"funding pool: {count} UTXOs, {value} uNOID")

    # 2. one small intent (4 outputs) to smoke the full path.
    proc = subprocess.run(
        [SPAMTOOL, "once", "--rpc", p.rpc_url, "--secret-hex", FUNDING_SECRET,
         "--parking-hex", PARKING_SECRET, "--outputs", "4", "--submit"],
        capture_output=True, text=True, timeout=1800,
    )
    log(f"smoke intent rc={proc.returncode}: {proc.stdout[-600:]} {proc.stderr[-300:]}")
    if proc.returncode != 0:
        raise ChaosError("smoke intent failed")

    # 3. max rate: two 255-output intents per block (bounded by pool depth).
    base = p.tip()
    measurements = {"prove_ms": [], "slots_per_block": [], "block_s": [], "ok": 0}
    spam_t0 = time.time()
    for round_index in range(6):
        n_utxos, _ = funding_pool_value(p.rpc_url, funding)
        lines, elapsed, rc = spamtool_batch(p.rpc_url, count=min(2, n_utxos),
                                             exclude=set())
        for entry in lines:
            if "prove_ms" in entry:
                measurements["prove_ms"].append(entry["prove_ms"])
                if entry.get("submit") == "ok":
                    measurements["ok"] += 1
        log(f"measure round {round_index}: {elapsed:.1f}s, "
            f"lines={len(lines)} ok={measurements['ok']}")
        # wait for these to land
        deadline = time.time() + 240
        while time.time() < deadline:
            tip = p.tip()
            if tip["height"] >= base["height"] + (round_index + 1) * 2:
                break
            time.sleep(5)
    # settle: measure slots/block and block cadence over the span
    time.sleep(90)
    tip = p.tip()
    blocks = tip["height"] - base["height"]
    slots_gained = tip["active_slot_count"] - base["active_slot_count"]
    wall = time.time() - spam_t0
    measurements["slots_per_block"] = slots_gained / max(blocks, 1)
    measurements["block_s"] = wall / max(blocks, 1)
    mean_prove = (
        sum(measurements["prove_ms"]) / len(measurements["prove_ms"])
        if measurements["prove_ms"] else None
    )

    remaining = THREE_QUARTERS - tip["active_slot_count"]
    spb = measurements["slots_per_block"]
    blocks_to_go = remaining / spb if spb else float("inf")
    days_measured = blocks_to_go * measurements["block_s"] / 86400
    days_target = blocks_to_go * 30 / 86400

    report = {
        "height": tip["height"],
        "active_slot_count": tip["active_slot_count"],
        "measured": {
            "slots_per_block": spb,
            "block_s": measurements["block_s"],
            "prove_ms_mean": mean_prove,
            "intents_submitted_ok": measurements["ok"],
            "blocks_observed": blocks,
        },
        "extrapolation": {
            "blocks_to_75": blocks_to_go,
            "days_at_measured_block_s": days_measured,
            "days_at_30s_target": days_target,
        },
    }
    (LANE / "MEASURE.md").write_text(
        "# halt-reorg state-build extrapolation\n\n```json\n"
        + json.dumps(report, indent=2) + "\n```\n"
    )
    log(f"MEASURE: {json.dumps(report['extrapolation'])}")
    if days_target > STOP_DAYS and days_measured > STOP_DAYS:
        (RUN / "STOP_EXTRAPOLATION.md").write_text(
            f"# STOP — extrapolated state build exceeds {STOP_DAYS} days\n\n"
            f"{json.dumps(report, indent=2)}\n"
        )
        log("STOP: extrapolation above 10 days — root decides")
        return False
    return True


def freeze_for_stalefee(topo):
    """50% checkpoint: freeze the chain, stop V, marker, wait, resume."""
    p, v = topo["p"], topo["v"]
    log("FREEZE 50%: stopping P worker + V for the stalefee checkpoint")
    worker = topo.pop("worker-p", None)
    if worker:
        worker.stop()
    time.sleep(30)
    if p.height() != v.height():
        log(f"V lagging P at freeze: P={p.height()} V={v.height()} (waiting)")
        wait_until("V catches up to frozen P", lambda: v.height() >= p.height() or None,
                   timeout=1800, interval=10)
    tip = p.tip()
    v.stop()
    ready = RUN / "STALEFEE_READY.md"
    ready.write_text(
        "# stalefee checkpoint ready (halt-reorg state build)\n\n"
        f"frozen tip: height={tip['height']} hash={tip['hash']} "
        f"active_slot_count={tip['active_slot_count']} log_slots={tip['log_slots']}\n"
        f"V data dir (copy this): {RUN / 'v-victim'}\n"
        f"P data dir (reference, do not move): {RUN / 'p-producer'}\n"
        f"written {time.strftime('%F %T')}\n\n"
        "Copy V's data dir now. When done, write run/STALEFEE_DONE.md "
        "(one line: who/when) so this driver resumes.\n"
    )
    log(f"STALEFEE_READY written; waiting for STALEFEE_DONE (timeout {STALEFEE_TIMEOUT_S}s)")
    done = RUN / "STALEFEE_DONE.md"
    deadline = time.time() + STALEFEE_TIMEOUT_S
    while not done.exists():
        if time.time() > deadline:
            log("WARNING: STALEFEE_DONE timeout — resuming anyway "
                "(later freezes are still copyable until 75%)")
            break
        time.sleep(60)
    log("stalefee checkpoint released; restarting V (compact resume)")
    topo["v"] = node_v().start()

    def compact_resume_line():
        text = topo["v"].managed.log_path.read_text(errors="replace")
        marker = text.rfind("===== START")
        recent = text[marker:] if marker >= 0 else text
        return ("resumed exact state from compact segment summaries" in recent
                or None)

    wait_until("V compact resume log line", compact_resume_line,
               timeout=600, interval=5)
    topo["worker-p"] = worker_p().start()
    log("FREEZE 50% complete; state build resumes")


def phase_build(topo):
    log("BUILD: driving active_slot_count to 12,582,912 (75%)")
    p = topo["p"]
    funding, _ = spamtool_address()
    pending = {}       # txid -> input_slot (in-flight intents to exclude)
    stalefee_done = False
    a_stopped = False
    b_star_logged = None
    last_progress_height = -1
    last_topup_height = -10
    block_times = []
    last_tip = None
    work_logs = []

    while True:
        tip = p.tip()
        now = time.time()
        if last_tip is not None and tip["height"] > last_tip["height"]:
            block_times.append((now - last_tip["ts"], tip["height"] - 1))
            block_times = block_times[-40:]
        last_tip = {"height": tip["height"], "ts": now}

        # --- progress marker ------------------------------------------------
        if tip["height"] - last_progress_height >= PROGRESS_EVERY:
            last_progress_height = tip["height"]
            used_kb, free_kb = disk_usage()
            mean_block = sum(bt[0] for bt in block_times) / max(len(block_times), 1)
            mean_prove = None
            if work_logs:
                mean_prove = sum(w for w in work_logs[-50:]) / len(work_logs[-50:])
            remaining = max(THREE_QUARTERS - tip["active_slot_count"], 0)
            spb = 508
            eta_days = remaining / spb * (mean_block or 30) / 86400
            write_progress(
                "state build",
                height=tip["height"],
                active_slot_count=f"{tip['active_slot_count']} "
                                  f"({100 * tip['active_slot_count'] / SLOT_CAP:.2f}%)",
                remaining_slots=remaining,
                mean_block_s=f"{mean_block:.1f}",
                eta_days=f"{eta_days:.2f}",
                mempool_size=rpc(p.rpc_url, "getMempoolSize"),
                v_height=topo["v"].height() if "v" in topo else "-",
                a_height=topo["a"].height() if "a" in topo else "-",
                run_dir_mb=f"{used_kb / 1024:.0f}",
                othersys_free_gb=f"{free_kb / 1048576:.1f}" if free_kb else "?",
                mean_prove_ms=f"{mean_prove:.0f}" if mean_prove is not None else "-",
                in_flight_intents=len(pending),
            )

        # --- 50% checkpoint --------------------------------------------------
        if not stalefee_done and tip["active_slot_count"] >= HALF:
            freeze_for_stalefee(topo)
            stalefee_done = True
            p = topo["p"]
            continue

        # --- 75%: prime toward the expansion block ----------------------------
        if tip["active_slot_count"] >= THREE_QUARTERS:
            proj = projected_expansion_parent(p, assume_ge=THREE_QUARTERS)
            if proj is not None:
                b_star = proj + 1
                if b_star != b_star_logged:
                    b_star_logged = b_star
                    log(f"75% reached; projected expansion block B*={b_star} "
                        f"(parent window {proj})")
                if not a_stopped and p.height() >= b_star - 3:
                    stopped = topo.pop("a", None)
                    if stopped:
                        a_tip_height = stopped.height()  # before the stop
                        stopped.stop()
                        log(f"A stopped at h={a_tip_height} (< B*={b_star})")
                    a_stopped = True
                if p.height() >= b_star + 3:
                    # T = B*+3 reached: freeze the chain for the attack.
                    worker = topo.pop("worker-p", None)
                    if worker:
                        worker.stop()
                    t_tip = p.tip()
                    fork_parent = b_star - 1
                    p_work = p.work_above(fork_parent)
                    (RUN / "EXPANSION_READY.md").write_text(
                        "# expansion reached — attack phase ready\n\n"
                        f"B*={b_star} T={t_tip['height']} "
                        f"T_hash={t_tip['hash']} count={t_tip['active_slot_count']} "
                        f"log_slots={t_tip['log_slots']}\n"
                        f"fork_parent_height={fork_parent}\n"
                        f"P cumulative work above fork parent: {p_work}\n"
                        f"written {time.strftime('%F %T')}\n"
                        "Run `statebuild.py --phase attack --resume` next.\n"
                    )
                    log(f"EXPANSION_READY: B*={b_star} T={t_tip['height']} "
                        f"P_work_above_fork={p_work}")
                    return

        # --- keep the mempool topped -----------------------------------------
        if "worker-p" not in topo:
            time.sleep(10)
            continue
        pool_size = rpc(p.rpc_url, "getMempoolSize")
        if pool_size is None:
            time.sleep(10)
            continue
        # drop pending txids that left the pool (mined or evicted)
        for txid in list(pending):
            entry = rpc(p.rpc_url, "getMempoolEntry", [txid], raw_error=True)
            if not isinstance(entry, dict) or entry.get("rpc_error") or entry.get("http_error"):
                del pending[txid]
        if pool_size < MEMPOOL_WATERMARK:
            exclude = set(pending.values())
            n_utxos, value = funding_pool_value(p.rpc_url, funding)
            if (n_utxos < TARGET_POOL_DEPTH
                    or value < TOPUP_TRIGGER_NOID * 1_000_000) \
                    and tip["height"] - last_topup_height > 3:
                balance = int(rpc(p.rpc_url, "walletGetBalance")["balance_micronoid"] or 0)
                send_amount = min(TOPUP_NOID * 1_000_000, max(balance - 5_000_000, 0))
                if send_amount > 1_000_000:
                    rpc(p.rpc_url, "walletSend", [funding, send_amount, 0])
                    last_topup_height = tip["height"]
                    log(f"topped up spamtool wallet with "
                        f"{send_amount / 1_000_000:.1f} NOID "
                        f"(pool {n_utxos}/{TARGET_POOL_DEPTH})")
                else:
                    log(f"wallet balance low ({balance} uNOID); waiting for coinbase")
                    time.sleep(30)
                    continue
            count = min(BATCH_COUNT, n_utxos - len(exclude))
            if count <= 0:
                time.sleep(15)
                continue
            lines, elapsed, rc = spamtool_batch(p.rpc_url, count=count, exclude=exclude)
            for entry in lines:
                if "txid" in entry and entry.get("submit") == "ok":
                    pending[entry["txid"]] = entry["input_slot"]
                    work_logs.append(entry.get("prove_ms", 0))
            if rc != 0:
                time.sleep(10)
        time.sleep(5)


def phase_attack(topo):
    """After EXPANSION_READY: A out-works P above the fork point, reseeds to
    V, and V's reorg triggers the shrink (flush_all_dirty on dirty∩evicted)."""
    log("ATTACK: relaunching A isolated + own worker")
    p, v = topo["p"], topo["v"]
    ready = (RUN / "EXPANSION_READY.md").read_text()
    b_star = int(ready.split("B*=")[1].split()[0])
    fork_parent = int(ready.split("fork_parent_height=")[1].split()[0])
    p_work = int(ready.split("P cumulative work above fork parent: ")[1].split()[0])

    # A isolated: no seeds, extminer mode, its own worker.
    a = Node("a-attacker", A_P2P, A_RPC, mode="extminer",
             mining_key=MINING_KEY_A, operator_key=OPERATOR_KEY_A)
    a.start()
    topo["a"] = a
    topo["worker-a"] = worker_a().start()

    log(f"fork parent {fork_parent}, B*={b_star}; A mines until "
        f"work > P's {p_work}")
    def a_wins():
        a_tip = a.tip()
        if a_tip["height"] <= fork_parent:
            return None
        if a.work_above(fork_parent) > p_work:
            return a_tip["height"]
        return None
    wait_until("A cumulative work exceeds P above fork", a_wins,
               timeout=12 * 3600, interval=30)
    a_work = a.work_above(fork_parent)
    log(f"A out-worked P above fork: {a_work} > {p_work}")
    worker = topo.pop("worker-a", None)
    if worker:
        worker.stop()
    a.stop()
    log("relaunching A seeded to V (127.0.0.1:%d)" % V_P2P)
    a_seeded = Node("a-attacker", A_P2P, A_RPC, seeds=[f"127.0.0.1:{V_P2P}"])
    a_seeded.start()
    topo["a"] = a_seeded
    log("ATTACK live: watch V for the shrink panic (RUST_BACKTRACE=1 preset)")
    (RUN / "ATTACK_LIVE.md").write_text(
        f"attack started {time.strftime('%F %T')}; A reseeded to V\n"
        f"A work above fork {a_work} vs P {p_work}\n"
    )

    # --- capture: the reorg revert of B* -> shrink 25->24 -> flush_all_dirty --
    log("CAPTURE: watching V for 5 minutes (panic / wedge / recovery)")
    time.sleep(300)
    capture = {"written": time.strftime("%F %T")}
    v_log = (LOGS / "v-victim.log").read_text(errors="replace")
    v_recent = v_log[v_log.rfind("===== START"):] if "===== START" in v_log else v_log
    capture["v_panic_seen"] = "panicked at" in v_recent or "panicked" in v_recent[-200_000:]
    capture["v_swallowed_panic"] = "exact suffix worker panicked" in v_recent
    capture["v_reorg_lines"] = [line for line in v_recent.splitlines()
                                if "reorg" in line.lower()][-10:]
    capture["v_rpc_alive"] = isinstance(
        rpc(v.rpc_url, "getChainInfo", raw_error=True), dict)
    capture["v_height"] = v.height() if capture["v_rpc_alive"] else None
    capture["v_tip"] = v.tip() if capture["v_rpc_alive"] else None
    capture["p_rpc_alive"] = isinstance(
        rpc(p.rpc_url, "getChainInfo", raw_error=True), dict)
    capture["p_height"] = p.height() if capture["p_rpc_alive"] else None
    capture["a_rpc_alive"] = isinstance(
        rpc(a_seeded.rpc_url, "getChainInfo", raw_error=True), dict)
    capture["a_height"] = a_seeded.height() if capture["a_rpc_alive"] else None
    # the process liveness check: PID files, by PID only
    for name in ("v-victim", "p-producer", "a-attacker"):
        pid_file = RUN / f"{name}.pid"
        alive = False
        if pid_file.exists():
            try:
                os.kill(int(pid_file.read_text().strip()), 0)
                alive = True
            except (ProcessLookupError, ValueError, OSError):
                alive = False
        capture[f"{name}_process_alive"] = alive
    if capture["v_panic_seen"]:
        tail = v_recent[-8000:]
        capture["v_panic_excerpt"] = tail
    (RUN / "PANIC_CAPTURE.md").write_text(
        "# panic capture (halt-reorg SR-1)\n\n```json\n"
        + json.dumps(capture, indent=2, default=str) + "\n```\n"
    )
    log(f"PANIC_CAPTURE written: v_panic_seen={capture['v_panic_seen']} "
        f"v_swallowed={capture['v_swallowed_panic']} "
        f"v_alive={capture['v-victim_process_alive']}")


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

PHASES = {
    "devnet": phase_devnet,
    "measure": phase_measure,
    "build": phase_build,
    "attack": phase_attack,
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--phases", required=True,
                        help="comma-separated: devnet,measure,build,attack")
    parser.add_argument("--resume", action="store_true",
                        help="rebuild topo from running PID files (nodes only)")
    args = parser.parse_args()

    RUN.mkdir(parents=True, exist_ok=True)
    LOGS.mkdir(parents=True, exist_ok=True)
    topo = {}
    if args.resume:
        for name, builder in (("p", node_p), ("v", node_v), ("a", node_a)):
            pid_file = RUN / f"{'p-producer' if name == 'p' else 'v-victim' if name == 'v' else 'a-attacker'}.pid"
            if pid_file.exists():
                node = builder()
                node.managed = Managed(
                    node.name, [], log_name=f"{node.name}.log")
                node.managed.pid_file = pid_file
                node.managed.proc = None
                topo[name] = node
                log(f"resumed {node.name} from {pid_file}")
        if (RUN / "worker-p.pid").exists():
            w = worker_p()
            w.pid_file = RUN / "worker-p.pid"
            w.proc = None
            topo["worker-p"] = w
            log("resumed worker-p from pid file")
    for phase in args.phases.split(","):
        phase = phase.strip()
        if not phase:
            continue
        log(f"===== PHASE {phase} =====")
        PHASES[phase](topo)
    log("all requested phases complete")


if __name__ == "__main__":
    main()
