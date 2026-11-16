#!/usr/bin/env python3
"""Shared driver library for the n1-squat lane (slot-hint prediction squat).

Adapted from devnet-chaos/scripts/chaoslib.py; nodes run the shared
early-finance binary. Every process started here gets a PID file under run/ and
is stopped only by that PID (RPC `paranoid_stop` first, then signals to the
recorded PID). Loopback addresses only. No pattern-based process management
anywhere.
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
BIN = LANE.parent / "shared-early-finance" / "bin"
RUN = LANE / "run"
LOGS = LANE / "logs"

NODE_BIN = BIN / "parano1d"
CLI_BIN = BIN / "parano1d-cli"
WORKER_BIN = Path(
    "/Users/ember/dev/parano1d-break/experiments/devnet-chaos/bin/parano1d-miner"
)
PROBE_BIN = LANE / "probe" / "target" / "release" / "n1-probe"

# Distinct bearer tokens for the two credentialed roles.
MINING_KEY_N1 = "n1-squat-mining-n1-8ad3f04c1e9b"
OPERATOR_KEY_N1 = "n1-squat-operator-n1-5c7b91e2d4a6"


class ChaosError(Exception):
    pass


def rpc(url, method, params=None, timeout=240, bearer=None, raw_error=False):
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
    """A parano1d daemon."""

    def __init__(
        self,
        name,
        p2p_port,
        rpc_port,
        mode="node",
        genesis=False,
        seeds=(),
        mining_key=None,
        operator_key=None,
        log="debug",
    ):
        self.name = name
        self.p2p_port = p2p_port
        self.rpc_port = rpc_port
        self.mode = mode
        self.genesis = genesis
        self.seeds = list(seeds)
        self.mining_key = mining_key
        self.operator_key = operator_key
        self.log = log
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
            NODE_BIN,
            "--mode",
            self.mode,
            "--data-dir",
            str(self.data_dir),
            "--p2p-listen",
            f"127.0.0.1:{self.p2p_port}",
            "--rpc-listen",
            f"127.0.0.1:{self.rpc_port}",
            "--disable-dns-seeds",
            "--log",
            self.log,
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

    def start(self):
        self.data_dir.mkdir(parents=True, exist_ok=True)
        self.managed = Managed(self.name, self.args()).start()
        wait_until(
            f"{self.name} RPC ready",
            lambda: self.info(check=True),
            timeout=90,
            interval=0.5,
            quiet=True,
        )
        return self

    def stop(self):
        if self.managed:
            self.managed.stop(rpc_url=self.rpc_url)

    def raw_log(self):
        return self.managed.log_path.read_text(errors="replace")

    def log_lines_since(self, marker_text=None):
        text = self.raw_log()
        if marker_text:
            index = text.rfind(marker_text)
            text = text[index:] if index >= 0 else text
        return text.splitlines()

    def info(self, check=False, bearer=None):
        result = rpc(self.rpc_url, "getChainInfo", bearer=bearer, raw_error=not check)
        if check and not isinstance(result, dict):
            raise ChaosError(f"getChainInfo not ready: {result}")
        return result

    def height(self):
        return int(self.info()["height"])

    def tip(self):
        info = self.info()
        return int(info["height"]), info["best_hash"]

    def cli(self, args, timeout=180, check=True):
        cmd = [str(CLI_BIN), "--rpc", self.rpc_url] + [str(a) for a in args]
        proc = subprocess.run(
            cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout
        )
        if check and proc.returncode != 0:
            raise ChaosError(f"cli {args} failed: {proc.stdout}\n{proc.stderr}")
        return proc.stdout.strip(), proc.stderr.strip(), proc.returncode


def start_worker(name, node, threads=4, key=None):
    args = [WORKER_BIN, "--rpc", node.rpc_url, "--threads", str(threads)]
    if key:
        args.extend(["--key", key])
    return Managed(name, args, env={"RUST_LOG": "info"}).start()


def worker_running(name):
    pid_file = RUN / f"{name}.pid"
    if not pid_file.exists():
        return False
    try:
        os.kill(int(pid_file.read_text().strip()), 0)
        return True
    except (ProcessLookupError, ValueError):
        return False


def stop_worker(name):
    pid_file = RUN / f"{name}.pid"
    if pid_file.exists():
        pid = int(pid_file.read_text().strip())
        try:
            os.kill(pid, 0)
            os.kill(pid, signal.SIGTERM)
            time.sleep(2)
            try:
                os.kill(pid, 0)
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        except ProcessLookupError:
            pass
        pid_file.unlink()
        print(f"[stop] {name} stopped by pid {pid}", flush=True)


def stop_all(managed_list):
    """Stop-by-PID for every Managed/Node process we started."""
    for item in reversed(managed_list):
        try:
            item.stop()
        except Exception as exc:  # noqa: BLE001 - cleanup reports, never aborts
            name = getattr(item, "name", item)
            print(f"[cleanup] {name}: {exc}", flush=True)


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


def probe(rpc_url, *args, timeout=600, check=True):
    """Run the n1-probe binary against a node."""
    cmd = [str(PROBE_BIN), rpc_url, *[str(a) for a in args]]
    proc = subprocess.run(
        cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout,
        cwd=str(LANE),
    )
    out = (proc.stdout + proc.stderr).strip()
    if check and proc.returncode != 0:
        raise ChaosError(f"probe {args} failed: {out[:2000]}")
    return out
