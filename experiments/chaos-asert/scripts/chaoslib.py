#!/usr/bin/env python3
"""Shared driver library for the chaos-asert lane.

Adapted from devnet-chaos/scripts/chaoslib.py. Every process started here gets
a PID file under run/ and is stopped only by that PID (RPC `paranoid_stop`
first, then SIGTERM/SIGKILL to the recorded PID). Loopback addresses only. No
pattern-based process management anywhere. Binaries are used read-only from the
devnet-chaos lane's bin/ directory.
"""

import json
import os
import re
import signal
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

LANE = Path(__file__).resolve().parents[1]
RUN = LANE / "run"
LOGS = LANE / "logs"

# Read-only stock + poison binaries from the devnet-chaos lane.
STOCK_BIN = Path(
    "/Users/ember/dev/parano1d-break/experiments/devnet-chaos/bin/parano1d"
)
POISON_BIN = Path(
    "/Users/ember/dev/parano1d-break/experiments/devnet-chaos/bin/parano1d-poison"
)
CLI_BIN = Path(
    "/Users/ember/dev/parano1d-break/experiments/devnet-chaos/bin/parano1d-cli"
)

GENESIS_TARGET_LE_HEX = "00" * 29 + "40" + "00" * 2  # 2^238, LE bytes
FLOOR_TARGET = 1 << 238
EPOCH_LENGTH = 6
BLOCK_TIME = 20
HALFLIFE = 120
MAX_FUTURE_DRIFT = 120


class ChaosError(Exception):
    pass


def rpc(url, method, params=None, timeout=30, bearer=None, raw_error=False):
    headers = {"content-type": "application/json"}
    if bearer:
        headers["authorization"] = f"Bearer {bearer}"
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
    if not quiet:
        print(f"[wait] {desc} (timeout {timeout}s)", flush=True)
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            value = predicate()
            if value:
                if not quiet:
                    print(f"[ok] {desc}: {value}", flush=True)
                return value
            last = value
        except Exception as exc:  # noqa: BLE001 - wait loop reports last error
            last = exc
        time.sleep(interval)
    raise ChaosError(f"timeout waiting for {desc}; last={last}")


def target_int(hexstr):
    """Parse a 64-char LE difficulty-target hex into an int."""
    return int.from_bytes(bytes.fromhex(hexstr), "little")


def target_log2_ratio(target_int_val):
    """log2 of how many times harder than the genesis floor this target is."""
    import math

    return math.log2(FLOOR_TARGET / target_int_val)


def block_header(url, height):
    return rpc(url, "getBlockHeader", [height], raw_error=True)


def header_row(url, height):
    hdr = block_header(url, height)
    if not isinstance(hdr, dict):
        return None
    return {
        "height": int(hdr.get("height", height)),
        "hash": hdr.get("hash"),
        "timestamp": int(hdr.get("timestamp", 0)),
        "target_hex": hdr.get("difficulty_target"),
        "target_int": target_int(hdr["difficulty_target"]) if hdr.get("difficulty_target") else None,
    }


class Managed:
    """A process with a PID file. Stop = RPC (if node) then signal by PID."""

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
            self.args,
            stdout=self.log_file,
            stderr=subprocess.STDOUT,
            env=full_env,
        )
        self.pid_file.write_text(str(self.proc.pid))
        print(f"[start] {self.name}: pid={self.proc.pid} log={self.log_path.name}", flush=True)
        return self

    def pid(self):
        if self.proc and self.proc.poll() is None:
            return self.proc.pid
        return None

    def alive(self):
        return self.pid() is not None

    def stop(self, rpc_url=None, timeout=15):
        pid = self.pid()
        if pid is None:
            self._cleanup()
            return False
        print(f"[stop] {self.name} pid={pid}", flush=True)
        if rpc_url:
            try:
                rpc(rpc_url, "stop", timeout=3)
            except Exception:  # noqa: BLE001 - best-effort graceful stop
                pass
        try:
            self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.kill(pid, signal.SIGTERM)
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.kill(pid, signal.SIGKILL)
                self.proc.wait(timeout=5)
        self._cleanup()
        return True

    def _cleanup(self):
        if self.log_file:
            self.log_file.close()
            self.log_file = None
        if self.pid_file.exists():
            self.pid_file.unlink()


class Node:
    """A parano1d daemon (stock or poison binary), any mode."""

    def __init__(
        self,
        name,
        p2p_port,
        rpc_port,
        binary=STOCK_BIN,
        mode="node",
        genesis=False,
        seeds=(),
        log="info",
        env=None,
        data_dir=None,
    ):
        self.name = name
        self.p2p_port = p2p_port
        self.rpc_port = rpc_port
        self.binary = binary
        self.mode = mode
        self.genesis = genesis
        self.seeds = list(seeds)
        self.log = log
        self.env = env
        self.data_dir = data_dir or (RUN / name)
        self.managed = None

    @property
    def rpc_url(self):
        return f"http://127.0.0.1:{self.rpc_port}"

    @property
    def p2p_addr(self):
        return f"127.0.0.1:{self.p2p_port}"

    def args(self):
        args = [
            self.binary,
            "--mode", self.mode,
            "--data-dir", str(self.data_dir),
            "--p2p-listen", f"127.0.0.1:{self.p2p_port}",
            "--rpc-listen", f"127.0.0.1:{self.rpc_port}",
            "--disable-dns-seeds",
            "--log", self.log,
        ]
        if self.genesis:
            args.append("--genesis")
        for seed in self.seeds:
            args.extend(["--seed", seed])
        return args

    def start(self):
        self.data_dir.mkdir(parents=True, exist_ok=True)
        self.managed = Managed(self.name, self.args(), env=self.env).start()
        wait_until(
            f"{self.name} RPC ready",
            lambda: self.info(check=True),
            timeout=300,
            interval=0.5,
            quiet=True,
        )
        return self

    def stop(self):
        if self.managed:
            self.managed.stop(rpc_url=self.rpc_url)

    def info(self, check=False):
        result = rpc(self.rpc_url, "getChainInfo", raw_error=not check)
        if check and not isinstance(result, dict):
            raise ChaosError(f"getChainInfo not ready: {result}")
        return result

    def height(self):
        info = self.info()
        return int(info["height"])

    def tip(self):
        info = self.info()
        return int(info["height"]), info["best_hash"]


ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")


def strip_ansi(text):
    return ANSI_RE.sub("", text)


def grep_log(path, pattern, since_last_start=True):
    """Return matching log lines, optionally only after the last START marker."""
    text = strip_ansi(Path(path).read_text(errors="replace"))
    if since_last_start:
        index = text.rfind("===== START")
        if index >= 0:
            text = text[index:]
    matcher = re.compile(pattern)
    return [line for line in text.splitlines() if matcher.search(line)]


def append_measurement(kind, **fields):
    RUN.mkdir(parents=True, exist_ok=True)
    row = {"kind": kind, "wall": time.time(), "wall_iso": time.strftime("%F %T")}
    row.update(fields)
    with open(RUN / "measurements.jsonl", "a") as fh:
        fh.write(json.dumps(row) + "\n")
    return row


def load_state():
    path = RUN / "state.json"
    if not path.exists():
        return {}
    return json.loads(path.read_text())


def save_state(state):
    RUN.mkdir(parents=True, exist_ok=True)
    (RUN / "state.json").write_text(json.dumps(state, indent=2))


def pid_alive(name):
    """Return the recorded PID if run/<name>.pid exists and the process lives."""
    pid_file = RUN / f"{name}.pid"
    if not pid_file.exists():
        return None
    try:
        pid = int(pid_file.read_text().strip())
    except ValueError:
        return None
    try:
        os.kill(pid, 0)
        return pid
    except ProcessLookupError:
        return None


def stop_by_pidfile(name, rpc_url=None, timeout=25):
    """Stop a lane process by the PID recorded in ITS pid file, and only that PID.

    RPC `paranoid_stop` first (graceful), then SIGTERM/SIGKILL to the recorded
    PID if needed. No pattern-based process management anywhere.
    """
    pid_file = RUN / f"{name}.pid"
    if not pid_file.exists():
        print(f"[stop] {name}: no pid file; nothing to stop", flush=True)
        return False
    pid = int(pid_file.read_text().strip())
    print(f"[stop] {name} pid={pid} (from pid file)", flush=True)
    if rpc_url:
        try:
            rpc(rpc_url, "stop", timeout=3)
        except Exception:  # noqa: BLE001 - best-effort graceful stop
            pass
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pid_file.unlink(missing_ok=True)
            return True
        time.sleep(1)
    os.kill(pid, signal.SIGTERM)
    deadline = time.time() + 8
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
            time.sleep(0.5)
        except ProcessLookupError:
            pid_file.unlink(missing_ok=True)
            return True
    os.kill(pid, signal.SIGKILL)
    time.sleep(2)
    pid_file.unlink(missing_ok=True)
    return True


def stop_all(managed_list):
    """Stop-by-PID for every Managed/Node process we started."""
    for item in reversed(managed_list):
        try:
            item.stop()
        except Exception as exc:  # noqa: BLE001 - cleanup reports, never aborts
            name = getattr(item, "name", item)
            print(f"[cleanup] {name}: {exc}", flush=True)
